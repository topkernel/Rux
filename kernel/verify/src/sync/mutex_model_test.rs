//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Fast deterministic tests over the extracted SleepMutex state machine
//! (mutex_model.rs). These run in the plain `cargo test` suite (no kani,
//! no loom) and pin the invariant table (INV-M1..M6) plus the canonical
//! R84/R85 scenarios. The loom harness (mutex_loom.rs) checks the same
//! invariants under exhaustive interleaving; kani (mutex_kani.rs) proves
//! them for all bounded op sequences.

use super::mutex_model::MutexState;

/// INV-M1..M4 hold on a fresh state.
#[test]
fn fresh_state_satisfies_invariants() {
    let st = MutexState::new();
    assert_eq!(st.invariant_violation(), None);
    assert_eq!(st.waiters, 0);
    assert_eq!(st.handoff_epoch, 0);
}

/// The fairness gate (R84): with a counted waiter parked, a fresh
/// sleepable caller is refused the FREE lock; the sole waiter's own
/// registration re-check takes it.
#[test]
fn fair_gate_blocks_fresh_caller_sole_waiter_takes() {
    let mut st = MutexState::new();
    assert!(st.try_acquire_or_recurse_fair(10, 1));
    st.release_outermost();
    st.waiters = 1; // a counted waiter exists (mid-loop)
    st.note_dropped_handoff(); // release found an empty queue
    assert_eq!(st.invariant_violation(), None);

    // A counted waiter exists (waiters == 1): fresh caller is gated...
    assert!(!st.try_acquire_or_recurse_fair(11, 2), "gate must refuse");
    // ...but the sole waiter's re-check takes the free lock.
    let mut counted = true;
    let mut first_park = 10u64;
    let take = st.register_and_recheck(11, 7, &mut counted, 0, &mut first_park, 500);
    assert!(take);
    assert!(!counted, "waiter must leave the count when it takes");
    assert_eq!(st.waiters, 0);
    assert_eq!(st.owner, 7);
    assert_eq!(st.invariant_violation(), None);
}

/// The orphaned-handoff epoch (R85): a release whose wake was dropped
/// (empty queue) bumps the epoch; a waiter whose epoch snapshot predates
/// the bump may take the FREE lock at its re-check even though other
/// waiters are counted (waiters > 1).
#[test]
fn epoch_take_recovers_dropped_handoff_with_multiple_waiters() {
    let mut st = MutexState::new();
    assert!(st.try_acquire_or_recurse(0, 1));
    // Two waiters counted (waiters == 2), both momentarily mid-loop.
    st.waiters = 2;
    let my_epoch = st.handoff_epoch;
    st.release_outermost(); // frees the lock; queue empty at wake time
    st.note_dropped_handoff(); // woke == 0 => epoch bump
    assert!(!st.locked);
    assert_eq!(st.handoff_epoch, my_epoch + 1);

    // Fresh callers stay gated (waiters > 0)...
    assert!(!st.try_acquire_or_recurse_fair(20, 9));
    // ...and a re-registering waiter with a stale epoch snapshot takes.
    let mut counted = true;
    let mut first_park = 0u64;
    let take = st.register_and_recheck(20, 7, &mut counted, my_epoch, &mut first_park, 500);
    assert!(take, "epoch term must recover the stranded free lock");
    assert_eq!(st.waiters, 1, "one counted waiter remains");
    assert_eq!(st.owner, 7);
    assert_eq!(st.invariant_violation(), None);
}

