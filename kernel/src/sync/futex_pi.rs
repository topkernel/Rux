//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
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
//! - CHAIN PROPAGATION (R6, reference rt_mutex_adjust_prio_chain):
//!   the boost/deboost of an owner must travel up the blocked-on
//!   chain — A holds C, B holds D and blocks on C, T blocks on D:
//!   boosting T boosts B (single edge), and B's NEW priority must
//!   re-rank B's waiter entry on C and boost A in turn. The walk
//!   (`pi_chain_walk`) mirrors the reference's step loop: one lock at
//!   a time, requeue the blocked owner's waiter entry with its live
//!   priority, re-evaluate the edge, follow to the next owner. Two
//!   Rux-native structural differences, both recorded in
//!   docs/architecture/design-divergences.md: (1) the waiter's boost
//!   input is evaluated LIVE from the task (the reference snapshots
//!   waiter->tree.prio and requeues inside the walk — our pool scan
//!   makes every edge derivation from current state anyway); (2) the
//!   walk holds at most ONE PI bucket at a time (never nested), so
//!   bucket-order deadlocks are impossible by construction where the
//!   reference needs the pi_lock/wait_lock trylock dance.
//! - KERNEL RT-MUTEXES (R6): SleepMutex instances constructed with
//!   `SleepMutex::new_rt` register PI states in the SAME pools with a
//!   FLAGS_KERNEL_RT key (uaddr = the mutex address, mm = 0). Their
//!   RT-class waiters join the priority chain, so a FIFO task blocked
//!   on the ext4 big lock boosts the lock's CFS owner, and if that
//!   owner is itself blocked on a user PI futex the boost keeps
//!   walking — one chain engine for both worlds, the reference's
//!   shared task->pi_blocked_on shape. Kernel keys never touch a user
//!   word: every word_read/word_cas on a PI state is guarded by
//!   `key_is_kernel` (the key's uaddr is a kernel static — writing it
//!   would corrupt kernel memory).

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
    let tr = crate::sched::edges::task_shared(task);
    if task.is_null() {
        return None;
    }
    let policy = crate::sched::edges::task_shared(task).policy();
    let rt = if matches!(policy, SchedPolicy::Fifo | SchedPolicy::Rr) {
        Some(crate::sched::edges::task_shared(task).rt_priority())
    } else {
        None
    };
    Some((rt, crate::sched::edges::task_shared(task).nice()))
}

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

fn word_read(uaddr: usize) -> Option<u32> {
    crate::sync::edges::word_read(uaddr)
}

