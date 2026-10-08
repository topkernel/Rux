//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 "ASID" layer — PCID is disabled at bring-up; the same function
//! names exist so mm/context code compiles against both backends.
//! `satp` in these names means CR3.

use super::super::cpu::{invlpg, read_cr3, write_cr3};
use super::memory_layout::PAGE_SIZE;

pub const ASID_BITS: u64 = 0;
pub const MAX_ASID: u16 = 1;
pub const ASID_KERNEL: u16 = 0;
pub const ASID_RESERVED: u16 = 0;
pub const ASID_FIRST: u16 = 0;

/// Always ASID 0 (PCID off)
pub fn alloc_asid() -> Option<u16> {
    Some(0)
}

pub fn free_asid(_asid: u16) {}

pub fn asid_usage_count() -> usize {
    1
}

/// Flush the whole TLB (reload CR3)
pub fn flush_tlb_all() {
    // SAFETY: reloading the current CR3 with PCID off flushes non-global
    // translations.
    unsafe { write_cr3(read_cr3()) };
}

/// Flush one ASID (== all, PCID off)
pub fn flush_tlb_asid(_asid: u16) {
    flush_tlb_all();
}

/// Flush one page
pub fn flush_tlb_page(addr: u64) {
    // SAFETY: caller passes a canonical address.
    unsafe { invlpg(addr) };
}

/// Flush a range page by page
pub fn flush_tlb_range(start: u64, end: u64) {
    let mut a = start & !(PAGE_SIZE - 1);
    while a < end {
        flush_tlb_page(a);
        a += PAGE_SIZE;
    }
}

/// Flush kernel (global) translations
pub fn flush_tlb_kernel() {
    flush_tlb_all();
}

/// CR3 value for (ppn, asid)
pub fn build_satp(_asid: u16, ppn: u64) -> u64 {
    ppn << 12
}

pub fn satp_to_asid(_cr3: u64) -> u16 {
    0
}

pub fn satp_to_ppn(cr3: u64) -> u64 {
    cr3 >> 12
}

pub fn read_satp() -> u64 {
    read_cr3()
}

/// # Safety
/// `cr3` must be a valid PML4 physical address.
pub unsafe fn write_satp(cr3: u64) {
    write_cr3(cr3)
}

/// ASID context (interface parity)
pub struct AsidContext;
impl AsidContext {
    pub const fn new() -> Self {
        AsidContext
    }
}

pub fn print_asid_status() {}
