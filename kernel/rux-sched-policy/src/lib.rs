//! The SCHED POLICY SERVICE CRATE (crate-flip S1b): the scheduling
//! classes (stop/dl/rt/fair/idle), the pick/enqueue/wake-placement/
//! census/diagnostic walks — under a crate-level unsafe-forbid
//! boundary. The framework keeps sched/sched.rs (the CORE half: GRQ +
//! guards, the switch machinery, current/tp, NEED_RESCHED, task-slot
//! lifecycle, the walkers) and sched/sched_hooks.rs (the hook table);
//! drivers and arch consume the policy through those.
//!
//! Migration state (S1b): the files are #[path]-included from
//! kernel/src/sched/ with ZERO moves; the sole-copy move + bin
//! registration follow the rung-2 recipe once the hook seam lands.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the service files' crate::X paths resolve unchanged (the proven shape
// from rux-syscall and rux-net).
pub use rux::{
    arch, cmdline, config, console, dfx, drivers, errno, fs, interrupt, list, mm,
    process, safety, sched, sync, timer, trace,
};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

// The policy tree at crate::sched_policy (the historical crate::sched::
// paths stay valid through the framework's re-export chain).
pub mod sched_policy {
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/sched/sched_policy.rs"] pub mod core_policy;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/sched/fair.rs"] pub mod fair;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/sched/rt.rs"] pub mod rt;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/sched/deadline.rs"] pub mod deadline;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/sched/class.rs"] pub mod class;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/sched/idle.rs"] pub mod idle;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/sched/stop_task.rs"] pub mod stop_task;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/sched/stats.rs"] pub mod stats;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/sched/sched_ev.rs"] pub mod sched_ev;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/sched/wakewatch.rs"] pub mod wakewatch;

    // The framework globs (the service files' bare-name refs).
    pub use crate::arch::*;
    pub use crate::config;
    pub use crate::dfx::*;
    pub use crate::drivers::*;
    pub use crate::errno;
    pub use crate::fs::*;
    pub use crate::mm::*;
    pub use crate::process::*;
    pub use crate::safety::*;
    pub use crate::sched::*;
    pub use crate::sync::*;
    pub use crate::timer::*;
    pub use crate::trace;
}

/// Registration: fills the framework's SchedHooks table (the S1b data
/// seam). Called by the composition bin before the framework impl.
pub fn register_all() {
    // (the full mapping lands with the hook table — the seam's rows are
    // measured in run/s1b-reverse2.json: 90 fn rows / 336 refs)
}
