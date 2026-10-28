//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Futex Implementation - Fast Userspace Mutex
//!
//! # Design
//! - Static waiter pool (spinlock-protected slots) for zero-allocation futex
//! - Static hash table mapping FutexKey → waiter chain (singly-linked list)
//! - All locks use `lock_irqsave()` for interrupt safety
//! - Wake uses `Task::wake_up()` (enqueue + resched) for correct scheduling
//! - Wait inserts into chain then sets INTERRUPTIBLE under lock to prevent lost wakeup
//!
//! # Futex key semantics (Linux parity)
//! - PRIVATE futexes key on (mm identity, uaddr): the second key component is
//!   the address of the task's `AddressSpace` Arc allocation. All CLONE_VM
//!   threads share one Arc → one key; a forked child COWs a NEW AddressSpace
//!   → different key (exactly Linux's `&mm->mm` key). Kernel threads (no mm)
//!   key on 0.
//! - SHARED futexes key on the virtual address only (see R31-9 note; a
//!   physical-frame key needs page pinning infrastructure).

use crate::sync::spinlock::Spinlock;
use core::sync::atomic::AtomicU32;
use crate::process::Task;
use crate::process::task::TaskState;
use crate::syscall::errno::{EINVAL, EFAULT, EAGAIN, ENOSYS, ETIMEDOUT};

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

/// Latch the hdf_devmgr pid for the FXDV tracer (idempotent; first caller
/// wins — binder.rs calls this when the SA 5100 registration is seen).
pub fn fxdv_latch_devmgr(pid: u32) {
    let prev = FXDV_PID.swap(pid, core::sync::atomic::Ordering::Relaxed);
    if prev != pid {
        crate::pr_warn!("FXDV latch devmgr pid={}", pid);
    }
}

// sc209 (leg 14): render_service futex lifecycle tracer. sc208 closed the
// SA-10 question to the publisher side: render_service main runs through
// composer/HGM init, logs "HgmTaskHandleThread init" (hgm_context.cpp:60),
// then parks in a futex forever — the very next source statement is
// InitHgmTaskHandleThread's PostSyncTask (an FFRT queue->wait(handle)),
// and AddSystemAbility(10) never runs (no "insert SA:10" all boot). The
// census sleepsnap shows its FFRT workers (OS_FFRT_5_0/_1) alive but
// asleep. This tracer latches the render_service tgid at exec and prints
// EVERY futex WAIT/WAKE/REQUEUE from that process (entry + return): a
// WAKE issued to the parked main's uaddr that it never consumes = kernel
// wake loss; silence = the FFRT worker never ran the task (userspace
// chain — the next hop is the worker's own wait lines).
static FXRS_TGID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static FXRS_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
// sc209 postmortem: 600 ops died inside RS's own init (EGL/composer
// setup is hundreds of condvar broadcasts), two dozen ops short of the
// HGM window. 4000 ops (~8000 short lines, ~1 min of UART across a
// 45 min boot) covers init + the PostSyncTask park window + any later
// render-loop ops of interest.
const FXRS_BUDGET: usize = 4000;

/// Latch the render_service tgid for the FXRS tracer (idempotent; called
/// from the exec path when /system/bin/render_service loads).
pub fn fxrs_latch(tgid: u32) {
    let prev = FXRS_TGID.swap(tgid, core::sync::atomic::Ordering::Relaxed);
    if prev != tgid {
        crate::pr_warn!("FXRS latch render_service tgid={}", tgid);
    }
}

// sc185 RWFRZ-WATCH: words named by RWFRZ-PARK (writers parked on
// 0x80000000|N readers), watched for their post-park futex lifecycle from
// ANY process. A later WAKE on the word that the parked writer never
// consumes = kernel wake loss; silence forever = readers never released
// (userspace). See the watch block in do_futex and the registration in the
// RWFRZ-PARK hook.
const RWFRZ_WATCH_SZ: usize = 8;
static RWFRZ_WATCH: [core::sync::atomic::AtomicUsize; RWFRZ_WATCH_SZ] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; RWFRZ_WATCH_SZ];
static RWFRZ_WATCH_CNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
// sc186: 64 lines died on arena churn before the wedge window; 192 keeps
// the same worst-case UART bound as one parkdump stack dump.
const RWFRZ_WATCH_BUDGET: usize = 192;

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
}

impl FutexKey {
    pub fn new(uaddr: usize, mm: usize, flags: u32) -> Self {
        Self { uaddr, mm, flags }
    }

