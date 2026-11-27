#![forbid(unsafe_code)]
//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! NOHZ — the reference kernel/time/tick-sched.c machinery (CONFIG_NO_HZ_
//! COMMON + CONFIG_NO_HZ_FULL), function-for-function. Structural mapping
//! (Rux divergences are of the reference's own concepts onto this tree,
//! not new mechanisms):
//! - per-rq  -> the global run queue (D-1): `rq->nr_running` reads the
//!   GRQ counter, so the SCHED tick dependency is evaluated on the fly
//!   by can_stop_full_tick() instead of being cached in a per-cpu bit;
//!   the 0->1 transition side (tick_nohz_full_kick_cpu) is kept as a
//!   resched-IPI kick from the enqueue edges.
//! - clockevent KTIME_MAX arm -> timer_arm_oneshot(u64::MAX), whose u32
//!   count clamp parks the LAPIC at its maximum delta (the reference's
//!   clockevent programming does the same clamp on KTIME_MAX).
//! - tick_do_timer_cpu -> the boot CPU (cpu 0); its sleep is capped at
//!   TIMEKEEPING_MAX_DEFERMENT because jiffies are advanced from the
//!   timebase inside the tick handler, so some CPU must keep waking to
//!   run increment_jiffies_from_timebase (the reference's do_timer duty).
//! - perf: the reference sets TICK_DEP_BIT_PERF_EVENTS for freq-mode
//!   events only (period-mode samples ride per-event hrtimers). Rux's
//!   software sampling rides the tick, so the dep is set while any
//!   sampling event is armed — the same API, documented divergence.
//!
//! Wired on x86_64 (LAPIC one-shot exists on all arches; the other
//! twins keep their grid-glue set_next_trigger until this lands there).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// KTIME_MAX — "no event / disarm to the device maximum".
pub const KTIME_MAX: u64 = u64::MAX;

/// The reference derives this from the clocksource's max deferment
/// (timekeeping_max_deferment()); it bounds how long the do_timer CPU
/// may sleep between jiffies updates. Rux's timebase (TSC/HPET) needs
/// no periodic correction, so this only bounds jiffies staleness.
pub const TIMEKEEPING_MAX_DEFERMENT: u64 = 10_000_000_000 / 100; // 10 s, nominal domain

/// enum tick_dep_bits (kernel/time/tick-sched.h).
pub mod tick_dep {
    pub const POSIX_TIMER: u32 = 1 << 0;
    pub const PERF_EVENTS: u32 = 1 << 1;
    pub const SCHED: u32 = 1 << 2;
    pub const CLOCK_UNSTABLE: u32 = 1 << 3;
    pub const RCU: u32 = 1 << 4;
    pub const RCU_EXP: u32 = 1 << 5;
}

/// struct tick_sched — the nohz-relevant fields only.
pub struct TickSched {
    /// CPU is in the idle mode (tick_nohz_idle_enter).
    pub inidle: AtomicBool,
    /// The tick has been stopped on this CPU.
    pub tick_stopped: AtomicBool,
    /// Per-CPU tick dependency mask.
    pub tick_dep_mask: AtomicU32,
    /// The expiry the tick was last armed at (ts->next_tick).
    pub next_tick: AtomicU64,
}

pub static TICK_CPU_SCHED: [TickSched; crate::config::MAX_CPUS] = [const {
    TickSched {
        inidle: AtomicBool::new(false),
        tick_stopped: AtomicBool::new(false),
        tick_dep_mask: AtomicU32::new(0),
        next_tick: AtomicU64::new(0),
    }
}; crate::config::MAX_CPUS];

/// static atomic_t tick_dep_mask — the global dependency mask.
static TICK_DEP_MASK: AtomicU32 = AtomicU32::new(0);

fn ts_of(cpu: usize) -> &'static TickSched {
    &TICK_CPU_SCHED[cpu % crate::config::MAX_CPUS]
}

/// static bool check_tick_dependency(atomic_t *dep) — same bit order as
/// the reference (POSIX_TIMER, PERF_EVENTS, SCHED, CLOCK_UNSTABLE, RCU,
/// RCU_EXP).
fn check_tick_dependency(mask: u32) -> bool {
    let val = mask;
    if val & tick_dep::POSIX_TIMER != 0 {
        return true;
    }
    if val & tick_dep::PERF_EVENTS != 0 {
        return true;
    }
    if val & tick_dep::SCHED != 0 {
        return true;
    }
    if val & tick_dep::CLOCK_UNSTABLE != 0 {
        return true;
    }
    if val & tick_dep::RCU != 0 {
        return true;
    }
    if val & tick_dep::RCU_EXP != 0 {
        return true;
    }
    false
}

