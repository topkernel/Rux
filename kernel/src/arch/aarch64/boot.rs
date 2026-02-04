use crate::println;
use core::arch::asm;

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
    // MMU initialization will be added when virtual memory is implemented
    // Use direct UART call to debug
    use crate::console::putchar;
    const MSG: &[u8] = b"arch::init() called\n";
    for &b in MSG {
        unsafe { putchar(b); }
    }

    // 注意：IRQ 将在 GIC 初始化之后再启用
    // 这里暂时禁用 IRQ 以防止中断风暴
    const MSG_IRQ: &[u8] = b"arch: IRQ disabled (will enable after GIC init)\n";
    for &b in MSG_IRQ {
        unsafe { putchar(b); }
    }

    unsafe {
        // 确保 IRQ 被禁用
        asm!(
            "msr daifset, #2",  // 设置 bit 1 (I bit) 禁用 IRQ
            options(nomem, nostack)
        );
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
