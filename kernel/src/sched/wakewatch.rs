//! Wakewatch — wake-to-run latency forensics for the vsync-starvation
//! family (compose-content's cc13 evidence: a 16 ms usleep in the loaded
//! OH system occasionally takes 0.5–0.7 s, plus one ~41 s dead stop per
//! boot, while isolation keeps the same sleep at 16.2/16.8 ms).
//!
//! The instrument discriminates the four hypotheses without perturbing
//! the system:
//!
//!   1. wake arrives on time, task not PICKED for the outlier interval
//!      → `WAKE-STRANDED` names the victim, its wake→pick latency, and
//!      the picking CPU (scheduler-pick starvation family);
//!   2. the timer itself fires late → timer.rs's `TIMER-LATE-FIRE`
//!      names the fire lateness (softirq starvation / long IRQ-off
//!      windows);
//!   3. the wake is dropped entirely → `WAKE-REFUSED` /
//!      `WAKE-PID-MISS` (state-mismatch lost wakes);
//!   4. none of the above (in-process time loss) → the outlier shows in
//!      the guest probe but no kernel event brackets it.
//!
//! Design notes:
//! - A fixed side table keyed by Task pointer hash — the Task struct
//!   layout is deliberately NOT touched (offset-sensitive scribble
//!   machinery and cross-branch merge cost outweigh one AtomicU64 pair
//!   per slot). Collisions overwrite a stale stamp at worst; the slot's
//!   task word is the validity key so a read never attributes another
//!   task's stamp.
//! - Stamps use `read_time()` (nominal 10 MHz ticks): monotonic under
//!   load, cheap to read, same domain the hres wheel compares against.
//! - Prints from GRQ-held contexts use the R34 discipline (direct
//!   console writes), rate-gated to one line per 2 jiffies; the
//!   heartbeat line (1 Hz, from the timer softirq via printk) carries
//!   the cumulative counters so rate-gating cannot lose the story.
//! - Overhead per wake: two atomic stores + one hash. Per pick: one
//!   atomic load on the happy path.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::config::TIMER_CLOCK_FREQ_HZ;

/// Side-table slots (power of two; hash-masked).
const SLOTS: usize = 1024;
const SLOT_MASK: usize = SLOTS - 1;

/// Wake→pick threshold that counts as "stranded" (ticks computed from
/// the nominal clock below). Isolated probes put a healthy wake→run at
/// a few ms; anything past 50 ms under load is part of the outlier
/// family we are hunting.
const STRAND_THRESHOLD_MS: u64 = 50;

struct WwSlot {
    task: AtomicU64,
    stamp: AtomicU64,
}

static TABLE: [WwSlot; SLOTS] = {
    // SAFETY-free const init: AtomicU64::new is a const fn.
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: WwSlot = WwSlot {
        task: AtomicU64::new(0),
        stamp: AtomicU64::new(0),
    };
    [ZERO; SLOTS]
};

// ---- cumulative counters (read by the 1 Hz heartbeat) ----
static WAKE_N: AtomicU64 = AtomicU64::new(0);
static PICK_N: AtomicU64 = AtomicU64::new(0);
/// Sum of observed wake→pick deltas (ticks) over PICKED stamps.
static DELTA_SUM_TICKS: AtomicU64 = AtomicU64::new(0);
static STRAND_N: AtomicU64 = AtomicU64::new(0);
static STRAND_MAX_MS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn now_ticks() -> u64 {
    crate::drivers::timer::read_time()
}

#[inline]
fn ticks_to_ms(t: u64) -> u64 {
    t * 1000 / TIMER_CLOCK_FREQ_HZ
}

#[inline]
fn slot_of(task: *mut crate::process::Task) -> &'static WwSlot {
    let a = task as usize;
    &TABLE[(a >> 6 ^ a >> 12) & SLOT_MASK]
}

/// Record that `task` just became runnable (enqueue success — wake
/// paths, fork first-link, heal re-inserts). Called with the GRQ lock
/// held by the callers in sched.rs.
pub fn note_wake(task: *mut crate::process::Task) {
    let slot = slot_of(task);
    slot.stamp.store(now_ticks(), Ordering::Release);
    slot.task.store(task as usize as u64, Ordering::Release);
    WAKE_N.fetch_add(1, Ordering::Relaxed);
}

/// Invalidate `task`'s side-table stamp at task destruction. The slot
/// key is the Task ADDRESS and task pages are recycled, so without this
/// a recycled Task at the same address inherits the previous occupant's
/// wake stamp — the first pick of the NEW task then reports a phantom
/// strand spanning the old task's whole lifetime (multi-ten-second
/// WAKE-STRANDED outliers that were never real scheduling events).
pub fn note_task_freed(task: *mut crate::process::Task) {
    let slot = slot_of(task);
    slot.stamp.store(0, Ordering::Release);
    slot.task.store(0, Ordering::Release);
}

