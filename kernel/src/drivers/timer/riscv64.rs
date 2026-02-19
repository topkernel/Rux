//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Timer 驱动
//!
//! 使用 SBI 调用来设置定时器
//!
//! ...
//! - `kernel/time/timer.c` - jiffies 和时间管理
//! - `kernel/sched/clock.c` - 调度器时钟
//! - `kernel/sched/fair.c` - scheduler_tick()

use riscv::register::time;
use crate::sbi;
use core::sync::atomic::{AtomicU64, Ordering};

/// 定时器频率 (QEMU virt 平台)
pub const CLOCK_FREQ: u64 = 10_000_000;  // 10 MHz

/// 系统时钟频率 (HZ)
///
/// 每秒触发 100 次时钟中断（每 10ms 一次）
pub const HZ: u64 = 100;

/// 时间片长度 (10 毫秒)
///
/// 每个时间片 10ms，用于抢占式调度
const TIME_SLICE_TICKS: u64 = CLOCK_FREQ / HZ;  // 10ms

/// jiffies - 全局时钟计数器
///
///
/// 用于：
/// - 时间测量
/// - 超时管理
/// - 调度统计
/// - 性能分析
///
/// 类型：AtomicU64（支持多核并发访问）
static JIFFIES: AtomicU64 = AtomicU64::new(0);

/// jiffies 相关函数

/// 获取当前 jiffies 值
///
///
/// # 返回
/// - 当前 jiffies 值（自系统启动以来的时钟中断次数）
#[inline]
pub fn get_jiffies() -> u64 {
    JIFFIES.load(Ordering::Acquire)
}

/// 增加 jiffies 计数器
///
/// 在每次时钟中断时调用
#[inline]
fn increment_jiffies() {
    JIFFIES.fetch_add(1, Ordering::Release);
}

/// 将 jiffies 转换为毫秒
///
#[inline]
pub const fn jiffies_to_msecs(jiffies: u64) -> u64 {
    jiffies * 1000 / HZ
}

/// 将毫秒转换为 jiffies
///
#[inline]
pub const fn msecs_to_jiffies(msecs: u64) -> u64 {
    msecs * HZ / 1000
}

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
pub fn set_next_trigger() {
    let current = read_time();
    let deadline = current + TIME_SLICE_TICKS;  // 10ms 后触发
    set_timer(deadline);
}

/// 时钟中断处理函数
///
///
/// # 功能
/// 1. 更新 jiffies 计数器
/// 2. 更新系统运行时间统计
/// 3. 触发调度器 tick
/// 4. 处理定时器回调
///
/// # 调用时机
/// 每次时钟中断时由 trap_handler 调用
///
/// # 注意
/// - 在中断上下文中调用，不能睡眠
/// - 需要尽快完成，避免影响系统性能
pub fn timer_interrupt_handler() {
    // 1. 更新 jiffies 计数器
    increment_jiffies();

    // 3. TODO: 更新进程运行时间统计
    //    - 当前进程的 utime/stime
    //    - CPU 统计信息

    // 4. TODO: 处理软件定时器
    //    - 检查到期的定时器
    //    - 调用定时器回调函数

    // 5. TODO: 触发调度器 tick
    //    - 更新当前进程运行时间
    //    - 检查是否需要调度
    //    - 设置 need_resched 标志

    // 注意：调度由 trap.rs 中的 schedule() 调用处理
}
