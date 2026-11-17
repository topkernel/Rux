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
//! - Priority inheritance follows the reference rt_mutex_setprio
//!   shape (RT-2X item 7): the boost is driven by the TOP waiter's
//!   scheduling class. An RT (FIFO/RR) top waiter raises the owner's
//!   rt_priority to max(owner, waiter) — or converts a CFS owner to
//!   SCHED_FIFO at the waiter's priority — via the GRQ-locked
//!   class/priority migration (change_task_policy, which re-links the
//!   priority-array bucket). A CFS top waiter applies the old
//!   nice-floor (owner's nice pulled down to the waiter's). The
//!   owner's saved (policy, rt_priority, nice) is restored at
//!   handoff/release; ownership handoff re-arms the boost on the
//!   incoming owner while waiters remain. The nice-only floor this
//!   module shipped first could NOT break an RT inversion (a FIFO
//!   medium-prio spinner starves a FIFO low-prio lock holder forever
//!   — pi_stress wedged exactly there: run/RT-2X.md). Chains (the
//!   owner itself blocked on another PI lock) are walked for deadlock
//!   detection and every link boosts transitively through the same
//!   pool scan.
//! - Lock ordering (INV-LOCK family): plain futex bucket -> PI bucket
//!   -> pool slot. cmp_requeue_pi (in futex.rs) is the only operation
//!   holding a plain bucket while taking PI locks, and it always takes
//!   them in that order. Task::wake_up runs under the PI bucket lock,
//!   matching the plain side's C8 invariant.

use crate::arch::cpu;
use crate::process::task::{SchedPolicy, Task, TaskState};
use crate::sync::futex::{futex_hash, task_futex_mm_id, FutexKey};

/// Set while any waiter is queued on the futex (reference bit 31).
pub const FUTEX_WAITERS: u32 = 0x8000_0000;
/// Set by the kernel when a PI futex owner terminated holding the lock
/// (reference bit 30). The next acquirer observes it and returns
/// EOWNERDEAD after taking ownership (robust-mutex recovery contract).
pub const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
/// Everything below the two flag bits is the owner TID.
const TID_MASK: u32 = !(FUTEX_WAITERS | FUTEX_OWNER_DIED);
/// Cap on the deadlock-chain walk (the reference rt_mutex walk has the
/// same depth guard; hitting it is treated as a cycle).
const MAX_PI_CHAIN: usize = 16;

/// One task queued on a PI futex. Allocated by the enqueue site (the
/// blocker itself for LOCK_PI, or cmp_requeue_pi for a requeued
/// waiter) and freed by whoever ends the park (the sleeper on every
/// exit path; the handoff only marks `handed`).
struct PiWaiter {
    task: *mut Task,
    /// Nice snapshot at enqueue time — the CFS ordering key (lower
    /// wins, FIFO among equals; the plain side uses the same rule).
    nice: i32,
    /// RT priority snapshot at enqueue time: Some(p) for SCHED_FIFO /
    /// SCHED_RR waiters, None for CFS-class waiters. The RT-PI key
    /// (RT-2X item 7): RT waiters order before every CFS waiter and
    /// the top waiter's rt_priority is what an RT owner inherits.
    rt_prio: Option<u32>,
    /// Ownership was transferred to this waiter (unlock_pi or the
    /// owner-died path): the user word already holds its TID; its park
    /// must end with success.
    handed: bool,
    next: Option<usize>,
}

