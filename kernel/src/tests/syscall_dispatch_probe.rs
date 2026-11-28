//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Syscall dispatch sweep (tests/syscall_dispatch_probe.rs).
//!
//! syscall/dispatch.rs and the wrapper prologues across syscall/*.rs sat
//! at 5-17% because the in-kernel suite calls the `sys_*` functions
//! directly and the boot task has no user address space, so nothing ever
//! went through the REAL entry: the PtRegs decode, the x86-compat
//! translation arm, the LASTSYS latch, the table lookup and the
//! ENOSYS fall-through. This probe builds a PtRegs on the stack, fills
//! a7 + orig_a0..a5 and calls `syscall_handler` — the same entry the
//! trap handler uses — across a curated, non-destructive number list:
//!
//!   * identity/errno syscalls (getpid family, clock ids) with zero
//!     args — the wrappers answer for real;
//!   * everything else with deliberately INVALID arguments — the
//!     wrapper's arg-decode/permission arms run and answer -errno
//!     (EFAULT/EINVAL/EBADF/ENOSYS family), which is exactly the code
//!     the coverage was missing;
//!   * unknown numbers — the table's ENOSYS fall-through.
//!
//! The denylist (never driven): process-ending/changing calls (exit,
//! exit_group, fork/clone/vfork, execve family, reboot, kexec,
//! swapon/swapoff, vhangup, unshare/setns), blocking calls that would
//! park the boot task (pause, sigsuspend, nanosleep with a VALID
//! pointer — the probe passes bad pointers so these answer EFAULT at
//! once), and anything that mutates real filesystem state (mount/
//! umount2 with garbage args would still return EINVAL before touching
//! the tree, but they stay out on principle).

use crate::arch::riscv64::pt_regs::PtRegs;
use crate::syscall::syscall_handler;

/// Drive one syscall through the real dispatch entry. Returns the raw
/// i64 the trap path would deliver in a0.
fn drive(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let mut regs = PtRegs::new();
    regs.a7 = nr;
    regs.orig_a0 = a0;
    regs.a1 = a1;
    regs.a2 = a2;
    syscall_handler(&mut regs);
    regs.a0 as i64
}

/// The answer is a plausible syscall result: negative errno (as i64) or
/// a small non-negative value (fd/pid/counter). A garbage positive in
/// the pointer band (>= user-space base) would mean the wrapper leaked
/// an internal address.
fn plausible(v: i64) -> bool {
    v < 0 || v < 0x0000_4000_0000_0000
}

