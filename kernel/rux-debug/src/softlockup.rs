//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The softlockup REPORT half (moved from kernel/src/debug/softlockup.rs,
//! crate-flip rung rux-debug): the one-shot "BUG: soft lockup" report —
//! holder line, leakorigin origins, stack trace, remote backtrace,
//! taint. check()/touch() and the per-CPU timestamp table stay
//! framework-side (timer-interrupt paths). Fires at most once per CPU
//! per occurrence; the framework's check() dispatches it through the
//! hook row AFTER its REPORTED compare-exchange wins.

use core::fmt::Write;

use crate::debug::backtrace;
use crate::debug::backtrace::ConsoleWriter;
use crate::debug::taint;

/// The lockup report (see the module doc). `timer_cnt` is the stuck
/// CPU's timer-IRQ count read by the caller for the report line.
pub fn report(cpu: usize, elapsed_secs: u64, timer_cnt: u64) {
    let mut w = ConsoleWriter::new();

    let _ = write!(
        w,
        "BUG: soft lockup - CPU#{} stuck for {}s! timer_irq={}\n",
        cpu, elapsed_secs, timer_cnt
    );

    // Print the stuck CPU's current task
    let stuck_task = crate::sched::sched::cpu_state(cpu).current;
    if !stuck_task.is_null() {
        let pid = crate::debug::forensics_edges::t_pid(stuck_task);
        let pc = crate::debug::forensics_edges::t_preempt_count(stuck_task);
        let _ = write!(w, "  CPU: {} PID: {} pc={:#x}\n", cpu, pid, pc);
        // oh-crash 92nd (IF=0 wedge, hp91h residual): the wedged
        // CPU cannot self-report — its tick is dead, so the
        // tick-side orphan scan never fires for it. This CPU (a
        // healthy one in timer context) reads the leakorigin
        // booking SHADOW for the stuck task: hsite names the trap
        // dispatch site that booked a never-released HARDIRQ bit
        // (the pc=0x10000 face keeps irq_exit from restoring IF),
        // ssite the SOFTIRQ booker (__do_softirq / SeqLock::write),
        // osite the outermost preempt acquire whose release never
        // ran. The shadow is plain atomics keyed by task pointer;
        // the stuck task is pinned by the wedged CPU's sched slot
        // (same read discipline as the pid above). All three are
        // budgeted and no-op unless rux:leakhunt (retired debug=leakhunt) is armed.
        crate::debug::leakorigin::report_hardirq_origin(
            b"LOCKUP-HARDIRQ", stuck_task,
        );
        crate::debug::leakorigin::report_softirq_origin(
            b"LOCKUP-SOFTIRQ", stuck_task,
        );
        crate::debug::leakorigin::report_outermost(
            b"LOCKUP-OUTER", stuck_task,
        );
    }

    // Stack trace
    backtrace::dump_stack();

    // oh-crash 91st: the DETECTOR's own chain (above) is not the
    // stuck CPU's. Poke the stuck CPU over the IRQ_WORK IPI and
    // capture its live interrupted stack (REMOTE-BT): its RIP +
    // frame chain is what that CPU has been spinning in for the
    // whole 10s+ window. A timeout is itself evidence (that CPU
    // stopped servicing interrupts entirely). x86_64 only.
    #[cfg(feature = "x86_64")]
    {
        crate::arch::ipi::remote_backtrace(cpu, "softlockup");
    }

    // Taint kernel
    taint::add_taint(taint::TaintFlags::SOFTLOCKUP);
}
