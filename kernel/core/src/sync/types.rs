//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The sync subsystem's type definitions (seed-core-design §2c.3).
//!
//! Definitions only. Every `struct`, `enum` and type alias of the
//! subsystem lives here; every `impl` block lives in `hotpath.rs` beside
//! the functions that use it — an impl is code, a definition is data
//! layout. The exceptions are definitions local to code: a type declared
//! inside a function of `hotpath.rs`.
//!
//! No constant lives here: none of these definitions names one. The
//! array-length constants of the futex waiter pool, the futex hash and
//! the PI pools size statics, not type definitions, and stay beside
//! those statics in their sections of `hotpath.rs`. The two lock-key
//! traits (`NestKey`, `CanNestInside`) stay beside their impls in the
//! `lockkey` section of `hotpath.rs`: a trait is an interface, not a
//! data layout.
//!
//! This file holds no `unsafe`. A field that was private to its old file
//! is `pub(super)` here because the code that touches it lives in the
//! sibling module `hotpath`: the sync directory is the smallest
//! visibility boundary Rust offers for that split.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this file must stay Safe Rust.
#![forbid(unsafe_code)]

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicU64, AtomicUsize};

use crate::list::ListHead;
use crate::process::Task;
use crate::process::task::SchedPolicy;
use crate::process::wait::WaitQueueHead;

use super::hotpath::NestKey;


// ---------------------------------------------------------------------------
// the semaphore and the mutex built on it
// ---------------------------------------------------------------------------

/// Semaphore
///
/// A semaphore is a non-negative integer used for process synchronization:
/// - Initialized to some positive integer
/// - P operation (down): decrement value, if 0 then block and wait
/// - V operation (up): increment value, if processes are waiting then wake one
#[repr(C)]
pub struct Semaphore {
    /// Semaphore count value
    /// Use atomic integer to ensure thread safety
    pub(super) count: AtomicI32,
    /// Wait queue
    /// When semaphore is 0, waiting processes join this queue
    pub(super) wait: WaitQueueHead,
}

/// Mutex Semaphore (Mutex)
///
/// Binary semaphore, initial value is 1, used for mutual exclusion
///
/// # Example
/// ```no_run
/// # use kernel::sync::Mutex;
/// # fn test(mutex: &Mutex) {
/// mutex.lock();
/// // ... critical section ...
/// mutex.unlock();
/// # }
/// ```
#[repr(C)]
pub struct Mutex {
    /// Internal semaphore
    pub(super) sem: Semaphore,
}

/// Mutex Guard (RAII)
///
/// Automatically manages lock lifetime
///
/// # Example
/// ```no_run
/// # use kernel::sync::Mutex;
/// # fn test(mutex: &Mutex) {
/// {
///     let _guard = mutex.guard();
///     // ... critical section ...
/// } // Automatically release lock
/// # }
/// ```
pub struct MutexGuard<'a> {
    pub(super) mutex: &'a Mutex,
}

// ---------------------------------------------------------------------------
// the futex key, a queued waiter, a hash chain and the wake-verdict record
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct FuxWakeOp {
    pub j: u64,
    pub tid: u32,
    pub tgid: u32,
    pub uaddr: u64,
    pub op: u8, // raw FUTEX cmd byte (FUTEX_WAKE=1 etc.)
    pub ret: i32,
}

