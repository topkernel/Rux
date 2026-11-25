//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Lock-free scheduler event ring (Rule 1, grq-lock-1st).
//!
//! THE RULE (user directive): scheduler hot paths — the GRQ section and
//! every path it runs on — carry NO prints. Invariant checks become
//! `debug_assert!`/`assert!`; diagnostic evidence becomes ONE WORD here.
//!
//! WHY: every print under the GRQ was either printk (which used to
//! re-acquire the GRQ through the console wake — the convicted
//! same-cpu deadlock) or the staged console (`putc_brief`), which pays
//! a stage-lock plus real serial baud time INSIDE the IRQs-off critical
//! section. A wedged-system dump is the one sanctioned slow path
//! (release-policy qualified); everything else records an event and
//! returns.
//!
//! THE SHAPE: a fixed ring of `(tag, pid, a, b, jiffies)` records.
//! Recording is one `fetch_add` plus five plain stores plus one Release
//! publish — no lock, no console, no scheduler entry, safe from any
//! context including the GRQ section itself. The drain runs from the
//! tick (outside every GRQ section; printk is Rule-2-safe there) with a
//! line budget, and prints each event with its original greppable tag
//! so the existing census tooling keeps working.
//!
//! Loss model: the ring holds [`EV_RING_SLOTS`] events; on overrun the
//! OLDEST event is overwritten (a diagnostic watermark, never a
//! correctness object) and [`EV_LOST`] counts the clobbers.

#![forbid(unsafe_code)]
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

/// Ring capacity. Large enough that a census burst (one event per
/// phantom heal, tens per boot) never overruns between ticks.
pub const EV_RING_SLOTS: usize = 128;

/// One recorded scheduler event.
#[repr(C)]
pub struct SchedEv {
    /// Event tag (the `EV_*` constants — the drain prints the name).
    pub tag: u32,
    /// The event's subject pid (0 = none / cpu-relative).
    pub pid: u32,
    /// First payload word (meaning per tag: qword, cpu, claim, ...).
    pub a: u64,
    /// Second payload word.
    pub b: u64,
    /// Jiffies at record time.
    pub jif: u64,
    /// Publication marker: 0 = being written, else record index + 1
    /// (the same one-word discipline as the printk ring).
    pub pub_seq: AtomicU64,
}

impl SchedEv {
    const fn empty() -> Self {
        Self {
            tag: 0,
            pid: 0,
            a: 0,
            b: 0,
            jif: 0,
            pub_seq: AtomicU64::new(0),
        }
    }
}

/// The ring substrate is the framework's generic (S1b): the Sync proof
/// and the UnsafeCell live side_edges-side; this alias binds the cell
/// type (the dual compilations share the framework type — identity
/// holds across the seam).
pub(crate) type EvRing = crate::sched::r#unsafe::EvRing<SchedEv, EV_RING_SLOTS>;
// The Sync proof lives in unsafe.rs (this file carries zero unsafe).

static EV_RING: EvRing = EvRing::new([const { SchedEv::empty() }; EV_RING_SLOTS]);

/// Reservation cursor: each `fetch_add` result is a unique record index.
static EV_CUR: AtomicU64 = AtomicU64::new(0);

/// Events clobbered by overrun (the watermark — diagnostics only).
pub static EV_LOST: AtomicU64 = AtomicU64::new(0);

// ---- Event tags (the drain prints these exact census prefixes) ----
pub const EV_ENQ_POISONED: u32 = 1; // "ENQ-POISONED-TASK"
pub const EV_ENQ_DEAD: u32 = 2; // "ENQ-DEAD-TASK"
pub const EV_DL_ENQ_DEFERRED: u32 = 3; // "DL-ENQ-DEFERRED"
pub const EV_R119_STALE_LIVE: u32 = 4; // "R119-STALE-LIVE-MARK-CLEARED"
pub const EV_R49_ONCPU_ORPHAN: u32 = 5; // "R49-ONCPU-ORPHAN healed"
pub const EV_R80_LIVE_PHANTOM: u32 = 6; // "R80-LIVE-PHANTOM-CLEARED"
pub const EV_R46_RUNNING_PHANTOM: u32 = 7; // "R46-RUNNING-PHANTOM healed"
pub const EV_W2_ENQ_HEAL_REFUSED: u32 = 8; // "W2-ENQUEUE-HEAL-REFUSED"
pub const EV_LP_WAKE_OUTCOME: u32 = 9; // "LP-WAKE-OUTCOME"
pub const EV_LP_WAKE_SKIP_EMPTY: u32 = 10; // "LP-WAKE-SKIP-EMPTY"
pub const EV_FREED_WHILE_LINKED: u32 = 11; // "FREED-WHILE-LINKED-UNLINKED"
pub const EV_NR_RUNNING_DESYNC: u32 = 12; // "NR-RUNNING-DESYNC"
pub const EV_R56_STEAL: u32 = 13; // "R56-STEAL"
pub const EV_R56_SLOT_CAPTURE: u32 = 14; // "R56-SLOT-CAPTURE-HEALED"
pub const EV_R56_QUEUED_MARK: u32 = 15; // "R56-QUEUED-MARK-CLEARED"
pub const EV_R80_LIVE_MARK: u32 = 16; // "R80-LIVE-MARK-CLEARED"
pub const EV_RQD_MIGRATE: u32 = 17; // "RQD-MIGRATE-DISPATCH"
pub const EV_RT_THROTTLED: u32 = 18; // "RT-CLASS-THROTTLED"

