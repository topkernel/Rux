//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 ASID management + TLB invalidation.
//!
//! The TLB-tag ASID source is TTBR0_EL1[63:48] (TCR_EL1.A1 = 0, the
//! boot.S configuration; CONTEXTIDR_EL1 is a trace/debug register and
//! does NOT drive TLB tags). ASID width follows TCR_EL1.AS: the boot
//! TCR clears it, so ASIDs are 8-bit (256) — the conservative floor the
//! allocator uses; stage 2 probes/re-programs the width with TCR_EL1.
//! The composed "satp" convention (mode|ASID|TTBR0 PPN) is defined in
//! pagetable.rs; read_satp/write_satp decompose it into TTBR0_EL1
//! (root PPA + ASID in [63:48]) writes.
//!
//! TLBI operand encoding (ARM DDI 0487, TLBI register payload, same
//! convention as Linux arm64 __TLBI_VADDR): bits [43:0] = VA[47:12]
//! (page number), bits [63:48] = ASID. The riscv64 `asid == 0 means
//! all address spaces` convention maps to the VA-ALL-ASIDs op
//! (`tlbi vaae1is`), never to ASID field 0 (which would match only
//! entries tagged ASID 0 and MISS every nG user entry).
//!
//! ASID pool: the riscv64 twin's reuse bitmap (alloc/free/count share
//! the same contract). The original bump-only counter never reused a
//! slot: 254 mm creations exhausted it permanently and every later mm
//! ran ASID 0 (TLB-miss-per-access; see alloc_asid). Reuse is safe
//! because every leaf write invalidates all-ASID and every switch
//! full-flushes; generation rollover (stage 2) remains the refinement
//! for large concurrent pools.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::arch::asm;
use core::sync::atomic::{AtomicU16, AtomicU64, Ordering};

/// ASID bits (conservative 8-bit floor; see module doc)
pub const ASID_BITS: usize = 8;
pub const MAX_ASID: u16 = (1 << ASID_BITS) - 1;

pub const ASID_KERNEL: u16 = 0;   // Kernel uses ASID 0
pub const ASID_RESERVED: u16 = 1; // Reserved for early boot
pub const ASID_FIRST: u16 = 2;

/// Number of 64-bit words covering MAX_ASID+1 bits (256 ASIDs -> 4 words)
pub(crate) const ASID_WORDS: usize = (MAX_ASID as usize + 1 + 63) / 64;

/// Global ASID allocator state: word 0 starts with ASID 0 and 1 reserved.
pub(crate) static ASID_BITMAP: [AtomicU64; ASID_WORDS] = {
    let mut words = [const { AtomicU64::new(0) }; ASID_WORDS];
    words[0] = AtomicU64::new(0b11); // ASID 0 and 1 reserved
    words
};

/// Allocate an ASID
///
/// The old allocator bumped NEXT_ASID monotonically and never consulted
/// the in-use table: after 254 mm creations (any process-churning
/// workload — LTP forks thousands) `alloc_asid` returned None forever
/// and every later mm fell back to ASID 0 (correct — every PTE-write
/// path flushes all-ASID and every switch full-flushes — but every user
/// access then TLB-misses). The riscv64 twin scans a bitmap and reuses
/// freed ASIDs; this is that allocator over the aarch64 256-entry pool
/// (TCR_EL1.AS = 0). Reuse safety: a recycled ASID cannot meet a stale
/// entry — every leaf write invalidates with asid=0 (all-ASID tlbi) and
/// every write_satp emits a full vmalle1is before the new mm runs.
pub fn alloc_asid() -> Option<u16> {
    'retry: loop {
        for w in 0..ASID_WORDS {
            let word = ASID_BITMAP[w].load(Ordering::Acquire);
            if word == u64::MAX {
                continue;
            }
            let base = (w * 64) as u16;
            for bit in 0..64u32 {
                let i = base + bit as u16;
                if i < ASID_FIRST || i > MAX_ASID {
                    continue;
                }
                let mask = 1u64 << bit;
                if word & mask == 0 {
                    // Found a free ASID, try to claim it
                    if ASID_BITMAP[w]
                        .compare_exchange(word, word | mask, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        return Some(i);
                    }
                    // CAS failed — another CPU claimed this slot; rescan.
                    continue 'retry;
                }
            }
        }
        // Every word is full (or holds only reserved/out-of-range bits) —
        // genuine concurrency exhaustion only; ASID 0 fallback (context.rs).
        return None;
    }
}

