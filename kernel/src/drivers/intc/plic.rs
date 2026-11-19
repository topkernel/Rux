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

// PLIC base address - QEMU virt platform uses 0x0c000000. Accessed via
// the kernel-half MMIO alias (mmio_alias): the identity low window is
// cloned into every user root where a MAP_FIXED user mapping (gjs cage
// at 0x0c000000) may replace it — the alias is user-unreachable.
const PLIC_BASE: usize =
    crate::arch::mm::memory_layout::mmio_alias(201326592) as usize; // alias of 0x0c000000

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
                // INV-MMIO-1: mapped PLIC enable register for this
                // S-mode context (MmioRegion owns the volatile word).
                crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, 0);
            }

            // Set threshold = 0 for this S-mode context
            self.set_threshold(hart, 0);
        }
    }

    fn set_priority(&self, irq: usize, priority: u32) {
        let addr = self.base + offset::PRIORITY + irq * 4;
        // INV-MMIO-1: mapped PLIC priority register (PLIC_BASE + 4*irq).
        crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, priority);
    }

    fn set_threshold(&self, hart: usize, threshold: u32) {
        let ctx = s_mode_ctx(hart);
        let addr = self.base + offset::CONTEXT_BASE + ctx * offset::CONTEXT_SIZE + offset::CONTEXT_THRESHOLD;
        // INV-MMIO-1: mapped PLIC threshold register for this context.
        crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, threshold);
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
        // INV-MMIO-1: mapped PLIC enable register for this S-mode
        // context. Each hart has its own enable context, so no
        // cross-hart race in the current configuration.
        let reg = crate::safety::mmio::MmioRegion::new(addr, 4);
        let value = reg.read_u32(0);
        reg.write_u32(0, value | (1 << bit));
    }

    fn disable_interrupt(&self, hart: usize, irq: usize) {
        let _rmw = ENABLE_RMW_LOCK.lock();
        let ctx = s_mode_ctx(hart);
        let word = irq / 32;
        let bit = irq % 32;
        let addr = self.base + offset::ENABLE_BASE + ctx * offset::ENABLE_SIZE + word * 4;
        // INV-MMIO-1: mapped PLIC enable register; RMW clears one bit.
        let reg = crate::safety::mmio::MmioRegion::new(addr, 4);
        let value = reg.read_u32(0);
        reg.write_u32(0, value & !(1 << bit));
    }

    pub fn claim(&self, hart: usize) -> Option<usize> {
        let ctx = s_mode_ctx(hart);
        let addr = self.base + offset::CONTEXT_BASE + ctx * offset::CONTEXT_SIZE + offset::CONTEXT_CLAIM;
        // INV-MMIO-1: mapped PLIC claim/complete register for this context.
        let irq = crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0);
        if irq == 0 { None } else { Some(irq as usize) }
    }

    pub fn complete(&self, hart: usize, irq: usize) {
        let ctx = s_mode_ctx(hart);
        let addr = self.base + offset::CONTEXT_BASE + ctx * offset::CONTEXT_SIZE + offset::CONTEXT_CLAIM;
        // INV-MMIO-1: writing irq completes the claim.
        crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, irq as u32);
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
        // INV-MMIO-1: mapped PLIC pending register (read-only).
        crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0)
    }

    /// Read the FIRST pending word (IRQs 0..31). See read_pending_word for
    /// the full array.
    pub fn read_pending(&self) -> u32 {
        self.read_pending_word(0)
    }

    pub fn trigger_ipi(&self, irq: usize) {
        if irq >= 32 { return; }
        let addr = self.base + offset::PENDING;
        // INV-MMIO-1: mapped PLIC pending register; RMW sets a pending bit.
        let pending = crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0);
        crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, pending | (1 << irq));
    }
}

/// R14-15 (MED-11): PLIC enable-word read-modify-writes from different
/// CPUs lose updates (one IRQ silently left enabled/disabled). All RMW
/// on enable words goes through this lock.
static ENABLE_RMW_LOCK: crate::sync::spinlock::Spinlock<()> =
    crate::sync::spinlock::Spinlock::new(());

static PLIC: Plic = Plic::new(PLIC_BASE, crate::config::MAX_CPUS);

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
    let hart = crate::arch::smp::cpu_id() as usize;
    PLIC.complete(hart, data.hwirq as usize);
}

