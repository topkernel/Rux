//! The SYSCALL SERVICE CRATE (crate-flip M2): the 12 syscall service
//! files, compiled here under a CRATE-LEVEL `#![forbid(unsafe_code)]`
//! — the compiler-enforced service boundary of the crate plan. The
//! framework lib (the `rux` crate) keeps the audited edges zone
//! (syscall/unsafe.rs) and the dispatch seam; this crate owns the
//! per-call implementations and registers them into the framework's
//! dispatch tables at boot (register_all, called by the BIN's
//! rust_main before the framework impl).
//!
//! The files live in this crate (src/), flat under its root; their
//! `crate::X` references resolve through the ROOT RE-EXPORTS below.

#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework crate's modules appear at THIS crate's
// root so the service files' crate::X paths resolve unchanged. Three
// names are NOT in the list because this crate's own items own them
// at the root: `sched` and `signal` (the files) and `errno` (the module
// lifted below: the constants at its top level, the shape the service
// files write as `errno::EFAULT`). The framework's are `rux::sched`,
// `rux::signal`, `rux::errno`.
pub use rux::{
    arch, config, console, debug, drivers, list,
    mm, module, perf, printk, safety, security,
    trace,
};
// The framework's sync module plus the moved futex diagnostics, merged
// at the sync crate's root (crate::sync::futex::X paths).
pub use ::sync;
// The timer service tree (cold-four lane: timer): the merged view
// (framework timekeeping core + the moved report halves) re-exported at
// this root — the sibling-service dependency (syscall -> timer) is
// one-way, the ipc precedent.
pub use ::timer;
// The io_uring service tree (cold-four lane: io_uring): the merged
// view (framework ring mechanism + the moved policy half) re-exported
// at this root — the sibling-service dependency (syscall -> io_uring)
// is one-way; the NR 425/426/427 wrappers below keep their
// crate::io_uring::X paths unchanged.
pub use ::io_uring;
// The ipc service tree (crate-flip rung: ipc): the SERVICE crate's
// merged ipc view (the movable policy + the framework's binder
// re-export) re-exported at this root — the sibling-service
// dependency (syscall -> ipc) is one-way, the net precedent.
pub use ::ipc;
// The fs service tree (crate-flip rung 3): the SERVICE crate's merged
// fs view (framework fs + the moved leaves) re-exported at this root —
// the sibling-service dependency (syscall -> fs-svc) is one-way, the
// net precedent.
pub use ::fs;
// The print macros (macro_rules exported at the framework root).
pub use rux::{pr_debug, pr_err, pr_info, pr_warn, println};
// The net service tree (crate-flip rung 2): re-exported at the crate
// root so the syscall service files' `crate::net::*` paths stay valid —
// the sibling-service dependency (syscall -> net) is one-way.
pub use ::net;
// The alloc crate (no_std extern): re-exported at the root so the
// service files' alloc:: paths resolve.
extern crate alloc;

// The service files (THIS crate's own — the sole copies), flat at the
// crate root (seed-core-design 2c.2). They still open with
// `use super::*;`: the names that imports are the crate root's own.
pub mod io;
pub mod process;
pub mod memory;
pub mod file;
pub mod signal;
pub mod time;
pub mod network;
pub mod sched;
pub mod misc;
#[cfg(feature = "x86_64")]
pub mod x86_compat;

// the moved diagnostic halves (cold-four lane)
#[path = "diag/strace1.rs"] pub mod diag_strace1;
#[path = "diag/seccomp_run.rs"] pub mod diag_seccomp_run;

// The shared UAPI types the old mod.rs carried at the syscall
// root (TimeVal, FdSet, FD_SETSIZE — the service files found them
// through `use super::*`).
/// Time value structure (struct timeval)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct TimeVal {
pub tv_sec: i64,
pub tv_usec: i64,
}

/// File descriptor set (fd_set)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct FdSet {
pub fds_bits: [u64; 16],
}