/// Without the epoch term the same shape strands: this documents WHY
/// the epoch exists (the pre-R85 b148 class). The sole-waiter term does
/// not fire (waiters == 2) and anti-starve has not elapsed.
#[test]
fn without_epoch_term_the_free_lock_strands_behind_the_gate() {
    let mut st = MutexState::new();
    assert!(st.try_acquire_or_recurse(0, 1));
    st.waiters = 2;
    let my_epoch = st.handoff_epoch;
    st.release_outermost();
    // Pre-R85 release(): no epoch bump on a dropped wake.
    assert!(!st.locked);
    // Gate refuses everyone...
    assert!(!st.try_acquire_or_recurse_fair(20, 9));
    // ...sole-waiter term does not apply (waiters == 2)...
    let mut counted = true;
    let mut first_park = 0u64;
    let take = st.register_and_recheck(20, 7, &mut counted, my_epoch, &mut first_park, u64::MAX);
    assert!(!take, "without the epoch term the free lock is stranded");
    // ...and the anti-starve override is the only remaining way out.
    let take =
        st.register_and_recheck(10_000, 7, &mut counted, my_epoch, &mut first_park, 500);
    assert!(take, "anti-starve override must eventually fire");
}

/// Token spend (R84): the woken waiter takes the FREE lock
/// unconditionally and leaves the count in the same section — even with
/// another waiter still counted.
#[test]
fn token_spend_takes_free_lock_and_leaves_count() {
    let mut st = MutexState::new();
    assert!(st.try_acquire_or_recurse(0, 1));
    st.waiters = 2;
    st.release_outermost();
    assert!(st.try_acquire_wake_token(5, 7));
    assert_eq!(st.waiters, 1, "token spend must decrement exactly one unit");
    assert_eq!(st.owner, 7);
    assert_eq!(st.invariant_violation(), None);
    // A spent token against a HELD lock loses and keeps its count.
    st.waiters = 1;
    assert!(!st.try_acquire_wake_token(6, 8));
    assert_eq!(st.waiters, 1);
}

/// Recursion: nested acquire by the owner bumps depth; nested releases
/// free only at the outermost (INV-M1).
#[test]
fn recursion_depth_and_outermost_release() {
    let mut st = MutexState::new();
    assert!(st.try_acquire_or_recurse_fair(0, 3));
    assert!(st.try_acquire_or_recurse_fair(1, 3));
    assert!(st.try_acquire_wake_token(2, 3));
    assert_eq!(st.depth, 3);
    assert_eq!(st.invariant_violation(), None);
    st.release_outermost();
    assert!(st.locked && st.depth == 2);
    st.release_outermost();
    assert!(st.locked && st.depth == 1);
    st.release_outermost();
    assert!(!st.locked && st.owner == 0 && st.depth == 0);
    assert_eq!(st.invariant_violation(), None);
}

/// release_if_owner (do_exit path): abandons nested sections wholesale
/// and hands one token down; INV-M1..M4 must still hold.
#[test]
fn release_if_owner_force_drop() {
    let mut st = MutexState::new();
    assert!(st.try_acquire_or_recurse(0, 5));
    assert!(st.try_acquire_or_recurse(0, 5));
    assert!(st.release_if_owner(5));
    assert!(!st.locked && st.owner == 0 && st.depth == 0);
    assert_eq!(st.invariant_violation(), None);
    // Not the owner: no-op.
    assert!(!st.release_if_owner(6));
    // Free lock with owner 0: pid-0 caller (early-boot spinner) may hold
    // it — locked && owner == 0 is legal (INV-M2 is one-directional).
    assert!(st.try_acquire_or_recurse(0, 0));
    assert!(st.locked && st.owner == 0);
    assert_eq!(st.invariant_violation(), None);
}

/// The anti-starve override: a waiter older than the bound takes the
/// FREE lock even with other waiters counted (bounded-latency
/// self-heal).
#[test]
fn anti_starve_override_takes_free_lock() {
    let mut st = MutexState::new();
    assert!(st.try_acquire_or_recurse(0, 1));
    st.waiters = 3;
    st.release_outermost();
    let mut counted = true;
    let mut first_park = 0u64;
    // Not yet starved (now - first_park <= bound): refused.
    assert!(!st.register_and_recheck(100, 7, &mut counted, 0, &mut first_park, 100));
    // Past the bound: takes even with other waiters queued.
    assert!(st.register_and_recheck(101, 7, &mut counted, 0, &mut first_park, 100));
    assert_eq!(st.waiters, 2);
    assert_eq!(st.owner, 7);
}