/// Futex key - uniquely identifies a futex
#[derive(Clone, Copy, Debug)]
pub struct FutexKey {
    /// Userspace address
    pub uaddr: usize,
    /// Address-space identity for private futexes (Linux: mm pointer).
    ///
    /// This is the Arc allocation address of the task's AddressSpace —
    /// shared by all CLONE_VM threads of one process, distinct across
    /// processes and across fork(). 0 for kernel threads / mm-less tasks.
    pub mm: usize,
    /// Flags
    pub flags: u32,
    /// PHYSICAL word identity — (page frame << 12) | byte offset —
    /// resolved from the caller's page tables at key construction.
    /// 0 when unresolved (kernel threads, torn-down exit paths, unit
    /// tests). Shared-futex equality is physical, not virtual: the
    /// reference kernel keys shared futexes on (inode, page, offset),
    /// so a wake from ANY mapping of the same word — including a fresh
    /// mmap at a DIFFERENT virtual address after fork+exec (LTP's
    /// tst_checkpoint protocol: parent waits on the MAP_SHARED results
    /// page, the exec'd child re-opens + re-mmaps and wakes) — reaches
    /// the waiter. Rux's MAP_SHARED file pages are single cache frames
    /// aliased in every mapper, so the frame number IS the identity.
    pub phys: usize,
    /// Object identity (FWK, reference FUT_OFF_MMSHARED parity): true
    /// only for SHARED-VMA words with a resolved phys — those key (and
    /// bucket) on `phys`, the stable shared object. Every anon word
    /// (PRIVATE VMA, any op flags) carries object=false and keys on
    /// (mm, uaddr): the word's frame is a COW transient (fork re-marks
    /// the PTE RO; the next store swaps the frame), so the frame can
    /// never be the identity of an anon futex.
    pub object: bool,
}

/// Waiter information
pub struct Waiter {
    /// Futex key
    pub key: FutexKey,
    /// Waiting task
    pub task: *mut Task,
    /// bitset
    pub bitset: u32,
    /// Whether already woken
    pub woken: bool,
    /// Jiffies when this waiter was linked into its bucket chain (the
    /// park stamp for the rux:fuxstuck (retired debug=fuxstuck) census; 0 on the alloc_waiter
    /// placeholder, overwritten at real insertion). NOT reset on requeue —
    /// the park episode continues, so the original stamp stays truthful.
    pub park_j: u64,
    /// Hash bucket this waiter is currently linked in. Tracked per-waiter
    /// because FUTEX_REQUEUE can move it to uaddr2's bucket while it sleeps;
    /// a stale bucket at removal time silently leaked the slot (review 6.1).
    pub bucket: usize,
    /// Next waiter in hash chain
    pub next: Option<usize>,
}

/// Waiter list head for each bucket. D-11 keyed (FutexBucketKey): the
/// guard's `proof()` is what the nested GRQ acquire in the wake path
/// requires — the bucket->GRQ nesting of INV-LOCK-5 (R12-2) is a
/// compile fact. NOTE: FUTEX_REQUEUE holds TWO buckets (lo/hi, index
/// ordered) via plain lock_irqsave — a same-class ordered double
/// acquire, deliberately outside the keyed-nesting API (which exists
/// to fence ACROSS classes).
///
/// 62nd (ohw27/ohw28): the chain is FIFO (tail append) — reference
/// plist parity. Linux's futex queues equal-priority waiters FIFO
/// (lib/plist.c plist_add walks FORWARD past equal priorities and
/// list_add_tails before the first higher-prio node), so the OLDEST
/// waiter receives the first wake of every burst. Our head-insert made
/// the chain LIFO: on a word with constant transient re-parkers (the
/// samgr FFRT worker family's contended inner mutex), every FUTEX_WAKE
/// served the newest transient and the one long-parked waiter — the
/// thread holding samgr's listenerMapLock_ inside its IPC handler —
/// starved for the whole contention window (ohw27: 33 ret=1 wakes by
/// four other threads, the parked owner's record woken=false to the
/// end). The map lock never released, the M2 waiter pandemic followed,
/// and the boot wedged at the appfwk gate 20+ boots in a row. With
/// FIFO, the parked holder gets the first wake and the wedge cannot
/// form. The tail pointer keeps append O(1) (no hot-path regression;
/// the requeue splice below becomes O(1) too).
#[derive(Clone, Copy)]
pub struct FutexChain {
    pub head: Option<usize>,
    pub tail: Option<usize>,
}

// ---------------------------------------------------------------------------
// the priority-inheritance waiter, the owner's saved scheduling configuration
// and the per-word PI state
// ---------------------------------------------------------------------------

