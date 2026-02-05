#![no_std]
#![no_main]
#![feature(lang_items, global_asm, naked_functions, alloc_error_handler, linkage)]

#[macro_use]
extern crate log;
extern crate alloc;

use core::panic::PanicInfo;
use core::arch::asm;

mod arch;
mod mm;
mod console;
mod print;
mod drivers;
mod config;
mod process;
mod fs;
mod signal;
mod collection;

// Allocation error handler for no_std
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("Allocation error: {:?}", layout);
}

// 包含平台特定的汇编代码
#[cfg(feature = "aarch64")]
use core::arch::global_asm;

#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/boot/boot.S"));

#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/trap.S"));

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 禁用中断直到中断控制器设置完成
    unsafe {
        asm!("msr daifset, #2", options(nomem, nostack));
    }

    // 初始化控制台（UART）
    console::init();

    println!("{} Kernel v{} starting...",
             crate::config::KERNEL_NAME,
             crate::config::KERNEL_VERSION);
    println!("Target platform: {}", crate::config::TARGET_PLATFORM);

    debug_println!("Initializing architecture...");
    arch::arch_init();

    debug_println!("Before trap init");
    debug_println!("Initializing trap handling...");
    arch::trap::init();
    debug_println!("After trap init");

    debug_println!("Initializing system calls...");
    arch::trap::init_syscall();

    debug_println!("Initializing heap...");
    crate::mm::init_heap();

    // 首先测试直接调用分配器
    debug_println!("Testing direct allocator call...");
    use core::alloc::{GlobalAlloc, Layout};
    unsafe {
        let layout = Layout::new::<u32>();
        let ptr = GlobalAlloc::alloc(&crate::mm::allocator::HEAP_ALLOCATOR, layout);
        if !ptr.is_null() {
            *(ptr as *mut u32) = 42;
            debug_println!("Direct alloc works!");
        } else {
            debug_println!("Direct alloc failed!");
        }
    }

    debug_println!("Testing SimpleVec...");
    use crate::collection::SimpleVec;
    match SimpleVec::with_capacity(10) {
        Some(mut test_vec) => {
            if test_vec.push(42) {
                debug_println!("SimpleVec::push works!");
                if let Some(val) = test_vec.get(0) {
                    // 使用多个 debug_println 调用
                    debug_println!("SimpleVec::get works, value = ");
                    unsafe {
                        use crate::console::putchar;
                        const DIGITS: &[u8] = b"0123456789";
                        let mut n = *val;
                        let mut buf = [0u8; 20];
                        let mut i = 19;
                        if n == 0 {
                            buf[i] = b'0';
                            i -= 1;
                        } else {
                            while n > 0 {
                                buf[i] = DIGITS[(n % 10) as usize];
                                n /= 10;
                                if i > 0 { i -= 1; }
                            }
                        }
                        for &b in &buf[i..] {
                            putchar(b);
                        }
                        const NEWLINE: &[u8] = b"\n";
                        for &b in NEWLINE {
                            putchar(b);
                        }
                    }
                } else {
                    debug_println!("SimpleVec::get failed!");
                }
            } else {
                debug_println!("SimpleVec::push failed!");
            }
        }
        None => {
            debug_println!("SimpleVec::with_capacity failed!");
        }
    }

    // 测试 SimpleBox
    debug_println!("Testing SimpleBox...");
    use crate::collection::SimpleBox;
    match SimpleBox::new(42) {
        Some(_box_val) => {
            debug_println!("SimpleBox works!");
        }
        None => {
            debug_println!("SimpleBox::new failed!");
        }
    }

    // 测试 SimpleString
    debug_println!("Testing SimpleString...");
    use crate::collection::SimpleString;
    match SimpleString::from_str("Hello Rux") {
        Some(_s) => {
            debug_println!("SimpleString works!");
        }
        None => {
            debug_println!("SimpleString::from_str failed!");
        }
    }

    // 测试 SimpleArc
    debug_println!("Testing SimpleArc...");
    use crate::collection::SimpleArc;
    match SimpleArc::new(12345) {
        Some(arc) => {
            // 测试克隆
            let _arc2 = arc.clone();
            debug_println!("SimpleArc works!");
        }
        None => {
            debug_println!("SimpleArc::new failed!");
        }
    }

    // GIC 初始化
    debug_println!("Initializing GIC...");
    drivers::intc::init();
    // 注意：IRQ 仍然禁用，将在稍后启用
    debug_println!("GIC initialized - IRQ still disabled");

    // Timer 初始化（在 GIC 之后，IRQ 使能之前）
    debug_println!("Initializing timer...");
    drivers::timer::init();

    // 在 GIC 中使能物理定时器中断（IRQ 30）
    debug_println!("Enabling timer IRQ in GIC...");
    drivers::intc::enable_irq(30);  // ARMv8 物理定时器 IRQ

    debug_println!("Initializing scheduler...");
    process::sched::init();

    debug_println!("Initializing VFS...");
    crate::fs::vfs_init();

    // SMP 初始化（使用 PSCI）
    debug_println!("Initializing SMP...");
    crate::arch::aarch64::smp::SmpData::init(2);

    // 尝试 PSCI 启动次核
    debug_println!("Attempting PSCI CPU_ON...");
    crate::arch::aarch64::smp::boot_secondary_cpus();

    // 等待次核启动
    debug_println!("Waiting for secondary CPUs...");
    let mut wait_count = 0;
    while crate::arch::aarch64::smp::SmpData::get_active_cpu_count() < 2 {
        // 使用 NOP 而不是 WFI，避免在 IRQ 未正确配置时挂起
        unsafe {
            core::arch::asm!("nop", options(nomem, nostack));
        }
        wait_count += 1;
        if wait_count > 1000 {
            debug_println!("SMP: Timeout waiting for CPU 1");
            break;
        }
    }
    let active_cpus = crate::arch::aarch64::smp::SmpData::get_active_cpu_count();

    // 打印 CPU 数量（使用 putchar）
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"SMP: ";
        for &b in MSG {
            putchar(b);
        }
        let hex = b"0123456789";
        putchar(hex[active_cpus as usize]);
        const MSG2: &[u8] = b" CPUs online\n";
        for &b in MSG2 {
            putchar(b);
        }
    }

    // 启用 IRQ（GIC 已初始化，SMP 已启动）
    debug_println!("Enabling IRQ...");
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));

        // 读取 DAIF 寄存器确认 IRQ 被使能
        use crate::console::putchar;
        const MSG_DAIF: &[u8] = b"DAIF after enable: 0x";
        for &b in MSG_DAIF {
            putchar(b);
        }
        let daif: u64;
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
        let hex = b"0123456789ABCDEF";
        putchar(hex[((daif >> 4) & 0xF) as usize]);
        putchar(hex[(daif & 0xF) as usize]);
        const MSG_NL: &[u8] = b"\n";
        for &b in MSG_NL {
            putchar(b);
        }
    }
    debug_println!("IRQ enabled");

    debug_println!("System ready");

    // 测试 1: 使用底层 putchar 测试
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"After System ready\n";
        for &b in MSG {
            putchar(b);
        }

        // 测试 PID 获取
        const MSG2: &[u8] = b"Getting PID...\n";
        for &b in MSG2 {
            putchar(b);
        }
    }

    // 测试 2: 获取当前 PID
    let current_pid = process::sched::get_current_pid();

    // 打印 PID（使用十六进制）
    unsafe {
        use crate::console::putchar;
        const MSG3: &[u8] = b"Current PID: ";
        for &b in MSG3 {
            putchar(b);
        }

        let hex_chars = b"0123456789ABCDEF";
        let pid = current_pid as u64;
        putchar(hex_chars[((pid >> 60) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 56) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 52) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 48) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 44) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 40) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 36) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 32) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 28) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 24) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 20) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 16) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 12) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 8) & 0xF) as usize]);
        putchar(hex_chars[((pid >> 4) & 0xF) as usize]);
        putchar(hex_chars[(pid & 0xF) as usize]);
        putchar(b'\n');
    }

    // 测试 fork 系统调用
    unsafe {
        use crate::console::putchar;
        const MSG5: &[u8] = b"Testing fork syscall...\n";
        for &b in MSG5 {
            putchar(b);
        }
    }

    // 直接调用 do_fork 测试（不通过系统调用）
    match process::sched::do_fork() {
        Some(child_pid) => {
            unsafe {
                use crate::console::putchar;
                const MSG: &[u8] = b"Fork success: child PID = ";
                for &b in MSG {
                    putchar(b);
                }

                let hex_chars = b"0123456789ABCDEF";
                let pid = child_pid as u64;
                putchar(hex_chars[((pid >> 28) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 24) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 20) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 16) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 12) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 8) & 0xF) as usize]);
                putchar(hex_chars[((pid >> 4) & 0xF) as usize]);
                putchar(hex_chars[(pid & 0xF) as usize]);
                putchar(b'\n');
            }
        }
        None => {
            unsafe {
                use crate::console::putchar;
                const MSG: &[u8] = b"Fork failed\n";
                for &b in MSG {
                    putchar(b);
                }
            }
        }
    }

    // 主内核循环 - 等待中断
    unsafe {
        use crate::console::putchar;
        const MSG4: &[u8] = b"Entering main loop\n";
        for &b in MSG4 {
            putchar(b);
        }
    }

    loop {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println!("!!! KERNEL PANIC !!!");
    println!("{}", _info);
    loop {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}

#[lang = "eh_personality"]
extern "C" fn eh_personality() {
    loop {}
}

#[no_mangle]
extern "C" fn abort() -> ! {
    panic!("aborted");
}
