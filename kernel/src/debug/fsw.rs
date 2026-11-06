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
//! Discipline: same contract as debug/syscost.rs — own cmdline key (`fsw=1`),
//! lazy-armed after cmdline init, one dump every FSW_DUMP_SECS from the
//! timer softirq, zero traffic when off, Relaxed atomics only (the counts
//! are statistics, not invariants).
//!
//! Dump format (stable, grep-able; two compact lines, emitted at :30 past
//! each minute to avoid the console-emit race with syscost's dump — pa28):
//! ```text
//! FSW1 t=123s br=<bread> rd=<dev reads> wr=<dev writes> sd=<sync_dirty>/<skip> ev=<evict>/<dirty> sa=<sync_all> pb=<pub flush>.<records> ps=<scoped>.<own records> pc=<constraint flushes> ph=<high-water drains> pb2=<batch drains> pm=<queue max> lvl=<console_loglevel> es=<console emits skipped>
//! FSW2 t=123s ri=<read_inode> wi=<write_inode> lk=<dir lookups>.<blocks> ls=<list_dir> ni=<new_inode> ar=<alloc runs> fr=<free runs> j=<jstart>.<jstop>.<jlazy>.<jcommit_fast>.<jrevoke> bl=<biglock>.<contended> w=<walks>.<hits>.<misses>
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

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

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
/// Leg-55: mkdirs counted at the scoped-flush point (the lazy-variant
/// follow-up reads this against the phase mix).
pub static PUB_LAZY: AtomicU64 = AtomicU64::new(0);
/// Op-boundary batched drains (publication_batch_drain past PUB_BATCH) —
/// the amortized creator-paid drain that keeps the queue short now that
/// per-mkdir scoped drains are gone.
pub static PUB_BATCH_DRAINS: AtomicU64 = AtomicU64::new(0);
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

// === op-cost anatomy (perf-fs lane 2026-10-31) ============================
/// Whole-bracket ext4_mkdir duration (ENTRY_MUTEX guard to journal stop),
/// microseconds: count / cumulative / max. The residual p6_mkdrmdir floor
/// (~2.5ms/pair under load with zero steady-state device I/O) needs an
/// in-op cost attribution; this bracket timer plus the registration-scan
/// counter below bracket the two suspects (CPU in the op vs jbd2 dedup
/// scans that grow with the running transaction's unique-block set).
pub static MKDIR_US_N: AtomicU64 = AtomicU64::new(0);
pub static MKDIR_US_SUM: AtomicU64 = AtomicU64::new(0);
pub static MKDIR_US_MAX: AtomicU64 = AtomicU64::new(0);
/// Same bracket timer for ext4_rmdir / ext4_unlink (us).
pub static RMDIR_US_SUM: AtomicU64 = AtomicU64::new(0);
pub static RMDIR_US_MAX: AtomicU64 = AtomicU64::new(0);
pub static UNLINK_US_SUM: AtomicU64 = AtomicU64::new(0);
pub static UNLINK_US_MAX: AtomicU64 = AtomicU64::new(0);
/// jbd2_journal_get_write_access dedup-scan iterations (the linear
/// already-tracked walk over t_dirty_buffers): grows O(set) per metadata
/// registration between commits.
pub static J_DEDUP_ITERS: AtomicU64 = AtomicU64::new(0);

// === VFS walk ============================================================
/// path_lookup calls.
pub static WALK: AtomicU64 = AtomicU64::new(0);
/// Walk components resolved from the dentry tree.
pub static WALK_HIT: AtomicU64 = AtomicU64::new(0);
/// Walk components that needed a filesystem lookup (dcache miss).
pub static WALK_MISS: AtomicU64 = AtomicU64::new(0);