/// INV-M5/M6 shape: every decrement path is paired with the caller's
/// counted flag. Modeled exhaustively by kani; here the canonical
/// re-park sequence: a woken token holder that LOSES the race re-parks
/// and keeps exactly one unit.
#[test]
fn lost_token_race_keeps_exactly_one_count_unit() {
    let mut st = MutexState::new();
    assert!(st.try_acquire_or_recurse(0, 1));
    st.waiters = 1; // the loser is counted
    st.release_outermost();
    // Another (spinner) takes the free lock before the token holder.
    assert!(st.try_acquire_or_recurse(1, 2));
    // Token holder wakes and loses.
    assert!(!st.try_acquire_wake_token(2, 7));
    assert_eq!(st.waiters, 1, "loser keeps its count unit");
    // Re-park: register_and_recheck must NOT double-count.
    let mut counted = true;
    let mut first_park = 0u64;
    assert!(!st.register_and_recheck(3, 7, &mut counted, 0, &mut first_park, 100));
    assert_eq!(st.waiters, 1, "re-park must not re-count");
}

/// REAL FINDING (S-1 pilot, loom, first run): a pid-0 caller (the
/// no-task context — get_current_pid() returns 0 with no current task)
/// that takes the FAIR acquire path while the lock is momentarily FREE
/// behind a counted waiter (the handoff window) used to fall into the
/// `owner == pid` recursion arm — 0 == 0 against the free state's
/// owner == 0 — and "recurse into" a FREE lock: depth=1, locked=false
/// (INV-M3 broken), with a follow-on chain where the bogus guard's
/// release can free a lock another task genuinely holds (mutual
/// exclusion broken). Kernel reachability: lock() runs its adaptive
/// fair-try BEFORE the can_sleep check, so early-boot / secondary-hart
/// callers (the exact contexts the degrade path serves) hit this arm
/// whenever a real task is parked on the mutex at that moment.
/// The fix: the recursion arms require `locked` (owner == pid implies
/// locked for every real pid; only pid 0 aliased the free state).
#[test]
fn pid0_caller_must_not_recurse_into_a_free_lock() {
    let mut st = MutexState::new();
    // A real holder and a counted waiter create the window.
    assert!(st.try_acquire_or_recurse(0, 7));
    st.waiters = 1;
    st.release_outermost(); // free behind one counted waiter (gate shut)
    // The pid-0 no-task caller's fair try (lock()'s adaptive spin):
    // must be REFUSED by the gate, not "recursed" into the free lock.
    assert!(
        !st.try_acquire_or_recurse_fair(1, 0),
        "pid-0 caller must not recurse into a free lock (owner==0 alias)"
    );
    assert_eq!(st.depth, 0, "free lock must have no recursion levels");
    assert_eq!(st.invariant_violation(), None);
    // The wake-token arm on the free lock is a FRESH take by design
    // (a token holder spends its handoff): locked=true, waiters spent,
    // never a free-state recursion.
    assert!(st.try_acquire_wake_token(2, 0));
    assert!(st.locked && st.depth == 1 && st.owner == 0);
    assert_eq!(st.waiters, 0);
    assert_eq!(st.invariant_violation(), None);
    st.release_outermost();
    // The recursion arms still work for a caller that actually owns
    // the lock, and a genuinely free lock still goes to pid 0 through
    // the ungated degrade path when no waiter is counted.
    assert!(st.try_acquire_or_recurse(3, 0), "free lock goes to the spinner");
    assert!(st.try_acquire_or_recurse(3, 0), "real recursion still honored");
    assert_eq!(st.depth, 2);
}