/// One task queued on a PI futex. Allocated by the enqueue site (the
/// blocker itself for LOCK_PI, or cmp_requeue_pi for a requeued
/// waiter) and freed by whoever ends the park (the sleeper on every
/// exit path; the handoff only marks `handed`).
pub(super) struct PiWaiter {
    pub(super) task: *mut Task,
    /// Nice snapshot at enqueue time — the CFS ordering key (lower
    /// wins, FIFO among equals; the plain side uses the same rule).
    pub(super) nice: i32,
    /// RT priority snapshot at enqueue time: Some(p) for SCHED_FIFO /
    /// SCHED_RR waiters, None for CFS-class waiters. The RT-PI key
    /// (RT-2X item 7): RT waiters order before every CFS waiter and
    /// the top waiter's rt_priority is what an RT owner inherits.
    pub(super) rt_prio: Option<u32>,
    /// Ownership was transferred to this waiter (unlock_pi or the
    /// owner-died path): the user word already holds its TID; its park
    /// must end with success.
    pub(super) handed: bool,
    pub(super) next: Option<usize>,
}

/// The owner's pre-boost scheduling configuration, restored when the
/// boost ends (reference: rt_mutex_setprio's saved prio/policy).
#[derive(Clone, Copy)]
pub(super) struct SavedSched {
    pub(super) nice: i32,
    pub(super) policy: SchedPolicy,
    pub(super) rt_prio: u32,
}

/// Per-futex-word PI state: owner TID plus a priority-ordered waiter
/// chain.
pub(super) struct PiState {
    pub(super) key: FutexKey,
    pub(super) owner_pid: u32,
    /// Owner's pre-boost (nice, policy, rt_priority) while a boost is
    /// applied; None when the owner runs at its own configuration.
    /// RT-2X item 7: an RT boost changes policy/rt_priority too, so the
    /// saved triple replaces the old nice-only slot.
    pub(super) owner_saved: Option<SavedSched>,
    /// Priority-ordered waiter chain (RT first by rt_priority desc,
    /// then CFS by nice asc; FIFO among equals).
    pub(super) waiters: Option<usize>,
}

// ---------------------------------------------------------------------------
// the sleeping mutex, its state word, the optimistic-probe outcome and its guard
// ---------------------------------------------------------------------------

pub(super) struct SleepMutexState {
    pub(super) locked: bool,
    pub(super) owner: u32,
    pub(super) depth: u32,
    /// Jiffies stamp of the outermost acquire — with the waiters'
    /// tripwire below it names a holder that never releases (a leaked
    /// lock after a synchronous kill inside the critical section parks
    /// every later user in D-state forever).
    pub(super) acquired_at: u64,
    /// R84 fairness: tasks parked on `wait` PLUS tasks holding an
    /// unspent release() wake token. A task enters the count when it
    /// first registers on the queue and leaves it in the same critical
    /// section in which it takes the lock (token spend or sole-waiter
    /// take). While nonzero, the fair fast path refuses fresh sleepable
    /// acquirers.
    pub(super) waiters: u32,
    /// R85: orphaned-handoff epoch. release() hands the lock to ONE
    /// waiter via wake_up_one; when the queue is empty at that moment
    /// (every counted waiter is mid-loop between finish_wait and its
    /// next prepare_to_wait) the wake is dropped and the lock sits FREE
    /// behind a nonzero `waiters` with nobody left to wake it. Each such
    /// drop bumps this epoch: a waiter whose registration snapshot of the
    /// epoch predates the bump may take the free lock at its
    /// re-check, so an orphaned handoff is recovered by the FIRST
    /// mid-loop waiter to re-register.
    pub(super) handoff_epoch: u64,
    /// R6 (rt mode only): RT-class waiters currently parked with a PI
    /// entry registered (advisory trigger for the hi-priority handoff
    /// wake and the boost routing — the PI state's own waiter list is
    /// the authoritative set; drift here can only ever fall back to
    /// the plain FIFO wake, never corrupt state).
    pub(super) rt_waiters: u32,
}

/// Outcome of one optimistic_probe round (see optimistic_probe).
pub(super) enum OptimisticProbe {
    /// Acquired (free+unwaited fast path or recursion) — guard inside.
    Acquired(SleepMutexGuard),
    /// Holder is on-CPU: optimistic spinning is profitable.
    Spin,
    /// Holder off-CPU / handoff window: queue instead of spinning.
    Stop,
}

