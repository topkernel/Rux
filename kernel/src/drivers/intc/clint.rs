//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V CLINT (Core-Local Interrupt Controller) 驱动
//!
//! **注意**：现代 RISC-V 系统（OpenSBI v1.3+）不允许 S-mode 直接访问 CLINT 寄存器
//! CLINT 被配置为 M-mode only，S-mode 必须使用 SBI 调用来访问定时器和 IPI 功能
//!
//! Clint 负责处理：
//! - 软件中断（MSIP）- 用于核间中断（IPI）
//! - 定时器中断（MTIMECMP）
//! - 时间寄存器（MTIME）
//!
//! 本实现使用 SBI 调用代替直接 MMIO 访问

use core::sync::atomic::{AtomicU32, Ordering};
use crate::sbi;

// IPI 计数器（每个 hart 一个）
static IPI_COUNT: [AtomicU32; 4] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

/// 初始化 CLINT 驱动
///
/// 注意：现代系统不需要直接访问 CLINT 寄存器
/// SBI 固件负责管理 CLINT，S-mode 通过 SBI 调用访问
pub fn init() {
    // SBI 系统会自动管理 CLINT
    // 不需要 S-mode 软件进行初始化
    // 清除 IPI 计数器
    for hart in 0..4 {
        IPI_COUNT[hart].store(0, Ordering::Relaxed);
    }
}

/// 发送 IPI 到指定 hart
///
/// 使用 SBI IPI Extension (EID #0x735049)
///
/// # 参数
/// * `target_hart` - 目标 hart ID (0-3)
pub fn send_ipi(target_hart: usize) {
    if target_hart >= 4 {
        return;
    }

    // 通过 SBI 发送 IPI（不直接访问 CLINT MSIP 寄存器）
    if sbi::send_ipi(target_hart) {
        // 更新计数器
        IPI_COUNT[target_hart].fetch_add(1, Ordering::Relaxed);
    }
}

/// 清除指定 hart 的 IPI
///
/// 注意：使用 SBI 时，IPI 的清除由 SBI 固件自动处理
/// S-mode 软件不需要手动清除 MSIP 寄存器
///
/// # 参数
/// * `hart` - hart ID
pub fn clear_ipi(hart: usize) {
    if hart >= 4 {
        return;
    }

    // SBI 系统会自动清除 IPI
    // 在软件中断处理程序中，SBI 会自动清除 pending 状态
    // 不需要 S-mode 软件手动清除

    // 可选：清除计数器（如果需要）
    // IPI_COUNT[hart].store(0, Ordering::Relaxed);
}

/// 获取发送到指定 hart 的 IPI 数量
///
/// # 参数
/// * `hart` - hart ID
///
/// # 返回
/// IPI 计数
pub fn get_ipi_count(hart: usize) -> u32 {
    if hart < 4 {
        IPI_COUNT[hart].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// 读取系统时间（time CSR）
///
/// 使用 RISC-V `rdtime` 指令读取时间
///
/// # 返回
/// 当前时间（cycles）
pub fn read_time() -> u64 {
    unsafe {
        let time: u64;
        core::arch::asm!(
            "rdtime {}",
            out(reg) time,
            options(nostack, readonly)
        );
        time
    }
}

/// 设置定时器比较值
///
/// 使用 SBI TIMER Extension 的 set_timer 函数
///
/// # 参数
/// * `hart` - hart ID（注意：set_timer 是 per-hart 的）
/// * `value` - 定时器比较值（绝对时间）
pub fn set_timecmp(_hart: usize, value: u64) {
    // 使用 SBI set_timer
    // 注意：SBI 的 set_timer 是 per-hart 的，会自动应用到当前 hart
    unsafe {
        sbi_rt::set_timer(value);
    }
}
