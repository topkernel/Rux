//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V PLIC (Platform-Level Interrupt Controller) 驱动
//!
//! 参考 RISC-V PLIC 规范
//! QEMU virt 平台内存布局

use core::arch::asm;
use crate::println;

// PLIC base address - QEMU virt platform uses 0x0c000000
// NOTE: Must use plain hex digits (0x0c000000) not (0x0c00_0000) to avoid
// the compiler dropping the leading zero!
const PLIC_BASE: usize = 201326592;  // 0x0c000000 in decimal

mod offset {
    // 优先级寄存器（每个中断 4 字节）
    pub const PRIORITY: usize = 0x0000;

    // 待取中断寄存器（每次读取 4 字节）
    pub const PENDING: usize = 0x1000;

    // 使能寄存器（每个 hart 一组）
    pub const ENABLE: usize = 0x2000;

    // 阈值寄存器（每个 hart 一个）
    // 位于 context 偏移 0x0000
    pub const THRESHOLD: usize = 0x0000;

    // Claim 寄存器（每个 hart 一个）
    // Complete 寄存器（每个 hart 一个）
    // 位于 context 偏移 0x0004
    pub const CLAIM_COMPLETE: usize = 0x0004;
}

pub const MAX_INTERRUPTS: usize = 128;

const CONTEXT_SIZE: usize = 0x1000;

pub const PLIC_PRIORITY_BASE: u32 = 1;
pub const PLIC_PRIORITY_MIN: u32 = 0;
pub const PLIC_PRIORITY_MAX: u32 = 7;

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
        println!("plic: Initializing PLIC: base = {} ({:#x}), num_harts = {}", self.base, self.base, self.num_harts);

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
        // 首先设置中断优先级（必须 > 0 才能触发）
        self.set_priority(irq, PLIC_PRIORITY_BASE);

        // 然后在 ENABLE 寄存器中设置对应的位
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
            // 调试：先读取 PENDING 寄存器
            let pending = self.read_pending();
            if pending != 0 {
                println!("plic: PENDING = 0x{:08x} (hart {})", pending, hart);
            }

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
                println!("plic: Claimed IRQ {} (hart {})", irq, hart);
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

    /// 触发软件中断（IPI）
    ///
    /// 注意：标准 PLIC 不支持软件触发中断
    /// 这个函数直接向 PENDING 寄存器写入来模拟中断
    /// 仅适用于 QEMU virt 等模拟环境
    pub fn trigger_ipi(&self, irq: usize) {
        if irq >= 32 {
            // PENDING 寄存器是 32-bit 的，只支持 IRQ 0-31
            return;
        }

        let addr = self.base + offset::PENDING;

        unsafe {
            // 读取当前 PENDING 状态
            let pending: u32;
            asm!(
                "lw {}, 0({})",
                out(reg) pending,
                in(reg) addr,
                options(nostack)
            );

            // 设置对应的位
            let new_pending = pending | (1 << irq);

            // 写回 PENDING 寄存器
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") new_pending,
                options(nostack)
            );
        }
    }
}

static PLIC: Plic = Plic::new(PLIC_BASE, 4);

pub fn init() {
    println!("intc: Initializing RISC-V PLIC...");

    PLIC.init();

    // 使能关键中断
    // RISC-V virt 平台中断映射（QEMU）:
    // - IRQ 1-8: VirtIO 设备（8 个 VirtIO 槽位）
    // - IRQ 10: UART (ns16550a)
    // - IRQ 11-13: IPI (软件中断，用于核间通信)
    let boot_hart = crate::arch::riscv64::smp::cpu_id();

    // 为启动核使能 VirtIO 设备中断
    // IRQ 1 是第一个 VirtIO 设备（通常是 VirtIO-Blk）
    PLIC.enable_interrupt(boot_hart, 1);
    // 也使能其他 VirtIO 槽位的 IRQ（以防有多个 VirtIO 设备）
    // IRQ 2-8 对应 VirtIO 槽位 1-7
    for virtio_irq in 2..9 {  // 2 到 8（包含 8）
        PLIC.enable_interrupt(boot_hart, virtio_irq);
    }

    // 为启动核使能 UART 中断（QEMU RISC-V virt: IRQ 10）
    PLIC.enable_interrupt(boot_hart, 10);

    // 使能 IPI 中断（用于核间通信）
    for hart in 0..4 {
        for ipi_irq in 11..14 {  // 11-13: IPI
            PLIC.enable_interrupt(hart, ipi_irq);
        }
    }

    println!("intc: PLIC initialized");
}

pub fn claim(hart: usize) -> Option<usize> {
    PLIC.claim(hart)
}

pub fn complete(hart: usize, irq: usize) {
    PLIC.complete(hart, irq)
}

pub fn enable_interrupt(hart: usize, irq: usize) {
    PLIC.enable_interrupt(hart, irq);
}

pub fn read_pending() -> u32 {
    PLIC.read_pending()
}

pub fn trigger_ipi(irq: usize) {
    PLIC.trigger_ipi(irq)
}
