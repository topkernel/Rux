//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

use core::arch::asm;
use crate::console::putchar;

// aarch64 内存布局
pub const MEMORY_MAP_BASE: usize = 0x80000_0000;
pub const MEMORY_MAP_SIZE: usize = 0x8000_0000; // 2GB

// 异常级别
#[repr(u64)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ExceptionLevel {
    EL0 = 0,
    EL1 = 1,
    EL2 = 2,
    EL3 = 3,
}

pub fn init() {
    // 输出架构初始化信息
    const MSG1: &[u8] = b"arch: Initializing aarch64 architecture...\n";
    for &b in MSG1 {
        unsafe { putchar(b); }
    }

    // 显示当前异常级别
    let current_el = get_current_el();
    const MSG2: &[u8] = b"arch: Current Exception Level: EL";
    for &b in MSG2 {
        unsafe { putchar(b); }
    }
    let el_num = current_el as u64;
    const DIGITS: &[u8] = b"0123456789";
    unsafe { putchar(DIGITS[el_num as usize]); }
    const MSG3: &[u8] = b"\n";
    for &b in MSG3 {
        unsafe { putchar(b); }
    }

    // 获取并显示 CPU ID
    let cpu_id = get_core_id();
    const MSG4: &[u8] = b"arch: CPU ID: ";
    for &b in MSG4 {
        unsafe { putchar(b); }
    }
    let hex_chars = b"0123456789ABCDEF";
    unsafe {
        putchar(hex_chars[((cpu_id >> 4) & 0xF) as usize]);
        putchar(hex_chars[(cpu_id & 0xF) as usize]);
    }
    const MSG5: &[u8] = b"\n";
    for &b in MSG5 {
        unsafe { putchar(b); }
    }

    // 禁用 IRQ
    const MSG6: &[u8] = b"arch: Disabling IRQ until GIC initialization...\n";
    for &b in MSG6 {
        unsafe { putchar(b); }
    }

    unsafe {
        // 确保 IRQ 被禁用
        asm!(
            "msr daifset, #2",  // 设置 bit 1 (I bit) 禁用 IRQ
            options(nomem, nostack)
        );
    }

    const MSG7: &[u8] = b"arch: Architecture initialization [DONE]\n";
    for &b in MSG7 {
        unsafe { putchar(b); }
    }
}

#[inline]
fn get_current_el() -> ExceptionLevel {
    let el: u64;
    unsafe {
        asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack, pure));
    }
    match (el >> 2) & 0x3 {
        0 => ExceptionLevel::EL0,
        1 => ExceptionLevel::EL1,
        2 => ExceptionLevel::EL2,
        3 => ExceptionLevel::EL3,
        _ => unreachable!(),
    }
}

/// 获取核心ID
#[inline]
pub fn get_core_id() -> u64 {
    let core_id: u64;
    unsafe {
        asm!("mrs {}, mpidr_el1", out(reg) core_id, options(nomem, nostack, pure));
    }
    core_id & 0xFF
}
