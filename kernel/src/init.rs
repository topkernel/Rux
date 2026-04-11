//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Init process management module
//!
//!
//! The init process is the first userspace process after kernel boot, responsible for:
//! - Mounting the root filesystem
//! - Starting system services
//! - Running shell

use crate::arch::riscv64::mm::{self, PageTableEntry, AddressSpace, get_kernel_page_table_ppn, phys_to_virt, PhysAddr};
use crate::fs::elf::{ElfLoader, ElfError, Elf64Ehdr};
use crate::fs::char_dev::CharDev;
use crate::fs::FdTable;
use crate::sched;
use crate::process::task::{Task, SchedPolicy};
use crate::println;
use crate::cmdline;
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::boxed::Box;
use core::slice;

// Static storage: init process and user context
// Use MaybeUninit to avoid auto-initialization issues
static mut INIT_TASK_STORAGE: core::mem::MaybeUninit<Task> = core::mem::MaybeUninit::uninit();

/// Initialize init process (PID 1)
///
///
/// # Features
/// 1. Create init process (PID 1)
/// 2. Load init program
/// 3. Set up standard file descriptors
/// 4. Add init process to scheduler
///
/// # Note
/// - Init process is the ancestor of all userspace processes
/// - If init exits, kernel will panic
pub fn init() {
    // Get init program path from command line
    let init_path = cmdline::get_init_program();

    // Try loading init program from RootFS
    let program_data = load_init_program(&init_path);

    if let Some(data) = program_data {
        // Create and start init process
        if create_and_start_init_process(&data, &init_path).is_none() {
            println!("init: Failed to create init process for {}", init_path);
            halt();
        }
    } else {
        println!("init: Failed to load {} from filesystem", init_path);
        halt();
    }
}

/// Load init program data
///
/// # Arguments
/// - `path`: init program path
///
/// # Returns
/// - `Some(data)`: Program data
/// - `None`: Load failed
///
/// # Loading order
/// 1. Try reading from PCI VirtIO block device's ext4 filesystem
/// 2. Try reading from MMIO VirtIO block device's ext4 filesystem
/// 3. Try reading from RootFS (memory filesystem)
fn load_init_program(path: &str) -> Option<Vec<u8>> {
    // 1. First try reading from PCI VirtIO block device's ext4 filesystem
    if let Some(disk) = crate::drivers::virtio::get_pci_gen_disk() {
        match crate::fs::ext4::read_file(disk as *const _, path) {
            Some(data) => {
                return Some(data);
            }
            None => {}
        }
    }

    // 2. Try reading from MMIO VirtIO block device's ext4 filesystem
    if let Some(virtio_dev) = crate::drivers::virtio::get_device() {
        let disk_ptr = &virtio_dev.disk as *const crate::drivers::blkdev::GenDisk;

        match crate::fs::ext4::read_file(disk_ptr, path) {
            Some(data) => {
                return Some(data);
            }
            None => {}
        }
    }

    // 3. Try reading from RootFS (memory filesystem)
    crate::fs::read_file_from_rootfs(path)
}

/// Create and start init process
///
/// This function will:
/// 1. Create init process structure
/// 2. Load ELF program into memory
/// 3. Mark init process as user process
/// 4. Add to scheduler run queue
fn create_and_start_init_process(program_data: &[u8], init_path: &str) -> Option<*mut Task> {
    unsafe {
        let task_ptr = INIT_TASK_STORAGE.as_mut_ptr();

        // Create init task, PID is fixed to 1
        // Note: new_task_at already allocates kernel stack internally
        Task::new_task_at(task_ptr, 1, SchedPolicy::Normal);

        (*task_ptr).set_parent(core::ptr::null_mut());

        // Create and initialize file descriptor table
        let fdtable = alloc::sync::Arc::new(FdTable::new());
        (*task_ptr).set_fdtable(Some(fdtable));

        // Create and initialize signal handling structure
        let signal_struct = alloc::sync::Arc::new(crate::signal::SignalStruct::new());
        (*task_ptr).signal = Some(signal_struct);

        // Create and initialize filesystem info (cwd, root, umask)
        let fs_struct = alloc::sync::Arc::new(crate::fs::FsStruct::new());
        (*task_ptr).set_fs(Some(fs_struct));

        // Initialize standard file descriptors
        // Note: FdTable has interior mutability, so &FdTable is sufficient
        if let Some(fdtable) = (*task_ptr).try_fdtable() {
            init_std_fds_for_task(fdtable);
        } else {
            return None;
        }

        // Load ELF program into memory and set up user context
        if load_and_setup_elf(task_ptr, program_data, init_path).is_err() {
            return None;
        }

        // Mark as user process (using TaskState::new(TaskState::RUNNING))
        (*task_ptr).set_state(crate::process::task::TaskState::new(crate::process::task::TaskState::RUNNING));

        // Register init process in PID hash table (required for find_task_by_pid)
        crate::process::pid_hash::pid_hash_insert(task_ptr);

        // Add init process to run queue
        sched::sched::enqueue_task(&mut *task_ptr);

        Some(task_ptr)
    }
}