/// void tick_nohz_dep_set(enum tick_dep_bits bit) — global dep; on the
/// 0->1 transition the reference kicks every nohz_full CPU
/// (tick_nohz_full_kick_all).
pub fn tick_nohz_dep_set(bit: u32) {
    let prev = TICK_DEP_MASK.fetch_or(bit, Ordering::AcqRel);
    if prev == 0 {
        tick_nohz_full_kick_all();
    }
}

/// void tick_nohz_dep_clear(enum tick_dep_bits bit)
pub fn tick_nohz_dep_clear(bit: u32) {
    TICK_DEP_MASK.fetch_and(!bit, Ordering::AcqRel);
}

/// void tick_nohz_dep_set_cpu(int cpu, enum tick_dep_bits bit) — per-CPU
/// dep with a local kick on the 0->1 transition.
pub fn tick_nohz_dep_set_cpu(cpu: usize, bit: u32) {
    let ts = ts_of(cpu);
    let prev = ts.tick_dep_mask.fetch_or(bit, Ordering::AcqRel);
    if prev == 0 && cpu == crate::arch::cpu_id() as usize {
        tick_nohz_full_kick_cpu(cpu);
    }
}

/// void tick_nohz_dep_clear_cpu(int cpu, enum tick_dep_bits bit)
pub fn tick_nohz_dep_clear_cpu(cpu: usize, bit: u32) {
    ts_of(cpu).tick_dep_mask.fetch_and(!bit, Ordering::AcqRel);
}

/// bool sched_can_stop_tick(struct rq *rq) — implemented next to the
/// run-queue internals (crate::sched::sched::sched_can_stop_tick); the
/// GRQ (D-1) is global, so the predicate reads the global counters.
/// sched_update_tick_dependency (sched.h) — the reference calls it from
/// add_nr_running()/sub_nr_running() at every rq load change; the GRQ
/// state is global, so can_stop_full_tick() evaluates sched_can_stop_tick
/// on the fly and this helper only performs the 0->1 transition side:
/// kick the CPUs whose tick is currently stopped so they re-evaluate at
/// their next exit.
pub fn sched_update_tick_dependency_on_load_change() {
    // The reference's sched_update_tick_dependency (sched.h) gates on
    // tick_nohz_full_cpu FIRST — the dependency machinery never runs
    // for CPUs outside the nohz_full mask. Without this gate the
    // can-stop predicate's per-CPU current scan lands in every enqueue
    // under the GRQ lock (measured: p2_ctxsw 1.242 -> 2.149 us, +73%).
    if !NOHZ_FULL_RUNNING.load(Ordering::Acquire) {
        return;
    }
    if !crate::sched::sched::sched_can_stop_tick() {
        tick_nohz_full_kick_stopped();
    }
}

/// tick_nohz_full_kick_cpu(int cpu) — the reference raises an empty
/// irq_work so the target re-evaluates at its irq_exit; the resched IPI
/// is this tree's kick primitive (its handler exit runs
/// tick_nohz_irq_exit). Skipped when the target is not tick-stopped —
/// a ticking CPU re-evaluates on its own next tick.
pub fn tick_nohz_full_kick_cpu(cpu: usize) {
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    if !TICK_CPU_SCHED[cpu].tick_stopped.load(Ordering::Acquire) {
        return;
    }
    if cpu != crate::arch::cpu_id() as usize {
        crate::sched::sched::resched_cpu(cpu);
    }
}

fn tick_nohz_full_kick_stopped() {
    let this = crate::arch::cpu_id() as usize;
    for cpu in 0..crate::config::MAX_CPUS {
        if cpu != this && TICK_CPU_SCHED[cpu].tick_stopped.load(Ordering::Acquire) {
            crate::sched::sched::resched_cpu(cpu);
        }
    }
}

fn tick_nohz_full_kick_all() {
    let this = crate::arch::cpu_id() as usize;
    for cpu in 0..crate::config::MAX_CPUS {
        if cpu != this {
            crate::sched::sched::resched_cpu(cpu);
        }
    }
}

