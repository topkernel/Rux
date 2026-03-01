//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 进程相关系统调用
//!
//! 包含：clone, execve, exit, wait4, getpid, getppid, kill, set_tid_address, uname 等

use super::*;

/// sys_clone - 创建子进程/线程
///
/// # 参数
/// - args[0]: flags - clone 标志
/// - args[1]: stack - 新栈指针
/// - args[2]: parent_tid - 父进程 TID 指针
/// - args[3]: tls - TLS 指针
/// - args[4]: child_tid - 子进程 TID 指针
///
/// # 返回
/// 在父进程中返回子进程 PID，在子进程中返回 0，失败返回负错误码
pub fn sys_clone(args: SyscallArgs) -> u64 {
    use crate::process::fork::{do_clone, CloneArgs};

    let flags = args[0];
    let stack = args[1];
    let parent_tid = args[2] as *mut i32;
    let child_tid = args[4] as *mut i32;
    let tls = args[3];

    let clone_args = CloneArgs {
        flags,
        stack,
        parent_tid,
        child_tid,
        tls,
    };

    match do_clone(clone_args) {
        Some(pid) => pid as u64,
        None => -errno::ENOMEM as u64,
    }
}

/// sys_execve - 执行程序
///
/// # 参数
/// - args[0]: pathname - 程序路径
/// - args[1]: argv - 参数数组
/// - args[2]: envp - 环境变量数组
///
/// # 返回
/// 成功不返回，失败返回负错误码
pub fn sys_execve(args: SyscallArgs) -> u64 {
    use crate::fs::elf::ElfLoader;
    use crate::arch::riscv64::context::UserContext;
    use alloc::vec::Vec;

    let pathname_ptr = args[0] as *const u8;
    let argv_ptr = args[1] as *const *const u8;
    let _envp_ptr = args[2] as *const *const u8;

    // 检查路径指针
    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // 读取路径
    let pathname = unsafe {
        let mut len = 0;
        let mut ptr = pathname_ptr;
        while *ptr != 0 && len < 256 {
            len += 1;
            ptr = ptr.add(1);
        }
        core::slice::from_raw_parts(pathname_ptr, len)
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // 构建完整路径
    let full_path = if pathname_str.starts_with('/') {
        alloc::borrow::Cow::Borrowed(pathname_str)
    } else {
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(cwd) {
                let mut path = alloc::string::String::with_capacity(cwd_str.len() + pathname_str.len() + 1);
                path.push_str(cwd_str);
                if !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(pathname_str);
                alloc::borrow::Cow::Owned(path)
            } else {
                alloc::borrow::Cow::Borrowed(pathname_str)
            }
        } else {
            alloc::borrow::Cow::Borrowed(pathname_str)
        }
    };

    // 读取 ELF 文件
    let program_data = match crate::fs::read_file_from_rootfs(full_path.as_ref()) {
        Some(data) => data,
        None => return -errno::ENOENT as u64,
    };

    // 获取当前进程
    let current = match crate::sched::current() {
        Some(c) => c,
        None => return -errno::ESRCH as u64,
    };

    // 加载 ELF
    unsafe {
        let _entry = match ElfLoader::get_entry(&program_data) {
            Ok(e) => e,
            Err(_) => return -errno::ENOEXEC as u64,
        };

        // 获取当前进程的地址空间
        let _addr_space = match (*current).address_space() {
            Some(aspace) => aspace,
            None => return -errno::ENOMEM as u64,
        };

        // 加载新的 ELF
        // TODO: 实现完整的 execve ELF 加载
        return -errno::ENOSYS as u64;
    }
}

/// sys_exit - 退出进程
///
/// # 参数
/// - args[0]: status - 退出状态码
///
/// # 返回
/// 不返回
pub fn sys_exit(args: SyscallArgs) -> u64 {
    let exit_code = args[0] as i32;
    crate::sched::do_exit(exit_code);
}

/// sys_wait4 - 等待子进程
///
/// # 参数
/// - args[0]: pid - 要等待的进程 ID
/// - args[1]: status - 存储退出状态的指针
/// - args[2]: options - 等待选项
/// - args[3]: rusage - 资源使用统计指针
///
/// # 返回
/// 成功返回子进程 PID，失败返回负错误码
pub fn sys_wait4(args: SyscallArgs) -> u64 {
    let pid = args[0] as i32;
    let wstatus = args[1] as *mut i32;
    let options = args[2] as i32;
    let _rusage = args[3] as *mut u8;

    // WNOHANG: 如果没有子进程退出，立即返回 0
    const WNOHANG: i32 = 0x00000001;

    if options & WNOHANG != 0 {
        // WNOHANG 模式：非阻塞检查
        match crate::sched::do_wait_nonblock(pid, wstatus) {
            Ok(child_pid) => child_pid as u64,
            Err(e) if e == -11 => 0,  // EAGAIN -> 返回 0 表示没有子进程退出
            Err(e) => e as u32 as u64,
        }
    } else {
        // 阻塞等待子进程退出
        match crate::sched::do_wait(pid, wstatus) {
            Ok(child_pid) => child_pid as u64,
            Err(e) => e as u32 as u64,
        }
    }
}

