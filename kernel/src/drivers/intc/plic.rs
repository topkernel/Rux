//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V PLIC (Platform-Level Interrupt Controller) driver
//!
//! Implements the IrqChip and IrqDomainOps traits for the QEMU virt PLIC.

use core::arch::asm;
use crate::println;
use crate::interrupt::{
    IrqChip, IrqData, IrqDomainOps, IrqDomain,
    irq_domain_create_linear, irq_create_mapping,
};

// PLIC base address - QEMU virt platform uses 0x0c000000
const PLIC_BASE: usize = 201326592;  // 0x0c000000 in decimal

mod offset {
    pub const PRIORITY: usize = 0x0000;
    pub const PENDING: usize = 0x1000;
    pub const ENABLE: usize = 0x2000;
    pub const THRESHOLD: usize = 0x0000;
    pub const CLAIM_COMPLETE: usize = 0x0004;
}

/// Maximum number of interrupts - from config
pub const MAX_INTERRUPTS: usize = crate::config::PLIC_MAX_INTERRUPTS;

const CONTEXT_SIZE: usize = 0x1000;

pub const PLIC_PRIORITY_BASE: u32 = 1;
pub const PLIC_PRIORITY_MIN: u32 = 0;
pub const PLIC_PRIORITY_MAX: u32 = 7;

// ==================== PLIC hardware operations ====================

pub struct Plic {
    base: usize,
    num_harts: usize,
}

impl Plic {
    pub const fn new(base: usize, num_harts: usize) -> Self {
        Self { base, num_harts }
    }

    /// Initialize PLIC hardware: disable all IRQs, set thresholds to 0.
    pub fn init(&self) {
        // Disable all interrupts (priority = 0)
        for irq in 1..MAX_INTERRUPTS {
            self.set_priority(irq, 0);
        }

        // Set threshold = 0 for all harts
        for hart in 0..self.num_harts {
            self.set_threshold(hart, 0);
        }

        // Clear all enable bits
        for hart in 0..self.num_harts {
            for word in 0..((MAX_INTERRUPTS + 31) / 32) {
                let addr = self.base + offset::ENABLE + hart * CONTEXT_SIZE + word * 4;
                unsafe {
                    asm!("sw zero, 0(a0)", in("a0") addr, options(nostack));
                }
            }
        }
    }

    fn set_priority(&self, irq: usize, priority: u32) {
        let addr = self.base + offset::PRIORITY + irq * 4;
        unsafe {
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") priority, options(nostack));
        }
    }

    fn set_threshold(&self, hart: usize, threshold: u32) {
        let addr = self.base + offset::THRESHOLD + hart * CONTEXT_SIZE;
        unsafe {
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") threshold, options(nostack));
        }
    }

    pub fn enable_interrupt(&self, hart: usize, irq: usize) {
        self.set_priority(irq, PLIC_PRIORITY_BASE);
        let word = irq / 32;
        let bit = irq % 32;
        let addr = self.base + offset::ENABLE + hart * CONTEXT_SIZE + word * 4;
        unsafe {
            let value: u32;
            asm!("lw {}, 0({})", out(reg) value, in(reg) addr, options(nostack));
            let new_value = value | (1 << bit);
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") new_value, options(nostack));
        }
    }

    fn disable_interrupt(&self, hart: usize, irq: usize) {
        let word = irq / 32;
        let bit = irq % 32;
        let addr = self.base + offset::ENABLE + hart * CONTEXT_SIZE + word * 4;
        unsafe {
            let value: u32;
            asm!("lw {}, 0({})", out(reg) value, in(reg) addr, options(nostack));
            let new_value = value & !(1 << bit);
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") new_value, options(nostack));
        }
    }

    pub fn claim(&self, hart: usize) -> Option<usize> {
        let addr = self.base + offset::CLAIM_COMPLETE + hart * CONTEXT_SIZE + 0x4;
        unsafe {
            let irq: u32;
            asm!("lw {}, 0({})", out(reg) irq, in(reg) addr, options(nostack));
            if irq == 0 { None } else { Some(irq as usize) }
        }
    }