/// Consume the stamp at pick time; classify and report. Called from
/// mark_picked_on_cpu under the GRQ lock — prints go through the direct
/// console (R34 discipline), rate-gated to one line per 2 jiffies.
pub fn check_picked(task: *mut crate::process::Task, cpu: usize) {
    let slot = slot_of(task);
    if slot.task.load(Ordering::Acquire) != task as usize as u64 {
        return;
    }
    let stamp = slot.stamp.swap(0, Ordering::AcqRel);
    if stamp == 0 {
        return;
    }
    slot.task.store(0, Ordering::Release);
    PICK_N.fetch_add(1, Ordering::Relaxed);
    let delta = now_ticks().saturating_sub(stamp);
    DELTA_SUM_TICKS.fetch_add(delta, Ordering::Relaxed);
    if delta * 1000 <= STRAND_THRESHOLD_MS * TIMER_CLOCK_FREQ_HZ {
        return;
    }
    let ms = ticks_to_ms(delta);
    STRAND_N.fetch_add(1, Ordering::Relaxed);
    let mut cur = STRAND_MAX_MS.load(Ordering::Relaxed);
    while ms > cur {
        match STRAND_MAX_MS.compare_exchange_weak(
            cur,
            ms,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
    if print_budget_ok() {
        // SAFETY: diagnostic reads of stable Task fields; the task is
        // being picked (GRQ held pins it).
        let pid = unsafe { (*task).pid() };
        ww_msg(b"WAKE-STRANDED pid=");
        ww_u64(pid as u64);
        ww_msg(b" cpu=");
        ww_u64(cpu as u64);
        ww_msg(b" strand=");
        ww_u64(ms);
        ww_msg(b"ms policy=");
        ww_u64(unsafe { (*task).policy() as u64 });
        // Monster-strand forensics (launcher perf gate round 3): name
        // each CPU's incumbent and its CURRENT-activation run length so
        // the report answers "what was the CPU doing instead" directly —
        // a kernel-loop hog shows a huge activation length; a healthy
        // rotation shows small ones.
        for c in 0..crate::config::MAX_CPUS {
            let cur = crate::sched::sched::cpu_state(c).current;
            ww_msg(b" h");
            ww_u64(c as u64);
            ww_msg(b"=(");
            if cur.is_null() {
                ww_msg(b"nil)");
                continue;
            }
            // SAFETY: init-time valid slot pointer; scalar field reads
            // tolerate a concurrent switch (diagnostic only).
            unsafe {
                ww_u64((*cur).pid() as u64);
                ww_msg(b"/p");
                ww_u64((*cur).policy() as u64);
                let se = (*cur).sched_entity();
                let act_ns = se
                    .sum_exec_runtime
                    .load(core::sync::atomic::Ordering::Relaxed)
                    .saturating_sub(
                        se.prev_sum_exec_runtime
                            .load(core::sync::atomic::Ordering::Relaxed),
                    );
                ww_msg(b"/");
                ww_u64(act_ns / 1_000_000);
                ww_msg(b"ms");
                // Kernel-preemption round (sc65): an incumbent whose
                // activation runs to tens of seconds is monopolizing its
                // CPU, and with IRQ-exit preemption that is only possible
                // while its preempt_count != 0. Print the count with the
                // activation so the report names the discipline directly:
                // 1 = a leaked PREEMPT_OFFSET (spinlock guard), 0x100 =
                // a leaked bh, 0x4000000 = a leaked PREEMPT_ACTIVE.
                ww_msg(b"/pc=");
                ww_u64((*cur).preempt_count() as u32 as u64);
                ww_msg(b")");
            }
        }
        // vruntime geometry (round 4): the victim's lead/lag against
        // min_vruntime and the queue's leftmost. A lead of ~strand_ms
        // says "former CPU hog waiting for the pack" (fair, if blunt);
        // a small lead with a big strand says the pick walk kept
        // skipping it (ownership/affinity/throttle) or charging is
        // inflated (runaway).
        {
            let (vr, minv, leftv) = crate::sched::sched::ww_cfs_geometry(task);
            ww_msg(b" vr=");
            ww_u64(vr.saturating_sub(minv) / 1_000_000);
            ww_msg(b"ms lead-lag=");
            ww_u64(leftv.saturating_sub(vr) / 1_000_000);
            ww_msg(b"ms");
        }
        ww_msg(b"\n");
    }
}

/// One heartbeat summary line (printk — safe from the timer softirq):
/// `WW: wakes=N picks=N avg=..ms stranded=N max=..ms`. Called ~1 Hz
/// from timer.rs; printk, not direct console, so it cannot interleave
/// with GRQ-held prints.
pub fn heartbeat() {
    let wake_n = WAKE_N.load(Ordering::Relaxed);
    let pick_n = PICK_N.load(Ordering::Relaxed);
    let sum_ms = ticks_to_ms(DELTA_SUM_TICKS.load(Ordering::Relaxed));
    let avg = if pick_n > 0 { sum_ms / pick_n } else { 0 };
    crate::pr_info!(
        "WW: wakes={} picks={} avg_run={}ms stranded={} strand_max={}ms",
        wake_n,
        pick_n,
        avg,
        STRAND_N.load(Ordering::Relaxed),
        STRAND_MAX_MS.load(Ordering::Relaxed)
    );
}

/// Rate gate: at most one direct-console line per 2 jiffies (matches
/// the fair.rs heal-report budget).
fn print_budget_ok() -> bool {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = crate::drivers::timer::get_jiffies();
    let last = LAST.load(Ordering::Acquire);
    if now.wrapping_sub(last) < 2 {
        return false;
    }
    LAST.compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn ww_msg(msg: &[u8]) {
    for &b in msg {
        unsafe { crate::console::putchar_no_lock(b) };
    }
}

fn ww_u64(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        buf[0] = b'0';
        n = 1;
    } else {
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    for i in (0..n).rev() {
        unsafe { crate::console::putchar_no_lock(buf[i]) };
    }
}
