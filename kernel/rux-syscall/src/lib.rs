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
    mm, module, perf, printk, process, safety, sched, security,
    signal, sync, timer, trace,
};
// The print macros (macro_rules exported at the framework root).
pub use rux::{pr_debug, pr_err, pr_info, pr_warn, println};
// The net service tree (crate-flip rung 2): re-exported at the crate
// root so the syscall service files' `crate::net::*` paths stay valid —
// the sibling-service dependency (syscall -> net) is one-way.
pub use rux_net::net;
// The alloc crate (no_std extern): re-exported at the root so the
// service files' alloc:: paths resolve.
extern crate alloc;


// The audited-edge zone stays FRAMEWORK-side; re-exported under the
// service tree's historical path (syscall/edges.rs's consumers).

// The service tree — NESTED under crate::syscall (the historical
// paths): the 10 files physically moved here at State B final (the
// lib no longer compiles them; the crate is the sole copy). The
// files' `use super::*;` pulls the glob re-exports below (the old
// shared mod.rs's shape).
pub mod syscall {
    // the service files (THIS crate's own — the sole copies)
    #[path = "io.rs"] pub mod io;
    #[path = "process.rs"] pub mod process;
    #[path = "memory.rs"] pub mod memory;
    #[path = "file.rs"] pub mod file;
    #[path = "signal.rs"] pub mod signal;
    #[path = "time.rs"] pub mod time;
    #[path = "network.rs"] pub mod network;
    #[path = "sched.rs"] pub mod sched;
    #[path = "misc.rs"] pub mod misc;
    #[cfg(feature = "x86_64")]
    #[path = "x86_compat.rs"] pub mod x86_compat;

    // The shared UAPI types the old mod.rs carried at the syscall
    // root (TimeVal, FdSet, ... — the service files' `use super::*`
    // found them there).

    // The shared UAPI types the old mod.rs carried at the syscall
    // root (TimeVal, FdSet — the service files' `use super::*`
    // found them there).
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

    // The framework's whole-module re-exports (the old mod.rs's
    // pub-use globs — the service files' `use super::*;` depends on
    // every one of them; sourced from the rux re-exports at this
    // crate's root).
    pub use crate::arch::*;
    pub use crate::config;
    pub use crate::console;
    pub use crate::dfx::*;
    pub use crate::drivers::*;
    pub use crate::fs::*;
    pub use crate::io_uring::*;
    pub use crate::ipc::*;
    pub use crate::list;
    pub use crate::mm::*;
    pub use crate::module;
    pub use crate::net::*;
    pub use crate::perf;
    pub use crate::printk;
    pub use crate::process::*;
    pub use crate::safety::*;
    pub use crate::sched::*;
    pub use crate::security::*;
    pub use crate::signal::*;
    pub use crate::sync::*;
    pub use crate::timer::*;
    pub use crate::trace;

    // the errno table (the shared mod.rs's re-export)
    pub mod errno {
        pub use crate::errno::constants::*;
    }

    // dispatch types the files reference (SyscallArgs)
    pub use rux::syscall::dispatch::{SyscallArgs, syscall_handler};

    // the seam modules under historical paths (edges via the lib's
    // syscall module)
    // the edges module's audited helpers (the service files call them
    // unqualified via use super::* — the old shared mod.rs's
    // `use super::edges;` + glob)
    pub use rux::syscall::edges;
    pub use rux::syscall::seccomp;
    pub use rux::syscall::support;
    pub use rux::syscall::dispatch;
}

// (No root-level aliases for the service modules — the names collide
// with the framework re-exports; register_all reaches them through
// crate::syscall::io etc.)

// The framework's syscall seam (dispatch + edges + seccomp + support)
// re-exported under the historical crate::syscall path via shim:
// The framework's syscall seam under explicit rux:: paths (the
// service tree owns the crate::syscall name here).
pub mod syscall_dispatch_shim {
    pub use rux::syscall::dispatch;
}
pub mod syscall_seam {
    pub use rux::syscall::edges;
    pub use rux::syscall::seccomp;
    pub use rux::syscall::support;
    pub use rux::syscall::dispatch;
}

