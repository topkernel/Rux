//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The spinwatch COMPAT ARM (moved from kernel/core/src/debug/spinwatch.rs,
//! crate-flip rung debug): the retired `spinwatch=1` cmdline alias
//! that arms the unified trace events. on()/record()/last_report_n()
//! and the WATCH table stay framework-side (the per-zero-event-return
//! record path in the epoll syscall). Body moved verbatim.

/// Compatibility arm (boot, before trace_event= is applied): the old
/// bespoke cmdline token `spinwatch=1` now just flips the unified
/// events' enable switches — same running state as
/// `trace_event=rux:spinwatch,rux:spinwatch_fd`.
pub fn compat_arm() {
    use core::sync::atomic::Ordering::Relaxed;
    if crate::cmdline::get_cmdline().is_none() {
        return;
    }
    if crate::cmdline::get_param("spinwatch").as_deref() == Some("1") {
        crate::trace::EVENT_SPINWATCH.enabled.store(true, Relaxed);
        crate::trace::EVENT_SPINWATCH_FD.enabled.store(true, Relaxed);
        crate::pr_warn!(
            "spinwatch: compat token spinwatch=1 armed (rux:spinwatch + rux:spinwatch_fd)"
        );
    }
}
