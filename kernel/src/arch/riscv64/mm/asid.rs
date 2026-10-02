//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Address Space ID (ASID) Management
//!
//! ASID is used to tag TLB entries, allowing multiple address spaces
//! to coexist in the TLB without requiring a full TLB flush on context switch.
//!
//! RISC-V Sv39 supports ASIDs up to 9 bits (512 ASIDs), but the actual
//! number of bits is determined by the hardware implementation.

use core::sync::atomic::{AtomicU64, AtomicU16, Ordering};

/// Maximum number of ASIDs supported (9 bits = 512)
pub const ASID_BITS: usize = 9;
pub const MAX_ASID: u16 = (1 << ASID_BITS) - 1;

/// Reserved ASIDs
pub const ASID_KERNEL: u16 = 0;      // Kernel uses ASID 0
pub const ASID_RESERVED: u16 = 1;    // Reserved for early boot

/// First usable ASID for user processes
pub const ASID_FIRST: u16 = 2;

// ==================== ASID Allocator ====================

/// Number of 64-bit words covering MAX_ASID+1 bits (512 ASIDs -> 8 words).
/// The old single-AtomicU64 bitmap only covered 64 ASIDs while the scan
/// ran to MAX_ASID — with 64+ concurrent processes `1u64 << i` overflowed
/// and PANICKED the kernel (LTP pipe13: 50 children + waiter processes).
const ASID_WORDS: usize = (MAX_ASID as usize + 1 + 63) / 64;

/// Global ASID allocator state: word 0 starts with ASID 0 and 1 reserved.
static ASID_BITMAP: [AtomicU64; ASID_WORDS] = {
    let mut words = [const { AtomicU64::new(0) }; ASID_WORDS];
    words[0] = AtomicU64::new(0b11); // ASID 0 and 1 reserved
    words
};

/// Allocate a new ASID
///
/// Returns the allocated ASID, or None if no ASIDs are available.
/// Lock-free: per-word CAS retry.
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
        // Every word is full (or holds only reserved/out-of-range bits).
        return None;
    }
}

/// Free an ASID
///
/// # Safety: Caller must ensure the ASID is no longer in use and TLB entries
/// for this ASID have been flushed.
pub fn free_asid(asid: u16) {
    if asid < ASID_FIRST || asid > MAX_ASID {
        return;
    }

    let w = (asid / 64) as usize;
    let mask = 1u64 << (asid % 64);
    ASID_BITMAP[w].fetch_and(!mask, Ordering::Release);
}

/// Get ASID usage count
pub fn asid_usage_count() -> u32 {
    let mut used = 0u32;
    for w in 0..ASID_WORDS {
        used += ASID_BITMAP[w].load(Ordering::Acquire).count_ones();
    }
    used
}

// ==================== TLB Flush Operations ====================

/// Flush all TLB entries (global TLB flush)
///
/// This is the most expensive TLB operation as it invalidates
/// all TLB entries across all ASIDs.
#[inline(always)]
pub fn flush_tlb_all() {
    // SAFETY: sfence.vma with zero,zero is a global TLB flush — always safe.
    unsafe {
        core::arch::asm!(
            "sfence.vma zero, zero",
            options(nostack, nomem)
        );
    }
}

/// Flush TLB entries for a specific ASID
///
/// Invalidates all TLB entries tagged with the given ASID.
#[inline(always)]
pub fn flush_tlb_asid(asid: u16) {
    // SAFETY: sfence.vma with zero,asid flushes all TLB entries for the given ASID.
    unsafe {
        core::arch::asm!(
            "sfence.vma zero, {0}",
            in(reg) asid,
            options(nostack, nomem)
        );
    }
}

/// Flush a single page from TLB
///
/// Invalidates the TLB entry for the given virtual address
/// in the specified ASID.
#[inline(always)]
pub fn flush_tlb_page(vaddr: usize, asid: u16) {
    // SAFETY: sfence.vma with a specific vaddr and asid invalidates a single TLB entry.
    unsafe {
        core::arch::asm!(
            "sfence.vma {0}, {1}",
            in(reg) vaddr,
            in(reg) asid,
            options(nostack, nomem)
        );
    }
}

