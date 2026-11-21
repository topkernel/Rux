//! The SYSCALL SERVICE CRATE (crate-flip M2): the 12 syscall service
//! files, compiled here under a CRATE-LEVEL `#![forbid(unsafe_code)]`
//! — the compiler-enforced service boundary of the crate plan. The
//! framework lib (the `rux` crate) keeps the audited edges zone
//! (syscall/edges.rs) and the dispatch seam; this crate owns the
//! per-call implementations and registers them into the framework's
//! dispatch tables at boot (register_all, called by the BIN's
//! rust_main before the framework impl).
//!
//! The files are #[path]-included from kernel/src/syscall/ with ZERO
//! moves; their `crate::X` references resolve through the ROOT
//! RE-EXPORTS below (the mechanism proven in run/flip-proto).

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework crate's modules appear at THIS crate's
// root so the service files' crate::X paths resolve unchanged.
pub use rux::{
    arch, config, console, dfx, drivers, errno, fs, io_uring, ipc, list,
    mm, module, net, perf, printk, process, safety, sched, security,
    signal, sync, timer, trace,
};
// The print macros (macro_rules exported at the framework root).
pub use rux::{pr_debug, pr_err, pr_info, pr_warn, println};
// The alloc crate (no_std extern): re-exported at the root so the
// service files' alloc:: paths resolve.
extern crate alloc;


// The audited-edge zone stays FRAMEWORK-side; re-exported under the
// service tree's historical path (syscall/edges.rs's consumers).
pub use rux::syscall_edges;

// The service tree, verbatim from the framework-side location.
#[path = "../../src/syscall/mod.rs"]
pub mod syscall;

/// The composition registration (called by the BIN's rust_main before
/// the framework impl; M2 shape: fills the framework's dispatch
/// OnceSlot tables from THIS crate's compiled sys_* fns — the tables
/// land with the entry-wiring commit; until then this is the stable
/// composition surface).
pub fn register_all() {
    // (tables registration lands with the bin-entry wiring commit —
    // the three-package topology decision in crate-plan.md.)
}
