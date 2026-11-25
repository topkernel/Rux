//! The TIMER SERVICE CRATE (cold-four lane): the timer subsystem's
//! cold report halves under a crate-level unsafe-deny boundary.
//!
//! What stays framework-side (kernel/core/src/timer.rs, a hot file; the
//! worklist's cold rows there are over-keeps, each recorded in the
//! lane report):
//! - the timekeeping core: read_time, get_jiffies, the jiffy math, the
//!   wall-clock epoch family, monotonic/realtime readers;
//! - the tick: timer_softirq_handler, expire_hres_inline,
//!   deliver_fired, run_wheel_sweep;
//! - the wheel (timer/wheel.rs) — TIMER_TABLE's inner type, its
//!   methods called by name from timer.rs (the type-substrate rule);
//! - nohz (timer/nohz.rs) — its entry points are called by name from
//!   the x86 trap and sched/sched.rs (hot files); the internal cluster
//!   is transitively theirs;
//! - the oneshot guards (timer/oneshot.rs) — WakeArmed is a type with
//!   framework users (futex, futex_pi, signal, task);
//! - softirq_diagnostics and tick_trap_report — mixed/unconditional
//!   sections on the tick and page-alloc paths (no fn-pointer row may
//!   sit there); fw_record/pdec/phex — markers with hot callers.
//!
//! The framework reaches THIS crate through the TimerHooks table
//! (kernel/core/src/timer_hooks.rs): one arm-gated row today (the WW
//! heartbeat report — the call site already gates on
//! EVENT_WWCENSUS.armed(), so disarmed boots never touch the table).

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

pub use rux::{arch, cmdline, config, console, debug, drivers, errno, fs,
    interrupt, ipc, list, mm, net, perf, printk, process, safety, sched,
    security, signal, sync, syscall, trace};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

// The service file (THIS crate's own — the sole copy), flat at the
// crate root (seed-core-design 2c.2).
pub mod ww_report;
// the moved helpers at their historical crate::timer::X paths (the
// framework's timer.rs no longer exports them).
pub use ww_report::{del_lookup, nsleep_iter_bump};

// The framework's timekeeping core + wheel/nohz/oneshot at the
// historical path (the union the sibling services consumed) — the
// merged view (`pub use timer;`) in the sibling crates resolves through it.
pub use rux::timer::*;

/// The boot-time registration (bin calls this before
/// rust_main_impl): the TimerHooks table's rows. OnceSlot-only — no
/// Spinlock, no per-CPU state (the x86_64 unit-test kernel lesson).
/// No boot-point constraint: the row is only consumed behind the
/// EVENT_WWCENSUS arm gate, long after registration.
pub fn register_all() {
    rux::timer_hooks::timer_register_hooks(rux::timer_hooks::TimerHooks {
        ww_heartbeat: ww_report::ww_heartbeat,
    });
}
