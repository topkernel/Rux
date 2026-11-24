//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! TIMER FLIP SEAM (cold-four lane: rux-timer): the hook table the
//! FRAMEWORK uses to reach the timer service crate's cold REPORT
//! halves. The timekeeping core, the tick, the wheel, nohz and the
//! oneshot guards stay framework-side (kernel/src/timer.rs is a
//! hot-path file; the subdirectory clusters are its data structures —
//! the type-substrate rule). Everything routed here is measured cold:
//! today one row, the WW heartbeat report — its call site (the timer
//! softirq's 1 Hz heartbeat slot) is ARM-GATED on EVENT_WWCENSUS, so
//! disarmed boots never touch the table (the S1 conditional-row
//! precedent: the wrapper's gate fires before the slot is loaded).
//!
//! Registration runs from rust_main via rux_timer::register_all —
//! OnceSlot-only publication (no Spinlock, no per-CPU state; the
//! x86_64 unit-test kernel lesson).

/// The timer report rows. Signatures carry native types (crate-plan
/// §3a rule 3): no adapter logic.
#[derive(Clone, Copy)]
pub struct TimerHooks {
    /// The WW census heartbeat report (formatting half; 1 Hz, armed).
    pub ww_heartbeat: fn(u64),
}

impl TimerHooks {
    pub const EMPTY: TimerHooks = TimerHooks {
        ww_heartbeat: no_report_ww_heartbeat,
    };
}

fn no_report_ww_heartbeat(_: u64) {
    // Empty default is fine: the row is only reached behind the
    // EVENT_WWCENSUS gate, which is off unless the crate registered.
}

pub static TIMER_HOOKS: crate::safety::slot::OnceSlot<TimerHooks> =
    crate::safety::slot::OnceSlot::new();

pub fn timer_register_hooks(hooks: TimerHooks) {
    let _ = TIMER_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> &'static TimerHooks {
    TIMER_HOOKS.get().unwrap_or(&TimerHooks::EMPTY)
}

#[inline]
pub fn ww_heartbeat(now_time: u64) {
    (hooks().ww_heartbeat)(now_time)
}