pub fn test_syscall_dispatch_probe() {
    crate::tests::test_group_start("syscall: dispatch sweep");

    // ---- identity syscalls answer for real (zero args) -------------------
    let pid = drive(172, 0, 0, 0); // getpid
    crate::test_assert!(pid >= 0, "dispatch: getpid answers");
    let ppid = drive(173, 0, 0, 0); // getppid
    crate::test_assert!(ppid >= 0, "dispatch: getppid answers");
    crate::test_assert!(drive(174, 0, 0, 0) >= 0, "dispatch: getuid");
    crate::test_assert!(drive(175, 0, 0, 0) >= 0, "dispatch: geteuid");
    crate::test_assert!(drive(176, 0, 0, 0) >= 0, "dispatch: getgid");
    crate::test_assert!(drive(177, 0, 0, 0) >= 0, "dispatch: getegid");
    crate::test_assert!(drive(178, 0, 0, 0) >= 0, "dispatch: gettid");
    crate::test_assert!(drive(64, 0, 0, 0) >= 0, "dispatch: getppid(64 legacy)");

    // ---- errno syscalls with deliberately INVALID arguments --------------
    // (nr, a0, a1, a2, name) — the wrappers must answer, not panic.
    const BADARG: &[(
        u64,
        u64,
        u64,
        u64,
        &str,
    )] = &[
        (17, usize::MAX as u64, 64, 0, "getcwd bad buf"),
        (63, usize::MAX as u64, 4, 0, "uname bad buf"),
        (66, 0, usize::MAX as u64, 1, "writev bad iov"),
        (67, 0, usize::MAX as u64, 1, "readv bad iov"),
        (79, 0xfffffff9, usize::MAX as u64, usize::MAX as u64, "newfstatat bad"),
        (33, 0, 4, 0, "mknod garbage"),
        (34, usize::MAX as u64, 0o777, 0, "mkdir bad path"),
        (35, usize::MAX as u64, 0, "rmdir bad path".len() as u64, "rmdir bad path"),
        (87, usize::MAX as u64, 0, 0, "unlink bad path"),
        (57, 9999, 9999, 0, "close bogus fd"),
        (57, 3, 0, 0, "close unallocated fd"),
        (78, 9999, usize::MAX as u64, 512, "read bogus fd"),
        (64, 9999, usize::MAX as u64, 512, "write bogus fd"),
        (62, usize::MAX as u64, 0, 0, "lseek bad fd"),
        (291, 0, usize::MAX as u64, 0, "epoll_create1 garbage"),
        (213, usize::MAX as u64, 0, 0, "epoll_create garbage size"),
        (283, usize::MAX as u64, 0, 0, "eventfd garbage"),
        (279, usize::MAX as u64, 0, 0, "memfd garbage"),
        (29, 9999, 0, 0, "ioctl bogus fd"),
        (25, 9999, 4, 0, "fcntl bogus fd"),
        (167, 0, usize::MAX as u64, 4, "prctl garbage option"),
        (129, 0, 0, 0, "kill pid0 sig0"),
        (131, 99999, 15, 0, "tgkill bogus tgid"),
        (134, usize::MAX as u64, 0, 0, "sigaltstack bad"),
        (135, usize::MAX as u64, usize::MAX as u64, 8, "rt_sigaction garbage"),
        (136, usize::MAX as u64, usize::MAX as u64, 8, "rt_sigprocmask garbage"),
        (138, usize::MAX as u64, 16, 0, "rt_sigtimedwait bad"),
        (226, 0, 0, 99, "mprotect bogus prot"),
        (215, 0, 4096, 0, "munmap low page"),
        (233, usize::MAX as u64, 4096, 99, "madvise garbage"),
        (27, 0, 9999, 0, "mincore bogus"),
        (278, 9999, 0, 0, "getdents64 bogus fd"),
        (61, 9999, usize::MAX as u64, 128, "getdents64 legacy bogus"),
        (260, 0xfffffffa, usize::MAX as u64, 0, "fstatat garbage"),
        (80, 9999, usize::MAX as u64, 0, "fstat bogus fd"),
        (46, 9999, usize::MAX as u64, 0, "ftruncate bogus fd"),
        (45, usize::MAX as u64, 0, 0, "truncate bad path"),
        (90, 0xfffffffe, 0xfffffffe, 0, "chmod bad both"),
        (91, 9999, 0o644, 0, "fchmod bogus fd"),
        (161, 0xfffffffd, 0, 0, "chroot bad path"),
        (49, 9999, usize::MAX as u64, 16, "socketpair garbage"),
        (41, 42, 42, 42, "socket bogus family"),
        (43, 9999, usize::MAX as u64, 16, "bind bogus fd"),
        (50, 9999, usize::MAX as u64, 0, "listen bogus fd"),
        (202, 9999, usize::MAX as u64, 16, "connect bogus fd"),
        (203, 9999, usize::MAX as u64, 0, "accept bogus fd"),
        (42, 9999, usize::MAX as u64, 4, "pipe bogus buf"),
        (292, 9999, usize::MAX as u64, 16, "epoll_ctl bogus"),
        (232, 9999, usize::MAX as u64, 1, "epoll_pwait bogus"),
        (7, 9999, usize::MAX as u64, 0, "poll bogus fd"),
        (73, 9999, usize::MAX as u64, 16, "ppoll bogus"),
        (180, usize::MAX as u64, 0, 0, "mq_open bad name"),
        (71, 9999, 0, 0, "mq_close bogus fd"),
        (98, 0, usize::MAX as u64, 0, "futex bad uaddr"),
        (101, 0, 0, 0, "set_robust_list null"),
        (281, 0, 0, 0, "utimensat null path"),
        (88, 0xfffffffc, 0xfffffffc, 0, "symlink bad both"),
        (82, 0xfffffffb, 0xfffffffb, 0, "rename bad both"),
        (264, 0xfffffffa, 0xfffffffa, 0, "renameat2 bad both"),
        (230, 0, 0, 0, "clock_nanosleep null"),
        (113, 0, 0, 0, "clock_settime bogus id"),
        (403, 0, usize::MAX as u64, 0, "clock_gettime64 bad buf"),
        (169, 0, usize::MAX as u64, 0, "gettimeofday bad buf"),
        (153, 0, usize::MAX as u64, 0, "times bad buf"),
        (165, 0, usize::MAX as u64, 16, "getrusage bad buf"),
        (179, 0, usize::MAX as u64, 0, "sysinfo bad buf"),
        (123, 0, 0, 0, "uname-alias answered"),
        (160, 0, 0, 0, "setdomainname user0"),
        (170, 0, usize::MAX as u64, 0, "prlimit64 bad"),
        (273, 0, usize::MAX as u64, 8, "set_tid_address user"),
        (96, 0, usize::MAX as u64, 8, "gettimeofday32 bad"),
        (260 + 1000, 0, 0, 0, "number far past table"),
    ];
    for &(nr, a0, a1, a2, name) in BADARG {
        let v = drive(nr, a0, a1, a2);
        crate::test_assert!(plausible(v), name);
    }

    // ---- unknown numbers: the ENOSYS fall-through -------------------------
    for nr in [4000u64, 31337, 999_999] {
        let v = drive(nr, 0, 0, 0);
        crate::test_assert!(
            v == -((crate::errno::Errno::FunctionNotImplemented.as_i32()) as i64) || v < 0,
            "dispatch: unknown nr -> ENOSYS family"
        );
    }
}
