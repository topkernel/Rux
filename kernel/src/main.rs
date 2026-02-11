//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
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
mod sync;
mod errno;
mod net;
mod cmdline;
mod init;

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

    // 初始化命令行参数解析（在 boot 核执行）
    #[cfg(feature = "riscv64")]
    {
        let dtb_ptr = arch::riscv64::boot::get_dtb_pointer();
        cmdline::init(dtb_ptr);
        println!("main: Kernel cmdline initialized");
    }

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
        // #[cfg(feature = "riscv64")]
        // {
        //     println!("main: Initializing PLIC...");
        //     drivers::intc::init();
        //     println!("main: PLIC initialized");
        // }

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

            // 初始化 block I/O 层
            println!("main:   Initializing block I/O...");
            fs::bio::init();
            println!("main:   Block I/O initialized");

            // 初始化 ext4 文件系统
            println!("main:   Initializing ext4...");
            fs::ext4::init();
            println!("main:   ext4 initialized");

            // 初始化 RootFS
            println!("main:   Initializing RootFS...");
            fs::rootfs::init_rootfs().expect("Failed to initialize RootFS");
            println!("main:   RootFS initialized");

            println!("main: File system initialized");
        }

        // 初始化网络设备
        {
            println!("main: Initializing network devices...");
            let _device_count = drivers::probe::init_network_devices();
            println!("main: Network devices initialized");
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

        // DEBUG: 直接输出，不使用 println!
        unsafe {
            use crate::console::putchar;
            const MSG: &[u8] = b"[OK] Timer interrupt enabled, system ready.\n";
            for &b in MSG {
                putchar(b);
            }
        }
        // println!("[OK] Timer interrupt enabled, system ready.");
        // println!("[OK] Timer interrupt disabled for debugging.");

        // 运行所有单元测试（禁用中断以避免干扰）
        #[cfg(feature = "unit-test")]
        {
            println!("main: Disabling interrupts for unit tests...");
            arch::trap::disable_timer_interrupt();
            tests::run_all_tests();
            println!("main: Re-enabling interrupts after unit tests...");
            arch::trap::enable_timer_interrupt();
            drivers::timer::set_next_trigger();
        }

        // 测试用户程序执行（Phase 11.5）
        #[cfg(feature = "riscv64")]
        {
            // 禁用定时器中断以避免干扰用户程序加载
            arch::trap::disable_timer_interrupt();

            // 用户程序执行测试已禁用
            // println!("test: ===== Starting User Program Execution Test =====");
            // test_shell_execution();
            // println!("test: ===== User Program Execution Test Completed =====");

            // 重新启用定时器中断
            arch::trap::enable_timer_interrupt();
            drivers::timer::set_next_trigger();
        }

        // ========== 启动 init 进程 ==========
        println!("main: ===== Starting Init Process =====");
        #[cfg(feature = "riscv64")]
        {
            init::init();
        }

        // ========== 进入调度器主循环 ==========
        println!("main: Entering scheduler main loop...");

        // 启动核进入空闲循环，参与任务调度
        // 对应 Linux 的 cpu_startup_entry() (kernel/sched/idle.c)
        #[cfg(feature = "riscv64")]
        {
            sched::cpu_idle_loop();
        }

        // 如果没有调度器，简单的 WFI 循环
        #[cfg(not(feature = "riscv64"))]
        {
            println!("main: No scheduler, entering WFI loop");
            loop {
                unsafe {
                    core::arch::asm!("wfi", options(nomem, nostack));
                }
            }
        }
    } else {
        // 次核：初始化调度器并进入空闲循环
        println!("main: Secondary hart - initializing scheduler...");

        // 初始化进程调度器（次核也需要）
        #[cfg(feature = "riscv64")]
        {
            sched::init();
            println!("main: Secondary hart - scheduler initialized");
        }

        // 进入空闲循环，参与任务调度
        #[cfg(feature = "riscv64")]
        {
            println!("main: Secondary hart - entering idle loop");
            sched::cpu_idle_loop();
        }

        // 如果没有调度器，简单的 WFI 循环
        #[cfg(not(feature = "riscv64"))]
        loop {
            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
        }
    }
}

// Panic handler
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"PANIC!\n";
        for &b in MSG {
            putchar(b);
        }

        // Try to print the location if available
        if let Some(loc) = info.location() {
            const MSG_FILE: &[u8] = b"  Location: ";
            for &b in MSG_FILE {
                putchar(b);
            }
            for b in loc.file().as_bytes() {
                putchar(*b);
            }
            putchar(b':');
            let line = loc.line();
            // Simple line number printing (0-999)
            if line < 10 {
                putchar(b'0' + line as u8);
            } else if line < 100 {
                putchar(b'0' + (line / 10) as u8);
                putchar(b'0' + (line % 10) as u8);
            } else if line < 1000 {
                putchar(b'0' + (line / 100) as u8);
                putchar(b'0' + ((line / 10) % 10) as u8);
                putchar(b'0' + (line % 10) as u8);
            }
            putchar(b'\n');
        }
    }
    loop {}
}