impl FdSet {
pub const fn new() -> Self {
    Self { fds_bits: [0; 16] }
}

pub fn set(&mut self, fd: i32) {
    if fd >= 0 && (fd as usize) < 1024 {
        let fd = fd as usize;
        self.fds_bits[fd / 64] |= 1 << (fd % 64);
    }
}

pub fn clear(&mut self, fd: i32) {
    if fd >= 0 && (fd as usize) < 1024 {
        let fd = fd as usize;
        self.fds_bits[fd / 64] &= !(1 << (fd % 64));
    }
}

pub fn is_set(&self, fd: i32) -> bool {
    if fd >= 0 && (fd as usize) < 1024 {
        let fd = fd as usize;
        (self.fds_bits[fd / 64] & (1 << (fd % 64))) != 0
    } else {
        false
    }
}

pub fn zero(&mut self) {
    self.fds_bits = [0; 16];
}
}

/// File descriptor count limit for select-style syscalls.
///
/// FD_SETSIZE is fixed at 1024 to match the FdSet layout above (16 × u64 =
/// 1024 bits) and the asm-generic ABI. Taking it from config would silently
/// break the bit layout whenever the config value differs (review 批次1).
pub const FD_SETSIZE: i32 = 1024;

// the errno table (the shared mod.rs's re-export)
pub mod errno {
    pub use rux::errno::constants::*;
}

// The audited-edge zone stays FRAMEWORK-side (rux::syscall); re-exported
// at the crate root, where the service files' `super::` paths and the
// sibling crates find it.
pub use rux::syscall::dispatch::{SyscallArgs, syscall_handler};
pub use rux::syscall::r#unsafe;
pub use rux::syscall::seccomp;
pub use rux::syscall::support;
pub use rux::syscall::dispatch;


// The framework's syscall seam (dispatch + edges + seccomp + support)
// under explicit names, for callers outside the service files.
pub mod syscall_dispatch_shim {
    pub use rux::syscall::dispatch;
}
pub mod syscall_seam {
    pub use rux::syscall::r#unsafe;
    pub use rux::syscall::seccomp;
    pub use rux::syscall::support;
    pub use rux::syscall::dispatch;
}

/// (Empty since the service tree was flattened; the seam items are at
/// the crate root.)
pub mod syscall_shim {
}

