//! RISC-V Timer 驱动
//!
//! 使用 SBI 调用来设置定时器

use riscv::register::time;
use crate::sbi;

/// 定时器频率 (QEMU virt 平台)
pub const CLOCK_FREQ: u64 = 10_000_000;  // 10 MHz

/// 时间片长度 (10 毫秒)
///
/// 对应 Linux 内核的 HZ=100 (CONFIG_HZ)
/// 每个时间片 10ms，用于抢占式调度
const TIME_SLICE_TICKS: u64 = CLOCK_FREQ / 100;  // 10ms

/// 读取当前时间 (time CSR)
#[inline]
pub fn read_time() -> u64 {
    time::read() as u64
}

/// 设置定时器 (使用 SBI 调用)
pub fn set_timer(deadline: u64) {
    sbi::set_timer(deadline);
}

/// 设置下一次定时器中断（时间片长度）
///
/// 对应 Linux 内核的 scheduler_tick() + tick_sched_timer()
pub fn set_next_trigger() {
    let current = read_time();
    let deadline = current + TIME_SLICE_TICKS;  // 10ms 后触发
    set_timer(deadline);
}
