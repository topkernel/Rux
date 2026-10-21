//! Buddy-meta shadow scan (dfx=metawatch) — wildwrite-hunt line.
//!
//! The unified-engine hypothesis (session 4-12 archives) gives the wild
//! writer two downstream faces: kernel-STATIC victims (watched by the
//! gdb watchpoint chain) and buddy-heap METADATA corruption — the
//! ww-v2 order-18 grind face, where an impossible coalescing scale ran
//! with only ~229 MB in use. The buddy metadata (free-list links +
//! per-page free bitmap) is megabytes and cannot be watchpointed; this
//! module is its in-kernel witness.
//!
//! On a 10 s timer cadence the allocator derives a shadow free set from
//! the free lists (see mm::buddy_allocator::buddy_meta_shadow_scan) and
//! cross-checks INV-HEAP-1/2/3. Pass 1 is lock-free (transient
//! mid-mutation states are expected under churn); any dirty pass 1 is
//! re-audited UNDER the heap irqsave lock, and only a PERSISTENT
//! mismatch is reported — a META-SMASH print with the finding details.
//! A META-SMASH on any boot is the buddy-face capture for the hunt:
//! record the boot, correlate with the watchpoint logs.
//!
//! Discipline: one arming print per boot; ZERO traffic until a hit
//! (transient pass-1 states are counted, never printed).

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::mm::buddy_allocator::{self, MetaFinding, EMPTY_FINDING};

/// Scan cadence in jiffies (100 Hz clock → 10 s). One scan sweeps the
/// 4096-word bitmap three times plus the free-list walk — low-single-
/// digit ms under TCG, ~0.05% of wall time. Corruption is persistent,
/// so detection latency costs nothing.
const SCAN_PERIOD: u64 = 1000;
/// Findings buffered per scan (softirq stack discipline — 8 x ~48 B).
const MAX_FINDINGS: usize = 8;
/// Rate limit for repeat META-SMASH summaries after the first full
/// report (jiffies).
const REPEAT_LIMIT: u64 = 600;

static LAST_SCAN: AtomicU64 = AtomicU64::new(0);
static SCANS: AtomicUsize = AtomicUsize::new(0);
static TRANSIENTS: AtomicUsize = AtomicUsize::new(0);
static FIRED: AtomicBool = AtomicBool::new(false);
static LAST_LOUD: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static SELFTEST_DONE: AtomicBool = AtomicBool::new(false);

/// Timer-softirq cadence entry (timer.rs calls this when the switch is
/// on; throttling is internal).
pub fn tick(current: u64) {
    if !SELFTEST_DONE.swap(true, Ordering::Relaxed) {
        // One-shot detector selftest (wildwatch-selftest precedent):
        // inject a single bad free_bits bit under the heap lock, expect
        // the audit to flag it, restore, expect clean. All before any
        // real scan — the boot either proves its tripwire or says so.
        if crate::cmdline::get_param("metawatch-selftest").is_some() {
            if buddy_allocator::buddy_meta_selftest() {
                crate::pr_err!(
                    "METAWATCH-SELFTEST PASS: injected free_bits bit flagged (BitmapClearListed) and restored, re-audit clean"
                );
            } else {
                crate::pr_err!(
                    "METAWATCH-SELFTEST FAIL: injected bit not flagged (or pre/post audit dirty) — detector broken, do not trust clean scans"
                );
            }
        }
        crate::pr_info!(
            "METAWATCH-ARMED period={} jiffies (buddy-meta shadow audit)",
            SCAN_PERIOD
        );
        ARMED.store(true, Ordering::Relaxed);
    }

    let last = LAST_SCAN.load(Ordering::Relaxed);
    if current.saturating_sub(last) < SCAN_PERIOD {
        return;
    }
    if LAST_SCAN
        .compare_exchange(last, current, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    scan(current);
}

fn scan(current: u64) {
    let mut findings = [EMPTY_FINDING; MAX_FINDINGS];
    match buddy_allocator::buddy_meta_shadow_scan(&mut findings) {
        // Another CPU's scan is in flight; corruption persists, so the
        // next cadence observes it too. Not a miss.
        None => {}
        Some(outcome) => {
            SCANS.fetch_add(1, Ordering::Relaxed);
            if outcome.confirmed_findings > 0 {
                report(&mut findings, outcome.confirmed_findings, current);
            } else if outcome.lockfree_findings > 0 {
                // Mid-mutation pass-1 state, clean under the lock —
                // expected under allocator churn. Counted, not printed.
                TRANSIENTS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

fn kind_str(kind: crate::mm::buddy_allocator::MetaFindKind) -> &'static str {
    use crate::mm::buddy_allocator::MetaFindKind::*;
    match kind {
        LinkOob => "LinkOob",
        OrderMismatch => "OrderMismatch",
        FreeByteClear => "FreeByteClear",
        Unaligned => "Unaligned",
        PrevBroken => "PrevBroken",
        BitmapSetUnlisted => "BitmapSetUnlisted",
        BitmapClearListed => "BitmapClearListed",
        DoubleListed => "DoubleListed",
        HopOverflow => "HopOverflow",
    }
}

fn report(findings: &mut [MetaFinding], confirmed: usize, current: u64) {
    let first = !FIRED.swap(true, Ordering::Relaxed);
    if first {
        crate::pr_err!(
            "META-SMASH confirmed={} scans={} transients={} (persistent buddy-meta INV-HEAP-* violation — the buddy face of the wild writer)",
            confirmed,
            SCANS.load(Ordering::Relaxed),
            TRANSIENTS.load(Ordering::Relaxed)
        );
        let shown = confirmed.min(findings.len());
        for f in findings[..shown].iter_mut() {
            crate::pr_err!(
                "META-SMASH kind={} order={} idx={} prev={:#x} next={:#x} word={} bits={:#x} derived={:#x}",
                kind_str(f.kind),
                f.order,
                f.idx,
                f.prev,
                f.next,
                f.word,
                f.bits,
                f.derived
            );
        }
        LAST_LOUD.store(current, Ordering::Relaxed);
        return;
    }
    // Rate-limited heartbeat so a long hunt boot shows the corruption
    // is still live without flooding the serial log.
    let last = LAST_LOUD.load(Ordering::Relaxed);
    if current.saturating_sub(last) >= REPEAT_LIMIT {
        LAST_LOUD.store(current, Ordering::Relaxed);
        crate::pr_err!(
            "META-SMASH still-live confirmed={} total_scans={} transients={}",
            confirmed,
            SCANS.load(Ordering::Relaxed),
            TRANSIENTS.load(Ordering::Relaxed)
        );
    }
}
