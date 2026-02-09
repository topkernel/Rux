#!/usr/bin/env rust-script
//!
//! 极简测试用户程序 - 只有一个无限循环
//! 用于验证用户模式切换是否工作

#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 无限循环 - 应该产生 timer interrupt
    loop {
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
