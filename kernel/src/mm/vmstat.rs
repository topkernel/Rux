//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel VM statistics — the counters behind Linux's /proc/vmstat.
//!
//! The counters live here (the memory manager owns them, like Linux's
//! mm/vmstat.c); `fs/procfs/vmstat.rs` renders them in Linux's
//! key/value format.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::mm::zone::ZoneType;

/// Page faults taken (Linux's `pgfault`).
pub static PGFAULT: AtomicU64 = AtomicU64::new(0);

/// Page faults that had to go to the backing store (Linux's
/// `pgmajfault`).
///
/// Rux populates file-backed pages from the page cache rather than by
/// reading from the block device inside the fault handler, so there is
/// no fault-time disk read to classify yet — the counter stays 0 until
/// that path exists. It is wired (`note_major_fault`) so the plumbing
/// is in place when it does.
pub static PGMAJFAULT: AtomicU64 = AtomicU64::new(0);

/// Pages returned to the free lists (Linux's `pgfree`).
pub static PGFREE: AtomicU64 = AtomicU64::new(0);

/// Pages allocated from the Normal zone (Linux's `pgalloc_normal`).
pub static PGALLOC_NORMAL: AtomicU64 = AtomicU64::new(0);

/// Pages allocated from the Movable zone (Linux's `pgalloc_movable`).
pub static PGALLOC_MOVABLE: AtomicU64 = AtomicU64::new(0);

/// Pages allocated from the DMA32 zone (Linux's `pgalloc_dma32`).
pub static PGALLOC_DMA32: AtomicU64 = AtomicU64::new(0);

/// Pages allocated from the DMA zone (Linux's `pgalloc_dma`).
pub static PGALLOC_DMA: AtomicU64 = AtomicU64::new(0);

/// Count one page fault.
pub fn note_fault() {
    PGFAULT.fetch_add(1, Ordering::Relaxed);
}

/// Count one major page fault.
pub fn note_major_fault() {
    PGMAJFAULT.fetch_add(1, Ordering::Relaxed);
}

/// Count `pages` pages freed back to the buddy free lists.
pub fn note_free(pages: u64) {
    PGFREE.fetch_add(pages, Ordering::Relaxed);
}

/// Count `pages` pages allocated from `zone`.
pub fn note_alloc(zone: ZoneType, pages: u64) {
    let counter = match zone {
        ZoneType::ZoneNormal => &PGALLOC_NORMAL,
        ZoneType::ZoneMovable => &PGALLOC_MOVABLE,
        ZoneType::ZoneDma32 => &PGALLOC_DMA32,
        ZoneType::ZoneDma => &PGALLOC_DMA,
        // ZoneCount is a sentinel, not a real zone.
        _ => return,
    };
    counter.fetch_add(pages, Ordering::Relaxed);
}