    /// Check if two keys match
    pub fn matches(&self, other: &FutexKey) -> bool {
        if !(self.flags & FLAGS_SHARED != 0) {
            self.uaddr == other.uaddr && self.mm == other.mm
        } else {
            self.uaddr == other.uaddr
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
    // SAFETY: caller guarantees a valid Task; address_space_arc() clones the
    // Arc (bumping the refcount) and we drop it after taking the pointer.
    unsafe {
        match (*task).address_space_arc() {
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
    match crate::sched::current() {
        // SAFETY: sched::current() returns the current task's raw pointer,
        // valid for the duration of this syscall.
        Some(t) => task_futex_mm_id(t),
        None => 0,
    }
}

/// DFX futex protocol tracer (feature `dfx-futex-trace`).
#[cfg(feature = "dfx-futex-trace")]
fn ftx_trace(tag: &[u8], uaddr: usize, a: u64, b: u64) {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    let pid = crate::sched::current().map(|t| unsafe { (*t).pid() as u64 }).unwrap_or(0);
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

#[cfg(feature = "dfx-futex-trace")]
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

#[cfg(not(feature = "dfx-futex-trace"))]
fn ftx_trace(_tag: &[u8], _uaddr: usize, _a: u64, _b: u64) {}

/// Waiter information
struct Waiter {
    /// Futex key
    key: FutexKey,
    /// Waiting task
    task: *mut Task,
    /// bitset
    bitset: u32,
    /// Whether already woken
    woken: bool,
    /// Hash bucket this waiter is currently linked in. Tracked per-waiter
    /// because FUTEX_REQUEUE can move it to uaddr2's bucket while it sleeps;
    /// a stale bucket at removal time silently leaked the slot (review 6.1).
    bucket: usize,
    /// Next waiter in hash chain
    next: Option<usize>,
}

// SAFETY: Waiter is only ever accessed while the per-slot Spinlock is held,
// serialising all reads and writes.  The `task` raw pointer is only
// dereferenced by the waker after validating it is non-null.
unsafe impl Send for Waiter {}
unsafe impl Sync for Waiter {}

/// Waiter pool size - from config
const WAITER_POOL_SIZE: usize = crate::config::FUTEX_WAITER_POOL_SIZE;

/// Waiter pool
static WAITER_POOL: [Spinlock<Option<Waiter>>; WAITER_POOL_SIZE] = {
    const INIT: Spinlock<Option<Waiter>> = Spinlock::new(None);
    [INIT; WAITER_POOL_SIZE]
};

/// Hash bucket count - from config
const HASH_SIZE: usize = crate::config::FUTEX_HASH_SIZE;

/// Waiter list head for each bucket. D-11 keyed (FutexBucketKey): the
/// guard's `proof()` is what the nested GRQ acquire in the wake path
/// requires — the bucket->GRQ nesting of INV-LOCK-5 (R12-2) is a
/// compile fact. NOTE: FUTEX_REQUEUE holds TWO buckets (lo/hi, index
/// ordered) via plain lock_irqsave — a same-class ordered double
/// acquire, deliberately outside the keyed-nesting API (which exists
/// to fence ACROSS classes).
static HASH_HEADS: [crate::sync::lockkey::KeyedSpinlock<
    crate::sync::lockkey::FutexBucketKey,
    Option<usize>,
>; HASH_SIZE] = {
    const INIT: crate::sync::lockkey::KeyedSpinlock<
        crate::sync::lockkey::FutexBucketKey,
        Option<usize>,
    > = crate::sync::lockkey::KeyedSpinlock::new(None);
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
        let mut idx = *h;
        while let Some(i) = idx {
            // SAFETY: slot indices come from chain links under the bucket
            // lock; slots are freed only under both locks, so the entry we
            // copy out stays live for the duration of the read.
            let (entry, next) = {
                let slot = WAITER_POOL[i].lock_irqsave();
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

/// Diagnostic (sc75 launcher IPC death loop): every live futex waiter that
/// belongs to `tgid` — the intra-process contention graph. Returns
/// (task_pid, uaddr, woken) triples; the binder stall dump prints them so a
/// handler blocked on a per-call condvar/mutex names the whole wait set and
/// each one's woken state. Entries whose task is no longer in the pid hash
/// are skipped (exit-race guard for the raw task deref).
pub fn waiters_of_tgid(tgid: u32) -> alloc::vec::Vec<(u32, usize, bool)> {
    let mut out = alloc::vec::Vec::new();
    for b in HASH_HEADS.iter() {
        let h = b.lock_irqsave();
        let mut idx = *h;
        while let Some(i) = idx {
            // SAFETY: slot indices come from chain links under the bucket
            // lock; the tuple is copied out under the slot lock.
            let (entry, next) = {
                let slot = WAITER_POOL[i].lock_irqsave();
                match *slot {
                    Some(ref w) if !w.task.is_null() => {
                        // SAFETY: liveness verified via the pid hash before
                        // the tgid read; the derefs touch only pid fields.
                        let t = unsafe { (*w.task).pid() };
                        let live = core::ptr::eq(
                            // SAFETY: diagnostic pid-hash lookup.
                            unsafe { crate::sched::find_task_by_pid(t) },
                            w.task,
                        );
                        let hit = live && (unsafe { (*w.task).tgid() as u32 } == tgid);
                        (Some((hit, t, w.key.uaddr, w.woken)), w.next)
                    }
                    Some(ref w2) => (None, w2.next),
                    None => (None, None),
                }
            };
            match entry {
                Some((true, pid, uaddr, woken)) => out.push((pid, uaddr, woken)),
                Some((false, ..)) => {}
                None => break,
            }
            idx = next;
        }
        drop(h);
    }
    out
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
                bucket: 0,
                next: None,
            });
            return Some(i);
        }
    }
    None
}

/// Free a waiter slot
fn free_waiter(index: usize) {
    let mut slot = WAITER_POOL[index].lock_irqsave();
    *slot = None;
}

/// Back off one jiffy when the waiter pool is exhausted (see
/// futex_wait_timeout). Sleeps INTERRUPTIBLE with a one-shot timer so a
/// slot frees while we are parked; returns immediately if no task/timer
/// context is available (caller retries anyway).
fn pool_full_backoff() {
    use crate::process::task::{Task, TaskState};

    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => return,
    };
    let target = crate::drivers::timer::get_jiffies() + 1;
    let pid = unsafe { (*current).pid() };
    let timer_id = crate::timer::add_timer_wakeup(target, pid);
    if timer_id == 0 {
        return; // no timer slot: busy retry (rare, bounded by wakers)
    }
    loop {
        if crate::drivers::timer::get_jiffies() >= target {
            break;
        }
        // Mark INTERRUPTIBLE BEFORE the final re-check (state-first
        // lost-wakeup discipline, same as sys_nanosleep).
        // SAFETY: current is the running task's pointer.
        unsafe {
            (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }
        if crate::drivers::timer::get_jiffies() >= target {
            // SAFETY: current is the running task's pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::RUNNING));
            }
            // A racing wake may have enqueued us while still executing —
            // take ourselves back off (NEW-C2 discipline).
            // SAFETY: current is the running task's pointer.
            unsafe {
                crate::sched::dequeue_task(&*current);
            }
            break;
        }
        // SAFETY: schedule() operates on the current task.
        unsafe {
            crate::sched::schedule();
        }
    }
    crate::timer::del_timer(timer_id);
}

/// Calculate futex hash value
fn futex_hash(key: &FutexKey) -> usize {
    // Hash by uaddr ALONE. matches() compares uaddr in BOTH branches
    // (private: uaddr+mm; shared: uaddr-only), so every waiter a wake
    // could possibly match shares the waiter's uaddr — a uaddr-keyed
    // bucket guarantees they are all in the scanned chain, and matches()
    // does the precise filtering.
    //
    // R31-9 fixed only the shared↔shared direction (mm excluded for
    // shared keys); the 4-thread hang hunt caught the remaining
    // shared-waiter ↔ private-waker split: musl's __thread_list_lock
    // waiters park with priv=0 (shared key, bucket uaddr%H), while the
    // kernel's CLONE_CHILD_CLEARTID exit wake (musl pthread_exit relies
    // on it to "unlock" the list lock — it passes &__thread_list_lock as
    // the clone ctid) hashed the private key (uaddr+mm)%H → a DIFFERENT
    // bucket → the wake scanned an empty chain (woken=0 with three
    // parked waiters) → every later exiter/joiner parked on the
    // never-released lock word forever. Linux solves the same interop
    // with FLAG_IMMUTABLE (the cleartid wake matches both key variants);
    // uaddr-only bucketing gives this kernel the same reach.
    //
    // F12: the raw `uaddr % HASH_SIZE` clustered EVERY thread-stack
    // futex into ONE bucket. musl hands out 1 MB pthread stacks with a
    // fixed stride (0x101000 — stack + guard page), which is a multiple
    // of 64, so the ctid/join futex words of all threads of a process
    // landed in the same bucket: pth_str02's 1000-thread join chain
    // serialised on a single spinlock (all 256 pool slots chained in one
    // bucket, every wake/alloc walking 256 entries under that lock).
    // Fibonacci-multiply the byte offset so any fixed stride spreads
    // across the buckets.
    let x = (key.uaddr >> 2) as u64;
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

    let key = FutexKey::new(uaddr, mm, flags);
    let bucket_idx = futex_hash(&key);
    #[cfg(feature = "dfx-futex-trace")]
    ftx_trace(b"WAKE-IN", uaddr, mm as u64, nr_wake as u64);

    let mut ret = 0i64;
    let mut prev_idx: Option<usize> = None;
    // Collect tasks to wake after releasing the bucket lock, like
    // kernel/futex/waitwake.c wake_futex() + wake_q_add(). Dynamically
    // sized: a fixed 8-entry array silently stranded every waiter past the
    // 8th (pthread_cond_broadcast) — review IPC-C3.
    let mut wake_list: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();

    // Hold the hash bucket lock for the entire traversal so no
    // concurrent futex_wait can insert/remove while we walk.
    let mut head = HASH_HEADS[bucket_idx].lock_irqsave();

    let mut current_idx = *head;
    while let Some(idx) = current_idx {
        if ret >= nr_wake as i64 {
            break;
        }

        let mut waiter_slot = WAITER_POOL[idx].lock_irqsave();
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

            // Mark woken so futex_wait knows it was explicitly woken.
            if let Some(ref mut w) = *waiter_slot {
                w.woken = true;
            }
            drop(waiter_slot);

            // Unlink from chain.
            if prev_idx.is_none() {
                *head = next_idx;
            } else if let Some(prev) = prev_idx {
                let mut prev_slot = WAITER_POOL[prev].lock_irqsave();
                if let Some(ref mut pw) = *prev_slot {
                    pw.next = next_idx;
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
        let woken_n = wake_list.len();
    for task in wake_list {
        if !task.is_null() {
            // R12-2 wake-under-bucket, now compile-checked: the proof
            // minted by the bucket guard is what the nested GRQ acquire
            // requires (INV-LOCK-5 forward; D-11).
            Task::wake_up_in_futex_bucket(task, &head.proof());
        }
    }
    drop(head);

    #[cfg(feature = "dfx-futex-trace")]
    ftx_trace(b"WAKE-OUT", uaddr, ret as u64, woken_n as u64);

    // b133 hunt (dfx=syscalltrace): OH's samgr/SA threads park forever in
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
        && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::FutexTrace)
    {
        static MISS_REPORTS: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let mut waker_pid = 0u32;
        if let Some(t) = crate::sched::current() {
            // SAFETY: sched::current() yields the current task for the
            // duration of this syscall.
            waker_pid = unsafe { (*t).pid() };
        }
        for (bi, b) in HASH_HEADS.iter().enumerate() {
            let mut h = b.lock_irqsave();
            let mut idx = *h;
            while let Some(i) = idx {
                // SAFETY: the slot index came from a chain link under the
                // bucket lock; slots are freed only under both locks.
                let (task, next, mismatch) = {
                    let slot = WAITER_POOL[i].lock_irqsave();
                    match *slot {
                        Some(ref w) => {
                            let mismatch = w.key.uaddr == uaddr
                                && (!w.key.matches(&key) || (w.bitset & bitset) == 0);
                            (w.task, w.next, mismatch)
                        }
                        None => (core::ptr::null_mut(), None, false),
                    }
                };
                if mismatch {
                    let n = MISS_REPORTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if n < 32 {
                        let mut wcomm = [0u8; 16];
                        if !task.is_null() {
                            // SAFETY: the task is linked in the bucket; it
                            // cannot be freed while we hold the lock.
                            let c = unsafe { (*task).comm() };
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
                        crate::pr_warn!(
                            "futex-wake-miss: uaddr={:#x} waker pid {} key(mm={:#x},shared={}) found waiter pid-key mm={:#x} bucket {} waiter-comm {}",
                            uaddr,
                            waker_pid,
                            mm,
                            flags & FLAGS_SHARED != 0,
                            wmm,
                            bi,
                            name
                        );
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

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -EFAULT as i64,
    };
    // SAFETY: current is the current task's raw pointer from sched::current(),
    // valid for the duration of this syscall.
    let mm = task_futex_mm_id(current);

    let key = FutexKey::new(uaddr, mm, flags);
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
        let uval = match unsafe {
            crate::arch::uaccess::get_user(uaddr_ptr as *const u32)
        } {
            Some(v) => v,
            None => return -EFAULT as i64,
        };
        #[cfg(feature = "dfx-futex-trace")]
        ftx_trace(b"WAIT-VAL", uaddr, uval as u64, val as u64);
        if uval != val {
            return -EAGAIN as i64;
        }

        // Deadline already passed while we were re-checking?
        if let Some(dl) = deadline {
            if crate::drivers::timer::get_jiffies() >= dl {
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
        // reserved by alloc_waiter in place).
        {
            let mut slot = WAITER_POOL[waiter_idx].lock_irqsave();
            if let Some(ref mut w) = *slot {
                w.key = key;
                w.task = current;
                w.bitset = bitset;
                w.woken = false;
                w.bucket = bucket_idx;
                w.next = *head;
            }
        }

        // Update chain head.
        *head = Some(waiter_idx);

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
        unsafe {
            (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
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
        let timer_id = deadline
            .map(|dl| crate::timer::add_timer_wakeup(
                dl, crate::sched::get_current_pid(),
            ))
            .unwrap_or(0);
        // R32 (NEW-3): if a deadline was requested but the timer pool is
        // exhausted (add_timer_wakeup → 0), schedule() below would sleep
        // FOREVER — nothing else wakes an un-signaled futex, so the timed
        // wait degenerated into an untimed hang. Unlink and fail instead.
        if deadline.is_some() && timer_id == 0 {
            remove_waiter(bucket_idx, waiter_idx);
            return -ENOMEM as i64;
        }
        crate::sched::schedule();
        if timer_id != 0 {
            crate::timer::del_timer(timer_id);
        }
        #[cfg(feature = "dfx-futex-trace")]
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
                    return -crate::syscall::errno::EINTR as i64;
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
                    if let Some(dl) = deadline {
                        if crate::drivers::timer::get_jiffies() >= dl {
                            return -ETIMEDOUT as i64;
                        }
                    }
                    // Spurious: loop — re-check the value and sleep again.
                    was_spurious = true;
                } else {
                    // Woken: the waker unlinked us from the chain and left the
                    // slot for us to free.
                    *slot = None;
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
fn remove_waiter(bucket_idx: usize, target_idx: usize) {
    let mut head = HASH_HEADS[bucket_idx].lock_irqsave();

    if *head == Some(target_idx) {
        let next = {
            let slot = WAITER_POOL[target_idx].lock_irqsave();
            slot.as_ref().and_then(|w| w.next)
        };
        *head = next;
        free_waiter(target_idx);
        return;
    }

    let mut current_idx = *head;
    while let Some(idx) = current_idx {
        let next = {
            let slot = WAITER_POOL[idx].lock_irqsave();
            slot.as_ref().and_then(|w| w.next)
        };

        if next == Some(target_idx) {
            let target_next = {
                let target_slot = WAITER_POOL[target_idx].lock_irqsave();
                target_slot.as_ref().and_then(|w| w.next)
            };
            {
                let mut slot = WAITER_POOL[idx].lock_irqsave();
                if let Some(ref mut w) = *slot {
                    w.next = target_next;
                }
            }
            free_waiter(target_idx);
            return;
        }

        current_idx = next;
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

    for bucket_idx in 0..HASH_SIZE {
        let mut head = HASH_HEADS[bucket_idx].lock_irqsave();
        let mut prev_idx: Option<usize> = None;
        let mut current_idx = *head;

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
                    *head = next;
                } else if let Some(prev) = prev_idx {
                    let mut prev_slot = WAITER_POOL[prev].lock_irqsave();
                    if let Some(ref mut pw) = *prev_slot {
                        pw.next = next;
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
pub fn futex_parse_timeout(timeout_ptr: u64, absolute: bool) -> Result<Option<u64>, i32> {
    use crate::drivers::timer::{get_jiffies, HZ};
    if timeout_ptr == 0 {
        return Ok(None);
    }
    if !crate::arch::uaccess::access_ok(timeout_ptr as usize, 16) {
        return Err(EFAULT);
    }
    let mut buf = [0u8; 16];
    // SAFETY: access_ok-validated user pointer; exception-table copy.
    let uncopied = unsafe {
        crate::arch::uaccess::copy_from_user(
            buf.as_mut_ptr(), timeout_ptr as *const u8, 16,
        )
    };
    if uncopied > 0 {
        return Err(EFAULT);
    }
    let sec = i64::from_le_bytes(buf[0..8].try_into().unwrap());
    let nsec = i64::from_le_bytes(buf[8..16].try_into().unwrap());
    if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
        return Err(EINVAL);
    }
    let jiffies = (sec as u64).saturating_mul(HZ)
        .saturating_add((nsec as u64 * HZ) / 1_000_000_000);
    if absolute {
        // Absolute CLOCK_REALTIME value; if already past, the min-1 clamp
        // arms an immediately-expiring timer → ETIMEDOUT on wake check.
        Ok(Some(jiffies.max(1)))
    } else {
        Ok(Some(get_jiffies().saturating_add(jiffies.max(1))))
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
/// Wakes up to `nr_wake` waiters on `uaddr`, then requeues up to `nr_requeue`
/// remaining waiters from `uaddr` to `uaddr2`.  For CMP_REQUEUE, verifies
/// `*uaddr == cmpval` first.
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

    let key1 = FutexKey::new(uaddr, mm, flags);

    // For CMP_REQUEUE, verify *uaddr == cmpval
    if is_cmp {
        // SAFETY: exception-table protected read; unmapped address → EFAULT.
        let uaddr_ptr = uaddr as *const u32;
        let uval = match unsafe {
            crate::arch::uaccess::get_user(uaddr_ptr)
        } {
            Some(v) => v,
            None => return -EFAULT as i64,
        };
        if uval != cmpval {
            return -EAGAIN as i64;
        }
    }

    // No requeue target or same address → just wake
    if uaddr2 == 0 || uaddr2 == uaddr || nr_requeue <= 0 {
        return futex_wake(uaddr, flags, nr_wake, FUTEX_BITSET_MATCH_ANY);
    }

    let key2 = FutexKey::new(uaddr2, mm, flags);
    let bucket1 = futex_hash(&key1);
    let bucket2 = futex_hash(&key2);

    let mut ret = 0i64;
    let mut woken = 0i32;

    // Collect tasks to wake and waiter indices to requeue. Dynamically
    // sized: fixed 8/32-entry arrays silently stranded waiters past their
    // capacity (pthread_cond_broadcast) — review IPC-C3/M7.
    let mut wake_list: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();
    let mut requeue_list: alloc::vec::Vec<usize> = alloc::vec::Vec::new();

    // Lock both buckets to prevent requeued entries from being in limbo.
    // Use address ordering (lower index first) to avoid ABBA deadlock.
    if bucket1 == bucket2 {
        // Same bucket — lock once.
        let mut head1 = HASH_HEADS[bucket1].lock_irqsave();

        let mut prev: Option<usize> = None;
        let mut cur = *head1;

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
                    *head1 = next;
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
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
            } else if (requeue_list.len() as i32) < nr_requeue
            {
                // Same bucket — just update the key (and keep the bucket
                // field truthful), stay in chain.
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot {
                        w.key = key2;
                        w.bucket = bucket2;
                    }
                }
                requeue_list.push(idx);
                ret += 1;
                prev = Some(idx);
                cur = next;
            } else {
                break;
            }
        }
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

        let mut prev: Option<usize> = None;
        let mut cur = *head1_ref;

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
                    *head1_ref = next;
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
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
            } else if (requeue_list.len() as i32) < nr_requeue
            {
                // Unlink from source chain.
                if prev.is_none() {
                    *head1_ref = next;
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
                }
                // Update key (incl. bucket bookkeeping) and insert into
                // destination chain immediately.
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot {
                        w.key = key2;
                        w.bucket = bucket2;
                        w.next = *head2_ref;
                    }
                }
                *head2_ref = Some(idx);

                requeue_list.push(idx);
                ret += 1;
                cur = next;
            } else {
                break;
            }
        }
        drop(guard_lo);
        drop(guard_hi);
    }

    // R12-2: the waiters were unlinked from their chains under the bucket
    // locks in the collecting pass; unlike futex_wake we are already past
    // those critical sections, so the PID revalidation stays as defense
    // in depth against the cross-CPU reap window.
    //
    // C8 wake-ordering invariant (waker side): every woken waiter had its
    // `woken` flag set to true under the bucket lock during the pass
    // above, strictly before these Task::wake_up calls — flag first, wake
    // second. This is what makes waking outside the bucket lock safe for
    // any wake source: a sleeper roused by a SIGNAL rather than by us
    // re-acquires the slot lock, reads woken == true (our store already
    // happened), and takes the success path instead of EINTR; a sleeper
    // that has not run yet is woken by the calls below. Reordered
    // (wake first, flag later) the signal-roused sleeper would read
    // woken == false, exit with EINTR, and the later store would wake no
    // one — the waiter's FUTEX_WAIT returns the wrong result or the
    // requeued-to futex never completes.
    for task in wake_list {
        if !task.is_null() {
            let pid = unsafe { (*task).pid() };
            let fresh = crate::process::pid_hash::pid_hash_lookup(pid);
            if fresh == task {
                Task::wake_up(task);
            }
        }
    }

    ret
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
    let op = (encoded >> 28) & 0xf;
    let cmp = (encoded >> 24) & 0xf;
    let oparg = ((encoded >> 12) & 0xfff) as i32;
    let cmparg = (encoded & 0xfff) as i32;

    let mm = current_futex_mm_id();
    let key2 = FutexKey::new(uaddr2, mm, flags);
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
        let old = match unsafe {
            crate::arch::uaccess::get_user(uaddr2 as *const u32)
        } {
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
            _ => return -EINVAL as i64,
        };
        cmp_holds = match cmp {
            FUTEX_OP_CMP_EQ => old_i == cmparg,
            FUTEX_OP_CMP_NE => old_i != cmparg,
            FUTEX_OP_CMP_LT => old_i < cmparg,
            FUTEX_OP_CMP_LE => old_i <= cmparg,
            FUTEX_OP_CMP_GT => old_i > cmparg,
            FUTEX_OP_CMP_GE => old_i >= cmparg,
            _ => return -EINVAL as i64,
        };
        if unsafe {
            !crate::arch::uaccess::put_user(uaddr2 as *mut u32, new)
        } {
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

/// DFX: dump the live state of a futex word + its waiter chain.
#[cfg(feature = "dfx-futex-trace")]
pub fn dfx_dump_futex_state(uaddr: usize, parked_pid: u32) {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    use crate::dfx::taskdump::dump_syscall_ring_for;
    let bucket = uaddr % HASH_SIZE;
    taskdump_raw_line(b"FTX-STATE u=");
    taskdump_dec(uaddr as u64);
    // The user word and its neighbours (16 words starting uaddr-16).
    for i in 0..4 {
        let a = uaddr + 16 * i;
        match unsafe { crate::arch::uaccess::get_user(a as *const u32) } {
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
            taskdump_dec(unsafe { (*t).pid() as u64 });
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
        if RWFRZ_WATCH[0].load(Relaxed) != 0 || RWFRZ_WATCH[1].load(Relaxed) != 0 {
            for w in RWFRZ_WATCH.iter() {
                let wa = w.load(Relaxed);
                if wa != 0 && wa == uaddr {
                    let c = RWFRZ_WATCH_CNT.fetch_add(1, Relaxed);
                    if c < RWFRZ_WATCH_BUDGET {
                        let (tid, tgid) = match crate::sched::current() {
                            Some(t) => unsafe { ((*t).pid(), (*t).tgid()) },
                            None => (0, 0),
                        };
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
                            crate::drivers::timer::get_jiffies(),
                        );
                    }
                    break;
                }
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
    {
        use core::sync::atomic::Ordering::Relaxed;
        let cur_tgid =
            crate::sched::current().map(|t| unsafe { (*t).tgid() as u32 });
        // sc185: the SA-5100 parcel latch never fired in sc184 (zero FXDV
        // lines), so also latch by process name — the first futex from any
        // thread of "hdf_devmgr" latches its tgid. The parcel matcher stays
        // (earlier latch = earlier coverage), this is the backstop.
        if FXDV_PID.load(Relaxed) == 0 {
            if let Some(t) = crate::sched::current() {
                let comm = unsafe { (*t).comm() };
                let name = core::str::from_utf8(&comm[..])
                    .unwrap_or("")
                    .trim_end_matches('\0');
                if name == "hdf_devmgr" {
                    fxdv_latch_devmgr(unsafe { (*t).tgid() as u32 });
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
            let interesting = is_rw_wait
                || (matches!(cmd, FUTEX_WAKE) && addr_idx != usize::MAX && val != 0);
            if interesting {
                let n = FXDV_N.fetch_add(1, Relaxed);
                if n < 400 {
                    // Dedup: first 3 per address, then every 64th.
                    let c = if addr_idx == usize::MAX {
                        0
                    } else {
                        FXDV_CNT[addr_idx].fetch_add(1, Relaxed)
                    };
                    if c < 3 || c % 64 == 0 {
                        crate::pr_warn!(
                            "FXDV #{} tid={} op={} uaddr={:#x} val={:#x} jif={}",
                            n,
                            crate::sched::current()
                                .map(|t| unsafe { (*t).pid() })
                                .unwrap_or(0),
                            match cmd {
                                FUTEX_WAIT => "WAIT",
                                FUTEX_WAIT_BITSET => "WAITB",
                                _ => "WAKE",
                            },
                            uaddr,
                            val,
                            crate::drivers::timer::get_jiffies(),
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
        if (0x7ff0_0000_0000..0x7ff0_0010_0000).contains(&uaddr) {
            use core::sync::atomic::Ordering::Relaxed as R2;
            let n = FXDV2_N.fetch_add(1, R2);
            if n < 500 {
                crate::pr_warn!(
                    "FXDV2 #{} tgid={} tid={} op={} uaddr={:#x} val={:#x} jif={}",
                    n,
                    crate::sched::current().map(|t| unsafe { (*t).tgid() }).unwrap_or(0),
                    crate::sched::current().map(|t| unsafe { (*t).pid() }).unwrap_or(0),
                    match cmd {
                        FUTEX_WAIT => "WAIT",
                        FUTEX_WAIT_BITSET => "WAITB",
                        FUTEX_WAKE => "WAKE",
                        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => "REQUEUE",
                        _ => "other",
                    },
                    uaddr,
                    val,
                    crate::drivers::timer::get_jiffies(),
                );
            }
        }
    }

    #[cfg(feature = "dfx-futex-trace")]
    {
        let pid = crate::sched::current().map(|t| unsafe { (*t).pid() as u64 }).unwrap_or(0);
        use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
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

    match cmd {
        FUTEX_WAIT => {
            // R7-A9: plain FUTEX_WAIT keeps a RELATIVE timeout even with
            // FUTEX_CLOCK_REALTIME set (Linux futex_init_timeout adds
            // ktime_get() for cmd == FUTEX_WAIT regardless of the flag);
            // only WAIT_BITSET interprets it absolutely.
            match futex_parse_timeout(_timeout, false) {
                Ok(dl) => futex_wait_timeout(uaddr, flags, val, FUTEX_BITSET_MATCH_ANY, dl),
                Err(e) => -(e as i64),
            }
        }
        FUTEX_WAKE => {
            futex_wake(uaddr, flags, val as i32, FUTEX_BITSET_MATCH_ANY)
        }
        FUTEX_WAIT_BITSET => {
            // WAIT_BITSET always interprets timeout as absolute time.
            match futex_parse_timeout(_timeout, true) {
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
            // Linux: negative nr_requeue or missing/misaligned uaddr2 is EINVAL.
            if nr_requeue < 0 {
                return -EINVAL as i64;
            }
            if uaddr2 == 0 || uaddr2 & 0x3 != 0 {
                return -EINVAL as i64;
            }
            // uaddr2 must be a readable user word (get_futex_key faults).
            if unsafe {
                crate::arch::uaccess::get_user(uaddr2 as *const u32).is_none()
            } {
                return -EFAULT as i64;
            }
            futex_requeue(uaddr, flags, val as i32, nr_requeue, uaddr2, val3, cmd == FUTEX_CMP_REQUEUE)
        }
        FUTEX_WAKE_OP => {
            // ABI: val = nr_wake on uaddr, args[3] (_timeout slot) = nr_wake2
            // on uaddr2, val3 = encoded op/cmp.
            if uaddr2 == 0 || uaddr2 & 0x3 != 0 {
                return -EINVAL as i64;
            }
            futex_wake_op(uaddr, flags, val as i32, _timeout as i32, uaddr2, val3)
        }
        _ => {
            // PI-related operations not yet supported
            -ENOSYS as i64
        }
    }
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
    if matches!(op & FUTEX_CMD_MASK, FUTEX_WAIT | FUTEX_WAIT_BITSET) {
        let readers = val & 0x7fffffff;
        if val & 0x80000000 != 0 && readers >= 4 && readers < 0x40000000 {
            static PARKED_DUMPS: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let n = PARKED_DUMPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // dfx=parkdump: each park dump costs hundreds of UART+kmsg
            // lines (hilogd.rd_kmsg eats a vCPU copying them out of
            // /dev/kmsg — the b128/b129 lesson), so the park-dump family
            // is OFF in launcher-milestone boots.
            if n < 16 && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::ParkDump) {
                if let Some(t) = crate::sched::current() {
                    let tk = unsafe { &*t };
                    let comm = tk.comm();
                    let name = core::str::from_utf8(&comm[..])
                        .unwrap_or("")
                        .trim_end_matches('\0');
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
                    unsafe { rwfz_stack_frames(t, "park", 12, 64) };
                }
            }
        }
    }
    // sc122 (Session P): the app-child park dump. The KILLDUMP path can
    // never sample the park — SIGKILL lands right after the userspace DFX
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
            if let Some(t) = crate::sched::current() {
                let tk = unsafe { &*t };
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
                    // dfx=parkdump: the APP-PARK dump prints the child's
                    // whole lib-region VMA table (APPMAP, ~2.9k lines per
                    // dump in sc179) + fd table + syscall rings — pure
                    // chain forensics, OFF in milestone boots (b128/b129
                    // hilogd.rd_kmsg lesson).
                    if n < 6
                        && crate::dfx::switches::enabled(
                            crate::dfx::switches::DfxSwitch::ParkDump,
                        )
                    {
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
                            const RL: usize = crate::process::task::LAST_CALLS_LEN;
                            let cur = tk.last_calls_cur.load(Relaxed);
                            let mut s2 = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(s2, "APP-PARK calls:");
                            for k in 0..RL {
                                let i = (cur + RL - k) % RL; // newest first
                                let v = tk.last_calls[i].load(Relaxed);
                                if v != 0 {
                                    let _ = write!(s2, " {}", v);
                                }
                            }
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
                            const RL: usize = crate::process::task::INIT_CALLS_LEN;
                            const BL: usize = crate::process::task::BIRTH_CALLS_LEN;
                            let cur = tk.init_calls_cur.load(Relaxed);
                            // sc143: 512 entries in chunks of 128 so a
                            // single UART flush cannot gut the whole line.
                            let mut chunk = alloc::string::String::new();
                            let mut shown = 0usize;
                            for k in 0..RL {
                                let i = (cur + RL - k) % RL; // newest first
                                let v = tk.init_calls[i].load(Relaxed);
                                if v != 0 {
                                    if shown % 128 == 0 {
                                        if !chunk.is_empty() {
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
                                crate::pr_warn!("{}", chunk);
                            }
                            // newest 96 with a0
                            let mut s3 = alloc::string::String::new();
                            let _ = write!(s3, "APP-PARK initA:");
                            for k in 0..96.min(RL) {
                                let i = (cur + RL - k) % RL;
                                let v = tk.init_calls[i].load(Relaxed);
                                if v != 0 {
                                    let a0 = tk.init_a0[i].load(Relaxed);
                                    let _ = write!(s3, " {}@{:#x}", v, a0);
                                }
                            }
                            crate::pr_warn!("{}", s3);
                            // sc143: the birth window (fork-exit onward),
                            // immune to loop churn.
                            let mut s4 = alloc::string::String::new();
                            let _ = write!(s4, "APP-PARK birth:");
                            for k in 0..BL {
                                let v = tk.birth_calls[k].load(Relaxed);
                                if v != 0 {
                                    let a0 = tk.birth_a0[k].load(Relaxed);
                                    let _ = write!(s4, " {}@{:#x}", v, a0);
                                }
                            }
                            crate::pr_warn!("{}", s4);
                        }
                        // sc125 (Session P): the pre-park FAILING-syscall
                        // tail — musl pthread_create mmaps the thread stack
                        // BEFORE clone, so an ENOMEM mmap means no clone
                        // ever reaches the census. Latched per task.
                        {
                            use core::sync::atomic::Ordering::Relaxed;
                            let cur = tk.last_fails_cur.load(Relaxed);
                            let mut s = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(s, "APP-PARK fails:");
                            for k in 0..8 {
                                let i = (cur + 8 - 1 - k) % 8; // newest first
                                let v = tk.last_fails[i].load(Relaxed);
                                if v != 0 {
                                    // sc135 (Session Q): a0/a1 name the
                                    // prctl option (nr167) and friends.
                                    let a0 = tk.last_fails_a0[i].load(Relaxed);
                                    let a1 = tk.last_fails_a1[i].load(Relaxed);
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
                            crate::pr_warn!("{}", s);
                        }
                        // SAFETY: t is the current task, in its own syscall.
                        unsafe { rwfz_stack_frames(t, "apppark", 12, 96) };
                        // sc139: the call-site-validated caller chain.
                        unsafe { apppark_rets(t) };
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
                                            let p = ops as *const crate::fs::FileOps;
                                            if core::ptr::eq(
                                                p,
                                                &crate::net::unix::UNIX_SOCKET_OPS,
                                            ) {
                                                match crate::net::unix::unix_file_of(
                                                    fd as usize,
                                                ) {
                                                    Some((sock, _)) => {
                                                        let bn = sock
                                                            .bound_name
                                                            .lock()
                                                            .clone()
                                                            .unwrap_or_else(|| {
                                                                alloc::string::String::from(
                                                                    "unbound",
                                                                )
                                                            });
                                                        let peer =
                                                            crate::net::unix::unix_peer_bound_name(
                                                                &sock,
                                                            )
                                                            .unwrap_or_else(|| {
                                                                alloc::string::String::from("-")
                                                            });
                                                        alloc::format!(
                                                            "unix bound={} peer={}",
                                                            bn, peer
                                                        )
                                                    }
                                                    None => "unix".into(),
                                                }
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::net::socket::SOCKET_OPS,
                                            ) {
                                                "inet-socket".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::net::netlink::NETLINK_OPS,
                                            ) {
                                                "netlink".into()
                                            } else if core::ptr::eq(p, &crate::net::raw::RAW_OPS)
                                            {
                                                "raw-socket".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::ipc::binder::BINDER_OPS,
                                            ) {
                                                "binder".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::fs::pipe::PIPE_OPS,
                                            ) {
                                                "pipe".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::fs::memfd::MEMFD_FILE_OPS,
                                            ) {
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
                                            unsafe {
                                                let regs = (top.wrapping_sub(
                                                    crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                                                ))
                                                    as *const crate::arch::x86_64::pt_regs::PtRegs;
                                                (*regs).rsp as usize
                                            }
                                        }
                                        #[cfg(feature = "riscv64")]
                                        {
                                            unsafe {
                                                let regs = (top.wrapping_sub(
                                                    crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
                                                ))
                                                    as *const crate::arch::riscv64::pt_regs::PtRegs;
                                                (*regs).sp as usize
                                            }
                                        }
                                        #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
                                        { 0usize }
                                    }
                                    None => 0usize,
                                };
                                if rsp != 0 {
                                    let rd64 = |va: usize| -> Option<u64> {
                                        let lo = unsafe {
                                            read_u32_via_task(t, va)
                                        }?;
                                        let hi = unsafe {
                                            read_u32_via_task(t, va + 4)
                                        }?;
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
                                                crate::mm::page::VirtAddr::new(
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
                                        crate::pr_warn!("{}", v);
                                    }
                                }
                            }
                            // (b) The global unix-socket registry once:
                            // which abstract/filesystem listeners exist at
                            // park time (the appconn hex was unreadable —
                            // strncpy stopped at the abstract NUL).
                            if n == 0 {
                                for line in crate::net::unix::registry_snapshot() {
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
    {
        const STK_PAGE: usize = 0x7fffffffc000;
        if uaddr & !0xfff == STK_PAGE {
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
                let comm = crate::sched::current()
                    .map(|t| unsafe { (*t).comm() })
                    .map(|c| core::str::from_utf8(c).unwrap_or(""))
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let tid = crate::sched::current()
                    .map(|t| unsafe { (*t).pid() })
                    .unwrap_or(0);
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
        if lt != 0
            && matches!(
                op & FUTEX_CMD_MASK,
                FUTEX_WAIT
                    | FUTEX_WAKE
                    | FUTEX_WAIT_BITSET
                    | FUTEX_WAKE_BITSET
                    | FUTEX_CMP_REQUEUE
            )
        {
            let mine = crate::sched::current()
                .map(|t| unsafe { (*t).tgid() == lt })
                .unwrap_or(false);
            if mine {
                let n = FXRS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < FXRS_BUDGET {
                    let tid = crate::sched::current()
                        .map(|t| unsafe { (*t).pid() })
                        .unwrap_or(0);
                    let comm = crate::sched::current()
                        .map(|t| unsafe { (*t).comm() })
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
                    crate::pr_warn!(
                        "FXRS #{} {} tid {} comm='{}' uaddr={:#x} val={:#x} u2={:#x} v3={:#x} tmo={:#x}",
                        n,
                        cmd,
                        tid,
                        comm,
                        uaddr,
                        val,
                        uaddr2,
                        val3,
                        timeout
                    );
                    let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
                    rwfz_op_log(uaddr, op, val, r);
                    crate::pr_warn!("FXRS #{} -> ret={}", n, r);
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
    if matches!(op & FUTEX_CMD_MASK, FUTEX_WAIT | FUTEX_WAIT_BITSET) && timeout != 0 {
        if let Some(t) = crate::sched::current() {
            let tk = unsafe { &*t };
            let comm = core::str::from_utf8(&tk.comm()[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            if comm.contains("foundation") || comm.contains("sa_main") {
                static FTW_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = FTW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 240 {
                    let mut ts = [0u8; 16];
                    let ts_ok = unsafe {
                        crate::arch::uaccess::copy_from_user(
                            ts.as_mut_ptr(),
                            timeout as *const u8,
                            ts.len(),
                        ) == 0
                    };
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
                        const RL: usize = crate::process::task::LAST_CALLS_LEN;
                        let cur = tk.last_calls_cur.load(Relaxed);
                        let mut s2 = alloc::string::String::new();
                        let _ = write!(s2, "FTWAIT calls:");
                        for k in 0..RL {
                            let i = (cur + RL - k) % RL; // newest first
                            let v = tk.last_calls[i].load(Relaxed);
                            if v != 0 {
                                let _ = write!(s2, " {}", v);
                            }
                        }
                        crate::pr_warn!("{}", s2);
                        let fcur = tk.last_fails_cur.load(Relaxed);
                        let mut s3 = alloc::string::String::new();
                        let _ = write!(s3, "FTWAIT fails:");
                        for k in 0..8 {
                            let i = (fcur + 8 - 1 - k) % 8;
                            let v = tk.last_fails[i].load(Relaxed);
                            if v != 0 {
                                let a0 = tk.last_fails_a0[i].load(Relaxed);
                                let _ = write!(s3, " nr{}=-{} a0={:#x}", v & 0xffff, v >> 16, a0);
                            }
                        }
                        crate::pr_warn!("{}", s3);
                    }
                    let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
                    rwfz_op_log(uaddr, op, val, r);
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
static RWFRZ_UADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static RWFRZ_MM: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static RWFRZ_TGID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static RWFRZ_FIRST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static RWFRZ_LASTVAL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static RWFRZ_LASTVAL_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static RWFRZ_DUMPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// sc82: per-word inflation-curve table. Each contended rwlock-shaped
/// word with readers >= 2 is tracked (last value + last print); one
/// line per word per minute carries the delta — a gradual +1 pile-up
/// (real readers parking in scopes) vs a single jump (count corruption
/// or a fork-inherited copy) is decided by this curve, not by the
/// terminal value alone.
struct HighWord {
    uaddr: usize,
    tgid: u32,
    lastv: u32,
    at: u64,
}
static RWFRZ_HIGHTAB: Spinlock<(usize, [HighWord; 8])> = Spinlock::new((
    0,
    [const {
        HighWord {
            uaddr: 0,
            tgid: 0,
            lastv: 0,
            at: 0,
        }
    }; 8],
));
static RWFRZ_BEAT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static RWFRZ_OPLOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Last tid census of the watched tgid + its size (leak forensics: a tid
/// that vanishes while the word never drains is a reader that exited).
static RWFRZ_TIDS: Spinlock<(usize, [u32; 64])> = Spinlock::new((0, [0; 64]));
/// Which tgid RWFRZ_TIDS currently censuses (sc82 early-watch support).
static RWFRZ_TIDS_CENSUS_TGID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Global thread-exit ring (sc82): every task unlink (thread OR process
/// leader) records (jiffies, pid, tgid). The freeze dump replays the
/// watched tgid's exits — readers that died BEFORE the watch adopted the
/// word (leak evidence with timestamps) instead of only post-adoption
/// vanishings. Forensics only; no behavior.
#[derive(Clone, Copy)]
struct ExitRec {
    j: u64,
    pid: u32,
    tgid: u32,
}
static EXIT_RING: Spinlock<(usize, [ExitRec; 256])> = Spinlock::new((0, [const { ExitRec { j: 0, pid: 0, tgid: 0 } }; 256]));

/// Called from pid_hash_remove under the bucket lock — keep it to two
/// atomic-ish stores (one short critical section on the ring lock).
/// sc86: when a word is adopted and the dying task belongs to the watched
/// tgid, ALSO snapshot the word through the dying task's page tables
/// (if its mm is still alive at unlink time) and print comm + value —
/// a reader that exits without the count decrementing is caught in the
/// act. The page-table read is skipped under the ring lock.
pub fn note_task_unlink(pid: u32, tgid: u32) {
    let j = crate::drivers::timer::get_jiffies() as u64;
    let mut g = EXIT_RING.lock_irqsave();
    let (ref mut head, ref mut ring) = *g;
    ring[*head % 256] = ExitRec { j, pid, tgid };
    *head += 1;
    drop(g);
    // Post-adoption exit snapshot (diagnostics only; no locks held here —
    // pid_hash_remove's bucket lock IS still held by the caller, so the
    // page walk must not take pid-hash locks; PageTableWalker does not).
    let uaddr = RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed);
    if uaddr != 0 && tgid == RWFRZ_TGID.load(core::sync::atomic::Ordering::Relaxed) {
        let t = unsafe { crate::sched::find_task_by_pid(pid) };
        if !t.is_null() {
            let tk = unsafe { &*t };
            let comm = tk.comm();
            let name = core::str::from_utf8(&comm[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            let word = unsafe { read_u32_via_task(t, uaddr) };
            crate::pr_warn!(
                "RWFRZ: EXIT-WORD tid {} comm='{}' word={:#x}",
                pid,
                name,
                word.unwrap_or(0xdeadbeef)
            );
        }
    }
}

fn exit_ring_dump_tgid(tgid: u32) {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let g = EXIT_RING.lock_irqsave();
    let head = g.0;
    let start = head.saturating_sub(256);
    for i in start..head {
        let r = g.1[i % 256];
        if r.tgid == tgid && r.pid != 0 {
            let _ = write!(s, " [pid {} @{}s]", r.pid, r.j / 100);
        }
    }
    drop(g);
    if s.is_empty() {
        crate::pr_warn!("RWFRZ: exit ring has NO exits for tgid {} — readers did not die by task unlink", tgid);
    } else {
        crate::pr_warn!("RWFRZ: exit ring for tgid {}:{}", tgid, s);
    }
}

/// sc93 launcher gate: AMS SIGKILLs a lifecycle-timed-out app while the
/// DFX userspace dumper itself fails on that app (every LIFECYCLE_TIMEOUT
/// freeze artifact says "normal stack:failed to fully dump due to
/// timeout") — the kernel is the only observer that can still read the
/// wedged process. Called at SIGKILL send time (signal.rs), when the app
/// is by definition terminally wedged: prints every thread of the tgid,
/// each thread's futex park word + its CURRENT value (binder reply waits,
/// condvars, mutexes — the park value names the primitive and protocol),
/// and rip/rsp + file-backed stack frames via the sc90 exec-VMA filter.
/// pr_warn so the quiet boot (loglevel=4) keeps it; 8 dumps/boot cap
/// against restart-loop flooding.
pub fn kill_dump_tgid(tgid: u32) {
    use core::fmt::Write;
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    if N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) >= 8 {
        return;
    }
    // Collect thread pointers only under the pid-hash walk (its bucket
    // locks forbid heavy work); the page-table reads happen after.
    // for_each_task takes an Fn closure, so the list goes through a
    // RefCell.
    // sc244 (leg-17): PIN every collected task. The dump runs at
    // SIGKILL send time, so the victim's threads exit CONCURRENTLY —
    // an unpinned raw pointer freed mid-dump printed a poisoned tid
    // (0xdeadbeef) and page-faulted the boot at the launcher floor
    // (sc244, t=1466, pidmax 4834). try_pin under the bucket lock is
    // the documented pin discipline; task_put at the end.
    let threads_cell = core::cell::RefCell::new(alloc::vec::Vec::<*mut Task>::new());
    crate::sched::for_each_task(|t| {
        if unsafe { (*t).tgid() as u32 } == tgid {
            if crate::process::task::Task::try_pin(t) {
                threads_cell.borrow_mut().push(t);
            }
        }
    });
    let threads = threads_cell.into_inner();
    let mut hdr = alloc::string::String::new();
    let _ = write!(hdr, "KILLDUMP: tgid {} threads={}", tgid, threads.len());
    for &t in threads.iter() {
        let tk = unsafe { &*t };
        let comm = core::str::from_utf8(tk.comm()).unwrap_or("").trim_end_matches('\0');
        let _ = write!(hdr, " [{}:{}]", tk.pid(), comm);
    }
    crate::pr_warn!("{}", hdr);
    for &t in threads.iter() {
        // Park word: this task's live (unwoken) entry in the waiter pool.
        let mut parked = None;
        for slot in WAITER_POOL.iter() {
            let s = slot.lock_irqsave();
            if let Some(w) = s.as_ref() {
                if core::ptr::eq(w.task, t) && !w.woken {
                    parked = Some((w.key.uaddr, w.bitset));
                }
            }
            drop(s);
            if parked.is_some() {
                break;
            }
        }
        let tk = unsafe { &*t };
        let comm = core::str::from_utf8(tk.comm()).unwrap_or("").trim_end_matches('\0');
        match parked {
            Some((uaddr, bitset)) => {
                let word = unsafe { read_u32_via_task(t, uaddr) };
                crate::pr_warn!(
                    "KILLDUMP: tid {} comm='{}' parked uaddr={:#x} bitset={:#x} word={:#x}",
                    tk.pid(),
                    comm,
                    uaddr,
                    bitset,
                    word.unwrap_or(0xdead_bee1)
                );
            }
            None => {
                crate::pr_warn!(
                    "KILLDUMP: tid {} comm='{}' NOT on a futex (blocked in another syscall or runnable)",
                    tk.pid(),
                    comm
                );
            }
        }
        // sc93: the saved entry frame carries the blocked syscall itself
        // — orig_rax (syscall nr) + args. For a task parked mid-syscall
        // this names the wait primitive outright (read/epoll/ioctl and
        // WHICH fd). Fd-like args (< 64) are resolved to paths.
        //
        // Stack-freed guard (KILLDUMP census race, monkey-b r38 +
        // monkey-a r32, identical badaddr 0x13ff0 on v9-x86 and v10-x86):
        // the pinned Task can outlive its own exit's free_kernel_stack()
        // by an unbounded window (reaping is asynchronous), and a
        // stackless task's kernel_stack_top() == KERNEL_STACK_SIZE — the
        // pt_regs read below then faults INSIDE the printer and panics
        // the boot. Skip the whole per-thread dump (entry frame, wedge
        // dump, frame walk) for such tasks; the header line already
        // counted them, so say why their lines are missing.
        let Some(_top) = tk.kernel_stack_top_checked() else {
            crate::pr_warn!(
                "KILLDUMP: tid {} stack-freed (exited mid-census; entry-frame/wedge/frame-walk skipped)",
                tk.pid()
            );
            continue;
        };
        {
            let top = _top;
            #[cfg(feature = "x86_64")]
            {
                let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
                    as *const crate::arch::x86_64::pt_regs::PtRegs;
                // SAFETY: diagnostic read of this task's entry frame.
                let r = unsafe { &*regs };
                crate::pr_warn!(
                    "KILLDUMP: tid {} syscall nr={:#x} rdi={:#x} rsi={:#x} rdx={:#x} r10={:#x}",
                    tk.pid(),
                    r.orig_rax,
                    r.rdi,
                    r.rsi,
                    r.rdx,
                    r.r10
                );
                let mut fdnote = alloc::string::String::new();
                for arg in [r.rdi, r.rsi, r.rdx] {
                    if arg < 64 {
                        // sc210: None-safe fdtable walk (just-forked task
                        // has no fdtable yet; fdtable() panics).
                        if let Some(f) = tk
                            .try_fdtable()
                            .and_then(|fdt| fdt.get_file(arg as usize))
                        {
                            use core::fmt::Write;
                            let _ = write!(fdnote, " fd{}={}", arg, f.path());
                        }
                    }
                }
                if !fdnote.is_empty() {
                    crate::pr_warn!("KILLDUMP: tid {} {}", tk.pid(), fdnote);
                }
                // sc97 wedge forensics: the sc96 kills put the launcher
                // main thread in writev(fd=3) on an inode-less file 6/8
                // times (plus one mount(2) and one open(2) park). Name
                // the fd TYPE by ops identity and dump the state that
                // decides the wedge; read the syscall's user arguments
                // (iovec layout + payload head, path/fstype strings); and
                // print the task run state so "parked" vs "runnable"
                // (slow exit path) can be told apart.
                unsafe { kill_dump_wedge(tk, t, r.orig_rax, [r.rdi, r.rsi, r.rdx]) };
            }
        }
        unsafe { rwfz_stack_frames(t, "KILLDUMP", 10, 96) };
    }
    // sc244: release the pins taken at collection (may free the last
    // references of the concurrently-dying threads — schedulable
    // context, no locks held).
    for &t in threads.iter() {
        crate::process::task::Task::task_put(t);
    }
}

/// sc97: type name for a File by fops identity (the kernel-image symbol
/// at the ops address names any not listed here, offline via nm).
fn kd_fd_type(file: &crate::fs::File) -> &'static str {
    let ops = match file.get_ops() {
        Some(o) => o as *const crate::fs::FileOps as *const (),
        None => return "no-ops",
    };
    let is = |p: &crate::fs::FileOps| ops == (p as *const _ as *const ());
    if is(&crate::fs::pipe::PIPE_OPS) {
        return "pipe";
    }
    if is(&crate::fs::fifo::FIFO_OPS) {
        return "fifo";
    }
    if is(&crate::net::unix::UNIX_SOCKET_OPS) {
        return "unix-sock";
    }
    if is(&crate::net::socket::SOCKET_OPS) {
        return "inet-sock";
    }
    if is(&crate::ipc::binder::BINDER_OPS) {
        return "binder";
    }
    if is(&crate::fs::inotify::INOTIFY_OPS) {
        return "inotify";
    }
    if is(&crate::syscall::misc::EVENTFD_OPS) {
        return "eventfd";
    }
    "other"
}

/// sc97: deep dump of the wedged syscall's fd and user arguments.
/// SAFETY: `t` is the live Task whose entry frame the args came from.
unsafe fn kill_dump_wedge(tk: &Task, t: *mut Task, nr: u64, args: [u64; 3]) {
    use core::fmt::Write;
    // sc97 discriminator: RUNNING-state at SIGKILL time means the entry
    // frame's syscall may be STALE (task is executing userspace — e.g. a
    // busy loop) or the task burns kernel time in a non-sleeping loop, or
    // it was set runnable but never linked/scheduled (wake-loss family).
    // utime vs stime names the burn mode; tiny both = never scheduled.
    let linked = match crate::sched::sched::grq_diag_cfs_linked(t) {
        Some(b) => {
            if b { "1" } else { "0" }
        }
        None => "n/a",
    };
    crate::pr_warn!(
        "KILLDUMP: tid {} state={:#x} on_cpu={} cfs_linked={} pick_state={} cpu={} utime={}t stime={}t",
        tk.pid(),
        tk.state().bits(),
        tk.on_cpu(),
        linked,
        tk.pick_state(),
        tk.ti_cpu(),
        tk.utime_ticks.load(core::sync::atomic::Ordering::Relaxed),
        tk.stime_ticks.load(core::sync::atomic::Ordering::Relaxed)
    );
    // sc105 stime-burn attribution: page-shaped positional reads (the mmap
    // demand-fault signature) with service time, cache-miss fills, and the
    // timer-tick kernel-rip histogram (per comm, 4 KiB pages). Global
    // counters — deltas across consecutive launcher kills isolate this
    // child's share; rip pages symbolize via nm/addr2line.
    {
        let (reads, misses, cycles, fills) = crate::fs::ext4::file::demand_read_stats();
        let mhz = {
            #[cfg(feature = "x86_64")]
            {
                crate::arch::x86_64::cpu::get_counter_freq() / 1_000_000
            }
            #[cfg(not(feature = "x86_64"))]
            {
                0
            }
        };
        let us = if mhz > 0 { cycles / mhz } else { 0 };
        crate::pr_warn!(
            "KILLDUMP: demand-fault reads={} misses={} fillmisses={} cyc={} (~{}us avg={}us)",
            reads,
            misses,
            fills,
            cycles,
            us,
            if reads > 0 { us / reads } else { 0 }
        );
        #[cfg(feature = "x86_64")]
        {
            let rows = crate::arch::x86_64::trap::stime_profile_top(true);
            for (comm, page, ticks) in rows.iter().take(8) {
                let comm_str = core::str::from_utf8(comm)
                    .unwrap_or("")
                    .trim_end_matches('\0');
                crate::pr_warn!(
                    "KILLDUMP: stime-page(FOCUS) comm='{}' rip_page={:#x} ticks={}",
                    comm_str,
                    page,
                    ticks
                );
            }
            let rows = crate::arch::x86_64::trap::stime_profile_top(false);
            for (comm, page, ticks) in rows.iter().take(8) {
                let comm_str = core::str::from_utf8(comm)
                    .unwrap_or("")
                    .trim_end_matches('\0');
                crate::pr_warn!(
                    "KILLDUMP: stime-page comm='{}' rip_page={:#x} ticks={}",
                    comm_str,
                    page,
                    ticks
                );
            }
        }
    }
    // Whole low fdtable: fd/type/path one line — where fd3 came from and
    // what the process holds overall.
    // sc210: try_fdtable — the wedge dump can observe a task whose
    // fdtable is not initialized (just-forked); fdtable() would panic
    // ("FdTable not initialized", task.rs) and take the whole boot down
    // with a kernel panic mid-census — exactly how sc210 (the deepest
    // boot ever, launcher executing) died at t=1410s.
    {
        let mut line = alloc::string::String::new();
        let _ = write!(line, "KILLDUMP: tid {} fdtab:", tk.pid());
        for fd in 0..16usize {
            if let Some(f) = tk.try_fdtable().and_then(|fdt| fdt.get_file(fd)) {
                let _ = write!(line, " [{}:{}:{}]", fd, kd_fd_type(&f), f.path());
            }
        }
        crate::pr_warn!("{}", line);
    }
    // The syscall's primary fd (rdi for read/write/writev/ioctl-family).
    if let Some(&fd) = args.first() {
        if fd < 64
            && tk.try_fdtable()
                .map(|fdt| fdt.get_file(fd as usize).is_some())
                .unwrap_or(false)
        {
            if let Some(f) = tk.try_fdtable().and_then(|fdt| fdt.get_file(fd as usize)) {
                let ty = kd_fd_type(&f);
                crate::pr_warn!(
                    "KILLDUMP: tid {} fd{} type={} flags={:#x}",
                    tk.pid(),
                    fd,
                    ty,
                    f.flags_bits()
                );
                match ty {
                    "pipe" => {
                        if let Some(pipe) = crate::fs::pipe::pipe_of_file(&f) {
                            let (fill, cap, rc, wc) = pipe.diag_snapshot();
                            let pipe_ptr = unsafe { *f.private_data.get() }
                                .map(|p| p as usize)
                                .unwrap_or(0);
                            crate::pr_warn!(
                                "KILLDUMP: tid {} pipe fill={}/{} rclosed={} wclosed={} ptr={:#x}",
                                tk.pid(),
                                fill,
                                cap,
                                rc,
                                wc,
                                pipe_ptr
                            );
                            // Who else holds an end? create_pipe installs the
                            // same Arc<Pipe> raw pointer in BOTH files, so a
                            // private_data match names every holder task and
                            // which end (O_RDONLY = reader) it owns.
                            if pipe_ptr != 0 {
                                let holders_cell =
                                    core::cell::RefCell::new(alloc::string::String::new());
                                crate::sched::for_each_task(|ht| {
                                    let hk = unsafe { &*ht };
                                    if hk.pid() == 0 {
                                        return;
                                    }
                                    for hfd in 0..32usize {
                                        // sc210: None-safe — the census walks
                                        // every task; a just-forked one has
                                        // no fdtable yet.
                                        if let Some(hf) = hk
                                            .try_fdtable()
                                            .and_then(|fdt| fdt.get_file(hfd))
                                        {
                                            let hty = kd_fd_type(&hf);
                                            if hty == "pipe"
                                                && unsafe { *hf.private_data.get() }
                                                    == Some(pipe_ptr as *mut u8)
                                            {
                                                let hcomm = core::str::from_utf8(hk.comm())
                                                    .unwrap_or("")
                                                    .trim_end_matches('\0');
                                                let _ = write!(
                                                    holders_cell.borrow_mut(),
                                                    " [{}:{} fd{} {}]",
                                                    hk.pid(),
                                                    hcomm,
                                                    hfd,
                                                    if hf.flags_bits() & 0o3 == 0 {
                                                        "READ"
                                                    } else {
                                                        "WRITE"
                                                    }
                                                );
                                            }
                                        }
                                    }
                                });
                                let holders = holders_cell.into_inner();
                                crate::pr_warn!(
                                    "KILLDUMP: tid {} pipe holders:{}",
                                    tk.pid(),
                                    if holders.is_empty() { " NONE" } else { &holders }
                                );
                            }
                        }
                    }
                    "unix-sock" => {
                        if let Some(us) = crate::net::unix::unix_of_file_diag(&f) {
                            let (qb, qt, cap, ql) = us.diag_snapshot();
                            let peer = us.dgram_peer.lock().clone();
                            let bound = us.bound_name.lock().clone();
                            crate::pr_warn!(
                                "KILLDUMP: tid {} unix kind={:?} bound={:?} dgram_peer={:?} queued={}B truesize={} sndbuf={} qlen={}",
                                tk.pid(),
                                us.kind,
                                bound,
                                peer,
                                qb,
                                qt,
                                cap,
                                ql
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // User-argument payloads: writev/write iovec heads (libhilog writes
    // header28/tag/content — the payload names the producer), open/mount
    // path strings.
    match nr {
        20 | 1 | 19 => {
            // writev / write / readv — iovec array at args[1] (writev) or
            // raw buffer for write.
            if nr == 20 {
                let mut iovs = alloc::vec![0u8; 3 * 16];
                let n = unsafe { read_bytes_via_task(t, args[1] as usize, &mut iovs) };
                for i in 0..3 {
                    if n < (i + 1) * 16 {
                        break;
                    }
                    let base = u64::from_le_bytes(iovs[i * 16..i * 16 + 8].try_into().unwrap());
                    let len = u64::from_le_bytes(iovs[i * 16 + 8..i * 16 + 16].try_into().unwrap());
                    let mut head = alloc::vec![0u8; 48];
                    let hn = unsafe { read_bytes_via_task(t, base as usize, &mut head) };
                    let mut txt = alloc::string::String::new();
                    for &b in &head[..hn] {
                        if b == 0 {
                            break;
                        }
                        if (0x20..0x7f).contains(&b) {
                            txt.push(b as char);
                        } else {
                            txt.push('.');
                        }
                    }
                    crate::pr_warn!(
                        "KILLDUMP: tid {} iov{} base={:#x} len={} head='{}'",
                        tk.pid(),
                        i,
                        base,
                        len,
                        txt
                    );
                }
            }
        }
        2 => {
            // open(2): pathname at args[0]
            let p = unsafe { read_cstr_via_task(t, args[0] as usize, 96) };
            crate::pr_warn!("KILLDUMP: tid {} open path='{}'", tk.pid(), p);
        }
        165 => {
            // mount(2): source/target/fstype at args[0..2]
            let s = unsafe { read_cstr_via_task(t, args[0] as usize, 96) };
            let tg = unsafe { read_cstr_via_task(t, args[1] as usize, 96) };
            let ft = unsafe { read_cstr_via_task(t, args[2] as usize, 32) };
            crate::pr_warn!(
                "KILLDUMP: tid {} mount src='{}' tgt='{}' fstype='{}'",
                tk.pid(),
                s,
                tg,
                ft
            );
        }
        _ => {}
    }
}

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
    let tid = crate::sched::current()
        .map(|t| t.pid() as u64)
        .unwrap_or(0);
    crate::pr_warn!(
        "RWFRZ-OP #{} {} tid {} val={:#x} ret={}",
        n,
        cmd,
        tid,
        val,
        result
    );
}

/// Read up to `buf.len()` bytes in `task`'s user address space via its
/// page tables, stopping at the first unmapped page (KILLDUMP sc97:
/// iovec arrays, path strings, log payload heads of the wedged task).
/// Returns how many bytes were actually read.
/// SAFETY: `task` must be a live Task; read-only diagnostic walk.
unsafe fn read_bytes_via_task(task: *const Task, va: usize, buf: &mut [u8]) -> usize {
    let tk = unsafe { &*task };
    let aspace = match tk.address_space() {
        Some(a) => a,
        None => return 0,
    };
    let root = aspace.root_ppn();
    let mut done = 0usize;
    while done < buf.len() {
        let cur = va + done;
        let (ppn, _) = match unsafe {
            crate::arch::mm::mm_ops::PageTableWalker::walk(root, cur as u64)
        } {
            Some(p) => p,
            None => break,
        };
        let pa = (ppn as usize) << 12;
        let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64))
            .bits() as usize
            + (cur & 0xfff);
        let chunk = core::cmp::min(buf.len() - done, 0x1000 - (cur & 0xfff));
        for i in 0..chunk {
            buf[done + i] = unsafe { core::ptr::read_volatile((kv + i) as *const u8) };
        }
        done += chunk;
    }
    done
}

/// Printable snapshot of a NUL-terminated user string in `task`'s space.
/// SAFETY: same contract as read_bytes_via_task.
unsafe fn read_cstr_via_task(task: *const Task, va: usize, max: usize) -> alloc::string::String {
    if va == 0 {
        return alloc::string::String::from("(null)");
    }
    let mut raw = alloc::vec![0u8; max];
    let n = unsafe { read_bytes_via_task(task, va, &mut raw) };
    let end = raw[..n].iter().position(|&b| b == 0).unwrap_or(n);
    let mut s = alloc::string::String::new();
    for &b in &raw[..end] {
        if (0x20..0x7f).contains(&b) {
            s.push(b as char);
        } else {
            s.push('.');
        }
    }
    s
}

/// Read one u32 in `task`'s user address space via its page tables
/// (khungtaskd's get_user would read through the wrong mm).
/// SAFETY: `task` must be a live Task; read-only diagnostic walk.
unsafe fn read_u32_via_task(task: *const Task, va: usize) -> Option<u32> {
    let tk = unsafe { &*task };
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    let (ppn, _) =
        unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va as u64)? };
    let pa = (ppn as usize) << 12;
    let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64)).bits()
        as usize
        + (va & 0xfff);
    Some(unsafe { core::ptr::read_volatile(kv as *const u32) })
}

/// sc138 (Session Q): display path for a File that may be a memfd —
/// `File::path()` prints "anon_inode:[<ino>]" for the pathless files;
/// resolve <ino> through the memfd name registry (recorded at
/// memfd_create) so APPFD/APPMAP can name the engine libs the children
/// map through memfds ("anon_inode:[17] memfd:libXXX").
fn probe_file_path(f: &crate::fs::file::File) -> alloc::string::String {
    let p = f.path();
    if let Some(rest) = p.strip_prefix("anon_inode:[") {
        if let Some(num) = rest.strip_suffix(']') {
            if let Ok(ino) = num.parse::<u64>() {
                if let Some(name) = crate::fs::memfd::memfd_name_by_ino(ino) {
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
/// IMMEDIATELY BEFORE it decode as a call instruction — `E8 rel32`
/// (v-5) or `FF D0-D7` call-reg (v-2) — the cheap x86-64 signature of
/// a real return address. Slots surviving that check are the real
/// frames. SAFETY: `task` live; diagnostic reads through its tables.
unsafe fn apppark_rets(task: *const Task) {
    let tk = unsafe { &*task };
    // Stack-freed guard (KILLDUMP census race class): nothing to walk
    // without a live stack.
    let Some(top) = tk.kernel_stack_top_checked() else {
        return;
    };
    #[cfg(feature = "x86_64")]
    let rsp = unsafe {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        (*regs).rsp as usize
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
        let lo = unsafe { read_u32_via_task(task, va)? };
        let hi = unsafe { read_u32_via_task(task, va + 4)? };
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
        let Some(vm) = vmas.find(crate::mm::page::VirtAddr::new(v as usize)) else {
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
    crate::pr_warn!("{}", out);
}

/// One user-stack frame line resolved to its covering VMA (Session F's
/// byte-match symbolization method). sc90: `scan_slots` parametrizes the
/// stack window (64 for per-task dumps; the recursion holder needs 256 —
/// its 16-deep chain with OH frame sizes sits ~1-2 KiB above rsp).
/// SAFETY: diagnostic walk of `task`'s tables; `task` live.
unsafe fn rwfz_stack_frames(
    task: *const Task,
    label: &str,
    max_frames: u64,
    scan_slots: u64,
) {
    let tk = unsafe { &*task };
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
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: diagnostic read of this task's entry frame.
        unsafe { ((*regs).rsp, (*regs).rip) }
    };
    #[cfg(feature = "riscv64")]
    let (rsp, rip) = {
        let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::riscv64::pt_regs::PtRegs;
        // SAFETY: same contract, riscv64.
        unsafe { ((*regs).sp, (*regs).epc) }
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
        let vm = vmas.find(crate::mm::page::VirtAddr::new(v as usize))?;
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
        let lo = unsafe { read_u32_via_task(task, va as usize)? };
        let hi = unsafe { read_u32_via_task(task, (va + 4) as usize)? };
        Some(lo as u64 | ((hi as u64) << 32))
    };
    // The RIP itself first — the futex park site names the primitive
    // (musl __timedwait caller: rdlock/wrlock/condvar). sc90: ripbytes
    // byte-identify the park-site lib offline (the mapping's fd is gone;
    // filesz prints 0 — offset+bytes are the surviving fingerprints).
    if let Some((s, e, fd, base, off, fsz)) = vma_of(rip) {
        crate::pr_warn!(
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
        );
    } else {
        crate::pr_warn!(
            "RWFRZ:   {} rsp={:#x} rip={:#x} (no vma)",
            label,
            rsp,
            rip
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
        crate::pr_warn!("{}", raw);
    }
    crate::pr_warn!("RWFRZ:   {} frames:{}", label, s);
}

/// Cluster of futex waiters on one (uaddr, mm): how many, which tgid and
/// one sample task (for the page-table read).
struct WordCluster {
    uaddr: usize,
    mm: usize,
    tgid: u32,
    n: usize,
    sample: *mut Task,
}

fn waiter_clusters(min: usize) -> alloc::vec::Vec<WordCluster> {
    let mut out: alloc::vec::Vec<WordCluster> = alloc::vec::Vec::new();
    for b in HASH_HEADS.iter() {
        let h = b.lock_irqsave();
        let mut idx = *h;
        'chain: while let Some(i) = idx {
            let (entry, next) = {
                let slot = WAITER_POOL[i].lock_irqsave();
                match slot.as_ref() {
                    Some(w) if !w.task.is_null() => {
                        let t = w.task;
                        let pid = unsafe { (*t).pid() };
                        let live = core::ptr::eq(
                            unsafe { crate::sched::find_task_by_pid(pid) },
                            t,
                        );
                        if live {
                            (
                                Some((w.key.uaddr, w.key.mm, unsafe { (*t).tgid() as u32 }, t)),
                                w.next,
                            )
                        } else {
                            (None, w.next)
                        }
                    }
                    Some(w2) => (None, w2.next),
                    None => (None, None),
                }
            };
            if let Some((uaddr, mm, tgid, t)) = entry {
                if let Some(c) = out.iter_mut().find(|c| c.uaddr == uaddr && c.mm == mm) {
                    c.n += 1;
                } else {
                    out.push(WordCluster { uaddr, mm, tgid, n: 1, sample: t });
                }
            }
            match next {
                Some(nx) => idx = Some(nx),
                None => break 'chain,
            }
        }
        drop(h);
    }
    out.retain(|c| c.n >= min);
    out
}

/// musl-rwlock-shaped word: waiters bit set and a sane holder count
/// (writer-held 0x7fffffff excluded — that shape drains normally).
fn rwlock_frozen_shape(v: u32) -> bool {
    let readers = v & 0x7fffffff;
    v & 0x80000000 != 0 && readers >= 1 && readers < 0x40000000
}

/// Called from the binder stall scan at khungtaskd cadence (process
/// context, no locks held). Tracks reader-count evolution on contended
/// rwlock words, watches the frozen one's futex traffic, diffs the tgid's
/// live tids, and fires a one-shot full-tgid park-stack dump.
pub fn freeze_scan(now: u64, hz: u64) {
    if hz == 0 {
        return;
    }
    // sc83: scanner liveness heartbeat — one line per minute. sc82 lost
    // khungtaskd silently ~2 min after adoption (stall_report's 500 ms
    // irq-off WORLD window); without this line that death is invisible.
    // Serial-cleanliness: the heartbeat joins the wwcensus family so
    // ordinary boots print nothing periodic.
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::WwCensus) {
        static BEAT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let last = BEAT.load(core::sync::atomic::Ordering::Relaxed);
        if now.saturating_sub(last) >= 60 * hz {
            BEAT.store(now, core::sync::atomic::Ordering::Relaxed);
            crate::pr_warn!("RWFRZ: scan alive at {}s", now / hz);
        }
    }
    // ---- adopt / refresh the watched word ----
    let mut adopted = false;
    for c in waiter_clusters(1) {
        let v = match unsafe { read_u32_via_task(c.sample, c.uaddr) } {
            Some(v) => v,
            None => continue,
        };
        let readers = v & 0x7fffffff;
        // Formation watch (sc82: threshold lowered 6 -> 2, per-word table
        // with delta): the inflation curve decides leak vs corruption.
        if readers >= 2
            && readers < 0x40000000
            && c.uaddr != RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed)
        {
            let mut hl = RWFRZ_HIGHTAB.lock_irqsave();
            let (ref mut hn, ref mut htab) = *hl;
            let slot = (0..*hn).find(|&i| htab[i].uaddr == c.uaddr);
            match slot {
                Some(i) => {
                    if now.saturating_sub(htab[i].at) >= 60 * hz {
                        let prev_r = htab[i].lastv & 0x7fffffff;
                        let cur_r = v & 0x7fffffff;
                        htab[i].lastv = v;
                        htab[i].at = now;
                        crate::pr_warn!(
                            "RWFRZ-HIGH uaddr={:#x} tgid {} waiters={} word={:#x} readers={} (was {}, d{:+})",
                            c.uaddr,
                            c.tgid,
                            c.n,
                            v,
                            cur_r,
                            prev_r,
                            cur_r as i64 - prev_r as i64
                        );
                    }
                }
                None => {
                    // sc83: evict-by-readers when full — sc82 filled all 8
                    // slots with transient readers=2 words at boot and the
                    // real frozen word could never enter the table.
                    let mut slot = if *hn < 8 { *hn } else { 0 };
                    if *hn >= 8 {
                        let mut min_r = u32::MAX;
                        for i in 0..8 {
                            let r = htab[i].lastv & 0x7fffffff;
                            if r < min_r {
                                min_r = r;
                                slot = i;
                            }
                        }
                        if readers <= min_r {
                            slot = usize::MAX; // nothing worth evicting
                        }
                    }
                    if slot != usize::MAX {
                        htab[slot] = HighWord {
                            uaddr: c.uaddr,
                            tgid: c.tgid,
                            lastv: v,
                            at: now,
                        };
                        if *hn < 8 {
                            *hn += 1;
                        }
                        crate::pr_warn!(
                            "RWFRZ-HIGH uaddr={:#x} tgid {} waiters={} word={:#x} readers={} (first seen)",
                            c.uaddr,
                            c.tgid,
                            c.n,
                            v,
                            readers
                        );
                    }
                }
            }
            drop(hl);
        }
        if adopted || c.n < 3 || !rwlock_frozen_shape(v) {
            continue;
        }
        let cur_u = RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed);
        if cur_u == c.uaddr {
            continue; // already watched (delta path below)
        }
        RWFRZ_UADDR.store(c.uaddr, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_MM.store(c.mm, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_TGID.store(c.tgid, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_FIRST.store(now, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_LASTVAL.store(v, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_LASTVAL_AT.store(now, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_DUMPED.store(false, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_OPLOG.store(0, core::sync::atomic::Ordering::Relaxed);
        adopted = true;
        crate::pr_warn!(
            "RWFRZ: watching uaddr={:#x} mm={:#x} tgid {} waiters={} word={:#x} readers={}",
            c.uaddr,
            c.mm,
            c.tgid,
            c.n,
            v,
            readers
        );
    }
    let uaddr = RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed);
    let tgid = RWFRZ_TGID.load(core::sync::atomic::Ordering::Relaxed);
    // ---- tid census diff (leak forensics) ----
    // sc82: run from boot start on the samgr tgid (334 on the scgold
    // image, stable across sc74..sc81) so pre-adoption reader exits are
    // timestamped too; switches to the adopted tgid once a word is taken.
    const EARLY_SAMGR_TGID: u32 = 334;
    let census_tgid = if uaddr != 0 { tgid } else { EARLY_SAMGR_TGID };
    {
        let seen_cell: core::cell::RefCell<(usize, [u32; 64])> =
            core::cell::RefCell::new((0, [0; 64]));
        crate::sched::for_each_task(|t| {
            // SAFETY: callback runs under the pid-hash bucket lock (same
            // contract as sleep_snapshot); reads confined to pid fields.
            let tk = unsafe { &*t };
            let idx = {
                let mut b = seen_cell.borrow_mut();
                if tk.tgid() as u32 == census_tgid && b.0 < 64 {
                    let i = b.0;
                    b.0 += 1;
                    Some(i)
                } else {
                    None
                }
            };
            if let Some(i) = idx {
                seen_cell.borrow_mut().1[i] = tk.pid();
            }
        });
        let (k, seen) = seen_cell.into_inner();
        let mut g = RWFRZ_TIDS.lock_irqsave();
        let (ref mut prev_n, ref mut prev) = *g;
        let prev_tgid = RWFRZ_TIDS_CENSUS_TGID.swap(census_tgid, core::sync::atomic::Ordering::Relaxed);
        if prev_tgid != census_tgid {
            // Baseline switch (early-watch -> adopted word): reset silently.
            *prev_n = 0;
        }
        for i in 0..*prev_n {
            let tid = prev[i];
            if tid != 0 && !seen[..k].contains(&tid) {
                crate::pr_warn!(
                    "RWFRZ: TID-VANISHED tgid {} tid {} (reader-exit window; word={:#x})",
                    census_tgid,
                    tid,
                    RWFRZ_LASTVAL.load(core::sync::atomic::Ordering::Relaxed)
                );
            }
        }
        for s in seen[..k].iter() {
            if !prev[..*prev_n].contains(s) {
                crate::pr_warn!("RWFRZ: TID-NEW tgid {} tid {}", census_tgid, s);
            }
        }
        prev[..k.min(64)].copy_from_slice(&seen[..k.min(64)]);
        *prev_n = k;
        drop(g);
    }
    if uaddr == 0 {
        return;
    }
    // Re-read through any live waiter of the tgid (sample may have exited).
    let mut word_now: Option<u32> = None;
    for c in waiter_clusters(1) {
        if c.uaddr == uaddr && c.mm == RWFRZ_MM.load(core::sync::atomic::Ordering::Relaxed) {
            word_now = unsafe { read_u32_via_task(c.sample, c.uaddr) };
            break;
        }
    }
    // ---- word delta ----
    if let Some(v) = word_now {
        let prev = RWFRZ_LASTVAL.load(core::sync::atomic::Ordering::Relaxed);
        if v != prev {
            RWFRZ_LASTVAL.store(v, core::sync::atomic::Ordering::Relaxed);
            RWFRZ_LASTVAL_AT.store(now, core::sync::atomic::Ordering::Relaxed);
            crate::pr_warn!(
                "RWFRZ: word delta {:#x} -> {:#x} (readers {} -> {}) at {}s",
                prev,
                v,
                prev & 0x7fffffff,
                v & 0x7fffffff,
                now / hz
            );
        } else {
            // Heartbeat once a minute on the frozen word: proves the count
            // is stuck, not merely unobserved. Does NOT touch LASTVAL_AT —
            // that stamp is the freeze-stability clock for the dump gate.
            let at = RWFRZ_BEAT.load(core::sync::atomic::Ordering::Relaxed);
            if now.saturating_sub(at) >= 60 * hz {
                RWFRZ_BEAT.store(now, core::sync::atomic::Ordering::Relaxed);
                let mm = RWFRZ_MM.load(core::sync::atomic::Ordering::Relaxed);
                let wcount = waiter_clusters(1)
                    .iter()
                    .find(|c| c.uaddr == uaddr && c.mm == mm)
                    .map(|c| c.n)
                    .unwrap_or(0);
                crate::pr_warn!(
                    "RWFRZ: word stuck {:#x} readers={} kernel_waiters={} tgid {} age {}s",
                    v,
                    v & 0x7fffffff,
                    wcount,
                    tgid,
                    now.saturating_sub(RWFRZ_FIRST.load(core::sync::atomic::Ordering::Relaxed)) / hz
                );
            }
        }
    }
    // ---- one-shot full-tgid dump ----
    // sc83: fire AT ADOPTION (gate removed). sc82 showed the scanner can
    // die on the NEXT scan (stall_report's own WORLD-guarded dump runs
    // ~500 ms with irqs off under load; khungtaskd never scanned again)
    // — waiting 120 s for stability loses the dump entirely. The frozen
    // shape (waiters>=3 + WAITERS|N-readers word) is already
    // cross-boot-stable; stability-waiting buys nothing.
    if !RWFRZ_DUMPED.load(core::sync::atomic::Ordering::Relaxed) {
        RWFRZ_DUMPED.store(true, core::sync::atomic::Ordering::Relaxed);
        let v = RWFRZ_LASTVAL.load(core::sync::atomic::Ordering::Relaxed);
        // neighbors through the first waiter we can find: +4/+8 disambiguate
        // the primitive (musl rwlock: waiter-count/shared-flag; musl mutex:
        // __m_type/__m_count — a type constant at +4 means this is NOT an
        // rwlock and the "readers" decode changes meaning entirely).
        let mm = RWFRZ_MM.load(core::sync::atomic::Ordering::Relaxed);
        let mut nb = (0u32, 0u32, 0u32, 0u32);
        for c in waiter_clusters(1) {
            if c.uaddr == uaddr && c.mm == mm {
                nb = (
                    unsafe { read_u32_via_task(c.sample, uaddr + 4) }.unwrap_or(0),
                    unsafe { read_u32_via_task(c.sample, uaddr + 8) }.unwrap_or(0),
                    unsafe { read_u32_via_task(c.sample, uaddr + 12) }.unwrap_or(0),
                    unsafe { read_u32_via_task(c.sample, uaddr + 16) }.unwrap_or(0),
                );
                break;
            }
        }
        crate::pr_warn!(
            "RWFRZ: FROZEN tgid {} uaddr={:#x} word={:#x} readers={} waiters_bit={} nb+4={:#x} nb+8={:#x} nb+12={:#x} nb+16={:#x} age {}s — full tgid dump",
            tgid,
            uaddr,
            v,
            v & 0x7fffffff,
            v >> 31,
            nb.0,
            nb.1,
            nb.2,
            nb.3,
            now.saturating_sub(RWFRZ_FIRST.load(core::sync::atomic::Ordering::Relaxed)) / hz
        );
        exit_ring_dump_tgid(tgid);
        let n = rwfz_dump_tgid(tgid, 32);
        crate::pr_warn!("RWFRZ: tgid {} enumerated {} tasks", tgid, n);
        rwfz_neighbor_dump(tgid, uaddr);
        // sc82: the freeze correlates with OTHER processes showing the
        // same 16-reader shape (sc81: tgid 716 & 791). Dump up to 2 more
        // of those tgid's in the same pass — one common holder pattern
        // would explain all of them; distinct stacks point at a shared
        // library-level bug instead.
        let mut extra = 0;
        for c in waiter_clusters(1) {
            if extra >= 2 {
                break;
            }
            if c.tgid == tgid || c.n < 1 {
                continue;
            }
            let v = match unsafe { read_u32_via_task(c.sample, c.uaddr) } {
                Some(v) => v,
                None => continue,
            };
            let r = v & 0x7fffffff;
            if v & 0x80000000 != 0 && r >= 8 && r < 0x40000000 {
                crate::pr_warn!(
                    "RWFRZ: EXTRA-FROZEN tgid {} uaddr={:#x} word={:#x} readers={} waiters={} — same-shape dump",
                    c.tgid,
                    c.uaddr,
                    v,
                    r,
                    c.n
                );
                exit_ring_dump_tgid(c.tgid);
                let en = rwfz_dump_tgid(c.tgid, 16);
                crate::pr_warn!("RWFRZ: EXTRA tgid {} enumerated {} tasks", c.tgid, en);
                rwfz_neighbor_dump(c.tgid, c.uaddr);
                extra += 1;
            }
        }
    }
}

/// sc89: dump the recursion-holder candidate. Session K (sc86): every
/// FROZEN dump's main thread parks on the SIBLING lock 0x94 below the
/// frozen word (same static object) while the frozen word claims 16
/// readers that exceed the process's live-thread count — a single
/// thread holding 16 recursive lock_shared scopes and then parking on
/// the second lock. This walks `tgid`'s tasks for a blocked futex whose
/// arg0 sits in [frozen-0x98, frozen-0x90] and dumps it with the
/// 64-slot exec-filtered window at a 24-frame budget — enough to carry
/// a 16-deep recursive chain plus its entry frames.
fn rwfz_neighbor_dump(tgid: u32, frozen_uaddr: usize) {
    #[cfg(feature = "x86_64")]
    const FUTEX_SYSNO: u64 = 202;
    #[cfg(feature = "riscv64")]
    const FUTEX_SYSNO: u64 = 98;
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    const FUTEX_SYSNO: u64 = 0;
    crate::sched::for_each_task(|t| {
        // SAFETY: same contract as rwfz_dump_tgid (pid-hash walk, reads
        // confined to the pt-regs entry frame).
        let tk = unsafe { &*t };
        if tk.tgid() as u32 != tgid {
            return;
        }
        // Stack-freed guard (KILLDUMP census race class): the pid-hash
        // walk can visit a task whose exit already freed its stack.
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        #[cfg(feature = "x86_64")]
        let (sys, a0) = {
            let regs = (top.wrapping_sub(
                crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::x86_64::pt_regs::PtRegs;
            // SAFETY: diagnostic read of this task's entry frame.
            unsafe { ((*regs).orig_rax, (*regs).rdi) }
        };
        #[cfg(feature = "riscv64")]
        let (sys, a0) = {
            let regs = (top.wrapping_sub(
                crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::riscv64::pt_regs::PtRegs;
            // SAFETY: same contract, riscv64.
            unsafe { ((*regs).a7, (*regs).a0) }
        };
        #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
        let (sys, a0) = (0u64, 0u64);
        if sys != FUTEX_SYSNO || a0 == 0 || a0 > frozen_uaddr as u64 {
            return;
        }
        let delta = frozen_uaddr - a0 as usize;
        if (0x90..=0x98).contains(&delta) {
            let comm = tk.comm();
            let name = core::str::from_utf8(&comm[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            crate::pr_warn!(
                "RWFRZ: NBR-PARK tgid {} tid {} comm='{}' parked on frozen-{:#x} (a0={:#x}) — recursion-holder candidate, deep stack",
                tgid,
                tk.pid(),
                name,
                delta,
                a0
            );
            // sc91: read the WORD the holder parks on (plus neighbors).
            // If it is a held pthread mutex the word IS the owner tid —
            // naming the true terminal blocker. If it is a shared_mutex
            // __state_ we see the writer bit. The frozen word itself is
            // 0x80000010-shaped, but this sibling lock's shape was never
            // captured (sc89/sc90 dumps only read the frozen word).
            let w0 = unsafe { read_u32_via_task(t, a0 as usize) }.unwrap_or(0);
            let wp4 = unsafe { read_u32_via_task(t, a0 as usize + 4) }.unwrap_or(0);
            let wm4 = unsafe { read_u32_via_task(t, a0 as usize - 4) }.unwrap_or(0);
            let wp8 = unsafe { read_u32_via_task(t, a0 as usize + 8) }.unwrap_or(0);
            crate::pr_warn!(
                "RWFRZ: NBR-WORD a0={:#x} word={:#x} +4={:#x} +8={:#x} -4={:#x}",
                a0,
                w0,
                wp4,
                wp8,
                wm4
            );
            // sc91: RAW stack-slot window — VMA metadata cannot identify
            // loader-mapped code frames (musl closes the fd AND the
            // offset bookkeeping is zeroed for merged LOAD mappings —
            // sc90 nbr dumps recovered only data-segment pointers).
            // Print the slot VALUES themselves, compactly; offline
            // analysis matches them against lib files (return addresses
            // sit right after `call` opcodes in exec LOADs).
            use core::fmt::Write;
            let sp = {
                // `top` is the guarded value from the closure head
                // (kernel_stack_top_checked) — never raw here.
                #[cfg(feature = "x86_64")]
                let regs = (top.wrapping_sub(
                    crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                )) as *const crate::arch::x86_64::pt_regs::PtRegs;
                #[cfg(feature = "riscv64")]
                let regs = (top.wrapping_sub(
                    crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
                )) as *const crate::arch::riscv64::pt_regs::PtRegs;
                // SAFETY: diagnostic read of this task's entry frame.
                #[cfg(feature = "x86_64")]
                {
                    unsafe { (*regs).rsp }
                }
                // SAFETY: same contract, riscv64.
                #[cfg(feature = "riscv64")]
                {
                    unsafe { (*regs).sp }
                }
                #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
                {
                    0u64
                }
            };
            let mut line = alloc::string::String::new();
            let mut n = 0u32;
            for i in 0..256u64 {
                let Some(v) = (unsafe { read_u32_via_task(t, (sp + 8 * i) as usize) })
                    .map(|lo| lo as u64)
                    .and_then(|lo| {
                        (unsafe { read_u32_via_task(t, (sp + 8 * i + 4) as usize) })
                            .map(|hi| lo | ((hi as u64) << 32))
                    })
                else {
                    continue;
                };
                if v >= 0x1000 && v <= 0x7fff_ffff_ffff {
                    let _ = write!(line, " {}:{}", i, v);
                    n += 1;
                    if n % 8 == 0 {
                        crate::pr_warn!("RWFRZ: NBR-RAW{}", line);
                        line.clear();
                    }
                }
            }
            if !line.is_empty() {
                crate::pr_warn!("RWFRZ: NBR-RAW{}", line);
            }
            // SAFETY: t is a live Task from the pid-hash walk.
            // sc90: 256-slot window (2 KiB) — the 16-deep recursive
            // chain sits ~1-2 KiB above rsp; 24 exec-file frames catch
            // repeated fileoffs at fixed deltas (the recursion cycle).
            // Emitted twice (nbr/nbr2): concurrent console traffic
            // splices long multi-line dumps mid-line (sc89 lost the
            // holder's chain between frame[2] and frame[17]); two
            // passes rarely lose the same line twice.
            unsafe { rwfz_stack_frames(t, "nbr", 24, 256) };
            unsafe { rwfz_stack_frames(t, "nbr2", 24, 256) };
        }
    });
}

/// sc82: one tgid's full park-stack dump (comm/state/syscall/rip + 8
/// byte-verified user-stack frames per thread). Pure diagnostics.
fn rwfz_dump_tgid(tgid: u32, max_tasks: usize) -> usize {
    let count_cell: core::cell::Cell<usize> = core::cell::Cell::new(0);
    crate::sched::for_each_task(|t| {
        // SAFETY: same contract as sleep_snapshot.
        let tk = unsafe { &*t };
        if tk.tgid() as u32 != tgid || count_cell.get() >= max_tasks {
            return;
        }
        count_cell.set(count_cell.get() + 1);
        // Stack-freed guard (KILLDUMP census race class): the pid-hash
        // walk can visit a task whose exit already freed its stack.
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        #[cfg(feature = "x86_64")]
        let (rip, sys, a0) = {
            let regs = (top.wrapping_sub(
                crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::x86_64::pt_regs::PtRegs;
            // SAFETY: diagnostic read of this task's entry frame.
            unsafe { ((*regs).rip, (*regs).orig_rax, (*regs).rdi) }
        };
        #[cfg(feature = "riscv64")]
        let (rip, sys, a0) = {
            let regs = (top.wrapping_sub(
                crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::riscv64::pt_regs::PtRegs;
            // SAFETY: same contract, riscv64.
            unsafe { ((*regs).epc, (*regs).a7, (*regs).a0) }
        };
        #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
        let (rip, sys, a0) = (0u64, 0u64, 0u64);
        let comm = tk.comm();
        let name = core::str::from_utf8(&comm[..])
            .unwrap_or("")
            .trim_end_matches('\0');
        crate::pr_warn!(
            "RWFRZ: tid {} comm='{}' state={:#x} sys={:#x} rip={:#x} a0={:#x}",
            tk.pid(),
            name,
            tk.state().bits(),
            sys,
            rip,
            a0
        );
        // SAFETY: t is a live Task from the pid-hash walk.
        unsafe { rwfz_stack_frames(t, "task", 8, 64) };
    });
    count_cell.get()
}