/// Post-SMP-bringup delivery restore (the VW-EXT4LOCK wedge root cause).
///
/// OpenSBI's per-hart warm init reprograms every secondary's S-mode PLIC
/// context as that hart wakes (threshold raised to max, enable bits
/// cleared), clobbering the kernel's early-init state on every context
/// except the boot hart's — the boot hart never passes through warm init,
/// so its context alone keeps the enables the kernel's request_irq
/// unmask had set on ALL contexts. Whether external IRQs deliver at all
/// after SMP online then depends on the boot hart's own context: with
/// num_harts hardcoded to 4, a boot hart of 4..7 kept its firmware
/// residue threshold=7 — ZERO external interrupts delivered anywhere
/// (virtio-blk completions AND UART RX). Every async block I/O then
/// waited to its 10 s deadline (the only async harvest fallback), the
/// shell held EXT4_BIG_LOCK across those waits, fdreapd D-parked behind
/// it, and the fed boot script never reached the shell (UART RX IRQ
/// dead): the VW-EXT4LOCK fdreapd-stall hang face, ~4/8 of boots on
/// -smp 8 cells (never on -smp 2: harts 0/1 are always init-covered).
///
/// Reference parity: Linux's sifive-plic driver re-establishes per-CPU
/// enable state through its cpu-online notifier
/// (irq_sifive_plic_cpu_starting). We do the same in one shot: when the
/// bringup window closes, zero every S-mode context's threshold and
/// re-apply the unmask of every registered, enabled IRQ. Idempotent;
/// runs once on the boot hart before userspace starts.
pub fn after_smp_online_restore() {
    for hart in 0..crate::config::MAX_CPUS {
        PLIC.set_threshold(hart, 0);
    }
    let rearmed = crate::interrupt::irqdesc::remask_registered_irqs();
    crate::pr_info!(
        "intc: post-SMP PLIC restore: thresholds=0 on {} S-mode contexts, {} IRQs re-enabled",
        crate::config::MAX_CPUS,
        rearmed
    );
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
    for hart in 0..crate::config::MAX_CPUS {
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

/// VW forensic snapshot: print the PLIC delivery state (VW-PLIC) — the
/// pending array, each S-mode context's enable words, and its threshold.
/// Read-only MMIO, safe to call from watchdog/deadline contexts. Answers
/// the "delivery vs assertion" question for a dead device IRQ: a pending
/// bit that stays set with zero handler entries names a delivery break
/// (enable/threshold); a clear pending bit means the device never
/// asserted (driver/device side).
pub fn vw_plic_dump() {
    let words = (MAX_INTERRUPTS + 31) / 32;
    // Pending array
    let mut pend = [0u32; 16];
    let n = words.min(pend.len());
    for w in 0..n {
        pend[w] = PLIC.read_pending_word(w);
    }
    crate::pr_err!(
        "VW-PLIC pending={:08x}{:08x} harts(maxcpus)={}",
        (pend[1] & 0xfffffffe) | ((pend[0] >> 31) & 1), // IRQs 32..62 (33=blk,32..47=gpex) compacted
        pend[0] & 0x7fffffff,
        crate::config::MAX_CPUS
    );
    // Enable words + threshold per S-mode context
    for hart in 0..crate::config::MAX_CPUS {
        let ctx = s_mode_ctx(hart);
        let addr = self_addr_enable(ctx, 0);
        let addr1 = self_addr_enable(ctx, 1);
        // INV-MMIO-1: mapped PLIC enable register reads (diagnostic).
        let en0 = crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0);
        let en1 = crate::safety::mmio::MmioRegion::new(addr1, 4).read_u32(0);
        let th = read_threshold(ctx);
        crate::pr_err!(
            "VW-PLIC ctx={} (hart {}) enable[0]={:08x} enable[1]={:08x} threshold={}",
            ctx,
            hart,
            en0,
            en1,
            th
        );
    }
}

fn self_addr_enable(ctx: usize, word: usize) -> usize {
    PLIC_BASE + offset::ENABLE_BASE + ctx * offset::ENABLE_SIZE + word * 4
}

fn read_threshold(ctx: usize) -> u32 {
    let addr = PLIC_BASE
        + offset::CONTEXT_BASE
        + ctx * offset::CONTEXT_SIZE
        + offset::CONTEXT_THRESHOLD;
    // INV-MMIO-1: mapped PLIC threshold register read (diagnostic).
    crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0)
}