/// Sleeping mutex for coarse, I/O-holding serialization (see the module
/// doc). All users are `static`s, hence the `&'static self` API.
pub struct SleepMutex {
    /// The lock word + bookkeeping. Held only for a few instructions at
    /// a time (never across I/O) — a genuine spinlock per rule 1.
    pub(super) state: Spinlock<SleepMutexState>,
    /// Tasks blocked while another task holds the mutex (the holder may
    /// sleep in block I/O for a long time).
    pub(super) wait: WaitQueueHead,
    /// Diagnostic name — used verbatim in the stall/census prints
    /// (e.g. "EXT4-BIGLOCK-STALL"); keep stable, logs parse them.
    pub(super) name: &'static str,
    /// Whether fsw (fs-write census) notes fire on acquire/contend.
    /// True only for the ext4 big lock so its counters stay comparable
    /// across the perf gate; other instances do not pollute them.
    pub(super) fsw_notes: bool,
    /// R6 rt-mutex mode: RT-class waiters join the kernel PI chain
    /// (owner boost + hi-priority handoff) through the shared PI
    /// engine (kernel-keyed states in sync/futex_pi.rs). All-CFS
    /// contention is byte-for-byte the R84/R85 behavior — the mode
    /// only adds the boost and changes WHICH waiter a release wakes
    /// when an RT-class waiter is queued.
    pub(super) rt: bool,
}

/// Guard: releases on drop (outermost only).
pub struct SleepMutexGuard {
    pub(super) lock: &'static SleepMutex,
}

// ---------------------------------------------------------------------------
// the raw spinlock, the generic Spinlock<T> and its plain / irqsave / bottom-half
// guards, and the acquire-evidence stamp
// ---------------------------------------------------------------------------

/// HEAL-GATE (122nd, runq-strand follow-up): per-CPU acquire-side
/// evidence for the tick-side PREEMPT-HEAL family.
///
/// The runq-strand conviction (rs2): the heal's 200-consecutive-
/// observation bar cannot distinguish a LEAKED count from a
/// LEGITIMATE waiter inside a slow acquire — the rs2 waits were on
/// the stalled binder OWNER-LATCH chain (latch_age 2820ms), and the
/// 20.5s heal storm zeroed live waiters' counts mid-acquire
/// (was=0x10001/0x10003), manufacturing phantom counts and (pre
/// preemptible-TTAS) permanent CPU monopolists. This stamp is the
/// acquire-side half of the fix: a waiter inside the CONTENDED TTAS
/// loop publishes (its Task pointer, jiffies) on its own CPU's slot
/// once per SPIN_PREEMPT_PERIOD (64K) iterations — the same 64K
/// boundary the preempt-exit samples need_resched at, so the stamp
/// rides the existing heartbeat (~0.07-2ms native/KVM, well inside
/// one tick). The tick-side heal consults the slot via
/// [`acquire_evidence_fresh`] before zeroing anything: a count with
/// fresh acquire evidence is a LIVE waiter, not a leak, and must
/// never be healed.
///
/// Concurrency argument (all Relaxed): the only writer is the CPU's
/// current spinner inside lock()'s contended loop, and the only
/// reader is the tick IRQ on that SAME CPU — the tick interrupts the
/// loop, so the pair never runs concurrently and per-word coherence
/// is enough (same shape as the TICK_ENTRY_PC stamps). A coupled
/// spinner (preempt_count >= 1) cannot migrate or be preempted, so
/// the stamp's task identity is stable for the whole contended wait.
pub struct AcquireEvidence {
    /// Task pointer of the spinner that last stamped this CPU's slot
    /// (0 = never stamped). Written by the spinner, read by its own
    /// CPU's tick.
    pub task: AtomicUsize,
    /// Jiffies at the last 64K-boundary stamp. Age is only meaningful
    /// together with a matching `task`.
    pub jiffies: AtomicU64,
}

