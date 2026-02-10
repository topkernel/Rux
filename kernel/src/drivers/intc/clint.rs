//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V CLINT (Core-Local Interrupt Controller) 驱动
//!
//! Clint 负责处理：
//! - 软件中断（MSIP）- 用于核间中断（IPI）
//! - 定时器中断（MTIMECMP）
//! - 时间寄存器（MTIME）

use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

const CLINT_BASE: usize = 0x0200_0000;

mod offset {
    // MSIP（Machine Software Interrupt Pending）寄存器
    // 每个 hart 一个 32-bit 寄存器，写入 1 触发软件中断
    pub const MSIP: usize = 0x0000;

    // MTIMECMP（Machine Timer Compare）寄存器
    // 每个 hart 一个 64-bit 寄存器
    pub const MTIMECMP: usize = 0x4000;

    // MTIME（Machine Time）寄存器
    // 全局 64-bit 时间计数器
    pub const MTIME: usize = 0xbff8;
}

pub struct Clint {
    base: usize,
    num_harts: usize,
}

impl Clint {
    /// 创建新的 Clint 实例
    pub const fn new(base: usize, num_harts: usize) -> Self {
        Self {
            base,
            num_harts,
        }
    }

    /// 发送软件中断到指定 hart
    ///
    /// 向 MSIP 寄存器写入 1 触发软件中断
    pub fn send_ipi(&self, hart: usize) {
        if hart >= self.num_harts {
            return;
        }

        let addr = self.base + offset::MSIP + hart * 4;

        unsafe {
            // 写入 1 触发软件中断
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") 1u32,
                options(nostack)
            );
        }
    }

    /// 清除指定 hart 的软件中断
    ///
    /// 向 MSIP 寄存器写入 0 清除软件中断
    pub fn clear_ipi(&self, hart: usize) {
        if hart >= self.num_harts {
            return;
        }

        let addr = self.base + offset::MSIP + hart * 4;

        unsafe {
            // 写入 0 清除软件中断
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") 0u32,
                options(nostack)
            );
        }
    }

    /// 读取 MTIME（时间计数器）
    pub fn read_time(&self) -> u64 {
        let addr = self.base + offset::MTIME;

        unsafe {
            let time: u64;
            // 读取 64-bit 时间值
            asm!(
                "ld {}, 0({})",
                out(reg) time,
                in(reg) addr,
                options(nostack, readonly)
            );
            time
        }
    }

    /// 设置 MTIMECMP（定时器比较值）
    pub fn set_timecmp(&self, hart: usize, value: u64) {
        if hart >= self.num_harts {
            return;
        }

        let addr = self.base + offset::MTIMECMP + hart * 8;

        unsafe {
            // 写入 64-bit 比较值
            // 注意：需要先写入高 32 位，再写入低 32 位
            let high = (value >> 32) as u32;
            let low = (value & 0xFFFFFFFF) as u32;

            // 写入高 32 位（地址 + 4）
            asm!(
                "sw t1, 4(a0)",
                in("a0") addr,
                in("t1") high,
                options(nostack)
            );

            // 写入低 32 位（地址 + 0）
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") low,
                options(nostack)
            );
        }
    }
}

static CLINT: Clint = Clint::new(CLINT_BASE, 4);

static IPI_COUNT: [AtomicU32; 4] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

pub fn init() {
    // 清除所有 hart 的待处理软件中断
    for hart in 0..4 {
        CLINT.clear_ipi(hart);
    }
}

pub fn send_ipi(target_hart: usize) {
    if target_hart >= 4 {
        return;
    }

    // 通过 Clint 发送软件中断
    CLINT.send_ipi(target_hart);

    // 更新计数器
    IPI_COUNT[target_hart].fetch_add(1, Ordering::Relaxed);
}

pub fn clear_ipi(hart: usize) {
    if hart >= 4 {
        return;
    }

    CLINT.clear_ipi(hart);
}

pub fn get_ipi_count(hart: usize) -> u32 {
    if hart < 4 {
        IPI_COUNT[hart].load(Ordering::Relaxed)
    } else {
        0
    }
}
