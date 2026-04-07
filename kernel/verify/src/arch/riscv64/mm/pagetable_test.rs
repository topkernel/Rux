//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Sv39 page table entry and satp invariant tests.
//!
//! Types copied from: kernel/src/arch/riscv64/mm/pagetable.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/arch/riscv64/mm/pagetable.rs
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub const V: u64 = 1 << 0;
    pub const R: u64 = 1 << 1;
    pub const W: u64 = 1 << 2;
    pub const X: u64 = 1 << 3;
    pub const U: u64 = 1 << 4;
    pub const G: u64 = 1 << 5;
    pub const A: u64 = 1 << 6;
    pub const D: u64 = 1 << 7;

    #[inline]
    pub const fn new() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn is_valid(&self) -> bool {
        self.0 & Self::V != 0
    }

    #[inline]
    pub fn is_readable(&self) -> bool {
        self.0 & Self::R != 0
    }

    #[inline]
    pub fn is_writable(&self) -> bool {
        self.0 & Self::W != 0
    }

    #[inline]
    pub fn is_executable(&self) -> bool {
        self.0 & Self::X != 0
    }

    #[inline]
    pub fn is_user(&self) -> bool {
        self.0 & Self::U != 0
    }

    #[inline]
    pub fn is_leaf(&self) -> bool {
        (self.0 & (Self::R | Self::W | Self::X)) != 0
    }

    #[inline]
    pub fn ppn(&self) -> u64 {
        (self.0 >> 10) & 0x00FFFFFFFFFFFFFF
    }

    #[inline]
    pub fn new_table(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V)
    }

    #[inline]
    pub fn new_page_kernel(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V | Self::R | Self::W | Self::X | Self::A | Self::D)
    }

    #[inline]
    pub fn new_page_user(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V | Self::R | Self::W | Self::X | Self::U | Self::A | Self::D)
    }

    #[inline]
    pub fn new_page_ro(ppn: u64) -> Self {
        Self((ppn << 10) | Self::V | Self::R | Self::X | Self::A)
    }
}

impl Default for PageTableEntry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Satp(pub u64);

impl Satp {
    pub const MODE_BARE: u64 = 0;
    pub const MODE_SV39: u64 = 8;

    #[inline]
    pub const fn new(mode: u64, asid: u16, ppn: u64) -> Self {
        Self(((mode as u64) << 60) | ((asid as u64) << 44) | (ppn & 0x0FFFFFFFFFFFFFFF))
    }

    #[inline]
    pub const fn sv39(ppn: u64, asid: u16) -> Self {
        Self::new(Self::MODE_SV39, asid, ppn)
    }

