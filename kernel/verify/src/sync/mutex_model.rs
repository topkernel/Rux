//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Faithful extraction of the SleepMutex handoff state machine
//! (kernel/src/sync/mutex.rs, R84/R85 fairness + orphaned-handoff
//! recovery) for host-side model checking (S-1 pilot).
//!
//! # What this file is
//!
//! The kernel primitive's logic is split in two layers: (a) the pure
//! state transitions executed under the internal `state` spinlock, and
//! (b) the environment (wait queue park/wake, scheduler, jiffies). The
//! bug-dense part — the R84 waiter/token bookkeeping and the R85
//! handoff-epoch recovery — lives entirely in layer (a). This module
//! extracts layer (a) verbatim, with every environment touch turned
//! into a parameter:
//!
//!   - `crate::drivers::timer::get_jiffies()` -> `now: u64` parameter,
//!   - the wait-queue `wake_up_one()` outcome  -> `woke_one: bool` at
//!     the release call site (see `LoomMutex::release` in mutex_loom.rs),
//!   - `Self::ANTI_STARVE_JIFFIES`            -> `anti_starve: u64`
//!     parameter (the kernel constant is KERNEL_HZ * 5; harnesses may
//!     shrink it to make the override reachable in a bounded model).
//!
//! Every transition body below is textually synchronized with the
//! kernel source: `scripts/verify_sync_check.py` (mapping
//! `sync/mutex_model`) normalizes the environment hooks above plus the
//! spinlock guard lines and compares the bodies. If you change
//! kernel/src/sync/mutex.rs state transitions, this file must change
//! in the same commit (the sync check enforces it in CI).
//!
//! # Invariant table (INV-M*)
//!
//! What "correct" means for this state machine, as asserted by the
//! kani proofs (mutex_kani.rs), the loom interleaving scenarios
//! (mutex_loom.rs), and the fast unit tests (mutex_model_test.rs):
//!
//! - INV-M1 `locked => depth >= 1`: a held lock always has at least
//!   one unreleased recursion level (acquire sets depth=1, recursion
//!   increments, release decrements and clears `locked` at zero).
//! - INV-M2 `!locked => owner == 0`: a free lock has no owner. (The
//!   converse does NOT hold: an early-boot/IRQ-context caller with
//!   `pid == 0` may hold the lock through the ungated spinner path,
//!   so `locked && owner == 0` is legal — see lock()'s no-task
//!   degradation in the kernel.)
//! - INV-M3 `!locked => depth == 0`: same critical section as INV-M2.
//! - INV-M4 `owner != 0 => locked`: owner is only ever set together
//!   with `locked = true` and cleared together with it.
//! - INV-M5 token conservation: `waiters` equals the number of tasks
//!   whose waiter-loop local `counted` flag is true (parked tasks plus
//!   unspent wake tokens). Every increment happens exactly once per
//!   waiter at first registration; every decrement happens in the same
//!   critical section in which that waiter takes ownership (token
//!   spend, sole-waiter take, epoch take, anti-starve take) or spends
//!   a recursive acquire from inside the loop. Checked by the harness
//!   layer because it needs the per-task view the kernel keeps in
//!   waiter-loop locals.
//! - INV-M6 `waiters` never underflows: the unguarded `waiters -= 1`
//!   in `register_and_recheck` is only legal because INV-M5 holds
//!   (a counted waiter owns one unit of the count). Kani proves the
//!   arithmetic cannot underflow for every disciplined op sequence;
//!   a debug build would panic on underflow anyway.

/// Extracted `SleepMutexState` (kernel/src/sync/mutex.rs). Field names
/// and semantics are identical; `acquired_at` is kept even though the
/// harnesses never assert on it, so the transition bodies stay
/// textually in sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutexState {
    pub locked: bool,
    pub owner: u32,
    pub depth: u32,
    pub acquired_at: u64,
    pub waiters: u32,
    pub handoff_epoch: u64,
}

impl Default for MutexState {
    fn default() -> Self {
        Self::new()
    }
}

impl MutexState {
    /// Mirrors `SleepMutex::new`'s state initializer.
    pub const fn new() -> Self {
        Self {
            locked: false,
            owner: 0,
            depth: 0,
            acquired_at: 0,
            waiters: 0,
            handoff_epoch: 0,
        }
    }

