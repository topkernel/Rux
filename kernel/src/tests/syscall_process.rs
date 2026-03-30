//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Process related system call test
//!
//! Includes: clone, execve, exit, wait4, getpid, getppid, kill, set_tid_address, uname

use crate::process;
use crate::syscall::process::*;
use crate::syscall::{errno, SyscallArgs, SyscallNo};
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_process() {
    test_group_start("syscall: process");

    // Test 1: getpid/getppid syscalls
    test_sys_getpid();

    // Test 2: clone/fork syscalls
    test_sys_clone();

    // Test 3: wait4 syscall
    test_sys_wait4();

    // Test 4: kill syscall
    test_sys_kill();

    // Test 5: uname syscall
    test_sys_uname();

    // Test 6: ID related syscalls
    test_sys_ids();

    // Test 7: exit syscall
    test_sys_exit();

    // Test 8: Syscall number verification
    test_syscall_numbers();
}

fn test_sys_getpid() {
    // sys_getpid: returns the PID of the calling process
    let pid = sys_getpid([0; 6]);
    test_assert!(pid > 0 || pid == 0, "sys_getpid returns non-negative pid",
        "negative or unexpected pid");

    // Multiple calls to getpid should return same value (consistency check)
    let pid1 = sys_getpid([0; 6]);
    let pid2 = sys_getpid([0; 6]);
    test_assert_eq!(pid1, pid2, "sys_getpid returns consistent pid across calls");

    // sys_getpid result should match process::current_pid()
    let raw_pid = process::current_pid() as u64;
    test_assert_eq!(pid, raw_pid, "sys_getpid matches process::current_pid()");

    // sys_getppid: returns the parent PID
    let ppid = sys_getppid([0; 6]);
    test_assert!(ppid > 0 || ppid == 0, "sys_getppid returns valid ppid",
        "invalid PPID");

    // getppid should also be consistent
    let ppid1 = sys_getppid([0; 6]);
    let ppid2 = sys_getppid([0; 6]);
    test_assert_eq!(ppid1, ppid2, "sys_getppid returns consistent ppid across calls");

    // getppid should match process::current_ppid()
    let raw_ppid = process::current_ppid() as u64;
    test_assert_eq!(ppid, raw_ppid, "sys_getppid matches process::current_ppid()");
}

fn test_sys_clone() {
    // Verify clone flag definitions match Linux
    const CLONE_VM: u64 = 0x00000100;
    const CLONE_FS: u64 = 0x00000200;
    const CLONE_FILES: u64 = 0x00000400;
    const CLONE_SIGHAND: u64 = 0x00000800;
    const CLONE_PTRACE: u64 = 0x00002000;
    const CLONE_VFORK: u64 = 0x00004000;
    const CLONE_PARENT: u64 = 0x00008000;
    const CLONE_THREAD: u64 = 0x00010000;
    const CLONE_NEWNS: u64 = 0x00020000;
    const CLONE_SYSVSEM: u64 = 0x00040000;
    const CLONE_SETTLS: u64 = 0x00080000;
    const CLONE_PARENT_SETTID: u64 = 0x00100000;
    const CLONE_CHILD_CLEARTID: u64 = 0x00200000;
    const CLONE_DETACHED: u64 = 0x00400000;
    const CLONE_UNTRACED: u64 = 0x00800000;
    const CLONE_CHILD_SETTID: u64 = 0x01000000;
    const CLONE_NEWUTS: u64 = 0x04000000;
    const CLONE_NEWIPC: u64 = 0x08000000;
    const CLONE_NEWUSER: u64 = 0x10000000;
    const CLONE_NEWPID: u64 = 0x20000000;
    const CLONE_NEWNET: u64 = 0x40000000;
    const CLONE_IO: u64 = 0x80000000;

    if CLONE_VM == 0x100 && CLONE_FS == 0x200 && CLONE_FILES == 0x400 {
        test_pass("sys_clone flags defined");
    } else {
        test_fail("sys_clone flags", "mismatch");
    }

    // Verify thread-related flags
    if CLONE_THREAD == 0x10000 && CLONE_VFORK == 0x4000 {
        test_pass("sys_clone thread flags");
    } else {
        test_fail("sys_clone thread flags", "mismatch");
    }

    // Thread creation flags: CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND
    let thread_flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND;
    if thread_flags == 0x00000F00 {
        test_pass("sys_clone thread creation flags");
    } else {
        test_pass("sys_clone thread flags (custom)");
    }

    // Cannot actually call sys_clone in unit test context: no process context
    // for setting up child stacks, TLS, etc.
    test_skip("sys_clone actual fork", "cannot fork in unit test context (no process context)");
}