/// Flush a range of pages from TLB
///
/// Invalidates TLB entries for a range of virtual addresses.
/// Uses page-granularity flushes.
pub fn flush_tlb_range(start: usize, end: usize, asid: u16) {
    const PAGE_SIZE: usize = 4096;
    let mut addr = start & !(PAGE_SIZE - 1);
    while addr < end {
        flush_tlb_page(addr, asid);
        addr += PAGE_SIZE;
    }
}

/// Flush kernel TLB entries (ASID 0)
#[inline(always)]
pub fn flush_tlb_kernel() {
    flush_tlb_asid(ASID_KERNEL);
}

// ==================== SATP Helpers ====================

/// Build SATP register value
///
/// # Arguments
/// - `asid`: Address Space ID
/// - `ppn`: Physical Page Number of root page table
///
/// # Returns
/// SATP register value for Sv39 mode
#[inline(always)]
pub fn build_satp(asid: u16, ppn: usize) -> usize {
    // SATP format for Sv39:
    // [63:60] MODE (8 = Sv39)
    // [59:44] ASID
    // [43:0]  PPN
    let mode: usize = 8;  // Sv39
    ((mode & 0xF) << 60) | ((asid as usize & 0xFFFF) << 44) | (ppn & 0xFFFFFFFFFFF)
}

/// Extract ASID from SATP value
#[inline(always)]
pub fn satp_to_asid(satp: usize) -> u16 {
    ((satp >> 44) & 0xFFFF) as u16
}

/// Extract PPN from SATP value
#[inline(always)]
pub fn satp_to_ppn(satp: usize) -> usize {
    satp & 0xFFFFFFFFFFF
}

/// Read current SATP value
#[inline(always)]
pub fn read_satp() -> usize {
    let satp: usize;
    // SAFETY: Reading the satp CSR is a simple register read with no side effects.
    unsafe {
        core::arch::asm!(
            "csrr {0}, satp",
            out(reg) satp,
            options(nostack, nomem)
        );
    }
    satp
}

/// Write SATP value
///
/// This changes the current page table and ASID.
/// A TLB flush is implicit when changing SATP.
#[inline(always)]
pub fn write_satp(satp: usize) {
    // SAFETY: Writing satp switches the page table root and ASID.
    // sfence.vma afterwards flushes the TLB, which is required after satp change.
    unsafe {
        core::arch::asm!(
            "csrw satp, {0}",
            "sfence.vma",
            in(reg) satp,
            options(nostack, nomem)
        );
    }
}

// ==================== ASID Context ====================

/// ASID context for a process
///
/// Each process has its own ASID context that tracks:
/// - The allocated ASID
/// - Generation counter for ASID reuse detection
pub struct AsidContext {
    /// Allocated ASID (0 means not allocated)
    asid: AtomicU16,
    /// Generation counter for ASID validation
    generation: AtomicU64,
}

impl AsidContext {
    /// Create a new ASID context (no ASID allocated)
    pub const fn new() -> Self {
        Self {
            asid: AtomicU16::new(0),
            generation: AtomicU64::new(0),
        }
    }

    /// Get the current ASID
    pub fn asid(&self) -> u16 {
        self.asid.load(Ordering::Acquire)
    }

    /// Allocate an ASID for this context
    pub fn alloc(&self) -> Option<u16> {
        let asid = alloc_asid()?;
        self.asid.store(asid, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Release);
        Some(asid)
    }

    /// Free the ASID
    ///
    /// # Safety
    /// Caller must ensure this ASID is no longer in use
    pub fn free(&self) {
        let asid = self.asid.swap(0, Ordering::AcqRel);
        if asid != 0 {
            flush_tlb_asid(asid);
            free_asid(asid);
        }
    }

    /// Get generation counter
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

// ==================== Debug/Info ====================

/// Print ASID allocator status
pub fn print_asid_status() {
    let used = asid_usage_count();
    let free = (MAX_ASID - ASID_FIRST + 1) as u32 - used;

    crate::println!("ASID Status:");
    crate::println!("  Total ASIDs:   {}", MAX_ASID + 1);
    crate::println!("  Reserved:      {}", ASID_FIRST);
    crate::println!("  Used:          {}", used);
    crate::println!("  Free:          {}", free);
    for w in 0..ASID_WORDS {
        crate::println!("  Bitmap[{}]:     {:#018x}", w, ASID_BITMAP[w].load(Ordering::Acquire));
    }
}
