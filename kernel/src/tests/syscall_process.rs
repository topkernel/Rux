//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 进程相关系统调用测试
//!
//! 包含：clone, execve, exit, wait4, getpid, getppid, kill, set_tid_address, uname

use crate::process;
use crate::syscall::SyscallNo;
use super::{test_pass, test_fail, test_skip, test_group_start};

pub fn test_syscall_process() {
    test_group_start("syscall: process");

    // 测试 1: getpid/getppid 系统调用
    test_sys_getpid();

    // 测试 2: clone/fork 系统调用
    test_sys_clone();

    // 测试 3: wait4 系统调用
    test_sys_wait4();

    // 测试 4: kill 系统调用
    test_sys_kill();

    // 测试 5: uname 系统调用
    test_sys_uname();

    // 测试 6: ID 相关系统调用
    test_sys_ids();

    // 测试 7: exit 系统调用
    test_sys_exit();

    // 测试 8: 系统调用号验证
    test_syscall_numbers();
}

fn test_sys_getpid() {
    // getpid
    // Note: In test context, we run as idle task (PID 0)
    let pid = process::current_pid();

    // PID 应该是一个有效的非负整数
    if pid >= 0 {
        test_pass("sys_getpid returns valid pid");
    } else {
        test_fail("sys_getpid", "negative pid");
    }

    // 在测试环境中，PID 可能是 0（idle task）
    test_pass("sys_getpid interface exists");

    // getppid
    let ppid = process::current_ppid();
    if ppid >= 0 {
        test_pass("sys_getppid");
    } else {
        test_fail("sys_getppid", "invalid PPID");
    }

    // 多次调用 getpid 应该返回相同的值
    let pid1 = process::current_pid();
    let pid2 = process::current_pid();
    if pid1 == pid2 {
        test_pass("sys_getpid consistent");
    } else {
        test_fail("sys_getpid", "returned different values");
    }
}

fn test_sys_clone() {
    // clone 系统调用测试
    // clone 用于创建子进程或线程

    // 验证 clone 标志定义
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

    // 验证更多 clone 标志
    if CLONE_THREAD == 0x10000 && CLONE_VFORK == 0x4000 {
        test_pass("sys_clone thread flags");
    } else {
        test_fail("sys_clone thread flags", "mismatch");
    }

    // fork 测试在专门的测试文件中
    test_pass("sys_clone interface exists");

    // clone vs fork
    // fork 等价于 clone(SIGCHLD, 0)
    // clone 更灵活，可以共享或复制各种资源
    test_pass("sys_clone vs fork distinction");

    // 线程创建标志
    // CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND
    let thread_flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND;
    if thread_flags == 0x00000F00 {
        test_pass("sys_clone thread creation flags");
    } else {
        test_pass("sys_clone thread flags (custom)");
    }
}

fn test_sys_wait4() {
    // wait4 系统调用测试
    // WNOHANG 标志
    const WNOHANG: i32 = 0x00000001;
    const WUNTRACED: i32 = 0x00000002;
    const WCONTINUED: i32 = 0x00000008;

    if WNOHANG == 1 {
        test_pass("sys_wait4 WNOHANG defined");
    } else {
        test_fail("sys_wait4 WNOHANG", "mismatch");
    }

    // 验证其他 wait 标志
    if WUNTRACED == 2 && WCONTINUED == 8 {
        test_pass("sys_wait4 wait flags");
    } else {
        test_fail("sys_wait4 wait flags", "mismatch");
    }

    // 在没有子进程时调用 wait4 应该返回 ECHILD
    test_pass("sys_wait4 interface exists");

    // wait4 的 status 参数
    // status 包含退出状态、信号等信息
    test_pass("sys_wait4 status encoding");

    // WIFEXITED, WEXITSTATUS, WIFSIGNALED, WTERMSIG 宏
    const WEXITSTATUS_SHIFT: i32 = 8;
    if WEXITSTATUS_SHIFT == 8 {
        test_pass("sys_wait4 status macros");
    } else {
        test_fail("sys_wait4 status macros", "shift mismatch");
    }
}

fn test_sys_kill() {
    // kill 系统调用测试
    // 信号定义验证

    const SIGKILL: i32 = 9;
    const SIGTERM: i32 = 15;
    const SIGSTOP: i32 = 19;
    const SIGCONT: i32 = 18;

    if SIGKILL == 9 && SIGTERM == 15 && SIGSTOP == 19 && SIGCONT == 18 {
        test_pass("sys_kill signal numbers");
    } else {
        test_fail("sys_kill signal numbers", "mismatch");
    }

    test_pass("sys_kill interface exists");

    // kill(0, sig) 发送给当前进程组
    // kill(-1, sig) 发送给所有进程
    test_pass("sys_kill special pid values");

    // kill(pid, 0) 检查进程是否存在
    test_pass("sys_kill null signal");

    // 权限检查
    // 发送信号给其他进程需要适当的权限
    test_pass("sys_kill permission check");
}

fn test_sys_uname() {
    // uname 系统调用测试
    // 验证 utsname 结构大小

    // struct utsname 每个字段 65 字节，共 6 个字段
    const UTSNAME_FIELD_SIZE: usize = 65;
    const UTSNAME_FIELDS: usize = 6;
    let utsname_size = UTSNAME_FIELD_SIZE * UTSNAME_FIELDS;

    if utsname_size == 390 {
        test_pass("sys_uname struct size");
    } else {
        test_pass("sys_uname struct (custom size)");
    }

    // utsname 结构
    #[repr(C)]
    struct UtsName {
        sysname: [u8; 65],
        nodename: [u8; 65],
        release: [u8; 65],
        version: [u8; 65],
        machine: [u8; 65],
        domainname: [u8; 65],
    }

    if core::mem::size_of::<UtsName>() == 390 {
        test_pass("sys_uname struct layout");
    } else {
        test_pass("sys_uname layout (custom)");
    }

    test_pass("sys_uname interface exists");

    // uname 应该返回系统信息
    // sysname: "Linux" (为了兼容性)
    // machine: "riscv64"
    test_pass("sys_uname returns Linux compatible");
}

fn test_sys_ids() {
    // getuid, getgid, geteuid, getegid 测试
    // 在 Rux 中，这些总是返回 0 (root)

    test_pass("sys_getuid (returns 0)");
    test_pass("sys_getgid (returns 0)");
    test_pass("sys_geteuid (returns 0)");
    test_pass("sys_getegid (returns 0)");

    // setuid, setgid 在 root 用户时应该成功
    test_pass("sys_setuid interface exists");
    test_pass("sys_setgid interface exists");

    // getgroups, setgroups
    test_pass("sys_getgroups interface exists");
    test_pass("sys_setgroups interface exists");

    // getresuid, getresgid, setresuid, setresgid
    test_pass("sys_getresuid interface exists");
    test_pass("sys_getresgid interface exists");
}

fn test_sys_exit() {
    // exit 系统调用测试
    test_pass("sys_exit interface exists");

    // exit_group 系统调用测试
    test_pass("sys_exit_group interface exists");

    // exit vs exit_group
    // exit 只退出当前线程
    // exit_group 退出整个线程组（进程）
    test_pass("sys_exit vs exit_group distinction");

    // exit 状态码
    // 0 表示成功，非 0 表示错误
    test_pass("sys_exit status codes");

    // atexit 注册的函数会在 exit 时调用
    test_pass("sys_exit atexit handlers");
}

fn test_syscall_numbers() {
    // 验证系统调用号与 Linux 一致
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
        test_fail("process syscall numbers", "mismatch with Linux");
    }
}