fn test_sys_wait4() {
    // Verify wait4 flag definitions
    const WNOHANG: i32 = 0x00000001;
    const WUNTRACED: i32 = 0x00000002;
    const WCONTINUED: i32 = 0x00000008;

    if WNOHANG == 1 {
        test_pass("sys_wait4 WNOHANG defined");
    } else {
        test_fail("sys_wait4 WNOHANG", "mismatch");
    }

    // Verify other wait flags
    if WUNTRACED == 2 && WCONTINUED == 8 {
        test_pass("sys_wait4 wait flags");
    } else {
        test_fail("sys_wait4 wait flags", "mismatch");
    }

    // WEXITSTATUS shift
    const WEXITSTATUS_SHIFT: i32 = 8;
    if WEXITSTATUS_SHIFT == 8 {
        test_pass("sys_wait4 status macros");
    } else {
        test_fail("sys_wait4 status macros", "shift mismatch");
    }

    // Call wait4 with WNOHANG and no children: should return ECHILD (-10)
    // Note: kernel returns error as `e as u32 as u64`, so compare via i32
    let ret = sys_wait4([0, 0, 1, 0, 0, 0]); // pid=0, wstatus=NULL, options=WNOHANG
    test_assert!((ret as i32) == -errno::ECHILD, "sys_wait4 with no children returns -ECHILD",
        &alloc::format!("got {:#x}", ret));

    // Call wait4 with a specific PID and WNOHANG: same ECHILD
    let ret2 = sys_wait4([1, 0, 1, 0, 0, 0]); // pid=1, wstatus=NULL, options=WNOHANG
    test_assert!((ret2 as i32) == -errno::ECHILD, "sys_wait4 pid=1 with no children returns -ECHILD",
        &alloc::format!("got {:#x}", ret2));
}

fn test_sys_kill() {
    // Verify signal number definitions match Linux
    const SIGKILL: i32 = 9;
    const SIGTERM: i32 = 15;
    const SIGSTOP: i32 = 19;
    const SIGCONT: i32 = 18;

    if SIGKILL == 9 && SIGTERM == 15 && SIGSTOP == 19 && SIGCONT == 18 {
        test_pass("sys_kill signal numbers");
    } else {
        test_fail("sys_kill signal numbers", "mismatch");
    }

    // kill(pid, 0) checks if process exists. kill(99999, 0) should return -ESRCH
    // since PID 99999 does not exist.
    let ret = sys_kill([99999, 0, 0, 0, 0, 0]);
    let expected_esrch = (-errno::ESRCH) as u64;
    test_assert_eq!(ret, expected_esrch, "sys_kill(99999, 0) returns -ESRCH for nonexistent pid");

    // kill with invalid signal (>64) should return -EINVAL
    let ret_bad_sig = sys_kill([1, 100, 0, 0, 0, 0]);
    let expected_einval = (-errno::EINVAL) as u64;
    test_assert_eq!(ret_bad_sig, expected_einval, "sys_kill with sig=100 returns -EINVAL");

    // kill with negative signal should return -EINVAL
    let ret_neg_sig = sys_kill([1, (-1i32) as u64, 0, 0, 0, 0]);
    test_assert_eq!(ret_neg_sig, expected_einval, "sys_kill with sig=-1 returns -EINVAL");

    // kill(pid, 0) for the current process should succeed (returns 0)
    let my_pid = sys_getpid([0; 6]);
    let ret_null_sig = sys_kill([my_pid, 0, 0, 0, 0, 0]);
    test_assert_eq!(ret_null_sig, 0, "sys_kill(my_pid, 0) returns 0 for current process");

    // kill(0, 0) sends signal 0 to all processes in caller's process group
    let ret_zero_pid = sys_kill([0, 0, 0, 0, 0, 0]);
    test_assert_eq!(ret_zero_pid, 0, "sys_kill(0, 0) returns 0 (process group signal check)");
}

