//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for SATP register encoding/decoding (Sv39).
//!
//! Types copied from: kernel/src/arch/riscv64/mm/asid.rs

#![cfg(kani)]

pub const ASID_BITS: usize = 9;
pub const MAX_ASID: u16 = (1 << ASID_BITS) - 1;

#[inline(always)]
fn build_satp(asid: u16, ppn: usize) -> usize {
    let mode: usize = 8; // Sv39
    ((mode & 0xF) << 60) | ((asid as usize & 0xFFFF) << 44) | (ppn & 0xFFFFFFFFFFF)
}

#[inline(always)]
fn satp_to_asid(satp: usize) -> u16 {
    ((satp >> 44) & 0xFFFF) as u16
}

#[inline(always)]
fn satp_to_ppn(satp: usize) -> usize {
    satp & 0xFFFFFFFFFFF
}

/// INV-SATP-K1: ASID round-trip: extract(build(asid, ppn)) == asid.
#[kani::proof]
fn verify_asid_roundtrip() {
    let asid: u16 = kani::any();
    let ppn: usize = kani::any();
    kani::assume(ppn < (1usize << 44));
    let satp = build_satp(asid, ppn);
    assert_eq!(satp_to_asid(satp), asid);
}

/// INV-SATP-K2: PPN round-trip: extract(build(asid, ppn)) == ppn (masked to 44 bits).
#[kani::proof]
fn verify_ppn_roundtrip() {
    let asid: u16 = kani::any();
    let ppn: usize = kani::any();
    kani::assume(ppn < (1usize << 44));
    let satp = build_satp(asid, ppn);
    assert_eq!(satp_to_ppn(satp), ppn);
}

/// INV-SATP-K3: High PPN bits (>44) are masked out.
#[kani::proof]
fn verify_ppn_masking() {
    let asid: u16 = kani::any();
    let ppn_lo: usize = kani::any();
    let ppn_hi: usize = kani::any();
    kani::assume(ppn_lo < (1usize << 44));
    kani::assume(ppn_hi < 256);
    let full_ppn = ppn_lo | (ppn_hi << 44);
    let satp = build_satp(asid, full_ppn);
    assert_eq!(satp_to_ppn(satp), ppn_lo);
}

/// INV-SATP-K4: Mode field is always Sv39 (8) in bits [63:60].
#[kani::proof]
fn verify_mode_field() {
    let asid: u16 = kani::any();
    let ppn: usize = kani::any();
    kani::assume(ppn < (1usize << 12));
    let satp = build_satp(asid, ppn);
    assert_eq!((satp >> 60) & 0xF, 8);
}

/// INV-SATP-K5: PPN field width is exactly 44 bits — max PPN preserved, overflow masked.
#[kani::proof]
fn verify_ppn_field_width() {
    let max_ppn = (1usize << 44) - 1;
    let satp = build_satp(0, max_ppn);
    assert_eq!(satp_to_ppn(satp), max_ppn);

    let overflow_ppn = 1usize << 44;
    let satp2 = build_satp(0, overflow_ppn);
    assert_eq!(satp_to_ppn(satp2), 0);
}
