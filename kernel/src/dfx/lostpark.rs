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
//! Everything is behind `dfx=lostpark` (DfxSwitch::LostParkTrace): the
//! record fast path is one Relaxed load when off.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Ring entries (keep modest: dumped on one serial line each).
pub const RING_LEN: usize = 64;

static SEQ: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static DUMPED: AtomicBool = AtomicBool::new(false);
static RING: [AtomicU64; RING_LEN * 5] = [const { AtomicU64::new(0) }; RING_LEN * 5];

/// Arm the ring (called when the dfx switch flips on at boot).
pub fn arm() {
    ARMED.store(true, Ordering::Release);
}

/// Whether the ring is armed (fast record gate).
#[inline]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
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

/// Dump the ring once (called from the wakewatch heartbeat's frozen-state
/// branch — a printk-safe softirq context). One line per entry, ordered
/// by seq; `tag` prefixes the block.
pub fn dump_once(tag: &[u8]) {
    if !armed() || DUMPED.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut idx = [0usize; RING_LEN];
    for k in 0..RING_LEN {
        idx[k] = k;
    }
    let seq_of = |k: usize| RING[k * 5].load(Ordering::Relaxed);
    idx.sort_by_key(|&k| seq_of(k));
    let mut tag_s = [0u8; 16];
    let tn = tag.len().min(16);
    tag_s[..tn].copy_from_slice(&tag[..tn]);
    let tag_str = core::str::from_utf8(&tag_s[..tn]).unwrap_or("frozen");
    crate::pr_info!("=== LOSTPARK RING ({}) ===", tag_str);
    for k in idx {
        let s = RING[k * 5].load(Ordering::Relaxed);
        let ev = RING[k * 5 + 1].load(Ordering::Relaxed);
        let pid = RING[k * 5 + 2].load(Ordering::Relaxed);
        let a = RING[k * 5 + 3].load(Ordering::Relaxed);
        let bj = RING[k * 5 + 4].load(Ordering::Relaxed);
        if s == 0 && ev == 0 {
            continue;
        }
        crate::pr_info!(
            "LP seq={} ev={} pid={} a={:#x} b={:#x} j={}",
            s,
            ev,
            pid,
            a,
            bj & 0xFFFF_FFFF,
            bj >> 32
        );
    }
    crate::pr_info!("=== LOSTPARK RING END ===");
}
