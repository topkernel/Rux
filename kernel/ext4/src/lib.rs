//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The EXT4 SERVICE CRATE (crate-flip rung 5): ext4 + jbd2 — the root
//! filesystem driver and its journal — sole-copied here under a
//! crate-level unsafe-deny boundary. The two trees contain ZERO unsafe
//! (the conversion rounds finished before the flip: 86 regions -> 0);
//! every raw view/copy they need is a framework edge
//! (fs/ext4_edges.rs, bio's with_block/bh_data pin family,
//! safety::bytes, syscall/process boxed-ref edges).

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root
// so the service files' crate::X paths resolve unchanged (the net/fs
// crate recipe).
pub use rux::{
    arch, config, console, debug, drivers, errno, interrupt, mm,
    net, printk, process, safety, sched, security, signal, sync,
    timer, trace,
};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println, print, print_status, print_status_ex};
pub use rux::syscall;
extern crate alloc;

/// The merged fs tree, nested under crate::fs: the service trees plus
/// every framework fs item (glob re-export — the framework no longer
/// declares ext4/jbd2, so no name conflicts).
pub mod fs {
    pub mod ext4;
    pub mod jbd2;
    pub use rux::fs::*;
}

/// The boot-time registration (bin calls this before kmain): fills
/// the framework's Ext4Hooks table. THE composition contract. The
/// mount-table rows ride the mount_rows_register row — invoked by the
/// framework at the register_builtin_handlers boot point, NOT here
/// (mounting anything this early wedged x86_64 in the 'R' beacon
/// reboot loop — the fs conviction).
pub fn register_all() {
    crate::fs::ext4::seam::register_all();
}

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (bin registers it at
/// boot). Reporting goes through the framework's counters, so
/// pass/fail lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    crate::tests::run_all();
}

// Service-internal tests live CENTRALLY in kernel/src/tests/services/
// (the project test-location rule); compiled here for crate visibility.
#[cfg(feature = "unit-test")]
#[path = "../../src/tests/services/ext4_tests.rs"]
mod tests;
