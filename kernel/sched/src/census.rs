//! sched census and diagnostic counters (union-cold observability family).
//!
//! Shrink stage (user method 2026-11-26; whole-subsystem move, user order
//! 2026-11-27): moved from the core's sched/hotpath.rs — zero samples in the
//! p23-hotmap union (docs/development/core-hotmap.md) and no staying core
//! caller. crate-level forbid(unsafe_code) applies.

#![forbid(unsafe_code)]
use crate::config::MAX_CPUS;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use crate::arch;
use crate::config::{CFS_LATENCY_NS, CFS_MIN_GRANULARITY_NS, KERNEL_HZ, MAX_TASKS};
use crate::errno;
use crate::list::ListHead;
use crate::println;
use crate::process::{types::Pid, types::SchedPolicy, types::Task, types::TaskState};
use crate::sync::types::{RawSpinlock, Spinlock};
use crate::trace;

#[allow(unused_imports)]
use crate::sched::hotpath::{SCHED_CS_PHASE, kernel_text_window};
#[allow(unused_imports)]
use crate::sched::types::{TlPoisonCounters};
#[allow(unused_imports)]
use crate::sched::r#unsafe::{task_shared};

pub fn sched_hist_refuse(task: *mut Task, skip_reason: u32, cpu: usize) { rux::api::sched::sched_hist_refuse(task, skip_reason, cpu) }

pub fn sched_hist_link(task: *mut Task) { rux::api::sched::sched_hist_link(task) }

pub fn sched_hist_pick(task: *mut Task, cpu: usize) { rux::api::sched::sched_hist_pick(task, cpu) }

pub fn psledger_note_publish(task: *mut Task, cpu: usize, pk: u32) { rux::api::sched::psledger_note_publish(task, cpu, pk) }

pub fn sched_calls(cpu: usize) -> u64 {
    if cpu < MAX_CPUS {
        SCHED_CALLS[cpu].load(core::sync::atomic::Ordering::Relaxed)
    } else {
        0
    }
}

pub fn sched_cs_phase_set(cpu: usize, v: u32) {
    if cpu < MAX_CPUS {
        crate::sched::hotpath::SCHED_CS_PHASE[cpu].store(v, core::sync::atomic::Ordering::Release);
    }
}

pub(crate) fn orphan_watch_confirm(task: *mut Task, pid: u32, now: u64, threshold: u64) -> bool { rux::api::sched::orphan_watch_confirm(task, pid, now, threshold) }

pub(crate) fn orphan_watch_clear(task: *mut Task) { rux::api::sched::orphan_watch_clear(task) }

pub fn pick_giveups() -> (u64, u64) { rux::api::sched::pick_giveups() }

pub fn stallwatch_tick() { rux::api::sched::stallwatch_tick() }

pub(crate) fn g_diag_idle_bit(cpu: usize) -> bool { rux::api::sched::g_diag_idle_bit(cpu) }

pub(crate) fn cpu_tick_stale_ms(cpu: usize) -> u64 { rux::api::sched::cpu_tick_stale_ms(cpu) }

pub fn ww_cfs_geometry(task: *mut Task) -> (u64, u64, u64) { rux::api::sched::ww_cfs_geometry(task) }

pub fn ww_cfs_tie_mass(task: *mut Task) -> u64 { rux::api::sched::ww_cfs_tie_mass(task) }

pub fn ww_cfs_tie_mass_vr(vr: u64) -> u64 { rux::api::sched::ww_cfs_tie_mass_vr(vr) }

pub fn ww_tail_fill(ix: usize) -> u64 { rux::api::sched::ww_tail_fill(ix) }

pub fn ww_slot_mismatch_counters() -> [u64; MAX_CPUS] { rux::api::sched::ww_slot_mismatch_counters() }

pub fn ww_sched_snapshot() -> [u64; 10] { rux::api::sched::ww_sched_snapshot() }

