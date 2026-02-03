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
