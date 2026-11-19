//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Sleeping-mutex (SleepMutex) protocol invariant tests.
//! Mirrors the state machine of: kernel/src/sync/mutex.rs
//!
//! The kernel primitive parks on a wait queue (schedule/timer machinery
//! that cannot run under std); this mirror models the part where the
//! bugs live — the state-transition predicates under `state.lock()`:
//!   - the fairness gate (fresh sleepable callers refused while
//!     `waiters > 0`),
//!   - the wake-token spend (woken waiter takes a free lock
//!     unconditionally, leaving the count in the same section),
//!   - the orphaned-handoff epoch recovery (a dropped wake must not
//!     strand a free lock behind the gate),
//!   - recursion (owner pid + depth).

use proptest::prelude::*;

// Simplified mirror of SleepMutexState + the acquire predicates.
pub struct SleepMutexModel {
    locked: bool,
    owner: u32,
    depth: u32,
    waiters: u32,
    handoff_epoch: u64,
}

pub const ACQUIRE_REFUSED: Option<AcquireOutcome> = None;

#[derive(Debug, PartialEq)]
pub struct AcquireOutcome {
    pub owner: u32,
    pub depth: u32,
}

impl SleepMutexModel {
    pub fn new() -> Self {
        Self { locked: false, owner: 0, depth: 0, waiters: 0, handoff_epoch: 0 }
    }

    /// try_acquire_or_recurse_fair — the acquire for fresh sleepable
    /// callers: a free lock is taken ONLY when no waiter is parked and
    /// no wake token is outstanding.
    pub fn try_acquire_or_recurse_fair(&mut self, pid: u32) -> Option<AcquireOutcome> {
        if !self.locked && self.waiters == 0 {
            self.locked = true;
            self.owner = pid;
            self.depth = 1;
            Some(AcquireOutcome { owner: pid, depth: 1 })
        } else if self.owner == pid {
            self.depth += 1;
            Some(AcquireOutcome { owner: pid, depth: self.depth })
        } else {
            ACQUIRE_REFUSED
        }
    }

    /// try_acquire_wake_token — the woken waiter spends its handoff
    /// token: unconditional take of a FREE lock, waiter count dropped
    /// in the same critical section.
    pub fn try_acquire_wake_token(&mut self, pid: u32) -> Option<AcquireOutcome> {
        if self.owner == pid {
            self.depth += 1;
            Some(AcquireOutcome { owner: pid, depth: self.depth })
        } else if !self.locked {
            if self.waiters > 0 {
                self.waiters -= 1;
            }
            self.locked = true;
            self.owner = pid;
            self.depth = 1;
            Some(AcquireOutcome { owner: pid, depth: 1 })
        } else {
            ACQUIRE_REFUSED
        }
    }

    /// The registration re-check take predicate: a counted waiter may
    /// take a free lock only when it is the sole waiter (`waiters == 1`),
    /// OR the handoff epoch moved since its registration snapshot
    /// (orphaned handoff recovery), OR it starved past the anti-starve
    /// bound (modeled here as a boolean).
    pub fn registration_take(
        &mut self,
        pid: u32,
        my_epoch: u64,
        anti_starved: bool,
    ) -> Option<AcquireOutcome> {
        let now_epoch = self.handoff_epoch;
        if self.owner == pid {
            if self.waiters > 0 {
                self.waiters -= 1;
            }
            self.depth += 1;
            return Some(AcquireOutcome { owner: pid, depth: self.depth });
        }
        if !self.locked
            && (self.waiters == 1 || now_epoch != my_epoch || anti_starved)
        {
            if self.waiters > 0 {
                self.waiters -= 1;
            }
            self.locked = true;
            self.owner = pid;
            self.depth = 1;
            return Some(AcquireOutcome { owner: pid, depth: 1 });
        }
        ACQUIRE_REFUSED
    }

    /// release() — outermost drop frees the lock; a dropped handoff
    /// (empty queue at wake time, modeled by `woke_one: false`) bumps
    /// the epoch so the first re-registering waiter recovers the free
    /// lock.
    pub fn release(&mut self, pid: u32, woke_one: bool) {
        if self.depth > 0 && self.owner == pid {
            self.depth -= 1;
            if self.depth == 0 {
                self.locked = false;
                self.owner = 0;
            }
        }
        if !woke_one && !self.locked && self.waiters > 0 {
            self.handoff_epoch += 1;
        }
    }
}