pub fn ww_pick_walks() -> u64 {
    WW_PICK_WALKS.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn ww_pick_walk_inc() {
    WW_PICK_WALKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

pub fn ww_oncpu_heals() -> u64 {
    WW_ONCPU_HEALS.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn ww_claim_heals() -> u64 {
    WW_CLAIM_HEALS.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn ww_heals() -> (u64, u64) { rux::api::sched::ww_heals() }

pub fn ww_subgate_counts() -> (u64, u64, u64) { rux::api::sched::ww_subgate_counts() }

pub fn ww_rt_pick_stats() -> (u64, u64, u64) { rux::api::sched::ww_rt_pick_stats() }

fn harvest_print_parked(task: *mut Task, pid: u32) {
    use crate::debug::hotpath::putc_brief as putchar;
    static LAST: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    let now = crate::timer::get_jiffies();
    let last = LAST.load(core::sync::atomic::Ordering::Relaxed);
    if now.wrapping_sub(last) < 25 {
        return;
    }
    if LAST
        .compare_exchange(last, now, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    fn pdec(mut v: u64) {
        use crate::debug::hotpath::putc_brief as putchar;
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
    }
    fn phex(v: u64) {
        use crate::debug::hotpath::putc_brief as putchar;
        for sh in (0..16u32).rev() {
            let nb = ((v >> (sh * 4)) & 0xF) as u8;
            putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
        }
    }
    {
        const M: &[u8] = b"  ORPHAN-CTX pid=";
        for &b in M { putchar(b); }
        pdec(pid as u64);
        const O: &[u8] = b" o=";
        for &b in O { putchar(b); }
        putchar(b'0' + crate::sched::r#unsafe::task_shared(task).on_cpu() as u8);
        let claim = crate::sched::r#unsafe::task_shared(task).running_on_cpu.load(core::sync::atomic::Ordering::Relaxed);
        const C: &[u8] = b" cl=";
        for &b in C { putchar(b); }
        if (claim as i64) < 0 { putchar(b'-'); }
        pdec((claim as i64).unsigned_abs() as u64);
        const P: &[u8] = b" pk=";
        for &b in P { putchar(b); }
        pdec(crate::sched::r#unsafe::task_shared(task).pick_state() as u64);
        const T: &[u8] = b" ti_cpu=";
        for &b in T { putchar(b); }
        pdec(crate::sched::r#unsafe::task_shared(task).ti_cpu() as i64 as u64);
        let thsp = crate::sched::r#unsafe::task_shared(task).thread().sp;
        const K: &[u8] = b" ksp=0x";
        for &b in K { putchar(b); }
        phex(crate::sched::r#unsafe::task_shared(task).ti_kernel_sp());
        if (thsp >> 48) == 0xffff && (thsp & 7) == 0 && thsp > 0x1000 {
            const S: &[u8] = b" stk=0x";
            for &b in S { putchar(b); }
            phex(thsp);
            // Caller frames above the saved sp (stacks grow down).
            let base = thsp as *const u64;
            let mut shown = 0;
            for k in 0..64 {
                // SAFETY: diagnostic volatile read of the pinned task's own
                // kernel-stack pages.
                let w = crate::sync::r#unsafe::read_volatile_u64((base as usize) + 8 * k);
                let (lo, hi) = kernel_text_window();
                if w >= lo && w < hi && (w & 1) == 0 {
                    putchar(b' ');
                    phex(w);
                    shown += 1;
                    if shown >= 6 {
                        break;
                    }
                }
            }
        }
        putchar(b'\n');
    }
}

pub static TL_POISON: TlPoisonCounters = TlPoisonCounters {
    inserts: core::sync::atomic::AtomicU64::new(0),
    erases: core::sync::atomic::AtomicU64::new(0),
};

pub static SCHED_CALLS: [AtomicU64; MAX_CPUS] =
    [const { AtomicU64::new(0) }; MAX_CPUS];

pub static SCHEDSTATS: AtomicBool = AtomicBool::new(false);

pub static WW_PICK_WALKS: AtomicU64 = AtomicU64::new(0);

pub static WW_ONCPU_HEALS: AtomicU64 = AtomicU64::new(0);

pub static WW_CLAIM_HEALS: AtomicU64 = AtomicU64::new(0);