/// static bool can_stop_full_tick(int cpu, struct tick_sched *ts) —
/// global deps + per-CPU deps + the current task's deps. The reference
/// reads current->tick_dep_mask / current->signal->tick_dep_mask (set
/// by posix CPU timers); this tree's cpu-time itimers store their
/// deadline in the task directly, so the same predicate reads
/// itimer_virt/itimer_prof deadlines — armed ⟺ deadline != 0.
fn can_stop_full_tick(cpu: usize) -> bool {
    if check_tick_dependency(TICK_DEP_MASK.load(Ordering::Acquire)) {
        return false;
    }
    if check_tick_dependency(ts_of(cpu).tick_dep_mask.load(Ordering::Acquire)) {
        return false;
    }
    // current->tick_dep_mask / signal->tick_dep_mask equivalent:
    // POSIX CPU timers need the tick to elapse.
    if let Some(task) = crate::sched::current() {
        {
            let virt = task.itimer_virt[0].load(Ordering::Acquire);
            let prof = task.itimer_prof[0].load(Ordering::Acquire);
            if virt != 0 || prof != 0 {
                return false;
            }
        }
    }
    // The SCHED dependency (sched_update_tick_dependency's per-cpu bit
    // in the reference) — the GRQ is global, so this reads the live
    // predicate.
    if !crate::sched::sched::sched_can_stop_tick() {
        return false;
    }
    true
}

/// static ktime_t tick_nohz_next_event(ts, cpu) — 0 means "retain the
/// tick" (the reference's `goto out` with timer_expires = 0).
/// `now` is the current nominal-domain time, `grid` the tick period in
/// the same domain.
fn tick_nohz_next_event(cpu: usize, now: u64, grid: u64) -> u64 {
    // "Keep the periodic tick, when RCU, architecture or irq_work
    // requests it. Aside of that check whether the local timer softirq
    // is pending..." — Rux: pending timer/hrtimer softirq work (an
    // already-expired timer would arm a minimal delta and bounce back
    // here); Rux's RCU makes no tick request (GP progress rides
    // context switches), no irq_work.
    // local_timer_softirq_pending(): only the timer-class bits (the
    // reference checks the TIMER softirq specifically — an already-
    // expired timer would arm a minimal delta and bounce right back).
    let timer_bits = (1 << crate::interrupt::softirq::SoftirqIndex::Timer as u32)
        | (1 << crate::interrupt::softirq::SoftirqIndex::Hrtimer as u32);
    if crate::interrupt::softirq::softirq_pending_raw(cpu) & timer_bits != 0 {
        return now + grid;
    }
    // get_next_timer_interrupt(): the next pending wheel deadline
    // (u64::MAX when none — the reference wheel's
    // NEXT_TIMER_MAX_DELTA return).
    let next_tick = crate::timer::hres_next_deadline();

    // "If the tick is due in the next period, keep it ticking or force
    // prod the timer."
    if next_tick != KTIME_MAX {
        let delta = next_tick.saturating_sub(now);
        if delta <= grid && !ts_of(cpu).tick_stopped.load(Ordering::Acquire) {
            return 0;
        }
    } else if !ts_of(cpu).tick_stopped.load(Ordering::Acquire) {
        // No timer anywhere and the tick has not stopped yet: the
        // reference's wheel would return a far-future next_tick, which
        // lands in the same stop path. Fall through and stop.
    }

    // "If this CPU is the one which had the do_timer() duty last, we
    // limit the sleep time to the timekeeping max_deferment value.
    // Otherwise we can sleep as long as we want."
    let cap = if cpu == tick_do_timer_cpu() {
        TIMEKEEPING_MAX_DEFERMENT
    } else {
        KTIME_MAX
    };

    // "Calculate the next expiry time ... ts->timer_expires =
    // min_t(u64, expires, next_tick)".
    let expires = if cap != KTIME_MAX && cap < KTIME_MAX - now {
        now + cap
    } else {
        KTIME_MAX
    };
    if next_tick < expires {
        next_tick
    } else {
        expires
    }
}

/// The do_timer duty holder: the boot CPU. The reference rotates
/// tick_do_timer_cpu on idle hand-off; with jiffies derived from the
/// timebase (not from IRQ counts) a fixed holder is equivalent — any
/// CPU that keeps waking runs the update, and cpu 0 is the only one
/// with a bounded sleep.
fn tick_do_timer_cpu() -> usize {
    0
}

