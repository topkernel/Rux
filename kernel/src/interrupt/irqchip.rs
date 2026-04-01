//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IRQ chip abstraction
//!
//! Function-pointer-table pattern (like BlockDeviceOps, INodeOps).
//! The PLIC driver provides the first implementation.

use super::irqdesc::IrqData;

/// Interrupt controller operation table.
///
/// Each interrupt controller (PLIC, future AIA) provides one static instance.
#[repr(C)]
pub struct IrqChip {
    /// Human-readable name (e.g., "riscv-plic")
    pub name: &'static str,

    /// Mask (disable) an interrupt source
    pub irq_mask: Option<fn(data: &IrqData)>,

    /// Unmask (enable) an interrupt source
    pub irq_unmask: Option<fn(data: &IrqData)>,

    /// Acknowledge an interrupt (edge-triggered)
    pub irq_ack: Option<fn(data: &IrqData)>,

    /// Signal end-of-interrupt (e.g., PLIC complete)
    pub irq_eoi: Option<fn(data: &IrqData)>,

    /// Set interrupt trigger type (edge/level)
    pub irq_set_type: Option<fn(data: &IrqData, flow_type: u32) -> i32>,

    /// Set interrupt affinity to a specific CPU
    pub irq_set_affinity: Option<fn(data: &IrqData, cpu_mask: u64) -> i32>,
}
