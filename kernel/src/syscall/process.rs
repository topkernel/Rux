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
    use crate::fs::elf::{ElfLoader, Elf64Ehdr};
    use alloc::vec::Vec;
    use alloc::string::String;

    let pathname_ptr = args[0] as *const u8;
    let argv_ptr = args[1] as *const *const u8;

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

    // 从文件系统读取 ELF 文件
    let program_data = if crate::fs::ext4::is_mounted() {
        match crate::fs::ext4::read_file_from_mounted(full_path.as_ref()) {
            Some(data) => data,
            None => return -errno::ENOENT as u64,
        }
    } else {
        match crate::fs::read_file_from_rootfs(full_path.as_ref()) {
            Some(data) => data,
            None => return -errno::ENOENT as u64,
        }
    };

    // 验证 ELF 格式
    if ElfLoader::validate(&program_data).is_err() {
        return -errno::ENOEXEC as u64;
    }

    // 获取入口点
    let entry = match ElfLoader::get_entry(&program_data) {
        Ok(e) => e,
        Err(_) => return -errno::ENOEXEC as u64,
    };

    // 获取程序头数量
    let phdr_count = match ElfLoader::get_program_headers(&program_data) {
        Ok(n) => n,
        Err(_) => return -errno::ENOEXEC as u64,
    };

    let ehdr = match unsafe { Elf64Ehdr::from_bytes(&program_data) } {
        Some(e) => e,
        None => return -errno::ENOEXEC as u64,
    };

    // 解析 argv
    let argv: Vec<String> = unsafe {
        let mut args = Vec::new();
        if !argv_ptr.is_null() {
            let mut i = 0usize;
            loop {
                let arg_ptr = *argv_ptr.add(i);
                if arg_ptr.is_null() {
                    break;
                }
                let mut len = 0usize;
                let mut p = arg_ptr;
                while *p != 0 && len < 1024 {
                    len += 1;
                    p = p.add(1);
                }
                let arg_slice = core::slice::from_raw_parts(arg_ptr, len);
                if let Ok(s) = core::str::from_utf8(arg_slice) {
                    args.push(String::from(s));
                }
                i += 1;
                if i > 64 { break; }
            }
        }
        if args.is_empty() {
            args.push(String::from(full_path.as_ref()));
        }
        args
    };

    // 获取当前进程
    let current = match crate::sched::current() {
        Some(c) => c,
        None => return -errno::ESRCH as u64,
    };

    // 执行 ELF 加载
    match do_execve_elf(current, &program_data, &argv, entry, phdr_count as usize, &ehdr, full_path.as_ref()) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
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
    crate::println!("exit: code={}", exit_code);
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

