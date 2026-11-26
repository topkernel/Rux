//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The sync subsystem's code (seed-core-design §2c.3): everything of the
//! subsystem that is neither `unsafe.rs` nor `types.rs`, in one file.
//!
//! Sections follow the order of the old module declarations; each banner
//! names the topic file it was (`futex` was futex.rs) and the file's
//! module doc follows it as the section's header comment. Inside a
//! section the items keep their original order and text. Type definitions
//! are in `types.rs`, the audited unsafe edges in `unsafe.rs`.
//!
//! semaphore.rs carried `#![forbid(unsafe_code)]`. That attribute cannot
//! stand at the top of a file that also holds the lock primitives, so it
//! is dropped here; it comes back in pass 3, when the scattered unsafe is
//! collected into `unsafe.rs`.
//!
//! The file's NAME overstates until the shrink stage has run: it holds all
//! of the subsystem's code under the hot-path name, not a verified set of
//! hot-path functions (§2c.3, user decision 2026-11-25).

use super::types::{
    Semaphore,
    Mutex,
    MutexGuard,
    FuxWakeOp,
    FutexKey,
    Waiter,
    FutexChain,
    PiWaiter,
    SavedSched,
    PiState,
    SleepMutexState,
    OptimisticProbe,
    SleepMutex,
    SleepMutexGuard,
    AcquireEvidence,
    RawSpinlock,
    Spinlock,
    SpinlockGuard,
    SpinlockIrqGuard,
    SpinlockBhGuard,
    RawRwSpinlock,
    RwSpinlock,
    RwSpinlockReadGuard,
    RwSpinlockWriteGuard,
    RwSpinlockIrqReadGuard,
    RwSpinlockIrqWriteGuard,
    RwSpinlockBhReadGuard,
    RwSpinlockBhWriteGuard,
    RwsemWaiter,
    OwnerState,
    WakeType,
    RwSemaphore,
    Rwsem,
    RwsemReadGuard,
    RwsemWriteGuard,
    RawSeqLock,
    SeqLock,
    SeqLockWriteGuard,
    RcuHead,
    Row,
    CpuLocal,
    CpuLocalRef,
    GrqKey,
    FutexBucketKey,
    NetStatsKey,
    NetQueueKey,
    WaiterSlotKey,
    KeyedRawSpinlock,
    NestProof,
    KeyedSpinlock,
    KeyedIrqGuard,
    KeyedGuard,
};

use core::sync::atomic::{
    AtomicBool,
    AtomicI32,
    AtomicI64,
    AtomicU32,
    AtomicU64,
    AtomicUsize,
    Ordering,
    Ordering as Relaxed2,
};
use crate::process::types::WaitQueueHead;
use crate::process::types::Task;
use crate::process::types::{SchedPolicy, TaskState};
use crate::errno::constants::{EAGAIN, EFAULT, EINVAL, ENOSYS, ETIMEDOUT};
use crate::arch::cpu;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::fmt;
use crate::config::MAX_CPUS;
use crate::list::ListHead;
use core::marker::PhantomData;

// ═══════════════ sync root ═════════════════════════════════════════
// (was mod.rs: its re-exports of the topic modules are gone, every path names
//  hotpath or types now; the comment line below is all that remains of it)
// Synchronization Primitives
//
// Contains:
// - Semaphore
// - Condvar (Condition Variable)
// - Futex (Fast Userspace Mutex)
// - Spinlock / RwSpinlock

// The futex cold diagnostic seam (SyncDiagHooks) is crate::api::sync.

// ═══════════════ semaphore ═════════════════════════════════════════
// (was semaphore.rs)
// Semaphore Mechanism
//
// Core concepts:
// - Semaphores are used for process synchronization and mutual exclusion
// - P operation (down/down_interruptible): acquire semaphore, may block
// - V operation (up): release semaphore, wake up waiting processes

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.

/// R77 (semaphore CAS discipline): try to take one unit from `count` —
/// succeeds only by atomically swapping a POSITIVE value to value-1. A
/// failed attempt leaves the count untouched (no reservation, no restore),
/// so the count is always exactly "units available", never a mix of
/// availability and outstanding waiter reservations.
fn cas_take(count: &AtomicI32) -> bool {
    let mut c = count.load(Ordering::Acquire);
    while c > 0 {
        match count.compare_exchange_weak(
            c,
            c - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(v) => c = v,
        }
    }
    false
}

impl Semaphore {
    /// Create a new semaphore
    ///
    /// # Arguments
    /// * `value` - Initial value
    ///
    /// # Example
    /// ```
    /// // Mutex semaphore (binary semaphore)
    /// let mutex = Semaphore::new(1);
    ///
    /// // Counting semaphore (resource pool)
    /// let pool = Semaphore::new(10);
    /// ```
    pub const fn new(value: i32) -> Self {
        Self {
            count: AtomicI32::new(value),
            wait: WaitQueueHead::new(),
        }
    }

    /// Initialize semaphore (runtime initialization)
    ///
    /// # Arguments
    /// * `value` - Initial value
    pub fn init(&self, value: i32) {
        self.count.store(value, Ordering::Release);
        // WaitQueueHead is already automatically initialized
    }

    /// P operation (non-interruptible)
    ///
    /// Also called down operation or wait operation
    ///
    /// # Behavior
    /// - Decrement semaphore value by 1
    /// - If value >= 0, return immediately
    /// - If value < 0, block and wait until value becomes positive
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) {
    /// sem.down();  // Acquire semaphore
    /// // ... critical section ...
    /// sem.up();    // Release semaphore
    /// # }
    /// ```
    pub fn down(&self) {
        // R77 (GNOME kill-storm wedge/corruption family — rewrite of the
        // reservation protocol): the old down() reserved with an
        // unconditional fetch_sub and, after schedule() returned, TRUSTED
        // that the wake was an up() handoff. But signal_wake_up
        // force-wakes UNINTERRUPTIBLE sleepers (kill -9 storms wake
        // D-state waiters — the gsd teardown churn), so a signal-woken
        // waiter returned WITHOUT owning the semaphore while its -1
        // reservation stayed queued forever: two tasks walked into one
        // critical section (the copy_user wild-pointer panics at
        // guest+360s/+960s) and the poisoned count made every later up()
        // pair against a ghost. The wake source cannot be distinguished
        // from inside down(), so stop reserving: take a slot only via
        // CAS on a POSITIVE count (no failed-attempt side effects), and
        // after ANY wake simply retry. A spurious or signal wake now
        // costs one extra prepare/sleep round instead of the mutex
        // invariant. The R7-B6b lost-wake window stays closed by the
        // post-registration recheck; R8-6's exclusive/FIFO registration
        // discipline is unchanged.
        let current = match crate::sched::hotpath::current() {
            Some(task) => task,
            None => {
                // No current task (early boot / IRQ context): fall back to
                // the unconditional decrement — callers in this context
                // must guarantee the count is available.
                self.count.fetch_sub(1, Ordering::Acquire);
                return;
            }
        };

        loop {
            // Fast path: take a slot if one is visible.
            if cas_take(&self.count) {
                return;
            }

            // Slow path: register EXCLUSIVE (tail insert, FIFO) + mark
            // UNINTERRUPTIBLE atomically under the waitqueue lock.
            self.wait.prepare_to_wait(current, true, false);

            // Recheck after registering (R7-B6b): an up() between the
            // failed CAS and the registration must find us queued.
            if cas_take(&self.count) {
                self.wait.finish_wait(current);
                // finish_wait restored RUNNING; a concurrent up() may ALSO
                // have seen us queued+sleeping and enqueued us (NEW-C2
                // family) — undo that (per-class on_rq guards make it a
                // no-op otherwise).
                crate::sched::hotpath::dequeue_task(&*current);
                return;
            }

            crate::arch::cpu::restore_irq(true);
            crate::sched::hotpath::schedule();
            self.wait.finish_wait(current);
            // R77: loop and retry — do NOT trust the wake source.
        }
    }

    /// P operation (interruptible)
    ///
    /// Also called down_interruptible operation
    ///
    /// # Behavior
    /// - Decrement semaphore value by 1
    /// - If value >= 0, return Ok(()) immediately
    /// - If value < 0, block and wait until value becomes positive or interrupted by signal
    ///
    /// # Returns
    /// - `Ok(())` - Successfully acquired semaphore
    /// - `Err(())` - Interrupted by signal
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) -> Result<(), ()> {
    /// match sem.down_interruptible() {
    ///     Ok(()) => {
    ///         // Successfully acquired semaphore
    ///         // ... critical section ...
    ///         sem.up();
    ///     }
    ///     Err(()) => {
    ///         // Interrupted by signal
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn down_interruptible(&self) -> Result<(), ()> {
        // R77: same CAS rewrite as down() — a returned-from-schedule wake
        // is not proof of an up() handoff (spurious wakes, signal wakes),
        // so retry the acquire instead of returning Ok(()) unowned. The
        // signal path keeps its exact semantics: pending signal after a
        // wake (or before sleeping) returns Err with NO count side
        // effects (there is no reservation to roll back anymore).
        let current = match crate::sched::hotpath::current() {
            Some(task) => task,
            None => {
                // No current task (early boot / IRQ context): cannot block.
                // Do not touch the count — report failure to the caller.
                return Err(());
            }
        };

        loop {
            // Fast path: take a slot if one is visible.
            if cas_take(&self.count) {
                return Ok(());
            }

            // Register + mark INTERRUPTIBLE (atomically under the waitqueue
            // lock). EXCLUSIVE (tail insert, FIFO), matching down().
            self.wait.prepare_to_wait(current, true, true);

            // Recheck after registering (R32-F10 / R7-B6b lost-wake
            // discipline: any up() that pairs with this attempt must find
            // us queued).
            if cas_take(&self.count) {
                self.wait.finish_wait(current);
                crate::sched::hotpath::dequeue_task(&*current);
                return Ok(());
            }

            // A signal that arrived before we sleep must interrupt
            // immediately (the caller maps Err to EINTR).
            if crate::signal::signal_pending() {
                self.wait.finish_wait(current);
                // R9-17: NEW-C2 discipline for the signal path too — undo a
                // concurrent wake enqueue before returning.
                crate::sched::hotpath::dequeue_task(&*current);
                return Err(());
            }

            crate::arch::cpu::restore_irq(true);
            crate::sched::hotpath::schedule();
            self.wait.finish_wait(current);

            // Interrupted by a signal rather than up(): report EINTR to
            // the caller. No count side effects to undo (R77: the CAS
            // discipline took nothing); if an up() raced the signal its +1
            // stays in the count for the next down — no token is lost.
            if crate::signal::signal_pending() {
                crate::sched::hotpath::dequeue_task(&*current);
                return Err(());
            }
            // R77: otherwise loop and retry the acquire — the wake alone is
            // not proof an up() handed us the slot.
        }
    }

    /// Try P operation (non-blocking)
    ///
    /// Also called try_down or down_trylock operation
    ///
    /// # Behavior
    /// - Decrement semaphore value by 1
    /// - If value >= 0, return Ok(())
    /// - If value < 0, immediately return Err(()), does not block
    ///
    /// # Returns
    /// - `Ok(())` - Successfully acquired semaphore
    /// - `Err(())` - Semaphore not available
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) -> Result<(), ()> {
    /// match sem.down_trylock() {
    ///     Ok(()) => {
    ///         // Successfully acquired semaphore
    ///         // ... critical section ...
    ///         sem.up();
    ///     }
    ///     Err(()) => {
    ///         // Semaphore not available
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn down_trylock(&self) -> Result<(), ()> {
        // R77: pure CAS take — the old fetch_sub/fetch_add restore pair
        // briefly published a negative count that a concurrent up() could
        // misread as a waiter reservation.
        if cas_take(&self.count) {
            Ok(())
        } else {
            Err(())
        }
    }

    /// V operation (release semaphore)
    ///
    /// Also called up operation or signal operation
    ///
    /// # Behavior
    /// - Increment semaphore value by 1
    /// - If processes are waiting, wake one process
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) {
    /// sem.down();
    /// // ... critical section ...
    /// sem.up();  // Release semaphore, wake waiting process
    /// # }
    /// ```
    pub fn up(&self) {
        // R77: with the CAS discipline the count never goes negative on a
        // FAILED down() (no reservations), so "old < 0" can no longer
        // serve as the waiters-exist test. Publish the slot and always
        // attempt one exclusive wake; waking an empty (or all-woken)
        // queue is a cheap no-op scan under the queue lock. If a fast-path
        // down() steals the slot before the woken waiter runs, the waiter
        // simply re-sleeps — no token is lost.
        self.count.fetch_add(1, Ordering::Release);
        self.wait.wake_up_one();
    }

    /// Get current semaphore value
    ///
    /// # Returns
    /// Current semaphore value
    ///
    /// # Note
    /// This value is for reference only, actual value may change immediately after call
    pub fn count(&self) -> i32 {
        self.count.load(Ordering::Acquire)
    }
}

impl Mutex {
    /// Create a new mutex
    ///
    /// # Example
    /// ```
    /// let mutex = Mutex::new();
    /// ```
    pub const fn new() -> Self {
        Self {
            sem: Semaphore::new(1),
        }
    }

    /// Acquire lock
    ///
    /// If lock is already held, block and wait
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
    pub fn lock(&self) {
        self.sem.down();
    }

    /// Try to acquire lock (non-blocking)
    ///
    /// # Returns
    /// - `Ok(())` - Successfully acquired lock
    /// - `Err(())` - Lock is already held
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Mutex;
    /// # fn test(mutex: &Mutex) -> Result<(), ()> {
    /// match mutex.try_lock() {
    ///     Ok(()) => {
    ///         // Successfully acquired lock
    ///         // ... critical section ...
    ///         mutex.unlock();
    ///     }
    ///     Err(()) => {
    ///         // Lock is already held
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_lock(&self) -> Result<(), ()> {
        self.sem.down_trylock()
    }

    /// Release lock
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
    pub fn unlock(&self) {
        self.sem.up();
    }
}

impl<'a> MutexGuard<'a> {
    /// Create lock guard
    ///
    /// # Arguments
    /// * `mutex` - Associated mutex
    pub fn new(mutex: &'a Mutex) -> Self {
        mutex.lock();
        Self { mutex }
    }
}

impl<'a> Drop for MutexGuard<'a> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

impl Mutex {
    /// Get lock guard (RAII)
    ///
    /// Automatically manages lock lifetime, releases lock when guard goes out of scope
    ///
    /// # Returns
    /// MutexGuard guard object
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Mutex;
    /// # fn test(mutex: &Mutex) {
    /// {
    ///     let _guard = mutex.guard();
    /// // ... critical section ...
    /// } // Automatically release lock
    /// # }
    /// ```
    pub fn guard(&self) -> MutexGuard<'_> {
        MutexGuard::new(self)
    }
}

// ═══════════════ futex ═════════════════════════════════════════
// (was futex.rs)
// Futex Implementation - Fast Userspace Mutex
//
// # Design
// - Static waiter pool (spinlock-protected slots) for zero-allocation futex
// - Static hash table mapping FutexKey → waiter chain (singly-linked list)
// - All locks use `lock_irqsave()` for interrupt safety
// - Wake uses `Task::wake_up()` (enqueue + resched) for correct scheduling
// - Wait inserts into chain then sets INTERRUPTIBLE under lock to prevent lost wakeup
//
// # Futex key semantics (Linux parity — get_futex_key, FWK round)
// - PRIVATE futexes key on (mm identity, uaddr): the second key component is
//   the address of the task's `AddressSpace` Arc allocation. All CLONE_VM
//   threads share one Arc → one key; a forked child COWs a NEW AddressSpace
//   → different key (exactly Linux's `&mm->mm` key). Kernel threads (no mm)
//   key on 0.
// - SHARED-SEMANTIC ops on an ANON word (a PRIVATE VMA — .data, heap,
//   thread stacks) key on (mm identity, uaddr) as well: reference
//   `get_futex_key` keys PageAnon words on (current->mm, address)
//   (FUT_OFF_MMSHARED), never on the physical frame. A frame-keyed anon
//   word LOSES wakes to fork-COW: fork marks the parent PTE RO+COW, the
//   next store COW-breaks the page to a fresh frame, and a waiter parked
//   keyed on the old frame sits in a bucket the waker (resolving the new
//   frame) never scans — the lostwake.c group-strand (FWK: same-mm
//   wake-miss, waiter phys 0x7ffc7xxx vs waker phys changing every kill
//   cycle, 8/8 parkers stuck with a live re-waking waker and zero
//   scheduler diagnostics).
// - SHARED-SEMANTIC ops on a SHARED VMA word (MAP_SHARED — the genuine
//   cross-process case) key on the PHYSICAL word identity
//   (frame << 12 | offset). Those frames are stable: fork's COW walk
//   exempts SHARED VMAs (mm_ops.rs cow_exempt), so the frame IS the
//   shared object (reference: inode/page-index key; Rux's MAP_SHARED
//   pages are single cache frames aliased in every mapper).

/// FUTEX opcodes
pub const FUTEX_WAIT: i32 = 0;
pub const FUTEX_WAKE: i32 = 1;
pub const FUTEX_FD: i32 = 2;
pub const FUTEX_REQUEUE: i32 = 3;
pub const FUTEX_CMP_REQUEUE: i32 = 4;
pub const FUTEX_WAKE_OP: i32 = 5;
pub const FUTEX_LOCK_PI: i32 = 6;
pub const FUTEX_UNLOCK_PI: i32 = 7;
pub const FUTEX_TRYLOCK_PI: i32 = 8;
pub const FUTEX_WAIT_BITSET: i32 = 9;
pub const FUTEX_WAKE_BITSET: i32 = 10;
pub const FUTEX_WAIT_REQUEUE_PI: i32 = 11;
pub const FUTEX_CMP_REQUEUE_PI: i32 = 12;
pub const FUTEX_LOCK_PI2: i32 = 13;

pub const FUTEX_PRIVATE_FLAG: i32 = 128;
pub const FUTEX_CLOCK_REALTIME: i32 = 256;
pub const FUTEX_CMD_MASK: i32 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

// sc184 (session R, screen gate): devmgr frozen-rwlock birth tracer state
// (see the FXDV block in do_futex). Latched from the binder SAREG tracer
// when hdf_devmgr's AddSystemAbility(SA 5100) is seen.
pub static FXDV_PID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
static FXDV_N: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
const FXDV_RINGSZ: usize = 24;
static FXDV_ADDR: [core::sync::atomic::AtomicUsize; FXDV_RINGSZ] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; FXDV_RINGSZ];
static FXDV_CNT: [core::sync::atomic::AtomicUsize; FXDV_RINGSZ] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; FXDV_RINGSZ];

pub static FXRS_TGID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static FXRS_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// 51st session: census-fed word watch list. When the FUX census prints a
/// stuck group, its uaddr lands here and the FXRS tracer follows every
/// futex op on that word from ANY process — the WAIT/WAKE lifecycle of
/// the frozen lock decides "holder never releases" (no WAKE ever) from
/// "kernel lost the wake" (WAKEs seen, waiter still parked).
pub static FXRS_WORDS: [core::sync::atomic::AtomicUsize; 16] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 16];

pub fn fxrs_word_add(uaddr: usize) {
    if uaddr == 0 {
        return;
    }
    for w in FXRS_WORDS.iter() {
        let cur = w.load(core::sync::atomic::Ordering::Relaxed);
        if cur == uaddr {
            return;
        }
        if cur == 0
            && w.compare_exchange(
                0,
                uaddr,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            return;
        }
    }
}

#[inline]
pub fn fxrs_word_watched(uaddr: usize) -> bool {
    for w in FXRS_WORDS.iter() {
        if w.load(core::sync::atomic::Ordering::Relaxed) == uaddr {
            return true;
        }
    }
    false
}

// sc58 (58th session): the wake-side verdict ring. Every wedge census so
// far shows the same shape on the stuck words — anywoken=0 and (FXRS)
// total futex-syscall silence AFTER the word went stuck — but FXRS only
// arms once the census prints a word (park age >= 3 s), so the decisive
// moment, the park's BIRTH, is unobserved: the wake that was lost (or
// never sent) happened in that gap. This ring records every WAKE-class
// futex syscall (WAKE / WAKE_BITSET / WAKE_OP / CMP_REQUEUE) system-wide
// from t=0 with its return value; the stuck census then dumps the window
// around the OLDEST stuck parks' birth (park_j-2s .. park_j+5s):
//   - a WAKE on the word with ret>=1 while the waiter never returned
//     names a kernel wake drop (delivery bug);
//   - a WAKE with ret==0 while the waiter WAS linked at that jiffy
//     names a wake-scan miss;
//   - no WAKE near the birth at all names the never-sent branch
//     (userspace ordering — reference-diff territory).
// rux:fuxstuck (retired debug=fuxstuck)-gated push (one lock+copy per wake op); zero cost off.
pub const FUXW_RING_SZ: usize = 16384; // per CPU: ~6 s of coverage at the OH boot's ~10k wake syscalls/s
const FUXW_NRINGS: usize = 4; // cpu_id % 4 — collision-free on <= 4-CPU boots
const FUXW_ZERO: FuxWakeOp = FuxWakeOp {
    j: 0,
    tid: 0,
    tgid: 0,
    uaddr: 0,
    op: 0,
    ret: 0,
};
pub static FUXW_RINGS: [Spinlock<(usize, [FuxWakeOp; FUXW_RING_SZ])>; FUXW_NRINGS] = [
    Spinlock::new((0, [FUXW_ZERO; FUXW_RING_SZ])),
    Spinlock::new((0, [FUXW_ZERO; FUXW_RING_SZ])),
    Spinlock::new((0, [FUXW_ZERO; FUXW_RING_SZ])),
    Spinlock::new((0, [FUXW_ZERO; FUXW_RING_SZ])),
];

/// Record one completed WAKE-class futex syscall (sc58). Called from
/// do_futex after the op returned. Per-CPU ring: the push must not
/// serialize wake syscalls across CPUs — the wedge it measures is
/// timing-sensitive, and a global lock here changed the boot's shape
/// (ohc74 booted past appfwk.ready with it, the only such boot in the
/// campaign).
pub fn fuxw_push(cmd: u8, uaddr: u64, ret: i32) {
    if !crate::trace::EVENT_FUXSTUCK.armed() {
        return;
    }
    // 80th (ohw126 AMS park): immediate per-word wake verdict line. The
    // sc58 ring only covers ~6 s of wake traffic; on ohw126 the tid-852
    // park's birth window (77-138 s) was overwritten long before the
    // census dumped it, so the lost-wake-vs-never-sent verdict was
    // unanswerable from the ring. When a fuxwatch slot is armed, every
    // WAKE-class op whose uaddr EQUALS the watched word prints at once:
    //   - wake ret>=1 while the parked waiter stays parked = kernel
    //     wake drop;
    //   - word write (FUXWATCH write log) with NO matching wake line =
    //     userspace never signaled;
    //   - neither = the producer never ran (graph stall, oh68 decode).
    // The watched word is quiet by construction (it is a stall word), so
    // the line volume is small; bounded anyway (the word may be a hot
    // lock before the wedge window).
    {
        let w1 = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
        let w2 = FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed);
        // 81st: the latched sync-holder park word matches too (the word
        // shifts per boot and even mid-park; the census latch tracks it,
        // so the immediate verdict follows the live word). Same-VA
        // foreign objects: the park word is a stack/heap VA mapped in
        // many processes — only the park tgid's ops are the verdict.
        let wp = FUXPARK_WORD.load(core::sync::atomic::Ordering::Relaxed);
        let wp_tgid = FUXPARK_TGID.load(core::sync::atomic::Ordering::Relaxed);
        let w1_tgid = FUXWATCH1_TGID.load(core::sync::atomic::Ordering::Relaxed);
        let cur0 = crate::sched::hotpath::current();
        let cur0_tgid = match cur0 {
            Some(t) => crate::sched::r#unsafe::task_shared(t).tgid() as u32,
            None => 0,
        };
        let m1 = uaddr == w1 && (w1_tgid == 0 || cur0_tgid == w1_tgid);
        let m2 = uaddr == w2;
        let mp = uaddr == wp && (wp_tgid == 0 || cur0_tgid == wp_tgid);
        if (m1 || m2 || mp) && uaddr != 0 {
            static FWW_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if FWW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 256 {
                let cur = crate::sched::hotpath::current();
                let (tid2, tgid2, comm) = match cur {
                    Some(t) => {
                        (
                            t.pid(),
                            t.tgid() as u32,
                            core::str::from_utf8(t.comm().as_slice()).unwrap_or(""),
                        )
                    },
                    None => (0, 0, ""),
                };
                let comm = comm.trim_end_matches('\0');
                // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                crate::pr_warn!(
                    "FUXWATCH-WAKE j={} tid={} tgid={} comm='{}' op={} uaddr={:#x} ret={}",
                    crate::timer::get_jiffies(),
                    tid2,
                    tgid2,
                    comm,
                    cmd,
                    uaddr,
                    ret
                );
            }
        }
    }
    let (tid, tgid) = match crate::sched::hotpath::current() {
        Some(t) => { (t.pid(), t.tgid() as u32) },
        None => (0, 0),
    };
    let r = crate::arch::cpu_id() as usize % FUXW_NRINGS;
    let mut g = FUXW_RINGS[r].lock();
    let (idx, arr) = &mut *g;
    arr[*idx] = FuxWakeOp {
        j: crate::timer::get_jiffies(),
        tid,
        tgid,
        uaddr,
        op: cmd,
        ret,
    };
    *idx = (*idx + 1) % FUXW_RING_SZ;
}

// sc58: waiter-pool occupancy maintained at alloc/free (the census reads
// it O(1) — a census-time sweep of 8192 irqsave locks ran in the timer
// tick and was its own timing pollution); jiffies of the last pool
// exhaustion (alloc_waiter None).
pub static FUX_POOL_USED: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
pub static FUX_POOL_FULL_J: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
// sc209 postmortem: 600 ops died inside RS's own init (EGL/composer
// setup is hundreds of condvar broadcasts), two dozen ops short of the
// HGM window. 4000 ops (~8000 short lines, ~1 min of UART across a
// 45 min boot) covers init + the PostSyncTask park window + any later
// render-loop ops of interest.
const FXRS_BUDGET: usize = 4000;

const RWFRZ_WATCH_SZ: usize = 8;
pub static RWFRZ_WATCH: [core::sync::atomic::AtomicUsize; RWFRZ_WATCH_SZ] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; RWFRZ_WATCH_SZ];
static RWFRZ_WATCH_CNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
// sc186: 64 lines died on arena churn before the wedge window; 192 keeps
// the same worst-case UART bound as one parkdump stack dump.
// glib-mutex: rwfrzbudget=N raises the cap (the garepro repro drives
// thousands of ops/min through the watched word; the default cap goes
// dark long before a freeze could form). Default unchanged.
pub static RWFRZ_WATCH_BUDGET: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(192);

/// rwlock-livelock: tgid whose THREAD EXITS are traced (the exit-path
/// forensics in do_exit read this). Latched at the FIRST plain-mutex
/// contention WAIT (expected val 1/2) from a process whose TGID LEADER
/// comm starts with "udisksd" — early enough to cover exits that happen
/// BEFORE the freeze census adopts the frozen word (the wedge's holder
/// must have died or stalled before the waiter parked).
pub static RWFRZ_EXIT_WATCH_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

// FXDV2: param-arena futex tracer budget (see the FXDV2 block in
// do_futex — traces every futex op on the fixed-address /dev/__parameters__
// window from any process).
static FXDV2_N: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);


pub const FUTEX_BITSET_MATCH_ANY: u32 = 0xffffffff;

// Internal flags
pub const FLAGS_SHARED: u32 = 0x0010;
pub const FLAGS_CLOCKRT: u32 = 0x0020;

/// FUTEX_WAKE_OP encoding (Linux include/uapi/linux/futex.h)
pub const FUTEX_OP_SET: u32 = 0;
pub const FUTEX_OP_ADD: u32 = 1;
pub const FUTEX_OP_OR: u32 = 2;
pub const FUTEX_OP_ANDN: u32 = 3;
pub const FUTEX_OP_XOR: u32 = 4;
pub const FUTEX_OP_CMP_EQ: u32 = 0;
pub const FUTEX_OP_CMP_NE: u32 = 1;
pub const FUTEX_OP_CMP_LT: u32 = 2;
pub const FUTEX_OP_CMP_LE: u32 = 3;
pub const FUTEX_OP_CMP_GT: u32 = 4;
pub const FUTEX_OP_CMP_GE: u32 = 5;

/// Resolve the physical word identity of `uaddr` in the CURRENT task's
/// page tables: (frame << 12) | (uaddr & 0xfff). Returns 0 when the walk
/// finds no present user leaf AND a read fault cannot materialize one
/// (callers fall back to uaddr keying — the pre-fix behavior, never
/// worse).
///
/// Fault-in parity with the reference get_futex_key: its
/// get_user_pages_fast FAULTS THE PAGE IN when the PTE is absent. That
/// matters on the WAKE side: FUTEX_WAKE never touches the word in
/// userspace, so a lazy mapping (e.g. the exec'd tst_reinit child that
/// only WAKEs — LTP setpgid03) has NO PTE at syscall entry. Without the
/// fault-in the waker resolves phys=0, hashes the uaddr bucket, and
/// never sees the phys-bucketed waiter (FXK hunt r23: parent
/// phys=0x7ffaa01c vs child phys=0, 10s of 1ms retry wakes, all lost).
fn resolve_futex_phys(uaddr: usize) -> usize {
    let Some(task) = crate::sched::hotpath::current() else {
        return 0;
    };

    let walk = |u: usize| -> usize {
        // SAFETY: read-only AddressSpace accessor on the current task.
        let root = match crate::sched::r#unsafe::task_shared(task).address_space() {
            Some(mm) => mm.root_ppn(),
            None => return 0,
        };
        // SAFETY: read-only page-table walk of the current address space.
        match crate::sync::r#unsafe::pt_walk(root, u as u64) {
            Some((ppn, bits)) => {
                // Present + user leaf only (device-window leaves and kernel
                // mappings are not futex words).
                if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 {
                    ((ppn as usize) << 12) | (u & 0xfff)
                } else {
                    0
                }
            }
            None => 0,
        }
    };

    let phys = walk(uaddr);
    if phys != 0 {
        return phys;
    }

    // No present user leaf: fault the page in (read access) like the
    // reference's get_user_pages_fast, then re-walk. resolve_futex_phys
    // runs at do_futex entry — BEFORE any futex bucket lock is held and
    // in the caller's own syscall context, so a sleeping fault (file I/O)
    // is legal here. Any fault outcome is fine: the re-walk decides
    // (a still-absent leaf just falls back to uaddr keying).
    // AddressSpace accessor; handle_mm_fault operates on the current
    // task's address space, same as the trap path (INV-SCE-2).
    {
        if let Some(mm) = crate::sched::r#unsafe::task_shared(task).address_space() {
            let page = uaddr & !0xfff;
            let _ = crate::api::mm::handle_mm_fault(
                &mm,
                crate::arch::mm::VirtAddr::new(page as u64),
                crate::arch::mm::FaultFlags::READ,
            );
        }
    }
    walk(uaddr)
}

impl FutexKey {
    pub fn new(uaddr: usize, mm: usize, flags: u32) -> Self {
        Self { uaddr, mm, flags, phys: 0, object: false }
    }

    /// Does the VMA covering `uaddr` in the CURRENT task's address space
    /// carry MAP_SHARED? FWK: only SHARED-VMA words may key on the
    /// physical frame — their frames are exempt from fork's COW marking
    /// and are therefore a stable object identity. Anon words (PRIVATE
    /// VMA) key on (mm, uaddr) whatever their frame currently is (the
    /// reference's PageAnon → FUT_OFF_MMSHARED rule). false on any
    /// lookup failure — the fallback keys (mm, uaddr), which is
    /// self-consistent on both sides of a wake.
    fn vma_is_shared(uaddr: usize) -> bool {
        let task = match crate::sched::hotpath::current() {
            Some(t) => t,
            None => return false,
        };
        // SAFETY: read-only AddressSpace accessor on the current task.
        let mm = match crate::sched::r#unsafe::task_shared(task).address_space() {
            Some(m) => m,
            None => return false,
        };
        let vma = match mm.find_vma(crate::mm::types::VirtAddr::new(uaddr)) {
            Some(v) => v,
            None => return false,
        };
        vma.flags().is_shared()
    }

    /// Production constructor (FWK keying, reference get_futex_key parity):
    /// - PRIVATE op: (mm, uaddr), no page walk, no VMA lookup (the
    ///   reference returns before any GUP) — the hot path gets cheaper.
    /// - SHARED-semantic op: object identity only when the word's VMA is
    ///   MAP_SHARED and the physical word resolved (frame-stable words);
    ///   every anon word keys (mm, uaddr), immune to COW frame swaps.
    pub fn new_user(uaddr: usize, mm: usize, flags: u32) -> Self {
        let k = if flags & FLAGS_SHARED == 0 {
            Self { uaddr, mm, flags, phys: 0, object: false }
        } else {
            let shared_vma = Self::vma_is_shared(uaddr);
            let phys = if shared_vma { resolve_futex_phys(uaddr) } else { 0 };
            Self { uaddr, mm, flags, phys, object: shared_vma && phys != 0 }
        };
        // FXK (r23 setpgid03 hunt): name every futex key construction —
        // uaddr AND the resolved physical identity — so a lost cross-
        // process wake (parent WAIT vs exec'd child WAKE on a MAP_SHARED
        // page) shows which side resolved which frame. Gated behind
        // rux:futextrace (retired debug=futextrace) (console-cost discipline; see FXDV2 note).
        if crate::trace::EVENT_FUTEX_MISS.armed() {
            static FXK_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = FXK_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 400 {
                let pid = crate::sched::hotpath::current()
                    .map(|t| t.pid())
                    .unwrap_or(0);
                let comm = crate::sched::hotpath::current()
                    .map(|t| {
                        alloc::string::String::from(t.comm_str())
                    })
                    .unwrap_or_else(|| alloc::string::String::from("?"));
                // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                crate::pr_warn!(
                    "FXK #{} pid={}({}) uaddr={:#x} phys={:#x} shared={} object={}",
                    n,
                    pid,
                    comm,
                    uaddr,
                    k.phys,
                    k.flags & FLAGS_SHARED != 0,
                    k.object
                );
            }
        }
        k
    }

    /// Check if two keys match
    pub fn matches(&self, other: &FutexKey) -> bool {
        if self.object && other.object {
            // Two object-keyed (SHARED-VMA) keys: physical-word identity
            // (any mapping of the same word matches — cross-address after
            // exec/fresh mmap, the tst_checkpoint protocol).
            self.phys == other.phys
        } else if !self.object && !other.object {
            // Anon/private identity: (mm, uaddr). Covers the musl
            // cleartid interop (shared-semantics waiter vs private
            // waker on the same word of one process) by construction —
            // both key (mm, uaddr) — and is immune to COW frame swaps.
            self.uaddr == other.uaddr && self.mm == other.mm
        } else {
            // Object identity vs (mm, uaddr) identity: different words
            // (reference: inode key never matches a private key).
            false
        }
    }
}

/// Derive the private-futex key identity for a task: the Arc allocation
/// address of its AddressSpace (CLONE_VM threads share it; fork does not).
/// Kernel threads without an mm key on 0.
pub fn task_futex_mm_id(task: *const Task) -> usize {
    if task.is_null() {
        return 0;
    }
    // Caller guarantees a valid Task (INV-SCE-2); address_space_arc()
    // clones the Arc (bumping the refcount) and we drop it after taking
    // the pointer.
    {
        match crate::sched::r#unsafe::task_shared(task).address_space_arc() {
            Some(arc) => {
                let id = alloc::sync::Arc::as_ptr(&arc) as usize;
                drop(arc);
                id
            }
            None => 0,
        }
    }
}

fn current_futex_mm_id() -> usize {
    match crate::sched::hotpath::current() {
        // SAFETY: sched::current() returns the current task's raw pointer,
        // valid for the duration of this syscall.
        Some(t) => task_futex_mm_id(t),
        None => 0,
    }
}

/// DEBUG futex protocol tracer (feature `debug-futex-trace`).
#[cfg(feature = "debug-futex-trace")]
fn ftx_trace(tag: &[u8], uaddr: usize, a: u64, b: u64) {
    use crate::debug::hotpath::{taskdump_dec, taskdump_raw_line};
    let pid = crate::sched::hotpath::current().map(|t| crate::sched::r#unsafe::task_shared(t).pid() as u64).unwrap_or(0);
    taskdump_raw_line(b"FTX ");
    taskdump_raw_line(tag);
    taskdump_raw_line(b" pid=");
    taskdump_dec(pid);
    taskdump_raw_line(b" u=");
    taskdump_dec(uaddr as u64);
    taskdump_raw_line(b" a=");
    taskdump_dec(a);
    taskdump_raw_line(b" b=");
    taskdump_dec(b);
    taskdump_raw_line(b"\n");
}

#[cfg(feature = "debug-futex-trace")]
fn ftx_opname(op: i32) -> &'static [u8] {
    match op & FUTEX_CMD_MASK {
        FUTEX_WAIT => b"WAIT",
        FUTEX_WAKE => b"WAKE",
        FUTEX_WAIT_BITSET => b"WAITBS",
        FUTEX_WAKE_BITSET => b"WAKEBS",
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => b"REQUEUE",
        FUTEX_WAKE_OP => b"WAKEOP",
        _ => b"OTHER",
    }
}

#[cfg(not(feature = "debug-futex-trace"))]
fn ftx_trace(_tag: &[u8], _uaddr: usize, _a: u64, _b: u64) {}

// SAFETY: Waiter is only ever accessed while the per-slot Spinlock is held,
// serialising all reads and writes.  The `task` raw pointer is only
// dereferenced by the waker after validating it is non-null.
unsafe impl Send for Waiter {}
unsafe impl Sync for Waiter {}

/// Waiter pool size - from config
const WAITER_POOL_SIZE: usize = crate::config::FUTEX_WAITER_POOL_SIZE;

/// Waiter pool. D-11 keyed (WaiterSlotKey, slice 3): slot locks taken
/// under a bucket guard go through `lock_nested(&bucket.proof())` (the
/// wake chain walk and the diagnostic scan do); the top-level
/// allocator and free paths stay plain. The requeue relink window's
/// same-class double-slot holds stay plain by design.
pub static WAITER_POOL: [super::types::KeyedSpinlock<
    super::types::WaiterSlotKey,
    Option<Waiter>,
>; WAITER_POOL_SIZE] = {
    const INIT: super::types::KeyedSpinlock<
        super::types::WaiterSlotKey,
        Option<Waiter>,
    > = super::types::KeyedSpinlock::new(None);
    [INIT; WAITER_POOL_SIZE]
};

/// Hash bucket count - from config
pub const HASH_SIZE: usize = crate::config::FUTEX_HASH_SIZE;

pub static HASH_HEADS: [super::types::KeyedSpinlock<
    super::types::FutexBucketKey,
    FutexChain,
>; HASH_SIZE] = {
    const INIT: super::types::KeyedSpinlock<
        super::types::FutexBucketKey,
        FutexChain,
    > = super::types::KeyedSpinlock::new(FutexChain {
        head: None,
        tail: None,
    });
    [INIT; HASH_SIZE]
};

/// Diagnostic (b133/b134 stall hunt): find the live futex-waiter entry of
/// `task`, if any. Returns (uaddr, key.mm, woken). The binder stall report
/// calls this for txn-holder threads parked in futex_wait: a `woken=true`
/// waiter that never resumed indicts the wake→run path, while `woken=false`
/// after minutes indicts the key match (compare key.mm with the task's own
/// current identity) or the userspace releaser.
pub fn waiter_of(task: *mut Task) -> Option<(usize, usize, bool)> {
    for b in HASH_HEADS.iter() {
        let h = b.lock_irqsave();
        let mut idx = h.head;
        while let Some(i) = idx {
            // SAFETY: slot indices come from chain links under the bucket
            // lock; slots are freed only under both locks, so the entry we
            // copy out stays live for the duration of the read.
            let (entry, next) = {
                let slot = WAITER_POOL[i].lock_nested(&h.proof());
                match *slot {
                    Some(ref w) => (
                        Some((w.key.uaddr, w.key.mm, w.woken, core::ptr::eq(w.task, task))),
                        w.next,
                    ),
                    None => (None, None),
                }
            };
            match entry {
                Some((uaddr, mm, woken, true)) => {
                    drop(h);
                    return Some((uaddr, mm, woken));
                }
                Some(_) => {
                    idx = next;
                }
                None => break,
            }
        }
        drop(h);
    }
    None
}

pub static FUXWATCH_ADDR: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
// sc57b: second watch slot — the chain walk auto-arms it on the first
// NEW shape-detected lock head that differs from FUXWATCH_ADDR's lock
// (the wedge's nested "B" mutex: the FFRT queue-internal lock).
pub static FUXWATCH2_ADDR: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
static FUXWATCH2_HITS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
pub static FUXWATCH_DELAY_S: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
static FUXWATCH_HITS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
// step-marker SET: two CPUs can fault+step concurrently; a single
// marker would lose one thread's #DB → SIGTRAP → thread killed.
static FUXWATCH_STEP_TIDS: [core::sync::atomic::AtomicU32; 4] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 4];
// huks-wedge lane (hw2): the stepped PAGE per marker. The #DB hook used
// to re-protect slot 1's CURRENT page whatever page was stepped — after
// a slot move that W-cleared the WRONG page (an armed-but-idle one) and
// left the stepped page writable. The pair (tid, page) makes the
// retirement land on the page the store actually executed on.
static FUXWATCH_STEP_PAGES: [core::sync::atomic::AtomicU64; 4] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 4];
pub static FUXWATCH_MM_DONE: [core::sync::atomic::AtomicU64; 128] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 128];
const FUXWATCH_BUDGET: usize = 4096;
// 81st: when slot 1 was armed by the sync-holder park latch, its word is
// a SINGLE process's object (foundation's main-stack condvar on ohw128:
// 0x7fffffffc304) — but the VA is mapped in EVERY process (main-stack
// page). Arming the page in all 67 mms both floods the exact-word log
// with foreign same-VA writes and (ohw128) overflowed the 16-slot mm
// done-set: mms armed-but-unrecorded took the un-watched fault path on
// their own stack writes -> Permission denied -> SIGSEGV storm ->
// sysrq-c cascade at 97 s. With this set, slot 1's page arms ONLY in
// the latched tgid's mms; foreign same-VA words never fault at all.
pub static FUXWATCH1_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
// huks-wedge lane (hw1): slot 2 gets its own tgid restriction. The
// 81st restricted only slot 1 (FUXWATCH1_TGID) but arm_round applied
// that restriction to the WHOLE round — with slot 1 tgid-latched, the
// round never visited any other tgid's mms, so a slot-2 word armed by
// the latch (huks on hw1w1) could never actually arm. Slot 2 is now
// tgid-restricted the same way and arm_round visits a task when EITHER
// slot's tgid matches.
pub static FUXWATCH2_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

// 81st (ohw126 frontier): the sync-holder futex park latch. The 80th's
// immediate per-word wake verdict (fuxw_push) only fires when a fuxwatch
// slot holds the word, but the tid-852 class park word SHIFTS per boot
// (and even mid-park: ohw126 0x7ff082348a84 -> ...fb4 within one page)
// so no fixed cmdline arm can target it. The TXN-HANG-BWR census is the
// first kernel component that SEES the word (a thread parked in FUTEX
// while holding a sync txn — exactly the AMS-looper shape); it latches
// the word here. The latch (a) feeds the immediate wake verdict, (b)
// arms a watch slot for the write side, (c) names the oh68 decode tgid
// (the holder's process — foundation on ohw126, not samgr, so the
// OH67_M2 gate would never decode it).
pub static FUXPARK_WORD: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub static FUXPARK_TID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub static FUXPARK_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// Jiffies of the first latch (park age reference for the verdict line).
pub static FUXPARK_J: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

#[cfg(feature = "x86_64")]
pub fn fuxwatch_retire_page(page: u64) {
    if page == 0 {
        return;
    }
    let va = page & !0xfff;
    let mut touched = 0usize;
    let g = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
    for d in FUXWATCH_MM_DONE.iter() {
        let root = d.load(core::sync::atomic::Ordering::Relaxed);
        if root == 0 {
            continue;
        }
        // SAFETY: read-only diagnostic walk of an armed root.
        match crate::sync::r#unsafe::pt_walk(root, va) {
            // Present + user + W-clear = a page WE armed (the arm round
            // wrote 0x1); restore RW.
            Some((_, bits)) if bits & 0x1 != 0 && bits & 0x4 != 0 && bits & 0x2 == 0 => {
                // SAFETY: same live root the arm round modified; one
                // page, RW.
                crate::arch::mm::mm_ops::mprotect_pte_range(root, va, 1, 0x3);
                touched += 1;
            }
            _ => {}
        }
    }
    drop(g);
    if touched > 0 {
        crate::arch::x86_64::mm::asid::flush_tlb_all();
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
        crate::pr_warn!("FUXWATCH-RETIRE page={:#x} restored W in {} mms", va, touched);
    }
}

/// Non-x86 stub: the W-bit watch is x86_64-only (see fuxwatch_arm_round).
#[cfg(not(feature = "x86_64"))]
pub fn fuxwatch_retire_page(_page: u64) {}

/// huks-wedge lane (hw2): full watch shutdown (budget trip). Retire
/// BOTH slot pages and clear the slot addresses so the fault hook,
/// the #DB hook and the arm rounds all stop matching. After this the
/// instrument is inert and every page it ever touched is writable.
pub fn fuxwatch_retire_all() {
    let a = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    let b = FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    fuxwatch_retire_page(a);
    fuxwatch_retire_page(b);
    FUXWATCH_ADDR.store(0, core::sync::atomic::Ordering::Relaxed);
    FUXWATCH2_ADDR.store(0, core::sync::atomic::Ordering::Relaxed);
    for d in FUXWATCH_MM_DONE.iter() {
        d.store(0, core::sync::atomic::Ordering::Relaxed);
    }
}

/// 62nd (ohw13 post-decode): owner-of-record latch for the watched
/// lock's l word. Every successful 0->1 acquire write overwrites the
/// latch; after a freeze no further 0->1 writes happen (only old=2
/// pile-ins), so the latch ends up naming the tid whose acquire was
/// the freeze's last one — the holder that never unlocked. The
/// TXN-HANG-BWR audit reads this and force-prints that thread's row
/// (FUX-OWNER) with a deep frame walk, regardless of the census
/// filters that previously hid it (tid344-class rows had no stacked
/// decode and no chain hit).
pub static FUXWATCH_OWNER_TID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// Jiffies of the latched acquire (the latch age, for the row).
pub static FUXWATCH_OWNER_J: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Arm round (censusd kthread context): clear W on the watched word's
/// page in every mm that maps it. Re-run cheaply; each mm armed once
/// (the fault/#DB dance keeps W cleared afterwards).
#[cfg(feature = "x86_64")]
pub fn fuxwatch_arm_round() {
    use core::sync::atomic::Ordering::Relaxed;
    let addr = FUXWATCH_ADDR.load(Relaxed);
    let addr2 = FUXWATCH2_ADDR.load(Relaxed);
    if (addr == 0 || FUXWATCH_HITS.load(Relaxed) >= FUXWATCH_BUDGET)
        && (addr2 == 0 || FUXWATCH2_HITS.load(Relaxed) >= FUXWATCH_BUDGET)
    {
        return;
    }
    if addr != 0
        && crate::timer::get_jiffies() < FUXWATCH_DELAY_S.load(Relaxed) * 100
    {
        return;
    }
    crate::sched::hotpath::for_each_task_try(|task| {
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        // 81st: a park-latched slot-1 word is watched in its OWN tgid
        // only (see FUXWATCH1_TGID) — the same VA in every other mm is
        // a different object. hw1 (huks-wedge): the restriction is now
        // PER SLOT — slot 2 carries its own tgid (FUXWATCH2_TGID) and
        // the round visits a task when EITHER slot's tgid matches; the
        // 81st applied slot 1's restriction to the whole round, so a
        // slot-2 word latched by the census could never arm.
        let park_tgid1 = FUXWATCH1_TGID.load(Relaxed);
        let park_tgid2 = FUXWATCH2_TGID.load(Relaxed);
        let this_tgid = tk.tgid() as u32;
        let ok1 = park_tgid1 == 0 || this_tgid == park_tgid1;
        let ok2 = park_tgid2 == 0 || this_tgid == park_tgid2;
        if !ok1 && !ok2 {
            return;
        }
        let Some(aspace) = tk.address_space() else {
            return;
        };
        let root = aspace.root_ppn();
        if root == 0 {
            return;
        }
        if FUXWATCH_MM_DONE
            .iter()
            .any(|d| d.load(Relaxed) == root)
        {
            return;
        }
        // page present and writable? (arm both watch words; a page
        // already armed via done-set is skipped whole)
        // SAFETY: read-only walk of the task's page tables.
        let mut to_arm: [u64; 2] = [0; 2];
        let mut narm = 0usize;
        for (si, wa) in [&addr, &addr2].iter().enumerate() {
            let wa: u64 = **wa;
            if wa == 0 || to_arm.contains(&wa) {
                continue;
            }
            if si == 0 && !ok1 {
                continue;
            }
            if si == 1 && !ok2 {
                continue;
            }
            let ok = crate::sync::r#unsafe::pt_walk(root, wa)
            .map(|(_, bits)| bits & 0x2 != 0)
            .unwrap_or(false);
            if ok {
                to_arm[narm] = wa;
                narm += 1;
            }
        }
        if narm == 0 {
            return;
        }
        // 81st (ohw128 SEGV storm): claim a done-slot BEFORE the
        // mprotect — an armed-but-unrecorded mm (done-set overflow: 82
        // mms armed into 16 slots) takes the UN-watched fault path on
        // its own writes -> present-RO PTE -> Permission denied ->
        // SIGSEGV. No free slot: skip the mm whole (never half-arm).
        let mut slot_free = false;
        for d in FUXWATCH_MM_DONE.iter() {
            if d.load(Relaxed) == 0 {
                slot_free = true;
                break;
            }
        }
        if !slot_free {
            return;
        }
        let g = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
        // SAFETY: root is a live user page-table root; R-only heap
        // data pages.
        for &wa in to_arm.iter().take(narm) {
            crate::arch::mm::mm_ops::mprotect_pte_range(root,
                    wa & !0xfff,
                    1,
                    0x1);
        }
        drop(g);
        crate::arch::x86_64::mm::asid::flush_tlb_all();
        for d in FUXWATCH_MM_DONE.iter() {
            if d
                .compare_exchange(0, root, Relaxed, Relaxed)
                .is_ok()
            {
                break;
            }
        }
        // HOTPATH-PRINT: one line per armed mm (debug-gated by the param).
        crate::pr_warn!(
            "FUXWATCH-ARM tgid={} root={:#x} page={:#x}",
            tk.tgid(),
            root,
            addr & !0xfff
        );
    });
}

#[cfg(not(feature = "x86_64"))]
pub fn fuxwatch_arm_round() {}

/// Fault-path predicates/helpers (called from the x86_64 fault hook).
#[inline]
pub fn fuxwatch_page_matches(fault_addr: u64) -> bool {
    let a = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    let b = FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    (a != 0 && (fault_addr & !0xfff) == (a & !0xfff))
        || (b != 0 && (fault_addr & !0xfff) == (b & !0xfff))
}

/// True only for mms WE armed (the done-set) — a fork-COW write fault
/// on the same VA in an un-armed mm must take the normal COW path,
/// never be granted W by the watchpoint.
#[inline]
pub fn fuxwatch_mm_armed(root_ppn: u64) -> bool {
    root_ppn != 0
        && FUXWATCH_MM_DONE
            .iter()
            .any(|d| d.load(core::sync::atomic::Ordering::Relaxed) == root_ppn)
}

#[inline]
pub fn fuxwatch_over_budget() -> bool {
    FUXWATCH_HITS.load(core::sync::atomic::Ordering::Relaxed) >= FUXWATCH_BUDGET
}

/// Log the write and arm the single-step marker; false = step must not
/// be armed (budget). `exact` — the faulting address is the watched
/// WORD itself (cr2 gives byte precision): only those log; same-page
/// neighbours of other heap objects step silently (they must not reach
/// do_page_fault, which would install W and kill the watch).
pub fn fuxwatch_log_write(tid: u32, rip: u64, exact: bool, old_word: u32, fault_addr: u64) -> bool {
    let w2 = FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    let claim = |tid: u32| {
        for (i, s) in FUXWATCH_STEP_TIDS.iter().enumerate() {
            if s.load(core::sync::atomic::Ordering::Relaxed) == tid {
                // already stepping: refresh the page so a same-tid
                // second fault (signal-handler interleave) retires the
                // page its store will land on
                FUXWATCH_STEP_PAGES[i].store(fault_addr & !0xfff, core::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
        for (i, s) in FUXWATCH_STEP_TIDS.iter().enumerate() {
            if s
                .compare_exchange(0, tid, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed)
                .is_ok()
            {
                FUXWATCH_STEP_PAGES[i].store(fault_addr & !0xfff, core::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
    };
    if !exact {
        claim(tid);
        return true;
    }
    let which2 = w2 != 0 && (fault_addr & !0x3) == (w2 & !0x3);
    let n = FUXWATCH_HITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if which2 {
        // sc58: slot 2 keeps its own budget accounting (arm_round tests
        // both counters; a shared counter let slot-2 writes retire BOTH
        // slots early).
        FUXWATCH2_HITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    if n >= FUXWATCH_BUDGET {
        // huks-wedge lane (hw2): the budget used to only stop logging —
        // the arm rounds stopped too, but the page stayed W-cleared, so
        // the NEXT write fell through the (now-skipped) fault hook to
        // do_page_fault → SEGV_ACCERR. Retire the whole watch once: both
        // pages restored, slots cleared — the instrument goes inert
        // instead of arming a trap nobody handles.
        static RETIRED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if !RETIRED.swap(true, core::sync::atomic::Ordering::Relaxed) {
            // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
            crate::pr_warn!("FUXWATCH budget reached — retiring watch");
            fuxwatch_retire_all();
        }
        return false;
    }
    // 70th: page-mate writes (the mutex cluster shares the watched page —
    // slot1 armed on the first devmgr waiter word, whose page also carries
    // the DevSvcManager mutex in every observed boot) get their own log
    // arm: the stale-EBUSY founder's acquire (0 -> 0x10, uncontended, no
    // syscall) is otherwise invisible. Budgeted separately.
    let page1 = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed) & !0xFFF;
    if !which2 && page1 != 0 && (fault_addr & !0xFFF) == page1 && fault_addr & !0x3 != FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed) & !0x3 {
        static PG_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let p = PG_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if p < 900 {
            // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
            crate::pr_warn!(
                "FW-PG #{} tid={} addr={:#x} rip={:#x} old={:#x}",
                p,
                tid,
                fault_addr,
                rip,
                old_word
            );
        }
    }
    // HOTPATH-PRINT: compact per-write line; the wedge window carries
    // at most a few hundred. (w2 marker identifies second-watch words
    // when both pages share rips.) 78th: tgid too — every FFRT process
    // allocates a mutexPrivate at the SAME heap VA, so a slot-2 watch
    // traces N processes' objects at once and tid alone cannot
    // attribute a write (ohw115: param_watcher's counter and samgr's
    // mutex shared 0x7ff000499a50).
    let wtgid = crate::sched::hotpath::current()
        .map(|t| crate::sched::r#unsafe::task_shared(t).tgid() as u32)
        .unwrap_or(0);
    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
    crate::pr_warn!("FW tid={} tgid={} rip={:#x} old={} n={} addr={:#x}{}", tid, wtgid, rip, old_word, n, fault_addr, if which2 { " w2" } else { "" });
    // 62nd: latch the owner-of-record on every successful 0->1
    // acquire-class write (the fast-path CAS or the xchg that wins
    // from UNLOCK). old_word==0 with the word transitioning to a
    // locked value = an acquire. After a freeze only old=2 pile-ins
    // remain, so the latch holds the freeze acquirer.
    // 78th: the slot-2 latch is samgr-tgid-gated — the chain armed the
    // slot on SAMGR's mutex word, but the watch itself covers every mm
    // mapping that VA, and foreign acquires (ohw115: param_watcher's
    // old=0 counter write) corrupted the latch and the OH68 owner
    // decode with it.
    if old_word == 0 && (!which2 || {
        let samgr = crate::ipc::hotpath::binder_sareg_samgr_pid();
        samgr == 0 || wtgid == samgr
    }) {
        FUXWATCH_OWNER_TID.store(tid, core::sync::atomic::Ordering::Relaxed);
        FUXWATCH_OWNER_J
            .store(crate::timer::get_jiffies() as u32, core::sync::atomic::Ordering::Relaxed);
        // oh68: latch the writer's ctx->task at the acquire instant (the
        // only moment a coroutine holder is provably executing); exact =
        // this IS the watched lock word's acquire, not a page-mate write.
        if exact {
            oh68_latch_owner_task(tid);
        }
    }
    claim(tid);
    true
}

/// Read the watched word's CURRENT value through the walker (fault
/// context: the word still holds its pre-write value).
pub fn fuxwatch_read_word(root_ppn: u64) -> u32 {
    let addr = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    if addr == 0 {
        return 0xffff_ffff;
    }
    // SAFETY: read-only walk of the current mm.
    match crate::sync::r#unsafe::pt_walk(root_ppn, addr) {
        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
            let kv = crate::arch::mm::memory_layout::phys_to_virt(
                crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (addr & 0xfff)),
            );
            // SAFETY: linear-mapped RAM word.
            crate::sync::r#unsafe::read_volatile_u32(kv.as_usize())
        }
        _ => 0xffff_ffff,
    }
}

/// #DB hook: Some(page) when this trap is our single step (marker
/// consumed) — the page the stepped store executed on. huks-wedge lane
/// (hw2): the retirement must land on THAT page; re-protecting whichever
/// page slot 1 currently points at (the old behavior) W-clears the wrong
/// page after a slot move and leaves the stepped one writable.
#[inline]
pub fn fuxwatch_take_step(tid: u32) -> Option<u64> {
    for (i, s) in FUXWATCH_STEP_TIDS.iter().enumerate() {
        if s
            .compare_exchange(tid, 0, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed)
            .is_ok()
        {
            let p = FUXWATCH_STEP_PAGES[i].swap(0, core::sync::atomic::Ordering::Relaxed);
            return Some(p);
        }
    }
    None
}

/// huks-wedge lane (hw2): is this page still watched (either slot)? The
/// #DB retirement only re-protects pages the watch still owns; a page
/// retired by a slot move must stay writable.
#[inline]
pub fn fuxwatch_page_watched(page: u64) -> bool {
    if page == 0 {
        return false;
    }
    let a = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    let b = FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    (a != 0 && (a & !0xfff) == page) || (b != 0 && (b & !0xfff) == page)
}

/// huks-wedge lane (hw2): read the u32 at `addr` (cr2 precision) through
/// the given root — the fault hook's `old_word` must describe the WORD
/// THAT FAULTED, not slot 1's word (slot-2 writes logged with slot 1's
/// old value before).
pub fn fuxwatch_read_word_at(root_ppn: u64, addr: u64) -> u32 {
    // SAFETY: read-only walk of the current mm.
    match crate::sync::r#unsafe::pt_walk(root_ppn, addr) {
        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
            let kv = crate::arch::mm::memory_layout::phys_to_virt(
                crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (addr & 0xfff)),
            );
            // SAFETY: linear-mapped RAM word.
            crate::sync::r#unsafe::read_volatile_u32(kv.as_usize())
        }
        _ => 0xffff_ffff,
    }
}

#[inline]
pub fn fuxwatch_addr() -> u64 {
    FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed)
}

#[inline]
pub fn fuxwatch2_addr() -> u64 {
    FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed)
}

/// sc57 (57th): one-shot region scan — a SUSPENDED FFRT coroutine's
/// stack lives in an 8MB stack-pool region (same class as the worker
/// pthread stacks the chain walk already sees); its frames hold the
/// contested lock pointer mid-stack while no thread does. Sweep the
/// top of every large anonymous region of the tgid's mm for the lock
/// range once per boot per lock; print region+offset hits.
/// 61st: single-word read through a task's page tables (walker-only,
/// diagnostic). Shared by the region scan's M3 shape test.
pub fn rd_word(root: u64, va: u64) -> Option<u64> {
    match crate::sync::r#unsafe::pt_walk(root, va) {
        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
            let kv = crate::arch::mm::memory_layout::phys_to_virt(
                crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
            );
            // SAFETY: linear-mapped RAM word.
            Some(crate::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
        }
        _ => None,
    }
}

/// sc57 (57th): holder scan — for a named lock-object address (the
/// waiter ring's closing head), scan every same-tgid thread's
/// entry-sp window for the lock pointer (the holder's active
/// lock_guard keeps `&lock` spilled in its frame). Walker-only reads
/// of each parked task's frozen mm; debug-gated by the caller. Returns
/// "tid+off, ..." candidate list (empty string when none).
/// 61st (ohw19 handoff): ONE-SHOT heap census of every ffrt::mutex
/// object in the tgid's address space. Signature: the mutexPrivate
/// vtable pointer at +0 — a SINGLE runtime VA per boot (read from the
/// KNOWN wedge lock), so every hit is a real ffrt::mutex (ExecuteCtx's
/// vtable differs; the QoS false-positive class is structurally
/// excluded). For each object: l value + waiter ring (up to 6 hops) +
/// the ring members' containing-ctx hint. The lock that is stuck
/// held-with-waiters and is NOT the watched one = L3 (the FFRT-internal
/// lock the M2 holder suspended on) — the cycle bottom the region scan
/// could not reach (the holder's guard lives in a coroutine chunk the
/// chunk-top sweep missed). Once per boot per tgid; the walk over the
/// anon VMAs is the cost (bounded by the same 160-region cap).
/// 61st: the heap-map arm flag — the chain closure stores the known
/// lock's OBJECT base here; the censusd KTHREAD consumes it ONCE and
/// runs the scan in sleepable context. The first cut called the map
/// straight from the TXN-HANG print path (timer/softirq): a full-heap
/// walk with IRQs off — ohw20's console died at 35 s mid-scan.
pub static HEAPMAP_ARM: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// g_executeCtxTlsKey VALUE (cached per boot; 0 = unknown).
pub static OH68_KEY: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// ExecuteCtx of the latched acquire writer (0 = none).
pub static OH68_OWNER_CTX: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
/// ctx->task at the acquire write (the holder coroutine/thread task).
pub static FUXWATCH_OWNER_TASK: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
#[cfg(target_arch = "x86_64")]
pub fn oh68_fs_base(tk: &crate::process::types::Task) -> u64 {
    tk.thread().fs_base
}
#[cfg(not(target_arch = "x86_64"))]
pub fn oh68_fs_base(_tk: &crate::process::types::Task) -> u64 {
    0
}

/// Resolve a thread's ExecuteCtx from its tsd array: the cached key, or a
/// validated scan of slots 1..64 (ctx->tid == the reading thread's tid).
/// Returns the ctx pointer (0 = not found); caches the key on first hit.
#[cfg(target_arch = "x86_64")]
pub fn oh68_ctx_of(root: u64, fs: u64, tid: u32) -> u64 {
    use core::sync::atomic::Ordering::Relaxed;
    if fs == 0 {
        return 0;
    }
    let Some(tsd) = rd_word(root, fs + 0x88) else {
        return 0;
    };
    if (tsd >> 47) != 0 || tsd < 0x1000 {
        return 0;
    }
    let try_ctx = |c: u64| -> bool {
        c != 0 && (c >> 47) == 0 && c > 0x1000 && matches!(rd_word(root, c + 248), Some(v) if (v & 0xffff_ffff) as u32 == tid)
    };
    let cached = OH68_KEY.load(Relaxed) as u64;
    if cached != 0 {
        if let Some(c) = rd_word(root, tsd + cached * 8) {
            if try_ctx(c) {
                return c;
            }
        }
    }
    for k in 1u64..64 {
        let Some(c) = rd_word(root, tsd + k * 8) else {
            continue;
        };
        if try_ctx(c) {
            OH68_KEY.compare_exchange(
                0,
                k as u32,
                Relaxed,
                Relaxed,
            )
            .ok();
            return c;
        }
    }
    0
}
#[cfg(not(target_arch = "x86_64"))]
pub fn oh68_ctx_of(_root: u64, _fs: u64, _tid: u32) -> u64 {
    0
}

/// Called from the FW write path on an exact acquire-class write
/// (old==0): the faulting thread is executing the lock acquirer at this
/// instant. Reads its TLS chain and latches ctx->task (the holder) plus
/// the ExecuteCtx (for the audit-side ThreadWait park-word check
/// ctx+224). Bounded: cached key = 3 reads, cold = up to 64.
pub fn oh68_latch_owner_task(tid: u32) {
    use core::sync::atomic::Ordering::Relaxed;
    let _ = tid;
    if cfg!(not(target_arch = "x86_64")) {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        let Some(tsk) = crate::sched::hotpath::current() else {
            return;
        };
        // SAFETY: current-task accessor in the FW fault path (the same
        // use the trap path itself makes right above this latch's
        // caller).
        let tk = crate::sched::r#unsafe::task_shared(tsk);
        let Some(aspace) = tk.address_space() else {
            return;
        };
        let root = aspace.root_ppn();
        let fs = oh68_fs_base(tk);
        let ctx = oh68_ctx_of(root, fs, tid);
        if ctx == 0 {
            return;
        }
        OH68_OWNER_CTX.store(ctx, Relaxed);
        if let Some(t) = rd_word(root, ctx + 16) {
            FUXWATCH_OWNER_TASK.store(t, Relaxed);
        }
    }
}

/// rux:fuxstuck (retired debug=fuxstuck) (17th successor): futex-stuck census, called at the ~1 Hz
/// timer heartbeat (same slot as the TXN-HANG audit). The post-creation OH
/// stall web parks threads in userspace-internal condvars (useriam's FFRT
/// executor 248-596 s, distributeddata's DB_TaskExecutor, samgr's Executor)
/// — lostpark already acquitted the kernel wake/park machinery, so the open
/// question is per-condvar: WHAT is parked where, for how long, and is any
/// producer of that condvar still runnable. This census stamps every futex
/// park (Waiter::park_j at chain insertion) and, at most one burst per 5 s,
/// reports every wait >= 3 s as one FUX-STUCK line:
///   - uaddr + mm identity of the condvar (exact, not the param-arena class)
///   - tgid/tid + the stuck thread's entry-frame syscall nr and rip
///   - the owning tgid's thread-state histogram (runnable vs futex-parked
///     vs other-int vs unint) — a tgid with zero runnable threads is
///     deadlocked userspace (no kernel lever); one with runnable threads
///     but a never-woken condvar names the starving producer chain
///     (scheduling/priority territory).
/// Locking: phase A walks the bucket chains under the bucket + nested slot
/// locks (waiter_of discipline) copying POD only; phase B reads task state
/// under for_each_task (pid-hash per-bucket lock, slowtask contract) — the
/// two phases never nest. Bounded: 8 lines per burst, 32 sampled waiters.
/// Zero cost when rux:fuxstuck (retired debug=fuxstuck) is off (one switch load).
pub fn futex_stuck_census() {
    use alloc::vec::Vec;
    use core::sync::atomic::Ordering::Relaxed;

    if !crate::trace::EVENT_FUXSTUCK.armed() {
        return;
    }
    let now = crate::timer::get_jiffies();
    // One burst per 5 s (500 jiffies at HZ=100); first call fires.
    static LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let last = LAST.load(Relaxed);
    if last != 0 && (now as u32).wrapping_sub(last) < 500 {
        return;
    }
    if LAST
        .compare_exchange(last, now as u32, Relaxed, Relaxed)
        .is_err()
    {
        return;
    }
    // Whole-boot line budget (a 25 min boot = ~300 bursts x <= 9 lines).
    static LINES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

    // 59th session: run-queue strand census rides the same burst —
    // pickable-shaped runnable tasks with a frozen exec clock (the
    // ohc74 close() strand class: linked, claim=-1, unexecuted 290 s).
    crate::sched::hotpath::runq_strand_census(now as u64);

    // runq-strand lane (121st): the culprits' side — per-CPU incumbents
    // holding their slot >= 2 s with the last tick's interrupted RIP and
    // mode (kernel loop vs user spin vs halted CPU).
    crate::sched::hotpath::monopolist_census(now as u64);

    // 63rd session (mission lever 2): the FFRT group's wake→pick
    // latency profile per burst — quiet-phase vs wedge-phase cost.
    crate::sched::hotpath::ffrt_wait_census();

    // lb343: 3 s bar (was 10 s). The OH boot cascade's decisive parks are
    // 4-5 s class — init's param-service thread sat 5.06 s between
    // hdf_devmgr's ready-set (5.433s) and init's handle (10.496s, lb342),
    // and the DB_TaskExecutor/DB metronomes poll on 4 s timers — all
    // invisible at the old bar. Diagnostics only; the 8-line burst cap
    // plus the SUM truncation line keep the volume bounded.
    const STUCK_MIN_J: u64 = 3 * 100;
    struct Stuck {
        task: *mut Task,
        uaddr: usize,
        mm: usize,
        age_s: u32,
        woken: bool,
        // Filled by phase B (0 = task not in the pid hash anymore).
        tgid: u32,
        tid: u32,
        sys: u64,
        epc: u64,
        // 48th session (wedge holder hunt): comm of the parked task
        // (OS_IPC_* vs OS_FFRT_* discriminates the samgr-internal park
        // classes), plus the LIVE value of the futex word read through
        // the pinned waiter's own page tables. fval is the ENTRY-time
        // expected value (the wait argument); live==fval after a long
        // park means the userspace event never fired (holder never ran
        // unlock), live!=fval while still parked means the wake-side
        // value change already happened and the waiter slept through
        // it — the direct lost-wake discriminator the 47th's analysis
        // (oh-src run/ffrt-park-analysis.md §2.2) said the census
        // could not make.
        comm: [u8; 16],
        live: u64,
        live_ok: bool,
        // 33rd successor — wait-site map. Futex syscall args from the
        // SAME entry frame (op/val/timeout) plus the user-space wait
        // site: the uaddr's VMA decode (which mapping, offset inside
        // it) and up to 4 user-stack words landing in EXECUTABLE VMAs
        // (candidate return addresses naming the FFRT/samgr call site
        // above the libc wrapper — the entry rip alone is the wrapper
        // for every park and discriminates nothing).
        fop: u64,
        fval: u64,
        fto: u64,
        // 34th successor — the timespec VALUE at fto (tv_sec/tv_nsec
        // read through the same foreign-mm walker). fto!=0 alone does
        // not say the park has a deadline: FFRT-class waiters pass a
        // non-NULL timeout whose value may be effectively infinite.
        // A short value that never fires indicts the timeout arm; a
        // huge value re-points at the missing wake event.
        fto_ok: bool,
        fto_sec: i64,
        fto_nsec: i64,
        uvma_ok: bool,
        uvma_start: usize,
        uvma_end: usize,
        uvma_off: usize,
        uvma_cls: u8, // 'A'non, 'F'ile, 'H'eap, 'S'tack, 'M'shm, 'D'ev
        uvma_fd: i32,
        uvma_voff: usize, // file offset when FileBacked (vma.offset + off)
        uret: [u64; 8],
        uret_n: usize,
        // 34th successor — VMA decode of the STRONGEST return-address
        // candidate (uret[1], the first non-libc one: uret[0] is the
        // wrapper's own internals): its containing VMA start/end/class
        // and, for file-backed, the file offset. Host-side: the file
        // offset symbolizes against the same .so on both kernels (the
        // image is identical) — names the samgr/FFRT call site without
        // needing the guest's ASLR base.
        u2v_ok: bool,
        u2v_start: usize,
        u2v_end: usize,
        u2v_off: usize,
        u2v_cls: u8,
        // 85th (arena founder hunt): VMA decodes for uret[2]/uret[3] —
        // the first frames ABOVE the libc wrapper + its immediate app
        // frame; the +0x8007xx arena parks showed uret[1] still inside
        // libc internals, so the app call site lives at frame 2/3.
        u3v_ok: bool,
        u3v_start: usize,
        u3v_off: usize,
        u4v_ok: bool,
        u4v_start: usize,
        u4v_off: usize,
        // 85th: the park page's structure signature — 8 words at the
        // park page start (allocator/arena headers live there: mallocng
        // group header, ring control block) and 6 words ending at
        // uaddr+0x18 (the wait object's neighborhood: mutex type word,
        // cond seq/mtx pair, list heads).
        pw0: [u64; 8],
        pw0_ok: bool,
        pwu: [u64; 6],
        pwu_ok: bool,
        // sc57 (57th session): FFRT waiter-chain walk. The park word is
        // the musl condvar seq word at ExecuteCtx->wn.cv (WaitUntilEntry
        // +0x88); the intrusive node (LinkedList prev@0/next@8) at wn+0
        // links every waiter of one ffrt::mutexPrivate /
        // SharedMutexPrivate into the OWNER object's list head (mutex:
        // `list` inside the mutex object; smx: wList1/wList2). Walking
        // node.next hops waiter->waiter->head; the hop that leaves the
        // waiter's own stack VMA is the head — the lock object itself.
        // Offsets verified against oh-robot ffrt include/sched/
        // execute_ctx.h (WaitUntilEntry: node 0x00, prev 0x10, next
        // 0x18, task 0x20, wtType 0x28, status 0x2c, hasWaitTime 0x30,
        // tp 0x38, cb 0x40, wl 0x60 (musl pthread_mutex 0x28), cv
        // 0x88); the prev-link mutuality check prunes wrong-guess
        // offsets (a hop only counts when *(hop) == cur).
        chn_task: u64,          // wn+0x20: CPUEUTask* (0 = plain thread)
        chn_wt: u32,            // wn+0x28: 0=NORMAL(mutex) 1=READ 2=WRITE
        chn_node: u64,          // the accepted WaitUntilEntry.node address
        chn_hops: [u64; 8],     // next-direction hops (0 = end)
        chn_hops_ok: u32,       // bit i set = hop i's back-link verified
        chn_prev3: [u64; 3],    // prev-direction hops
        chn_head: u64,          // ring-closing hop = the lock object's list head
        chn_head_win: [u64; 8], // words at chn_head-0x28 .. +0x28
    }
    // NO-ALLOC-UNDER-LOCK (oh-launch round 4): Phase A pushes into this
    // Vec while holding each bucket's lock (IRQs off) — a growth there
    // allocates inside the section, the pile-up ingredient the wake
    // paths just lost. The walk caps collection at 192, so reserving
    // that capacity up front (one alloc, no locks held) makes every
    // in-section push capacity-bounded: no allocation under any bucket
    // lock for the whole census.
    let mut stuck: Vec<Stuck> = Vec::with_capacity(192);
    let mut total = 0usize;

    // ---- Phase A: bucket chains (POD + task ptr copy only) ----
    'buckets: for b in HASH_HEADS.iter() {
        let h = b.lock_irqsave();
        let mut idx = h.head;
        while let Some(i) = idx {
            // SAFETY: slot index from a chain link held under the bucket
            // lock; the tuple is copied out under the nested slot lock.
            let (entry, next) = {
                let slot = WAITER_POOL[i].lock_nested(&h.proof());
                match *slot {
                    Some(ref w) if !w.task.is_null() && w.park_j != 0 => {
                        let age = now.wrapping_sub(w.park_j);
                        if age >= STUCK_MIN_J {
                            (Some((w.task, w.key.uaddr, w.key.mm, age, w.woken)), w.next)
                        } else {
                            (None, w.next)
                        }
                    }
                    Some(ref w) => (None, w.next),
                    None => (None, None),
                }
            };
            if let Some((task, uaddr, mm, age, woken)) = entry {
                total += 1;
                // 48th session: cap raised 32 -> 96 (and the early-break
                // 64 -> 192 below) — on a wedged boot the system-wide
                // stuck population is 65+, and the first-32 sample by
                // bucket-walk order missed the samgr parks entirely
                // (r41v41: FUX-GRAPH showed 1 of 8 wedge parks; the
                // TXN-HANG rows named 8 futex-parked samgr handlers the
                // census never reached). Prints stay bounded (8 + graph);
                // this only widens the walk's collection window.
                // 118th: 96 -> 192 (early-break 192 -> 384) — the wedge
                // population on the oh image is 124+ waiters, and the
                // first-96 bucket-order sample missed exactly the two
                // words the hunt needed (foundation tid 1398's
                // 0x2aab58a77a74 park — only visible via TXN-HANG — and
                // input_user_host's leader word). Same collection-window
                // argument as the 48th; phase-B cost is the O(tasks x
                // stuck) pointer-compare loop.
                if stuck.len() < 192 {
                    stuck.push(Stuck {
                        task,
                        uaddr,
                        mm,
                        age_s: (age / 100) as u32,
                        woken,
                        tgid: 0,
                        tid: 0,
                        sys: 0,
                        epc: 0,
                        comm: [0; 16],
                        live: 0,
                        live_ok: false,
                        fop: 0,
                        fval: 0,
                        fto: 0,
                        fto_ok: false,
                        fto_sec: 0,
                        fto_nsec: 0,
                        uvma_ok: false,
                        uvma_start: 0,
                        uvma_end: 0,
                        uvma_off: 0,
                        uvma_cls: b'-',
                        uvma_fd: -1,
                        uvma_voff: 0,
                        uret: [0; 8],
                        uret_n: 0,
                        u2v_ok: false,
                        u2v_start: 0,
                        u2v_end: 0,
                        u2v_off: 0,
                        u2v_cls: b'-',
                        u3v_ok: false,
                        u3v_start: 0,
                        u3v_off: 0,
                        u4v_ok: false,
                        u4v_start: 0,
                        u4v_off: 0,
                        pw0: [0; 8],
                        pw0_ok: false,
                        pwu: [0; 6],
                        pwu_ok: false,
                        chn_task: 0,
                        chn_wt: 0,
                        chn_node: 0,
                        chn_hops: [0; 8],
                        chn_hops_ok: 0,
                        chn_prev3: [0; 3],
                        chn_head: 0,
                        chn_head_win: [0; 8],
                    });
                } else if total > 384 {
                    // Sample cap reached with clear headroom — stop the
                    // walk (the SUM line carries the truncation).
                    drop(h);
                    break 'buckets;
                }
            }
            idx = next;
        }
        drop(h);
    }
    if stuck.is_empty() {
        return;
    }

    // ---- Phase B1: identify the stuck tasks (fill tgid/tid/sys/epc and
    // the wait-site decode) ----
    // lb325 wedge: this census runs at the timer-softirq heartbeat (IF=0);
    // the BLOCKING task walk self-deadlocks against a pid-hash bucket held
    // by an interrupted task-context writer. Try-walk (skipped buckets just
    // leave that stuck entry's tgid/tid fields at their phase-A defaults —
    // the uaddr/park-span data from phase A is already in hand).
    crate::sched::hotpath::for_each_task_try(|task| {
        for s in stuck.iter_mut() {
            // SAFETY: pointer identity comparison only (no deref of the
            // stored pointer).
            if core::ptr::eq(s.task, task) {
                // SAFETY: diagnostic reads under the for_each_task
                // contract (task pinned in its pid-hash bucket).
                let tk = crate::sched::r#unsafe::task_shared(task);
                s.tgid = tk.tgid() as u32;
                s.tid = tk.pid();
                s.comm.copy_from_slice(tk.comm());
                let mut usp: u64 = 0;
                if let Some(top) = tk.kernel_stack_top_checked() {
                    #[cfg(feature = "x86_64")]
                    {
                        let r = crate::sched::r#unsafe::pt_regs_at_top(top);
                        s.sys = r.orig_rax as u64;
                        s.epc = r.rip as u64;
                        // futex(uaddr, op, val, timeout, ...): rsi/rdx/r10.
                        s.fop = r.rsi;
                        s.fval = r.rdx;
                        s.fto = r.r10;
                        usp = r.rsp;
                    }
                    #[cfg(feature = "riscv64")]
                    {
                        let r = crate::sched::r#unsafe::pt_regs_at_top(top);
                        s.sys = r.a7 as u64;
                        s.epc = r.epc as u64;
                        s.fop = r.a1;
                        s.fval = r.a2;
                        s.fto = r.a3;
                        usp = r.sp;
                    }
                    #[cfg(feature = "aarch64")]
                    {
                        let r = crate::sched::r#unsafe::pt_regs_at_top(top);
                        s.sys = r.syscallno as u64;
                        s.epc = r.pc as u64;
                        s.fop = r.regs[1];
                        s.fval = r.regs[2];
                        s.fto = r.regs[3];
                        usp = r.sp;
                    }
                }
                // Wait-site decode (33rd successor). The waiter is parked
                // in this very syscall, so its user stack is frozen and
                // its mm cannot vanish while the task is pinned here.
                // VMA lookups under a TRY read lock (softirq context —
                // skip the decode this burst rather than spin IF=0
                // against an mmap/munmap writer).
                if let Some(aspace) = tk.address_space() {
                    if let Some(mgr) = aspace.vma_try_read() {
                        use crate::mm::types::{VirtAddr, VmaType};
                        // (a) uaddr's mapping
                        if let Some(vma) = mgr.find(VirtAddr::new(s.uaddr)) {
                            s.uvma_ok = true;
                            s.uvma_start = vma.start().as_usize();
                            s.uvma_end = vma.end().as_usize();
                            s.uvma_off = s.uaddr - s.uvma_start;
                            s.uvma_voff = vma.offset() + s.uvma_off;
                            let flags = vma.flags();
                            s.uvma_cls = if flags.contains(
                                crate::mm::types::VmaFlags::GROWSDOWN,
                            ) {
                                b'S'
                            } else if s.uvma_start == aspace.start_brk() {
                                b'H'
                            } else {
                                match vma.vma_type() {
                                    VmaType::FileBacked => {
                                        s.uvma_fd = vma.file_fd();
                                        b'F'
                                    }
                                    VmaType::SharedMemory => b'M',
                                    VmaType::Device => b'D',
                                    VmaType::Anonymous => b'A',
                                }
                            };
                        }
                        // (a1) LIVE futex word (48th session): the word's
                        // value NOW, through the same walker. 4-byte
                        // read at the (4-aligned) uaddr; a misaligned or
                        // unmapped uaddr just leaves live_ok=false.
                        // Compare against fval (the entry-time expected
                        // value): equal after a long park = the wake
                        // event never fired; different while still
                        // parked = the waiter slept through the value
                        // change (lost-wake class).
                        if s.uvma_ok && s.uaddr <= u64::MAX as usize - 4 {
                            let root = aspace.root_ppn();
                            if let Some((ppn, bits)) = crate::sync::r#unsafe::pt_walk(
                                    root,
                                    s.uaddr as u64,
                                ) {
                                if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 {
                                    let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                        crate::arch::mm::memory_layout::PhysAddr::new(
                                            (ppn << 12) | ((s.uaddr & 0xfff) as u64),
                                        ),
                                    );
                                    // SAFETY: linear-mapped RAM word; the
                                    // waiter parked on it this syscall.
                                    let w32 = crate::sync::r#unsafe::read_volatile_u32(kv.as_usize());
                                    s.live = w32 as u64;
                                    s.live_ok = true;
                                }
                            }
                        }
                        // (a2) the timespec at fto (34th successor): two
                        // u64 words through the walker (the struct can
                        // straddle a page boundary, so walk each word).
                        // Only when fto is non-NULL and user-addressable;
                        // misaligned/speculative pointers just skip the
                        // decode (fto_ok stays false).
                        if s.fto != 0
                            && s.fto <= u64::MAX - 16
                            && crate::arch::uaccess::access_ok(s.fto as usize, 16)
                        {
                            let root = aspace.root_ppn();
                            let mut words = [0u64; 2];
                            let mut ok = true;
                            for wi in 0..2usize {
                                let va = s.fto + 8 * (wi as u64);
                                // SAFETY: read-only walk of the pinned
                                // waiter's page tables; the timespec was
                                // copied in by futex_parse_timeout before
                                // the park, so the page is resident.
                                match crate::sync::r#unsafe::pt_walk(root, va) {
                                    Some((ppn, bits))
                                        if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 =>
                                    {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word.
                                        words[wi] = crate::sync::r#unsafe::read_volatile_u64(kv.as_usize());
                                    }
                                    _ => {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                            if ok {
                                s.fto_ok = true;
                                s.fto_sec = words[0] as i64;
                                s.fto_nsec = words[1] as i64;
                            }
                        }
                        // (b) user-stack return-address candidates: scan
                        // the 32 words above the entry sp; a word landing
                        // inside an EXECUTABLE vma is a plausible return
                        // address into userspace text (musl omits frame
                        // pointers, so this is a filtered scan, not a
                        // frame walk). Foreign-mm reads go through the
                        // walker + linear map — no CR3 switch, page is
                        // resident (the thread executed on it).
                        // 85th: window 32 -> 64 words, cap 4 -> 8 — the
                        // +0x8007xx arena parks kept uret[1] inside libc
                        // internals; the app call site sits at frame 2/3.
                        if usp != 0 {
                            let root = aspace.root_ppn();
                            let mut va = (usp + 7) & !7u64;
                            for _ in 0..64 {
                                if s.uret_n >= 8 {
                                    break;
                                }
                                // SAFETY: root_ppn of the pinned task's
                                // live mm; walk is read-only.
                                if let Some((ppn, bits)) = crate::sync::r#unsafe::pt_walk(
                                        root, va,
                                    ) {
                                    if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word.
                                        let w = crate::sync::r#unsafe::read_volatile_u64(kv.as_usize());
                                        if let Some(vma) =
                                            mgr.find(VirtAddr::new(w as usize))
                                        {
                                            if vma.flags().is_executable() {
                                                s.uret[s.uret_n] = w;
                                                s.uret_n += 1;
                                            }
                                        }
                                    }
                                }
                                va += 8;
                            }
                        }
                        // (c) VMA decode of uret[1] (34th successor): the
                        // strongest call-site candidate. File-backed ->
                        // host symbolizes uret[1]-start+off against the
                        // same .so from the (identical) system image.
                        if s.uret_n > 1 {
                            if let Some(vma) = mgr.find(VirtAddr::new(s.uret[1] as usize)) {
                                s.u2v_ok = true;
                                s.u2v_start = vma.start().as_usize();
                                s.u2v_end = vma.end().as_usize();
                                s.u2v_off = s.uret[1] as usize - s.u2v_start;
                                let fl = vma.flags();
                                s.u2v_cls = if fl.contains(
                                    crate::mm::types::VmaFlags::GROWSDOWN,
                                ) {
                                    b'S'
                                } else if s.u2v_start == aspace.start_brk() {
                                    b'H'
                                } else {
                                    match vma.vma_type() {
                                        VmaType::FileBacked => b'F',
                                        VmaType::SharedMemory => b'M',
                                        VmaType::Device => b'D',
                                        VmaType::Anonymous => b'A',
                                    }
                                };
                            }
                        }
                        // (c1) 85th: VMA decode of uret[2]/uret[3] — the
                        // app-frame candidates above the libc wrapper
                        // (file offset symbolizes offline vs the image).
                        if s.uret_n > 2 {
                            if let Some(vma) = mgr.find(VirtAddr::new(s.uret[2] as usize)) {
                                s.u3v_ok = true;
                                s.u3v_start = vma.start().as_usize();
                                s.u3v_off = s.uret[2] as usize - s.u3v_start;
                            }
                        }
                        if s.uret_n > 3 {
                            if let Some(vma) = mgr.find(VirtAddr::new(s.uret[3] as usize)) {
                                s.u4v_ok = true;
                                s.u4v_start = vma.start().as_usize();
                                s.u4v_off = s.uret[3] as usize - s.u4v_start;
                            }
                        }
                        // (c2) 85th: park-page structure signature. The
                        // +0x8007xx arena-control-page family parks point
                        // at words 0x6xx-0x8xx into an 8MiB+4K anonymous
                        // VMA's LAST page — either a dedicated control
                        // page of a ring arena or a small mapping merged
                        // behind an FFRT CSA arena. 8 words at the park
                        // page start + 6 words around the wait word name
                        // the owning structure (mallocng group header at
                        // page start vs control-block vs object array).
                        if s.uvma_ok {
                            let root = aspace.root_ppn();
                            let pw_base = (s.uaddr & !0xfff) as u64;
                            let mut ok = true;
                            for wi in 0..8usize {
                                let va = pw_base + 8 * (wi as u64);
                                match crate::sync::r#unsafe::pt_walk(root, va) {
                                    Some((ppn, bits))
                                        if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 =>
                                    {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word on
                                        // the parked waiter's own page.
                                        s.pw0[wi] = crate::sync::r#unsafe::read_volatile_u64(kv.as_usize());
                                    }
                                    _ => {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                            s.pw0_ok = ok;
                            // 6 words ending at uaddr+0x18 (uaddr-0x28 ..
                            // uaddr+0x18): the wait object neighborhood.
                            let ub = (s.uaddr & !0x7) as u64;
                            let mut ok = true;
                            for wi in 0..6usize {
                                let va = ub.wrapping_sub(0x28) + 8 * (wi as u64);
                                match crate::sync::r#unsafe::pt_walk(root, va) {
                                    Some((ppn, bits))
                                        if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 =>
                                    {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word on
                                        // the parked waiter's own page.
                                        s.pwu[wi] = crate::sync::r#unsafe::read_volatile_u64(kv.as_usize());
                                    }
                                    _ => {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                            s.pwu_ok = ok;
                        }
                        // (c3) 85th: one-shot geometry dump for the stuck
                        // tgid — (i) every anonymous VMA >= 8MiB (the FFRT
                        // CSA arenas are exactly 0x800000; the park VMAs
                        // are 0x801000 — merged? dedicated control page?)
                        // plus the park VMA's immediate neighbors, and
                        // (ii) the executable file-backed map so host-side
                        // symbolization of uret[1..3] works for ANY stuck
                        // pid (FUX-MAPS only ever dumped samgr). Budget:
                        // 6 distinct tgids per boot, 24+48 lines each.
                        if s.uvma_ok && s.uvma_end >= s.uvma_start + 0x800000 {
                            const ARENA_TGID_BUDGET: usize = 6;
                            static ARENA_SEEN: [core::sync::atomic::AtomicU32;
                                ARENA_TGID_BUDGET] = [
                                core::sync::atomic::AtomicU32::new(0),
                                core::sync::atomic::AtomicU32::new(0),
                                core::sync::atomic::AtomicU32::new(0),
                                core::sync::atomic::AtomicU32::new(0),
                                core::sync::atomic::AtomicU32::new(0),
                                core::sync::atomic::AtomicU32::new(0),
                            ];
                            let mut already = false;
                            for slot in ARENA_SEEN.iter() {
                                if slot.load(Relaxed) == s.tgid {
                                    already = true;
                                    break;
                                }
                            }
                            let mut claimed = false;
                            if !already {
                                for slot in ARENA_SEEN.iter() {
                                    if slot
                                        .compare_exchange(
                                            0,
                                            s.tgid,
                                            Relaxed,
                                            Relaxed,
                                        )
                                        .is_ok()
                                    {
                                        claimed = true;
                                        break;
                                    }
                                }
                            }
                            if claimed {
                                use crate::mm::types::VmaType;
                                // Collect the VMA table (small: < 256
                                // entries per OH process), then select.
                                struct VRow {
                                    vs: usize,
                                    ve: usize,
                                    anon_big: bool,
                                    exec_fb: bool,
                                    off: usize,
                                    fd: i32,
                                }
                                let mut rows: alloc::vec::Vec<VRow> = alloc::vec::Vec::new();
                                for vma in mgr.iter() {
                                    if rows.len() >= 256 {
                                        break;
                                    }
                                    let vs = vma.start().as_usize();
                                    let ve = vma.end().as_usize();
                                    rows.push(VRow {
                                        vs,
                                        ve,
                                        anon_big: matches!(vma.vma_type(), VmaType::Anonymous)
                                            && ve - vs >= 0x800000,
                                        exec_fb: vma.flags().is_executable()
                                            && matches!(vma.vma_type(), VmaType::FileBacked),
                                        off: vma.offset(),
                                        fd: vma.file_fd(),
                                    });
                                }
                                let park_idx = rows
                                    .iter()
                                    .position(|r| r.vs == s.uvma_start && r.ve == s.uvma_end);
                                let mut arena_n = 0usize;
                                let mut exec_n = 0usize;
                                for (i, r) in rows.iter().enumerate() {
                                    let near_park = park_idx.is_some_and(|p| {
                                        i + 1 == p || i == p + 1 || i == p
                                    });
                                    if (r.anon_big || near_park) && arena_n < 24 {
                                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                        crate::pr_warn!(
                                            "ARENA-VMAS pid={} {:x}-{:x} sz={:#x} park{}",
                                            s.tgid,
                                            r.vs,
                                            r.ve,
                                            r.ve - r.vs,
                                            if near_park { "*" } else { "" }
                                        );
                                        arena_n += 1;
                                    }
                                    if r.exec_fb && exec_n < 48 {
                                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                        crate::pr_warn!(
                                            "ARENA-EXEC pid={} {:x}-{:x} off={:#x} fd={}",
                                            s.tgid,
                                            r.vs,
                                            r.ve,
                                            r.off,
                                            r.fd
                                        );
                                        exec_n += 1;
                                    }
                                }
                            }
                        }
                        // (d) sc57: FFRT waiter-chain walk —
                        // IMAGE-CALIBRATED (the 56th's +0x1000 symbol
                        // fix re-rooted the park: ffrt::
                        // mutexPrivate::wait+0x157 after std::
                        // condition_variable::wait, disassembly of the
                        // image libffrt: ctx in rbx with wn.task@ctx+0x40,
                        // wn.wl@ctx+0x90, wn.cv@ctx+0xb8, mutex in r14
                        // with wlock@mutex+0xc). The musl cond wait
                        // parks on the waiter's OWN stack node.state
                        // (node = sp-frame word; solo waiters have NULL
                        // node links), so the word itself does not chain
                        // — but pthread_cond_timedwait SPILLS r14 =
                        // &wn.cv (a heap pointer: ExecuteCtx is new'd)
                        // into its frame. Scan the entry-sp window for
                        // heap-class pointers; for each, test whether
                        // cv-{0x88,0x98} is a linked FFRT LinkedList
                        // node (WaitUntilEntry.node, first member);
                        // then walk the verified ring — the hops are the
                        // OTHER waiters' ctx->wn nodes and the ring's
                        // head lives inside the contested
                        // mutexPrivate/SharedMutexPrivate object. All
                        // reads are page-table walks of the pinned
                        // waiter's frozen mm; wrong candidates fail the
                        // back-link verification and leave zeros.
                        if usp != 0 {
                            let root = aspace.root_ppn();
                            // SAFETY: read-only AddressSpace accessor on
                            // the pinned waiter.
                            let rd = |va: u64| -> Option<u64> {
                                match crate::sync::r#unsafe::pt_walk(root, va) {
                                    Some((ppn, bits))
                                        if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 =>
                                    {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word.
                                        Some(crate::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
                                    }
                                    _ => None,
                                }
                            };
                            // is a virtual address inside a writable
                            // non-stack VMA (heap/data — where an
                            // ExecuteCtx or lock object lives)?
                            let heapish = |va: u64| -> bool {
                                if va <= 0x10000 || (va >> 47) != 0 {
                                    return false;
                                }
                                match mgr.find(VirtAddr::new(va as usize)) {
                                    Some(vma) => {
                                        !vma.flags().is_executable()
                                            && !vma
                                                .flags()
                                                .contains(
                                                    crate::mm::types::VmaFlags::GROWSDOWN,
                                                )
                                    }
                                    None => false,
                                }
                            };
                            'cand: for wi in 0..96u64 {
                                let Some(w) = rd(usp + 8 * wi) else {
                                    continue;
                                };
                                if !heapish(w) {
                                    continue;
                                }
                                // W on the stack may be &wn.cv (r14
                                // spill; cv@wn+0x88 or +0x98 depending
                                // on the WaitEntry packing) or &wn.wl
                                // (mutexPrivate::wait's own spill;
                                // wl@wn+0x60 or +0x70) — try each; the
                                // ring verification prunes the wrong
                                // ones.
                                for &xoff in &[0x60u64, 0x70, 0x88, 0x98] {
                                    let node = w.wrapping_sub(xoff);
                                    if !heapish(node) {
                                        continue;
                                    }
                                    let (Some(prev), Some(next)) = (rd(node), rd(node + 8))
                                    else {
                                        continue;
                                    };
                                    // unlinked LinkedList nodes point at
                                    // themselves; linked ones never do
                                    if prev == 0
                                        || next == 0
                                        || prev == node
                                        || next == node
                                        || !heapish(next)
                                        || !heapish(prev)
                                    {
                                        continue;
                                    }
                                    // verify BOTH directions' back-links
                                    // before trusting the ring
                                    let ok_next = rd(next) == Some(node);
                                    let ok_prev = rd(prev + 8) == Some(node);
                                    if !ok_next && !ok_prev {
                                        continue;
                                    }
                                    // candidate accepted: walk next up to
                                    // 8 hops, window the hop that closes
                                    // the ring (the head — a member of
                                    // the lock object)
                                    s.chn_task = rd(node + 0x20).unwrap_or(0);
                                    s.chn_wt = rd(node + 0x28).unwrap_or(0) as u32;
                                    s.chn_node = node;
                                    let mut cur = node;
                                    let mut prev_hop = node;
                                    for h in 0..8 {
                                        let Some(nxt) = rd(cur + 8) else {
                                            break;
                                        };
                                        if nxt == node {
                                            // ring closed: prev_hop is the
                                            // head (its container is the
                                            // lock object)
                                            if s.chn_head == 0 && prev_hop != node {
                                                s.chn_head = prev_hop;
                                                for (hw, wv) in
                                                    s.chn_head_win.iter_mut().enumerate()
                                                {
                                                    if let Some(x) = rd(
                                                        prev_hop
                                                            .wrapping_sub(0x28)
                                                            + 8 * hw as u64,
                                                    ) {
                                                        *wv = x;
                                                    }
                                                }
                                            }
                                            break;
                                        }
                                        if nxt == 0 || (nxt >> 47) != 0 || !heapish(nxt) {
                                            break;
                                        }
                                        if rd(nxt) == Some(cur) {
                                            s.chn_hops_ok |= 1 << h;
                                        }
                                        s.chn_hops[h] = nxt;
                                        prev_hop = cur;
                                        cur = nxt;
                                    }
                                    if s.chn_hops[0] == 0 {
                                        // next-walk dead-ended: the head
                                        // sits the other way; record the
                                        // prev direction
                                        let mut cur = node;
                                        for h in 0..3 {
                                            let Some(prv) = rd(cur) else {
                                                break;
                                            };
                                            if prv == 0
                                                || prv == node
                                                || (prv >> 47) != 0
                                                || !heapish(prv)
                                            {
                                                break;
                                            }
                                            s.chn_prev3[h] = prv;
                                            cur = prv;
                                        }
                                    }
                                    break 'cand;
                                }
                            }
                        }
                    }
                }
                break;
            }
        }
    });

    // ---- Phase B2: per-tgid thread histograms for the stuck tgids ----
    struct Histo {
        tgid: u32,
        run: u32,
        futex: u32,
        int_other: u32,
        unint: u32,
    }
    let mut histos: Vec<Histo> = Vec::new();
    // Same lb325 discipline as phase B1: try-walk from the heartbeat —
    // a skipped bucket only under-counts that tgid's histogram this burst.
    crate::sched::hotpath::for_each_task_try(|task| {
        // SAFETY: diagnostic reads under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        let tgid = tk.tgid() as u32;
        if !stuck.iter().any(|s| s.tgid == tgid) {
            return;
        }
        let st_bits = tk.state().bits();
        let run = st_bits == crate::process::types::TaskState::RUNNING
            || tk.on_cpu()
            || tk.sched_entity().is_on_rq();
        let fux = tk.ext.futex_wait_enqueued.load(Relaxed);
        let slot = match histos.iter_mut().find(|h| h.tgid == tgid) {
            Some(h) => h,
            None => {
                if histos.len() >= 16 {
                    return;
                }
                histos.push(Histo {
                    tgid,
                    run: 0,
                    futex: 0,
                    int_other: 0,
                    unint: 0,
                });
                histos.last_mut().unwrap()
            }
        };
        if run {
            slot.run += 1;
        } else if fux {
            slot.futex += 1;
        } else if st_bits & 0x1 != 0 {
            slot.int_other += 1;
        } else if st_bits & 0x2 != 0 {
            slot.unint += 1;
        }
    });

    // ---- Phase C: aggregate by (tgid, uaddr) and print ----
    // Two visibility buckets (lb323 lesson: a pure oldest-first top-8 was
    // monopolised by 155 s-class peripheral parks — bluetooth/audio — and
    // never showed the critical-path park that decided the creation race):
    //   OLDEST 4 groups by max park age  — the long-standing anchors;
    //   FRESH   4 groups by min park age  — the currently-developing stall
    //            (smallest age >= threshold among stuck waiters).
    stuck.sort_by(|a, b| b.age_s.cmp(&a.age_s));
    struct Group {
        tgid: u32,
        uaddr: usize,
        n: usize,
        max_age: u32,
        min_age: u32,
        any_woken: bool,
        rep_tid: u32,
        rep_mm: usize,
        rep_sys: u64,
        rep_epc: u64,
        rep_comm: [u8; 16],
        rep_live: u64,
        rep_live_ok: bool,
        rep_fop: u64,
        rep_fval: u64,
        rep_fto: u64,
        rep_fto_ok: bool,
        rep_fto_sec: i64,
        rep_fto_nsec: i64,
        rep_uvma_ok: bool,
        rep_uvma_start: usize,
        rep_uvma_end: usize,
        rep_uvma_off: usize,
        rep_uvma_cls: u8,
        rep_uvma_fd: i32,
        rep_uvma_voff: usize,
        rep_uret: [u64; 8],
        rep_uret_n: usize,
        rep_u2v_ok: bool,
        rep_u2v_start: usize,
        rep_u2v_end: usize,
        rep_u2v_off: usize,
        rep_u2v_cls: u8,
        // 85th: app-frame decodes + park-page structure signature.
        rep_u3v_ok: bool,
        rep_u3v_start: usize,
        rep_u3v_off: usize,
        rep_u4v_ok: bool,
        rep_u4v_start: usize,
        rep_u4v_off: usize,
        rep_pw0: [u64; 8],
        rep_pw0_ok: bool,
        rep_pwu: [u64; 6],
        rep_pwu_ok: bool,
        rep_chn_task: u64,
        rep_chn_wt: u32,
        rep_chn_node: u64,
        rep_chn_hops: [u64; 8],
        rep_chn_hops_ok: u32,
        rep_chn_prev3: [u64; 3],
        rep_chn_head: u64,
        rep_chn_head_win: [u64; 8],
    }
    let mut groups: Vec<Group> = Vec::new();
    for s in stuck.iter() {
        if s.tgid == 0 {
            continue;
        }
        let existing = groups
            .iter_mut()
            .find(|g| g.tgid == s.tgid && g.uaddr == s.uaddr);
        match existing {
            Some(g) => {
                g.n += 1;
                g.max_age = g.max_age.max(s.age_s);
                g.min_age = g.min_age.min(s.age_s);
                g.any_woken |= s.woken;
            }
            None => {
                // 48th session: 24 -> 48 — a wedged boot carries ~16
                // samgr park groups plus ~20 peripheral ones; the 24 cap
                // could exclude the samgr tail from the FUX-GRAPH pass.
                if groups.len() < 48 {
                    groups.push(Group {
                        tgid: s.tgid,
                        uaddr: s.uaddr,
                        n: 1,
                        max_age: s.age_s,
                        min_age: s.age_s,
                        any_woken: s.woken,
                        rep_tid: s.tid,
                        rep_mm: s.mm,
                        rep_sys: s.sys,
                        rep_epc: s.epc,
                        rep_comm: s.comm,
                        rep_live: s.live,
                        rep_live_ok: s.live_ok,
                        rep_fop: s.fop,
                        rep_fval: s.fval,
                        rep_fto: s.fto,
                        rep_fto_ok: s.fto_ok,
                        rep_fto_sec: s.fto_sec,
                        rep_fto_nsec: s.fto_nsec,
                        rep_uvma_ok: s.uvma_ok,
                        rep_uvma_start: s.uvma_start,
                        rep_uvma_end: s.uvma_end,
                        rep_uvma_off: s.uvma_off,
                        rep_uvma_cls: s.uvma_cls,
                        rep_uvma_fd: s.uvma_fd,
                        rep_uvma_voff: s.uvma_voff,
                        rep_uret: s.uret,
                        rep_uret_n: s.uret_n,
                        rep_u2v_ok: s.u2v_ok,
                        rep_u2v_start: s.u2v_start,
                        rep_u2v_end: s.u2v_end,
                        rep_u2v_off: s.u2v_off,
                        rep_u2v_cls: s.u2v_cls,
                        rep_u3v_ok: s.u3v_ok,
                        rep_u3v_start: s.u3v_start,
                        rep_u3v_off: s.u3v_off,
                        rep_u4v_ok: s.u4v_ok,
                        rep_u4v_start: s.u4v_start,
                        rep_u4v_off: s.u4v_off,
                        rep_pw0: s.pw0,
                        rep_pw0_ok: s.pw0_ok,
                        rep_pwu: s.pwu,
                        rep_pwu_ok: s.pwu_ok,
                        rep_chn_task: s.chn_task,
                        rep_chn_wt: s.chn_wt,
                        rep_chn_node: s.chn_node,
                        rep_chn_hops: s.chn_hops,
                        rep_chn_hops_ok: s.chn_hops_ok,
                        rep_chn_prev3: s.chn_prev3,
                        rep_chn_head: s.chn_head,
                        rep_chn_head_win: s.chn_head_win,
                    });
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|&a, &b| groups[b].max_age.cmp(&groups[a].max_age));
    // 77th: the OS_Dfx* periodic pollers (DfxWatchdog 60 s poll,
    // DfxIpcCheck) are always-benign stuck groups that monopolize the
    // 8-line print budget on wedged OH boots (ohw108: stuck_total=193
    // printed=8, all watchdog-class — the samgr processLock parks
    // never surfaced). Deprioritize them in BOTH selectors: skipped in
    // the by-age top-4 and in the fresh fill; a genuinely stuck
    // DfxWatchdog still shows via TXN-HANG (it would hold a txn).
    let is_dfx_poller = |g: &Group| g.rep_comm.starts_with(b"OS_Dfx");
    let mut chosen: Vec<(usize, bool)> = Vec::new(); // (group idx, is_fresh)
    for &gi in order.iter() {
        if chosen.len() >= 4 {
            break;
        }
        if !is_dfx_poller(&groups[gi]) {
            chosen.push((gi, false));
        }
    }
    let mut fresh_order = order.clone();
    fresh_order.sort_by(|&a, &b| groups[a].min_age.cmp(&groups[b].min_age));
    for &gi in fresh_order.iter() {
        if chosen.len() >= 8 {
            break;
        }
        if is_dfx_poller(&groups[gi]) {
            continue;
        }
        if !chosen.iter().any(|(c, _)| *c == gi) {
            chosen.push((gi, true));
        }
    }
    let mut printed = 0usize;
    for (gi, is_fresh) in chosen {
        let g = &groups[gi];
        let h = histos.iter().find(|h| h.tgid == g.tgid);
        // HOTPATH-PRINT: fuxstuck census report — gated by
        // rux:fuxstuck (retired debug=fuxstuck) at futex_stuck_census() entry plus the 5s burst
        // budget and the 1200-line LINES cap; never fires on a plain boot.
        use core::fmt::Write as _;
        let mut vmx = alloc::string::String::new();
        if g.rep_uvma_ok && g.rep_uvma_cls == b'F' {
            let _ = write!(vmx, " fd={} voff={:#x}", g.rep_uvma_fd, g.rep_uvma_voff);
        }
        let mut uret_s = alloc::string::String::new();
        for i in 0..g.rep_uret_n {
            let _ = write!(uret_s, "{:#x} ", g.rep_uret[i]);
        }
        let mut fto_s = alloc::string::String::new();
        if g.rep_fto_ok {
            let _ = write!(fto_s, "={}s+{}ns", g.rep_fto_sec, g.rep_fto_nsec);
        }
        let mut u2v_s = alloc::string::String::new();
        if g.rep_u2v_ok {
            let _ = write!(
                u2v_s,
                " u2v={:#x}+{:#x}{}",
                g.rep_u2v_start,
                g.rep_u2v_off,
                g.rep_u2v_cls as char
            );
        }
        // 85th: frame-2/3 decodes (app call-site candidates).
        if g.rep_u3v_ok {
            let _ = write!(u2v_s, " u3v={:#x}+{:#x}", g.rep_u3v_start, g.rep_u3v_off);
        }
        if g.rep_u4v_ok {
            let _ = write!(u2v_s, " u4v={:#x}+{:#x}", g.rep_u4v_start, g.rep_u4v_off);
        }
        // 48th session: comm + live-word verdict on the print.
        let comm_s = core::str::from_utf8(&g.rep_comm)
            .unwrap_or("")
            .trim_end_matches('\0');
        // fval is the 32-bit syscall arg sign-extended into the saved
        // u64 register slot — compare the low 32 bits only (a negative
        // expected value, e.g. the musl requeue-stamped 0x80000010,
        // otherwise reads as a false LIVE!=fval).
        let live_s = if g.rep_live_ok {
            if g.rep_live == (g.rep_fval & 0xffff_ffff) && g.rep_fval != 0 {
                "live==fval"
            } else {
                "LIVE!=fval"
            }
        } else {
            "live=?"
        };
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
        crate::pr_warn!(
            "FUX-STUCK{} tgid={} tid={} ({}) uaddr={:#x} mm={:#x} n={} park={}s..{}s anywoken={} sys={} epc={:#x} fop={:#x} fval={:#x} {}={:#x} fto={:#x}{} uvma={:#x}-{:#x} {}+{:#x}{} u2v={} uret={}tgidthr[run={} futex={} int={} unint={}]",
            if is_fresh { "-FRESH" } else { "" },
            g.tgid,
            g.rep_tid,
            comm_s,
            g.uaddr,
            g.rep_mm,
            g.n,
            g.min_age,
            g.max_age,
            g.any_woken as u32,
            g.rep_sys,
            g.rep_epc,
            g.rep_fop,
            g.rep_fval,
            live_s,
            g.rep_live,
            g.rep_fto,
            fto_s,
            g.rep_uvma_start,
            g.rep_uvma_end,
            g.rep_uvma_cls as char,
            g.rep_uvma_off,
            vmx,
            u2v_s,
            uret_s,
            h.map(|h| h.run).unwrap_or(0),
            h.map(|h| h.futex).unwrap_or(0),
            h.map(|h| h.int_other).unwrap_or(0),
            h.map(|h| h.unint).unwrap_or(0),
        );
        // debug-migrate: the census row also records into the unified
        // framework (rux:fuxstuck — the event is also the family's arm
        // switch, so it is armed by construction here). The record
        // carries the row's identifying fields; the serial line above
        // keeps the full context dump.
        crate::trace::trace_fuxstuck(
            g.tgid,
            g.rep_tid,
            g.uaddr as u64,
            g.rep_mm as u64,
            g.n as u32,
            g.min_age,
            g.max_age,
            g.rep_sys as i32,
            g.rep_epc,
        );
        // neverword lane (forensic-only, not for main): EventRunner-named
        // standing park >= 60 s gets the CTD decode UNCONDITIONALLY. The
        // lr-series f2 boot proved the W-trigger's 2-per-burst budget is
        // consumed every burst by older standing words (storage_daemon
        // parked 8 s, bluetooth 12 s ...) iterated ahead of foundation's
        // FRESH-tail group (parked 10.5 s): CTD-END W fired for tgids
        // 421/438/637/641 only, never for foundation 709 — the launcher
        // wall's word never decoded. The DONE latch inside ctd_decode
        // makes this at-most-once per tgid per boot.
        if is_fresh
            && g.max_age >= 60
            && g.uaddr > 0x1000
            && comm_s.starts_with("EventRunner")
        {
            crate::api::sync::ctd_decode(g.tgid, g.uaddr as u64, "V");
        }
        // 118th (mouse-endgame): standing-word holder scan. A group whose
        // park has stood >= 60 s names a lock that will never turn again;
        // the TXN-HANG parkv arm only latches rwfrz_reader_scan for looper
        // threads owing a txn, so the empty-stack parks (foundation tid
        // 1332 on the 0x80000010 mutex; the HDI host leader parks at
        // fval 0x80000000) never got a holder verdict. Run the existing
        // reader scan for THIS word in THIS tgid — every same-tgid thread
        // whose stack window references the word prints as an RDR row
        // with its own park word (the holder's terminal park; a zero-hit
        // pass acquits thread-side holders and points at a suspended
        // coroutine). Once per (tgid, word) per 60 s, budget 2 per burst
        // — the same for_each_task_try + page-walk cost class the parkv
        // arm already runs in this heartbeat slot.
        if g.max_age >= 60 && g.uaddr > 0x1000 {
            use core::sync::atomic::Ordering::Relaxed;
            static SCAN_TGID: [core::sync::atomic::AtomicU32; 24] =
                [const { core::sync::atomic::AtomicU32::new(0) }; 24];
            static SCAN_WORD: [core::sync::atomic::AtomicU64; 24] =
                [const { core::sync::atomic::AtomicU64::new(0) }; 24];
            static SCAN_J: [core::sync::atomic::AtomicU32; 24] =
                [const { core::sync::atomic::AtomicU32::new(0) }; 24];
            static SCAN_BURST: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            static SCAN_USED: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let burst = (now / 500) as u32;
            let cur_burst = SCAN_BURST.load(Relaxed);
            if cur_burst != burst
                && SCAN_BURST
                    .compare_exchange(cur_burst, burst, Relaxed, Relaxed)
                    .is_ok()
            {
                SCAN_USED.store(0, Relaxed);
            }
            let used = SCAN_USED.load(Relaxed);
            let mut slot_due = false;
            let mut slot_ix = usize::MAX;
            for i in 0..24 {
                let t = SCAN_TGID[i].load(Relaxed);
                let w = SCAN_WORD[i].load(Relaxed);
                if t == g.tgid && w == g.uaddr as u64 {
                    slot_ix = i;
                    let j = SCAN_J[i].load(Relaxed);
                    slot_due = now.wrapping_sub(j as u64) >= 600;
                    break;
                }
                if t == 0 && slot_ix == usize::MAX {
                    slot_ix = i;
                    slot_due = true;
                }
            }
            if slot_due
                && used < 8
                && slot_ix != usize::MAX
                && SCAN_USED
                    .compare_exchange(used, used + 1, Relaxed, Relaxed)
                    .is_ok()
            {
                SCAN_TGID[slot_ix].store(g.tgid, Relaxed);
                SCAN_WORD[slot_ix].store(g.uaddr as u64, Relaxed);
                SCAN_J[slot_ix].store(now as u32, Relaxed);
                crate::api::sync::rwfrz_reader_scan(g.tgid, g.uaddr as u64);
                // 120th ROOT-CAUSE CORRECTION: the 118th's gate
                // `uaddr & 0xffff_f000 == 0x3fbf_13e000` used a 32-bit
                // mask against a 42-bit constant — always false, so LLVM
                // (correctly) DCE'd malloc_holder_scan out of every
                // release binary since the 118th. 64-bit page mask.
                let gu = g.uaddr as u64;
                if gu & 0xffff_ffff_f000 == 0x3fbf_13e000 {
                    crate::api::sync::malloc_holder_scan(g.tgid, gu);
                }
                // 120th GENERALIZED TRIGGER: a word that stood parked
                // >= 60 s with zero wakes gets the CoTask decoder with
                // THIS word as the key. The holder datum (a suspended
                // coroutine whose saved stack references the word) and
                // the resumer chains (owning serial queue active flag +
                // qname; WE ring; WUE deadline) are word-generic — only
                // the ALLOC-RIP/aframes arms are malloc-word-specific
                // and they add signal, never gate. The historical
                // foundation freeze keyed the malloc word; the
                // deterministic pre-foundation wedge (ctf15+ faces)
                // keys FFRT mutex/rwlock words in samgr/storage/param
                // instead — same release-side-never-runs family, and
                // now decodable. Cost bound: the per-tgid DONE latch
                // inside ctd_decode (16 slots) allows one decode per
                // tgid per boot.
                crate::api::sync::ctd_decode(g.tgid, gu, "W");
            }
        }
        // 51st session: follow this word's futex lifecycle from here on
        // (FXRS word watch — the WAIT/WAKE trace decides holder-never-
        // releases from kernel-lost-wake).
        // sc57: the FFRT waiter-chain walk lines (only when the layout
        // check admitted the wn — plain futex parks print nothing).
        if g.rep_chn_hops.iter().any(|&h| h != 0) || g.rep_chn_prev3.iter().any(|&h| h != 0) {
            let mut hops_s = alloc::string::String::new();
            for (h, &a) in g.rep_chn_hops.iter().enumerate() {
                if a == 0 {
                    break;
                }
                let _ = write!(
                    hops_s,
                    " {:#x}{}",
                    a,
                    if g.rep_chn_hops_ok & (1 << h) != 0 { "*" } else { "?" }
                );
            }
            let mut prev_s = alloc::string::String::new();
            for &a in g.rep_chn_prev3.iter() {
                if a == 0 {
                    break;
                }
                let _ = write!(prev_s, " {:#x}", a);
            }
            // HOTPATH-PRINT: FUX-CHAIN — same rux:fuxstuck (retired debug=fuxstuck) gate and
            // burst budget as the FUX-STUCK row it follows; one to
            // three lines per reported group.
            crate::pr_warn!(
                "FUX-CHAIN tid={} node={:#x} task={:#x} wt={} hops:{} prev:{}",
                g.rep_tid,
                g.rep_chn_node,
                g.rep_chn_task,
                g.rep_chn_wt,
                hops_s,
                prev_s
            );
            if g.rep_chn_head != 0 {
                let mut win_s = alloc::string::String::new();
                for &wv in g.rep_chn_head_win.iter() {
                    let _ = write!(win_s, "{:#x} ", wv);
                }
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "FUX-CHAIN-HEAD tid={} head={:#x} win:{}",
                    g.rep_tid,
                    g.rep_chn_head,
                    win_s
                );
            }
        }
        // 85th: park-page structure signature — page-start words and the
        // wait-word neighborhood; same rux:fuxstuck (retired debug=fuxstuck) gate + burst budget
        // as the row above (follows its group).
        if g.rep_pw0_ok || g.rep_pwu_ok {
            let mut w_s = alloc::string::String::new();
            if g.rep_pw0_ok {
                let _ = write!(w_s, " pw0:");
                for &wv in g.rep_pw0.iter() {
                    let _ = write!(w_s, " {:#x}", wv);
                }
            }
            if g.rep_pwu_ok {
                let _ = write!(w_s, " pwu[-28..+18]:");
                for &wv in g.rep_pwu.iter() {
                    let _ = write!(w_s, " {:#x}", wv);
                }
            }
            // HOTPATH-PRINT: park-page words — one line per reported
            // group, inside the same 8-line burst budget.
            crate::pr_warn!(
                "FUX-PAGE tid={} page={:#x}{}",
                g.rep_tid,
                g.uaddr & !0xfff,
                w_s
            );
        }
        fxrs_word_add(g.uaddr);
        printed += 1;
        if LINES.fetch_add(1, Relaxed) > 1200 {
            break;
        }
    }
    if total > printed && printed == 8 {
        // HOTPATH-PRINT: census overflow marker — same rux:fuxstuck (retired debug=fuxstuck) gate
        // and 5s burst budget as the FUX-STUCK rows above (8 per burst).
        crate::pr_warn!(
            "FUX-STUCK-SUM stuck_total={} printed=8 (sampled {})",
            total,
            96
        );
    }

    // ---- sc58 (58th): the wake-side verdict pass. Three additions, all
    // under this same 5 s burst:
    //   (a) waiter-pool occupancy + last-exhaustion stamp (a wedge-time
    //       pool-full makes every later FUTEX_WAIT spin in backoff
    //       retries — invisible to the bucket-walk census above);
    //   (b) the FUXW ring birth-window dump: WAKE-class syscalls recorded
    //       around the OLDEST stuck parks' births (park_j-2s .. +5s) —
    //       closes the FXRS arming gap (FXRS starts watching a word only
    //       3+ s after it parked; the lost wake happens at birth);
    //   (c) auto-arm fuxwatch slot 2 on the first stuck FFRT-worker park
    //       word in the registered samgr pid — that word IS the FFRT
    //       queue-internal mutex's musl lock word (fval=0x80000010 =
    //       held|waiters): its write trace decides "unlock happens but
    //       waiter never wakes" (kernel drop) from "no unlock ever"
    //       (holder never runs — reference-diff branch).
    //       60th DISABLED: the park word is a musl cv waiter node on
    //       the waiter's OWN stack — its write trace cannot identify
    //       the mutex owner, and arming it stole the slot (and the
    //      censusd arm rounds) from the chain-walk's LOCK-HEAD arm
    //      (fuxwatch-LOCKHEAD), which is the owner-naming instrument.
    {
        use core::fmt::Write as _;
        // (a) pool stats — O(1) counter (see FUX_POOL_USED).
        let pool_used = FUX_POOL_USED.load(Relaxed);
        let last_full = FUX_POOL_FULL_J.load(Relaxed);
        // (c) DISABLED (60th): see the comment above — the stack-word
        // auto-arm lost the slot race against the chain-walk's
        // LOCK-HEAD arm; kept as a const-gated block for reference.
        const CENSUS_STACK_WORD_ARM: bool = false;
        let samgr_pid = crate::ipc::hotpath::binder_sareg_samgr_pid();
        if CENSUS_STACK_WORD_ARM && samgr_pid != 0 && FUXWATCH2_ADDR.load(Relaxed) == 0 {
            for g in groups.iter() {
                if g.tgid == samgr_pid && g.rep_comm[..7] == *b"OS_FFRT" {
                    let w2 = g.uaddr as u64;
                    if FUXWATCH2_ADDR
                        .compare_exchange(0, w2, Relaxed, Relaxed)
                        .is_ok()
                    {
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!(
                            "FUXWATCH2-AUTO word={:#x} tid={} park={}s",
                            w2,
                            g.rep_tid,
                            g.max_age
                        );
                    }
                    break;
                }
            }
        }
        // (b cont.) pick the dump targets first (top-3 by max_age plus
        // any samgr FFRT group), then snapshot ONLY matching entries —
        // the ring is 64k entries; copying it whole every 5 s would be
        // a 2 MB alloc churn under its lock.
        static CTX_LINES: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        {
            let mut order2: alloc::vec::Vec<usize> = (0..groups.len()).collect();
            order2.sort_by(|&a, &b| groups[b].max_age.cmp(&groups[a].max_age));
            let mut picks: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
            for &gi in order2.iter().take(3) {
                picks.push(gi);
            }
            for (gi, g) in groups.iter().enumerate() {
                if picks.len() >= 4 {
                    break;
                }
                if !picks.contains(&gi)
                    && g.tgid == samgr_pid
                    && g.rep_comm[..7] == *b"OS_FFRT"
                {
                    picks.push(gi);
                }
            }
            if picks.is_empty() || CTX_LINES.load(Relaxed) > 420 {
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "FUXW-SUM pool={}/{} last_full_j={} (no dump targets)",
                    pool_used,
                    WAITER_POOL_SIZE,
                    if last_full == 0 { -1i64 } else { last_full as i64 },
                );
            } else {
                // windows + word set for the filter pass
                let wins: alloc::vec::Vec<(u64, u64, u64)> = picks
                    .iter()
                    .map(|&gi| {
                        let g = &groups[gi];
                        let birth = now.saturating_sub(g.max_age as u64 * 100);
                        (
                            birth.saturating_sub(200),
                            birth + 500,
                            g.uaddr as u64,
                        )
                    })
                    .collect();
                let mut oldest_j = u64::MAX;
                let mut newest_j = 0u64;
                let mut ring_n = 0u64;
                let mut snap: alloc::vec::Vec<FuxWakeOp> = alloc::vec::Vec::new();
                for ring in FUXW_RINGS.iter() {
                    let g = ring.lock();
                    let (idx, arr) = &*g;
                    for k in 0..FUXW_RING_SZ {
                        let i = (*idx + k) % FUXW_RING_SZ;
                        let e = arr[i];
                        if e.j == 0 {
                            continue;
                        }
                        ring_n += 1;
                        oldest_j = oldest_j.min(e.j);
                        newest_j = newest_j.max(e.j);
                        if wins
                            .iter()
                            .any(|&(lo, hi, w)| (e.j >= lo && e.j <= hi) || e.uaddr == w)
                        {
                            snap.push(e);
                        }
                    }
                }
                snap.sort_by_key(|e| e.j);
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "FUXW-SUM pool={}/{} last_full_j={} ring={} oldest={} newest={}",
                    pool_used,
                    WAITER_POOL_SIZE,
                    if last_full == 0 { -1i64 } else { last_full as i64 },
                    ring_n,
                    if oldest_j == u64::MAX { -1i64 } else { oldest_j as i64 },
                    newest_j
                );
                for (pi, &gi) in picks.iter().enumerate() {
                    let g = &groups[gi];
                    let birth = now.saturating_sub(g.max_age as u64 * 100);
                    let (win_lo, win_hi) = (wins[pi].0, wins[pi].1);
                    let in_win: alloc::vec::Vec<&FuxWakeOp> = snap
                        .iter()
                        .filter(|e| e.j >= win_lo && e.j <= win_hi)
                        .collect();
                    let ever_word =
                        snap.iter().filter(|e| e.uaddr == g.uaddr as u64).count();
                    if in_win.is_empty() && ever_word == 0 {
                        continue;
                    }
                    use core::fmt::Write as _;
                    let mut hdr = alloc::string::String::new();
                    let _ = write!(
                        hdr,
                        "FUXW-CTX tid={} ({}) word={:#x} birth_j={} win={}..{} in_win={} ever_word={}",
                        g.rep_tid,
                        core::str::from_utf8(&g.rep_comm).unwrap_or("").trim_end_matches('\0'),
                        g.uaddr,
                        birth,
                        win_lo,
                        win_hi,
                        in_win.len(),
                        ever_word
                    );
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!("{}", hdr);
                    for e in in_win.iter().take(12) {
                        if CTX_LINES.fetch_add(1, Relaxed) > 420 {
                            break;
                        }
                        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                        crate::pr_warn!(
                            "FUXW j={} tid={} tgid={} op={} uaddr={:#x} ret={}",
                            e.j,
                            e.tid,
                            e.tgid,
                            e.op,
                            e.uaddr,
                            e.ret
                        );
                    }
                    CTX_LINES.fetch_add(1, Relaxed);
                }
            }
        }
    }

    // ---- Phase D (48th session): FUX-GRAPH — the complete samgr park
    // graph every 6th burst (~30 s). The 8-line sample above cannot name
    // the listenerMapLock_ holder: the wedge's decisive parks (FFRT
    // workers in the samgr pid, park classes WorkerIdleAction /
    // mutexPrivate::wait / BlockUntilThreadAvailable) are drowned by
    // peripheral parks. This pass prints EVERY stuck group whose tgid
    // is the registered samgr pid, with comm and the live-word verdict,
    // so the holder is identifiable by elimination (the parked thread
    // that is NOT a waiter on the contested set). Volume: <= 24 lines
    // per 30 s on a wedged boot, zero on a healthy one (samgr parks
    // are sub-3s when healthy — the phase-A 3s bar filters them).
    static GRAPH_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let gn = GRAPH_N.fetch_add(1, Relaxed);
    if gn % 6 == 5 {
        use core::fmt::Write as _;
        let samgr_pid = crate::ipc::hotpath::binder_sareg_samgr_pid();

        // ---- FUX-MAPS (48th session): dump the samgr process's
        // executable file-backed VMAs ONCE (first graph burst). The
        // wedge anchors (park epc, fault rips, uret candidates) decode
        // against image .so files only with the guest's actual mapping
        // geometry — vma start/end + file offset fingerprints each
        // segment (readelf match offline), no path resolution needed.
        static MAPS_DONE: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        if MAPS_DONE
            .compare_exchange(0, 1, Relaxed, Relaxed)
            .is_ok()
            && samgr_pid != 0
        {
            let mut maps_n = 0usize;
            crate::sched::hotpath::for_each_task_try(|task| {
                // SAFETY: diagnostic reads under the for_each_task
                // contract.
                let tk = crate::sched::r#unsafe::task_shared(task);
                // 60th: cap 24 -> 40 — samgr's libffrt/libipc mappings
                // sort AFTER the first 24 file VMAs, and the wedge frame
                // chain (TXN-HANG-BWR ffrt-chain uret addrs) lands in
                // exactly those; without them the frames symbolize as
                // "unmapped".
                if tk.pid() != samgr_pid || maps_n >= 72 {
                    return;
                }
                if let Some(aspace) = tk.address_space() {
                    if let Some(mgr) = aspace.vma_try_read() {
                        use crate::mm::types::VmaType;
                        for vma in mgr.iter() {
                            if maps_n >= 72 {
                                break;
                            }
                            if !vma.flags().is_executable() {
                                continue;
                            }
                            if !matches!(vma.vma_type(), VmaType::FileBacked) {
                                continue;
                            }
                            // HOTPATH-PRINT: one-shot fuxstuck maps dump.
                            crate::pr_warn!(
                                "FUX-MAPS pid={} {:x}-{:x} off={:#x} fd={}",
                                samgr_pid,
                                vma.start().as_usize(),
                                vma.end().as_usize(),
                                vma.offset(),
                                vma.file_fd()
                            );
                            maps_n += 1;
                        }
                    }
                }
            });
        }
        if samgr_pid != 0 {
            // sc57: name the samgr pid's RUNNABLE threads each burst —
            // an owner stuck in a userspace spin (e.g. the TimerManager
            // spin-yield loops) never parks, so every park-based census
            // is blind to it; a persistent runnable tid is the spin
            // signature.
            {
                use core::fmt::Write as _;
                let mut run_s = alloc::string::String::new();
                let mut nf = 0u32;
                let mut ni = 0u32;
                crate::sched::hotpath::for_each_task_try(|task| {
                    // SAFETY: diagnostic reads under the for_each_task
                    // contract.
                    let tk = crate::sched::r#unsafe::task_shared(task);
                    if tk.tgid() as u32 != samgr_pid {
                        return;
                    }
                    let st_bits = tk.state().bits();
                    let run = st_bits == crate::process::types::TaskState::RUNNING
                        || tk.on_cpu()
                        || tk.sched_entity().is_on_rq();
                    if run {
                        let comm = core::str::from_utf8(tk.comm())
                            .unwrap_or("")
                            .trim_end_matches('\0');
                        let _ = write!(run_s, " {}({})", tk.pid(), comm);
                    } else if tk.ext.futex_wait_enqueued.load(Relaxed) {
                        nf += 1;
                    } else if st_bits & 0x1 != 0 {
                        ni += 1;
                    }
                });
                // HOTPATH-PRINT: fuxstuck graph-burst gate.
                crate::pr_warn!(
                    "FUX-RUN samgr_pid={} run:{} futex={} int={}",
                    samgr_pid,
                    run_s,
                    nf,
                    ni
                );
            }
            let mut graph_lines = 0usize;
            for g in groups.iter() {
                if g.tgid != samgr_pid {
                    continue;
                }
                if LINES.fetch_add(1, Relaxed) > 1200 {
                    break;
                }
                let comm_s = core::str::from_utf8(&g.rep_comm)
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let live_s = if g.rep_live_ok {
                    if g.rep_live == (g.rep_fval & 0xffff_ffff) && g.rep_fval != 0 {
                        "live==fval"
                    } else {
                        "LIVE!=fval"
                    }
                } else {
                    "live=?"
                };
                let mut u2v_s = alloc::string::String::new();
                if g.rep_u2v_ok {
                    let _ = write!(
                        u2v_s,
                        "u2v={:#x}+{:#x}{}",
                        g.rep_u2v_start,
                        g.rep_u2v_off,
                        g.rep_u2v_cls as char
                    );
                }
                // HOTPATH-PRINT: fuxstuck graph row — same rux:fuxstuck (retired debug=fuxstuck)
                // gate + 5s burst budget + LINES cap as FUX-STUCK.
                crate::pr_warn!(
                    "FUX-GRAPH tgid={} tid={} ({}) uaddr={:#x} n={} park={}s..{}s anywoken={} fval={:#x} {}={:#x} fto={:#x} A+{:#x} {} uret={}",
                    g.tgid,
                    g.rep_tid,
                    comm_s,
                    g.uaddr,
                    g.n,
                    g.min_age,
                    g.max_age,
                    g.any_woken as u32,
                    g.rep_fval,
                    live_s,
                    g.rep_live,
                    g.rep_fto,
                    g.rep_uvma_off,
                    u2v_s,
                    {
                        let mut s = alloc::string::String::new();
                        for i in 0..g.rep_uret_n {
                            let _ = write!(s, "{:#x} ", g.rep_uret[i]);
                        }
                        s
                    }
                );
                graph_lines += 1;
                // sc57: chain lines for graph rows too (the graph set
                // is the reliable samgr coverage — the 8-row sample is
                // contested by peripheral parks).
                if g.rep_chn_hops.iter().any(|&h| h != 0)
                    || g.rep_chn_prev3.iter().any(|&h| h != 0)
                {
                    let mut hops_s = alloc::string::String::new();
                    for (h, &a) in g.rep_chn_hops.iter().enumerate() {
                        if a == 0 {
                            break;
                        }
                        let _ = write!(
                            hops_s,
                            " {:#x}{}",
                            a,
                            if g.rep_chn_hops_ok & (1 << h) != 0 { "*" } else { "?" }
                        );
                    }
                    let mut prev_s = alloc::string::String::new();
                    for &a in g.rep_chn_prev3.iter() {
                        if a == 0 {
                            break;
                        }
                        let _ = write!(prev_s, " {:#x}", a);
                    }
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!(
                        "FUX-CHAIN tid={} node={:#x} task={:#x} wt={} hops:{} prev:{}",
                        g.rep_tid,
                        g.rep_chn_node,
                        g.rep_chn_task,
                        g.rep_chn_wt,
                        hops_s,
                        prev_s
                    );
                    if g.rep_chn_head != 0 {
                        let mut win_s = alloc::string::String::new();
                        for &wv in g.rep_chn_head_win.iter() {
                            let _ = write!(win_s, "{:#x} ", wv);
                        }
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!(
                            "FUX-CHAIN-HEAD tid={} head={:#x} win:{}",
                            g.rep_tid,
                            g.rep_chn_head,
                            win_s
                        );
                    }
                }
            }
            if graph_lines > 0 {
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "FUX-GRAPH-SUM samgr_pid={} groups={} (burst {})",
                    samgr_pid,
                    graph_lines,
                    gn
                );
            }
        }
    }
}

/// Allocate a waiter slot
fn alloc_waiter() -> Option<usize> {
    for i in 0..WAITER_POOL_SIZE {
        let mut slot = WAITER_POOL[i].lock_irqsave();
        if slot.is_none() {
            // Reserve INSIDE the critical section: leaving the slot empty
            // until the caller initializes it let two CPUs hand out the
            // same index (review IPC-C2). A placeholder (task == null)
            // makes the slot visibly occupied; chain walkers skip null-task
            // waiters via the key/task checks below.
            *slot = Some(Waiter {
                key: FutexKey::new(0, 0, 0),
                task: core::ptr::null_mut(),
                bitset: 0,
                woken: false,
                park_j: 0,
                bucket: 0,
                next: None,
            });
            FUX_POOL_USED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            return Some(i);
        }
    }
    // sc58: stamp the exhaustion moment — the census reports it; a
    // wedge-time pool-full means FUTEX_WAITs spin in backoff retries,
    // invisible to the bucket-walk census.
    FUX_POOL_FULL_J.store(
        crate::timer::get_jiffies(),
        core::sync::atomic::Ordering::Relaxed,
    );
    None
}

/// Free a waiter slot
fn free_waiter(index: usize) {
    let mut slot = WAITER_POOL[index].lock_irqsave();
    *slot = None;
    // sc58: occupancy counter (census reads it O(1)).
    FUX_POOL_USED.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
}

/// Back off one jiffy when the waiter pool is exhausted (see
/// futex_wait_timeout). Sleeps INTERRUPTIBLE with a one-shot timer so a
/// slot frees while we are parked; returns immediately if no task/timer
/// context is available (caller retries anyway).
fn pool_full_backoff() {
    use crate::process::types::{Task, TaskState};

    let current = match crate::sched::hotpath::current() {
        Some(c) => c as *mut Task,
        None => return,
    };
    let target = crate::timer::get_jiffies() + 1;
    let pid = crate::sched::r#unsafe::task_shared(current).pid();
    // arch-s5: typed one-shot guard — the scope-exit Drop cancels the
    // registration (the old explicit trailing del_timer).
    let Some(wake) = crate::timer::WakeArmed::arm_jiffy(target, pid) else {
        return; // no timer slot: busy retry (rare, bounded by wakers)
    };
    loop {
        if crate::timer::get_jiffies() >= target {
            break;
        }
        // Mark INTERRUPTIBLE BEFORE the final re-check (state-first
        // lost-wakeup discipline, same as sys_nanosleep).
        // SAFETY: current is the running task's pointer.
        crate::sched::r#unsafe::task_shared(current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        if crate::timer::get_jiffies() >= target {
            // SAFETY: current is the running task's pointer.
            crate::sched::r#unsafe::task_shared(current).set_state(TaskState::new(TaskState::RUNNING));
            // A racing wake may have enqueued us while still executing —
            // take ourselves back off (NEW-C2 discipline).
            // SAFETY: current is the running task's pointer.
            crate::sched::hotpath::dequeue_task(crate::sched::r#unsafe::task_shared(current));
            break;
        }
        // SAFETY: schedule() operates on the current task.
        crate::sched::hotpath::schedule();
    }
    drop(wake);
}

/// Calculate futex hash value
pub fn futex_hash(key: &FutexKey) -> usize {
    // FWK (reference get_futex_key parity): object-keyed (SHARED-VMA)
    // words hash by the PHYSICAL word identity — a parent waiting on a
    // MAP_SHARED page at vaddr A and an exec'd child waking from a fresh
    // mmap at vaddr B share a bucket (the LTP tst_checkpoint protocol;
    // waitid08). Every anon word — PRIVATE-flag op or SHARED-semantic op
    // on a PRIVATE VMA — hashes (mm, uaddr): the musl cleartid interop
    // (priv=0 waiters vs the kernel's CLONE_CHILD_CLEARTID exit wake on
    // __thread_list_lock) lands both sides in ONE bucket by
    // construction, replacing the old phys-for-all bucketing. That old
    // scheme made anon keys frame-DEPENDENT: fork re-marks the page
    // RO+COW, the next store swaps the frame, and a parked waiter keyed
    // on the old frame became unreachable to every later wake — the
    // lostwake.c group strand (same-mm wake-miss with phys differing per
    // kill cycle; 8/8 parkers lost with a live waker).
    //
    // F12: the raw `uaddr % HASH_SIZE` clustered EVERY thread-stack
    // futex into ONE bucket. musl hands out 1 MB pthread stacks with a
    // fixed stride (0x101000 — stack + guard page), which is a multiple
    // of 64, so the ctid/join futex words of all threads of a process
    // landed in the same bucket: pth_str02's 1000-thread join chain
    // serialised on a single spinlock (all 256 pool slots chained in one
    // bucket, every wake/alloc walking 256 entries under that lock).
    // Fibonacci-multiply the identity so any fixed stride spreads
    // across the buckets.
    let x = if key.object {
        (key.phys >> 2) as u64
    } else {
        // mm: Arc allocation address (≥8-aligned); uaddr: 4-aligned.
        // Rotate the mm so its low entropy bits spread before the
        // Fibonacci multiply.
        (((key.mm as u64) >> 3).rotate_left(21)) ^ ((key.uaddr as u64) >> 2)
    };
    let mixed = x.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    // Take the high bits (multiplicative hashing) modulo HASH_SIZE.
    let bits = HASH_SIZE.trailing_zeros();
    (mixed >> (64 - bits)) as usize
}

/// Wake up waiters on a futex, keyed on the CURRENT task's mm.
pub fn futex_wake(uaddr: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    let mm = current_futex_mm_id();
    futex_wake_in_mm(uaddr, mm, flags, nr_wake, bitset)
}

/// Wake up waiters on a futex keyed by an explicit mm identity.
///
/// Used by the exit path (clear_child_tid / robust list) where the waker's
/// CURRENT mm would be wrong or already dropped — the key must be the
/// EXITING task's mm.
///
/// Walks the hash chain for the given futex, waking up to `nr_wake` tasks
/// whose bitset intersects with the requested bitset.  Uses
/// `Task::wake_up()` which properly enqueues the task on the run queue
/// and triggers rescheduling on the target CPU.
pub fn futex_wake_in_mm(uaddr: usize, mm: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    if bitset == 0 {
        return -EINVAL as i64;
    }

    let key = FutexKey::new_user(uaddr, mm, flags);
    let bucket_idx = futex_hash(&key);
    #[cfg(feature = "debug-futex-trace")]
    ftx_trace(b"WAKE-IN", uaddr, mm as u64, nr_wake as u64);

    let mut ret = 0i64;
    let mut prev_idx: Option<usize> = None;
    // Collect tasks to wake after releasing the bucket lock, like
    // kernel/futex/waitwake.c wake_futex() + wake_q_add(). Dynamically
    // sized: a fixed 8-entry array silently stranded every waiter past the
    // 8th (pthread_cond_broadcast) — review IPC-C3.
    //
    // NO-ALLOC-UNDER-LOCK (oh-launch round 4): the pushes below run
    // INSIDE the bucket-lock section, and a Vec growth there allocates
    // with IRQs off — the pile-up ingredient (fdk3: 3 CPUs spun 2 s+ on
    // HASH_HEADS+0x870 at 6-8 s). The reference's bucket sections never
    // allocate (wake_q is stack-preallocated; waiters embed their list
    // nodes). The walk breaks at ret >= nr_wake, so reserving nr_wake
    // slots BEFORE the lock makes the section allocation-free by
    // construction; nr_wake <= 0 callers never reach the walk.
    let mut wake_list: alloc::vec::Vec<*mut Task> =
        // CLAMP (fc1/fc2 postmortem): libc's pthread_cond_broadcast
        // passes nr_wake=INT_MAX — reserving it verbatim asked bigalloc
        // for 0x3fffffff8 bytes, the reject threw, and the
        // alloc_error_handler SIGKILLed init at 1.8 s (2/2 boots).
        // Real wakes on one bucket are single digits; broadcasts spike
        // to tens. 1024 covers every observed shape; an overflow past
        // it merely grows the Vec (the pre-fix behavior), not a die.
        alloc::vec::Vec::with_capacity((nr_wake as usize).clamp(1, 1024));

    // Hold the hash bucket lock for the entire traversal so no
    // concurrent futex_wait can insert/remove while we walk.
    let mut head = HASH_HEADS[bucket_idx].lock_irqsave();

    let mut current_idx = head.head;
    while let Some(idx) = current_idx {
        if ret >= nr_wake as i64 {
            break;
        }

        // Slot under the bucket guard — proof-checked (D-11 slice 3).
        let mut waiter_slot = WAITER_POOL[idx].lock_nested(&head.proof());
        let should_wake = match *waiter_slot {
            Some(ref waiter) => {
                waiter.key.matches(&key) && (waiter.bitset & bitset) != 0
            }
            None => break,
        };

        if should_wake {
            let woken_task = match *waiter_slot {
                Some(ref waiter) => waiter.task,
                None => break,
            };
            let next_idx = match *waiter_slot {
                Some(ref waiter) => waiter.next,
                None => break,
            };
            // 51st session — cross-mm wake-theft alarm: a PRIVATE-keyed
            // wake consuming a waiter whose key mm differs from the
            // waker's is a key collision (phys-keyed bucketing feeding a
            // shared-keyed waiter to a private waker, or worse). The
            // stolen wake strands the true owner's waiter forever — the
            // frozen-mutex wedge shape. Rate-limited to 32 prints.
            if flags & FLAGS_SHARED == 0 && !woken_task.is_null() {
                let waiter_mm = match *waiter_slot {
                    Some(ref waiter) => waiter.key.mm,
                    None => 0,
                };
                if waiter_mm != 0 && waiter_mm != mm {
                    static THEFT_N: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let tn = THEFT_N
                        .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if tn < 32 {
                        // Read-only pid of a waiter still linked in this
                        // bucket under our lock (INV-SCE-2).
                        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                        crate::pr_warn!(
                            "FUTEX-WAKE-THEFT waker mm={:#x} consumed waiter mm={:#x} uaddr={:#x} waiter tid {}",
                            mm,
                            waiter_mm,
                            uaddr,
                            crate::sched::r#unsafe::task_shared(woken_task).pid(),
                        );
                    }
                }
            }

            // Mark woken so futex_wait knows it was explicitly woken.
            if let Some(ref mut w) = *waiter_slot {
                w.woken = true;
            }
            drop(waiter_slot);

            // Unlink from chain (62nd: maintain the FIFO tail — the
            // removed node was the tail iff its next is None).
            if prev_idx.is_none() {
                head.head = next_idx;
                if next_idx.is_none() {
                    head.tail = None;
                }
            } else if let Some(prev) = prev_idx {
                let mut prev_slot = WAITER_POOL[prev].lock_nested(&head.proof());
                if let Some(ref mut pw) = *prev_slot {
                    pw.next = next_idx;
                }
                if next_idx.is_none() {
                    head.tail = prev_idx;
                }
            }

            // Do NOT free the waiter slot here: the sleeping task still
            // inspects it after waking and frees it itself (review IPC-H4).
            // Defer wakeup — collect task pointer, wake after dropping lock.
            if !woken_task.is_null() {
                wake_list.push(woken_task);
            }

            ret += 1;
            current_idx = next_idx;
        } else {
            prev_idx = Some(idx);
            current_idx = match *waiter_slot {
                Some(ref waiter) => waiter.next,
                None => break,
            };
        }
    }

        // R12-2: wake WHILE STILL HOLDING the bucket lock — same reasoning as
        // WaitQueueHead::wake_up (R12-1): the waiter is still linked here, so
        // it cannot have passed its unlink-under-this-lock and exited. Bucket
        // -> GRQ order is safe (no GRQ-held path takes a futex bucket). The
        // old drop-then-wake window was the deferred-wake UAF.
        //
        // C8 wake-ordering invariant (waker side): for every waiter above,
        // (1) the task pointer was extracted from its entry under the slot
        // lock, (2) the `woken` flag was set to true (the slot spinlock's
        // Release store publishes it), and only then (3) Task::wake_up is
        // invoked — in that order. Reordering loses wakeups: the moment
        // wake_up runs, the sleeper is schedulable; a woken sleeper that
        // polls `woken` BEFORE the flag store returns from schedule(),
        // reads false, classifies the wake as spurious, re-checks *uaddr
        // and goes back to sleep — the later `woken = true` is a plain
        // store that schedules no one, and with no second FUTEX_WAKE the
        // waiter sleeps forever despite having been woken once.
    // timer-gap lane: every WAKE's walk outcome (rt2xdbg=1, 128 lines) —
    // uaddr, requested nr, matched/consumed ret, and the bucket chain
    // length at walk time. The pi_min wedge: did the gow WAKE even SEE
    // the parked waiters?
    if crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1") {
        static FWW: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        if FWW.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 128 {
            use crate::debug::hotpath::putc_brief as putchar;
            fn ph(mut v: u64) {
                for _ in 0..16 {
                    let n = (v >> 60) as u8;
                    putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                    v <<= 4;
                }
            }
            let mut chain = 0u64;
            let mut idx = head.head;
            while let Some(i) = idx {
                chain += 1;
                if chain > 64 {
                    break;
                }
                let s = WAITER_POOL[i].lock_irqsave();
                idx = s.as_ref().map(|w| w.next).flatten();
            }
            for &b in b"FWW uaddr=0x" { putchar(b); }
            ph(uaddr as u64);
            for &b in b" nr=" { putchar(b); }
            ph(nr_wake as u64);
            for &b in b" ret=" { putchar(b); }
            ph(ret as u64);
            for &b in b" chain=" { putchar(b); }
            ph(chain);
            putchar(b'\n');
        }
    }

        let woken_n = wake_list.len();
    // oh67: for the watched lock words, name WHO the wake consumed —
    // a wake returning ret=1 that consumes a task which never proceeds
    // names a stolen/stale waiter (the frozen-mutex wedge shape).
    let oh67_watch = {
        let a = FUXWATCH_ADDR.load(Relaxed2::Relaxed);
        let b2 = FUXWATCH2_ADDR.load(Relaxed2::Relaxed);
        uaddr as u64 == a || uaddr as u64 == b2
    };
    for task in wake_list {
        if !task.is_null() {
            if oh67_watch {
                static WOKE_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if WOKE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 64 {
                    // SAFETY: the task is linked in this bucket under our
                    // lock; cannot be freed while we hold it.
                    let tk = crate::sched::r#unsafe::task_shared(task);
                    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                    crate::pr_warn!(
                        "FUXW-WOKE uaddr={:#x} woken tid={} tgid={} state={:#x}",
                        uaddr,
                        tk.pid(),
                        tk.tgid(),
                        tk.state().bits()
                    );
                }
            }
            // R12-2 wake-under-bucket, now compile-checked: the proof
            // minted by the bucket guard is what the nested GRQ acquire
            // requires (INV-LOCK-5 forward; D-11).
            Task::wake_up_in_futex_bucket(task, &head.proof());
        }
    }
    drop(head);

    // oh67: the lost-notify probe — a FUTEX_WAKE that found NO waiter
    // (ret=0) on a THREAD-STACK word is either a benign timeout-canceled
    // notify or a LOST wake: the notifier's seq bump + wake landed while
    // the waiter was en route to park, or the waiter's node address was
    // stale. musl's cond protocol closes the first window in userspace;
    // any residual ret=0 on a word whose waiter stays parked is the
    // wedge's lost edge. Budget 240/boot, fuxstuck-gated; correlate
    // offline against the stuck-park births (FUX-CTX groups by word).
    if ret == 0
        && (0x7ff0_8000_0000..=0x7ff0_9fff_ffff).contains(&uaddr)
        && crate::trace::EVENT_FUXSTUCK.armed()
    {
        static MISS0_N: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        static MISS0_DUP: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = MISS0_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 240 {
            let waker = crate::sched::hotpath::current().map(|t| {
                (t.pid(), t.tgid() as u32)
            });
            // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
            crate::pr_warn!("FUXW-MISS0 #{} uaddr={:#x} waker={:?}", n, uaddr, waker);
        } else {
            MISS0_DUP.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    }

    #[cfg(feature = "debug-futex-trace")]
    ftx_trace(b"WAKE-OUT", uaddr, ret as u64, woken_n as u64);

    // b133 hunt (rux:syscalltrace (retired debug=syscalltrace)): OH's samgr/SA threads park forever in
    // PRIVATE futex_wait inside musl's ld.so malloc locks — 120s+ binder
    // stalls queue the whole display chain behind them. Wake/wait protocol
    // is documented-solid, so the prime suspect is a KEY MISMATCH: a
    // waiter linked under a different (uaddr, mm) than this waker
    // computes. When a wake wanted more waiters than it found, scan every
    // bucket for same-uaddr entries whose key does NOT match, and report
    // both identities. Gated + rate-limited: the scan walks all buckets,
    // and uncontended FUTEX_WAKEs (every pthread_mutex_unlock) must not
    // pay for it.
    if ret < nr_wake as i64
        && crate::trace::EVENT_FUTEX_MISS.armed()
    {
        static MISS_REPORTS: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let mut waker_pid = 0u32;
        if let Some(t) = crate::sched::hotpath::current() {
            // SAFETY: sched::current() yields the current task for the
            // duration of this syscall.
            waker_pid = crate::sched::r#unsafe::task_shared(t).pid();
        }
        for (bi, b) in HASH_HEADS.iter().enumerate() {
            let mut h = b.lock_irqsave();
            let mut idx = h.head;
            while let Some(i) = idx {
                // SAFETY: the slot index came from a chain link under the
                // bucket lock; slots are freed only under both locks.
                let (task, next, mismatch, w_phys) = {
                    let slot = WAITER_POOL[i].lock_irqsave();
                    match *slot {
                        Some(ref w) => {
                            let mismatch = w.key.uaddr == uaddr
                                && (!w.key.matches(&key) || (w.bitset & bitset) == 0);
                            (w.task, w.next, mismatch, w.key.phys)
                        }
                        None => (core::ptr::null_mut(), None, false, 0),
                    }
                };
                if mismatch {
                    let n = MISS_REPORTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if n < 32 {
                        let mut wcomm = [0u8; 16];
                        if !task.is_null() {
                            // SAFETY: the task is linked in the bucket; it
                            // cannot be freed while we hold the lock.
                            let c = crate::sched::r#unsafe::task_shared(task).comm();
                            wcomm.copy_from_slice(c);
                        }
                        let name = core::str::from_utf8(
                            &wcomm[..wcomm.iter().position(|&c| c == 0).unwrap_or(16)],
                        )
                        .unwrap_or("?");
                        let wmm = {
                            // SAFETY: diagnostic identity read.
                            task_futex_mm_id(task)
                        };
                        // FWK (futex-lostwake lane): phys on BOTH sides — a
                        // COW frame swap between the waiter's park and this
                        // wake shows waiter-phys != waker-phys with the same
                        // uaddr: the phys-keyed bucket moved and the wake
                        // scanned the waker's bucket only.
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!(
                            "futex-wake-miss: uaddr={:#x} waker pid {} key(mm={:#x},shared={},phys={:#x}) found waiter pid-key mm={:#x} phys={:#x} bucket {} waiter-comm {}",
                            uaddr,
                            waker_pid,
                            mm,
                            flags & FLAGS_SHARED != 0,
                            key.phys,
                            wmm,
                            w_phys,
                            bi,
                            name
                        );
                        // debug-migrate: the hit also records into the
                        // unified framework (rux:futex_miss carries the
                        // two mm identities — the lost-wake signature).
                        crate::trace::trace_futex_miss(uaddr as u64, mm as u64, wmm as u64, 1);
                    }
                }
                idx = next;
            }
            drop(h);
        }
    }

    ret
}

/// Wait for a futex
///
/// Checks `*uaddr` under the hash bucket lock.  If it still equals `val`,
/// inserts a waiter into the chain, sets state to INTERRUPTIBLE (still under
/// the lock), then drops the lock and schedules.  The "insert + set state
/// under lock" ordering prevents the lost-wakeup race: by the time
/// futex_wake sees the waiter, the task is already in INTERRUPTIBLE state
/// so `Task::wake_up()` (which checks `is_sleeping()`) can succeed.
pub fn futex_wait(uaddr: usize, flags: u32, val: u32, bitset: u32) -> i64 {
    futex_wait_timeout(uaddr, flags, val, bitset, None)
}

/// FUTEX_WAIT with an optional jiffies deadline (see do_futex).
pub fn futex_wait_timeout(uaddr: usize, flags: u32, val: u32, bitset: u32, deadline: Option<u64>) -> i64 {
    if bitset == 0 {
        return -EINVAL as i64;
    }

    let uaddr_ptr = uaddr as *const AtomicU32;
    if uaddr_ptr.is_null() {
        return -EINVAL as i64;
    }

    let current = match crate::sched::hotpath::current() {
        Some(t) => t,
        None => return -EFAULT as i64,
    };
    // SAFETY: current is the current task's raw pointer from sched::current(),
    // valid for the duration of this syscall.
    let mm = task_futex_mm_id(current);

    let key = FutexKey::new_user(uaddr, mm, flags);
    let bucket_idx = futex_hash(&key);

    // Linux futex_wait semantics: success (0) is returned ONLY when
    // FUTEX_WAKE (or a requeue) explicitly woke us. A spurious wakeup
    // (blocked-signal wake, timer noise) must RE-CHECK the value and go
    // back to sleep — returning 0 here let LTP's checkpoint handshake
    // (sighold02: child holds all signals, parent kills 60 of them, then
    // FUTEX_WAKEs) complete the wait early: the parent's wake found no
    // waiter and TBROK'd with ETIMEDOUT.
    loop {
        // Lock the hash bucket.  All subsequent operations (value check,
        // waiter insertion, state change) happen under this lock.
        let mut head = HASH_HEADS[bucket_idx].lock_irqsave();

        // Re-check value under lock (prevents lost wakeup).
        // SAFETY: get_user goes through the exception-table copy path; an
        // unmapped user address yields EFAULT instead of a kernel page fault.
        let uval = match crate::sync::r#unsafe::word_read(uaddr_ptr as usize) {
            Some(v) => v,
            None => return -EFAULT as i64,
        };
        #[cfg(feature = "debug-futex-trace")]
        ftx_trace(b"WAIT-VAL", uaddr, uval as u64, val as u64);
        if uval != val {
            return -EAGAIN as i64;
        }

        // Deadline already passed while we were re-checking?
        // 63rd: deadlines are absolute monotonic ns now (was: jiffies).
        if let Some(dl) = deadline {
            if crate::timer::monotonic_ns_since_boot() >= dl {
                return -ETIMEDOUT as i64;
            }
        }

        // Allocate waiter slot.
        //
        // F12: a full pool must NEVER fail the wait with ENOMEM. Linux
        // parks futex waiters on a futex_q living on the CALLER'S KERNEL
        // STACK (futex_wait_queue), so a futex wait cannot fail for
        // capacity reasons, and userspace relies on that: musl's join /
        // __timedwait loops only exit on ETIMEDOUT/EINVAL and blindly
        // re-issue FUTEX_WAIT on any other error. With ENOMEM here,
        // pth_str02's 1000-thread join chain live-locked — every joiner
        // spinning lock-acquire → full-pool scan → ENOMEM, starving the
        // exit-path wakes that would have freed slots. Instead: drop the
        // bucket lock, back off one jiffy, and retry the whole sequence
        // (value re-check included). Slots free as wakes progress; the
        // exit path (ctid wake) never needs a slot, so forward progress
        // is guaranteed.
        let waiter_idx = match alloc_waiter() {
            Some(idx) => idx,
            None => {
                drop(head);
                pool_full_backoff();
                continue;
            }
        };

        // Initialize and insert waiter into hash chain (fill the placeholder
        // reserved by alloc_waiter in place). 62nd: TAIL append — FIFO
        // waiter order, reference plist parity (see FutexChain). The
        // OLDEST waiter must receive the first wake of every burst; the
        // head-insert (LIFO) starved the long-parked M2 holder behind
        // the samgr workers' transient re-parkers (ohw27/28).
        {
            let mut slot = WAITER_POOL[waiter_idx].lock_irqsave();
            if let Some(ref mut w) = *slot {
                w.key = key;
                w.task = current;
                w.bitset = bitset;
                w.woken = false;
                w.park_j = crate::timer::get_jiffies();
                w.bucket = bucket_idx;
                w.next = None;
            }
        }

        // Append at the chain tail.
        match head.tail {
            Some(t) => {
                let mut tslot = WAITER_POOL[t].lock_nested(&head.proof());
                if let Some(ref mut tw) = *tslot {
                    tw.next = Some(waiter_idx);
                }
                head.tail = Some(waiter_idx);
            }
            None => {
                head.head = Some(waiter_idx);
                head.tail = Some(waiter_idx);
            }
        }

        // Residency flag (see Task::futex_wait_enqueued): linked into the
        // chain here, cleared by every unlink/free path — do_exit's
        // futex_cleanup gates its 512-bucket scan on it. We still hold the
        // bucket lock; the flag is only read by this task's own exit path,
        // so a plain store suffices.
        crate::sched::r#unsafe::task_shared(current).ext.futex_wait_enqueued.store(true, core::sync::atomic::Ordering::Release);

        // oh67: name WHO parks on the watched lock words (the frozen-mutex
        // wedge: a waiter parked on M2.l that tid343-class unlocks keep
        // consuming without the web progressing).
        {
            let a = FUXWATCH_ADDR.load(Relaxed2::Relaxed);
            let b2 = FUXWATCH2_ADDR.load(Relaxed2::Relaxed);
            if uaddr as u64 == a || uaddr as u64 == b2 {
                static PARK_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if PARK_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 64 {
                    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                    crate::pr_warn!(
                        "FUXW-PARK uaddr={:#x} tid={} tgid={} val={:#x}",
                        uaddr,
                        current.pid(),
                        current.tgid(),
                        val
                    );
                }
            }
        }

        // C8 wake-ordering invariant (sleeper side): in order — (1) our
        // waiter entry (task pointer, the flag the waker polls) is linked
        // into the bucket chain, (2) the task state is stored as
        // INTERRUPTIBLE with Release, both strictly BEFORE (3) the bucket
        // lock is dropped and schedule() runs. The waker (futex_wake /
        // futex_requeue) takes the same lock, so it can never observe the
        // entry without also observing the sleeping state — Task::wake_up
        // drops every wake of a non-sleeping task. Reordering loses
        // wakeups: linked-but-RUNNING lets the waker consume the wake
        // (entry marked woken, task never enqueued), and our later state
        // store + schedule() then sleep forever; symmetrically, the
        // post-schedule `woken` poll below must come AFTER the waker's
        // flag store, which the waker guarantees by setting `woken`
        // before it invokes Task::wake_up (see futex_wake_in_mm).
        //
        // Set task state to INTERRUPTIBLE while still holding the hash lock.
        // This guarantees that any futex_wake that sees the waiter in the chain
        // will also see the task in INTERRUPTIBLE state, preventing the
        // lost-wakeup race.
        // SAFETY: current is the current task, valid for the duration of this
        // function.  We hold the hash bucket lock so futex_wake will see the
        // state transition before checking is_sleeping().
        {
            // INV-SCE-2: current is the parking task itself; the bucket
            // lock holds the chain stable (see the comment above).
            let current_t = crate::sched::r#unsafe::task_shared(current);
            // timer-gap lane: the park edge for cpu0-pinned FIFO tasks
            // (rt2xdbg=1, 64 lines) — the pi_min wedge question is
            // whether the waiter ever parks on the word the waker walks.
            if crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1")
                && current_t.cpus_allowed() == 1
            {
                static FWT: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if FWT.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 64 {
                    use crate::debug::hotpath::putc_brief as putchar;
                    fn ph(mut v: u64) {
                        for _ in 0..16 {
                            let n = (v >> 60) as u8;
                            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                            v <<= 4;
                        }
                    }
                    for &b in b"FWT-PARK pid=0x" { putchar(b); }
                    ph(current_t.pid() as u64);
                    for &b in b" uaddr=0x" { putchar(b); }
                    ph(uaddr as u64);
                    for &b in b" val=0x" { putchar(b); }
                    ph(uval as u64);
                    putchar(b'\n');
                }
            }
            current_t.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }

        // Release the hash bucket lock.  The Release semantics ensure that the
        // waiter entry (chain + INTERRUPTIBLE state) is visible to other
        // CPUs before they can observe the lock is free.
        drop(head);

        // Schedule — yields the CPU.  The task will be re-enqueued by
        // Task::wake_up() when futex_wake (or a signal) wakes it.  Arm a
        // wakeup timer when a deadline is set: nothing else would wake a
        // futex that is never signaled (pthread_cond_timedwait would hang).
        crate::arch::cpu::restore_irq(true);
        // 63rd: HIGH-RESOLUTION wake at the precise ns deadline — the
        // jiffy-grid arm fired every sub-10 ms FFRT timed wait at the
        // next 10 ms tick (reference hrtimer parity; same discipline as
        // nanosleep/sigtimedwait). The jiffy value is wheel bookkeeping
        // only (jiffies_covering_deadline ceils so the wheel entry never
        // precedes the hres expiry); drop(wake) below deletes the timer
        // after schedule() returns.
        let wake = deadline.map(|dl| {
            let id = crate::timer::add_timer_wakeup_hres(
                crate::syscall::support::jiffies_covering_deadline(dl),
                crate::syscall::support::deadline_ns_to_hres_ticks(dl),
                crate::sched::hotpath::get_current_pid(),
            );
            crate::drivers::hotpath::timer::rearm_for_hres();
            id
        });
        // R32 (NEW-3): if a deadline was requested but the timer pool is
        // exhausted (arm → 0), schedule() below would sleep
        // FOREVER — nothing else wakes an un-signaled futex, so the timed
        // wait degenerated into an untimed hang. Unlink and fail instead.
        if deadline.is_some() && wake == Some(0) {
            remove_waiter(bucket_idx, waiter_idx);
            return -ENOMEM as i64;
        }
        crate::sched::hotpath::schedule();
        if let Some(id) = wake {
            if id != 0 {
                crate::timer::del_timer(id);
            }
        }
        #[cfg(feature = "debug-futex-trace")]
        ftx_trace(b"WAIT-RET", uaddr, val as u64, 0);

        // The waiter's bucket may have changed while we slept (FUTEX_REQUEUE
        // moved us to uaddr2's bucket). Re-read it from the slot so the
        // removal below hits the right chain.
        let live_bucket = {
            let slot = WAITER_POOL[waiter_idx].lock_irqsave();
            slot.as_ref().map(|w| w.bucket).unwrap_or(bucket_idx)
        };

        // Check for signal interruption (EINTR). Ownership guard: only act on
        // the slot if it still belongs to us (task pointer matches).
        {
            let slot = WAITER_POOL[waiter_idx].lock_irqsave();
            let mine = slot.as_ref().map(|w| w.task == current).unwrap_or(false);
            drop(slot);
            if !mine {
                // Slot was recycled underneath us (should not happen now that
                // the waker never frees slots, but stay defensive).
                return 0;
            }
            if crate::signal::signal_pending() {
                let woken = {
                    let slot = WAITER_POOL[waiter_idx].lock_irqsave();
                    slot.as_ref().map(|w| w.woken).unwrap_or(false)
                };
                if !woken {
                    remove_waiter(live_bucket, waiter_idx);
                    return -crate::errno::constants::EINTR as i64;
                }
            }
        }

        // After waking up, check if we were explicitly woken. The waiter owns
        // its slot: unlink paths that did not wake us leave it in the chain
        // (remove), the waker unlinked it already (just free the slot).
        let mut was_spurious = false;
        {
            let mut slot = WAITER_POOL[waiter_idx].lock_irqsave();
            let mine = slot.as_ref().map(|w| w.task == current).unwrap_or(false);
            if mine {
                let was_woken = slot.as_ref().map(|w| w.woken).unwrap_or(false);
                if !was_woken {
                    // Not explicitly woken (spurious wakeup or the timeout
                    // timer): still in the chain.
                    drop(slot);
                    remove_waiter(live_bucket, waiter_idx);
                    // Timeout semantics (review 2R.9): if we were not woken and
                    // the deadline has passed, this is a genuine ETIMEDOUT —
                    // returning success here broke every timed waiter.
                    // 63rd: absolute monotonic-ns deadlines.
                    if let Some(dl) = deadline {
                        if crate::timer::monotonic_ns_since_boot() >= dl {
                            return -ETIMEDOUT as i64;
                        }
                    }
                    // Spurious: loop — re-check the value and sleep again.
                    was_spurious = true;
                } else {
                    // Woken: the waker unlinked us from the chain and left the
                    // slot for us to free.
                    *slot = None;
                    // sc58: occupancy counter (paired with alloc_waiter).
                    FUX_POOL_USED.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
                    // Residency flag (see Task::futex_wait_enqueued): the
                    // waker's unlink left the flag set for us to clear here.
                    crate::sched::r#unsafe::task_shared(current).ext.futex_wait_enqueued.store(false, core::sync::atomic::Ordering::Release);
                }
            }
        }
        if !was_spurious {
            return 0;
        }
        // else: continue the loop (Linux futex_wait re-sleeps).
    }
}

/// Remove waiter from hash chain.
///
/// Reference parity (kernel/futex/core.c futex_unqueue): the caller read
/// `w.bucket` WITHOUT any bucket lock, and FUTEX_REQUEUE can relink a
/// sleeping waiter into a different bucket's chain between that read and
/// the lock acquisition here. Re-read the waiter's CURRENT bucket under
/// the acquired bucket lock and retry with the fresh one. The invariant
/// exploited: `w.bucket` names the chain that contains the node, the
/// requeue updates both under BOTH bucket locks, and once we hold the
/// lock named by a fresh read, no requeue can move the node before we
/// unlink it (it would need the lock we hold).
///
/// Without this, a remove that raced a requeue walked the STALE chain,
/// found nothing, and silently returned — leaving the waiter linked in
/// the NEW chain while its task returned to userspace. The stale entry
/// then ate later FUTEX_WAKEs on the requeue target (the missed-wakeup
/// shape the FFRT mutex-word park census flagged): the wake's budget was
/// consumed unlinking a ghost whose task was long gone.
fn remove_waiter(bucket_idx: usize, target_idx: usize) {
    let mut b = bucket_idx;
    loop {
        let mut head = HASH_HEADS[b].lock_irqsave();

        // Revalidate under the bucket lock (futex_unqueue's lock_ptr
        // recheck-retry). Slot lock serializes against requeue's relink
        // writes to the same fields.
        let live = {
            let slot = WAITER_POOL[target_idx].lock_irqsave();
            match *slot {
                Some(ref w) if !w.task.is_null() => w.bucket,
                // Slot already freed or recycled underneath us: nothing
                // to remove (reference: q->lock_ptr == NULL case).
                _ => return,
            }
        };
        if live != b {
            drop(head);
            b = live;
            continue;
        }

        if head.head == Some(target_idx) {
            let next = {
                let slot = WAITER_POOL[target_idx].lock_irqsave();
                let n = slot.as_ref().and_then(|w| w.next);
                // Residency flag: this unlink takes the waiter out of every
                // chain (see Task::futex_wait_enqueued).
                if let Some(w) = slot.as_ref() {
                    if !w.task.is_null() {
                        crate::sched::r#unsafe::task_shared(w.task).ext.futex_wait_enqueued.store(false, core::sync::atomic::Ordering::Release);
                    }
                }
                n
            };
            head.head = next;
            if next.is_none() {
                // the removed head was also the tail
                head.tail = None;
            }
            free_waiter(target_idx);
            return;
        }

        let mut current_idx = head.head;
        let mut found = false;
        while let Some(idx) = current_idx {
            let next = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                slot.as_ref().and_then(|w| w.next)
            };

            if next == Some(target_idx) {
                let target_next = {
                    let target_slot = WAITER_POOL[target_idx].lock_irqsave();
                    let n = target_slot.as_ref().and_then(|w| w.next);
                    if let Some(w) = target_slot.as_ref() {
                        if !w.task.is_null() {
                            crate::sched::r#unsafe::task_shared(w.task).ext.futex_wait_enqueued.store(false, core::sync::atomic::Ordering::Release);
                        }
                    }
                    n
                };
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot {
                        w.next = target_next;
                    }
                }
                if target_next.is_none() {
                    // the removed node was the tail: prev becomes the tail
                    head.tail = Some(idx);
                }
                free_waiter(target_idx);
                found = true;
                break;
            }

            current_idx = next;
        }
        if !found {
            // Unreachable while the invariants hold (we held the bucket
            // the waiter itself says it is linked into): surface it
            // rather than silently leaking the slot, but do not spin.
            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
            crate::pr_warn!(
                "futex-remove-miss: slot {} bucket {} not in chain",
                target_idx,
                b
            );
        }
        return;
    }
}

/// Clean up all futex waiters for a given task.
///
/// Called from `do_exit` so that no dangling waiter entries remain in
/// the hash chains after the task is freed.  Wakes the task (so it can
/// continue exiting) and frees all its waiter slots.
pub fn futex_cleanup(task: *mut Task) {
    if task.is_null() {
        return;
    }

    // Fast path (perf-mm lane, 2026-10-31): the residency flag is clear
    // unless this task died with a waiter entry still linked (kill raced
    // the sleeper-side unlink). The common exit — never parked on a
    // futex, or the wait returned and unlinked itself — skips the whole
    // 512-bucket scan (one lock_irqsave pair per bucket per exit, the
    // dominant cost of the exit "rest" block: measured ~50-80us of the
    // ~90us block under TCG).
    // SAFETY: do_exit calls this in the task's own context; no other CPU
    // stores the flag for this task concurrently.
    if !crate::sched::r#unsafe::task_shared(task).ext.futex_wait_enqueued.load(core::sync::atomic::Ordering::Acquire) {
        return;
    }

    for bucket_idx in 0..HASH_SIZE {
        let mut head = HASH_HEADS[bucket_idx].lock_irqsave();
        let mut prev_idx: Option<usize> = None;
        let mut current_idx = head.head;

        while let Some(idx) = current_idx {
            // Match on the task POINTER (unique per waiter). The old pid
            // match broke once threads shared a tgid side of the key.
            let remove = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                slot.as_ref().map_or(false, |w| w.task == task)
            };

            if remove {
                // Unlink from chain.
                let next = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().and_then(|w| w.next)
                };
                if prev_idx.is_none() {
                    head.head = next;
                    if next.is_none() {
                        head.tail = None;
                    }
                } else if let Some(prev) = prev_idx {
                    let mut prev_slot = WAITER_POOL[prev].lock_irqsave();
                    if let Some(ref mut pw) = *prev_slot {
                        pw.next = next;
                    }
                    if next.is_none() {
                        head.tail = prev_idx;
                    }
                }
                free_waiter(idx);
                current_idx = next;
            } else {
                prev_idx = Some(idx);
                current_idx = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().and_then(|w| w.next)
                };
            }
        }
        // Release bucket lock here (end of iteration — `head` dropped on next loop or at end).
        drop(head);
    }

    // Wake the task so it can continue the exit path — done outside all bucket locks.
    // C8: no `woken` flag is needed here (unlike futex_wake) because the
    // target never re-sleeps: do_exit calls this after the task's last
    // schedule(), so there is no sleeper-side poll that a reordered wake
    // could strand.
    // Residency flag: the scan above removed every chained entry of this
    // task — the flag is now false for good (this is the task's own exit
    // context; no new wait can start).
    // SAFETY: same context argument as the load above.
    crate::sched::r#unsafe::task_shared(task).ext.futex_wait_enqueued.store(false, core::sync::atomic::Ordering::Release);
    Task::wake_up(task);
}

/// ENOMEM
const ENOMEM: i32 = 12;

/// FUTEX_WAIT_BITSET implementation
pub fn futex_wait_bitset(uaddr: usize, flags: u32, val: u32, _timeout: u64, bitset: u32, deadline: Option<u64>) -> i64 {
    futex_wait_timeout(uaddr, flags, val, bitset, deadline)
}

/// Parse the futex ABI `struct timespec *timeout` (raw user pointer) into a
/// jiffies deadline.
///
/// - `Ok(None)`: NULL timeout — wait forever.
/// - `Ok(Some(dl))`: parsed deadline.
/// - `Err(EFAULT)`: non-NULL but unreadable / invalid timespec. Linux
///   returns EFAULT instead of silently waiting forever.
///
/// `absolute`: FUTEX_WAIT_BITSET (and FUTEX_WAIT|FUTEX_CLOCK_REALTIME) pass
/// an absolute timespec; CLOCK_REALTIME here counts from boot (CLINT cycles
/// / TIMER_CLOCK_FREQ_HZ) and so does jiffies, so the conversion needs no
/// offset. Plain FUTEX_WAIT passes a relative duration (round 6 MED: was
/// always relative, so every pthread_cond_timedwait fired instantly or
/// never).
/// Parse a futex timeout into an ABSOLUTE monotonic-ns deadline.
///
/// 63rd session (strand-wall/wedge hunt): the old conversion quantized
/// every timed futex wait onto the jiffy grid (HZ=100 → 10 ms) —
/// `(nsec*HZ)/1e9` TRUNCATED and a `.max(1)` floor made every sub-10 ms
/// FFRT timed wait (SerialQueue::Pull's cond_.wait_for(delay),
/// musl cv timeouts) fire at exactly the next 10 ms tick, and waits of
/// 10-20 ms could fire a full grid step EARLY. The reference arms
/// futex timeouts on hrtimers (ns resolution, never early); musl in
/// the OH image parks cv waiters via plain relative FUTEX_WAIT (op
 /// 0x80 in every FUX-STUCK row), so this path carries EVERY timed
/// pthread synchronization edge. Timed waits now convert exactly:
/// relative = now_ns + spec_ns (ns-exact, never early); WAIT_BITSET
/// keeps Linux's CLOCK_MONOTONIC-absolute default (the value is
/// boot-relative ns — same timebase as monotonic_ns_since_boot), and
/// FUTEX_CLOCK_REALTIME absolute values are rebased through the wall
/// epoch offset (the old jiffy arithmetic compared 1970-epoch seconds
/// against boot jiffies — a 2026-epoch deadline read as ~365 days of
/// monotonic wait and never expired; the ohw30 census caught exactly
/// that shape, fto=31535994s).
pub fn futex_parse_timeout_ns(
    timeout_ptr: u64,
    absolute: bool,
    clock_realtime: bool,
) -> Result<Option<u64>, i32> {
    if timeout_ptr == 0 {
        return Ok(None);
    }
    if !crate::arch::uaccess::access_ok(timeout_ptr as usize, 16) {
        return Err(EFAULT);
    }
    let mut buf = [0u8; 16];
    // SAFETY: access_ok-validated user pointer; exception-table copy.
    let uncopied =
        crate::sync::r#unsafe::copy_from_user_bytes(buf.as_mut_ptr(), timeout_ptr as usize, 16);
    if uncopied > 0 {
        return Err(EFAULT);
    }
    let sec = i64::from_le_bytes(buf[0..8].try_into().unwrap());
    let nsec = i64::from_le_bytes(buf[8..16].try_into().unwrap());
    if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
        return Err(EINVAL);
    }
    let spec_ns = (sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64);
    if !absolute {
        // Relative (FUTEX_WAIT, futex(2) WAIT): Linux futex_init_timeout
        // adds ktime_get() regardless of FUTEX_CLOCK_REALTIME.
        return Ok(Some(
            crate::timer::monotonic_ns_since_boot().saturating_add(spec_ns),
        ));
    }
    if clock_realtime {
        // WAIT_BITSET + FUTEX_CLOCK_REALTIME: absolute against the
        // realtime clock — rebase onto the monotonic timebase the wake
        // timers use. A deadline already in the past saturates to 0 and
        // expires on the next check (Linux returns ETIMEDOUT promptly).
        let wall = crate::timer::wall_epoch_offset_ns() as u128;
        let spec = spec_ns as u128;
        Ok(Some(spec.saturating_sub(wall) as u64))
    } else {
        // WAIT_BITSET default: absolute CLOCK_MONOTONIC — the timespec
        // is already boot-relative ns.
        Ok(Some(spec_ns))
    }
}

/// FUTEX_WAKE_BITSET implementation
pub fn futex_wake_bitset(uaddr: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    futex_wake(uaddr, flags, nr_wake, bitset)
}

/// Convert FUTEX opcode to internal flags
pub fn futex_to_flags(op: u32) -> u32 {
    let mut flags = 0u32;

    if (op & FUTEX_PRIVATE_FLAG as u32) == 0 {
        flags |= FLAGS_SHARED;
    }

    if (op & FUTEX_CLOCK_REALTIME as u32) != 0 {
        flags |= FLAGS_CLOCKRT;
    }

    flags
}

/// FUTEX_REQUEUE / FUTEX_CMP_REQUEUE implementation.
///
/// Reference shape (kernel/futex/requeue.c futex_requeue, non-PI path):
/// - the cmpval check reads `*uaddr` UNDER the bucket lock(s), so the
///   "generation unchanged" verdict is atomic with the chain migration
///   (EAGAIN is decided while holding the same locks the waiters queue
///   under — our futex_wait also re-checks the value under its bucket
///   lock, which is what makes the pair airtight);
/// - the scan wakes up to `nr_wake` matched waiters (unlink + flag under
///   the locks) and requeues up to `nr_requeue` more by unlinking them
///   from uaddr's chain and linking them into uaddr2's chain, all under
///   BOTH bucket locks — a requeued waiter is never observable in limbo;
/// - requeued waiters are appended at the TAIL of the destination chain
///   (reference requeue_futex's plist_add: FIFO within the priority
///   class — a requeued waiter must not overtake waiters already queued
///   on uaddr2);
/// - woken tasks are woken while the bucket guards are still held
///   (R12-2, same invariant as futex_wake_in_mm: the deferred
///   wake-after-unlock used to leave a UAF window where a signal-roused
///   waiter returned, exited and was swept before Task::wake_up ran).
///
/// Returns the total number of waiters woken + requeued, or a negative errno.
pub fn futex_requeue(
    uaddr: usize,
    flags: u32,
    nr_wake: i32,
    nr_requeue: i32,
    uaddr2: usize,
    cmpval: u32,
    is_cmp: bool,
) -> i64 {
    let mm = current_futex_mm_id();

    let key1 = FutexKey::new_user(uaddr, mm, flags);

    // Degenerate shapes (dispatch already rejects uaddr2 == 0, but this
    // fn is reachable from other callers): same key or nothing to requeue
    // still runs the cmpval check under the bucket lock first — the
    // reference does not shortcut to futex_wake, so CMP_REQUEUE with a
    // stale value must fail with EAGAIN instead of waking.
    if uaddr2 == 0 || uaddr2 == uaddr || nr_requeue <= 0 {
        let mut head1 = HASH_HEADS[futex_hash(&key1)].lock_irqsave();
        if is_cmp {
            // SAFETY: exception-table protected read; unmapped → EFAULT.
            match crate::sync::r#unsafe::word_read(uaddr) {
                Some(uval) if uval == cmpval => {}
                Some(_) => return -EAGAIN as i64,
                None => return -EFAULT as i64,
            }
        }
        drop(head1);
        return futex_wake(uaddr, flags, nr_wake, FUTEX_BITSET_MATCH_ANY);
    }

    let key2 = FutexKey::new_user(uaddr2, mm, flags);
    let bucket1 = futex_hash(&key1);
    let bucket2 = futex_hash(&key2);

    let mut ret = 0i64;
    let mut woken = 0i32;
    let mut requeued = 0i32;

    // Collect tasks to wake. Dynamically sized: fixed 8/32-entry arrays
    // silently stranded waiters past their capacity (pthread_cond_broadcast)
    // — review IPC-C3/M7.
    //
    // NO-ALLOC-UNDER-LOCK (oh-launch round 4): pushes run inside the
    // (double) bucket-lock sections and are bounded by the walk's
    // woken < nr_wake break — reserve that capacity BEFORE the locks so
    // the sections never allocate (the reference's wake_q discipline;
    // see futex_wake_in_mm's note for the pile-up this closes).
    let mut wake_list: alloc::vec::Vec<*mut Task> =
        // CLAMP (fc1/fc2 postmortem): libc's pthread_cond_broadcast
        // passes nr_wake=INT_MAX — reserving it verbatim asked bigalloc
        // for 0x3fffffff8 bytes, the reject threw, and the
        // alloc_error_handler SIGKILLed init at 1.8 s (2/2 boots).
        // Real wakes on one bucket are single digits; broadcasts spike
        // to tens. 1024 covers every observed shape; an overflow past
        // it merely grows the Vec (the pre-fix behavior), not a die.
        alloc::vec::Vec::with_capacity((nr_wake as usize).clamp(1, 1024));

    // Lock both buckets to prevent requeued entries from being in limbo.
    // Use index ordering (lower first) to avoid ABBA deadlock — same
    // ordering as the reference's double_lock_hb on bucket address.
    if bucket1 == bucket2 {
        // Same bucket — lock once (reference double_lock_hb takes hb1 only
        // when hb1 == hb2).
        let mut head1 = HASH_HEADS[bucket1].lock_irqsave();

        // CMP_REQUEUE: verify *uaddr == cmpval UNDER the bucket lock
        // (reference futex_get_value_locked + EAGAIN at out_unlock).
        if is_cmp {
            // SAFETY: exception-table protected read; unmapped → EFAULT.
            match crate::sync::r#unsafe::word_read(uaddr) {
                Some(uval) if uval == cmpval => {}
                Some(_) => return -EAGAIN as i64,
                None => return -EFAULT as i64,
            }
        }

        let mut prev: Option<usize> = None;
        let mut cur = head1.head;

        while let Some(idx) = cur {
            let (matches, next) = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                match *slot {
                    Some(ref w) => (w.key.matches(&key1), w.next),
                    None => break,
                }
            };

            if !matches {
                prev = Some(idx);
                cur = next;
                continue;
            }

            if woken < nr_wake {
                let task = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().map(|w| w.task).unwrap_or(core::ptr::null_mut())
                };
                if prev.is_none() {
                    head1.head = next;
                    if next.is_none() {
                        head1.tail = None;
                    }
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
                    if next.is_none() {
                        head1.tail = prev;
                    }
                }
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot { w.woken = true; }
                }
                // The woken waiter frees its own slot (see futex_wait).

                if !task.is_null() {
                    wake_list.push(task);
                }
                woken += 1;
                ret += 1;
                cur = next;
            } else if requeued < nr_requeue {
                // Same bucket: the reference's requeue_futex only updates
                // the key when hb1 == hb2 — the node keeps its position.
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot {
                        w.key = key2;
                        w.bucket = bucket2;
                    }
                }
                requeued += 1;
                ret += 1;
                prev = Some(idx);
                cur = next;
            } else {
                break;
            }
        }

        // R12-2: wake while the bucket guard is still held — the
        // flag-then-wake order (C8) is unchanged, but the Task deref is
        // bounded by the guard exactly like futex_wake_in_mm. A
        // signal-roused waiter that returned early cannot complete
        // do_exit past futex_cleanup's acquire of this bucket before the
        // wake below finishes; the pid revalidation stays as defense in
        // depth for the residency-fastpath residue.
        for task in &wake_list {
            if !task.is_null() {
                let pid = crate::sched::r#unsafe::task_shared(*task).pid();
                let fresh = crate::process::hotpath::pid_hash_lookup(pid);
                if fresh == *task {
                    Task::wake_up_in_futex_bucket(*task, &head1.proof());
                }
            }
        }
        drop(head1);
    } else {
        // Different buckets — lock both with deadlock-avoidance ordering.
        let (lo, hi) = if bucket1 < bucket2 {
            (bucket1, bucket2)
        } else {
            (bucket2, bucket1)
        };

        let mut guard_lo = HASH_HEADS[lo].lock_irqsave();
        let mut guard_hi = HASH_HEADS[hi].lock_irqsave();

        // Get references to the actual heads we need.
        let (head1_ref, head2_ref) = if bucket1 < bucket2 {
            (&mut *guard_lo, &mut *guard_hi)
        } else {
            (&mut *guard_hi, &mut *guard_lo)
        };

        // CMP_REQUEUE: verify *uaddr == cmpval UNDER both bucket locks
        // (reference futex_get_value_locked inside the double_lock_hb
        // region). Our get_user goes through the exception table, so the
        // in-lock read cannot sleep-block on an unmapped word: it returns
        // None → EFAULT, where the reference drops the locks, faults the
        // page in with get_user, and retries — private futex words are
        // always resident here, so the observable contract matches.
        if is_cmp {
            // SAFETY: exception-table protected read; unmapped → EFAULT.
            match crate::sync::r#unsafe::word_read(uaddr) {
                Some(uval) if uval == cmpval => {}
                Some(_) => return -EAGAIN as i64,
                None => return -EFAULT as i64,
            }
        }

        let mut prev: Option<usize> = None;
        let mut cur = head1_ref.head;

        while let Some(idx) = cur {
            let (matches, next) = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                match *slot {
                    Some(ref w) => (w.key.matches(&key1), w.next),
                    None => break,
                }
            };

            if !matches {
                prev = Some(idx);
                cur = next;
                continue;
            }

            if woken < nr_wake {
                let task = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().map(|w| w.task).unwrap_or(core::ptr::null_mut())
                };
                if prev.is_none() {
                    head1_ref.head = next;
                    if next.is_none() {
                        head1_ref.tail = None;
                    }
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
                    if next.is_none() {
                        head1_ref.tail = prev;
                    }
                }
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot { w.woken = true; }
                }
                // The woken waiter frees its own slot (see futex_wait).

                if !task.is_null() {
                    wake_list.push(task);
                }
                woken += 1;
                ret += 1;
                cur = next;
            } else if requeued < nr_requeue {
                // Unlink from source chain.
                if prev.is_none() {
                    head1_ref.head = next;
                    if next.is_none() {
                        head1_ref.tail = None;
                    }
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
                    if next.is_none() {
                        head1_ref.tail = prev;
                    }
                }
                // Relink at the TAIL of the destination chain and update
                // the key + bucket bookkeeping under the same slot lock,
                // so a concurrently-signalled sleeper reads a consistent
                // (bucket, key, next) triple from its own slot. Tail
                // append, not head push: reference requeue_futex inserts
                // with plist_add (FIFO within a priority class), so this
                // waiter must not overtake waiters already queued on
                // uaddr2 — head insertion inverted that order, letting a
                // just-requeued condvar waiter steal the mutex handoff
                // wake from a longer-blocked mutex waiter.
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot {
                        w.key = key2;
                        w.bucket = bucket2;
                        w.next = None;
                    }
                }
                // 62nd: the bucket now carries its tail — O(1) append
                // (the old version walked the whole destination chain
                // under both bucket locks to find it).
                match head2_ref.tail {
                    None => {
                        head2_ref.head = Some(idx);
                        head2_ref.tail = Some(idx);
                    }
                    Some(t) => {
                        // Plain slot lock (not nested): the requeue holds
                        // BOTH bucket guards here — the D-11 same-class
                        // double-slot hold this path already uses for its
                        // relink writes.
                        let mut tslot = WAITER_POOL[t].lock_irqsave();
                        if let Some(ref mut tw) = *tslot {
                            tw.next = Some(idx);
                        }
                        head2_ref.tail = Some(idx);
                    }
                }

                requeued += 1;
                ret += 1;
                cur = next;
            } else {
                break;
            }
        }

        // R12-2: wake while BOTH bucket guards are still held (see the
        // same-bucket branch above; the proof of either guard satisfies
        // the nested GRQ acquire — INV-LOCK-5 class crossing).
        for task in &wake_list {
            if !task.is_null() {
                let pid = crate::sched::r#unsafe::task_shared(*task).pid();
                let fresh = crate::process::hotpath::pid_hash_lookup(pid);
                if fresh == *task {
                    Task::wake_up_in_futex_bucket(*task, &guard_lo.proof());
                }
            }
        }
        drop(guard_lo);
        drop(guard_hi);
    }

    ret
}

/// FUTEX_OP_OPARG_SHIFT (include/uapi/linux/futex.h): bit 3 of the op
/// nibble makes oparg a SHIFT AMOUNT (oparg = 1 << oparg) instead of an
/// immediate.
pub const FUTEX_OP_OPARG_SHIFT: u32 = 1 << 3;

/// The decoded FUTEX_WAKE_OP operand encoding.
///
/// Reference contract (kernel/futex/waitwake.c futex_atomic_op_inuser):
/// - op nibble is 3 bits + the OPARG_SHIFT bit: `(encoded & 0x70000000) >> 28`
///   selects the operation, bit 3 (`0x80000000`) turns oparg into a shift
///   amount;
/// - oparg and cmparg are 12-bit SIGNED fields (sign_extend32(..., 11)) —
///   0xfff is -1, not 4095;
/// - an op nibble 5-7 or a cmp nibble 6-15 is -ENOSYS, not -EINVAL.
///
/// Split out so the in-kernel suite can pin the ABI decode (the old inline
/// decode masked op with 0xf and read the args unsigned).
pub fn futex_wake_op_decode(encoded: u32) -> Result<(u32, i32, u32, i32), i32> {
    let op = (encoded & 0x7000_0000) >> 28;
    let cmp = (encoded & 0x0f00_0000) >> 24;
    let mut oparg = sign_extend12((encoded & 0x00ff_f000) >> 12);
    let cmparg = sign_extend12(encoded & 0x0000_0fff);
    if encoded & (FUTEX_OP_OPARG_SHIFT << 28) != 0 {
        // Reference: oparg &= 31 after a ratelimited complaint when out of
        // range; negative oparg can never occur here (12-bit sign-extended
        // values in -2048..2047 — the reference's < 0 check dates from its
        // int-typed decode). 1 << (oparg & 31).
        oparg = 1i32 << (oparg & 31);
    }
    if op > FUTEX_OP_XOR {
        return Err(ENOSYS);
    }
    if cmp > self::FUTEX_OP_CMP_GE {
        return Err(ENOSYS);
    }
    Ok((op, oparg, cmp, cmparg))
}

/// sign_extend32(v, 11) of the reference: 12-bit field to a signed i32.
const fn sign_extend12(v: u32) -> i32 {
    ((v << 20) as i32) >> 20
}

/// FUTEX_WAKE_OP implementation (Linux futex_wake_op).
///
/// Atomically (w.r.t. futex waiters on `uaddr2`) applies the encoded
/// operation to `*uaddr2`, wakes up to `nr_wake` waiters on `uaddr`, and
/// wakes up to `nr_wake2` waiters on `uaddr2` if the comparison holds.
///
/// Encoding (val3): `(op << 28) | (cmp << 24) | (oparg << 12) | cmparg`.
pub fn futex_wake_op(
    uaddr: usize,
    flags: u32,
    nr_wake: i32,
    nr_wake2: i32,
    uaddr2: usize,
    encoded: u32,
) -> i64 {
    // Reference errno shape: an invalid op/cmp nibble is -ENOSYS
    // (futex_atomic_op_inuser's default cases), not -EINVAL.
    let (op, oparg, cmp, cmparg) = match futex_wake_op_decode(encoded) {
        Ok(t) => t,
        Err(e) => return -(e as i64),
    };

    let mm = current_futex_mm_id();
    let key2 = FutexKey::new_user(uaddr2, mm, flags);
    let bucket2 = futex_hash(&key2);

    // Perform the user-word operation while holding uaddr2's bucket lock:
    // a concurrent futex_wait on uaddr2 either re-reads the word under
    // this lock (sees the new value) or is already queued when we wake.
    // Note: the read-modify-write of the user word itself is NOT atomic
    // against userspace amo instructions (no kernel-side user-atomic op
    // here); WAKE_OP users in practice pair it with their own atomics.
    let cmp_holds;
    {
        let _head2 = HASH_HEADS[bucket2].lock_irqsave();
        // SAFETY: exception-table protected access; EFAULT on bad pointer.
        let old = match crate::sync::r#unsafe::word_read(uaddr2) {
            Some(v) => v,
            None => return -EFAULT as i64,
        };
        let old_i = old as i32;
        let new: u32 = match op {
            FUTEX_OP_SET => (oparg) as u32,
            FUTEX_OP_ADD => old_i.wrapping_add(oparg) as u32,
            FUTEX_OP_OR => old | (oparg as u32),
            FUTEX_OP_ANDN => old & !(oparg as u32),
            FUTEX_OP_XOR => old ^ (oparg as u32),
            _ => unreachable!("op range checked in futex_wake_op_decode"),
        };
        cmp_holds = match cmp {
            FUTEX_OP_CMP_EQ => old_i == cmparg,
            FUTEX_OP_CMP_NE => old_i != cmparg,
            FUTEX_OP_CMP_LT => old_i < cmparg,
            FUTEX_OP_CMP_LE => old_i <= cmparg,
            FUTEX_OP_CMP_GT => old_i > cmparg,
            FUTEX_OP_CMP_GE => old_i >= cmparg,
            _ => unreachable!("cmp range checked in futex_wake_op_decode"),
        };
        if !crate::sync::r#unsafe::word_write(uaddr2 as usize, new) {
            return -EFAULT as i64;
        }
    }

    // The user-word RMW above happens under uaddr2's bucket lock so a
    // concurrent futex_wait on uaddr2 cannot miss it; the actual wakes go
    // through futex_wake, which carries the C8 waker-side invariant
    // (extract task → set `woken` → Task::wake_up).
    let mut woken = futex_wake(uaddr, flags, nr_wake, FUTEX_BITSET_MATCH_ANY);
    if cmp_holds && nr_wake2 > 0 {
        let woken2 = futex_wake(uaddr2, flags, nr_wake2, FUTEX_BITSET_MATCH_ANY);
        if woken >= 0 && woken2 >= 0 {
            woken += woken2;
        }
    }
    woken
}

/// DEBUG: dump the live state of a futex word + its waiter chain.
#[cfg(feature = "debug-futex-trace")]
pub fn debug_dump_futex_state(uaddr: usize, parked_pid: u32) {
    use crate::debug::hotpath::{taskdump_dec, taskdump_raw_line};
    use crate::debug::hotpath::dump_syscall_ring_for;
    let bucket = uaddr % HASH_SIZE;
    taskdump_raw_line(b"FTX-STATE u=");
    taskdump_dec(uaddr as u64);
    // The user word and its neighbours (16 words starting uaddr-16).
    for i in 0..4 {
        let a = uaddr + 16 * i;
        match crate::sync::r#unsafe::word_read(a) {
            Some(v) => {
                taskdump_raw_line(b" +");
                taskdump_dec(i as u64 * 4);
                taskdump_raw_line(b"=");
                taskdump_dec(v as u64);
            }
            None => {
                taskdump_raw_line(b" EFAULT");
                break;
            }
        }
    }
    // The bucket chain.
    taskdump_raw_line(b" bucket=");
    taskdump_dec(bucket as u64);
    taskdump_raw_line(b" chain:");
    let head = HASH_HEADS[bucket].lock_irqsave();
    let mut idx = *head;
    let mut n = 0;
    while let Some(i) = idx {
        let (t, ua, wk) = {
            let slot = WAITER_POOL[i].lock_irqsave();
            match slot.as_ref() {
                Some(w) => (w.task, w.key.uaddr, w.woken),
                None => (core::ptr::null_mut(), 0, false),
            }
        };
        taskdump_raw_line(b" [slot=");
        taskdump_dec(i as u64);
        if !t.is_null() {
            taskdump_raw_line(b" pid=");
            taskdump_dec(crate::sched::r#unsafe::task_shared(t).pid() as u64);
        }
        taskdump_raw_line(b" u=");
        taskdump_dec(ua as u64);
        if wk {
            taskdump_raw_line(b" WOKEN");
        }
        taskdump_raw_line(b"]");
        idx = {
            let slot = WAITER_POOL[i].lock_irqsave();
            slot.as_ref().and_then(|w| w.next)
        };
        n += 1;
        if n > 8 {
            taskdump_raw_line(b" ...");
            break;
        }
    }
    drop(head);
    taskdump_raw_line(b"\n");
    // Also replay this task's recent syscalls (completed ones) from the
    // forensic ring — the parked wait never records, so the tail shows
    // exactly what led up to it. One-shot: the ring is global/boot-long.
    static RING_DUMPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if !RING_DUMPED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        dump_syscall_ring_for(parked_pid);
    }
}

/// do_futex - main dispatch function
pub fn do_futex(uaddr: usize, op: i32, val: u32, _timeout: u64, uaddr2: usize, _val2: u32, val3: u32) -> i64 {
    let flags = futex_to_flags(op as u32);
    let cmd = op & FUTEX_CMD_MASK;

    // All futex words must be 4-byte aligned (Linux get_futex_key).
    if uaddr & 0x3 != 0 {
        return -EINVAL as i64;
    }

    // sc185 RWFRZ-WATCH: any futex op on a word that RWFRZ-PARK named (a
    // writer parked there on 0x80000000|N readers). This is the birth-
    // moment lifecycle: a WAKE arriving on the word while the parked
    // writer never resumes = kernel lost the wake; total silence = the
    // readers never released (userspace leak/stall); a re-issued WAIT
    // from the parked tid = it DID wake once and re-parked (val still
    // nonzero — reader side stuck). Budgeted at 64 lines total.
    {
        use core::sync::atomic::Ordering::Relaxed;
        // rwlock-livelock: latch the exit-watch tgid at the FIRST
        // plain-mutex contention WAIT (expected val 1/2 — glibc
        // pthread_mutex slow path) from a process whose tgid LEADER is
        // udisksd. The wedge's mutex holder matches no live task, so
        // the discriminating datum is whether a thread of this process
        // EXITS in the window before the waiter parks; the census adopts
        // the frozen word only minutes later, so the latch must ride
        // the very first contention.
        //
        // RETIRED (probe-retire): the latch self-armed on EVERY boot
        // (udisksd contention is ordinary on 24.04) and the exit.rs
        // forensics then printed per thread exit. Behind the existing
        // futex-hunt switch (rux:futextrace (retired debug=futextrace)), same as freeze_scan.
        if matches!(cmd, FUTEX_WAIT | FUTEX_WAIT_BITSET)
            && (val == 1 || val == 2)
            && RWFRZ_EXIT_WATCH_TGID.load(Relaxed) == 0
            && crate::trace::EVENT_FUTEX_MISS.armed()
        {
            if let Some(t) = crate::sched::hotpath::current() {
                // SAFETY: read-only tgid of the current task.
                let tgid = crate::sched::r#unsafe::task_shared(t).tgid() as u32;
                // SAFETY: pid-hash lookup of the tgid leader; read-only
                // comm read.
                let lead = crate::sched::hotpath::find_task_by_pid(tgid);
                if !lead.is_null()
                    && crate::sched::r#unsafe::task_shared(lead).comm().starts_with(b"udisksd")
                {
                    let _ = RWFRZ_EXIT_WATCH_TGID.compare_exchange(
                        0,
                        tgid,
                        Relaxed,
                        Relaxed,
                    );
                }
            }
        }
        // 120th (cotask-decode): the malloc-lock wedge EARLY trigger.
        // ld-musl's __malloc_lock sits at the fixed VA 0x3fbf13e6a8 in
        // every OH process; the historical foundation freeze latches it
        // held-forever with waiters arriving 1:1 onto the low count.
        // Healthy malloc contention turns over in <100 ms (EAGAIN churn),
        // so >=2 s of CONTINUOUS held-bit WAITs from one tgid is the
        // wedge signature — and the waiters' own FUTEX_WAIT is the first
        // kernel-visible event of it (the census adopts the word only
        // ~60 s later; the OH68 acquire latch is process-blind). On the
        // 2 s persistence edge: arm the write watch (a late unlock then
        // logs an FW row with rip), run the thread-side holder hunt, and
        // run the CoTask saved-stack decoder — while any live holder is
        // still parked mid-critical-section and nameable. Once per tgid;
        // rux:fuxstuck (retired debug=fuxstuck)-gated (the page compare below is the only cost
        // on switch-off boots).
        // 120th ROOT-CAUSE CORRECTION (presence hunt): the 118th's
        // original gate compared `uaddr & 0xffff_f000 == 0x3fbf_13e000`
        // — but 0xffff_f000 is a 32-bit mask, so the masked value can
        // never carry the constant's bits 32..41: the compare was
        // ALWAYS false and LLVM (correctly) DCE'd the whole subtree out
        // of every release build, including the 120th atomic+black_box
        // variant which kept the broken mask. The page mask must be
        // 64-bit; the plain literal compare is runtime-variable and
        // survives optimization (verified on a rustc -O repro).
        const CTDE_PAGE: u64 = 0x3fbf_13e000;
        const CTDE_WORD: u64 = 0x3fbf_13e6a8;
        let ctde_uaddr = uaddr as u64;
        if matches!(cmd, FUTEX_WAIT | FUTEX_WAIT_BITSET)
            && ctde_uaddr & 0xffff_ffff_f000 == CTDE_PAGE
            && val & 0x8000_0000 != 0
            && val & 0x7fff_ffff >= 2
            && crate::trace::EVENT_FUXSTUCK.armed()
        {
            const CTDE_SLOTS: usize = 8;
            static CTDE_TGID: [core::sync::atomic::AtomicU32; CTDE_SLOTS] =
                [const { core::sync::atomic::AtomicU32::new(0) }; CTDE_SLOTS];
            static CTDE_J: [core::sync::atomic::AtomicU32; CTDE_SLOTS] =
                [const { core::sync::atomic::AtomicU32::new(0) }; CTDE_SLOTS];
            static CTDE_FIRED: [core::sync::atomic::AtomicBool; CTDE_SLOTS] =
                [const { core::sync::atomic::AtomicBool::new(false) }; CTDE_SLOTS];
            if let Some(t) = crate::sched::hotpath::current() {
                // SAFETY: read-only pid/tgid of the current task.
                let (tid, tgid) = { (t.pid() as u32, t.tgid() as u32) };
                let now = crate::timer::get_jiffies() as u32;
                let mut slot = usize::MAX;
                let mut fire = false;
                for i in 0..CTDE_SLOTS {
                    let s = CTDE_TGID[i].load(Relaxed);
                    if s == tgid {
                        slot = i;
                        if !CTDE_FIRED[i].load(Relaxed)
                            && now.wrapping_sub(CTDE_J[i].load(Relaxed)) >= 200
                        {
                            fire = CTDE_FIRED[i]
                                .compare_exchange(false, true, Relaxed, Relaxed)
                                .is_ok();
                        }
                        break;
                    }
                    if s == 0 && slot == usize::MAX {
                        slot = i;
                        // claim the slot with its start jiffies; the
                        // persistence check runs on later waits.
                        if CTDE_TGID[i]
                            .compare_exchange(0, tgid, Relaxed, Relaxed)
                            .is_ok()
                        {
                            CTDE_J[i].store(now, Relaxed);
                        }
                    }
                }
                if fire {
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!(
                        "CTD-EARLY tgid={} tid={} word=0x3fbf13e6a8 val={:#x} (held >=2s — full decode now)",
                        tgid,
                        tid,
                        val
                    );
                    let ctw = CTDE_WORD;
                    crate::api::sync::fuxpark_latch(ctw, tid, tgid);
                    crate::api::sync::malloc_holder_scan(tgid, ctw);
                    crate::api::sync::ctd_decode(tgid, ctw, "E");
                }
            }
        }
        // rwlock-livelock: the guard read only slots 0/1 — a word latched
        // into a later slot (glibc-shape latch after musl parks took the
        // front slots) never printed. Scan all 8; the all-zero fast exit
        // is the same cost as the old two loads.
        for w in RWFRZ_WATCH.iter() {
            let wa = w.load(Relaxed);
            if wa != 0 && wa == uaddr {
                let c = RWFRZ_WATCH_CNT.fetch_add(1, Relaxed);
                if c < RWFRZ_WATCH_BUDGET.load(core::sync::atomic::Ordering::Relaxed) {
                    let (tid, tgid) = match crate::sched::hotpath::current() {
                        Some(t) => { (t.pid(), t.tgid()) },
                        None => (0, 0),
                    };
                    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                    crate::pr_warn!(
                        "RWFRZ-WATCH #{} tid={} tgid={} op={} uaddr={:#x} val={:#x} jif={}",
                        c,
                        tid,
                        tgid,
                        match cmd {
                            FUTEX_WAIT => "WAIT",
                            FUTEX_WAIT_BITSET => "WAITB",
                            FUTEX_WAKE => "WAKE",
                            FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => "REQUEUE",
                            _ => "other",
                        },
                        uaddr,
                        val,
                        crate::timer::get_jiffies(),
                    );
                }
                break;
            }
        }
    }

    // sc184 (session R, the screen gate): hdf_devmgr's frozen-rwlock birth
    // tracer. Every stall dump shows devmgr's IPC threads parked in
    // FUTEX_WAIT on a musl-rwlock word reading 0x80000000|N-readers (16
    // and 148 observed) while their binder transactions sit SERVER-OWING —
    // the wedge that keeps render_service's GetService unanswered and SA
    // 10 unpublished (black screen). The word's lifecycle inside devmgr
    // is invisible to every existing probe (uncontended rd/unlock paths
    // never syscall). This tracer prints, for the latched devmgr pid
    // ONLY, every WAIT whose expected value carries the waiters bit (the
    // rwlock writer path — each line's `val` carries the reader count the
    // writer saw) plus every WAKE on an address such a WAIT named,
    // deduped per address so the UART survives: "WAIT-forever preceded by
    // matching WAKEs" vs "no WAKE ever" decides lost-wake vs leaked-reader
    // directly.
    //
    // Serial-cleanliness (print-audit lane, 2026-11): this whole block
    // runs on EVERY futex syscall. Unlatched, it paid a sched::current +
    // comm read + utf8 + name compare per op on every boot that never
    // runs hdf_devmgr; latched (or in the FXDV2 arena window below), it
    // printed up to 900 lines of HEALTHY churn per OH boot (~0.6 s of
    // polled-16550 time). The hunt is gated behind rux:futextrace (retired debug=futextrace) (the
    // same switch as the wake-miss scan in futex_wake_in_mm); the
    // sc184/sc185 hunts re-arm with the flag.
    if crate::trace::EVENT_FUTEX_MISS.armed()
    {
        use core::sync::atomic::Ordering::Relaxed;
        let cur_tgid =
            crate::sched::hotpath::current().map(|t| crate::sched::r#unsafe::task_shared(t).tgid() as u32);
        // sc185: the SA-5100 parcel latch never fired in sc184 (zero FXDV
        // lines), so also latch by process name — the first futex from any
        // thread of "hdf_devmgr" latches its tgid. The parcel matcher stays
        // (earlier latch = earlier coverage), this is the backstop.
        if FXDV_PID.load(Relaxed) == 0 {
            if let Some(t) = crate::sched::hotpath::current() {
                let comm = crate::sched::r#unsafe::task_shared(t).comm();
                let name = core::str::from_utf8(&comm[..])
                    .unwrap_or("")
                    .trim_end_matches('\0');
                if name == "hdf_devmgr" {
                    crate::api::sync::fxdv_latch_devmgr(crate::sched::r#unsafe::task_shared(t).tgid() as u32);
                }
            }
        }
        if FXDV_PID.load(Relaxed) != 0 && cur_tgid == Some(FXDV_PID.load(Relaxed)) {
            let is_rw_wait = matches!(cmd, FUTEX_WAIT | FUTEX_WAIT_BITSET)
                && val & 0x8000_0000 != 0
                && (val & 0x7fff_ffff) < 0x4000_0000;
            let mut addr_idx = usize::MAX;
            for (i, s) in FXDV_ADDR.iter().enumerate() {
                if s.load(Relaxed) == uaddr {
                    addr_idx = i;
                    break;
                }
            }
            if is_rw_wait && addr_idx == usize::MAX {
                for (i, s) in FXDV_ADDR.iter().enumerate() {
                    if s.load(Relaxed) == 0 {
                        s.store(uaddr, Relaxed);
                        addr_idx = i;
                        break;
                    }
                }
            }
            // 70th (ohw84 decode): the frozen devmgr words carry states no
            // clean musl path produces (a stale EBUSY fast-path marker with
            // every task of the tgid parked as a waiter, and a no-owner
            // 0x80000000 word), and uncontended lock/unlock never syscalls,
            // so the writers are invisible to WAIT/WAKE tracing alone. At
            // the FIRST waiter-bit WAIT from devmgr, arm the sc57 write
            // watchpoint on that word's PAGE (the mutex cluster — the
            // stub/svc/holder locks — shares one page in every observed
            // boot) and on the second distinct word; fuxwatch then logs
            // every write (tid, rip, old->new) from that moment on.
            if is_rw_wait {
                let s1 = FUXWATCH_ADDR.load(Relaxed);
                if s1 == 0 {
                    FUXWATCH_ADDR.store(uaddr as u64, Relaxed);
                    crate::debug::hotpath::ensure_defer_thread();
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!("FXDV-ARM slot1={:#x} jif={}", uaddr, crate::timer::get_jiffies());
                } else if (s1 as usize) != uaddr && FUXWATCH2_ADDR.load(Relaxed) == 0 {
                    FUXWATCH2_ADDR.store(uaddr as u64, Relaxed);
                    crate::debug::hotpath::ensure_defer_thread();
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!("FXDV-ARM slot2={:#x} jif={}", uaddr, crate::timer::get_jiffies());
                }
                // 70th (ohw86): birth-curve sampler — on every traced devmgr
                // futex, sample slot1's word AND its +0x94 neighbor (the
                // mutex cluster: sem at mutex-0x94 in every observed boot;
                // ohw86 proved the frozen mutex took its last write BEFORE
                // any wait armed the page watch) and print each CHANGE.
                // The final 0->0x10 transition's timestamp brackets the
                // founder to the second, before any watch can arm.
                if let Some(t) = crate::sched::hotpath::current() {
                    let base = FUXWATCH_ADDR.load(Relaxed) as usize;
                    if base != 0 {
                        static LAST1: [core::sync::atomic::AtomicU32; 2] =
                            [const { core::sync::atomic::AtomicU32::new(u32::MAX) }; 2];
                        for (k, wa) in [base, base.wrapping_add(0x94)].iter().enumerate() {
                            if let Some(w) = crate::sync::r#unsafe::read_u32_via_task(t, *wa) {
                                let prev = LAST1[k].load(Relaxed);
                                if w != prev {
                                    LAST1[k].store(w, Relaxed);
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                    crate::pr_warn!(
                                        "FXDV-CURVE word{:+#x} {:#x} -> {:#x} jif={} tid={}",
                                        if k == 0 { 0 } else { 0x94 },
                                        prev,
                                        w,
                                        crate::timer::get_jiffies(),
                                        crate::sched::r#unsafe::task_shared(t).pid()
                                    );
                                }
                            }
                        }
                    }
                }
                // FXDV-WORD: the word's live value at every traced wait —
                // the formation curve (0x10 -> 0x80000010 -> stuck) with
                // the waiter's own expected val beside it decides
                // stale-marker vs racing-writer vs kernel stomp.
                if let Some(t) = crate::sched::hotpath::current() {
                    if let Some(w) = crate::sync::r#unsafe::read_u32_via_task(t, uaddr) {
                        let c = if addr_idx == usize::MAX {
                            0
                        } else {
                            FXDV_CNT[addr_idx].load(Relaxed)
                        };
                        if c < 8 || c % 32 == 0 {
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!(
                                "FXDV-WORD tid={} uaddr={:#x} word={:#x} want={:#x} jif={}",
                                crate::sched::r#unsafe::task_shared(t).pid(),
                                uaddr,
                                w,
                                val,
                                crate::timer::get_jiffies()
                            );
                        }
                    }
                }
            }
            let interesting = is_rw_wait
                || (matches!(cmd, FUTEX_WAKE) && addr_idx != usize::MAX && val != 0);
            if interesting {
                let n = FXDV_N.fetch_add(1, Relaxed);
                if n < 400 {
                    // Dedup: first 12 per address, then every 16th (ohw84:
                    // first-3 hid the freeze-formation ordering).
                    let c = if addr_idx == usize::MAX {
                        0
                    } else {
                        FXDV_CNT[addr_idx].fetch_add(1, Relaxed)
                    };
                    if c < 12 || c % 16 == 0 {
                        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                        crate::pr_warn!(
                            "FXDV #{} tid={} op={} uaddr={:#x} val={:#x} jif={}",
                            n,
                            crate::sched::hotpath::current()
                                .map(|t| t.pid())
                                .unwrap_or(0),
                            match cmd {
                                FUTEX_WAIT => "WAIT",
                                FUTEX_WAIT_BITSET => "WAITB",
                                _ => "WAKE",
                            },
                            uaddr,
                            val,
                            crate::timer::get_jiffies(),
                        );
                    }
                }
            }
        }
        // FXDV2 (sc184 follow-up): the PARAM-ARENA lock tracer. The OH
        // parameter service's shared arena is mapped at the fixed base
        // 0x7ff000000000 (/dev/__parameters__ + fixed-address anon pages —
        // the ww-line's mmap-time naming). Its rwlock word at 0x7ff00002ec64
        // froze with 16 readers in sc184 while writers from param_watcher,
        // el5_filekey_man and storage_manager parked on it — a MAP_SHARED
        // lock whose readers span processes. Trace EVERY futex op in the
        // first 64K of the arena window from ANY process: the WAIT/WAKE
        // lifecycle of the shared lock decides leaked-reader (WAKEs stop
        // while the count is nonzero) vs lost-wake (WAKE issued, writer
        // still parked) vs coherence drift (two processes observe
        // different vals for the same word).
        // GATED behind rux:futextrace (retired debug=futextrace) since the lb243 decode: this tracer
        // ran ALWAYS-ON from inside do_futex's hot path, and the param
        // arena is the boot critical path (param_service sync + every
        // param read). Its 500 budgeted console writes (each allocating +
        // 0.36-0.93ms UART) all execute in the first seconds of boot; when
        // the still-unfixed phantom-preempt allocator stall bites
        // mid-print, the printing thread parks INSIDE do_futex with the
        // print half-done (lb243: samgr's self-send worker frozen at
        // do_futex+0x112e over LowerHex/unix_write_slice_owned residue)
        // and the param sync chain freezes the whole service graph — the
        // 'samgr-dark' boot family. Hunt with rux:futextrace (retired debug=futextrace), never by
        // default.
        if (0x7ff0_0000_0000..0x7ff0_0010_0000).contains(&uaddr)
            && crate::trace::EVENT_FUTEX_MISS.armed()
        {
            use core::sync::atomic::Ordering::Relaxed as R2;
            let n = FXDV2_N.fetch_add(1, R2);
            if n < 500 {
                // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                crate::pr_warn!(
                    "FXDV2 #{} tgid={} tid={} op={} uaddr={:#x} val={:#x} jif={}",
                    n,
                    crate::sched::hotpath::current().map(|t| t.tgid()).unwrap_or(0),
                    crate::sched::hotpath::current().map(|t| t.pid()).unwrap_or(0),
                    match cmd {
                        FUTEX_WAIT => "WAIT",
                        FUTEX_WAIT_BITSET => "WAITB",
                        FUTEX_WAKE => "WAKE",
                        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => "REQUEUE",
                        _ => "other",
                    },
                    uaddr,
                    val,
                    crate::timer::get_jiffies(),
                );
            }
        }
    }

    #[cfg(feature = "debug-futex-trace")]
    {
        let pid = crate::sched::hotpath::current().map(|t| crate::sched::r#unsafe::task_shared(t).pid() as u64).unwrap_or(0);
        use crate::debug::hotpath::{taskdump_dec, taskdump_raw_line};
        taskdump_raw_line(b"FTX ENTER pid=");
        taskdump_dec(pid);
        taskdump_raw_line(b" op=");
        taskdump_raw_line(ftx_opname(op));
        taskdump_raw_line(b" priv=");
        taskdump_dec(((op & FUTEX_PRIVATE_FLAG) != 0) as u64);
        taskdump_raw_line(b" u=");
        taskdump_dec(uaddr as u64);
        taskdump_raw_line(b" val=");
        taskdump_dec(val as u64);
        taskdump_raw_line(b"\n");
    }

    let ret = match cmd {
        FUTEX_WAIT => {
            // R7-A9: plain FUTEX_WAIT keeps a RELATIVE timeout even with
            // FUTEX_CLOCK_REALTIME set (Linux futex_init_timeout adds
            // ktime_get() for cmd == FUTEX_WAIT regardless of the flag);
            // only WAIT_BITSET interprets it absolutely.
            match futex_parse_timeout_ns(_timeout, false, false) {
                Ok(dl) => futex_wait_timeout(uaddr, flags, val, FUTEX_BITSET_MATCH_ANY, dl),
                Err(e) => -(e as i64),
            }
        }
        FUTEX_WAKE => {
            futex_wake(uaddr, flags, val as i32, FUTEX_BITSET_MATCH_ANY)
        }
        FUTEX_WAIT_BITSET => {
            // WAIT_BITSET always interprets timeout as absolute time
            // (CLOCK_MONOTONIC base; FUTEX_CLOCK_REALTIME rebases it).
            match futex_parse_timeout_ns(_timeout, true, (flags & FLAGS_CLOCKRT) != 0) {
                Ok(dl) => futex_wait_bitset(uaddr, flags, val, _timeout, val3, dl),
                Err(e) => -(e as i64),
            }
        }
        FUTEX_WAKE_BITSET => {
            futex_wake_bitset(uaddr, flags, val as i32, val3)
        }
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            // _timeout is repurposed as nr_requeue in the futex ABI.
            let nr_requeue = _timeout as i32;
            // Linux: negative nr_wake OR negative nr_requeue is EINVAL
            // (requeue.c: `if (nr_wake < 0 || nr_requeue < 0)`); a
            // negative nr_wake used to silently degrade into
            // "requeue everything" instead.
            if nr_requeue < 0 || (val as i32) < 0 {
                return -EINVAL as i64;
            }
            if uaddr2 == 0 || uaddr2 & 0x3 != 0 {
                return -EINVAL as i64;
            }
            // uaddr2 must be a readable user word (get_futex_key faults).
            if crate::sync::r#unsafe::word_read(uaddr2).is_none() {
                return -EFAULT as i64;
            }
            futex_requeue(uaddr, flags, val as i32, nr_requeue, uaddr2, val3, cmd == FUTEX_CMP_REQUEUE)
        }
        FUTEX_WAKE_OP => {
            // ABI: val = nr_wake on uaddr, args[3] (_timeout slot) = nr_wake2
            // on uaddr2, val3 = encoded op/cmp. uaddr2 == NULL needs no
            // special case: the reference runs the op through
            // pagefault-disabled user access and reports EFAULT, which is
            // exactly what futex_wake_op's get_user produces on its own.
            if uaddr2 & 0x3 != 0 {
                return -EINVAL as i64;
            }
            futex_wake_op(uaddr, flags, val as i32, _timeout as i32, uaddr2, val3)
        }
        FUTEX_LOCK_PI | FUTEX_LOCK_PI2 | FUTEX_TRYLOCK_PI | FUTEX_UNLOCK_PI => {
            // RT-2X item 7 (paired-test queue): dispatch the PI ops to
            // the long-implemented-but-never-wired futex_pi module (the
            // pair test suite's pi_stress row failed here with ENOSYS).
            // Shared/private rides the FLAGS_PRIVATE bit like every
            // other op. LOCK_PI/LOCK_PI2 treat the timeout as an
            // ABSOLUTE CLOCK_MONOTONIC timespec (the module parks on an
            // absolute-jiffy deadline); TRYLOCK/UNLOCK take none.
            let shared = (flags & FLAGS_SHARED) != 0;
            match cmd {
                FUTEX_TRYLOCK_PI => self::futex_trylock_pi(uaddr, shared),
                FUTEX_UNLOCK_PI => self::futex_unlock_pi(uaddr, shared),
                _ => {
                    let dl = match futex_parse_timeout_ns(_timeout, true, false) {
                        Ok(Some(ns)) => {
                            // ns (absolute, boot-anchored MONOTONIC) ->
                            // absolute jiffies, rounded up so an
                            // in-progress jiffy never under-times
                            let hz = crate::config::KERNEL_HZ as u64;
                            Some(ns.saturating_mul(hz).div_ceil(1_000_000_000))
                        }
                        Ok(None) => None,
                        Err(e) => return -(e as i64),
                    };
                    self::futex_lock_pi(uaddr, dl, shared)
                }
            }
        }
        _ => {
            // The remaining PI family (WAIT_REQUEUE_PI / CMP_REQUEUE_PI,
            // the glibc PI-condvar path) is not yet implemented.
            -ENOSYS as i64
        }
    };
    // rwlock-livelock: post-dispatch twin of the RWFRZ-WATCH entry print.
    // The verdict ladder needs op OUTCOMES on the frozen word, not just
    // arrivals: a WAKE that returned 0 while a waiter stays parked is the
    // (b) wake-miss shape; a WAKE that returned >=1 with the waiter still
    // parked is (c) re-park; a WAIT returning -EAGAIN in a loop is the
    // userspace spin itself; total silence after the latch is (a)
    // never-sent. glibc's rwlock retry ladder keys off futex_wake's
    // return count, so ret is printed for every op class. Shares the
    // 192-line budget with the entry print (a spinner eats it in ms, but
    // the first 192 lines ARE the loop's shape).
    {
        use core::sync::atomic::Ordering::Relaxed;
        for w in RWFRZ_WATCH.iter() {
            let wa = w.load(Relaxed);
            if wa != 0 && wa == uaddr {
                let c = RWFRZ_WATCH_CNT.fetch_add(1, Relaxed);
                if c < RWFRZ_WATCH_BUDGET.load(core::sync::atomic::Ordering::Relaxed) {
                    let (tid, tgid) = match crate::sched::hotpath::current() {
                        Some(t) => { (t.pid(), t.tgid()) },
                        None => (0, 0),
                    };
                    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                    crate::pr_warn!(
                        "RWFRZ-WATCH-RET #{} tid={} tgid={} op={} uaddr={:#x} val={:#x} ret={} jif={}",
                        c,
                        tid,
                        tgid,
                        match cmd {
                            FUTEX_WAIT => "WAIT",
                            FUTEX_WAIT_BITSET => "WAITB",
                            FUTEX_WAKE => "WAKE",
                            FUTEX_WAKE_BITSET => "WAKEB",
                            FUTEX_WAKE_OP => "WAKEOP",
                            FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => "REQUEUE",
                            _ => "other",
                        },
                        uaddr,
                        val,
                        ret,
                        crate::timer::get_jiffies(),
                    );
                }
                break;
            }
        }
    }
    // sc58: record completed WAKE-class ops in the verdict ring (the
    // census dumps the window around stuck parks' births). Includes the
    // error returns — a rejected op is also a "fired but delivered
    // nothing" data point.
    if matches!(
        cmd,
        FUTEX_WAKE | FUTEX_WAKE_BITSET | FUTEX_WAKE_OP | FUTEX_REQUEUE | FUTEX_CMP_REQUEUE
    ) {
        fuxw_push(cmd as u8, uaddr as u64, ret as i32);
    }
    // 76th (launcher lane, rux:txnhunt (retired debug=txnhunt)): samgr FFRT wake audit. The
    // accountmgr ready-flag gate (ohw106): accountmgr's four SA-listener
    // subscribes post two subscribe-time OnAddSystemAbility notify tasks
    // into samgr's FFRT workHandler queue ~30 ms apart — the FIRST runs
    // (notify sent 26.375 s) and the SECOND never does, silently
    // dropping the BMS ready callback and with it the whole StartUser(46)
    // chain. FFRT dispatches queues via futex wakes on its worker pool;
    // this tap prints every WAKE-class op issued by a samgr thread
    // during the first 70 s so the missing PostTask's wake (and its ret
    // — whether a parked worker was actually found) is visible. Zero
    // cost when rux:txnhunt (retired debug=txnhunt) is off.
    if matches!(
        cmd,
        FUTEX_WAKE | FUTEX_WAKE_BITSET | FUTEX_WAKE_OP | FUTEX_REQUEUE | FUTEX_CMP_REQUEUE
    ) {
        let sp = crate::ipc::hotpath::binder_sareg_samgr_pid();
        if sp != 0 {
            let mine = crate::sched::hotpath::current()
                .map(|t| crate::sched::r#unsafe::task_shared(t).tgid() == sp)
                .unwrap_or(false);
            if mine
                && crate::trace::EVENT_TXNHANG.armed()
                && crate::timer::get_jiffies() <= 7000
            {
                static SMW_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if SMW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 3000 {
                    let tid = crate::sched::hotpath::current()
                        .map(|t| t.pid())
                        .unwrap_or(0);
                    let comm = crate::sched::hotpath::current()
                        .map(|t| t.comm())
                        .map(|c| core::str::from_utf8(c).unwrap_or(""))
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                    crate::pr_warn!(
                        "SAMWAKE tid={} comm='{}' cmd={} uaddr={:#x} val={} ret={}",
                        tid,
                        comm,
                        match cmd {
                            FUTEX_WAKE => "WAKE",
                            FUTEX_WAKE_BITSET => "WAKE-bs",
                            FUTEX_WAKE_OP => "WAKE-op",
                            FUTEX_REQUEUE => "REQ",
                            _ => "CMP-REQ",
                        },
                        uaddr,
                        val,
                        ret
                    );
                }
            }
        }
    }
    ret
}

/// sys_futex system call entry point
pub fn sys_futex_handler(args: &[u64; 6]) -> i64 {
    let uaddr = args[0] as usize;
    let op = args[1] as i32;
    let val = args[2] as u32;
    let timeout = args[3];
    let uaddr2 = args[4] as usize;
    let val3 = args[5] as u32;

    // sc87: first-writer-park forensics. A FUTEX_WAIT whose expected value
    // is WAITERS|N-readers with N>=4 is a musl-rwlock WRITER parking on a
    // word that ALREADY claims many readers — the freeze's birth moment.
    // The freeze scan (60 s cadence) always arrives after the fact; this
    // catches the very first park, in the parking thread's own syscall
    // context, where the user stack still carries the full write-lock
    // caller chain. Capped at 16 dumps per boot.
    // 118th (mouse-endgame): the cap is now PER-TGID 3 within a global
    // 32 — the 16 global dumps all landed in the 2.6-3.4 s startup
    // contention burst (hilogd, samgr FFRT workers, init children —
    // every one transient), so the wedge-window park of foundation tid
    // 1332 on the 0x80000010 mutex at ~13.6 s (the exact caller-chain
    // this arm exists for) never dumped. Per-tgid budgeting keeps the
    // early spam bounded while leaving room for late parks.
    if matches!(op & FUTEX_CMD_MASK, FUTEX_WAIT | FUTEX_WAIT_BITSET) {
        let readers = val & 0x7fffffff;
        if val & 0x80000000 != 0 && readers >= 4 && readers < 0x40000000 {
            static PARKED_DUMPS: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            static PARK_TGID: [core::sync::atomic::AtomicU32; 8] =
                [const { core::sync::atomic::AtomicU32::new(0) }; 8];
            static PARK_TGID_N: [core::sync::atomic::AtomicU32; 8] =
                [const { core::sync::atomic::AtomicU32::new(0) }; 8];
            let n = PARKED_DUMPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // Per-tgid accounting: find this tgid's slot (or the first
            // free one) and take one unit of its 3-dump budget.
            use core::sync::atomic::Ordering::Relaxed;
            let cur_tgid = crate::sched::hotpath::current()
                .map(|t| crate::sched::r#unsafe::task_shared(t).tgid() as u32)
                .unwrap_or(0);
            let mut tslot = usize::MAX;
            for i in 0..8 {
                let t = PARK_TGID[i].load(Relaxed);
                if cur_tgid != 0 && t == cur_tgid {
                    tslot = i;
                    break;
                }
                if t == 0 && tslot == usize::MAX {
                    tslot = i;
                }
            }
            let tgid_ok = tslot != usize::MAX && {
                // Claim a free slot for this tgid (racy claim is fine —
                // a lost claimant simply skips its dump this once).
                if PARK_TGID[tslot].load(Relaxed) == 0 {
                    let _ = PARK_TGID[tslot]
                        .compare_exchange(0, cur_tgid, Relaxed, Relaxed);
                }
                PARK_TGID[tslot].load(Relaxed) == cur_tgid
                    && PARK_TGID_N[tslot].fetch_add(1, Relaxed) < 3
            };
            // rux:parkdump (retired debug=parkdump): each park dump costs hundreds of UART+kmsg
            // lines (hilogd.rd_kmsg eats a vCPU copying them out of
            // /dev/kmsg — the b128/b129 lesson), so the park-dump family
            // is OFF in launcher-milestone boots.
            if n < 32
                && tgid_ok
                && crate::trace::EVENT_PARKDUMP.armed()
            {
                if let Some(t) = crate::sched::hotpath::current() {
                    let tk = crate::sched::r#unsafe::task_shared(t);
                    let comm = tk.comm();
                    let name = core::str::from_utf8(&comm[..])
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    crate::trace::trace_parkdump(0, tk.pid(), uaddr as u64, val as u64);
                    // HOTPATH-PRINT: switch/cmdline-gated (off by default).
                    crate::pr_warn!(
                        "RWFRZ-PARK #{} tid {} comm='{}' uaddr={:#x} val={:#x} (writer parking on {} readers)",
                        n,
                        tk.pid(),
                        name,
                        uaddr,
                        val,
                        readers
                    );
                    // sc185: register the word for RWFRZ-WATCH (post-park
                    // lifecycle from any process — see the watch block in
                    // do_futex's prologue). sc185 lesson: the param-arena
                    // word (0x7ff00002ec64, inside FXDV2's window) churns
                    // healthily all boot and ate the whole 64-line budget
                    // by jif=1502, before the terminal freeze (born
                    // ~jif=1900+). FXDV2 owns the arena window; skip it
                    // here so the budget survives to the wedge window.
                    if !(0x7ff0_0000_0000..0x7ff0_0010_0000).contains(&uaddr) {
                        for w in RWFRZ_WATCH.iter() {
                            if w.load(core::sync::atomic::Ordering::Relaxed) == 0 {
                                w.store(uaddr, core::sync::atomic::Ordering::Relaxed);
                                break;
                            }
                        }
                    }
                    // SAFETY: t is the current task, in its own syscall.
                    rwfz_stack_frames(t, "park", 12, 64, false);
                }
            }
        }
    }
    // sc122 (Session P): the app-child park dump. The KILLDUMP path can
    // never sample the park — SIGKILL lands right after the userspace DEBUG
    // freeze-dump handshake, so every kill catches the child inside that
    // handshake (getrusage loop / open), never the eternal wait. This
    // hook dumps AT WAIT ENTRY, in the parking thread's own syscall
    // context, for exactly the app-child shape: uaddr on the main-stack
    // top pages (0x7ffffffd0000-0x7ffffffff000 — covers the observed
    // c754/c7f4/cc54/cff4 sites with margin for frame variance) from an
    // app comm, any val (2 seen so far, but do not re-filter on it).
    // rwfz_stack_frames then prints rip/rsp/VMA + the RAW 24-word chain
    // (musl callers + the runtime caller + the timespec at rsp+0x10)
    // + file-backed frames. Budget 6 — the park repeats per child; the
    // first dump already carries the chain.
    if matches!(op & FUTEX_CMD_MASK, FUTEX_WAIT | FUTEX_WAIT_BITSET) {
        if uaddr >= 0x7ffffffd0000 && uaddr < 0x7ffffffff000 {
            if let Some(t) = crate::sched::hotpath::current() {
                let tk = crate::sched::r#unsafe::task_shared(t);
                let comm = core::str::from_utf8(&tk.comm()[..])
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let is_app = comm.contains("launcher")
                    || comm.contains("settingsdata")
                    || comm.contains("systemui")
                    || comm.contains("dataability");
                if is_app {
                    static APPPARK_DUMPS: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let n =
                        APPPARK_DUMPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    // rux:parkdump (retired debug=parkdump): the APP-PARK dump prints the child's
                    // whole lib-region VMA table (APPMAP, ~2.9k lines per
                    // dump in sc179) + fd table + syscall rings — pure
                    // chain forensics, OFF in milestone boots (b128/b129
                    // hilogd.rd_kmsg lesson).
                    if n < 6
                        && crate::trace::EVENT_PARKDUMP.armed()
                    {
                        crate::trace::trace_parkdump(1, tk.pid(), uaddr as u64, val as u64);
                        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                        crate::pr_warn!(
                            "APP-PARK #{} tid {} comm='{}' uaddr={:#x} val={:#x} op={:#x}",
                            n,
                            tk.pid(),
                            comm,
                            uaddr,
                            val,
                            op
                        );
                        // sc128 (Session P): the nr-only ALL-syscall ring
                        // (last 96 since sc135, successes included,
                        // newest first) — the final action sequence
                        // between the dlopen cascade and the park.
                        {
                            use core::sync::atomic::Ordering::Relaxed;
                            const RL: usize = crate::process::types::LAST_CALLS_LEN;
                            let cur = tk.ext.last_calls_cur.load(Relaxed);
                            let mut s2 = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(s2, "APP-PARK calls:");
                            for k in 0..RL {
                                let i = (cur + RL - k) % RL; // newest first
                                let v = tk.ext.last_calls[i].load(Relaxed);
                                if v != 0 {
                                    let _ = write!(s2, " {}", v);
                                }
                            }
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!("{}", s2);
                        }
                        // sc138 (Session Q): the FILTERED init ring — the
                        // 96-entry unfiltered ring above drowns in the
                        // retry loop; this one skips {clock, writev,
                        // connect, main-stack futex} so the child's whole
                        // init since fork fits. initA carries a0 for the
                        // newest 32 (prctl option / mmap addr / futex
                        // uaddr). This is the parked side of the diff;
                        // APP-THR prints the working side.
                        {
                            use core::fmt::Write;
                            use core::sync::atomic::Ordering::Relaxed;
                            const RL: usize = crate::process::types::INIT_CALLS_LEN;
                            const BL: usize = crate::process::types::BIRTH_CALLS_LEN;
                            let cur = tk.ext.init_calls_cur.load(Relaxed);
                            // sc143: 512 entries in chunks of 128 so a
                            // single UART flush cannot gut the whole line.
                            let mut chunk = alloc::string::String::new();
                            let mut shown = 0usize;
                            for k in 0..RL {
                                let i = (cur + RL - k) % RL; // newest first
                                let v = tk.ext.init_calls[i].load(Relaxed);
                                if v != 0 {
                                    if shown % 128 == 0 {
                                        if !chunk.is_empty() {
                                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                            crate::pr_warn!("{}", chunk);
                                        }
                                        chunk = alloc::string::String::new();
                                        let _ = write!(chunk, "APP-PARK init[{}-]:", shown);
                                    }
                                    let _ = write!(chunk, " {}", v);
                                    shown += 1;
                                }
                            }
                            if !chunk.is_empty() {
                                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                crate::pr_warn!("{}", chunk);
                            }
                            // newest 96 with a0
                            let mut s3 = alloc::string::String::new();
                            let _ = write!(s3, "APP-PARK initA:");
                            for k in 0..96.min(RL) {
                                let i = (cur + RL - k) % RL;
                                let v = tk.ext.init_calls[i].load(Relaxed);
                                if v != 0 {
                                    let a0 = tk.ext.init_a0[i].load(Relaxed);
                                    let _ = write!(s3, " {}@{:#x}", v, a0);
                                }
                            }
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!("{}", s3);
                            // sc143: the birth window (fork-exit onward),
                            // immune to loop churn.
                            let mut s4 = alloc::string::String::new();
                            let _ = write!(s4, "APP-PARK birth:");
                            for k in 0..BL {
                                let v = tk.ext.birth_calls[k].load(Relaxed);
                                if v != 0 {
                                    let a0 = tk.ext.birth_a0[k].load(Relaxed);
                                    let _ = write!(s4, " {}@{:#x}", v, a0);
                                }
                            }
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!("{}", s4);
                        }
                        // sc125 (Session P): the pre-park FAILING-syscall
                        // tail — musl pthread_create mmaps the thread stack
                        // BEFORE clone, so an ENOMEM mmap means no clone
                        // ever reaches the census. Latched per task.
                        {
                            use core::sync::atomic::Ordering::Relaxed;
                            let cur = tk.ext.last_fails_cur.load(Relaxed);
                            let mut s = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(s, "APP-PARK fails:");
                            for k in 0..8 {
                                let i = (cur + 8 - 1 - k) % 8; // newest first
                                let v = tk.ext.last_fails[i].load(Relaxed);
                                if v != 0 {
                                    // sc135 (Session Q): a0/a1 name the
                                    // prctl option (nr167) and friends.
                                    let a0 = tk.ext.last_fails_a0[i].load(Relaxed);
                                    let a1 = tk.ext.last_fails_a1[i].load(Relaxed);
                                    let _ = write!(
                                        s,
                                        " nr{}=-{} a0={:#x} a1={:#x}",
                                        v & 0xffff,
                                        v >> 16,
                                        a0,
                                        a1
                                    );
                                }
                            }
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!("{}", s);
                        }
                        // SAFETY: t is the current task, in its own syscall.
                        rwfz_stack_frames(t, "apppark", 12, 96, false);
                        // sc139: the call-site-validated caller chain.
                        apppark_rets(t);
                        // sc136 (Session Q): the park chain's caller frame
                        // (raw ustack slot[12] 0x7ff0816ff8a8 family) sits
                        // in a lib VMA never printed by the frames scan
                        // (anon-filtered or fd-closed) — and sc135's only
                        // slot-27 frame line was cut at every UART flush
                        // boundary by interleaved hilog output. Dump the
                        // child's whole lib-region VMA table + open fd
                        // paths in COMPACT one-line-per-VMA form (short
                        // lines survive flush boundaries) so every chain
                        // slot resolves offline against the image libs.
                        {
                            use core::fmt::Write;
                            // sc242: Arc-pin the child's mm (unaccounted
                            // foreign-borrow #GP family — see
                            // rwfz_stack_frames).
                            if let Some(aspace_pin) = tk.address_space_arc() {
                                let aspace = aspace_pin.as_ref();
                                let vmas = aspace.vma_read();
                                for vm in vmas.iter() {
                                    let s0 = vm.start().as_usize();
                                    let e0 = vm.end().as_usize();
                                    // lib region only: musl at 0x3fbf...,
                                    // engine libs/arenas at 0x7ff0-0x7ffb.
                                    let in_lib_region = (0x3f00000000..0x400000000)
                                        .contains(&s0)
                                        || (0x7ff000000000..0x7ffb00000000)
                                            .contains(&s0);
                                    if !in_lib_region {
                                        continue;
                                    }
                                    let fd = vm.file_fd();
                                    let path = if fd >= 0 {
                                        // sc210: the wedge dump can walk a
                                        // just-forked task whose fdtable is
                                        // not set yet — fdtable() would
                                        // panic and kill the boot (kernel
                                        // panic "FdTable not initialized",
                                        // task.rs:3958). Diagnostic walks
                                        // must always be None-safe.
                                        tk.try_fdtable()
                                            .and_then(|fdt| fdt.get_file(fd as usize))
                                            .map(|f| probe_file_path(&f))
                                            .unwrap_or_else(|| alloc::string::String::from("?"))
                                    } else {
                                        alloc::string::String::from("-")
                                    };
                                    let mut v = alloc::string::String::new();
                                    let _ = write!(
                                        v,
                                        "APPMAP {:?} {:#x}-{:#x} off={:#x} fd={} sz={:#x} {}",
                                        comm,
                                        s0,
                                        e0,
                                        vm.offset(),
                                        fd,
                                        vm.file_size() as usize,
                                        path
                                    );
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                    crate::pr_warn!("{}", v);
                                }
                            }
                            // Open fd table: names the connect/writev
                            // socket (hilog? samgr session?) and any lib
                            // fds still alive. sc138: classify each fd by
                            // its FileOps identity (unix/tcp/netlink/
                            // binder/pipe/io_uring/file) — fd3's 10MB
                            // anon_inode mapping in sc137 needs the true
                            // object type named, and unix sockets also
                            // print their bound+peer names.
                            // sc210: fdtable() panics on a task whose
                            // fdtable is not set — the dump must be
                            // None-safe or it kills the boot it dumps
                            // (kernel panic "FdTable not initialized").
                            'fdscan: {
                                let Some(fdt) = tk.try_fdtable() else {
                                    break 'fdscan;
                                };
                            let mut shown = 0u32;
                            for fd in 0..32u32 {
                                if let Some(f) = fdt.get_file(fd as usize) {
                                    let kind = match f.get_ops() {
                                        Some(ops) => {
                                            let p = ops as *const crate::fs::types::FileOps;
                                            if crate::api::net::is_unix_socket_ops(ops) {
                                                match crate::api::net::unix_fd_bound_peer(
                                                    fd as usize,
                                                ) {
                                                    Some((bn, peer)) => {
                                                        alloc::format!(
                                                            "unix bound={} peer={}",
                                                            bn, peer
                                                        )
                                                    }
                                                    None => "unix".into(),
                                                }
                                            } else if crate::api::net::is_inet_socket_ops(
                                                ops,
                                            ) {
                                                "inet-socket".into()
                                            } else if crate::api::net::is_netlink_ops(ops) {
                                                "netlink".into()
                                            } else if crate::api::net::is_raw_ops(ops) {
                                                "raw-socket".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::ipc::hotpath::BINDER_OPS,
                                            ) {
                                                "binder".into()
                                            } else if crate::api::fs::is_pipe_ops(ops) {
                                                "pipe".into()
                                            } else if crate::api::fs::is_memfd_ops(ops) {
                                                "memfd".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::io_uring::IO_URING_OPS,
                                            ) {
                                                "io_uring".into()
                                            } else {
                                                "file".into()
                                            }
                                        }
                                        None => "anon".into(),
                                    };
                                    let mut v = alloc::string::String::new();
                                    let _ = write!(
                                        v,
                                        "APPFD {:?} fd={} {} kind={}",
                                        comm,
                                        fd,
                                        probe_file_path(&f),
                                        kind
                                    );
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                    crate::pr_warn!("{}", v);
                                    shown += 1;
                                    if shown >= 16 {
                                        break;
                                    }
                                }
                            }
                            }
                            // sc138 (Session Q): decode round 3.
                            // (a) The park caller's code bytes: raw
                            // ustack slot[12] is the return address into
                            // the caller frame — sc137 resolved it to an
                            // ANONYMOUS exec VMA (JIT stub arena or a
                            // memfd-loaded lib). 24 bytes ending at
                            // slot[12] identify the call instruction
                            // offline; the first 8 bytes of the
                            // containing VMA separate memfd-lib (ELF
                            // magic) from a JIT arena.
                            {
                                // Stack-freed guard (KILLDUMP census race
                                // class — see kernel_stack_top_checked):
                                // rsp=0 falls into the existing skip.
                                let rsp = match tk.kernel_stack_top_checked() {
                                    Some(top) => {
                                        #[cfg(feature = "x86_64")]
                                        {
                                            {
                                                let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
                                                regs.rsp as usize
                                            }
                                        }
                                        #[cfg(feature = "riscv64")]
                                        {
                                            {
                                                let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
                                                regs.sp as usize
                                            }
                                        }
                                        #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
                                        { 0usize }
                                    }
                                    None => 0usize,
                                };
                                if rsp != 0 {
                                    let rd64 = |va: usize| -> Option<u64> {
                                        let lo = crate::sync::r#unsafe::read_u32_via_task(t, va)?;
                                        let hi = crate::sync::r#unsafe::read_u32_via_task(t, va + 4)?;
                                        Some(lo as u64 | ((hi as u64) << 32))
                                    };
                                    if let Some(slot12) = rd64(rsp + 8 * 12) {
                                        let mut v = alloc::string::String::new();
                                        let _ = write!(
                                            v,
                                            "APPPK12 {:?} slot12={:#x}",
                                            comm, slot12
                                        );
                                        for k in 0..3 {
                                            match rd64((slot12.wrapping_sub(24) + 8 * k) as usize) {
                                                Some(b) => {
                                                    let _ = write!(v, " b{}={:#018x}", 24 - 8 * k, b);
                                                }
                                                None => {
                                                    let _ = write!(v, " b{}=?", 24 - 8 * k);
                                                }
                                            }
                                        }
                                        // VMA start bytes (ELF check)
                                        if let Some(aspace2) = tk.address_space() {
                                            let vmas2 = aspace2.vma_read();
                                            if let Some(vm) = vmas2.find(
                                                crate::mm::types::VirtAddr::new(
                                                    slot12 as usize,
                                                ),
                                            ) {
                                                let start = vm.start().as_usize();
                                                let _ = write!(
                                                    v,
                                                    " vma={:#x}-{:#x} vmab0=",
                                                    start,
                                                    vm.end().as_usize()
                                                );
                                                let first = rd64(start);
                                                match first {
                                                    Some(b) => {
                                                        let _ = write!(v, "{:#018x}", b);
                                                    }
                                                    None => {
                                                        let _ = write!(v, "?");
                                                    }
                                                }
                                                let elf = first.map_or(false, |w| (w & 0xffff) == 0x457f);
                                                let _ = write!(
                                                    v,
                                                    " {}",
                                                    if elf { "MEMFD-LIB(ELF)" } else { "JIT/anon" }
                                                );
                                            }
                                        }
                                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                        crate::pr_warn!("{}", v);
                                    }
                                }
                            }
                            // (b) The global unix-socket registry once:
                            // which abstract/filesystem listeners exist at
                            // park time (the appconn hex was unreadable —
                            // strncpy stopped at the abstract NUL).
                            if n == 0 {
                                for line in crate::api::net::unix_registry_snapshot() {
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                    crate::pr_warn!("{}", line);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // sc118 decode (Session P): every OH app child parks single-threaded
    // in FUTEX_WAIT on its own main-thread stack at the SAME site
    // (uaddr 0x7fffffffc754 / 0x7fffffffc7f4 — page 0x7fffffffc000; the
    // site is shared across launcher/settingsdata/systemui binaries, so
    // it lives in the common musl/early-runtime startup). The sc117
    // clone-refusal probes never fired, so no clone failed. Remaining
    // hypotheses: (a) the runtime never attempts the worker clone,
    // (b) the worker clone succeeds but the thread dies before signaling,
    // (c) the worker DOES send FUTEX_WAKE on this page and the wake is
    // lost or mismatches. This census logs every futex op on that page
    // from ANY comm (workers rename themselves — OS_FFRT_* etc. — so a
    // comm filter would miss the very wake we hunt). Two lines per op:
    // the entry BEFORE do_futex (a WAIT that parks for minutes must
    // still have printed its arrival) and the result after. A WAKE line
    // printing ->0 while a waiter sits parked is a lost wake; no WAKE
    // at all after a WAIT is never-sent ((a) or (b) — the paired
    // appclone census in sys_clone separates those). Budget 150 lines.
    //
    // Serial-cleanliness (print-audit lane, 2026-11): STK_PAGE is 3
    // pages below the stack top of EVERY x86_64 process, so any
    // main-thread futex on a shallow frame hit this — two pr_warn
    // lines per op through the polled UART until the 400-op budget
    // burned (~0.8 s of serial per boot). The census joins the
    // rux:futextrace (retired debug=futextrace) family; the sc118/sc119 hunts re-arm with the
    // flag.
    {
        const STK_PAGE: usize = 0x7fffffffc000;
        if uaddr & !0xfff == STK_PAGE
            && crate::trace::EVENT_FUTEX_MISS.armed()
        {
            // Budget raised 150 -> 400 for sc119: the page is 3 pages below
            // the stack top of EVERY x86_64 process, so ANY main-thread
            // futex on a shallow frame burns it — appspawn's own main-loop
            // wait would exhaust 150 before the first app child parks.
            // 400 ops = <=800 lines ~= 7 s of UART across a 15 min boot
            // (vs sc115's 2472-line RWFRZ storm at ~1 s) — affordable to
            // guarantee the census still has budget when children park.
            static NS: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if NS.load(core::sync::atomic::Ordering::Relaxed) < 400 {
                NS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                let comm = crate::sched::hotpath::current()
                    .map(|t| t.comm())
                    .map(|c| core::str::from_utf8(c).unwrap_or(""))
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let tid = crate::sched::hotpath::current()
                    .map(|t| t.pid())
                    .unwrap_or(0);
                // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                crate::pr_warn!(
                    "stkfutex: tid {} op={:#x} uaddr={:#x} val={:#x} comm='{}'",
                    tid,
                    op,
                    uaddr,
                    val,
                    comm
                );
                let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
                rwfz_op_log(uaddr, op, val, r);
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!("stkfutex: tid {} -> {}", tid, r);
                return r;
            }
        }
    }
    // sc209 (leg 14): render_service futex lifecycle tracer (see the
    // FXRS_TGID block near the top of this file). Entry line BEFORE
    // do_futex + result line after, for every WAIT/WAKE/BITSET/REQUEUE op
    // issued by any thread of the latched render_service tgid. Main's
    // park uaddr + whether ANY wake ever targets it decides kernel
    // wake-loss vs the FFRT worker never running the synced task.
    {
        let lt = FXRS_TGID.load(core::sync::atomic::Ordering::Relaxed);
        let word_hit = fxrs_word_watched(uaddr);
        if (lt != 0 || word_hit)
            && matches!(
                op & FUTEX_CMD_MASK,
                FUTEX_WAIT
                    | FUTEX_WAKE
                    | FUTEX_WAIT_BITSET
                    | FUTEX_WAKE_BITSET
                    | FUTEX_CMP_REQUEUE
            )
        {
            let mine = crate::sched::hotpath::current()
                .map(|t| crate::sched::r#unsafe::task_shared(t).tgid() == lt)
                .unwrap_or(false);
            if mine || word_hit {
                let n = FXRS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < FXRS_BUDGET {
                    let tid = crate::sched::hotpath::current()
                        .map(|t| t.pid())
                        .unwrap_or(0);
                    let tgid = crate::sched::hotpath::current()
                        .map(|t| t.tgid())
                        .unwrap_or(0);
                    let comm = crate::sched::hotpath::current()
                        .map(|t| t.comm())
                        .map(|c| core::str::from_utf8(c).unwrap_or(""))
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    let cmd = match op & FUTEX_CMD_MASK {
                        FUTEX_WAIT => "WAIT",
                        FUTEX_WAKE => "WAKE",
                        FUTEX_WAIT_BITSET => "WAIT-bs",
                        FUTEX_WAKE_BITSET => "WAKE-bs",
                        FUTEX_CMP_REQUEUE => "CMP-REQ",
                        _ => "other",
                    };
                    crate::pr_debug!(
                        "FXRS #{} {} tid {} tgid {} comm='{}' uaddr={:#x} val={:#x} u2={:#x} v3={:#x} tmo={:#x}",
                        n,
                        cmd,
                        tid,
                        tgid,
                        comm,
                        uaddr,
                        val,
                        uaddr2,
                        val3,
                        timeout
                    );
                    let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
                    rwfz_op_log(uaddr, op, val, r);
                    crate::pr_debug!("FXRS #{} -> ret={}", n, r);
                    return r;
                }
            }
        }
    }
    // sc156 (Session R): the SA-501 wedge tracer. sc155 closed the
    // registration question to the registrar side: foundation's safwk
    // loader main thread opens libs 401/4606/4607/3299/3203/3704 then
    // NEVER reaches OpenSo(180) — and safwk's FindAndStartPhaseTasks
    // escapes any hung OnStart through WaitForTasks'
    // condition_variable::wait_for(100s), so the loader can only be
    // permanently stuck if a TIMED futex wait never returns. This census
    // prints every timed WAIT/WAIT_BITSET from foundation/sa_main
    // threads (entry line BEFORE the wait + result line after): a
    // missing "->" line names a never-firing timeout as the wedge. The
    // first two waits also dump the caller's syscall rings. The timespec
    // is decoded so relative(100s)/absolute(now+100s) deadlines are
    // distinguishable offline.
    //
    // Serial-cleanliness (print-audit lane, 2026-11): condition_variable
    // wait_for is a hot per-op path for foundation/sa_main threads —
    // ungated this paid a comm compare per timed WAIT plus up to 240
    // pr_warn lines per OH boot. The census joins the rux:futextrace (retired debug=futextrace)
    // family; the sc156 hunt re-arms with the flag.
    if matches!(op & FUTEX_CMD_MASK, FUTEX_WAIT | FUTEX_WAIT_BITSET)
        && timeout != 0
        && crate::trace::EVENT_FUTEX_MISS.armed()
    {
        if let Some(t) = crate::sched::hotpath::current() {
            let tk = crate::sched::r#unsafe::task_shared(t);
            let comm = core::str::from_utf8(&tk.comm()[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            if comm.contains("foundation") || comm.contains("sa_main") {
                static FTW_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = FTW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 240 {
                    let mut ts = [0u8; 16];
                    let ts_ok = crate::sync::r#unsafe::copy_from_user_bytes(
                        ts.as_mut_ptr(),
                        timeout as usize,
                        ts.len(),
                    ) == 0;
                    let (sec, nsec) = if ts_ok {
                        (
                            u64::from(ts[0]) | (u64::from(ts[1]) << 8) | (u64::from(ts[2]) << 16)
                                | (u64::from(ts[3]) << 24),
                            u32::from(ts[8]) | (u32::from(ts[9]) << 8)
                                | (u32::from(ts[10]) << 16)
                                | (u32::from(ts[11]) << 24),
                        )
                    } else {
                        (0, 0)
                    };
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!(
                        "FTWAIT #{} tid {} comm='{}' uaddr={:#x} val={:#x} op={:#x} ts={}{}.{:09}s",
                        n,
                        tk.pid(),
                        comm,
                        uaddr,
                        val,
                        op,
                        if ts_ok { "" } else { "?" },
                        sec,
                        nsec
                    );
                    // sc156: ring dump on the first two timed waits only —
                    // the wait that never returns carries its caller
                    // context in the rings.
                    if n < 2 {
                        use core::fmt::Write;
                        use core::sync::atomic::Ordering::Relaxed;
                        const RL: usize = crate::process::types::LAST_CALLS_LEN;
                        let cur = tk.ext.last_calls_cur.load(Relaxed);
                        let mut s2 = alloc::string::String::new();
                        let _ = write!(s2, "FTWAIT calls:");
                        for k in 0..RL {
                            let i = (cur + RL - k) % RL; // newest first
                            let v = tk.ext.last_calls[i].load(Relaxed);
                            if v != 0 {
                                let _ = write!(s2, " {}", v);
                            }
                        }
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!("{}", s2);
                        let fcur = tk.ext.last_fails_cur.load(Relaxed);
                        let mut s3 = alloc::string::String::new();
                        let _ = write!(s3, "FTWAIT fails:");
                        for k in 0..8 {
                            let i = (fcur + 8 - 1 - k) % 8;
                            let v = tk.ext.last_fails[i].load(Relaxed);
                            if v != 0 {
                                let a0 = tk.ext.last_fails_a0[i].load(Relaxed);
                                let _ = write!(s3, " nr{}=-{} a0={:#x}", v & 0xffff, v >> 16, a0);
                            }
                        }
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!("{}", s3);
                    }
                    let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
                    rwfz_op_log(uaddr, op, val, r);
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!("FTWAIT #{} -> ret={}", n, r);
                    return r;
                }
            }
        }
    }
    let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
    rwfz_op_log(uaddr, op, val, r);
    r
}

// ==================== rwlock freeze watch (sc81) ====================
//
// sc76/sc80 terminal shape: samgr's abilityMapLock_ (musl rwlock) shows
// word 0x80000010 — waiters bit + 16 held READ counts — while the tgid
// has 7 live threads, ALL parked on that word in FUTEX_WAIT. Decoded OH
// musl protocol (from /system/lib/ld-musl-x86_64.so.1):
//   word==0            free
//   0 < count < 0x7fffffff   N readers hold the lock
//   word==0x7fffffff  writer holds (bit31 ORed while parked waiters exist)
// Readers NEVER park at a positive count (tryrdlock CASes unconditionally
// unless overflow) — so every thread parked on this word is a WRITER, and
// new read acquisitions still succeed. The wedge is writer starvation:
// the 16 read counts must drain to 0 for any writer to proceed, and with
// only 7 live threads none of them can hold more than a non-recursive
// scope's worth. Either threads exited holding read scopes (leak), the
// count was inflated by corruption, or threads exist that the futex
// waiter graph does not show (parked in non-futex sleeps). This watch
// decides: word deltas (any unlock traffic), futex op traffic on the
// word, per-scan tid-liveness of the tgid (reader exits), and a one-shot
// full-tgid park-stack dump when the freeze holds.

/// Watched word (uaddr); 0 = inactive. The freeze scan publishes it when
/// a cluster of >=3 waiters sits on an rwlock-shaped word.
pub static RWFRZ_UADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
pub static RWFRZ_OPLOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Log futex ops on the watched word: every park (with the userspace
/// expected value), every wake (with how many were woken), every wait
/// return. This is the word's entire syscall-visible traffic. Bursts are
/// capped so a pathological loop cannot flood the console.
fn rwfz_op_log(uaddr: usize, op: i32, val: u32, result: i64) {
    if uaddr == 0 || uaddr != RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let n = RWFRZ_OPLOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if n > 128 && n % 16 != 0 {
        return;
    }
    let cmd = match op & FUTEX_CMD_MASK {
        FUTEX_WAIT => "WAIT",
        FUTEX_WAKE => "WAKE",
        FUTEX_WAIT_BITSET => "WAIT-bs",
        FUTEX_WAKE_BITSET => "WAKE-bs",
        FUTEX_CMP_REQUEUE => "CMP-REQ",
        _ => "other",
    };
    let tid = crate::sched::hotpath::current()
        .map(|t| t.pid() as u64)
        .unwrap_or(0);
    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
    crate::pr_warn!(
        "RWFRZ-OP #{} {} tid {} val={:#x} ret={}",
        n,
        cmd,
        tid,
        val,
        result
    );
}

/// sc138 (Session Q): display path for a File that may be a memfd —
/// `File::path()` prints "anon_inode:[<ino>]" for the pathless files;
/// resolve <ino> through the memfd name registry (recorded at
/// memfd_create) so APPFD/APPMAP can name the engine libs the children
/// map through memfds ("anon_inode:[17] memfd:libXXX").
fn probe_file_path(f: &crate::fs::types::File) -> alloc::string::String {
    let p = f.path();
    if let Some(rest) = p.strip_prefix("anon_inode:[") {
        if let Some(num) = rest.strip_suffix(']') {
            if let Ok(ino) = num.parse::<u64>() {
                if let Some(name) = crate::api::fs::memfd_name_by_ino(ino) {
                    return alloc::format!("{} memfd:{}", p, name);
                }
            }
        }
    }
    p
}

/// sc139 (Session Q): call-site-validated stack scan for the parked
/// child. sc138 killed BOTH candidate "caller" slots as stale decoys
/// (slot[1] resolved into libc++ string::assign — data, not code flow;
/// slot[12] points into a zero page), so the cond's true caller chain
/// is still unnamed. This pass keeps a stack qword only when the bytes
/// Public shim for the exit-path forensics (process/exit.rs): walk the
/// DYING thread's user stack — its frames are intact at do_exit time.
/// SAFETY: caller guarantees `task` is the current, still-live task.
pub fn apppark_rets_public(task: *const Task) {
    apppark_rets(task)
}

/// IMMEDIATELY BEFORE it decode as a call instruction — `E8 rel32`
/// (v-5) or `FF D0-D7` call-reg (v-2) — the cheap x86-64 signature of
/// a real return address. Slots surviving that check are the real
/// frames. SAFETY: `task` live; diagnostic reads through its tables.
pub fn apppark_rets(task: *const Task) {
    let tk = crate::sched::r#unsafe::task_shared(task);
    // Stack-freed guard (KILLDUMP census race class): nothing to walk
    // without a live stack.
    let Some(top) = tk.kernel_stack_top_checked() else {
        return;
    };
    #[cfg(feature = "x86_64")]
    let rsp = {
        let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
        regs.rsp as usize
    };
    #[cfg(not(feature = "x86_64"))]
    let rsp = 0usize;
    if rsp == 0 {
        return;
    }
    // sc242 (leg-17): Arc-pin the foreign task's mm for the whole scan
    // (same unaccounted-borrow #GP family as rwfz_stack_frames).
    let Some(aspace_pin) = tk.address_space_arc() else { return };
    let aspace = aspace_pin.as_ref();
    let vmas = aspace.vma_read();
    let read_u64_at = |va: usize| -> Option<u64> {
        if va & 0xfff > 0xff8 {
            return None;
        }
        let lo = crate::sync::r#unsafe::read_u32_via_task(task, va)?;
        let hi = crate::sync::r#unsafe::read_u32_via_task(task, va + 4)?;
        Some(lo as u64 | ((hi as u64) << 32))
    };
    use core::fmt::Write;
    let mut out = alloc::string::String::new();
    let _ = write!(out, "APPPARK-RETS:");
    let mut found = 0u32;
    for i in 0..128u32 {
        if found >= 12 {
            break;
        }
        let Some(v) = read_u64_at(rsp + 8 * i as usize) else {
            continue;
        };
        if v < 0x1000 || v > 0x7fff_ffff_ffff {
            continue;
        }
        // Must land in some VMA (code or JIT — anon allowed here, the
        // call-site check filters garbage).
        let Some(vm) = vmas.find(crate::mm::types::VirtAddr::new(v as usize)) else {
            continue;
        };
        // 8 bytes ending at v: b[7] is the byte at v-1.
        let Some(w) = read_u64_at(v as usize - 8) else {
            continue;
        };
        let b = w.to_le_bytes();
        let is_call = b[3] == 0xE8 /* call rel32 ends at v */
            || (b[6] == 0xFF && (b[7] & 0xF8) == 0xD0) /* call reg */
            || (b[4] == 0xFF && (b[5] & 0xC7) == 0x05); /* call [rip+disp32]-ish */
        if !is_call {
            continue;
        }
        let _ = write!(
            out,
            " [{}]@{:#x}({:#x}-{:#x},off={:#x},b={:#018x})",
            i,
            v,
            vm.start().as_usize(),
            vm.end().as_usize(),
            vm.offset() + (v as usize - vm.start().as_usize()),
            read_u64_at(v as usize).unwrap_or(0)
        );
        found += 1;
    }
    if found == 0 {
        let _ = write!(out, " none");
    }
    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
    crate::pr_warn!("{}", out);
}

/// 71st (70th item 2): RWFRZ dump emission goes through the taskdump
/// focus ring ("rwfz:" class) instead of direct pr_warn. ohw85 showed
/// the direct prints spliced MID-RECORD by the other CPU's census flood
/// (a ret[55]= line cut at 12 hex chars, tail lines lost), so the
/// second-lock caller chain of the frozen-word holder never reached the
/// serial. censusd drains the focus ring single-threaded and first.
/// Splits multi-line strings, chunks each line under the 240-byte
/// printk record cap (cutting at the last space when one is near).
/// `prio` routes to the "rwfza:" ring (drained before everything) —
/// reserved for the ADOPTION dump: ohw90 showed the shared focus class
/// gets flooded by the NBR-PARK mass latch, evicting the founder dump.
pub fn rwfz_emit_prio(s: &str, prio: bool) {
    const CAP: usize = 200;
    let tag = if prio { "rwfza" } else { "rwfz" };
    for line in s.split('\n') {
        let mut rest = line;
        let mut part = 0u32;
        while !rest.is_empty() {
            let take = core::cmp::min(CAP, rest.len());
            let mut cut = take;
            if cut < rest.len() {
                // Prefer a whitespace boundary within the last 24 bytes.
                if let Some(sp) = rest[..cut].rfind(' ').filter(|&i| i > cut.saturating_sub(24)) {
                    cut = sp;
                }
            }
            crate::debug::hotpath::defer_emit_line(alloc::format!(
                "{}:{} {}",
                tag,
                part,
                &rest[..cut]
            ));
            rest = &rest[cut..];
            part += 1;
        }
    }
}

fn rwfz_emit(s: &str) {
    rwfz_emit_prio(s, false);
}

/// One user-stack frame line resolved to its covering VMA (Session F's
/// byte-match symbolization method). sc90: `scan_slots` parametrizes the
/// stack window (64 for per-task dumps; the recursion holder needs 256 —
/// its 16-deep chain with OH frame sizes sits ~1-2 KiB above rsp).
/// SAFETY: diagnostic walk of `task`'s tables; `task` live.
pub fn rwfz_stack_frames(
    task: *const Task,
    label: &str,
    max_frames: u64,
    scan_slots: u64,
    prio: bool,
) {
    let tk = crate::sched::r#unsafe::task_shared(task);
    // Stack-freed guard (same class as the KILLDUMP census skip above):
    // rwfz_stack_frames runs on FOREIGN tasks that may have exited and
    // freed their kernel stack — a stackless top is KERNEL_STACK_SIZE and
    // the entry-frame read faults (monkey-b r38 panicked exactly here,
    // rip inside rwfz_stack_frames). Nothing to walk without a stack.
    let Some(top) = tk.kernel_stack_top_checked() else {
        return;
    };
    #[cfg(feature = "x86_64")]
    let (rsp, rip) = {
        let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
        // SAFETY: diagnostic read of this task's entry frame.
        { (regs.rsp, regs.rip) }
    };
    #[cfg(feature = "riscv64")]
    let (rsp, rip) = {
        let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
        // SAFETY: same contract, riscv64.
        { (regs.sp, regs.epc) }
    };
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    let (rsp, rip) = (0u64, 0u64);
    // sc242 (leg-17): PIN the mm for the whole dump — the unaccounted
    // `address_space()` borrow of a FOREIGN task lets the victim's
    // concurrent exit_mm free the MmStruct (and its VMA btree) mid-walk
    // (the documented "/proc maps #GP family"), and KILLDUMP runs
    // exactly when the victim's exit is tearing the mm down. sc242
    // #GP'd in the freed VMA tree at the second frame's vma_of lookup.
    let Some(aspace_pin) = tk.address_space_arc() else { return };
    let aspace = aspace_pin.as_ref();
    let vmas = aspace.vma_read();
    // sc90: return the VMA's BASE file offset separately — the loader
    // closes the fd after mmap, leaving fd=-1 on code VMAs that still
    // carry a real mapping offset (sc89: every park rip printed
    // fd=-1 fileoff=0x116dea). Anonymous VMAs have offset 0 AND fd -1,
    // so "offset != 0 || fd >= 0" separates file-backed code frames
    // from heap/stack data pointers.
    let vma_of = |v: u64| -> Option<(usize, usize, i32, usize, usize, u64)> {
        let vm = vmas.find(crate::mm::types::VirtAddr::new(v as usize))?;
        let base_off = vm.offset();
        Some((
            vm.start().as_usize(),
            vm.end().as_usize(),
            vm.file_fd(),
            base_off,
            base_off + (v as usize - vm.start().as_usize()),
            vm.file_size(),
        ))
    };
    let read_u64_at = |va: u64| -> Option<u64> {
        if va & 0xfff > 0xff8 {
            return None;
        }
        let lo = crate::sync::r#unsafe::read_u32_via_task(task, va as usize)?;
        let hi = crate::sync::r#unsafe::read_u32_via_task(task, (va + 4) as usize)?;
        Some(lo as u64 | ((hi as u64) << 32))
    };
    // The RIP itself first — the futex park site names the primitive
    // (musl __timedwait caller: rdlock/wrlock/condvar). sc90: ripbytes
    // byte-identify the park-site lib offline (the mapping's fd is gone;
    // filesz prints 0 — offset+bytes are the surviving fingerprints).
    if let Some((s, e, fd, base, off, fsz)) = vma_of(rip) {
        rwfz_emit_prio(&alloc::format!(
            "RWFRZ:   {} rsp={:#x} rip={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} baseoff={:#x} filesz={:#x} ripbytes={:#018x}",
            label,
            rsp,
            rip,
            s,
            e,
            fd,
            off,
            base,
            fsz as usize,
            read_u64_at(rip).unwrap_or(0)
        ), prio);
    } else {
        rwfz_emit_prio(
            &alloc::format!("RWFRZ:   {} rsp={:#x} rip={:#x} (no vma)", label, rsp, rip),
            prio,
        );
    }
    // sc86: resolve fd -> file PATH for file-backed frames (offline
    // fileoff/byte matching hit missing-lib walls; the path names the
    // library outright). Cached per-fd for the whole dump of this task.
    let mut path_cache: alloc::vec::Vec<(i32, alloc::string::String)> = alloc::vec::Vec::new();
    let path_of = |fd: i32,
                   cache: &mut alloc::vec::Vec<(i32, alloc::string::String)>|
     -> alloc::string::String {
        if let Some((_, p)) = cache.iter().find(|(f, _)| *f == fd) {
            return p.clone();
        }
        let p = tk
            .try_fdtable()
            .and_then(|fdt| fdt.get_file(fd as usize))
            .map(|file| file.path())
            .unwrap_or_else(|| alloc::string::String::from("?"));
        cache.push((fd, p.clone()));
        p
    };
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let mut printed = 0u64;
    // sc88: scan deep (callers sat below the 24-slot window in sc84-87)
    // and keep ONLY file-backed candidates — pure heap/stack pointers
    // drowned the signal (every dump showed data pointers, never the
    // code chain). sc90: the filter is `base offset != 0 || fd >= 0`:
    // musl's loader CLOSES the mapping fd after mmap, so loader-mapped
    // code VMAs carry fd=-1 with a real offset (sc89 nbr dump: the
    // holder's only survivors were two fd>=0 DATA-segment pointers —
    // the fd<0 drop had removed the entire code chain).
    for i in 0..scan_slots {
        if printed >= max_frames {
            break;
        }
        let Some(v) = read_u64_at(rsp + 8 * i) else { continue };
        if v < 0x1000 || v > 0x7fff_ffff_ffff {
            continue;
        }
        if let Some((s0, e0, fd, base, off, fsz)) = vma_of(v) {
            if fd < 0 && base == 0 {
                continue; // anonymous (heap/stack): data pointers only
            }
            let bytes = read_u64_at(v).unwrap_or(0);
            let path = if fd >= 0 {
                path_of(fd, &mut path_cache)
            } else {
                alloc::string::String::from("-")
            };
            let _ = write!(
                s,
                "\nRWFRZ:   frame ustack[{}]={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} baseoff={:#x} filesz={:#x} bytes={:#018x} path={}",
                i, v, s0, e0, fd, off, base, fsz as usize, bytes, path
            );
            printed += 1;
        }
    }
    // sc120 (Session P): the app-child park hunt needs the RAW frame
    // chain — the file-backed-only scan below DROPS return addresses
    // into the loader-mapped musl VMA at 0x3fbf000000 (fd=-1 AND
    // baseoff==0 for the merged first segment: filtered on both
    // clauses), so the musl syscall -> __timedwait -> pthread_primitive
    // -> RUNTIME-caller chain never printed and the only survivors were
    // stale decoys (a libc++ string-assign return at slot 1). 24 raw
    // words = 3 lines: enough to see the whole chain unfiltered.
    {
        use core::fmt::Write;
        let mut raw = alloc::string::String::new();
        let _ = write!(raw, "\nRWFRZ:   {} raw ustack:", label);
        for i in 0..24 {
            if let Some(v) = read_u64_at(rsp + 8 * i) {
                let _ = write!(raw, " {}", v);
            } else {
                break;
            }
        }
        rwfz_emit_prio(&raw, prio);
    }
    // 70th (ohw84): RETS — call-byte-validated return-address scan over a
    // deep window (the apppark_rets method): a slot is a REAL frame only
    // when the 8 bytes ending at it contain a CALL encoding. Raw-window
    // frames proved unreliable (stale decoys from earlier locks in the
    // same stack region decided nothing); the validated chain names the
    // OsalMutexLock callers exactly.
    {
        use core::fmt::Write;
        let mut rets = alloc::string::String::new();
        let _ = write!(rets, "\nRWFRZ:   {} rets:", label);
        let mut found = 0u32;
        for i in 0..96u32 {
            if found >= 14 {
                break;
            }
            let Some(v) = read_u64_at(rsp + 8 * i as u64) else {
                continue;
            };
            if v < 0x1000 || v > 0x7fff_ffff_ffff {
                continue;
            }
            if vmas
                .find(crate::mm::types::VirtAddr::new(v as usize))
                .is_none()
            {
                continue;
            }
            let Some(w) = read_u64_at(v - 8) else {
                continue;
            };
            let b = w.to_le_bytes();
            let is_call = b[3] == 0xE8
                || (b[6] == 0xFF && (b[7] & 0xF8) == 0xD0)
                || (b[4] == 0xFF && (b[5] & 0xC7) == 0x05);
            if !is_call {
                continue;
            }
            if let Some((s0, e0, fd, _base, off, _fsz)) = vma_of(v) {
                let _ = write!(
                    rets,
                    "\nRWFRZ:   {}  ret[{}]={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} path={}",
                    label,
                    i,
                    v,
                    s0,
                    e0,
                    fd,
                    off,
                    if fd >= 0 {
                        path_of(fd, &mut path_cache)
                    } else {
                        alloc::string::String::from("-")
                    }
                );
                found += 1;
            }
        }
        if found == 0 {
            let _ = write!(rets, " none");
        }
        rwfz_emit_prio(&rets, prio);
    }
    rwfz_emit_prio(&alloc::format!("RWFRZ:   {} frames:{}", label, s), prio);
}

// ═══════════════ futex_pi ═════════════════════════════════════════
// (was futex_pi.rs)
// PI futex state engine (priority-inheritance futex operations).
//
// Reference semantics (futex(2)): FUTEX_LOCK_PI / FUTEX_UNLOCK_PI /
// FUTEX_TRYLOCK_PI / FUTEX_LOCK_PI2 implement a PI mutex on a 32-bit
// user word whose low bits carry the OWNER TID and whose high bits
// are the FUTEX_WAITERS (0x80000000) and FUTEX_OWNER_DIED
// (0x40000000) flags. FUTEX_WAIT_REQUEUE_PI / FUTEX_CMP_REQUEUE_PI
// (implemented beside the plain wait queue in the futex section, driving this
// engine) move a plain waiter from a non-PI futex onto the PI queue
// of a second futex without waking it, so a condvar->mutex handoff
// costs one wake.
//
// Design (Rux-native, external ABI identical):
// - All PI state lives in two fixed pools. A PiState exists ONLY
//   while at least one waiter exists — an uncontended PI lock is
//   just the user word, exactly like the reference's lazily attached
//   pi_state. No per-Task fields: a task's held-PI-locks and
//   blocked-on relationships are derived by scanning the pools; PI
//   operations are rare next to plain futexes, so O(pool) slow paths
//   are fine and the Task-struct dual-construction hazard does not
//   apply.
// - The per-hash PI bucket lock (PI_BUCKET_HEADS) serializes every
//   user-word mutation and every state-chain edit, the same contract
//   the plain side implements with its hash buckets.
// - Priority inheritance follows the reference rt_mutex_setprio
//   shape (RT-2X item 7): the boost is driven by the TOP waiter's
//   scheduling class. An RT (FIFO/RR) top waiter raises the owner's
//   rt_priority to max(owner, waiter) — or converts a CFS owner to
//   SCHED_FIFO at the waiter's priority — via the GRQ-locked
//   class/priority migration (change_task_policy, which re-links the
//   priority-array bucket). A CFS top waiter applies the old
//   nice-floor (owner's nice pulled down to the waiter's). The
//   owner's saved (policy, rt_priority, nice) is restored at
//   handoff/release; ownership handoff re-arms the boost on the
//   incoming owner while waiters remain. The nice-only floor this
//   module shipped first could NOT break an RT inversion (a FIFO
//   medium-prio spinner starves a FIFO low-prio lock holder forever
//   — pi_stress wedged exactly there: run/RT-2X.md). Chains (the
//   owner itself blocked on another PI lock) are walked for deadlock
//   detection and every link boosts transitively through the same
//   pool scan.
// - Lock ordering (INV-LOCK family): plain futex bucket -> PI bucket
//   -> pool slot. cmp_requeue_pi (in the futex section) is the only operation
//   holding a plain bucket while taking PI locks, and it always takes
//   them in that order. Task::wake_up runs under the PI bucket lock,
//   matching the plain side's C8 invariant.
// - CHAIN PROPAGATION (R6, reference rt_mutex_adjust_prio_chain):
//   the boost/deboost of an owner must travel up the blocked-on
//   chain — A holds C, B holds D and blocks on C, T blocks on D:
//   boosting T boosts B (single edge), and B's NEW priority must
//   re-rank B's waiter entry on C and boost A in turn. The walk
//   (`pi_chain_walk`) mirrors the reference's step loop: one lock at
//   a time, requeue the blocked owner's waiter entry with its live
//   priority, re-evaluate the edge, follow to the next owner. Two
//   Rux-native structural differences, both recorded in
//   docs/architecture/design-divergences.md: (1) the waiter's boost
//   input is evaluated LIVE from the task (the reference snapshots
//   waiter->tree.prio and requeues inside the walk — our pool scan
//   makes every edge derivation from current state anyway); (2) the
//   walk holds at most ONE PI bucket at a time (never nested), so
//   bucket-order deadlocks are impossible by construction where the
//   reference needs the pi_lock/wait_lock trylock dance.
// - KERNEL RT-MUTEXES (R6): SleepMutex instances constructed with
//   `SleepMutex::new_rt` register PI states in the SAME pools with a
//   FLAGS_KERNEL_RT key (uaddr = the mutex address, mm = 0). Their
//   RT-class waiters join the priority chain, so a FIFO task blocked
//   on the ext4 big lock boosts the lock's CFS owner, and if that
//   owner is itself blocked on a user PI futex the boost keeps
//   walking — one chain engine for both worlds, the reference's
//   shared task->pi_blocked_on shape. Kernel keys never touch a user
//   word: every word_read/word_cas on a PI state is guarded by
//   `key_is_kernel` (the key's uaddr is a kernel static — writing it
//   would corrupt kernel memory).

/// Set while any waiter is queued on the futex (reference bit 31).
pub const FUTEX_WAITERS: u32 = 0x8000_0000;
/// Set by the kernel when a PI futex owner terminated holding the lock
/// (reference bit 30). The next acquirer observes it and returns
/// EOWNERDEAD after taking ownership (robust-mutex recovery contract).
pub const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
/// Everything below the two flag bits is the owner TID.
const TID_MASK: u32 = !(FUTEX_WAITERS | FUTEX_OWNER_DIED);
/// Key flag marking a KERNEL rt-mutex PI state (SleepMutex::new_rt):
/// uaddr is the mutex's kernel address, mm is 0, and no user word is
/// ever read or written through such a key.
pub const FLAGS_KERNEL_RT: u32 = 0x0040;
/// Cap on the deadlock-chain walk (the reference rt_mutex walk has the
/// same depth guard; hitting it is treated as a cycle).
const MAX_PI_CHAIN: usize = 16;

/// True when `key` names a kernel rt-mutex (see FLAGS_KERNEL_RT) —
/// every user-word access on a PI state must be skipped for these.
fn key_is_kernel(key: &FutexKey) -> bool {
    key.flags & FLAGS_KERNEL_RT != 0
}

/// Kernel rt-mutex key for a SleepMutex static (R6).
fn kernel_key(mutex: usize) -> FutexKey {
    FutexKey {
        uaddr: mutex,
        mm: 0,
        flags: FLAGS_KERNEL_RT,
        phys: 0,
        object: false,
    }
}

/// Live scheduling configuration of `task` as a waiter boost input:
/// Some(rt_prio) for FIFO/RR, None for CFS-class (keyed by nice).
/// SAFETY: caller guarantees `task` is a valid Task (queued waiter or
/// pid-resolved owner). Reads are plain field loads; a task blocked in
/// a PI park changes these only via PI boosts, so a torn (policy, prio)
/// pair is a transient a later walk re-evaluates (priority is a
/// scheduling hint, not a memory-safety datum).
fn task_live_rank(task: *mut Task) -> Option<(Option<u32>, i32)> {
    let tr = crate::sched::r#unsafe::task_shared(task);
    if task.is_null() {
        return None;
    }
    let policy = crate::sched::r#unsafe::task_shared(task).policy();
    let rt = if matches!(policy, SchedPolicy::Fifo | SchedPolicy::Rr) {
        Some(crate::sched::r#unsafe::task_shared(task).rt_priority())
    } else {
        None
    };
    Some((rt, crate::sched::r#unsafe::task_shared(task).nice()))
}

/// Total order on waiters: RT waiters first (higher rt_priority wins),
/// then CFS by nice ascending; equal keys keep arrival (FIFO) order.
/// Returns (class, rank) — smaller wins.
fn waiter_rank(idx: usize) -> (u8, u32) {
    let w = PI_WAITER_POOL[idx].lock_irqsave();
    match w.as_ref() {
        Some(w) => match w.rt_prio {
            Some(p) => (0, u32::MAX - p.min(99)),
            None => (1, (w.nice + 20).clamp(0, 39) as u32),
        },
        None => (2, u32::MAX),
    }
}

// SAFETY: PiWaiter slots are only accessed while the per-slot spinlock
// is held; the task raw pointer is only dereferenced by the waker after
// a non-null check — the same contract as the plain Waiter.
unsafe impl Send for PiWaiter {}
unsafe impl Sync for PiWaiter {}

const PI_WAITER_POOL_SIZE: usize = 128;
const PI_STATE_POOL_SIZE: usize = 128;
const PI_BUCKETS: usize = 64;

static PI_WAITER_POOL:
    [super::types::Spinlock<Option<PiWaiter>>; PI_WAITER_POOL_SIZE] = {
        use super::types::Spinlock;
        const INIT: Spinlock<Option<PiWaiter>> = Spinlock::new(None);
        [INIT; PI_WAITER_POOL_SIZE]
    };
static PI_STATE_POOL:
    [super::types::Spinlock<Option<PiState>>; PI_STATE_POOL_SIZE] = {
        use super::types::Spinlock;
        const INIT: Spinlock<Option<PiState>> = Spinlock::new(None);
        [INIT; PI_STATE_POOL_SIZE]
    };
static PI_BUCKET_HEADS:
    [super::types::Spinlock<Option<usize>>; PI_BUCKETS] = {
        use super::types::Spinlock;
        const INIT: Spinlock<Option<usize>> = Spinlock::new(None);
        [INIT; PI_BUCKETS]
    };

pub(crate) fn pi_bucket_lock(key: &FutexKey) -> super::types::SpinlockIrqGuard<'static, Option<usize>> {
    PI_BUCKET_HEADS[(futex_hash(key) as usize) & (PI_BUCKETS - 1)].lock_irqsave()
}

// ---------------------------------------------------------------------------
// User-word helpers. Every mutation happens under the PI bucket lock,
// which serializes all kernel-side accessors of the word (the same
// contract the plain side uses for WAKE_OP).
// ---------------------------------------------------------------------------

fn word_read(uaddr: usize) -> Option<u32> {
    crate::sync::r#unsafe::word_read(uaddr)
}

/// Compare-and-swap the user word: writes `new` only when it reads
/// `expect`. Returns the observed pre-value; callers compare against
/// `expect` to learn whether the swap happened.
fn word_cas(uaddr: usize, expect: u32, new: u32) -> Option<u32> {
    crate::sync::r#unsafe::word_cas(uaddr, expect, new)
}

// ---------------------------------------------------------------------------
// Pool management. Pool scans replace hash chains: states are scarce
// (one per CONTENDED pi futex) and PI operations are rare.
// ---------------------------------------------------------------------------

fn pi_waiter_alloc(task: *mut Task) -> Option<usize> {
    // Snapshot the scheduling configuration under the pool scan — the
    // waiter's class/priority cannot change while it is blocked, so the
    // enqueue-time snapshot is stable for the chain's lifetime.
    let (nice, rt_prio) = {
        let policy = crate::sched::r#unsafe::task_shared(task).policy();
        let rt = if matches!(policy, SchedPolicy::Fifo | SchedPolicy::Rr) {
            Some(crate::sched::r#unsafe::task_shared(task).rt_priority())
        } else {
            None
        };
        (crate::sched::r#unsafe::task_shared(task).nice(), rt)
    };
    for i in 0..PI_WAITER_POOL_SIZE {
        let mut slot = PI_WAITER_POOL[i].lock_irqsave();
        if slot.is_none() {
            *slot = Some(PiWaiter { task, nice, rt_prio, handed: false, next: None });
            return Some(i);
        }
    }
    None
}

pub(crate) fn pi_waiter_free(idx: usize) {
    let mut slot = PI_WAITER_POOL[idx].lock_irqsave();
    *slot = None;
}

pub fn pi_waiter_handed(idx: usize) -> bool {
    let w = PI_WAITER_POOL[idx].lock_irqsave();
    w.as_ref().map(|w| w.handed).unwrap_or(false)
}

/// Find the PiState for `key` (None when uncontended — no state).
fn pi_state_find(key: &FutexKey) -> Option<usize> {
    for i in 0..PI_STATE_POOL_SIZE {
        let st = PI_STATE_POOL[i].lock_irqsave();
        if let Some(ref s) = *st {
            if s.key.matches(key) {
                return Some(i);
            }
        }
    }
    None
}

/// Get or create the PiState for `key` owned by `owner_pid`.
fn pi_state_get_or_create(key: &FutexKey, owner_pid: u32) -> Option<usize> {
    if let Some(i) = pi_state_find(key) {
        return Some(i);
    }
    for i in 0..PI_STATE_POOL_SIZE {
        let mut st = PI_STATE_POOL[i].lock_irqsave();
        if st.is_none() {
            *st = Some(PiState {
                key: *key,
                owner_pid,
                owner_saved: None,
                waiters: None,
            });
            return Some(i);
        }
    }
    None
}

/// Drop the state when its last waiter left. Caller verified emptiness
/// or calls this opportunistically after an unlink.
fn pi_state_maybe_free(si: usize) {
    let mut st = PI_STATE_POOL[si].lock_irqsave();
    if let Some(ref s) = *st {
        if s.waiters.is_none() {
            *st = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Waiter chain operations (caller holds the PI bucket lock)
// ---------------------------------------------------------------------------

/// Insert ordered by `waiter_rank` (RT by rt_priority desc first, then
/// CFS nice asc), FIFO among equals — the same discipline as the
/// reference plist, extended with the RT class (RT-2X item 7).
fn pi_queue_insert(si: usize, wi: usize) {
    let rank = waiter_rank(wi);
    let mut st = PI_STATE_POOL[si].lock_irqsave();
    let s = match st.as_mut() {
        Some(s) => s,
        None => return,
    };
    let Some(head) = s.waiters else {
        s.waiters = Some(wi);
        return;
    };
    let head_rank = waiter_rank(head);
    if rank <= head_rank {
        let mut w = PI_WAITER_POOL[wi].lock_irqsave();
        if let Some(ref mut x) = *w {
            x.next = Some(head);
        }
        drop(w);
        s.waiters = Some(wi);
        return;
    }
    let mut prev = head;
    loop {
        let next = {
            let w = PI_WAITER_POOL[prev].lock_irqsave();
            w.as_ref().and_then(|w| w.next)
        };
        let Some(n) = next else { break };
        let n_rank = waiter_rank(n);
        if rank <= n_rank {
            let mut w = PI_WAITER_POOL[wi].lock_irqsave();
            if let Some(ref mut x) = *w {
                x.next = Some(n);
            }
            drop(w);
            let mut pw = PI_WAITER_POOL[prev].lock_irqsave();
            if let Some(ref mut p) = *pw {
                p.next = Some(wi);
            }
            return;
        }
        prev = n;
    }
    let mut pw = PI_WAITER_POOL[prev].lock_irqsave();
    if let Some(ref mut p) = *pw {
        p.next = Some(wi);
    }
}

/// Unlink waiter `wi` from state `si`; true when it was in the chain.
fn pi_queue_unlink(si: usize, wi: usize) -> bool {
    let mut st = PI_STATE_POOL[si].lock_irqsave();
    let s = match st.as_mut() {
        Some(s) => s,
        None => return false,
    };
    match s.waiters {
        None => false,
        Some(h) if h == wi => {
            let next = {
                let w = PI_WAITER_POOL[wi].lock_irqsave();
                w.as_ref().and_then(|w| w.next)
            };
            s.waiters = next;
            let mut w = PI_WAITER_POOL[wi].lock_irqsave();
            if let Some(ref mut x) = *w {
                x.next = None;
            }
            true
        }
        Some(h) => {
            let mut prev = h;
            loop {
                let next = {
                    let w = PI_WAITER_POOL[prev].lock_irqsave();
                    w.as_ref().and_then(|w| w.next)
                };
                match next {
                    None => return false,
                    Some(n) if n == wi => {
                        let wn = {
                            let w = PI_WAITER_POOL[wi].lock_irqsave();
                            w.as_ref().and_then(|w| w.next)
                        };
                        let mut pw = PI_WAITER_POOL[prev].lock_irqsave();
                        if let Some(ref mut p) = *pw {
                            p.next = wn;
                        }
                        let mut w = PI_WAITER_POOL[wi].lock_irqsave();
                        if let Some(ref mut x) = *w {
                            x.next = None;
                        }
                        return true;
                    }
                    Some(n) => prev = n,
                }
            }
        }
    }
}

/// Count the waiter chain of state `si`.
#[allow(dead_code)]
fn pi_state_waiter_count(si: usize) -> usize {
    let st = PI_STATE_POOL[si].lock_irqsave();
    let Some(ref s) = *st else { return 0 };
    let mut cur = s.waiters;
    drop(st);
    let mut n = 0;
    while let Some(wi) = cur {
        n += 1;
        let w = PI_WAITER_POOL[wi].lock_irqsave();
        cur = w.as_ref().and_then(|w| w.next);
    }
    n
}

/// Restore `task` to a saved scheduling configuration (the boost-end
/// half of the reference's rt_mutex_setprio). The GRQ-locked class
/// migration handles both the RT->RT priority drop and the FIFO->CFS
/// conversion; a pure nice change takes the renice path.
fn pi_restore_saved(task: *mut Task, sv: &SavedSched) {
    let cur_policy = crate::sched::r#unsafe::task_shared(task).policy();
    if cur_policy != sv.policy {
        crate::sched::hotpath::change_task_policy(task, sv.policy, sv.rt_prio);
        // change_task_policy does not restore the nice (it only moves
        // class/priority) — a boosted-then-converted owner returns to
        // its CFS weight here.
        crate::sched::hotpath::sched_renice_locked(crate::sched::r#unsafe::task_mut(task), sv.nice);
        return;
    }
    if matches!(sv.policy, SchedPolicy::Fifo | SchedPolicy::Rr) {
        if crate::sched::r#unsafe::task_shared(task).rt_priority() != sv.rt_prio {
            crate::sched::hotpath::change_task_policy(task, sv.policy, sv.rt_prio);
        }
        return;
    }
    if crate::sched::r#unsafe::task_shared(task).nice() != sv.nice {
        crate::sched::hotpath::sched_renice_locked(crate::sched::r#unsafe::task_mut(task), sv.nice);
    }
}

/// Re-evaluate the owner's boost against the current top waiter — the
/// single evaluation point called after every chain edit (insert,
/// unlink, ownership handoff; caller holds the PI bucket lock). This
/// is the reference's rt_mutex_adjust_prio / rt_mutex_setprio shape:
///
///   top waiter RT(p)  -> owner (kept RT) raised to max(p, own), or a
///                        CFS owner converted to SCHED_FIFO at p
///   top waiter CFS(n) -> CFS owner's nice pulled down to min(n, own)
///                        (an RT owner needs nothing — it already
///                        outranks every CFS waiter)
///   no waiters        -> restore the saved configuration
///
/// A changed top waiter first restores, then re-arms — an upgrade
/// (higher RT waiter arrived) and a downgrade (the head left) both
/// fall out of the same restore-then-apply sequence. The boost can
/// only ever apply to the LIVE owner: ownership moves re-point
/// `owner_pid` and route through here before returning (see
/// futex_unlock_pi / the owner-died handoff).
fn pi_adjust_owner_boost(si: usize) {
    let (owner_pid, head, saved) = {
        let st = PI_STATE_POOL[si].lock_irqsave();
        match st.as_ref() {
            Some(s) => (s.owner_pid, s.waiters, s.owner_saved),
            None => return,
        }
    };
    let task = crate::sched::hotpath::find_task_by_pid(owner_pid);
    if task.is_null() {
        // Owner gone (exited between edits): drop the saved slot; a
        // dead task's configuration is moot.
        if saved.is_some() {
            let mut st = PI_STATE_POOL[si].lock_irqsave();
            if let Some(ref mut s) = *st {
                s.owner_saved = None;
            }
        }
        return;
    }

    // The desired boost, derived from the top waiter's LIVE class and
    // priority (R6: a chained waiter — one that owns another PI lock —
    // is itself boosted while parked, so the enqueue-time snapshot in
    // its PiWaiter is stale by design; the reference reads the top
    // waiter task's current prio through rt_mutex_setprio the same
    // way). The snapshot fields stay authoritative for QUEUE ORDER
    // within the state (refreshed by pi_waiter_refresh_rank during
    // chain walks — the reference's requeue step).
    let desired = head.and_then(|wi| {
        let wtask = {
            let w = PI_WAITER_POOL[wi].lock_irqsave();
            w.as_ref().map(|w| w.task).unwrap_or(core::ptr::null_mut())
        };
        task_live_rank(wtask)
    });

    // Does the owner's LIVE configuration already equal the desired
    // boost? (Avoids restore+re-apply churn on every chain edit.)
    let already = |desired: Option<(Option<u32>, i32)>| -> bool {
        match desired {
            None => saved.is_none(),
            Some((Some(p), _)) => {
                matches!(crate::sched::r#unsafe::task_shared(task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr)
                    && crate::sched::r#unsafe::task_shared(task).rt_priority() >= p.min(99)
            }
            Some((None, n)) => {
                !matches!(crate::sched::r#unsafe::task_shared(task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr)
                    && crate::sched::r#unsafe::task_shared(task).nice() <= n
            }
        }
    };
    if already(desired) {
        return;
    }

    // Restore the pre-boost configuration first (no-op when the saved
    // slot is empty), then apply the new boost.
    if let Some(sv) = saved {
        pi_restore_saved(task, &sv);
        let mut st = PI_STATE_POOL[si].lock_irqsave();
        if let Some(ref mut s) = *st {
            s.owner_saved = None;
        }
    }
    let Some((rt_prio, nice)) = desired else { return };

    // Arm the saved slot from the owner's (now unboosted) live state.
    {
        let mut st = PI_STATE_POOL[si].lock_irqsave();
        if let Some(ref mut s) = *st {
            s.owner_saved = Some(SavedSched {
                nice: crate::sched::r#unsafe::task_shared(task).nice(),
                policy: crate::sched::r#unsafe::task_shared(task).policy(),
                rt_prio: crate::sched::r#unsafe::task_shared(task).rt_priority(),
            });
        }
    }
    match rt_prio {
        Some(p) => {
            let p = p.min(99);
            if matches!(crate::sched::r#unsafe::task_shared(task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr) {
                if crate::sched::r#unsafe::task_shared(task).rt_priority() < p {
                    // Same class, higher priority: the class migration
                    // re-links the priority-array bucket (change_task_policy
                    // keeps the policy when it matches).
                    crate::sched::hotpath::change_task_policy(task, crate::sched::r#unsafe::task_shared(task).policy(), p);
                }
            } else {
                // CFS owner under an RT waiter: convert to FIFO at the
                // waiter's priority (the reference rt_mutex_setprio
                // does exactly this via __setscheduler).
                crate::sched::hotpath::change_task_policy(task, SchedPolicy::Fifo, p);
            }
        }
        None => {
            if !matches!(crate::sched::r#unsafe::task_shared(task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr)
                && crate::sched::r#unsafe::task_shared(task).nice() > nice
            {
                crate::sched::hotpath::sched_renice_locked(crate::sched::r#unsafe::task_mut(task), nice);
            }
        }
    }
}

/// Ownership-handoff boost routing: the outgoing owner's boost ends
/// (its saved configuration is restored), the incoming owner's is
/// armed from the remaining chain. Called after `owner_pid` was
/// re-pointed (caller holds the PI bucket lock).
fn pi_owner_handoff(si: usize, old_pid: u32) {
    let saved = {
        let mut st = PI_STATE_POOL[si].lock_irqsave();
        match st.as_mut() {
            Some(s) => s.owner_saved.take(),
            None => return,
        }
    };
    if let Some(sv) = saved {
        let task = crate::sched::hotpath::find_task_by_pid(old_pid);
        if !task.is_null() {
            pi_restore_saved(task, &sv);
        }
    }
    pi_adjust_owner_boost(si);
}

/// Snapshot of state `si`'s key (Copy read under the slot lock; a
/// state's key is immutable while it exists). None when the slot is
/// empty (state freed).
fn pi_state_key(si: usize) -> Option<FutexKey> {
    let st = PI_STATE_POOL[si].lock_irqsave();
    st.as_ref().map(|s| s.key)
}

/// Snapshot of state `si`'s owner pid (0 when absent).
fn pi_state_owner(si: usize) -> u32 {
    let st = PI_STATE_POOL[si].lock_irqsave();
    st.as_ref().map(|s| s.owner_pid).unwrap_or(0)
}

/// Re-snapshot `task`'s waiter entry in state `si` from its LIVE
/// scheduling configuration and re-insert it at its new rank — the
/// reference chain-walk's requeue step ([7] rt_mutex_adjust_prio_chain:
/// dequeue, waiter_update_prio, enqueue). Caller holds si's PI bucket
/// lock. No-op when the task has no entry (it left the queue between
/// the pool scan and this critical section — the walk's fresh re-scan
/// makes that benign).
fn pi_waiter_refresh_rank(si: usize, task: *mut Task) {
    // Find the entry (pi_queue_unlink needs the index).
    let mut cur = {
        let st = PI_STATE_POOL[si].lock_irqsave();
        st.as_ref().and_then(|s| s.waiters)
    };
    let mut found = None;
    while let Some(wi) = cur {
        let (t, n) = {
            let w = PI_WAITER_POOL[wi].lock_irqsave();
            match w.as_ref() {
                Some(w) => (w.task, w.next),
                None => (core::ptr::null_mut(), None),
            }
        };
        if t == task {
            found = Some(wi);
            break;
        }
        cur = n;
    }
    let Some(wi) = found else { return };
    let Some((rt_prio, nice)) = task_live_rank(task) else { return };
    if !pi_queue_unlink(si, wi) {
        return;
    }
    {
        let mut w = PI_WAITER_POOL[wi].lock_irqsave();
        if let Some(ref mut x) = *w {
            x.rt_prio = rt_prio;
            x.nice = nice;
        }
    }
    pi_queue_insert(si, wi);
}

/// The R6 chain walk — reference rt_mutex_adjust_prio_chain, adapted to
/// the pool structure. `owner` is the task whose edge was JUST adjusted
/// by the caller (it queued a waiter on / removed a waiter from a state
/// owned by `owner`, or handed ownership to it): if `owner` is itself
/// blocked on another PI state, its live (possibly new) priority must
/// re-rank its waiter entry there and re-evaluate THAT edge, and so on
/// up the chain.
///
/// Step shape (reference numbers in brackets):
///   [2]  waiter = the state `owner` is blocked on (pool scan — our
///        task->pi_blocked_on equivalent)
///   [7]  requeue that waiter entry at its live priority
///   [10] follow to the state's owner; adjust its boost [11]
///   [12] if that owner is blocked in turn, continue
///
/// ONE bucket lock held at a time (never nested) — opposite-direction
/// walks cannot deadlock, and each step derives from freshly read
/// state, so a chain that moved between steps is followed correctly
/// from wherever it actually is now (the reference's next_lock-change
/// abort exists because its walk carries waiter pointers across steps;
/// our re-scan needs no such guard). Bounded by MAX_PI_CHAIN like the
/// reference's max_lock_depth.
fn pi_chain_walk(owner: *mut Task) {
    if owner.is_null() {
        return;
    }
    let mut cur = owner;
    for _ in 0..MAX_PI_CHAIN {
        // [2] What is cur blocked on? (Fresh scan every step.)
        let Some(si) = pi_blocked_state_of(cur) else { return };
        let Some(key) = pi_state_key(si) else { return };
        let _bkt = pi_bucket_lock(&key);
        // [7] re-rank cur's waiter entry with its live configuration.
        pi_waiter_refresh_rank(si, cur);
        // [10]/[11] re-evaluate the edge's owner boost.
        pi_adjust_owner_boost(si);
        let next_pid = pi_state_owner(si);
        drop(_bkt);
        if next_pid == 0 {
            return;
        }
        let next = crate::sched::hotpath::find_task_by_pid(next_pid);
        if next.is_null() || crate::sched::r#unsafe::task_shared(next).state().is_dead() {
            return;
        }
        cur = next;
    }
    // Depth cap hit: treat like the reference's max_lock_depth — the
    // queue-time pi_chain_hits screen reports true cycles as EDEADLK;
    // a walk this long without one is a pathological but acyclic
    // chain, and the boost state it leaves behind is re-evaluated by
    // the next edit on any of its edges.
}

/// The state index `task` is blocked on, if any (deadlock-chain walk).
fn pi_blocked_state_of(task: *mut Task) -> Option<usize> {
    for si in 0..PI_STATE_POOL_SIZE {
        let st = PI_STATE_POOL[si].lock_irqsave();
        let Some(ref s) = *st else { continue };
        let mut cur = s.waiters;
        drop(st);
        while let Some(wi) = cur {
            let w = PI_WAITER_POOL[wi].lock_irqsave();
            let (t, n) = match w.as_ref() {
                Some(w) => (w.task, w.next),
                None => (core::ptr::null_mut(), None),
            };
            drop(w);
            if t == task {
                return Some(si);
            }
            cur = n;
        }
    }
    None
}

/// Deadlock detection for queueing `start` on `target_si`: the new
/// edge start->target_si closes a cycle iff following the chain
/// target_si.owner -> (that owner's blocked-on state) -> (that state's
/// owner) ... reaches `start` (the reference's check at chain-walk step
/// [6]: rt_mutex_owner(lock) == top_task). The PRE-R6 walk started
/// from `start`'s own blocked-on edges — which are EMPTY for a task in
/// the middle of acquiring — and missed owner-side cycles (X owns A,
/// Y blocks on A, X blocks on B owned by Y: legal-looking edges, dead
/// graph; found by the R6 model's cycle scenario).
fn pi_chain_hits(start: *mut Task, target_si: usize) -> bool {
    let mut cur_pid = {
        let st = PI_STATE_POOL[target_si].lock_irqsave();
        match st.as_ref() {
            Some(s) => s.owner_pid,
            None => return false,
        }
    };
    for _ in 0..MAX_PI_CHAIN {
        if cur_pid == 0 {
            return false;
        }
        let cur = crate::sched::hotpath::find_task_by_pid(cur_pid);
        if cur.is_null() {
            return false;
        }
        if cur == start as *mut Task {
            return true; // the new edge loops back to the acquirer
        }
        let Some(si) = pi_blocked_state_of(cur) else {
            return false; // chain ends at a running owner
        };
        cur_pid = {
            let st = PI_STATE_POOL[si].lock_irqsave();
            st.as_ref().map(|s| s.owner_pid).unwrap_or(0)
        };
    }
    true
}

// ---------------------------------------------------------------------------
// FUTEX_LOCK_PI / FUTEX_LOCK_PI2 / FUTEX_TRYLOCK_PI / FUTEX_UNLOCK_PI
// ---------------------------------------------------------------------------

fn my_task() -> Option<*mut Task> {
    crate::sched::hotpath::current().map(|t| t as *mut Task)
}

/// Post-handoff user-word fixup (the reference's fixup_owner shape):
/// after a handoff marked us `handed`, the word must end holding
/// `mytid`, PRESERVING a latched FUTEX_OWNER_DIED so glibc's robust
/// recovery sees the dead owner. Returns the syscall result: 0 for a
/// normal handoff, -EOWNERDEAD when the death flag is (or must be)
/// set, -EAGAIN when the word moved to a foreign owner (userspace
/// broke the kernel-writer contract; glibc re-issues LOCK_PI).
/// Caller holds the PI bucket lock.
fn handed_fixup(uaddr: usize, mytid: u32) -> i64 {
    use crate::errno::constants::{EAGAIN, EFAULT, EOWNERDEAD};
    fn death_take(uaddr: usize, uval: u32, mytid: u32) -> i64 {
        use crate::errno::constants::{EAGAIN, EFAULT};
        // Reference contract (kernel/futex/pi.c has NO EOWNERDEAD
        // return): LOCK_PI returns 0 while the word carries
        // FUTEX_OWNER_DIED beside the new owner TID — glibc's robust
        // path reads the flag from the WORD (__futex_lock_pi64's
        // fatal-error switch does not accept -EOWNERDEAD) and reports
        // EOWNERDEAD to the application itself.
        let newv = mytid | FUTEX_OWNER_DIED | (uval & FUTEX_WAITERS);
        match word_cas(uaddr, uval, newv) {
            Some(v) if v == uval => 0,
            Some(_) => -(EAGAIN as i64),
            None => -(EFAULT as i64),
        }
    }
    let Some(uval) = word_read(uaddr) else {
        return -(EFAULT as i64);
    };
    let tid = uval & TID_MASK;
    if tid == mytid {
        // The handoff (or a prior fixup) already installed us. Return
        // 0 either way: a latched OWNER_DIED stays IN THE WORD for
        // glibc's robust recovery to see (reference contract).
        return 0;
    }
    if uval & FUTEX_OWNER_DIED != 0 {
        // Death-marked with the TID cleared (robust walk or the exit
        // path): install ourselves as the recovery owner.
        return death_take(uaddr, uval, mytid);
    }
    if tid == 0 {
        // Free word: the dying owner's CAS lost and nobody else queued.
        // We hold the handoff, the previous owner is gone — take it
        // with the death flag so robust users recover.
        return death_take(uaddr, uval, mytid);
    }
    -(EAGAIN as i64)
}

/// FUTEX_LOCK_PI: acquire the PI futex at `uaddr`, blocking under an
/// ABSOLUTE jiffies deadline (the timeout is absolute, like
/// FUTEX_WAIT_BITSET). Returns 0 acquired (word holds our TID — with
/// FUTEX_OWNER_DIED PRESERVED IN THE WORD when we inherited a dead
/// owner's lock; the reference kernel never returns -EOWNERDEAD here,
/// and modern glibc's __futex_lock_pi64 treats that errno as fatal —
/// robust recovery reads the flag from the word), -EDEADLK on
/// self/cycle, -ETIMEDOUT / -EINTR as usual.
pub fn futex_lock_pi(uaddr: usize, deadline: Option<u64>, shared: bool) -> i64 {
    use crate::errno::constants::{
        EAGAIN, EDEADLK, EFAULT, EINTR, EOWNERDEAD, ETIMEDOUT,
    };
    let Some(current) = my_task() else {
        return -(crate::errno::constants::EFAULT as i64);
    };
    let mytid = crate::sched::r#unsafe::task_shared(current).pid() as u32;

    loop {
        let Some(uval) = (word_read(uaddr)) else {
            return -(EFAULT as i64);
        };
        let tid = uval & TID_MASK;
        if tid == mytid {
            return -(EDEADLK as i64);
        }
        if tid == 0 {
            // Uncontended acquire; preserve a latched OWNER_DIED so the
            // new owner runs robust recovery (reference behavior).
            let newv = mytid | (uval & FUTEX_OWNER_DIED);
            match word_cas(uaddr, uval, newv) {
                Some(v) if v == uval => {
                    // Preserve the latched OWNER_DIED IN THE WORD and
                    // return 0 — glibc's robust recovery reads the
                    // flag from the word (the reference returns no
                    // EOWNERDEAD here either).
                    return 0;
                }
                Some(_) => continue, // raced; re-read
                None => return -(EFAULT as i64),
            }
        }

        // A TID with no live task — gone, or already ZOMBIE/DEAD (it
        // can never UNLOCK_PI now) — is the owner-died takeover path:
        // the next acquirer takes the word with FUTEX_OWNER_DIED latched
        // and returns EOWNERDEAD (the documented death contract robust
        // users recover from).
        let owner_live = {
            let t = crate::sched::hotpath::find_task_by_pid(tid as u32);
            if t.is_null() {
                false
            } else {
                !crate::sched::r#unsafe::task_shared(t).state().is_dead()
            }
        };
        if !owner_live {
            let newv = mytid | FUTEX_OWNER_DIED | FUTEX_WAITERS;
            match word_cas(uaddr, uval, newv) {
                // 0 with the death flag IN THE WORD (glibc contract —
                // see handed_fixup).
                Some(v) if v == uval => return 0,
                Some(_) => continue,
                None => return -(EFAULT as i64),
            }
        }

        // Latch WAITERS before queueing (reference order).
        if uval & FUTEX_WAITERS == 0 {
            match word_cas(uaddr, uval, uval | FUTEX_WAITERS) {
                Some(_) => {} // landed or raced; either way re-loop
                None => return -(EFAULT as i64),
            }
            continue;
        }

        // Queue on the PI state under the PI bucket lock.
        let mm = task_futex_mm_id(current);
        let key = FutexKey::new_user(
            uaddr,
            mm,
            if shared { self::FLAGS_SHARED } else { 0 },
        );
        let _bkt = pi_bucket_lock(&key);

        let Some(uval2) = (word_read(uaddr)) else {
            return -(EFAULT as i64);
        };
        let tid2 = uval2 & TID_MASK;
        if tid2 == mytid {
            return -(EDEADLK as i64);
        }
        if tid2 != tid || uval2 & FUTEX_WAITERS == 0 {
            continue; // ownership/flags raced; restart the acquire
        }

        let Some(si) = pi_state_get_or_create(&key, tid) else {
            return -(EAGAIN as i64); // state pool exhausted; caller
                                     // retries (glibc re-issues LOCK_PI)
        };
        let Some(wi) = pi_waiter_alloc(current) else {
            pi_state_maybe_free(si);
            return -(EAGAIN as i64);
        };

        // Dominated-assignment: every path that falls through the
        // unsafe block below has assigned walk_from (EDEADLK returns,
        // the owner-moved retry continues, the committed path assigns).
        #[allow(unused_assignments)]
        let mut walk_from: *mut Task = core::ptr::null_mut();
        {
            if pi_chain_hits(current, si) {
                pi_waiter_free(wi);
                pi_state_maybe_free(si);
                return -(EDEADLK as i64);
            }

            pi_queue_insert(si, wi);
            pi_adjust_owner_boost(si);
            // Owner moved between validation and queue? Unlink/retry.
            let owner_now = word_read(uaddr).map(|v| v & TID_MASK);
            if owner_now != Some(tid) {
                pi_queue_unlink(si, wi);
                pi_waiter_free(wi);
                pi_adjust_owner_boost(si);
                pi_state_maybe_free(si);
                continue;
            }
            // We are committed to park on this state: resolve the owner
            // for the post-publication chain walk (R6) while the state
            // is pinned under the bucket.
            walk_from = crate::sched::hotpath::find_task_by_pid(tid);

            // Park. C8 ordering: waiter state + INTERRUPTIBLE published
            // under the bucket lock, then drop, then schedule.
            crate::sched::r#unsafe::task_shared(current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }
        drop(_bkt);
        cpu::restore_irq(true);
        // R6 chain walk — the reference runs adjust_prio_chain from
        // task_blocks_on_rt_mutex BEFORE the blocker sleeps: our arrival
        // may have boosted the owner, and if that owner is itself
        // parked on another PI lock the boost must re-rank it there and
        // keep walking up. We are still executing (sleep state
        // published, not yet scheduled away), so the walk runs in our
        // own context exactly like the reference's. One bucket at a
        // time; a concurrent unlock that hands to us (handed=true)
        // finds our sleep state already published, so the wake pairs
        // with the schedule() below.
        pi_chain_walk(walk_from);
        let wake = deadline.map(|dl| {
            crate::timer::WakeArmed::arm_jiffy(
                dl,
                crate::sched::hotpath::get_current_pid(),
            )
        });
        crate::sched::hotpath::schedule();
        drop(wake);

        if pi_waiter_handed(wi) {
            // Ownership was transferred to us. Fix up the user word and
            // report the death contract (R6): a normal unlock_pi
            // handoff already wrote our TID (0); a death handoff (exit
            // cleanup / the robust walk) leaves FUTEX_OWNER_DIED
            // latched — install our TID beside it and return
            // EOWNERDEAD (the reference's fixup_owner; glibc's robust
            // recovery keys on exactly this pair).
            let ret = {
                let _bkt2 = pi_bucket_lock(&key);
                handed_fixup(uaddr, mytid)
            };
            pi_waiter_free(wi);
            return ret;
        }
        // Timed out / interrupted / spurious: leave the queue and end
        // or restart the acquire accordingly. The bucket lock is
        // RE-TAKEN for the unlink (RT-2X): the park dropped it, and an
        // unlock_pi handoff racing this removal could otherwise mark
        // `handed` on a freed (possibly reused) waiter slot and hand
        // the word to a task that already left.
        let timed = deadline.is_some_and(|dl| {
            crate::timer::get_jiffies() >= dl
        });
        {
            let mut walk_from: *mut Task = core::ptr::null_mut();
            {
                let _bkt2 = pi_bucket_lock(&key);
                {
                    pi_queue_unlink(si, wi);
                    pi_waiter_free(wi);
                    pi_adjust_owner_boost(si);
                    // Our departure may DEBOOST the owner (we were its
                    // top waiter): if that owner is itself parked on
                    // another PI lock, its stale-high rank there must
                    // relax too — resolve it while the state exists.
                    walk_from = crate::sched::hotpath::find_task_by_pid(pi_state_owner(si));
                    pi_state_maybe_free(si);
                }
            }
            // R6 deboost propagation (reference remove_waiter ->
            // adjust_prio_chain).
            pi_chain_walk(walk_from);
        }
        if timed {
            return -(ETIMEDOUT as i64);
        }
        if crate::signal::signal_pending() {
            return -(EINTR as i64);
        }
        // Spurious: loop re-runs the acquire (fast path if now free).
    }
}

/// FUTEX_TRYLOCK_PI: non-blocking acquire attempt.
pub fn futex_trylock_pi(uaddr: usize, shared: bool) -> i64 {
    use crate::errno::constants::{EAGAIN, EDEADLK, EFAULT, EOWNERDEAD};
    let Some(current) = my_task() else {
        return -(EFAULT as i64);
    };
    let mytid = crate::sched::r#unsafe::task_shared(current).pid() as u32;
    loop {
        let Some(uval) = (word_read(uaddr)) else {
            return -(EFAULT as i64);
        };
        let tid = uval & TID_MASK;
        if tid == mytid {
            return -(EDEADLK as i64);
        }
        if tid == 0 {
            let newv = mytid | (uval & FUTEX_OWNER_DIED);
            match word_cas(uaddr, uval, newv) {
                // Death flag preserved in the word; ret 0 (glibc
                // contract — see handed_fixup).
                Some(v) if v == uval => return 0,
                Some(_) => continue,
                None => return -(EFAULT as i64),
            }
        }
        // Contended: EWOULDBLOCK (== EAGAIN in the reference ABI).
        return -(EAGAIN as i64);
    }
}

/// FUTEX_UNLOCK_PI: release, handing ownership to the top waiter.
pub fn futex_unlock_pi(uaddr: usize, shared: bool) -> i64 {
    use crate::errno::constants::{EAGAIN, EFAULT, EPERM, ESRCH};
    let Some(current) = my_task() else {
        return -(EFAULT as i64);
    };
    let mytid = crate::sched::r#unsafe::task_shared(current).pid() as u32;

    let Some(uval) = (word_read(uaddr)) else {
        return -(EFAULT as i64);
    };
    if uval & TID_MASK != mytid {
        return -(EPERM as i64);
    }

    let mm = task_futex_mm_id(current);
    let key = FutexKey::new_user(
        uaddr,
        mm,
        if shared { self::FLAGS_SHARED } else { 0 },
    );
    let _bkt = pi_bucket_lock(&key);

    let Some(si) = pi_state_find(&key) else {
        // No kernel state: plain release of the word.
        return match word_cas(uaddr, uval, 0) {
            Some(v) if v == uval => 0,
            Some(_) => {
                // The word changed under us. If it still names us it
                // gained WAITERS (a locker latched the bit between our
                // read and CAS): retry once through the whole path.
                let now = word_read(uaddr);
                match now {
                    Some(v) if v & TID_MASK == mytid => {
                        drop(_bkt);
                        futex_unlock_pi(uaddr, shared)
                    }
                    Some(_) => -(EPERM as i64),
                    None => -(EFAULT as i64),
                }
            }
            None => -(EFAULT as i64),
        };
    };

    let top = {
        let st = PI_STATE_POOL[si].lock_irqsave();
        st.as_ref().and_then(|s| s.waiters)
    };
    let Some(wi) = top else {
        // State exists with no chain (transient): release + drop.
        pi_state_maybe_free(si);
        return match word_cas(uaddr, uval, 0) {
            Some(v) if v == uval => 0,
            Some(_) => -(EAGAIN as i64),
            None => -(EFAULT as i64),
        };
    };

    let (wtask, more) = {
        let w = PI_WAITER_POOL[wi].lock_irqsave();
        match w.as_ref() {
            Some(w) => {
                (w.task, w.next.is_some())
            }
            None => return -(ESRCH as i64),
        }
    };
    let wtid = crate::sched::r#unsafe::task_shared(wtask).pid() as u32;
    let newv = wtid | if more { FUTEX_WAITERS } else { 0 };
    match word_cas(uaddr, uval, newv) {
        Some(v) if v == uval => {}
        Some(_) => {
            // Userspace raced the word. If it no longer names us the
            // reference shape is EPERM; else a retry-able race.
            let now = word_read(uaddr).map(|x| x & TID_MASK);
            return if now != Some(mytid) {
                -(EPERM as i64)
            } else {
                -(EAGAIN as i64)
            };
        }
        None => return -(EFAULT as i64),
    }
    pi_queue_unlink(si, wi);
    {
        let mut w = PI_WAITER_POOL[wi].lock_irqsave();
        if let Some(ref mut x) = *w {
            x.handed = true;
        }
    }
    // Ownership moved to the top waiter; the boost follows the lock:
    // our own saved configuration is restored and the incoming owner
    // is re-armed from the remaining chain (RT-2X item 7 — the old
    // call left OUR boost on the state keyed to the NEW owner pid,
    // leaking the boost and corrupting whoever restored it later).
    {
        let mut st = PI_STATE_POOL[si].lock_irqsave();
        if let Some(ref mut s) = *st {
            s.owner_pid = wtid;
        }
    }
    pi_owner_handoff(si, mytid);
    let _ = Task::wake_up(wtask);
    pi_state_maybe_free(si);
    0
}

// ---------------------------------------------------------------------------
// requeue_pi engine (driven by the futex section's wait/cmp requeue ops)
// ---------------------------------------------------------------------------

/// Attach `task` (a FUTEX_WAIT_REQUEUE_PI sleeper requeued by
/// CMP_REQUEUE_PI) to the PI queue of `key2`. Returns the PiWaiter
/// index for the sleeper's handle. When `handed_direct` is set the
/// word already names `task` (proxy trylock won) and the waiter is
/// pre-marked handed so the sleeper's next wake ends its park.
///
/// Caller holds uaddr1's PLAIN bucket lock and `pi_bkt` (this PI
/// bucket) — the fixed plain->PI order.
pub fn pi_requeue_attach(
    pi_bkt: &mut super::types::SpinlockIrqGuard<'static, Option<usize>>,
    key2: &FutexKey,
    owner_tid: u32,
    task: *mut Task,
    handed_direct: bool,
) -> Option<usize> {
    let _ = pi_bkt; // ordering documented; guard held by caller
    let si = pi_state_get_or_create(key2, owner_tid)?;
    let wi = pi_waiter_alloc(task)?;
    pi_queue_insert(si, wi);
    if handed_direct {
        let mut w = PI_WAITER_POOL[wi].lock_irqsave();
        if let Some(ref mut x) = *w {
            x.handed = true;
        }
        let _ = Task::wake_up(task);
    } else {
        pi_adjust_owner_boost(si);
    }
    Some(wi)
}

/// Sleeper-side detach of a requeue_pi waiter (timeout / signal /
/// exit before handoff). Idempotent.
pub fn pi_requeue_detach(key2: &FutexKey, wi: usize) {
    let _bkt = pi_bucket_lock(key2);
    if let Some(si) = pi_state_find(key2) {
        pi_queue_unlink(si, wi);
        pi_adjust_owner_boost(si);
        pi_state_maybe_free(si);
    }
    pi_waiter_free(wi);
}

// ---------------------------------------------------------------------------
// Kernel rt-mutex hooks (SleepMutex::new_rt — R6)
// ---------------------------------------------------------------------------

/// Park-side PI registration for an RT-class waiter on a kernel
/// rt-mutex (a SleepMutex in rt mode). Creates (or reuses) the
/// kernel-keyed PiState owned by `owner_pid`, queues the waiter, arms
/// the owner's boost from the (live) top waiter, and propagates up the
/// owner's own blocked-on chain — a FIFO task parking on the ext4 big
/// lock boosts the lock's CFS owner immediately, and if that owner is
/// itself blocked on a user PI futex the walk keeps going.
///
/// Returns the (state, waiter) indices for the matching rt_waiter_leave.
/// None for CFS-class tasks (kernel rt-mutexes boost only for RT
/// waiters — the recorded divergence: no nice-floor churn on the fs big
/// locks, CFS contention keeps the R84 FIFO discipline unchanged) or on
/// pool exhaustion (PI degrades to none; the park itself proceeds).
pub fn rt_waiter_enter(
    mutex: usize,
    owner_pid: u32,
    task: *mut Task,
) -> Option<(usize, usize)> {
    {
        let policy = crate::sched::r#unsafe::task_shared(task).policy();
        if !matches!(policy, SchedPolicy::Fifo | SchedPolicy::Rr) {
            return None;
        }
        let key = kernel_key(mutex);
        let walk_from;
        let out = {
            let _bkt = pi_bucket_lock(&key);
            let si = pi_state_get_or_create(&key, owner_pid)?;
            let wi = match pi_waiter_alloc(task) {
                Some(wi) => wi,
                None => {
                    pi_state_maybe_free(si);
                    return None;
                }
            };
            pi_queue_insert(si, wi);
            pi_adjust_owner_boost(si);
            walk_from =
                crate::sched::hotpath::find_task_by_pid(pi_state_owner(si));
            Some((si, wi))
        };
        // Upward propagation (reference task_blocks_on_rt_mutex's
        // adjust_prio_chain) — OUTSIDE the bucket: the walk takes one
        // bucket at a time.
        pi_chain_walk(walk_from);
        out
    }
}

/// Leave-side: remove the parked waiter (it acquired the lock) and
/// re-evaluate the owner's boost, walking up when the departure
/// deboosts the owner (reference remove_waiter).
pub fn rt_waiter_leave(mutex: usize, si: usize, wi: usize) {
    let key = kernel_key(mutex);
    let mut walk_from: *mut Task = core::ptr::null_mut();
    {
        let _bkt = pi_bucket_lock(&key);
        {
            pi_queue_unlink(si, wi);
            pi_waiter_free(wi);
            pi_adjust_owner_boost(si);
            walk_from =
                crate::sched::hotpath::find_task_by_pid(pi_state_owner(si));
        }
        pi_state_maybe_free(si);
    }
    pi_chain_walk(walk_from);
}

/// Release-side boost routing (reference mark_wakeup_next_waiter): the
/// outgoing owner's boost ends (its saved configuration restored), the
/// incoming owner (`new_owner_pid` — the task the release's hi-prio
/// wake just picked) inherits from the remaining waiters. A no-op when
/// no kernel state exists (no RT waiter ever queued, or the last one
/// left and freed it).
pub fn rt_release_handoff(mutex: usize, new_owner_pid: u32) {
    let key = kernel_key(mutex);
    let _bkt = pi_bucket_lock(&key);
    if let Some(si) = pi_state_find(&key) {
        let old = pi_state_owner(si);
        {
            // Re-point BEFORE the handoff routing (same order as
            // futex_unlock_pi): the incoming owner inherits the boost
            // from the remaining chain.
            {
                let mut st = PI_STATE_POOL[si].lock_irqsave();
                if let Some(ref mut s) = *st {
                    s.owner_pid = new_owner_pid;
                }
            }
            pi_owner_handoff(si, old);
        }
        pi_state_maybe_free(si);
    }
}

// ---------------------------------------------------------------------------
// Exit cleanup (owner-died protocol)
// ---------------------------------------------------------------------------

/// Release every PI futex `task` holds and dequeue every PI queue it
/// waits in. Called beside futex_cleanup in do_exit. A dying owner
/// hands the lock to its top waiter with FUTEX_OWNER_DIED latched
/// (robust-mutex contract); the waiter's park ends OWNING the lock and
/// its post-park fixup (handed_fixup) installs its TID beside the
/// death flag, returning EOWNERDEAD — the reference's split is
/// handle_futex_death (word mark, no PI wake) + exit_pi_state_list
/// (wake the rt_mutex's top waiter); our combined path does both under
/// the one PI bucket lock, accepting BOTH word shapes (pre- and
/// post-robust-walk — exit_robust_list runs earlier in do_exit and
/// has already cleared the dead TID when it registered the mutex).
/// KERNEL rt-mutex states (FLAGS_KERNEL_RT) share the waiter-side and
/// boost routing but never touch a user word.
pub fn futex_pi_exit_cleanup(task: *mut Task) {
    // 1. Waiter side: leave every PI queue (no wake).
    while let Some(si) = pi_blocked_state_of(task) {
        let mut found = None;
        {
            let st = PI_STATE_POOL[si].lock_irqsave();
            if let Some(ref s) = *st {
                let mut cur = s.waiters;
                while let Some(wi) = cur {
                    let w = PI_WAITER_POOL[wi].lock_irqsave();
                    let (t, n) = match w.as_ref() {
                        Some(w) => (w.task, w.next),
                        None => (core::ptr::null_mut(), None),
                    };
                    drop(w);
                    if t == task {
                        found = Some(wi);
                        break;
                    }
                    cur = n;
                }
            }
        }
        let Some(wi) = found else { break };
        let mut walk_from: *mut Task = core::ptr::null_mut();
        {
            pi_queue_unlink(si, wi);
            pi_waiter_free(wi);
            pi_adjust_owner_boost(si);
            walk_from =
                crate::sched::hotpath::find_task_by_pid(pi_state_owner(si));
        }
        pi_state_maybe_free(si);
        // R6: the dying waiter's removal DEBOOSTS the owner — propagate
        // up its chain (reference remove_waiter walk).
        pi_chain_walk(walk_from);
    }

    // 2. Owner side: died holding PI locks — hand each to its top
    //    waiter with OWNER_DIED latched.
    let dead_tid = crate::sched::r#unsafe::task_shared(task).pid() as u32;
    for si in 0..PI_STATE_POOL_SIZE {
        let owner_key = {
            let st = PI_STATE_POOL[si].lock_irqsave();
            match st.as_ref() {
                Some(s) if s.owner_pid == dead_tid => Some(s.key),
                _ => None,
            }
        };
        let Some(key) = owner_key else { continue };
        let kernel = key_is_kernel(&key);
        let _bkt = pi_bucket_lock(&key);
        let top = {
            let st = PI_STATE_POOL[si].lock_irqsave();
            st.as_ref().and_then(|s| s.waiters)
        };
        match top {
            Some(wi) => {
                let (wtask, more) = {
                    let w = PI_WAITER_POOL[wi].lock_irqsave();
                    match w.as_ref() {
                        Some(w) => (w.task, w.next.is_some()),
                        None => {
                            pi_state_maybe_free(si);
                            continue;
                        }
                    }
                };
                let wtid = crate::sched::r#unsafe::task_shared(wtask).pid() as u32;
                let newv = wtid
                    | FUTEX_OWNER_DIED
                    | if more { FUTEX_WAITERS } else { 0 };
                if !kernel {
                    // Accept both word shapes: the raw contended word
                    // (dead_tid | WAITERS) AND the post-robust-walk
                    // shape (TID cleared, OWNER_DIED latched —
                    // exit_robust_list ran earlier in do_exit). A word
                    // already naming the incoming waiter or a foreign
                    // owner is left alone (userspace moved it; the
                    // handoff below still ends the park).
                    let _ = word_cas(key.uaddr, dead_tid | FUTEX_WAITERS, newv);
                    let _ = word_cas(
                        key.uaddr,
                        FUTEX_OWNER_DIED | FUTEX_WAITERS,
                        newv,
                    );
                    let _ = word_cas(key.uaddr, FUTEX_OWNER_DIED, newv);
                }
                // Even if the CASs lost (userspace moved the word), the
                // waiter must not sleep forever: hand unconditionally.
                pi_queue_unlink(si, wi);
                {
                    let mut w = PI_WAITER_POOL[wi].lock_irqsave();
                    if let Some(ref mut x) = *w {
                        x.handed = true;
                    }
                }
                // Re-point the state's owner BEFORE the handoff routing
                // so the incoming owner inherits the boost from the
                // remaining chain (the old call left owner_pid naming
                // the dead task).
                {
                    let mut st = PI_STATE_POOL[si].lock_irqsave();
                    if let Some(ref mut s) = *st {
                        s.owner_pid = wtid;
                    }
                }
                let _ = Task::wake_up(wtask);
                pi_owner_handoff(si, dead_tid);
                pi_state_maybe_free(si);
                // No chain walk from here: the dead owner's edges are
                // torn down by this very section, and the incoming
                // owner is being WOKEN (it is not parked anywhere) —
                // whichever lock it parks on next runs its own walk.
            },
            None => {
                // Dying owner with a state but no waiters: the word
                // KEEPS the dead TID (reference contract — a state only
                // exists after contention, and exit does not scrub an
                // uncontended word). The next LOCK_PI finds the dead
                // owner and takes over with EOWNERDEAD; robust users
                // rely on exactly that.
                pi_state_maybe_free(si);
            }
        }
    }
}

// ═══════════════ mutex ═════════════════════════════════════════
// (was mutex.rs)
// Sleeping mutex — the coarse-lock primitive (reference recipe).
//
// This is the kernel's mutex for locks that may be held across blocking
// I/O (VFS_MUTATION_LOCK, EXT4_BIG_LOCK). It follows the reference
// kernel's locking discipline (see run notes of the fix-lostpark lane,
// "Design review vs the reference kernel"):
//
// - Rule 0: a lock that may be held across blocking I/O is a MUTEX,
//   not a spinlock. Holders of this lock may sleep in virtio I/O for
//   milliseconds at a time; waiters therefore park on a wait queue
//   instead of spinning preempt-disabled (a preempt-disabled spinner
//   cannot be preempted to run the holder — the LTP r2 WEDGE family).
// - Rule 1: spinlocks never sleep. The only spinlock here is the
//   internal `state` word, held for a few instructions and never
//   across I/O.
// - Rule 2: the sleeping slowpath is queue-based and wake-safe:
//   `prepare_to_wait` registers the waiter and sets its sleep state,
//   the park site is the waiter's OWN `schedule()`, and release wakes
//   the queue head (FIFO, wake-one handoff). No bare yield exists
//   anywhere in the acquire.
//
// # Context restrictions
//
// **NOT usable from interrupt context.** A waiter must be able to call
// `schedule()`; hardirq/softirq/NMI context may never sleep. Two
// defensive degradations are kept from the fs-era implementation
// (behavioral compatibility):
//   - no current task (very early boot, secondary harts): degrade to
//     pure spinning — single-CPU boot paths, the holder is on this CPU;
//   - preempt_count != 0 (caller inside a spinlock section — a lock
//     ordering bug): degrade to pure spinning rather than refuse, but
//     this path must never be reached deliberately; a task that holds
//     a spinlock and then acquires a sleeping mutex violates the
//     hierarchy in docs/architecture/lock-ordering.md.
//
// # Fairness (R84/R85 history, preserved)
//
// Wake-one alone is not a handoff: a fresh fast-path caller can barge
// past a woken-but-not-yet-scheduled waiter indefinitely under KVM's
// .so page-fault churn (the b139 starvation: 10+ s D-state waiters
// while holders cycled). The acquire therefore gates fresh sleepable
// callers on a waiters count (parked tasks + unspent wake tokens),
// the woken waiter spends its token as unconditional ownership
// (reference mutex HANDOFF in miniature), and an orphaned handoff
// (wake dropped on an empty queue) is recovered through a handoff
// epoch plus an anti-starvation self-wake timer. All of this is
// bounded-latency insurance: in normal operation tokens flow FIFO and
// the overrides never fire.
//
// # Recursion
//
// Recursive per task (owner pid + depth): a loop device backed by a
// file on another ext4 instance re-enters ext4 from inside the block
// layer while holding the big lock, so nested acquire by the SAME
// task is legal and the outermost guard releases.
//
// # Lock-ordering position
//
// This primitive sits OUTSIDE the spinlock hierarchy: it is taken in
// syscall/task context, may be nested inside another SleepMutex
// (VFS_MUTATION_LOCK -> EXT4_BIG_LOCK is the sanctioned order), and
// everything it protects is above the state spinlock. It must never
// be taken while holding any spinlock.

/// R77-style stall tripwire support: is the recorded owner still a live
/// task? (pid-hash lookup; the stall tripwire only fires while the owner
/// lives — a DEAD owner means the lock leaked with its holder unless
/// release_if_owner ran.)
fn locked_owner_alive_wait(owner: &u32) -> bool {
    if *owner == 0 {
        return false;
    }
    let t = crate::process::hotpath::pid_hash_lookup_pinned(*owner);
    if t.is_null() {
        return false;
    }
    let alive = unsafe { !(*t).state().is_dead() };
    crate::process::types::Task::task_put(t);
    alive
}

impl SleepMutex {
    /// R84 fairness: a waiter older than this may take a FREE lock at
    /// the registration re-check even with other waiters queued — the
    /// self-heal that bounds any residual gate pathology at a few
    /// seconds of extra wait instead of an unbounded wedge.
    const ANTI_STARVE_JIFFIES: u64 = crate::config::KERNEL_HZ as u64 * 5;

    /// R84: stall tripwire threshold on the WAITER's own queue age.
    /// Normal parking lasts milliseconds; 3 s queued means the handoff
    /// is not reaching this waiter.
    const STALL_WAITER_JIFFIES: u64 = crate::config::KERNEL_HZ as u64 * 3;

    /// A waiter parked longer than this may take a free lock at the
    /// registration re-check even with other waiters queued: bounded-
    /// latency self-heal so the gate itself can never strand a free
    /// lock behind a stuck count.
    pub const fn new(name: &'static str, fsw_notes: bool) -> Self {
        Self {
            state: Spinlock::new(SleepMutexState {
                locked: false,
                owner: 0,
                depth: 0,
                acquired_at: 0,
                waiters: 0,
                handoff_epoch: 0,
                rt_waiters: 0,
            }),
            wait: WaitQueueHead::new(),
            name,
            fsw_notes,
            rt: false,
        }
    }

    /// R6: rt-mutex mode constructor — the locks that need PI
    /// semantics (the plan's "convert ONLY the locks that need PI"):
    /// RT-class waiters boost the owner through the shared PI engine
    /// and releases hand off to the highest-priority waiter (reference
    /// rt_mutex mark_wakeup_next_waiter). See the module doc for the
    /// divergence record: NO nice-floor boosts and NO spinlock-tree
    /// conversion — only FIFO waiters of RT class arm the chain.
    pub const fn new_rt(name: &'static str, fsw_notes: bool) -> Self {
        let mut s = Self::new(name, fsw_notes);
        s.rt = true;
        s
    }

    /// Try to take the lock / recurse into it, UNGATED: a free lock goes
    /// to whoever asks. Caller must NOT hold the state spinlock.
    ///
    /// R84: this is not the general acquire. It survives for exactly the
    /// callers the queue cannot serve: the non-sleeping spinner in
    /// lock() (no task context or preempt_count != 0 — it cannot
    /// register on the waitqueue, so gating it would livelock it behind
    /// a queue that never drains under load).
    ///
    /// The recursion arm requires `locked`: `owner == 0` also names the
    /// FREE state, and a pid-0 caller (get_current_pid() with no
    /// current task — the very contexts the degrade path serves) would
    /// otherwise "recurse into" a momentarily free lock during a
    /// handoff window (depth=1 on locked=false; the bogus guard's later
    /// release can then free a lock another task genuinely holds).
    /// Found by the S-1 loom harness (kernel/verify mutex_loom.rs).
    fn try_acquire_or_recurse(&'static self, pid: u32) -> Option<SleepMutexGuard> {
        let mut st = self.state.lock();
        if !st.locked {
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = crate::timer::get_jiffies();
            Some(SleepMutexGuard { lock: self })
        } else if st.locked && st.owner == pid {
            st.depth += 1;
            Some(SleepMutexGuard { lock: self })
        } else {
            None
        }
    }

    /// R84 fairness: the acquire for fresh sleepable callers. Identical
    /// to try_acquire_or_recurse except a FREE lock is only taken when
    /// no waiter is parked and no wake token is outstanding
    /// (`waiters == 0`) — everyone else must queue. This is what stops
    /// the barging: without it, any task on another CPU could take the
    /// just-released lock during the scheduling window of the waiter
    /// that release() had woken, and under KVM's .so-page-fault churn
    /// the bargers never stopped.
    ///
    /// The recursion arm requires `locked` for the same reason as in
    /// try_acquire_or_recurse: lock() runs its adaptive fair-try BEFORE
    /// the can_sleep check, so a pid-0 no-task caller reaches this arm;
    /// gated on the free branch (waiters > 0), it must be refused, not
    /// "recursed" into the free lock via the owner == 0 alias.
    fn try_acquire_or_recurse_fair(&'static self, pid: u32) -> Option<SleepMutexGuard> {
        let mut st = self.state.lock();
        if !st.locked && st.waiters == 0 {
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = crate::timer::get_jiffies();
            Some(SleepMutexGuard { lock: self })
        } else if st.locked && st.owner == pid {
            st.depth += 1;
            Some(SleepMutexGuard { lock: self })
        } else {
            None
        }
    }

    /// Optimistic-spin probe (one state-lock section + owner on-CPU
    /// check). Reference discipline: mutex_optimistic_spin only spins
    /// while the OWNER IS RUNNING on a CPU (mutex_spin_on_owner reads
    /// owner->on_cpu); the moment the holder is descheduled the
    /// reference stops spinning and queues. Spinning against an
    /// off-CPU holder is pure waste — and worse than waste under a
    /// convoy: on a 2-CPU box with 5-6 contenders on VFS_MUTATION_LOCK
    /// / EXT4_BIG_LOCK (ftest03 shape), spinners burning their whole
    /// optimistic budget per round starve the very holder whose
    /// critical section gates everyone (measured: 10 probe iterations
    /// 14s -> ~95s, 7x, LTP ftest03 census FAIL via the 30s budget
    /// TERM -> child TBROK unlink exits).
    ///
    /// Outcomes:
    /// - `Acquired`: the fair try succeeded (free+unwaited, or
    ///   recursion) — same gates as try_acquire_or_recurse_fair.
    /// - `Spin`: lock held by ANOTHER task that is on-CPU right now —
    ///   optimistic spinning is profitable, keep going.
    /// - `Stop`: stop spinning and queue — holder off-CPU (or its task
    ///   is gone), or the lock is free behind counted waiters (the R84
    ///   handoff window: a fresh caller must not take it, so there is
    ///   nothing to spin FOR).
    fn optimistic_probe(&'static self, pid: u32) -> OptimisticProbe {
        // One locked section: the fair-try gates + owner snapshot.
        let owner_pid = {
            let mut st = self.state.lock();
            if !st.locked && st.waiters == 0 {
                st.locked = true;
                st.owner = pid;
                st.depth = 1;
                st.acquired_at = crate::timer::get_jiffies();
                return OptimisticProbe::Acquired(SleepMutexGuard { lock: self });
            } else if st.locked && st.owner == pid {
                st.depth += 1;
                return OptimisticProbe::Acquired(SleepMutexGuard { lock: self });
            } else if !st.locked {
                // Free behind counted waiters: the handoff window. The
                // fair gate refuses us; the woken token holder is on its
                // way. Spinning here only burns the CPU it needs.
                return OptimisticProbe::Stop;
            }
            st.owner
        };
        // Holder task on-CPU? Pinned lookup keeps the Task alive across
        // the read (a racing release+exit would otherwise free it).
        let t = crate::process::hotpath::pid_hash_lookup_pinned(owner_pid);
        if t.is_null() {
            // Owner pid unresolvable (exiting / hash moment): cannot
            // prove spinning is profitable — queue. The R77 stall
            // tripwire separately watches a dead-owner leak.
            return OptimisticProbe::Stop;
        }
        let on_cpu = unsafe {
            (*t).ti_on_cpu.load(core::sync::atomic::Ordering::Acquire)
        };
        crate::process::types::Task::task_put(t);
        if on_cpu {
            OptimisticProbe::Spin
        } else {
            OptimisticProbe::Stop
        }
    }

    /// R84 fairness: spend the release() handoff token. The woken
    /// waiter takes a FREE lock unconditionally and leaves the waiter
    /// count in the same critical section — the count keeps covering the
    /// token holder for the whole wake-to-take window, which is what
    /// holds fresh callers at the gate until ownership has actually
    /// changed hands. A held lock or a lost race against another token
    /// holder returns None: the caller re-parks and keeps its count.
    ///
    /// The recursion arm requires `locked` (same owner == 0 alias as
    /// the other acquires; see try_acquire_or_recurse).
    fn try_acquire_wake_token(&'static self, pid: u32) -> Option<SleepMutexGuard> {
        let mut st = self.state.lock();
        if st.locked && st.owner == pid {
            st.depth += 1;
            Some(SleepMutexGuard { lock: self })
        } else if !st.locked {
            if st.waiters > 0 {
                st.waiters -= 1;
            }
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = crate::timer::get_jiffies();
            Some(SleepMutexGuard { lock: self })
        } else {
            None
        }
    }

    /// Acquire (sleep on contention, recursive per task).
    ///
    /// NOT for interrupt context (see module doc); syscall/task context
    /// only. Returns a guard that releases on drop (outermost only).
    pub fn lock(&'static self) -> SleepMutexGuard {
        match self.lock_inner(false) {
            Some(g) => g,
            // Unkillable acquisition: the only abort path is the
            // killable one below — unreachable here by construction.
            None => unreachable!("lock_inner(false) never aborts"),
        }
    }

    /// Killable acquisition (the reference's mutex_lock_killable /
    /// rwsem_down_write_killable family): identical to `lock`, but the
    /// park honors a pending fatal signal — a waiter whose SIGKILL is
    /// pending aborts the wait and returns None instead of parking
    /// forever. The abort is SAFE by construction: the waiter never
    /// held the mutex, so there is no critical section to unwind; the
    /// waiter count is left in the registration critical section and a
    /// dropped wake token is recovered by the existing R85 self-heals
    /// (handoff epoch + ANTI_STARVE override). Normal signals do NOT
    /// abort (the park only re-checks on a wake); only SIGKILL does —
    /// matching signal_pending_state(TASK_KILLABLE).
    ///
    /// oh-launch round 3: this is the foundation-wedge escape. A member
    /// SIGKILLed by exec's de_thread (or exit_group) while blocked in
    /// open(O_CREAT) on VFS_MUTATION_LOCK used to park UNINTERRUPTIBLE
    /// forever — signal_wake_up DID wake it, but the loop had no fatal
    /// arm and just re-parked — so the member never reached do_exit,
    /// the group never drained, and the exec'ing leader parked in
    /// de_thread forever (the 150 s foundation freeze, 3/3 boots).
    pub fn lock_killable(&'static self) -> Option<SleepMutexGuard> {
        self.lock_inner(true)
    }

    fn lock_inner(&'static self, killable: bool) -> Option<SleepMutexGuard> {
        let pid = crate::process::hotpath::current_pid();
        if self.fsw_notes {
            crate::debug::hotpath::note_biglock();
        }

        // Adaptive optimistic spin (reference mutex discipline): the
        // reference kernel's mutex_optimistic_spin / osq spins with a
        // generous budget while the holder is on-CPU, only falling to
        // the wait queue when need_resched fires, the budget ends, or
        // the HOLDER LEAVES THE CPU (mutex_spin_on_owner reads
        // owner->on_cpu every round). Our 4×64 budget (~100ns total)
        // fell through to the queue on nearly every SMP ping-pong
        // contention — the ctx-switch cost regression (0.73→1.85µs)
        // traced here. The holder-on-CPU guard is load-bearing: without
        // it the ftest03 convoy (5-6 tasks on the fs biglocks, 2 CPUs,
        // ms-long sections through jbd2) had spinners burning their
        // full budget against descheduled holders — starving the very
        // holder that gates everyone (10 probe iterations 14s -> ~95s,
        // 7x; LTP census FAIL via the 30s budget TERM). The R84 fair
        // gate is preserved inside the probe; need_resched still breaks
        // out immediately.
        'outer: for _ in 0..100 {
            match self.optimistic_probe(pid) {
                OptimisticProbe::Acquired(g) => return Some(g),
                // Holder off-CPU / handoff window: spinning is waste —
                // queue now (reference: mutex_spin_on_owner false).
                OptimisticProbe::Stop => break 'outer,
                OptimisticProbe::Spin => {}
            }
            for _ in 0..64 {
                core::hint::spin_loop();
                // Break the optimistic spin the moment the scheduler
                // wants us (reference: mutex_can_spin_on_owner checks
                // need_resched before each retry; we check per inner
                // batch, matching the reference's granularity).
                if crate::sched::hotpath::need_resched() {
                    break 'outer;
                }
            }
        }

        // Contended for real. If we have no task context (early boot /
        // IRQ) we cannot sleep — degrade to pure spin (see module doc:
        // IRQ-context use is a bug; this is a defensive fallback). A
        // task that holds a spinlock (preempt_count != 0) cannot sleep
        // EITHER: __schedule's preempt discipline refuses to switch it
        // out, so the wait-then-schedule loop below degenerates into a
        // hot retry loop whose dequeue_if_enqueued hammers the GRQ lock
        // and starves the other CPUs' timer ticks (the sendmsg02
        // stall-quiet wedge). Spin instead: interrupts stay enabled,
        // the scheduler is never touched, and whichever CPU runs the
        // holder can still take its ticks and release us.
        let can_sleep = crate::sched::hotpath::current().is_some()
            && crate::interrupt::preempt::preempt_count() == 0;
        if self.fsw_notes {
            crate::debug::hotpath::note_biglock_contend();
        }
        let cur = match crate::sched::hotpath::current() {
            Some(t) if can_sleep => t,
            _ => {
                loop {
                    if let Some(g) = self.try_acquire_or_recurse(pid) {
                        return Some(g);
                    }
                    for _ in 0..64 {
                        core::hint::spin_loop();
                    }
                }
            }
        };

        // R84 waiter bookkeeping:
        //  counted      <=> st.waiters already includes us (we are on the
        //                   queue or hold an unspent wake token); the count
        //                   is entered once at first registration and left
        //                   in the same critical section as a take.
        //  woken_retry  <=> schedule() just returned: the release() that
        //                   woke us spent its one handoff token on us.
        //  first_park   <=> jiffies of our FIRST registration; re-parks
        //                   keep it so the anti-starvation override below
        //                   measures true queue age.
        //  timer_id     <=> R85: anti-starve deadline wake armed for the
        //                   CURRENT park (0 = none). A parked waiter has
        //                   no waker of its own when a handoff is dropped
        //                   (empty-queue release) or its token is lost —
        //                   without this timer the ANTI_STARVE override
        //                   below can never fire, because it is only
        //                   evaluated on a loop iteration and iterating
        //                   requires a wake.
        let mut counted = false;
        let mut woken_retry = false;
        let mut first_park = 0u64;
        let mut timer_id = 0u64;
        // R6 rt mode: our PI waiter entry (kernel-keyed state index +
        // waiter index) once the first park commits; cleared by
        // rt_leave on every acquire exit below.
        let mut pi_q: Option<(usize, usize)> = None;

        loop {
            // KILLABLE ABORT (mutex_lock_killable shape): a waiter whose
            // SIGKILL is pending leaves the queue instead of re-parking —
            // the reference's signal_pending_state(TASK_KILLABLE) arm in
            // __mutex_lock_common's wait loop. Cleanup mirrors the take
            // paths: leave the waiter count under the state lock (when
            // counted), finish_wait removes the entry; the anti-starve
            // timer is already deleted by the post-schedule code above
            // every arrival at this arm. A wake token we held is dropped
            // on exit — the R85 handoff-epoch / ANTI_STARVE self-heals
            // recover the free lock for the remaining waiters (that is
            // their design purpose). Runs BEFORE the handoff spend so an
            // aborting waiter never takes a lock it will immediately
            // drop.
            if killable && crate::signal::fatal_signal_pending() {
                if counted {
                    let mut st = self.state.lock();
                    if st.waiters > 0 {
                        st.waiters -= 1;
                    }
                }
                self.wait.finish_wait(cur);
                if self.rt {
                    self.rt_leave(&mut pi_q);
                }
                return None;
            }
            if woken_retry {
                // Handoff: the free lock is ours if it is still free.
                // Losing it means another token holder won the race or a
                // spurious wake — fall through and re-park like any
                // contender (our count stays).
                if let Some(g) = self.try_acquire_wake_token(pid) {
                    counted = false;
                    if self.rt {
                        self.rt_leave(&mut pi_q);
                    }
                    return Some(g);
                }
                woken_retry = false;
            }
            if let Some(g) = self.try_acquire_or_recurse_fair(pid) {
                if self.rt {
                    self.rt_leave(&mut pi_q);
                }
                return Some(g);
            }
            // Register on the wait queue BEFORE re-checking the state
            // (R7-B6b discipline): any release() that pairs with this
            // attempt finds us queued and hands its wake token to us.
            // EXCLUSIVE + tail insert => FIFO, one wake per release.
            // The state set here is the waiter's OWN sleep state, set
            // immediately before its OWN schedule() below — reference
            // rule 2's park-site ownership. Killable acquisitions park
            // INTERRUPTIBLE (TASK_KILLABLE): signal_wake_up's kick then
            // reaches the park and the loop-top fatal arm decides; a
            // non-fatal wake is a spurious retry, same as any foreign
            // wake (the loop re-parks). Unkillable acquisitions keep
            // UNINTERRUPTIBLE — the reference's plain mutex_lock.
            self.wait.prepare_to_wait(cur, true, killable);
            // R85: snapshot the orphaned-handoff epoch BEFORE the
            // registration critical section below — a release() that
            // dropped its wake (empty queue) between this read and the
            // re-check shows up as an epoch change, letting us take the
            // free lock it stranded instead of parking past it.
            let my_epoch = {
                let st = self.state.lock();
                st.handoff_epoch
            };
            // R7-B6b re-check, fairness-aware (R84): count ourselves under
            // the state lock — once; re-parks keep the count — then take
            // the free lock ONLY when no OTHER waiter/token exists
            // (`waiters == 1`: us). That covers the release that raced
            // our registration (its wake found an empty queue — nobody
            // else can ever hand the lock out, so we must). With older
            // waiters queued the pending handoff belongs to them and we
            // sleep like everyone else. The anti-starvation override
            // (parked > ANTI_STARVE_JIFFIES and the lock is free) is the
            // self-heal that guarantees the gate can never strand a free
            // lock behind a stuck count. R85 adds the epoch term: a
            // handoff that was DROPPED (empty-queue release bumped the
            // epoch after we registered) belongs to nobody — take the
            // free lock now.
            {
                let now = crate::timer::get_jiffies();
                let mut st = self.state.lock();
                if !counted {
                    st.waiters += 1;
                    counted = true;
                    first_park = now;
                }
                // The recursion arm requires `locked` (owner == 0 also
                // names the free state; see try_acquire_or_recurse).
                let take = if st.locked && st.owner == pid {
                    // Unreachable while parked (a counted waiter has not
                    // taken the lock, so it cannot own it), kept so
                    // recursion is honored on every path.
                    st.waiters -= 1;
                    counted = false;
                    st.depth += 1;
                    true
                } else if !st.locked
                    && (st.waiters == 1
                        || st.handoff_epoch != my_epoch
                        || now.wrapping_sub(first_park)
                            > Self::ANTI_STARVE_JIFFIES)
                {
                    st.waiters -= 1;
                    counted = false;
                    st.locked = true;
                    st.owner = pid;
                    st.depth = 1;
                    st.acquired_at = now;
                    true
                } else {
                    false
                };
                drop(st);
                if take {
                    self.wait.finish_wait(cur);
                    // A concurrent release()'s wake_up_one may have seen
                    // us queued+sleeping and enqueued us between the
                    // registration and the acquire — undo it (the
                    // per-class on_rq guards make it a no-op when we
                    // were not actually enqueued). Mirrors Semaphore::down.
                    crate::sched::hotpath::dequeue_task(&*cur);
                    if timer_id != 0 && crate::timer::timer_pending(timer_id) {
                        crate::timer::del_timer(timer_id);
                    }
                    if self.rt {
                        self.rt_leave(&mut pi_q);
                    }
                    return Some(SleepMutexGuard { lock: self });
                }
            }
            // R6 rt mode: register the PI waiter once, when the first
            // park is committed (the registration re-check above said
            // the lock is not ours). RT-class waiters only — CFS
            // contention never touches the PI engine (the divergence
            // record: kernel rt-mutexes boost for FIFO/RR waiters, no
            // nice-floor churn on the fs big locks). Pool exhaustion
            // degrades to an un-boosted park (rt_waiter_enter's None).
            if self.rt && pi_q.is_none() {
                let owner_pid = {
                    let st = self.state.lock();
                    st.owner
                };
                if let Some((si, wi)) = self::rt_waiter_enter(
                    self as *const _ as usize,
                    owner_pid,
                    cur as *mut crate::process::types::Task,
                ) {
                    pi_q = Some((si, wi));
                    let mut st = self.state.lock();
                    st.rt_waiters += 1;
                }
            }
            // Sleep until release() wakes us (UNINTERRUPTIBLE for plain
            // lock(): sections protected by this lock are not signal-
            // interruptible and the guard must be carried back out of the
            // critical section; lock_killable() parks TASK_KILLABLE — see
            // the prepare_to_wait note above).
            //
            // R77 wedge-hunt tripwire: a holder that never releases
            // (killed synchronously inside the critical section — RAII
            // never runs through do_exit) shows up as held growing. R84:
            // a WAITER parked/retrying for many seconds while holders
            // cycle (held ~0) is the starvation shape the fairness gate
            // exists to kill. R85: the stranded-LOCK-FREE shape (owner=0,
            // waiters stuck, dropped handoff) is reported too — the b148
            // silent crawl printed nothing while the whole cohort sat in
            // D-state against a free lock. Rate-limited to one line per
            // 10 s.
            {
                let now = crate::timer::get_jiffies();
                let (owner, depth, held, nwaiters, locked) = {
                    let st = self.state.lock();
                    (
                        st.owner,
                        st.depth,
                        now.saturating_sub(st.acquired_at),
                        st.waiters,
                        st.locked,
                    )
                };
                let waited = now.wrapping_sub(first_park);
                let stranded_free = !locked && nwaiters > 0 && waited >= Self::STALL_WAITER_JIFFIES;
                if (waited >= Self::STALL_WAITER_JIFFIES
                    || held >= crate::config::KERNEL_HZ as u64 * 10)
                    && (stranded_free || locked_owner_alive_wait(&owner))
                {
                    static LAST_WAIT_REPORT: core::sync::atomic::AtomicU64 =
                        core::sync::atomic::AtomicU64::new(0);
                    let last = LAST_WAIT_REPORT.load(core::sync::atomic::Ordering::Relaxed);
                    if now.wrapping_sub(last) >= crate::config::KERNEL_HZ as u64 * 10
                        && LAST_WAIT_REPORT
                            .compare_exchange(
                                last,
                                now,
                                core::sync::atomic::Ordering::Relaxed,
                                core::sync::atomic::Ordering::Relaxed,
                            )
                            .is_ok()
                    {
                        let cur_pid = crate::process::hotpath::current_pid();
                        if stranded_free {
                            crate::pr_err!(
                                "{}-STALL-FREE waiter={} owner=none waiters={} waited={}j (dropped handoff / stuck count)",
                                self.name,
                                cur_pid,
                                nwaiters,
                                waited
                            );
                        } else {
                            crate::pr_err!(
                                "{}-STALL waiter={} holder={} depth={} held={}j waiters={} waited={}j",
                                self.name,
                                cur_pid,
                                owner,
                                depth,
                                held,
                                nwaiters,
                                waited
                            );
                        }
                    }
                }
            }
            // R85: arm the anti-starve self-wake. schedule() below blocks
            // until SOMETHING wakes us; when the handoff was dropped (the
            // empty-queue release) or our token is stolen, that someone
            // does not exist — without this timer the override above can
            // never evaluate and a stranded waiter sleeps indefinitely
            // (re-armed on every re-park so a held lock is polled at
            // ANTI_STARVE granularity, not hot-looped). Timer-table full
            // (id 0) degrades to the old wake-dependent behavior.
            if timer_id != 0 {
                crate::timer::del_timer(timer_id);
                timer_id = 0;
            }
            timer_id = crate::timer::add_timer_wakeup(
                crate::timer::get_jiffies() + Self::ANTI_STARVE_JIFFIES,
                pid,
            );
            crate::arch::cpu::restore_irq(true);
            crate::sched::hotpath::schedule();
            if timer_id != 0 {
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
                timer_id = 0;
            }
            self.wait.finish_wait(cur);
            // Woken: the release() that issued this wake spent its handoff
            // token on us — spend it at the top of the loop. (R85: a timer
            // or foreign wake ALSO lands here with no token; the token
            // spend fails or succeeds on lock state alone, so a spurious
            // retry is harmless.)
            woken_retry = true;
        }
    }

    /// R6 rt mode: leave the kernel PI queue (the attempt acquired the
    /// lock) and re-evaluate the boost chain upward (deboost of the
    /// previous owner's boost from OUR departure).
    fn rt_leave(&'static self, pi_q: &mut Option<(usize, usize)>) {
        if let Some((si, wi)) = pi_q.take() {
            self::rt_waiter_leave(
                self as *const _ as usize,
                si,
                wi,
            );
            let mut st = self.state.lock();
            st.rt_waiters = st.rt_waiters.saturating_sub(1);
        }
    }

    /// RAII alias, mirroring the older `Mutex::guard()` spelling.
    pub fn guard(&'static self) -> SleepMutexGuard {
        self.lock()
    }

    fn release(&'static self) {
        // Drop our nesting level FIRST, in every mode (the same order
        // release_if_owner uses): the woken waiter spends a HANDOFF
        // token through try_acquire_wake_token, which can only take a
        // lock the state machine has already freed (!locked). The rt-r6
        // landing ran the rt handoff BEFORE this section and returned
        // early on success — the mutex stayed locked with owner = the
        // departed releaser, the woken waiter's token spend failed
        // forever, and the OH boot wedged deterministically behind the
        // stuck lock (VFS-MUTATION-STALL holder growing, init blocked
        // >120 s inside its provisioning mutation; oh-launch bisect
        // 017bf556b..e9a0c1749, guilty commit 0b12e6e18).
        {
            let mut st = self.state.lock();
            if st.depth > 0 {
                st.depth -= 1;
                if st.depth == 0 {
                    st.locked = false;
                    st.owner = 0;
                }
            }
        }
        // R6 rt mode: with RT-class waiters queued the handoff picks
        // the HIGHEST-priority one (reference mark_wakeup_next_waiter)
        // and routes the boost to it — the boost we held ends, the
        // incoming owner inherits from the remaining chain. The lock is
        // already free per the section above; the woken waiter takes it
        // through the same token discipline as the FIFO handoff below.
        // No RT waiter queued (or the wake was refused): fall through
        // to the plain R84 FIFO-head wake, unchanged.
        if self.rt {
            let rtq = {
                let st = self.state.lock();
                st.rt_waiters
            };
            if rtq > 0 {
                if let Some(wtask) = self.wait.wake_up_one_hiprio() {
                    let wpid = unsafe { (*(wtask as *mut crate::process::types::Task)).pid() as u32 };
                    self::rt_release_handoff(
                        self as *const _ as usize,
                        wpid,
                    );
                    return;
                }
            }
        }
        // Hand the lock to one waiter. R84: this wake is a HANDOFF
        // token, not a retry hint — the woken waiter takes the free lock
        // unconditionally in lock(), and try_acquire_or_recurse_fair
        // keeps fresh callers queued behind it until the token is spent.
        // Still done outside the state spinlock: the waker takes the
        // waitqueue lock, the waiter's finish_wait takes it too — never
        // both at once. Waking an empty queue is a no-op (an outstanding
        // token holder takes the free lock on its own), so this is safe
        // even without waiter accounting.
        let woke = self.wait.wake_up_one();
        if woke == 0 {
            // R85: the handoff was DROPPED — the queue was empty, so
            // every counted waiter is mid-loop between finish_wait and
            // its next prepare_to_wait and the just-freed lock has no
            // taker: the fresh-caller gate (waiters > 0) bars everyone
            // else. Bump the orphaned-handoff epoch so the first
            // mid-loop waiter to re-register takes the free lock at its
            // re-check instead of parking past it. (No counted waiters
            // at all: the bump is inert — nobody's registration snapshot
            // can predate it.)
            let mut st = self.state.lock();
            if !st.locked && st.waiters > 0 {
                st.handoff_epoch += 1;
            }
        }
    }

    /// R73 exit-path release: force-drop the lock when its owner dies.
    ///
    /// do_exit() does not unwind Rust guards — a task killed while inside
    /// a critical section (signal during a slow TCG read, OOM kill,
    /// abort) leaks the lock forever: every later user then spins or
    /// sleeps on a dead owner. Called from do_exit only — the dying task
    /// can no longer make forward progress in its critical section, so
    /// dropping the lock is strictly better than stranding the whole
    /// subsystem. Hand out exactly ONE retry token, same as release():
    /// the woken waiter either takes the lock (and its later release()
    /// hands the next token down the queue) or re-sleeps — a storm of
    /// woken retryers is pointless load on the scheduler.
    pub fn release_if_owner(&'static self, pid: u32) {
        let was_held = {
            let mut st = self.state.lock();
            if st.locked && st.owner == pid {
                if st.depth != 1 {
                    crate::pr_err!(
                        "{}: force-released at exit: pid {} depth {} (nested sections abandoned)",
                        self.name,
                        pid,
                        st.depth
                    );
                }
                st.locked = false;
                st.owner = 0;
                st.depth = 0;
                true
            } else {
                false
            }
        };
        if was_held {
            // R6 rt mode: same hi-priority handoff + boost routing as
            // release() — a dying owner's boost ends with it, the next
            // (strongest) waiter takes over.
            if self.rt {
                let rtq = {
                    let st = self.state.lock();
                    st.rt_waiters
                };
                if rtq > 0 {
                    if let Some(wtask) = self.wait.wake_up_one_hiprio() {
                        let wpid = unsafe { (*(wtask as *mut crate::process::types::Task)).pid() as u32 };
                        self::rt_release_handoff(
                            self as *const _ as usize,
                            wpid,
                        );
                        return;
                    }
                }
            }
            let woke = self.wait.wake_up_one();
            if woke == 0 {
                // R85: same dropped-handoff recovery as release().
                let mut st = self.state.lock();
                if !st.locked && st.waiters > 0 {
                    st.handoff_epoch += 1;
                }
            }
        }
    }

    /// Diagnostic snapshot (VW forensic instrumentation): (locked, owner,
    /// depth, queued, woken-flagged). Called from the UART dump path (RX
    /// IRQ context) — uses TRY-locks only: a blocking lock here would
    /// self-deadlock against an interrupted holder on this CPU and wedge
    /// the dump itself. queued=usize::MAX means "list lock busy".
    pub fn vw_state(&'static self) -> (bool, u32, u32, usize, usize) {
        let mut locked = false;
        let mut owner = 0u32;
        let mut depth = 0u32;
        if let Some(st) = self.state.try_lock() {
            locked = st.locked;
            owner = st.owner;
            depth = st.depth;
        } else {
            owner = u32::MAX; // state lock busy marker
        }
        let (queued, woken) = if let Some((t, w)) = self.wait.vw_queue_census_try() {
            (t, w)
        } else {
            (usize::MAX, 0)
        };
        (locked, owner, depth, queued, woken)
    }

    /// R85 (b148 hunt): one SBI-console census line for the periodic
    /// taskdump — the b148 wedge was invisible because nothing printed
    /// the lock's own state (zero STALL prints while every service sat
    /// D-parked). Try-locks only (taskdump context may be the watchdog);
    /// busy locks print as '?' fields, never block.
    pub fn debug_census_line(&'static self) {
        use crate::debug::hotpath::{taskdump_dec, taskdump_raw_line};
        let (locked, owner, depth, waiters, epoch) = match self.state.try_lock() {
            Some(st) => (st.locked, st.owner, st.depth, st.waiters, st.handoff_epoch),
            None => (false, u32::MAX, 0, u32::MAX, u64::MAX),
        };
        let (queued, woken) = match self.wait.vw_queue_census_try() {
            Some(v) => v,
            None => (usize::MAX, 0),
        };
        taskdump_raw_line(self.name.as_bytes());
        taskdump_raw_line(b"-CENSUS locked=");
        taskdump_dec(locked as u64);
        taskdump_raw_line(b" owner=");
        taskdump_dec(owner as u64);
        taskdump_raw_line(b" depth=");
        taskdump_dec(depth as u64);
        taskdump_raw_line(b" waiters=");
        taskdump_dec(waiters as u64);
        taskdump_raw_line(b" epoch=");
        taskdump_dec(epoch);
        taskdump_raw_line(b" queued=");
        taskdump_dec(queued as u64);
        taskdump_raw_line(b" wokenflag=");
        taskdump_dec(woken as u64);
        taskdump_raw_line(b"\n");
    }
}

impl Drop for SleepMutexGuard {
    fn drop(&mut self) {
        self.lock.release();
    }
}

// ═══════════════ spinlock ═════════════════════════════════════════
// (was spinlock.rs)
// Spinlock with preempt / IRQ / BH variants.
//
// API:
//   lock()          — preempt disable + lock
//   lock_irq()      — disable interrupts + preempt disable + lock
//   lock_irqsave()  — save interrupt state + disable + preempt disable + lock
//   lock_bh()       — disable bottom-half (softirq) + lock
//
// Backend: TTAS (test-and-test-and-set) via compare_exchange.
// Ticket lock causes interactive-input deadlock on QEMU
// (likely QEMU's amoadd.w emulation bug), so TTAS is used for now.
//
// # Safety Invariants — Lock Ordering & Deadlock Prevention
//
// Lock nesting must always follow this outermost-to-innermost order.
// Violating this order risks deadlock.
//
// ```text
// Level 0 (outermost): IRQ disable (irq_save / irq)
//   └── Level 1: preempt_disable
//         ├── Level 2a: GRQ lock (sched/sched.rs)
//         │     └── Level 3a: per-zone lock (mm/zone.rs)
//         │     └── Level 3b: futex hash bucket lock (futex section)
//         │           └── Level 4: waiter slot lock
//         ├── Level 2b: process tree lock (process/task.rs)
//         ├── Level 2c: inode lock (fs/)
//         └── Level 2d: dentry cache lock (fs/)
// ```
//
// - **INV-LOCK-1**: `preempt_disable` must precede any spinlock acquire.
//
// - **INV-LOCK-2**: `irq_save` (or `irq`) must precede `preempt_disable`
//   when both are needed.
//
// - **INV-LOCK-3**: Release order is the reverse: unlock → preempt_enable →
//   irq_restore.
//
// - **INV-LOCK-4**: No lock acquisition cycles across levels (deadlock freedom).
//   In particular, a Level-3 lock must never be acquired while holding a
//   different Level-2 lock.
//
// - **INV-LOCK-5**: GRQ lock and futex hash bucket lock nesting direction is
//   consistent: GRQ may nest inside futex bucket, but never the reverse.
//
// - **INV-LOCK-6** (virtio-blk ABBA fix): a wait-queue lock
//   (`WaitQueueHead`'s internal `Spinlock<Vec<WaitQueueEntry>>`) must NEVER
//   be acquired while holding a virtio driver lock (`VIRTIO_PCI_BLK_LOCK`,
//   the MMIO device's `virtqueue` lock, `VIRTIO_*_PENDING`). Wait-queue
//   wakeups run `sched::wake_up_process` (GRQ lock) per waiter while
//   holding the wait-queue lock, so nesting it under a virtio lock builds
//   the BLK→waitqueue convoy that froze GNOME final6 (deadlock watchdog:
//   one CPU stuck on the virtio BSS lock, another on a heap wait-queue
//   lock, timer IRQs stopped). Drivers must COLLECT completed work under
//   their locks and deliver wakes OUTSIDE them (timer.rs R12-3 pattern);
//   see drivers/virtio/mod.rs and `assert_no_virtio_lock`.

// (INV-SPIN-1: the spinlock family — guards deref through the
// held-lock lifetime; the panic/deadlock-warn rows are PANIC-TIME
// BEST-EFFORT: they re-read the lock word and owner fields to print
// diagnostics while the machine is already failing — correctness is
// the pre-panic discipline, these reads tolerate garbage.)

// ==================== Acquire evidence (HEAL-GATE, 122nd) ====================

/// Freshness bound for acquire evidence, in jiffies. A live spinner
/// refreshes the stamp every ~0.07-2ms (KVM/native; TCG ~10x slower),
/// so 10 jiffies (100ms at 100Hz) is a 50-1300x margin over the
/// refresh period while staying 20x UNDER the heal's own 200-tick
/// (2s) bar — a leaked count is delayed by at most this window past
/// its last genuine contended spin.
pub const ACQUIRE_EVIDENCE_FRESH_JIF: u64 = 10;

/// The per-CPU acquire-evidence slots (see [`AcquireEvidence`]).
pub static ACQUIRE_EVIDENCE: [AcquireEvidence; crate::config::MAX_CPUS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AcquireEvidence = AcquireEvidence {
        task: AtomicUsize::new(0),
        jiffies: AtomicU64::new(0),
    };
    [ZERO; crate::config::MAX_CPUS]
};

/// Publish acquire-side evidence from lock()'s CONTENDED loop (called
/// once per SPIN_PREEMPT_PERIOD iterations; the uncontended fast path
/// never executes this). Costs one per-CPU slot read (the SAME
/// cpu_state().current word the tick-side heal adjudicates against —
/// writer and reader are symmetric by construction, no tp-vs-slot
/// skew possible), one jiffies read and two relaxed local stores per
/// 64K spins.
#[inline]
pub fn note_acquire_evidence() {
    let cpu = crate::arch::smp::cpu_id() as usize;
    if cpu >= ACQUIRE_EVIDENCE.len() {
        return;
    }
    // Same-CPU plain read of the scheduler's published current pointer
    // (the spinner cannot be migrated or preempted mid-loop, so this
    // is stable for the whole contended wait).
    let t = crate::sched::hotpath::cpu_state(cpu).current as usize;
    if t == 0 {
        return;
    }
    let ev = &ACQUIRE_EVIDENCE[cpu];
    ev.task.store(t, Ordering::Relaxed);
    ev.jiffies
        .store(crate::timer::get_jiffies(), Ordering::Relaxed);
}

/// Does `task` (a Task pointer) have LIVE acquire-side evidence on
/// `cpu` — i.e. was it observed inside lock()'s contended loop on
/// that CPU within [`ACQUIRE_EVIDENCE_FRESH_JIF`] jiffies of `now`?
/// Called from the tick-side PREEMPT-HEAL adjudication: a `true`
/// result means the incumbent's base count is acquire-coupled and
/// live (a slow legitimate waiter), and the heal must be refused.
#[inline]
pub fn acquire_evidence_fresh(cpu: usize, task: usize, now: u64) -> bool {
    if cpu >= ACQUIRE_EVIDENCE.len() {
        return false;
    }
    let ev = &ACQUIRE_EVIDENCE[cpu];
    ev.task.load(Ordering::Relaxed) == task
        && now.wrapping_sub(ev.jiffies.load(Ordering::Relaxed))
            <= ACQUIRE_EVIDENCE_FRESH_JIF
}

// ==================== RawSpinlock (TTAS) ====================

/// Capture this function's return address (the spinlock call site) for
/// deadlock diagnostics.
#[inline]
fn caller_return_address() -> usize {
    let ra: usize;
    #[cfg(feature = "riscv64")]
    // SAFETY: pure register read, no memory access or side effects.
    unsafe {
        core::arch::asm!("mv {}, ra", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(feature = "x86_64")]
    // SAFETY: frame pointers are forced on, so [rbp+8] is the return
    // address of this inlined function's caller.
    unsafe {
        core::arch::asm!("mov {}, qword ptr [rbp + 8]", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    {
        ra = 0;
    }
    ra
}

impl RawSpinlock {
    /// R11 (deadlock-layer unblinding): the raw lock word for the
    /// deadlock dumps — 0 = free, 1 = held, ANYTHING ELSE = the word
    /// itself was scribbled (the acquire spin waits on this exact word,
    /// so garbage here wedges every waiter with no holder anywhere).
    /// One relaxed load; cold diagnostic paths only.
    pub fn locked_word(&self) -> u32 {
        self.locked.load(Ordering::Relaxed)
    }

    #[inline]
    pub const fn new() -> Self {
        Self {
            locked: AtomicU32::new(0),
            #[cfg(feature = "debug-lock-owner")]
            owner: AtomicUsize::new(0),
            #[cfg(feature = "debug-lock-owner")]
            acq_pid: AtomicU32::new(0),
            #[cfg(feature = "debug-lock-owner")]
            acq_site: AtomicUsize::new(0),
            #[cfg(feature = "debug-lock-owner")]
            acq_jiffies: AtomicU64::new(0),
        }
    }

    /// Record the holder (debug-lock-owner feature only).
    #[inline]
    #[cfg(feature = "debug-lock-owner")]
    fn record_owner(&self, ra: usize) {
        // cpu_id() is u64 on x86_64 / usize elsewhere — cast to the
        // owner slot's usize.
        self.owner.store((crate::arch::smp::cpu_id() + 1) as usize, Ordering::Relaxed);
        // Leak forensics (see field docs): last acquirer's pid + site.
        self.acq_pid.store(crate::process::hotpath::current_pid(), Ordering::Relaxed);
        self.acq_site.store(ra, Ordering::Relaxed);
        self.acq_jiffies
            .store(crate::timer::get_jiffies(), Ordering::Relaxed);
    }

    /// Clear the holder (debug-lock-owner feature only).
    #[inline]
    #[cfg(feature = "debug-lock-owner")]
    pub(crate) fn clear_owner(&self) {
        self.owner.store(0, Ordering::Relaxed);
    }

    // ---- owner-in-word: the single-authority protocol (grq-source-fix) --
    //
    // Word encoding: 0 = free, `cpu + 1` = held by `cpu`. Every protocol
    // operation publishes lock state and ownership in ONE atomic:
    //   acquire  = cmpxchg(0 -> cpu+1)   (lock + owner, together)
    //   release  = store(0, Release)     (unlock + owner-clear, together)
    // The TTAS spin (`load() != 0`) and `is_locked()` are unchanged for
    // any nonzero encoding. There is no state in which the word is set
    // but unattributable: the orphaned-word class (word set, every
    // per-cpu tracker clear — the gl3 wedge) is unreachable by
    // construction because the tracker no longer exists as a separate
    // authority to disagree with it.

    /// The held word value for THIS cpu (`cpu + 1`; 1..=MAX_CPUS).
    #[inline]
    fn held_word() -> u32 {
        (crate::arch::smp::cpu_id() as u32) + 1
    }

    /// Which cpu holds this lock right now, DERIVED from the lock word
    /// (the single authority — never stale relative to it).
    ///
    /// The debug_assert is the ORPHAN-STATE DETECTOR (grq-source-fix):
    /// a set word whose value names no valid cpu can now only be
    /// produced by a wild store into the word itself, not by any
    /// interrupt/retry interleaving of the acquire/release protocol.
    /// It compiles out of release builds (hot-path rules); its silence
    /// across the boot batteries is the proof the state is unreachable.
    #[inline]
    pub fn holder_cpu(&self) -> Option<usize> {
        let w = self.locked.load(Ordering::Relaxed);
        debug_assert!(
            w == 0 || (w as usize) <= crate::config::MAX_CPUS,
            "spinlock word corrupted (no valid owner): {}",
            w
        );
        if w == 0 || (w as usize) > crate::config::MAX_CPUS {
            return None;
        }
        Some(w as usize - 1)
    }

    /// Positive knowledge that `cpu` holds this lock right now —
    /// derived from the lock word (single load, single authority).
    /// For same-cpu queries (the printk deferral gate, the terminal
    /// release) the word was written by this cpu's own acquire, so a
    /// Relaxed load observes it coherently; only this cpu's own release
    /// store can change the answer under us.
    #[inline]
    pub fn held_by(&self, cpu: usize) -> bool {
        self.locked.load(Ordering::Relaxed) == (cpu as u32).wrapping_add(1)
    }

    #[inline]
    #[cfg(not(feature = "debug-lock-owner"))]
    fn record_owner(&self, _ra: usize) {}

    #[inline]
    #[cfg(not(feature = "debug-lock-owner"))]
    pub(crate) fn clear_owner(&self) {}

    /// Spinlock deadlock threshold (iterations before warning).
    /// On SMP with QEMU emulation, brief contention is normal — PLIC IRQ
    /// claim/release, GRQ lock, etc. can take 10-100ms of spin time.
    /// 100M iterations ≈ 100-500ms depending on load/CAS latency.
    const DEADLOCK_WARN_ITERS: u32 = 100_000_000;

    /// RUNQ-STRAND fix (121st): preempt-exit sampling period, in spin
    /// iterations. Power of two so the check is a mask. At ~1-30ns per
    /// TTAS iteration (native/KVM) this probes need_resched every
    /// ~0.07-2ms — inside one tick period, so a preemptible spinner
    /// yields within a single 10ms slice. The check itself is one
    /// branch on a mask plus (rarely) a few loads; the uncontended
    /// fast path never executes any of this.
    const SPIN_PREEMPT_PERIOD: u32 = 65_536;

    /// RUNQ-STRAND fix (121st): may this spinner call schedule()?
    ///
    /// Every in-kernel acquire of a raw spinlock is preempt-coupled
    /// (Spinlock guards, GrqGuard, KeyedIrqGuard all add PREEMPT_OFFSET
    /// around the raw claim), so `preempt_count() == 0` proves the
    /// waiter holds NO spinlock at any nesting level and sits in no
    /// interrupt/softirq nesting. The two producers of a count==0
    /// spinner are (a) the tick-side PREEMPT-HEAL zeroing the count of
    /// a task legitimately inside a slow acquire (a false heal — the
    /// 121st census: accesstoken_ser/wallpaper_servi/netmanager spun
    /// at RawSpinlock::lock+0x52 for 500+s with pc=0 and need_resched=1
    /// after heals, each permanently monopolizing its CPU), and (b) a
    /// future uncoupled raw caller. For both, rescheduling from the
    /// WAIT loop is safe: nothing is acquired yet (the TTAS retry is
    /// idempotent across a context switch), and the guards exclude the
    /// deleted preemptible-wait's lost-park hazard (state must still be
    /// RUNNING — a wait path that prepared INTERRUPTIBLE before a
    /// guarded op must not sleep inside the acquire) and irq-off
    /// contexts. This is the reference's preempt_schedule_irq contract
    /// (a preemptible kernel loop must honor need_resched), placed at
    /// the only point where the full guard set is checkable.
    // RT-plan R1 completion note: this check's preempt_count()==0 term now
    // also covers raw locks acquired by THIS thread (raw lock/unlock bump
    // the count) — a nested holder stops yielding until it releases, the
    // reference's raw_spin_lock semantic.
    fn spin_may_preempt() -> bool {
        crate::sched::hotpath::need_resched()
            && crate::interrupt::preempt::preempt_count() == 0
            && crate::arch::cpu::get_interrupts_state()
            && crate::sched::hotpath::current()
                .map(|t| {
                    t.state().bits() == crate::process::types::TaskState::RUNNING
                })
                .unwrap_or(false)
    }

    #[inline(never)]
    pub fn lock(&self) {
        // Capture caller's return address before spinning
        let caller_ra = caller_return_address();
        let mut spins: u32 = 0;
        // TTAS (test-and-test-and-set, C7): one CAS to acquire; while
        // contended, spin on a plain Relaxed load until the word reads
        // free, then retry the CAS. Relaxed is sufficient for the spin
        // load: it only decides WHEN to retry — the acquiring CAS pairs
        // with the holder's Release store in unlock() and provides all
        // necessary synchronization. The old tight CAS loop kept issuing
        // an atomic RMW on every iteration, hammering the cacheline (and
        // the QEMU TCG atomic slow path) for the whole hold duration.
        loop {
            // Single cmpxchg publishes lock + owner together (see
            // `held_word`): the winner is self-describing in the word.
            if self
                .locked
                .compare_exchange(0, Self::held_word(), Ordering::Acquire, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
            while self.locked.load(Ordering::Relaxed) != 0 {
                spins = spins.wrapping_add(1);
                // RUNQ-STRAND fix: a provably-preemptible waiter must not
                // monopolize its CPU until the holder releases — yield
                // once per period when the tick asked us to. schedule()
                // clears need_resched at its entry, so the loop cannot
                // re-fire recursively out of the scheduler's own locks.
                if spins & (Self::SPIN_PREEMPT_PERIOD - 1) == 0 {
                    // HEAL-GATE (122nd): publish acquire-side evidence at
                    // the same 64K boundary — the tick-side PREEMPT-HEAL
                    // refuses to zero any count whose owner is inside
                    // this loop RIGHT NOW (a slow legitimate waiter on a
                    // stalled holder chain is not a leak). Contended path
                    // only; the fast path never reaches here.
                    note_acquire_evidence();
                    if Self::spin_may_preempt() {
                        crate::sched::hotpath::schedule();
                        spins = 0;
                        continue; // re-observe the lock word in the loop head
                    }
                }
                if spins == Self::DEADLOCK_WARN_ITERS {
                    Self::deadlock_warn(self as *const Self, caller_ra);
                    // Once-per-(lock,site) per boot (the dedup lives in
                    // deadlock_warn): a truly stuck lock re-fires every
                    // ~100M spins — hundreds of fires on the 32 s
                    // monopolist spins would flood the serial.
                    spins = 0; // continue spinning (might resolve)
                }
                core::hint::spin_loop();
            }
        }
        // Diagnostics: record the holder AFTER the acquire (a spinner's
        // watchdog may read a stale 0 in the tiny window — acceptable).
        self.record_owner(caller_ra);
        // RT-plan R1 completion (docs/development/rt-kernel-plan.md): the
        // holder is non-preemptible for the critical section — the
        // reference's raw_spin_lock semantics (CONFIG_PREEMPT). This makes
        // INV-LOCK-1 ("preempt_disable must precede any spinlock acquire")
        // automatic for every raw user, closing the sc70 family (the
        // irq-exit kernel-preempt hook preempting inside a raw-lock section
        // whose data is mid-mutation) that resurfaced as the intermittent
        // early-boot freeze when the hook went default-ON over the new
        // entity-embedded CFS timeline. The count is None-safe (early boot)
        // and nests (hardirq/softirq/grq chains each add their own).
        crate::interrupt::preempt::preempt_count_add(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
    }

    /// Print deadlock warning via SBI (works even with interrupts disabled).
    fn deadlock_warn(lock_addr: *const Self, caller_ra: usize) {
        // Raw UART output — printk might need locks we're spinning on.
        // uart_putc_raw, NOT putchar_no_lock: the latter is compiled out
        // of release builds, so this warn printed NOTHING on every
        // production-config boot (the round-2 monopolist spun 32 s in
        // kill() with zero DEADLOCK lines for exactly this reason).
        // Once per (lock, site) per boot — see the call site.
        {
            static SEEN: [core::sync::atomic::AtomicUsize; 64] =
                [const { core::sync::atomic::AtomicUsize::new(0) }; 64];
            let key = (lock_addr as usize >> 4) ^ (caller_ra << 8);
            let slot = &SEEN[key % 64];
            if slot.load(Ordering::Relaxed) == key {
                return;
            }
            slot.store(key, Ordering::Relaxed);
        }
        let cpu = crate::arch::smp::cpu_id();
        let msg = b"DEADLOCK: spinlock stuck cpu=";
        for &b in msg {
            crate::console::uart_putc_raw(b);
        }
        // Print CPU id as decimal digit
        if cpu < 10 {
            crate::console::uart_putc_raw(b'0' + cpu as u8);
        }
        // Print lock address in hex
        let msg2 = b" lock=0x";
        for &b in msg2 {
            crate::console::uart_putc_raw(b);
        }
        let addr = lock_addr as usize;
        let mut shift = (core::mem::size_of::<usize>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (addr >> (shift as usize)) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            crate::console::uart_putc_raw(c);
        }

        // Print caller return address (ra) for debugging
        let msg3 = b" ra=0x";
        for &b in msg3 {
            crate::console::uart_putc_raw(b);
        }
        let mut shift = (core::mem::size_of::<usize>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (caller_ra >> shift) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            crate::console::uart_putc_raw(c);
        }

        // R11 (deadlock-layer unblinding, the seventh blind printer): the
        // lock WORD value, always. The holder-identity block below is
        // debug-lock-owner-gated, so on standard builds every stuck lock
        // printed with NO holder datum at all — and the one value that
        // discriminates "a real holder the tracker missed" (word == 1)
        // from "the word itself was scribbled" (word ∉ {0,1}) was never
        // printed. One relaxed load, cold path, raw uart.
        let msg_lw = b" locked=0x";
        for &b in msg_lw {
            crate::console::uart_putc_raw(b);
        }
        let lw = unsafe { (&*lock_addr).locked_word() };
        let mut shift_lw: i32 = 8;
        while shift_lw > 0 {
            shift_lw -= 4;
            let nibble = ((lw >> shift_lw) & 0xF) as u8;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            crate::console::uart_putc_raw(c);
        }

        // Holder identity: owner = hart+1 (0 = free / mid-handoff).
        // oh-crash 91st extension (the meltdown holder hunt): the owner
        // CPU's current task (pid/comm), the heap allocation CONTAINING
        // the lock (small-object span + first words for offline
        // symbolization of any ops/vtable pointers), and one REMOTE-BT
        // of the holder per unique lock address per boot — the IPI
        // handler on the holder captures its live interrupted stack.
        // grq-lock-1st: the GRQ crime scene. A GRQ holder runs with IRQs
        // OFF, so the REMOTE-BT capture below (an IPI) can never answer
        // on a GRQ wedge — that is why every GRQ wedge so far ended with
        // the holder UNNAMED. The GRQ site stamp is published BY the
        // holder, so this dump reads it without any interrupt.
        crate::sched::hotpath::grq_dump_holder("deadlock");

        #[cfg(feature = "debug-lock-owner")]
        {
            let msg4 = b" holder=";
            for &b in msg4 {
                crate::console::uart_putc_raw(b);
            }
            // SAFETY: lock_addr is a valid RawSpinlock pointer (from lock()).
            let holder = unsafe { (&*lock_addr).owner.load(Ordering::Relaxed) };
            let mut holder_cpu: Option<usize> = None;
            if holder == 0 {
                let msg5 = b"switching";
                for &b in msg5 {
                    crate::console::uart_putc_raw(b);
                }
            } else {
                let h = holder - 1;
                holder_cpu = Some(h);
                if h < 10 {
                    crate::console::uart_putc_raw(b'0' + h as u8);
                }
                // Holder CPU's current task: pid + comm (best-effort —
                // an irqsave holder cannot migrate, so this is the task
                // that acquired; plain lock() holders may have been
                // preempted, in which case the remote-BT below shows
                // what the CPU runs NOW and the pid names the victim).
                // SAFETY: cpu_state(h).current is the scheduler's own
                // published pointer; read-only access.
                let cur = unsafe { crate::sched::hotpath::cpu_state(h).current };
                if !cur.is_null() {
                    // SAFETY: read-only access to a live Task.
                    let pid = unsafe { (*cur).pid() };
                    let msg_pid = b" task=";
                    for &b in msg_pid {
                        crate::console::uart_putc_raw(b);
                    }
                    Self::putdec(pid as u64);
                    // SAFETY: comm is a fixed [u8;16], no allocation.
                    let comm = unsafe { (*cur).comm() };
                    crate::console::uart_putc_raw(b'(');
                    let mut any = false;
                    for &b in comm.iter() {
                        if b == 0 {
                            break;
                        }
                        if (0x20..0x7f).contains(&b) {
                            crate::console::uart_putc_raw(b);
                            any = true;
                        }
                    }
                    if !any {
                        crate::console::uart_putc_raw(b'?');
                    }
                    unsafe { crate::console::uart_putc_raw(b')'); }
                }
            }

        crate::console::uart_putc_raw(b'\n');

        // Leak forensics (debug-lock-owner): the LAST acquirer's pid,
        // call site and acquire age — these persist past a cleared
        // owner word, so a leaked lock names who took it and when
        // even when the owner stamp is mid-handoff. Plus the raw
        // locked word (1 = genuinely held; anything else is a torn
        // or corrupted state worth seeing).
        #[cfg(feature = "debug-lock-owner")]
        {
            // SAFETY: lock_addr is a valid RawSpinlock pointer (from lock()).
            let l = unsafe { &*lock_addr };
            let apid = l.acq_pid.load(Ordering::Relaxed);
            let asite = l.acq_site.load(Ordering::Relaxed);
            let ajif = l.acq_jiffies.load(Ordering::Relaxed);
            let wv = l.locked.load(Ordering::Relaxed);
            let now = crate::timer::get_jiffies();
            let msg = b"LOCK-ACQ: word=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::puthex(wv as u64);
            let msg = b" pid=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::putdec(apid as u64);
            let msg = b" site=0x";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::puthex(asite as u64);
            let msg = b" age=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::putdec(now.wrapping_sub(ajif));
            let msg = b"j\n";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
        }

        // Containing heap object (works with or without a known holder):
        // slab span + the first 6 words of the object. Any word landing
        // in kernel text (0xffffffff8...) symbolizes offline against
        // gen-ksyms output — that names the lock's OWNER STRUCT (ops
        // table / vtable / function pointer members).
        if let Some((base, sz)) = crate::mm::hotpath::object_span(lock_addr as usize) {
            let msg = b"LOCK-OBJ: small kmalloc-";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::putdec(sz as u64);
            let msg = b" base=0x";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::puthex(base as u64);
            let msg = b" words=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            for i in 0..6 {
                // SAFETY: diagnostic read of the object's first words;
                // the span was validated by object_span (inside a live
                // slab page, in-bounds of the object's slot).
                let w = unsafe { ((base + i * 8) as *const u64).read_unaligned() };
                Self::puthex(w);
                if i < 5 {
                    crate::console::uart_putc_raw(b',');
                }
            }
            crate::console::uart_putc_raw(b'\n');
        } else if let Some((base, order, free)) =
            crate::mm::hotpath::GLOBAL_ALLOCATOR.buddy_block_span(lock_addr as usize)
        {
            // oh-crash 98th: DIRECT BUDDY blocks (>1KB kmalloc — Task
            // structs, large fs/ipc objects). Same offline-symbolization
            // trick on the first words; `free!=0` (a lock inside a FREE
            // block) is itself evidence of a freed-with-lock-reference
            // object, printed as words=FREE.
            let msg = b"LOCK-OBJ: buddy order-";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::putdec(order as u64);
            let msg = b" base=0x";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::puthex(base as u64);
            let msg = b" free=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::putdec(free as u64);
            let msg = b" words=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            for i in 0..6 {
                // SAFETY: diagnostic read of the block's first words;
                // buddy_block_span validated the address against the
                // heap window and the block's base alignment.
                let w = unsafe { ((base + i * 8) as *const u64).read_unaligned() };
                Self::puthex(w);
                if i < 5 {
                    crate::console::uart_putc_raw(b',');
                }
            }
            crate::console::uart_putc_raw(b'\n');
        }

        // One REMOTE-BT per unique lock address per boot: the IPI
        // handler on the holder CPU captures the live interrupted
        // stack. (x86_64 only — the riscv64/aarch64 twins have no
        // IRQ_WORK remote capture yet.)
        #[cfg(feature = "x86_64")]
        {
            static LAST_BT_LOCK: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let la = lock_addr as usize;
            let prev = LAST_BT_LOCK.load(Ordering::Acquire);
            if prev != la
                && LAST_BT_LOCK
                    .compare_exchange(prev, la, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                if let Some(h) = holder_cpu {
                    let _ = crate::arch::ipi::remote_backtrace(h, "deadlock-holder");
                }
            }
        }
        }
        #[cfg(not(feature = "debug-lock-owner"))]
        {
            crate::console::uart_putc_raw(b'\n');

            // The heap-object span dump is cheap and holder-independent:
            // keep it on default builds too (the meltdown hunts run
            // stock feature sets as controls).
            if let Some((base, sz)) = crate::mm::hotpath::object_span(lock_addr as usize) {
                let msg = b"LOCK-OBJ: small kmalloc-";
                for &b in msg {
                    crate::console::uart_putc_raw(b);
                }
                Self::putdec(sz as u64);
                let msg = b" base=0x";
                for &b in msg {
                    crate::console::uart_putc_raw(b);
                }
                Self::puthex(base as u64);
                crate::console::uart_putc_raw(b'\n');
            } else if let Some((base, order, free)) =
                crate::mm::hotpath::GLOBAL_ALLOCATOR.buddy_block_span(lock_addr as usize)
            {
                let msg = b"LOCK-OBJ: buddy order-";
                for &b in msg {
                    crate::console::uart_putc_raw(b);
                }
                Self::putdec(order as u64);
                let msg = b" base=0x";
                for &b in msg {
                    crate::console::uart_putc_raw(b);
                }
                Self::puthex(base as u64);
                let msg = b" free=";
                for &b in msg {
                    crate::console::uart_putc_raw(b);
                }
                Self::putdec(free as u64);
                crate::console::uart_putc_raw(b'\n');
            }
        }

        // rux:watchdog (retired debug=watchdog) (debug-migrate: rux:taskdump kind=WATCHDOG):
        // follow the warning with a full task-state snapshot
        // (silent-wedge diagnosis — CPUs idle + sleepers never woken).
        if crate::trace::EVENT_TASKDUMP.armed() {
            crate::trace::trace_taskdump(2, 0);
            crate::debug::hotpath::dump_all_tasks("deadlock-watchdog");
        }
    }

    /// Lockless hex print (deadlock diagnostics — putchar path only).
    fn puthex(v: u64) {
        let mut shift: i32 = 64;
        while shift > 0 {
            shift -= 4;
            let nibble = ((v >> shift) & 0xF) as u8;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            unsafe { crate::console::putchar_no_lock(c); }
        }
    }

    /// Lockless decimal print (deadlock diagnostics).
    fn putdec(mut v: u64) {
        let mut buf = [0u8; 20];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (v % 10) as u8;
            i += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while i > 0 {
            i -= 1;
            unsafe { crate::console::putchar_no_lock(buf[i]); }
        }
    }

    #[inline]
    pub fn try_lock(&self) -> bool {
        let ok = self
            .locked
            .compare_exchange(0, Self::held_word(), Ordering::Acquire, Ordering::Acquire)
            .is_ok();
        if ok {
            self.record_owner(caller_return_address());
            // RT-plan R1 completion: symmetric with lock()/unlock().
            crate::interrupt::preempt::preempt_count_add(
                crate::interrupt::preempt::PREEMPT_OFFSET,
            );
        }
        ok
    }

    #[inline]
    pub fn unlock(&self) {
        // The ownership DETECTOR (debug builds only — the reference's
        // CONFIG_DEBUG_SPINLOCK `spin_bug` shape): the releaser must be
        // the holder the word names. A double unlock, a foreign unlock,
        // or a corrupted word all land here instead of silently
        // corrupting someone else's section.
        debug_assert!(
            self.locked.load(Ordering::Relaxed) == Self::held_word(),
            "spinlock released by non-holder: word={:#x}",
            self.locked.load(Ordering::Relaxed)
        );
        // RT-plan R1 completion: pair of the acquire-side bump (see lock).
        // Sub BEFORE the store: the glib-mutex ORDER FIX discipline (preempt
        // sub precedes the unlock store) — the grq-10ns capture convicted
        // the reverse order.
        crate::interrupt::preempt::preempt_count_sub(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
        self.clear_owner();
        // THE release: one ordered store clears lock + owner together
        // (owner-in-word, grq-source-fix). Nothing can observe a
        // post-release state that still names this cpu, and nothing
        // this side of the store can strand the word.
        self.locked.store(0, Ordering::Release);
    }

    /// Terminal-path force release (oh-launch round 6, gw1; reworked
    /// grq-source-fix): a CPU that abandons a section forever (a task
    /// dying mid-section — the paging-oops class faulting under the
    /// lock; a park in the KERNPANIC halt loop) never runs its paired
    /// unlock, and the word would stay held while every later acquirer
    /// spins (the permanent GRQ wedge: four CPUs at lock+1359).
    ///
    /// The caller must have PROVEN this CPU holds the lock — the proof
    /// is the lock word itself (`held_by(cpu)`: the single authority
    /// naming the caller as the owner), never a second tracker and
    /// never speculation: force-releasing a lock another CPU holds
    /// corrupts that section. This is exit-releases-what-it-holds
    /// semantics (the reference's exit releases the dying task's
    /// locks), with ownership provable from one load. No
    /// preempt_count_sub: the terminal path resets the whole count
    /// anyway.
    pub fn force_unlock_terminal(&self) {
        self.clear_owner();
        self.locked.store(0, Ordering::Release);
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Acquire) != 0
    }

    #[inline]
    pub unsafe fn reset(&mut self) {
        self.clear_owner();
        self.locked.store(0, Ordering::Release);
    }
}

// ==================== Spinlock<T> ====================

impl Spinlock<i32> {
    /// Offset of the data word inside this Spinlock layout (the compiler
    /// reorders raw/data — diagnostic watchpoints must use this, never
    /// fixed arithmetic).
    pub fn data_offset() -> usize {
        core::mem::offset_of!(Spinlock<i32>, data)
    }
}

// SAFETY: Spinlock<T> only allows &mut T access via the guard, which exists
// only while the inner lock is held.  Send is required so the lock (and its
// data) can be moved between threads; Sync is safe because the lock mediates
// all access.
unsafe impl<T: ?Sized + Send> Send for Spinlock<T> {}
unsafe impl<T: ?Sized + Send> Sync for Spinlock<T> {}

impl<T> Spinlock<T> {
    #[inline]
    pub const fn new(data: T) -> Self {
        Self { raw: RawSpinlock::new(), data: UnsafeCell::new(data) }
    }

    /// Preempt disable + lock.
    /// Guard drop: unlock + preempt enable.
    /// track_caller: leakorigin's outermost-acquire shadow records the
    /// LOCK CALLER's site (preempt-hunt), not this helper's line.
    ///
    /// Reference discipline (sync-realign, 2026-11): a spinlock NEVER
    /// sleeps. The acquire is preempt_disable + pure bounded spinning
    /// (spin until acquired — the DEADLOCK_WARN threshold only prints,
    /// it never gives up); no task-state change and no schedule()
    /// anywhere in this path. This is sound ONLY because every lock
    /// that may be held across blocking I/O is a SleepMutex
    /// (kernel/core/src/sync/hotpath.rs), never a Spinlock: a preempt-disabled
    /// spinner can starve the holder's wake IPI, so a Spinlock section
    /// must stay short by construction. The former "preemptible
    /// bounded wait" that yielded via schedule() here (preempt-hunt,
    /// then the lost-park-patched handoff) was a divergence from the
    /// reference design and is deleted — the lost-park hazard class
    /// (a caller inside a wait path's prepared window parked inside a
    /// spinlock acquire) is structurally impossible now.
    ///
    /// RUNQ-STRAND fix (121st) precision on that discipline: the raw
    /// TTAS loop below CAN call schedule(), but only through
    /// `spin_may_preempt()` — which requires preempt_count()==0 and is
    /// therefore unreachable for THIS acquire (our own preempt_disable
    /// guarantees count >= 1 for every Spinlock-guarded caller). The
    /// exit exists solely for a waiter whose count was zeroed under it
    /// (tick-side PREEMPT-HEAL false positive) or a future uncoupled
    /// raw caller; see `spin_may_preempt` for the full guard set.
    #[inline]
    #[track_caller]
    pub fn lock(&self) -> SpinlockGuard<'_, T> {
        preempt_disable();
        self.raw.lock();
        SpinlockGuard { lock: self }
    }

    /// Disable interrupts + preempt disable + lock.
    /// Guard drop: unlock + preempt enable + restore interrupts.
    #[inline]
    #[track_caller]
    pub fn lock_irq(&self) -> SpinlockIrqGuard<'_, T> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
        self.raw.lock();
        SpinlockIrqGuard { lock: self, flags }
    }

    /// Save interrupt state + disable + preempt disable + lock.
    /// Guard drop: unlock + preempt enable + restore interrupt state.
    #[inline]
    #[track_caller]
    pub fn lock_irqsave(&self) -> SpinlockIrqGuard<'_, T> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
        self.raw.lock();
        SpinlockIrqGuard { lock: self, flags }
    }

    /// Disable bottom-half (softirq) + lock.
    /// bh_disable() increments preempt_count by SOFTIRQ_OFFSET,
    /// which also disables preemption.
    /// Guard drop: unlock + bh_enable (decrements preempt_count).
    #[inline]
    pub fn lock_bh(&self) -> SpinlockBhGuard<'_, T> {
        bh_disable();
        self.raw.lock();
        SpinlockBhGuard { lock: self }
    }

    #[inline]
    #[track_caller]
    pub fn try_lock(&self) -> Option<SpinlockGuard<'_, T>> {
        preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_lock() {
            Some(SpinlockGuard { lock: self })
        } else {
            preempt_enable();
            None
        }
    }

    #[inline]
    #[track_caller]
    pub fn try_lock_irqsave(&self) -> Option<SpinlockIrqGuard<'_, T>> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_lock() {
            Some(SpinlockIrqGuard { lock: self, flags })
        } else {
            preempt_enable();
            irq_restore(flags);
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool { self.raw.is_locked() }

    /// Get a shared reference to the inner data without locking.
    ///
    /// # Safety
    /// Caller must ensure no concurrent mutable access (e.g., data is
    /// write-once during boot and only read thereafter).
    #[inline]
    pub unsafe fn get_ref(&self) -> &T {
        &*self.data.get()
    }

    /// Get a mutable reference to the inner data without locking.
    ///
    /// # Safety
    /// Caller must ensure exclusive access (e.g., no concurrent readers or writers).
    #[inline]
    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        &mut *self.data.get()
    }

    /// Consume the lock and return the inner data.
    ///
    /// # Safety
    /// Caller must ensure no other thread holds a reference to the lock or data.
    #[inline]
    pub unsafe fn into_inner(self) -> T { self.data.into_inner() }
}

// ==================== SpinlockGuard (plain) ====================

// SAFETY: SpinlockGuard exists only while the lock is held, so the guarded
// data cannot be accessed concurrently from another thread.
unsafe impl<T: ?Sized + Send> Send for SpinlockGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Guard holds the lock — no concurrent &mut access possible.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinlockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Guard holds the lock exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinlockGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.unlock();
        // Leak-hunt: pass the lock instance so the underflow report can
        // name WHICH Spinlock double-dropped (static locks symbolize
        // against the ELF; heap locks identify the owning object).
            crate::interrupt::preempt::preempt_count_sub2_at(
                crate::interrupt::preempt::PREEMPT_OFFSET,
                core::panic::Location::caller(),
                // RawSpinlock is Spinlock's first field, so its address
                // IS the Spinlock instance's address (T may be ?Sized,
                // making &Spinlock<T> a fat reference — go through the
                // thin raw field pointer).
                &self.lock.raw as *const RawSpinlock as usize,
            );
    }
}

// ==================== SpinlockIrqGuard (irqsave) ====================

// D-11 typed-lock-key (round-5): a live irqsave guard proves the holder
// owns this lock's critical section — OnceSlot::get_mut_locked accepts
// it as the exclusivity witness (the audited assertion lives at the
// slot site, which names THIS lock as the mutation governor).
impl<T: ?Sized> crate::safety::slot::ExclusivityProof for SpinlockIrqGuard<'_, T> {}

// SAFETY: SpinlockIrqGuard exists only while the lock is held, so the
// guarded data cannot be accessed concurrently.
unsafe impl<T: ?Sized + Send> Send for SpinlockIrqGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockIrqGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Guard holds the lock — no concurrent &mut access possible.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinlockIrqGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Guard holds the lock exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinlockIrqGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // ORDER FIX (glib-mutex lane, 2026-11-14): preempt sub BEFORE the
        // raw unlock. The old order exposed a [unlock..sub] window in
        // which the lock was free while the dropping task still carried
        // its +1 — any observer adjudicating "parked task with a live
        // count" (the WAKE-HEAL class) could zero an in-flight guard.
        // preempt_enable is a bare sub (no resched) and IF stays 0 until
        // irq_restore below, so the reorder cannot enable preemption
        // under the lock.
        preempt_enable();
        self.lock.raw.unlock();
        irq_restore(self.flags);
    }
}

impl<T: ?Sized> SpinlockIrqGuard<'_, T> {
    /// Release only the spinlock (preempt_enable + unlock), returning
    /// the saved IRQ flags. The caller must later call `irq_restore(flags)`
    /// to restore interrupt state.
    ///
    /// This is used by the scheduler: we must drop the rq lock before
    /// context_switch but keep interrupts disabled until after
    /// context_switch returns (following Linux's pattern where
    /// finish_task_switch releases the lock).
    ///
    /// ORDER FIX (glib-mutex lane): preempt sub before unlock — see the
    /// Drop impl above; the GRQ twin's capture (garepro26 WAKE-HEAL /
    /// PREEMPT-UNDERFLOW pair) convicted this window.
    #[inline]
    pub fn unlock_irqretain(self) -> bool {
        let flags = self.flags;
        preempt_enable();
        self.lock.raw.unlock();
        core::mem::forget(self); // prevent Drop from running
        flags
    }
}

// ==================== SpinlockBhGuard (bottom-half) ====================

// SAFETY: SpinlockBhGuard exists only while the lock is held, so the
// guarded data cannot be accessed concurrently.
unsafe impl<T: ?Sized + Send> Send for SpinlockBhGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockBhGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Guard holds the lock — no concurrent &mut access possible.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinlockBhGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Guard holds the lock exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinlockBhGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.unlock();
        bh_enable();
    }
}

// ==================== Inline helpers ====================

#[inline]
#[track_caller]
fn preempt_disable() {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        core::panic::Location::caller(),
    );
}

/// As `preempt_disable`, booking the CALLER-of-the-lock's location (the
/// #[track_caller] lock helpers forward their own caller down so the
/// leakorigin shadow names the real acquire site).
#[inline]
fn preempt_disable_at(loc: &'static core::panic::Location<'static>) {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        loc,
    );
}

#[inline]
#[track_caller]
fn preempt_enable() {
    crate::interrupt::preempt::preempt_count_sub_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        core::panic::Location::caller(),
    );
}

#[inline]
#[track_caller]
fn irq_save() -> bool {
    // track_caller: syscost's REST histogram sees the outermost lock
    // user instead of this line (no-op unless syscost is armed).
    crate::arch::cpu::save_and_disable_irq()
}

#[inline]
#[track_caller]
fn irq_restore(flags: bool) {
    crate::arch::cpu::restore_irq(flags);
}

#[inline]
#[track_caller]
fn bh_disable() {
    crate::interrupt::preempt::preempt_count_add(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
    );
}

#[inline]
#[track_caller]
fn bh_enable() {
    crate::interrupt::preempt::preempt_count_sub_at(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
        core::panic::Location::caller(),
    );
}

// ==================== Free functions (C-style API) ====================

#[inline(always)]
pub fn raw_spin_lock(l: &RawSpinlock) { l.lock(); }
#[inline(always)]
pub fn raw_spin_unlock(l: &RawSpinlock) { l.unlock(); }
#[inline(always)]
pub fn raw_spin_is_locked(l: &RawSpinlock) -> bool { l.is_locked() }
#[inline(always)]
pub fn raw_spin_trylock(l: &mut RawSpinlock) -> bool { l.try_lock() }

// ═══════════════ rwlock ═════════════════════════════════════════
// (was rwlock.rs)
// Read-Write Spinlock
//
// Allows concurrent readers or a single exclusive writer.
//
// API:
//   read()           — preempt disable + read lock
//   write()          — preempt disable + write lock
//   read_irqsave()   — save IRQ + preempt disable + read lock
//   write_irqsave()  — save IRQ + preempt disable + write lock
//   read_bh()        — disable BH + read lock
//   write_bh()       — disable BH + write lock

// (INV-RWLOCK-1: the rwlock family — reader counts and the writer
// word are atomics with the acquire/release pairing documented at the
// slowpath; guard derefs ride the guard's lifetime (the lock's own
// discipline); panic-path rows are best-effort prints (same contract
// as spinlock's below).

const WRITER_BIT: u32 = 1u32 << 31;
/// Writer-pending bit: set by the FIRST contended writer while readers
/// still hold the lock (the reference qrwlock's queued-writer discipline
/// — see `write()` below). New readers block on it, so a continuous
/// cross-CPU stream of short readers cannot starve a waiting writer out
/// of the lock (the post-realign hazard: with the yielding slowpath
/// deleted, a preempt-disabled writer could only spin while readers on
/// other CPUs kept re-acquiring forever).
const WRITER_PENDING_BIT: u32 = 1u32 << 30;
const READER_MASK: u32 = !(WRITER_BIT | WRITER_PENDING_BIT);

impl RawRwSpinlock {
    #[inline]
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
        }
    }

    /// Acquire a shared (reader) lock.
    #[inline]
    pub fn read(&self) {
        loop {
            let s = self.state.load(Ordering::Acquire);
            // A HELD writer excludes; a PENDING writer also blocks new
            // readers (writer fairness — see the struct doc).
            if s & (WRITER_BIT | WRITER_PENDING_BIT) != 0 {
                core::hint::spin_loop();
                continue;
            }
            // Guard against reader count overflow into the flag bits.
            if (s & READER_MASK) >= READER_MASK {
                core::hint::spin_loop();
                continue;
            }
            if self
                .state
                .compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Try to acquire a shared (reader) lock.
    #[inline]
    pub fn try_read(&self) -> bool {
        let s = self.state.load(Ordering::Acquire);
        if s & (WRITER_BIT | WRITER_PENDING_BIT) != 0 {
            return false;
        }
        if (s & READER_MASK) >= READER_MASK {
            return false;
        }
        self.state
            .compare_exchange(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Release a shared (reader) lock.
    #[inline]
    pub fn read_unlock(&self) {
        self.state.fetch_sub(1, Ordering::Release);
    }

    /// Acquire an exclusive (writer) lock (two-phase, writer-fair).
    ///
    /// Phase 1 — register PENDING: exactly ONE waiting writer owns the
    /// pending bit at a time (a second writer sees it set and waits for
    /// it to clear, which its owner's phase-2 CAS guarantees). Setting
    /// PENDING is allowed while readers still hold the lock; it blocks
    /// only NEW readers, so the current reader generation drains.
    ///
    /// Phase 2 — wait for the reader count to reach zero, then flip
    /// PENDING → WRITER. Once we own PENDING, no new reader can enter
    /// (they block on it) and no other writer can take WRITER (they
    /// cannot reach phase 2 without owning PENDING), so the flip's CAS
    /// races nothing; the loop keeps its retry shape for the same
    /// defensive reason RawSpinlock::lock does.
    #[inline]
    pub fn write(&self) {
        // Phase 1: become the one pending writer.
        loop {
            let s = self.state.load(Ordering::Acquire);
            if s & WRITER_BIT != 0 {
                core::hint::spin_loop();
                continue;
            }
            if s & WRITER_PENDING_BIT != 0 {
                // Another writer already queued — wait for its turn to
                // complete (it WILL take the lock: readers drain and the
                // flip races nothing).
                core::hint::spin_loop();
                continue;
            }
            if self
                .state
                .compare_exchange_weak(
                    s,
                    s | WRITER_PENDING_BIT,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
            core::hint::spin_loop();
        }
        // Phase 2: drain wait + flip.
        loop {
            let s = self.state.load(Ordering::Acquire);
            if s & READER_MASK == 0 {
                if self
                    .state
                    .compare_exchange_weak(
                        s,
                        (s | WRITER_BIT) & !WRITER_PENDING_BIT,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return;
                }
            }
            core::hint::spin_loop();
        }
    }

    /// Try to acquire an exclusive (writer) lock.
    #[inline]
    pub fn try_write(&self) -> bool {
        self.state
            .compare_exchange(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Release an exclusive (writer) lock.
    #[inline]
    pub fn write_unlock(&self) {
        self.state.fetch_and(!WRITER_BIT, Ordering::Release);
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.state.load(Ordering::Acquire) != 0
    }

    /// A writer holds the lock or has registered pending (readers may
    /// not enter either way). Diagnostic/probe helper (the deleted
    /// preemptible read-wait path was its only internal consumer).
    #[inline]
    pub fn has_writer(&self) -> bool {
        self.state.load(Ordering::Acquire) & (WRITER_BIT | WRITER_PENDING_BIT) != 0
    }
}

// ==================== Generic RwSpinlock<T> ====================

// SAFETY: RwSpinlock<T> mediates all access to the inner data: shared (&T) via
// read-locked guards, exclusive (&mut T) via write-locked guards.  Send allows
// moving between threads; Sync allows shared references across threads because
// the rwlock serialises access.
unsafe impl<T: ?Sized + Send> Send for RwSpinlock<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for RwSpinlock<T> {}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwSpinlock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RwSpinlock")
            .field("locked", &self.raw.is_locked())
            .finish_non_exhaustive()
    }
}

impl<T> RwSpinlock<T> {
    #[inline]
    pub const fn new(data: T) -> Self {
        Self {
            raw: RawRwSpinlock::new(),
            data: UnsafeCell::new(data),
        }
    }

    /// Preempt disable + read lock.
    /// Guard drop: read unlock + preempt enable.
    /// track_caller: leakorigin's outermost-acquire shadow records the
    /// LOCK CALLER's site (preempt-hunt), not this helper's line.
    ///
    /// Reference discipline (sync-realign, 2026-11): a spin-family lock
    /// NEVER sleeps — no task-state change and no schedule() anywhere
    /// in the acquire; preempt stays disabled for the whole spin. The
    /// former "preemptible read-wait" that yielded via a bare
    /// schedule() here (preempt-hunt) was the same divergence deleted
    /// from Spinlock::lock, and worse: it could park a caller caught in
    /// a wait path's prepared window (the lost-park family). Locks whose
    /// holders block in I/O must be SleepMutex (mutex section), not
    /// rwlocks.
    #[inline]
    #[track_caller]
    pub fn read(&self) -> RwSpinlockReadGuard<'_, T> {
        rwlock_preempt_disable_at(core::panic::Location::caller());
        self.raw.read();
        RwSpinlockReadGuard { lock: self }
    }

    /// Preempt disable + write lock.
    /// Guard drop: write unlock + preempt enable.
    /// Same pure-spinning discipline as `read` (see there).
    #[inline]
    #[track_caller]
    pub fn write(&self) -> RwSpinlockWriteGuard<'_, T> {
        rwlock_preempt_disable_at(core::panic::Location::caller());
        self.raw.write();
        RwSpinlockWriteGuard { lock: self }
    }

    /// Save IRQ + preempt disable + read lock.
    /// Guard drop: read unlock + preempt enable + restore IRQ.
    #[inline]
    #[track_caller]
    pub fn read_irqsave(&self) -> RwSpinlockIrqReadGuard<'_, T> {
        let flags = rwlock_irq_save();
        rwlock_preempt_disable_at(core::panic::Location::caller());
        self.raw.read();
        RwSpinlockIrqReadGuard { lock: self, flags }
    }

    /// Save IRQ + preempt disable + write lock.
    /// Guard drop: write unlock + preempt enable + restore IRQ.
    #[inline]
    #[track_caller]
    pub fn write_irqsave(&self) -> RwSpinlockIrqWriteGuard<'_, T> {
        let flags = rwlock_irq_save();
        rwlock_preempt_disable_at(core::panic::Location::caller());
        self.raw.write();
        RwSpinlockIrqWriteGuard { lock: self, flags }
    }

    /// Disable BH + read lock.
    /// rwlock_bh_disable() increments preempt_count by SOFTIRQ_OFFSET.
    /// Guard drop: read unlock + rwlock_bh_enable.
    #[inline]
    pub fn read_bh(&self) -> RwSpinlockBhReadGuard<'_, T> {
        rwlock_bh_disable();
        self.raw.read();
        RwSpinlockBhReadGuard { lock: self }
    }

    /// Disable BH + write lock.
    /// rwlock_bh_disable() increments preempt_count by SOFTIRQ_OFFSET.
    /// Guard drop: write unlock + rwlock_bh_enable.
    #[inline]
    pub fn write_bh(&self) -> RwSpinlockBhWriteGuard<'_, T> {
        rwlock_bh_disable();
        self.raw.write();
        RwSpinlockBhWriteGuard { lock: self }
    }

    #[inline]
    #[track_caller]
    pub fn try_read(&self) -> Option<RwSpinlockReadGuard<'_, T>> {
        rwlock_preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_read() {
            Some(RwSpinlockReadGuard { lock: self })
        } else {
            rwlock_preempt_enable();
            None
        }
    }

    #[inline]
    #[track_caller]
    pub fn try_write(&self) -> Option<RwSpinlockWriteGuard<'_, T>> {
        rwlock_preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_write() {
            Some(RwSpinlockWriteGuard { lock: self })
        } else {
            rwlock_preempt_enable();
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    /// Consume the lock and return the inner data.
    ///
    /// # Safety
    /// Caller must ensure no other thread holds a reference to the lock or data.
    #[inline]
    pub unsafe fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

// ==================== Read Guard (plain — preempt) ====================

// SAFETY: ReadGuard exists only while a read lock is held, preventing any
// writer from obtaining exclusive access to the data.
unsafe impl<T: ?Sized + Sync> Send for RwSpinlockReadGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Read lock is held — no &mut access can occur concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.read_unlock();
        rwlock_preempt_enable();
    }
}

// ==================== Write Guard (plain — preempt) ====================

// SAFETY: WriteGuard exists only while the write lock is held exclusively,
// so no other thread can access the data.
unsafe impl<T: ?Sized + Send> Send for RwSpinlockWriteGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Write lock is held exclusively — no concurrent access.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwSpinlockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Write lock is held exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.write_unlock();
        rwlock_preempt_enable();
    }
}

// ==================== IRQ Read Guard ====================

// SAFETY: IrqReadGuard exists only while a read lock + IRQ disable is held,
// preventing any writer from accessing the data.
unsafe impl<T: ?Sized + Sync> Send for RwSpinlockIrqReadGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockIrqReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Read lock is held — no &mut access can occur concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockIrqReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.read_unlock();
        rwlock_preempt_enable();
        rwlock_irq_restore(self.flags);
    }
}

// ==================== IRQ Write Guard ====================

// SAFETY: IrqWriteGuard exists only while the write lock + IRQ disable is
// held exclusively, so no other thread can access the data.
unsafe impl<T: ?Sized + Send> Send for RwSpinlockIrqWriteGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockIrqWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Write lock is held exclusively — no concurrent access.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwSpinlockIrqWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Write lock is held exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockIrqWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.write_unlock();
        rwlock_preempt_enable();
        rwlock_irq_restore(self.flags);
    }
}

// ==================== BH Read Guard ====================

// SAFETY: BhReadGuard exists only while a read lock + BH disable is held,
// preventing any writer from accessing the data.
unsafe impl<T: ?Sized + Sync> Send for RwSpinlockBhReadGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockBhReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Read lock is held — no &mut access can occur concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockBhReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.read_unlock();
        rwlock_bh_enable();
    }
}

// ==================== BH Write Guard ====================

// SAFETY: BhWriteGuard exists only while the write lock + BH disable is
// held exclusively, so no other thread can access the data.
unsafe impl<T: ?Sized + Send> Send for RwSpinlockBhWriteGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockBhWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Write lock is held exclusively — no concurrent access.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwSpinlockBhWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Write lock is held exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockBhWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.write_unlock();
        rwlock_bh_enable();
    }
}

// ==================== Inline helpers ====================

#[inline]
#[track_caller]
fn rwlock_preempt_disable() {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        core::panic::Location::caller(),
    );
}

/// As `rwlock_preempt_disable`, booking the caller-of-the-lock's location (the
/// #[track_caller] lock helpers forward their own caller down so the
/// leakorigin shadow names the real acquire site; preempt-hunt).
#[inline]
fn rwlock_preempt_disable_at(loc: &'static core::panic::Location<'static>) {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        loc,
    );
}

#[inline]
fn rwlock_preempt_enable() {
    crate::interrupt::preempt::preempt_count_sub(
        crate::interrupt::preempt::PREEMPT_OFFSET,
    );
}

#[inline]
#[track_caller]
fn rwlock_irq_save() -> bool { // track_caller: syscost REST site attribution
    crate::arch::cpu::save_and_disable_irq()
}

#[inline]
#[track_caller]
fn rwlock_irq_restore(flags: bool) {
    crate::arch::cpu::restore_irq(flags);
}

#[inline]
fn rwlock_bh_disable() {
    crate::interrupt::preempt::preempt_count_add(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
    );
}

#[inline]
fn rwlock_bh_enable() {
    crate::interrupt::preempt::preempt_count_sub(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
    );
}

// ═══════════════ rwsem ═════════════════════════════════════════
// (was rwsem.rs)
// Sleepable read-write semaphore — the mmap_lock primitive
// (reference recipe, linux-6.6 `kernel/locking/rwsem.c`).
//
// This is the sleepable counterpart of the rwlock section. The VMA lock
// (`MmStruct::vma_manager`) was that pure spinner and mm paths held
// it across slow work — pagefault COW under a read guard while page
// allocation crawls, `VmaManager::add` under the write guard — so a
// slow reader meeting a pending writer monopolized CPUs and blocked
// mmap writers (the 52nd's §6b evidence: mm_struct.rs:987 leak-site
// attribution, the ohc24 frozen CPU caught spinning at syscall
// entry). The reference's mmap_lock is a `struct rw_semaphore`:
// waiters SLEEP, writers wait uninterruptibly (never spin the CPU
// away), and adaptive optimistic spinning exists but is bounded and
// owner-guarded.
//
// SEMANTICS PORTED (reference line numbers are linux-6.6
// kernel/locking/rwsem.c / include/linux/rwsem.h):
//
// - count word bit layout (rwsem.c:117-128, 64-bit shape):
//   `[0]` writer locked, `[1]` waiters present, `[2]` lock handoff,
//   `[8..62]` reader count (bias `1<<8`), `[63]` read-fail guard.
// - Fast-path reader: `fetch_add(READER_BIAS)`; falls into the
//   slowpath on WRITER|WAITERS|HANDOFF|READFAIL (rwsem.c:241-254) —
//   a queued waiter therefore blocks NEW readers from barging (the
//   anti-starvation core; no invented fairness scheme).
// - Fast-path writer: CAS 0 -> WRITER_LOCKED (rwsem.c:256-266).
// - Writer optimistic spinning (rwsem.c:819-935): only while the
//   owner is a WRITER that is ON-CPU (owner-word spin,
//   rwsem_spin_on_owner:746-792), need_resched aborts, and a
//   READER-owned lock is spun on only for a time budget
//   `(20 + readers)/2 us` capped at 25 us (rwsem_rspin_threshold)
//   after which the NONSPINNABLE latch is set in the owner word.
//   There is no osq MCS queue in Rux; the SleepMutex-proven
//   bounded-rounds budget bounds the same behavior (documented
//   divergence: budget shape, not semantics).
// - Waiters sleep UNINTERRUPTIBLE queued FIFO on an internal wait
//   list protected by `wait_lock` (a genuine spinlock). Readers are
//   granted in batches (phase-fair reader wake,
//   rwsem_mark_wake:509-522, up to MAX_READERS_WAKEUP=0x100) by
//   having the GRANTER pre-add their reader bias to count; a writer
//   at the head is woken to re-run the queued-acquire CAS itself
//   (other writers may steal from it until it wins the CAS).
// - HANDOFF protocol (rwsem.c:604-666, RWSEM_WAIT_TIMEOUT ~4 ms,
//   rwsem.c:359): a waiter queued longer than the timeout sets the
//   HANDOFF bit; while it is set, fast paths and steals refuse
//   everyone else, so the lock drains to the head waiter. HANDOFF
//   is only ever set/cleared under wait_lock (rwsem.c:108-115).
// - up_read wakes when the last reader leaves with waiters present;
//   up_write always wakes with waiters present (rwsem.c:1338-1378).
//
// # Rust/Rux divergences (behavior-preserving, documented)
//
// - `owner` holds a PID (bits 0..31) plus READER_OWNED / NONSPINNABLE
//   flag bits, not a task pointer: task lifetime for the on-CPU
//   check is managed with `pid_hash_lookup_pinned`/`task_put` (the
//   reference relies on preempt-disable == RCU read side around the
//   raw pointer; Rux optimistic spinning runs preemptible).
// - The fast paths bracket the count RMW + owner-word update with a
//   preempt-disable section (reference rwsem.c:137-139: owner and
//   count updates share one preempt section), but a HELD lock no
//   longer disables preemption — sections under this lock are
//   preemptible and may nest spinlocks below it (INV-LOCK-7 keeps
//   its position: vma lock above the PTE section). The slowpaths
//   re-enable preemption before parking (Rux cannot schedule() with
//   preempt_count != 0; the reference's schedule_preempt_disabled
//   has no Rux equivalent).
// - The sleep/wake mechanics use Rux's state-first + re-check
//   discipline (`Task::sleep` warning): the waiter sets
//   UNINTERRUPTIBLE BEFORE the last grant re-check under wait_lock,
//   so a racing grant either finds the condition already true or
//   its `wake_up_process` lands on a sleeping-state task. No
//   WaitQueueHead is needed: the internal wait list IS the wake
//   registry; grant (list removal for readers, acquire-CAS for
//   writers) is the wake token.
// - Waiters are UNINTERRUPTIBLE only (down_read/down_write; no
//   killable/interruptible variants — mmap_lock users don't take
//   them in the reference either, and an uninterruptible waiter
//   cannot exit mid-wait, so entries never outlive their tasks).
// - No RT-task branches (Rux has one fair scheduling class;
//   `!rt_task()` reduces to true). No lockdep/lockevent counters.
// - Degradations (SleepMutex parity): with no current task (early
//   boot / secondary harts) the acquire degrades to bounded pure
//   spinning instead of refusing — single-CPU boot paths still make
//   progress. (Unlike SleepMutex there is NO preempt_count != 0
//   degradation: taking this lock while holding a spinlock is a
//   lock-ordering violation that must fail loudly in review, and
//   every VMA-lock caller runs at preempt_count 0 — trap entry from
//   user and syscall contexts.)
//
// # Context restrictions
//
// NOT usable from interrupt context (a waiter must be able to call
// `schedule()`). `try_read`/`try_write` never sleep and are safe
// anywhere (the fuxstuck census relies on this).

// (INV-RWSEM-1: the rwsem family — the count word's atomic protocol
// pairs with the waitqueue; guard derefs ride the held semaphore
// lifetime; panic rows are best-effort (INV-SPIN-1's contract).)

// ==================== count word (rwsem.c:117-128) ====================

/// Bit 0: the lock is owned by a writer.
const WRITER_LOCKED: i64 = 1 << 0;
/// Bit 1: the wait list is (or was very recently) non-empty.
const FLAG_WAITERS: i64 = 1 << 1;
/// Bit 2: the lock must be handed to the first waiter.
const FLAG_HANDOFF: i64 = 1 << 2;
/// Bit 63: reader-count overflow guard (never expected to fire).
const FLAG_READFAIL: i64 = 1i64 << 63;
/// Readers count in units of 1<<8.
const READER_SHIFT: u32 = 8;
const READER_BIAS: i64 = 1 << READER_SHIFT;
/// Everything from bit 8 up (reader count + READFAIL).
const RWSEM_READER_MASK: i64 = !(READER_BIAS - 1);
const LOCK_MASK: i64 = WRITER_LOCKED | RWSEM_READER_MASK;
/// A fast-path reader fails when ANY of these is set
/// (rwsem.c:127-128) — this is what stops reader barging past queued
/// waiters.
const READ_FAILED_MASK: i64 = WRITER_LOCKED | FLAG_WAITERS | FLAG_HANDOFF | FLAG_READFAIL;

// ==================== owner word (rwsem.c:63-65) ====================

const OWNER_PID_MASK: usize = 0xFFFF_FFFF;
/// Bit 32: the lock is (or was last) owned by readers.
const OWNER_READER_OWNED: usize = 1 << 32;
/// Bit 33: optimistic spinning disabled (reader-phase spin budget
/// exceeded — rwsem_set_nonspinnable, rwsem.c:228-239).
const OWNER_NONSPINNABLE: usize = 1 << 33;

/// Waiters queued longer than this may demand a handoff
/// (rwsem.c:359: `RWSEM_WAIT_TIMEOUT = DIV_ROUND_UP(HZ, 250)`, ~4 ms).
const WAIT_TIMEOUT_JIFFIES: u64 = ((crate::config::KERNEL_HZ as u64) + 249) / 250;
/// Batch limit for one reader-phase wakeup (rwsem.c:367).
const MAX_READERS_WAKEUP: usize = 0x100;

/// `time_after(a, b)` in wrapping jiffies arithmetic.
#[inline]
fn time_after(now: u64, then: u64) -> bool {
    now.wrapping_sub(then) < (1u64 << 63)
}

/// Stall tripwire (SleepMutex STALL precedent — print-only DEBUG, no
/// semantic effect): a waiter parked longer than this names itself in
/// a rate-limited print. Normal parking lasts microseconds-to-
/// milliseconds; 3 s means the grant is not reaching this waiter.
const STALL_WAITER_JIFFIES: u64 = (crate::config::KERNEL_HZ as u64) * 3;

/// Global rate limit for RWSEM-STALL prints (one line / 10 s — the
/// wedge class prints hundreds of lines otherwise).
fn stall_report_ok() -> bool {
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::timer::get_jiffies();
    let last = LAST.load(Ordering::Relaxed);
    now.wrapping_sub(last) >= (crate::config::KERNEL_HZ as u64) * 10
        && LAST
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

// SAFETY: same contract as WaitQueueHead/SleepMutex — the raw task
// pointers inside the wait list are only ever passed to
// sched::wake_up_process (whose own guards handle stale targets);
// every list mutation is serialized by wait_lock; a queued waiter
// cannot exit (UNINTERRUPTIBLE), so entries never dangle.
unsafe impl Send for RwSemaphore {}
unsafe impl Sync for RwSemaphore {}

impl RwSemaphore {
    pub const fn new(name: &'static str) -> Self {
        Self {
            count: AtomicI64::new(0),
            owner: AtomicUsize::new(0),
            wait_lock: Spinlock::new(alloc::vec::Vec::new()),
            name,
        }
    }

    // ==================== owner word helpers ====================

    #[inline]
    fn owner_value(&self) -> usize {
        self.owner.load(Ordering::Relaxed)
    }

    /// Writer ownership publish (rwsem.c:140-144).
    #[inline]
    fn set_owner_writer(&self, pid: u32) {
        self.owner.store(pid as usize, Ordering::Release);
    }

    /// Owner clear on write unlock (rwsem.c:146-150).
    #[inline]
    fn clear_owner(&self) {
        self.owner.store(0, Ordering::Release);
    }

    /// Reader ownership publish, preserving NONSPINNABLE
    /// (rwsem.c:170-177, `__rwsem_set_reader_owned`). The pid is
    /// advisory (spinners only look at it when the writer flag
    /// pattern says WRITER); grant-time publishes may use 0.
    #[inline]
    fn set_reader_owned(&self, pid: u32) {
        let mut v = self.owner.load(Ordering::Relaxed);
        loop {
            let new = (pid as usize) | OWNER_READER_OWNED | (v & OWNER_NONSPINNABLE);
            match self
                .owner
                .compare_exchange_weak(v, new, Ordering::Release, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(cur) => v = cur,
            }
        }
    }

    #[inline]
    fn owner_state_of(&self, v: usize) -> OwnerState {
        if v & OWNER_NONSPINNABLE != 0 {
            OwnerState::Nonspinnable
        } else if v & OWNER_READER_OWNED != 0 {
            OwnerState::Reader
        } else if v & OWNER_PID_MASK != 0 {
            OwnerState::Writer
        } else {
            OwnerState::Null
        }
    }

    /// Latch NONSPINNABLE while the lock stays reader-owned
    /// (rwsem.c:228-239).
    fn set_nonspinnable(&self) {
        let mut v = self.owner.load(Ordering::Relaxed);
        loop {
            if v & OWNER_READER_OWNED == 0 || v & OWNER_NONSPINNABLE != 0 {
                return;
            }
            match self.owner.compare_exchange_weak(
                v,
                v | OWNER_NONSPINNABLE,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(cur) => v = cur,
            }
        }
    }

    /// Clear NONSPINNABLE once the reader count reaches zero
    /// (rwsem.c:941-945).
    #[inline]
    fn clear_nonspinnable(&self) {
        if self.owner.load(Ordering::Relaxed) & OWNER_NONSPINNABLE != 0 {
            self.owner.fetch_and(!OWNER_NONSPINNABLE, Ordering::Release);
        }
    }

    // ==================== fast paths ====================

    /// Acquire the read lock (down_read). Sleeps on contention.
    #[track_caller]
    pub fn down_read(&self) {
        let loc = core::panic::Location::caller();
        rwsem_preempt_disable_at(loc);
        // rwsem_read_trylock (rwsem.c:241-254)
        let count = self.count.fetch_add(READER_BIAS, Ordering::Acquire);
        if count & READ_FAILED_MASK == 0 {
            self.set_reader_owned(crate::process::hotpath::current_pid());
            rwsem_preempt_enable();
            return;
        }
        self.down_read_slowpath(count, loc);
    }

    /// Acquire the write lock (down_write). Sleeps on contention.
    #[track_caller]
    pub fn down_write(&self) {
        let loc = core::panic::Location::caller();
        rwsem_preempt_disable_at(loc);
        // rwsem_write_trylock (rwsem.c:256-266)
        if self
            .count
            .compare_exchange(0, WRITER_LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.set_owner_writer(crate::process::hotpath::current_pid());
            rwsem_preempt_enable();
            return;
        }
        self.down_write_slowpath(loc);
    }

    /// Try to acquire the read lock without sleeping
    /// (down_read_trylock, rwsem.c:1276-1295). Refuses when a writer
    /// holds, OR any waiter is queued, OR handoff is pending.
    #[inline]
    pub fn try_read(&self) -> bool {
        let mut c = self.count.load(Ordering::Acquire);
        while c & READ_FAILED_MASK == 0 {
            match self.count.compare_exchange_weak(
                c,
                c + READER_BIAS,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.set_reader_owned(crate::process::hotpath::current_pid());
                    return true;
                }
                Err(cur) => c = cur,
            }
        }
        false
    }

    /// Try to acquire the write lock without sleeping
    /// (down_write_trylock — CAS 0 -> WRITER_LOCKED).
    #[inline]
    pub fn try_write(&self) -> bool {
        if self
            .count
            .compare_exchange(0, WRITER_LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.set_owner_writer(crate::process::hotpath::current_pid());
            true
        } else {
            false
        }
    }

    /// Release the read lock (up_read, rwsem.c:1338-1355): wake a
    /// waiter when the last reader left with waiters present.
    #[inline]
    pub fn up_read(&self) {
        rwsem_preempt_disable();
        // Reference compares the POST-subtraction word
        // (atomic_long_add_return); fetch_sub returns the PRE word —
        // subtract the bias before testing (the first version compared
        // the pre word and never fired the wake: a writer queued behind
        /// readers could then strand on a quiet mm until some later
        /// up_write pumped the queue).
        let tmp = self.count.fetch_sub(READER_BIAS, Ordering::Release) - READER_BIAS;
        if tmp & (LOCK_MASK | FLAG_WAITERS) == FLAG_WAITERS {
            self.clear_nonspinnable();
            self.rwsem_wake();
        }
        rwsem_preempt_enable();
    }

    /// Release the write lock (up_write, rwsem.c:1360-1378).
    #[inline]
    pub fn up_write(&self) {
        rwsem_preempt_disable();
        self.clear_owner();
        let tmp = self.count.fetch_add(-WRITER_LOCKED, Ordering::Release);
        if tmp & FLAG_WAITERS != 0 {
            self.rwsem_wake();
        }
        rwsem_preempt_enable();
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.count.load(Ordering::Acquire) != 0
    }

    /// rwsem_is_contended: waiters present (lock-free approximation —
    /// FLAG_WAITERS lags one release behind the list).
    #[inline]
    pub fn is_contended(&self) -> bool {
        self.count.load(Ordering::Acquire) & FLAG_WAITERS != 0
    }

    // ==================== wake dispatch ====================

    /// rwsem_wake (rwsem.c:1203-1217): wake the queue head (writer)
    /// or grant a reader batch.
    fn rwsem_wake(&self) {
        let mut wakers = alloc::vec::Vec::with_capacity(8);
        {
            let mut list = self.wait_lock.lock();
            if !list.is_empty() {
                self.mark_wake(WakeType::Any, &mut list, &mut wakers);
            }
        }
        // Wakes strictly outside wait_lock (SleepMutex discipline:
        // never hold the state spinlock across the wake path).
        for t in wakers {
            crate::sched::hotpath::wake_up_process(t);
        }
    }

    /// rwsem_mark_wake (rwsem.c:411-568), ported. Caller holds
    /// wait_lock, list non-empty. Grants are reflected into `count`
    /// and the entries BEFORE any wake; wake targets are collected
    /// into `wakers` (the reference's wake_q) for the caller to wake
    /// outside the lock.
    fn mark_wake(
        &self,
        wake_type: WakeType,
        list: &mut alloc::vec::Vec<RwsemWaiter>,
        wakers: &mut alloc::vec::Vec<*mut crate::process::types::Task>,
    ) {
        // rwsem.c:427-441: writer at the head — mark it for wake; it
        // re-runs the acquire CAS itself, and until it runs, other
        // writers may still steal the lock.
        if list[0].is_writer {
            if wake_type == WakeType::Any {
                wakers.push(list[0].task);
            }
            return;
        }

        // Reader phase. rwsem.c:446-447: read-fail guard.
        if self.count.load(Ordering::Acquire) < 0 {
            return;
        }

        let mut adjustment: i64 = 0;
        if wake_type != WakeType::ReadOwned {
            // rwsem.c:454-484: tentatively add ONE reader bias; if a
            // writer stole the lock, demand HANDOFF once the head
            // waiter timed out, then back the tentative add out.
            adjustment = READER_BIAS;
            let oldcount = self.count.fetch_add(adjustment, Ordering::AcqRel);
            if oldcount & WRITER_LOCKED != 0 {
                let now = crate::timer::get_jiffies();
                if time_after(now, list[0].timeout) {
                    if oldcount & FLAG_HANDOFF == 0 {
                        adjustment -= FLAG_HANDOFF;
                    }
                    list[0].handoff_set = true;
                }
                self.count.fetch_add(-adjustment, Ordering::AcqRel);
                return;
            }
            // Readers now own the lock: publish the reader phase for
            // spinners (rwsem.c:477-483).
            self.owner.fetch_or(OWNER_READER_OWNED, Ordering::Release);
        }

        // Collect up to MAX_READERS_WAKEUP readers (rwsem.c:509-522)
        // — phase-fair: every queued reader is eligible regardless of
        // position; writers stay queued.
        let mut woken: i64 = 0;
        let mut grant: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
        for i in 0..list.len() {
            if woken as usize >= MAX_READERS_WAKEUP {
                break;
            }
            if !list[i].is_writer {
                grant.push(i);
                woken += 1;
            }
        }

        // Count adjustment for the granted batch (rwsem.c:524-546):
        // queued readers' biases were backed out at queue time, so
        // add woken*BIAS (minus a tentative one already applied);
        // clear WAITERS when the list empties; clear HANDOFF once a
        // reader phase is granted.
        adjustment = woken * READER_BIAS - adjustment;
        let count_now = self.count.load(Ordering::Acquire);
        if grant.len() == list.len() {
            adjustment -= FLAG_WAITERS;
            if count_now & FLAG_HANDOFF != 0 {
                adjustment -= FLAG_HANDOFF;
            }
        } else if woken > 0 && count_now & FLAG_HANDOFF != 0 {
            adjustment -= FLAG_HANDOFF;
        }
        if adjustment != 0 {
            self.count.fetch_add(adjustment, Ordering::AcqRel);
        }

        // Second pass (rwsem.c:548-567): the grant IS the removal —
        // a granted reader's bias is already counted, so waking it
        // hands it a lock it owns. Collect wake targets.
        for i in grant.iter().rev() {
            let w = list.remove(*i);
            wakers.push(w.task);
        }
    }

    /// rwsem_try_write_lock (rwsem.c:604-666), ported. Caller holds
    /// wait_lock. `index` is the caller's slot in the list. Returns
    /// true when the write lock was acquired (entry removed, owner
    /// published).
    fn try_write_lock(
        &self,
        list: &mut alloc::vec::Vec<RwsemWaiter>,
        index: usize,
    ) -> bool {
        let mut count = self.count.load(Ordering::Acquire);
        loop {
            let has_handoff = count & FLAG_HANDOFF != 0;
            if has_handoff && list[0].handoff_set && index != 0 {
                // Honor a handoff owned by the first waiter
                // (rwsem.c:616-624).
                return false;
            }

            let mut new = count;
            if count & LOCK_MASK != 0 {
                // Lock held: only a timed-out waiter proceeds, by
                // demanding HANDOFF (rwsem.c:628-638).
                if has_handoff {
                    return false;
                }
                if !time_after(crate::timer::get_jiffies(), list[index].timeout) {
                    return false;
                }
                new |= FLAG_HANDOFF;
            } else {
                // Lock free: take it (rwsem.c:639-644).
                new |= WRITER_LOCKED;
                new &= !FLAG_HANDOFF;
                if list.len() == 1 {
                    new &= !FLAG_WAITERS;
                }
            }

            match self.count.compare_exchange_weak(
                count,
                new,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    count = new;
                    break;
                }
                Err(cur) => count = cur,
            }
        }

        if count & FLAG_HANDOFF != 0 {
            // We set the handoff bit; it belongs to the FIRST waiter
            // (rwsem.c:648-657).
            list[0].handoff_set = true;
            return false;
        }

        // Acquired — remove our entry and publish ownership
        // (rwsem.c:659-665).
        list.remove(index);
        self.set_owner_writer(crate::process::hotpath::current_pid());
        true
    }

    // ==================== optimistic spinning (writers) ====================

    /// rwsem_can_spin_on_owner (rwsem.c:705-730).
    fn can_spin_on_owner(&self) -> bool {
        if crate::sched::hotpath::need_resched() {
            return false;
        }
        let v = self.owner_value();
        if v & OWNER_NONSPINNABLE != 0 {
            return false;
        }
        let pid = (v & OWNER_PID_MASK) as u32;
        if pid != 0 && v & OWNER_READER_OWNED == 0 {
            // Writer-owned: profitable only while the writer is on CPU.
            if !owner_pid_on_cpu(pid) {
                return false;
            }
        }
        true
    }

    /// rwsem_spin_on_owner (rwsem.c:746-792): spin while the owner
    /// word is unchanged; stop on owner change, need_resched, or the
    /// owner leaving the CPU. The pinned owner task is looked up per
    /// owner generation (Rux owner word carries a pid).
    fn spin_on_owner(&self) -> OwnerState {
        let mut v = self.owner_value();
        let mut state = self.owner_state_of(v);
        if state != OwnerState::Writer {
            return state;
        }
        let mut pinned_pid = (v & OWNER_PID_MASK) as u32;
        let mut pinned = pid_hash_lookup_pinned(pinned_pid);
        loop {
            let new = self.owner_value();
            if new != v {
                v = new;
                state = self.owner_state_of(v);
                break;
            }
            if crate::sched::hotpath::need_resched() {
                state = OwnerState::Nonspinnable;
                break;
            }
            let pid = (v & OWNER_PID_MASK) as u32;
            if pid != pinned_pid {
                if !pinned.is_null() {
                    task_put(pinned);
                }
                pinned_pid = pid;
                pinned = pid_hash_lookup_pinned(pid);
            }
            if pinned.is_null()
                || !crate::sched::r#unsafe::task_shared(pinned).ti_on_cpu.load(Ordering::Acquire)
            {
                state = OwnerState::Nonspinnable;
                break;
            }
            core::hint::spin_loop();
        }
        if !pinned.is_null() {
            task_put(pinned);
        }
        state
    }

    /// rwsem_try_write_lock_unqueued (rwsem.c:690-703).
    fn try_write_lock_unqueued(&self) -> bool {
        let mut count = self.count.load(Ordering::Acquire);
        while count & (LOCK_MASK | FLAG_HANDOFF) == 0 {
            match self.count.compare_exchange_weak(
                count,
                count | WRITER_LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.set_owner_writer(crate::process::hotpath::current_pid());
                    return true;
                }
                Err(cur) => count = cur,
            }
        }
        false
    }

    /// Reader-owned spin budget in ns (rwsem_rspin_threshold,
    /// rwsem.c:806-817: `(20 + min(readers,30))/2 us`, cap 25 us).
    fn rspin_threshold_ns(&self) -> u64 {
        let count = self.count.load(Ordering::Acquire);
        let readers = ((count >> READER_SHIFT) as u64).min(30);
        (20 + readers) * 1_000 / 2
    }

    /// rwsem_optimistic_spin (rwsem.c:819-935), bounded-rounds shape
    /// (SleepMutex budget class — no osq in Rux). Returns true when
    /// the write lock was acquired.
    fn optimistic_spin(&self) -> bool {
        let mut prev_owner_state = OwnerState::Null;
        let mut loop_count: u32 = 0;
        let mut rspin_deadline: u64 = 0;

        'outer: for _ in 0..100 {
            let owner_state = self.spin_on_owner();
            if owner_state == OwnerState::Nonspinnable {
                break 'outer;
            }

            if self.try_write_lock_unqueued() {
                return true;
            }

            if owner_state == OwnerState::Reader {
                if prev_owner_state != OwnerState::Reader {
                    // Fresh reader phase: reset the budget
                    // (rwsem.c:862-867).
                    if self.owner_value() & OWNER_NONSPINNABLE != 0 {
                        break 'outer;
                    }
                    rspin_deadline = crate::timer::monotonic_ns_since_boot()
                        + self.rspin_threshold_ns();
                    loop_count = 0;
                } else {
                    // Check the time budget once every 16 iterations
                    // (rwsem.c:876-880 — avoid a clock read per spin).
                    loop_count = loop_count.wrapping_add(1);
                    if loop_count & 0xf == 0
                        && crate::timer::monotonic_ns_since_boot() > rspin_deadline
                    {
                        self.set_nonspinnable();
                        break 'outer;
                    }
                }
            }

            // need_resched check for non-writer owners (rwsem.c:914).
            if owner_state != OwnerState::Writer && crate::sched::hotpath::need_resched() {
                break 'outer;
            }
            prev_owner_state = owner_state;

            for _ in 0..64 {
                core::hint::spin_loop();
                if crate::sched::hotpath::need_resched() {
                    break 'outer;
                }
            }
        }
        false
    }

    // ==================== slow paths ====================

    /// rwsem_down_read_slowpath (rwsem.c:995-1101), ported. Entered
    /// with preempt disabled (down_read's section) and our
    /// READER_BIAS already in `count`.
    fn down_read_slowpath(&self, count: i64, _loc: &'static core::panic::Location<'static>) {
        let rcnt = count >> READER_SHIFT;
        let owner = self.owner_value();

        // Reader optimistic lock stealing (rwsem.c:1003-1032): a
        // constant stream of readers must not starve a sleeping
        // waiter, so do NOT steal when readers own the lock with more
        // than one reader active.
        let reader_stream = owner & OWNER_READER_OWNED != 0
            && rcnt > 1
            && count & WRITER_LOCKED == 0;

        if !reader_stream && count & (WRITER_LOCKED | FLAG_HANDOFF) == 0 {
            // Steal: keep the bias we already added.
            self.set_reader_owned(crate::process::hotpath::current_pid());
            // First reader past queued waiters: hand the reader phase
            // to the queued readers too (rwsem.c:1023-1030).
            if rcnt == 1 && count & FLAG_WAITERS != 0 {
                let mut wakers = alloc::vec::Vec::with_capacity(8);
                {
                    let mut list = self.wait_lock.lock();
                    if !list.is_empty() {
                        self.mark_wake(WakeType::ReadOwned, &mut list, &mut wakers);
                    }
                }
                for t in wakers {
                    crate::sched::hotpath::wake_up_process(t);
                }
            }
            rwsem_preempt_enable();
            return;
        }

        // Parkable path from here. Re-enable preemption first: Rux
        // cannot schedule() with preempt_count != 0, and the true
        // caller state (not our fast-path section) decides sleep.
        rwsem_preempt_enable();

        let cur = match crate::sched::hotpath::current() {
            Some(t) => t as *mut crate::process::types::Task,
            None => {
                // Degradation (SleepMutex parity): no task context
                // (early boot / secondary hart) — cannot sleep. Back
                // our bias out and spin on try_read.
                self.count.fetch_sub(READER_BIAS, Ordering::Release);
                loop {
                    if self.try_read() {
                        return;
                    }
                    for _ in 0..64 {
                        core::hint::spin_loop();
                    }
                }
            }
        };

        let timeout = crate::timer::get_jiffies() + WAIT_TIMEOUT_JIFFIES.max(1);
        let mut enqueued = false;
        let first_park = crate::timer::get_jiffies();

        loop {
            let mut wakers = alloc::vec::Vec::with_capacity(8);
            let granted;
            {
                let mut list = self.wait_lock.lock();

                if !enqueued {
                    // rwsem.c:1041-1057: empty queue + no writer =>
                    // our bias already owns the lock — leave now.
                    if list.is_empty()
                        && self.count.load(Ordering::Acquire) & WRITER_LOCKED == 0
                    {
                        granted = true;
                    } else {
                        // Queue (rwsem.c:1058-1061): back our bias
                        // out; announce WAITERS if the list was empty.
                        if list.is_empty() {
                            self.count
                                .fetch_add(FLAG_WAITERS - READER_BIAS, Ordering::AcqRel);
                        } else {
                            self.count.fetch_sub(READER_BIAS, Ordering::AcqRel);
                        }
                        list.push(RwsemWaiter {
                            task: cur,
                            is_writer: false,
                            timeout,
                            handoff_set: false,
                        });
                        enqueued = true;
                        granted = false;
                        // rwsem_cond_wake_waiter (rwsem.c:975-990,
                        // via rwsem.c:1063).
                        self.cond_wake_waiter(&mut list, &mut wakers);
                    }
                } else {
                    // Re-looping after a wake: the grant is the
                    // removal of our entry (mark_wake second pass).
                    granted = !list.iter().any(|w| w.task == cur);
                }
            }
            for t in wakers {
                crate::sched::hotpath::wake_up_process(t);
            }

            if granted {
                self.set_reader_owned(crate::process::hotpath::current_pid());
                // A concurrent grant's wake may have enqueued us
                // while we never slept — undo (no-op otherwise).
                crate::sched::hotpath::dequeue_task(crate::sched::r#unsafe::task_shared(cur));
                return;
            }

            // State-first + re-check (Task::sleep discipline): set
            // UNINTERRUPTIBLE BEFORE the final grant check under
            // wait_lock, so a racing grant's wake_up_process lands on
            // a sleeping-state task.
            {
                crate::sched::r#unsafe::task_shared(cur).set_state(crate::process::types::TaskState::new(
                    crate::process::types::TaskState::UNINTERRUPTIBLE,
                ));
            }
            let granted_now = {
                let list = self.wait_lock.lock();
                !list.iter().any(|w| w.task == cur)
            };
            // Stall tripwire (see STALL_WAITER_JIFFIES): name a waiter
            // the grant is not reaching.
            if !granted_now
                && time_after(crate::timer::get_jiffies(), first_park + STALL_WAITER_JIFFIES)
                && stall_report_ok()
            {
                let (count, owner, queued) = self.debug_state();
                crate::pr_err!(
                    "RWSEM-STALL-R {} waiter={} waited={}j count={:#x} owner={:#x} queued={} (reader parked without grant)",
                    self.name,
                    crate::sched::r#unsafe::task_shared(cur).pid(),
                    crate::timer::get_jiffies().wrapping_sub(first_park),
                    count,
                    owner,
                    queued
                );
            }
            if granted_now {
                {
                    crate::sched::r#unsafe::task_shared(cur).set_state(crate::process::types::TaskState::new(
                        crate::process::types::TaskState::RUNNING,
                    ));
                }
                self.set_reader_owned(crate::process::hotpath::current_pid());
                crate::sched::hotpath::dequeue_task(crate::sched::r#unsafe::task_shared(cur));
                return;
            }

            crate::arch::cpu::restore_irq(true);
            crate::sched::hotpath::schedule();
            // Woken (or spurious) — state is RUNNING again; loop.
        }
    }

    /// rwsem_cond_wake_waiter (rwsem.c:975-990): after enqueueing a
    /// waiter, kick the queue if the lock is not writer-held. Caller
    /// holds wait_lock.
    fn cond_wake_waiter(
        &self,
        list: &mut alloc::vec::Vec<RwsemWaiter>,
        wakers: &mut alloc::vec::Vec<*mut crate::process::types::Task>,
    ) {
        let count = self.count.load(Ordering::Acquire);
        if count & WRITER_LOCKED != 0 {
            return;
        }
        if count & RWSEM_READER_MASK != 0 {
            self.mark_wake(WakeType::Readers, list, wakers);
        } else {
            self.clear_nonspinnable();
            self.mark_wake(WakeType::Any, list, wakers);
        }
    }

    /// rwsem_down_write_slowpath (rwsem.c:1106-1197), ported. Entered
    /// with preempt disabled (down_write's section).
    fn down_write_slowpath(&self, _loc: &'static core::panic::Location<'static>) {
        // Optimistic spinning first (rwsem.c:1112-1116). Runs
        // preemptible — Rux cannot schedule() with preempt disabled;
        // the need_resched guards are inside.
        rwsem_preempt_enable();
        if self.can_spin_on_owner() && self.optimistic_spin() {
            return;
        }

        let cur = match crate::sched::hotpath::current() {
            Some(t) => t as *mut crate::process::types::Task,
            None => {
                // Degradation: no task context — pure spin.
                loop {
                    if self.try_write() {
                        return;
                    }
                    for _ in 0..64 {
                        core::hint::spin_loop();
                    }
                }
            }
        };

        let timeout = crate::timer::get_jiffies() + WAIT_TIMEOUT_JIFFIES.max(1);
        let first_park = crate::timer::get_jiffies();

        loop {
            let mut wakers = alloc::vec::Vec::with_capacity(8);
            let acquired;
            {
                let mut list = self.wait_lock.lock();

                if !list.iter().any(|w| w.task == cur) {
                    // Enqueue (rwsem.c:1127-1128).
                    list.push(RwsemWaiter {
                        task: cur,
                        is_writer: true,
                        timeout,
                        handoff_set: false,
                    });
                    // rwsem.c:1131-1145: the FIRST waiter announces
                    // WAITERS; any other kicker wakes the head.
                    if list.len() == 1 {
                        self.count.fetch_or(FLAG_WAITERS, Ordering::AcqRel);
                    } else {
                        self.cond_wake_waiter(&mut list, &mut wakers);
                    }
                }

                // Queued acquire attempt (rwsem.c:1151-1155).
                let index = list.iter().position(|w| w.task == cur).unwrap();
                acquired = self.try_write_lock(&mut list, index);
            }
            for t in wakers {
                crate::sched::hotpath::wake_up_process(t);
            }
            if acquired {
                // try_write_lock removed our entry and published us
                // as owner. Undo a concurrent wake's enqueue if any.
                crate::sched::hotpath::dequeue_task(crate::sched::r#unsafe::task_shared(cur));
                return;
            }

            // State-first + re-check, then sleep.
            {
                crate::sched::r#unsafe::task_shared(cur).set_state(crate::process::types::TaskState::new(
                    crate::process::types::TaskState::UNINTERRUPTIBLE,
                ));
            }
            let acquired_now = {
                let mut list = self.wait_lock.lock();
                match list.iter().position(|w| w.task == cur) {
                    Some(i) => self.try_write_lock(&mut list, i),
                    None => true, // removed => granted earlier
                }
            };
            // Stall tripwire (see STALL_WAITER_JIFFIES).
            if !acquired_now
                && time_after(crate::timer::get_jiffies(), first_park + STALL_WAITER_JIFFIES)
                && stall_report_ok()
            {
                let (count, owner, queued) = self.debug_state();
                // Owner liveness: a writer bit with a dead owner pid is a
                // leaked section; a live owner is a genuinely slow holder.
                let owner_pid = (owner & OWNER_PID_MASK) as u32;
                let owner_alive = if owner_pid != 0 {
                    let t = pid_hash_lookup_pinned(owner_pid);
                    if t.is_null() {
                        false
                    } else {
                        let alive = !crate::sched::r#unsafe::task_shared(t).state().is_dead();
                        task_put(t);
                        alive
                    }
                } else {
                    false
                };
                crate::pr_err!(
                    "RWSEM-STALL-W {} waiter={} waited={}j count={:#x} owner={} alive={} queued={} (writer parked without grant)",
                    self.name,
                    crate::sched::r#unsafe::task_shared(cur).pid(),
                    crate::timer::get_jiffies().wrapping_sub(first_park),
                    count,
                    owner_pid,
                    owner_alive,
                    queued
                );
            }
            if acquired_now {
                {
                    crate::sched::r#unsafe::task_shared(cur).set_state(crate::process::types::TaskState::new(
                        crate::process::types::TaskState::RUNNING,
                    ));
                }
                crate::sched::hotpath::dequeue_task(crate::sched::r#unsafe::task_shared(cur));
                return;
            }

            crate::arch::cpu::restore_irq(true);
            crate::sched::hotpath::schedule();
            // Woken: loop re-runs the queued acquire.
        }
    }

    /// Diagnostic snapshot (fuxstuck census / taskdump shape,
    /// try-locks only): (count, owner, queued; queued == usize::MAX
    /// means the wait lock was busy).
    pub fn debug_state(&self) -> (i64, usize, usize) {
        let queued = match self.wait_lock.try_lock() {
            Some(l) => l.len(),
            None => usize::MAX,
        };
        (
            self.count.load(Ordering::Relaxed),
            self.owner_value(),
            queued,
        )
    }

    /// Diagnostic name.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

// ==================== helpers ====================

#[inline]
fn pid_hash_lookup_pinned(pid: u32) -> *mut crate::process::types::Task {
    if pid == 0 {
        return core::ptr::null_mut();
    }
    crate::process::hotpath::pid_hash_lookup_pinned(pid)
}

#[inline]
fn task_put(t: *mut crate::process::types::Task) {
    crate::process::types::Task::task_put(t);
}

#[inline]
fn owner_pid_on_cpu(pid: u32) -> bool {
    let t = pid_hash_lookup_pinned(pid);
    if t.is_null() {
        return false;
    }
    let on = crate::sched::r#unsafe::task_shared(t).ti_on_cpu.load(Ordering::Acquire);
    task_put(t);
    on
}

#[inline]
#[track_caller]
fn rwsem_preempt_disable_at(loc: &'static core::panic::Location<'static>) {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        loc,
    );
}

#[inline]
fn rwsem_preempt_enable() {
    crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::PREEMPT_OFFSET);
}

#[inline]
fn rwsem_preempt_disable() {
    crate::interrupt::preempt::preempt_count_add(crate::interrupt::preempt::PREEMPT_OFFSET);
}

// ==================== generic Rwsem<T> ====================

// SAFETY: Rwsem<T> mediates all access to the inner data: shared (&T)
// via read-locked guards, exclusive (&mut T) via write-locked guards.
unsafe impl<T: ?Sized + Send> Send for Rwsem<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for Rwsem<T> {}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Rwsem<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rwsem")
            .field("locked", &self.raw.is_locked())
            .finish_non_exhaustive()
    }
}

impl<T> Rwsem<T> {
    #[inline]
    pub const fn new(name: &'static str, data: T) -> Self {
        Self {
            raw: RwSemaphore::new(name),
            data: UnsafeCell::new(data),
        }
    }

    /// Read lock (down_read). Guard drop = up_read.
    #[inline]
    #[track_caller]
    pub fn read(&self) -> RwsemReadGuard<'_, T> {
        self.raw.down_read();
        RwsemReadGuard { lock: self }
    }

    /// Write lock (down_write). Guard drop = up_write.
    #[inline]
    #[track_caller]
    pub fn write(&self) -> RwsemWriteGuard<'_, T> {
        self.raw.down_write();
        RwsemWriteGuard { lock: self }
    }

    /// Non-sleeping read trylock.
    #[inline]
    #[track_caller]
    pub fn try_read(&self) -> Option<RwsemReadGuard<'_, T>> {
        if self.raw.try_read() {
            Some(RwsemReadGuard { lock: self })
        } else {
            None
        }
    }

    /// Non-sleeping write trylock.
    #[inline]
    #[track_caller]
    pub fn try_write(&self) -> Option<RwsemWriteGuard<'_, T>> {
        if self.raw.try_write() {
            Some(RwsemWriteGuard { lock: self })
        } else {
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    #[inline]
    pub fn is_contended(&self) -> bool {
        self.raw.is_contended()
    }

    /// 78th (ohw115): diagnostic snapshot of the raw owner word and
    /// count for the census (the samgr VMA-lock wedge hunt — a thread
    /// stuck mid-brk needs to know WHO holds the write lock). Relaxed
    /// reads only; never on any fast path beyond the two loads.
    #[inline]
    pub fn diag(&self) -> (usize, i64) {
        (
            self.raw.owner.load(Ordering::Relaxed),
            self.raw.count.load(Ordering::Relaxed),
        )
    }
}

// SAFETY: guard exists only while the read lock is held, preventing
// any writer from obtaining exclusive access to the data.
unsafe impl<T: ?Sized + Sync> Send for RwsemReadGuard<'_, T> {}

impl<T: ?Sized> Deref for RwsemReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: read lock held — no &mut access can occur concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwsemReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.up_read();
    }
}

// SAFETY: guard exists only while the write lock is held exclusively.
unsafe impl<T: ?Sized + Send> Send for RwsemWriteGuard<'_, T> {}

impl<T: ?Sized> Deref for RwsemWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: write lock held exclusively.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwsemWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: write lock held exclusively.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwsemWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.up_write();
    }
}

// ═══════════════ seqlock ═════════════════════════════════════════
// (was seqlock.rs)
// SeqLock — sequence lock for read-mostly data.
//
// Writers are exclusive (serialized like a spinlock).
// Readers never block: they snapshot the data and retry if a write
// was in progress during the read window.
//
// Constraints:
//   - T must be Copy (readers snapshot the data).
//   - Readers disable preemption internally to prevent same-CPU
//     writer preemption during the read window.
//   - Suitable for small-to-moderate structs (copy cost is the read cost).
//
// Protocol:
//   - Sequence counter low bit: even = no writer, odd = writer active.
//   - Writer: increment (odd) → write data → increment (even).
//   - Reader: read seq → snapshot data → re-read seq → retry if changed.

// (INV-SEQLOCK-1: the seqlock family — data cells are read under the
// even-sequence validation (odd = writer in flight); writers hold the
// exclusive phase; the UnsafeCell rows are the discipline itself.)

// ============================================================================
// RawSeqLock
// ============================================================================

impl RawSeqLock {
    #[inline]
    pub const fn new() -> Self {
        Self {
            sequence: AtomicUsize::new(0),
        }
    }

    /// Acquire writer slot: spin until sequence is even, then CAS to odd.
    #[inline]
    pub fn write_lock(&self) {
        loop {
            let seq = self.sequence.load(Ordering::Relaxed);
            if seq & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            if self.sequence
                .compare_exchange_weak(seq, seq + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Release writer slot: increment sequence to even.
    #[inline]
    pub fn write_unlock(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
    }

    /// Try to acquire writer slot. Returns true on success.
    #[inline]
    pub fn try_write_lock(&self) -> bool {
        let seq = self.sequence.load(Ordering::Relaxed);
        if seq & 1 != 0 {
            return false;
        }
        self.sequence
            .compare_exchange_weak(seq, seq + 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Read the current sequence number (Acquire ordering).
    #[inline]
    pub fn read_begin(&self) -> usize {
        self.sequence.load(Ordering::Acquire)
    }

    /// Check if the sequence changed or a writer is active.
    /// Returns true if the read must be retried.
    #[inline]
    pub fn read_retry(&self, seq1: usize) -> bool {
        let seq2 = self.sequence.load(Ordering::Acquire);
        seq1 != seq2 || seq2 & 1 != 0
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.sequence.load(Ordering::Relaxed) & 1 != 0
    }
}

// ============================================================================
// SeqLock<T>
// ============================================================================

// Safety: writers have exclusive access; readers get a Copy snapshot.
unsafe impl<T: Copy + Send> Send for SeqLock<T> {}
unsafe impl<T: Copy + Send> Sync for SeqLock<T> {}

impl<T: Copy> SeqLock<T> {
    #[inline]
    pub const fn new(data: T) -> Self {
        Self {
            raw: RawSeqLock::new(),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the writer lock. Returns a guard with `DerefMut`.
    #[inline]
    pub fn write(&self) -> SeqLockWriteGuard<'_, T> {
        // R21-N5: IRQs OFF, not just preempt — a softirq writer on the
        // same CPU (loopback stats from Timer/NetRx at irq_exit) used to
        // self-deadlock on the odd sequence held by the interrupted
        // preempt-disabled writer. The guard's Drop must match (see below).
        crate::interrupt::preempt::preempt_count_add(
            crate::interrupt::preempt::SOFTIRQ_OFFSET,
        );
        let irq_state = crate::arch::cpu::save_and_disable_irq();
        self.raw.write_lock();
        SeqLockWriteGuard { lock: self, irq_state }
    }

    /// Try to acquire the writer lock. Returns None if a writer is active.
    ///
    /// Accounting must mirror `write()`/`Drop` exactly (R21-N5): the old
    /// body added PREEMPT_OFFSET here while the guard's Drop subtracts
    /// SOFTIRQ_OFFSET — a successful try_write left preempt_count at
    /// 1 − 256 = −255 (underflow: the task looked in_interrupt() forever
    /// after; the leakorigin user-return tripwire would have flagged it).
    /// No in-tree caller existed yet, so the bug was latent.
    #[inline]
    pub fn try_write(&self) -> Option<SeqLockWriteGuard<'_, T>> {
        preempt_disable_bh();
        let irq_state = crate::arch::cpu::save_and_disable_irq();
        if self.raw.try_write_lock() {
            Some(SeqLockWriteGuard { lock: self, irq_state })
        } else {
            crate::arch::cpu::restore_irq(irq_state);
            preempt_enable_bh();
            None
        }
    }

    /// Read a snapshot of the protected data.
    ///
    /// Disables preemption, reads the sequence counter, copies the data,
    /// re-reads the sequence counter, and retries if a write intervened.
    /// Returns the owned snapshot.
    #[inline]
    pub fn read(&self) -> T {
        seqlock_preempt_disable();
        let result = self.read_inner();
        seqlock_preempt_enable();
        result
    }

    /// Read without disabling preemption.
    ///
    /// # Safety
    /// Caller must hold preemption disabled for the duration of this call.
    #[inline]
    pub unsafe fn read_preempt_disabled(&self) -> T {
        self.read_inner()
    }

    /// Check if a writer is currently active.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    /// Core read loop (no preemption management).
    #[inline]
    fn read_inner(&self) -> T {
        loop {
            let seq = self.raw.read_begin();
            if seq & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            // SAFETY: preemption is disabled and sequence is even (no writer).
            let snapshot = unsafe { core::ptr::read_volatile(self.data.get()) };
            if self.raw.read_retry(seq) {
                core::hint::spin_loop();
                continue;
            }
            return snapshot;
        }
    }
}

// ============================================================================
// SeqLockWriteGuard
// ============================================================================

unsafe impl<T: Copy + Send> Send for SeqLockWriteGuard<'_, T> {}

impl<T: Copy> Deref for SeqLockWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: Copy> DerefMut for SeqLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: Copy> Drop for SeqLockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.write_unlock();
        // R21-N5: undo the irqsave with the exact saved state.
        crate::arch::cpu::restore_irq(self.irq_state);
        crate::interrupt::preempt::preempt_count_sub(
            crate::interrupt::preempt::SOFTIRQ_OFFSET,
        );
    }
}

// ============================================================================
// Inline helpers
// ============================================================================

#[inline]
fn seqlock_preempt_disable() {
    crate::interrupt::preempt::preempt_count_add(crate::interrupt::preempt::PREEMPT_OFFSET);
}

#[inline]
fn seqlock_preempt_enable() {
    crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::PREEMPT_OFFSET);
}

/// BH-variant bracket used by the writer guards (R21-N5): `write()` and
/// `try_write()` add SOFTIRQ_OFFSET (which also disables preemption) and
/// `SeqLockWriteGuard::drop` subtracts the same — see there.
#[inline]
fn preempt_disable_bh() {
    crate::interrupt::preempt::preempt_count_add(crate::interrupt::preempt::SOFTIRQ_OFFSET);
}

#[inline]
fn preempt_enable_bh() {
    crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::SOFTIRQ_OFFSET);
}

// ═══════════════ rcu ═════════════════════════════════════════
// (was rcu.rs)
// Tiny RCU — Read-Copy-Update for non-preemptible kernels
//
// rcu_read_lock() = preempt_disable()
// rcu_read_unlock() = preempt_enable()
//
// Grace period: all CPUs must pass through at least one quiescent state
// (context switch, idle loop, or return to user mode).

// (INV-RCU-1: the RCU family — grace-period state is per-CPU
// counters under the gp lock; the callback trampolines' raw fn words
// are published before the gp flip and only invoked after it (the
// wait/flip discipline the module documents.)

// ============================================================================
// Types
// ============================================================================

impl RcuHead {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            list: ListHead::new(),
            func: |_| {},
        }
    }

    pub fn init(&mut self) {
        self.list.init();
        self.initialized = true;
    }
}

// ============================================================================
// Per-CPU state
// ============================================================================

/// Per-CPU callback list head (static, zero-initialized, `init()` called at boot).
static mut RCU_CBS: [ListHead; MAX_CPUS] = {
    const INIT: ListHead = ListHead::new();
    [INIT; MAX_CPUS]
};

/// Per-CPU lock for the callback list (simple TAS spinlock).
static RCU_CBS_LOCK: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

// ============================================================================
// Grace-period generation counter
// ============================================================================

/// Global generation counter. Incremented when any CPU's softirq finishes
/// processing a batch of callbacks.
static RCU_GEN: AtomicU32 = AtomicU32::new(1);

/// Per-CPU quiescent-state generation. Records the most recent `RCU_GEN`
/// value this CPU reported via `rcu_note_context_switch()`.
static RCU_QS_GEN: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];

// ============================================================================
// Public API: read-side
// ============================================================================

/// Enter an RCU read-side critical section (preempt_disable).
#[inline]
pub fn rcu_read_lock() {
    crate::interrupt::preempt::preempt_count_add(
        crate::interrupt::preempt::PREEMPT_OFFSET,
    );
}

/// Exit an RCU read-side critical section (preempt_enable).
#[inline]
pub fn rcu_read_unlock() {
    crate::interrupt::preempt::preempt_count_sub(
        crate::interrupt::preempt::PREEMPT_OFFSET,
    );
}

// ============================================================================
// Public API: callback registration
// ============================================================================

/// Register an RCU callback. After all CPUs pass through a quiescent state,
/// `head.func` will be called with `head` as the argument from softirq context.
///
/// # Safety
/// - `head` must remain valid until the callback executes.
/// - `head.func` must be a valid function pointer.
/// - `head.list` must have been initialized via `init()`.
/// Queue `head`'s callback for the post-grace free (the reference's
/// `__call_rcu`). Takes the RcuHead by reference — the caller's
/// init-before-use contract is the type's own (init() writes
/// `initialized`), so the guard below is a plain field read.
pub fn call_rcu(head: &mut RcuHead) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    // Guard against uninitialized RcuHead — if init() was never called,
    // add_tail would corrupt the list.
    if !head.initialized {
        return;
    }

    lock_cb_list(cpu);
    // SAFETY: the cb_list lock is held for this CPU, so RCU_CBS[cpu] is
    // not accessed concurrently; head.list was initialised via init()
    // (the RcuHead protocol) and the intrusive rewire is the list's own
    // audited operation (the reference's blist_add_tail counterpart) —
    // it mutates THIS node's links, hence &mut in the signature.
    unsafe {
        head.list.add_tail(&mut RCU_CBS[cpu]);
    }
    unlock_cb_list(cpu);

    // Raise softirq so callbacks get processed promptly.
    crate::interrupt::softirq::raise_softirq(
        crate::interrupt::softirq::SoftirqIndex::Rcu as usize,
    );
}

/// Number of tasks blocked inside `synchronize_rcu()`. When zero — the
/// overwhelmingly common state — the QS report fast-path below costs one
/// relaxed load and stops there, keeping the per-syscall tax on the
/// user-return path at that single load (perf gate: p1/p2 must not move).
static RCU_WAITERS: AtomicU32 = AtomicU32::new(0);

/// Consecutive failed grace rounds (reset by any passing round) and the
/// one-shot latch for the frozen-CPU task dump — see wait_qs_sleepy.
static GP_CONSEC_FAIL: AtomicU32 = AtomicU32::new(0);
static GP_DUMPED: AtomicBool = AtomicBool::new(false);

// ============================================================================
// Public API: quiescent-state reporting
// ============================================================================

/// Report a quiescent state for the current CPU.
///
/// Call from: `__schedule()`, `cpu_idle_loop()`, and the return-to-user
/// path (the common trap tails). The user-return hookup is what makes
/// grace periods complete on syscall-heavy CPUs without waiting for
/// their scheduler tick: before it existed, a CPU spinning in user
/// syscalls never reported a QS and every `synchronize_rcu()` waiter
/// (e.g. the MmStruct drop on every process exit) stalled for a full
/// time slice (~10 ms) — the measured 38x p4_fork_exit gap.
#[inline]
pub fn rcu_note_context_switch() {
    if RCU_WAITERS.load(Ordering::Relaxed) == 0 {
        return;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < MAX_CPUS {
        let gen = RCU_GEN.load(Ordering::Acquire);
        RCU_QS_GEN[cpu].store(gen, Ordering::Release);
    }
}

// ============================================================================
// Public API: grace-period wait
// ============================================================================

/// Wait for a full grace period.
///
/// Must be called from process context (preempt_count == 0).
pub fn synchronize_rcu() {
    let gen = RCU_GEN.load(Ordering::Acquire);

    // Announce the waiter BEFORE snapshotting/reporting so every
    // user-return from here on reports its QS (see RCU_WAITERS).
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);

    // Report our own quiescent state.
    rcu_note_context_switch();

    loop {
        // Check every online CPU has reported a QS at or after `gen`.
        let mut all_qs = true;
        for i in 0..MAX_CPUS {
            if !crate::sched::hotpath::cpu_online(i) {
                continue;
            }
            if !cpu_quiescent(i, gen) {
                all_qs = false;
                break;
            }
        }
        if all_qs {
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        // Yield the CPU instead of busy-spinning.  A bare spin_loop()
        // deadlocks on UP (no other task can produce quiescent states)
        // and wastes CPU time on SMP.  schedule() lets other tasks run
        // so they can context-switch and report their quiescent states.
        crate::sched::hotpath::schedule();
    }
}

/// Bounded grace-period wait — the exit-path liveness fix (lb125 decode).
///
/// The single sticking point of the bimodal OH boots: a CPU whose current
/// task runs unpreemptible for a long window (phantom preempt counts, the
/// monopolist family, or any long preempt-off kernel loop) reports NO
/// quiescent state, so an UNBOUNDED synchronize_rcu() stalls for the whole
/// window. The exit path waits it in THREE places (MmStruct::drop's raw-
/// borrower grace wait, thread kernel-stack free, release_task's task-
/// memory free) — a stalled exit cannot reach ZOMBIE + the deferred
/// SIGCHLD, and init — whose whole loop serializes on child exits —
/// freezes with it: no params, no service starts, no reaping. On lb125
/// the chipset_init child (pid 466) exited at 42.03s and init learned at
/// 105.39s (63s inside the grace wait): every service-start freeze,
/// device_manager 15s-timeout chain and the crawl-vs-healthy bimodality
/// of the whole campaign queued behind that one wait.
///
/// Our RCU read sections are preempt-disable-based and microseconds long
/// (pid_hash lookups); every real reader is done in far less than the
/// bound. A wait that exceeds it is by construction a monopolist window,
/// not a reader — proceed and NAME it. The wall-clock protections the
/// exit path already stacks on top (vma_write walker exclusion leaves an
/// empty tree for late readers; deferred_free holds the struct free
/// another 1.5 s) are unchanged.
pub fn synchronize_rcu_bounded(max_ms: u64, site: &'static str) {
    let _ = synchronize_rcu_bounded_checked(max_ms, site);
}

/// Bounded grace-period wait that reports whether the grace period
/// completed in budget. Callers whose drop REQUIRES the grace (the
/// fd-table retire reaper requeues its batch on a timeout instead of
/// dropping) need the outcome; everyone else keeps the void form.
///
/// NOTE (fake-grace hazard): this form snapshots the current RCU_GEN
/// without advancing it. The generation only advances when an RCU
/// softirq finishes a callback batch, so on a quiet system the snapshot
/// equals the value every CPU already reported at the LAST grace — the
/// wait returns instantly with no grace at all. Correct only for
/// callers whose lifetimes do not hinge on a fresh grace. Lifetime
/// callers use `synchronize_rcu_forced`.
pub fn synchronize_rcu_bounded_checked(max_ms: u64, site: &'static str) -> bool {
    let gen = RCU_GEN.load(Ordering::Acquire);
    announce_and_wait(gen, max_ms, site)
}

/// Snapshot-form grace wait with SLEEPING polls — the exit-path form.
///
/// Same snapshot semantics as `synchronize_rcu_bounded_checked` (an
/// instant pass stays possible when reports are already fresh — e.g.
/// right after another waiter's round), but the wait SLEEPS between
/// checks instead of yielding in a runnable spin: a waiter that is the
/// only runnable task on its CPU repicks itself through `schedule()`
/// and never idles, which on single-accelerator boots both burns the
/// CPU and starves the tick (measured as the ~1s TIMER-SOFTIRQ-STALL
/// class during long userspace phases). The exit paths pay this wait
/// per exit, so the fast-pass matters: keep the snapshot, sleep only
/// while actually behind.
pub fn synchronize_rcu_bounded_sleepy(max_ms: u64, site: &'static str) -> bool {
    let gen = RCU_GEN.load(Ordering::Acquire);
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);
    rcu_note_context_switch();
    wait_qs_sleepy(gen, max_ms, site)
}

/// Snapshot-form grace wait with a bounded POLL phase before the sleepy
/// fallback — the exit-path form (106th reap-engine fix).
///
/// The reap path (wait4 -> release_task -> task-memory free) and the
/// exit path's own mm drop paid `synchronize_rcu_bounded_sleepy` per
/// exit: when the snapshotted generation is stale (any recent
/// `synchronize_rcu_forced*` or callback batch advanced RCU_GEN), the
/// first check fails and the sleepy loop's next observation point is
/// one jiffie out — every exit under a busy install stream rounded its
/// grace wait UP to the next tick (~10 ms class; the record's largest
/// un-attacked engine, 5.9k deferred-PMI samples in schedule <-
/// release_task <- do_wait_nonblock <- sys_wait4). The missing reporter
/// is almost always a CPU microseconds away from its next user-return
/// or context switch, which is a report site; a ~100 µs pause-hinted
/// poll (same shape as `synchronize_rcu_forced_polled`'s phase 1)
/// covers it, and only a genuinely monopolist CPU falls through to the
/// one-jiffy sleeping poll. Snapshot semantics unchanged (no RCU_GEN
/// churn from the exit path).
pub fn synchronize_rcu_bounded_pollfirst(max_ms: u64, site: &'static str) -> bool {
    let gen = RCU_GEN.load(Ordering::Acquire);
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);
    rcu_note_context_switch();
    // Poll phase: ~100 µs of pause-hinted checks (see forced_polled).
    for _ in 0..50_000 {
        if all_online_cpus_reported(gen) {
            shared_grace_done();
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            return true;
        }
        core::hint::spin_loop();
    }
    wait_qs_sleepy(gen, max_ms, site)
}

/// TRUE bounded grace period, for callers that free objects on its
/// strength: advance RCU_GEN first, so every quiescent-state report
/// already recorded is BELOW the waited-for generation and cannot
/// satisfy the check. Every CPU must then pass a report site
/// (user-return, context switch, idle) that executed after this call
/// began — which is exactly the guarantee an RCU read-side window
/// (preempt-off load→increment, no schedule point inside) needs.
pub fn synchronize_rcu_forced(max_ms: u64, site: &'static str) -> bool {
    let gen = shared_grace_target();
    announce_and_wait(gen, max_ms, site)
}

/// Shared-grace acquisition (the reference's synchronize_rcu shape):
/// concurrent waiters share ONE in-flight grace period. The FIRST
/// waiter opens the period (RCU_GEN.fetch_add); everyone else latches
/// the same target while it is in flight. Safety for a late joiner:
/// its target is always >= the gen it observed at entry + 1 — the
/// reference's wait_rcu_gp(gp_seq + 1) form — so a quiescent report
/// older than its read-side window can never satisfy its wait.
///
/// Why this exists: the per-waiter fetch_add let a close-storm's
/// waiters push the completion target past every CPU's freshest report
/// (measured: qs trailing the target by 1-3 generations in the
/// RCU-GP-BOUND dumps) — each waiter timed out, deferred, and became
/// another waiter: the lat_fs / bw_tcp bimodal stall family.
static GP_IN_FLIGHT: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Public twin for the async reap drainer: open (or latch) one shared
/// grace target for a batch of deferrals. The bump happens strictly
/// after the callers' reader-window-closing removals, so a full round
/// at this target covers every window that predates the batch.
pub fn shared_grace_target_pub() -> u32 {
    shared_grace_target()
}

fn shared_grace_target() -> u32 {
    if GP_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        // First waiter: open a new period.
        RCU_GEN.fetch_add(1, Ordering::AcqRel) + 1
    } else {
        // A period is in flight — latch its target. The read-side
        // window this waiter needs closed STARTED before this load, and
        // the in-flight period opened at or after the previous period's
        // completion — a CPU report satisfying this target was taken
        // after this period began, which is after our window opened
        // (the same guarantee the per-waiter fetch_add gave).
        RCU_GEN.load(Ordering::Acquire)
    }
}

/// Complete the in-flight period (called by the waiter that observed
/// all CPUs reported) so the next shared_grace_target opens a fresh
/// one.
fn shared_grace_done() {
    GP_IN_FLIGHT.store(false, Ordering::Release);
}

/// Public twin for the async reap drainer: release the shared-grace
/// in-flight slot after a POLLED round completes (or is abandoned) —
/// without this the next target only ever latches the finished gen.
pub fn shared_grace_done_pub() {
    shared_grace_done();
}

/// Public twin: hold the waiter count while a polled grace round is
/// pending. rcu_note_context_switch SUPPRESSES its QS store when
/// RCU_WAITERS == 0 — a poller that never holds the count starves its
/// own round (the drainer's cap-exhaustion root cause, 2026-11-21).
pub fn grace_waiter_hold() {
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);
}

/// Public twin for the async reap drainer: the CURRENT generation,
/// observed WITHOUT bumping or latching (the mint-time snapshot for the
/// per-entry grace bound — see reapdrain's needs field).
pub fn rcu_gen_now() -> u32 {
    RCU_GEN.load(Ordering::Acquire)
}

/// Public twin for the async reap drainer: the grace WATERMARK — the
/// largest generation every online CPU has reported at or after (idle
/// CPUs are permanently quiescent and do not bound it). This is
/// `all_online_cpus_reported(T)` solved for T: an RCU reader window
/// that opened BEFORE a removal at generation G is closed on every CPU
/// once the watermark is >= G+1 (reports at G+1 can only have been
/// taken at a round OPENED after that removal — the
/// synchronize_rcu_forced discipline, per-entry).
pub fn rcu_grace_watermark() -> u32 {
    let mut wm = u32::MAX;
    for i in 0..MAX_CPUS {
        if !crate::sched::hotpath::cpu_online(i) {
            continue;
        }
        if crate::sched::hotpath::cpu_running_idle(i) {
            continue;
        }
        let g = RCU_QS_GEN[i].load(Ordering::Acquire);
        if g < wm {
            wm = g;
        }
    }
    wm
}

/// Public twin for the async reap drainer: report THIS cpu as
/// quiescent at the current generation (the boot-118 fix — the
/// drainer's tick context is outside every RCU read-side section, so
/// its own report is sound; a round whose only laggard is a cpu that
/// reports via switches alone could stall for the whole storm when
/// that cpu runs long user-mode stretches between switches).
pub fn report_self_quiescent() {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }
    if RCU_WAITERS.load(Ordering::Relaxed) == 0 {
        return;
    }
    let gen = RCU_GEN.load(Ordering::Acquire);
    RCU_QS_GEN[cpu].store(gen, Ordering::Release);
}

/// Public twin: release the held waiter count.
pub fn grace_waiter_release() {
    RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
}

/// Public twin: the CPUs holding a polled round at `target` back —
/// online, not idle, QS below target. The drainer's expedited nudge
/// (resched IPI per the reference's synchronize_rcu_expedited shape)
/// targets exactly these.
pub fn grace_lagging_cpus(target: u32) -> alloc::vec::Vec<usize> {
    let mut v = alloc::vec::Vec::new();
    for i in 0..MAX_CPUS {
        if !crate::sched::hotpath::cpu_online(i) {
            continue;
        }
        if cpu_quiescent(i, target) {
            continue;
        }
        v.push(i);
    }
    v
}

/// Forced grace period with SLEEPING polls — the fd-table reaper's form.
///
/// Same protocol as `synchronize_rcu_forced` (forced epoch, every CPU
/// must report a fresh QS), but the wait SLEEPS one jiffy between
/// checks instead of yielding in a runnable loop: a kthread that is the
/// only runnable task on its CPU repicks itself through `schedule()`
/// (the same-task repick fast path never enters idle), so its CPU never
/// reaches an idle QS report and a quiet boot livelocks the wait. The
/// sleep is the khungtaskd idiom: arm a one-shot timer, INTERRUPTIBLE,
/// re-check, schedule, clean up the racing timer. Checks come first —
/// a grace round whose reporters are already past (idle and
/// syscall-exiting CPUs report continuously while a waiter is armed)
/// still completes without sleeping at all.
pub fn synchronize_rcu_forced_sleepy(max_ms: u64, site: &'static str) -> bool {
    let gen = shared_grace_target();
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);
    rcu_note_context_switch();
    wait_qs_sleepy(gen, max_ms, site)
}

/// Forced grace period with a bounded POLL phase before the sleepy
/// fallback — the fd-close completion form (r26).
///
/// A busy peer CPU stores its QS report at its next return to user
/// (microseconds away), but the sleepy form would only notice at its
/// next 1-jiffy poll — turning every synchronous close into a ~1-jiffy
/// stall (measured: LTP inotify11's 10k-file churn blew its 30 s
/// budget). This form spin-polls (with the pause hint) for the
/// typical µs-scale report window FIRST, and only falls back to the
/// sleeping poll for a genuinely monopolist CPU.
pub fn synchronize_rcu_forced_polled(max_ms: u64, site: &'static str) -> bool {
    let gen = shared_grace_target();
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);
    rcu_note_context_switch();
    // Poll phase: ~100 µs of pause-hinted checks covers a peer's
    // user-return several times over on KVM.
    for _ in 0..50_000 {
        if all_online_cpus_reported(gen) {
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            return true;
        }
        core::hint::spin_loop();
    }
    wait_qs_sleepy(gen, max_ms, site)
}

/// The sleeping poll loop shared by the sleepy wait forms: check first
/// (fresh reporters pass without ever sleeping), then one-jiffy
/// INTERRUPTIBLE sleeps armed by a one-shot timer (the khungtaskd
/// idiom), until every online CPU reports at/after `gen` or the budget
/// expires. Returns whether the round completed.
fn wait_qs_sleepy(gen: u32, max_ms: u64, site: &'static str) -> bool {
    use crate::process::types::TaskState;

    let hz = crate::drivers::hotpath::timer::HZ as u64;
    let deadline = crate::timer::get_jiffies() + max_ms * hz / 1000;

    loop {
        if all_online_cpus_reported(gen) {
            shared_grace_done();
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            GP_CONSEC_FAIL.store(0, Ordering::Relaxed);
            return true;
        }
        if crate::timer::get_jiffies() >= deadline {
            shared_grace_done();
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            static SGP_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = SGP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // Print budget 4 (was 200): each report is a long IF=0
            // polled-UART line; under the close storm the reports
            // themselves pinned the console while the peer CPU's PID 1
            // sat blocked in write(1) — manufacturing the very "stuck
            // CPU" they were diagnosing (the bw_file_rd livelock
            // anatomy, 2026-11-19). Four lines name the class; the
            // rest stay silent.
            if n < 4 {
                // rwlock-livelock: the old print showed only qs[0..4] —
                // on the SMP8 ub2404 rig the non-quiescent CPU was
                // invisible (qs0-3 all == gen while the round failed),
                // so the monopolist was unnameable. Show all MAX_CPUS
                // (gen|on|idle triplets) and the first stuck CPU's
                // slot-current task inline (same fields the
                // frozen-CPU truth dump reads; no IPI here).
                let mut qs_desc = alloc::string::String::new();
                let mut stuck = usize::MAX;
                for i in 0..MAX_CPUS {
                    let on = crate::sched::hotpath::cpu_online(i);
                    let idle = crate::sched::hotpath::cpu_running_idle(i);
                    let q = RCU_QS_GEN[i].load(Ordering::Acquire);
                    use core::fmt::Write as _;
                    let _ = write!(
                        qs_desc,
                        "{}:{:x}/{}/{},",
                        i, q, on as u8, idle as u8
                    );
                    if on && stuck == usize::MAX && !cpu_quiescent(i, gen) {
                        stuck = i;
                    }
                }
                let mut victim = alloc::string::String::new();
                if stuck != usize::MAX {
                    let cur = crate::sched::hotpath::grq_diag_cpu_currents();
                    let t = cur[stuck].2 as *mut crate::process::types::Task;
                    if !t.is_null() {
                        // SAFETY: diagnostic read of stable fields on the
                        // per-CPU current Task (idle/static or heap; both
                        // stay valid while current on that CPU).
                        let tk = unsafe { &*t };
                        use core::fmt::Write as _;
                        let cb = tk.comm();
                        let cl = cb.iter().position(|&b| b == 0).unwrap_or(16);
                        let name = core::str::from_utf8(&cb[..cl]).unwrap_or("?");
                        let _ = write!(
                            victim,
                            " stuck-cpu{}-cur pid={} tgid={} {} preempt={:#x}",
                            stuck,
                            tk.pid(),
                            tk.tgid(),
                            name,
                            tk.preempt_count()
                        );
                        // rwlock-livelock: the victim's live kernel frames
                        // sit BELOW the sp saved at its last switch-in
                        // (same technique as the FROZEN-CPU IF=0 fallback,
                        // but at STALL TIME — mid-episode; the one-shot
                        // IPI dump kept arriving after rolling monopolies
                        // had already drained). Names the leaking loop.
                        let sp = tk.thread().sp;
                        if (sp >> 48) == 0xffff && (sp & 7) == 0 && sp > 0x1000 {
                            let base = sp as *const u64;
                            let mut shown = 0usize;
                            // SAFETY: diagnostic read within the current
                            // task's own kernel stack pages (mapped while
                            // the Task exists); races cost at worst a
                            // torn-but-bounded word.
                            for k in 0..96 {
                                let v = unsafe {
                                    core::ptr::read_volatile(base.wrapping_sub(k))
                                };
                                #[cfg(feature = "x86_64")]
                                let (lo, hi) = (0xffff_ffff_8020_0000u64, 0xffff_ffff_8200_0000u64);
                                #[cfg(not(feature = "x86_64"))]
                                let (lo, hi) = (0xffff_ffff_8000_0000u64, 0xffff_ffff_8020_0000u64);
                                if v >= lo && v < hi && (v & 1) == 0 {
                                    let _ = write!(victim, " {v:#x}");
                                    shown += 1;
                                    if shown >= 10 {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                crate::pr_warn!(
                    "RCU-GP-BOUND site={} waited>{}ms (sleepy): no full quiescent round gen={} per-cpu(cpu:gen/on/idle,...)={}:{}",
                    site,
                    max_ms,
                    gen,
                    qs_desc,
                    victim,
                );
            }
            // Comprehensive-fix residual hunt (2026-11-04, cgf1): a CPU
            // frozen non-quiescent across MANY consecutive grace budgets
            // is the wedge-family monopolist (IF=0/preempt-off holder —
            // its own tick cannot fire, so every tick-side instrument is
            // blind to it; the 13th/25th cross-CPU-dump prescription).
            // This waiter runs on ANOTHER CPU in task (syscall) context,
            // so it can walk the frozen CPU's tasks.
            //
            // 52nd session (oh-crash): the trigger bar moved 32 -> 4
            // consecutive failed rounds. The ohc10-19 frozen face took
            // 32 rounds to arm because grace rounds kept passing BETWEEN
            // rolling monopolies; ohc20 proved the pandemic forms with
            // NO terminal freeze at all — rolling preempt/softirq-leak
            // carriers (PREEMPT-LEAK pc=0x10001/0x100, LEAK-ORIGIN
            // slab.rs/unix.rs sites) starve the box while each monopoly
            // lasts only seconds. Four failed rounds (~200ms of a stuck
            // CPU at the 50ms fdreap budget) catches a carrier
            // mid-monopoly; a healthy boot never fails even one sleepy
            // round. One-shot per boot (GP_DUMPED), same latch as the
            // full dump below.
            let consec = GP_CONSEC_FAIL.fetch_add(1, Ordering::Relaxed) + 1;
            // rwlock-livelock: consec==2 added — the ub2404 boot showed
            // rolling monopolies where successes reset the counter
            // before 4 accumulated, leaving the monopolist unnameable.
            if (consec == 2 || consec == 4 || consec == 32)
                && !GP_DUMPED.swap(true, Ordering::AcqRel)
            {
                // FIRST name the stuck CPU's live producer — per-CPU
                // tick counts + last tick-entry RIP, then an IPI probe
                // whose callback answers with the percpu canary (gs cpu
                // id + magic vs LAPIC id) and the interrupted RIP + live
                // stack of whatever runs there (or stays silent =
                // IRQ-off monopolist) — BEFORE the full task dump
                // floods the log.
                let mut stuck = usize::MAX;
                for i in 0..MAX_CPUS {
                    if crate::sched::hotpath::cpu_online(i) && !cpu_quiescent(i, gen) {
                        stuck = i;
                        break;
                    }
                }
                if stuck != usize::MAX {
                    crate::debug::hotpath::dump_frozen_cpu_truth(stuck);
                }
                crate::debug::hotpath::dump_all_tasks("rcu-gp-frozen-cpu");
            }
            return false;
        }

        let current = match crate::sched::hotpath::current() {
            Some(t) => t,
            None => {
                // Early-boot / no-task context: no sleep possible —
                // plain yield rounds keep the wait alive.
                crate::sched::hotpath::schedule();
                continue;
            }
        };
        // SAFETY: current is this task's own pointer.
        let my_pid = unsafe { (*current).pid() };
        // r16 (mechanism B — the GB lost-wake-mid-exit, re-landed): the
        // one-shot wake below is keyed by PID, and a group member's
        // do_exit used to free its pid BEFORE its opportunistic sweep, so
        // this sleepy wait ran with a pid already returned to the
        // allocator. The expiry then resolved the pid to nothing
        // (WAKE-PID-MISS — silently dropped) or to a REUSED task — either
        // way this waiter parked at the schedule() below and the deadline
        // line under it never ran, because the bound is only re-checked
        // on wake: the "bounded" wait became permanent (GB worker
        // 312/313, both wedged mid-exit; the drain's rescue wake,
        // addressed by task pointer, is what finally moved them). The
        // exit path now frees the pid only AFTER the sweep, but keep the
        // independent guard here: validate the pid->task mapping before
        // arming, and when it is broken (any future pid-then-sleep
        // order), fall back to a yield-poll that enforces its own
        // deadline and never depends on a wake token. Rate-limited
        // report: every firing names a producer of the order.
        let pid_resolves = {
            let t = crate::process::hotpath::pid_hash_lookup_pinned(my_pid);
            if t.is_null() {
                false
            } else {
                let ok = t as *const crate::process::types::Task
                    == current as *const crate::process::types::Task;
                // Release the lookup pin in both cases (unmatched or
                // matched): the pinned refcount is the lookup's own.
                crate::process::types::Task::task_put(t);
                ok
            }
        };
        if !pid_resolves {
            static PIDFREE_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if PIDFREE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 16 {
                crate::pr_warn!(
                    "RCU-SLEEPY-PID-FREE pid={} site={} — yield-poll round (pid-keyed timer wake would be lost)",
                    my_pid,
                    site
                );
            }
            // Cooperative yield round: state stays RUNNING (no park, no
            // wake token), the schedule() itself reports this CPU's QS
            // and lets peers run; the loop top re-checks completion and
            // the deadline on every return.
            crate::arch::cpu::restore_irq(true);
            crate::sched::hotpath::schedule();
            continue;
        }
        let target = crate::timer::get_jiffies() + 1;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);
        if timer_id != 0 {
            // SAFETY: current is this task's own pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
            }
            // Re-check QS AFTER marking sleeping: the last missing
            // reporter typically reports in the microseconds between
            // the failed check at the loop top and here (the fd-close
            // completion loop yields through schedule(), which is a
            // report site). Aborting the sleep when it lands converts
            // what was otherwise a full 1-jiffy stall — measured
            // 2026-11-05: one close(2) per 10 ms tick stalled up to
            // 10 ms in fdreapd's sleepy wait (probe: open+close median
            // 1.07 us but mean 2.45 us, every stall ending at a jiffy
            // boundary) — back into the microsecond poll window.
            if all_online_cpus_reported(gen) {
                // SAFETY: current is this task's own pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    // NEW-C2: a racing wake may have enqueued us while
                    // still executing — take ourselves back off.
                    crate::sched::hotpath::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
                continue;
            }
            // Re-check the one-shot AFTER marking sleeping: it may have
            // fired between arming and set_state (khungtaskd lesson).
            if crate::timer::get_jiffies() >= target {
                // SAFETY: current is this task's own pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    // NEW-C2: a racing wake may have enqueued us while
                    // still executing — take ourselves back off.
                    crate::sched::hotpath::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
            } else {
                crate::arch::cpu::restore_irq(true);
                crate::sched::hotpath::schedule();
                // Spurious wake before the deadline: drop the timer and
                // loop (re-armed next iteration).
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            // Timer table full — stay runnable, yield one round.
            crate::sched::hotpath::schedule();
        }
    }
}

/// Whether every online CPU has reported a quiescent state at or after
/// generation `gen`.
/// Public twin for the async reap drainer: poll (never sleep) whether
/// the shared-grace round at `target` has completed on every online
/// CPU. Same predicate `wait_qs_sleepy` loops on.
pub fn all_online_cpus_reported_pub(gen: u32) -> bool {
    all_online_cpus_reported(gen)
}

fn all_online_cpus_reported(gen: u32) -> bool {
    for i in 0..MAX_CPUS {
        if !crate::sched::hotpath::cpu_online(i) {
            continue;
        }
        if !cpu_quiescent(i, gen) {
            return false;
        }
    }
    true
}

/// Whether CPU `i` is quiescent with respect to generation `gen`:
/// either it stored a QS report at/after `gen`, or it is currently
/// executing its idle task. A halted idle CPU stores reports only once
/// per wake, so its `RCU_QS_GEN` entry can lag arbitrarily far behind
/// reality while the CPU is in fact executing nothing — the fd-table
/// reader window (get_file's load→increment) runs only over a non-idle
/// `current`, so idle-loop execution is quiescent by construction.
/// This keeps grace periods from stalling on parked secondary CPUs
/// (r26: without it, every synchronous close-completion wait paid the
/// sleepy poll's 1-jiffy granularity).
#[inline]
fn cpu_quiescent(i: usize, gen: u32) -> bool {
    RCU_QS_GEN[i].load(Ordering::Acquire) >= gen
        || crate::sched::hotpath::cpu_running_idle(i)
}

/// Shared body of both bounded waits: announce a waiter, report our own
/// QS, then wait (bounded, resched between checks) for every online
/// CPU's QS generation to reach `gen`.
fn announce_and_wait(gen: u32, max_ms: u64, site: &'static str) -> bool {
    RCU_WAITERS.fetch_add(1, Ordering::AcqRel);
    rcu_note_context_switch();

    let hz = crate::drivers::hotpath::timer::HZ as u64;
    let deadline = crate::timer::get_jiffies() + max_ms * hz / 1000;

    loop {
        let mut all_qs = true;
        let mut stuck_cpu = 0usize;
        for i in 0..MAX_CPUS {
            if !crate::sched::hotpath::cpu_online(i) {
                continue;
            }
            if !cpu_quiescent(i, gen) {
                all_qs = false;
                stuck_cpu = i;
                break;
            }
        }
        if all_qs {
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            return true;
        }
        if crate::timer::get_jiffies() >= deadline {
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            static GPB_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = GPB_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 4 {
                crate::pr_warn!(
                    "RCU-GP-BOUND site={} waited>{}ms: cpu{} reports no quiescent state (monopolist window)",
                    site,
                    max_ms,
                    stuck_cpu
                );
            }
            return false;
        }
        crate::sched::hotpath::schedule();
    }
}

// ============================================================================
// Softirq handler
// ============================================================================

/// RCU softirq handler — drains the local CPU callback list.
///
/// Registered at `SoftirqIndex::Rcu` (vector 9).
pub fn rcu_softirq_handler(_nr: usize) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    // Detach the entire callback list under lock.
    let mut batch = ListHead::new();
    {
        lock_cb_list(cpu);
        // SAFETY: We hold cb_list lock for this CPU, so RCU_CBS[cpu] is only
        // accessed by us.  The list pointers were initialised in init().
        unsafe {
            if !RCU_CBS[cpu].is_empty() {
                // Splice the list into `batch`.
                batch.next = RCU_CBS[cpu].next;
                batch.prev = RCU_CBS[cpu].prev;
                (*batch.next).prev = &mut batch;
                (*batch.prev).next = &mut batch;
                RCU_CBS[cpu].init();
            }
        }
        unlock_cb_list(cpu);
    }

    // Invoke all callbacks.
    // SAFETY: We detached the entire callback list above and now own it
    // exclusively.  Each RcuHead was registered via call_rcu with a valid
    // func pointer and remains valid until the callback executes.
    unsafe {
        let sentinel = &mut batch as *mut ListHead;
        let mut node = batch.next;
        while node != sentinel {
            let next = (*node).next;
            let head = node as *mut RcuHead;
            let func = (*head).func;
            func(head as *mut core::ffi::c_void);
            node = next;
        }
    }

    // Advance global generation so `synchronize_rcu()` callers can make progress.
    RCU_GEN.fetch_add(1, Ordering::AcqRel);
}

// ============================================================================
// Initialization
// ============================================================================

/// Initialize the RCU subsystem (called once during boot).
pub fn init() {
    for i in 0..MAX_CPUS {
        // SAFETY: Called once during boot before any CPU uses RCU — no
        // concurrent access to RCU_CBS[i].
        unsafe { RCU_CBS[i].init(); }
        RCU_QS_GEN[i].store(0, Ordering::Relaxed);
    }

    crate::interrupt::softirq::open_softirq(
        crate::interrupt::softirq::SoftirqIndex::Rcu as usize,
        rcu_softirq_handler,
    );

}

// ============================================================================
// Internal helpers
// ============================================================================

#[inline]
fn lock_cb_list(cpu: usize) {
    while RCU_CBS_LOCK[cpu]
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

#[inline]
fn unlock_cb_list(cpu: usize) {
    RCU_CBS_LOCK[cpu].store(false, Ordering::Release);
}

// ═══════════════ percpu ═════════════════════════════════════════
// (was percpu.rs)
// Typed per-CPU storage (rust-design-headroom §3, divergence D-7).
//
// The mainstream approach this replaces is a `static mut` array
// indexed by a raw cpu id, whose safety rests on a REVIEW-enFORCED
// convention: "callers must have IF=0". The paid incident family is
// the small_alloc magazine divergences — r8final (runs/gnome-r8final,
// t=515 s, two-CPU row entry via a cpu_id()/ti_cpu divergence) and
// the 2026-10-26 transient two-CPU entry fixed on main (7a6e1bf4) —
// both were exactly this convention failing silently.
//
// This module encodes the convention in the TYPE instead:
//
// - A per-CPU slot ([`CpuLocal`]) is a plain static whose rows are
//   unreachable except through [`CpuLocal::with`].
// - `with` disables interrupts, reads the cpu id, and hands the
//   closure a [`CpuLocalRef`] guard. The guard is the ONLY way to
//   touch the row; it is `!Send` (cannot migrate to another cpu) and
//   its lifetime cannot escape the closure, so the interrupt-disabled
//   window is exactly the closure body — the guard's `Drop` restores
//   the saved IRQ state.
// - Rows are cache-line aligned and padded: two CPUs magazine-ing
//   adjacent rows never share a line.
// - Two tripwires survive as data, not review notes: an out-of-range
//   cpu id makes `with` return `None` (the caller falls to its locked
//   body — the r8final survive-and-taint stance), and a same-slot
//   re-entry on the same cpu is refused (counted + tainted; the
//   outer guard's window is still intact because nesting
//   save/restore is flag-correct).
//
// The module is deliberately small: `T: Copy` for the const
// initializer (per-row constructors can come later), and the GRQ
// typed lock key named in §3 is the follow-on slice, not this one.

// (INV-PERCPU-1: the per-CPU bank family — banks are addressed by
// cpu id through the fixed static array (bounds by construction on
// our SMP sizes); preemption-off windows own the per-bank access.)

/// Divergences refused by the re-entry tripwire (same slot, same cpu,
/// while a guard is live). Zero on a healthy boot; every nonzero value
/// names a call site that must be rewritten — the alias it attempted
/// would have been the r8final class.
static REENTRY_EVENTS: AtomicUsize = AtomicUsize::new(0);

/// Number of re-entry refusals since boot (diagnostic hook for the
/// census/debug lines).
pub fn reentry_events() -> usize {
    REENTRY_EVENTS.load(Ordering::Relaxed)
}

// Row participates in const array-repeat initialization (`[Row { .. };
// N]` needs Copy); the manual impl keeps the bound on T explicit.
impl<T: Copy> Clone for Row<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Copy> Copy for Row<T> {}

// SAFETY: see the struct comment — rows are cpu-private by
// construction of `with`.
unsafe impl<T, const N: usize> Sync for CpuLocal<T, N> {}

impl<T: Copy, const N: usize> CpuLocal<T, N> {
    /// Const constructor: every cpu's row starts as a copy of `val`.
    pub const fn new(val: T) -> Self {
        CpuLocal {
            rows: UnsafeCell::new([Row {
                val,
                in_use: false,
            }; N]),
        }
    }
}

impl<T, const N: usize> CpuLocal<T, N> {
    /// Run `f` with this cpu's row, interrupts disabled for exactly
    /// the duration of `f`. Returns `None` — with interrupts restored
    /// — when the acting cpu id is out of range for this bank (the
    /// r8final divergence family; callers keep their locked fallback)
    /// or on a refused same-slot re-entry.
    #[inline]
    pub fn with<R>(&self, f: impl FnOnce(CpuLocalRef<'_, T>) -> R) -> Option<R> {
        let flags = crate::arch::cpu::save_and_disable_irq();
        let cpu = crate::arch::cpu_id() as usize;
        if cpu >= N {
            crate::arch::cpu::restore_irq(flags);
            return None;
        }
        let rows = self.rows.get(); // *mut [Row<T>; N], interior-mutable
        // SAFETY (pointer arithmetic and both field pointers): `cpu`
        // is bounds-checked above; the row is private to this cpu for
        // as long as interrupts stay disabled (they are — the guard
        // restores at drop); the in_use flag read/set below refuses a
        // second live guard, so the &mut handed out is exclusive. All
        // access goes through raw pointers derived from the UnsafeCell
        // — never through a `&`-path (the D-7 incident family).
        let row = unsafe { rows.cast::<Row<T>>().add(cpu) }; // *mut Row<T>
        let (val, in_use): (*mut T, *mut bool) = unsafe {
            (
                core::ptr::addr_of_mut!((*row).val),
                core::ptr::addr_of_mut!((*row).in_use),
            )
        };
        if unsafe { *in_use } {
            // Same slot, same cpu, a guard is already live (the outer
            // window still holds — flags were saved with IRQs already
            // off and false restores nothing). Refuse the alias,
            // count + taint: this is a call-site bug of the r8final
            // class, and the taint names it instead of absorbing it.
            REENTRY_EVENTS.fetch_add(1, Ordering::Relaxed);
            crate::debug::hotpath::add_taint(crate::debug::types::TaintFlags::BAD_PAGE);
            crate::arch::cpu::restore_irq(flags);
            return None;
        }
        unsafe { *in_use = true };
        let guard = CpuLocalRef {
            val: unsafe { &mut *val },
            cpu,
            flags,
            preempt_guard: false,
            in_use,
            _nosend: PhantomData,
        };
        Some(f(guard))
    }

    /// Run `f` with this cpu's row, PREEMPTION disabled but INTERRUPTS
    /// LEFT ENABLED, for exactly the duration of `f` (106th, the
    /// allocator irq-toggle restructure).
    ///
    /// The exclusivity this guard provides is strictly weaker than
    /// [`CpuLocal::with`]: it excludes a context switch (so the cpu id
    /// read inside cannot go stale — no migration between the read and
    /// the row access) but it does NOT exclude interrupt context on the
    /// same cpu. It is therefore only correct for banks whose IRQ-context
    /// access is ROUTED AWAY from the rows by the caller — the small_alloc
    /// magazines: interrupt-context allocs/frees take the class-lock path
    /// directly and never touch a row, so a task-context row access can be
    /// interrupted but never raced. `with_preempt` returns `None` (nothing
    /// touched) whenever the caller is NOT in task context
    /// (`in_interrupt()` — hardirq/softirq/NMI) or on the same
    /// out-of-range/re-entry refusals as `with`; callers keep their locked
    /// fallback for every `None`.
    ///
    /// Why this exists: `with` pays a cli/sti pair per access. On the
    /// allocation fast paths that is ~46M IF=0 windows per boot, the
    /// second-largest surviving interrupt-off shadow (the deferred-PMI
    /// `enable_irq+0x1d` sample class). A preempt-disable window keeps
    /// the tick and device IRQs flowing through the access.
    pub fn with_preempt<R>(&self, f: impl FnOnce(CpuLocalRef<'_, T>) -> R) -> Option<R> {
        if crate::interrupt::preempt::in_interrupt() {
            return None;
        }
        crate::interrupt::preempt::preempt_count_add(crate::interrupt::preempt::PREEMPT_OFFSET);
        let cpu = crate::arch::cpu_id() as usize;
        if cpu >= N {
            crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::PREEMPT_OFFSET);
            return None;
        }
        let rows = self.rows.get(); // *mut [Row<T>; N], interior-mutable
        // SAFETY: `cpu` is bounds-checked above; the row is private to
        // this cpu for as long as preemption stays disabled (no context
        // switch can migrate the task between the cpu_id read and the row
        // access — the r8final divergence family); interrupt context is
        // excluded by the caller-side routing documented on this method.
        // The in_use flag read/set refuses a second live guard.
        let row = unsafe { rows.cast::<Row<T>>().add(cpu) }; // *mut Row<T>
        let (val, in_use): (*mut T, *mut bool) = unsafe {
            (
                core::ptr::addr_of_mut!((*row).val),
                core::ptr::addr_of_mut!((*row).in_use),
            )
        };
        if unsafe { *in_use } {
            // Same refusal semantics as `with` (see there).
            REENTRY_EVENTS.fetch_add(1, Ordering::Relaxed);
            crate::debug::hotpath::add_taint(crate::debug::types::TaintFlags::BAD_PAGE);
            crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::PREEMPT_OFFSET);
            return None;
        }
        unsafe { *in_use = true };
        let guard = CpuLocalRef {
            val: unsafe { &mut *val },
            cpu,
            flags: false,
            preempt_guard: true,
            in_use,
            _nosend: PhantomData,
        };
        Some(f(guard))
    }
}

impl<T> CpuLocalRef<'_, T> {
    /// The cpu whose row this guard owns (diagnostics; the value is
    /// stable for the guard's lifetime — IF=0 or preempt-off forbid
    /// migration).
    pub fn cpu(&self) -> usize {
        self.cpu
    }
}

impl<T> Deref for CpuLocalRef<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.val
    }
}

impl<T> DerefMut for CpuLocalRef<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.val
    }
}

impl<T> Drop for CpuLocalRef<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: same-cpu exclusivity still holds until the restore
        // below; the raw store is the only writer.
        unsafe { *self.in_use = false };
        if self.preempt_guard {
            crate::interrupt::preempt::preempt_count_sub(
                crate::interrupt::preempt::PREEMPT_OFFSET,
            );
        } else {
            crate::arch::cpu::restore_irq(self.flags);
        }
    }
}

// ═══════════════ lockkey ═════════════════════════════════════════
// (was lockkey.rs)
// Typed lock keys — the lock-order hierarchy as types
// (docs/architecture/rust-design-headroom.md §3, second slice; the
// companion of divergence D-7 CpuLocal, which typed the per-CPU
// interrupt window).
//
// The bug class this kills: INV-LOCK-* ordering violations
// (docs/architecture/lock-ordering.md) are today review-enforced and
// watchdog-detected — the futex-wake path nests GRQ inside the bucket
// lock (INV-LOCK-5), and the reverse order deadlocks. Here the
// permitted nestings are TRAIT IMPLS: acquiring a keyed lock while
// another is held goes through [`KeyedRawSpinlock::lock_nested`],
// which only compiles when an `impl CanNestInside<Outer> for Inner`
// exists. The reverse or same-level acquisition is not a runtime
// spin-to-deadlock — it does not compile:
//
// ```ignore
// let bucket = KeyedRawSpinlock::<FutexBucketKey>::new();
// let grq = KeyedRawSpinlock::<GrqKey>::new();
// let bg = bucket.lock();
// let gg = grq.lock_nested(&bg);        // OK: INV-LOCK-5 forward
// drop(gg);
// let gg2 = grq.lock();
// let bg2 = bucket.lock_nested(&gg2);   // DOES NOT COMPILE
//                                       // (no CanNestInside<GrqKey>
//                                       //  for FutexBucketKey)
// ```
//
// Scope discipline (the same opportunistic doctrine as D-6/D-4):
// unkeyed spinlocks keep compiling unchanged; a pair migrates to keys
// when its invariant is named in lock-ordering.md, and the migration
// must not widen the public API of the migrated subsystem. The first
// adoption is the GRQ's own lock (`sched::sched::GlobalRunQueue`),
// typed `GrqKey` internally with its guard API unchanged; the futex
// bucket side of INV-LOCK-5 (and the INV-LOCK-6 stats/net pair)
// follow in later slices, at which point the wake path's nesting
// becomes a compile-checked `lock_nested`.
//
// The IRQ-save/preempt discipline is deliberately NOT part of the key:
// that window is typed by CpuLocal's guard (D-7); the key types only
// the ORDER fact. One mechanism per concern.

/// A lock-order class. One zero-sized type per named invariant in
/// `docs/architecture/lock-ordering.md`; the class NAME is what the
/// deadlock watchdog prints alongside the type-level story.
pub trait NestKey: 'static + Copy {
    /// The invariant this key participates in (for diagnostics).
    const NAME: &'static str;
}

/// Permitted nesting: `impl CanNestInside<Outer> for Inner` states that
/// a lock of class `Inner` may be acquired while a lock of class
/// `Outer` is held. Absence of the impl is the invariant: the reverse
/// (or same-level) nesting cannot compile.
pub trait CanNestInside<Outer: NestKey>: NestKey {}

// ---- The key classes (lock-ordering.md, INV-LOCK-5/6) -------------

impl NestKey for GrqKey {
    const NAME: &'static str = "grq";
}
/// GRQ may nest inside the futex bucket lock (INV-LOCK-5 forward:
/// futex_wake -> Task::wake_up -> enqueue_task).
impl CanNestInside<FutexBucketKey> for GrqKey {}

impl NestKey for FutexBucketKey {
    const NAME: &'static str = "futex-bucket";
}

impl NestKey for NetStatsKey {
    const NAME: &'static str = "net-stats";
}
/// INV-LOCK-6 forward: stats acquired while a queue lock is held.
impl CanNestInside<NetQueueKey> for NetStatsKey {}

impl NestKey for NetQueueKey {
    const NAME: &'static str = "net-queue";
}

impl NestKey for WaiterSlotKey {
    const NAME: &'static str = "futex-waiter-slot";
}
/// A slot may be acquired while its bucket lock is held (the shape of
/// every chain walk under a bucket guard). Same-class double-slot
/// acquisition (the requeue relink window) stays on the plain API —
/// the keyed surface fences ACROSS classes.
impl CanNestInside<FutexBucketKey> for WaiterSlotKey {}

// ---- The keyed lock ----------------------------------------------

// SAFETY: RawSpinlock's atomic protocol is the synchronization; the
// key is zero-sized and carries no data.
unsafe impl<K: NestKey> Sync for KeyedRawSpinlock<K> {}

impl<K: NestKey> KeyedRawSpinlock<K> {
    pub const fn new() -> Self {
        Self { raw: RawSpinlock::new(), _key: PhantomData }
    }

    /// Top-level acquire (no keyed lock held). The plain call stays
    /// available because most call sites are top-level by construction;
    /// nesting while another keyed lock is held MUST use `lock_nested`.
    #[inline]
    #[track_caller]
    pub fn lock(&self) -> KeyedGuard<'_, K> {
        self.raw.lock();
        KeyedGuard {
            lock: self as *const Self as *mut Self,
            _key: PhantomData,
            _lifetime: PhantomData,
        }
    }

    /// Try-lock variant of the top-level acquire.
    #[inline]
    #[track_caller]
    pub fn try_lock(&self) -> Option<KeyedGuard<'_, K>> {
        if self.raw.try_lock() {
            Some(KeyedGuard {
                lock: self as *const Self as *mut Self,
                _key: PhantomData,
                _lifetime: PhantomData,
            })
        } else {
            None
        }
    }

    /// Claim without returning a guard — for subsystems that wrap the
    /// raw lock in their OWN irqsave/preempt guard protocol around the
    /// lock (the GRQ pattern: its GrqGuard carries a pointer back to
    /// the queue and unlocks through it). Pairs with [`unlock`].
    #[inline]
    #[track_caller]
    pub fn try_lock_claim(&self) -> bool {
        self.raw.try_lock()
    }

    /// Unconditional guard-less claim — the claim twin of
    /// [`try_lock_claim`], for a protocol wrapper's blocking acquire.
    /// The 2b092c49 conviction rule: a subsystem with its own guard
    /// type must claim HERE, never through the guard-returning
    /// [`lock`] (whose discarded return silently releases).
    #[inline]
    #[track_caller]
    pub fn lock_claim(&self) {
        self.raw.lock()
    }

    /// Unlock without a guard (for subsystems whose guard wraps the
    /// full irqsave/preempt protocol around the raw lock — the GRQ
    /// pattern). Delegates to the raw protocol; the key neither adds
    /// nor removes anything here.
    #[inline]
    #[track_caller]
    pub fn unlock(&self) {
        self.raw.unlock();
    }
}

impl<Inner: NestKey> KeyedRawSpinlock<Inner> {
    /// Compile-checked nested acquire: take this (`Inner`-class) lock
    /// while an `Outer`-class keyed lock is held. The proof is minted
    /// by the OUTER guard (`guard.proof()`); the call only compiles
    /// when `Inner: CanNestInside<Outer>` — the trait impl list IS the
    /// permitted-nesting table of lock-ordering.md. Violations (reverse
    /// order, same class) are compile errors, not deadlocks.
    #[inline]
    #[track_caller]
    pub fn lock_nested<'a, Outer: NestKey>(
        &self,
        _outer: &NestProof<'a, Outer>,
    ) -> KeyedGuard<'_, Inner>
    where
        Inner: CanNestInside<Outer>,
    {
        self.lock()
    }
}

// SAFETY: same rule as Spinlock itself — the atomic protocol
// synchronizes; the key is zero-sized and carries no data.
unsafe impl<K: NestKey, T: Send> Sync for KeyedSpinlock<K, T> {}

impl<K: NestKey, T> KeyedSpinlock<K, T> {
    pub const fn new(data: T) -> Self {
        Self { inner: super::types::Spinlock::new(data), _key: PhantomData }
    }

    /// Top-level acquire with the full irqsave protocol (the bucket
    /// locks' historical discipline — unchanged bytes at the call
    /// sites).
    #[inline]
    #[track_caller]
    pub fn lock_irqsave(&self) -> KeyedIrqGuard<'_, K, T> {
        KeyedIrqGuard { inner: self.inner.lock_irqsave(), _key: PhantomData }
    }
}

impl<Inner: NestKey, T> KeyedSpinlock<Inner, T> {
    /// Compile-checked nested acquire — the data-carrying twin of
    /// [`KeyedRawSpinlock::lock_nested`]: only compiles when
    /// `Inner: CanNestInside<Outer>`; same guard (Deref/DerefMut,
    /// proof-minting) as the top-level acquire.
    #[inline]
    #[track_caller]
    pub fn lock_nested<'a, Outer: NestKey>(
        &self,
        _proof: &NestProof<'a, Outer>,
    ) -> KeyedIrqGuard<'_, Inner, T>
    where
        Inner: CanNestInside<Outer>,
    {
        self.lock_irqsave()
    }
}

impl<K: NestKey, T: ?Sized> KeyedIrqGuard<'_, K, T> {
    /// Mint the held-section proof (see [`NestProof`]).
    #[inline]
    pub fn proof(&self) -> NestProof<'_, K> {
        NestProof { _key: PhantomData, _hold: PhantomData }
    }
}

impl<K: NestKey, T: ?Sized> core::ops::Deref for KeyedIrqGuard<'_, K, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.inner.deref()
    }
}

impl<K: NestKey, T: ?Sized> core::ops::DerefMut for KeyedIrqGuard<'_, K, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.inner.deref_mut()
    }
}

impl<K: NestKey> Drop for KeyedGuard<'_, K> {
    #[inline]
    fn drop(&mut self) {
        unsafe { (*self.lock).raw.unlock() };
    }
}

impl<K: NestKey> KeyedGuard<'_, K> {
    /// Mint the held-section proof (see [`NestProof`]).
    #[inline]
    pub fn proof(&self) -> NestProof<'_, K> {
        NestProof { _key: PhantomData, _hold: PhantomData }
    }
}

// The guard is the outer-lock proof token; keep it !Send like other
// spinlock guards (it is not `Send` automatically because of the raw
// pointer, and that is the correct default for a hold proof).
