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
/// sc167 storm gate: raised to 1000 ms for the OH lane. Under the boot
/// print-storm, EVERY wake strands past 50 ms and the report (printed
/// in the PICKED task's context with IF=0, ~100 direct-console chars)
/// eats exactly the LIFECYCLE_TIMEOUT budget of the child being picked
/// — sc166 printed 2380 lines for five pids. Sub-second strands are
/// storm weather; the pathological family this table exists for is the
/// multi-second monopoly, which still reports.
const STRAND_THRESHOLD_MS: u64 = 1000;

struct WwSlot {
    task: AtomicU64,
    stamp: AtomicU64,
    /// s16 (WW5): pick-walk count at wake time — the strand line's
    /// visits= divides the walks that ran during the strand window.
    walks: AtomicU64,
    /// s16 (WW5): timeline entries at-or-below the victim's key at WAKE
    /// time (the tie-mass it entered behind). tie= on the strand line
    /// reads the POST-DEQUEUE queue (everyone remaining is above a
    /// finally-picked leftmost victim — it cannot show the entry-time
    /// mass); wtie= is that entry-time mass, and it is the field that
    /// separates deep-queue service latency (wtie ~ nr_running) from a
    /// walk defect (wtie ~ 1 with visits in the hundreds).
    /// Computed at note_wake only under dfx=wwcensus (O(nr) walk).
    wtie: AtomicU64,
}