    pub fn complete(&self, hart: usize, irq: usize) {
        let addr = self.base + offset::CLAIM_COMPLETE + hart * CONTEXT_SIZE + 0x4;
        unsafe {
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") irq as u32, options(nostack));
        }
    }

    pub fn read_pending(&self) -> u32 {
        let addr = self.base + offset::PENDING;
        unsafe {
            let pending: u32;
            asm!("lw {}, 0({})", out(reg) pending, in(reg) addr, options(nostack));
            pending
        }
    }

    pub fn trigger_ipi(&self, irq: usize) {
        if irq >= 32 { return; }
        let addr = self.base + offset::PENDING;
        unsafe {
            let pending: u32;
            asm!("lw {}, 0({})", out(reg) pending, in(reg) addr, options(nostack));
            let new_pending = pending | (1 << irq);
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") new_pending, options(nostack));
        }
    }
}

static PLIC: Plic = Plic::new(PLIC_BASE, 4);

// ==================== IrqChip implementation ====================

fn plic_mask(data: &IrqData) {
    let hart = crate::arch::riscv64::smp::cpu_id() as usize;
    PLIC.disable_interrupt(hart, data.hwirq as usize);
}

fn plic_unmask(data: &IrqData) {
    let hart = crate::arch::riscv64::smp::cpu_id() as usize;
    PLIC.enable_interrupt(hart, data.hwirq as usize);
}

fn plic_eoi(data: &IrqData) {
    let hart = crate::arch::riscv64::smp::cpu_id() as usize;
    PLIC.complete(hart, data.hwirq as usize);
}

/// PLIC irq_chip (function-pointer-table pattern)
static PLIC_CHIP: IrqChip = IrqChip {
    name: "riscv-plic",
    irq_mask: Some(plic_mask),
    irq_unmask: Some(plic_unmask),
    irq_ack: None,
    irq_eoi: Some(plic_eoi),
    irq_set_type: None,
    irq_set_affinity: None,
};

// ==================== IrqDomainOps implementation ====================

fn plic_irq_map(_domain: &IrqDomain, _virq: u32, _hwirq: u32) -> i32 {
    // chip is already set by irq_create_mapping
    0
}

fn plic_irq_unmap(_domain: &IrqDomain, _virq: u32) {
    // Nothing to do
}

static PLIC_DOMAIN_OPS: IrqDomainOps = IrqDomainOps {
    map: Some(plic_irq_map),
    unmap: Some(plic_irq_unmap),
};

// ==================== Public API ====================

/// Initialize PLIC and create the IRQ domain.
pub fn init() {
    // 1. Initialize PLIC hardware
    PLIC.init();

    // 2. Create the PLIC IRQ domain (linear, 1:1 mapping)
    let domain = irq_domain_create_linear(
        &PLIC_DOMAIN_OPS,
        MAX_INTERRUPTS,
        PLIC_BASE,
        Some(&PLIC_CHIP),
    );

    // 3. Pre-map all PLIC IRQs (1:1 identity mapping)
    for hwirq in 1..MAX_INTERRUPTS {
        irq_create_mapping(domain, hwirq as u32);
    }

    // 4. Enable IPI interrupts for all harts (special case:
    //    IPI handlers are registered early, before request_irq)
    for hart in 0..4 {
        for ipi_irq in 11..14 {
            PLIC.enable_interrupt(hart, ipi_irq);
        }
    }
}

pub fn claim(hart: usize) -> Option<usize> {
    PLIC.claim(hart)
}

pub fn complete(hart: usize, irq: usize) {
    PLIC.complete(hart, irq)
}

pub fn enable_interrupt(hart: usize, irq: usize) {
    PLIC.enable_interrupt(hart, irq)
}

pub fn read_pending() -> u32 {
    PLIC.read_pending()
}

pub fn trigger_ipi(irq: usize) {
    PLIC.trigger_ipi(irq)
}
