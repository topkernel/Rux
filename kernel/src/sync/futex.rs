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

/// Waiter list head for each bucket
static HASH_HEADS: [Spinlock<Option<usize>>; HASH_SIZE] = {
    const INIT: Spinlock<Option<usize>> = Spinlock::new(None);
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
            Task::wake_up(task);
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
            if n < 16 {
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
                    // SAFETY: t is the current task, in its own syscall.
                    unsafe { rwfz_stack_frames(t, "park", 12) };
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

/// One user-stack frame line resolved to its covering VMA (Session F's
/// byte-match symbolization method). SAFETY: diagnostic walk of `task`'s
/// tables; `task` live.
unsafe fn rwfz_stack_frames(task: *const Task, label: &str, max_frames: u64) {
    let tk = unsafe { &*task };
    let top = tk.kernel_stack_top();
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
    let Some(aspace) = tk.address_space() else { return };
    let vmas = aspace.vma_read();
    let vma_of = |v: u64| -> Option<(usize, usize, i32, usize, u64)> {
        let vm = vmas.find(crate::mm::page::VirtAddr::new(v as usize))?;
        let off = vm.offset() + (v as usize - vm.start().as_usize());
        Some((
            vm.start().as_usize(),
            vm.end().as_usize(),
            vm.file_fd(),
            off,
            vm.file_size(),
        ))
    };
    // The RIP itself first — the futex park site names the primitive
    // (musl __timedwait caller: rdlock/wrlock/condvar).
    if let Some((s, e, fd, off, fsz)) = vma_of(rip) {
        crate::pr_warn!(
            "RWFRZ:   {} rip={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} filesz={:#x}",
            label,
            rip,
            s,
            e,
            fd,
            off,
            fsz as usize
        );
    } else {
        crate::pr_warn!("RWFRZ:   {} rip={:#x} (no vma)", label, rip);
    }
    let read_u64_at = |va: u64| -> Option<u64> {
        if va & 0xfff > 0xff8 {
            return None;
        }
        let lo = unsafe { read_u32_via_task(task, va as usize)? };
        let hi = unsafe { read_u32_via_task(task, (va + 4) as usize)? };
        Some(lo as u64 | ((hi as u64) << 32))
    };
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
            .fdtable()
            .get_file(fd as usize)
            .map(|file| file.path())
            .unwrap_or_else(|| alloc::string::String::from("?"));
        cache.push((fd, p.clone()));
        p
    };
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let mut printed = 0u64;
    for i in 0..24u64 {
        if printed >= max_frames {
            break;
        }
        let Some(v) = read_u64_at(rsp + 8 * i) else { continue };
        if v < 0x1000 || v > 0x7fff_ffff_ffff {
            continue;
        }
        if let Some((s0, e0, fd, off, fsz)) = vma_of(v) {
            let bytes = read_u64_at(v).unwrap_or(0);
            let path = if fd >= 0 {
                path_of(fd, &mut path_cache)
            } else {
                alloc::string::String::new()
            };
            let _ = write!(
                s,
                "\nRWFRZ:   frame ustack[{}]={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} filesz={:#x} bytes={:#018x} path={}",
                i, v, s0, e0, fd, off, fsz as usize, bytes, path
            );
            printed += 1;
        }
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
    {
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
                extra += 1;
            }
        }
    }
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
        let top = tk.kernel_stack_top();
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
        unsafe { rwfz_stack_frames(t, "task", 8) };
    });
    count_cell.get()
}
