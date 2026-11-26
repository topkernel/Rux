//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The driftwatch REPORT half (moved from kernel/core/src/debug/driftwatch.rs,
//! crate-flip rung debug): the per-site DRIFTWATCH lines. The
//! counters (pub statics) and note_zero_put stay framework-side (mm
//! free paths write them). Body moved verbatim.

use core::sync::atomic::Ordering;

use crate::debug::hotpath::*;

/// One line per nonzero counter (printk path — ledger context only).
pub fn report() {
    for (i, c) in ZERO_PUT.iter().enumerate() {
        let v = c.load(Ordering::Relaxed);
        if v != 0 {
            crate::println!("DRIFTWATCH zero_put[{}] {}={}", i, DRIFTWATCH_SITE_NAMES[i], v);
        }
    }
    let v = FREE_OK.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_ok={}", v);
    }
    let v = FREE_REFUSED_MAPPED.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_refused_mapped={}", v);
    }
    let v = FREE_DROPPED_WILD.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_dropped_wild={}", v);
    }
    let v = FREE_REFUSED_RESERVED.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_refused_reserved={}", v);
    }
    let v = FREE_REFUSED_TEXT.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_refused_text={}", v);
    }
    let v = FREE_NO_ZONE.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_no_zone={}", v);
    }
    let v = FREE_NULL.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH free_null={}", v);
    }
    let v = PUT_UNDERFLOW.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH put_underflow={}", v);
    }
    let v = PCACHE_LEAF_PHANTOM_PUT.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH pcache_leaf_phantom_put={}", v);
    }
    let v = PCACHE_LIVE_PT_REFUSED.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH pcache_live_pt_refused={}", v);
    }
    let v = MM_POISONED.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH mm_poisoned={}", v);
    }
    let v = MM_POISON_FREES.load(Ordering::Relaxed);
    if v != 0 {
        crate::println!("DRIFTWATCH mm_poison_frees_refused={}", v);
    }
}
