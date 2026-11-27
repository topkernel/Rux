//! The timer subsystem's ONE audited unsafe file (mainline minimal-first
//! consolidation, 2026-11-27): every raw edge the timer wheel still
//! needs lives here behind one audited item; wheel.rs, nohz.rs and
//! oneshot.rs are `#![forbid(unsafe_code)]` safe code over it.
//!
//! Single edge family:
//! - the shared UNIT-TEST wheel's `static mut` storage
//!   (`test_shared`, INV-TIMER-TESTWHEEL-1).

use super::wheel::TimerWheel;

/// INV-TIMER-TESTWHEEL-1: `TEST_WHEEL` is fixed `.bss` storage handed
/// out as an exclusive borrow. The unit-test suite is single-threaded
/// (boot CPU, timer IRQs disabled before `run_all_tests`) and the test
/// group is non-reentrant, so at any instant at most one `&mut` exists
/// and no shared borrow can alias it.
impl TimerWheel {
    /// Borrow the shared UNIT-TEST wheel. The wheel is ~67 KB of fixed
    /// storage BY DESIGN (the R37 no-alloc discipline) — the suite runs
    /// `run_all_tests` on the 64 KB BOOT STACK, so a stack-constructed
    /// test wheel overflows it and the slot writes scribble over
    /// whatever sits below the stack (the first version of the shape
    /// tests did exactly that: the corruption surfaced as the
    /// scheduler-group KERNPANIC / GRQ-deadlock family — CPU-3 idle
    /// illegal-instruction, WAKE-STRANDED, then sys_sched_yield wedged
    /// on the GRQ). Tests use this static exactly like the kernel uses
    /// its `static TIMER_TABLE`: fixed .bss storage, never the stack.
    #[cfg(feature = "unit-test")]
    pub fn test_shared() -> &'static mut TimerWheel {
        static mut TEST_WHEEL: TimerWheel = TimerWheel::new();
        // SAFETY: INV-TIMER-TESTWHEEL-1 above — single-threaded suite,
        // non-reentrant test group, exclusive borrow sound.
        unsafe { &mut *core::ptr::addr_of_mut!(TEST_WHEEL) }
    }
}
