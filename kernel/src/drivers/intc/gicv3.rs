//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! GICv3 interrupt controller driver — stage-3 stub.
//!
//! QEMU virt with `-cpu cortex-a72` defaults to GICv3 (the DTB `/intc`
//! node is authoritative — parse `compatible = "arm,gic-v3"` at stage-3
//! bring-up). Until then: init() and enable_cpu_interface() are no-ops
//! so the boot path proceeds with interrupts masked at the DAIF level
//! (stage-1/2 boots never unmask IRQs).
//!
//! Stage-3 work items (see docs/development/aarch64-port-plan.md §7):
//! - distributor (GICD_*): disable, group defaults, SPI routing
//! - redistributors (GICR_*): per-CPU wake, enable
//! - CPU interface (ICC_* + SRE): system register enable path
//! - PPI wiring: virtual timer (INTID 27), PL011 (SPI), virtio-mmio (SPI)
//! - SGI generation (ICC_SGI1R) for the ipi module

/// GICv3 init — stage 3 (no-op until the distributor is mapped)
pub fn init() {
    crate::pr_info!("intc: GICv3 bring-up lands in stage 3 (interrupts stay DAIF-masked)");
}

/// Enable the CPU interface (unmask IRQs at the GIC level) — stage 3
pub fn enable_cpu_interface() {
    // Stage 3: ICC_PMR_EL1 = 0xFF; ICC_IGRPEN1_EL1 = 1
}
