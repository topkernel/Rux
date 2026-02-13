//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Init 进程管理模块
//!
//! 对应 Linux 的 init 进程 (PID 1)
//!
//! Init 进程是内核启动后的第一个用户空间进程，负责：
//! - 挂载根文件系统
//! - 启动系统服务
//! - 运行 shell

use crate::arch::riscv64::mm::{self, PageTableEntry};
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
/// 对应 Linux 的 kernel_init() (init/main.c)
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
    println!("init: Starting init process (PID 1)...");

    // 从命令行获取 init 程序路径
    let init_path = cmdline::get_init_program();

    // 尝试从 RootFS 加载 init 程序
    let program_data = load_init_program(&init_path);

    if let Some(data) = program_data {
        // 创建并启动 init 进程
        if let Some(_init_task) = create_and_start_init_process(&data) {
            println!("init: Created init process with PID 1, enqueued");
        } else {
            println!("init: Failed to create init process");
            println!("init: Halting system...");
            halt();
        }
    } else {
        println!("init: Failed to load init program: {}", init_path);
        println!("init: This is expected if using default init=/bin/sh without embedding");
        println!("init: System will continue with idle task only");
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
/// 1. 尝试从 PCI VirtIO 块设备读取（如果可用）
/// 2. 尝试从 ext4 块设备读取（如果 VirtIO 块设备可用）
/// 3. 尝试从 RootFS（内存文件系统）读取
/// 4. 尝试从嵌入式用户程序读取
fn load_init_program(path: &str) -> Option<Vec<u8>> {
    // 1. 首先尝试从 PCI VirtIO 块设备读取
    if let Some(pci_dev) = crate::drivers::virtio::get_pci_device() {
        println!("init: Attempting to load {} from PCI VirtIO block device", path);

        // 使用 PCI VirtIO 设备直接读取 512 字节（第一个扇区）
        // 注意：这是临时测试，真正的 ext4 文件系统需要更复杂的逻辑
        let mut buf = [0u8; 512];
        match crate::drivers::virtio::virtio_pci::read_block_using_configured_queue(pci_dev, 0, &mut buf) {
            Ok(n) => {
                println!("init: Read {} bytes from PCI VirtIO device", n);
                println!("init: First 16 bytes: {:02x?}", &buf[0..16]);
                // 简单检查是否为有效的 ext4 文件系统
                if &buf[0x38..0x3A] == b"\x53\xEF" {  // ext4 magic 检查
                    println!("init: Detected ext4 filesystem on PCI VirtIO device");
                    return Some(buf.to_vec());
                } else {
                    println!("init: PCI VirtIO device does not contain ext4 filesystem");
                }
            }
            Err(e) => {
                println!("init: Failed to read from PCI VirtIO device: {}", e);
            }
        }
    }

    // 2. 尝试从 ext4 块设备读取（真实 rootfs）
    if let Some(virtio_dev) = crate::drivers::virtio::get_device() {
        let disk_ptr = &virtio_dev.disk as *const crate::drivers::blkdev::GenDisk;
        println!("init: Attempting to load {} from ext4 filesystem", path);

        match crate::fs::ext4::read_file(disk_ptr, path) {
            Some(data) => {
                println!("init: Loaded {} from ext4 ({} bytes)", path, data.len());
                return Some(data);
            }
            None => {
                println!("init: File not found in ext4: {}", path);
            }
        }
    } else {
        println!("init: No MMIO VirtIO block device available");
    }

    // 3. 尝试从 RootFS（内存文件系统）读取
    match crate::fs::read_file_from_rootfs(path) {
        Some(data) => {
            println!("init: Loaded {} from RootFS ({} bytes)", path, data.len());
            return Some(data);
        }
        None => {
            println!("init: Failed to load init program: {} from RootFS", path);
            return None;
        }
    }
}

/// 创建并启动 init 进程
///
/// 这个函数会：
/// 1. 创建 init 进程结构
/// 2. 加载 ELF 程序到内存
/// 3. 将 init 进程标记为用户进程
/// 4. 加入调度器运行队列
fn create_and_start_init_process(program_data: &[u8]) -> Option<*mut Task> {
    unsafe {
        let task_ptr = INIT_TASK_STORAGE.as_mut_ptr();

        // 创建 init 任务，PID 固定为 1
        Task::new_task_at(task_ptr, 1, SchedPolicy::Normal);

        // 设置父进程为 0（没有父进程）
        (*task_ptr).set_parent(core::ptr::null_mut());

        // 创建并初始化文件描述符表
        let fdtable = Box::new(FdTable::new());
        (*task_ptr).set_fdtable(Some(fdtable));

        // 初始化标准文件描述符
        if let Some(fdtable) = (*task_ptr).try_fdtable_mut() {
            init_std_fds_for_task(fdtable);
        } else {
            println!("init: Failed to initialize fdtable for init process");
            return None;
        }

        // 加载 ELF 程序到内存并设置用户上下文
        if let Err(e) = load_and_setup_elf(task_ptr, program_data) {
            println!("init: Failed to load ELF: {:?}", e);
            return None;
        }

        // 标记为用户进程（使用 TaskState::Running）
        (*task_ptr).set_state(crate::process::task::TaskState::Running);

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
fn load_and_setup_elf(task_ptr: *mut Task, program_data: &[u8]) -> Result<(), ElfError> {
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

    for i in 0..phdr_count {
        let phdr = unsafe { ehdr.get_program_header(program_data, i) }
            .ok_or(ElfError::InvalidProgramHeaders)?;

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
    let mut loaded = 0;

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

            loaded += 1;
        }
    }

    // 分配用户栈 (64KB)
    const USER_STACK_TOP: u64 = 0x000000003FFF8000;
    const USER_STACK_SIZE: u64 = 0x10000;

    let stack_flags = PageTableEntry::V | PageTableEntry::U |
                     PageTableEntry::R | PageTableEntry::W |
                     PageTableEntry::A | PageTableEntry::D;

    let _user_stack_phys = unsafe {
        mm::alloc_and_map_to_kernel_table(
            USER_STACK_TOP - USER_STACK_SIZE,
            USER_STACK_SIZE,
            stack_flags,
        )
    }.ok_or(ElfError::OutOfMemory)?;

    // 创建用户上下文并存储在静态存储中
    unsafe {
        // 在静态存储上构造 UserContext
        let user_ctx_ptr = INIT_USER_CTX_STORAGE.as_mut_ptr();
        user_ctx_ptr.write(crate::arch::riscv64::context::UserContext::new(entry, USER_STACK_TOP));

        // 将用户上下文指针存储在 Task 的 context 中
        // 我们使用 CpuContext 的 x1 字段暂时存储 UserContext 指针
        let ctx = (*task_ptr).context_mut();
        ctx.x1 = user_ctx_ptr as u64;
    }

    Ok(())
}

/// 初始化任务的标准文件描述符
fn init_std_fds_for_task(fdtable: &crate::fs::FdTable) {
    use crate::fs::char_dev::{CharDev, CharDevType};
    use crate::fs::{File, FileFlags, FileOps};
    use alloc::sync::Arc;

    // 创建 UART 字符设备
    let uart_dev = CharDev::new(CharDevType::UartConsole, 0);

    // 文件操作函数表
    static UART_OPS: FileOps = FileOps {
        read: Some(uart_file_read),
        write: Some(uart_file_write),
        lseek: None,
        close: None,
    };

    // 创建 stdin (fd=0)
    let stdin = Arc::new(File::new(FileFlags::new(FileFlags::O_RDONLY)));
    stdin.set_ops(&UART_OPS);
    stdin.set_private_data(&uart_dev as *const CharDev as *mut u8);

    // 创建 stdout (fd=1)
    let stdout = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    stdout.set_ops(&UART_OPS);
    stdout.set_private_data(&uart_dev as *const CharDev as *mut u8);

    // 创建 stderr (fd=2)
    let stderr = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
    stderr.set_ops(&UART_OPS);
    stderr.set_private_data(&uart_dev as *const CharDev as *mut u8);

    // 安装标准文件描述符
    let _ = fdtable.install_fd(0, stdin);
    let _ = fdtable.install_fd(1, stdout);
    let _ = fdtable.install_fd(2, stderr);
}

fn uart_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    if let Some(priv_data) = unsafe { *file.private_data.get() } {
        let char_dev = unsafe { &*(priv_data as *const CharDev) };
        unsafe { char_dev.read(buf.as_mut_ptr(), buf.len()) }
    } else {
        -9  // EBADF
    }
}

fn uart_file_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    if let Some(priv_data) = unsafe { *file.private_data.get() } {
        let char_dev = unsafe { &*(priv_data as *const CharDev) };
        unsafe { char_dev.write(buf.as_ptr(), buf.len()) }
    } else {
        -9  // EBADF
    }
}

/// 停止系统
fn halt() -> ! {
    println!("init: System halted.");
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
    }
}
