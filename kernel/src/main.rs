#![no_std]
#![no_main]
#![feature(lang_items, global_asm, naked_functions, alloc_error_handler, linkage)]

#[macro_use]
extern crate log;
extern crate alloc;

use core::panic::PanicInfo;
use core::arch::asm;

mod arch;
mod sbi;
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

// RISC-V 汇编支持
#[cfg(feature = "riscv64")]
use core::arch::global_asm;

#[cfg(feature = "riscv64")]
global_asm!(include_str!("arch/riscv64/boot.S"));

// RISC-V kernel main function
#[cfg(feature = "riscv64")]
#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    // 使用底层输出
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"rust_main: entered\n";
        for &b in MSG {
            putchar(b);
        }
    }

    // 初始化控制台
    console::init();

    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"Rux Kernel starting...\n";
        for &b in MSG {
            putchar(b);
        }
    }

    // 初始化 trap
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"Initializing trap...\n";
        for &b in MSG {
            putchar(b);
        }
    }
    arch::trap::init();

    // 使能 timer interrupt
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"Enabling timer interrupt...\n";
        for &b in MSG {
            putchar(b);
        }
    }
    arch::trap::enable_timer_interrupt();

    // 设置第一次定时器中断
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"Setting first timer...\n";
        for &b in MSG {
            putchar(b);
        }
    }
    drivers::timer::set_next_trigger();

    println!("Timer interrupt enabled! Waiting for interrupts...");

    // 主循环：等待中断
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

// ARMv8 kernel entry point
#[cfg(feature = "aarch64")]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 禁用中断直到中断控制器设置完成
    // 测试不同的 DAIF 值来找到正确的映射
    // 目标：同时屏蔽 I 和 F

    unsafe {
        // 尝试 0xC0 (bits 7,6) → 之前得到 0x80 (只有 bit 7)
        // 尝试 0x40 (bit 6) → 刚才得到 0x40 (正确!)
        // 尝试 0x80 (bit 7)
        let daif_val = 0xC0u64;  // 尝试设置 bits 7 和 6
        core::arch::asm!("msr daif, {}", in(reg) daif_val);
    }

    // 验证最终的 DAIF 值
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"_start: Final DAIF = 0x";
        for &b in MSG {
            putchar(b);
        }
        let daif_check: u64;
        asm!("mrs {}, daif", out(reg) daif_check, options(nomem, nostack));
        let hex = b"0123456789ABCDEF";
        for i in (0..16).rev() {
            putchar(hex[((daif_check >> (i * 4)) & 0xF) as usize]);
        }
        const NL: &[u8] = b"\n";
        for &b in NL {
            putchar(b);
        }
    }

    // 设置栈指针
    unsafe {
        asm!(
            "mv sp, {}",  // 设置栈指针 (SP)
            in(reg) 0x80018000,  // 栈地址
            options(nostack)
        );
    }

    // 清零 BSS
    extern "C" {
        #[link_name = "__bss_start"]
        static mut __bss_start: u64;
        #[link_name = "__bss_end"]
        static mut __bss_end: u64;
    }
    unsafe {
        let start = &mut __bss_start as *mut u64 as usize;
        let end = &mut __bss_end as *mut u64 as usize;
        if start < end {
            core::slice::from_raw_parts_mut(start, end - start).fill(0);
        }
    }

    // 跳转到 Rust 代码
    unsafe {
        asm!(
            "b {}",
            sym rust_main,
            options(nostack)
        );
    }

    loop {}
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

// Allocation error handler for no_std
#[alloc_error_handler]
fn alloc_error_handler(_layout: core::alloc::Layout) -> ! {
    panic!("Allocation error!");
}

// 汇编符号（供汇编代码调用）
extern "C" {
    fn rust_main() -> !;
}
