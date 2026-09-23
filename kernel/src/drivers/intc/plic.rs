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

/// Basic priority layering (review LINUX-DIFF: every IRQ used to get
/// priority 1, so the PLIC could never prefer the console over bulk device
/// work). QEMU virt IRQ map: 1..=8 are the virtio-mmio slots, 10 is the
/// UART console. Console/interactive sources outrank block/net throughput
/// sources; everything else stays at the base priority.
fn default_priority_for(irq: usize) -> u32 {
    match irq {
        10 => 5,            // UART console — keep interactive latency low
        1..=8 => 3,         // virtio-mmio (blk/net/input/gpu)
        _ => PLIC_PRIORITY_BASE,
    }
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
        let _rmw = ENABLE_RMW_LOCK.lock();
        self.set_priority(irq, default_priority_for(irq));
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
        let _rmw = ENABLE_RMW_LOCK.lock();
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

    /// Read one word of the PLIC pending array.
    ///
    /// The pending array at offset::PENDING has one 32-bit word per 32
    /// IRQs (MAX_INTERRUPTS/32 words total). The old `read_pending` only
    /// ever read word 0, so IRQs >= 32 were invisible to any diagnostics
    /// (review BUG: read_pending 全字).
    pub fn read_pending_word(&self, word: usize) -> u32 {
        let words = (MAX_INTERRUPTS + 31) / 32;
        if word >= words {
            return 0;
        }
        let addr = self.base + offset::PENDING + word * 4;
        // SAFETY: addr is a valid PLIC pending register (read-only).
        unsafe {
            let pending: u32;
            asm!("lw {}, 0({})", out(reg) pending, in(reg) addr, options(nostack));
            pending
        }
    }

    /// Read the FIRST pending word (IRQs 0..31). See read_pending_word for
    /// the full array.
    pub fn read_pending(&self) -> u32 {
        self.read_pending_word(0)
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

/// R14-15 (MED-11): PLIC enable-word read-modify-writes from different
/// CPUs lose updates (one IRQ silently left enabled/disabled). All RMW
/// on enable words goes through this lock.
static ENABLE_RMW_LOCK: crate::sync::spinlock::Spinlock<()> =
    crate::sync::spinlock::Spinlock::new(());

static PLIC: Plic = Plic::new(PLIC_BASE, 4);

// ==================== IrqChip implementation ====================

fn plic_mask(data: &IrqData) {
    for hart in 0..crate::config::MAX_CPUS {
        PLIC.disable_interrupt(hart, data.hwirq as usize);
    }
}

fn plic_unmask(data: &IrqData) {
    // Multi-core enable (review LINUX-DIFF): the old code enabled the IRQ
    // on the boot hart ONLY, serially funneling every external interrupt
    // onto hart 0 and starving the other CPUs. PLIC claim semantics make
    // multi-hart enable safe: when the IRQ fires, every enabled hart traps
    // and claims, the hardware hands the IRQ to exactly ONE claimer and
    // returns 0 to the rest (the old comment's "all claim the same IRQ and
    // deadlock on the action lock" scenario cannot happen) — racing claims
    // are precisely how interrupt load spreads across CPUs.
    // Enabling an offline hart's context is harmless (it never claims).
    for hart in 0..crate::config::MAX_CPUS {
        PLIC.enable_interrupt(hart, data.hwirq as usize);
    }
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
