//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Soft-wedge heartbeat (`rux:schb (retired debug=schb)`, sc-series quiet-quiet hunt, session R).
//!
//! The quiet-quiet boot (quietcon + loglevel=3) has a second failure shape
//! the classic-race probes cannot see: the pid flow stalls in the
//! 900-1400 zone from ~min 10-12 with NO rwlock freeze word and NO kernel
//! evidence (sc196/sc197; sc195 passes it 1-in-3). The evidence gap is
//! structural: `printk_bytes` drops every record above CONSOLE_LOGLEVEL
//! BEFORE the ring, so at loglevel=3 all pr_warn! census output
//! (SACMP, binder-stall, stkfutex, RWFRZ) never existed — muted is the
//! wrong model; it was never recorded.
//!
//! This module buys visibility back with a budget far below the
//! loglevel-4 watermark (the sc198 lesson: probe traffic modulates the
//! console race):
//!
//! - ONE ~90-byte heartbeat line every `BEAT_SECS` (10s), emitted from
//!   the schbd kthread's own context (never a racer thread) straight to
//!   the UART via `ConsoleWriter` — no printk ring write, no persistent-log
//!   disk I/O, nothing on any binder/futex critical path.
//! - `SC-PROC` lines name each new binder process as the heartbeat
//!   drains a ring that `binder_open` fills — the port-IO again lands
//!   in schbd, not the opener.
//! - When the pid flow plateaus for `PLATEAU_BEATS` beats (60s) past
//!   early boot, ONE census dump fires per episode (the wedge census).
//!
//! CRATE-FLIP RUNG rux-obs: the kthread, the wedge census and the OOM
//! anatomy moved to the rux-obs service crate (kernel/rux-obs/src/
//! schb.rs). THIS file keeps the per-event RECORD half — note_proc_open
//! (binder_open), note_cow/note_drct/note_slow (fault / direct-reclaim
//! / alloc-slowpath hot paths, one relaxed add each) — plus the pub
//! peek surface (PROC_RING/PROC_SEQ, pace_snapshot) the crate's beat
//! line reads, and the same-named wrappers (init, oom_anat_dump) that
//! dispatch the hook rows.

#![forbid(unsafe_code)]
use core::sync::atomic::{AtomicU64, Ordering};

/// Depth of the binder-proc open ring (one slot per binder_open; an OH
/// boot opens on the order of a hundred). PUB: the rux-obs crate's
/// schbd drains the ring through it.
pub const PROC_RING_LEN: usize = 128;

/// binder_open trace ring: (seq, pid, jiffies). Filled by `note_proc_open`
/// from the opener's context (spinlock push only — no I/O there); drained
/// and printed by schbd (rux-obs) so every byte of UART cost stays off
/// racer paths. PUB peek surface (crate-flip rung rux-obs).
pub static PROC_RING: crate::sync::spinlock::Spinlock<[(u64, u32, u64); PROC_RING_LEN]> =
    crate::sync::spinlock::Spinlock::new([(0, 0, 0); PROC_RING_LEN]);
/// PUB peek surface (rux-obs's schbd drains up to this sequence).
pub static PROC_SEQ: AtomicU64 = AtomicU64::new(0);

/// Record a binder-proc creation (called from `binder_open`). No-op
/// unless `rux:schb (retired debug=schb)`. Bounded cost: one switch load + one spinlock store.
pub fn note_proc_open(pid: u32) {
    if !crate::trace::EVENT_SCHB.armed() {
        return;
    }
    let seq = PROC_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let j = crate::timer::get_jiffies();
    let mut ring = PROC_RING.lock();
    ring[(seq as usize - 1) % PROC_RING_LEN] = (seq, pid, j);
}

// ===== Leg-15 pace counters (launcher round decomposition) =====
//
// sc210 measured ~11 s per launcher getOsAccountLocalId round = ~3000 COW
// faults + 40 forks, with the zone pinned at free~100 pages (wmark low
// 2451, in_direct_reclaim=1) — every COW fault's page alloc rides the
// serialized direct-reclaim slowpath. These cumulative counters decompose
// the round cost from the heartbeat alone: avg ms/fault = 1000/KERNEL_HZ *
// d(cowJ)/d(cow), reclaim share = d(drctJ)/beat-jiffies, regime =
// zfree/heapK trajectory. All sites are fault/alloc hot paths: ONE
// relaxed fetch_add per event, no locks held at any call site, no I/O —
// the sc198 lesson (probe traffic modulates the race) is about prints;
// the only UART cost stays in schbd's beat (rux-obs).

static PACE_COW: AtomicU64 = AtomicU64::new(0);
static PACE_COW_J: AtomicU64 = AtomicU64::new(0);
static PACE_DRCT: AtomicU64 = AtomicU64::new(0);
static PACE_DRCT_J: AtomicU64 = AtomicU64::new(0);
static PACE_DRCT_MAXJ: AtomicU64 = AtomicU64::new(0);
static PACE_SLOW: AtomicU64 = AtomicU64::new(0);

/// Record one COW fault resolution attempt. `j0` is the jiffies value
/// captured at handle_cow_fault entry (callers pass it so the exit-side
/// cost stays one read + adds). No-op unless `rux:schb (retired debug=schb)`.
pub fn note_cow(j0: u64) {
    if !crate::trace::EVENT_SCHB.armed() {
        return;
    }
    PACE_COW.fetch_add(1, Ordering::Relaxed);
    let d = crate::timer::get_jiffies().saturating_sub(j0);
    PACE_COW_J.fetch_add(d, Ordering::Relaxed);
}

/// Record one direct-reclaim round (try_to_free_pages), same shape.
pub fn note_drct(j0: u64) {
    if !crate::trace::EVENT_SCHB.armed() {
        return;
    }
    PACE_DRCT.fetch_add(1, Ordering::Relaxed);
    let d = crate::timer::get_jiffies().saturating_sub(j0);
    PACE_DRCT_J.fetch_add(d, Ordering::Relaxed);
    PACE_DRCT_MAXJ.fetch_max(d, Ordering::Relaxed);
}

/// Record one page-alloc slowpath entry (zone fastpath failed).
pub fn note_slow() {
    if !crate::trace::EVENT_SCHB.armed() {
        return;
    }
    PACE_SLOW.fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the pace counters for the beat line (schbd context only).
/// PUB peek surface (crate-flip rung rux-obs): the rux-obs crate's
/// schbd reads this for its SC-HB line.
pub fn pace_snapshot() -> (u64, u64, u64, u64, u64, u64) {
    (
        PACE_COW.load(Ordering::Relaxed),
        PACE_COW_J.load(Ordering::Relaxed),
        PACE_DRCT.load(Ordering::Relaxed),
        PACE_DRCT_J.load(Ordering::Relaxed),
        PACE_DRCT_MAXJ.load(Ordering::Relaxed),
        PACE_SLOW.load(Ordering::Relaxed),
    )
}

/// Start the schbd kthread (moved to the rux-obs crate): same-named
/// wrapper for the late-boot call site; dispatches the hook row. Gated
/// on the `rux:schb (retired debug=schb)` switch inside the crate's body.
pub fn init() {
    crate::debug::obs_hooks::schb_init();
}

/// One-shot allocator anatomy at the first fake-OOM floor (moved to the
/// rux-obs crate): same-named wrapper for the OOM killer's
/// no-killable-processes path (rux-mm's oom_kill); dispatches the hook
/// row.
pub fn oom_anat_dump() {
    crate::debug::obs_hooks::schb_oom_anat_dump();
}
