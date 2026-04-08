//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for preempt counter bitfield masks and context queries.
//! Copied from: kernel/src/interrupt/preempt.rs

use proptest::prelude::*;

// Copied preempt constants
pub const PREEMPT_MASK: i32 = 0x0000_00FF;
pub const SOFTIRQ_MASK: i32 = 0x0000_FF00;
pub const HARDIRQ_MASK: i32 = 0x000F_0000;
pub const NMI_MASK: i32 = 0x0010_0000;
pub const PREEMPT_ACTIVE: i32 = 0x0400_0000;

pub const PREEMPT_OFFSET: i32 = 1;
pub const SOFTIRQ_OFFSET: i32 = 1 << 8;
pub const HARDIRQ_OFFSET: i32 = 1 << 16;
pub const NMI_OFFSET: i32 = 1 << 20;

// Copied query functions (operating on a given preempt_count value)
pub fn in_interrupt(pc: i32) -> bool {
    (pc & (HARDIRQ_MASK | SOFTIRQ_MASK | NMI_MASK)) != 0
}

pub fn in_irq(pc: i32) -> bool {
    (pc & HARDIRQ_MASK) != 0
}

pub fn in_softirq(pc: i32) -> bool {
    (pc & SOFTIRQ_MASK) != 0
}

pub fn in_nmi(pc: i32) -> bool {
    (pc & NMI_MASK) != 0
}

pub fn in_task(pc: i32) -> bool {
    !in_interrupt(pc)
}

pub fn preemptible(pc: i32) -> bool {
    pc == 0
}

proptest! {
    #[test]
    fn test_masks_non_overlapping(_v in 0u8..1u8) {
        let masks = [PREEMPT_MASK, SOFTIRQ_MASK, HARDIRQ_MASK, NMI_MASK];
        for i in 0..masks.len() {
            for j in (i+1)..masks.len() {
                assert_eq!(masks[i] & masks[j], 0,
                    "masks[{}] and masks[{}] overlap", i, j);
            }
        }
    }

    #[test]
    fn test_preempt_active_no_overlap(_v in 0u8..1u8) {
        assert_eq!(PREEMPT_ACTIVE & PREEMPT_MASK, 0);
        assert_eq!(PREEMPT_ACTIVE & SOFTIRQ_MASK, 0);
        assert_eq!(PREEMPT_ACTIVE & HARDIRQ_MASK, 0);
        assert_eq!(PREEMPT_ACTIVE & NMI_MASK, 0);
    }

    #[test]
    fn test_offsets_in_masks(_v in 0u8..1u8) {
        assert_eq!(PREEMPT_OFFSET & PREEMPT_MASK, PREEMPT_OFFSET);
        assert_ne!(PREEMPT_OFFSET & PREEMPT_MASK, 0);
        assert_eq!(SOFTIRQ_OFFSET & SOFTIRQ_MASK, SOFTIRQ_OFFSET);
        assert_ne!(SOFTIRQ_OFFSET & SOFTIRQ_MASK, 0);
        assert_eq!(HARDIRQ_OFFSET & HARDIRQ_MASK, HARDIRQ_OFFSET);
        assert_ne!(HARDIRQ_OFFSET & HARDIRQ_MASK, 0);
        assert_eq!(NMI_OFFSET & NMI_MASK, NMI_OFFSET);
        assert_ne!(NMI_OFFSET & NMI_MASK, 0);
    }

    #[test]
    fn test_offset_not_in_other_masks(_v in 0u8..1u8) {
        // Each offset only belongs to its own mask
        assert_eq!(PREEMPT_OFFSET & SOFTIRQ_MASK, 0);
        assert_eq!(PREEMPT_OFFSET & HARDIRQ_MASK, 0);
        assert_eq!(PREEMPT_OFFSET & NMI_MASK, 0);

        assert_eq!(SOFTIRQ_OFFSET & PREEMPT_MASK, 0);
        assert_eq!(SOFTIRQ_OFFSET & HARDIRQ_MASK, 0);
        assert_eq!(SOFTIRQ_OFFSET & NMI_MASK, 0);

        assert_eq!(HARDIRQ_OFFSET & PREEMPT_MASK, 0);
        assert_eq!(HARDIRQ_OFFSET & SOFTIRQ_MASK, 0);
        assert_eq!(HARDIRQ_OFFSET & NMI_MASK, 0);

        assert_eq!(NMI_OFFSET & PREEMPT_MASK, 0);
        assert_eq!(NMI_OFFSET & SOFTIRQ_MASK, 0);
        assert_eq!(NMI_OFFSET & HARDIRQ_MASK, 0);
    }

    #[test]
    fn test_in_task_complement(pc in 0i32..0x0500_0000i32) {
        assert_eq!(in_task(pc), !in_interrupt(pc));
    }

    #[test]
    fn test_interrupt_decomposition(pc in 0i32..0x0500_0000i32) {
        // in_interrupt == in_irq || in_softirq || in_nmi
        let by_parts = in_irq(pc) || in_softirq(pc) || in_nmi(pc);
        assert_eq!(in_interrupt(pc), by_parts);
    }

    #[test]
    fn test_preemptible_only_at_zero(_v in 0u8..1u8) {
        assert!(preemptible(0));
        assert!(!preemptible(1));
        assert!(!preemptible(PREEMPT_OFFSET));
        assert!(!preemptible(SOFTIRQ_OFFSET));
        assert!(!preemptible(HARDIRQ_OFFSET));
        assert!(!preemptible(NMI_OFFSET));
        assert!(!preemptible(PREEMPT_ACTIVE));
    }

    #[test]
    fn test_preemptible_any_nonzero(pc in 1i32..0x0500_0000i32) {
        assert!(!preemptible(pc));
    }

    #[test]
    fn test_mask_coverage(_v in 0u8..1u8) {
        // OR of all masks + PREEMPT_ACTIVE covers 0x041FFFFF
        let all = PREEMPT_MASK | SOFTIRQ_MASK | HARDIRQ_MASK | NMI_MASK | PREEMPT_ACTIVE;
        assert_eq!(all, 0x041F_FFFF);
    }

    #[test]
    fn test_irq_enter_exit_symmetry(pc in 0i32..0x000F_0000i32) {
        // Constrain to fit within HARDIRQ_MASK (4 bits at 16-19)
        let after_enter = pc + HARDIRQ_OFFSET;
        assert!(in_irq(after_enter));
        let after_exit = after_enter - HARDIRQ_OFFSET;
        assert_eq!(after_exit, pc);
        assert_eq!(in_irq(after_exit), in_irq(pc));
    }

    #[test]
    fn test_softirq_enter_exit(pc in 0i32..0x00FF_0000i32) {
        // Constrain to avoid SOFTIRQ mask overflow (8 bits at 8-15)
        let after_enter = pc + SOFTIRQ_OFFSET;
        assert!(in_softirq(after_enter));
        let after_exit = after_enter - SOFTIRQ_OFFSET;
        assert_eq!(after_exit, pc);
    }

    #[test]
    fn test_nmi_enter_exit(pc in 0i32..0x0010_0000i32) {
        // Constrain to avoid NMI mask overflow (1 bit at 20)
        let after_enter = pc + NMI_OFFSET;
        assert!(in_nmi(after_enter));
        let after_exit = after_enter - NMI_OFFSET;
        assert_eq!(after_exit, pc);
    }
}