proptest! {
    #[test]
    fn mutual_exclusion_two_holders_never(
        a in 1u32..8, b in 1u32..8,
    ) {
        let mut m = SleepMutexModel::new();
        let ga = m.try_acquire_or_recurse_fair(a).expect("uncontended take");
        assert_eq!(ga.owner, a);
        if a != b {
            // Second task is refused on the fast path (lock held, not owner).
            prop_assert!(m.try_acquire_or_recurse_fair(b).is_none());
            // ...and cannot take via the token path or the registration
            // re-check while the lock is held.
            prop_assert!(m.try_acquire_wake_token(b).is_none());
            m.waiters = 1; // model: b registered
            prop_assert!(m.registration_take(b, 0, false).is_none());
            m.waiters = 0;
        }
    }

    #[test]
    fn gate_blocks_barging_when_waiters_queued(
        holder in 1u32..8, barger in 1u32..8,
    ) {
        prop_assume!(holder != barger);
        let mut m = SleepMutexModel::new();
        m.try_acquire_or_recurse_fair(holder).unwrap();
        m.release(holder, true);
        // One waiter is counted (parked); a fresh caller must be refused
        // the now-free lock by the gate.
        m.waiters = 1;
        prop_assert!(m.try_acquire_or_recurse_fair(barger).is_none());
        // The sole waiter's own registration re-check takes it.
        prop_assert!(m.registration_take(barger, m.handoff_epoch, false).is_some());
        assert_eq!(m.owner, barger);
        assert_eq!(m.waiters, 0);
    }

    #[test]
    fn token_spend_takes_free_lock_unconditionally(
        waiter in 1u32..8, other in 1u32..8,
    ) {
        prop_assume!(waiter != other);
        let mut m = SleepMutexModel::new();
        let _ = m.try_acquire_or_recurse_fair(other).unwrap();
        m.release(other, true);
        m.waiters = 2; // two counted waiters, woken one holds a token
        // The token holder wins even though another waiter is counted.
        let g = m.try_acquire_wake_token(waiter).expect("token must spend");
        assert_eq!(g.owner, waiter);
        assert_eq!(m.waiters, 1, "token spend must leave the count in-section");
        assert!(m.locked);
    }

    #[test]
    fn epoch_recovers_dropped_handoff(
        waiter in 1u32..8, holder in 1u32..8,
    ) {
        prop_assume!(waiter != holder);
        let mut m = SleepMutexModel::new();
        m.try_acquire_or_recurse_fair(holder).unwrap();
        // Waiter registers, snapshots the epoch, then sleeps.
        m.waiters = 1;
        let my_epoch = m.handoff_epoch;
        m.release(holder, false); // wake dropped: queue momentarily empty
        assert!(!m.locked, "lock must be free after release");
        // Gate still bars fresh callers (waiters > 0)...
        prop_assert!(m.try_acquire_or_recurse_fair(99).is_none());
        // ...but the epoch bump lets the mid-loop waiter take it.
        let g = m.registration_take(waiter, my_epoch, false)
            .expect("epoch must recover the stranded lock");
        assert_eq!(g.owner, waiter);
        assert_eq!(m.waiters, 0);
    }

    #[test]
    fn recursion_depth_and_outermost_release(pid in 1u32..8, n in 1u32..5) {
        let mut m = SleepMutexModel::new();
        m.try_acquire_or_recurse_fair(pid).unwrap();
        for expected_depth in 2..=(n + 1) {
            let g = m.try_acquire_or_recurse_fair(pid).unwrap();
            assert_eq!(g.depth, expected_depth);
        }
        // Nested releases keep the lock held until the outermost.
        for _ in 0..n {
            m.release(pid, true);
            assert!(m.locked, "nested release must keep the lock held");
        }
        m.release(pid, true);
        assert!(!m.locked, "outermost release must free the lock");
        assert_eq!(m.owner, 0);
    }

    #[test]
    fn release_by_non_owner_is_inert(
        holder in 1u32..8, stranger in 1u32..8,
    ) {
        prop_assume!(holder != stranger);
        let mut m = SleepMutexModel::new();
        m.try_acquire_or_recurse_fair(holder).unwrap();
        m.release(stranger, true);
        assert!(m.locked, "stranger release must not free the lock");
        assert_eq!(m.owner, holder);
    }
}
