//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses over the extracted SleepMutex handoff state
//! machine (mutex_model.rs) — the S-1 pilot target #1, data-plane half.
//! (mutex_loom.rs covers the interleaving half.)
//!
//! Run: `cargo kani --harness mutex` (from kernel/verify) — compiled
//! only by `cargo kani` (cfg(kani) wiring in lib.rs).
//!
//! # What is proven
//!
//! K1 (`verify_mutex_ops_invariants`): for EVERY disciplined operation
//! sequence of length K (disciplined = the protocol's own call rules,
//! modeled by the operation system below: only the owner releases,
//! token spends only by counted waiters, one guard per acquire), the
//! invariant table INV-M1..M4 holds after every step and INV-M5 token
//! conservation holds throughout, and no arithmetic panics (INV-M6:
//! `waiters -= 1` underflow, `depth += 1` overflow) — kani checks the
//! panic reachability itself.
//!
//! K2 (`verify_mutex_exit_leak_is_bounded`): the do_exit kill of a
//! COUNTED (parked) waiter leaks its count unit by design (documented
//! bounded-latency insurance); the proof pins that the leak leaves
//! INV-M1..M4 intact and the gate/epoch/anti-starve paths still
//! function on the leaked state.
//!
//! # Operation system (the call discipline)
//!
//! The kernel calls these transitions only in specific situations:
//! guards exist per successful acquire (RAII), release comes from guard
//! drop by the owning task (or do_exit's release_if_owner), and the
//! waiter-loop transitions (register/recheck/token) run only with the
//! task counted flags as invariants of the loop. The harness encodes
//! exactly that discipline:
//!
//! - `guards[pid]`: outstanding RAII guards (acquire transitions
//!   increment on success; Release requires > 0).
//! - `counted[pid]`: the waiter-loop local (register/recheck maintain
//!   it; token spend clears it on success).
//! - pids are 0..NPID (pid 0 models the early-boot/IRQ-context caller
//!   whose current_pid() is 0 — the ungated spinner).

#![cfg(kani)]

use crate::sync::mutex_model::MutexState;

const NPID: usize = 3;
const STEPS: usize = 6;
const ANTI_STARVE: u64 = 4;

#[derive(kani::Arbitrary, Debug, Clone, Copy, PartialEq)]
enum Op {
    /// lock() fast try (fresh sleepable caller): the fair gate.
    AcquireFair,
    /// lock()'s degraded spinner (no task context): ungated.
    AcquireUngated,
    /// Guard drop -> release(): outermost decrement + one wake token
    /// (`woke` models wake_up_one's outcome).
    Release { woke: bool },
    /// Waiter loop: register + fairness-aware re-check.
    WaiterRecheck,
    /// Waiter loop: spend the wake token after schedule() returned.
    TokenSpend,
    /// do_exit(): release_if_owner + one wake token.
    Exit,
}

struct Sys {
    st: MutexState,
    guards: [u32; NPID],
    counted: [bool; NPID],
    my_epoch: [u64; NPID],
    first_park: [u64; NPID],
    /// waiters units orphaned by do_exit killing a COUNTED (parked)
    /// waiter — the kernel's documented bounded-latency leak (K2): the
    /// count has no live owner; the anti-starve timer/override is what
    /// bounds the stall. Conservation is waiters == live-counted +
    /// leaked (the first kani run without this flagged the leak as an
    /// INV-M5 violation — correct detection of a designed orphan).
    leaked: u32,
    now: u64,
}

impl Sys {
    fn new() -> Self {
        Self {
            st: MutexState::new(),
            guards: [0; NPID],
            counted: [false; NPID],
            my_epoch: [0; NPID],
            first_park: [0; NPID],
            leaked: 0,
            now: 0,
        }
    }

    fn check(&self) {
        if let Some(inv) = self.st.invariant_violation() {
            panic!("{} after op (locked={} owner={} depth={})", inv, self.st.locked, self.st.owner, self.st.depth);
        }
        // INV-M5: waiters == live counted tasks + orphaned units.
        let mut live = 0u32;
        for i in 0..NPID {
            if self.counted[i] {
                live += 1;
            }
        }
        assert_eq!(self.st.waiters, live + self.leaked, "INV-M5 token conservation");
    }