/// Record one scheduler event (the Rule-1 replacement for every
/// in-section print). ~10 ns: one fetch_add, five stores, one publish.
#[inline(always)]
pub fn sched_ev(tag: u32, pid: u32, a: u64, b: u64) {
    let idx = EV_CUR.fetch_add(1, Ordering::Relaxed);
    let slot = (idx as usize) % EV_RING_SLOTS;
    // SAFETY: EV_RING is a fixed static array; slot is bounds-checked by
    // the modulo; this call owns the slot until the publish store below
    // (unique fetch_add index). An overrun clobbers the oldest record —
    // counted, never a correctness object.
    let ev = crate::sched::r#unsafe::evring_slot_mut(&EV_RING, slot);
    if ev.pub_seq.load(Ordering::Relaxed) != 0 {
        EV_LOST.fetch_add(1, Ordering::Relaxed);
    }
    ev.pub_seq.store(0, Ordering::Relaxed);
    core::sync::atomic::fence(Ordering::Release);
    ev.tag = tag;
    ev.pid = pid;
    ev.a = a;
    ev.b = b;
    ev.jif = crate::timer::get_jiffies();
    ev.pub_seq.store(idx + 1, Ordering::Release);
}

/// The drain cursor (sequence of the next event to print).
static EV_DRAIN: AtomicU64 = AtomicU64::new(0);

/// Name a tag for the drain line (stable census prefixes).
fn tag_name(tag: u32) -> &'static str {
    match tag {
        EV_ENQ_POISONED => "ENQ-POISONED-TASK",
        EV_ENQ_DEAD => "ENQ-DEAD-TASK",
        EV_DL_ENQ_DEFERRED => "DL-ENQ-DEFERRED",
        EV_R119_STALE_LIVE => "R119-STALE-LIVE-MARK-CLEARED",
        EV_R49_ONCPU_ORPHAN => "R49-ONCPU-ORPHAN healed",
        EV_R80_LIVE_PHANTOM => "R80-LIVE-PHANTOM-CLEARED",
        EV_R46_RUNNING_PHANTOM => "R46-RUNNING-PHANTOM healed",
        EV_W2_ENQ_HEAL_REFUSED => "W2-ENQUEUE-HEAL-REFUSED",
        EV_LP_WAKE_OUTCOME => "LP-WAKE-OUTCOME",
        EV_LP_WAKE_SKIP_EMPTY => "LP-WAKE-SKIP-EMPTY",
        EV_FREED_WHILE_LINKED => "FREED-WHILE-LINKED-UNLINKED",
        EV_NR_RUNNING_DESYNC => "NR-RUNNING-DESYNC",
        EV_R56_STEAL => "R56-STEAL",
        EV_R56_SLOT_CAPTURE => "R56-SLOT-CAPTURE-HEALED",
        EV_R56_QUEUED_MARK => "R56-QUEUED-MARK-CLEARED",
        EV_R80_LIVE_MARK => "R80-LIVE-MARK-CLEARED",
        EV_RQD_MIGRATE => "RQD-MIGRATE-DISPATCH",
        EV_RT_THROTTLED => "RT-CLASS-THROTTLED",
        _ => "SCHED-EV-UNKNOWN",
    }
}

/// Drain pending events, at most `budget` lines, from the tick.
///
/// printk is Rule-2-safe from here (hardirq context defers the console
/// emit; the ring write is lock-free) and the tick runs outside every
/// GRQ section — so the drain may print freely.
pub fn sched_ev_drain(budget: usize) -> usize {
    let mut printed = 0usize;
    while printed < budget {
        let next = EV_CUR.load(Ordering::Relaxed);
        let mut d = EV_DRAIN.load(Ordering::Relaxed);
        // Clamp for overrun: never print more than the newest
        // EV_RING_SLOTS events (older slots were clobbered).
        let oldest = next.saturating_sub(EV_RING_SLOTS as u64);
        if d < oldest {
            d = oldest;
        }
        if d >= next {
            break;
        }
        let slot = (d as usize) % EV_RING_SLOTS;
        // SAFETY: read-only diagnostic access; publication is validated
        // by the pub_seq check below before any field is trusted.
        let ev = crate::sched::r#unsafe::evring_slot_ref(&EV_RING, slot);
        if ev.pub_seq.load(Ordering::Acquire) != d + 1 {
            break; // writer mid-fill (or clobbered) — retry next tick
        }
        crate::pr_info!(
            "{} pid={} a={:#x} b={:#x} jif={}",
            tag_name(ev.tag),
            ev.pid,
            ev.a,
            ev.b,
            ev.jif
        );
        EV_DRAIN.store(d + 1, Ordering::Release);
        printed += 1;
    }
    printed
}
