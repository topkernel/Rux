//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for IRQ domain identity mapping and revmap lookup.
//! Copied from: kernel/src/interrupt/domain.rs

use proptest::prelude::*;

const UNMAPPED: u32 = u32::MAX;

// Simplified IrqDomain: Vec instead of AtomicU32 array
pub struct IrqDomain {
    size: usize,
    revmap: Vec<u32>,  // hwirq -> virq mapping
}

impl IrqDomain {
    pub fn new(size: usize) -> Self {
        Self {
            size,
            revmap: vec![UNMAPPED; size],
        }
    }

    // irq_create_mapping: identity mapping (hwirq == virq)
    // Returns u32::MAX if hwirq >= size
    pub fn create_mapping(&mut self, hwirq: u32) -> u32 {
        if (hwirq as usize) >= self.size {
            return UNMAPPED;
        }
        let virq = hwirq; // Phase 1: identity mapping
        self.revmap[hwirq as usize] = virq;
        virq
    }

    // generic_handle_domain_irq: look up revmap and return virq
    // Returns None if unmapped or out of range
    pub fn handle_irq(&self, hwirq: u32) -> Option<u32> {
        if (hwirq as usize) >= self.size {
            return None;
        }
        let virq = self.revmap[hwirq as usize];
        if virq == UNMAPPED {
            None
        } else {
            Some(virq)
        }
    }
}

proptest! {
    #[test]
    fn test_identity_mapping(size in 1usize..256usize, hwirq in 0u32..255u32) {
        let mut domain = IrqDomain::new(size);
        let result = domain.create_mapping(hwirq);
        if (hwirq as usize) < size {
            assert_eq!(result, hwirq, "identity mapping should return hwirq");
        } else {
            assert_eq!(result, UNMAPPED, "out of range should return UNMAPPED");
        }
    }

    #[test]
    fn test_out_of_range_returns_unmapped(size in 1usize..100usize) {
        let mut domain = IrqDomain::new(size);
        let hwirq = size as u32;
        assert_eq!(domain.create_mapping(hwirq), UNMAPPED);
        // Also test larger values
        assert_eq!(domain.create_mapping(u32::MAX), UNMAPPED);
    }

    #[test]
    fn test_revmap_lookup_after_mapping(size in 1usize..256usize, hwirq in 0u32..255u32) {
        let mut domain = IrqDomain::new(size);
        if (hwirq as usize) < size {
            domain.create_mapping(hwirq);
            assert_eq!(domain.handle_irq(hwirq), Some(hwirq));
        }
    }

    #[test]
    fn test_unmapped_lookup_returns_none(size in 1usize..256usize, hwirq in 0u32..255u32) {
        let domain = IrqDomain::new(size);
        if (hwirq as usize) < size {
            // No mapping created → None
            assert_eq!(domain.handle_irq(hwirq), None);
        }
    }

    #[test]
    fn test_handle_irq_out_of_range(size in 1usize..100usize) {
        let domain = IrqDomain::new(size);
        let hwirq = size as u32;
        assert_eq!(domain.handle_irq(hwirq), None);
    }

    #[test]
    fn test_mapping_is_idempotent(size in 1usize..256usize, hwirq in 0u32..255u32) {
        let mut domain = IrqDomain::new(size);
        if (hwirq as usize) < size {
            let r1 = domain.create_mapping(hwirq);
            let r2 = domain.create_mapping(hwirq);
            assert_eq!(r1, hwirq);
            assert_eq!(r2, hwirq);
            assert_eq!(domain.handle_irq(hwirq), Some(hwirq));
        }
    }

    #[test]
    fn test_multiple_mappings(size in 10usize..256usize) {
        let mut domain = IrqDomain::new(size);
        // Map all IRQs
        for hwirq in 0..(size as u32) {
            assert_eq!(domain.create_mapping(hwirq), hwirq);
        }
        // Verify all lookups
        for hwirq in 0..(size as u32) {
            assert_eq!(domain.handle_irq(hwirq), Some(hwirq));
        }
    }

    #[test]
    fn test_zero_size_domain(_v in 0u8..1u8) {
        let mut domain = IrqDomain::new(0);
        assert_eq!(domain.create_mapping(0), UNMAPPED);
        assert_eq!(domain.handle_irq(0), None);
    }

    #[test]
    fn test_initial_revmap_all_unmapped(size in 1usize..256usize) {
        let domain = IrqDomain::new(size);
        for i in 0..size {
            assert_eq!(domain.revmap[i], UNMAPPED);
        }
    }
}
