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

// ---- scalar slots (bump-counted, reported raw) ----
pub const S_FIXED: usize = P_COUNT + 0; // MAP_FIXED mmaps
pub const S_HINT: usize = P_COUNT + 1; // hinted mmaps (addr != 0)
pub const S_ANON0: usize = P_COUNT + 2; // kernel-placed mmaps (addr == 0)
pub const S_UNPAGES: usize = P_COUNT + 3; // pages visited by unmap walks
pub const S_UNEMPTY: usize = P_COUNT + 4; // visited pages with no PTE
pub const S_MPPAGES: usize = P_COUNT + 5; // pages visited by mprotect walks
pub const S_VMAADD: usize = P_COUNT + 6; // VmaManager::add calls
pub const S_MUNLOOPS: usize = P_COUNT + 7; // munmap chunk iterations
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
#[inline]
pub fn add(p: usize, t0: u64) {
    let d = now().saturating_sub(t0);
    let s = &STATS[p];
    s.count.fetch_add(1, Ordering::Relaxed);
    s.cyc.fetch_add(d, Ordering::Relaxed);
    s.max.fetch_max(d, Ordering::Relaxed);
}

/// Scalar counter (pages walked, fixed mmaps, ...).
#[inline]
pub fn bump(p: usize, n: u64) {
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

/// Attribute one mmap to its issuer's comm (8-slot linear table).
#[inline]
pub fn bump_comm() {
    let Some(t) = crate::sched::current() else {
        return;
    };
    let comm: [u8; 16] = unsafe { *(*t).comm() };
    for slot in COMMS.iter() {
        let mut cur = [0u8; 16];
        for (i, b) in cur.iter_mut().enumerate() {
            *b = slot.comm[i].load(Ordering::Relaxed);
        }
        if cur == comm {
            slot.n.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if slot.n.load(Ordering::Relaxed) == 0 {
            // empty slot: claim it
            for (i, b) in comm.iter().enumerate() {
                slot.comm[i].store(*b, Ordering::Relaxed);
            }
            slot.n.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    // table full: fold into the last slot's count so the total still adds up
    COMMS[COMM_SLOTS - 1].n.fetch_add(1, Ordering::Relaxed);
}

const MMPROF_DUMP_SECS: u64 = 60;
static LAST_DUMP: AtomicU64 = AtomicU64::new(0);

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
    report();
}

fn fmt_stat(name: &str, c: u64, cyc: u64, max: u64, per_us: u64) {
    if c == 0 {
        if cyc != 0 {
            crate::println!("MMPROF {} scalar={}", name, cyc);
        }
        return;
    }
    let avg = cyc / c.max(1) / per_us;
    let mx = max / per_us;
    crate::println!("MMPROF {} n={} avg={}us max={}us", name, c, avg, mx);
}

/// Print the phase table. Called from the heartbeat tick and the
/// shutdown cascade (forkprof pattern).
pub fn report() {
    if !on() {
        return;
    }
    #[cfg(feature = "x86_64")]
    let hz = crate::drivers::intc::apic::TSC_FREQ.load(Ordering::Acquire);
    #[cfg(not(feature = "x86_64"))]
    let hz = 0; // cycle-domain report only on the other ports
    let per_us = if hz > 1_000_000 { hz / 1_000_000 } else { 1 };
    let secs = crate::drivers::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
    let names = [
        "m_total", "m_body", "m_lockwait", "m_findfree", "m_add", "m_fixedmun",
        "mu_body", "mu_surg", "mu_walk", "mu_tlb", "mu_yield",
        "mp_total", "mp_lock", "mp_cover", "mp_pte", "mp_tlb",
    ];
    crate::println!("MMPROF BEGIN t={}s hz={}", secs, hz);
    for (i, name) in names.iter().enumerate() {
        let s = &STATS[i];
        fmt_stat(name, s.count.load(Ordering::Relaxed), s.cyc.load(Ordering::Relaxed), s.max.load(Ordering::Relaxed), per_us);
    }
    let snames = [
        "s_fixed", "s_hint", "s_anon0", "s_unpages", "s_unempty", "s_mppages",
        "s_vmaadd", "s_munloops",
    ];
    for (i, name) in snames.iter().enumerate() {
        let n = SCALARS[i].load(Ordering::Relaxed);
        if n != 0 {
            crate::println!("MMPROF {} ={}", name, n);
        }
    }
    for (b, name) in B_NAMES.iter().enumerate() {
        let s = &BUCKETS[b];
        fmt_stat(name, s.count.load(Ordering::Relaxed), s.cyc.load(Ordering::Relaxed), s.max.load(Ordering::Relaxed), per_us);
    }
    // issuer attribution
    let mut line = alloc::string::String::from("MMPROF issuers:");
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
        line.push_str(&alloc::format!(" {}={}", name, n));
    }
    crate::println!("{}", line);
    crate::println!("MMPROF END");
}
