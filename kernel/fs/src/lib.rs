//! The FS SERVICE CRATE (crate-flip rung 3): the movable filesystem
//! leaves under a crate-level unsafe-deny boundary. The files live
//! in this crate (src/), flat under its root.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the service files' crate::X paths resolve unchanged (the net recipe).
pub use rux::{
    arch, cmdline, config, console, debug, drivers, errno,
    interrupt, ipc, mm, module, net, printk, process, safety, sched,
    security, signal, sync, timer, trace,
};
// The guest-test counters (unit-test builds only — the framework gates
// the module behind its own feature).
#[cfg(feature = "unit-test")]
pub use rux::tests;
// (`initrd` is NOT in that list: this crate's own initrd module owns
// the name at the root; the framework's is `rux::initrd`.)
// The framework macros the moved leaves invoke as `crate::X!` —
// macro re-exports resolve at this crate's root.
pub use rux::wait_event_interruptible;
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println, print, print_status};
pub use rux::syscall;
extern crate alloc;
pub use rux::fs::file::File;

// The fs leaves, flat at the crate root (seed-core-design 2c.2).
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
pub mod initrd;
pub mod tmpfs;
pub mod devfs;
pub mod procfs;
pub mod xattr;
// The fs-rest split of the locks family: this module shadows the
// framework's (smaller) locks remainder for every crate::locks
// path — the moved logic plus re-exports of the framework half's
// directly-called legs (execve's ETXTBSY gate and the exec-commit
// text hold, which syscall names crate-side).
pub mod locks;

// The ext4 service trees (rung 5): re-exported so the downstream
// service crate's `crate::ext4` paths (syscall) resolve.
pub use ::ext4::{ext4, jbd2};
// Every framework fs item at this crate's root (glob re-export — the
// framework no longer declares the moved leaves, so no name conflicts):
// the syscall crate's merged view (`pub use fs;`) resolves through it.
pub use rux::fs::*;

// Service-internal tests live CENTRALLY in kernel/core/src/tests/services/
// (the project test-location rule); compiled here for crate visibility.
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/pipe2_test.rs"]
pub mod pipe2_test;
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/initrd_test.rs"]
pub mod initrd_test;
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/rootfs_test.rs"]
pub mod rootfs_test;
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/tmpfs_test.rs"]
pub mod tmpfs_test;
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/tmpfs_mknod_test.rs"]
pub mod tmpfs_mknod_test;
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/procfs_test.rs"]
pub mod procfs_test;

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (bin registers it at
/// boot). Reporting goes through the framework's counters, so pass/fail
/// lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    rux::tests::test_group_start("pipe2");
    crate::pipe2_test::test_pipe2();
    rux::tests::test_group_start("initrd");
    crate::initrd_test::test_initrd();
    rux::tests::test_group_start("fs_pins/rootfs");
    crate::rootfs_test::test_rootfs_pins();
    rux::tests::test_group_start("tmpfs-mknod");
    crate::tmpfs_mknod_test::test_tmpfs_mknod();
    // tmpfs_test opens its own "tmpfs" group, as it did framework-side.
    // The move landed the file without this call and the suite silently
    // lost the group's 48 assertions (879 -> 831).
    crate::tmpfs_test::test_tmpfs();
    rux::tests::test_group_start("procfs");
    crate::procfs_test::test_procfs();
}

/// Every leaf's mount-table registration, in the order the framework's
/// own built-in list used before the tranche (tmpfs, procfs, devfs,
/// sysfs, cgroup, debugfs, tracefs) with selinuxfs last (it registered
/// through the hook row). The framework invokes this ONCE, from
/// mount_table::register_builtin_handlers at its boot point — never at
/// rust_main time (the mount table and the allocator are not up yet
/// that early; an early call wedges x86_64 before the console exists,
/// the boot.S 'R'-beacon reboot loop).
fn leaf_mount_rows_register() {
    crate::tmpfs::mount_register();
    crate::procfs::mount_register();
    crate::devfs::mount_register();
    crate::sysfs::mount_register();
    crate::cgroup::mount_register();
    crate::debugfs::mount_register();
    crate::tracefs::mount_register();
    crate::selinuxfs::mount_register();
}

