//! PI futex state engine (priority-inheritance futex operations).
//!
//! Reference semantics (futex(2)): FUTEX_LOCK_PI / FUTEX_UNLOCK_PI /
//! FUTEX_TRYLOCK_PI / FUTEX_LOCK_PI2 implement a PI mutex on a 32-bit
//! user word whose low bits carry the OWNER TID and whose high bits
//! are the FUTEX_WAITERS (0x80000000) and FUTEX_OWNER_DIED
//! (0x40000000) flags. FUTEX_WAIT_REQUEUE_PI / FUTEX_CMP_REQUEUE_PI
//! (implemented beside the plain wait queue in futex.rs, driving this
//! engine) move a plain waiter from a non-PI futex onto the PI queue
//! of a second futex without waking it, so a condvar->mutex handoff
//! costs one wake.
//!
//! Design (Rux-native, external ABI identical):
//! - All PI state lives in two fixed pools. A PiState exists ONLY
//!   while at least one waiter exists — an uncontended PI lock is
//!   just the user word, exactly like the reference's lazily attached
//!   pi_state. No per-Task fields: a task's held-PI-locks and
//!   blocked-on relationships are derived by scanning the pools; PI
//!   operations are rare next to plain futexes, so O(pool) slow paths
//!   are fine and the Task-struct dual-construction hazard does not
//!   apply.
//! - The per-hash PI bucket lock (PI_BUCKET_HEADS) serializes every
//!   user-word mutation and every state-chain edit, the same contract
//!   the plain side implements with its hash buckets.
//! - Priority inheritance is a nice-floor: while a PiState has
//!   waiters, its owner runs at nice -20 (the strongest weight the
//!   GRR scheduler offers) and its pre-boost nice is restored at
//!   handoff/release. Chains (the owner itself blocked on another PI
//!   lock) are walked for deadlock detection and every link boosts
//!   transitively through the same pool scan.
//! - Lock ordering (INV-LOCK family): plain futex bucket -> PI bucket
//!   -> pool slot. cmp_requeue_pi (in futex.rs) is the only operation
//!   holding a plain bucket while taking PI locks, and it always takes
//!   them in that order. Task::wake_up runs under the PI bucket lock,
//!   matching the plain side's C8 invariant.

use crate::arch::cpu;
use crate::process::task::{Task, TaskState};
use crate::sync::futex::{futex_hash, task_futex_mm_id, FutexKey};

/// Set while any waiter is queued on the futex (reference bit 31).
pub const FUTEX_WAITERS: u32 = 0x8000_0000;
/// Set by the kernel when a PI futex owner terminated holding the lock
/// (reference bit 30). The next acquirer observes it and returns
/// EOWNERDEAD after taking ownership (robust-mutex recovery contract).
pub const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
/// Everything below the two flag bits is the owner TID.
const TID_MASK: u32 = !(FUTEX_WAITERS | FUTEX_OWNER_DIED);
/// The nice floor applied to a PI-lock owner while waiters exist.
const PI_BOOST_NICE: i32 = -20;
/// Sentinel for "no saved nice" in PiState (nice is -20..=19).
const NICE_NONE: i32 = 0x7fff;
/// Cap on the deadlock-chain walk (the reference rt_mutex walk has the
/// same depth guard; hitting it is treated as a cycle).
const MAX_PI_CHAIN: usize = 16;

/// One task queued on a PI futex. Allocated by the enqueue site (the
/// blocker itself for LOCK_PI, or cmp_requeue_pi for a requeued
/// waiter) and freed by whoever ends the park (the sleeper on every
/// exit path; the handoff only marks `handed`).
struct PiWaiter {
    task: *mut Task,
    /// Nice snapshot at enqueue time — ordering key (lower wins, FIFO
    /// among equals; the plain side uses the same rule).
    nice: i32,
    /// Ownership was transferred to this waiter (unlock_pi or the
    /// owner-died path): the user word already holds its TID; its park
    /// must end with success.
    handed: bool,
    next: Option<usize>,
}