/// The composition registration (called by the BIN's rust_main before
/// the framework impl): fills the framework's two dispatch OnceSlot
/// tables from THIS crate's compiled sys_* fns (the 381-arm inventory:
/// the args-family here today via the macro; the fully-qualified arms
/// and the regs-variant follow in the M3-B mechanical pass — the table
/// lookup falls back to the in-lib match for any unregistered nr, so
/// partial registration is always sound).
pub fn register_all() {
    use rux::syscall::dispatch::{syscall_register_tables, SyscallArgsFn};
    static ARGS: &[(usize, SyscallArgsFn)] = &[
        (0, memory::sys_io_setup),
        (1, memory::sys_io_destroy),
        (2, memory::sys_io_submit),
        (3, memory::sys_io_cancel),
        (4, memory::sys_io_getevents),
        (5, file::sys_setxattr),
        (6, file::sys_lsetxattr),
        (7, file::sys_fsetxattr),
        (8, file::sys_getxattr),
        (9, file::sys_lgetxattr),
        (10, file::sys_fgetxattr),
        (11, file::sys_listxattr),
        (12, file::sys_llistxattr),
        (13, file::sys_flistxattr),
        (14, file::sys_removexattr),
        (15, file::sys_lremovexattr),
        (16, file::sys_fremovexattr),
        (17, file::sys_getcwd),
        (18, time::sys_lookup_dcookie),
        (19, misc::sys_eventfd2),
        (20, misc::sys_epoll_create1),
        (21, misc::sys_epoll_ctl),
        (22, misc::sys_epoll_pwait),
        (23, io::sys_dup),
        (24, io::sys_dup3),
        (25, io::sys_fcntl),
        (26, misc::sys_inotify_init1),
        (27, misc::sys_inotify_add_watch),
        (28, misc::sys_inotify_rm_watch),
        (29, io::sys_ioctl),
        (30, process::sys_ioprio_set),
        (31, process::sys_ioprio_get),
        (32, io::sys_flock),
        (33, file::sys_mknodat),
        (34, file::sys_mkdirat),
        (35, file::sys_unlinkat),
        (36, file::sys_symlinkat),
        (37, file::sys_linkat),
        (38, file::sys_renameat),
        (39, file::sys_umount),
        (40, file::sys_mount),
        (41, process::sys_pivot_root),
        (42, time::sys_nfsservctl),
        (43, file::sys_statfs),
        (44, file::sys_fstatfs),
        (45, file::sys_truncate),
        (46, file::sys_ftruncate),
        (47, file::sys_fallocate),
        (48, file::sys_faccessat),
        (49, file::sys_chdir),
        (50, file::sys_fchdir),
        (51, file::sys_chroot),
        (52, file::sys_fchmod),
        (53, file::sys_fchmodat),
        (54, file::sys_fchownat),
        (55, file::sys_fchown),
        (56, file::sys_openat),
        (57, file::sys_close),
        (58, file::sys_vhangup),
        (59, io::sys_pipe2),
        (60, process::sys_quotactl),
        (61, file::sys_getdents64),
        (62, file::sys_lseek),
        (63, io::sys_read),
        (64, io::sys_write),
        (65, io::sys_readv),
        (66, io::sys_writev),
        (67, io::sys_pread64),
        (68, io::sys_pwrite64),
        (69, io::sys_preadv),
        (70, io::sys_pwritev),
        (71, io::sys_sendfile),
        (72, misc::sys_pselect6),
        (73, misc::sys_ppoll),
        (74, signal::sys_signalfd4),
        (75, io::sys_vmsplice),
        (76, io::sys_splice),
        (77, io::sys_tee),
        (78, file::sys_readlinkat),
        (79, file::sys_fstatat),
        (80, file::sys_fstat),
        (81, file::sys_sync),
        (82, file::sys_fsync),
        (83, file::sys_fdatasync),
        (84, file::sys_sync_file_range),
        (85, misc::sys_timerfd_create),
        (86, misc::sys_timerfd_settime),
        (87, misc::sys_timerfd_gettime),
        (88, file::sys_futimesat),
        (89, file::sys_acct),
        (90, process::sys_capget),
        (91, process::sys_capset),
        (92, process::sys_personality),
        (93, process::sys_exit),
        (94, process::sys_exit_group),
        (95, process::sys_waitid),
        (96, process::sys_set_tid_address),
        (97, process::sys_unshare),
        (98, sched::sys_futex),
        (99, process::sys_set_robust_list),
        (100, time::sys_get_robust_list),
        (101, time::sys_nanosleep),
        (102, time::sys_getitimer),
        (103, time::sys_setitimer),
        (104, process::sys_kexec_load),
        (105, process::sys_init_module),
        (106, process::sys_delete_module),
        (107, time::sys_timer_create),
        (108, time::sys_timer_gettime),
        (109, time::sys_timer_getoverrun),
        (110, time::sys_timer_settime),
        (111, time::sys_timer_delete),
        (112, time::sys_clock_settime),
        (113, time::sys_clock_gettime),
        (114, time::sys_clock_getres),
        (115, time::sys_clock_nanosleep),
        (116, crate::printk::sys_syslog),
        (117, ::process::ptrace::sys_ptrace),
        (118, sched::sys_sched_setparam),
        (119, sched::sys_sched_setscheduler),
        (120, sched::sys_sched_getscheduler),
        (121, sched::sys_sched_getparam),
        (122, sched::sys_sched_setaffinity),
        (123, sched::sys_sched_getaffinity),
        (124, sched::sys_sched_yield),
        (125, sched::sys_sched_get_priority_max),
        (126, sched::sys_sched_get_priority_min),
        (127, sched::sys_sched_rr_get_interval),
        (128, signal::sys_restart_syscall),
        (129, process::sys_kill),
        (130, signal::sys_tkill),
        (131, process::sys_tgkill),
        (132, signal::sys_sigaltstack),
        (133, signal::sys_rt_sigsuspend),
        (134, signal::sys_rt_sigaction),
        (135, signal::sys_rt_sigprocmask),
        (136, signal::sys_sigpending),
        (137, process::sys_rt_sigtimedwait),
        (138, process::sys_rt_sigqueueinfo),
        (140, sched::sys_setpriority),
        (141, sched::sys_getpriority),
        (142, process::sys_reboot),
        (143, process::sys_setregid),
        (144, process::sys_setgid),
        (145, process::sys_setreuid),
        (146, process::sys_setuid),
        (147, process::sys_setresuid),
        (148, process::sys_getresuid),
        (149, process::sys_setresgid),
        (150, process::sys_getresgid),
        (151, process::sys_setfsuid),
        (152, process::sys_setfsgid),
        (153, process::sys_times),
        (154, process::sys_setpgid),
        (155, process::sys_getpgid),
        (156, process::sys_getsid),
        (157, process::sys_setsid),
        (158, process::sys_getgroups),
        (159, process::sys_setgroups),
        (160, process::sys_uname),
        (161, process::sys_sethostname),
        (162, process::sys_setdomainname),
        (163, process::sys_getrlimit),
        (164, process::sys_setrlimit),
        (165, process::sys_getrusage),
        (166, file::sys_umask),
        (167, process::sys_prctl),
        (168, process::sys_getcpu),
        (169, time::sys_gettimeofday),
        (170, time::sys_settimeofday),
        (171, time::sys_adjtimex),
        (172, process::sys_getpid),
        (173, process::sys_getppid),
        (174, process::sys_getuid),
        (175, process::sys_geteuid),
        (176, process::sys_getgid),
        (177, process::sys_getegid),
        (178, process::sys_gettid),
        (179, process::sys_sysinfo),
        (180, crate::ipc::posix_mq::sys_mq_open),
        (181, crate::ipc::posix_mq::sys_mq_unlink),
        (182, crate::ipc::posix_mq::sys_mq_timedsend),
        (183, crate::ipc::posix_mq::sys_mq_timedreceive),
        (184, crate::ipc::posix_mq::sys_mq_notify),
        (185, crate::ipc::posix_mq::sys_mq_getsetattr),
        (186, crate::ipc::sysv_msg::sys_msgget),
        (187, crate::ipc::sysv_msg::sys_msgctl),
        (188, crate::ipc::sysv_msg::sys_msgrcv),
        (189, crate::ipc::sysv_msg::sys_msgsnd),
        (190, crate::ipc::sysv_sem::sys_semget),
        (191, crate::ipc::sysv_sem::sys_semctl),
        (192, crate::ipc::sysv_sem::sys_semtimedop),
        (193, crate::ipc::sysv_sem::sys_semop),
        (194, crate::ipc::sysv_shm::sys_shmget),
        (195, crate::ipc::sysv_shm::sys_shmctl),
        (196, crate::ipc::sysv_shm::sys_shmat),
        (197, crate::ipc::sysv_shm::sys_shmdt),
        (198, network::sys_socket),
        (199, network::sys_socketpair),
        (200, network::sys_bind),
        (201, network::sys_listen),
        (202, network::sys_accept),
        (203, network::sys_connect),
        (204, network::sys_getsockname),
        (205, network::sys_getpeername),
        (206, network::sys_sendto),
        (207, network::sys_recvfrom),
        (208, network::sys_setsockopt),
        (209, network::sys_getsockopt),
        (210, network::sys_shutdown),
        (211, network::sys_sendmsg),
        (212, network::sys_recvmsg),
        (213, file::sys_readahead),
        (214, memory::sys_brk),
        (215, memory::sys_munmap),
        (216, memory::sys_mremap),
        (217, crate::ipc::keys::sys_add_key),
        (218, crate::ipc::keys::sys_request_key),
        (219, crate::ipc::keys::sys_keyctl),
        (220, process::sys_clone),
        (221, process::sys_execve),
        (222, memory::sys_mmap),
        (223, memory::sys_fadvise64),
        (224, file::sys_swapon),
        (225, file::sys_swapoff),
        (226, memory::sys_mprotect),
        (227, memory::sys_msync),
        (228, memory::sys_mlock),
        (229, memory::sys_munlock),
        (230, memory::sys_mlockall),
        (231, memory::sys_munlockall),
        (232, memory::sys_mincore),
        (233, memory::sys_madvise),
        (234, memory::sys_remap_file_pages),
        (235, memory::sys_mbind),
        (236, memory::sys_get_mempolicy),
        (237, memory::sys_set_mempolicy),
        (238, memory::sys_migrate_pages),
        (239, memory::sys_move_pages),
        (240, process::sys_rt_tgsigqueueinfo),
        (241, process::sys_perf_event_open),
        (242, network::sys_accept4),
        (243, network::sys_recvmmsg),
        (258, process::sys_riscv_hwprobe),
        (259, process::sys_riscv_flush_icache),
        (260, process::sys_wait4),
        (261, process::sys_prlimit64),
        (262, time::sys_fanotify_init),
        (263, time::sys_fanotify_mark),
        (264, file::sys_name_to_handle_at),
        (265, file::sys_open_by_handle_at),
        (266, time::sys_clock_adjtime),
        (267, process::sys_syncfs),
        (268, process::sys_setns),
        (269, network::sys_sendmmsg),
        (270, process::sys_process_vm_readv),
        (271, process::sys_process_vm_writev),
        (272, process::sys_kcmp),
        (273, process::sys_finit_module),
        (274, sched::sys_sched_setattr),
        (275, sched::sys_sched_getattr),
        (276, file::sys_renameat2),
        (277, process::sys_seccomp),
        (278, misc::sys_getrandom),
        (279, process::sys_memfd_create),
        (280, process::sys_bpf),
        (281, process::sys_execveat),
        (282, process::sys_userfaultfd),
        (283, process::sys_membarrier),
        (284, memory::sys_mlock2),
        (285, file::sys_copy_file_range),
        (286, file::sys_preadv2),
        (287, file::sys_pwritev2),
        (288, memory::sys_pkey_mprotect),
        (289, memory::sys_pkey_alloc),
        (290, memory::sys_pkey_free),
        (291, file::sys_statx),
        (292, memory::sys_io_pgetevents),
        (293, time::sys_rseq),
        (294, process::sys_kexec_file_load),
        (403, time::sys_clock_gettime64),
        (404, time::sys_clock_settime64),
        (405, time::sys_clock_adjtime64),
        (406, time::sys_clock_getres_time64),
        (407, time::sys_clock_nanosleep_time64),
        (408, time::sys_timer_gettime64),
        (409, time::sys_timer_settime64),
        (410, time::sys_timerfd_gettime64),
        (411, time::sys_timerfd_settime64),
        (412, time::sys_utimensat_time64),
        (413, time::sys_pselect6_time64),
        (414, time::sys_ppoll_time64),
        (416, time::sys_io_pgetevents_time64),
        (417, time::sys_recvmmsg_time64),
        (418, crate::ipc::posix_mq::sys_mq_timedsend),
        (419, crate::ipc::posix_mq::sys_mq_timedreceive),
        (420, crate::ipc::sysv_sem::sys_semtimedop),
        (421, time::sys_rt_sigtimedwait_time64),
        (422, time::sys_futex_time64),
        (423, time::sys_sched_rr_get_interval_time64),
        (424, process::sys_pidfd_send_signal),
        (425, process::sys_io_uring_setup),
        (426, process::sys_io_uring_enter),
        (427, process::sys_io_uring_register),
        (428, file::sys_open_tree),
        (429, file::sys_move_mount_tap),
        (430, file::sys_fsopen),
        (431, file::sys_fsconfig),
        (432, file::sys_fsmount),
        (433, file::sys_fspick),
        (434, process::sys_pidfd_open),
        (435, process::sys_clone3),
        (436, process::sys_close_range),
        (437, file::sys_openat2),
        (438, process::sys_pidfd_getfd),
        (439, process::sys_faccessat2),
        (440, process::sys_process_madvise),
        (441, file::sys_epoll_pwait2),
        (442, file::sys_mount_setattr_tap),
        (443, file::sys_quotactl_fd),
        (444, file::sys_landlock_create_ruleset),
        (445, file::sys_landlock_add_rule),
        (446, file::sys_landlock_restrict_self),
        (447, process::sys_memfd_secret),
        (448, process::sys_process_mrelease),
        (449, file::sys_futex_waitv),
        (450, memory::sys_set_mempolicy_home_node),
        (451, file::sys_cachestat),
        (452, file::sys_fchmodat2),
        (453, file::sys_map_shadow_stack),
        (454, file::sys_futex_wake),
        (455, file::sys_futex_wait),
        (456, file::sys_futex_requeue),
        (457, file::sys_statmount),
        (458, file::sys_listmount),
        (459, file::sys_lsm_get_self_attr),
        (460, file::sys_lsm_set_self_attr),
        (461, file::sys_lsm_list_modules),
        (462, file::sys_mseal),
        (463, file::sys_setxattrat),
        (464, file::sys_getxattrat),
        (465, file::sys_listxattrat),
        (466, file::sys_removexattrat),
        (467, file::sys_open_tree_attr),
        (468, file::sys_file_getattr),
        (469, file::sys_file_setattr),
        (470, file::sys_listns),
        (1180, time::sys_time),
        // Legacy x86_64-native numbers with no asm-generic slot: the
        // dispatch preamble arms route to these via free-table entries
        // (never issued as syscall numbers themselves).
        (1181, file::sys_utime),
        (1182, file::sys_sysfs),
        // The same-pattern arms the State-B-final sweep mis-wired to
        // occupied generic slots (poll->fsetxattr@7, select->dup@23,
        // pause->mkdirat@34, alarm->linkat@37, getdents->getpriority@141,
        // epoll_create/epoll_wait/utimes/futimesat->io_setup@0): their
        // real handlers, at their own free slots.
        (1183, misc::sys_poll),
        (1184, misc::sys_select),
        (1185, signal::sys_pause),
        (1186, time::sys_alarm),
        (1187, file::sys_getdents),
        (1188, misc::sys_epoll_create),
        (1189, misc::sys_epoll_wait),
        (1190, file::sys_utimes),
        (1191, file::sys_futimesat_native),
    ];
    static REGS: &[(usize, rux::syscall::dispatch::SyscallRegsFn)] = &[
        (0, signal::sys_rt_sigreturn),
    ];
    syscall_register_tables(ARGS, REGS);
    register_hooks();
}

