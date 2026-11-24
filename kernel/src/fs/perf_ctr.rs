//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The KERNEL-WIDE ACCOUNTING CELLS (the fs-substrate split, 2026-11-23):
//! the counters /proc/{stat,interrupts,loadavg,schedstat,uptime} REPORT
//! moved here from the procfs leaf, framework-side — the mutation paths
//! (arch trap tick charging, scheduler context-switch/fork notes, the
//! loadavg sampler, idle accounting) stay DIRECT framework calls with
//! zero indirection (the S1 devirtualization rule: no fn-pointer rows on
//! the trap/sched hot paths). The procfs leaf (fs crate) formats
//! these cells in its generate() paths. Linux alignment: the reference
//! keeps these in kernel/sched + per-cpu irq counters, not in procfs.
//!
//! Everything here is atomics — no locks, IRQ-context safe.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ============================================================================
// /proc/interrupts cells
// ============================================================================

const MAX_CPUS: usize = crate::config::MAX_CPUS;

/// Per-CPU timer interrupt counters.
pub static TIMER_COUNT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Per-CPU software-interrupt (IPI) counters.
pub static SOFT_COUNT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Charge one timer interrupt to `cpu` (arch trap path).
pub fn timer_inc(cpu: usize) {
    if cpu < MAX_CPUS {
        TIMER_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
    }
}

/// Charge one software interrupt to `cpu` (arch trap path).
pub fn soft_inc(cpu: usize) {
    if cpu < MAX_CPUS {
        SOFT_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
    }
}