    #[inline]
    pub fn bits(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn mode(&self) -> u64 {
        self.0 >> 60
    }

    #[inline]
    pub fn asid(&self) -> u16 {
        ((self.0 >> 44) & 0xFFFF) as u16
    }

    #[inline]
    pub fn ppn(&self) -> u64 {
        self.0 & 0x0FFFFFFFFFFFFFFF
    }

    #[inline]
    pub fn is_bare(&self) -> bool {
        self.mode() == Self::MODE_BARE
    }

    #[inline]
    pub fn is_sv39(&self) -> bool {
        self.mode() == Self::MODE_SV39
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-PTE-1: new_page_user sets V, R, W, X, U, A, D
    #[test]
    fn test_new_page_user(ppn in 0u64..(1u64 << 44)) {
        let pte = PageTableEntry::new_page_user(ppn);
        prop_assert!(pte.is_valid());
        prop_assert!(pte.is_readable());
        prop_assert!(pte.is_writable());
        prop_assert!(pte.is_executable());
        prop_assert!(pte.is_user());
        prop_assert!(pte.is_leaf());
        prop_assert_eq!(pte.ppn(), ppn);
    }

    /// INV-PTE-2: new_page_kernel sets V, R, W, X, A, D (no U)
    #[test]
    fn test_new_page_kernel(ppn in 0u64..(1u64 << 44)) {
        let pte = PageTableEntry::new_page_kernel(ppn);
        prop_assert!(pte.is_valid());
        prop_assert!(pte.is_readable());
        prop_assert!(pte.is_writable());
        prop_assert!(pte.is_executable());
        prop_assert!(!pte.is_user());
        prop_assert!(pte.is_leaf());
        prop_assert_eq!(pte.ppn(), ppn);
    }

    /// INV-PTE-3: new_page_ro sets V, R, X, A (no W, no D)
    #[test]
    fn test_new_page_ro(ppn in 0u64..(1u64 << 44)) {
        let pte = PageTableEntry::new_page_ro(ppn);
        prop_assert!(pte.is_valid());
        prop_assert!(pte.is_readable());
        prop_assert!(!pte.is_writable());
        prop_assert!(pte.is_executable());
        prop_assert!(pte.is_leaf());
        prop_assert_eq!(pte.ppn(), ppn);
    }

    /// INV-PTE-4: is_leaf detects R, W, or X bits
    #[test]
    fn test_is_leaf(flags_val in 0u64..256u64) {
        let pte = PageTableEntry::from_bits(flags_val);
        let has_rwx = flags_val & (PageTableEntry::R | PageTableEntry::W | PageTableEntry::X) != 0;
        prop_assert_eq!(pte.is_leaf(), has_rwx);
    }

    /// INV-PTE-5: new_table sets V but not R/W/X (non-leaf)
    #[test]
    fn test_new_table(ppn in 0u64..(1u64 << 44)) {
        let pte = PageTableEntry::new_table(ppn);
        prop_assert!(pte.is_valid());
        prop_assert!(!pte.is_leaf());
        prop_assert_eq!(pte.ppn(), ppn);
    }

    /// INV-PTE-6: ppn extracts bits [53:10]
    #[test]
    fn test_ppn_extraction(ppn in 0u64..(1u64 << 44)) {
        let pte = PageTableEntry::new_page_user(ppn);
        let extracted = pte.ppn();
        prop_assert_eq!(extracted, ppn);
    }

    /// INV-PTE-7: from_bits and bits roundtrip
    #[test]
    fn test_bits_roundtrip(val in 0u64..(1u64 << 54)) {
        let pte = PageTableEntry::from_bits(val);
        prop_assert_eq!(pte.bits(), val);
    }

    /// INV-SATP-1: sv39 sets mode to 8
    #[test]
    fn test_sv39_mode(ppn in 0u64..(1u64 << 44)) {
        let satp = Satp::sv39(ppn, 0);
        prop_assert!(satp.is_sv39());
        prop_assert!(!satp.is_bare());
        prop_assert_eq!(satp.mode(), 8);
        prop_assert_eq!(satp.asid(), 0);
        prop_assert_eq!(satp.ppn(), ppn);
    }

    /// INV-SATP-3: new packs fields correctly (asid=0 path)
    #[test]
    fn test_new_packing(mode in 0u64..16u64, ppn in 0u64..(1u64 << 44)) {
        let satp = Satp::new(mode, 0, ppn);
        prop_assert_eq!(satp.mode(), mode);
        prop_assert_eq!(satp.asid(), 0);
        prop_assert_eq!(satp.ppn(), ppn);
    }
}

/// INV-PTE-8: default PTE is zero (invalid)
#[test]
fn test_default() {
    let pte = PageTableEntry::default();
    assert!(!pte.is_valid());
    assert_eq!(pte.bits(), 0);
}

/// INV-SATP-2: bare mode has mode 0
#[test]
fn test_bare_mode() {
    let satp = Satp::new(0, 0, 0);
    assert!(satp.is_bare());
    assert!(!satp.is_sv39());
}

/// INV-SATP-4: asid extraction
#[test]
fn test_asid_extraction() {
    for asid in [0u16, 1u16, 100u16, 4095u16] {
        let satp = Satp::new(8, asid, 0);
        assert_eq!(satp.asid(), asid);
    }
}

/// INV-SATP-5: mode extraction
#[test]
fn test_mode_extraction() {
    assert_eq!(Satp::new(0, 0, 0).mode(), 0);
    assert_eq!(Satp::new(8, 0, 0).mode(), 8);
    assert_eq!(Satp::new(15, 0, 0).mode(), 15);
}