pub struct RawSpinlock {
    /// THE single authority for the lock's state: `0` = free,
    /// `cpu + 1` = held by `cpu` (grq-source-fix, 2026-11-19).
    ///
    /// The reference discipline (qspinlock's locked byte + tail cpu, and
    /// debug spinlocks' owner check) is that ONE word answers BOTH
    /// "is it held" and "by whom": the acquire is a single cmpxchg that
    /// publishes lock+owner together, and the release is a single
    /// ordered Release store of 0 that clears them together. An
    /// auxiliary "held" tracker written as a SECOND step (the round-6
    /// GRQ_HELD per-cpu bool) made the release interruptible between
    /// "tracker now says free" and "word still says held" — a fault in
    /// that window strands a word no tracker claims (the gl3 orphaned-
    /// word wedge: seven waiters, no holder, forever). With the owner
    /// IN the word there is no second authority that can disagree:
    /// any interruption before the release store leaves the word
    /// self-describing (owner = the interrupted cpu), and after it
    /// leaves nothing stranded. Ownership queries (`holder_cpu`,
    /// `held_by`) are DERIVED from this word and can never be stale
    /// relative to it.
    pub(super) locked: AtomicU32,
    /// Deadlock diagnostics: holder's hart id + 1 (0 = free).  Written
    /// AFTER acquiring / cleared BEFORE releasing, so a concurrent reader
    /// may briefly see a stale value — fine for the watchdog print, which
    /// only needs "who held it when everything wedged".
    #[cfg(feature = "debug-lock-owner")]
    pub(super) owner: AtomicUsize,
    /// oh-crash 91st (leak hunt): LAST-acquire forensics — pid + call
    /// site + jiffies of the most recent acquire. NOT cleared on unlock
    /// (they name the acquirer even after a leaked/dropped unlock), at
    /// one relaxed store each on the acquire path.
    #[cfg(feature = "debug-lock-owner")]
    pub(super) acq_pid: AtomicU32,
    #[cfg(feature = "debug-lock-owner")]
    pub(super) acq_site: AtomicUsize,
    #[cfg(feature = "debug-lock-owner")]
    pub(super) acq_jiffies: AtomicU64,
}

pub struct Spinlock<T: ?Sized> {
    pub(super) raw: RawSpinlock,
    pub(super) data: UnsafeCell<T>,
}

pub struct SpinlockGuard<'a, T: ?Sized> {
    pub(super) lock: &'a Spinlock<T>,
}

pub struct SpinlockIrqGuard<'a, T: ?Sized> {
    pub(super) lock: &'a Spinlock<T>,
    pub(super) flags: bool,
}

pub struct SpinlockBhGuard<'a, T: ?Sized> {
    pub(super) lock: &'a Spinlock<T>,
}

// ---------------------------------------------------------------------------
// the raw read-write spinlock, RwSpinlock<T> and its six guards
// ---------------------------------------------------------------------------

/// Raw read-write spinlock.
///
/// Bit layout of the `state` word:
/// - `[31]`    — writer held flag
/// - `[30]`    — writer pending flag (one waiting writer owns it)
/// - `[29:0]`  — active reader count
///
/// Writer fairness (reference qrwlock discipline, sync-realign followup):
/// a contended writer first registers WRITER_PENDING_BIT — readers still
/// holding the lock drain normally, but NEW readers block, so the writer
/// is handed the lock after at most one reader-generation instead of
/// competing forever against a cross-CPU reader stream.
pub struct RawRwSpinlock {
    pub(super) state: AtomicU32,
}

pub struct RwSpinlock<T: ?Sized> {
    pub(super) raw: RawRwSpinlock,
    pub(super) data: UnsafeCell<T>,
}

pub struct RwSpinlockReadGuard<'a, T: ?Sized> {
    pub(super) lock: &'a RwSpinlock<T>,
}

pub struct RwSpinlockWriteGuard<'a, T: ?Sized> {
    pub(super) lock: &'a RwSpinlock<T>,
}

pub struct RwSpinlockIrqReadGuard<'a, T: ?Sized> {
    pub(super) lock: &'a RwSpinlock<T>,
    pub(super) flags: bool,
}

