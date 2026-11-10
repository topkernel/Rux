//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Spinlock-deadlock last-gasp ktest (debug-dlspin lane).
//!
//! Drives the synthetic 2-CPU deadlock as BACKGROUND kthreads: a
//! holder freezes holding the test lock with interrupts disabled on a
//! remote CPU, and a waiter kthread spins on the lock until the
//! deadlock watchdog's last-gasp bundle fires (100M spin iterations —
//! tens of seconds under single-thread TCG, which is why the wedge
//! OVERLAPS the rest of the suite instead of blocking it; the verdict
//! is collected at suite end by run_all_tests).
//!
//! The PASS criterion is that the bundle ran; the serial log carries
//! the full proof — the bundle must name BOTH the waiter (cpu, ra,
//! symbolized) and the frozen holder (owner word + cross-CPU peek:
//! comm/pid/state/preempt + symbolized stack frames).
//!
//! The cross-CPU wedge with ALL CPUs IRQ-off cannot be driven safely
//! in-test (the suite runner itself must keep scheduling to report the
//! verdict), so the 2-CPU variant is the tested limit — documented in
//! the lane's run/LANE.md.

use super::{test_fail, test_group_start, test_pass, test_skip};
use crate::debug::dlspin;

pub fn test_dlspin() {
    test_group_start("spinlock deadlock last-gasp");

    // Full holder attribution: the unit-test build pulls in the
    // debug-lock-owner feature (Cargo.toml); arm its recording.
    dlspin::test_arm_spinowner();

    if crate::arch::smp::num_started_cpus() < 3 {
        // The background wedge consumes two CPUs (frozen holder +
        // spinning waiter) while the suite keeps running; below three
        // started CPUs the suite itself would starve.
        test_skip("dlspin synthetic deadlock", "needs >= 3 started CPUs");
        return;
    }

    if dlspin::test_deadlock_start() {
        test_pass("synthetic deadlock launched (verdict at suite end)");
    } else {
        test_fail("dlspin launch", "holder/waiter kthread spawn failed");
    }
}

/// Suite-end verdict (called by run_all_tests after the last group).
pub fn test_dlspin_verdict() {
    test_group_start("dlspin verdict");
    match dlspin::test_deadlock_verdict() {
        Some(true) => test_pass("last-gasp bundle fired (waiter + frozen holder on serial)"),
        Some(false) => test_fail(
            "dlspin last-gasp bundle",
            "waiter acquired without the deadlock warn firing (see serial)",
        ),
        None => test_fail(
            "dlspin last-gasp bundle",
            "waiter never completed within the 300s bound (see serial)",
        ),
    }
}
