//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for huge page constants and invariants.
//! Copied from: kernel/src/mm/hugepage.rs

use proptest::prelude::*;

// Copied constants
pub const PAGE_SHIFT: usize = 12;
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT; // 4096
pub const PMD_SHIFT: usize = 21;
pub const PGDIR_SHIFT: usize = 30;
pub const PMD_SIZE: usize = 1 << PMD_SHIFT;   // 2MB
pub const PGDIR_SIZE: usize = 1 << PGDIR_SHIFT; // 1GB
pub const PMD_MASK: usize = !(PMD_SIZE - 1);
pub const PGDIR_MASK: usize = !(PGDIR_SIZE - 1);
pub const HPAGE_PMD_NR: usize = PMD_SIZE / PAGE_SIZE;  // 512
pub const HPAGE_PGD_NR: usize = PGDIR_SIZE / PAGE_SIZE; // 262144
pub const HPAGE_PMD_ORDER: usize = PMD_SHIFT - PAGE_SHIFT;  // 9
pub const HPAGE_PGD_ORDER: usize = PGDIR_SHIFT - PAGE_SHIFT; // 18

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HugePageType {
    HugePagePmd,
    HugePagePgd,
}

impl HugePageType {
    pub fn size(&self) -> usize {
        match self {
            HugePageType::HugePagePmd => PMD_SIZE,
            HugePageType::HugePagePgd => PGDIR_SIZE,
        }
    }

    pub fn order(&self) -> usize {
        match self {
            HugePageType::HugePagePmd => HPAGE_PMD_ORDER,
            HugePageType::HugePagePgd => HPAGE_PGD_ORDER,
        }
    }
}

// PTE flags for huge pages
pub mod pte_flags {
    pub const V: u64 = 1 << 0;
    pub const R: u64 = 1 << 1;
    pub const W: u64 = 1 << 2;
    pub const X: u64 = 1 << 3;
    pub const U: u64 = 1 << 4;
    pub const G: u64 = 1 << 5;
    pub const A: u64 = 1 << 6;
    pub const D: u64 = 1 << 7;
    pub const KERNEL_HUGE: u64 = V | R | W | X | A | D;
    pub const USER_HUGE: u64 = V | R | W | X | U | A | D;
}

// VMA flags for huge pages
pub mod vm_flags {
    pub const VM_HUGETLB: u64 = 1 << 0;
    pub const VM_HUGE_PMD: u64 = 1 << 1;
    pub const VM_HUGE_PGD: u64 = 1 << 2;
    pub const VM_HUGE_ALIGN: u64 = 1 << 3;
}

// Alignment helpers (pure arithmetic)
pub fn is_pmd_aligned(addr: usize) -> bool { addr & (PMD_SIZE - 1) == 0 }
pub fn is_pgd_aligned(addr: usize) -> bool { addr & (PGDIR_SIZE - 1) == 0 }
pub fn pmd_align_down(addr: usize) -> usize { addr & PMD_MASK }
pub fn pmd_align_up(addr: usize) -> usize { (addr + PMD_SIZE - 1) & PMD_MASK }
pub fn pgd_align_down(addr: usize) -> usize { addr & PGDIR_MASK }
pub fn pgd_align_up(addr: usize) -> usize { (addr + PGDIR_SIZE - 1) & PGDIR_MASK }

