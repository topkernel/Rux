//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The mmprof REPORT half (moved from kernel/core/src/debug/mmprof.rs,
//! crate-flip rung debug): the mmprofd dump kthread and the rotating
//! MMPROF-A/B/C report lines. The add/bump/now/bucket_len/bump_comm
//! record markers, their STATS/SCALARS/BUCKETS/COMMS statics and the
//! cadence gate (tick) stay framework-side (mmap/mprotect/munmap
//! paths); this module reads the statics through their pub peek
//! surface. Bodies moved verbatim.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::debug::mmprof::{
    BUCKETS, COMMS, SCALARS, STATS, B_N, B_NAMES, M_ADD, M_FIXEDMUN, M_FINDFREE, M_LOCKWAIT,
    M_TOTAL, MP_COVER, MP_LOCK, MP_PTE, MP_TOTAL, MU_BODY, MU_SURG, MU_TLB, MU_WALK, MU_YIELD,
    S_ANON0, S_FIXED, S_HINT, S_MUNLOOPS, S_UNPAGES, S_UNEMPTY,
};

const MMPROF_DUMP_SECS: u64 = 60;

// ---- dump kthread ----
static DUMP_THREAD_ON: AtomicBool = AtomicBool::new(false);
static DUMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Spawn the task-context dump thread (once; called from the timer
/// softirq tick's cadence gate and from debug::init — either way the
/// thread starts once).
pub fn ensure_dump_thread() {
    if DUMP_THREAD_ON.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::process::hotpath::kthread_run(
        mmprof_dump_fn,
        core::ptr::null_mut(),
        "mmprofd",
    );
}

extern "C" fn mmprof_dump_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::types::TaskState;
    loop {
        if crate::process::hotpath::kthread_should_stop() {
            break;
        }
        // Sleep MMPROF_DUMP_SECS of jiffies (nanosleep discipline:
        // one-shot wakeup timer + INTERRUPTIBLE schedule).
        let current = match crate::sched::hotpath::current() {
            Some(t) => t,
            None => return 0, // no task context
        };
        // current() is the safe handle to the running task (we are it).
        let pid = current.pid();
        let wake_at = crate::timer::get_jiffies()
            + MMPROF_DUMP_SECS * crate::config::KERNEL_HZ as u64;
        let timer_id = crate::timer::add_timer_wakeup(wake_at, pid);
        current.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        // Re-arm IRQs so the timer tick reaches this CPU (R54).
        crate::arch::cpu::restore_irq(true);
        crate::sched::hotpath::schedule();
        if timer_id != 0 {
            crate::timer::del_timer(timer_id);
        }
        let k = DUMP_SEQ.fetch_add(1, Ordering::Relaxed) % 3;
        report_line(k);
    }
    0
}

/// Print ONE compact stat as `name=avg/max(n)` inside a caller's line.
fn fmt_cell(out: &mut alloc::string::String, name: &str, c: u64, cyc: u64, max: u64, per_us: u64) {
    if c == 0 {
        if cyc != 0 {
            out.push_str(&alloc::format!(" {}=s{}", name, cyc));
        }
        return;
    }
    let avg = cyc / c.max(1) / per_us;
    let mx = max / per_us;
    out.push_str(&alloc::format!(" {}={}/{}(n{})", name, avg, mx, c));
}

fn stat(i: usize) -> (u64, u64, u64) {
    let s = &STATS[i];
    (
        s.count.load(Ordering::Relaxed),
        s.cyc.load(Ordering::Relaxed),
        s.max.load(Ordering::Relaxed),
    )
}