    /// INV-M1..M4 check. Returns the violated invariant's name, or None.
    /// INV-M5/INV-M6 live at the harness layer (they need the task view).
    pub fn invariant_violation(&self) -> Option<&'static str> {
        if self.locked && self.depth == 0 {
            return Some("INV-M1 locked => depth >= 1");
        }
        if !self.locked && self.owner != 0 {
            return Some("INV-M2 !locked => owner == 0");
        }
        if !self.locked && self.depth != 0 {
            return Some("INV-M3 !locked => depth == 0");
        }
        if self.owner != 0 && !self.locked {
            return Some("INV-M4 owner != 0 => locked");
        }
        None
    }

    /// Try to take the lock / recurse into it, UNGATED: a free lock goes
    /// to whoever asks. Kernel call sites: the no-task / preempt-count
    /// degraded spinner loop in `lock()`. The recursion arm requires
    /// `locked` (owner == 0 also names the free state — pid-0 callers
    /// must not "recurse into" a free lock; found by the loom harness).
    pub fn try_acquire_or_recurse(&mut self, now: u64, pid: u32) -> bool {
        if !self.locked {
            self.locked = true;
            self.owner = pid;
            self.depth = 1;
            self.acquired_at = now;
            true
        } else if self.locked && self.owner == pid {
            self.depth += 1;
            true
        } else {
            false
        }
    }

    /// The acquire for fresh sleepable callers: identical to
    /// `try_acquire_or_recurse` except a FREE lock is only taken when
    /// no waiter is parked and no wake token is outstanding
    /// (`waiters == 0`) — the R84 fairness gate. The recursion arm
    /// requires `locked` (same owner == 0 alias; lock()'s adaptive
    /// fair-try runs before the can_sleep check, so pid-0 no-task
    /// callers reach this arm gated, and must be refused).
    pub fn try_acquire_or_recurse_fair(&mut self, now: u64, pid: u32) -> bool {
        if !self.locked && self.waiters == 0 {
            self.locked = true;
            self.owner = pid;
            self.depth = 1;
            self.acquired_at = now;
            true
        } else if self.locked && self.owner == pid {
            self.depth += 1;
            true
        } else {
            false
        }
    }

    /// Spend the release() handoff token: the woken waiter takes a FREE
    /// lock unconditionally and leaves the waiter count in the same
    /// critical section. A held lock or a lost race against another
    /// token holder returns false: the caller re-parks and keeps its
    /// count. The recursion arm requires `locked` (same alias).
    pub fn try_acquire_wake_token(&mut self, now: u64, pid: u32) -> bool {
        if self.locked && self.owner == pid {
            self.depth += 1;
            true
        } else if !self.locked {
            if self.waiters > 0 {
                self.waiters -= 1;
            }
            self.locked = true;
            self.owner = pid;
            self.depth = 1;
            self.acquired_at = now;
            true
        } else {
            false
        }
    }

    /// The registration critical section in `lock()`'s park loop (the
    /// kernel's R7-B6b re-check, fairness-aware): count ourselves once
    /// (`counted`), then take the free lock ONLY when we are the sole
    /// waiter, OR the handoff epoch moved since our snapshot (R85
    /// orphaned-handoff recovery), OR we starved past the anti-starve
    /// bound. `counted`/`first_park` are the waiter-loop locals, passed
    /// by reference because the kernel mutates its own locals here.
    /// Returns `take`.
    pub fn register_and_recheck(
        &mut self,
        now: u64,
        pid: u32,
        counted: &mut bool,
        my_epoch: u64,
        first_park: &mut u64,
        anti_starve: u64,
    ) -> bool {
        if !*counted {
            self.waiters += 1;
            *counted = true;
            *first_park = now;
        }
        let take = if self.locked && self.owner == pid {
            // Unreachable while parked (a counted waiter has not taken
            // the lock, so it cannot own it), kept so recursion is
            // honored on every path.
            self.waiters -= 1;
            *counted = false;
            self.depth += 1;
            true
        } else if !self.locked
            && (self.waiters == 1
                || self.handoff_epoch != my_epoch
                || now.wrapping_sub(*first_park) > anti_starve)
        {
            self.waiters -= 1;
            *counted = false;
            self.locked = true;
            self.owner = pid;
            self.depth = 1;
            self.acquired_at = now;
            true
        } else {
            false
        };
        take
    }

    /// The outermost-release state section of `release()`: drop one
    /// recursion level; the outermost drop frees the lock. The kernel
    /// does NOT check the owner here (the RAII guard is the discipline:
    /// one release per guard) — this extraction keeps that shape.
    pub fn release_outermost(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
            if self.depth == 0 {
                self.locked = false;
                self.owner = 0;
            }
        }
    }

    /// The orphaned-handoff epoch bump: called by release() when
    /// `wake_up_one()` woke nobody (empty queue) and the freed lock
    /// would otherwise sit stranded behind the fairness gate.
    pub fn note_dropped_handoff(&mut self) {
        if !self.locked && self.waiters > 0 {
            self.handoff_epoch += 1;
        }
    }

    /// The do_exit() force-release (`release_if_owner`): the dying
    /// owner's nested sections are abandoned wholesale. The kernel also
    /// logs when depth != 1; the log is environment, elided here.
    /// Returns whether the lock was force-released.
    pub fn release_if_owner(&mut self, pid: u32) -> bool {
        if self.locked && self.owner == pid {
            self.locked = false;
            self.owner = 0;
            self.depth = 0;
            true
        } else {
            false
        }
    }
}