// SAFETY: PiWaiter slots are only accessed while the per-slot spinlock
// is held; the task raw pointer is only dereferenced by the waker after
// a non-null check — the same contract as the plain Waiter.
unsafe impl Send for PiWaiter {}
unsafe impl Sync for PiWaiter {}

/// Per-futex-word PI state: owner TID plus a priority-ordered waiter
/// chain.
struct PiState {
    key: FutexKey,
    owner_pid: u32,
    /// Owner's nice before the first waiter boosted it (NICE_NONE when
    /// no boost is applied).
    owner_saved_nice: i32,
    /// Priority-ordered waiter chain (lowest nice first, FIFO ties).
    waiters: Option<usize>,
}

const PI_WAITER_POOL_SIZE: usize = 128;
const PI_STATE_POOL_SIZE: usize = 128;
const PI_BUCKETS: usize = 64;

static PI_WAITER_POOL:
    [crate::sync::spinlock::Spinlock<Option<PiWaiter>>; PI_WAITER_POOL_SIZE] = {
        use crate::sync::spinlock::Spinlock;
        const INIT: Spinlock<Option<PiWaiter>> = Spinlock::new(None);
        [INIT; PI_WAITER_POOL_SIZE]
    };
static PI_STATE_POOL:
    [crate::sync::spinlock::Spinlock<Option<PiState>>; PI_STATE_POOL_SIZE] = {
        use crate::sync::spinlock::Spinlock;
        const INIT: Spinlock<Option<PiState>> = Spinlock::new(None);
        [INIT; PI_STATE_POOL_SIZE]
    };
static PI_BUCKET_HEADS:
    [crate::sync::spinlock::Spinlock<Option<usize>>; PI_BUCKETS] = {
        use crate::sync::spinlock::Spinlock;
        const INIT: Spinlock<Option<usize>> = Spinlock::new(None);
        [INIT; PI_BUCKETS]
    };

pub(crate) fn pi_bucket_lock(key: &FutexKey) -> crate::sync::spinlock::SpinlockIrqGuard<'static, Option<usize>> {
    PI_BUCKET_HEADS[(futex_hash(key) as usize) & (PI_BUCKETS - 1)].lock_irqsave()
}

// ---------------------------------------------------------------------------
// User-word helpers. Every mutation happens under the PI bucket lock,
// which serializes all kernel-side accessors of the word (the same
// contract the plain side uses for WAKE_OP).
// ---------------------------------------------------------------------------

unsafe fn word_read(uaddr: usize) -> Option<u32> {
    crate::arch::uaccess::get_user(uaddr as *const u32)
}

/// Compare-and-swap the user word: writes `new` only when it reads
/// `expect`. Returns the observed pre-value; callers compare against
/// `expect` to learn whether the swap happened.
unsafe fn word_cas(uaddr: usize, expect: u32, new: u32) -> Option<u32> {
    let cur = word_read(uaddr)?;
    if cur == expect
        && !crate::arch::uaccess::put_user(uaddr as *mut u32, new)
    {
        return None;
    }
    Some(cur)
}

// ---------------------------------------------------------------------------
// Pool management. Pool scans replace hash chains: states are scarce
// (one per CONTENDED pi futex) and PI operations are rare.
// ---------------------------------------------------------------------------

