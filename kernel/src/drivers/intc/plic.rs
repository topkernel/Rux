//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V PLIC (Platform-Level Interrupt Controller) driver
//!
//! Implements the IrqChip and IrqDomainOps traits for the QEMU virt PLIC.
//!
//! QEMU virt PLIC has two contexts per hart:
//!   Context 2*N   = Hart N, M-mode
//!   Context 2*N+1 = Hart N, S-mode
//!
//! All register accesses use the S-mode context (2*hart + 1).

use core::arch::asm;
use crate::println;
use crate::interrupt::{
    IrqChip, IrqData, IrqDomainOps, IrqDomain,
    irq_domain_create_linear, irq_create_mapping,
};

// PLIC base address - QEMU virt platform uses 0x0c000000
const PLIC_BASE: usize = 201326592;  // 0x0c000000 in decimal

mod offset {
    pub const PRIORITY: usize = 0x000000;
    pub const PENDING: usize = 0x001000;
    pub const ENABLE_BASE: usize = 0x002000;
    pub const ENABLE_SIZE: usize = 0x80;        // per context
    pub const CONTEXT_BASE: usize = 0x200000;
    pub const CONTEXT_SIZE: usize = 0x1000;      // per context
    pub const CONTEXT_THRESHOLD: usize = 0x00;
    pub const CONTEXT_CLAIM: usize = 0x04;
}

/// Maximum number of interrupts - from config
pub const MAX_INTERRUPTS: usize = crate::config::PLIC_MAX_INTERRUPTS;

pub const PLIC_PRIORITY_BASE: u32 = 1;
pub const PLIC_PRIORITY_MIN: u32 = 0;
pub const PLIC_PRIORITY_MAX: u32 = 7;

/// S-mode context ID for a given hart (2 * hart + 1)
fn s_mode_ctx(hart: usize) -> usize {
    2 * hart + 1
}

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

        // Set threshold = 0 for all S-mode contexts and clear enables
        for hart in 0..self.num_harts {
            let ctx = s_mode_ctx(hart);

            // Clear all enable bits for this S-mode context
            for word in 0..((MAX_INTERRUPTS + 31) / 32) {
                let addr = self.base + offset::ENABLE_BASE + ctx * offset::ENABLE_SIZE + word * 4;
                // SAFETY: addr is a valid PLIC enable register for the S-mode context.
                unsafe {
                    asm!("sw zero, 0(a0)", in("a0") addr, options(nostack));
                }
            }

            // Set threshold = 0 for this S-mode context
            self.set_threshold(hart, 0);
        }
    }

    fn set_priority(&self, irq: usize, priority: u32) {
        let addr = self.base + offset::PRIORITY + irq * 4;
        // SAFETY: addr is a valid PLIC priority register (PLIC_BASE + 4*irq).
        unsafe {
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") priority, options(nostack));
        }
    }

    fn set_threshold(&self, hart: usize, threshold: u32) {
        let ctx = s_mode_ctx(hart);
        let addr = self.base + offset::CONTEXT_BASE + ctx * offset::CONTEXT_SIZE + offset::CONTEXT_THRESHOLD;
        // SAFETY: addr is a valid PLIC threshold register for the S-mode context.
        unsafe {
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") threshold, options(nostack));
        }
    }

    /// Enable an interrupt for a given hart.
    ///
    /// NOTE: The read-modify-write on the enable register is not atomic.
    /// Safe under current single-hart-per-context setup (each hart has its
    /// own enable register set). For SMP where multiple harts share a
    /// context, this needs AMO (`amoadd.w`) or a spinlock.
    pub fn enable_interrupt(&self, hart: usize, irq: usize) {
        self.set_priority(irq, PLIC_PRIORITY_BASE);
        let ctx = s_mode_ctx(hart);
        let word = irq / 32;
        let bit = irq % 32;
        let addr = self.base + offset::ENABLE_BASE + ctx * offset::ENABLE_SIZE + word * 4;
        // SAFETY: addr is a valid PLIC enable register for the S-mode context.
        // Each hart has its own enable context, so no cross-hart race in
        // the current configuration.
        unsafe {
            let value: u32;
            asm!("lw {}, 0({})", out(reg) value, in(reg) addr, options(nostack));
            let new_value = value | (1 << bit);
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") new_value, options(nostack));
        }
    }

    fn disable_interrupt(&self, hart: usize, irq: usize) {
        let ctx = s_mode_ctx(hart);
        let word = irq / 32;
        let bit = irq % 32;
        let addr = self.base + offset::ENABLE_BASE + ctx * offset::ENABLE_SIZE + word * 4;
        // SAFETY: addr is a valid PLIC enable register; read-modify-write to clear one bit.
        unsafe {
            let value: u32;
            asm!("lw {}, 0({})", out(reg) value, in(reg) addr, options(nostack));
            let new_value = value & !(1 << bit);
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") new_value, options(nostack));
        }
    }

    pub fn claim(&self, hart: usize) -> Option<usize> {
        let ctx = s_mode_ctx(hart);
        let addr = self.base + offset::CONTEXT_BASE + ctx * offset::CONTEXT_SIZE + offset::CONTEXT_CLAIM;
        // SAFETY: addr is a valid PLIC claim/complete register for the S-mode context.
        unsafe {
            let irq: u32;
            asm!("lw {}, 0({})", out(reg) irq, in(reg) addr, options(nostack));
            if irq == 0 { None } else { Some(irq as usize) }
        }
    }

    pub fn complete(&self, hart: usize, irq: usize) {
        let ctx = s_mode_ctx(hart);
        let addr = self.base + offset::CONTEXT_BASE + ctx * offset::CONTEXT_SIZE + offset::CONTEXT_CLAIM;
        // SAFETY: addr is a valid PLIC claim/complete register; writing irq completes the claim.
        unsafe {
            asm!("sw t1, 0(a0)", in("a0") addr, in("t1") irq as u32, options(nostack));
        }
    }

    pub fn read_pending(&self) -> u32 {
        let addr = self.base + offset::PENDING;
        // SAFETY: addr is a valid PLIC pending register (read-only).
        unsafe {
            let pending: u32;
            asm!("lw {}, 0({})", out(reg) pending, in(reg) addr, options(nostack));
            pending
        }
    }

    pub fn trigger_ipi(&self, irq: usize) {
        if irq >= 32 { return; }
        let addr = self.base + offset::PENDING;
        // SAFETY: addr is a valid PLIC pending register; read-modify-write to set a pending bit.
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
    for hart in 0..crate::config::MAX_CPUS {
        PLIC.disable_interrupt(hart, data.hwirq as usize);
    }
}

fn plic_unmask(data: &IrqData) {
    // Only enable the IRQ on the boot hart.
    // PLIC delivers a pending IRQ to ALL harts that have it enabled.
    // If enabled on multiple harts, they all claim the same IRQ and
    // contend on the irq_desc action lock → deadlock.
    // TODO: implement proper IRQ affinity (set_affinity) to distribute.
    let hart = crate::arch::riscv64::smp::boot_hart_id() as usize;
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

    // 4. Enable IPI interrupts for all harts (S-mode contexts)
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
