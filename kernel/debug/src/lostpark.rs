//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The lostpark REPORT half (moved from kernel/core/src/debug/lostpark.rs,
//! crate-flip rung debug): the one-shot ring dump. armed()/record()
//! and the RING/SEQ statics stay framework-side (wait/sysv_sem event
//! paths record into them). Body moved verbatim; the framework keeps
//! a same-named wrapper dispatching this fn through the hook row.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::debug::lostpark::{RING, RING_LEN, SEQ};

static DUMPED: AtomicBool = AtomicBool::new(false);

/// Dump the ring once (called from the wakewatch heartbeat's frozen-state
/// branch — a printk-safe softirq context). One line per entry, ordered
/// by seq; `tag` prefixes the block.
pub fn dump_once(tag: &[u8]) {
    if !crate::debug::lostpark::armed() || DUMPED.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::trace::trace_lostpark(0, 0, SEQ.load(Ordering::Relaxed).min(RING_LEN as u64), 0);
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
