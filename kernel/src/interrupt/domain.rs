//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IRQ domain management
//!
//! Maps hardware IRQ numbers to virtual IRQ numbers.
//! Phase 1: linear 1:1 mapping for PLIC.

use core::sync::atomic::{AtomicU32, Ordering};
use crate::sync::spinlock::Spinlock;

use crate::config::PLIC_MAX_INTERRUPTS;
use super::irqchip::IrqChip;
use super::irqdesc::{irq_to_desc, IrqData, handle_fasteoi_irq};

/// Maximum number of IRQs
const MAX_IRQS: usize = PLIC_MAX_INTERRUPTS;

/// IRQ domain operation table (function-pointer-table pattern).
pub struct IrqDomainOps {
    /// Called when a hardware IRQ is mapped to a virtual IRQ.
    /// Sets up irq_data (chip, chip_data) for this mapping.
    pub map: Option<fn(domain: &IrqDomain, virq: u32, hwirq: u32) -> i32>,

    /// Called when a mapping is torn down.
    pub unmap: Option<fn(domain: &IrqDomain, virq: u32)>,
}

/// Interrupt domain.
/// Maps hardware IRQ numbers to virtual IRQ numbers.
/// Phase 1: simple linear 1:1 mapping (hwirq == virq).
pub struct IrqDomain {
    /// Operation table
    pub ops: &'static IrqDomainOps,
    /// Linear reverse-map: hwirq → virq.
    /// u32::MAX means unmapped.
    pub revmap: [AtomicU32; MAX_IRQS],
    /// Number of IRQs in this domain
    pub size: usize,
    /// Opaque host data (e.g., PLIC base address)
    pub host_data: usize,
    /// The irq_chip for this domain
    pub chip: Option<&'static IrqChip>,
}

impl IrqDomain {
    pub const fn new(
        ops: &'static IrqDomainOps,
        size: usize,
        host_data: usize,
        chip: Option<&'static IrqChip>,
    ) -> Self {
        Self {
            ops,
            revmap: [const { AtomicU32::new(u32::MAX) }; MAX_IRQS],
            size,
            host_data,
            chip,
        }
    }
}

/// The default (root) IRQ domain. Set during PLIC initialization.
static DEFAULT_DOMAIN: Spinlock<Option<&'static IrqDomain>> = Spinlock::new(None);

/// Storage for the PLIC domain instance.
static mut PLIC_DOMAIN: Option<IrqDomain> = None;

/// Create a linear IRQ domain with 1:1 hwirq→virq mapping.
///
/// # Arguments
/// - `ops`: Domain operation table
/// - `size`: Number of IRQs
/// - `host_data`: Opaque value for chip use
/// - `chip`: The irq_chip for this domain
pub fn irq_domain_create_linear(
    ops: &'static IrqDomainOps,
    size: usize,
    host_data: usize,
    chip: Option<&'static IrqChip>,
) -> &'static IrqDomain {
    unsafe {
        PLIC_DOMAIN = Some(IrqDomain::new(ops, size, host_data, chip));
        let domain = PLIC_DOMAIN.as_ref().unwrap();
        *DEFAULT_DOMAIN.lock_irqsave() = Some(domain);
        domain
    }
}

/// Get the default IRQ domain.
pub fn get_default_domain() -> Option<&'static IrqDomain> {
    *DEFAULT_DOMAIN.lock_irqsave()
}

/// Create a mapping from hwirq to virq in the domain.
/// For Phase 1 linear mapping: hwirq == virq (identity).
/// Calls domain->ops.map to set up irq_data.
pub fn irq_create_mapping(domain: &IrqDomain, hwirq: u32) -> u32 {
    if (hwirq as usize) >= domain.size {
        return u32::MAX;
    }

    // Phase 1: identity mapping
    let virq = hwirq;
    domain.revmap[hwirq as usize].store(virq, Ordering::Release);

    // Set chip in irq_desc (irqsafe: same lock taken in IRQ dispatch)
    if let Some(desc) = irq_to_desc(virq) {
        let mut irq_data = desc.irq_data.lock_irqsave();
        irq_data.hwirq = hwirq;
        irq_data.chip = domain.chip;
        irq_data.chip_data = domain.host_data;
    }

    // Call domain ops->map
    if let Some(map) = domain.ops.map {
        map(domain, virq, hwirq);
    }

    virq
}

/// Look up an IRQ in a domain's reverse map and dispatch it.
/// This is the main entry point called from trap.rs.
///
/// Flow:
/// 1. Look up hwirq in revmap → virq
/// 2. Call handle_fasteoi_irq(virq)
pub fn generic_handle_domain_irq(domain: &IrqDomain, hwirq: u32) {
    if (hwirq as usize) >= domain.size {
        return;
    }

    let virq = domain.revmap[hwirq as usize].load(Ordering::Acquire);
    if virq == u32::MAX {
        // Unmapped IRQ — spurious
        return;
    }

    handle_fasteoi_irq(virq);
}