/// Load ELF and set up user context
///
/// This function will:
/// 1. Validate ELF format
/// 2. Create user address space
/// 3. Allocate user memory and stack
/// 4. Load ELF segments
/// 5. Create UserContext and store in Task
fn load_and_setup_elf(task_ptr: *mut Task, program_data: &[u8], init_path: &str) -> Result<(), ElfError> {
    // Validate ELF format
    ElfLoader::validate(program_data)?;

    // Get entry point
    let entry = ElfLoader::get_entry(program_data)?;

    // Get program header count
    let phdr_count = ElfLoader::get_program_headers(program_data)?;

    let ehdr = unsafe { Elf64Ehdr::from_bytes(program_data) }
        .ok_or(ElfError::InvalidHeader)?;

    // Find virtual address range
    let mut min_vaddr: u64 = u64::MAX;
    let mut max_vaddr: u64 = 0;

    // For storing gp value (__global_pointer$ = BSS segment start address)
    let mut global_pointer: u64 = 0;

    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(ElfError::InvalidProgramHeaders)?;

        if phdr.is_load() {
            let virt_addr = phdr.p_vaddr;
            let mem_size = phdr.p_memsz;
            let file_size = phdr.p_filesz;

            if virt_addr < min_vaddr {
                min_vaddr = virt_addr;
            }
            if virt_addr + mem_size > max_vaddr {
                max_vaddr = virt_addr + mem_size;
            }

            // Calculate global pointer: BSS segment start address (vaddr + filesz)
            // Note: This calculation may be incorrect because __global_pointer$ is set at link time
            // RISC-V programs usually set gp themselves in _start, so set to 0 here
            // If the program depends on kernel setting gp, may need to read __global_pointer$ from ELF symbol table
            // Keep as 0 for now, let program startup code set it itself
            if mem_size > file_size && virt_addr > 0x10000 {
                // Don't set global_pointer, let program set it itself
            }
        }
    }

    // Page align
    let virt_start = min_vaddr & !(mm::PAGE_SIZE - 1);
    let virt_end = (max_vaddr + mm::PAGE_SIZE - 1) & !(mm::PAGE_SIZE - 1);

    // Reserve extra space for stack and TLS (1MB)
    // musl libc needs this space to store pthread structures, DTV and TLS data
    const STACK_TLS_RESERVED: u64 = 1024 * 1024;
    let total_size = virt_end - virt_start + STACK_TLS_RESERVED;

    // Create user address space (independent user page table)
    // User page table contains kernel mapping (for system calls) and userspace mapping
    let user_ppn = mm::create_user_address_space().ok_or(ElfError::OutOfMemory)?;

    // One-time allocate and map entire user memory range to user page table
    let flags = PageTableEntry::V | PageTableEntry::U |
               PageTableEntry::R | PageTableEntry::W |
               PageTableEntry::X | PageTableEntry::A |
               PageTableEntry::D;

    let phys_base = unsafe {
        mm::alloc_and_map_to_user_table(
            user_ppn,
            virt_start,
            total_size,
            flags,
        )
    }.ok_or(ElfError::OutOfMemory)?;

    // Verify phys_base is within valid range
    if phys_base < 0x80000000 {
        return Err(ElfError::OutOfMemory);
    }

    // Second pass: load each segment's data
    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(ElfError::InvalidProgramHeaders)?;

        if phdr.is_load() {
            let virt_addr = phdr.p_vaddr;
            let file_size = phdr.p_filesz;
            let mem_size = phdr.p_memsz;
            let offset = phdr.p_offset as usize;
            let _flags = phdr.p_flags;

            // Calculate physical address
            let virt_offset = virt_addr - virt_start;
            let phys_addr = (phys_base + virt_offset) as usize;

            // Convert physical address to kernel virtual address for access
            let kernel_virt_addr = phys_to_virt(PhysAddr::new(phys_addr as u64));
            let virt_addr_ptr = kernel_virt_addr.bits() as *mut u8;

            // Copy ELF data to physical memory
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

    // Stack is already in ELF's PT_LOAD segment (defined by linker script)
    // But we need to ensure there's enough space for argc/argv/auxv setup
    //
    // musl libc expected stack layout:
    //   sp+0      argc (8 bytes)
    //   sp+8      argv[0] pointer
    //   sp+16     argv[1] pointer
    //   ...
    //   NULL (8 bytes)
    //   envp[0] pointer
    //   ...
    //   NULL (8 bytes)
    //   auxv[0].a_type (8 bytes)
    //   auxv[0].a_val (8 bytes)
    //   ...
    //   AT_NULL (16 bytes: type=0, val=0)
    //
    // Key auxv entries needed by musl:
    //   AT_PHDR (3)   - Program header table address
    //   AT_PHENT (4)  - Program header entry size
    //   AT_PHNUM (5)  - Program header count
    //   AT_PAGESZ (6) - Page size
    //   AT_ENTRY (9)  - Entry point address
    //   AT_UID (11)   - User ID
    //   AT_GID (13)   - Group ID
    //   AT_RANDOM (25)- Random number pointer
    //
    // Place stack at the end of mapped region (virt_end + STACK_TLS_RESERVED - 256)
    // This ensures stack is always within valid userspace range and has enough space for TLS
    let stack_top = virt_end + STACK_TLS_RESERVED - 256;

    // Set initial stack content
    // Calculate physical address of stack content
    let virt_offset = stack_top - virt_start;
    let phys_stack_top = (phys_base + virt_offset) as usize;

    // Program header table info
    let phent = ehdr.e_phentsize as u64;
    let phnum = ehdr.e_phnum as u64;
    let phsize = phnum.checked_mul(phent).ok_or(ElfError::InvalidProgramHeaders)? as usize;  // Program header table total size
    let page_size = mm::PAGE_SIZE as u64;

    // Program header table handling:
    // Always copy program header table to user stack, this is more reliable
    // (even if it's in PT_LOAD segment, userspace access may have issues)
    let phdr_file_offset = ehdr.e_phoff;
    let need_phdr_copy = true;  // Always copy

    // auxv type constants
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

    // Stack layout (from low to high address):
    //   slot 0: argc
    //   slot 1: argv[0]
    //   slot 2: argv terminator (NULL)
    //   slot 3: envp terminator (NULL)
    //   slots 4 to 4+auxv_slots-1: auxv entries
    //   slots after auxv: random bytes (2 slots = 16 bytes)
    //   slots after random: strings (argv[0])

    // toybox determines command name through argv[0]'s basename
    // When /bin/sh -> toybox, argv[0] = "/bin/sh", toybox will extract "sh" as command
    // So only need to pass argv[0], no extra parameters needed
    let argc: u64 = 1;
    let argv_count: usize = 1;

    // Default environment variables for init process
    const INIT_ENV_VARS: &[&str] = &[
        "PATH=/bin:/usr/bin:/sbin:/usr/sbin",
        "HOME=/root",
        "TERM=linux",
        "PS1=\x1b[1;32mroot\x1b[0m:\x1b[1;34m${PWD}\x1b[0m# ",
        "ENV=/etc/mrshrc",
    ];
    let env_count: usize = INIT_ENV_VARS.len();

    // auxv entry count
    // AT_PHDR, AT_PHENT, AT_PHNUM, AT_PAGESZ, AT_BASE, AT_ENTRY,
    // AT_UID, AT_EUID, AT_GID, AT_EGID, AT_HWCAP, AT_CLKTCK,
    // AT_SECURE, AT_RANDOM, AT_EXECFN, AT_NULL
    let auxv_entries: usize = 15;
    let auxv_slots: usize = auxv_entries * 2;

    // Calculate string storage space (argv strings + env var strings)
    let mut string_space: usize = ((init_path.len() + 1 + 7) / 8) * 8;
    for env in INIT_ENV_VARS.iter() {
        string_space += (env.len() + 1 + 7) / 8 * 8;
    }

    // Calculate program header table storage space (if copy needed)
    let phdr_space: usize = if need_phdr_copy {
        ((phsize + 7) / 8) * 8  // 8-byte aligned
    } else {
        0
    };

    // Calculate offsets for each part (new layout to avoid overlap)
    // Stack layout (from low to high address):
    //   argc, argv, envp, envp_term, auxv, random(16), PHDR, strings
    let envp_end: usize = 1 + argv_count + 1 + env_count;
    let random_bytes_offset: usize = envp_end + 1 + auxv_slots;
    let phdr_offset: usize = random_bytes_offset + 2;  // PHDR after random bytes
    let string_offset: usize = phdr_offset + (phdr_space + 7) / 8;  // strings after PHDR

    // Calculate total slots needed
    let pre_string_slots: usize = 1 + argv_count + 1 + env_count + 1 + auxv_slots + 2;
    let total_extra_slots: usize = pre_string_slots + (phdr_space + 7) / 8 + (string_space + 7) / 8;
    let adjusted_stack_top = stack_top.saturating_sub((total_extra_slots * 8) as u64);

    // Validate that adjusted_stack_top does not underflow below virt_start
    if adjusted_stack_top < virt_start {
        return Err(ElfError::OutOfMemory);
    }

    // Correctly calculate physical address corresponding to adjusted_stack_top
    // Physical address = phys_base + (virtual address - virt_start)
    let adjusted_virt_offset = adjusted_stack_top - virt_start;
    let adjusted_phys_stack_top = (phys_base + adjusted_virt_offset) as usize;

    // Convert physical address to kernel virtual address for stack access
    let adjusted_stack_virt_addr = phys_to_virt(PhysAddr::new(adjusted_phys_stack_top as u64));
    let adjusted_stack_virt_addr_bits = adjusted_stack_virt_addr.bits();

    unsafe {
        let stack_ptr = adjusted_stack_virt_addr_bits as *mut u64;
        let mut offset: isize = 0;

        // Calculate program header table address (always on stack)
        let phdr_pos = phdr_offset * 8;
        let phdr_addr = adjusted_stack_top + phdr_pos as u64;

        // Copy program header table data to stack
        let src_ptr = program_data.as_ptr().add(phdr_file_offset as usize);
        let dst_ptr = (stack_ptr as *mut u8).add(phdr_pos);
        core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, phsize);

        // Stack layout (from low to high address):
        // 1. argc (1 slot)
        // 2. argv[0] (argv_count slots)
        // 3. argv terminator (1 slot)
        // 4. envp[0..N] (env_count slots)
        // 5. envp terminator (1 slot)
        // 6. auxv entries (auxv_slots)
        // 7. random bytes (2 slots = 16 bytes)
        // 8. strings (variable)
        // 9. program header table (if copy needed)

        let random_vaddr = adjusted_stack_top + (random_bytes_offset * 8) as u64;

        // Write argv[0] string (init_path)
        let arg0_bytes = init_path.as_bytes();
        let string_pos = string_offset * 8;
        for (i, &b) in arg0_bytes.iter().enumerate() {
            core::ptr::write_volatile(
                (stack_ptr as *mut u8).offset((string_pos + i) as isize),
                b
            );
        }
        core::ptr::write_volatile(
            (stack_ptr as *mut u8).offset((string_pos + arg0_bytes.len()) as isize),
            0
        );
        let arg0_vaddr = adjusted_stack_top + string_pos as u64;

        // Write environment variable strings after argv string
        let mut env_vaddrs: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(env_count);
        let mut env_str_pos = string_pos + ((init_path.len() + 1 + 7) / 8) * 8;
        for env in INIT_ENV_VARS.iter() {
            let env_bytes = env.as_bytes();
            let env_vaddr = adjusted_stack_top + env_str_pos as u64;
            env_vaddrs.push(env_vaddr);
            for (i, &b) in env_bytes.iter().enumerate() {
                core::ptr::write_volatile(
                    (stack_ptr as *mut u8).offset((env_str_pos + i) as isize),
                    b
                );
            }
            core::ptr::write_volatile(
                (stack_ptr as *mut u8).offset((env_str_pos + env_bytes.len()) as isize),
                0
            );
            env_str_pos += (env_bytes.len() + 1 + 7) / 8 * 8;
        }

        // argc
        core::ptr::write_volatile(stack_ptr, argc);
        offset += 1;

        // argv[0]
        core::ptr::write_volatile(stack_ptr.offset(offset), arg0_vaddr);
        offset += 1;

        // argv terminator = NULL
        core::ptr::write_volatile(stack_ptr.offset(offset), 0u64);
        offset += 1;

        // envp pointers
        for &addr in &env_vaddrs {
            core::ptr::write_volatile(stack_ptr.offset(offset), addr);
            offset += 1;
        }

        // envp terminator = NULL
        core::ptr::write_volatile(stack_ptr.offset(offset), 0u64);
        offset += 1;

        // auxv entries - single write, correct order
        let auxv_start = offset;

        // AT_PHDR
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_PHDR);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), phdr_addr);
        offset += 2;

        // AT_PHENT
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_PHENT);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), phent);
        offset += 2;

        // AT_PHNUM
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_PHNUM);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), phnum);
        offset += 2;

        // AT_PAGESZ
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_PAGESZ);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), page_size);
        offset += 2;

        // AT_BASE (interpreter, 0 for static)
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_BASE);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);
        offset += 2;

        // AT_ENTRY
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_ENTRY);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), entry);
        offset += 2;

        // AT_UID
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_UID);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);
        offset += 2;

        // AT_EUID
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_EUID);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);
        offset += 2;

        // AT_GID
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_GID);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);
        offset += 2;

        // AT_EGID
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_EGID);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);
        offset += 2;

        // AT_HWCAP
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_HWCAP);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);
        offset += 2;

        // AT_CLKTCK
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_CLKTCK);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 100u64);
        offset += 2;

        // AT_SECURE - not a setuid program
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_SECURE);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);
        offset += 2;

        // AT_RANDOM - pointer to random bytes
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_RANDOM);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), random_vaddr);
        offset += 2;

        // AT_EXECFN - executable filename (points to argv[0] string)
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_EXECFN);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), arg0_vaddr);
        offset += 2;

        // AT_NULL - terminator
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_NULL);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);
        offset += 2;

        // Write 16 bytes random number (after auxv)
        core::ptr::write_volatile(stack_ptr.offset(offset), 0xdeadc0debeefcafeu64);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0x123456789abcdef0u64);
    }

    // ===== Use fork to set up pt_regs =====
    // init process returns to user mode through ret_from_fork
    // Uses same path as fork child process
    //
    // Steps:
    //   pt_regs is stored at kernel stack top
    //   thread.ra = ret_from_fork
    //   thread.sp = pt_regs (address)
    unsafe {
        use crate::arch::riscv64::pt_regs::PtRegs;

        // Get pt_regs at kernel stack top
        let child_regs = (*task_ptr).pt_regs();
        if child_regs.is_null() {
            return Err(ElfError::OutOfMemory);
        }

        // Set PtRegs - construct trap frame for returning to user mode
        // SPP = 0 means return to user mode, SPIE = 1 means enable interrupts
        const SR_SPP: u64 = 1 << 8;
        const SR_SPIE: u64 = 1 << 5;
        const SR_SUM: u64 = 1 << 18;

        let child_status = SR_SPIE | SR_SUM;  // Clear SPP, set SPIE and SUM

        core::ptr::write(child_regs, PtRegs {
            epc: entry,                    // User program entry point
            ra: 0,                         // Return address (not needed in user mode)
            sp: adjusted_stack_top,        // User stack pointer
            gp: global_pointer,            // Global pointer
            tp: 0,                         // TLS pointer (set by musl libc)
            t0: 0, t1: 0, t2: 0,
            s0: 0, s1: 0,
            a0: 0,                         // argc is on stack
            a1: 0, a2: 0, a3: 0, a4: 0, a5: 0, a6: 0, a7: 0,
            s2: 0, s3: 0, s4: 0, s5: 0, s6: 0, s7: 0, s8: 0, s9: 0, s10: 0, s11: 0,
            t3: 0, t4: 0, t5: 0, t6: 0,
            status: child_status,
            badaddr: 0,
            cause: 0,
            orig_a0: 0,
        });

        // Set up thread struct for context switch
        // For init process (NOT created by fork), we directly return to user mode
        // via ret_from_exception, not ret_from_fork
        extern "C" {
            fn ret_from_exception();
        }

        // Kernel is linked at KERNEL_LINK_ADDR, so function pointers are
        // already virtual addresses. No offset conversion needed.
        let ret_from_exception_addr = ret_from_exception as u64;

        let thread = (*task_ptr).thread_mut();
        thread.ra = ret_from_exception_addr;
        thread.sp = child_regs as u64;        // Stack pointer = pt_regs address

        // Callee-saved registers (s0-s11) are already 0 (initialized in Task::new)
    }

    // Use previously created user address space (user_ppn created at function start)
    let addr_space = unsafe { crate::mm::MmStruct::new_user(user_ppn) };

    // Register VMA for ELF segments
    use crate::mm::vma::{Vma, VmaFlags};
    use crate::mm::page::VirtAddr as PageVirtAddr;

    // Iterate PT_LOAD segments, register VMA for each segment
    let mut first_exec_set = false;
    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(ElfError::InvalidProgramHeaders)?;

        if phdr.is_load() {
            let vaddr = phdr.p_vaddr;
            let memsz = phdr.p_memsz as usize;

            // Page align
            let aligned_vaddr = vaddr & !(mm::PAGE_SIZE - 1);
            let aligned_end = ((vaddr + memsz as u64 + mm::PAGE_SIZE - 1) & !(mm::PAGE_SIZE - 1));

            // Set VMA flags
            let mut vma_flags = VmaFlags::new();
            vma_flags.insert(VmaFlags::READ);
            if phdr.p_flags & crate::fs::elf::PF_W != 0 {
                vma_flags.insert(VmaFlags::WRITE);
            }
            if phdr.p_flags & crate::fs::elf::PF_X != 0 {
                vma_flags.insert(VmaFlags::EXEC);
                // Mark first executable segment as VM_EXECUTABLE
                if !first_exec_set {
                    vma_flags.insert(VmaFlags::EXECUTABLE);
                    first_exec_set = true;
                }
            }

            let vma = Vma::new(
                PageVirtAddr::new(aligned_vaddr as usize),
                PageVirtAddr::new(aligned_end as usize),
                vma_flags,
            );

            // Ignore add errors (some segments may overlap)
            let _ = addr_space.vma_write().add(vma);
        }
    }

    // Register VMA for stack (stack is near virt_end)
    // Note: If ELF's PT_LOAD segment already contains stack area, this will fail
    // But this is not a problem because ELF segments already have correct permissions
    let stack_bottom = virt_end.saturating_sub(64 * 1024);
    let mut stack_vma_flags = VmaFlags::new();
    stack_vma_flags.insert(VmaFlags::READ | VmaFlags::WRITE | VmaFlags::GROWSDOWN);
    let stack_vma = Vma::new(
        PageVirtAddr::new(stack_bottom as usize),
        PageVirtAddr::new(virt_end as usize),
        stack_vma_flags,
    );
    let _ = addr_space.vma_write().add(stack_vma);

    unsafe {
        // Set executable path
        (*task_ptr).set_exe_path(init_path.as_bytes());
        (*task_ptr).set_address_space(Some(alloc::sync::Arc::new(addr_space)));
    }

    Ok(())
}