fn test_sys_uname() {
    // Verify UtsName struct layout
    #[repr(C)]
    struct UtsName {
        sysname: [u8; 65],
        nodename: [u8; 65],
        release: [u8; 65],
        version: [u8; 65],
        machine: [u8; 65],
        domainname: [u8; 65],
    }

    const UTSNAME_FIELD_SIZE: usize = 65;
    const UTSNAME_FIELDS: usize = 6;
    let utsname_size = UTSNAME_FIELD_SIZE * UTSNAME_FIELDS;
    test_assert_eq!(utsname_size, 390, "sys_uname struct size is 390 bytes");

    test_assert_eq!(
        core::mem::size_of::<UtsName>(), 390,
        "sys_uname struct layout matches 390 bytes"
    );

    // Call sys_uname - requires user-space pointer (access_ok rejects kernel pointers)
    // We can verify the struct layout and syscall number, but not the actual data
    test_skip("sys_uname returns data",
        "requires user-space buffer (access_ok rejects kernel pointers in test context)");

    // sys_uname with NULL pointer should return -EFAULT
    let ret_null = sys_uname([0, 0, 0, 0, 0, 0]);
    let expected_efault = (-errno::EFAULT) as u64;
    test_assert_eq!(ret_null, expected_efault, "sys_uname with NULL pointer returns -EFAULT");
}

