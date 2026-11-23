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
    arch, cmdline, config, console, dfx, drivers, errno, initrd, interrupt,
    mm, net, printk, process, safety, sched, security, signal, sync,
    timer, trace,
};
// The guest-test counters (unit-test builds only — the framework gates
// the module behind its own feature).
#[cfg(feature = "unit-test")]
pub use rux::tests;
// The framework macros the moved leaves invoke as `crate::X!` —
// macro re-exports resolve at this crate's root.
pub use rux::wait_event_interruptible;
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
    pub mod debugfs;
    pub mod tracefs;
    pub mod cgroup;
    pub mod sysfs;
    pub mod tty;
    pub mod pty;
    pub mod char_dev;
    pub mod rootfs;
    #[cfg(feature = "unit-test")]
    pub mod initrd_test;
    #[cfg(feature = "unit-test")]
    pub mod rootfs_test;
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
    rux::tests::test_group_start("initrd");
    crate::fs::initrd_test::test_initrd();
    rux::tests::test_group_start("fs_pins/rootfs");
    crate::fs::rootfs_test::test_rootfs_pins();
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
        // Registered as hook rows so the FRAMEWORK invokes them at the
        // register_builtin_handlers boot point (mount table ready); a
        // direct call here would run before any init (x86_64 wedges in
        // a 'R'-beacon reboot loop — convicted 2026-11-23).
        leaf_mount_registers: &[
            crate::fs::selinuxfs::mount_register,
            crate::fs::debugfs::mount_register,
            crate::fs::tracefs::mount_register,
            crate::fs::cgroup::mount_register,
            crate::fs::sysfs::mount_register,
        ],
        // ---- debugfs rows ----
        debugfs_create_dir: crate::fs::debugfs::create_dir,
        debugfs_create_file: |path, mode, show, store| {
            match (show, store) {
                (Some(show), Some(store)) => crate::fs::debugfs::create_file(path, mode, show, Some(store)),
                (Some(show), None) => crate::fs::debugfs::create_file(path, mode, show, None::<fn(&[u8]) -> i32>),
                _ => false,
            }
        },
        debugfs_init: crate::fs::debugfs::init_debugfs,
        debugfs_root_present: || crate::fs::debugfs::debugfs_root().is_some(),
        debugfs_create_root_inode: || Some(crate::fs::debugfs::create_root_inode()),
        debugfs_mount_boot: crate::fs::debugfs::mount_debugfs,
        // ---- tracefs rows ----
        tracefs_create_dir: crate::fs::tracefs::create_dir,
        tracefs_create_file: |path, mode, show, store| {
            match (show, store) {
                (Some(show), Some(store)) => crate::fs::tracefs::create_file(path, mode, show, Some(store)),
                (Some(show), None) => crate::fs::tracefs::create_file(path, mode, show, None::<fn(&[u8]) -> i32>),
                _ => false,
            }
        },
        tracefs_create_stream_file: |path, mode, show| {
            match show {
                Some(show) => crate::fs::tracefs::create_stream_file(path, mode, show),
                None => false,
            }
        },
        tracefs_root_present: || crate::fs::tracefs::tracefs_root().is_some(),
        tracefs_init: crate::fs::tracefs::init_tracefs,
        tracefs_create_root_inode: || Some(crate::fs::tracefs::create_root_inode()),
        tracefs_mount_at: crate::fs::tracefs::mount_at,
        // ---- cgroup rows ----
        cgroup_init_cgroupfs: crate::fs::cgroup::init_cgroupfs,
        cgroup_create_root_inode: || Some(crate::fs::cgroup::create_root_inode()),
        cgroup_mount_cgroupfs: crate::fs::cgroup::mount_cgroupfs,
        // ---- sysfs rows ----
        sysfs_init_sysfs: crate::fs::sysfs::init_sysfs,
        sysfs_mount_sysfs: crate::fs::sysfs::mount_sysfs,
        sysfs_create_root_inode: || Some(crate::fs::sysfs::create_root_inode()),
        sysfs_publish_module: crate::fs::sysfs::publish_module,
        sysfs_unpublish_module: crate::fs::sysfs::unpublish_module,
        sysfs_register_drm_device: crate::fs::sysfs::register_drm_device,
        sysfs_netdev_uevent: crate::fs::sysfs::netdev_uevent,
        sysfs_register_block_disk_at: crate::fs::sysfs::register_block_disk_at,
        sysfs_register_input_event: crate::fs::sysfs::register_input_event,
        sysfs_uevent_send_full: crate::fs::sysfs::uevent_send_full,
        char_dev_init_std_fds: crate::fs::char_dev::init_std_fds_for_task,
        char_dev_file_stat: crate::fs::char_dev::char_dev_file_stat,
        rootfs_init: crate::fs::rootfs::init_rootfs,
        rootfs_create_root_inode: || Some(crate::fs::rootfs::create_root_inode()),
        rootfs_initrd_sb: || crate::fs::rootfs::initrd_sb_view(),
        rootfs_read_file: crate::fs::rootfs::read_file,
        rootfs_sb_ref_interim: crate::fs::rootfs::initrd_sb_view,
        char_dev_uart_write: crate::fs::char_dev::uart_write,
        tty_console_lflag: || crate::fs::tty::console().lflag(),
        tty_console_echo_enabled: || crate::fs::tty::console().echo_enabled(),
        tty_console_receive_byte: |c| crate::fs::tty::console().receive_byte(c),
        tty_console_fg_pgid: || crate::fs::tty::console().fg_pgrp.load(core::sync::atomic::Ordering::Acquire),
        tty_set_console_sink_quiet: crate::fs::tty::set_console_sink_quiet,
        tty_console_read_input: |buf, nonblock| crate::fs::tty::console().read_input(buf, nonblock),
        tty_console_output_translates_nl: || crate::fs::tty::console().output_translates_nl(),
        tty_console_input_poll_ready: || crate::fs::tty::console().input_poll_ready(),
        pty_devfs_register: crate::fs::pty::devfs_register,
        pty_ptmx_open: crate::fs::pty::ptmx_open,
        pty_slave_open: crate::fs::pty::slave_open,
        pty_devno_is_ptmx: |devno| devno == crate::fs::pty::DEV_PTMX,
        pty_dev_ptmx_devno: || crate::fs::pty::DEV_PTMX,
        pty_slave_major: || crate::fs::pty::PTY_SLAVE_MAJOR,
    });
}