/// Free an ASID
pub fn free_asid(asid: u16) {
    if asid < ASID_FIRST || asid > MAX_ASID {
        return;
    }
    let w = (asid / 64) as usize;
    let mask = 1u64 << (asid % 64);
    ASID_BITMAP[w].fetch_and(!mask, Ordering::Release);
}

/// Number of ASIDs currently allocated
pub fn asid_usage_count() -> u32 {
    let mut used = 0u32;
    for w in 0..ASID_WORDS {
        used += ASID_BITMAP[w].load(Ordering::Acquire).count_ones();
    }
    used
}

// ==================== TLB invalidation ====================
/// Flush the entire TLB (all EL1, all ASIDs)
///
/// Collected into `arch::arch_floor` (INV-ARCH-ASID-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn flush_tlb_all() {
    crate::arch::arch_floor::aarch64::mm::asid::flush_tlb_all()
}
/// Flush all entries for one ASID
///
/// Collected into `arch::arch_floor` (INV-ARCH-ASID-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn flush_tlb_asid(asid: u16) {
    crate::arch::arch_floor::aarch64::mm::asid::flush_tlb_asid(asid)
}
/// Flush one page in one ASID.
///
/// Collected into `arch::arch_floor` (INV-ARCH-ASID-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn flush_tlb_page(vaddr: usize, asid: u16) {
    crate::arch::arch_floor::aarch64::mm::asid::flush_tlb_page(vaddr, asid)
}
/// Flush a VA range in one ASID (page-granular loop; asid 0 = all ASIDs,
///
/// Collected into `arch::arch_floor` (INV-ARCH-ASID-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn flush_tlb_range(start: usize, end: usize, asid: u16) {
    crate::arch::arch_floor::aarch64::mm::asid::flush_tlb_range(start, end, asid)
}
/// Flush kernel-half (global) entries
///
/// Collected into `arch::arch_floor` (INV-ARCH-ASID-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn flush_tlb_kernel() {
    crate::arch::arch_floor::aarch64::mm::asid::flush_tlb_kernel()
}

// ==================== Composed "satp" register ====================

/// Compose the satp-equivalent value (mode 8 | ASID | TTBR0 root PPN)
pub fn build_satp(asid: u16, ppn: usize) -> usize {
    (8usize << 60) | ((asid as usize) << 44) | (ppn & 0xFFFF_FFFF_FFFF)
}

/// Extract the ASID from a composed value
pub fn satp_to_asid(satp: usize) -> u16 {
    ((satp >> 44) & 0xFFFF) as u16
}

/// Extract the root PPN from a composed value
pub fn satp_to_ppn(satp: usize) -> usize {
    satp & 0xFFFF_FFFF_FFFF
}

/// Read the composed value (TTBR0_EL1 root PPN + ASID)
pub fn read_satp() -> usize {
    super::pagetable::get_satp().0 as usize
}
/// Write the composed value: switch TTBR0_EL1 (root PPA + ASID in
///
/// Collected into `arch::arch_floor` (INV-ARCH-ASID-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn write_satp(satp: usize) {
    crate::arch::arch_floor::aarch64::mm::asid::write_satp(satp)
}

// ==================== ASID context helper ====================

/// RAII-ish ASID context (riscv64 parity)
pub struct AsidContext {
    pub(crate) asid: AtomicU16,
    pub(crate) generation: core::sync::atomic::AtomicU64,
}

impl AsidContext {
    pub const fn new() -> Self {
        Self {
            asid: AtomicU16::new(0),
            generation: core::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn asid(&self) -> u16 {
        self.asid.load(Ordering::Acquire)
    }

    pub fn alloc(&self) -> Option<u16> {
        let a = alloc_asid()?;
        self.asid.store(a, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Relaxed);
        Some(a)
    }

    pub fn free(&self) {
        let a = self.asid.swap(0, Ordering::AcqRel);
        if a != 0 {
            // Twin parity (riscv64 AsidContext::free): kill the outgoing
            // ASID's entries before the slot returns to the pool. Belt
            // layer only — every leaf write invalidates all-ASID and every
            // switch full-flushes, so a recycled ASID never meets a stale
            // entry anyway.
            flush_tlb_asid(a);
            free_asid(a);
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

/// Print ASID allocation status (diagnostics)
pub fn print_asid_status() {
    crate::println!(
        "asid: allocated {} / pool {} (bits {})",
        asid_usage_count(),
        MAX_ASID + 1,
        ASID_BITS
    );
}
