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
use alloc::format;

mod arch;

/// 打印初始化状态信息
///
/// # 参数
/// - `module`: 模块名称
/// - `desc`: 功能描述
/// - `success`: 是否成功
///
/// # 格式
/// 成功: "module:             desc              [ok]"
/// 失败: 红色整行 "module:             desc              [fail]"
#[cfg(feature = "riscv64")]
fn print_status(module: &str, desc: &str, success: bool) {
    // ANSI 颜色代码
    const RED: &[u8] = b"\x1b[31m";
    const RESET: &[u8] = b"\x1b[0m";
    const OK: &[u8] = b"[ok]";
    const FAIL: &[u8] = b"[fail]";

    unsafe {
        use crate::console::putchar;

        // 失败时先打印红色开始代码
        if !success {
            for &b in RED {
                putchar(b);
            }
        }

        // 打印模块名 + 冒号（固定宽度 16 字符，左对齐）
        for b in module.as_bytes() {
            putchar(*b);
        }
        putchar(b':');
        let module_len = module.len() + 1; // +1 for colon
        if module_len < 16 {
            for _ in 0..(16 - module_len) {
                putchar(b' ');
            }
        }

        // 打印描述（固定宽度 32 字符，左对齐，超长截断）
        // 先打印 2 个空格作为列分隔符
        putchar(b' ');
        putchar(b' ');
        let desc_bytes = desc.as_bytes();
        let desc_len = if desc_bytes.len() > 32 { 32 } else { desc_bytes.len() };
        for i in 0..desc_len {
            putchar(desc_bytes[i]);
        }
        if desc_len < 32 {
            for _ in 0..(32 - desc_len) {
                putchar(b' ');
            }
        }
        // 状态列前留 3 个空格对齐
        putchar(b' ');
        putchar(b' ');
        putchar(b' ');

        // 打印状态符号
        if success {
            for &b in OK {
                putchar(b);
            }
        } else {
            for &b in FAIL {
                putchar(b);
            }
        }

        // 失败时打印颜色重置代码
        if !success {
            for &b in RESET {
                putchar(b);
            }
        }

        putchar(b'\n');
    }
}
mod sbi;
mod mm;
mod console;
mod print;
mod drivers;
mod input;
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

    // 初始化控制台（必须最先，其他初始化才能打印）
    console::init();

    // 打印启动横幅
    unsafe {
        use crate::console::putchar;
        // ANSI 颜色
        const CYAN: &[u8] = b"\x1b[36m";
        const GREEN: &[u8] = b"\x1b[32m";
        const BOLD: &[u8] = b"\x1b[1m";
        const RESET: &[u8] = b"\x1b[0m";

        // 打印 ANSI 颜色
        for &b in CYAN { putchar(b); }
        for &b in BOLD { putchar(b); }

        // ASCII Art Logo - RUX (使用 UTF-8 █ 字符)
        // █ = 0xE2 0x96 0x88 (3 bytes in UTF-8)
        const L1: &[u8] = b"\n\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88    \xe2\x96\x88\xe2\x96\x88 \xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88\n";
        const L2: &[u8] = b"\xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88 \xe2\x96\x88\xe2\x96\x88    \xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88 \xe2\x96\x88\xe2\x96\x88\n";
        const L3: &[u8] = b"\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88    \xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\n";
        const L4: &[u8] = b"\xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88 \xe2\x96\x88\xe2\x96\x88    \xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88 \xe2\x96\x88\xe2\x96\x88\n";
        const L5: &[u8] = b"\xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88\xe2\x96\x88  \xe2\x96\x88\xe2\x96\x88   \xe2\x96\x88\xe2\x96\x88\n";

        for &b in L1 { putchar(b); }
        for &b in L2 { putchar(b); }
        for &b in L3 { putchar(b); }
        for &b in L4 { putchar(b); }
        for &b in L5 { putchar(b); }

        // 重置并打印版本
        for &b in RESET { putchar(b); }
        for &b in GREEN { putchar(b); }
        const VERSION: &[u8] = b"  [ RISC-V 64-bit | POSIX Compatible | v";
        for &b in VERSION { putchar(b); }
        let ver = env!("CARGO_PKG_VERSION");
        for b in ver.as_bytes() { putchar(*b); }
        const END: &[u8] = b" ]\n\n";
        for &b in END { putchar(b); }
        for &b in RESET { putchar(b); }
    }

    // 初始化 trap 处理
    arch::trap::init();
    arch::trap::init_syscall();

    // 初始化 MMU（必须在堆初始化之前）
    #[cfg(feature = "riscv64")]
    {
        arch::mm::init();
    }

    // 初始化堆分配器（MMU 必须先初始化）
    mm::init_heap();

    // 初始化 Slab 分配器（在堆之后）
    // 堆结束地址：0x80A0_0000 + KERNEL_HEAP_SIZE
    // 使用 4MB slab 区域以支持更多小对象分配
    let slab_start = 0x80A0_0000 + crate::config::KERNEL_HEAP_SIZE;
    mm::init_slab(slab_start, 4 * 1024 * 1024);  // 4MB for slab

    // ========== 堆已初始化，以下可以使用 format! ==========

    // 打印启动提示
    unsafe {
        use crate::console::putchar;
        const YELLOW: &[u8] = b"\x1b[33m";
        const RESET: &[u8] = b"\x1b[0m";
        for &b in YELLOW { putchar(b); }
        const MSG: &[u8] = b"Kernel starting...\n\n";
        for &b in MSG { putchar(b); }
        for &b in RESET { putchar(b); }
    }

    // 打印表头
    unsafe {
        use crate::console::putchar;
        const CYAN: &[u8] = b"\x1b[36m";
        const RESET: &[u8] = b"\x1b[0m";
        for &b in CYAN { putchar(b); }
        // Module(16) + 2 spaces + Description(32) + 3 spaces + Status
        const HEADER: &[u8] = b"Module            Description                        Status\n";
        for &b in HEADER { putchar(b); }
        const DIVIDER: &[u8] = b"----------------  --------------------------------   --------\n";
        for &b in DIVIDER { putchar(b); }
        for &b in RESET { putchar(b); }
    }

    print_status("console", "UART ns16550a driver", true);

    // 初始化 SMP 多核支持信息
    #[cfg(feature = "riscv64")]
    {
        let cpu_count = arch::smp::num_started_cpus();
        if cpu_count > 1 {
            print_status("smp", &format!("{} CPU(s) online", cpu_count), true);
        }
    }

    print_status("trap", "stvec handler installed", true);
    print_status("trap", "ecall syscall handler", true);
    print_status("mm", "Sv39 3-level page table", true);
    print_status("mm", "satp CSR configured", true);
    print_status("mm", "buddy allocator order 0-12", true);

    // 使用配置值显示堆大小
    let heap_mb = crate::config::KERNEL_HEAP_SIZE / (1024 * 1024);
    let heap_info = format!("heap region {}MB @ 0x80A00000", heap_mb);
    print_status("mm", &heap_info, true);
    print_status("mm", "slab allocator 4MB", true);

    // 初始化命令行参数解析（需要在堆初始化之后）
    #[cfg(feature = "riscv64")]
    {
        let dtb_ptr = arch::riscv64::boot::get_dtb_pointer();
        cmdline::init(dtb_ptr);
        print_status("boot", "FDT/DTB parsed", true);
        if let Some(cmdline) = cmdline::get_cmdline() {
            if !cmdline.is_empty() {
                // 截断过长的 cmdline
                let display = if cmdline.len() > 22 {
                    format!("cmd: {}...", &cmdline[..22])
                } else {
                    format!("cmd: {}", cmdline)
                };
                print_status("boot", &display, true);
            }
        }
    }

    // 只有启动核才会执行到这里
    #[cfg(feature = "riscv64")]
    if is_boot_hart {
        // 初始化用户物理页分配器
        #[cfg(feature = "riscv64")]
        {
            arch::mm::init_user_phys_allocator(0x80000000, 0x8000000); // 128MB 内存
            print_status("mm", "user frame allocator 64MB", true);

            // 初始化页描述符（struct Page）
            // 物理内存从 0x80000000 开始，初始化 64MB 的页描述符
            let start_pfn = 0x80000000 / mm::PAGE_SIZE;
            let nr_pages = mm::page_desc::MAX_PAGES;
            mm::page::init_page_descriptors(start_pfn, nr_pages);
            print_status("mm", &format!("{} page descriptors", nr_pages), true);
        }

        // 初始化 PLIC（中断控制器）
        #[cfg(feature = "riscv64")]
        {
            drivers::intc::init();
            print_status("intc", "PLIC @ 0x0C000000", true);
            print_status("intc", "external IRQ routing", true);
        }

        // 初始化 IPI（核间中断）
        #[cfg(feature = "riscv64")]
        {
            arch::ipi::init();
            print_status("ipi", "SSIP software IRQ", true);
        }

        // 初始化文件系统
        {
            // 初始化 block I/O 层
            fs::bio::init();
            print_status("bio", "buffer cache layer", true);

            // 初始化 ext4 文件系统
            fs::ext4::init();
            print_status("fs", "ext4 driver loaded", true);

            // 初始化 RootFS
            let rootfs_result = fs::rootfs::init_rootfs();
            print_status("fs", "ramfs mounted /", rootfs_result.is_ok());

            // 初始化 ProcFS 并挂载到 /proc
            let procfs_result = fs::procfs::init_procfs();
            print_status("fs", "procfs initialized", procfs_result.is_ok());
            if procfs_result.is_ok() {
                let mount_result = fs::procfs::mount_procfs();
                print_status("fs", "procfs mounted /proc", mount_result.is_ok());
            }
        }

        // 初始化块设备（用于 rootfs）
        {
            // 先扫描 MMIO 设备（virtio-blk-device）
            let mmio_count = drivers::probe::init_block_devices();
            if mmio_count > 0 {
                print_status("driver", &format!("virtio-blk MMIO x{}", mmio_count), true);
            }
            // 再扫描 PCI 设备（virtio-blk-pci）
            let pci_count = drivers::probe::init_pci_block_devices();
            if pci_count > 0 {
                print_status("driver", &format!("virtio-blk PCI x{}", pci_count), true);
                print_status("driver", "GenDisk registered", true);
            }
        }

        // 初始化网络设备
        {
            let device_count = drivers::probe::init_network_devices();
            if device_count > 0 {
                print_status("driver", &format!("virtio-net x{}", device_count), true);
            }
        }

        // 初始化进程调度器
        #[cfg(feature = "riscv64")]
        {
            sched::init();
            print_status("sched", "CFS scheduler v1", true);
            print_status("sched", "runqueue per-CPU", true);
            print_status("sched", "PID allocator init", true);
            print_status("sched", "idle task (PID 0)", true);

            // 初始化 Per-CPU Pages（在调度器初始化之后）
            let boot_cpu = arch::cpu_id() as usize;
            mm::init_percpu_pages(boot_cpu);
            print_status("mm", &format!("PCP cpu{} hotpage", boot_cpu), true);
        }

        // 使能外部中断
        #[cfg(feature = "riscv64")]
        {
            arch::trap::enable_external_interrupt();
            print_status("trap", "sie.SEIE enabled", true);
        }

        // ========== 图形系统初始化 (VirtIO-GPU) ==========
        #[cfg(feature = "riscv64")]
        {
            // 探测 VirtIO-GPU 设备
            if let Some(mut gpu_device) = drivers::gpu::probe_virtio_gpu() {
                print_status("driver", "virtio-gpu probed", true);
                // 初始化帧缓冲区
                if let Some(fb_info) = gpu_device.init_framebuffer() {
                    print_status("gpu", &format!("{}x{} 32bpp framebuffer", fb_info.width, fb_info.height), true);
                    // 保存 framebuffer 信息供用户态 mmap 使用
                    drivers::gpu::set_framebuffer_info(*fb_info);
                } else {
                    print_status("gpu", "framebuffer init failed", false);
                }
            }
        }

        // ========== 初始化输入系统 ==========
        #[cfg(feature = "riscv64")]
        {
            input::init();
            print_status("driver", "PS/2 keyboard", true);
            print_status("driver", "PS/2 mouse", true);
        }

        println!();

        // 使能 timer interrupt
        // 注意：暂时禁用以调试 ext4 文件读取问题
        // arch::trap::enable_timer_interrupt();
        // drivers::timer::set_next_trigger();

        // 运行所有单元测试（禁用中断以避免干扰）
        #[cfg(feature = "unit-test")]
        {
            arch::trap::disable_timer_interrupt();
            tests::run_all_tests();
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

            // 暂时禁用定时器中断以调试用户模式执行
            // arch::trap::enable_timer_interrupt();
            // drivers::timer::set_next_trigger();
        }

        // ========== 启动 init 进程 ==========
        #[cfg(feature = "riscv64")]
        {
            // 获取 init 路径
            let init_path = cmdline::get_init_program();
            print_status("init", &format!("loading {}", init_path), true);
            init::init();
            print_status("init", "ELF loaded to user space", true);
            print_status("init", "init task (PID 1) enqueued", true);
        }

        println!();

        // ========== 进入调度器主循环 ==========

        // 启动核进入空闲循环，参与任务调度
        #[cfg(feature = "riscv64")]
        {
            sched::cpu_idle_loop();
        }

        // 如果没有调度器，简单的 WFI 循环
        #[cfg(not(feature = "riscv64"))]
        {
            loop {
                unsafe {
                    core::arch::asm!("wfi", options(nomem, nostack));
                }
            }
        }
    } else {
        // 次核：初始化调度器并进入空闲循环

        // 初始化进程调度器（次核也需要）
        #[cfg(feature = "riscv64")]
        {
            sched::init();
        }

        // 进入空闲循环，参与任务调度
        #[cfg(feature = "riscv64")]
        {
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

