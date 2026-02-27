//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Init 进程管理模块
//!
//! ...
//!
//! Init 进程是内核启动后的第一个用户空间进程，负责：
//! - 挂载根文件系统
//! - 启动系统服务
//! - 运行 shell

use crate::arch::riscv64::mm::{self, PageTableEntry, AddressSpace, get_kernel_page_table_ppn};
use crate::arch::riscv64::context::UserContext;
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

// 静态存储：init 进程和用户上下文
// 使用 MaybeUninit 避免自动初始化问题
static mut INIT_TASK_STORAGE: core::mem::MaybeUninit<Task> = core::mem::MaybeUninit::uninit();
static mut INIT_USER_CTX_STORAGE: core::mem::MaybeUninit<UserContext> = core::mem::MaybeUninit::uninit();

/// 初始化 init 进程（PID 1）
///
///
/// # 功能
/// 1. 创建 init 进程（PID 1）
/// 2. 加载 init 程序
/// 3. 设置标准文件描述符
/// 4. 将 init 进程加入调度器
///
/// # 注意
/// - Init 进程是所有用户空间进程的祖先
/// - 如果 init 退出，内核会 panic
pub fn init() {
    // 从命令行获取 init 程序路径
    let init_path = cmdline::get_init_program();

    // 尝试从 RootFS 加载 init 程序
    let program_data = load_init_program(&init_path);

    if let Some(data) = program_data {
        // 创建并启动 init 进程
        if create_and_start_init_process(&data, &init_path).is_none() {
            println!("init: Failed to create init process for {}", init_path);
            halt();
        }
    } else {
        println!("init: Failed to load {} from filesystem", init_path);
        halt();
    }
}

/// 加载 init 程序数据
///
/// # 参数
/// - `path`: init 程序路径
///
/// # 返回
/// - `Some(data)`: 程序数据
/// - `None`: 加载失败
///
/// # 加载顺序
/// 1. 尝试从 PCI VirtIO 块设备的 ext4 文件系统读取
/// 2. 尝试从 MMIO VirtIO 块设备的 ext4 文件系统读取
/// 3. 尝试从 RootFS（内存文件系统）读取
fn load_init_program(path: &str) -> Option<Vec<u8>> {
    // 1. 首先尝试从 PCI VirtIO 块设备的 ext4 文件系统读取
    if let Some(disk) = crate::drivers::virtio::get_pci_gen_disk() {
        match crate::fs::ext4::read_file(disk as *const _, path) {
            Some(data) => {
                return Some(data);
            }
            None => {}
        }
    }

    // 2. 尝试从 MMIO VirtIO 块设备的 ext4 文件系统读取
    if let Some(virtio_dev) = crate::drivers::virtio::get_device() {
        let disk_ptr = &virtio_dev.disk as *const crate::drivers::blkdev::GenDisk;

        match crate::fs::ext4::read_file(disk_ptr, path) {
            Some(data) => {
                return Some(data);
            }
            None => {}
        }
    }

    // 3. 尝试从 RootFS（内存文件系统）读取
    crate::fs::read_file_from_rootfs(path)
}

/// 创建并启动 init 进程
///
/// 这个函数会：
/// 1. 创建 init 进程结构
/// 2. 加载 ELF 程序到内存
/// 3. 将 init 进程标记为用户进程
/// 4. 加入调度器运行队列
fn create_and_start_init_process(program_data: &[u8], init_path: &str) -> Option<*mut Task> {
    unsafe {
        let task_ptr = INIT_TASK_STORAGE.as_mut_ptr();

        // 创建 init 任务，PID 固定为 1
        Task::new_task_at(task_ptr, 1, SchedPolicy::Normal);

        // 设置父进程为 0（没有父进程）
        (*task_ptr).set_parent(core::ptr::null_mut());

        // 创建并初始化文件描述符表
        let fdtable = Box::new(FdTable::new());
        (*task_ptr).set_fdtable(Some(fdtable));

        // 创建并初始化信号处理结构
        let signal_struct = Box::new(crate::signal::SignalStruct::new());
        (*task_ptr).signal = Some(signal_struct);

        // 初始化标准文件描述符
        if let Some(fdtable) = (*task_ptr).try_fdtable_mut() {
            init_std_fds_for_task(fdtable);
        } else {
            return None;
        }

        // 加载 ELF 程序到内存并设置用户上下文
        if load_and_setup_elf(task_ptr, program_data, init_path).is_err() {
            return None;
        }

        // 标记为用户进程（使用 TaskState::new(TaskState::RUNNING)）
        (*task_ptr).set_state(crate::process::task::TaskState::new(crate::process::task::TaskState::RUNNING));

        // 将 init 进程加入运行队列
        sched::sched::enqueue_task(&mut *task_ptr);

        Some(task_ptr)
    }
}

