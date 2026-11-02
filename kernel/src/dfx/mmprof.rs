//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! mmap/mprotect stage profiler — attribution probe for the OH vm-op wall
//! (syscost handoff: nr=9 mmap n=179k avg 72us max 722ms, nr=10 mprotect
//! avg 53us, 50-70x the reference kernel's 1-5us class; distributeddata's
//! 27-32s SA init rides this family).
//!
//! Armed by the kernel cmdline `mmprof=1` (own key, forkprof pattern; no
//! dfx/switches.rs bit consumed). Cost when off: one relaxed load per
//! hook. Cost when on: ~6 rdtsc+fetch_add brackets per mmap/mprotect —
//! noise against a 72us syscall.
//!
//! What it measures (the named candidate set from the perf handoff):
//! - mmap: vma_write acquire wait, find_free_area search, VmaManager::add,
//!   the MAP_FIXED replacement path (chunked munmap: surgery, per-page
//!   walk, TLB flush tail, cond_resched yields), length buckets,
//!   fixed/hint/anon placement counters, per-comm issuer attribution.
//! - mprotect: vma_write acquire wait, coverage+EACCES walk, PTE rewrite
//!   walk, per-page count.
//! - munmap (both callers): body, surgery, walk, TLB tail, yields.
//!
//! Dump: periodic (timer heartbeat, every MMPROF_DUMP_SECS) + the
//! shutdown cascade. Lines: MMPROF <name> n= avg=us max=us.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

// ---- phase slots ----
pub const M_TOTAL: usize = 0; // sys_mmap_inner whole body
pub const M_BODY: usize = 1; // MmStruct::mmap body (x86_64)
pub const M_LOCKWAIT: usize = 2; // vma_write() acquire wait (mmap paths)
pub const M_FINDFREE: usize = 3; // find_free_area_locked search
pub const M_ADD: usize = 4; // VmaManager::add + vm accounting
pub const M_FIXEDMUN: usize = 5; // MAP_FIXED's self.munmap + install loop
pub const MU_BODY: usize = 6; // munmap body (both callers)
pub const MU_SURG: usize = 7; // per-chunk VMA surgery (release+split+re-add)
pub const MU_WALK: usize = 8; // unmap_pages body
pub const MU_TLB: usize = 9; // unmap_pages flush tail (IPI + full flush)
pub const MU_YIELD: usize = 10; // cond_resched() calls in the munmap loop
pub const MP_TOTAL: usize = 11; // sys_mprotect_inner whole body
pub const MP_LOCK: usize = 12; // mprotect vma_write acquire wait
pub const MP_COVER: usize = 13; // coverage walk + EACCES scan
pub const MP_PTE: usize = 14; // mprotect_pte_range (per-page rewrite)
pub const MP_TLB: usize = 15; // post-rewrite flush (riscv sfence arm)
pub const P_COUNT: usize = 16;

// ---- scalar slots (bump-counted, reported raw; separate namespace
// from the phase slots — bump() indexes SCALARS[0..P_SCALARS]) ----
pub const S_FIXED: usize = 0; // MAP_FIXED mmaps
pub const S_HINT: usize = 1; // hinted mmaps (addr != 0)
pub const S_ANON0: usize = 2; // kernel-placed mmaps (addr == 0)
pub const S_UNPAGES: usize = 3; // pages visited by unmap walks
pub const S_UNEMPTY: usize = 4; // visited pages with no PTE
pub const S_MPPAGES: usize = 5; // pages visited by mprotect walks
pub const S_VMAADD: usize = 6; // VmaManager::add calls
pub const S_MUNLOOPS: usize = 7; // munmap chunk iterations
pub const P_SCALARS: usize = 8;

// ---- length buckets for successful mmaps (count + cycles) ----
pub const B_LE64K: usize = 0;
pub const B_LE1M: usize = 1;
pub const B_LE16M: usize = 2;
pub const B_GT16M: usize = 3;
pub const B_N: usize = 4;
pub const B_NAMES: [&str; B_N] = ["len<=64K", "len<=1M", "len<=16M", "len>16M"];