// 测试：执行 shell 用户程序
#[cfg(feature = "riscv64")]
fn test_shell_execution() {
    test_hello_world_execution();
}

// 测试：执行 hello_world 用户程序
#[cfg(feature = "riscv64")]
fn test_hello_world_execution() {
    use crate::arch::riscv64::mm::{self, PageTableEntry};
    use crate::fs::elf::ElfLoader;
    use core::slice;

    println!("test: ===== Starting Hello World User Program Execution =====");
    println!("test: Step 1 - Getting hello_world ELF data...");

    unsafe {
        // 获取 hello_world ELF 数据
        let program_data = crate::embedded_user_programs::HELLO_WORLD_ELF;
        println!("test:    hello_world ELF data ptr = {:#x}", program_data.as_ptr() as usize);
        println!("test:    hello_world ELF size = {} bytes", program_data.len());

        // 验证 ELF 格式
        println!("test: Step 2 - Validating ELF format...");
        if let Err(e) = ElfLoader::validate(program_data) {
            println!("test:    ERROR - Invalid ELF format: {:?}", e);
            return;
        }
        println!("test:    ELF format validated successfully");

        // 使用Linux单一页表方式（不创建单独用户页表）
        println!("test: Step 3 - Using Linux-style single page table approach...");
        let kernel_ppn = mm::get_kernel_page_table_ppn();
        println!("test:    Using kernel page table, PPN = {:#x}", kernel_ppn);

        // 解析 ELF
        println!("test: Step 4 - Parsing ELF...");
        let entry = match ElfLoader::get_entry(program_data) {
            Ok(addr) => {
                println!("test:    Entry point = {:#x}", addr);
                addr
            }
            Err(e) => {
                println!("test:    ERROR - Failed to get entry point: {:?}", e);
                return;
            }
        };

        let phdr_count = match ElfLoader::get_program_headers(program_data) {
            Ok(count) => {
                println!("test:    Program headers count = {}", count);
                count
            }
            Err(e) => {
                println!("test:    ERROR - Failed to get program headers: {:?}", e);
                return;
            }
        };

        // 获取 ELF 头用于读取程序头
        let ehdr = match unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(program_data) } {
            Some(e) => e,
            None => {
                println!("test:    ERROR - Failed to get ELF header");
                return;
            }
        };

        // 第一遍：找到虚拟地址范围
        println!("test: Step 5 - Calculating virtual address range...");
        let mut min_vaddr: u64 = u64::MAX;
        let mut max_vaddr: u64 = 0;

        for i in 0..phdr_count {
            let phdr = match unsafe { ehdr.get_program_header(program_data, i) } {
                Some(p) => p,
                None => {
                    println!("test:    ERROR - Failed to get phdr {}", i);
                    return;
                }
            };

            if phdr.is_load() {
                let virt_addr = phdr.p_vaddr;
                let mem_size = phdr.p_memsz;

                println!("test:    PHDR {}: vaddr={:#x}, mem_size={:#x}", i, virt_addr, mem_size);

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

        println!("test:    Virtual range: {:#x} - {:#x} ({} bytes)", virt_start, virt_end, total_size);

        // 一次性分配并映射整个用户内存范围（到内核页表）
        println!("test: Step 6 - Allocating and mapping user memory to kernel page table...");
        let flags = PageTableEntry::V | PageTableEntry::U |
                   PageTableEntry::R | PageTableEntry::W |
                   PageTableEntry::X | PageTableEntry::A |
                   PageTableEntry::D;

        let phys_base = match mm::alloc_and_map_to_kernel_table(
            virt_start,
            total_size,
            flags,
        ) {
            Some(addr) => {
                println!("test:    User memory allocated at phys={:#x}", addr);
                addr
            }
            None => {
                println!("test:    ERROR - Failed to allocate user memory");
                return;
            }
        };

        // 第二遍：加载每个段的数据
        println!("test: Step 7 - Loading ELF segments...");
        let mut loaded = 0;

        for i in 0..phdr_count {
            let phdr = match unsafe { ehdr.get_program_header(program_data, i) } {
                Some(p) => p,
                None => {
                    println!("test:    ERROR - Failed to get phdr {}", i);
                    return;
                }
            };

            if phdr.is_load() {
                let virt_addr = phdr.p_vaddr;
                let file_size = phdr.p_filesz;
                let mem_size = phdr.p_memsz;
                let offset = phdr.p_offset as usize;

                println!("test:    Loading PHDR {}: vaddr={:#x}, file_size={:#x}, mem_size={:#x}",
                         i, virt_addr, file_size, mem_size);

                // 计算物理地址
                let virt_offset = virt_addr - virt_start;
                let phys_addr = (phys_base + virt_offset) as usize;

                println!("test:      virt_offset={:#x}, phys_addr={:#x}", virt_offset, phys_addr);

                // 复制 ELF 数据到物理内存
                if file_size > 0 {
                    let src = &program_data[offset..offset + file_size as usize];
                    let dst = slice::from_raw_parts_mut(phys_addr as *mut u8, file_size as usize);
                    dst.copy_from_slice(src);
                    println!("test:      Copied {} bytes from offset {:#x} to phys {:#x}", file_size, offset, phys_addr);
                }

                // 清零 BSS
                if mem_size > file_size {
                    let bss_start = phys_addr + file_size as usize;
                    let bss_size = (mem_size - file_size) as usize;
                    let bss_dst = slice::from_raw_parts_mut(bss_start as *mut u8, bss_size);
                    bss_dst.fill(0);
                    println!("test:      Zeroed BSS: {} bytes", bss_size);
                }

                loaded += 1;
            }
        }

        println!("test:    Loaded {} segments successfully", loaded);

        // 分配用户栈 (64KB) 到内核页表
        println!("test: Step 8 - Allocating user stack to kernel page table...");
        const USER_STACK_TOP: u64 = 0x000000003FFF8000;
        const USER_STACK_SIZE: u64 = 0x10000;

        let stack_flags = PageTableEntry::V | PageTableEntry::U |
                         PageTableEntry::R | PageTableEntry::W |
                         PageTableEntry::A | PageTableEntry::D;

        let _user_stack_phys = match mm::alloc_and_map_to_kernel_table(
            USER_STACK_TOP - USER_STACK_SIZE,
            USER_STACK_SIZE,
            stack_flags,
        ) {
            Some(addr) => {
                println!("test:    User stack allocated at phys={:#x}", addr);
                addr
            }
            None => {
                println!("test:    ERROR - Failed to allocate user stack");
                return;
            }
        };

        // 刷新指令缓存，确保用户程序指令可见
        // 根据 RISC-V 规范，需要先 fence 确保 write 完成，然后 fence.i 刷新指令缓存
        println!("test: Step 8.5 - Flushing caches...");
        unsafe {
            // fence 确保 store 完成
            // core::arch::asm!("fence", options(nomem, nostack));
            // fence.i 刷新指令缓存
            // core::arch::asm!("fence.i", options(nomem, nostack));
        }
        println!("test:    Caches flushed (fence disabled for debugging)");

        println!("test: Step 9 - Switching to user mode (Linux style)...");
        println!("test:    Entry point = {:#x}", entry);
        println!("test:    User stack top = {:#x}", USER_STACK_TOP);
        println!("test:    Using Linux single page table approach");

        // 使用Linux风格切换到用户模式（单一页表，不切换satp）
        println!("test:    Using Linux-style switch (single page table, no satp change)...");
        println!();

        // 验证用户程序入口点的指令
        println!("test:    Verifying user program entry point...");
        unsafe {
            let entry_virt = entry as usize;
            let entry_phys = (phys_base as usize + (entry_virt - virt_start as usize)) as usize;
            println!("test:      Entry virt = {:#x}", entry_virt);
            println!("test:      Entry phys = {:#x}", entry_phys);

            // 读取入口点的指令
            let entry_insn = core::ptr::read_volatile(entry_phys as *const u32);
            println!("test:      Entry instruction = {:#010x}", entry_insn);

            // 解析指令
            if entry_insn == 0x00000297 {
                println!("test:      This is auipc t0, 0 (likely correct)");
            } else if entry_insn == 0x00000013 {
                println!("test:      This is nop (might be a simple test program)");
            } else {
                println!("test:      Unknown instruction");
            }
        }

        println!("test:    About to switch to user mode...");
        println!();
        println!("=======================================================================");
        println!("test: USER PROGRAM STARTING");
        println!("test:   [User Mode] hello_world program will now execute");
        println!("=======================================================================");
        println!();

        unsafe {
            crate::arch::riscv64::mm::switch_to_user_linux(
                entry,
                USER_STACK_TOP,
            );
        }

        // 不应该到达这里
        println!();
        println!("=======================================================================");
        println!("test: UNEXPECTED - Returned from test_sret_simple!");
        println!("=======================================================================");
    }
}

