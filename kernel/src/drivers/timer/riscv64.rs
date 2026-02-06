//! RISC-V Timer 驱动
//!
//! 使用 SBI 调用来设置定时器

use core::arch::asm;
use crate::sbi;

/// 定时器频率 (QEMU virt 平台)
pub const CLOCK_FREQ: u64 = 10_000_000;  // 10 MHz

/// 读取当前时间 (time CSR)
#[inline]
pub fn read_time() -> u64 {
    unsafe {
        let time: u64;
        asm!("csrr {}, time", out(reg) time);
        time
    }
}

/// 设置定时器 (使用 SBI 调用)
pub fn set_timer(deadline: u64) {
    let _ = sbi::set_timer(deadline);
}

/// 设置下一次定时器中断（1 秒后）
pub fn set_next_trigger() {
    let current = read_time();
    let delay_ticks = CLOCK_FREQ;  // 1 秒
    let deadline = current + delay_ticks;
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"timer: setting deadline\n";
        for &b in MSG {
            putchar(b);
        }
    }
    set_timer(deadline);
    unsafe {
        use crate::console::putchar;
        const MSG: &[u8] = b"timer: deadline set\n";
        for &b in MSG {
            putchar(b);
        }
    }
}