// === path-walk cost anatomy (perf-path lane 2026-11-01) ==================
// Sub-microsecond phases: NS sums (converted at dump), one count each
// where the divisor is not implied by the counters above. The brackets
// live in sys_fstatat / sys_openat / path_lookup; the question they
// answer: of the measured stat/open latency, how much is (a) the syscall
// prelude (user copies, resolve_user_path, shortcut gates), (b) the walk
// prologue (root fetch, normalize, component split), (c) the per-
// component loop, and inside it (d) get_inode+permission, (e) the dentry
// child-map lookup, (f) the per-hit icache refresh, (g) mount+symlink
// follow, versus (h) getattr+copyout / File+fdtable for the tail.
pub static STAT_N: AtomicU64 = AtomicU64::new(0);
pub static STAT_PRE_NS: AtomicU64 = AtomicU64::new(0);
pub static STAT_TAIL_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_N: AtomicU64 = AtomicU64::new(0);
pub static OPEN_PRE_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_TAIL_NS: AtomicU64 = AtomicU64::new(0);
pub static WALK_PRE_NS: AtomicU64 = AtomicU64::new(0);
pub static WALK_LOOP_NS: AtomicU64 = AtomicU64::new(0);
pub static WALK_COMP_N: AtomicU64 = AtomicU64::new(0);
/// Loop arm: current.get_inode() + dir/x permission checks.
pub static WALK_A_NS: AtomicU64 = AtomicU64::new(0);
/// Loop arm: lookup_child (dentry child-map lock + compare).
pub static WALK_B_NS: AtomicU64 = AtomicU64::new(0);
/// Loop hit arm: icache refresh (global ICACHE lock path).
pub static WALK_C_NS: AtomicU64 = AtomicU64::new(0);
/// Loop arm: get_mount + follow_symlink check.
pub static WALK_D_NS: AtomicU64 = AtomicU64::new(0);
/// Loop miss arm: fs lookup + iget + dentry add (excludes A/B/D).
pub static WALK_MISSCOST_NS: AtomicU64 = AtomicU64::new(0);
/// Final dentry inode re-fetch + trailing-slash checks (walk tail).
pub static WALK_TAIL_NS: AtomicU64 = AtomicU64::new(0);
/// Walk misses whose fs lookup returned ENOENT (existence probes —
/// each one paid the full ext4 directory scan; the negative-memo's
/// target class).
pub static WALK_MISS_ENOENT: AtomicU64 = AtomicU64::new(0);
/// Negative-memo hits (ENOENT served from the bounded per-parent memo).
pub static NEGHIT: AtomicU64 = AtomicU64::new(0);
/// Stale negative memos discarded (parent mutated since insert).
pub static NEGSTALE: AtomicU64 = AtomicU64::new(0);

// === open tail anatomy (perf-gap-hunt 10th) ================================
// OPEN_TAIL split: the full-path memo lookup itself, the path_lookup call
// around it (lk - memo = walk prologue + VfsPath build), the post-lookup
// checks (directory/device/DAC/lease/fifo), File Arc construction + ops
// resolution, and the fdtable install + inotify gate. Divisor = OPEN_N.
pub static MEMO_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_LK_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_CHK_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_FIL_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_INS_NS: AtomicU64 = AtomicU64::new(0);

// === open() wall-vs-exec anatomy (21st successor) =========================
// syscost's open row is WALL time per syscall — under the OH boot's load
// it conflates kernel CPU with deschedule + spin-wait. These brackets
// split the two: OPEN_WALL_MAX names the worst wall per open,
// OPEN_EXEC_NS/OPEN_EXEC_MAX measure the sched-exec share (actual CPU
// consumed inside the syscall, from the CFS entity's exec accounting).
// exec ~= wall  -> real kernel cost (fix the fs path);
// exec << wall  -> waiting (scheduling / lock-holder descheduled).
pub static OPEN_WALL_MAX_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_EXEC_NS: AtomicU64 = AtomicU64::new(0);
pub static OPEN_EXEC_MAX_NS: AtomicU64 = AtomicU64::new(0);

/// Max-update helper: keep the running maximum of `val` in `dst`.
#[inline]
fn max_update(dst: &AtomicU64, val: u64) {
    let mut cur = dst.load(Ordering::Relaxed);
    while val > cur {
        match dst.compare_exchange_weak(cur, val, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(c) => cur = c,
        }
    }
}