proptest! {
    #[test]
    fn test_shift_hierarchy(shifts in 0usize..3) {
        // PAGE_SHIFT < PMD_SHIFT < PGDIR_SHIFT — strict ordering
        let all = [PAGE_SHIFT, PMD_SHIFT, PGDIR_SHIFT];
        assert!(all[0] < all[1], "PAGE_SHIFT < PMD_SHIFT");
        assert!(all[1] < all[2], "PMD_SHIFT < PGDIR_SHIFT");
    }

    #[test]
    fn test_size_is_power_of_two(_v in 0u8..1u8) {
        // Both PMD_SIZE and PGDIR_SIZE must be powers of two
        assert!(PMD_SIZE > 0 && (PMD_SIZE & (PMD_SIZE - 1)) == 0, "PMD_SIZE is power of two");
        assert!(PGDIR_SIZE > 0 && (PGDIR_SIZE & (PGDIR_SIZE - 1)) == 0, "PGDIR_SIZE is power of two");
    }

    #[test]
    fn test_size_matches_shift(_v in 0u8..1u8) {
        assert_eq!(PMD_SIZE, 1usize << PMD_SHIFT);
        assert_eq!(PGDIR_SIZE, 1usize << PGDIR_SHIFT);
    }

    #[test]
    fn test_pmd_size_is_2mb(_v in 0u8..1u8) {
        assert_eq!(PMD_SIZE, 2 * 1024 * 1024);
    }

    #[test]
    fn test_pgd_size_is_1gb(_v in 0u8..1u8) {
        assert_eq!(PGDIR_SIZE, 1024 * 1024 * 1024);
    }

    #[test]
    fn test_hpage_nr_page_count(_v in 0u8..1u8) {
        assert_eq!(HPAGE_PMD_NR, PMD_SIZE / PAGE_SIZE);
        assert_eq!(HPAGE_PGD_NR, PGDIR_SIZE / PAGE_SIZE);
        assert_eq!(HPAGE_PMD_NR, 512);
        assert_eq!(HPAGE_PGD_NR, 262144);
    }

    #[test]
    fn test_order_equals_shift_diff(_v in 0u8..1u8) {
        assert_eq!(HPAGE_PMD_ORDER, PMD_SHIFT - PAGE_SHIFT);
        assert_eq!(HPAGE_PGD_ORDER, PGDIR_SHIFT - PAGE_SHIFT);
        assert_eq!(HPAGE_PMD_ORDER, 9);
        assert_eq!(HPAGE_PGD_ORDER, 18);
    }

    #[test]
    fn test_mask_covers_all_bits(_v in 0u8..1u8) {
        // PMD_MASK and PGDIR_MASK mask off lower bits
        assert_eq!(PMD_MASK, !((1usize << PMD_SHIFT) - 1));
        assert_eq!(PGDIR_MASK, !((1usize << PGDIR_SHIFT) - 1));
    }

    #[test]
    fn test_mask_aligns_size(addr in 0usize..(1usize << 50)) {
        // pmd_align_down(addr) rounds down to PMD boundary
        let aligned = pmd_align_down(addr);
        assert!(aligned <= addr);
        assert_eq!(aligned % PMD_SIZE, 0);
        if addr % PMD_SIZE != 0 {
            assert!(aligned < addr);
        }
    }

    #[test]
    fn test_pgd_align_down_rounds(addr in 0usize..(1usize << 50)) {
        let aligned = pgd_align_down(addr);
        assert!(aligned <= addr);
        assert_eq!(aligned % PGDIR_SIZE, 0);
    }

    #[test]
    fn test_align_up_rounds_to_boundary(addr in 0usize..(1usize << 40)) {
        let down = pmd_align_down(addr);
        let up = pmd_align_up(addr);
        assert!(up >= addr);
        assert_eq!(up % PMD_SIZE, 0);
        if addr % PMD_SIZE != 0 {
            assert_eq!(up, down + PMD_SIZE);
        } else {
            assert_eq!(up, down);
        }
    }

    #[test]
    fn test_huge_page_type_size_order(_v in 0u8..1u8) {
        assert_eq!(HugePageType::HugePagePmd.size(), PMD_SIZE);
        assert_eq!(HugePageType::HugePagePgd.size(), PGDIR_SIZE);
        assert_eq!(HugePageType::HugePagePmd.order(), HPAGE_PMD_ORDER);
        assert_eq!(HugePageType::HugePagePgd.order(), HPAGE_PGD_ORDER);
    }

    #[test]
    fn test_pte_flags_are_powers_of_two(_v in 0u8..1u8) {
        let flags = [pte_flags::V, pte_flags::R, pte_flags::W, pte_flags::X,
                     pte_flags::U, pte_flags::G, pte_flags::A, pte_flags::D];
        for (i, &f) in flags.iter().enumerate() {
            assert_eq!(f, 1u64 << i, "PTE flag {} should be 1<<{}", i, i);
        }
    }

    #[test]
    fn test_pte_flags_distinct(_v in 0u8..1u8) {
        let flags = [pte_flags::V, pte_flags::R, pte_flags::W, pte_flags::X,
                     pte_flags::U, pte_flags::G, pte_flags::A, pte_flags::D];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0, "PTE flags {} and {} overlap", i, j);
            }
        }
    }

    #[test]
    fn test_kernel_user_huge_flags_differ(_v in 0u8..1u8) {
        // KERNEL_HUGE lacks U (user) bit; USER_HUGE has U
        assert_ne!(pte_flags::KERNEL_HUGE, pte_flags::USER_HUGE);
        assert_eq!(pte_flags::KERNEL_HUGE & pte_flags::U, 0, "KERNEL_HUGE should not have U bit");
        assert_ne!(pte_flags::USER_HUGE & pte_flags::U, 0, "USER_HUGE should have U bit");
    }

    #[test]
    fn test_vm_flags_distinct(_v in 0u8..1u8) {
        let flags = [vm_flags::VM_HUGETLB, vm_flags::VM_HUGE_PMD,
                     vm_flags::VM_HUGE_PGD, vm_flags::VM_HUGE_ALIGN];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0, "VM flags {} and {} overlap", i, j);
            }
        }
    }
}
