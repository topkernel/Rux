//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IRQ framework
//!
//! Interrupt management:
//! - irq_desc: per-IRQ descriptor with action chain
//! - irq_chip: hardware interrupt controller abstraction
//! - irq_domain: hardware-to-virtual IRQ number mapping
//! - preempt: preempt_count, irq_enter/irq_exit, nmi_enter/nmi_exit
//! - softirq: bottom-half deferred work framework
//! - tasklet: dynamic deferred work on top of softirq
//! - ksoftirqd: per-CPU kernel thread for softirq overflow
//! - request_irq/free_irq: handler registration API
//! - request_nmi/free_nmi: NMI handler registration API

pub mod irqdesc;
pub mod irqchip;
pub mod domain;
pub mod preempt;
pub mod softirq;
pub mod tasklet;
pub mod ksoftirqd;

// Re-export commonly used types and functions
pub use irqdesc::{
    IrqReturn, IrqAction, IrqData,
    request_irq, free_irq, irq_to_desc,
    irq_inc_count, irq_get_count, irq_get_name,
    handle_fasteoi_irq, IRQF_SHARED,
    request_nmi, free_nmi, handle_fasteoi_nmi,
    arch_trigger_cpumask_backtrace,
};
pub use irqchip::IrqChip;
pub use preempt::{
    preempt_count, in_interrupt, in_irq, in_softirq, in_task, preemptible,
    preempt_count_add, preempt_count_sub, irq_enter, irq_exit,
    nmi_enter, nmi_exit, in_nmi,
    irqentry_nmi_enter, irqentry_nmi_exit,
    PREEMPT_MASK, SOFTIRQ_MASK, HARDIRQ_MASK, NMI_MASK, PREEMPT_ACTIVE,
    PREEMPT_OFFSET, SOFTIRQ_OFFSET, HARDIRQ_OFFSET, NMI_OFFSET,
};
pub use domain::{
    IrqDomain, IrqDomainOps,
    irq_domain_create_linear, get_default_domain,
    irq_create_mapping, generic_handle_domain_irq,
};
pub use softirq::{
    open_softirq, raise_softirq, raise_softirq_irqoff,
    invoke_softirq, __do_softirq, has_pending_softirqs,
    SoftirqHandler, SoftirqIndex, NR_SOFTIRQS,
};
pub use tasklet::{
    TaskletStruct, tasklet_schedule, tasklet_hi_schedule,
    tasklet_kill,
};

/// Initialize the IRQ framework.
/// Must be called once during boot, before driver probe.
pub fn init() {
    irqdesc::init();
    softirq::init();
    crate::sync::rcu::init();
    tasklet::init();
    // ksoftirqd::init() is called later from main.rs after sched::init()
}
