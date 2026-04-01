//! IRQ framework
//!
//! Linux-compatible interrupt management:
//! - irq_desc: per-IRQ descriptor with action chain
//! - irq_chip: hardware interrupt controller abstraction
//! - irq_domain: hardware-to-virtual IRQ number mapping
//! - request_irq/free_irq: handler registration API

pub mod irqdesc;
pub mod irqchip;
pub mod domain;
pub mod preempt;

// Re-export commonly used types and functions
pub use irqdesc::{
    IrqReturn, IrqAction, IrqData,
    request_irq, free_irq, irq_to_desc,
    irq_inc_count, irq_get_count, irq_get_name,
    handle_fasteoi_irq, IRQF_SHARED,
};
pub use irqchip::IrqChip;
pub use preempt::{
    preempt_count, in_interrupt, in_irq, in_softirq, in_task, preemptible,
    preempt_count_add, preempt_count_sub, irq_enter, irq_exit,
    PREEMPT_MASK, SOFTIRQ_MASK, HARDIRQ_MASK, NMI_MASK, PREEMPT_ACTIVE,
    PREEMPT_OFFSET, SOFTIRQ_OFFSET, HARDIRQ_OFFSET, NMI_OFFSET,
};
pub use domain::{
    IrqDomain, IrqDomainOps,
    irq_domain_create_linear, get_default_domain,
    irq_create_mapping, generic_handle_domain_irq,
};

/// Initialize the IRQ framework.
/// Must be called once during boot, before driver probe.
pub fn init() {
    irqdesc::init();
}