/// The per-CPU timer count (procfs generation).
pub fn timer_count(cpu: usize) -> u64 {
    if cpu < MAX_CPUS {
        TIMER_COUNT[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// The per-CPU softirq count (procfs generation).
pub fn soft_count(cpu: usize) -> u64 {
    if cpu < MAX_CPUS {
        SOFT_COUNT[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}

// ============================================================================
// /proc/stat cells
// ============================================================================

/// One CPU's time accounting, in jiffies (Linux /proc/stat columns).
///
/// `nice`, `iowait`, `irq`, `softirq`, `steal`, `guest` and
/// `guest_nice` exist because Linux's file has those columns; Rux has
/// no nice levels, no iowait accounting and no IRQ-time accounting
/// (CONFIG_IRQ_TIME_ACCOUNTING is off on many Linux boxes too), so
/// they report zero rather than a fabricated number.
pub struct CpuTime {
    pub user: AtomicU64,
    pub nice: AtomicU64,
    pub system: AtomicU64,
    pub idle: AtomicU64,
    pub iowait: AtomicU64,
    pub irq: AtomicU64,
    pub softirq: AtomicU64,
    pub steal: AtomicU64,
    pub guest: AtomicU64,
    pub guest_nice: AtomicU64,
}

impl CpuTime {
    const fn new() -> Self {
        Self {
            user: AtomicU64::new(0),
            nice: AtomicU64::new(0),
            system: AtomicU64::new(0),
            idle: AtomicU64::new(0),
            iowait: AtomicU64::new(0),
            irq: AtomicU64::new(0),
            softirq: AtomicU64::new(0),
            steal: AtomicU64::new(0),
            guest: AtomicU64::new(0),
            guest_nice: AtomicU64::new(0),
        }
    }
}

pub static CPU_TIME: [CpuTime; MAX_CPUS] = [const { CpuTime::new() }; MAX_CPUS];

/// Total context switches since boot (Linux /proc/stat "ctxt").
pub static CTXT_SWITCHES: AtomicU64 = AtomicU64::new(0);

/// Total forks since boot (Linux /proc/stat "processes").
pub static FORKS: AtomicU64 = AtomicU64::new(0);

/// Charge one tick to the calling CPU's time accounting.
///
/// `user_mode` is the privilege mode the timer hit. The idle task
/// (pid 0) means the CPU had nothing to run — that is idle time, not
/// system time.
pub fn charge_tick(user_mode: bool) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }
    let idle = match crate::sched::current() {
        Some(task) => task.pid() == 0,
        None => true,
    };
    if idle {
        CPU_TIME[cpu].idle.fetch_add(1, Ordering::Relaxed);
    } else if user_mode {
        CPU_TIME[cpu].user.fetch_add(1, Ordering::Relaxed);
    } else {
        CPU_TIME[cpu].system.fetch_add(1, Ordering::Relaxed);
    }
}

/// Count one context switch and split it into the per-task voluntary /
/// involuntary buckets (Linux's `__schedule` accounting).
///
/// A task that was still runnable when it was switched out was
/// PREEMPTED — involuntary. A task that had gone to sleep had BLOCKED
/// — voluntary.
pub fn note_switch(prev: &crate::process::task::Task) {
    CTXT_SWITCHES.fetch_add(1, Ordering::Relaxed);
    // perf per-CPU context-switch accounting (PERF_COUNT_SW_CONTEXT_
    // SWITCHES; one relaxed load when no perf event wants it).
    crate::perf::note_ctxsw();
    if prev.pid() == 0 {
        return; // the idle task is not a process
    }
    let state = prev.state().bits();
    let blocked = state & (crate::process::task::TaskState::INTERRUPTIBLE | crate::process::task::TaskState::UNINTERRUPTIBLE) != 0;
    if blocked {
        prev.nvcsw.fetch_add(1, Ordering::Relaxed);
        // NOTE: this is a voluntary context switch (nvcsw), NOT a
        // schedstat yld_count — Linux counts yld_count only in the
        // sched_yield paths (kernel/sched/syscalls.c), so the schedstat
        // counter is bumped from sys_sched_yield, not here.
    } else {
        prev.nivcsw.fetch_add(1, Ordering::Relaxed);
    }
}

/// Count one fork (Linux /proc/stat "processes").
pub fn note_fork() {
    FORKS.fetch_add(1, Ordering::Relaxed);
}

// ============================================================================
// /proc/loadavg cells
// ============================================================================

/// Sample interval in jiffies (5s at HZ=250 — Linux's LOAD_FREQ).
const LOAD_FREQ: u64 = 5 * crate::config::KERNEL_HZ as u64;

/// Fixed-point exponential decay factors (1/5/15 min; 0.32 format).
const EXP_1: u64 = (0.920044414676f64 * (1u64 << 32) as f64) as u64;
const EXP_5: u64 = (0.983471453846f64 * (1u64 << 32) as f64) as u64;
const EXP_15: u64 = (0.994459848005f64 * (1u64 << 32) as f64) as u64;
const EXP_FACTOR: [u64; 3] = [EXP_1, EXP_5, EXP_15];

/// Fixed-point (0.32 format) load average accumulators.
/// To get the float value: avenrun[i] / 2^32 ≈ avenrun[i] >> 16 / 65536.
pub static AVENRUN: [AtomicU64; 3] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Jiffies counter at last load sample.
static LAST_LOAD_JIFFIES: AtomicU64 = AtomicU64::new(0);

/// Running task count at last sample (for /proc/loadavg output).
pub static LAST_RUNNING: AtomicU32 = AtomicU32::new(0);

/// Total task count at last sample (for /proc/loadavg output).
pub static LAST_TOTAL: AtomicU32 = AtomicU32::new(0);

/// Update load average from scheduler_tick().
/// Call this from the timer tick path; it auto-throttles to LOAD_FREQ.
///
/// IRQ-CONTEXT CONSTRAINT (the fbterm-input wedge): scheduler_tick() runs
/// from handle_timer_interrupt() with SIE=0. The old implementation walked
/// the PID hash here (pid_hash_collect_all + find_task_by_pid per pid),
/// taking pid-hash bucket spinlocks. On the single-CPU QEMU virt setup,
/// any task-context holder of a bucket lock (fork insert, wait4/kill
/// lookup, send_signal_to_pgid broadcast — the fbterm boot exercises all
/// of them) that is interrupted by the 5-second sample boundary wedges
/// the whole machine: the tick spins on lock_bucket forever with
/// interrupts off, so the holder can never be resumed — no more timer
/// ticks, no UART RX IRQs, every task frozen (screen stuck on the fbterm
/// banner, serial input dead). GDB proof: pc spinning in lock_bucket,
/// backtrace scheduler_tick -> update_load_avg -> pid_hash_collect_all.
///
/// The sample therefore reads ONLY scheduler-owned atomic state. /proc
/// semantics are unaffected: `running` feeds the EMA and the display
/// field, and generate() re-counts `total`/`last_pid` in the read
/// (task) path, where taking bucket locks is legal.
pub fn update_load_avg() {
    let now = crate::timer::get_jiffies();
    let last = LAST_LOAD_JIFFIES.load(Ordering::Relaxed);
    if now.wrapping_sub(last) < LOAD_FREQ {
        return;
    }

    // Try to claim the update slot. CAS avoids multiple CPUs updating simultaneously.
    if LAST_LOAD_JIFFIES.compare_exchange(last, now, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return;
    }

    // Runnable+running tasks across all scheduling classes — lock-free
    // atomic reads (per-class nr_running counters), safe in IRQ context.
    let running = crate::sched::sched::GlobalRunQueue::grq_nr_running() as u64;

    LAST_RUNNING.store(running as u32, Ordering::Relaxed);
    LAST_TOTAL.store(running as u32, Ordering::Relaxed);

    // Exponential moving average: avenrun = avenrun * exp + nrun * (1 - exp)
    // In fixed-point: avenrun = avenrun * exp >> 32 + nrun * ((1<<32 - exp) >> 32)
    for i in 0..3 {
        let prev = AVENRUN[i].load(Ordering::Relaxed);
        let exp = EXP_FACTOR[i];
        // Fixed-point EMA: (prev*exp)>>32 keeps the full precision — the
        // old (prev>>32)*(exp>>32) truncated both words to ~0, so the
        // averages never moved off zero and increments rounded away
        // (review VFS-M11). The product is computed in u128: with
        // load > 1.0 (prev > 2^32) a u64 multiply would overflow.
        let decayed = (((prev as u128) * (exp as u128)) >> 32) as u64;
        let new_val = decayed + running * ((1u64 << 32) - exp);
        AVENRUN[i].store(new_val, Ordering::Relaxed);
    }
}

// ============================================================================
// /proc/schedstat cells
// ============================================================================

/// Voluntary context switches (yields) per CPU.
pub static YLD_COUNT: [AtomicU64; MAX_CPUS] =
    [const { AtomicU64::new(0) }; MAX_CPUS];

/// Scheduler invocations per CPU.
pub static SCHED_COUNT: [AtomicU64; MAX_CPUS] =
    [const { AtomicU64::new(0) }; MAX_CPUS];

/// Count one sched_yield() served by `cpu` (Linux's rq->yld_count,
/// incremented only in the sched_yield paths — kernel/sched/syscalls.c).
pub fn note_yield(cpu: usize) {
    if cpu < MAX_CPUS {
        YLD_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
    }
}

/// Count one scheduler invocation on `cpu`.
pub fn note_sched_run(cpu: usize) {
    if cpu < MAX_CPUS {
        SCHED_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
    }
}

// ============================================================================
// /proc/uptime cells
// ============================================================================

/// Per-CPU cumulative cycles spent in the idle loop's WFI (P2: real idle
/// accounting for /proc/uptime). Charged by cpu_idle_loop() around each
/// wfi; the sum across CPUs is the second /proc/uptime column.
pub static IDLE_WFI_CYCLES: [AtomicU64; MAX_CPUS] =
    [const { AtomicU64::new(0) }; MAX_CPUS];

/// Charge `cycles` of idle time to the calling CPU (idle loop only).
pub fn account_idle_cycles(cpu: usize, cycles: u64) {
    if cpu < MAX_CPUS {
        IDLE_WFI_CYCLES[cpu].fetch_add(cycles, Ordering::Relaxed);
    }
}

/// Total idle cycles summed across all CPUs.
pub fn total_idle_cycles() -> u64 {
    let mut sum = 0u64;
    for c in IDLE_WFI_CYCLES.iter() {
        sum = sum.saturating_add(c.load(Ordering::Relaxed));
    }
    sum
}
