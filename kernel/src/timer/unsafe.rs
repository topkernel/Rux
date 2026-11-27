//! The timer subsystem's ONE audited unsafe file (mainline minimal-first
//! consolidation, 2026-11-27): every raw edge the timer wheel still
//! needs lives here behind one audited item; wheel.rs, nohz.rs and
//! oneshot.rs are `#![forbid(unsafe_code)]` safe code over it.
//!
//! Single edge family:
//! - the shared UNIT-TEST wheel's `static mut` storage
//!   (`test_shared`, INV-TIMER-TESTWHEEL-1).

use super::wheel::TimerWheel;
use crate::process::task::Task;
use crate::process::task_ref::TaskBorrow;

/// INV-TIMER-TASKADOPT-1: the timer wheel's expiry path adopts a
/// raw task pointer that the wheel's own removal discipline pins (the
/// FiredToken target is held by the token's id through the removal
/// path), and the adopt is a short syscall-context read/write on this
/// CPU with no yield — the same contract `TaskBorrow::from_raw`
/// documents for adopt-once handles (INV-TASKREF-1 hatch row).
///
/// Adopt an already-validated raw task handle (the wheel's pinned
/// FiredToken target).
pub(super) fn task_borrow_from_raw(p: *mut Task) -> Option<TaskBorrow> {
    // SAFETY: INV-TIMER-TASKADOPT-1 above (pinned target, short
    // syscall-context access, no yield).
    unsafe { TaskBorrow::from_raw(p) }
}

/// INV-TIMER-TIONCPU-1: the diagnostic volatile read of `ti_on_cpu`
/// targets the task the wheel callback is currently running on (its
/// execution pins the page); the field is 8-byte aligned (compile-time
/// assert in task.rs) so a u64 volatile read is well-formed, and the
/// read is diagnostic-only.
///
/// Volatile read of the task's ti_on_cpu word (diagnostics).
pub(super) fn ti_on_cpu_volatile_read(task: &Task) -> u64 {
    // SAFETY: INV-TIMER-TIONCPU-1 above (pinned task, aligned field).
    unsafe {
        core::ptr::read_volatile(core::ptr::addr_of!(task.ti_on_cpu) as *const u64)
    }
}

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