/// Emit ONE line per 60s dump, rotating through three variants.
/// Console discipline: the printk emit-lock carries a 100us budget in
/// softirq context and the OH boot floods at ~700us per UART line, so a
/// multi-line burst loses all but 1-2 lines (lb332 evidence). One
/// short line per dump survives; the full table completes every 3 min.
pub fn report_line(k: u64) {
    if !crate::debug::mmprof::on() {
        return;
    }
    #[cfg(feature = "x86_64")]
    let hz = crate::drivers::hotpath::intc::apic::TSC_FREQ.load(Ordering::Acquire);
    #[cfg(not(feature = "x86_64"))]
    let hz = 0; // cycle-domain report only on the other ports
    let per_us = if hz > 1_000_000 { hz / 1_000_000 } else { 1 };
    let secs = crate::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
    match k {
        0 => {
            // A: mmap + mprotect stages
            let mut l = alloc::format!("MMPROF-A t={}s mmap:", secs);
            let (c, cy, mx) = stat(M_TOTAL);
            fmt_cell(&mut l, "tot", c, cy, mx, per_us);
            let (c, cy, mx) = stat(M_LOCKWAIT);
            fmt_cell(&mut l, "lock", c, cy, mx, per_us);
            let (c, cy, mx) = stat(M_FINDFREE);
            fmt_cell(&mut l, "find", c, cy, mx, per_us);
            let (c, cy, mx) = stat(M_ADD);
            fmt_cell(&mut l, "add", c, cy, mx, per_us);
            let (c, cy, mx) = stat(M_FIXEDMUN);
            fmt_cell(&mut l, "fixmun", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MP_TOTAL);
            fmt_cell(&mut l, "| mprot-tot", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MP_LOCK);
            fmt_cell(&mut l, "mplock", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MP_COVER);
            fmt_cell(&mut l, "mpcov", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MP_PTE);
            fmt_cell(&mut l, "mppte", c, cy, mx, per_us);
            crate::pr_warn!("{}", l);
        }
        1 => {
            // B: munmap stages + placement scalars
            let mut l = alloc::format!("MMPROF-B t={}s munmap:", secs);
            let (c, cy, mx) = stat(MU_BODY);
            fmt_cell(&mut l, "body", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MU_SURG);
            fmt_cell(&mut l, "surg", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MU_WALK);
            fmt_cell(&mut l, "walk", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MU_TLB);
            fmt_cell(&mut l, "tlb", c, cy, mx, per_us);
            let (c, cy, mx) = stat(MU_YIELD);
            fmt_cell(&mut l, "yld", c, cy, mx, per_us);
            crate::pr_warn!(
                "{} | fixed={} hint={} anon0={} unpg={} unempty={} munloops={}",
                l,
                SCALARS[S_FIXED].load(Ordering::Relaxed),
                SCALARS[S_HINT].load(Ordering::Relaxed),
                SCALARS[S_ANON0].load(Ordering::Relaxed),
                SCALARS[S_UNPAGES].load(Ordering::Relaxed),
                SCALARS[S_UNEMPTY].load(Ordering::Relaxed),
                SCALARS[S_MUNLOOPS].load(Ordering::Relaxed)
            );
        }
        _ => {
            // C: length buckets + issuers
            let mut l = alloc::format!("MMPROF-C t={}s", secs);
            for b in 0..B_N {
                let s = &BUCKETS[b];
                fmt_cell(&mut l, B_NAMES[b], s.count.load(Ordering::Relaxed), s.cyc.load(Ordering::Relaxed), s.max.load(Ordering::Relaxed), per_us);
            }
            l.push_str(" | iss:");
            for slot in COMMS.iter() {
                let n = slot.n.load(Ordering::Relaxed);
                if n == 0 {
                    continue;
                }
                let mut comm = [0u8; 16];
                for (i, b) in comm.iter_mut().enumerate() {
                    *b = slot.comm[i].load(Ordering::Relaxed);
                }
                let end = comm.iter().position(|&c| c == 0).unwrap_or(16);
                let name = core::str::from_utf8(&comm[..end]).unwrap_or("?");
                l.push_str(&alloc::format!(" {}={}", name, n));
            }
            crate::pr_warn!("{}", l);
        }
    }
}
