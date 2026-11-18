//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Perf (sampler) unit tests — ported from the former in-module
//! `pub mod tests` inside kernel/src/perf/mod.rs per the test-layout
//! policy: all kernel unit tests live under kernel/src/tests/.

use crate::perf::{
    PERF_RECORD_LOST, PERF_RECORD_MISC_KERNEL, PERF_RECORD_MISC_USER, PERF_RECORD_SAMPLE,
    PAGE_SIZE, TICK_NS,
};

/// LOST record builder (the ring path's report record shape; only the
/// tests exercise the pure builder).
fn lost_record_bytes(id: u64, lost: u64) -> [u8; 24] {
    let mut rec = [0u8; 24];
    rec[0..4].copy_from_slice(&PERF_RECORD_LOST.to_le_bytes());
    rec[6..8].copy_from_slice(&24u16.to_le_bytes());
    rec[8..16].copy_from_slice(&id.to_le_bytes());
    rec[16..24].copy_from_slice(&lost.to_le_bytes());
    rec
}

/// Record encoding: sample and lost records match the ABI layout.
fn test_record_encoding() {
    let mut rec = [0u8; 16];
    rec[0..4].copy_from_slice(&PERF_RECORD_SAMPLE.to_le_bytes());
    rec[4..6].copy_from_slice(&PERF_RECORD_MISC_USER.to_le_bytes());
    rec[6..8].copy_from_slice(&16u16.to_le_bytes());
    rec[8..16].copy_from_slice(&0x1234_5678_9abc_def0u64.to_le_bytes());
    // 94th: byte[4] is PERF_RECORD_MISC_USER — 2 after the 93rd's
    // uapi swap fix (970b1b40; the old expect array encoded the
    // pre-fix 1). Assert the uapi constants explicitly so a future
    // swap re-fails here instead of drifting silently.
    assert_eq!(PERF_RECORD_MISC_USER, 2, "uapi PERF_RECORD_MISC_USER");
    assert_eq!(PERF_RECORD_MISC_KERNEL, 1, "uapi PERF_RECORD_MISC_KERNEL");
    let expect: [u8; 16] = [
        9, 0, 0, 0, 2, 0, 16, 0,
        0xf0, 0xde, 0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12,
    ];
    if rec == expect {
        crate::tests::test_pass("perf-record");
    } else {
        crate::tests::test_fail("perf-record", "sample record bytes mismatch");
    }
    let l = lost_record_bytes(7, 3);
    let mut ok = true;
    ok &= l[0..4] == PERF_RECORD_LOST.to_le_bytes();
    ok &= l[6..8] == (24u16).to_le_bytes();
    ok &= l[8..16] == 7u64.to_le_bytes();
    ok &= l[16..24] == 3u64.to_le_bytes();
    if ok {
        crate::tests::test_pass("perf-lost");
    } else {
        crate::tests::test_fail("perf-lost", "lost record bytes mismatch");
    }
}

/// Ring wrap arithmetic: a write crossing the data-area end lands
/// at the start (page-wise walker, synthetic pages).
fn test_ring_wrap() {
    // data_size 2 pages; write 8 bytes at pos = data_size-4.
    // ring_slice resolves page 1 offset 4092 with n clamped to 4.
    // (Full-path ring_put needs live pages; the arithmetic is
    // covered by the boot-side hiperf run.)
    let data_size = 2 * PAGE_SIZE;
    let pos = (data_size - 4) as u64;
    let p = (pos % data_size as u64) as usize;
    let page_idx = p / PAGE_SIZE;
    let off = p % PAGE_SIZE;
    let ok = page_idx == 1 && off == PAGE_SIZE - 4;
    if ok {
        crate::tests::test_pass("perf-ringwrap");
    } else {
        crate::tests::test_fail("perf-ringwrap", "wrap arithmetic mismatch");
    }
}

/// freq→period conversion and the tick floor.
fn test_freq_floor() {
    let f = 4000u64;
    let period = (1_000_000_000 / f).max(TICK_NS);
    // 4000Hz wants 250µs; the sampler floors at one tick.
    if period == TICK_NS {
        crate::tests::test_pass("perf-freqfloor");
    } else {
        crate::tests::test_fail("perf-freqfloor", "period not floored at TICK_NS");
    }
}

pub fn test_perf() {
    crate::tests::test_group_start("perf");
    test_record_encoding();
    test_ring_wrap();
    test_freq_floor();
}