/// The boot-time registration (bin calls this before kmain): the
/// rung-3 FsHooks plus the three leaf interface tables
/// (rux::fs::leaf_hooks). THE composition contract. Publishing only
/// stores plain values — no row body runs here.
pub fn register_all() {
    rux::fs::fs_hooks::fs_register_hooks(rux::fs::fs_hooks::FsHooks {
        notify: crate::inotify::notify,
        notify_file: crate::inotify::notify_file,
        notify_file_close: crate::inotify::notify_file_close,
        alloc_cookie: crate::inotify::alloc_cookie,
        pipe_stat: crate::pipe::pipe_file_stat,
        pipe_get_sz: crate::pipe::pipe_get_sz,
        pipe_set_sz: crate::pipe::pipe_set_sz,
        pipe_oasync_register: crate::pipe::oasync_register_pipe,
        pipe_oasync_unregister: crate::pipe::oasync_unregister_file,
        pipe_diag_snapshot: |f: &File| {
            crate::pipe::pipe_of_file(f).map(|p| p.diag_snapshot())
        },
        fifo_forget: crate::fifo::forget,
        fifo_open: |fs_id: u64, ino: u64, flags: u32| {
            let peer = crate::fifo::peer_for(fs_id, ino);
            crate::fifo::fifo_open_file(&peer, flags)
        },
        memfd_add_seals: crate::memfd::add_seals,
        memfd_get_seals: crate::memfd::get_seals,
        memfd_name_by_ino: crate::memfd::memfd_name_by_ino,
        is_pipe_ops: |ops| core::ptr::eq(ops as *const _, &crate::pipe::PIPE_OPS as *const _),
        is_fifo_ops: |ops| core::ptr::eq(ops as *const _, &crate::fifo::FIFO_OPS as *const _),
        is_inotify_ops: |ops| core::ptr::eq(ops as *const _, &crate::inotify::INOTIFY_OPS as *const _),
        is_memfd_ops: |ops| core::ptr::eq(ops as *const _, &crate::memfd::MEMFD_FILE_OPS as *const _),
        selinuxfs_root_inode: || Some(crate::selinuxfs::create_root_inode()),
        inotify_max_queued_events: crate::inotify::max_queued_events,
        inotify_set_max_queued_events: |v| {
            crate::inotify::MAX_QUEUED_EVENTS_SYSCTL.store(v, core::sync::atomic::Ordering::Release)
        },
        inotify_fdinfo_watches: crate::inotify::fdinfo_watches,
    });

    // The locks family's cold rows (fs-rest split): the fcntl lock arms
    // and the slow release bodies behind the framework half's count
    // gates. Same contract — publishing stores plain values only.
    rux::fs::lock_hooks::register_locks_hooks(rux::fs::lock_hooks::LocksHooks {
        flock_release_slow: crate::locks::flock_release_file_slow,
        ofd_release_slow: crate::locks::ofd_release_file_slow,
        posix_release_slow: crate::locks::posix_release_key_slow,
        lease_release_slow: crate::locks::lease_release_file_slow,
        break_lease_slow: crate::locks::break_lease_full,
        posix_test_lock: crate::locks::posix_test_lock,
        posix_set_lock: crate::locks::posix_set_lock,
        set_lease: crate::locks::set_lease,
        get_lease: crate::locks::get_lease,
    });

    use rux::fs::leaf_hooks::{LeafBootHooks, LeafPathHooks, LeafPopulateHooks};

    // 1. Boot + mount lifecycle. Every row is a plain leaf fn.
    rux::fs::leaf_hooks::register_boot_hooks(LeafBootHooks {
        mount_rows_register: leaf_mount_rows_register,
        debugfs_init: crate::debugfs::init_debugfs,
        debugfs_root_present: crate::debugfs::root_present,
        debugfs_create_root_inode: crate::debugfs::create_root_inode,
        debugfs_mount: crate::debugfs::mount_debugfs,
        tracefs_init: crate::tracefs::init_tracefs,
        tracefs_root_present: crate::tracefs::root_present,
        tracefs_create_root_inode: crate::tracefs::create_root_inode,
        tracefs_mount_at: crate::tracefs::mount_at,
        cgroup_init: crate::cgroup::init_cgroupfs,
        cgroup_create_root_inode: crate::cgroup::create_root_inode,
        cgroup_mount: crate::cgroup::mount_cgroupfs,
        sysfs_init: crate::sysfs::init_sysfs,
        sysfs_mount: crate::sysfs::mount_sysfs,
        sysfs_create_root_inode: crate::sysfs::create_root_inode,
        procfs_init: crate::procfs::init_procfs,
        procfs_mount: crate::procfs::mount_procfs,
        procfs_sb_present: crate::procfs::sb_present,
        procfs_create_root_inode: crate::procfs::create_root_inode,
        devfs_init: crate::devfs::init,
        devfs_create_root_inode: crate::devfs::mounted_root_inode,
        tmpfs_ensure_registered: crate::tmpfs::ensure_registered,
        tmpfs_create_mount_instance: crate::tmpfs::create_tagged_mount_instance,
        rootfs_init: crate::rootfs::init_rootfs,
        rootfs_create_root_inode: crate::rootfs::create_root_inode,
        rootfs_view: crate::rootfs::ramfs_view,
        rootfs_read_file: crate::rootfs::read_file,
        initrd_gunzip: crate::initrd::gunzip,
        initrd_unpack_newc: crate::initrd::unpack_newc,
        char_dev_init_std_fds: crate::char_dev::init_std_fds_for_task,
    });

    // 2. Kernel-side population.
    rux::fs::leaf_hooks::register_populate_hooks(LeafPopulateHooks {
        debugfs_create_dir: crate::debugfs::create_dir,
        debugfs_create_file: crate::debugfs::create_file_boxed,
        tracefs_create_dir: crate::tracefs::create_dir,
        tracefs_create_file: crate::tracefs::create_file_boxed,
        tracefs_create_stream_file: crate::tracefs::create_stream_file_boxed,
        sysfs_publish_module: crate::sysfs::publish_module,
        sysfs_unpublish_module: crate::sysfs::unpublish_module,
        sysfs_register_drm_device: crate::sysfs::register_drm_device,
        sysfs_netdev_uevent: crate::sysfs::netdev_uevent,
        sysfs_register_block_disk_at: crate::sysfs::register_block_disk_at,
        sysfs_register_input_event: crate::sysfs::register_input_event,
        devfs_mkdir: crate::devfs::mkdir,
        devfs_mknod: crate::devfs::mknod,
        devfs_register_char_device: crate::devfs::registry::register_char_device,
    });

    // 3. Path dispatch. The identities are the leaves' real ops tables;
    // the framework compares them INLINE at the call sites, so the three
    // identity probes that used to be rows are gone.
    rux::fs::leaf_hooks::register_path_hooks(LeafPathHooks {
        tmpfs_file_ops: &crate::tmpfs::TMPFS_FILE_OPS,
        devfs_reg_file_ops: &crate::devfs::DEVFS_REG_FILE_OPS,
        loop_file_ops: &crate::devfs::LOOP_FILE_OPS,
        devfs_condev_ops: &crate::devfs::CONDEV_OPS,
        proc_piddir_ops: &crate::procfs::PROC_PIDDIR_OPS,
        proc_fd_dir_ops: &crate::procfs::PROCFS_DIR_OPS,
        tty_console_lflag: crate::tty::console_lflag,
        tty_console_echo_enabled: crate::tty::console_echo_enabled,
        tty_console_receive_byte: crate::tty::console_receive_byte,
        tty_console_fg_pgid: crate::tty::console_fg_pgid,
        tty_set_console_sink_quiet: crate::tty::set_console_sink_quiet,
        char_dev_file_stat: crate::char_dev::char_dev_file_stat,
        devfs_condev_read: crate::devfs::condev_read,
        devfs_condev_write: crate::devfs::condev_write,
        devfs_condev_poll: crate::devfs::condev_poll,
        tmpfs_fallocate: crate::tmpfs::fallocate,
        procfs_is_valid_pid: crate::procfs::pid::is_valid_pid,
        procfs_get_fd_file: crate::procfs::pid::get_fd_file,
        procfs_core_pattern: crate::procfs::core_pattern,
        devfs_reg_current_size: crate::devfs::devfs_reg_current_size,
        devfs_shared_fault_file_page: crate::devfs::shared_fault_file_page,
    });
}
