//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Process-related system calls
//!
//! Includes: clone, execve, exit, wait4, getpid, getppid, kill, set_tid_address, uname, etc.

use super::*;
use crate::arch::riscv64::mm::{phys_to_virt, PhysAddr};
use crate::arch::riscv64::uaccess::strncpy_from_user;

/// sys_clone - Create child process/thread
///
/// # Arguments
/// - args[0]: flags - clone flags
/// - args[1]: stack - new stack pointer
/// - args[2]: parent_tid - parent TID pointer
/// - args[3]: tls - TLS pointer
/// - args[4]: child_tid - child TID pointer
///
/// # Returns
/// Returns child process PID in parent, 0 in child, negative error code on failure
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

/// sys_execve - Execute program
///
/// # Arguments
/// - args[0]: pathname - program path
/// - args[1]: argv - argument array
/// - args[2]: envp - environment variable array
///
/// # Returns
/// Does not return on success, negative error code on failure
pub fn sys_execve(args: SyscallArgs) -> u64 {
    use crate::fs::elf::{ElfLoader, Elf64Ehdr};
    use alloc::vec::Vec;
    use alloc::string::String;

    let pathname_ptr = args[0] as *const u8;
    let argv_ptr = args[1] as *const *const u8;

    // Check path pointer
    if pathname_ptr.is_null() {
        return -errno::EFAULT as u64;
    }

    // Read pathname from user space safely
    let mut kernel_buf = [0u8; 256];
    let pathname = match strncpy_from_user(pathname_ptr, 256, &mut kernel_buf) {
        Ok(s) => s,
        Err(e) => return e as u64,
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -errno::EINVAL as u64,
    };

    // Build full path
    let full_path = if pathname_str.starts_with('/') {
        alloc::borrow::Cow::Borrowed(pathname_str)
    } else {
        if let Some(current) = crate::sched::current() {
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(&cwd) {
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

    // Read ELF file from file system
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

    // Validate ELF format
    if ElfLoader::validate(&program_data).is_err() {
        return -errno::ENOEXEC as u64;
    }

    // Get entry point
    let entry = match ElfLoader::get_entry(&program_data) {
        Ok(e) => e,
        Err(_) => return -errno::ENOEXEC as u64,
    };

    // Get program header count
    let phdr_count = match ElfLoader::get_program_headers(&program_data) {
        Ok(n) => n,
        Err(_) => return -errno::ENOEXEC as u64,
    };

    let ehdr = match unsafe { Elf64Ehdr::from_bytes(&program_data) } {
        Some(e) => e,
        None => return -errno::ENOEXEC as u64,
    };

    // Parse argv
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

    // Get current process
    let current = match crate::sched::current() {
        Some(c) => c,
        None => return -errno::ESRCH as u64,
    };

    // Execute ELF loading
    match do_execve_elf(current, &program_data, &argv, entry, phdr_count as usize, &ehdr, full_path.as_ref()) {
        Ok(()) => 0,
        Err(e) => e as i64 as u64,
    }
}

/// sys_exit - Exit process
///
/// # Arguments
/// - args[0]: status - exit status code
///
/// # Returns
/// Does not return
pub fn sys_exit(args: SyscallArgs) -> u64 {
    let exit_code = args[0] as i32;
    crate::sched::do_exit(exit_code);
}

/// sys_wait4 - Wait for child process
///
/// # Arguments
/// - args[0]: pid - process ID to wait for
/// - args[1]: status - pointer to store exit status
/// - args[2]: options - wait options
/// - args[3]: rusage - resource usage statistics pointer
///
/// # Returns
/// Returns child process PID on success, negative error code on failure
pub fn sys_wait4(args: SyscallArgs) -> u64 {
    let pid = args[0] as i32;
    let wstatus = args[1] as *mut i32;
    let options = args[2] as i32;
    let _rusage = args[3] as *mut u8;

    // Validate wstatus pointer
    if !wstatus.is_null() && !crate::arch::riscv64::uaccess::access_ok(wstatus as usize, 4) {
        return -errno::EFAULT as u64;
    }

    // WNOHANG: If no child process has exited, return 0 immediately
    const WNOHANG: i32 = 0x00000001;

    if options & WNOHANG != 0 {
        // WNOHANG mode: non-blocking check
        match crate::sched::do_wait_nonblock(pid, wstatus) {
            Ok(child_pid) => child_pid as u64,
            Err(e) if e == -11 => 0,  // EAGAIN -> return 0 means no child process exited
            Err(e) => e as u32 as u64,
        }
    } else {
        // Blocking wait for child process to exit
        match crate::sched::do_wait(pid, wstatus) {
            Ok(child_pid) => child_pid as u64,
            Err(e) => e as u32 as u64,
        }
    }
}

/// sys_getpid - Get process ID
pub fn sys_getpid(_args: SyscallArgs) -> u64 {
    if let Some(current) = crate::sched::current() {
        unsafe { (*current).pid() as u64 }
    } else {
        0
    }
}

/// sys_getppid - Get parent process ID
pub fn sys_getppid(_args: SyscallArgs) -> u64 {
    crate::process::current_ppid() as u64
}

/// sys_kill - Send signal
pub fn sys_kill(args: SyscallArgs) -> u64 {
    let pid = args[0] as i32;
    let sig = args[1] as i32;

    if sig < 0 || sig > 64 {
        return -errno::EINVAL as u64;
    }

    if pid <= 0 {
        // Process group operations not supported
        return -errno::ESRCH as u64;
    }

    // Find target process and send signal
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

/// sys_set_tid_address - Set TID address
pub fn sys_set_tid_address(args: SyscallArgs, tp: u64) -> u64 {
    let tidptr = args[0] as *mut i32;

    // Validate tidptr pointer
    if !tidptr.is_null() && !crate::arch::riscv64::uaccess::access_ok(tidptr as usize, 4) {
        return -errno::EFAULT as u64;
    }

    if let Some(current) = crate::sched::current() {
        unsafe {
            (*current).set_clear_child_tid(tidptr);
            return (*current).pid() as u64;
        }
    }

    0
}

/// sys_set_robust_list - Set robust list
pub fn sys_set_robust_list(_args: SyscallArgs) -> u64 {
    // Simplified implementation
    0
}

/// sys_uname - Get system information
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

    // Validate user pointer
    if !crate::arch::riscv64::uaccess::access_ok(buf as usize, core::mem::size_of::<Utsname>()) {
        return -errno::EFAULT as u64;
    }

    unsafe {
        let uname = &mut *buf;

        // Fill system information
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

/// sys_getuid - Get user ID
pub fn sys_getuid(_args: SyscallArgs) -> u64 {
    0  // root
}

/// sys_getgid - Get group ID
pub fn sys_getgid(_args: SyscallArgs) -> u64 {
    0  // root
}

/// sys_geteuid - Get effective user ID
pub fn sys_geteuid(_args: SyscallArgs) -> u64 {
    0  // root
}

/// sys_getegid - Get effective group ID
pub fn sys_getegid(_args: SyscallArgs) -> u64 {
    0  // root
}

/// sys_prlimit64 - Get/set resource limits
pub fn sys_prlimit64(args: SyscallArgs) -> u64 {
    let _pid = args[0] as i32;
    let resource = args[1] as i32;
    let new_rlim = args[2] as *const u8;
    let old_rlim = args[3] as *mut u8;

    // Validate pointers
    if !new_rlim.is_null() && !crate::arch::riscv64::uaccess::access_ok(new_rlim as usize, 16) {
        return -errno::EFAULT as u64;
    }
    if !old_rlim.is_null() && !crate::arch::riscv64::uaccess::access_ok(old_rlim as usize, 16) {
        return -errno::EFAULT as u64;
    }

    // Only support querying
    if !new_rlim.is_null() {
        return -errno::EPERM as u64;
    }

    if old_rlim.is_null() {
        return -errno::EFAULT as u64;
    }

    // RLIMIT_NOFILE = 7
    if resource == 7 {
        unsafe {
            // Return default file descriptor limit
            let rlim = old_rlim as *mut u64;
            *rlim = 1024;        // rlim_cur
            *rlim.offset(1) = 1024 * 1024;  // rlim_max
        }
        return 0;
    }

    -errno::EINVAL as u64
}

/// Execute ELF loading (execve internal function)
///
/// This function will:
/// 1. Create new address space
/// 2. Load ELF segments
/// 3. Set up stack and arguments
/// 4. Update process context
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

    // Find virtual address range
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

    // Page align
    let virt_start = min_vaddr & !(PAGE_SIZE - 1);
    let virt_end = (max_vaddr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Reserve space for stack
    const STACK_RESERVED: u64 = 128 * 1024;
    let total_size = virt_end - virt_start + STACK_RESERVED;

    // Create new user address space
    let user_ppn = create_user_address_space().ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

    // Allocate and map user memory
    let flags = PageTableEntry::V | PageTableEntry::U |
               PageTableEntry::R | PageTableEntry::W |
               PageTableEntry::X | PageTableEntry::A |
               PageTableEntry::D;

    let phys_base = unsafe {
        alloc_and_map_to_user_table(user_ppn, virt_start, total_size, flags)
    }.ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

    // Load each segment
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

            // Convert physical address to kernel virtual address for access
            let virt_addr_ptr = phys_to_virt(PhysAddr::new(phys_addr as u64)).bits() as *mut u8;

            // Copy data
            if file_size > 0 {
                let src = &program_data[offset..offset + file_size as usize];
                unsafe {
                    let dst = slice::from_raw_parts_mut(virt_addr_ptr, file_size as usize);
                    dst.copy_from_slice(src);
                }
            }

            // Zero BSS
            if mem_size > file_size {
                let bss_size = (mem_size - file_size) as usize;
                unsafe {
                    let bss_dst = virt_addr_ptr.add(file_size as usize);
                    core::ptr::write_bytes(bss_dst, 0, bss_size);
                }
            }
        }
    }

    // Set up stack
    let stack_top = virt_end + STACK_RESERVED - 256;
    let virt_offset = stack_top - virt_start;
    let phys_stack_top = (phys_base + virt_offset) as usize;

    // Convert physical address to kernel virtual address for stack access
    let stack_virt_addr = phys_to_virt(PhysAddr::new(phys_stack_top as u64)).bits();

    // auxv constants
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

    // Calculate stack layout
    let auxv_slots: usize = 30;  // 15 auxv entries * 2
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

    // Convert physical address to kernel virtual address for stack access
    let adjusted_stack_virt_addr = phys_to_virt(PhysAddr::new(adjusted_phys_stack_top as u64)).bits();

    unsafe {
        let stack_ptr = adjusted_stack_virt_addr as *mut u64;
        let mut offset: isize = 0;

        let phdr_addr = adjusted_stack_top + (phdr_offset * 8) as u64;
        let random_vaddr = adjusted_stack_top + (random_offset * 8) as u64;

        // Copy program header table
        let src_ptr = program_data.as_ptr().add(ehdr.e_phoff as usize);
        let dst_ptr = (stack_ptr as *mut u8).add(phdr_offset * 8);
        core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, phsize);

        // Write argv strings
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

        // Random numbers
        core::ptr::write_volatile(stack_ptr.offset(offset + 2), 0xdeadc0debeefcafeu64);
        core::ptr::write_volatile(stack_ptr.offset(offset + 3), 0x123456789abcdef0u64);
    }

    // Create new address space structure
    let new_addr_space = unsafe { crate::mm::MmStruct::new_user(user_ppn) };

    // Update process
    unsafe {
        // Set new address space
        (*task_ptr).set_address_space(Some(alloc::sync::Arc::new(new_addr_space)));

        // Update exe_path
        (*task_ptr).set_exe_path(pathname.as_bytes());

        // Set user stack pointer
        (*task_ptr).set_user_sp(adjusted_stack_top);

        // Switch to new address space
        let satp = (8u64 << 60) | (user_ppn);  // MODE=8 (Sv39), PPN=user_ppn
        core::arch::asm!(
            "csrw satp, {}",
            "sfence.vma",
            in(reg) satp,
            options(nostack)
        );

        // ===== Return to user mode immediately after successful execve =====
        // After execve returns, sret will jump to new program entry

        // Get current trap frame
        use crate::arch::riscv64::trap::current_pt_regs;
        use crate::arch::riscv64::pt_regs::PtRegs;
        let current_regs = current_pt_regs() as *mut PtRegs;
        if current_regs.is_null() {
            // No trap frame, this is the init process case
            // Need to return via ret_from_fork
            return Ok(());
        }

        // Directly modify current trap frame
        // SPP = 0 means return to user mode, SPIE = 1 means enable interrupts
        const SR_SPP: u64 = 1 << 8;
        const SR_SPIE: u64 = 1 << 5;
        const SR_SUM: u64 = 1 << 18;

        unsafe {
            (*current_regs).epc = entry;                // New program entry point
            (*current_regs).sp = adjusted_stack_top;   // New user stack
            (*current_regs).status = SR_SPIE | SR_SUM; // Clear SPP, set SPIE and SUM
            (*current_regs).a0 = 0;                   // argc is on stack
            // Other registers remain 0
        }

        // Note: Do not free PtRegs memory here because trap frame is on stack
    }

    Ok(())
}
