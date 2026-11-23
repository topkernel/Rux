//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Rung-3 FLIP SEAM: the fs-service hook table (the NetHooks pattern).
//! The framework's per-File op hooks (pipe/fifo/memfd), the lifecycle
//! notify fanout, and the ops-identity probes dispatch through THIS
//! nullable table — no-op until the rux-fs crate registers at boot
//! (rux-bin calls its register_all before kmain, the composition
//! contract). The framework never names a service leaf.

use crate::fs::file::File;
use crate::fs::leaf_glue::TmpfsTagKind;
use alloc::boxed::Box;
use crate::fs::inode::Inode;

/// The fs-service hook set.
#[derive(Clone, Copy)]
pub struct FsHooks {
    // ---- lifecycle notify (inotify) ----
    pub notify: fn(Option<&Inode>, Option<&Inode>, u32, Option<&[u8]>, u32),
    pub notify_file: fn(&File, u32),
    pub notify_file_close: fn(&File),
    pub alloc_cookie: fn() -> u32,
    // ---- pipe per-File ops ----
    pub pipe_stat: fn(&File, &mut crate::fs::Stat) -> Option<i32>,
    pub pipe_get_sz: fn(&File) -> Option<usize>,
    pub pipe_set_sz: fn(&File, usize) -> Result<usize, i32>,
    pub pipe_oasync_register: fn(&File, i32),
    pub pipe_oasync_unregister: fn(u64),
    /// The futex park-dump snapshot: (fill, capacity, r_open, w_open).
    pub pipe_diag_snapshot: fn(&File) -> Option<(usize, usize, bool, bool)>,
    // ---- fifo ----
    pub fifo_forget: fn(u64, u64),
    /// The special-open path: peer resolution + open, one call.
    pub fifo_open: fn(u64, u64, u32) -> Result<alloc::sync::Arc<File>, i32>,
    // ---- memfd seals ----
    pub memfd_add_seals: fn(&File, u32) -> Result<(), i32>,
    pub memfd_get_seals: fn(&File) -> Result<u32, i32>,
    pub memfd_name_by_ino: fn(u64) -> Option<&'static str>,
    // ---- ops-identity probes (fd-kind census + futex dumps) ----
    pub is_pipe_ops: fn(&'static crate::fs::FileOps) -> bool,
    pub is_fifo_ops: fn(&'static crate::fs::FileOps) -> bool,
    pub is_inotify_ops: fn(&'static crate::fs::FileOps) -> bool,
    pub is_memfd_ops: fn(&'static crate::fs::FileOps) -> bool,
    // ---- misc leaf surfaces ----
    /// fsctx's selinuxfs root-inode factory.
    pub selinuxfs_root_inode: fn() -> Option<alloc::sync::Arc<Inode>>,
    /// procfs sysctl: /proc/sys/fs/inotify/max_queued_events value.
    pub inotify_max_queued_events: fn() -> usize,
    pub inotify_set_max_queued_events: fn(usize),
    /// procfs fdinfo: an inotify fd's watch rows.
    pub inotify_fdinfo_watches: fn(&File) -> Option<alloc::vec::Vec<(i32, u64, u64, u32)>>,
    /// The service crate's mount-table row registrations (every moved
    /// leaf's mount_register, in registration order). MUST run at the
    /// register_builtin_handlers boot point — NOT at rust_main time: the
    /// mount table and the allocator are not up yet that early (an early
    /// call panics on x86_64 before the console exists: only the boot.S
    /// 'R' beacon prints, in a reboot loop).
    pub leaf_mount_registers: &'static [fn()],
    // ---- substrate/boot leaf rows (fs-substrate tranche) ----
    /// debugfs: create_dir population entry.
    pub debugfs_create_dir: fn(&str) -> bool,
    /// debugfs: create_file population entry (boxed closures; the
    /// framework's leaf_glue boxes the generic originals).
    pub debugfs_create_file: fn(&str, u32, Option<Box<dyn Fn() -> alloc::vec::Vec<u8> + Send + Sync>>, Option<Box<dyn Fn(&[u8]) -> i32 + Send + Sync>>) -> bool,
    pub debugfs_init: fn() -> Result<(), i32>,
    /// debugfs_root().is_some() (the fsctx lazy-init probe).
    pub debugfs_root_present: fn() -> bool,
    pub debugfs_create_root_inode: fn() -> Option<alloc::sync::Arc<Inode>>,
    /// lib.rs boot's mount_debugfs().
    pub debugfs_mount_boot: fn() -> Result<(), i32>,
    /// tracefs population rows (same shapes as debugfs).
    pub tracefs_create_dir: fn(&str) -> bool,
    pub tracefs_create_file: fn(&str, u32, Option<Box<dyn Fn() -> alloc::vec::Vec<u8> + Send + Sync>>, Option<Box<dyn Fn(&[u8]) -> i32 + Send + Sync>>) -> bool,
    pub tracefs_create_stream_file: fn(&str, u32, Option<Box<dyn Fn() -> alloc::vec::Vec<u8> + Send + Sync>>) -> bool,
    pub tracefs_root_present: fn() -> bool,
    pub tracefs_init: fn() -> Result<(), i32>,
    pub tracefs_create_root_inode: fn() -> Option<alloc::sync::Arc<Inode>>,
    /// lib.rs boot's tracefs mount_at() (the two canonical locations).
    pub tracefs_mount_at: fn(&str) -> Result<(), i32>,
    /// cgroup rows.
    pub cgroup_init_cgroupfs: fn() -> Result<(), i32>,
    pub cgroup_create_root_inode: fn() -> Option<alloc::sync::Arc<Inode>>,
    pub cgroup_mount_cgroupfs: fn(&str) -> Result<(), i32>,
    /// sysfs rows.
    pub sysfs_init_sysfs: fn() -> Result<(), i32>,
    pub sysfs_mount_sysfs: fn() -> Result<(), i32>,
    pub sysfs_create_root_inode: fn() -> Option<alloc::sync::Arc<Inode>>,
    /// /sys/module/<name> publish/unpublish (module loader's exposure).
    pub sysfs_publish_module: fn(&str),
    pub sysfs_unpublish_module: fn(&str),
    pub sysfs_register_drm_device: fn(&str),
    pub sysfs_netdev_uevent: fn(&str, &str) -> u64,
    pub sysfs_register_block_disk_at: fn(&str, u32, u32, u64, &[(&str, &str)]) -> u64,
    pub sysfs_register_input_event: fn(&super::leaf_glue::InputEventReg) -> u64,
    /// devfs's uevent broadcast (framework-side until devfs itself moves).
    pub sysfs_uevent_send_full: fn(&str, &str, &str, &[(&str, &str)]) -> u64,
    // ---- char_dev / tty / pty rows (the console cluster) ----
    /// Boot stdio construction (init_std_fds_for_task's body).
    pub char_dev_init_std_fds: fn(&crate::fs::FdTable),
    /// file_hooks' stat fallback for char-device Files.
    pub char_dev_file_stat: fn(&File, &mut crate::fs::Stat) -> Option<i32>,
    /// devfs's console write bottom (interim until devfs joins the crate).
    pub char_dev_uart_write: fn(&[u8]) -> isize,
    /// The console tty's line discipline flags (ISIG/ECHO gating reads).
    pub tty_console_lflag: fn() -> u32,
    pub tty_console_echo_enabled: fn() -> bool,
    /// Serial input ingest (per input byte — cold by baud rate).
    pub tty_console_receive_byte: fn(u8),
    /// The console tty's foreground process group (0 = none).
    pub tty_console_fg_pgid: fn() -> u32,
    pub tty_set_console_sink_quiet: fn(bool),
    /// devfs console rows (interim until devfs joins the crate).
    pub tty_console_read_input: fn(&mut [u8], bool) -> isize,
    pub tty_console_output_translates_nl: fn() -> bool,
    pub tty_console_input_poll_ready: fn() -> bool,
    pub pty_devfs_register: fn(),
    pub pty_ptmx_open: fn(&File) -> i32,
    pub pty_slave_open: fn(&File, u32) -> i32,
    pub pty_devno_is_ptmx: fn(crate::fs::dev_t::DevNo) -> bool,
    pub pty_dev_ptmx_devno: fn() -> crate::fs::dev_t::DevNo,
    pub pty_slave_major: fn() -> u32,
    // ---- rootfs rows ----
    /// lib.rs boot's init_rootfs().
    pub rootfs_init: fn() -> Result<(), i32>,
    pub rootfs_create_root_inode: fn() -> Option<alloc::sync::Arc<Inode>>,
    /// initrd's unpack target (the InitrdRamfs view of the global sb).
    pub rootfs_initrd_sb: fn() -> Option<&'static dyn super::leaf_glue::InitrdRamfs>,
    /// The legacy read_file_from_rootfs (init.rs's config probe).
    pub rootfs_read_file: fn(&str) -> Option<alloc::vec::Vec<u8>>,
    // ---- tmpfs rows ----
    /// fsctx's mount-instance factory (devpts/securityfs/mqueue tag the
    /// fresh root with a type-ops twin).
    pub tmpfs_create_mount_instance: fn(Option<&str>, Option<TmpfsTagKind>) -> Option<alloc::sync::Arc<Inode>>,
    pub tmpfs_ensure_registered: fn(),
    /// mm/policy's file-identity probe (SHARED-fault routing).
    pub tmpfs_is_file: fn(&File) -> bool,
    /// vfs_fallocate's tmpfs arm (None = not tmpfs).
    pub tmpfs_fallocate: fn(&Inode, i32, u64, u64) -> Option<Result<(), i32>>,
    // ---- devfs rows ----
    pub devfs_init: fn(),
    /// The boot /dev mount (get_root_entry + create_root_inode combined).
    pub devfs_create_root_inode: fn() -> Option<alloc::sync::Arc<Inode>>,
    pub devfs_mkdir: fn(&str) -> Result<(), i32>,
    pub devfs_mknod: fn(&str, crate::fs::dev_t::DevNo, u32) -> Result<(), i32>,
    pub devfs_register_char_device: fn(crate::fs::dev_t::DevNo, &'static crate::fs::FileOps) -> Result<(), ()>,
    /// mm/policy's live-size probe for regular-registry files.
    pub devfs_reg_current_size: fn(&File) -> Option<u64>,
    /// mm/policy's SHARED fault page handoff (the ops-identity gate is
    /// folded into the row: None for non-registry files).
    pub devfs_shared_fault_file_page: fn(&File, usize) -> Option<u64>,
    /// vt's console-device backing (read/write/poll).
    pub devfs_condev_read: fn(&File, &mut [u8]) -> isize,
    pub devfs_condev_write: fn(&File, &[u8]) -> isize,
    pub devfs_condev_poll: fn(&File, u16) -> u16,
    /// The loop-device ops identity probe + value (loop_dev's ioctl
    /// dispatch and loop-control file).
    pub devfs_is_loop_ops: fn(&'static crate::fs::FileOps) -> bool,
    pub devfs_loop_file_ops: fn() -> &'static crate::fs::FileOps,
    /// CONDEV_OPS for the test-suite console fd.
    pub devfs_condev_ops: fn() -> &'static crate::fs::FileOps,
    /// procfs's mounts view (interim until procfs joins the crate).
    pub rootfs_sb_ref_interim: fn() -> Option<&'static dyn super::leaf_glue::InitrdRamfs>,
}

impl FsHooks {
    pub const EMPTY: FsHooks = FsHooks {
        notify: |_, _, _, _, _| {},
        notify_file: |_, _| {},
        notify_file_close: |_| {},
        alloc_cookie: || 0,
        pipe_stat: |_, _| None,
        pipe_get_sz: |_| None,
        pipe_set_sz: |_, _| Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
        pipe_oasync_register: |_, _| {},
        pipe_oasync_unregister: |_| {},
        pipe_diag_snapshot: |_| None,
        fifo_forget: |_, _| {},
        fifo_open: |_, _, _| Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        memfd_add_seals: |_, _| Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
        memfd_get_seals: |_| Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
        memfd_name_by_ino: |_| None,
        is_pipe_ops: |_| false,
        is_fifo_ops: |_| false,
        is_inotify_ops: |_| false,
        is_memfd_ops: |_| false,
        selinuxfs_root_inode: || None,
        inotify_max_queued_events: || 16384,
        inotify_set_max_queued_events: |_| {},
        inotify_fdinfo_watches: |_| None,
        leaf_mount_registers: &[],
        debugfs_create_dir: |_| false,
        debugfs_create_file: |_, _, _, _| false,
        debugfs_init: || Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        debugfs_root_present: || false,
        debugfs_create_root_inode: || None,
        debugfs_mount_boot: || Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        tracefs_create_dir: |_| false,
        tracefs_create_file: |_, _, _, _| false,
        tracefs_create_stream_file: |_, _, _| false,
        tracefs_root_present: || false,
        tracefs_init: || Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        tracefs_create_root_inode: || None,
        tracefs_mount_at: |_| Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        cgroup_init_cgroupfs: || Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        cgroup_create_root_inode: || None,
        cgroup_mount_cgroupfs: |_| Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        sysfs_init_sysfs: || Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        sysfs_mount_sysfs: || Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        sysfs_create_root_inode: || None,
        sysfs_publish_module: |_| {},
        sysfs_unpublish_module: |_| {},
        sysfs_register_drm_device: |_| {},
        sysfs_netdev_uevent: |_, _| 0,
        sysfs_register_block_disk_at: |_, _, _, _, _| 0,
        sysfs_register_input_event: |_| 0,
        sysfs_uevent_send_full: |_, _, _, _| 0,
        char_dev_init_std_fds: |_| {},
        char_dev_file_stat: |_, _| None,
        char_dev_uart_write: |_| 0,
        tty_console_lflag: || 0,
        tty_console_echo_enabled: || false,
        tty_console_receive_byte: |_| {},
        tty_console_fg_pgid: || 0,
        tty_set_console_sink_quiet: |_| {},
        tty_console_read_input: |_, _| 0,
        tty_console_output_translates_nl: || false,
        tty_console_input_poll_ready: || false,
        pty_devfs_register: || {},
        pty_ptmx_open: |_| -crate::errno::constants::ENOENT,
        pty_slave_open: |_, _| -crate::errno::constants::ENOENT,
        pty_devno_is_ptmx: |_| false,
        pty_dev_ptmx_devno: || crate::fs::dev_t::DevNo::new(5, 2),
        pty_slave_major: || 136,
        rootfs_init: || Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        rootfs_create_root_inode: || None,
        rootfs_initrd_sb: || None,
        rootfs_read_file: |_| None,
        tmpfs_create_mount_instance: |_, _| None,
        tmpfs_ensure_registered: || {},
        tmpfs_is_file: |_| false,
        tmpfs_fallocate: |_, _, _, _| None,
        devfs_init: || {},
        devfs_create_root_inode: || None,
        devfs_mkdir: |_| Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        devfs_mknod: |_, _, _| Err(crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        devfs_register_char_device: |_, _| Err(()),
        devfs_reg_current_size: |_| None,
        devfs_shared_fault_file_page: |_, _| None,
        devfs_condev_read: |_, _| 0,
        devfs_condev_write: |_, _| 0,
        devfs_condev_poll: |_, _| 0,
        devfs_is_loop_ops: |_| false,
        devfs_loop_file_ops: || &crate::fs::file::DIR_FILE_OPS,
        devfs_condev_ops: || &crate::fs::file::DIR_FILE_OPS,
        rootfs_sb_ref_interim: || None,
    };
}

pub static FS_HOOKS: crate::safety::slot::OnceSlot<FsHooks> =
    crate::safety::slot::OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins).
pub fn fs_register_hooks(hooks: FsHooks) {
    let _ = FS_HOOKS.publish(hooks);
}

#[inline]
pub fn hooks() -> &'static FsHooks {
    FS_HOOKS.get().unwrap_or(&FsHooks::EMPTY)
}

pub fn notify(parent: Option<&Inode>, inode: Option<&Inode>, mask: u32, name: Option<&[u8]>, cookie: u32) {
    (hooks().notify)(parent, inode, mask, name, cookie)
}
pub fn notify_file(file: &File, mask: u32) {
    (hooks().notify_file)(file, mask)
}
pub fn notify_file_close(file: &File) {
    (hooks().notify_file_close)(file)
}
pub fn alloc_cookie() -> u32 {
    (hooks().alloc_cookie)()
}

// ---- ergonomic accessors (the net_hooks shape) ----
pub fn is_pipe_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_pipe_ops)(ops) }
pub fn is_fifo_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_fifo_ops)(ops) }
pub fn is_inotify_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_inotify_ops)(ops) }
pub fn is_memfd_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_memfd_ops)(ops) }
pub fn pipe_diag_snapshot(file: &File) -> Option<(usize, usize, bool, bool)> { (hooks().pipe_diag_snapshot)(file) }
pub fn memfd_name_by_ino(ino: u64) -> Option<&'static str> { (hooks().memfd_name_by_ino)(ino) }
