//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Whole-syscall cost table + tick-mode accounting + kernel-rip page
//! histogram for the systematic performance-audit line
//! (`feature/perf-audit`, docs/development/perf-audit.md).
//!
//! Everything here is armed by the kernel cmdline `syscost=1` and costs
//! nothing when off (one cached atomic load per syscall exit; the tick
//! hooks are two relaxed adds). No `dfx/switches.rs` bit is consumed —
//! that file belongs to the wild-write line; this probe owns its own
//! cmdline key so the branches can merge without coordination.
//!
//! What it measures and why:
//!
//! 1. `enter`/`exit` around `syscall::dispatch::syscall_handler` —
//!    per-native-nr { count, wall cycles, max } plus a small ring of
//!    >4 ms outliers with the caller comm. Wall cycles include blocking
//!    (futex sleep, pipe waits) — deliberately: the ranking question is
//!    "where does the 16-23 min OH boot spend wall time", and CPU-vs-
//!    block is separated by the tick-mode counters below. Diverging
//!    paths (do_exit from exit_group / fatal signals) skip the exit
//!    hook, so those calls are undercounted; noted in the report.
//!
//! 2. Tick-mode counters per CPU — user / kernel-task / idle — from the
//!    same timer-tick site the stime accounting uses. `idle` means the
//!    tick landed with no user address space current (idle task or the
//!    boot kernel context). This decomposes the boot into "CPU-saturated
//!    in kernel", "CPU-saturated in user" and "idle-blocked" — the
//!    first split of the systematic diagnosis.
//!
//! 3. A 256-slot kernel-text page histogram sampled on kernel-mode
//!    ticks (same sample site as sc105's STIME_HIST, but global and
//!    comm-free so no task starves the table; per-task attribution
//!    comes from taskdump's utime/stime columns). Pages are symbolized
//!    offline against the built ELF's nm(1) output.
//!
//! Dump: `tick()` is called from the timer softirq; every
//! SYSCOST_DUMP_SECS it prints the tables (sorted, compact, one line
//! each) plus the current uptime so host-side serial timestamps can be
//! correlated with guest time.

use core::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};

use crate::alloc::string::ToString;

/// Syscall-number slots (native x86_64 numbers go up to ~335; generic
/// table numbers stay below; 512 leaves headroom).
const NR_SLOTS: usize = 512;

/// Kernel-text page histogram slots.
const RIP_SLOTS: usize = 256;

/// Slow-op ring size (>4 ms wall).
const SLOW_RING: usize = 64;

/// Slow-op threshold in raw TSC cycles. Calibrated at dump time against
/// read_time(); 4 ms at ~3 GHz is ~12M cycles — 10M is a safe floor
/// that only genuine outliers cross (a normal fast syscall is 1-30 us).
const SLOW_CYC: u64 = 10_000_000;

/// Dump cadence in jiffies (100 Hz -> 60 s).
const SYSCOST_DUMP_SECS: u64 = 60;

struct SysStat {
    count: AtomicU64,
    cyc: AtomicU64,
    max: AtomicU64,
}

const EMPTY_SYS: SysStat = SysStat {
    count: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
};

static TABLE: [SysStat; NR_SLOTS] = [EMPTY_SYS; NR_SLOTS];

struct RipSlot {
    page: AtomicU64,
    count: AtomicU64,
}

const EMPTY_RIP: RipSlot = RipSlot {
    page: AtomicU64::new(0),
    count: AtomicU64::new(0),
};

static RIP_HIST: [RipSlot; RIP_SLOTS] = [EMPTY_RIP; RIP_SLOTS];

struct SlowEnt {
    nr: AtomicU64,
    cyc: AtomicU64,
    comm: [AtomicU8; 16],
}

const EMPTY_SLOW: SlowEnt = SlowEnt {
    nr: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    comm: [const { AtomicU8::new(0) }; 16],
};

static SLOW: [SlowEnt; SLOW_RING] = [EMPTY_SLOW; SLOW_RING];
static SLOW_IDX: AtomicUsize = AtomicUsize::new(0);

/// Per-CPU tick modes: user / kernel-task / idle.
static TICK_USER: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
static TICK_KERN: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
static TICK_IDLE: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

static LAST_DUMP: AtomicU64 = AtomicU64::new(0);

/// Armed cache: 0 unknown, 1 off, 2 on (cmdline `syscost=1`).
static ARMED: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn on() -> bool {
    match ARMED.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = if crate::cmdline::get_param("syscost").as_deref() == Some("1") {
                2
            } else {
                1
            };
            ARMED.store(v, Ordering::Relaxed);
            v == 2
        }
    }
}

