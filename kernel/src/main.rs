#![no_std]
#![no_main]
#![feature(lang_items, alloc_error_handler, linkage)]

extern crate log;
extern crate alloc;

use core::panic::PanicInfo;

mod arch;
mod sbi;
mod mm;
mod console;
mod print;
mod drivers;
mod config;
mod process;
mod sched;
mod fs;
mod signal;
mod collection;
mod sync;
mod errno;

#[cfg(feature = "unit-test")]
mod tests;

// 嵌入的用户程序
#[cfg(feature = "riscv64")]
mod embedded_user_programs;

// Allocation error handler for no_std
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("Allocation error: {:?}", layout);
}

// 包含平台特定的汇编代码
#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/boot/boot.S"));

#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/trap.S"));

// RISC-V kernel main function
#[cfg(feature = "riscv64")]
#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    // 初始化控制台（所有 CPU 都需要）
    console::init();

    // 初始化堆分配器（所有 CPU 都需要）
    mm::init_heap();

    // 初始化 trap 处理（所有 CPU 都需要）
    arch::trap::init();

    // 初始化 MMU（所有 CPU 都需要，但只有启动核执行完整初始化）
    #[cfg(feature = "riscv64")]
    arch::mm::init();

    println!("main: MMU init completed");

    // 初始化 SMP（多核支持）
    // 只有启动核返回，次核会进入空闲循环
    #[cfg(feature = "riscv64")]
    let is_boot_hart = arch::smp::init();

    println!("main: SMP init completed, is_boot_hart={}", is_boot_hart);

    // 只有启动核才会执行到这里
    if is_boot_hart {
        println!("Rux OS v{} - RISC-V 64-bit", env!("CARGO_PKG_VERSION"));

        // 初始化用户物理页分配器
        #[cfg(feature = "riscv64")]
        {
            println!("main: Initializing user physical allocator...");
            arch::mm::init_user_phys_allocator(0x80000000, 0x8000000); // 128MB 内存
            println!("main: User physical allocator initialized");
        }

        // 初始化 PLIC（中断控制器）
        #[cfg(feature = "riscv64")]
        {
            println!("main: Initializing PLIC...");
            drivers::intc::init();
            println!("main: PLIC initialized");
        }

        // 初始化 IPI（核间中断）
        #[cfg(feature = "riscv64")]
        {
            println!("main: Initializing IPI...");
            arch::ipi::init();
            println!("main: IPI initialized");
        }

        // 初始化文件系统
        {
            println!("main: Initializing file system...");
            fs::rootfs::init_rootfs().expect("Failed to initialize RootFS");
            println!("main: RootFS initialized");
        }

        // 初始化进程调度器
        #[cfg(feature = "riscv64")]
        {
            println!("main: Initializing process scheduler...");
            sched::init();
            println!("main: Process scheduler initialized");
        }

        // 使能外部中断
        #[cfg(feature = "riscv64")]
        arch::trap::enable_external_interrupt();

        // 使能 timer interrupt
        println!("main: Enabling timer interrupt...");
        arch::trap::enable_timer_interrupt();

        // 设置第一次定时器中断
        drivers::timer::set_next_trigger();

        println!("[OK] Timer interrupt enabled, system ready.");

        // 测试 file_open() 功能
        #[cfg(feature = "unit-test")]
        tests::file_open::test_file_open();

        // 测试 ListHead 双向链表功能
        #[cfg(feature = "unit-test")]
        tests::listhead::test_listhead();

        // 测试 Path 路径解析功能
        #[cfg(feature = "unit-test")]
        tests::path::test_path();

        // 测试 FileFlags 文件标志
        #[cfg(feature = "unit-test")]
        tests::file_flags::test_file_flags();

        // 测试文件描述符管理
        #[cfg(feature = "unit-test")]
        tests::fdtable::test_fdtable();

        // 测试页分配器
        #[cfg(feature = "unit-test")]
        tests::page_allocator::test_page_allocator();

        // 测试堆分配器
        #[cfg(feature = "unit-test")]
        tests::heap_allocator::test_heap_allocator();

        // 测试进程调度器
        #[cfg(feature = "unit-test")]
        tests::scheduler::test_scheduler();

        // 测试信号处理
        #[cfg(feature = "unit-test")]
        tests::signal::test_signal();

        // 测试多核启动（仅在 SMP 模式下）
        #[cfg(feature = "unit-test")]
        tests::smp::test_smp();

        // 测试进程树管理功能
        #[cfg(feature = "unit-test")]
        tests::process_tree::test_process_tree();

        // 测试 fork 系统调用
        #[cfg(feature = "unit-test")]
        tests::fork::test_fork();

        // 测试 execve 系统调用
        #[cfg(feature = "unit-test")]
        tests::execve::test_execve();

        // 测试 wait4 系统调用
        #[cfg(feature = "unit-test")]
        tests::wait4::test_wait4();

        println!("test: Entering main loop...");
    }

    // 主循环：等待中断
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"PANIC!\n";
        for &b in MSG {
            putchar(b);
        }
    }
    loop {}
}

