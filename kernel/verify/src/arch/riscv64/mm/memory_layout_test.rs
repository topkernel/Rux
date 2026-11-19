//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Sv39 VirtAddr and memory layout invariant tests.
//!
//! Types copied from: kernel/src/arch/riscv64/mm/memory_layout.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/arch/riscv64/mm/memory_layout.rs
// ============================================================================

pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_SHIFT: u64 = 12;
pub const PAGE_OFFSET_MASK: u64 = (1 << PAGE_SHIFT) - 1;
pub const VA_BITS: u64 = 39;
pub const VA_MASK: u64 = (1 << VA_BITS) - 1;
pub const PTRS_PER_PTE: u64 = 512;
pub const PTRS_PER_PMD: u64 = 512;
pub const PTRS_PER_PUD: u64 = 512;
pub const PTRS_PER_PGD: u64 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    pub const fn new(addr: u64) -> Self {
        let bit38 = (addr >> 38) & 1;
        if bit38 == 1 {
            Self(addr | 0xFFFFFFC0_00000000)
        } else {
            Self(addr & 0x0000007F_FFFFFFFF)
        }
    }

    pub const fn bits(&self) -> u64 {
        self.0
    }

    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_OFFSET_MASK == 0
    }

    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_OFFSET_MASK)
    }

    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_SIZE - 1) & !PAGE_OFFSET_MASK)
    }

    pub fn page_offset(&self) -> u64 {
        self.0 & PAGE_OFFSET_MASK
    }

    pub fn vpn(&self, level: u8) -> u64 {
        (self.0 >> (PAGE_SHIFT + 9 * level as u64)) & 0x1FF
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-VA-1: user address (bit 38 = 0) clears upper bits
    #[test]
    fn test_user_sign_extend(addr in 0u64..0x3FFFFFFFFFu64) {
        let va = VirtAddr::new(addr);
        prop_assert_eq!(va.bits() & 0xFFFFFFC0_00000000, 0);
    }

    /// INV-VA-2: kernel address (bit 38 = 1) sets upper bits
    #[test]
    fn test_kernel_sign_extend(addr in 0x4000000000u64..0x8000000000u64) {
        let va = VirtAddr::new(addr);
        prop_assert_eq!(va.bits() & 0xFFFFFFC0_00000000, 0xFFFFFFC0_00000000);
    }

    /// INV-VA-3: VPN level 0 extracts bits [20:12]
    #[test]
    fn test_vpn_level0(vpn_val in 0u64..511u64) {
        let addr = vpn_val << 12; // Put vpn_val in level 0 position
        let va = VirtAddr::new(addr);
        prop_assert_eq!(va.vpn(0), vpn_val);
    }

    /// INV-VA-4: VPN level 1 extracts bits [29:21]
    #[test]
    fn test_vpn_level1(vpn_val in 0u64..511u64) {
        let addr = vpn_val << 21;
        let va = VirtAddr::new(addr);
        prop_assert_eq!(va.vpn(1), vpn_val);
    }

    /// INV-VA-5: VPN level 2 extracts bits [38:30]
    #[test]
    fn test_vpn_level2(vpn_val in 0u64..256u64) {
        let addr = vpn_val << 30;
        let va = VirtAddr::new(addr);
        prop_assert_eq!(va.vpn(2), vpn_val);
    }

    /// INV-VA-6: VPN always returns 9-bit value (0..511)
    #[test]
    fn test_vpn_9bit(
        addr in 0u64..0x100000000u64,
        level in 0u8..3u8,
    ) {
        let va = VirtAddr::new(addr);
        let vpn = va.vpn(level);
        prop_assert!(vpn < 512);
    }

    /// INV-VA-7: is_aligned
    #[test]
    fn test_is_aligned(addr in 0u64..0x10000u64) {
        let va = VirtAddr::new(addr);
        prop_assert_eq!(va.is_aligned(), (addr & PAGE_OFFSET_MASK) == 0);
    }

    /// INV-VA-8: floor(addr) <= addr
    #[test]
    fn test_floor(addr in 0u64..0x100000000u64) {
        let va = VirtAddr::new(addr);
        prop_assert!(va.floor().bits() <= addr);
    }

    /// INV-VA-9: ceil(addr) >= addr
    #[test]
    fn test_ceil(addr in 0u64..0x100000000u64) {
        let va = VirtAddr::new(addr);
        prop_assert!(va.ceil().bits() >= addr);
    }

    /// INV-VA-10: page_offset extracts low 12 bits
    #[test]
    fn test_page_offset(addr in 0u64..0x100000u64) {
        let va = VirtAddr::new(addr);
        prop_assert_eq!(va.page_offset(), addr & 0xFFF);
    }

    /// INV-VA-11: PTRS_PER_PTE == 512
    #[test]
    fn test_ptrs_per_pte(_v in 0u8..1u8) {
        prop_assert_eq!(PTRS_PER_PTE, 512);
        prop_assert_eq!(PTRS_PER_PMD, 512);
        prop_assert_eq!(PTRS_PER_PUD, 512);
        prop_assert_eq!(PTRS_PER_PGD, 512);
    }

    /// INV-VA-12: floor of page-aligned address is itself
    #[test]
    fn test_floor_aligned(frame in 0u64..1000u64) {
        let addr = frame << PAGE_SHIFT;
        let va = VirtAddr::new(addr);
        prop_assert_eq!(va.floor(), va);
    }

    /// INV-VA-13: VA_MASK covers all 39 bits
    #[test]
    fn test_va_mask(_v in 0u8..1u8) {
        prop_assert_eq!(VA_MASK, (1u64 << 39) - 1);
        prop_assert_eq!(VA_BITS, 39);
    }

    /// INV-VA-14: VPN of zero address is 0 at all levels
    #[test]
    fn test_zero_vpn(_v in 0u8..1u8) {
        let va = VirtAddr::new(0);
        prop_assert_eq!(va.vpn(0), 0);
        prop_assert_eq!(va.vpn(1), 0);
        prop_assert_eq!(va.vpn(2), 0);
    }
}