pub struct RwSpinlockIrqWriteGuard<'a, T: ?Sized> {
    pub(super) lock: &'a RwSpinlock<T>,
    pub(super) flags: bool,
}

pub struct RwSpinlockBhReadGuard<'a, T: ?Sized> {
    pub(super) lock: &'a RwSpinlock<T>,
}

pub struct RwSpinlockBhWriteGuard<'a, T: ?Sized> {
    pub(super) lock: &'a RwSpinlock<T>,
}

// ---------------------------------------------------------------------------
// the sleepable read-write semaphore, a queued waiter, the owner-word
// classification, the wake-type dispatch, Rwsem<T> and its two guards
// ---------------------------------------------------------------------------

/// One queued waiter (reference `struct rwsem_waiter`, rwsem.c:338).
pub(super) struct RwsemWaiter {
    pub(super) task: *mut crate::process::Task,
    pub(super) is_writer: bool,
    /// Jiffies deadline after which this waiter may demand HANDOFF.
    pub(super) timeout: u64,
    /// HANDOFF latch. Written by whichever try_write_lock sets the
    /// count HANDOFF bit while this waiter is first (rwsem.c:654).
    pub(super) handoff_set: bool,
}

/// Owner-word classification (rwsem.c:679-684).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum OwnerState {
    Null,
    Writer,
    Reader,
    Nonspinnable,
}

/// Wake-type dispatch for mark_wake (rwsem.c:348-352).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum WakeType {
    Any,
    Readers,
    ReadOwned,
}

/// Sleepable read-write semaphore. See the module doc for the full
/// reference mapping. Context: task/syscall only for down_*; try_*
/// are context-free.
pub struct RwSemaphore {
    pub(super) count: AtomicI64,
    pub(super) owner: AtomicUsize,
    /// Wait list. A genuine spinlock held for instructions at a
    /// time, never across schedule() and never across a wake (all
    /// three HANDOFF mutation sites hold it — rwsem.c:108-115).
    pub(super) wait_lock: Spinlock<alloc::vec::Vec<RwsemWaiter>>,
    /// Diagnostic name (stall prints / census lines).
    pub(super) name: &'static str,
}

/// Data-carrying sleepable rwlock (the `RwSpinlock<T>` shape on the
/// sleepable primitive). `read()`/`write()` return RAII guards; a
/// held section is PREEMPTIBLE and the holder may sleep below it
/// (never take this lock from interrupt context; never take it while
/// holding a spinlock).
pub struct Rwsem<T: ?Sized> {
    pub(super) raw: RwSemaphore,
    pub(super) data: UnsafeCell<T>,
}

pub struct RwsemReadGuard<'a, T: ?Sized> {
    pub(super) lock: &'a Rwsem<T>,
}

pub struct RwsemWriteGuard<'a, T: ?Sized> {
    pub(super) lock: &'a Rwsem<T>,
}

// ---------------------------------------------------------------------------
// the raw sequence lock, SeqLock<T> and its write guard
// ---------------------------------------------------------------------------

/// Raw sequence lock backend.
///
/// The sequence counter uses the low bit as a "writer active" flag:
///   - Even value  => no writer active, data is consistent
///   - Odd value   => writer active, data may be inconsistent
pub struct RawSeqLock {
    pub(super) sequence: AtomicUsize,
}

/// A sequence lock protecting data of type `T`.
///
/// `T` must be `Copy` because readers snapshot the entire value without
/// holding any lock.
pub struct SeqLock<T: Copy> {
    pub(super) raw: RawSeqLock,
    pub(super) data: UnsafeCell<T>,
}

/// RAII guard for an active SeqLock writer.
///
/// Provides `Deref` and `DerefMut` to the inner data.
/// On drop: releases the writer slot and re-enables preemption.
pub struct SeqLockWriteGuard<'a, T: Copy> {
    pub(super) lock: &'a SeqLock<T>,
    /// R21-N5: saved IRQ state for exact restore.
    pub(super) irq_state: bool,
}

// ---------------------------------------------------------------------------
// the callback type and the embedded callback head
// ---------------------------------------------------------------------------