// ---- per-comm issuer attribution (mmap count) ----
pub const COMM_SLOTS: usize = 8;

struct PhaseStat {
    count: AtomicU64,
    cyc: AtomicU64,
    max: AtomicU64,
}

const EMPTY: PhaseStat = PhaseStat {
    count: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
};

static STATS: [PhaseStat; P_COUNT] = [EMPTY; P_COUNT];
static SCALARS: [AtomicU64; P_SCALARS] = [const { AtomicU64::new(0) }; P_SCALARS];
static BUCKETS: [PhaseStat; B_N] = [EMPTY; B_N];

struct CommEnt {
    comm: [AtomicU8; 16],
    n: AtomicU64,
}
const EMPTY_COMM: CommEnt = CommEnt {
    comm: [const { AtomicU8::new(0) }; 16],
    n: AtomicU64::new(0),
};
static COMMS: [CommEnt; COMM_SLOTS] = [EMPTY_COMM; COMM_SLOTS];

static ARMED: AtomicU8 = AtomicU8::new(0);

/// Cached arm check (forkprof::on pattern; never cache before cmdline).
#[inline]
pub fn on() -> bool {
    match ARMED.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = match crate::cmdline::get_param("mmprof").as_deref() {
                Some("1") => 2,
                _ => 1,
            };
            ARMED.store(v, Ordering::Relaxed);
            if v == 2 {
                crate::pr_warn!("MMPROF armed (cmdline mmprof=1)");
            }
            v == 2
        }
    }
}

/// Cycle-counter read (raw TSC / cycle domain; the report converts).
#[inline]
pub fn now() -> u64 {
    crate::arch::cpu::read_counter()
}

/// Account `elapsed = now() - t0` cycles against phase `p`.
/// Out-of-range slots are dropped, never a panic (diagnostic discipline).
#[inline]
pub fn add(p: usize, t0: u64) {
    if p >= P_COUNT {
        return;
    }
    let d = now().saturating_sub(t0);
    let s = &STATS[p];
    s.count.fetch_add(1, Ordering::Relaxed);
    s.cyc.fetch_add(d, Ordering::Relaxed);
    s.max.fetch_max(d, Ordering::Relaxed);
}

/// Scalar counter (pages walked, fixed mmaps, ...).
#[inline]
pub fn bump(p: usize, n: u64) {
    if p >= P_SCALARS {
        return;
    }
    SCALARS[p].fetch_add(n, Ordering::Relaxed);
}

/// Account a successful mmap's cycles into its length bucket.
#[inline]
pub fn bucket_len(total: usize, t0: u64) {
    let b = if total <= 64 * 1024 {
        B_LE64K
    } else if total <= 1024 * 1024 {
        B_LE1M
    } else if total <= 16 * 1024 * 1024 {
        B_LE16M
    } else {
        B_GT16M
    };
    let d = now().saturating_sub(t0);
    BUCKETS[b].count.fetch_add(1, Ordering::Relaxed);
    BUCKETS[b].cyc.fetch_add(d, Ordering::Relaxed);
    BUCKETS[b].max.fetch_max(d, Ordering::Relaxed);
}

