// The crate-level forbid at fs/src/lib.rs is the boundary; this
// file-level attribute satisfies the forbid-registry ratchet.
#![forbid(unsafe_code)]

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

use crate::process::types::TaskState;

/// One CPU's time accounting, in jiffies (Linux /proc/stat columns).
///
/// `nice`, `iowait`, `irq`, `softirq`, `steal`, `guest` and
/// `guest_nice` exist because Linux's file has those columns; Rux has
/// no nice levels, no iowait accounting and no IRQ-time accounting
/// (CONFIG_IRQ_TIME_ACCOUNTING is off on many Linux boxes too), so
/// they report zero rather than a fabricated number.
// The CPU-time cells, tick charging and the context-switch/fork notes
// moved framework-side (fs::perf_ctr — the fs-substrate devirtualization:
// arch traps and the scheduler call them directly, zero indirection);
// the readers below delegate to the framework cells.
pub use rux::fs::perf_ctr::{CpuTime, CPU_TIME, CTXT_SWITCHES, FORKS};
pub use rux::fs::perf_ctr::{charge_tick, note_switch, note_fork};

pub fn total_context_switches() -> u64 {
    CTXT_SWITCHES.load(Ordering::Relaxed)
}

/// Total forks since boot.
pub fn total_forks() -> u64 {
    FORKS.load(Ordering::Relaxed)
}

/// Number of runnable tasks (Linux "procs_running").
fn procs_running() -> u64 {
    crate::sched::hotpath::grq_diag_counts().0 as u64
}

/// Number of tasks blocked on I/O (Linux "procs_blocked": state D).
fn procs_blocked() -> u64 {
    let mut blocked = 0u64;
    // Non-blocking walk: a busy bucket is skipped rather than waited
    // for, so reading /proc/stat never blocks the caller.
    crate::process::hotpath::pid_hash_for_each_task_try(|task| {
        // SAFETY: the pid-hash walk hands out live task pointers.
        let t = task;
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
    crate::timer::wall_epoch_offset_ns() / 1_000_000_000
}

/// Generate /proc/stat content (Linux layout).
///
/// leg-36: the fixed-shape cpu rows are derived by `procfile!`'s
/// `cols` form (rust-design-headroom §4, track-1 third conversion) —
/// the column list, the row function's argument order and the
/// rendered byte order are one declaration (`stat_cpu_row`,
/// `STAT_CPU_COLUMNS`, shape-tested). The two DYNAMIC-arity lines
/// (`intr`, `softirq` — sums over live IRQ/vector tables) and the
/// kv tail (`render_kv`, keys listed once, in order) stay explicit:
/// they have no fixed field list to derive.
pub fn generate() -> Vec<u8> {
    let mut out = String::new();
    let num_cpus = crate::arch::smp::num_started_cpus().min(crate::config::MAX_CPUS);

    // Aggregate line: the sum over all CPUs. Linux prints the label
    // as "cpu  " (two spaces) so the columns align under the cpu0
    // rows — the trailing space belongs to the label passed here.
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
    out.push_str(&stat_cpu_row(
        "cpu ",
        user, nice, system, idle, iowait, irq, softirq, steal, guest, guest_nice,
    ));

    // Per-CPU lines.
    for cpu in 0..num_cpus {
        out.push_str(&stat_cpu_row(
            &format!("cpu{}", cpu),
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
    // kv tail: Linux order ctxt/btime/processes/procs_running/
    // procs_blocked (fs/proc/stat.c show_stat); keys and order are one
    // list here — same drift floor as the macro's kv form.
    out.push_str(&super::procfile::render_kv(&[
        ("ctxt", &total_context_switches()),
        ("btime", &btime()),
        ("processes", &total_forks()),
        ("procs_running", &procs_running()),
        ("procs_blocked", &procs_blocked()),
    ]));
    // Linux's aggregate softirq line: one total per vector, in the
    // kernel's vector order. Dynamic arity (NR_SOFTIRQS sums), so it
    // stays explicit — see the generate() doc note.
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

crate::procfile! {
    /// /proc/stat cpu rows — fixed 10-column jiffy lines (Linux
    /// fs/proc/stat.c show_stat order; user..guest_nice). The argument
    /// order IS the column order; the aggregate row passes `"cpu "` as
    /// the label (Linux's two-space alignment, see generate()).
    rowfn stat_cpu_row(label: &str) columns STAT_CPU_COLUMNS test stat_row_shape
        -> alloc::string::String cols {
        "user"        => user,
        "nice"        => nice,
        "system"      => system,
        "idle"        => idle,
        "iowait"      => iowait,
        "irq"         => irq,
        "softirq"     => softirq,
        "steal"       => steal,
        "guest"       => guest,
        "guest_nice"  => guest_nice,
    }
}