/// RCU callback function type.
/// `data` points to the `RcuHead` that was passed to `call_rcu`.
pub type RcuCallback = fn(*mut core::ffi::c_void);

/// Embedded RCU callback head.
///
/// Place at the end of a struct to enable deferred reclamation via `call_rcu`.
/// The callback receives the `RcuHead` pointer; use `ListHead::entry()` or
/// `container_of!` to recover the enclosing struct.
#[repr(C)]
pub struct RcuHead {
    /// Whether this head has been initialized (list.init() called).
    pub(super) initialized: bool,
    /// Intrusive list link for per-CPU callback queue.
    pub list: ListHead,
    /// Callback invoked after grace period.
    pub func: RcuCallback,
}

// ---------------------------------------------------------------------------
// one cpu's row, the per-CPU bank and the handle to a row
// ---------------------------------------------------------------------------

/// One cpu's slot, padded to a full cache line so adjacent rows never
/// share one (the allocator rows are written under IF=0 on every
/// fast-path op — line splitting between CPUs is measurable traffic).
#[repr(C, align(64))]
pub(super) struct Row<T> {
    pub(super) val: T,
    // Plain bool, not AtomicBool: the flag is only ever read/written
    // by the cpu that owns the row, under the same IF=0 exclusivity
    // that guards `val` (an atomic would buy nothing and forfeit the
    // Copy bound the const row initializer needs). Stores go through
    // the raw pointer in the guard, never through a shared reference.
    pub(super) in_use: bool,
}

/// A per-CPU bank of `T`: `CpuLocal<T, N>` holds one `T` per cpu for
/// machines of at most `N` cpus. Build it as a `static` with
/// [`CpuLocal::new`]; reach rows only through [`CpuLocal::with`].
///
/// `Sync` is sound because every access path runs with interrupts
/// disabled on the acting cpu, so a given row is only ever touched by
/// the cpu that owns it, and the re-entry tripwire refuses a second
/// live guard on the same row.
///
/// D-7 incident fix (2026-10-27, run/pa68-d7-rootcause.txt): the rows
/// sit inside [`UnsafeCell`]. The first landing used a plain
/// `[Row<T>; N]`, and `unsafe impl Sync` does NOT confer interior
/// mutability — so the `static MAGAZINES: CpuLocal<...>` in
/// small_alloc was a non-mut static without interior mutability, which
/// rustc emits as an LLVM `constant` global (.rodata). The kernel maps
/// the image P|RW, so debug builds booted by luck; under opt>=1 LLVM
/// deleted every store into the bank (stores to constant memory) and
/// hoisted the loads, the magazine refill loop never terminated, and
/// every release-profile boot died at the first c64 miss. UnsafeCell
/// is the piece that makes the mutation LEGITIMATE to the compiler
/// instead of invisible to it: the static now lands in writable
/// memory and the stores survive at every opt level. The release-strong
/// boot is the standing verification for this family (debug-only
/// testing cannot see it).
pub struct CpuLocal<T, const N: usize> {
    pub(super) rows: UnsafeCell<[Row<T>; N]>,
}

/// The only handle to a per-CPU row. Exists solely inside a
/// [`CpuLocal::with`] or [`CpuLocal::with_preempt`] closure: it cannot
/// be sent to another cpu (`!Send` via the raw-pointer marker) and
/// cannot outlive the closure, so the exclusivity window it depends on
/// is its own lifetime.
pub struct CpuLocalRef<'a, T> {
    pub(super) val: &'a mut T,
    pub(super) cpu: usize,
    /// Saved IRQ flags (plain `with` guards only).
    pub(super) flags: bool,
    /// Preempt guards restore the count instead of the IRQ flags.
    pub(super) preempt_guard: bool,
    pub(super) in_use: *mut bool,
    pub(super) _nosend: PhantomData<*mut ()>,
}

// ---------------------------------------------------------------------------
// the five lock-order keys and the keyed locks, proof and guards that take them
// ---------------------------------------------------------------------------

/// The global run queue lock (divergence D-1). INV-LOCK-5: nests INSIDE
/// the futex bucket lock, never the reverse; same-level nesting is a
/// self-deadlock and has no impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrqKey;