/// The debug/event hook table (State B): the diagnostics + eventfd
/// identity that lib-side callers reach through
/// rux::syscall::support — implemented here where the types live.
fn register_hooks() {
    use rux::syscall::support::{SyscallHooks, syscall_register_hooks};
    let hooks = SyscallHooks {
        epoll_interest_snapshot: misc::epoll_interest_snapshot,
        timerfd_wheel_audit: misc::timerfd_wheel_audit,
        pid1_epoll_interest_dump: misc::pid1_epoll_interest_dump,
        kernel_random_bytes: misc::kernel_random_bytes,
        oh67_fd_ready: misc::oh67_fd_ready,
        jiffies_covering_deadline: misc::jiffies_covering_deadline,
        deadline_ns_to_hres_ticks: misc::deadline_ns_to_hres_ticks,
        timerfd_expire_notify: misc::timerfd_expire_notify,
        param_stall_dump: misc::param_stall_dump,
        is_eventfd_ops: |ops: &'static rux::fs::types::FileOps| core::ptr::eq(ops, &misc::EVENTFD_OPS),
        syscall_eio_line: |nr, args| io::syscall_eio_line(nr as usize, *args),
        epoll_dump_for_file: |fid| misc::epoll_dump_for_file(fid),
        seccomp_run: diag_seccomp_run::seccomp_run,
        strace1_line: diag_strace1::strace1_line,
    };
    syscall_register_hooks(hooks);
}
