//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Block device I/O statistics — the counters behind Linux's
//! `/proc/diskstats` (Linux keeps them in the block layer,
//! `block/genhd.c`; so does Rux).
//!
//! Linux's /proc/diskstats prints one line per device:
//!
//! ```text
//!    7       0 loop0 14 0 34 2 0 0 0 0 0 1 2 0 0 0 0 0 0
//! ```
//!
//! The columns after the device name are: reads completed, reads merged,
//! sectors read, milliseconds spent reading, writes completed, writes
//! merged, sectors written, milliseconds spent writing, I/Os in progress,
//! milliseconds spent doing I/O, weighted milliseconds spent doing I/O,
//! discards completed, discards merged, sectors discarded, milliseconds
//! spent discarding, flush requests and milliseconds spent flushing.
//!
//! Rux has no request merging, no discards and no separate flush path,
//! so those columns report zero; the read/write/sector/time columns are
//! real.

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::sync::spinlock::Spinlock;

/// One device's I/O counters.
pub struct DiskStat {
    /// Major device number.
    pub major: u32,
    /// First minor device number.
    pub minor: u32,
    /// Device name (as in /proc/partitions).
    pub name: String,
    /// Reads completed.
    pub reads: u64,
    /// Sectors read.
    pub sectors_read: u64,
    /// Milliseconds spent reading.
    pub ms_reading: u64,
    /// Writes completed.
    pub writes: u64,
    /// Sectors written.
    pub sectors_written: u64,
    /// Milliseconds spent writing.
    pub ms_writing: u64,
    /// I/Os in progress right now.
    pub io_in_progress: u64,
    /// Milliseconds spent doing I/O.
    pub ms_doing_io: u64,
}

/// The live counters of one device.
struct Entry {
    major: u32,
    minor: u32,
    name: String,
    reads: AtomicU64,
    sectors_read: AtomicU64,
    ms_reading: AtomicU64,
    writes: AtomicU64,
    sectors_written: AtomicU64,
    ms_writing: AtomicU64,
    io_in_progress: AtomicU64,
    ms_doing_io: AtomicU64,
}

static STATS: Spinlock<Vec<Entry>> = Spinlock::new(Vec::new());

/// Register a device so it shows up in /proc/diskstats.
pub fn register_device(major: u32, minor: u32, name: &str) {
    let mut stats = STATS.lock();
    if stats.iter().any(|e| e.major == major && e.minor == minor) {
        return;
    }
    stats.push(Entry {
        major,
        minor,
        name: name.to_string(),
        reads: AtomicU64::new(0),
        sectors_read: AtomicU64::new(0),
        ms_reading: AtomicU64::new(0),
        writes: AtomicU64::new(0),
        sectors_written: AtomicU64::new(0),
        ms_writing: AtomicU64::new(0),
        io_in_progress: AtomicU64::new(0),
        ms_doing_io: AtomicU64::new(0),
    });
}

/// Count one completed I/O.
///
/// `sectors` is the transfer size in 512-byte sectors and `elapsed_ns`
/// the time it took.
pub fn note_io(major: u32, minor: u32, is_write: bool, sectors: u64, elapsed_ns: u64) {
    let stats = STATS.lock();
    let Some(entry) = stats
        .iter()
        .find(|e| e.major == major && e.minor == minor)
    else {
        return;
    };
    let ms = elapsed_ns / 1_000_000;
    if is_write {
        entry.writes.fetch_add(1, Ordering::Relaxed);
        entry.sectors_written.fetch_add(sectors, Ordering::Relaxed);
        entry.ms_writing.fetch_add(ms, Ordering::Relaxed);
    } else {
        entry.reads.fetch_add(1, Ordering::Relaxed);
        entry.sectors_read.fetch_add(sectors, Ordering::Relaxed);
        entry.ms_reading.fetch_add(ms, Ordering::Relaxed);
    }
    entry.ms_doing_io.fetch_add(ms, Ordering::Relaxed);
}

/// One I/O in flight (Linux's "I/Os in progress" column).
pub fn note_start(major: u32, minor: u32) {
    let stats = STATS.lock();
    if let Some(entry) = stats.iter().find(|e| e.major == major && e.minor == minor) {
        entry.io_in_progress.fetch_add(1, Ordering::Relaxed);
    }
}

/// The I/O finished.
pub fn note_end(major: u32, minor: u32) {
    let stats = STATS.lock();
    if let Some(entry) = stats.iter().find(|e| e.major == major && e.minor == minor) {
        entry.io_in_progress.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A snapshot of every device's counters.
pub fn snapshot() -> Vec<DiskStat> {
    let stats = STATS.lock();
    stats
        .iter()
        .map(|e| DiskStat {
            major: e.major,
            minor: e.minor,
            name: e.name.clone(),
            reads: e.reads.load(Ordering::Relaxed),
            sectors_read: e.sectors_read.load(Ordering::Relaxed),
            ms_reading: e.ms_reading.load(Ordering::Relaxed),
            writes: e.writes.load(Ordering::Relaxed),
            sectors_written: e.sectors_written.load(Ordering::Relaxed),
            ms_writing: e.ms_writing.load(Ordering::Relaxed),
            io_in_progress: e.io_in_progress.load(Ordering::Relaxed),
            ms_doing_io: e.ms_doing_io.load(Ordering::Relaxed),
        })
        .collect()
}