/// 执行 ELF 加载 (execve 内部函数)
///
/// 这个函数会：
/// 1. 创建新的地址空间
/// 2. 加载 ELF 段
/// 3. 设置栈和参数
/// 4. 更新进程上下文
fn do_execve_elf(
    task_ptr: *mut crate::process::task::Task,
    program_data: &[u8],
    argv: &[alloc::string::String],
    entry: u64,
    phdr_count: usize,
    ehdr: &crate::fs::elf::Elf64Ehdr,
    pathname: &str,
) -> Result<(), i32> {
    use crate::arch::riscv64::mm::{create_user_address_space, alloc_and_map_to_user_table, PAGE_SIZE, PageTableEntry};
    use core::slice;

    // 找到虚拟地址范围
    let mut min_vaddr: u64 = u64::MAX;
    let mut max_vaddr: u64 = 0;

    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(crate::errno::Errno::FunctionNotImplemented.as_neg_i32())?;

        if phdr.is_load() {
            let virt_addr = phdr.p_vaddr;
            let mem_size = phdr.p_memsz;

            if virt_addr < min_vaddr {
                min_vaddr = virt_addr;
            }
            if virt_addr + mem_size > max_vaddr {
                max_vaddr = virt_addr + mem_size;
            }
        }
    }

    // 页对齐
    let virt_start = min_vaddr & !(PAGE_SIZE - 1);
    let virt_end = (max_vaddr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // 为栈预留空间
    const STACK_RESERVED: u64 = 128 * 1024;
    let total_size = virt_end - virt_start + STACK_RESERVED;

    // 创建新的用户地址空间
    let user_ppn = create_user_address_space().ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

    // 分配并映射用户内存
    let flags = PageTableEntry::V | PageTableEntry::U |
               PageTableEntry::R | PageTableEntry::W |
               PageTableEntry::X | PageTableEntry::A |
               PageTableEntry::D;

    let phys_base = unsafe {
        alloc_and_map_to_user_table(user_ppn, virt_start, total_size, flags)
    }.ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

    // 加载每个段
    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(crate::errno::Errno::FunctionNotImplemented.as_neg_i32())?;

        if phdr.is_load() {
            let virt_addr = phdr.p_vaddr;
            let file_size = phdr.p_filesz;
            let mem_size = phdr.p_memsz;
            let offset = phdr.p_offset as usize;

            let virt_offset = virt_addr - virt_start;
            let phys_addr = (phys_base + virt_offset) as usize;

            // 复制数据
            if file_size > 0 {
                let src = &program_data[offset..offset + file_size as usize];
                unsafe {
                    let dst = slice::from_raw_parts_mut(phys_addr as *mut u8, file_size as usize);
                    dst.copy_from_slice(src);
                }
            }

            // 清零 BSS
            if mem_size > file_size {
                let bss_start = phys_addr + file_size as usize;
                let bss_size = (mem_size - file_size) as usize;
                unsafe {
                    let bss_dst = slice::from_raw_parts_mut(bss_start as *mut u8, bss_size);
                    bss_dst.fill(0);
                }
            }
        }
    }

    // 设置栈
    let stack_top = virt_end + STACK_RESERVED - 256;
    let virt_offset = stack_top - virt_start;
    let phys_stack_top = (phys_base + virt_offset) as usize;

    // auxv 常量
    const AT_NULL: u64 = 0;
    const AT_PHDR: u64 = 3;
    const AT_PHENT: u64 = 4;
    const AT_PHNUM: u64 = 5;
    const AT_PAGESZ: u64 = 6;
    const AT_BASE: u64 = 7;
    const AT_ENTRY: u64 = 9;
    const AT_UID: u64 = 11;
    const AT_EUID: u64 = 12;
    const AT_GID: u64 = 13;
    const AT_EGID: u64 = 14;
    const AT_HWCAP: u64 = 16;
    const AT_CLKTCK: u64 = 17;
    const AT_SECURE: u64 = 23;
    const AT_RANDOM: u64 = 25;
    const AT_EXECFN: u64 = 31;

    let argc = argv.len() as u64;
    let argv_count = argv.len();
    let phent = ehdr.e_phentsize as u64;
    let phnum = ehdr.e_phnum as u64;
    let phsize = (phnum * phent) as usize;

    // 计算栈布局
    let auxv_slots: usize = 30;  // 15 个 auxv 条目 * 2
    let mut string_space: usize = 0;
    for arg in argv.iter() {
        string_space += ((arg.len() + 1 + 7) / 8) * 8;
    }
    let phdr_space: usize = ((phsize + 7) / 8) * 8;

    let random_offset: usize = 1 + argv_count + 1 + 1 + auxv_slots;
    let phdr_offset: usize = random_offset + 2;
    let string_offset: usize = phdr_offset + (phdr_space + 7) / 8;
    let total_slots: usize = string_offset + (string_space + 7) / 8;
    let adjusted_stack_top = stack_top.saturating_sub((total_slots * 8) as u64);

    let adjusted_virt_offset = adjusted_stack_top - virt_start;
    let adjusted_phys_stack_top = (phys_base + adjusted_virt_offset) as usize;

    unsafe {
        let stack_ptr = adjusted_phys_stack_top as *mut u64;
        let mut offset: isize = 0;

        let phdr_addr = adjusted_stack_top + (phdr_offset * 8) as u64;
        let random_vaddr = adjusted_stack_top + (random_offset * 8) as u64;

        // 复制程序头表
        let src_ptr = program_data.as_ptr().add(ehdr.e_phoff as usize);
        let dst_ptr = (stack_ptr as *mut u8).add(phdr_offset * 8);
        core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, phsize);

        // 写入 argv 字符串
        let mut argv_addrs: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(argv_count);
        let mut current_string_offset = string_offset;

        for arg in argv.iter() {
            let string_pos = current_string_offset * 8;
            let arg_bytes = arg.as_bytes();
            for (i, &b) in arg_bytes.iter().enumerate() {
                core::ptr::write_volatile(
                    (stack_ptr as *mut u8).offset((string_pos + i) as isize),
                    b
                );
            }
            core::ptr::write_volatile(
                (stack_ptr as *mut u8).offset((string_pos + arg_bytes.len()) as isize),
                0
            );
            argv_addrs.push(adjusted_stack_top + string_pos as u64);
            current_string_offset += ((arg.len() + 1 + 7) / 8);
        }

        // argc
        core::ptr::write_volatile(stack_ptr, argc);
        offset += 1;

        // argv
        for &addr in &argv_addrs {
            core::ptr::write_volatile(stack_ptr.offset(offset), addr);
            offset += 1;
        }

        // argv terminator
        core::ptr::write_volatile(stack_ptr.offset(offset), 0u64);
        offset += 1;

        // envp terminator
        core::ptr::write_volatile(stack_ptr.offset(offset), 0u64);
        offset += 1;

        // auxv
        let auxv = &[
            (AT_PHDR, phdr_addr),
            (AT_PHENT, phent),
            (AT_PHNUM, phnum),
            (AT_PAGESZ, PAGE_SIZE as u64),
            (AT_BASE, 0),
            (AT_ENTRY, entry),
            (AT_UID, 0),
            (AT_EUID, 0),
            (AT_GID, 0),
            (AT_EGID, 0),
            (AT_HWCAP, 0),
            (AT_CLKTCK, 100),
            (AT_SECURE, 0),
            (AT_RANDOM, random_vaddr),
            (AT_EXECFN, argv_addrs.first().copied().unwrap_or(0)),
        ];

        for (typ, val) in auxv {
            core::ptr::write_volatile(stack_ptr.offset(offset), *typ);
            core::ptr::write_volatile(stack_ptr.offset(offset + 1), *val);
            offset += 2;
        }

        // AT_NULL
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_NULL);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);

        // 随机数
        core::ptr::write_volatile(stack_ptr.offset(offset + 2), 0xdeadc0debeefcafeu64);
        core::ptr::write_volatile(stack_ptr.offset(offset + 3), 0x123456789abcdef0u64);
    }

    // 创建新的地址空间结构
    let new_addr_space = unsafe { crate::mm::MmStruct::new_user(user_ppn) };

    // 更新进程
    unsafe {
        // 设置新的地址空间
        (*task_ptr).set_address_space(Some(alloc::boxed::Box::new(new_addr_space)));

        // 更新 exe_path
        (*task_ptr).set_exe_path(pathname.as_bytes());

        // 设置用户栈指针
        (*task_ptr).set_user_sp(adjusted_stack_top);

        // 切换到新的地址空间
        let satp = (8u64 << 60) | (user_ppn);  // MODE=8 (Sv39), PPN=user_ppn
        core::arch::asm!(
            "csrw satp, {}",
            "sfence.vma",
            in(reg) satp,
            options(nostack)
        );

        // 设置 execve 上下文（新入口点）
        let user_ctx = crate::arch::riscv64::context::UserContext::new(entry, adjusted_stack_top);
        (*task_ptr).set_execve_context(user_ctx);
    }

    Ok(())
}
