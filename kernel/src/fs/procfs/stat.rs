//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/stat — per-CPU time accounting and system counters.
//!
//! Linux's /proc/stat is the first file anyone reads when the question
//! is "where did the time go". Its layout:
//!
//! ```text
//! cpu  <user> <nice> <system> <idle> <iowait> <irq> <softirq> <steal> <guest> <guest_nice>
//! cpu0 ...
//! ...
//! intr <total interrupts since boot>
//! ctxt <total context switches since boot>
//! btime <boot time, epoch seconds>
//! processes <forks since boot>
//! procs_running <runnable tasks>
//! procs_blocked <tasks blocked on I/O>
//! ```
//!
//! Accounting model (Linux's `account_process_tick`): the timer tick
//! charges one jiffy to whatever was running — the IDLE time when the
//! CPU had nothing to run, the task's USER time when the tick hit it in
//! user mode, and SYSTEM time when it hit it in kernel mode. A kernel
//! thread (no address space) is SYSTEM time, not idle time.
//!
//! The per-task half of the same accounting (utime/stime) lives on the
//! `Task` and is charged by the architecture tick handler; the switches
//! are counted in the scheduler's `context_switch`.

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::process::task::TaskState;

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

static CPU_TIME: [CpuTime; crate::config::MAX_CPUS] =
    [const { CpuTime::new() }; crate::config::MAX_CPUS];

/// Total context switches since boot (Linux /proc/stat "ctxt").
static CTXT_SWITCHES: AtomicU64 = AtomicU64::new(0);

/// Total forks since boot (Linux /proc/stat "processes").
static FORKS: AtomicU64 = AtomicU64::new(0);

/// Charge one tick to the calling CPU's time accounting.
///
/// `user_mode` is the privilege mode the timer hit. The idle task
/// (pid 0) means the CPU had nothing to run — that is idle time, not
/// system time.
pub fn charge_tick(user_mode: bool) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
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
    if prev.pid() == 0 {
        return; // the idle task is not a process
    }
    let state = prev.state().bits();
    let blocked = state & (TaskState::INTERRUPTIBLE | TaskState::UNINTERRUPTIBLE) != 0;
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

/// Total context switches since boot.
pub fn total_context_switches() -> u64 {
    CTXT_SWITCHES.load(Ordering::Relaxed)
}

/// Total forks since boot.
pub fn total_forks() -> u64 {
    FORKS.load(Ordering::Relaxed)
}

/// Number of runnable tasks (Linux "procs_running").
fn procs_running() -> u64 {
    crate::sched::sched::grq_diag_counts().0 as u64
}

/// Number of tasks blocked on I/O (Linux "procs_blocked": state D).
fn procs_blocked() -> u64 {
    let mut blocked = 0u64;
    // Non-blocking walk: a busy bucket is skipped rather than waited
    // for, so reading /proc/stat never blocks the caller.
    crate::process::pid_hash::pid_hash_for_each_task_try(|task| {
        // SAFETY: the pid-hash walk hands out live task pointers.
        let t = unsafe { &*task };
        if t.pid() != 0 && t.state().bits() & TaskState::UNINTERRUPTIBLE != 0 {
            blocked += 1;
        }
    });
    blocked
}

/// Boot time in epoch seconds (Linux "btime").
///
/// The wall-clock epoch offset is the realtime value that corresponds to
/// monotonic time zero, i.e. the moment the kernel started counting —
/// exactly what Linux reports in btime.
fn btime() -> u64 {
    crate::syscall::time::wall_epoch_offset_ns() / 1_000_000_000
}