/// sys_getpid - 获取进程 ID
pub fn sys_getpid(_args: SyscallArgs) -> u64 {
    if let Some(current) = crate::sched::current() {
        unsafe { (*current).pid() as u64 }
    } else {
        0
    }
}

/// sys_getppid - 获取父进程 ID
pub fn sys_getppid(_args: SyscallArgs) -> u64 {
    crate::process::current_ppid() as u64
}

/// sys_kill - 发送信号
pub fn sys_kill(args: SyscallArgs) -> u64 {
    let pid = args[0] as i32;
    let sig = args[1] as i32;

    if sig < 0 || sig > 64 {
        return -errno::EINVAL as u64;
    }

    if pid <= 0 {
        // 不支持进程组操作
        return -errno::ESRCH as u64;
    }

    // 查找目标进程并发送信号
    unsafe {
        let target = crate::sched::find_task_by_pid(pid as u32);
        if target.is_null() {
            return -errno::ESRCH as u64;
        }

        if sig > 0 {
            crate::signal::send_signal(pid as u32, sig);
        }
    }

    0
}

/// sys_set_tid_address - 设置 TID 地址
pub fn sys_set_tid_address(args: SyscallArgs, tp: u64) -> u64 {
    let tidptr = args[0] as *mut i32;

    if let Some(current) = crate::sched::current() {
        unsafe {
            (*current).set_clear_child_tid(tidptr);
            return (*current).pid() as u64;
        }
    }

    0
}

/// sys_set_robust_list - 设置 robust list
pub fn sys_set_robust_list(_args: SyscallArgs) -> u64 {
    // 简化实现
    0
}

/// sys_uname - 获取系统信息
pub fn sys_uname(args: SyscallArgs) -> u64 {
    #[repr(C)]
    struct Utsname {
        sysname: [u8; 65],
        nodename: [u8; 65],
        release: [u8; 65],
        version: [u8; 65],
        machine: [u8; 65],
        domainname: [u8; 65],
    }

    let buf = args[0] as *mut Utsname;

    if buf.is_null() {
        return -errno::EFAULT as u64;
    }

    unsafe {
        let uname = &mut *buf;

        // 填充系统信息
        let sysname = b"Rux\0";
        let nodename = b"rux\0";
        let release = b"0.1.0\0";
        let version = b"Rux OS v0.1.0\0";
        let machine = b"riscv64\0";
        let domainname = b"\0";

        uname.sysname[..sysname.len()].copy_from_slice(sysname);
        uname.nodename[..nodename.len()].copy_from_slice(nodename);
        uname.release[..release.len()].copy_from_slice(release);
        uname.version[..version.len()].copy_from_slice(version);
        uname.machine[..machine.len()].copy_from_slice(machine);
        uname.domainname[..domainname.len()].copy_from_slice(domainname);
    }

    0
}

/// sys_getuid - 获取用户 ID
pub fn sys_getuid(_args: SyscallArgs) -> u64 {
    0  // root
}

/// sys_getgid - 获取组 ID
pub fn sys_getgid(_args: SyscallArgs) -> u64 {
    0  // root
}

/// sys_geteuid - 获取有效用户 ID
pub fn sys_geteuid(_args: SyscallArgs) -> u64 {
    0  // root
}

/// sys_getegid - 获取有效组 ID
pub fn sys_getegid(_args: SyscallArgs) -> u64 {
    0  // root
}

/// sys_prlimit64 - 获取/设置资源限制
pub fn sys_prlimit64(args: SyscallArgs) -> u64 {
    let _pid = args[0] as i32;
    let resource = args[1] as i32;
    let new_rlim = args[2] as *const u8;
    let old_rlim = args[3] as *mut u8;

    // 只支持查询
    if !new_rlim.is_null() {
        return -errno::EPERM as u64;
    }

    if old_rlim.is_null() {
        return -errno::EFAULT as u64;
    }

    // RLIMIT_NOFILE = 7
    if resource == 7 {
        unsafe {
            // 返回默认的文件描述符限制
            let rlim = old_rlim as *mut u64;
            *rlim = 1024;        // rlim_cur
            *rlim.offset(1) = 1024 * 1024;  // rlim_max
        }
        return 0;
    }

    -errno::EINVAL as u64
}