static TABLE: [WwSlot; SLOTS] = {
    // SAFETY-free const init: AtomicU64::new is a const fn.
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: WwSlot = WwSlot {
        task: AtomicU64::new(0),
        stamp: AtomicU64::new(0),
        walks: AtomicU64::new(0),
        wtie: AtomicU64::new(0),
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
// s16 (WW5): sub-gate wake→pick counters. The strand print gates at
// STRAND_THRESHOLD_MS (1000), so the observed "1s grid" cannot be
// distinguished from the gate's own floor by prints alone. These
// cumulative counters give the true distribution below the gate: if
// resolutions routinely cross 250/500/750ms, no window-bound producer
// (the 50-jiffy confirm = 500ms) can own the family.
static STRAND_N250: AtomicU64 = AtomicU64::new(0);
static STRAND_N500: AtomicU64 = AtomicU64::new(0);
static STRAND_N750: AtomicU64 = AtomicU64::new(0);

/// (n250, n500, n750) cumulative pick counts whose wake→pick delta
/// crossed the given sub-gate threshold (WW5 heartbeat).
pub fn ww_subgate_counts() -> (u64, u64, u64) {
    (
        STRAND_N250.load(Ordering::Relaxed),
        STRAND_N500.load(Ordering::Relaxed),
        STRAND_N750.load(Ordering::Relaxed),
    )
}

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
    slot.walks
        .store(super::fair::ww_pick_walks(), Ordering::Release);
    // s16: wake-time tie-mass, only on census boots (O(nr) timeline
    // walk; note_wake is hot).
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::WwCensus) {
        let vr = unsafe { (*task).sched_entity().get_vruntime() };
        slot.wtie.store(
            crate::sched::sched::ww_cfs_tie_mass_vr(vr),
            Ordering::Release,
        );
    }
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
    // s16 (WW5): sub-gate census before the print gate swallows the
    // distribution below 1000 ms.
    let ms = ticks_to_ms(delta);
    if ms >= 750 {
        STRAND_N750.fetch_add(1, Ordering::Relaxed);
    }
    if ms >= 500 {
        STRAND_N500.fetch_add(1, Ordering::Relaxed);
    }
    if ms >= 250 {
        STRAND_N250.fetch_add(1, Ordering::Relaxed);
    }
    if delta * 1000 <= STRAND_THRESHOLD_MS * TIMER_CLOCK_FREQ_HZ {
        return;
    }
    let ms = ticks_to_ms(delta);
    let walks_at_wake = slot.walks.load(Ordering::Acquire);
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
    // sc108: probe prints burn the PICKED task's CPU (UART TX is a busy
    // THRE poll at 115200 — ~87us/byte charged as stime to whoever was
    // picked). The launcher child's whole 3s appspawn watchdog budget
    // went to Session-L/M diagnostics printing in its context. Gate the
    // prints behind `sprobe`; the strand counters above keep running.
    if print_budget_ok() && probe_prints_on() {
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
            // leg-24 leakhunt v3: the slot incumbent can be a stale
            // identity (R55 family) — print what the ARCH tp slot says
            // this CPU is actually executing (tp=pid/pc), so a strand
            // incumbent with pc!=0 is attributed to the task that
            // really holds the CPU. One line per report, dfx-gated.
            if crate::dfx::leakorigin::ENABLED
                .load(core::sync::atomic::Ordering::Relaxed)
            {
                let tp = crate::arch::cpu::get_thread_id_of(c)
                    as *mut crate::process::Task;
                ww_msg(b" tp=");
                if tp.is_null() {
                    ww_msg(b"nil");
                } else {
                    // SAFETY: diagnostic reads of stable Task fields.
                    unsafe {
                        ww_u64((*tp).pid() as u64);
                        ww_msg(b"/pc=");
                        ww_u64((*tp).preempt_count() as u32 as u64);
                    }
                }
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
        // s16 (WW5): the two decisive census fields.
        // visits= — pick walks that ran during the strand window. High
        // with skips=0 means candidates kept being picked around the
        // victim (ordering/tie question); ~0 means picks were not
        // happening at all (slice/preemption question).
        ww_msg(b" visits=");
        ww_u64(
            super::fair::ww_pick_walks().saturating_sub(walks_at_wake),
        );
        // tie= — timeline entries keyed at or before the victim (the
        // min_vruntime tie-mass it stands behind; see ww_tie_mass).
        ww_msg(b" tie=");
        ww_u64(crate::sched::sched::ww_cfs_tie_mass(task));
        // wtie= — the same mass as it was AT WAKE (entry-time rank-1).
        // The discriminator: wtie ~ nr_running says deep-queue service
        // latency; wtie ~ 1 with visits in the hundreds says the walk
        // passed a leftmost never-refused candidate (defect).
        ww_msg(b" wtie=");
        ww_u64(slot.wtie.load(Ordering::Acquire));
        ww_msg(b"\n");
    }
}

/// One heartbeat summary line (printk — safe from the timer softirq):
/// `WW: wakes=N picks=N avg=..ms stranded=N max=..ms`. Called ~1 Hz
/// from timer.rs; printk, not direct console, so it cannot interleave
/// with GRQ-held prints.
pub fn heartbeat() {
    // Serial-noise policy: hunt instrument, prints only on dfx=wwcensus
    // boots; ordinary boots keep a clean serial prompt.
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::WwCensus) {
        return;
    }
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

/// Rate gate: at most one direct-console line per 20 jiffies (200 ms;
/// was 2 jiffies — sc166's 2380-line strand storm, see STRAND_THRESHOLD_MS).
fn print_budget_ok() -> bool {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = crate::drivers::timer::get_jiffies();
    let last = LAST.load(Ordering::Acquire);
    if now.wrapping_sub(last) < 20 {
        return false;
    }
    LAST.compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// sc108 gate for wake-probe console output (see check_picked): pass
/// `sprobe=0` on the kernel cmdline to silence — each printed line
/// costs the picked task ~87us/byte of busy UART poll. Default: on
/// (concurrent hunts still rely on the prints).
fn probe_prints_on() -> bool {
    crate::cmdline::get_param("sprobe").map(|v| v != "0").unwrap_or(true)
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

/// Hex printer (leg-33B): tick RIPs read as text are unreadable; the
/// strand census prints them 0x-prefixed for direct addr2line use.
fn ww_hex(v: u64) {
    ww_msg(b"0x");
    let mut buf = [0u8; 16];
    let mut n = 0;
    let mut v = v;
    if v == 0 {
        buf[0] = b'0';
        n = 1;
    } else {
        while v > 0 && n < buf.len() {
            let d = (v & 0xf) as u8;
            buf[n] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
            v >>= 4;
            n += 1;
        }
    }
    for i in (0..n).rev() {
        unsafe { crate::console::putchar_no_lock(buf[i]) };
    }
}

// ============================================================================
// sc158 STALLWATCH (Session R): one-shot park dump for the foundation/
// sa_main registrar threads.
//
// sc155-158 closed the SA-501 question to "foundation's safwk loader dies
// or wedges mid-boot"; the wedge shape is the loader main thread parked
// with NO timed futex outstanding (FTWAIT shows no entry), so its park is
// an UNTIMED futex (mutex/cond event wait) or a binder read. This scan
// walks the task table every ~5 s of jiffies; any foundation/sa_main
// task continuously INTERRUPTIBLE for > 90 s gets ONE dump of its last
// syscall ring + failing-syscall tail, naming the park call-site family.
// Side-table keyed by Task address (same recycling contract as TABLE
// above; wake clears the entry so a task parks twice prints twice only
// if both parks exceed the window).
// ============================================================================
struct StallSlot {
    task: AtomicU64,
    since: AtomicU64,
    printed: AtomicU64,
}

static STALL_TABLE: [StallSlot; 128] = {
    const ZERO: StallSlot = StallSlot {
        task: AtomicU64::new(0),
        since: AtomicU64::new(0),
        printed: AtomicU64::new(0),
    };
    [ZERO; 128]
};

fn stall_slot_of(task: *mut crate::process::Task) -> &'static StallSlot {
    let a = task as usize;
    &STALL_TABLE[(a >> 6 ^ a >> 12) & 127]
}

/// Called from the timer tick path (softirq context): cheap jiffies gate,
/// then the scan. Prints via pr_warn (rate is inherently tiny: one line
/// per 90+s stall).
pub fn stallwatch_tick() {
    let now = crate::drivers::timer::get_jiffies();
    static LAST_SCAN: AtomicU64 = AtomicU64::new(0);
    let last = LAST_SCAN.load(Ordering::Relaxed);
    // Every ~5 s (assume HZ >= 50; a smaller HZ just scans more often).
    if now.wrapping_sub(last) < 250 {
        return;
    }
    if LAST_SCAN
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    // Report lines collect here and print AFTER the walk (see the note
    // at the pr_warn loop below).
    let mut reports: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
    // sc215 (leg 15): the walk itself must never block. This scanner
    // runs in the timer-tick (softirq) path via ww_heartbeat; the
    // blocking pid_hash_for_each_task takes per-bucket spinlocks, and a
    // task-context holder (fork insert / exit remove / kill for_each —
    // all hold buckets with IRQs on) interrupted by the tick
    // self-deadlocks: the handler spins for the very bucket its
    // interrupted owner holds, the second vCPU's scan reaches the same
    // stuck bucket and joins, ticks stop, the console dies mid-line.
    // sc215 froze exactly this way at BUCKET_LOCK[96] (monitor-
    // convicted: stuck byte=1, CPU0 stack ww_heartbeat ->
    // stallwatch_tick -> for_each_task -> lock_bucket spin, tick
    // counter frozen at both samples). The sc207 "print after the
    // walk" fix removed the printk-under-bucket-lock half of the
    // hazard but left the acquisition itself blocking. The _try walk
    // skips held buckets — a skipped round just misses tasks for one
    // ~5s scan, harmless for a stall detector that re-scans forever.
    let _skipped = crate::process::pid_hash::pid_hash_for_each_task_try(|t| {
        // SAFETY: for_each_task pins the task (per-bucket lock); reads
        // are of stable fields.
        let tk = unsafe { &*t };
        let comm = core::str::from_utf8(&tk.comm()[..]).unwrap_or("");
        let comm = comm.trim_end_matches('\0');
        // sc160: widen to the launcher cohort — sc159's appspawn children
        // park right after "GetOnDemandSystemAbilityIds succeed" with no
        // further narration and no dump (the filter missed them); their
        // syscall ring names the park call-site family directly.
        if !comm.contains("foundation")
            && !comm.contains("sa_main")
            && !comm.contains("launcher")
            && !comm.contains("systemui")
            && !comm.contains("appspawn")
            && !comm.contains("samgr")
        {
            return;
        }
        if !tk.state().is_interruptible() {
            // Awake or dead: clear any side-table entry.
            let slot = stall_slot_of(t);
            slot.task.store(0, Ordering::Release);
            slot.printed.store(0, Ordering::Release);
            return;
        }
        let slot = stall_slot_of(t);
        if slot.task.load(Ordering::Acquire) != t as usize as u64 {
            slot.task.store(t as usize as u64, Ordering::Release);
            slot.since.store(now, Ordering::Release);
            slot.printed.store(0, Ordering::Release);
            return;
        }
        if slot.printed.load(Ordering::Acquire) != 0 {
            return;
        }
        let since = slot.since.load(Ordering::Acquire);
        if now.wrapping_sub(since) < 90 * 100 {
            return;
        }
        slot.printed.store(1, Ordering::Release);
        let mut s = alloc::string::String::new();
        use core::fmt::Write;
        let _ = write!(
            s,
            "STALLWATCH tid {} comm='{}' parked >90s calls:",
            tk.pid(),
            comm
        );
        const RL: usize = crate::process::task::LAST_CALLS_LEN;
        let cur = tk.last_calls_cur.load(Ordering::Relaxed);
        for k in 0..RL {
            let i = (cur + RL - k) % RL;
            let v = tk.last_calls[i].load(Ordering::Relaxed);
            if v != 0 {
                let _ = write!(s, " {}", v);
            }
        }
        reports.push(s);
        let fcur = tk.last_fails_cur.load(Ordering::Relaxed);
        let mut s2 = alloc::string::String::new();
        let _ = write!(s2, "STALLWATCH tid {} fails:", tk.pid());
        for k in 0..8 {
            let i = (fcur + 8 - 1 - k) % 8;
            let v = tk.last_fails[i].load(Ordering::Relaxed);
            if v != 0 {
                let a0 = tk.last_fails_a0[i].load(Ordering::Relaxed);
                let _ = write!(s2, " nr{}=-{} a0={:#x}", v & 0xffff, v >> 16, a0);
            }
        }
        reports.push(s2);
    });
    // sc207 (launcher leg 13): print AFTER the walk. The closure runs
    // under pid-hash bucket locks; a printk there can hold a bucket
    // across console port-IO, and this scanner itself runs at softirq
    // time - any task-context bucket holder interrupted by the tick
    // self-deadlocks against this very walk (sc207 hard-froze both
    // vCPUs exactly this way against the sleepsnap census printer).
    for r in &reports {
        crate::pr_warn!("{}", r);
    }
    // sc215: name near-misses — buckets a slow task-context holder
    // kept locked during this scan. The _try walk skipped them (pre-
    // fix this was the deadlock window itself); a recurring high
    // number here is the argument for irqsave bucket locks.
    if _skipped != 0 {
        crate::pr_warn!("STALLWATCH scan skipped {} held pid-hash buckets", _skipped);
    }
}
