#!/usr/bin/env rust-script
//!
//! Rux 用户程序示例 - Hello World
//!
//! 这是一个最小的 RISC-V 用户程序，演示如何：
//! 1. 使用 no_std 环境
//! 2. 通过系统调用输出字符串
//! 3. 正常退出程序

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// ============================================================================
// 系统调用接口（RISC-V Linux ABI）
// ============================================================================

/// 系统调用号（遵循 RISC-V Linux ABI）
mod syscall {
    /// 系统调用号
    pub const SYS_WRITE: u64 = 64;
    pub const SYS_EXIT: u64 = 93;

    /// 执行系统调用
    ///
    /// RISC-V 系统调用约定：
    /// - a7: 系统调用号
    /// - a0-a5: 参数
    /// - 返回值: a0
    #[inline(always)]
    pub unsafe fn syscall1(n: u64, a0: u64) -> u64 {
        let mut ret: u64;
        core::arch::asm!(
            "ecall",
            inlateout("a7") n => _,
            inlateout("a0") a0 => ret,
            lateout("a1") _,
            lateout("a2") _,
            lateout("a3") _,
            lateout("a4") _,
            lateout("a5") _,
            lateout("a6") _,
            options(nostack, nomem)
        );
        ret
    }

    /// 执行系统调用（3个参数）
    #[inline(always)]
    pub unsafe fn syscall3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
        let mut ret: u64;
        core::arch::asm!(
            "ecall",
            inlateout("a7") n => _,
            inlateout("a0") a0 => ret,
            inlateout("a1") a1 => _,
            inlateout("a2") a2 => _,
            lateout("a3") _,
            lateout("a4") _,
            lateout("a5") _,
            lateout("a6") _,
            options(nostack, nomem)
        );
        ret
    }
}

// ============================================================================
// 标准输出函数
// ============================================================================

/// 写入字符串到标准输出
fn print(s: &str) {
    unsafe {
        // fd = 1 (stdout), buf = s.as_ptr(), count = s.len()
        syscall::syscall3(
            syscall::SYS_WRITE,
            1,              // fd = stdout
            s.as_ptr() as u64,
            s.len() as u64,
        );
    }
}

// ============================================================================
// 程序入口点
// ============================================================================

/// 用户程序入口点
///
/// 注意：链接器会查找名为 `_start` 的符号作为入口点
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 简单测试：只调用 sys_exit
    // SYS_EXIT = 93
    unsafe { syscall::syscall1(93, 0) };

    // 如果 sys_exit 失败（不应该发生），进入死循环
    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}

// ============================================================================
// Panic 处理
// ============================================================================

/// Panic 处理函数
///
/// 当程序发生 panic 时调用，简单地进入死循环
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}