/// Planted-bug variants (harness detection demos — NOT the kernel
/// protocol, deliberately NOT sync-checked). Each is the faithful
/// transition with one historical mistake re-introduced; the loom
/// scenarios (mutex_loom.rs) and the deterministic regression tests
/// (mutex_model_test.rs) demonstrate that the harness catches it.
pub mod planted {
    use super::MutexState;

    /// P1 (pre-R85, the b148 class): the registration re-check without
    /// the orphaned-handoff epoch term.
    pub fn register_and_recheck_no_epoch(
        st: &mut MutexState,
        now: u64,
        pid: u32,
        counted: &mut bool,
        my_epoch: u64,
        first_park: &mut u64,
        anti_starve: u64,
    ) -> bool {
        if !*counted {
            st.waiters += 1;
            *counted = true;
            *first_park = now;
        }
        let take = if st.locked && st.owner == pid {
            st.waiters -= 1;
            *counted = false;
            st.depth += 1;
            true
        } else if !st.locked
            && (st.waiters == 1 || now.wrapping_sub(*first_park) > anti_starve)
        {
            let _ = my_epoch; // the epoch term is the planted omission
            st.waiters -= 1;
            *counted = false;
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = now;
            true
        } else {
            false
        };
        take
    }

    /// P2: the token spend forgets to leave the count (`waiters -= 1`
    /// dropped) — every handed-off lock leaks one waiter unit.
    pub fn try_acquire_wake_token_no_decrement(
        st: &mut MutexState,
        now: u64,
        pid: u32,
    ) -> bool {
        if st.locked && st.owner == pid {
            st.depth += 1;
            true
        } else if !st.locked {
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = now;
            true
        } else {
            false
        }
    }

    /// P4: the re-check re-counts on every re-park (the `if !counted`
    /// guard dropped) — each park loop iteration inflates `waiters`.
    pub fn register_and_recheck_recount(
        st: &mut MutexState,
        now: u64,
        pid: u32,
        counted: &mut bool,
        my_epoch: u64,
        first_park: &mut u64,
        anti_starve: u64,
    ) -> bool {
        st.waiters += 1;
        *counted = true;
        *first_park = now;
        let take = if st.locked && st.owner == pid {
            st.waiters -= 1;
            *counted = false;
            st.depth += 1;
            true
        } else if !st.locked
            && (st.waiters == 1
                || st.handoff_epoch != my_epoch
                || now.wrapping_sub(*first_park) > anti_starve)
    {
            st.waiters -= 1;
            *counted = false;
            st.locked = true;
            st.owner = pid;
            st.depth = 1;
            st.acquired_at = now;
            true
        } else {
            false
        };
        take
    }
}
