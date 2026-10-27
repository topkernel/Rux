//! ext4/bio cost-anatomy counters (perf-audit line, cmdline `fsw=1`).
//!
//! Leg-18 F4 diagnosis: the ext4 metadata family measures 100-300x Linux
//! per-op (open 271 us, fsync 90 ms). This probe counts the VOLUMES under
//! those latencies — block-cache hits/misses, synchronous writes,
//! evictions, journal events, directory scans, allocator round trips,
//! big-lock acquisitions — so the cost centers can be ranked by
//! (counter delta) x (per-event cost implied by the syscall table),
//! instead of guessing from code reading.
//!
//! Discipline: same contract as dfx/syscost.rs — own cmdline key (`fsw=1`),
//! lazy-armed after cmdline init, one dump every FSW_DUMP_SECS from the
//! timer softirq, zero traffic when off, Relaxed atomics only (the counts
//! are statistics, not invariants).
//!
//! Dump format (stable, grep-able; two compact lines, emitted at :30 past
//! each minute to avoid the console-emit race with syscost's dump — pa28):
//! ```text
//! FSW1 t=123s br=<bread> rd=<dev reads> wr=<dev writes> sd=<sync_dirty>/<skip> ev=<evict>/<dirty> sa=<sync_all> pb=<pub flush>.<records> ps=<scoped>.<own records> pc=<constraint flushes> ph=<high-water drains> pm=<queue max> lvl=<console_loglevel> es=<console emits skipped>
//! FSW2 t=123s ri=<read_inode> wi=<write_inode> lk=<dir lookups>.<blocks> ls=<list_dir> ni=<new_inode> ar=<alloc runs> fr=<free runs>.<segments>.<blocks> j=<jstart>.<jstop>.<jlazy>.<jcommit_fast>.<jrevoke> bl=<biglock>.<contended> w=<walks>.<hits>.<misses>
//! FSW3 t=123s bs=<bitmap syncs> bsus=<bitmap sync us> sd=<sync_dirty> br=<bread>
//! ```
//!
//! `lvl`/`es` exist so the two pa28 probe-bug classes are separable from
//! the serial alone (leg 18 paid two boot iterations to tell them apart):
//! a vanished round with `es` climbing = emit-lock loss (this probe dumps
//! from the timer softirq, whose console-emit acquire budget is 100µs —
//! printing right behind syscost's ~70-line dump lost that race 11/12
//! rounds); a vanished round with `es` flat and `lvl` < 4 = the records
//! were dropped before the ring by the printk level filter (pa28-a: the
//! first build dumped via pr_info, dead at the pa-boot default loglevel
//! 4 — dump must stay at pr_warn or louder). Both fields are read once
//! per dump; no hot-path cost.