/// Syscall entry stamp. `None` when the probe is off.
#[inline]
pub fn enter() -> Option<u64> {
    if on() {
        Some(crate::arch::cpu::read_counter())
    } else {
        None
    }
}

/// Syscall exit accounting (wall cycles include blocking by design).
#[inline]
pub fn exit(nr: usize, t0: Option<u64>) {
    let t0 = match (t0, on()) {
        (Some(t), true) => t,
        _ => return,
    };
    if nr >= NR_SLOTS {
        return;
    }
    let dt = crate::arch::cpu::read_counter().saturating_sub(t0);
    let e = &TABLE[nr];
    e.count.fetch_add(1, Ordering::Relaxed);
    e.cyc.fetch_add(dt, Ordering::Relaxed);
    // Monotonic-ish max: CAS grow; a lost race keeps the older max.
    let mut m = e.max.load(Ordering::Relaxed);
    while dt > m {
        match e
            .max
            .compare_exchange_weak(m, dt, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(cur) => m = cur,
        }
    }
    if dt > SLOW_CYC {
        let i = SLOW_IDX.fetch_add(1, Ordering::Relaxed) % SLOW_RING;
        SLOW[i].nr.store(nr as u64, Ordering::Relaxed);
        SLOW[i].cyc.store(dt, Ordering::Relaxed);
        if let Some(t) = crate::sched::current() {
            // SAFETY: current task; comm is a fixed-size field.
            let c = unsafe { (*t).comm() };
            for j in 0..16 {
                SLOW[i].comm[j].store(c[j], Ordering::Relaxed);
            }
        }
    }
}