/// static void tick_nohz_stop_tick(ts, cpu) — program the event at the
/// computed expiry; KTIME_MAX parks the device at its maximum delta.
/// `arm` is the arch one-shot primitive (Rux: timer_arm_oneshot's
/// nominal-domain deadline).
fn tick_nohz_stop_tick(cpu: usize, expires: u64) {
    let ts = ts_of(cpu);
    let tick = expires;

    // "Skip reprogram of event if it's not changed."
    if ts.tick_stopped.load(Ordering::Acquire) && tick == ts.next_tick.load(Ordering::Acquire) {
        return;
    }

    // "In the first call we save the current tick time, so we can
    // restart the scheduler tick in nohz_restart_sched_tick."
    // (ts->last_tick: not needed here — the restart arms the next grid
    // tick from the current time, matching the reference's
    // tick_nohz_restart() forward-clamping.)
    ts.tick_stopped.store(true, Ordering::Release);
    ts.next_tick.store(tick, Ordering::Release);

    if tick == KTIME_MAX {
        // "If the expiration time == KTIME_MAX, then we simply stop the
        // tick timer." The one-shot arm clamps to the device maximum.
        crate::drivers::timer::set_timer(KTIME_MAX);
        return;
    }
    crate::drivers::timer::set_timer(tick);
}

/// static void tick_nohz_stop_sched_tick(ts, cpu)
fn tick_nohz_stop_sched_tick(cpu: usize, now: u64, grid: u64) {
    let expires = tick_nohz_next_event(cpu, now, grid);
    if expires != 0 {
        tick_nohz_stop_tick(cpu, expires);
    } else {
        // tick_nohz_retain_tick(): the reference's periodic tick is a
        // forwarded hrtimer that re-arms itself; this tree's LAPIC is
        // one-shot, so the retain case re-arms the grid glue here.
        crate::drivers::timer::set_next_trigger();
    }
}

/// static void tick_nohz_restart_sched_tick(ts, now) — resume the
/// periodic tick: clear tick_stopped and arm the next grid tick (the
/// reference's tick_nohz_restart() forward-clamps to the next period).
fn tick_nohz_restart_sched_tick(cpu: usize, now: u64, grid: u64) {
    let ts = ts_of(cpu);
    ts.tick_stopped.store(false, Ordering::Release);
    ts.next_tick.store(0, Ordering::Release);
    // tick_nohz_restart(): tick_do_update_jiffies64(now) — Rux's jiffies
    // advance from the timebase in the handler; arm the next grid tick.
    let deadline = (now / grid + 1) * grid;
    crate::drivers::timer::set_timer(deadline);
}

/// static void __tick_nohz_full_update_tick(ts, now)
fn tick_nohz_full_update_tick_at(cpu: usize, now: u64, grid: u64) {
    if can_stop_full_tick(cpu) {
        tick_nohz_stop_sched_tick(cpu, now, grid);
    } else if ts_of(cpu).tick_stopped.load(Ordering::Acquire) {
        tick_nohz_restart_sched_tick(cpu, now, grid);
    }
}

/// nohz_full=<cpulist> (the reference's __setup("nohz_full=...")):
/// comma/hyphen CPU list, e.g. nohz_full=1 or nohz_full=1-7. CPUs in
/// the list run the full-dynticks stop machinery; absent parameter =
/// no full-dynticks CPUs (the reference's default) — the idle stop
/// (NO_HZ_COMMON) still covers every CPU.
static NOHZ_FULL_MASK: AtomicU64 = AtomicU64::new(0);
/// bool tick_nohz_full_running.
static NOHZ_FULL_RUNNING: AtomicBool = AtomicBool::new(false);

/// bool tick_nohz_full_cpu(int cpu)
pub fn tick_nohz_full_cpu(cpu: usize) -> bool {
    NOHZ_FULL_RUNNING.load(Ordering::Acquire)
        && cpu < crate::config::MAX_CPUS
        && NOHZ_FULL_MASK.load(Ordering::Acquire) & (1u64 << cpu) != 0
}