/// 加载 ELF 并设置用户上下文
///
/// 这个函数会：
/// 1. 验证 ELF 格式
/// 2. 分配用户内存和栈
/// 3. 加载 ELF 段
/// 4. 创建 UserContext 并存储在 Task 中
fn load_and_setup_elf(task_ptr: *mut Task, program_data: &[u8], init_path: &str) -> Result<(), ElfError> {
    // 验证 ELF 格式
    ElfLoader::validate(program_data)?;

    // 获取入口点
    let entry = ElfLoader::get_entry(program_data)?;

    // 获取程序头数量
    let phdr_count = ElfLoader::get_program_headers(program_data)?;

    let ehdr = unsafe { Elf64Ehdr::from_bytes(program_data) }
        .ok_or(ElfError::InvalidHeader)?;

    // 找到虚拟地址范围
    let mut min_vaddr: u64 = u64::MAX;
    let mut max_vaddr: u64 = 0;

    // 用于存储 gp 值（__global_pointer$ = BSS 段起始地址）
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

            // 计算全局指针：BSS 段起始地址（vaddr + filesz）
            // 注意：这个计算可能不正确，因为 __global_pointer$ 是链接时设置的
            // RISC-V 程序的 _start 通常会自己设置 gp，所以这里设为 0
            // 如果程序依赖内核设置 gp，可能需要从 ELF 符号表中读取 __global_pointer$
            // 暂时保持为 0，让程序的启动代码自己设置
            if mem_size > file_size && virt_addr > 0x10000 {
                // 不设置 global_pointer，让程序自己设置
            }
        }
    }

    // 页对齐
    let virt_start = min_vaddr & !(mm::PAGE_SIZE - 1);
    let virt_end = (max_vaddr + mm::PAGE_SIZE - 1) & !(mm::PAGE_SIZE - 1);
    let total_size = virt_end - virt_start;

    // 一次性分配并映射整个用户内存范围
    let flags = PageTableEntry::V | PageTableEntry::U |
               PageTableEntry::R | PageTableEntry::W |
               PageTableEntry::X | PageTableEntry::A |
               PageTableEntry::D;

    let phys_base = unsafe {
        mm::alloc_and_map_to_kernel_table(
            virt_start,
            total_size,
            flags,
        )
    }.ok_or(ElfError::OutOfMemory)?;

    // 第二遍：加载每个段的数据
    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(ElfError::InvalidProgramHeaders)?;

        if phdr.is_load() {
            let virt_addr = phdr.p_vaddr;
            let file_size = phdr.p_filesz;
            let mem_size = phdr.p_memsz;
            let offset = phdr.p_offset as usize;

            // 计算物理地址
            let virt_offset = virt_addr - virt_start;
            let phys_addr = (phys_base + virt_offset) as usize;

            // 复制 ELF 数据到物理内存
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

    // 栈已经在 ELF 的 PT_LOAD 段中（链接脚本定义）
    // 但我们需要确保有足够的空间设置 argc/argv/auxv
    //
    // musl libc 期望的栈布局：
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
    // musl 需要的关键 auxv 条目：
    //   AT_PHDR (3)   - 程序头表地址
    //   AT_PHENT (4)  - 程序头条目大小
    //   AT_PHNUM (5)  - 程序头数量
    //   AT_PAGESZ (6) - 页大小
    //   AT_ENTRY (9)  - 入口点地址
    //   AT_UID (11)   - 用户 ID
    //   AT_GID (13)   - 组 ID
    //   AT_RANDOM (25)- 随机数指针
    //
    // 将栈放在映射区域的末尾（virt_end - 256）
    // 这确保栈总是在有效的用户空间范围内
    let stack_top = virt_end.saturating_sub(256);

    // 设置初始栈内容
    // 计算栈内容的物理地址
    let virt_offset = stack_top - virt_start;
    let phys_stack_top = (phys_base + virt_offset) as usize;

    // 计算程序头表地址（在用户虚拟地址空间中）
    let phdr_addr = virt_start + ehdr.e_phoff;
    let phent = ehdr.e_phentsize as u64;
    let phnum = ehdr.e_phnum as u64;
    let page_size = mm::PAGE_SIZE as u64;

    // auxv 类型常量
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

    // 栈布局（从低地址到高地址）：
    //   slot 0: argc
    //   slot 1: argv[0]
    //   slot 2: argv terminator (NULL)
    //   slot 3: envp terminator (NULL)
    //   slots 4 to 4+auxv_slots-1: auxv entries
    //   slots after auxv: random bytes (2 slots = 16 bytes)
    //   slots after random: strings (argv[0])

    // toybox 通过 argv[0] 的 basename 来确定命令名
    // 当 /bin/sh -> toybox 时，argv[0] = "/bin/sh"，toybox 会提取 "sh" 作为命令
    // 所以只需要传递 argv[0]，不需要额外的参数
    let argc: u64 = 1;
    let argv_count: usize = 1;

    // auxv 条目数量
    // AT_PHDR, AT_PHENT, AT_PHNUM, AT_PAGESZ, AT_BASE, AT_ENTRY,
    // AT_UID, AT_EUID, AT_GID, AT_EGID, AT_HWCAP, AT_CLKTCK,
    // AT_SECURE, AT_RANDOM, AT_EXECFN, AT_NULL
    let auxv_entries: usize = 15;
    let auxv_slots: usize = auxv_entries * 2;

    // 计算字符串存储空间
    let string_space: usize = ((init_path.len() + 1 + 7) / 8) * 8;

    // 计算各部分的偏移量
    let random_bytes_offset: usize = 1 + argv_count + 1 + 1 + auxv_slots;
    let string_offset: usize = random_bytes_offset + 2;

    // 计算总共需要的 slots
    // = argc(1) + argv(argv_count) + argv_term(1) + envp_term(1) + auxv(auxv_slots) + random(2) + strings
    let pre_string_slots: usize = 1 + argv_count + 1 + 1 + auxv_slots + 2;
    let total_extra_slots: usize = pre_string_slots + (string_space + 7) / 8;
    let adjusted_stack_top = stack_top.saturating_sub((total_extra_slots * 8) as u64);

    // 正确计算 adjusted_stack_top 对应的物理地址
    // 物理地址 = phys_base + (虚拟地址 - virt_start)
    let adjusted_virt_offset = adjusted_stack_top - virt_start;
    let adjusted_phys_stack_top = (phys_base + adjusted_virt_offset) as usize;

    unsafe {
        let stack_ptr = adjusted_phys_stack_top as *mut u64;
        let mut offset: isize = 0;

        // 栈布局（从低地址到高地址）：
        // 1. argc (1 slot)
        // 2. argv[0] (argv_count slots)
        // 3. argv terminator (1 slot)
        // 4. envp terminator (1 slot)
        // 5. auxv entries (auxv_slots)
        // 6. 随机字节 (2 slots = 16 bytes)
        // 7. 字符串 (variable)

        let random_vaddr = adjusted_stack_top + (random_bytes_offset * 8) as u64;

        // 写入 argv[0] 字符串 (init_path)
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

        // argc
        core::ptr::write_volatile(stack_ptr, argc);
        offset += 1;

        // argv[0]
        core::ptr::write_volatile(stack_ptr.offset(offset), arg0_vaddr);
        offset += 1;

        // argv terminator = NULL
        core::ptr::write_volatile(stack_ptr.offset(offset), 0u64);
        offset += 1;

        // 当没有环境变量时，envp 只需要一个 NULL 终止符
        // Linux 标准栈布局: argc, argv[0..n], NULL, envp[0..m], NULL, auxv...
        // 如果没有 envp，则布局是: argc, argv[0..n], NULL, NULL, auxv...
        // 即 argv 终止符后直接跟 envp 终止符（NULL）
        core::ptr::write_volatile(stack_ptr.offset(offset), 0u64); // envp terminator (no env vars)
        offset += 1;

        // auxv 条目 - 单次写入，正确的顺序
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

        // AT_SECURE - 不是 setuid 程序
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_SECURE);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);
        offset += 2;

        // AT_RANDOM - 指向随机数字节
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_RANDOM);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), random_vaddr);
        offset += 2;

        // AT_EXECFN - 可执行文件名（指向 argv[0] 字符串）
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_EXECFN);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), arg0_vaddr);
        offset += 2;

        // AT_NULL - 终止符
        core::ptr::write_volatile(stack_ptr.offset(offset), AT_NULL);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0u64);
        offset += 2;

        // 写入 16 字节随机数（在 auxv 之后）
        core::ptr::write_volatile(stack_ptr.offset(offset), 0xdeadc0debeefcafeu64);
        core::ptr::write_volatile(stack_ptr.offset(offset + 1), 0x123456789abcdef0u64);
    }

    // 创建用户上下文并存储在静态存储中
    unsafe {
        // 在静态存储上构造 UserContext
        let user_ctx_ptr = INIT_USER_CTX_STORAGE.as_mut_ptr();
        let user_ctx = crate::arch::riscv64::context::UserContext::new_with_gp(entry, adjusted_stack_top, global_pointer);
        user_ctx_ptr.write(user_ctx);

        // 将用户上下文指针存储在 Task 的 context 中
        // 我们使用 CpuContext 的 a[1] 字段暂时存储 UserContext 指针
        let ctx = (*task_ptr).context_mut();
        ctx.a[1] = user_ctx_ptr as u64;
    }

    // 设置地址空间（使用内核页表，单一页表单一页表）
    // 这对于 fork() 正常工作是必需的
    let kernel_ppn = get_kernel_page_table_ppn();
    let addr_space = unsafe { crate::mm::MmStruct::new_user(kernel_ppn) };

    // 为 ELF 段注册 VMA
    use crate::mm::vma::{Vma, VmaFlags};
    use crate::mm::page::VirtAddr as PageVirtAddr;

    // 遍历 PT_LOAD 段，为每个段注册 VMA
    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(ElfError::InvalidProgramHeaders)?;

        if phdr.is_load() {
            let vaddr = phdr.p_vaddr;
            let memsz = phdr.p_memsz as usize;

            // 页对齐
            let aligned_vaddr = vaddr & !(mm::PAGE_SIZE - 1);
            let aligned_end = ((vaddr + memsz as u64 + mm::PAGE_SIZE - 1) & !(mm::PAGE_SIZE - 1));

            // 设置 VMA 标志
            let mut vma_flags = VmaFlags::new();
            vma_flags.insert(VmaFlags::READ);
            if phdr.p_flags & crate::fs::elf::PF_W != 0 {
                vma_flags.insert(VmaFlags::WRITE);
            }
            if phdr.p_flags & crate::fs::elf::PF_X != 0 {
                vma_flags.insert(VmaFlags::EXEC);
            }

            let vma = Vma::new(
                PageVirtAddr::new(aligned_vaddr as usize),
                PageVirtAddr::new(aligned_end as usize),
                vma_flags,
            );

            // 忽略添加错误（某些段可能重叠）
            let _ = addr_space.vma_write().add(vma);
        }
    }

    // 为栈注册 VMA（栈在 virt_end 附近）
    // 注意：如果 ELF 的 PT_LOAD 段已经包含栈区域，这里会失败
    // 但这不是问题，因为 ELF 段已经有正确的权限
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
        // 设置可执行文件路径
        (*task_ptr).set_exe_path(init_path.as_bytes());
        (*task_ptr).set_address_space(Some(alloc::boxed::Box::new(addr_space)));
    }

    Ok(())
}