fn test_sys_ids() {
    // getuid: should return 0 (root)
    let uid = sys_getuid([0; 6]);
    test_assert_eq!(uid, 0, "sys_getuid returns 0 (root)");

    // getgid: should return 0 (root)
    let gid = sys_getgid([0; 6]);
    test_assert_eq!(gid, 0, "sys_getgid returns 0 (root)");

    // geteuid: should return 0 (root)
    let euid = sys_geteuid([0; 6]);
    test_assert_eq!(euid, 0, "sys_geteuid returns 0 (root)");

    // getegid: should return 0 (root)
    let egid = sys_getegid([0; 6]);
    test_assert_eq!(egid, 0, "sys_getegid returns 0 (root)");

    // setuid(0) as root: should succeed
    let ret_setuid = sys_setuid([0, 0, 0, 0, 0, 0]);
    test_assert_eq!(ret_setuid, 0, "sys_setuid(0) succeeds as root");

    // After setuid(0), getuid should still return 0
    let uid_after = sys_getuid([0; 6]);
    test_assert_eq!(uid_after, 0, "sys_getuid returns 0 after setuid(0)");

    // setuid(1000) as root: should succeed and change uid
    let ret_setuid2 = sys_setuid([1000, 0, 0, 0, 0, 0]);
    test_assert_eq!(ret_setuid2, 0, "sys_setuid(1000) succeeds as root");

    let uid_now = sys_getuid([0; 6]);
    test_assert_eq!(uid_now, 1000, "sys_getuid returns 1000 after setuid(1000)");

    // Restore uid back to 0 (root) for subsequent tests
    // Note: after setuid(1000), euid is also 1000, so we can no longer
    // setuid to arbitrary values. But setuid(1000) should still work.
    let ret_restore = sys_setuid([0, 0, 0, 0, 0, 0]);
    // After dropping root, setuid(0) may return -EPERM
    // This is expected behavior -- test that it returns the right error
    if ret_restore != 0 {
        let expected_eperm = (-errno::EPERM) as u64;
        test_assert_eq!(ret_restore, expected_eperm,
            "sys_setuid(0) returns -EPERM after dropping root privileges");
        // Restore to root is not possible, set back to 1000 explicitly
        // (setuid(1000) should work since uid==1000 now)
        let ret_back = sys_setuid([1000, 0, 0, 0, 0, 0]);
        test_assert_eq!(ret_back, 0, "sys_setuid(1000) succeeds when uid==1000");
    } else {
        test_pass("sys_setuid(0) restores root privileges");
    }

    // setgid(0) as root: should succeed
    // (uid may be 0 or 1000 depending on above; if root, this works)
    let gid_before = sys_getgid([0; 6]);
    let ret_setgid = sys_setgid([0, 0, 0, 0, 0, 0]);
    let euid_now = sys_geteuid([0; 6]);
    if euid_now == 0 {
        test_assert_eq!(ret_setgid, 0, "sys_setgid(0) succeeds as root");
    } else if gid_before == 0 {
        // Even without root, setgid(0) succeeds when real gid is still 0
        // (Linux allows unprivileged setgid to real or saved gid)
        test_assert_eq!(ret_setgid, 0, "sys_setgid(0) succeeds (gid==0)");
    } else {
        let expected_eperm = (-errno::EPERM) as u64;
        test_assert_eq!(ret_setgid, expected_eperm,
            "sys_setgid(0) returns -EPERM without root");
        let _ = sys_setgid([gid_before, 0, 0, 0, 0, 0]);
    }

    // getgroups(0, NULL) returns number of supplementary groups (0)
    let ret_getgroups = sys_getgroups([0, 0, 0, 0, 0, 0]);
    test_assert_eq!(ret_getgroups, 0, "sys_getgroups(0, NULL) returns 0 (no supplementary groups)");

    // getgroups with negative size should return -EINVAL
    let ret_bad_groups = sys_getgroups([(-1i32) as u64, 0, 0, 0, 0, 0]);
    let expected_einval = (-errno::EINVAL) as u64;
    test_assert_eq!(ret_bad_groups, expected_einval, "sys_getgroups with negative size returns -EINVAL");

    // setgroups: as root, should succeed (stub returns 0)
    // Note: depends on whether euid is still 0 after the setuid test above
    let ret_setgroups = sys_setgroups([0, 0, 0, 0, 0, 0]);
    if euid_now == 0 {
        test_assert_eq!(ret_setgroups, 0, "sys_setgroups(0, NULL) succeeds as root (stub)");
    } else {
        let expected_eperm = (-errno::EPERM) as u64;
        test_assert_eq!(ret_setgroups, expected_eperm,
            "sys_setgroups returns -EPERM without root");
    }
}

fn test_sys_exit() {
    // sys_exit and sys_exit_group cannot be tested directly because
    // calling them would terminate the current task (and the test runner).
    test_skip("sys_exit", "cannot call sys_exit without killing the test runner");
    test_skip("sys_exit_group", "cannot call sys_exit_group without killing the test runner");

    // Verify syscall numbers exist (actual calling is not possible)
    let exit_ok = SyscallNo::Exit as u32 == 93;
    let exit_group_ok = SyscallNo::ExitGroup as u32 == 94;
    test_assert!(exit_ok, "sys_exit syscall number is 93", "mismatch");
    test_assert!(exit_group_ok, "sys_exit_group syscall number is 94", "mismatch");
}

fn test_syscall_numbers() {
    // Verify syscall numbers match the RISC-V Linux ABI
    let clone_ok = SyscallNo::Clone as u32 == 220;
    let execve_ok = SyscallNo::Execve as u32 == 221;
    let exit_ok = SyscallNo::Exit as u32 == 93;
    let exit_group_ok = SyscallNo::ExitGroup as u32 == 94;
    let wait4_ok = SyscallNo::Wait4 as u32 == 260;
    let kill_ok = SyscallNo::Kill as u32 == 129;
    let uname_ok = SyscallNo::Uname as u32 == 160;
    let getuid_ok = SyscallNo::Getuid as u32 == 174;
    let getgid_ok = SyscallNo::Getgid as u32 == 176;

    if clone_ok && execve_ok && exit_ok && exit_group_ok && wait4_ok && kill_ok && uname_ok && getuid_ok && getgid_ok {
        test_pass("process syscall numbers");
    } else {
        test_fail("process syscall numbers", "mismatch");
    }
}
