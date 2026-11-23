//! The FS SERVICE CRATE (crate-flip rung 3): the movable filesystem
//! leaves under a crate-level unsafe-deny boundary. Zero-move step:
//! the service files are #[path]-included from the framework tree
//! (paths verified by build; physical moves are the later cosmetic
//! step per the crate-plan recipe).

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the service files' crate::X paths resolve unchanged (the net recipe).
pub use rux::{
    arch, cmdline, config, console, debug, drivers, errno, interrupt, mm,
    process, safety, sched, security, signal, sync, timer, trace,
};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println, print, print_status};
pub use rux::syscall;
extern crate alloc;
use rux::fs::file::File;

/// The fs tree, nested under crate::fs: the service leaves plus every
/// framework fs item (glob re-export — the framework no longer
/// declares the moved leaves, so no name conflicts).
pub mod fs {
    // The ext4 service trees (rung 5): re-exported so downstream
    // service crates (rux-syscall's crate::fs::ext4 paths) resolve.
    pub use rux_ext4::fs::{ext4, jbd2};
    pub mod pipe;
    pub mod fifo;
    pub mod inotify;
    pub mod memfd;
    pub mod selinuxfs;
    pub use rux::fs::*;
}

// Service-internal test lives CENTRALLY in kernel/src/tests/services/.
#[cfg(feature = "unit-test")]
#[path = "../../src/tests/services/pipe2_test.rs"]
pub mod pipe2_test;

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (rux-bin registers it at
/// boot). Reporting goes through the framework's counters, so pass/fail
/// lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    rux::tests::test_group_start("pipe2");
    crate::pipe2_test::test_pipe2();
}

/// The boot-time registration (rux-bin calls this before kmain):
/// the mount names + the FsHooks slots. THE composition contract.
pub fn register_all() {
    rux::fs::fs_hooks::fs_register_hooks(rux::fs::fs_hooks::FsHooks {
        notify: crate::fs::inotify::notify,
        notify_file: crate::fs::inotify::notify_file,
        notify_file_close: crate::fs::inotify::notify_file_close,
        alloc_cookie: crate::fs::inotify::alloc_cookie,
        pipe_stat: crate::fs::pipe::pipe_file_stat,
        pipe_get_sz: crate::fs::pipe::pipe_get_sz,
        pipe_set_sz: crate::fs::pipe::pipe_set_sz,
        pipe_oasync_register: crate::fs::pipe::oasync_register_pipe,
        pipe_oasync_unregister: crate::fs::pipe::oasync_unregister_file,
        pipe_diag_snapshot: |f: &File| {
            crate::fs::pipe::pipe_of_file(f).map(|p| p.diag_snapshot())
        },
        fifo_forget: crate::fs::fifo::forget,
        fifo_open: |fs_id: u64, ino: u64, flags: u32| {
            let peer = crate::fs::fifo::peer_for(fs_id, ino);
            crate::fs::fifo::fifo_open_file(&peer, flags)
        },
        memfd_add_seals: crate::fs::memfd::add_seals,
        memfd_get_seals: crate::fs::memfd::get_seals,
        memfd_name_by_ino: crate::fs::memfd::memfd_name_by_ino,
        is_pipe_ops: |ops| core::ptr::eq(ops as *const _, &crate::fs::pipe::PIPE_OPS as *const _),
        is_fifo_ops: |ops| core::ptr::eq(ops as *const _, &crate::fs::fifo::FIFO_OPS as *const _),
        is_inotify_ops: |ops| core::ptr::eq(ops as *const _, &crate::fs::inotify::INOTIFY_OPS as *const _),
        is_memfd_ops: |ops| core::ptr::eq(ops as *const _, &crate::fs::memfd::MEMFD_FILE_OPS as *const _),
        selinuxfs_root_inode: || Some(crate::fs::selinuxfs::create_root_inode()),
        inotify_max_queued_events: crate::fs::inotify::max_queued_events,
        inotify_set_max_queued_events: |v| {
            crate::fs::inotify::MAX_QUEUED_EVENTS_SYSCTL.store(v, core::sync::atomic::Ordering::Release)
        },
        inotify_fdinfo_watches: crate::fs::inotify::fdinfo_watches,
        // Registered as a hook row so the FRAMEWORK invokes it at the
        // register_builtin_handlers boot point (mount table ready); a
        // direct call here would run before any init (x86_64 wedges in
        // a 'R'-beacon reboot loop — convicted 2026-11-23).
        leaf_mount_register: crate::fs::selinuxfs::mount_register,
    });
}