/// Tick-mode accounting hook — call once per timer tick that landed in
/// a context with a user address space (`user_mode` per the trap
/// frame), plus the idle variant for ticks without one.
#[inline]
pub fn tick_user() {
    if !on() {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        TICK_USER[c].fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
pub fn tick_kern(rip: u64) {
    if !on() {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        TICK_KERN[c].fetch_add(1, Ordering::Relaxed);
    }
    rip_sample(rip);
}

#[inline]
pub fn tick_idle() {
    if !on() {
        return;
    }
    let c = crate::arch::cpu_id() as usize;
    if c < crate::config::MAX_CPUS {
        TICK_IDLE[c].fetch_add(1, Ordering::Relaxed);
    }
}

/// Kernel-text page sample (mirrors sc105 STIME_HIST, comm-free so the
/// whole boot crowd shares one table).
#[inline]
fn rip_sample(rip: u64) {
    let (lo, hi) = crate::sched::sched::kernel_text_window();
    if rip < lo || rip >= hi {
        return;
    }
    let page = rip & !0xFFFu64;
    // Fast path: find an existing slot.
    let mut min_i = 0usize;
    let mut min_c = u64::MAX;
    for i in 0..RIP_SLOTS {
        let p = RIP_HIST[i].page.load(Ordering::Relaxed);
        if p == page {
            RIP_HIST[i].count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if p == 0 {
            // Claim empty slot.
            if RIP_HIST[i]
                .page
                .compare_exchange(0, page, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                RIP_HIST[i].count.store(1, Ordering::Relaxed);
                return;
            }
            continue;
        }
        let c = RIP_HIST[i].count.load(Ordering::Relaxed);
        if c < min_c {
            min_c = c;
            min_i = i;
        }
    }
    // Steal the least-sampled slot.
    if RIP_HIST[min_i]
        .count
        .compare_exchange_weak(min_c, 1, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        RIP_HIST[min_i].page.store(page, Ordering::Relaxed);
    }
}

/// Calibrate raw TSC/counter cycles -> microseconds, against the
/// calibrated timekeeper (`read_time()` nominal @10 MHz), over a ~2 ms
/// busy window at dump time.
fn cycles_per_us() -> u64 {
    let n0 = crate::drivers::timer::read_time();
    let c0 = crate::arch::cpu::read_counter();
    // Busy-wait ~2 ms nominal (20_000 ticks @10 MHz). Loop on the raw
    // counter so the loop itself cannot be constant-folded.
    while crate::arch::cpu::read_counter().wrapping_sub(c0) < 20_000_000 {
        core::hint::spin_loop();
    }
    let n1 = crate::drivers::timer::read_time();
    let c1 = crate::arch::cpu::read_counter();
    let dn = n1.saturating_sub(n0); // nominal ticks (100 ns each)
    let dc = c1.saturating_sub(c0); // raw counter ticks
    if dn == 0 || dc == 0 {
        return 3000; // ~3 GHz fallback
    }
    // us = cyc * (dn * 100ns->us/10) / dc  => cyc_per_us = dn*10/dc
    let per_us = dn * 10 / dc;
    if per_us == 0 {
        1
    } else {
        per_us
    }
}

/// Periodic dump from the timer softirq. Cheap when off.
pub fn tick(now_jiffies: u64) {
    if !on() {
        return;
    }
    if now_jiffies.saturating_sub(LAST_DUMP.load(Ordering::Relaxed))
        < SYSCOST_DUMP_SECS * crate::config::KERNEL_HZ as u64
    {
        return;
    }
    LAST_DUMP.store(now_jiffies, Ordering::Relaxed);
    dump();
}

/// Print the full report (also usable from a debug trigger).
pub fn dump() {
    let cpu = cycles_per_us();
    let secs = crate::drivers::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;

    // Tick modes: one line per dump.
    let mut line = alloc::string::String::from("SYSCOST t=");
    line.push_str(&secs.to_string());
    line.push_str("s cpu_per_us=");
    line.push_str(&cpu.to_string());
    line.push_str(" ticks(u/k/i):");
    for c in 0..crate::config::MAX_CPUS {
        let u = TICK_USER[c].load(Ordering::Relaxed);
        let k = TICK_KERN[c].load(Ordering::Relaxed);
        let i = TICK_IDLE[c].load(Ordering::Relaxed);
        if u | k | i == 0 {
            continue;
        }
        line.push_str(&alloc::format!(" c{}={}/{}/{}", c, u, k, i));
    }
    crate::pr_info!("{}", line);

    // Syscall table: collect (nr, count, cyc, max), sort by cyc desc.
    let mut rows: alloc::vec::Vec<(usize, u64, u64, u64)> = alloc::vec::Vec::new();
    for i in 0..NR_SLOTS {
        let n = TABLE[i].count.load(Ordering::Relaxed);
        if n == 0 {
            continue;
        }
        rows.push((
            i,
            n,
            TABLE[i].cyc.load(Ordering::Relaxed),
            TABLE[i].max.load(Ordering::Relaxed),
        ));
    }
    rows.sort_by(|a, b| b.2.cmp(&a.2));
    let total_cyc: u64 = rows.iter().map(|r| r.2).sum();
    crate::pr_info!(
        "SYSCOST t={}s syscalls: {} kinds {} calls tot={} ms",
        secs,
        rows.len(),
        rows.iter().map(|r| r.1).sum::<u64>(),
        total_cyc / cpu / 1000
    );
    for r in rows.iter().take(48) {
        crate::pr_info!(
            "SYSCOST t={}s nr={} n={} avg={}us max={}ms tot={}ms",
            secs,
            r.0,
            r.1,
            r.2 / cpu / r.1.max(1) / 1, // us
            r.3 / cpu / 1000,
            r.2 / cpu / 1000
        );
    }

    // Slow ring (last SLOW_RING outliers, ring order).
    crate::pr_info!("SYSCOST t={}s slow-ring (>4ms wall):", secs);
    let base = SLOW_IDX.load(Ordering::Relaxed);
    for k in 0..SLOW_RING {
        let i = (base + SLOW_RING - 1 - k) % SLOW_RING; // newest first
        let cyc = SLOW[i].cyc.load(Ordering::Relaxed);
        if cyc == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = SLOW[i].comm[j].load(Ordering::Relaxed);
        }
        let name = core::str::from_utf8(&comm)
            .unwrap_or("?")
            .split('\0')
            .next()
            .unwrap_or("?");
        crate::pr_info!(
            "SYSCOST t={}s slow nr={} cyc={}us comm={}",
            secs,
            SLOW[i].nr.load(Ordering::Relaxed),
            cyc / cpu,
            name
        );
    }

    // Kernel-rip page histogram: top 32 by count.
    let mut pages: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
    for i in 0..RIP_SLOTS {
        let p = RIP_HIST[i].page.load(Ordering::Relaxed);
        if p == 0 {
            continue;
        }
        pages.push((p, RIP_HIST[i].count.load(Ordering::Relaxed)));
    }
    pages.sort_by(|a, b| b.1.cmp(&a.1));
    let tot: u64 = pages.iter().map(|p| p.1).sum();
    crate::pr_info!(
        "SYSCOST t={}s rip-pages: {} pages {} kern-ticks",
        secs,
        pages.len(),
        tot
    );
    for p in pages.iter().take(32) {
        crate::pr_info!(
            "SYSCOST t={}s rip page={:#x} n={} ({}%)",
            secs,
            p.0,
            p.1,
            p.1 * 100 / tot.max(1)
        );
    }
}
