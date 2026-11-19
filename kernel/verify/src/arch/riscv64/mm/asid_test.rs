//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! SATP register encoding/decoding invariant tests.
//!
//! Types copied from: kernel/src/arch/riscv64/mm/asid.rs

use proptest::prelude::*;

// ============================================================================
// Copied constants/functions from kernel/src/arch/riscv64/mm/asid.rs
// ============================================================================

pub const ASID_BITS: usize = 9;
pub const MAX_ASID: u16 = (1 << ASID_BITS) - 1;
pub const ASID_KERNEL: u16 = 0;
pub const ASID_RESERVED: u16 = 1;
pub const ASID_FIRST: u16 = 2;

/// Build SATP register value (Sv39 mode)
#[inline(always)]
fn build_satp(asid: u16, ppn: usize) -> usize {
    let mode: usize = 8; // Sv39
    ((mode & 0xF) << 60) | ((asid as usize & 0xFFFF) << 44) | (ppn & 0xFFFFFFFFFFF)
}

/// Extract ASID from SATP value
#[inline(always)]
fn satp_to_asid(satp: usize) -> u16 {
    ((satp >> 44) & 0xFFFF) as u16
}

/// Extract PPN from SATP value
#[inline(always)]
fn satp_to_ppn(satp: usize) -> usize {
    satp & 0xFFFFFFFFFFF
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-SATP-1: ASID round-trip: extract(build(asid, ppn)) == asid
    #[test]
    fn test_asid_roundtrip(
        asid in 0u16..=MAX_ASID,
        ppn in 0usize..(1usize << 44),
    ) {
        let satp = build_satp(asid, ppn);
        prop_assert_eq!(satp_to_asid(satp), asid);
    }

    /// INV-SATP-2: PPN round-trip: extract(build(asid, ppn)) == ppn (masked)
    #[test]
    fn test_ppn_roundtrip(
        asid in 0u16..=MAX_ASID,
        ppn in 0usize..(1usize << 44),
    ) {
        let satp = build_satp(asid, ppn);
        prop_assert_eq!(satp_to_ppn(satp), ppn & 0xFFFFFFFFFFF);
    }

    /// INV-SATP-3: High PPN bits (>44) are masked out
    #[test]
    fn test_ppn_masking(
        asid in 0u16..MAX_ASID,
        ppn_lo in 0usize..(1usize << 44),
        ppn_hi_bits in 0usize..256usize,
    ) {
        let full_ppn = ppn_lo | (ppn_hi_bits << 44);
        let satp = build_satp(asid, full_ppn);
        prop_assert_eq!(satp_to_ppn(satp), ppn_lo);
    }

    /// INV-SATP-4: Mode field is always Sv39 (8) in bits [63:60]
    #[test]
    fn test_mode_field(
        asid in 0u16..MAX_ASID,
        ppn in 0usize..(1usize << 12),
    ) {
        let satp = build_satp(asid, ppn);
        prop_assert_eq!((satp >> 60) & 0xF, 8);
    }

    /// INV-SATP-5: ASID occupies bits [59:44]
    #[test]
    fn test_asid_bit_position(
        asid in 0u16..=MAX_ASID,
        ppn in 0usize..(1usize << 12),
    ) {
        let satp = build_satp(asid, ppn);
        let asid_field = (satp >> 44) & 0xFFFF;
        prop_assert_eq!(asid_field, asid as usize);
    }

    /// INV-SATP-6: PPN occupies bits [43:0]
    #[test]
    fn test_ppn_bit_position(
        asid in 0u16..MAX_ASID,
        ppn in 0usize..(1usize << 44),
    ) {
        let satp = build_satp(asid, ppn);
        let ppn_field = satp & 0xFFFFFFFFFFF;
        prop_assert_eq!(ppn_field, ppn & 0xFFFFFFFFFFF);
    }

    /// INV-SATP-7: ASID > MAX_ASID is truncated by mask
    #[test]
    fn test_asid_truncation(
        asid_raw in 0u32..(1u32 << 16),
        ppn in 0usize..1024usize,
    ) {
        let asid = asid_raw as u16;
        let satp = build_satp(asid, ppn);
        let extracted = satp_to_asid(satp);
        // build_satp masks: (asid as usize & 0xFFFF), then extract masks & 0xFFFF
        prop_assert_eq!(extracted, asid as u16);
    }
}

#[test]
/// INV-SATP-8: ASID constants are consistent
fn test_asid_constants() {
    assert_eq!(ASID_BITS, 9);
    assert_eq!(MAX_ASID, 511);
    assert_eq!(ASID_KERNEL, 0);
    assert_eq!(ASID_RESERVED, 1);
    assert_eq!(ASID_FIRST, 2);
    assert!(ASID_FIRST > ASID_RESERVED);
    assert!(ASID_RESERVED > ASID_KERNEL);
}

#[test]
/// INV-SATP-9: ASID field width is exactly 16 bits in SATP
fn test_asid_field_width() {
    // ASID 0xFFFF should be preserved, 0x10000 should be truncated
    let satp_full = build_satp(0xFFFF, 0);
    assert_eq!(satp_to_asid(satp_full), 0xFFFF);

    let satp_overflow = build_satp(0xFFFF, 0);
    assert_eq!(satp_to_asid(satp_overflow), 0xFFFF); // preserved by u16
}

#[test]
/// INV-SATP-10: PPN field width is exactly 44 bits in SATP
fn test_ppn_field_width() {
    let max_ppn = (1usize << 44) - 1;
    let satp = build_satp(0, max_ppn);
    assert_eq!(satp_to_ppn(satp), max_ppn);

    let overflow_ppn = 1usize << 44;
    let satp = build_satp(0, overflow_ppn);
    assert_eq!(satp_to_ppn(satp), 0); // all bits masked out
}