/// Entry stamp for the open wall/exec brackets. Returns (wall_t0,
/// exec_armed): the exec side charges the task's CFS accounting up to
/// the entry instant (the returned pre-open window is discarded); the
/// exit call then returns exactly the on-CPU time consumed inside the
/// syscall. (0, false) when the probe is off or no task context exists.
#[inline]
pub fn open_exec_stamp() -> (u64, bool) {
    if !on() {
        return (0, false);
    }
    let w0 = crate::drivers::timer::monotonic_ns_since_boot();
    match crate::sched::current() {
        None => (w0, false),
        Some(t) => {
            t.sched_entity()
                .update_exec_runtime(crate::sched::fair::sched_clock());
            (w0, true)
        }
    }
}

/// Close the open wall/exec brackets opened by open_exec_stamp().
#[inline]
pub fn open_close(w0: u64, exec_armed: bool) {
    if w0 == 0 {
        return;
    }
    let wall = crate::drivers::timer::monotonic_ns_since_boot().saturating_sub(w0);
    max_update(&OPEN_WALL_MAX_NS, wall);
    if exec_armed {
        if let Some(t) = crate::sched::current() {
            let d = t
                .sched_entity()
                .update_exec_runtime(crate::sched::fair::sched_clock());
            OPEN_EXEC_NS.fetch_add(d, Ordering::Relaxed);
            max_update(&OPEN_EXEC_MAX_NS, d);
        }
    }
}

pub fn note_walk_miss_enoent() {
    WALK_MISS_ENOENT.fetch_add(1, Ordering::Relaxed);
}

pub fn note_neghit() {
    NEGHIT.fetch_add(1, Ordering::Relaxed);
}

pub fn note_negstale() {
    NEGSTALE.fetch_add(1, Ordering::Relaxed);
}

// (21st successor) cmdline `negmemo=1`: bounded negative-lookup memo in
// the path walk (ext4 ENOENT results cached as negative dentry children,
// invalidated by per-parent mut_gen, capped per parent). Gated separately
// from the counters so a boot can measure the miss class with the fix
// off, then A/B with it on.
static NEG_MEMO_ARMED8: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn negmemo_on() -> bool {
    match NEG_MEMO_ARMED8.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => crate::cmdline::get_param("negmemo").as_deref() == Some("1"),
            };
            NEG_MEMO_ARMED8.store(if v { 2 } else { 1 }, Ordering::Relaxed);
            crate::pr_warn!(
                "negmemo: {} (cmdline negmemo param = {:?})",
                if v { "armed" } else { "off" },
                crate::cmdline::get_param("negmemo")
            );
            v
        }
    }
}

/// Bracket timestamp (0 when the probe is off — skips the clock read).
#[inline]
pub fn pc_now() -> u64 {
    if on() {
        crate::drivers::timer::monotonic_ns_since_boot()
    } else {
        0
    }
}