/// __setup("nohz_full=") — parse the cpulist into the mask.
pub fn nohz_full_setup(arg: &str) {
    let mut mask: u64 = 0;
    for part in arg.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('-') {
            Some((a, b)) => {
                if let (Ok(lo), Ok(hi)) = (a.trim().parse::<u64>(), b.trim().parse::<u64>()) {
                    let (lo, hi) = (lo.min(hi), lo.max(hi));
                    for c in lo..=hi {
                        if c < 64 {
                            mask |= 1u64 << c;
                        }
                    }
                }
            }
            None => {
                if let Ok(c) = part.parse::<u64>() {
                    if c < 64 {
                        mask |= 1u64 << c;
                    }
                }
            }
        }
    }
    NOHZ_FULL_MASK.store(mask, Ordering::Release);
    NOHZ_FULL_RUNNING.store(mask != 0, Ordering::Release);
}

/// static void tick_nohz_full_update_tick(ts) — first gate is the
/// reference's `if (!tick_nohz_full_cpu(...)) return;`.
pub fn tick_nohz_full_update_tick(cpu: usize) {
    if !tick_nohz_full_cpu(cpu) {
        // Not a full-dynticks CPU: keep the plain grid-glue arm — the
        // LAPIC is one-shot, so every tick fire MUST re-arm. This is
        // the pre-nohz behavior, unchanged for CPUs outside the
        // nohz_full mask (the reference's non-nohz_full CPUs likewise
        // keep their periodic tick).
        crate::drivers::timer::set_next_trigger();
        return;
    }
    let now = crate::drivers::timer::read_time();
    let grid = grid_period();
    tick_nohz_full_update_tick_at(cpu, now, grid);
}

/// void tick_nohz_irq_exit(void) — "When an interrupt fires while we
/// are idle and it doesn't cause a reschedule, it may still add, modify
/// or delete a timer ... So we need to re-calculate and reprogram the
/// next tick event." Called at IRQ exit: idle → start_idle (the same
/// stop machinery; the reference's tick_nohz_start_idle just stamps the
/// entry and the idle loop decides), else → full_update_tick.
pub fn tick_nohz_irq_exit() {
    let cpu = crate::arch::cpu_id() as usize;
    // The reference branches on ts->inidle (start_idle stamps; the
    // cpuidle loop stops the tick inside its own bracket). Rux's idle
    // loop calls schedule() every pass, so the same can_stop machinery
    // decides for both paths here: solo/idle → stop at the next event;
    // a dependency present → resume the periodic tick. Non-full CPUs
    // keep the grid glue (the gate inside full_update_tick).
    tick_nohz_full_update_tick(cpu);
}

/// void tick_nohz_idle_enter(void) — set inidle (state stamp; the stop
/// decision runs in tick_nohz_irq_exit for both paths on this tree).
pub fn tick_nohz_idle_enter() {
    let cpu = crate::arch::cpu_id() as usize;
    ts_of(cpu).inidle.store(true, Ordering::Release);
}

/// void tick_nohz_idle_exit(void) — clear inidle (state stamp).
pub fn tick_nohz_idle_exit() {
    let cpu = crate::arch::cpu_id() as usize;
    ts_of(cpu).inidle.store(false, Ordering::Release);
}

/// The grid period in the nominal read_time() domain (1 s = 10 MHz
/// counts, so one period = 10 MHz / KERNEL_HZ).
#[inline]
fn grid_period() -> u64 {
    10_000_000 / (crate::config::KERNEL_HZ as u64)
}

/// The timer-enqueue re-arm (rearm_for_hres): the sleeper's CPU
/// re-evaluates immediately — tick_nohz_next_event arms at the new
/// wheel deadline, the retain case keeps the grid glue. The
/// reference's timer enqueue re-programs the local clockevent the
/// same way (and kicks a remote idle CPU via wake_up_nohz_cpu; a
/// remote CPU here re-evaluates at its own next exit).
pub fn tick_nohz_rearm_local() {
    let cpu = crate::arch::cpu_id() as usize;
    let now = crate::drivers::timer::read_time();
    let grid = grid_period();
    let ts = ts_of(cpu);
    if can_stop_full_tick(cpu) {
        tick_nohz_stop_sched_tick(cpu, now, grid);
    } else if ts.tick_stopped.load(Ordering::Acquire) {
        tick_nohz_restart_sched_tick(cpu, now, grid);
    } else {
        // Not stopped and cannot stop: the plain grid-glue arm.
        let deadline = (now / grid + 1) * grid;
        let hres = crate::timer::hres_next_deadline();
        let deadline = if hres < deadline { hres.max(now + 1) } else { deadline };
        crate::drivers::timer::set_timer(deadline);
    }
}
