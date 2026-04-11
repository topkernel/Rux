//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Process execution (execve) implementation
//!
//! - do_execve_elf: Load ELF binary and set up user execution context

use alloc::string::String;
use core::slice;

/// Base virtual address for loading the ELF interpreter (dynamic linker)
const INTERP_BASE: usize = 0x3FBF000000;

/// Execute ELF loading (execve internal function)
///
/// This function will:
/// 1. Create new address space
/// 2. Load ELF segments
/// 3. Load interpreter (dynamic linker) if present
/// 4. Set up user stack (argc, argv, envp, auxv)
/// 5. Update process context (address space, stack pointer, trap frame)
///
pub(crate) fn do_execve_elf(
    task_ptr: *mut crate::process::task::Task,
    program_data: &[u8],
    argv: &[String],
    envp: &[String],
    entry: u64,
    phdr_count: usize,
    ehdr: &crate::fs::elf::Elf64Ehdr,
    pathname: &str,
    interp_data: Option<&[u8]>,
) -> Result<(), i32> {
    use crate::arch::riscv64::mm::{
        alloc_and_map_to_user_table, create_user_address_space,
        PAGE_SIZE, PageTableEntry, phys_to_virt, PhysAddr,
    };

    // Close file descriptors with close-on-exec flag
    // SAFETY: task_ptr points to the current task which is valid throughout execve.
    if let Some(fdtable) = unsafe { (*task_ptr).try_fdtable() } {
        fdtable.close_cloexec_fds();
    }

    // Find virtual address range
    let mut min_vaddr: u64 = u64::MAX;
    let mut max_vaddr: u64 = 0;

    for i in 0..phdr_count {
        // SAFETY: program_data is a valid ELF file slice and i < phdr_count is bounded
        // by the ELF header's e_phnum, so the program header index is in range.
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(crate::errno::Errno::FunctionNotImplemented.as_neg_i32())?;

        if phdr.is_load() {
            let virt_addr = phdr.p_vaddr;
            let mem_size = phdr.p_memsz;

            if virt_addr < min_vaddr {
                min_vaddr = virt_addr;
            }
            if let Some(end) = virt_addr.checked_add(mem_size) {
                if end > max_vaddr {
                    max_vaddr = end;
                }
            } else {
                return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
            }
        }
    }

    // Page align
    let virt_start = min_vaddr & !(PAGE_SIZE - 1);
    let virt_end = (max_vaddr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Calculate initial stack size needed
    let argc = argv.len() as u64;
    let argv_count = argv.len();
    let phent = ehdr.e_phentsize as u64;
    let phnum = ehdr.e_phnum as u64;
    let phsize = (phnum * phent) as usize;

    // Calculate stack layout size (same as below)
    let auxv_slots: usize = 30;  // 15 auxv entries * 2
    let envp_count = envp.len();
    let mut string_space: usize = 0;
    for arg in argv.iter() {
        string_space += ((arg.len() + 1 + 7) / 8) * 8;
    }
    let mut env_string_space: usize = 0;
    for env in envp.iter() {
        env_string_space += ((env.len() + 1 + 7) / 8) * 8;
    }
    let phdr_space: usize = ((phsize + 7) / 8) * 8;
    let total_slots: usize = 1 + argv_count + 1 + envp_count + 1 + auxv_slots + 2 + (phdr_space + 7) / 8 + (string_space + 7) / 8 + (env_string_space + 7) / 8;
    let args_size = (total_slots * 8) as u64;

    // Initial stack size: args + 128KB
    const STACK_EXPAND: u64 = 128 * 1024;
    let initial_stack_size = match args_size.checked_add(STACK_EXPAND).and_then(|v| v.checked_add(PAGE_SIZE - 1)) {
        Some(v) => v & !(PAGE_SIZE - 1),
        None => return Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
    };

    // Maximum stack size (8MB)
    const STACK_MAX_SIZE: u64 = 8 * 1024 * 1024;

    // Total size to allocate: ELF segments + initial stack
    let total_size = match virt_end.checked_sub(virt_start).and_then(|v| v.checked_add(initial_stack_size)) {
        Some(v) => v,
        None => return Err(crate::errno::Errno::InvalidArgument.as_neg_i32()),
    };

    // Create new user address space
    let user_ppn = create_user_address_space().ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

    // Allocate and map user memory (ELF segments + initial stack)
    let flags = PageTableEntry::V | PageTableEntry::U |
               PageTableEntry::R | PageTableEntry::W |
               PageTableEntry::X | PageTableEntry::A |
               PageTableEntry::D;

    // SAFETY: user_ppn is a freshly allocated page table root, virt_start..virt_start+total_size
    // is a valid range, and flags contains valid PTE bits for user pages.
    let phys_base = unsafe {
        alloc_and_map_to_user_table(user_ppn, virt_start, total_size, flags)
    }.ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

    // Load each segment
    for i in 0..phdr_count {
        // SAFETY: Same as above — i < phdr_count and program_data is a valid ELF slice.
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
                // SAFETY: virt_addr_ptr was derived from phys_to_virt of a freshly mapped
                // physical page for this segment, and file_size is within the segment's memsz.
                unsafe {
                    let dst = slice::from_raw_parts_mut(virt_addr_ptr, file_size as usize);
                    dst.copy_from_slice(src);
                }
            }

            // Zero BSS
            if mem_size > file_size {
                let bss_size = (mem_size - file_size) as usize;
                // SAFETY: bss_dst points into the same mapped region right after file data,
                // and bss_size = mem_size - file_size which is within the total allocation.
                unsafe {
                    let bss_dst = virt_addr_ptr.add(file_size as usize);
                    core::ptr::write_bytes(bss_dst, 0, bss_size);
                }
            }
        }
    }

    // Load interpreter (dynamic linker) if present
    let (actual_entry, at_base) = if let Some(interp_bytes) = interp_data {
        let interp_base: u64 = INTERP_BASE as u64;  // mmap_start - 16MB

        // SAFETY: interp_bytes is a validated ELF interpreter file loaded earlier.
        let interp_ehdr = unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(interp_bytes) }
            .ok_or(crate::errno::Errno::ExecFormatError.as_neg_i32())?;

        let mut interp_min_vaddr: u64 = u64::MAX;
        let mut interp_max_vaddr: u64 = 0;
        for i in 0..interp_ehdr.e_phnum as usize {
            // SAFETY: i < e_phnum and interp_bytes is a valid ELF file.
            if let Some(phdr) = unsafe { interp_ehdr.get_program_header(interp_bytes, i) } {
                if phdr.is_load() {
                    if phdr.p_vaddr < interp_min_vaddr { interp_min_vaddr = phdr.p_vaddr; }
                    if let Some(end) = phdr.p_vaddr.checked_add(phdr.p_memsz) {
                        if end > interp_max_vaddr { interp_max_vaddr = end; }
                    } else {
                        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
                    }
                }
            }
        }
        let interp_size = (interp_max_vaddr - interp_min_vaddr + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

        // SAFETY: user_ppn is valid page table root, interp_base/interp_size describe the
        // interpreter's virtual memory range, flags are valid user PTE bits.
        let interp_phys = unsafe {
            alloc_and_map_to_user_table(user_ppn, interp_base, interp_size, flags)
        }.ok_or(crate::errno::Errno::OutOfMemory.as_neg_i32())?;

        let interp_kva = phys_to_virt(PhysAddr::new(interp_phys as u64)).bits();

        // SAFETY: interp_bytes is a valid ELF file, interp_kva is the kernel virtual
        // address of the freshly mapped interpreter memory region.
        let (entry_offset, _) = unsafe {
            crate::fs::elf::ElfLoader::load_dynamic_to(interp_bytes, interp_kva)
        }.map_err(|_| crate::errno::Errno::ExecFormatError.as_neg_i32())?;

        let interp_entry = interp_base + entry_offset;

        crate::pr_debug!("execve: loaded interpreter at {:#x}, entry={:#x}, size={:#x}",
            interp_base, interp_entry, interp_size);

        (interp_entry, interp_base)
    } else {
        (entry, 0)
    };

    // Set up stack
    let stack_top = virt_end + initial_stack_size - 256;
    let virt_offset = stack_top - virt_start;
    let phys_stack_top = (phys_base + virt_offset) as usize;

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

    let phent = ehdr.e_phentsize as u64;
    let phnum = ehdr.e_phnum as u64;
    let phsize = (phnum * phent) as usize;

    // Calculate stack layout
    let auxv_slots: usize = 30;  // 15 auxv entries * 2
    let envp_count = envp.len();
    let mut string_space: usize = 0;
    for arg in argv.iter() {
        string_space += ((arg.len() + 1 + 7) / 8) * 8;
    }
    let mut env_string_space: usize = 0;
    for env in envp.iter() {
        env_string_space += ((env.len() + 1 + 7) / 8) * 8;
    }
    let phdr_space: usize = ((phsize + 7) / 8) * 8;

    let random_offset: usize = 1 + argv_count + 1 + envp_count + 1 + auxv_slots;
    let phdr_offset: usize = random_offset + 2;
    let env_string_offset: usize = phdr_offset + (phdr_space + 7) / 8;
    let string_offset: usize = env_string_offset + (env_string_space + 7) / 8;
    let total_slots: usize = string_offset + (string_space + 7) / 8;
    let adjusted_stack_top = stack_top.saturating_sub((total_slots * 8) as u64);

    let adjusted_virt_offset = adjusted_stack_top - virt_start;
    let adjusted_phys_stack_top = (phys_base + adjusted_virt_offset) as usize;

    let adjusted_stack_virt_addr = phys_to_virt(PhysAddr::new(adjusted_phys_stack_top as u64)).bits();

    // SAFETY: adjusted_stack_virt_addr points into freshly mapped and allocated user stack
    // memory. All offsets are pre-calculated to stay within the allocation bounds.
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

        // Write envp strings
        let mut envp_addrs: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(envp_count);
        let mut current_env_string_offset = env_string_offset;

        for env in envp.iter() {
            let string_pos = current_env_string_offset * 8;
            let env_bytes = env.as_bytes();
            for (i, &b) in env_bytes.iter().enumerate() {
                core::ptr::write_volatile(
                    (stack_ptr as *mut u8).offset((string_pos + i) as isize),
                    b
                );
            }
            core::ptr::write_volatile(
                (stack_ptr as *mut u8).offset((string_pos + env_bytes.len()) as isize),
                0
            );
            envp_addrs.push(adjusted_stack_top + string_pos as u64);
            current_env_string_offset += ((env.len() + 1 + 7) / 8);
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

        // envp pointers
        for &addr in &envp_addrs {
            core::ptr::write_volatile(stack_ptr.offset(offset), addr);
            offset += 1;
        }

        // envp terminator
        core::ptr::write_volatile(stack_ptr.offset(offset), 0u64);
        offset += 1;

        // auxv
        let auxv = &[
            (AT_PHDR, phdr_addr),
            (AT_PHENT, phent),
            (AT_PHNUM, phnum),
            (AT_PAGESZ, PAGE_SIZE as u64),
            (AT_BASE, at_base),
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
    // SAFETY: user_ppn is a freshly allocated page table root with no prior users.
    let new_addr_space = unsafe { crate::mm::MmStruct::new_user(user_ppn) };

    // Record envp range for /proc/pid/environ
    if !envp.is_empty() {
        let env_start_addr = adjusted_stack_top + (env_string_offset * 8) as u64;
        let mut env_end_offset = env_string_offset;
        for env in envp.iter() {
            env_end_offset += (env.len() + 1 + 7) / 8;
        }
        let env_end_addr = adjusted_stack_top + (env_end_offset * 8) as u64;
        new_addr_space.setup_envp(env_start_addr as usize, env_end_addr as usize);
    }

    // Set up stack VMA with GROWSDOWN flag and stack limit
    let stack_bottom = virt_end;
    let stack_limit = stack_top.saturating_sub(STACK_MAX_SIZE) + PAGE_SIZE;
    new_addr_space.set_start_stack(stack_top as usize);
    new_addr_space.set_stack_limit(stack_limit as usize);

    // Add stack VMA with GROWSDOWN flag
    {
        use crate::mm::vma::{Vma, VmaFlags, VmaType};
        let stack_vma = Vma::new(
            crate::mm::page::VirtAddr::new(stack_bottom as usize),
            crate::mm::page::VirtAddr::new(stack_top as usize),
            VmaFlags::from_bits(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSDOWN),
        );
        let mut vma_mgr = new_addr_space.vma_write();
        let _ = vma_mgr.add(stack_vma);

        // Add VMAs for each ELF PT_LOAD segment
        let mut first_exec_set = false;
        for i in 0..phdr_count {
            // SAFETY: i < phdr_count and program_data is a valid ELF file slice.
            if let Some(phdr) = unsafe { ehdr.get_program_header(program_data, i) } {
                if phdr.is_load() {
                    let seg_start = phdr.p_vaddr;
                    let seg_end = seg_start + phdr.p_memsz;
                    if seg_end <= seg_start {
                        continue;
                    }

                    // Page-align: start rounds down, end rounds up
                    let vma_start = seg_start & !(PAGE_SIZE as u64 - 1);
                    let vma_end = (seg_end + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
                    if vma_end <= vma_start {
                        continue;
                    }

                    let mut vma_flags = VmaFlags::new();
                    if phdr.is_readable() { vma_flags.insert(VmaFlags::READ); }
                    if phdr.is_writable() { vma_flags.insert(VmaFlags::WRITE); }
                    if phdr.is_executable() { vma_flags.insert(VmaFlags::EXEC); }
                    vma_flags.insert(VmaFlags::PRIVATE);

                    // Mark first executable segment as VM_EXECUTABLE
                    if phdr.is_executable() && !first_exec_set {
                        vma_flags.insert(VmaFlags::EXECUTABLE);
                        first_exec_set = true;
                    }

                    let vma = Vma::new(
                        crate::mm::page::VirtAddr::new(vma_start as usize),
                        crate::mm::page::VirtAddr::new(vma_end as usize),
                        vma_flags,
                    );
                    let _ = vma_mgr.add(vma);
                }
            }
        }

        // Add VMA for interpreter (dynamic linker)
        if let Some(interp_bytes) = interp_data {
            let interp_base: u64 = 0x3FBF000000u64;
            // SAFETY: interp_bytes was already validated as a valid ELF file above.
            let interp_ehdr = unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(interp_bytes) }
                .ok_or(crate::errno::Errno::ExecFormatError.as_neg_i32())?;
            let mut interp_min: u64 = u64::MAX;
            let mut interp_max: u64 = 0;
            for i in 0..interp_ehdr.e_phnum as usize {
                // SAFETY: i < e_phnum and interp_bytes is a valid ELF file.
                if let Some(phdr) = unsafe { interp_ehdr.get_program_header(interp_bytes, i) } {
                    if phdr.is_load() {
                        if phdr.p_vaddr < interp_min { interp_min = phdr.p_vaddr; }
                        if let Some(end) = phdr.p_vaddr.checked_add(phdr.p_memsz) {
                            if end > interp_max { interp_max = end; }
                        } else {
                            return Err(crate::errno::Errno::ExecFormatError.as_neg_i32());
                        }
                    }
                }
            }
            if interp_max > interp_min {
                // Page-align the size
                let raw_size = interp_max - interp_min;
                let interp_size = (raw_size + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
                let mut interp_flags = VmaFlags::from_bits(VmaFlags::READ | VmaFlags::EXEC | VmaFlags::PRIVATE);
                let interp_vma = Vma::new(
                    crate::mm::page::VirtAddr::new(interp_base as usize),
                    crate::mm::page::VirtAddr::new((interp_base + interp_size) as usize),
                    interp_flags,
                );
                let _ = vma_mgr.add(interp_vma);
            }
        }
    }

    // Update process
    // SAFETY: task_ptr is the current task, valid throughout this execve operation.
    // We are replacing the task's address space and trap frame for the new program.
    unsafe {
        // Set new address space (this will drop old Arc if no other references)
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
        const SR_SPIE: u64 = 1 << 5;
        const SR_SUM: u64 = 1 << 18;

        // SAFETY: current_regs was obtained from current_pt_regs() and checked for null above.
        // We are modifying the trap frame to set up the new program's entry point and stack.
        unsafe {
            (*current_regs).epc = actual_entry;         // Entry point (interpreter or program)
            (*current_regs).sp = adjusted_stack_top;   // New user stack
            (*current_regs).status = SR_SPIE | SR_SUM; // Clear SPP, set SPIE and SUM
            (*current_regs).tp = 0;                   // Clear TLS pointer - musl libc will reinitialize
            (*current_regs).a0 = 0;                   // argc is on stack
            // Other registers remain 0
        }

        // Note: Do not free PtRegs memory here because trap frame is on stack
    }

    Ok(())
}
