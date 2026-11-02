//! Allocator class-lock holder watch (dfx=allocwatch).
//!
//! The OH dark-boot family (2026-11-02 decodes, lb234/235/241/243/244):
//! every allocating syscall parks (unix_write_slice_owned -> RawVec
//! finish_grow, epoll_create1) because a small_alloc/slab/buddy class lock
//! is held by a thread frozen mid-section. The holder sits at IF=0 inside
//! lock_irqsave, so its OWN CPU's tick never fires — every tick-side
//! observer (PREEMPT-HEAL, claim census) is blind to it; only a cross-CPU
//! observer can name it. This instrument is that observer:
//!
//!   - mm sites stamp (pid, tgid, site, class, jiffies, epoch) at
//!     class-lock acquire and clear it at release (slow paths only; the
//!     magazine fast path never takes the lock);
//!   - scheduler_tick calls tick_check(): a stamp older than 1 s prints
//!     the holder's CURRENT task state (parked vs runnable-not-picked vs
//!     on-cpu-with-preempt-bits) — the discrimination the sched-vs-mm
//!     split needs:
//!       (a) holder state RUNNING with clean preempt bits and not current
//!           on its cpu  => runnable-but-never-picked — scheduler strand
//!           family (the preempt-hunt Face C candidate);
//!       (b) holder state SLEEPING/STOPPED while holding => mm lock misuse
//!           (sleeping under a spinlock — a kernel bug to fix in mm);
//!       (c) holder state RUNNING with phantom preempt bits (0x10000/
//!           0x10001 class) on-cpu => the phantom-preempt monopolist —
//!           the producer hunt's direct evidence.
//!
//! Slots are keyed by (site, class) with a monotonic epoch so a stamp is
//! cleared only by its own release — a frozen holder on one class lock
//! cannot be hidden by traffic on another class mapping to the same slot
//! (first version had one slot per site; different classes share a site
//! and last-writer-wins erased exactly the frozen holders).
//!
//! Gated OFF by default (Relaxed atomic loads/stores per slow-path
//! acquire when on; a single load when off). Budgeted prints: 200
//! reports per boot, re-report per holder at most every 5 s.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub const SITE_SMALL_ALLOC: u8 = 0;
pub const SITE_SLAB: u8 = 1;
pub const SITE_BUDDY: u8 = 2;
pub const SITE_MAX: usize = 3;

pub const SITE_NAMES: [&str; SITE_MAX] = ["small_alloc", "slab", "buddy"];

const SLOTS: usize = 16;

struct Holder {
    /// (site | class << 8) the stamp was written for; also the exit key.
    key: AtomicU32,
    /// Monotonic acquire epoch; exit only clears a matching epoch so an
    /// overwritten slot cannot be cleared by a stale release.
    epoch: AtomicU64,
    pid: AtomicU32,
    tgid: AtomicU32,
    jif: AtomicU64,
}

impl Holder {
    const fn new() -> Self {
        Holder {
            key: AtomicU32::new(0),
            epoch: AtomicU64::new(0),
            pid: AtomicU32::new(0),
            tgid: AtomicU32::new(0),
            jif: AtomicU64::new(0),
        }
    }
}

static HOLDERS: [Holder; SLOTS] = [
    Holder::new(), Holder::new(), Holder::new(), Holder::new(),
    Holder::new(), Holder::new(), Holder::new(), Holder::new(),
    Holder::new(), Holder::new(), Holder::new(), Holder::new(),
    Holder::new(), Holder::new(), Holder::new(), Holder::new(),
];
static EPOCH: AtomicU64 = AtomicU64::new(1);
static REPORTS: AtomicU32 = AtomicU32::new(0);
static LAST_REPORT_JIF: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);

fn enabled() -> bool {
    if ARMED.load(Ordering::Relaxed) {
        return true;
    }
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::AllocWatch) {
        ARMED.store(true, Ordering::Relaxed);
        return true;
    }
    false
}