/// Generate /proc/stat content (Linux layout).
pub fn generate() -> Vec<u8> {
    let mut out = String::new();
    let num_cpus = crate::arch::smp::num_started_cpus().min(crate::config::MAX_CPUS);

    // Aggregate line: the sum over all CPUs.
    let mut user = 0u64;
    let mut nice = 0u64;
    let mut system = 0u64;
    let mut idle = 0u64;
    let mut iowait = 0u64;
    let mut irq = 0u64;
    let mut softirq = 0u64;
    let mut steal = 0u64;
    let mut guest = 0u64;
    let mut guest_nice = 0u64;
    for cpu in 0..num_cpus {
        user += CPU_TIME[cpu].user.load(Ordering::Relaxed);
        nice += CPU_TIME[cpu].nice.load(Ordering::Relaxed);
        system += CPU_TIME[cpu].system.load(Ordering::Relaxed);
        idle += CPU_TIME[cpu].idle.load(Ordering::Relaxed);
        iowait += CPU_TIME[cpu].iowait.load(Ordering::Relaxed);
        irq += CPU_TIME[cpu].irq.load(Ordering::Relaxed);
        softirq += CPU_TIME[cpu].softirq.load(Ordering::Relaxed);
        steal += CPU_TIME[cpu].steal.load(Ordering::Relaxed);
        guest += CPU_TIME[cpu].guest.load(Ordering::Relaxed);
        guest_nice += CPU_TIME[cpu].guest_nice.load(Ordering::Relaxed);
    }
    out.push_str(&format!(
        "cpu  {} {} {} {} {} {} {} {} {} {}\n",
        user, nice, system, idle, iowait, irq, softirq, steal, guest, guest_nice
    ));

    // Per-CPU lines.
    for cpu in 0..num_cpus {
        out.push_str(&format!(
            "cpu{} {} {} {} {} {} {} {} {} {} {}\n",
            cpu,
            CPU_TIME[cpu].user.load(Ordering::Relaxed),
            CPU_TIME[cpu].nice.load(Ordering::Relaxed),
            CPU_TIME[cpu].system.load(Ordering::Relaxed),
            CPU_TIME[cpu].idle.load(Ordering::Relaxed),
            CPU_TIME[cpu].iowait.load(Ordering::Relaxed),
            CPU_TIME[cpu].irq.load(Ordering::Relaxed),
            CPU_TIME[cpu].softirq.load(Ordering::Relaxed),
            CPU_TIME[cpu].steal.load(Ordering::Relaxed),
            CPU_TIME[cpu].guest.load(Ordering::Relaxed),
            CPU_TIME[cpu].guest_nice.load(Ordering::Relaxed),
        ));
    }

    // Linux's intr line: the grand total, then one sum over CPUs per
    // IRQ, in IRQ index order (fs/proc/stat.c show_stat). Linux
    // accumulates the total from the very values it prints; Rux reads
    // each source once and sums those, so the total always equals the
    // tail (no read-skew between two passes). The per-IRQ tail uses
    // the same source list as /proc/interrupts: the two arch-internal
    // vectors first (timer, IPI soft-interrupt), then the external
    // (PLIC) lines.
    out.push_str("intr ");
    {
        let mut total = 0u64;
        let mut line = String::new();
        // Arch-internal vectors (not in the PLIC irq space).
        for cpu in 0..num_cpus {
            let v = super::interrupts::timer_count(cpu);
            total = total.saturating_add(v);
            line.push(' ');
            line.push_str(&v.to_string());
        }
        for cpu in 0..num_cpus {
            let v = super::interrupts::soft_count(cpu);
            total = total.saturating_add(v);
            line.push(' ');
            line.push_str(&v.to_string());
        }
        // External IRQs, IRQ index order 0..48 (rows 16..31 do not
        // exist on either board we boot; Linux prints its full NR_IRQS
        // space the same way, holes included).
        for irq in 0..48u32 {
            let mut sum = 0u64;
            for cpu in 0..num_cpus {
                sum = sum.saturating_add(crate::interrupt::irq_get_count(irq, cpu));
            }
            total = total.saturating_add(sum);
            line.push(' ');
            line.push_str(&sum.to_string());
        }
        out.push_str(&total.to_string());
        out.push_str(&line);
    }
    out.push('\n');
    out.push_str(&format!("ctxt {}\n", total_context_switches()));
    out.push_str(&format!("btime {}\n", btime()));
    out.push_str(&format!("processes {}\n", total_forks()));
    out.push_str(&format!("procs_running {}\n", procs_running()));
    out.push_str(&format!("procs_blocked {}\n", procs_blocked()));
    // Linux's aggregate softirq line: one total per vector, in the
    // kernel's vector order.
    out.push_str("softirq");
    for vec in 0..crate::interrupt::softirq::NR_SOFTIRQS {
        let mut total = 0u64;
        for cpu in 0..num_cpus {
            total = total.saturating_add(crate::interrupt::softirq::raised_count(cpu, vec));
        }
        out.push_str(&format!(" {}", total));
    }
    out.push('\n');

    out.into_bytes()
}