/// The owner's pre-boost scheduling configuration, restored when the
/// boost ends (reference: rt_mutex_setprio's saved prio/policy).
#[derive(Clone, Copy)]
struct SavedSched {
    nice: i32,
    policy: SchedPolicy,
    rt_prio: u32,
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

/// Per-futex-word PI state: owner TID plus a priority-ordered waiter
/// chain.
struct PiState {
    key: FutexKey,
    owner_pid: u32,
    /// Owner's pre-boost (nice, policy, rt_priority) while a boost is
    /// applied; None when the owner runs at its own configuration.
    /// RT-2X item 7: an RT boost changes policy/rt_priority too, so the
    /// saved triple replaces the old nice-only slot.
    owner_saved: Option<SavedSched>,
    /// Priority-ordered waiter chain (RT first by rt_priority desc,
    /// then CFS by nice asc; FIFO among equals).
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

fn pi_waiter_alloc(task: *mut Task) -> Option<usize> {
    // Snapshot the scheduling configuration under the pool scan — the
    // waiter's class/priority cannot change while it is blocked, so the
    // enqueue-time snapshot is stable for the chain's lifetime.
    let (nice, rt_prio) = unsafe {
        let policy = (*task).policy();
        let rt = if matches!(policy, SchedPolicy::Fifo | SchedPolicy::Rr) {
            Some((*task).rt_priority())
        } else {
            None
        };
        ((*task).nice(), rt)
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
unsafe fn pi_queue_insert(si: usize, wi: usize) {
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
unsafe fn pi_restore_saved(task: *mut Task, sv: &SavedSched) {
    let cur_policy = (*task).policy();
    if cur_policy != sv.policy {
        crate::sched::sched::change_task_policy(task, sv.policy, sv.rt_prio);
        // change_task_policy does not restore the nice (it only moves
        // class/priority) — a boosted-then-converted owner returns to
        // its CFS weight here.
        crate::sched::sched::sched_renice_locked(task, sv.nice);
        return;
    }
    if matches!(sv.policy, SchedPolicy::Fifo | SchedPolicy::Rr) {
        if (*task).rt_priority() != sv.rt_prio {
            crate::sched::sched::change_task_policy(task, sv.policy, sv.rt_prio);
        }
        return;
    }
    if (*task).nice() != sv.nice {
        crate::sched::sched::sched_renice_locked(task, sv.nice);
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
unsafe fn pi_adjust_owner_boost(si: usize) {
    let (owner_pid, head, saved) = {
        let st = PI_STATE_POOL[si].lock_irqsave();
        match st.as_ref() {
            Some(s) => (s.owner_pid, s.waiters, s.owner_saved),
            None => return,
        }
    };
    let task = crate::sched::sched::find_task_by_pid(owner_pid);
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

    // The desired boost, derived from the top waiter's class.
    let desired = head.map(|wi| {
        let w = PI_WAITER_POOL[wi].lock_irqsave();
        w.as_ref().map(|w| (w.rt_prio, w.nice)).unwrap_or((None, 0))
    });

    // Does the owner's LIVE configuration already equal the desired
    // boost? (Avoids restore+re-apply churn on every chain edit.)
    let already = |desired: Option<(Option<u32>, i32)>| -> bool {
        match desired {
            None => saved.is_none(),
            Some((Some(p), _)) => {
                matches!((*task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr)
                    && (*task).rt_priority() >= p.min(99)
            }
            Some((None, n)) => {
                !matches!((*task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr)
                    && (*task).nice() <= n
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
                nice: (*task).nice(),
                policy: (*task).policy(),
                rt_prio: (*task).rt_priority(),
            });
        }
    }
    match rt_prio {
        Some(p) => {
            let p = p.min(99);
            if matches!((*task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr) {
                if (*task).rt_priority() < p {
                    // Same class, higher priority: the class migration
                    // re-links the priority-array bucket (change_task_policy
                    // keeps the policy when it matches).
                    crate::sched::sched::change_task_policy(task, (*task).policy(), p);
                }
            } else {
                // CFS owner under an RT waiter: convert to FIFO at the
                // waiter's priority (the reference rt_mutex_setprio
                // does exactly this via __setscheduler).
                crate::sched::sched::change_task_policy(task, SchedPolicy::Fifo, p);
            }
        }
        None => {
            if !matches!((*task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr)
                && (*task).nice() > nice
            {
                crate::sched::sched::sched_renice_locked(task, nice);
            }
        }
    }
}

/// Ownership-handoff boost routing: the outgoing owner's boost ends
/// (its saved configuration is restored), the incoming owner's is
/// armed from the remaining chain. Called after `owner_pid` was
/// re-pointed (caller holds the PI bucket lock).
unsafe fn pi_owner_handoff(si: usize, old_pid: u32) {
    let saved = {
        let mut st = PI_STATE_POOL[si].lock_irqsave();
        match st.as_mut() {
            Some(s) => s.owner_saved.take(),
            None => return,
        }
    };
    if let Some(sv) = saved {
        let task = crate::sched::sched::find_task_by_pid(old_pid);
        if !task.is_null() {
            pi_restore_saved(task, &sv);
        }
    }
    pi_adjust_owner_boost(si);
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
        let Some(wi) = pi_waiter_alloc(current) else {
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
        // or restart the acquire accordingly. The bucket lock is
        // RE-TAKEN for the unlink (RT-2X): the park dropped it, and an
        // unlock_pi handoff racing this removal could otherwise mark
        // `handed` on a freed (possibly reused) waiter slot and hand
        // the word to a task that already left.
        let timed = deadline.is_some_and(|dl| {
            crate::drivers::timer::get_jiffies() >= dl
        });
        {
            let _bkt2 = pi_bucket_lock(&key);
            unsafe {
                pi_queue_unlink(si, wi);
                pi_waiter_free(wi);
                pi_adjust_owner_boost(si);
                pi_state_maybe_free(si);
            }
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
    unsafe {
        pi_owner_handoff(si, mytid);
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
pub unsafe fn pi_requeue_detach(key2: &FutexKey, wi: usize) {
    let _bkt = pi_bucket_lock(key2);
    if let Some(si) = pi_state_find(key2) {
        pi_queue_unlink(si, wi);
        pi_adjust_owner_boost(si);
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
            pi_adjust_owner_boost(si);
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