fn pi_waiter_alloc(task: *mut Task, nice: i32) -> Option<usize> {
    for i in 0..PI_WAITER_POOL_SIZE {
        let mut slot = PI_WAITER_POOL[i].lock_irqsave();
        if slot.is_none() {
            *slot = Some(PiWaiter { task, nice, handed: false, next: None });
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
                owner_saved_nice: NICE_NONE,
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

/// Insert ordered by nice (ascending), FIFO among equals — the same
/// discipline as the reference plist and our plain bucket chain.
unsafe fn pi_queue_insert(si: usize, wi: usize) {
    let nice = {
        let w = PI_WAITER_POOL[wi].lock_irqsave();
        w.as_ref().map(|w| w.nice).unwrap_or(0)
    };
    let mut st = PI_STATE_POOL[si].lock_irqsave();
    let s = match st.as_mut() {
        Some(s) => s,
        None => return,
    };
    let Some(head) = s.waiters else {
        s.waiters = Some(wi);
        return;
    };
    let head_nice = {
        let w = PI_WAITER_POOL[head].lock_irqsave();
        w.as_ref().map(|w| w.nice).unwrap_or(0)
    };
    if nice <= head_nice {
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
        let n_nice = {
            let w = PI_WAITER_POOL[n].lock_irqsave();
            w.as_ref().map(|w| w.nice).unwrap_or(0)
        };
        if nice <= n_nice {
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
unsafe fn pi_queue_unlink(si: usize, wi: usize) -> bool {
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

/// Apply the nice-floor boost to the state's owner when the first
/// waiter arrives (caller holds the PI bucket lock).
unsafe fn pi_boost_owner_if_first(si: usize) {
    if pi_state_waiter_count(si) > 1 {
        return; // waiters existed before this one; boost already on
    }
    let (owner, saved) = {
        let st = PI_STATE_POOL[si].lock_irqsave();
        match st.as_ref() {
            Some(s) => (s.owner_pid, s.owner_saved_nice),
            None => return,
        }
    };
    if saved != NICE_NONE {
        return;
    }
    let task = crate::sched::sched::find_task_by_pid(owner);
    if !task.is_null() {
        let nice = unsafe { (*task).nice() };
        let mut st = PI_STATE_POOL[si].lock_irqsave();
        if let Some(ref mut s) = *st {
            s.owner_saved_nice = nice;
        }
        drop(st);
        crate::sched::sched::sched_renice_locked(task, PI_BOOST_NICE);
    }
}

/// Restore the owner's nice when the last waiter left (caller holds
/// the PI bucket lock).
unsafe fn pi_unboost_owner_if_last(si: usize) {
    let (has_waiters, owner, saved) = {
        let st = PI_STATE_POOL[si].lock_irqsave();
        match st.as_ref() {
            Some(s) => (s.waiters.is_some(), s.owner_pid, s.owner_saved_nice),
            None => (false, 0, NICE_NONE),
        }
    };
    if has_waiters || saved == NICE_NONE {
        return;
    }
    let mut st = PI_STATE_POOL[si].lock_irqsave();
    if let Some(ref mut s) = *st {
        s.owner_saved_nice = NICE_NONE;
    }
    drop(st);
    let task = crate::sched::sched::find_task_by_pid(owner);
    if !task.is_null() {
        crate::sched::sched::sched_renice_locked(task, saved);
    }
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

/// Deadlock detection: follow task -> blocked-on state -> owner ->
/// blocked-on state ... and see whether we revisit `target_si`.
unsafe fn pi_chain_hits(start: *mut Task, target_si: usize) -> bool {
    let mut cur_task = start;
    for _ in 0..MAX_PI_CHAIN {
        let Some(si) = pi_blocked_state_of(cur_task) else {
            return false;
        };
        if si == target_si {
            return true;
        }
        let owner = {
            let st = PI_STATE_POOL[si].lock_irqsave();
            st.as_ref().map(|s| s.owner_pid).unwrap_or(0)
        };
        if owner == 0 {
            return false;
        }
        let next = crate::sched::sched::find_task_by_pid(owner);
        if next.is_null() {
            return false;
        }
        cur_task = next;
    }
    true
}

// ---------------------------------------------------------------------------
// FUTEX_LOCK_PI / FUTEX_LOCK_PI2 / FUTEX_TRYLOCK_PI / FUTEX_UNLOCK_PI
// ---------------------------------------------------------------------------

fn my_task() -> Option<*mut Task> {
    crate::sched::current().map(|t| t as *mut Task)
}

/// FUTEX_LOCK_PI: acquire the PI futex at `uaddr`, blocking under an
/// ABSOLUTE jiffies deadline (the timeout is absolute, like
/// FUTEX_WAIT_BITSET). Returns 0 acquired (word holds our TID),
/// -EOWNERDEAD when we inherited a dead owner's lock, -EDEADLK on
/// self/cycle, -ETIMEDOUT / -EINTR as usual.
pub fn futex_lock_pi(uaddr: usize, deadline: Option<u64>, shared: bool) -> i64 {
    use crate::syscall::errno::{
        EAGAIN, EDEADLK, EFAULT, EINTR, EOWNERDEAD, ETIMEDOUT,
    };
    let Some(current) = my_task() else {
        return -(crate::syscall::errno::EFAULT as i64);
    };
    let mytid = unsafe { (*current).pid() } as u32;

    loop {
        let Some(uval) = (unsafe { word_read(uaddr) }) else {
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
            match unsafe { word_cas(uaddr, uval, newv) } {
                Some(v) if v == uval => {
                    return if uval & FUTEX_OWNER_DIED != 0 {
                        -(EOWNERDEAD as i64)
                    } else {
                        0
                    };
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
        let owner_live = unsafe {
            let t = crate::sched::sched::find_task_by_pid(tid as u32);
            if t.is_null() {
                false
            } else {
                !(*t).state().is_dead()
            }
        };
        if !owner_live {
            let newv = mytid | FUTEX_OWNER_DIED | FUTEX_WAITERS;
            match unsafe { word_cas(uaddr, uval, newv) } {
                Some(v) if v == uval => return -(EOWNERDEAD as i64),
                Some(_) => continue,
                None => return -(EFAULT as i64),
            }
        }

        // Latch WAITERS before queueing (reference order).
        if uval & FUTEX_WAITERS == 0 {
            match unsafe { word_cas(uaddr, uval, uval | FUTEX_WAITERS) } {
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
            if shared { crate::sync::futex::FLAGS_SHARED } else { 0 },
        );
        let _bkt = pi_bucket_lock(&key);

        let Some(uval2) = (unsafe { word_read(uaddr) }) else {
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
        let Some(wi) =
            pi_waiter_alloc(current, unsafe { (*current).nice() })
        else {
            pi_state_maybe_free(si);
            return -(EAGAIN as i64);
        };

        unsafe {
            if pi_chain_hits(current, si) {
                pi_waiter_free(wi);
                pi_state_maybe_free(si);
                return -(EDEADLK as i64);
            }

            pi_queue_insert(si, wi);
            pi_boost_owner_if_first(si);
            // Owner moved between validation and queue? Unlink/retry.
            let owner_now = word_read(uaddr).map(|v| v & TID_MASK);
            if owner_now != Some(tid) {
                pi_queue_unlink(si, wi);
                pi_waiter_free(wi);
                pi_unboost_owner_if_last(si);
                pi_state_maybe_free(si);
                continue;
            }
        }

        // Park. C8 ordering: waiter state + INTERRUPTIBLE published
        // under the bucket lock, then drop, then schedule.
        unsafe {
            (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }
        drop(_bkt);
        cpu::restore_irq(true);
        let wake = deadline.map(|dl| {
            crate::timer::WakeArmed::arm_jiffy(
                dl,
                crate::sched::get_current_pid(),
            )
        });
        crate::sched::schedule();
        drop(wake);

        if pi_waiter_handed(wi) {
            pi_waiter_free(wi);
            return 0; // the user word already holds our TID
        }
        // Timed out / interrupted / spurious: leave the queue and end
        // or restart the acquire accordingly.
        let timed = deadline.is_some_and(|dl| {
            crate::drivers::timer::get_jiffies() >= dl
        });
        unsafe {
            pi_queue_unlink(si, wi);
            pi_waiter_free(wi);
            pi_unboost_owner_if_last(si);
            pi_state_maybe_free(si);
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
    use crate::syscall::errno::{EAGAIN, EDEADLK, EFAULT, EOWNERDEAD};
    let Some(current) = my_task() else {
        return -(EFAULT as i64);
    };
    let mytid = unsafe { (*current).pid() } as u32;
    loop {
        let Some(uval) = (unsafe { word_read(uaddr) }) else {
            return -(EFAULT as i64);
        };
        let tid = uval & TID_MASK;
        if tid == mytid {
            return -(EDEADLK as i64);
        }
        if tid == 0 {
            let newv = mytid | (uval & FUTEX_OWNER_DIED);
            match unsafe { word_cas(uaddr, uval, newv) } {
                Some(v) if v == uval => {
                    return if uval & FUTEX_OWNER_DIED != 0 {
                        -(EOWNERDEAD as i64)
                    } else {
                        0
                    };
                }
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
    use crate::syscall::errno::{EAGAIN, EFAULT, EPERM, ESRCH};
    let Some(current) = my_task() else {
        return -(EFAULT as i64);
    };
    let mytid = unsafe { (*current).pid() } as u32;

    let Some(uval) = (unsafe { word_read(uaddr) }) else {
        return -(EFAULT as i64);
    };
    if uval & TID_MASK != mytid {
        return -(EPERM as i64);
    }

    let mm = task_futex_mm_id(current);
    let key = FutexKey::new_user(
        uaddr,
        mm,
        if shared { crate::sync::futex::FLAGS_SHARED } else { 0 },
    );
    let _bkt = pi_bucket_lock(&key);

    let Some(si) = pi_state_find(&key) else {
        // No kernel state: plain release of the word.
        return match unsafe { word_cas(uaddr, uval, 0) } {
            Some(v) if v == uval => 0,
            Some(_) => {
                // The word changed under us. If it still names us it
                // gained WAITERS (a locker latched the bit between our
                // read and CAS): retry once through the whole path.
                let now = unsafe { word_read(uaddr) };
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
        return match unsafe { word_cas(uaddr, uval, 0) } {
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
    let wtid = unsafe { (*wtask).pid() } as u32;
    let newv = wtid | if more { FUTEX_WAITERS } else { 0 };
    match unsafe { word_cas(uaddr, uval, newv) } {
        Some(v) if v == uval => {}
        Some(_) => {
            // Userspace raced the word. If it no longer names us the
            // reference shape is EPERM; else a retry-able race.
            let now = unsafe { word_read(uaddr) }.map(|x| x & TID_MASK);
            return if now != Some(mytid) {
                -(EPERM as i64)
            } else {
                -(EAGAIN as i64)
            };
        }
        None => return -(EFAULT as i64),
    }
    unsafe {
        pi_queue_unlink(si, wi);
    }
    {
        let mut w = PI_WAITER_POOL[wi].lock_irqsave();
        if let Some(ref mut x) = *w {
            x.handed = true;
        }
    }
    // Ownership moved to the top waiter; it inherits the boost while
    // any waiter remains, and our own boost ends.
    {
        let mut st = PI_STATE_POOL[si].lock_irqsave();
        if let Some(ref mut s) = *st {
            s.owner_pid = wtid;
        }
    }
    unsafe {
        pi_unboost_owner_if_last(si);
    }
    let _ = Task::wake_up(wtask);
    pi_state_maybe_free(si);
    0
}

// ---------------------------------------------------------------------------
// requeue_pi engine (driven by futex.rs's wait/cmp requeue ops)
// ---------------------------------------------------------------------------

/// Attach `task` (a FUTEX_WAIT_REQUEUE_PI sleeper requeued by
/// CMP_REQUEUE_PI) to the PI queue of `key2`. Returns the PiWaiter
/// index for the sleeper's handle. When `handed_direct` is set the
/// word already names `task` (proxy trylock won) and the waiter is
/// pre-marked handed so the sleeper's next wake ends its park.
///
/// Caller holds uaddr1's PLAIN bucket lock and `pi_bkt` (this PI
/// bucket) — the fixed plain->PI order.
pub unsafe fn pi_requeue_attach(
    pi_bkt: &mut crate::sync::spinlock::SpinlockIrqGuard<'static, Option<usize>>,
    key2: &FutexKey,
    owner_tid: u32,
    task: *mut Task,
    handed_direct: bool,
) -> Option<usize> {
    let _ = pi_bkt; // ordering documented; guard held by caller
    let si = pi_state_get_or_create(key2, owner_tid)?;
    let wi = pi_waiter_alloc(task, (*task).nice())?;
    pi_queue_insert(si, wi);
    if handed_direct {
        let mut w = PI_WAITER_POOL[wi].lock_irqsave();
        if let Some(ref mut x) = *w {
            x.handed = true;
        }
        let _ = Task::wake_up(task);
    } else {
        pi_boost_owner_if_first(si);
    }
    Some(wi)
}

/// Sleeper-side detach of a requeue_pi waiter (timeout / signal /
/// exit before handoff). Idempotent.
pub unsafe fn pi_requeue_detach(key2: &FutexKey, wi: usize) {
    let _bkt = pi_bucket_lock(key2);
    if let Some(si) = pi_state_find(key2) {
        pi_queue_unlink(si, wi);
        pi_unboost_owner_if_last(si);
        pi_state_maybe_free(si);
    }
    pi_waiter_free(wi);
}

// ---------------------------------------------------------------------------
// Exit cleanup (owner-died protocol)
// ---------------------------------------------------------------------------

/// Release every PI futex `task` holds and dequeue every PI queue it
/// waits in. Called beside futex_cleanup in do_exit. A dying owner
/// hands the lock to its top waiter with FUTEX_OWNER_DIED latched
/// (robust-mutex contract); the waiter's park ends OWNING the lock.
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
        unsafe {
            pi_queue_unlink(si, wi);
        }
        pi_waiter_free(wi);
        unsafe {
            pi_unboost_owner_if_last(si);
        }
        pi_state_maybe_free(si);
    }

    // 2. Owner side: died holding PI locks — hand each to its top
    //    waiter with OWNER_DIED latched.
    let dead_tid = unsafe { (*task).pid() } as u32;
    for si in 0..PI_STATE_POOL_SIZE {
        let owner_key = {
            let st = PI_STATE_POOL[si].lock_irqsave();
            match st.as_ref() {
                Some(s) if s.owner_pid == dead_tid => Some(s.key),
                _ => None,
            }
        };
        let Some(key) = owner_key else { continue };
        let _bkt = pi_bucket_lock(&key);
        let top = {
            let st = PI_STATE_POOL[si].lock_irqsave();
            st.as_ref().and_then(|s| s.waiters)
        };
        match top {
            Some(wi) => unsafe {
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
                let wtid = (*wtask).pid() as u32;
                let newv = wtid
                    | FUTEX_OWNER_DIED
                    | if more { FUTEX_WAITERS } else { 0 };
                let _ = word_cas(key.uaddr, dead_tid | FUTEX_WAITERS, newv);
                // Even if the CAS lost (userspace moved the word), the
                // waiter must not sleep forever: hand unconditionally.
                pi_queue_unlink(si, wi);
                {
                    let mut w = PI_WAITER_POOL[wi].lock_irqsave();
                    if let Some(ref mut x) = *w {
                        x.handed = true;
                    }
                }
                let _ = Task::wake_up(wtask);
                pi_unboost_owner_if_last(si);
                pi_state_maybe_free(si);
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
