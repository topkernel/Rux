//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IRQ descriptor and return value invariant tests.
//!
//! Types copied from: kernel/src/interrupt/irqdesc.rs
//! NOTE: Spinlock/Atomic types simplified for std testing.

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/interrupt/irqdesc.rs
// ============================================================================

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqReturn {
    None = 0,
    Handled = 1,
    WakeThread = 2,
}

pub const IRQF_SHARED: u32 = 0x00000001;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IrqData {
    pub irq: u32,
    pub hwirq: u32,
    /// chip and chip_data are Option<usize> for testing (real: Option<&IrqChip>, usize)
    pub chip: usize,
    pub chip_data: usize,
}

impl IrqData {
    pub const fn new(irq: u32) -> Self {
        Self {
            irq,
            hwirq: irq,
            chip: 0,
            chip_data: 0,
        }
    }
}

/// Simplified IrqDesc for testing (no Spinlock/Atomic)
pub struct IrqDesc {
    pub depth: u32,
    pub per_cpu_count: Vec<u64>,
}

impl IrqDesc {
    pub const fn new_const() -> Self {
        Self {
            depth: 0,
            per_cpu_count: Vec::new(),
        }
    }

    pub fn new(max_cpus: usize) -> Self {
        Self {
            depth: 0,
            per_cpu_count: vec![0u64; max_cpus],
        }
    }

    pub fn inc_count(&mut self, cpu: usize) {
        if cpu < self.per_cpu_count.len() {
            self.per_cpu_count[cpu] += 1;
        }
    }

    pub fn get_count(&self, cpu: usize) -> u64 {
        if cpu < self.per_cpu_count.len() {
            self.per_cpu_count[cpu]
        } else {
            0
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-IRQ-1: IrqReturn equality works correctly
    #[test]
    fn test_irq_return_equality(_v in 0u8..1u8) {
        prop_assert_eq!(IrqReturn::None, IrqReturn::None);
        prop_assert_eq!(IrqReturn::Handled, IrqReturn::Handled);
        prop_assert_eq!(IrqReturn::WakeThread, IrqReturn::WakeThread);
        prop_assert_ne!(IrqReturn::None, IrqReturn::Handled);
        prop_assert_ne!(IrqReturn::Handled, IrqReturn::WakeThread);
        prop_assert_ne!(IrqReturn::None, IrqReturn::WakeThread);
    }

    /// INV-IRQ-2: IrqData::new sets irq == hwirq, chip=0, chip_data=0
    #[test]
    fn test_irq_data_new(irq in 0u32..256u32) {
        let data = IrqData::new(irq);
        prop_assert_eq!(data.irq, irq);
        prop_assert_eq!(data.hwirq, irq);
        prop_assert_eq!(data.chip, 0);
        prop_assert_eq!(data.chip_data, 0);
    }

    /// INV-IRQ-3: IrqData::new(0) creates zeroed data
    #[test]
    fn test_irq_data_new_zero(_v in 0u8..1u8) {
        let data = IrqData::new(0);
        prop_assert_eq!(data.irq, 0);
        prop_assert_eq!(data.hwirq, 0);
    }

    /// INV-IRQ-4: IrqDesc::new has depth=0 and all counts=0
    #[test]
    fn test_irq_desc_new(max_cpus in 1usize..8usize) {
        let desc = IrqDesc::new(max_cpus);
        prop_assert_eq!(desc.depth, 0);
        for cpu in 0..max_cpus {
            prop_assert_eq!(desc.get_count(cpu), 0);
        }
    }

    /// INV-IRQ-5: inc_count increments specific CPU counter
    #[test]
    fn test_inc_count(
        max_cpus in 2usize..8usize,
        cpu in 0usize..8usize,
        steps in 1usize..100usize,
    ) {
        let mut desc = IrqDesc::new(max_cpus);
        for _ in 0..steps {
            desc.inc_count(cpu);
        }
        if cpu < max_cpus {
            prop_assert_eq!(desc.get_count(cpu), steps as u64);
        } else {
            // Out of range: no effect
            prop_assert_eq!(desc.get_count(cpu), 0);
        }
    }

    /// INV-IRQ-6: inc_count on one CPU doesn't affect others
    #[test]
    fn test_inc_count_isolated(
        max_cpus in 3usize..8usize,
        cpu1 in 0usize..8usize,
        cpu2 in 0usize..8usize,
    ) {
        if cpu1 == cpu2 || cpu1 >= 8 || cpu2 >= 8 {
            return Ok(());
        }
        let min_cpus = if cpu1 > cpu2 { cpu1 + 1 } else { cpu2 + 1 };
        let max_cpus = max_cpus.max(min_cpus).min(8);
        let mut desc = IrqDesc::new(max_cpus);
        desc.inc_count(cpu1);
        if cpu2 < max_cpus {
            prop_assert_eq!(desc.get_count(cpu2), 0);
        }
        if cpu1 < max_cpus {
            prop_assert_eq!(desc.get_count(cpu1), 1);
        }
    }

    /// INV-IRQ-7: IRQF_SHARED is bit 0
    #[test]
    fn test_irqf_shared(_v in 0u8..1u8) {
        prop_assert_eq!(IRQF_SHARED, 1);
        prop_assert_eq!(IRQF_SHARED & (IRQF_SHARED - 1), 0);
    }

    /// INV-IRQ-8: IrqReturn discriminants are 0, 1, 2
    #[test]
    fn test_irq_return_discriminants(_v in 0u8..1u8) {
        prop_assert_eq!(IrqReturn::None as u32, 0);
        prop_assert_eq!(IrqReturn::Handled as u32, 1);
        prop_assert_eq!(IrqReturn::WakeThread as u32, 2);
    }

    /// INV-IRQ-9: IrqData::new is deterministic
    #[test]
    fn test_irq_data_deterministic(irq in 0u32..1000u32) {
        let d1 = IrqData::new(irq);
        let d2 = IrqData::new(irq);
        prop_assert_eq!(d1.irq, d2.irq);
        prop_assert_eq!(d1.hwirq, d2.hwirq);
    }

    /// INV-IRQ-10: get_count out of range returns 0
    #[test]
    fn test_get_count_out_of_range(
        max_cpus in 1usize..4usize,
        cpu in 4usize..100usize,
    ) {
        let desc = IrqDesc::new(max_cpus);
        prop_assert_eq!(desc.get_count(cpu), 0);
    }

    /// INV-IRQ-11: depth field can be read and modified
    #[test]
    fn test_depth_field(depth in 0u32..100u32) {
        let mut desc = IrqDesc::new(4);
        desc.depth = depth;
        prop_assert_eq!(desc.depth, depth);
    }

    /// INV-IRQ-12: IrqData copy preserves all fields
    #[test]
    fn test_irq_data_copy(irq in 0u32..256u32) {
        let data = IrqData::new(irq);
        let copy = data;
        prop_assert_eq!(copy.irq, data.irq);
        prop_assert_eq!(copy.hwirq, data.hwirq);
        prop_assert_eq!(copy.chip, data.chip);
        prop_assert_eq!(copy.chip_data, data.chip_data);
    }
}