/// Compare-and-swap the user word: writes `new` only when it reads
/// `expect`. Returns the observed pre-value; callers compare against
/// `expect` to learn whether the swap happened.
fn word_cas(uaddr: usize, expect: u32, new: u32) -> Option<u32> {
    crate::sync::edges::word_cas(uaddr, expect, new)
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
        let policy = crate::sched::edges::task_shared(task).policy();
        let rt = if matches!(policy, SchedPolicy::Fifo | SchedPolicy::Rr) {
            Some(crate::sched::edges::task_shared(task).rt_priority())
        } else {
            None
        };
        (crate::sched::edges::task_shared(task).nice(), rt)
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
    let cur_policy = crate::sched::edges::task_shared(task).policy();
    if cur_policy != sv.policy {
        crate::sched::sched::change_task_policy(task, sv.policy, sv.rt_prio);
        // change_task_policy does not restore the nice (it only moves
        // class/priority) — a boosted-then-converted owner returns to
        // its CFS weight here.
        crate::sched::sched::sched_renice_locked(crate::sched::edges::task_mut(task), sv.nice);
        return;
    }
    if matches!(sv.policy, SchedPolicy::Fifo | SchedPolicy::Rr) {
        if crate::sched::edges::task_shared(task).rt_priority() != sv.rt_prio {
            crate::sched::sched::change_task_policy(task, sv.policy, sv.rt_prio);
        }
        return;
    }
    if crate::sched::edges::task_shared(task).nice() != sv.nice {
        crate::sched::sched::sched_renice_locked(crate::sched::edges::task_mut(task), sv.nice);
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
                matches!(crate::sched::edges::task_shared(task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr)
                    && crate::sched::edges::task_shared(task).rt_priority() >= p.min(99)
            }
            Some((None, n)) => {
                !matches!(crate::sched::edges::task_shared(task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr)
                    && crate::sched::edges::task_shared(task).nice() <= n
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
                nice: crate::sched::edges::task_shared(task).nice(),
                policy: crate::sched::edges::task_shared(task).policy(),
                rt_prio: crate::sched::edges::task_shared(task).rt_priority(),
            });
        }
    }
    match rt_prio {
        Some(p) => {
            let p = p.min(99);
            if matches!(crate::sched::edges::task_shared(task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr) {
                if crate::sched::edges::task_shared(task).rt_priority() < p {
                    // Same class, higher priority: the class migration
                    // re-links the priority-array bucket (change_task_policy
                    // keeps the policy when it matches).
                    crate::sched::sched::change_task_policy(task, crate::sched::edges::task_shared(task).policy(), p);
                }
            } else {
                // CFS owner under an RT waiter: convert to FIFO at the
                // waiter's priority (the reference rt_mutex_setprio
                // does exactly this via __setscheduler).
                crate::sched::sched::change_task_policy(task, SchedPolicy::Fifo, p);
            }
        }
        None => {
            if !matches!(crate::sched::edges::task_shared(task).policy(), SchedPolicy::Fifo | SchedPolicy::Rr)
                && crate::sched::edges::task_shared(task).nice() > nice
            {
                crate::sched::sched::sched_renice_locked(crate::sched::edges::task_mut(task), nice);
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
        let task = crate::sched::sched::find_task_by_pid(old_pid);
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
        let next = crate::sched::sched::find_task_by_pid(next_pid);
        if next.is_null() || crate::sched::edges::task_shared(next).state().is_dead() {
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
        let cur = crate::sched::sched::find_task_by_pid(cur_pid);
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
    crate::sched::current().map(|t| t as *mut Task)
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
    use crate::syscall::errno::{EAGAIN, EFAULT, EOWNERDEAD};
    fn death_take(uaddr: usize, uval: u32, mytid: u32) -> i64 {
        use crate::syscall::errno::{EAGAIN, EFAULT};
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
    use crate::syscall::errno::{
        EAGAIN, EDEADLK, EFAULT, EINTR, EOWNERDEAD, ETIMEDOUT,
    };
    let Some(current) = my_task() else {
        return -(crate::syscall::errno::EFAULT as i64);
    };
    let mytid = crate::sched::edges::task_shared(current).pid() as u32;

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
            let t = crate::sched::sched::find_task_by_pid(tid as u32);
            if t.is_null() {
                false
            } else {
                !crate::sched::edges::task_shared(t).state().is_dead()
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
            if shared { crate::sync::futex::FLAGS_SHARED } else { 0 },
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
            walk_from = crate::sched::sched::find_task_by_pid(tid);

            // Park. C8 ordering: waiter state + INTERRUPTIBLE published
            // under the bucket lock, then drop, then schedule.
            crate::sched::edges::task_shared(current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
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
                crate::sched::get_current_pid(),
            )
        });
        crate::sched::schedule();
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
            crate::drivers::timer::get_jiffies() >= dl
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
                    walk_from = crate::sched::sched::find_task_by_pid(pi_state_owner(si));
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
    use crate::syscall::errno::{EAGAIN, EDEADLK, EFAULT, EOWNERDEAD};
    let Some(current) = my_task() else {
        return -(EFAULT as i64);
    };
    let mytid = crate::sched::edges::task_shared(current).pid() as u32;
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
    use crate::syscall::errno::{EAGAIN, EFAULT, EPERM, ESRCH};
    let Some(current) = my_task() else {
        return -(EFAULT as i64);
    };
    let mytid = crate::sched::edges::task_shared(current).pid() as u32;

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
        if shared { crate::sync::futex::FLAGS_SHARED } else { 0 },
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
    let wtid = crate::sched::edges::task_shared(wtask).pid() as u32;
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
pub fn pi_requeue_attach(
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
        let policy = crate::sched::edges::task_shared(task).policy();
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
                crate::sched::sched::find_task_by_pid(pi_state_owner(si));
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
                crate::sched::sched::find_task_by_pid(pi_state_owner(si));
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
                crate::sched::sched::find_task_by_pid(pi_state_owner(si));
        }
        pi_state_maybe_free(si);
        // R6: the dying waiter's removal DEBOOSTS the owner — propagate
        // up its chain (reference remove_waiter walk).
        pi_chain_walk(walk_from);
    }

    // 2. Owner side: died holding PI locks — hand each to its top
    //    waiter with OWNER_DIED latched.
    let dead_tid = crate::sched::edges::task_shared(task).pid() as u32;
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
                let wtid = crate::sched::edges::task_shared(wtask).pid() as u32;
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
