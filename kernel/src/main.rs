#![no_std]
#![no_main]
#![feature(lang_items, global_asm, naked_functions)]

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

// 包含平台特定的汇编代码
#[cfg(feature = "aarch64")]
use core::arch::global_asm;

#[cfg(feature = "aarch64")]
global_asm!(include_str!("arch/aarch64/boot.S"));

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

    debug_println!("Initializing scheduler...");
    process::sched::init();

    // TODO: VFS、GIC、Timer 初始化暂时禁用
    // println!("Initializing VFS...");
    // crate::fs::vfs_init();

    // 暂时禁用 GIC 和 Timer，避免导致挂起
    /*
    debug_println!("Initializing GIC...");
    drivers::intc::init();

    debug_println!("Initializing timer...");
    drivers::timer::init();
    */

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