/// Stamp the current task as holder of (site, class)'s lock. Runs under
/// the acquired lock with IF=0: straight Relaxed stores, no allocation,
/// no scan beyond the slot table.
fn enter_keyed(site: u8, class: u8) -> u64 {
    let key = site as u32 | ((class as u32) << 8);
    let (pid, tgid) = match crate::sched::current() {
        Some(t) => ((*t).pid(), (*t).tgid()),
        None => (0, 0),
    };
    // Slot choice: exact key match first, then a free slot, then the
    // oldest stamp (collision eviction — acceptable, see module doc).
    let mut victim = 0usize;
    let mut victim_jif = u64::MAX;
    let mut free = usize::MAX;
    let mut exact = usize::MAX;
    for (i, h) in HOLDERS.iter().enumerate() {
        let p = h.pid.load(Ordering::Relaxed);
        if p == 0 {
            if free == usize::MAX {
                free = i;
            }
            continue;
        }
        if h.key.load(Ordering::Relaxed) == key {
            exact = i;
            break;
        }
        let j = h.jif.load(Ordering::Relaxed);
        if j < victim_jif {
            victim_jif = j;
            victim = i;
        }
    }
    let slot = if exact != usize::MAX {
        exact
    } else if free != usize::MAX {
        free
    } else {
        victim
    };
    let ep = EPOCH.fetch_add(1, Ordering::Relaxed);
    let h = &HOLDERS[slot];
    h.key.store(key, Ordering::Relaxed);
    h.epoch.store(ep, Ordering::Relaxed);
    h.pid.store(pid, Ordering::Relaxed);
    h.tgid.store(tgid, Ordering::Relaxed);
    h.jif
        .store(crate::drivers::timer::get_jiffies() as u64, Ordering::Relaxed);
    ep
}

/// Clear the stamp (lock released) — only if the slot still carries our
/// epoch (an evicted/overwritten stamp belongs to someone else now).
fn exit_keyed(site: u8, class: u8, ep: u64) {
    let key = site as u32 | ((class as u32) << 8);
    for h in HOLDERS.iter() {
        if h.key.load(Ordering::Relaxed) == key
            && h.epoch.load(Ordering::Relaxed) == ep
        {
            h.pid.store(0, Ordering::Relaxed);
            return;
        }
    }
}

/// RAII stamp: `let _aw = allocwatch::guard(site, class);` immediately
/// after acquiring the class lock. Drops (and clears) BEFORE the lock
/// guard releases (reverse declaration order) — the stamp slightly
/// overstates the hold, never understates it.
pub fn guard(site: u8, class: u8) -> AwGuard {
    let ep = if enabled() { enter_keyed(site, class) } else { 0 };
    AwGuard { site, class, ep }
}

pub struct AwGuard {
    site: u8,
    class: u8,
    ep: u64,
}

impl Drop for AwGuard {
    fn drop(&mut self) {
        if self.ep != 0 {
            exit_keyed(self.site, self.class, self.ep);
        }
    }
}

/// Live stamp present for (site, class)?
fn stamp_live(site: u8, class: u8) -> bool {
    let key = site as u32 | ((class as u32) << 8);
    for h in HOLDERS.iter() {
        if h.pid.load(Ordering::Relaxed) != 0 && h.key.load(Ordering::Relaxed) == key {
            return true;
        }
    }
    false
}

// STUCK detection: a class lock's WORD observed locked at every tick for
// >1 s with NO live holder stamp is not a held lock — it is a stuck/
// corrupt word (the wild-write producer's freeze face: waiters spin
// forever on a lock nobody holds, and the preemptible TTAS wait makes
// them cycle through finish_grow forever — the exact OH victim
// signature with NO frozen holder to name).
static SA_STUCK: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static SLAB_STUCK: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
static BUDDY_STUCK: AtomicU64 = AtomicU64::new(0);

fn stuck_report(
    now: u64,
    site: &str,
    class: u8,
    first: u64,
) {
    let last = LAST_REPORT_JIF.load(Ordering::Relaxed);
    if now < last + 500 {
        return;
    }
    LAST_REPORT_JIF.store(now, Ordering::Relaxed);
    let n = REPORTS.fetch_add(1, Ordering::Relaxed);
    if n > 200 {
        return;
    }
    crate::pr_warn!(
        "ALLOC-STUCK #{} site={} class={} stuck={}ms (lock word held with NO live holder stamp — corrupt/stuck word, wild-write producer face)",
        n,
        site,
        class,
        (now - first) * 10
    );
}