// 测试：执行 shell 用户程序
#[cfg(feature = "riscv64")]
fn test_shell_execution() {
    use crate::arch::riscv64::mm::{self, PageTableEntry};
    use crate::fs::elf::ElfLoader;
    use core::slice;

    println!("test: Starting shell user program execution...");

    unsafe {
        // 获取 shell ELF 数据
        let shell_data = crate::embedded_user_programs::SHELL_ELF;

        // 验证 ELF 格式
        if let Err(e) = ElfLoader::validate(shell_data) {
            println!("test: Invalid ELF format: {:?}", e);
            return;
        }

        // 创建用户地址空间
        let user_root_ppn = match mm::create_user_address_space() {
            Some(ppn) => ppn,
            None => {
                println!("test: Failed to create user address space");
                return;
            }
        };

        println!("test: User address space created, root PPN = {:#x}", user_root_ppn);

        // 解析 ELF
        let entry = match ElfLoader::get_entry(shell_data) {
            Ok(addr) => addr,
            Err(e) => {
                println!("test: Failed to get entry point: {:?}", e);
                return;
            }
        };

        let phdr_count = match ElfLoader::get_program_headers(shell_data) {
            Ok(count) => count,
            Err(e) => {
                println!("test: Failed to get program headers: {:?}", e);
                return;
            }
        };

        // 获取 ELF 头用于读取程序头
        let ehdr = match unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(shell_data) } {
            Some(e) => e,
            None => {
                println!("test: Failed to get ELF header");
                return;
            }
        };

        // 第一遍：找到虚拟地址范围
        let mut min_vaddr: u64 = u64::MAX;
        let mut max_vaddr: u64 = 0;

        for i in 0..phdr_count {
            let phdr = match unsafe { ehdr.get_program_header(shell_data, i) } {
                Some(p) => p,
                None => {
                    println!("test: Failed to get phdr {}", i);
                    return;
                }
            };

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

        println!("test: Virtual range: {:#x} - {:#x} ({} bytes)", virt_start, virt_end, total_size);

        // 一次性分配并映射整个用户内存范围
        let flags = PageTableEntry::V | PageTableEntry::U |
                   PageTableEntry::R | PageTableEntry::W |
                   PageTableEntry::X | PageTableEntry::A |
                   PageTableEntry::D;

        let phys_base = match mm::alloc_and_map_user_memory(
            user_root_ppn,
            virt_start,
            total_size,
            flags,
        ) {
            Some(addr) => addr,
            None => {
                println!("test: Failed to allocate user memory");
                return;
            }
        };

        println!("test: User memory allocated at phys={:#x}", phys_base);

        // 第二遍：加载每个段的数据
        let mut loaded = 0;

        for i in 0..phdr_count {
            let phdr = match unsafe { ehdr.get_program_header(shell_data, i) } {
                Some(p) => p,
                None => {
                    println!("test: Failed to get phdr {}", i);
                    return;
                }
            };

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
                    let src = &shell_data[offset..offset + file_size as usize];
                    let dst = slice::from_raw_parts_mut(phys_addr as *mut u8, file_size as usize);
                    dst.copy_from_slice(src);
                }

                // 清零 BSS
                if mem_size > file_size {
                    let bss_start = phys_addr + file_size as usize;
                    let bss_size = (mem_size - file_size) as usize;
                    let bss_dst = slice::from_raw_parts_mut(bss_start as *mut u8, bss_size);
                    bss_dst.fill(0);
                }

                loaded += 1;
            }
        }

        println!("test: Loaded {} segments", loaded);

        // 分配用户栈 (64KB)
        const USER_STACK_TOP: u64 = 0x000000003FFF8000;
        const USER_STACK_SIZE: u64 = 0x10000;

        let stack_flags = PageTableEntry::V | PageTableEntry::U |
                         PageTableEntry::R | PageTableEntry::W |
                         PageTableEntry::A | PageTableEntry::D;

        let _user_stack_phys = match mm::alloc_and_map_user_memory(
            user_root_ppn,
            USER_STACK_TOP - USER_STACK_SIZE,
            USER_STACK_SIZE,
            stack_flags,
        ) {
            Some(addr) => addr,
            None => {
                println!("test: Failed to allocate user stack");
                return;
            }
        };

        println!("test: User stack ready, entry={:#x}", entry);

        // 切换到用户模式执行
        mm::switch_to_user(user_root_ppn, entry, USER_STACK_TOP);
    }
}