/// Initialize standard file descriptors for task (stdin/stdout/stderr)
///
/// This function is public and can be reused by fork and other operations
pub fn init_std_fds_for_task(fdtable: &crate::fs::FdTable) {
    use crate::fs::char_dev::{CharDev, CharDevType, UART_OPS};
    use crate::fs::{File, FileFlags};
    use alloc::sync::Arc;

    // Create UART character device (use static to avoid dangling pointer)
    static UART_DEV: CharDev = CharDev::new(CharDevType::UartConsole, 0);

    // Create stdin (fd=0)
    let stdin = Arc::new(File::new(FileFlags::new(FileFlags::O_RDONLY)));
    stdin.set_ops(&UART_OPS);
    stdin.set_private_data(&UART_DEV as *const CharDev as *mut u8);

    // Create stdout (fd=1)
    let stdout = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    stdout.set_ops(&UART_OPS);
    stdout.set_private_data(&UART_DEV as *const CharDev as *mut u8);

    // Create stderr (fd=2)
    let stderr = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    stderr.set_ops(&UART_OPS);
    stderr.set_private_data(&UART_DEV as *const CharDev as *mut u8);

    // Install standard file descriptors
    let _ = fdtable.install_fd(0, stdin);
    let _ = fdtable.install_fd(1, stdout);
    let _ = fdtable.install_fd(2, stderr);
}

/// Halt the system
fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
    }
}