/// A futex hash-bucket lock (`sync::futex::HASH_HEADS`). Nothing may be
/// acquired inside it EXCEPT the classes with explicit impls above —
/// today only `GrqKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FutexBucketKey;

/// Per-socket queue statistics (INV-LOCK-6): stats nest inside the
/// virtio-net rx/tx queue locks, never the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetStatsKey;

/// A virtio-net rx/tx queue lock (INV-LOCK-6 outer class).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetQueueKey;

/// A futex waiter-pool SLOT lock (`sync::futex::WAITER_POOL`). Slot
/// locks are taken inside the bucket lock that owns the chain the slot
/// is linked into (the wait/wake/exit/requeue paths); the allocator
/// and the free path take them top-level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaiterSlotKey;

/// A [`RawSpinlock`] whose position in the lock hierarchy is part of
/// its type. Same size as the raw lock (`PhantomData<K>` is free), the
/// acquire paths inline to the same instructions; the ONLY new API is
/// `lock_nested`, the compile-checked nesting acquisition.
pub struct KeyedRawSpinlock<K: NestKey> {
    pub(super) raw: RawSpinlock,
    pub(super) _key: PhantomData<K>,
}

/// Zero-sized PROOF that a keyed lock of class `K` is currently held
/// by THIS thread. MINTED ONLY by a live keyed guard's `proof()`
/// (no public constructor); consumed by `lock_nested` /
/// `lock_irqsave_nested`, which only compile when the nesting is
/// permitted. The lifetime ties the proof to the guard that minted
/// it, so it cannot outlive the held section.
pub struct NestProof<'a, K: NestKey> {
    pub(super) _key: PhantomData<K>,
    pub(super) _hold: PhantomData<&'a ()>,
}

/// A [`Spinlock<T>`](super::types::Spinlock) whose position in the
/// lock hierarchy is part of its type — the data-carrying twin of
/// [`KeyedRawSpinlock`]. `lock_irqsave` returns the same protocol
/// guard wrapped with the key (call sites unchanged: Deref/DerefMut to
/// the data); the ONLY new capability is `guard.proof()`, which mints
/// the [`NestProof`] other keyed locks require for a nested acquire.
pub struct KeyedSpinlock<K: NestKey, T> {
    pub(super) inner: Spinlock<T>,
    pub(super) _key: PhantomData<K>,
}

/// Guard for the data-carrying keyed spinlock: derefs to `T`; mints
/// [`NestProof`] for nested acquisitions while held.
#[must_use = "a discarded keyed-lock guard releases the lock immediately (the 2b092c49 conviction)"]
pub struct KeyedIrqGuard<'a, K: NestKey, T: ?Sized> {
    pub(super) inner: SpinlockIrqGuard<'a, T>,
    pub(super) _key: PhantomData<K>,
}

/// Guard for a keyed lock — unlocks on drop (raw lock only; the
/// irqsave/preempt layers wrap it from the outside, exactly as
/// `GrqGuard` wraps `RawSpinlock` today).
///
/// `#[must_use]` is LOAD-BEARING (the 2b092c49 conviction): the GRQ
/// adoption's pre-existing `self.lock.lock();` statement — written for
/// `RawSpinlock::lock() -> ()` — silently DISCARDED this guard under
/// the wrapper, whose Drop released the lock at the end of the
/// statement. Every "GRQ-locked" section then ran unexcluded
/// (x86: 6/6 CFS-timeline BTreeMap corruption; riscv64: R74
/// double-run). A discarded keyed guard is now a compile warning; a
/// subsystem wrapping its own guard protocol must use
/// `lock_claim`/`try_lock_claim`/`unlock`, never the guard-returning
/// `lock()`.
#[must_use = "a discarded keyed-lock guard releases the lock immediately (the 2b092c49 conviction)"]
pub struct KeyedGuard<'a, K: NestKey> {
    pub(super) lock: *mut KeyedRawSpinlock<K>,
    pub(super) _key: PhantomData<K>,
    pub(super) _lifetime: PhantomData<&'a ()>,
}