/// Framework-side seam items under the historical paths (the service
/// files' `super::` and `crate::syscall::` references).
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
        (0, syscall::memory::sys_io_setup),
        (1, syscall::memory::sys_io_destroy),
        (2, syscall::memory::sys_io_submit),
        (3, syscall::memory::sys_io_cancel),
        (4, syscall::memory::sys_io_getevents),
        (5, syscall::file::sys_setxattr),
        (6, syscall::file::sys_lsetxattr),
        (7, syscall::file::sys_fsetxattr),
        (8, syscall::file::sys_getxattr),
        (9, syscall::file::sys_lgetxattr),
        (10, syscall::file::sys_fgetxattr),
        (11, syscall::file::sys_listxattr),
        (12, syscall::file::sys_llistxattr),
        (13, syscall::file::sys_flistxattr),
        (14, syscall::file::sys_removexattr),
        (15, syscall::file::sys_lremovexattr),
        (16, syscall::file::sys_fremovexattr),
        (17, syscall::file::sys_getcwd),
        (18, syscall::time::sys_lookup_dcookie),
        (19, syscall::misc::sys_eventfd2),
        (20, syscall::misc::sys_epoll_create1),
        (21, syscall::misc::sys_epoll_ctl),
        (22, syscall::misc::sys_epoll_pwait),
        (23, syscall::io::sys_dup),
        (24, syscall::io::sys_dup3),
        (25, syscall::io::sys_fcntl),
        (26, syscall::misc::sys_inotify_init1),
        (27, syscall::misc::sys_inotify_add_watch),
        (28, syscall::misc::sys_inotify_rm_watch),
        (29, syscall::io::sys_ioctl),
        (30, syscall::process::sys_ioprio_set),
        (31, syscall::process::sys_ioprio_get),
        (32, syscall::io::sys_flock),
        (33, syscall::file::sys_mknodat),
        (34, syscall::file::sys_mkdirat),
        (35, syscall::file::sys_unlinkat),
        (36, syscall::file::sys_symlinkat),
        (37, syscall::file::sys_linkat),
        (38, syscall::file::sys_renameat),
        (39, syscall::file::sys_umount),
        (40, syscall::file::sys_mount),
        (41, syscall::process::sys_pivot_root),
        (42, syscall::time::sys_nfsservctl),
        (43, syscall::file::sys_statfs),
        (44, syscall::file::sys_fstatfs),
        (45, syscall::file::sys_truncate),
        (46, syscall::file::sys_ftruncate),
        (47, syscall::file::sys_fallocate),
        (48, syscall::file::sys_faccessat),
        (49, syscall::file::sys_chdir),
        (50, syscall::file::sys_fchdir),
        (51, syscall::file::sys_chroot),
        (52, syscall::file::sys_fchmod),
        (53, syscall::file::sys_fchmodat),
        (54, syscall::file::sys_fchownat),
        (55, syscall::file::sys_fchown),
        (56, syscall::file::sys_openat),
        (57, syscall::file::sys_close),
        (58, syscall::file::sys_vhangup),
        (59, syscall::io::sys_pipe2),
        (60, syscall::process::sys_quotactl),
        (61, syscall::file::sys_getdents64),
        (62, syscall::file::sys_lseek),
        (63, syscall::io::sys_read),
        (64, syscall::io::sys_write),
        (65, syscall::io::sys_readv),
        (66, syscall::io::sys_writev),
        (67, syscall::io::sys_pread64),
        (68, syscall::io::sys_pwrite64),
        (69, syscall::io::sys_preadv),
        (70, syscall::io::sys_pwritev),
        (71, syscall::io::sys_sendfile),
        (72, syscall::misc::sys_pselect6),
        (73, syscall::misc::sys_ppoll),
        (74, syscall::signal::sys_signalfd4),
        (75, syscall::io::sys_vmsplice),
        (76, syscall::io::sys_splice),
        (77, syscall::io::sys_tee),
        (78, syscall::file::sys_readlinkat),
        (79, syscall::file::sys_fstatat),
        (80, syscall::file::sys_fstat),
        (81, syscall::file::sys_sync),
        (82, syscall::file::sys_fsync),
        (83, syscall::file::sys_fdatasync),
        (84, syscall::file::sys_sync_file_range),
        (85, syscall::misc::sys_timerfd_create),
        (86, syscall::misc::sys_timerfd_settime),
        (87, syscall::misc::sys_timerfd_gettime),
        (88, syscall::file::sys_futimesat),
        (89, syscall::file::sys_acct),
        (90, syscall::process::sys_capget),
        (91, syscall::process::sys_capset),
        (92, syscall::process::sys_personality),
        (93, syscall::process::sys_exit),
        (94, syscall::process::sys_exit_group),
        (95, syscall::process::sys_waitid),
        (96, syscall::process::sys_set_tid_address),
        (97, syscall::process::sys_unshare),
        (98, syscall::sched::sys_futex),
        (99, syscall::process::sys_set_robust_list),
        (100, syscall::time::sys_get_robust_list),
        (101, syscall::time::sys_nanosleep),
        (102, syscall::time::sys_getitimer),
        (103, syscall::time::sys_setitimer),
        (104, syscall::process::sys_kexec_load),
        (105, syscall::process::sys_init_module),
        (106, syscall::process::sys_delete_module),
        (107, syscall::time::sys_timer_create),
        (108, syscall::time::sys_timer_gettime),
        (109, syscall::time::sys_timer_getoverrun),
        (110, syscall::time::sys_timer_settime),
        (111, syscall::time::sys_timer_delete),
        (112, syscall::time::sys_clock_settime),
        (113, syscall::time::sys_clock_gettime),
        (114, syscall::time::sys_clock_getres),
        (115, syscall::time::sys_clock_nanosleep),
        (116, crate::printk::sys_syslog),
        (117, crate::process::ptrace::sys_ptrace),
        (118, syscall::sched::sys_sched_setparam),
        (119, syscall::sched::sys_sched_setscheduler),
        (120, syscall::sched::sys_sched_getscheduler),
        (121, syscall::sched::sys_sched_getparam),
        (122, syscall::sched::sys_sched_setaffinity),
        (123, syscall::sched::sys_sched_getaffinity),
        (124, syscall::sched::sys_sched_yield),
        (125, syscall::sched::sys_sched_get_priority_max),
        (126, syscall::sched::sys_sched_get_priority_min),
        (127, syscall::sched::sys_sched_rr_get_interval),
        (128, syscall::signal::sys_restart_syscall),
        (129, syscall::process::sys_kill),
        (130, syscall::signal::sys_tkill),
        (131, syscall::process::sys_tgkill),
        (132, syscall::signal::sys_sigaltstack),
        (133, syscall::signal::sys_rt_sigsuspend),
        (134, syscall::signal::sys_rt_sigaction),
        (135, syscall::signal::sys_rt_sigprocmask),
        (136, syscall::signal::sys_sigpending),
        (137, syscall::process::sys_rt_sigtimedwait),
        (138, syscall::process::sys_rt_sigqueueinfo),
        (140, syscall::sched::sys_setpriority),
        (141, syscall::sched::sys_getpriority),
        (142, syscall::process::sys_reboot),
        (143, syscall::process::sys_setregid),
        (144, syscall::process::sys_setgid),
        (145, syscall::process::sys_setreuid),
        (146, syscall::process::sys_setuid),
        (147, syscall::process::sys_setresuid),
        (148, syscall::process::sys_getresuid),
        (149, syscall::process::sys_setresgid),
        (150, syscall::process::sys_getresgid),
        (151, syscall::process::sys_setfsuid),
        (152, syscall::process::sys_setfsgid),
        (153, syscall::process::sys_times),
        (154, syscall::process::sys_setpgid),
        (155, syscall::process::sys_getpgid),
        (156, syscall::process::sys_getsid),
        (157, syscall::process::sys_setsid),
        (158, syscall::process::sys_getgroups),
        (159, syscall::process::sys_setgroups),
        (160, syscall::process::sys_uname),
        (161, syscall::process::sys_sethostname),
        (162, syscall::process::sys_setdomainname),
        (163, syscall::process::sys_getrlimit),
        (164, syscall::process::sys_setrlimit),
        (165, syscall::process::sys_getrusage),
        (166, syscall::file::sys_umask),
        (167, syscall::process::sys_prctl),
        (168, syscall::process::sys_getcpu),
        (169, syscall::time::sys_gettimeofday),
        (170, syscall::time::sys_settimeofday),
        (171, syscall::time::sys_adjtimex),
        (172, syscall::process::sys_getpid),
        (173, syscall::process::sys_getppid),
        (174, syscall::process::sys_getuid),
        (175, syscall::process::sys_geteuid),
        (176, syscall::process::sys_getgid),
        (177, syscall::process::sys_getegid),
        (178, syscall::process::sys_gettid),
        (179, syscall::process::sys_sysinfo),
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
        (198, syscall::network::sys_socket),
        (199, syscall::network::sys_socketpair),
        (200, syscall::network::sys_bind),
        (201, syscall::network::sys_listen),
        (202, syscall::network::sys_accept),
        (203, syscall::network::sys_connect),
        (204, syscall::network::sys_getsockname),
        (205, syscall::network::sys_getpeername),
        (206, syscall::network::sys_sendto),
        (207, syscall::network::sys_recvfrom),
        (208, syscall::network::sys_setsockopt),
        (209, syscall::network::sys_getsockopt),
        (210, syscall::network::sys_shutdown),
        (211, syscall::network::sys_sendmsg),
        (212, syscall::network::sys_recvmsg),
        (213, syscall::file::sys_readahead),
        (214, syscall::memory::sys_brk),
        (215, syscall::memory::sys_munmap),
        (216, syscall::memory::sys_mremap),
        (217, crate::ipc::keys::sys_add_key),
        (218, crate::ipc::keys::sys_request_key),
        (219, crate::ipc::keys::sys_keyctl),
        (220, syscall::process::sys_clone),
        (221, syscall::process::sys_execve),
        (222, syscall::memory::sys_mmap),
        (223, syscall::memory::sys_fadvise64),
        (224, syscall::file::sys_swapon),
        (225, syscall::file::sys_swapoff),
        (226, syscall::memory::sys_mprotect),
        (227, syscall::memory::sys_msync),
        (228, syscall::memory::sys_mlock),
        (229, syscall::memory::sys_munlock),
        (230, syscall::memory::sys_mlockall),
        (231, syscall::memory::sys_munlockall),
        (232, syscall::memory::sys_mincore),
        (233, syscall::memory::sys_madvise),
        (234, syscall::memory::sys_remap_file_pages),
        (235, syscall::memory::sys_mbind),
        (236, syscall::memory::sys_get_mempolicy),
        (237, syscall::memory::sys_set_mempolicy),
        (238, syscall::memory::sys_migrate_pages),
        (239, syscall::memory::sys_move_pages),
        (240, syscall::process::sys_rt_tgsigqueueinfo),
        (241, syscall::process::sys_perf_event_open),
        (242, syscall::network::sys_accept4),
        (243, syscall::network::sys_recvmmsg),
        (258, syscall::process::sys_riscv_hwprobe),
        (259, syscall::process::sys_riscv_flush_icache),
        (260, syscall::process::sys_wait4),
        (261, syscall::process::sys_prlimit64),
        (262, syscall::time::sys_fanotify_init),
        (263, syscall::time::sys_fanotify_mark),
        (264, syscall::file::sys_name_to_handle_at),
        (265, syscall::file::sys_open_by_handle_at),
        (266, syscall::time::sys_clock_adjtime),
        (267, syscall::process::sys_syncfs),
        (268, syscall::process::sys_setns),
        (269, syscall::network::sys_sendmmsg),
        (270, syscall::process::sys_process_vm_readv),
        (271, syscall::process::sys_process_vm_writev),
        (272, syscall::process::sys_kcmp),
        (273, syscall::process::sys_finit_module),
        (274, syscall::sched::sys_sched_setattr),
        (275, syscall::sched::sys_sched_getattr),
        (276, syscall::file::sys_renameat2),
        (277, syscall::process::sys_seccomp),
        (278, syscall::misc::sys_getrandom),
        (279, syscall::process::sys_memfd_create),
        (280, syscall::process::sys_bpf),
        (281, syscall::process::sys_execveat),
        (282, syscall::process::sys_userfaultfd),
        (283, syscall::process::sys_membarrier),
        (284, syscall::memory::sys_mlock2),
        (285, syscall::file::sys_copy_file_range),
        (286, syscall::file::sys_preadv2),
        (287, syscall::file::sys_pwritev2),
        (288, syscall::memory::sys_pkey_mprotect),
        (289, syscall::memory::sys_pkey_alloc),
        (290, syscall::memory::sys_pkey_free),
        (291, syscall::file::sys_statx),
        (292, syscall::memory::sys_io_pgetevents),
        (293, syscall::time::sys_rseq),
        (294, syscall::process::sys_kexec_file_load),
        (403, syscall::time::sys_clock_gettime64),
        (404, syscall::time::sys_clock_settime64),
        (405, syscall::time::sys_clock_adjtime64),
        (406, syscall::time::sys_clock_getres_time64),
        (407, syscall::time::sys_clock_nanosleep_time64),
        (408, syscall::time::sys_timer_gettime64),
        (409, syscall::time::sys_timer_settime64),
        (410, syscall::time::sys_timerfd_gettime64),
        (411, syscall::time::sys_timerfd_settime64),
        (412, syscall::time::sys_utimensat_time64),
        (413, syscall::time::sys_pselect6_time64),
        (414, syscall::time::sys_ppoll_time64),
        (416, syscall::time::sys_io_pgetevents_time64),
        (417, syscall::time::sys_recvmmsg_time64),
        (418, crate::ipc::posix_mq::sys_mq_timedsend),
        (419, crate::ipc::posix_mq::sys_mq_timedreceive),
        (420, crate::ipc::sysv_sem::sys_semtimedop),
        (421, syscall::time::sys_rt_sigtimedwait_time64),
        (422, syscall::time::sys_futex_time64),
        (423, syscall::time::sys_sched_rr_get_interval_time64),
        (424, syscall::process::sys_pidfd_send_signal),
        (425, syscall::process::sys_io_uring_setup),
        (426, syscall::process::sys_io_uring_enter),
        (427, syscall::process::sys_io_uring_register),
        (428, syscall::file::sys_open_tree),
        (429, syscall::file::sys_move_mount_tap),
        (430, syscall::file::sys_fsopen),
        (431, syscall::file::sys_fsconfig),
        (432, syscall::file::sys_fsmount),
        (433, syscall::file::sys_fspick),
        (434, syscall::process::sys_pidfd_open),
        (435, syscall::process::sys_clone3),
        (436, syscall::process::sys_close_range),
        (437, syscall::file::sys_openat2),
        (438, syscall::process::sys_pidfd_getfd),
        (439, syscall::process::sys_faccessat2),
        (440, syscall::process::sys_process_madvise),
        (441, syscall::file::sys_epoll_pwait2),
        (442, syscall::file::sys_mount_setattr_tap),
        (443, syscall::file::sys_quotactl_fd),
        (444, syscall::file::sys_landlock_create_ruleset),
        (445, syscall::file::sys_landlock_add_rule),
        (446, syscall::file::sys_landlock_restrict_self),
        (447, syscall::process::sys_memfd_secret),
        (448, syscall::process::sys_process_mrelease),
        (449, syscall::file::sys_futex_waitv),
        (450, syscall::memory::sys_set_mempolicy_home_node),
        (451, syscall::file::sys_cachestat),
        (452, syscall::file::sys_fchmodat2),
        (453, syscall::file::sys_map_shadow_stack),
        (454, syscall::file::sys_futex_wake),
        (455, syscall::file::sys_futex_wait),
        (456, syscall::file::sys_futex_requeue),
        (457, syscall::file::sys_statmount),
        (458, syscall::file::sys_listmount),
        (459, syscall::file::sys_lsm_get_self_attr),
        (460, syscall::file::sys_lsm_set_self_attr),
        (461, syscall::file::sys_lsm_list_modules),
        (462, syscall::file::sys_mseal),
        (463, syscall::file::sys_setxattrat),
        (464, syscall::file::sys_getxattrat),
        (465, syscall::file::sys_listxattrat),
        (466, syscall::file::sys_removexattrat),
        (467, syscall::file::sys_open_tree_attr),
        (468, syscall::file::sys_file_getattr),
        (469, syscall::file::sys_file_setattr),
        (470, syscall::file::sys_listns),
        (1180, syscall::time::sys_time),
    ];
    static REGS: &[(usize, rux::syscall::dispatch::SyscallRegsFn)] = &[
        (0, syscall::signal::sys_rt_sigreturn),
    ];
    syscall_register_tables(ARGS, REGS);
    register_hooks();
}