/// Attribute one mmap to its issuer's comm (8-slot linear table,
/// min-count eviction — the first-come table saturated with boot-time
/// one-shot processes and never named the churner; the churner's count
/// dwarfs theirs within a minute, so evicting the smallest slot
/// converges on it).
#[inline]
pub fn bump_comm() {
    let Some(t) = crate::sched::current() else {
        return;
    };
    let comm: [u8; 16] = unsafe { *(*t).comm() };
    // fast path: present slot
    for slot in COMMS.iter() {
        let mut cur = [0u8; 16];
        for (i, b) in cur.iter_mut().enumerate() {
            *b = slot.comm[i].load(Ordering::Relaxed);
        }
        if cur == comm && slot.n.load(Ordering::Relaxed) != 0 {
            slot.n.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    // slow path: claim an empty slot, else evict the smallest count
    let mut victim = 0usize;
    let mut victim_n = u64::MAX;
    for (i, slot) in COMMS.iter().enumerate() {
        let n = slot.n.load(Ordering::Relaxed);
        if n == 0 {
            victim = i;
            victim_n = 0;
            break;
        }
        if n < victim_n {
            victim = i;
            victim_n = n;
        }
    }
    let slot = &COMMS[victim];
    if victim_n == 0 {
        for (i, b) in comm.iter().enumerate() {
            slot.comm[i].store(*b, Ordering::Relaxed);
        }
        slot.n.store(1, Ordering::Relaxed);
        return;
    }
    // evict: compare_exchange the count so a concurrent claimant cannot
    // resurrect the evicted comm under us
    if slot
        .n
        .compare_exchange(victim_n, 1, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        for (i, b) in comm.iter().enumerate() {
            slot.comm[i].store(*b, Ordering::Relaxed);
        }
    } else {
        slot.n.fetch_add(1, Ordering::Relaxed);
    }
}

const MMPROF_DUMP_SECS: u64 = 60;
static LAST_DUMP: AtomicU64 = AtomicU64::new(0);
static DUMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Timer-heartbeat hook (next to syscost::tick) — periodic dump.
pub fn tick(now_jiffies: u64) {
    if !on() {
        return;
    }
    if now_jiffies.saturating_sub(LAST_DUMP.load(Ordering::Relaxed))
        < MMPROF_DUMP_SECS * crate::config::KERNEL_HZ as u64
    {
        return;
    }
    LAST_DUMP.store(now_jiffies, Ordering::Relaxed);
    // Line emission is deferred to the dump kthread: a softirq emitter
    // carries a 100us emit-lock budget while the OH hilog flood holds the
    // console ~17ms per 256B line (emulated baud) — the lb336 t=60s dump
    // never reached the serial. Task context waits up to 2s and wins.
    ensure_dump_thread();
}

// ---- dump kthread ----
static DUMP_THREAD_ON: AtomicBool = AtomicBool::new(false);

/// Spawn the task-context dump thread (once; called from the timer
/// softirq tick and from dfx::init — either way the thread starts once).
pub fn ensure_dump_thread() {
    if DUMP_THREAD_ON.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::process::kthread::kthread_run(
        mmprof_dump_fn,
        core::ptr::null_mut(),
        "mmprofd",
    );
}

extern "C" fn mmprof_dump_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::task::{Task, TaskState};
    loop {
        if crate::process::kthread::kthread_should_stop() {
            break;
        }
        // Sleep MMPROF_DUMP_SECS of jiffies (nanosleep discipline:
        // one-shot wakeup timer + INTERRUPTIBLE schedule).
        let current = match crate::sched::current() {
            Some(t) => t as *mut Task,
            None => return 0, // no task context
        };
        // SAFETY: current is the running task's pointer (we are it).
        let pid = unsafe { (*current).pid() };
        let wake_at = crate::drivers::timer::get_jiffies()
            + MMPROF_DUMP_SECS * crate::config::KERNEL_HZ as u64;
        let timer_id = crate::timer::add_timer_wakeup(wake_at, pid);
        // SAFETY: setting our own task's state.
        unsafe {
            (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }
        // Re-arm IRQs so the timer tick reaches this CPU (R54).
        crate::arch::cpu::restore_irq(true);
        crate::sched::schedule();
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
    if !on() {
        return;
    }
    #[cfg(feature = "x86_64")]
    let hz = crate::drivers::intc::apic::TSC_FREQ.load(Ordering::Acquire);
    #[cfg(not(feature = "x86_64"))]
    let hz = 0; // cycle-domain report only on the other ports
    let per_us = if hz > 1_000_000 { hz / 1_000_000 } else { 1 };
    let secs = crate::drivers::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
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
