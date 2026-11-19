//! hp89 hunt: post-hoc attribution ring for every PTE-clearing event.
//!
//! The mallocng-corruption signature (hp88m/hp89a/hp89c: healthy victim
//! PTEs, singly-referenced non-ring anon frames, ZEROED data, no mm
//! syscall of the victim tgid covering the VA) needs SOMEONE to run the
//! full unmap sequence (rmap-remove + put_page + PTE clear) against a
//! live mapping — or a frame free+re-sale the descriptors cannot see.
//! The syscall-driven paths are exonerated by the per-task mm_ops ring
//! (GP-MMH prints); what remains are the non-syscall PTE clearers:
//! `unmap_pages` (munmap/brk-shrink/mremap/madvise engine — the mm and
//! range are known even when the caller is not a syscall of the victim
//! tgid), the dying-mm teardown (`free_user_page_tables` — the fork
//! child exit face), and the rmap walkers (`try_to_unmap*` — the
//! reclaim/migration face).
//!
//! This ring records every such event (kind, pid, key addresses, leaf
//! count) into a small fixed array under one spinlock — NO printk on
//! the record path (serial spam provably closes the race being hunted,
//! hp88f/i). The #GP forensics (trap.rs gp_mmhistory_dump) prints the
//! tail at fault time; correlating the victim VAs/PFNs against the
//! entries names the clearing path in one corrupting boot.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Event kinds.
pub const KIND_UNMAP_PAGES: u8 = 1;
pub const KIND_MM_TEARDOWN: u8 = 2;
pub const KIND_TRY_TO_UNMAP: u8 = 3;
pub const KIND_LEAF_REPLACE: u8 = 4;

const RING_LEN: usize = 256;

/// One event: plain atomics (the Task::mm_ops shape) — a concurrent
/// reader may see a partially-updated event under contention, which is
/// acceptable for forensics (LOCK still serializes writers).
struct UnEvent {
    /// KIND_* (0 = never written).
    kind: AtomicU64,
    /// Acting pid.
    pid: AtomicU64,
    /// Kind-specific: start VA (1), root ppn (2), target pfn (3/4).
    a: AtomicU64,
    /// End VA (1), count (2/3), new_bits (4).
    b: AtomicU64,
    /// Leaves cleared / PTEs replaced.
    cnt: AtomicU64,
    /// jiffies at record time.
    j: AtomicU64,
}

static RING: [UnEvent; RING_LEN] = [const {
    UnEvent {
        kind: AtomicU64::new(0),
        pid: AtomicU64::new(0),
        a: AtomicU64::new(0),
        b: AtomicU64::new(0),
        cnt: AtomicU64::new(0),
        j: AtomicU64::new(0),
    }
}; RING_LEN];

static CURSOR: AtomicUsize = AtomicUsize::new(0);
static LOCK: crate::sync::spinlock::Spinlock<()> = crate::sync::spinlock::Spinlock::new(());

/// Record one PTE-clearing event. Cheap (one short spinlock section);
/// never prints.
pub fn note(kind: u8, a: u64, b: u64, cnt: u64) {
    let pid = crate::process::current_pid() as u64;
    let j = crate::drivers::timer::get_jiffies();
    let idx = CURSOR.fetch_add(1, Ordering::Relaxed) % RING_LEN;
    let _g = LOCK.lock_irqsave();
    let e = &RING[idx];
    e.kind.store(kind as u64, Ordering::Relaxed);
    e.pid.store(pid, Ordering::Relaxed);
    e.a.store(a, Ordering::Relaxed);
    e.b.store(b, Ordering::Relaxed);
    e.cnt.store(cnt, Ordering::Relaxed);
    e.j.store(j, Ordering::Relaxed);
}

/// Dump the ring's tail (newest first), bounded — called from #GP
/// forensics only (post-hoc; the hunt window has already closed).
pub fn dump_tail(n: usize) {
    let cur = CURSOR.load(Ordering::Relaxed);
    let total = cur.min(RING_LEN);
    let n = n.min(total);
    let _g = LOCK.lock_irqsave();
    for k in 0..n {
        let idx = (cur + RING_LEN - 1 - k) % RING_LEN;
        let e = &RING[idx];
        let kind = e.kind.load(Ordering::Relaxed);
        if kind == 0 {
            continue;
        }
        let name = match kind {
            1 => "unmap_pages",
            2 => "mm_teardown",
            3 => "try_to_unmap",
            4 => "leaf_replace",
            _ => "?",
        };
        crate::pr_err!(
            "UNWATCH: #{:<4} {} pid={} a={:#x} b={:#x} cnt={} j={}",
            cur - k,
            name,
            e.pid.load(Ordering::Relaxed),
            e.a.load(Ordering::Relaxed),
            e.b.load(Ordering::Relaxed),
            e.cnt.load(Ordering::Relaxed),
            e.j.load(Ordering::Relaxed)
        );
    }
}

/// Whether any recorded unmap_pages event's [a, b) range covers `va`,
/// and if so PRINT up to `max_show` of the covering events (kind, pid,
/// range, jiffies — the attribution the #GP forensics needs). Called
/// from #GP forensics for each victim pointer.
pub fn covered_by_unmap(va: u64) -> bool {
    let cur = CURSOR.load(Ordering::Relaxed);
    let total = cur.min(RING_LEN);
    let mut shown = 0;
    let mut hit = false;
    let _g = LOCK.lock_irqsave();
    for k in 0..total {
        let idx = (cur + RING_LEN - k) % RING_LEN;
        let e = &RING[idx];
        if e.kind.load(Ordering::Relaxed) == KIND_UNMAP_PAGES as u64
            && va >= e.a.load(Ordering::Relaxed)
            && va < e.b.load(Ordering::Relaxed)
        {
            hit = true;
            if shown < 4 {
                crate::pr_err!(
                    "UNWATCH-COVER: va={:#x} event #{} unmap_pages pid={} a={:#x} b={:#x} cnt={} j={} (age {}/100 s)",
                    va,
                    cur - k,
                    e.pid.load(Ordering::Relaxed),
                    e.a.load(Ordering::Relaxed),
                    e.b.load(Ordering::Relaxed),
                    e.cnt.load(Ordering::Relaxed),
                    e.j.load(Ordering::Relaxed),
                    crate::drivers::timer::get_jiffies().saturating_sub(e.j.load(Ordering::Relaxed))
                );
                shown += 1;
            }
        }
    }
    hit
}
