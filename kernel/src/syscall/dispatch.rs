//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! System call dispatch module
//!
//! This module handles system call dispatch and common processing

use crate::arch::riscv64::pt_regs::PtRegs;
use super::*;

/// System call argument array type
pub type SyscallArgs = [u64; 6];

/// Get system call number from PtRegs
#[inline]
fn syscall_get_nr(regs: &PtRegs) -> u64 {
    regs.a7
}

/// Get system call arguments from PtRegs
#[inline]
fn syscall_get_arguments(regs: &PtRegs) -> SyscallArgs {
    [regs.orig_a0, regs.a1, regs.a2, regs.a3, regs.a4, regs.a5]
}

/// Set system call return value
#[inline]
fn syscall_set_return_value(regs: &mut PtRegs, value: u64) {
    regs.a0 = value;
}

/// System call entry function
///
/// Called by trap.rs, dispatches to specific system call handlers
pub extern "C" fn syscall_handler(regs: &mut PtRegs) {
    let syscall_no = syscall_get_nr(regs);
    let args = syscall_get_arguments(regs);

    crate::pr_debug!("syscall: pid={}, nr={}, args=[{:#x}, {:#x}, {:#x}]",
        crate::process::current_pid(), syscall_no, args[0], args[1], args[2]);

    // Dispatch based on system call number (sorted by number)
    let result: u64 = match syscall_no as u32 {
        // ==================== Linux AIO (NR 0-4) ====================
        0 => memory::sys_io_setup(args),       // io_setup
        1 => memory::sys_io_destroy(args),     // io_destroy
        2 => memory::sys_io_submit(args),      // io_submit
        3 => memory::sys_io_cancel(args),      // io_cancel
        4 => memory::sys_io_getevents(args),   // io_getevents

        // ==================== Extended Attributes (NR 5-16) ====================
        5 => file::sys_setxattr(args),         // setxattr
        6 => file::sys_lsetxattr(args),        // lsetxattr
        7 => file::sys_fsetxattr(args),        // fsetxattr
        8 => file::sys_getxattr(args),         // getxattr
        9 => file::sys_lgetxattr(args),        // lgetxattr
        10 => file::sys_fgetxattr(args),       // fgetxattr
        11 => file::sys_listxattr(args),       // listxattr
        12 => file::sys_llistxattr(args),      // llistxattr
        13 => file::sys_flistxattr(args),      // flistxattr
        14 => file::sys_removexattr(args),     // removexattr
        15 => file::sys_lremovexattr(args),    // lremovexattr
        16 => file::sys_fremovexattr(args),    // fremovexattr

        // ==================== File Operations ====================
        17 => file::sys_getcwd(args),
        18 => time::sys_lookup_dcookie(args),  // lookup_dcookie
        19 => misc::sys_eventfd2(args),        // eventfd2
        20 => misc::sys_epoll_create(args),    // epoll_create1
        21 => misc::sys_epoll_ctl(args),       // epoll_ctl
        22 => misc::sys_epoll_pwait(args),     // epoll_pwait
        26 => misc::sys_inotify_init1(args),   // inotify_init1
        27 => misc::sys_inotify_add_watch(args), // inotify_add_watch
        28 => misc::sys_inotify_rm_watch(args),  // inotify_rm_watch
        23 => io::sys_dup(args),               // dup
        24 => io::sys_dup3(args),              // dup3
        25 => io::sys_fcntl(args),             // fcntl
        29 => io::sys_ioctl(args),             // ioctl
        30 => process::sys_ioprio_set(args),   // ioprio_set
        31 => process::sys_ioprio_get(args),   // ioprio_get
        32 => io::sys_flock(args),             // flock
        33 => file::sys_mknodat(args),         // mknodat
        34 => file::sys_mkdirat(args),         // mkdirat
        35 => file::sys_unlinkat(args),        // unlinkat
        36 => file::sys_symlinkat(args),       // symlinkat
        37 => file::sys_linkat(args),          // linkat
        38 => file::sys_renameat(args),        // renameat
        39 => file::sys_umount(args),          // umount2
        40 => file::sys_mount(args),           // mount
        41 => process::sys_pivot_root(args),   // pivot_root
        42 => time::sys_nfsservctl(args),      // nfsservctl
        43 => file::sys_statfs(args),          // statfs
        44 => file::sys_fstatfs(args),         // fstatfs
        45 => file::sys_truncate(args),        // truncate
        46 => file::sys_ftruncate(args),       // ftruncate
        47 => file::sys_fallocate(args),       // fallocate
        48 => file::sys_faccessat(args),       // faccessat
        49 => file::sys_chdir(args),           // chdir
        50 => file::sys_fchdir(args),          // fchdir
        51 => file::sys_chroot(args),          // chroot
        52 => file::sys_fchmod(args),          // fchmod
        53 => file::sys_fchmodat(args),        // fchmodat
        54 => file::sys_fchownat(args),        // fchownat
        55 => file::sys_fchown(args),          // fchown
        56 => file::sys_openat(args),          // openat
        57 => file::sys_close(args),           // close
        58 => file::sys_vhangup(args),         // vhangup
        59 => io::sys_pipe2(args),             // pipe2
        60 => process::sys_quotactl(args),     // quotactl
        61 => file::sys_getdents64(args),      // getdents64
        62 => file::sys_lseek(args),           // lseek
        63 => io::sys_read(args),              // read
        64 => io::sys_write(args),             // write
        65 => io::sys_readv(args),             // readv
        66 => io::sys_writev(args),            // writev
        67 => io::sys_pread64(args),           // pread64
        68 => io::sys_pwrite64(args),          // pwrite64
        69 => io::sys_preadv(args),            // preadv
        70 => io::sys_pwritev(args),           // pwritev
        71 => io::sys_sendfile(args),          // sendfile
        72 => misc::sys_pselect6(args),        // pselect6
        73 => misc::sys_ppoll(args),           // ppoll
        74 => signal::sys_signalfd4(args),     // signalfd4
        75 => io::sys_vmsplice(args),          // vmsplice
        76 => io::sys_splice(args),            // splice
        77 => io::sys_tee(args),               // tee
        78 => file::sys_readlinkat(args),      // readlinkat
        79 => file::sys_fstatat(args),         // fstatat
        80 => file::sys_fstat(args),           // fstat
        81 => file::sys_sync(args),            // sync
        82 => file::sys_fsync(args),           // fsync
        83 => file::sys_fdatasync(args),       // fdatasync
        84 => file::sys_sync_file_range(args), // sync_file_range
        85 => misc::sys_timerfd_create(args),  // timerfd_create
        86 => misc::sys_timerfd_settime(args), // timerfd_settime
        87 => misc::sys_timerfd_gettime(args), // timerfd_gettime
        88 => file::sys_futimesat(args),       // utimensat
        89 => file::sys_acct(args),            // acct

        // ==================== Process Operations ====================
        90 => process::sys_capget(args),       // capget
        91 => process::sys_capset(args),       // capset
        92 => process::sys_personality(args),  // personality
        93 => process::sys_exit(args),         // exit
        94 => process::sys_exit(args),         // exit_group
        95 => process::sys_waitid(args),       // waitid
        96 => process::sys_set_tid_address(args, regs.tp),
        97 => process::sys_unshare(args),      // unshare
        98 => sched::sys_futex(args),          // futex
        99 => process::sys_set_robust_list(args),
        100 => time::sys_get_robust_list(args), // get_robust_list
        101 => time::sys_nanosleep(args),      // nanosleep
        102 => time::sys_getitimer(args),      // getitimer
        103 => time::sys_setitimer(args),      // setitimer
        104 => process::sys_kexec_load(args),  // kexec_load
        105 => process::sys_init_module(args), // init_module
        106 => process::sys_delete_module(args), // delete_module
        107 => time::sys_timer_create(args),   // timer_create
        108 => time::sys_timer_gettime(args),  // timer_gettime
        109 => time::sys_timer_getoverrun(args), // timer_getoverrun
        110 => time::sys_timer_settime(args),  // timer_settime
        111 => time::sys_timer_delete(args),   // timer_delete
        112 => time::sys_clock_settime(args),  // clock_settime
        113 => time::sys_clock_gettime(args),  // clock_gettime
        114 => time::sys_clock_getres(args),   // clock_getres
        115 => time::sys_clock_nanosleep(args),// clock_nanosleep
        116 => crate::printk::sys_syslog(args),  // syslog
        117 => process::sys_ptrace(args),      // ptrace
        118 => sched::sys_sched_setparam(args),// sched_setparam
        119 => sched::sys_sched_setscheduler(args), // sched_setscheduler
        120 => sched::sys_sched_getscheduler(args), // sched_getscheduler
        121 => sched::sys_sched_getaffinity(args), // sched_getaffinity
        122 => sched::sys_sched_getparam(args),// sched_getparam
        123 => sched::sys_sched_setaffinity(args), // sched_setaffinity
        124 => sched::sys_sched_yield(args),   // sched_yield
        125 => sched::sys_sched_get_priority_max(args), // sched_get_priority_max
        126 => sched::sys_sched_get_priority_min(args), // sched_get_priority_min
        127 => sched::sys_sched_rr_get_interval(args), // sched_rr_get_interval
        128 => signal::sys_restart_syscall(args), // restart_syscall
        129 => process::sys_kill(args),        // kill
        130 => signal::sys_tkill(args),        // tkill
        131 => process::sys_tgkill(args),      // tgkill
        132 => signal::sys_sigaltstack(args),  // sigaltstack
        133 => signal::sys_rt_sigsuspend(args), // rt_sigsuspend (Linux NR 133)
        134 => signal::sys_rt_sigaction(args), // rt_sigaction (Linux NR 134)
        135 => signal::sys_rt_sigprocmask(args),// rt_sigprocmask
        136 => signal::sys_sigpending(args),   // rt_sigpending (Linux NR 136)
        137 => process::sys_rt_sigtimedwait(args), // rt_sigtimedwait
        138 => process::sys_rt_sigqueueinfo(args), // rt_sigqueueinfo
        139 => signal::sys_rt_sigreturn(regs), // rt_sigreturn
        140 => sched::sys_getpriority(args),   // getpriority
        141 => sched::sys_setpriority(args),   // setpriority
        142 => process::sys_reboot(args),      // reboot
        143 => process::sys_setregid(args),    // setregid
        144 => process::sys_setgid(args),      // setgid
        145 => process::sys_setreuid(args),    // setreuid
        146 => process::sys_setuid(args),      // setuid
        147 => process::sys_setresuid(args),   // setresuid
        148 => process::sys_getresuid(args),   // getresuid
        149 => process::sys_setresgid(args),   // setresgid
        150 => process::sys_getresgid(args),   // getresgid
        151 => process::sys_setfsuid(args),    // setfsuid
        152 => process::sys_setfsgid(args),    // setfsgid
        153 => process::sys_times(args),       // times
        154 => process::sys_setpgid(args),     // setpgid
        155 => process::sys_getpgid(args),     // getpgid
        156 => process::sys_getsid(args),      // getsid
        157 => process::sys_setsid(args),      // setsid
        158 => process::sys_getgroups(args),   // getgroups
        159 => process::sys_setgroups(args),   // setgroups
        160 => process::sys_uname(args),       // newuname
        161 => process::sys_sethostname(args), // sethostname
        162 => process::sys_setdomainname(args), // setdomainname
        163 => process::sys_getrlimit(args),   // getrlimit
        164 => process::sys_setrlimit(args),   // setrlimit
        165 => process::sys_getrusage(args),   // getrusage
        166 => file::sys_umask(args),          // umask
        167 => process::sys_prctl(args),       // prctl
        168 => process::sys_getcpu(args),      // getcpu
        169 => time::sys_gettimeofday(args),   // gettimeofday
        170 => time::sys_settimeofday(args),   // settimeofday
        171 => time::sys_adjtimex(args),       // adjtimex
        172 => process::sys_getpid(args),      // getpid
        173 => process::sys_getppid(args),     // getppid
        174 => process::sys_getuid(args),      // getuid
        175 => process::sys_geteuid(args),     // geteuid
        176 => process::sys_getgid(args),      // getgid
        177 => process::sys_getegid(args),     // getegid
        178 => process::sys_gettid(args),      // gettid
        179 => process::sys_sysinfo(args),     // sysinfo

        // ==================== IPC Operations ====================
        180 => crate::ipc::posix_mq::sys_mq_open(args),      // mq_open
        181 => crate::ipc::posix_mq::sys_mq_unlink(args),    // mq_unlink
        182 => crate::ipc::posix_mq::sys_mq_timedsend(args), // mq_timedsend
        183 => crate::ipc::posix_mq::sys_mq_timedreceive(args), // mq_timedreceive
        184 => crate::ipc::posix_mq::sys_mq_notify(args),   // mq_notify
        185 => crate::ipc::posix_mq::sys_mq_getsetattr(args), // mq_getsetattr
        186 => crate::ipc::sysv_msg::sys_msgget(args),      // msgget
        187 => crate::ipc::sysv_msg::sys_msgctl(args),      // msgctl
        188 => crate::ipc::sysv_msg::sys_msgrcv(args),      // msgrcv
        189 => crate::ipc::sysv_msg::sys_msgsnd(args),      // msgsnd
        190 => crate::ipc::sysv_sem::sys_semget(args),      // semget
        191 => crate::ipc::sysv_sem::sys_semctl(args),      // semctl
        192 => crate::ipc::sysv_sem::sys_semtimedop(args),  // semtimedop
        193 => crate::ipc::sysv_sem::sys_semop(args),       // semop
        194 => crate::ipc::sysv_shm::sys_shmget(args),      // shmget
        195 => crate::ipc::sysv_shm::sys_shmctl(args),      // shmctl
        196 => crate::ipc::sysv_shm::sys_shmat(args),       // shmat
        197 => crate::ipc::sysv_shm::sys_shmdt(args),       // shmdt

        // ==================== Network Operations ====================
        198 => network::sys_socket(args),      // socket
        199 => network::sys_socketpair(args),  // socketpair
        200 => network::sys_bind(args),        // bind
        201 => network::sys_listen(args),      // listen
        202 => network::sys_accept(args),      // accept
        203 => network::sys_connect(args),     // connect
        204 => network::sys_getsockname(args), // getsockname
        205 => network::sys_getpeername(args), // getpeername
        206 => network::sys_sendto(args),      // sendto
        207 => network::sys_recvfrom(args),    // recvfrom
        208 => network::sys_setsockopt(args),  // setsockopt
        209 => network::sys_getsockopt(args),  // getsockopt
        210 => network::sys_shutdown(args),    // shutdown
        211 => network::sys_sendmsg(args),     // sendmsg
        212 => network::sys_recvmsg(args),     // recvmsg
        243 => network::sys_recvmmsg(args),    // recvmmsg
        242 => network::sys_accept4(args),     // accept4

        // ==================== Memory Operations ====================
        213 => file::sys_readahead(args),      // readahead
        214 => memory::sys_brk(args),          // brk
        215 => memory::sys_munmap(args),       // munmap
        216 => memory::sys_mremap(args),       // mremap
        222 => memory::sys_mmap(args),         // mmap
        223 => memory::sys_fadvise64(args),    // fadvise64
        224 => file::sys_swapon(args),         // swapon
        225 => file::sys_swapoff(args),        // swapoff
        226 => memory::sys_mprotect(args),     // mprotect
        227 => memory::sys_msync(args),        // msync
        228 => memory::sys_mlock(args),        // mlock
        229 => memory::sys_munlock(args),      // munlock
        230 => memory::sys_mlockall(args),     // mlockall
        231 => memory::sys_munlockall(args),   // munlockall
        232 => memory::sys_mincore(args),      // mincore
        233 => memory::sys_madvise(args),      // madvise
        234 => memory::sys_remap_file_pages(args), // remap_file_pages
        235 => memory::sys_mbind(args),        // mbind
        236 => memory::sys_get_mempolicy(args), // get_mempolicy
        237 => memory::sys_set_mempolicy(args), // set_mempolicy
        238 => memory::sys_migrate_pages(args), // migrate_pages
        239 => memory::sys_move_pages(args),   // move_pages
        240 => process::sys_perf_event_open(args), // perf_event_open

        // ==================== Process Operations (cont.) ====================
        220 => process::sys_clone(args),       // clone
        221 => process::sys_execve(args),      // execve
        260 => process::sys_wait4(args),       // wait4
        261 => process::sys_prlimit64(args),   // prlimit64
        268 => process::sys_setns(args),       // setns

        // ==================== RISC-V Specific ====================
        258 => process::sys_riscv_hwprobe(args), // riscv_hwprobe
        259 => process::sys_riscv_flush_icache(args), // riscv_flush_icache
        267 => process::sys_syncfs(args),      // syncfs

        // ==================== Scheduler Extended ====================
        274 => sched::sys_sched_setattr(args), // sched_setattr (Linux NR 274)
        275 => sched::sys_sched_getattr(args), // sched_getattr (Linux NR 275)

        // ==================== File Handle / Copy ====================
        264 => file::sys_name_to_handle_at(args), // name_to_handle_at
        265 => file::sys_open_by_handle_at(args), // open_by_handle_at
        266 => time::sys_clock_adjtime(args),  // clock_adjtime
        269 => network::sys_sendmmsg(args),    // sendmmsg
        270 => process::sys_process_vm_readv(args), // process_vm_readv
        271 => process::sys_process_vm_writev(args), // process_vm_writev
        272 => process::sys_kcmp(args),        // kcmp
        273 => process::sys_finit_module(args), // finit_module

        // ==================== Select/Poll/Epoll ====================
        276 => file::sys_renameat2(args),      // renameat2
        277 => process::sys_seccomp(args),     // seccomp
        278 => misc::sys_getrandom(args),      // getrandom
        279 => process::sys_memfd_create(args), // memfd_create
        280 => process::sys_bpf(args),         // bpf
        281 => process::sys_execveat(args),    // execveat
        282 => process::sys_userfaultfd(args), // userfaultfd
        283 => process::sys_membarrier(args),  // membarrier
        284 => memory::sys_mlock2(args),       // mlock2
        285 => file::sys_copy_file_range(args), // copy_file_range
        286 => file::sys_preadv2(args),        // preadv2
        287 => file::sys_pwritev2(args),       // pwritev2
        288 => memory::sys_pkey_mprotect(args), // pkey_mprotect
        289 => memory::sys_pkey_alloc(args),   // pkey_alloc
        290 => memory::sys_pkey_free(args),    // pkey_free (was dispatch for eventfd; eventfd NR differs on some archs)
        292 => memory::sys_io_pgetevents(args), // io_pgetevents
        293 => time::sys_rseq(args),           // rseq
        294 => process::sys_kexec_file_load(args), // kexec_file_load

        // ==================== Fanotify ====================
        262 => time::sys_fanotify_init(args),  // fanotify_init
        263 => time::sys_fanotify_mark(args),  // fanotify_mark

        // ==================== Others ====================
        290 => misc::sys_eventfd(args),        // eventfd (legacy NR)
        291 => file::sys_statx(args),          // statx
        437 => file::sys_openat2(args),        // openat2

        // ==================== _time64 variants (NR 403-423) ====================
        403 => time::sys_clock_gettime64(args),    // clock_gettime64
        404 => time::sys_clock_settime64(args),    // clock_settime64
        405 => time::sys_clock_adjtime64(args),    // clock_adjtime64
        406 => time::sys_clock_getres_time64(args), // clock_getres_time64
        407 => time::sys_clock_nanosleep_time64(args), // clock_nanosleep_time64
        408 => time::sys_timer_gettime64(args),   // timer_gettime64
        409 => time::sys_timer_settime64(args),   // timer_settime64
        410 => time::sys_timerfd_gettime64(args), // timerfd_gettime64
        411 => time::sys_timerfd_settime64(args), // timerfd_settime64
        412 => time::sys_utimensat_time64(args),  // utimensat_time64
        413 => time::sys_pselect6_time64(args),  // pselect6_time64
        414 => time::sys_ppoll_time64(args),     // ppoll_time64
        416 => time::sys_io_pgetevents_time64(args), // io_pgetevents_time64
        417 => time::sys_recvmmsg_time64(args),  // recvmmsg_time64
        418 => crate::ipc::posix_mq::sys_mq_timedsend(args), // mq_timedsend_time64
        419 => crate::ipc::posix_mq::sys_mq_timedreceive(args), // mq_timedreceive_time64
        420 => crate::ipc::sysv_sem::sys_semtimedop(args), // semtimedop_time64
        421 => time::sys_rt_sigtimedwait_time64(args), // rt_sigtimedwait_time64
        422 => time::sys_futex_time64(args),    // futex_time64
        423 => time::sys_sched_rr_get_interval_time64(args), // sched_rr_get_interval_time64

        // ==================== Latest Kernel Features (NR 424-470) ====================
        424 => process::sys_pidfd_send_signal(args), // pidfd_send_signal
        425 => process::sys_io_uring_setup(args),   // io_uring_setup
        426 => process::sys_io_uring_enter(args),   // io_uring_enter
        427 => process::sys_io_uring_register(args), // io_uring_register
        428 => file::sys_open_tree(args),       // open_tree
        429 => file::sys_move_mount(args),      // move_mount
        430 => file::sys_fsopen(args),         // fsopen
        431 => file::sys_fsconfig(args),       // fsconfig
        432 => file::sys_fsmount(args),        // fsmount
        433 => file::sys_fspick(args),         // fspick
        434 => process::sys_pidfd_open(args),   // pidfd_open
        435 => process::sys_clone3(args),       // clone3
        436 => process::sys_close_range(args),  // close_range
        438 => process::sys_pidfd_getfd(args),  // pidfd_getfd
        439 => process::sys_faccessat2(args),  // faccessat2
        440 => process::sys_process_madvise(args), // process_madvise
        441 => file::sys_epoll_pwait2(args),   // epoll_pwait2
        442 => file::sys_mount_setattr(args),  // mount_setattr
        443 => file::sys_quotactl_fd(args),   // quotactl_fd
        444 => file::sys_landlock_create_ruleset(args), // landlock_create_ruleset
        445 => file::sys_landlock_add_rule(args), // landlock_add_rule
        446 => file::sys_landlock_restrict_self(args), // landlock_restrict_self
        447 => process::sys_memfd_secret(args), // memfd_secret
        448 => process::sys_process_mrelease(args), // process_mrelease
        449 => file::sys_futex_waitv(args),     // futex_waitv
        450 => memory::sys_set_mempolicy_home_node(args), // set_mempolicy_home_node
        451 => file::sys_cachestat(args),       // cachestat
        452 => file::sys_fchmodat2(args),       // fchmodat2
        453 => file::sys_map_shadow_stack(args), // map_shadow_stack
        454 => file::sys_futex_wake(args),      // futex_wake
        455 => file::sys_futex_wait(args),      // futex_wait
        456 => file::sys_futex_requeue(args),   // futex_requeue
        457 => file::sys_statmount(args),       // statmount
        458 => file::sys_listmount(args),       // listmount
        459 => file::sys_lsm_get_self_attr(args), // lsm_get_self_attr
        460 => file::sys_lsm_set_self_attr(args), // lsm_set_self_attr
        461 => file::sys_lsm_list_modules(args), // lsm_list_modules
        462 => file::sys_mseal(args),          // mseal
        463 => file::sys_setxattrat(args),      // setxattrat
        464 => file::sys_getxattrat(args),      // getxattrat
        465 => file::sys_listxattrat(args),     // listxattrat
        466 => file::sys_removexattrat(args),   // removexattrat
        467 => file::sys_open_tree_attr(args),  // open_tree_attr
        468 => file::sys_file_getattr(args),    // file_getattr
        469 => file::sys_file_setattr(args),    // file_setattr
        470 => file::sys_listns(args),         // listns

        // ==================== Unimplemented System Calls ====================
        _ => {
            crate::println!("syscall: unknown syscall {} (args: {:#x}, {:#x}, {:#x})",
                syscall_no, args[0], args[1], args[2]);
            (-errno::ENOSYS) as u64
        }
    };

    syscall_set_return_value(regs, result);

    crate::pr_debug!("syscall: pid={}, nr={}, ret={:#x} ({})",
        crate::process::current_pid(), syscall_no, result,
        if (result as i64) < 0 { "error" } else { "ok" });
}