/// The dfx/event hook table (State B): the diagnostics + eventfd
/// identity that lib-side callers reach through
/// rux::syscall::support — implemented here where the types live.
fn register_hooks() {
    use rux::syscall::support::{SyscallHooks, syscall_register_hooks};
    let hooks = SyscallHooks {
        epoll_interest_snapshot: syscall::misc::epoll_interest_snapshot,
        timerfd_wheel_audit: syscall::misc::timerfd_wheel_audit,
        pid1_epoll_interest_dump: syscall::misc::pid1_epoll_interest_dump,
        kernel_random_bytes: syscall::misc::kernel_random_bytes,
        oh67_fd_ready: syscall::misc::oh67_fd_ready,
        jiffies_covering_deadline: syscall::misc::jiffies_covering_deadline,
        deadline_ns_to_hres_ticks: syscall::misc::deadline_ns_to_hres_ticks,
        timerfd_expire_notify: syscall::misc::timerfd_expire_notify,
        param_stall_dump: syscall::misc::param_stall_dump,
        is_eventfd_ops: |ops: &'static rux::fs::FileOps| core::ptr::eq(ops, &syscall::misc::EVENTFD_OPS),
        syscall_eio_line: |nr, args| syscall::io::syscall_eio_line(nr as usize, *args),
        epoll_dump_for_file: |fid| syscall::misc::epoll_dump_for_file(fid),
    };
    syscall_register_hooks(hooks);
}
