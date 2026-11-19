//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! PhysAddr/VirtAddr/PhysFrame/VirtPage arithmetic invariant tests.
//!
//! Types copied from: kernel/src/mm/page.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/mm/page.rs
// ============================================================================

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(pub usize);

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtAddr(pub usize);

impl PhysAddr {
    pub fn new(addr: usize) -> Self {
        Self(addr & !PAGE_MASK)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }

    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_MASK == 0
    }

    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_MASK)
    }

    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_MASK) & !PAGE_MASK)
    }

    pub fn frame_number(&self) -> usize {
        self.0 / PAGE_SIZE
    }

    pub fn ppn(&self) -> usize {
        self.0 / PAGE_SIZE
    }
}

impl VirtAddr {
    pub fn new(addr: usize) -> Self {
        Self(addr & !PAGE_MASK)
    }

    pub fn as_usize(&self) -> usize {
        self.0
    }

    pub fn is_aligned(&self) -> bool {
        self.0 & PAGE_MASK == 0
    }

    pub fn floor(&self) -> Self {
        Self(self.0 & !PAGE_MASK)
    }

    pub fn ceil(&self) -> Self {
        Self((self.0 + PAGE_MASK) & !PAGE_MASK)
    }

    pub fn page_number(&self) -> usize {
        self.0 / PAGE_SIZE
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PhysFrame {
    pub number: usize,
}

impl PhysFrame {
    pub const fn new(number: usize) -> Self {
        Self { number }
    }

    pub fn containing_address(addr: PhysAddr) -> Self {
        Self::new(addr.frame_number())
    }

    pub fn start_address(&self) -> PhysAddr {
        PhysAddr(self.number * PAGE_SIZE)
    }

    pub fn range(&self) -> core::ops::Range<PhysAddr> {
        let start = self.start_address();
        let end = PhysAddr(start.as_usize() + PAGE_SIZE);
        start..end
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct VirtPage {
    pub number: usize,
}

impl VirtPage {
    pub const fn new(number: usize) -> Self {
        Self { number }
    }

    pub fn containing_address(addr: VirtAddr) -> Self {
        Self::new(addr.page_number())
    }

    pub fn start_address(&self) -> VirtAddr {
        VirtAddr(self.number * PAGE_SIZE)
    }

    pub fn range(&self) -> core::ops::Range<VirtAddr> {
        let start = self.start_address();
        let end = VirtAddr(start.as_usize() + PAGE_SIZE);
        start..end
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-PADDR-1: new masks to page boundary
    #[test]
    fn test_new(addr in 0usize..0x100000usize) {
        let pa = PhysAddr::new(addr);
        prop_assert_eq!(pa.as_usize() & PAGE_MASK, 0);
    }

    /// INV-PADDR-2: is_aligned iff addr is page-aligned
    #[test]
    fn test_is_aligned(addr in 0usize..0x100000usize) {
        let pa = PhysAddr::new(addr);
        prop_assert_eq!(pa.is_aligned(), addr & PAGE_MASK == 0);
    }

    /// INV-PADDR-3: floor(addr) <= addr
    #[test]
    fn test_floor_le(addr in 0usize..0x100000usize) {
        let pa = PhysAddr(addr);
        prop_assert!(pa.floor().as_usize() <= addr);
    }

    /// INV-PADDR-4: ceil(addr) >= addr
    #[test]
    fn test_ceil_ge(addr in 0usize..0x100000usize) {
        let pa = PhysAddr(addr);
        prop_assert!(pa.ceil().as_usize() >= addr);
    }

    /// INV-PADDR-5: floor of aligned address is itself
    #[test]
    fn test_floor_aligned(frame in 0usize..1000usize) {
        let addr = frame * PAGE_SIZE;
        let pa = PhysAddr(addr);
        prop_assert_eq!(pa.floor(), pa);
    }

    /// INV-PADDR-6: ceil of aligned address is itself
    #[test]
    fn test_ceil_aligned(frame in 0usize..1000usize) {
        let addr = frame * PAGE_SIZE;
        let pa = PhysAddr(addr);
        prop_assert_eq!(pa.ceil(), pa);
    }

    /// INV-PADDR-7: frame_number == ppn
    #[test]
    fn test_frame_number_eq_ppn(addr in 0usize..0x100000usize) {
        let pa = PhysAddr::new(addr);
        prop_assert_eq!(pa.frame_number(), pa.ppn());
    }

    /// INV-PADDR-8: PhysFrame start_address roundtrip
    #[test]
    fn test_frame_roundtrip(frame in 0usize..10000usize) {
        let pf = PhysFrame::new(frame);
        let addr = pf.start_address();
        let pf2 = PhysFrame::containing_address(addr);
        prop_assert_eq!(pf, pf2);
    }

    /// INV-PADDR-9: PhysFrame range is PAGE_SIZE wide
    #[test]
    fn test_frame_range(frame in 0usize..1000usize) {
        let pf = PhysFrame::new(frame);
        let range = pf.range();
        prop_assert_eq!(range.end.as_usize() - range.start.as_usize(), PAGE_SIZE);
    }

    /// INV-PADDR-10: VirtAddr mirrors PhysAddr invariants
    #[test]
    fn test_virtaddr_new(addr in 0usize..0x100000usize) {
        let va = VirtAddr::new(addr);
        prop_assert_eq!(va.as_usize() & PAGE_MASK, 0);
    }

    /// INV-PADDR-11: VirtPage start_address roundtrip
    #[test]
    fn test_virtpage_roundtrip(page in 0usize..10000usize) {
        let vp = VirtPage::new(page);
        let addr = vp.start_address();
        let vp2 = VirtPage::containing_address(addr);
        prop_assert_eq!(vp, vp2);
    }

    /// INV-PADDR-12: VirtPage range is PAGE_SIZE wide
    #[test]
    fn test_virtpage_range(page in 0usize..1000usize) {
        let vp = VirtPage::new(page);
        let range = vp.range();
        prop_assert_eq!(range.end.as_usize() - range.start.as_usize(), PAGE_SIZE);
    }

    /// INV-PADDR-13: floor then ceil of same addr >= floor
    #[test]
    fn test_floor_ceil(addr in 0usize..0x100000usize) {
        let pa = PhysAddr(addr);
        let f = pa.floor();
        let c = pa.ceil();
        prop_assert!(c.as_usize() >= f.as_usize());
        // Difference at most one page
        prop_assert!(c.as_usize() - f.as_usize() <= PAGE_SIZE);
    }

    /// INV-PADDR-14: frame_number * PAGE_SIZE == start_address
    #[test]
    fn test_frame_times_page_size(frame in 0usize..10000usize) {
        let pf = PhysFrame::new(frame);
        prop_assert_eq!(pf.start_address().as_usize(), frame * PAGE_SIZE);
    }

    /// INV-PADDR-15: PAGE_MASK == 0xFFF
    #[test]
    fn test_page_mask(_v in 0u8..1u8) {
        prop_assert_eq!(PAGE_MASK, 0xFFF);
        prop_assert_eq!(PAGE_SIZE, 4096);
    }
}