/// Close a bracket opened with pc_now() (no-op when t0 == 0, i.e. off).
#[inline]
pub fn pc_add(dst: &AtomicU64, t0: u64) {
    if t0 != 0 {
        dst.fetch_add(
            crate::drivers::timer::monotonic_ns_since_boot().saturating_sub(t0),
            Ordering::Relaxed,
        );
    }
}

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
            if v {
                crate::fs::path_memo::arm_gauges();
            }
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
        if crate::debug::fsw::on() {
            $c.fetch_add(1, Ordering::Relaxed);
        }
    };
    ($c:expr, $n:expr) => {
        if crate::debug::fsw::on() {
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
    if crate::debug::fsw::on() {
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
    if crate::debug::fsw::on() {
        PUB_FLUSH.fetch_add(1, Ordering::Relaxed);
        PUB_RECORDS.fetch_add(records as u64, Ordering::Relaxed);
    }
}
#[inline]
pub fn note_pub_scoped(records: usize) {
    if crate::debug::fsw::on() {
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
pub fn note_pub_batch() {
    bump!(PUB_BATCH_DRAINS);
}
/// Record one whole-bracket ext4_mkdir duration (microseconds).
#[inline]
pub fn note_mkdir_us(delta_us: u64) {
    if crate::debug::fsw::on() {
        MKDIR_US_N.fetch_add(1, Ordering::Relaxed);
        MKDIR_US_SUM.fetch_add(delta_us, Ordering::Relaxed);
        let mut cur = MKDIR_US_MAX.load(Ordering::Relaxed);
        while delta_us > cur {
            match MKDIR_US_MAX.compare_exchange_weak(
                cur,
                delta_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }
}
/// jbd2 registration dedup-scan iterations (see J_DEDUP_ITERS).
#[inline]
pub fn note_j_dedup(iters: usize) {
    bump!(J_DEDUP_ITERS, iters);
}
/// Freed-block dirty-buffer drops (bio::drop_freed_block) — the
/// discard-on-free path that keeps dead data out of publication drains.
pub static DROP_BLOCK: AtomicU64 = AtomicU64::new(0);
#[inline]
pub fn note_drop_block() {
    bump!(DROP_BLOCK);
}
/// Record one whole-bracket ext4_rmdir duration (microseconds). Count
/// rides MKDIR-class counters' shape: RMDIR count = FREE_RUN-free path is
/// not 1:1, so a dedicated count is not kept — use MKDIR_US_N for mkdir
/// only; rmdir/unlink report sum/max.
#[inline]
pub fn note_rmdir_us(delta_us: u64) {
    if crate::debug::fsw::on() {
        RMDIR_US_SUM.fetch_add(delta_us, Ordering::Relaxed);
        let mut cur = RMDIR_US_MAX.load(Ordering::Relaxed);
        while delta_us > cur {
            match RMDIR_US_MAX.compare_exchange_weak(
                cur,
                delta_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }
}
/// Record one whole-bracket ext4_unlink duration (microseconds).
#[inline]
pub fn note_unlink_us(delta_us: u64) {
    if crate::debug::fsw::on() {
        UNLINK_US_SUM.fetch_add(delta_us, Ordering::Relaxed);
        let mut cur = UNLINK_US_MAX.load(Ordering::Relaxed);
        while delta_us > cur {
            match UNLINK_US_MAX.compare_exchange_weak(
                cur,
                delta_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }
}
/// ext4_rmdir_inner per-phase nanosecond sums (monotonic clock):
/// find+is_dir_empty / delete_entry / inode writes / free_inode /
/// free_inode_blocks. Dumped as FSW3.
pub static RMD_PHASE1_NS: AtomicU64 = AtomicU64::new(0);
pub static RMD_PHASE2_NS: AtomicU64 = AtomicU64::new(0);
pub static RMD_PHASE3_NS: AtomicU64 = AtomicU64::new(0);
pub static RMD_PHASE4_NS: AtomicU64 = AtomicU64::new(0);
pub static RMD_PHASE5_NS: AtomicU64 = AtomicU64::new(0);
/// ext4_rmdir_inner phase-N max (microseconds), same order.
pub static RMD_PHASE_MAX_US: [AtomicU64; 5] = [
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0),
];
#[inline]
pub fn note_rmdir_phases(p1: u64, p2: u64, p3: u64, p4: u64, p5: u64) {
    if !crate::debug::fsw::on() {
        return;
    }
    RMD_PHASE1_NS.fetch_add(p1, Ordering::Relaxed);
    RMD_PHASE2_NS.fetch_add(p2, Ordering::Relaxed);
    RMD_PHASE3_NS.fetch_add(p3, Ordering::Relaxed);
    RMD_PHASE4_NS.fetch_add(p4, Ordering::Relaxed);
    RMD_PHASE5_NS.fetch_add(p5, Ordering::Relaxed);
    let ps = [p1, p2, p3, p4, p5];
    for (i, &v) in ps.iter().enumerate() {
        let us = v / 1000;
        let mut cur = RMD_PHASE_MAX_US[i].load(Ordering::Relaxed);
        while us > cur {
            match RMD_PHASE_MAX_US[i].compare_exchange_weak(
                cur, us, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => cur = x,
            }
        }
    }
}
#[inline]
pub fn note_pub_lazy() {
    bump!(PUB_LAZY);
}
#[inline]
pub fn note_pub_qmax(len: usize) {
    if crate::debug::fsw::on() {
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
    if crate::debug::fsw::on() {
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
    if crate::debug::fsw::on() {
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
    if !on() {
        return; // shutdown-path dump hook: silent when the probe is off
    }
    let secs = crate::drivers::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
    crate::pr_warn!(
        // sd: sync_dirty calls / deferred-skips — the separator matters:
        // two concatenated u64s are unreadable (the pa59 route-1/control
        // pair had to guess the split: read 31765|408 vs 70770|0).
        // 0cd29a75 merge: main's scoped-publication counters (ps/pc/ph/pm)
        // carried in alongside; slot count unchanged.
        "FSW1 t={}s br={} rd={} wr={} sd={}/{} ev={}/{} sa={} pb={}.{} ps={}.{} pc={} ph={} pb2={} pm={} pl={} lvl={} es={}",
        secs,
        v(&BREAD), v(&DEV_READ), v(&DEV_WRITE),
        v(&SYNC_DIRTY), v(&SYNC_DIRTY_SKIP),
        v(&EVICT), v(&EVICT_DIRTY), v(&SYNC_ALL),
        v(&PUB_FLUSH), v(&PUB_RECORDS),
        v(&PUB_SCOPED), v(&PUB_SCOPED_RECORDS),
        v(&PUB_CSTR), v(&PUB_HIWAT), v(&PUB_BATCH_DRAINS), v(&PUB_QMAX), v(&PUB_LAZY),
        crate::printk::get_console_loglevel(),
        crate::printk::console_emit_skipped(),
    );
    crate::pr_warn!(
        "FSW2 t={}s ri={} wi={} lk={}.{} ls={} ni={} ar={} fr={} j={}.{}.{}.{}.{} bl={}.{} w={}.{}.{} mk={}.{}.{} rm={}.{} ul={}.{} jd={}",
        secs,
        v(&READ_INODE), v(&WRITE_INODE),
        v(&DIR_LOOKUP), v(&DIR_BLOCKS), v(&LIST_DIR),
        v(&NEW_INODE), v(&ALLOC_RUN), v(&FREE_RUN),
        v(&J_START), v(&J_STOP), v(&J_LAZY),
        v(&J_COMMIT_FAST), v(&J_REVOKE),
        v(&BIGLOCK), v(&BIGLOCK_CONTEND),
        v(&WALK), v(&WALK_HIT), v(&WALK_MISS),
        v(&MKDIR_US_N), v(&MKDIR_US_SUM), v(&MKDIR_US_MAX),
        v(&RMDIR_US_SUM), v(&RMDIR_US_MAX),
        v(&UNLINK_US_SUM), v(&UNLINK_US_MAX),
        v(&J_DEDUP_ITERS),
    );
    crate::pr_warn!(
        // rmdir phase sums in MICROSECONDS (find+empty / delete / inodes
        // / free_inode / free_blocks) and per-phase max in us.
        "FSW3 t={}s rmd={} {} {} {} {} rmdmax={} {} {} {} {} db={}",
        secs,
        v(&RMD_PHASE1_NS) / 1000, v(&RMD_PHASE2_NS) / 1000,
        v(&RMD_PHASE3_NS) / 1000, v(&RMD_PHASE4_NS) / 1000,
        v(&RMD_PHASE5_NS) / 1000,
        v(&RMD_PHASE_MAX_US[0]), v(&RMD_PHASE_MAX_US[1]),
        v(&RMD_PHASE_MAX_US[2]), v(&RMD_PHASE_MAX_US[3]),
        v(&RMD_PHASE_MAX_US[4]),
        v(&DROP_BLOCK),
    );
    crate::pr_warn!(
        // path-walk cost anatomy (perf-path lane): phase NS sums and
        // their divisor counts. Averages: stat pre = STAT_PRE_NS/STAT_N,
        // walk pre = WALK_PRE_NS/WALK, loop = WALK_LOOP_NS/WALK,
        // per-component A..D = WALK_x_NS/WALK_COMP_N, miss arm =
        // WALK_MISSCOST_NS/WALK_MISS, walk tail = WALK_TAIL_NS/WALK,
        // open pre = OPEN_PRE_NS/OPEN_N, tails likewise.
        // (PC1/PC2 split, 21st successor: the printk line cap truncated
        // the single line before the miss/negative/open-exec fields —
        // lb346 lost everything past 'c='.)
        "PC1 t={}s stat={} pre={} tail={} open={} pre={} tail={} walk pre={} loop={} tail={} comp={} a={} b={} c={} d={}",
        secs,
        v(&STAT_N), v(&STAT_PRE_NS), v(&STAT_TAIL_NS),
        v(&OPEN_N), v(&OPEN_PRE_NS), v(&OPEN_TAIL_NS),
        v(&WALK_PRE_NS), v(&WALK_LOOP_NS), v(&WALK_TAIL_NS),
        v(&WALK_COMP_N), v(&WALK_A_NS), v(&WALK_B_NS),
        v(&WALK_C_NS), v(&WALK_D_NS),
    );
    crate::pr_warn!(
        // 21st successor PC2: the miss class (the walk loop's 85%
        // occupant — 130us per miss, 52% of them ENOENT existence
        // probes), the bounded negative-memo verdicts, and the open
        // wall-vs-exec split (OPEN_EXEC_NS average = /OPEN_N; exec ~=
        // wall means real kernel CPU, exec << wall means waiting).
        "PC2 t={}s miss={} misseo={} neghit={} negstale={} pmhit={} pmmiss={} pmins={} openwmax={}us openexec={} openexecavg={}ns openexecmax={}us",
        secs,
        v(&WALK_MISSCOST_NS), v(&WALK_MISS_ENOENT), v(&NEGHIT), v(&NEGSTALE),
        v(&crate::fs::path_memo::HIT), v(&crate::fs::path_memo::MISS),
        v(&crate::fs::path_memo::INSERT),
        v(&OPEN_WALL_MAX_NS) / 1000, v(&OPEN_EXEC_NS),
        if OPEN_N.load(Ordering::Relaxed) > 0 {
            v(&OPEN_EXEC_NS) / v(&OPEN_N)
        } else {
            0
        },
        v(&OPEN_EXEC_MAX_NS) / 1000,
    );
    crate::pr_warn!(
        // perf-gap-hunt 10th, open tail anatomy (all /OPEN_N, ns avg):
        // memo = the full-path memo lookup call itself, lk = the whole
        // path_lookup, chk = post-lookup checks, fil = File Arc + ops,
        // ins = fdtable install + inotify gate.
        "PC3 t={}s memo={} lk={} chk={} fil={} ins={}",
        secs,
        if OPEN_N.load(Ordering::Relaxed) > 0 {
            v(&MEMO_NS) / v(&OPEN_N)
        } else {
            0
        },
        if OPEN_N.load(Ordering::Relaxed) > 0 {
            v(&OPEN_LK_NS) / v(&OPEN_N)
        } else {
            0
        },
        if OPEN_N.load(Ordering::Relaxed) > 0 {
            v(&OPEN_CHK_NS) / v(&OPEN_N)
        } else {
            0
        },
        if OPEN_N.load(Ordering::Relaxed) > 0 {
            v(&OPEN_FIL_NS) / v(&OPEN_N)
        } else {
            0
        },
        if OPEN_N.load(Ordering::Relaxed) > 0 {
            v(&OPEN_INS_NS) / v(&OPEN_N)
        } else {
            0
        },
    );
}