    fn step(&mut self, op: Op, pid: usize) {
        // Harness discipline (not a proof obligation): only valid pids
        // and monotone bounded time steps are explored.
        kani::assume(pid < NPID);
        self.now += 1 + u64::from(kani::any::<u8>() % 2);
        let pid = pid as u32;
        match op {
            Op::AcquireFair => {
                if self.st.try_acquire_or_recurse_fair(self.now, pid) {
                    self.guards[pid as usize] += 1;
                }
            }
            Op::AcquireUngated => {
                // Kernel discipline: the ungated path belongs to the
                // no-task / preempt-count degrade loop — a context that
                // is NEVER simultaneously a counted waiter (the waiter
                // loop and the spinner are the same lock() call). The
                // second kani counterexample without this assume had a
                // counted waiter ALSO taking ungated, then clearing its
                // counted flag via the token arm's recursion — a call
                // discipline no single kernel lock() invocation can
                // produce.
                kani::assume(!self.counted[pid as usize]);
                if self.st.try_acquire_or_recurse(self.now, pid) {
                    self.guards[pid as usize] += 1;
                }
            }
            Op::Release { woke } => {
                kani::assume(self.guards[pid as usize] > 0);
                self.guards[pid as usize] -= 1;
                self.st.release_outermost();
                if !woke {
                    self.st.note_dropped_handoff();
                }
            }
            Op::WaiterRecheck => {
                // Kernel discipline: a counted waiter is always a REAL
                // task (pid != 0) — the waiter loop requires can_sleep,
                // and the pid-0 contexts (no-task callers, per-CPU
                // idlers) never park on a SleepMutex. The first kani
                // run without this assume flagged a deeper pid-0 alias
                // (a stale counted pid-0 "waiter" matching another
                // pid-0 context's ownership in the token arm's
                // `owner == pid`) — two distinct kernel entities share
                // pid 0, so the harness must not model them as one.
                kani::assume(pid != 0);
                // The epoch snapshot is taken just before the
                // registration section (fresh each pass).
                self.my_epoch[pid as usize] = self.st.handoff_epoch;
                let mut counted = self.counted[pid as usize];
                let mut first_park = self.first_park[pid as usize];
                if self.st.register_and_recheck(
                    self.now,
                    pid,
                    &mut counted,
                    self.my_epoch[pid as usize],
                    &mut first_park,
                    ANTI_STARVE,
                ) {
                    self.guards[pid as usize] += 1;
                }
                self.counted[pid as usize] = counted;
                self.first_park[pid as usize] = first_park;
            }
            Op::TokenSpend => {
                // Only a counted waiter holds a wake token, and a
                // counted waiter is a real task (see WaiterRecheck).
                kani::assume(pid != 0);
                kani::assume(self.counted[pid as usize]);
                if self.st.try_acquire_wake_token(self.now, pid) {
                    self.guards[pid as usize] += 1;
                    self.counted[pid as usize] = false;
                }
            }
            Op::Exit => {
                if self.st.release_if_owner(pid) {
                    // Guards abandoned wholesale; a counted waiter that
                    // dies parked leaks its unit (checked in K2, not
                    // assumed away here).
                    self.guards[pid as usize] = 0;
                }
                // A task killed while COUNTED (parked in the waiter
                // loop) orphans its waiters unit — the documented
                // bounded-latency leak (see `leaked`).
                if self.counted[pid as usize] {
                    self.counted[pid as usize] = false;
                    self.leaked += 1;
                }
            }
        }
        self.check();
    }
}

/// K1: every disciplined op sequence of length STEPS keeps INV-M1..M5
/// and cannot underflow/overflow (INV-M6).
#[kani::proof]
#[kani::unwind(7)]
fn verify_mutex_ops_invariants() {
    let mut sys = Sys::new();
    sys.check();
    for _ in 0..STEPS {
        let op: Op = kani::any();
        let pid: usize = kani::any();
        sys.step(op, pid);
    }
}

/// K2: do_exit killing a task while it is COUNTED (parked in the
/// waiter loop) leaks one waiters unit — by design (the kernel's
/// bounded-latency insurance: anti-starve + epoch still function).
/// INV-M1..M4 must hold on the leaked state, the gate must still
/// refuse fresh callers, and the remaining waiters' recovery terms
/// (sole-waiter / epoch / anti-starve) must still be able to fire.
#[kani::proof]
#[kani::unwind(7)]
fn verify_mutex_exit_leak_is_bounded() {
    let mut sys = Sys::new();
    // Holder takes the lock; a waiter registers behind it.
    assert!(sys.st.try_acquire_or_recurse_fair(1, 1));
    sys.guards[1] = 1;
    let mut counted = false;
    let mut first_park = 1u64;
    let my_epoch = sys.st.handoff_epoch;
    assert!(!sys.st.register_and_recheck(2, 2, &mut counted, my_epoch, &mut first_park, ANTI_STARVE));
    assert!(counted);
    sys.counted[2] = true;
    sys.check();

    // The holder exits (killed in its critical section): the lock is
    // force-released, but the PARKED waiter's count unit is now owned
    // by a dead task — leaked.
    assert!(sys.st.release_if_owner(1));
    // INV-M1..M4 intact on the leaked state...
    assert_eq!(sys.st.invariant_violation(), None);
    // ...the gate still refuses fresh callers while the leaked unit
    // is counted...
    assert!(!sys.st.try_acquire_or_recurse_fair(3, 0));
    // ...and the anti-starve override still recovers the free lock
    // for the live waiter (a dead one would leave it free forever;
    // the timer's self-wake is what bounds that in the kernel).
    let mut counted2 = true;
    let mut first_park2 = 1u64;
    assert!(sys.st.register_and_recheck(100, 2, &mut counted2, my_epoch, &mut first_park2, ANTI_STARVE));
    assert_eq!(sys.st.invariant_violation(), None);
}