/// 初始化任务的标准文件描述符
/// 初始化任务的标准文件描述符 (stdin/stdout/stderr)
///
/// 此函数是公开的，可被 fork 等操作复用
pub fn init_std_fds_for_task(fdtable: &crate::fs::FdTable) {
    use crate::fs::char_dev::{CharDev, CharDevType, UART_OPS};
    use crate::fs::{File, FileFlags};
    use alloc::sync::Arc;

    // 创建 UART 字符设备（使用 static 避免悬垂指针）
    static UART_DEV: CharDev = CharDev::new(CharDevType::UartConsole, 0);

    // 创建 stdin (fd=0)
    let stdin = Arc::new(File::new(FileFlags::new(FileFlags::O_RDONLY)));
    stdin.set_ops(&UART_OPS);
    stdin.set_private_data(&UART_DEV as *const CharDev as *mut u8);

    // 创建 stdout (fd=1)
    let stdout = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    stdout.set_ops(&UART_OPS);
    stdout.set_private_data(&UART_DEV as *const CharDev as *mut u8);

    // 创建 stderr (fd=2)
    let stderr = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    stderr.set_ops(&UART_OPS);
    stderr.set_private_data(&UART_DEV as *const CharDev as *mut u8);

    // 安装标准文件描述符
    let _ = fdtable.install_fd(0, stdin);
    let _ = fdtable.install_fd(1, stdout);
    let _ = fdtable.install_fd(2, stderr);
}

/// 停止系统
fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
    }
}
