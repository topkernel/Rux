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
    // 初始化 SMP（多核支持）- 必须最先执行！
    // 只有启动核返回 true，次核会进入空闲循环
    #[cfg(feature = "riscv64")]
    let is_boot_hart = arch::smp::init();

    // 次核进入空闲循环，不执行任何初始化
    #[cfg(feature = "riscv64")]
    if !is_boot_hart {
        loop {
            unsafe {
                core::arch::asm!("wfi", options(nomem, nostack));
            }
        }
    }

    // ========== 以下代码只有启动核执行 ==========

    // 初始化控制台
    console::init();

    // 初始化 trap 处理
    arch::trap::init();

    // 初始化 MMU（必须在堆初始化之前）
    #[cfg(feature = "riscv64")]
    arch::mm::init();

    println!("main: MMU init completed");

    // 初始化堆分配器（MMU 必须先初始化）
    mm::init_heap();
    println!("main: Heap allocator initialized");

    // 初始化命令行参数解析（需要在堆初始化之后）
    #[cfg(feature = "riscv64")]
    {
        let dtb_ptr = arch::riscv64::boot::get_dtb_pointer();
        cmdline::init(dtb_ptr);
        println!("main: Kernel cmdline initialized");
    }

    // 只有启动核才会执行到这里
    #[cfg(feature = "riscv64")]
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

        // 初始化块设备（用于 rootfs）
        {
            println!("main: Initializing block devices...");
            // 先扫描 MMIO 设备（virtio-blk-device）
            let mmio_count = drivers::probe::init_block_devices();
            // 再扫描 PCI 设备（virtio-blk-pci）
            let pci_count = drivers::probe::init_pci_block_devices();
            println!("main: Block devices initialized ({} MMIO, {} PCI)", mmio_count, pci_count);
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

        println!("main: Timer interrupt enabled [OK]");
        println!("main: System ready");

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

        // 测试用户程序执行
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

