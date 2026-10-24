//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/diskstats — the per-device I/O statistics (Linux's
//! fs/block.c `diskstats_show`).
//!
//! Linux's layout, one line per device:
//!
//! ```text
//!    7       0 loop0 14 0 34 2 0 0 0 0 0 1 2 0 0 0 0 0 0
//! ```
//!
//! Major, minor, name, then: reads completed, reads merged, sectors
//! read, ms reading, writes completed, writes merged, sectors written,
//! ms writing, I/Os in progress, ms doing I/O, weighted ms doing I/O,
//! discards completed, discards merged, sectors discarded, ms
//! discarding, flush requests, ms flushing.
//!
//! Rux merges no requests, has no discard path and no separate flush
//! path, so those columns report zero.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Generate /proc/diskstats content.
pub fn generate() -> Vec<u8> {
    let mut out = String::new();
    for stat in crate::drivers::blkdev::iostat::snapshot().iter() {
        // Column order is Linux's: writes COMPLETED is field 8 and
        // writes MERGED is field 9 (the merged columns are the literal
        // zeros — Rux merges no requests).
        out.push_str(&format!(
            "{} {} {} {} 0 {} {} {} 0 {} {} {} {} {} 0 0 0 0 0 0\n",
            stat.major,
            stat.minor,
            stat.name,
            stat.reads,
            stat.sectors_read,
            stat.ms_reading,
            stat.writes,
            stat.sectors_written,
            stat.ms_writing,
            stat.io_in_progress,
            stat.ms_doing_io,
            // Linux's weighted column counts the I/O time weighted by the
            // number of tasks in the queue; Rux has no queue depth
            // weighting, so it matches the plain I/O time.
            stat.ms_doing_io,
        ));
    }
    out.into_bytes()
}
