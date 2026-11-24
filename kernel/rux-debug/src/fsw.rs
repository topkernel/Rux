//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The fsw REPORT half (moved from kernel/src/debug/fsw.rs, crate-flip
//! rung rux-obs): the FSW1/FSW2/FSW3/PC1/PC2 counter snapshot lines.
//! The counters (already pub statics) and the note_*/pc_* record fns
//! stay framework-side; the cadence gate (tick) stays there too. Body
//! moved verbatim — the line formats are parsed by text by the perf
//! gates.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::debug::fsw::*;

fn v(c: &AtomicU64) -> u64 {
    c.load(Ordering::Relaxed)
}

/// Print the counter snapshot (cumulative since boot).
pub fn dump() {
    if !crate::debug::fsw::on() {
        return; // shutdown-path dump hook: silent when the probe is off
    }
    let secs = crate::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
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
        "PC2 t={}s miss={} misseo={} neghit={} negstale={} openwmax={}us openexec={} openexecavg={}ns openexecmax={}us",
        secs,
        v(&WALK_MISSCOST_NS), v(&WALK_MISS_ENOENT), v(&NEGHIT), v(&NEGSTALE),
        v(&OPEN_WALL_MAX_NS) / 1000, v(&OPEN_EXEC_NS),
        if OPEN_N.load(Ordering::Relaxed) > 0 {
            v(&OPEN_EXEC_NS) / v(&OPEN_N)
        } else {
            0
        },
        v(&OPEN_EXEC_MAX_NS) / 1000,
    );
}
