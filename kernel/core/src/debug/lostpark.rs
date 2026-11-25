//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Lost-park event ring (fix-lostpark lane).
//!
//! The SMP lost-park wedge (lmbench lat_sem / semop ping-pong at -smp 2)
//! freezes a two-task IPC ping-pong with both vCPUs idle; every per-event
//! print drowns in the 100k+ healthy rounds that precede the wedge, and
//! the wedge itself is silent (no wake ever fires again). This module
//! keeps a small lock-free ring of the LAST scheduling-relevant events
//! (semop park enter/exit, refused wake tokens, semaphore value
//! transitions) so the 1 Hz wakewatch heartbeat can dump the final
//! window the moment it detects the frozen state (wakes delta == 0,
//! global queue empty).
//!
//! Everything is behind the `rux:lostpark` trace event (retired
//! record fast path is one Relaxed load when off.

#![forbid(unsafe_code)]

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Ring entries (keep modest: dumped on one serial line each).
pub const RING_LEN: usize = 64;

/// PUB peek surface (crate-flip rung debug): the debug report
/// half reads the ring.
pub static SEQ: AtomicU64 = AtomicU64::new(0);
static DUMPED: AtomicBool = AtomicBool::new(false);
pub static RING: [AtomicU64; RING_LEN * 5] = [const { AtomicU64::new(0) }; RING_LEN * 5];

/// Whether the ring is armed (fast record gate).
#[inline]
pub fn armed() -> bool {
    crate::trace::EVENT_LOSTPARK.armed()
}

/// Record one event: (event, pid, a, b, jiffies). Lock-free; events may
/// interleave under SMP but each slot's seq makes the order recoverable.
pub fn record(event: u64, pid: u64, a: u64, b: u64, jiffies: u64) {
    if !armed() {
        return;
    }
    let s = SEQ.fetch_add(1, Ordering::Relaxed);
    let i = (s as usize % RING_LEN) * 5;
    RING[i].store(s, Ordering::Relaxed);
    RING[i + 1].store(event, Ordering::Relaxed);
    RING[i + 2].store(pid, Ordering::Relaxed);
    RING[i + 3].store(a, Ordering::Relaxed);
    RING[i + 4].store(b | (jiffies << 32), Ordering::Relaxed);
}
/// Dump the ring once (moved to the debug crate): same-named wrapper
/// for the wakewatch heartbeat's frozen-state branch; dispatches the
/// hook row.
pub fn dump_once(tag: &[u8]) {
    crate::api::debug::lostpark_dump_once(tag);
}