use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Dump cadence in seconds (same as syscost's 60 s).
const FSW_DUMP_SECS: u64 = 60;

static LAST_DUMP: AtomicU64 = AtomicU64::new(0);

// === bio layer ============================================================
pub static BREAD: AtomicU64 = AtomicU64::new(0);
/// Actual device reads (block-cache misses that went to disk).
pub static DEV_READ: AtomicU64 = AtomicU64::new(0);
/// Actual device writes from BufferHead::sync() (every synced 4K block:
/// inline write-through, eviction writeback, journal write-through, sync_all).
pub static DEV_WRITE: AtomicU64 = AtomicU64::new(0);
/// sync_dirty_buffer calls that performed (or attempted) a sync.
pub static SYNC_DIRTY: AtomicU64 = AtomicU64::new(0);
/// sync_dirty_buffer calls suppressed by an active journal handle.
pub static SYNC_DIRTY_SKIP: AtomicU64 = AtomicU64::new(0);
/// LRU evictions completed.
pub static EVICT: AtomicU64 = AtomicU64::new(0);
/// Evictions whose victim was dirty (paid a synchronous write).
pub static EVICT_DIRTY: AtomicU64 = AtomicU64::new(0);
/// sync_buffers() whole-cache drains.
pub static SYNC_ALL: AtomicU64 = AtomicU64::new(0);
/// flush_publications() drains that had work (records processed).
pub static PUB_FLUSH: AtomicU64 = AtomicU64::new(0);
pub static PUB_RECORDS: AtomicU64 = AtomicU64::new(0);
/// Scoped drains (flush_publications_scoped): the calling task's OWN
/// records published (leg-30A per-cookie fix; the world-drain latency
/// family — the 43-min hiview fdatasync class — moves off PUB_FLUSH onto
/// these).
pub static PUB_SCOPED: AtomicU64 = AtomicU64::new(0);
pub static PUB_SCOPED_RECORDS: AtomicU64 = AtomicU64::new(0);
/// Per-block constraint flushes (flush_publication_constraints) before an
/// out-of-band write of an order-constrained block (commit write-through,
/// inline sync, eviction).
pub static PUB_CSTR: AtomicU64 = AtomicU64::new(0);
/// High-water producer-paid drains (publication_defer past PUB_HIGH_WATER).
pub static PUB_HIWAT: AtomicU64 = AtomicU64::new(0);
/// Maximum pending-publication queue length observed.
pub static PUB_QMAX: AtomicU64 = AtomicU64::new(0);

// === ext4 =================================================================
pub static READ_INODE: AtomicU64 = AtomicU64::new(0);
pub static WRITE_INODE: AtomicU64 = AtomicU64::new(0);
/// ext4 directory lookups (linear scans over the directory's blocks).
pub static DIR_LOOKUP: AtomicU64 = AtomicU64::new(0);
/// Directory blocks scanned by those lookups.
pub static DIR_BLOCKS: AtomicU64 = AtomicU64::new(0);
/// list_dir calls (getdents64 re-materializes the whole directory each call).
pub static LIST_DIR: AtomicU64 = AtomicU64::new(0);
pub static NEW_INODE: AtomicU64 = AtomicU64::new(0);
/// Allocator runs (single block = run of 1): one bitmap read + one bitmap
/// sync write + one group-desc sync write + one SUPERBLOCK sync write each.
pub static ALLOC_RUN: AtomicU64 = AtomicU64::new(0);
pub static FREE_RUN: AtomicU64 = AtomicU64::new(0);
/// leg-37B family B (foundation ftruncate episodes): free-run GROUP
/// SEGMENTS — each is one read+RMW+sync bitmap round trip under
/// EXT4_BIG_LOCK (a multi-extent truncate pays one per extent). FREE_BLK
/// is the block volume behind them.
pub static FREE_SEG: AtomicU64 = AtomicU64::new(0);
pub static FREE_BLK: AtomicU64 = AtomicU64::new(0);
/// Block-bitmap sync writes (write_block_bitmap) and cumulative time
/// spent inside their sync_dirty_buffer waits, in MICROSECONDS (jiffies
/// at 100 Hz cannot resolve a single 2-5 ms sync; sched_clock ns → us).
pub static BITMAP_SYNC: AtomicU64 = AtomicU64::new(0);
pub static BITMAP_SYNC_US: AtomicU64 = AtomicU64::new(0);

// === jbd2 =================================================================
pub static J_START: AtomicU64 = AtomicU64::new(0);
pub static J_STOP: AtomicU64 = AtomicU64::new(0);
/// Stops that left the transaction open for batching (lazy commit).
pub static J_LAZY: AtomicU64 = AtomicU64::new(0);
/// Write-through fast-path commits (one sync write per registered buffer).
pub static J_COMMIT_FAST: AtomicU64 = AtomicU64::new(0);
pub static J_COMMIT_FAST_BUFS: AtomicU64 = AtomicU64::new(0);
/// Slow-path commits (descriptor + data + revoke + commit blocks).
pub static J_COMMIT_SLOW: AtomicU64 = AtomicU64::new(0);
pub static J_REVOKE: AtomicU64 = AtomicU64::new(0);

// === serialization =======================================================
/// EXT4_BIG_LOCK acquisitions.
pub static BIGLOCK: AtomicU64 = AtomicU64::new(0);
/// Acquisitions that fell past the adaptive spin (queued/spun for the lock).
pub static BIGLOCK_CONTEND: AtomicU64 = AtomicU64::new(0);

// === VFS walk ============================================================
/// path_lookup calls.
pub static WALK: AtomicU64 = AtomicU64::new(0);
/// Walk components resolved from the dentry tree.
pub static WALK_HIT: AtomicU64 = AtomicU64::new(0);
/// Walk components that needed a filesystem lookup (dcache miss).
pub static WALK_MISS: AtomicU64 = AtomicU64::new(0);

/// Probe armed? Lazy verdict after cmdline init (syscost's pa15 lesson:
/// never cache a verdict before the cmdline exists). Tri-state AtomicU8
/// exactly like syscost (0 = unresolved, 1 = off, 2 = on) plus a one-shot
/// resolution print so a dead probe is diagnosable from the serial.
static ARMED8: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn on() -> bool {
    match ARMED8.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            // Do NOT cache a verdict before the cmdline exists (pa15).
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => crate::cmdline::get_param("fsw").as_deref() == Some("1"),
            };
            ARMED8.store(if v { 2 } else { 1 }, Ordering::Relaxed);
            crate::pr_warn!(
                "fsw: {} (cmdline fsw param = {:?})",
                if v { "armed" } else { "off" },
                crate::cmdline::get_param("fsw")
            );
            v
        }
    }
}