fn tick_stuck_scan(now: u64) {
    let sa = crate::mm::small_alloc::audit_class_locks();
    for i in 0..8u32 {
        let s = &SA_STUCK[i as usize];
        if sa & (1 << i) != 0 && !stamp_live(SITE_SMALL_ALLOC, i as u8) {
            let first = s.load(Ordering::Relaxed);
            if first == 0 {
                s.store(now, Ordering::Relaxed);
            } else if now - first >= 100 {
                stuck_report(now, "small_alloc", i as u8, first);
                // keep the run (no re-arm) — the word is still stuck;
                // the global throttle paces the re-reports.
            }
        } else {
            s.store(0, Ordering::Relaxed);
        }
    }
    let sl = crate::mm::slab::audit_cache_locks();
    for i in 0..10u32 {
        let s = &SLAB_STUCK[i as usize];
        if sl & (1 << i) != 0 && !stamp_live(SITE_SLAB, i as u8) {
            let first = s.load(Ordering::Relaxed);
            if first == 0 {
                s.store(now, Ordering::Relaxed);
            } else if now - first >= 100 {
                stuck_report(now, "slab", i as u8, first);
            }
        } else {
            s.store(0, Ordering::Relaxed);
        }
    }
    if crate::mm::buddy_allocator::audit_lock_held() && !stamp_live(SITE_BUDDY, 0) {
        let first = BUDDY_STUCK.load(Ordering::Relaxed);
        if first == 0 {
            BUDDY_STUCK.store(now, Ordering::Relaxed);
        } else if now - first >= 100 {
            stuck_report(now, "buddy", 0, first);
        }
    } else {
        BUDDY_STUCK.store(0, Ordering::Relaxed);
    }
}

/// Cross-CPU check from scheduler_tick: report stamps older than 1 s.
/// The holder's own CPU cannot run this (IF=0 froze its tick); the other
/// CPU's tick scans ALL slots regardless of which CPU stamped them.
pub fn tick_check() {
    if !enabled() {
        return;
    }
    let now = crate::drivers::timer::get_jiffies() as u64;
    tick_stuck_scan(now);
    for i in 0..SLOTS {
        let h = &HOLDERS[i];
        let pid = h.pid.load(Ordering::Relaxed);
        if pid == 0 {
            continue;
        }
        let jif = h.jif.load(Ordering::Relaxed);
        if now <= jif || now - jif < 100 {
            continue; // held < 1 s (100 Hz)
        }
        // Re-report throttle: one line per boot per 5 s.
        let last = LAST_REPORT_JIF.load(Ordering::Relaxed);
        if now < last + 500 {
            return;
        }
        LAST_REPORT_JIF.store(now, Ordering::Relaxed);
        let n = REPORTS.fetch_add(1, Ordering::Relaxed);
        if n > 200 {
            return;
        }
        let tgid = h.tgid.load(Ordering::Relaxed);
        let key = h.key.load(Ordering::Relaxed);
        let held_ms = (now - jif) * 10;
        let site = (key & 0xff) as usize;
        let site_name = if site < SITE_MAX { SITE_NAMES[site] } else { "?" };
        let class = key >> 8;
        // Find the holder's CURRENT state. Non-blocking iteration: we run
        // from the tick; a busy pid-hash bucket is skipped (partial info
        // beats a wedged diagnostic).
        let mut found = false;
        let mut state_bits = 0u32;
        let mut preempt = 0i32;
        let mut ti_cpu = 0i32;
        let mut comm_out = [0u8; 16];
        let mut comm_len = 0usize;
        unsafe {
            crate::process::pid_hash::pid_hash_for_each_task_try(|t| {
                if !found && (*t).pid() == pid {
                    state_bits = (*t).state().bits();
                    preempt = (*t).preempt_count();
                    ti_cpu = (*t).ti_cpu();
                    let c = (*t).comm();
                    let n = c.iter().position(|&x| x == 0).unwrap_or(16);
                    comm_out[..n].copy_from_slice(&c[..n]);
                    comm_len = n;
                    found = true;
                }
            });
        }
        let comm = core::str::from_utf8(&comm_out[..comm_len]).unwrap_or("?");
        let cls = if found {
            match state_bits {
                0 => {
                    // RUNNING: on-cpu-with-phantom-bits vs
                    // runnable-not-picked. preempt != 0 while RUNNING is
                    // the monopolist face; clean preempt + not picked is
                    // the strand face. Print both discriminators raw.
                    if preempt != 0 {
                        "RUNNING+preempt-bits(monopolist?)"
                    } else {
                        "RUNNING-preempt-clean(strand?)"
                    }
                }
                1 => "SLEEPING(parked-while-holding: mm lock misuse)",
                2 => "STOPPED-while-holding(mm lock misuse)",
                _ => "other-state",
            }
        } else {
            "not-found(task exited? stamp stale)"
        };
        crate::pr_warn!(
            "ALLOC-HOLD #{} slot={} site={} class={} pid={} tgid={} comm={} held={}ms state={} preempt={:#x} ti_cpu={} verdict={}",
            n,
            i,
            site_name,
            class,
            pid,
            tgid,
            comm,
            held_ms,
            if found { state_bits } else { 0xff },
            preempt,
            ti_cpu,
            cls
        );
    }
}
