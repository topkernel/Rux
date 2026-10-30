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

use core::arch::asm;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};

/// ASID bits (conservative 8-bit floor; see module doc)
pub const ASID_BITS: usize = 8;
pub const MAX_ASID: u16 = (1 << ASID_BITS) - 1;

pub const ASID_KERNEL: u16 = 0;   // Kernel uses ASID 0
pub const ASID_RESERVED: u16 = 1; // Reserved for early boot
pub const ASID_FIRST: u16 = 2;

static NEXT_ASID: AtomicU16 = AtomicU16::new(ASID_FIRST);
static ASID_COUNT: AtomicU32 = AtomicU32::new(0);
static ASID_IN_USE: [AtomicU16; 256] = [const { AtomicU16::new(0) }; 256];

/// Allocate an ASID
pub fn alloc_asid() -> Option<u16> {
    loop {
        let cur = NEXT_ASID.load(Ordering::Acquire);
        if cur > MAX_ASID {
            return None; // pool exhausted (stage 2: generation rollover)
        }
        match NEXT_ASID.compare_exchange_weak(
            cur,
            cur + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                ASID_IN_USE[cur as usize].store(1, Ordering::Release);
                ASID_COUNT.fetch_add(1, Ordering::Relaxed);
                return Some(cur);
            }
            Err(actual) => {
                if actual > MAX_ASID {
                    return None;
                }
                continue;
            }
        }
    }
}

/// Free an ASID
pub fn free_asid(asid: u16) {
    if (asid as usize) < ASID_IN_USE.len() && ASID_IN_USE[asid as usize].swap(0, Ordering::AcqRel) != 0 {
        ASID_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Number of ASIDs currently allocated
pub fn asid_usage_count() -> u32 {
    ASID_COUNT.load(Ordering::Acquire)
}

// ==================== TLB invalidation ====================

/// Flush the entire TLB (all EL1, all ASIDs)
pub fn flush_tlb_all() {
    // SAFETY: tlbi vmalle1ais broadcasts full invalidation to all PEs;
    // callers have repaired the affected translations first.
    unsafe {
        asm!(
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nostack, preserves_flags)
        );
    }
}

/// Flush all entries for one ASID
pub fn flush_tlb_asid(asid: u16) {
    // SAFETY: ASID-scoped inner-shareable invalidation. The operand's
    // ASID field lives in bits [63:48].
    unsafe {
        asm!(
            "tlbi aside1is, {a}",
            "dsb ish",
            "isb",
            a = in(reg) ((asid & 0xFF) as u64) << 48,
            options(nostack, preserves_flags)
        );
    }
}

/// Flush one page in one ASID.
///
/// `asid == 0` follows the riscv64 convention "all address spaces" and
/// emits `tlbi vaae1is` (invalidate by VA across ALL ASIDs); a nonzero
/// ASID emits `tlbi vae1is` scoped to that ASID.
pub fn flush_tlb_page(vaddr: usize, asid: u16) {
    let pn = ((vaddr >> 12) & 0x0000_FFFF_FFFF_F) as u64;
    // SAFETY: page-scoped invalidation; operand = VA[47:12] | ASID<<48.
    unsafe {
        if asid == 0 {
            asm!(
                "tlbi vaae1is, {v}",
                "dsb ish",
                "isb",
                v = in(reg) pn,
                options(nostack, preserves_flags)
            );
        } else {
            asm!(
                "tlbi vae1is, {v}",
                "dsb ish",
                "isb",
                v = in(reg) pn | (((asid & 0xFF) as u64) << 48),
                options(nostack, preserves_flags)
            );
        }
    }
}

/// Flush a VA range in one ASID (page-granular loop; asid 0 = all ASIDs,
/// see flush_tlb_page)
pub fn flush_tlb_range(start: usize, end: usize, asid: u16) {
    let mut va = start & !0xFFF;
    while va < end {
        let pn = ((va >> 12) & 0x0000_FFFF_FFFF_F) as u64;
        // SAFETY: per-page invalidation (see flush_tlb_page).
        unsafe {
            if asid == 0 {
                asm!(
                    "tlbi vaae1is, {v}",
                    v = in(reg) pn,
                    options(nostack, preserves_flags)
                );
            } else {
                asm!(
                    "tlbi vae1is, {v}",
                    v = in(reg) pn | (((asid & 0xFF) as u64) << 48),
                    options(nostack, preserves_flags)
                );
            }
        }
        va += 0x1000;
    }
    // SAFETY: completion barrier after the loop.
    unsafe {
        asm!("dsb ish", "isb", options(nostack, preserves_flags));
    }
}

/// Flush kernel-half (global) entries
pub fn flush_tlb_kernel() {
    // SAFETY: full flush is the conservative kernel-halves choice
    // (aarch64 has no separate kernel-only tlbi op at EL1 short of
    // vmalle1; kernel mappings are global/nG=0 so this is correct).
    unsafe {
        asm!(
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nostack, preserves_flags)
        );
    }
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

/// Read the composed value (TTBR0_EL1 + CONTEXTIDR_EL1)
pub fn read_satp() -> usize {
    super::pagetable::get_satp().0 as usize
}

/// Write the composed value: switch TTBR0_EL1 (root PPA + ASID in
/// [63:48], the TCR_EL1.A1=0 tag source) with a full flush. The
/// switch_mm path may prefer ASID-scoped invalidation once generation
/// rollover lands (stage 2); today every switch flushes.
pub fn write_satp(satp: usize) {
    let ppn = satp_to_ppn(satp) as u64;
    let asid = (satp_to_asid(satp) & 0xFF) as u64;
    let ttbr0 = (ppn << 12) & 0x0000_FFFF_FFFF_F000 | (asid << 48);
    // SAFETY: TTBR0 switch (root + ASID) with full invalidation. The
    // order matches the canonical arm64 switch_mm: root write, barrier,
    // invalidation, context-sync.
    unsafe {
        asm!(
            "msr ttbr0_el1, {t}",
            "dsb ish",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            t = in(reg) ttbr0,
            options(nostack)
        );
    }
}

// ==================== ASID context helper ====================

/// RAII-ish ASID context (riscv64 parity)
pub struct AsidContext {
    asid: AtomicU16,
    generation: core::sync::atomic::AtomicU64,
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