macro_rules! bump {
    ($c:expr) => {
        if crate::dfx::fsw::on() {
            $c.fetch_add(1, Ordering::Relaxed);
        }
    };
    ($c:expr, $n:expr) => {
        if crate::dfx::fsw::on() {
            $c.fetch_add($n as u64, Ordering::Relaxed);
        }
    };
}

// bio layer notes
#[inline]
pub fn note_bread() {
    bump!(BREAD);
}
#[inline]
pub fn note_dev_read() {
    bump!(DEV_READ);
}
#[inline]
pub fn note_dev_write() {
    bump!(DEV_WRITE);
}
#[inline]
pub fn note_sync_dirty() {
    bump!(SYNC_DIRTY);
}
#[inline]
pub fn note_sync_dirty_skip() {
    bump!(SYNC_DIRTY_SKIP);
}
#[inline]
pub fn note_evict(dirty: bool) {
    if crate::dfx::fsw::on() {
        EVICT.fetch_add(1, Ordering::Relaxed);
        if dirty {
            EVICT_DIRTY.fetch_add(1, Ordering::Relaxed);
        }
    }
}
#[inline]
pub fn note_sync_all() {
    bump!(SYNC_ALL);
}
#[inline]
pub fn note_pub_flush(records: usize) {
    if crate::dfx::fsw::on() {
        PUB_FLUSH.fetch_add(1, Ordering::Relaxed);
        PUB_RECORDS.fetch_add(records as u64, Ordering::Relaxed);
    }
}
#[inline]
pub fn note_pub_scoped(records: usize) {
    if crate::dfx::fsw::on() {
        PUB_SCOPED.fetch_add(1, Ordering::Relaxed);
        PUB_SCOPED_RECORDS.fetch_add(records as u64, Ordering::Relaxed);
    }
}
#[inline]
pub fn note_pub_cstr() {
    bump!(PUB_CSTR);
}
#[inline]
pub fn note_pub_hiwat() {
    bump!(PUB_HIWAT);
}
#[inline]
pub fn note_pub_qmax(len: usize) {
    if crate::dfx::fsw::on() {
        // Relaxed max (statistics only).
        let mut cur = PUB_QMAX.load(Ordering::Relaxed);
        let l = len as u64;
        while l > cur {
            match PUB_QMAX.compare_exchange_weak(
                cur,
                l,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }
}

// ext4 layer notes
#[inline]
pub fn note_read_inode() {
    bump!(READ_INODE);
}
#[inline]
pub fn note_write_inode() {
    bump!(WRITE_INODE);
}
#[inline]
pub fn note_dir_lookup(blocks: usize) {
    if crate::dfx::fsw::on() {
        DIR_LOOKUP.fetch_add(1, Ordering::Relaxed);
        DIR_BLOCKS.fetch_add(blocks as u64, Ordering::Relaxed);
    }
}
#[inline]
pub fn note_list_dir() {
    bump!(LIST_DIR);
}
#[inline]
pub fn note_new_inode() {
    bump!(NEW_INODE);
}
#[inline]
pub fn note_alloc_run() {
    bump!(ALLOC_RUN);
}
#[inline]
pub fn note_free_run() {
    bump!(FREE_RUN);
}
#[inline]
pub fn note_free_seg(blocks: u64) {
    if crate::dfx::fsw::on() {
        FREE_SEG.fetch_add(1, Ordering::Relaxed);
        FREE_BLK.fetch_add(blocks, Ordering::Relaxed);
    }
}
#[inline]
pub fn note_bitmap_sync(ns: u64) {
    if crate::dfx::fsw::on() {
        BITMAP_SYNC.fetch_add(1, Ordering::Relaxed);
        BITMAP_SYNC_US.fetch_add(ns / 1000, Ordering::Relaxed);
    }
}

// jbd2 notes
#[inline]
pub fn note_j_start() {
    bump!(J_START);
}
#[inline]
pub fn note_j_stop() {
    bump!(J_STOP);
}
#[inline]
pub fn note_j_lazy() {
    bump!(J_LAZY);
}
#[inline]
pub fn note_j_commit_fast(bufs: usize) {
    if crate::dfx::fsw::on() {
        J_COMMIT_FAST.fetch_add(1, Ordering::Relaxed);
        J_COMMIT_FAST_BUFS.fetch_add(bufs as u64, Ordering::Relaxed);
    }
}
#[inline]
pub fn note_j_commit_slow() {
    bump!(J_COMMIT_SLOW);
}
#[inline]
pub fn note_j_revoke() {
    bump!(J_REVOKE);
}

// serialization notes
#[inline]
pub fn note_biglock() {
    bump!(BIGLOCK);
}
#[inline]
pub fn note_biglock_contend() {
    bump!(BIGLOCK_CONTEND);
}

// VFS walk notes
#[inline]
pub fn note_walk() {
    bump!(WALK);
}
#[inline]
pub fn note_walk_hit() {
    bump!(WALK_HIT);
}
#[inline]
pub fn note_walk_miss() {
    bump!(WALK_MISS);
}

/// Periodic dump from the timer softirq. Cadence is 60 s OFFSET BY half an
/// interval from syscost's dumps (pa28 lesson: my four lines immediately
/// after syscost's ~70-line dump lost the console-emit race every round
/// except once in 12 — one line survived at t=420; the counters themselves
/// were fine). Dumping at :30 past each minute keeps the emission window
/// free. Output is TWO compact lines (RECORD_TEXT_SIZE caps a record at
/// 256 bytes — keep each line short enough not to truncate).
pub fn tick(now_jiffies: u64) {
    if !on() {
        return;
    }
    let last = LAST_DUMP.load(Ordering::Relaxed);
    if last == 0 {
        // First call: park the gate half an interval ahead.
        LAST_DUMP.store(now_jiffies + FSW_DUMP_SECS * crate::config::KERNEL_HZ as u64 / 2, Ordering::Relaxed);
        return;
    }
    if now_jiffies.saturating_sub(last) < FSW_DUMP_SECS * crate::config::KERNEL_HZ as u64 {
        return;
    }
    LAST_DUMP.store(now_jiffies, Ordering::Relaxed);
    dump();
}

fn v(c: &AtomicU64) -> u64 {
    c.load(Ordering::Relaxed)
}

/// Print the counter snapshot (cumulative since boot).
pub fn dump() {
    let secs = crate::drivers::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
    crate::pr_warn!(
        "FSW1 t={}s br={} rd={} wr={} sd={}{} ev={}/{} sa={} pb={}.{} ps={}.{} pc={} ph={} pm={} lvl={} es={}",
        secs,
        v(&BREAD), v(&DEV_READ), v(&DEV_WRITE),
        v(&SYNC_DIRTY), v(&SYNC_DIRTY_SKIP),
        v(&EVICT), v(&EVICT_DIRTY), v(&SYNC_ALL),
        v(&PUB_FLUSH), v(&PUB_RECORDS),
        v(&PUB_SCOPED), v(&PUB_SCOPED_RECORDS),
        v(&PUB_CSTR), v(&PUB_HIWAT), v(&PUB_QMAX),
        crate::printk::get_console_loglevel(),
        crate::printk::console_emit_skipped(),
    );
    crate::pr_warn!(
        "FSW2 t={}s ri={} wi={} lk={}.{} ls={} ni={} ar={} fr={}.{}.{} j={}.{}.{}.{}.{} bl={}.{} w={}.{}.{}",
        secs,
        v(&READ_INODE), v(&WRITE_INODE),
        v(&DIR_LOOKUP), v(&DIR_BLOCKS), v(&LIST_DIR),
        v(&NEW_INODE), v(&ALLOC_RUN),
        v(&FREE_RUN), v(&FREE_SEG), v(&FREE_BLK),
        v(&J_START), v(&J_STOP), v(&J_LAZY),
        v(&J_COMMIT_FAST), v(&J_REVOKE),
        v(&BIGLOCK), v(&BIGLOCK_CONTEND),
        v(&WALK), v(&WALK_HIT), v(&WALK_MISS),
    );
    crate::pr_warn!(
        "FSW3 t={}s bs={} bsus={} sd={} br={}",
        secs,
        v(&BITMAP_SYNC), v(&BITMAP_SYNC_US),
        v(&SYNC_DIRTY), v(&BREAD),
    );
}

/// leg-37B family B: compact counter snapshot for the TXN-HANG audit —
/// one line per hung server txn per 5 s burst carries the free-volume and
/// bitmap-sync numbers AT the hang instants (the 60 s FSW dump cadence and
/// the 30 s periodic census both straddle the 1-100 s ftruncate episodes;
/// sc335B proved that unusable). Values are cumulative since boot; deltas
/// between consecutive bursts are the episode's volume. Counts read 0 when
/// fsw=1 is off (the notes are armed-gated) — printing them anyway costs
/// four Relaxed loads on an already-gated audit path.
pub fn hang_snapshot() -> (u64, u64, u64, u64, u64) {
    (
        FREE_RUN.load(Ordering::Relaxed),
        FREE_SEG.load(Ordering::Relaxed),
        FREE_BLK.load(Ordering::Relaxed),
        BITMAP_SYNC.load(Ordering::Relaxed),
        BITMAP_SYNC_US.load(Ordering::Relaxed),
    )
}
