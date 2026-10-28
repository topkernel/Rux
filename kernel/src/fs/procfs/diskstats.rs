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

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Generate /proc/diskstats content.
pub fn generate() -> Vec<u8> {
    let mut out = String::new();
    for stat in crate::drivers::blkdev::iostat::snapshot().iter() {
        // Linux's identity triple (major, minor, name) is the row label;
        // the 17 counter columns behind it are the macro's declaration
        // list — the merged/discard/flush zeros are named columns in
        // that list, not anonymous `0` tokens lost inside a format
        // string (the shifted-column incident class of D-4).
        out.push_str(&diskstats_row(
            &format!("{} {} {}", stat.major, stat.minor, stat.name),
            stat.reads,
            0,
            stat.sectors_read,
            stat.ms_reading,
            stat.writes,
            0,
            stat.sectors_written,
            stat.ms_writing,
            stat.io_in_progress,
            stat.ms_doing_io,
            // Linux's weighted column counts the I/O time weighted by the
            // number of tasks in the queue; Rux has no queue depth
            // weighting, so it matches the plain I/O time.
            stat.ms_doing_io,
            0,
            0,
            0,
            0,
            0,
            0,
        ));
    }
    out.into_bytes()
}

crate::procfile! {
    /// /proc/diskstats rows — Linux's 20-field layout (block/genhd.c
    /// diskstats_show): the device identity triple rides in the label,
    /// then reads/merges/sectors/ms, writes/merges/sectors/ms, in-flight,
    /// I/O time, weighted I/O time, discards (4) and flush (2). Rux
    /// merges no requests, has no discard path and no separate flush
    /// path, so those columns are the literal zeros passed at the call
    /// site — declared by name here, in Linux's field order.
    rowfn diskstats_row(label: &str) columns DISKSTAT_COLUMNS test diskstats_row_shape
        -> alloc::string::String cols {
        "reads"              => reads,
        "reads_merged"       => reads_merged,
        "sectors_read"       => sectors_read,
        "ms_reading"         => ms_reading,
        "writes"             => writes,
        "writes_merged"      => writes_merged,
        "sectors_written"    => sectors_written,
        "ms_writing"         => ms_writing,
        "io_in_progress"     => io_in_progress,
        "ms_doing_io"        => ms_doing_io,
        "ms_doing_io_weight" => ms_doing_io_weight,
        "discards"           => discards,
        "discards_merged"    => discards_merged,
        "sectors_discarded"  => sectors_discarded,
        "ms_discarding"      => ms_discarding,
        "flush_requests"     => flush_requests,
        "ms_flushing"        => ms_flushing,
    }
}

/// Conversion pins for /proc/diskstats (D-4 slice): the macro's shape
/// floor covers token count and spacing; this snapshot pins the exact
/// row BYTES — 20 single-space-separated fields, the zeros in Linux's
/// merged/discard/flush slots — so a column added, dropped or swapped
/// in the declaration fails here, not in a user's parser. Callable body
/// for the in-kernel unit-test suite.
#[cfg(any(test, feature = "unit-test"))]
pub mod diskstats_file_tests {
    pub fn snapshot_check() {
        let got = super::diskstats_row(
            "254 0 vda",
            14, 0, 34, 2,      // reads family
            7, 0, 19, 3,       // writes family
            1, 12, 12,         // in-flight, io time, weighted
            0, 0, 0, 0,        // discards
            0, 0,              // flush
        );
        assert_eq!(
            got,
            "254 0 vda 14 0 34 2 7 0 19 3 1 12 12 0 0 0 0 0 0\n",
            "/proc/diskstats row bytes drifted from Linux's 20-field layout"
        );
        // Field-count floor: 3 identity tokens + the declared columns.
        assert_eq!(super::DISKSTAT_COLUMNS.len(), 17, "declared column count drifted");
        let tokens = got.trim_end_matches('\n')
            .split(' ').filter(|t| !t.is_empty()).count();
        assert_eq!(tokens, 3 + super::DISKSTAT_COLUMNS.len(), "/proc/diskstats field count drifted");
    }

    #[cfg(test)]
    #[test]
    fn diskstats_snapshot() {
        snapshot_check();
    }
}
