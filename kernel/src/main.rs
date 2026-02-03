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

    // TODO: VFS初始化暂时禁用
    // println!("Initializing VFS...");
    // crate::fs::vfs_init();

    debug_println!("Initializing GIC...");
    drivers::intc::init();

    debug_println!("Initializing timer...");
    drivers::timer::init();

    debug_println!("Enabling interrupts...");
    // 启用中断
    unsafe {
        asm!("msr daifclr, #2", options(nomem, nostack));
        asm!("isb", options(nomem, nostack));
    }

    debug_println!("System ready");

    // 注意：SVC 系统调用应该从用户模式 (EL0) 调用，不是内核模式 (EL1)
    // 用户程序执行测试已经验证系统调用框架正常工作
    // （之前输出显示了 [SVC:00] 和 sys_read: invalid fd）

    // 测试用户程序执行
    debug_println!("Testing user program execution...");
    process::test_user_program();

    // 主内核循环 - 等待中断
    debug_println!("Entering main loop");
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
