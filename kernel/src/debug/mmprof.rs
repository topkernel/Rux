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
//! debug/switches.rs bit consumed). Cost when off: one relaxed load per
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
//!
//! CRATE-FLIP RUNG rux-obs: the mmprofd dump kthread and the rotating
//! MMPROF-A/B/C report lines moved to the rux-obs service crate
//! (kernel/rux-obs/src/mmprof.rs). THIS file keeps the per-op RECORD
//! half — now/add/bump/bucket_len/bump_comm and the statics they write
//! (mmap/mprotect/munmap paths call them directly) — the cadence gate
//! (tick), and the same-named wrappers (ensure_dump_thread,
//! report_line) that dispatch the hook rows. The statics are the pub
//! peek surface the crate's report reads.

#![forbid(unsafe_code)]
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

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

/// PUB peek surface (crate-flip rung rux-obs): the rux-obs report
/// crate's stat()/report_line read these. All-atomic slots; writes stay
/// in THIS module's record fns by construction.
pub struct PhaseStat {
    pub count: AtomicU64,
    pub cyc: AtomicU64,
    pub max: AtomicU64,
}

const EMPTY: PhaseStat = PhaseStat {
    count: AtomicU64::new(0),
    cyc: AtomicU64::new(0),
    max: AtomicU64::new(0),
};

pub static STATS: [PhaseStat; P_COUNT] = [EMPTY; P_COUNT];
pub static SCALARS: [AtomicU64; P_SCALARS] = [const { AtomicU64::new(0) }; P_SCALARS];
pub static BUCKETS: [PhaseStat; B_N] = [EMPTY; B_N];

/// PUB peek surface (crate-flip rung rux-obs; see PhaseStat).
pub struct CommEnt {
    pub comm: [AtomicU8; 16],
    pub n: AtomicU64,
}
const EMPTY_COMM: CommEnt = CommEnt {
    comm: [const { AtomicU8::new(0) }; 16],
    n: AtomicU64::new(0),
};
pub static COMMS: [CommEnt; COMM_SLOTS] = [EMPTY_COMM; COMM_SLOTS];

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
    let comm: [u8; 16] = *t.comm();
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

/// Timer-heartbeat hook (next to syscost::tick) — periodic dump. The
/// line emission lives in the rux-obs crate's mmprofd kthread; the
/// cadence gate stays here (hot tick site, one jiffies compare).
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

/// Spawn the task-context dump thread (moved to the rux-obs crate):
/// same-named wrapper for the tick cadence gate and debug::init;
/// dispatches the hook row (idempotent inside the crate body).
pub fn ensure_dump_thread() {
    crate::debug::obs_hooks::mmprof_ensure_dump_thread();
}

/// Emit ONE line of the rotating report (moved to the rux-obs crate):
/// same-named wrapper for the shutdown cascade's report_line(0) call;
/// dispatches the hook row.
pub fn report_line(k: u64) {
    crate::debug::obs_hooks::mmprof_report_line(k);
}
