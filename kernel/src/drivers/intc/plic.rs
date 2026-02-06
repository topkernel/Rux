//! RISC-V PLIC (Platform-Level Interrupt Controller) 驱动
//!
//! 参考 RISC-V PLIC 规范
//! QEMU virt 平台内存布局

use core::sync::atomic::{AtomicU32, Ordering};
use core::arch::asm;
use crate::println;

/// PLIC 基地址（QEMU virt 平台）
const PLIC_BASE: usize = 0x0c00_0000;

/// PLIC 寄存器偏移
mod offset {
    // 优先级寄存器（每个中断 4 字节）
    pub const PRIORITY: usize = 0x0000;

    // 待取中断寄存器（每次读取 4 字节）
    pub const PENDING: usize = 0x1000;

    // 使能寄存器（每个 hart 一组）
    pub const ENABLE: usize = 0x2000;

    // Claim/Complete 寄存器（每个 hart 一个）
    pub const CLAIM_COMPLETE: usize = 0x200000;

    // 阈值寄存器（每个 hart 一个）
    pub const THRESHOLD: usize = 0x200000;
}

/// 最大中断数（QEMU virt 平台）
pub const MAX_INTERRUPTS: usize = 128;

/// 上下文大小（每个 hart 的寄存器区域大小）
const CONTEXT_SIZE: usize = 0x1000;

/// 中断优先级
pub const PLIC_PRIORITY_BASE: u32 = 1;
pub const PLIC_PRIORITY_MIN: u32 = 0;
pub const PLIC_PRIORITY_MAX: u32 = 7;

/// PLIC 实例
pub struct Plic {
    base: usize,
    num_harts: usize,
}

impl Plic {
    /// 创建新的 PLIC 实例
    pub const fn new(base: usize, num_harts: usize) -> Self {
        Self {
            base,
            num_harts,
        }
    }

    /// 初始化 PLIC
    ///
    /// 禁用所有中断，设置阈值
    pub fn init(&self) {
        // 禁用所有中断（设置为优先级 0，表示禁用）
        for irq in 1..MAX_INTERRUPTS {
            self.set_priority(irq, 0);
        }

        // 为每个 hart 设置阈值（只响应优先级 > threshold 的中断）
        for hart in 0..self.num_harts {
            self.set_threshold(hart, 0);
        }

        // 禁用所有 hart 的中断
        for hart in 0..self.num_harts {
            for irq_in_word in 0..(MAX_INTERRUPTS / 32) {
                self.disable_interrupts(hart, irq_in_word);
            }
        }
    }

    /// 设置中断优先级
    fn set_priority(&self, irq: usize, priority: u32) {
        let addr = self.base + offset::PRIORITY + irq * 4;
        unsafe {
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") priority,
                options(nostack)
            );
        }
    }

    /// 设置 hart 的中断阈值
    ///
    /// 只有优先级 > threshold 的中断才会被传递给 hart
    fn set_threshold(&self, hart: usize, threshold: u32) {
        let addr = self.base + offset::THRESHOLD + hart * CONTEXT_SIZE;
        unsafe {
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") threshold,
                options(nostack)
            );
        }
    }

    /// 使能指定 hart 的中断
    pub fn enable_interrupt(&self, hart: usize, irq: usize) {
        let word = irq / 32;
        let bit = irq % 32;
        let addr = self.base + offset::ENABLE + hart * CONTEXT_SIZE + word * 4;

        unsafe {
            let value: u32;
            asm!(
                "lw {}, 0({})",
                out(reg) value,
                in(reg) addr,
                options(nostack)
            );

            // 设置对应的位
            let new_value = value | (1 << bit);

            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") new_value,
                options(nostack)
            );
        }
    }

    /// 禁用指定 hart 的中断（禁用一个 32-bit word 中的所有中断）
    fn disable_interrupts(&self, hart: usize, word: usize) {
        let addr = self.base + offset::ENABLE + hart * CONTEXT_SIZE + word * 4;
        unsafe {
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") 0u32,
                options(nostack)
            );
        }
    }

    /// Claim（声明）中断
    ///
    /// 返回最高优先级的待处理中断 ID
    pub fn claim(&self, hart: usize) -> Option<usize> {
        let addr = self.base + offset::CLAIM_COMPLETE + hart * CONTEXT_SIZE + 0x4;

        unsafe {
            let irq: u32;
            asm!(
                "lw {}, 0({})",
                out(reg) irq,
                in(reg) addr,
                options(nostack)
            );

            if irq == 0 {
                None
            } else {
                Some(irq as usize)
            }
        }
    }

    /// Complete（完成）中断
    ///
    /// 通知 PLIC 中断处理已完成
    pub fn complete(&self, hart: usize, irq: usize) {
        let addr = self.base + offset::CLAIM_COMPLETE + hart * CONTEXT_SIZE + 0x4;

        unsafe {
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") irq as u32,
                options(nostack)
            );
        }
    }

    /// 读取待取中断状态
    pub fn read_pending(&self) -> u32 {
        let addr = self.base + offset::PENDING;

        unsafe {
            let pending: u32;
            asm!(
                "lw {}, 0({})",
                out(reg) pending,
                in(reg) addr,
                options(nostack)
            );

            pending
        }
    }
}

/// 全局 PLIC 实例（QEMU virt: 4 harts）
static PLIC: Plic = Plic::new(PLIC_BASE, 4);

/// 初始化 PLIC
pub fn init() {
    println!("intc: Initializing RISC-V PLIC...");

    PLIC.init();

    // 使能关键中断
    // 中断 1: UART (ns16550a)
    // 中断 10-13: IPI (软件中断，用于核间通信)
    let boot_hart = crate::arch::riscv64::smp::cpu_id();

    // 为启动核使能 UART 中断
    PLIC.enable_interrupt(boot_hart, 1);

    // 使能 IPI 中断（用于核间通信）
    for hart in 0..4 {
        for ipi_irq in 10..14 {
            PLIC.enable_interrupt(hart, ipi_irq);
        }
    }

    println!("intc: PLIC initialized");
}

/// Claim 中断
pub fn claim(hart: usize) -> Option<usize> {
    PLIC.claim(hart)
}

/// Complete 中断
pub fn complete(hart: usize, irq: usize) {
    PLIC.complete(hart, irq)
}

/// 使能中断
pub fn enable_interrupt(hart: usize, irq: usize) {
    PLIC.enable_interrupt(hart, irq);
}

/// 读取待取中断状态
pub fn read_pending() -> u32 {
    PLIC.read_pending()
}
