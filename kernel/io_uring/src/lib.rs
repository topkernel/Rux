//! The IO_URING SERVICE CRATE (cold-four lane): the syscall-context
//! policy half of io_uring under a crate-level unsafe-deny boundary.
//!
//! What stayed in the framework (kernel/core/src/io_uring/) and why:
//! - the ring regions + the INV-IOURING-RING-1 window views (sq_hdr /
//!   cq_hdr / sqe_at / cqe_write — unsafe, user-shared memory);
//! - the lifecycle discipline (LIFECYCLE_LOCK, pin_verified_ring /
//!   PinnedRing — the R7-C4 read-inside-the-lock invariant,
//!   INV-IOURING-PIN) and io_uring_close / io_uring_poll / the
//!   IO_URING_OPS static (pointer-identity type-confusion guards);
//! - the completion path (io_uring_post_cqe + signal_eventfd_file) —
//!   on the wakeup path;
//! - mmap (page-table writes) and io_uring_create (private-field type
//!   assembly + the unsafe region allocator).
//!
//! The moved code's unsafe was converted at the flip: the error-path
//! Box::from_raw pairs ride ring_release_on_install_error
//! (INV-IOURING-RING-2), the private_data reads ride pin_verified_ring,
//! and the raw read_volatile head/tail pair rides cq_completed().
//! No hook table is needed: every moved function's callers are either
//! in this crate or in syscall (whose crate::io_uring path now
//! resolves to THIS crate's root — the allowed downward edge).

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

pub use rux::{arch, config, console, debug, drivers, errno, fs, interrupt,
    ipc, list, mm, module, net, perf, printk, safety, sched, security,
    signal, sync, syscall, timer, trace};
pub use rux::{pr_debug, pr_err, pr_info, pr_warn, println};
extern crate alloc;

// The policy files (THIS crate's own — the sole copies), flat at the
// crate root (seed-core-design 2c.2).
pub mod submit;
pub mod ops;
pub mod syscall_api;

// The names the moved policy files' `use super::*` expects (the old
// framework mod.rs's own imports — the ipc recipe's glob-surface
// contract).
#[allow(unused_imports)]
use crate::fs::{File, FileOps, FileFlags};
#[allow(unused_imports)]
use crate::alloc::sync::Arc;

// The moved syscall entries at their historical
// crate::io_uring::sys_io_uring_* paths (syscall's NR 425/426/427
// wrappers keep resolving unchanged).
pub use syscall_api::{sys_io_uring_setup, sys_io_uring_enter, sys_io_uring_register};
pub use submit::{submit_sqes, wait_for_cqes};
pub use ops::io_uring_dispatch_op;

// the framework's mechanism at the historical path (types, ring
// views, lifecycle, mmap, create, post_cqe, ops table) — the merged
// view (`pub use io_uring;`) in the syscall crate resolves through it.
pub use rux::io_uring::*;
