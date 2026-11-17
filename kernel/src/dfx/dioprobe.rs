//! TEMPORARY development probe (wrfsync lane) — NOT for merge.
//! Cycle decomposition of the direct-IO / fsync paths on the fio rig.
//! Armed by the `dioprobe` cmdline param; the dump prints at poweroff
//! (gated in sys_reboot). Every hook is one relaxed load when off.

use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::AtomicU64;

macro_rules! dioprobe_counters {
    ($($name:ident),* $(,)?) => {
        $(
            pub static $name: AtomicU64 = AtomicU64::new(0);
        )*
    };
}

// ---- read side (pread O_DIRECT 1 MiB) ----
dioprobe_counters! {
    RD_N,            // pread O_DIRECT calls bracketed
    RD_CYCLES,       // total cycles in sys_pread64 body
    RD_ALLOC_CYCLES, // alloc_staging_buf
    RD_FILL_CYCLES,  // file.read_at_slice (whole ext4 side)
    RD_SUBMIT_CYCLES,// inside blkdev_read_async submissions (sum)
    RD_WAIT_CYCLES,  // completion.wait() inside drain (sum)
    RD_COPYOUT_CYCLES,// copy_out user
    RD_SUBMITS,      // blkdev_read_async call count
    RD_DRAINS,       // drain windows
    RD_EXTWALK_CYCLES,// ext4_ext_get_run calls (sum)
    RD_EXTWALKS,     // ext4_ext_get_run call count
}

// ---- write side (pwrite O_DIRECT 1 MiB) ----
dioprobe_counters! {
    WR_N,
    WR_CYCLES,        // total cycles in sys_pwrite64 body
    WR_ALLOC_CYCLES,  // alloc_staging_buf
    WR_COPYIN_CYCLES, // copy_in from user
    WR_CORE_CYCLES,   // file.write_at_slice (whole ext4 side)
    WR_ALLOCBLK_CYCLES,// allocate_blocks_for_file (sum)
    WR_FASTSUB_CYCLES,// DIO fast-run submissions + drain inside ext4_file_write
    WR_RSS_CYCLES,    // run_sync_spans (whole, per O_DIRECT post-write)
    WR_SUBMIT_CYCLES, // blkdev_write_async submission bodies (sum)
    WR_WAIT_CYCLES,   // WriteBatch flush waits (sum)
    WR_SUBMITS,
    WR_DRAINS,
}

// ---- fsync side (write+fsync=1 4k) ----
dioprobe_counters! {
    FS_N,
    FS_CYCLES,        // total cycles in do_fsync body (ext4)
    FS_FDI_CYCLES,    // ext4_flush_dirty_inode
    FS_WAC_CYCLES,    // ext4_write_at_core calls inside flush (sum)
    FS_RSS_CYCLES,    // run_sync_spans calls inside flush (sum)
    FS_SPAN_DRAIN_CYCLES,// SpanFlusher drain device waits (sum)
    FS_WALK_CYCLES,   // sync_file data walk (extent run walk + probes)
    FS_ITB_CYCLES,    // inode-table block batched sync
    FS_JSTOP_CYCLES,  // ext4_journal_stop (incl. commit)
    FS_COMMIT_CYCLES, // jbd2_journal_commit_transaction fast path
    FS_COMMIT_SYNC_CYCLES, // sync_dirty_blocks_batched inside commit
    FS_SUBMIT_CYCLES, // all blkdev_write_async submission bodies (sum)
    FS_WAIT_CYCLES,   // all completion.wait in flush windows (sum)
    FS_SUBMITS,
    FS_DRAINS,
    FS_BREADS,        // bread() calls from these paths (sum)
    // sub-attribution of FS_WAC_CYCLES (the flush's write core) and walk
    FS_WAC_RI_CYCLES,   // fs.read_inode inside ext4_write_at_core (sum)
    FS_WAC_FW_CYCLES,   // ext4_file_write inside write core (sum)
    FS_WAC_WI_CYCLES,   // write_inode inside write core (sum)
    FS_WAC_POP_CYCLES,  // page-cache populate/update inside write core (sum)
    FS_WAC_JS_CYCLES,   // null-journal start+stop inside write core (sum)
    FS_WALK_PROBES,     // block_dirty_cached probes in the fsync data walk
    FS_WALK_DIRTY,      // dirty blocks the walk found (sum)
    FS_ITB_N,           // itb sync entries (how many fsyncs paid the itb round trip)
    // DIO fast-run gate attribution (why did the fast run not fire?)
    WR_GATE_NOBATCH,   // dio_batch was None (dio_fast false or bs!=4096)
    WR_GATE_NOEXTENT,  // inode.has_extent() == false (extent path not even tried)
    WR_GATE_EXTENT,    // extent run resolve failed / zero
    WR_GATE_CACHED,    // run truncated to a cached single block
    WR_GATE_SUBMIT_NO, // submit_ref returned false
    WR_GATE_HIT,       // fast run submitted
}

// ---- write(2) buffered (fsync row's write leg) ----
dioprobe_counters! {
    BW_N,
    BW_CYCLES,        // ext4_buffered_write total
}

#[inline]
pub fn on() -> bool {
    // Not a trace event (temp probe): cmdline param.
    crate::cmdline::has_param("dioprobe")
}

#[inline]
pub fn now() -> u64 {
    crate::arch::cpu::read_counter()
}

#[inline]
pub fn charge(ctr: &AtomicU64, since: u64) {
    let d = now().wrapping_sub(since);
    ctr.fetch_add(d, Relaxed);
}

#[inline]
pub fn bump(ctr: &AtomicU64) {
    ctr.fetch_add(1, Relaxed);
}

/// cycles -> microseconds (tsc hz measured lazily once)
fn us(cycles: u64, hz: u64) -> u64 {
    if hz == 0 {
        0
    } else {
        cycles * 1_000_000 / hz
    }
}

/// Poweroff dump. One line per counter group, compact for serial logs.
pub fn dump() {
    if !on() {
        return;
    }
    // calibrate tsc against the 10 MHz timekeeper
    let t0 = crate::drivers::timer::read_time();
    let c0 = now();
    while crate::drivers::timer::read_time().wrapping_sub(t0) < 20_000 {
        core::hint::spin_loop();
    }
    let dt = crate::drivers::timer::read_time().wrapping_sub(t0);
    let hz = if dt > 0 {
        now().wrapping_sub(c0) * 10_000_000 / dt
    } else {
        0
    };
    let g = |c: &AtomicU64| c.load(Relaxed);
    let line = |n: u64, cyc: &AtomicU64| -> alloc::string::String {
        alloc::format!("{}us/{}={}ns", us(g(cyc), hz), n, if n > 0 { us(g(cyc), hz) * 1000 / n } else { 0 })
    };
    crate::println!("DIOPROBE tsc_hz={}", hz);
    crate::println!(
        "RD n={} tot={} alloc={} fill={} submit={}({}) wait={}({}) copyout={} extwalk={}({}) drains={}",
        g(&RD_N), line(g(&RD_N), &RD_CYCLES), line(g(&RD_N), &RD_ALLOC_CYCLES),
        line(g(&RD_N), &RD_FILL_CYCLES), line(g(&RD_SUBMITS), &RD_SUBMIT_CYCLES), g(&RD_SUBMITS),
        line(g(&RD_DRAINS), &RD_WAIT_CYCLES), g(&RD_DRAINS),
        line(g(&RD_N), &RD_COPYOUT_CYCLES), line(g(&RD_EXTWALKS), &RD_EXTWALK_CYCLES), g(&RD_EXTWALKS),
        g(&RD_DRAINS),
    );
    crate::println!(
        "WR n={} tot={} alloc={} copyin={} core={} allocblk={} fastsub={} rss={} submit={}({}) wait={}({}) drains={}",
        g(&WR_N), line(g(&WR_N), &WR_CYCLES), line(g(&WR_N), &WR_ALLOC_CYCLES),
        line(g(&WR_N), &WR_COPYIN_CYCLES), line(g(&WR_N), &WR_CORE_CYCLES),
        line(g(&WR_N), &WR_ALLOCBLK_CYCLES), line(g(&WR_N), &WR_FASTSUB_CYCLES),
        line(g(&WR_N), &WR_RSS_CYCLES), line(g(&WR_SUBMITS), &WR_SUBMIT_CYCLES), g(&WR_SUBMITS),
        line(g(&WR_DRAINS), &WR_WAIT_CYCLES), g(&WR_DRAINS), g(&WR_DRAINS),
    );
    crate::println!(
        "FS n={} tot={} fdi={} wac={} rss={} spandrain={} walk={} itb={} jstop={} commit={} commitsync={} submit={}({}) wait={}({}) breads={}",
        g(&FS_N), line(g(&FS_N), &FS_CYCLES), line(g(&FS_N), &FS_FDI_CYCLES),
        line(g(&FS_N), &FS_WAC_CYCLES), line(g(&FS_N), &FS_RSS_CYCLES),
        line(g(&FS_N), &FS_SPAN_DRAIN_CYCLES), line(g(&FS_N), &FS_WALK_CYCLES),
        line(g(&FS_N), &FS_ITB_CYCLES), line(g(&FS_N), &FS_JSTOP_CYCLES),
        line(g(&FS_N), &FS_COMMIT_CYCLES), line(g(&FS_N), &FS_COMMIT_SYNC_CYCLES),
        line(g(&FS_SUBMITS), &FS_SUBMIT_CYCLES), g(&FS_SUBMITS),
        line(g(&FS_DRAINS), &FS_WAIT_CYCLES), g(&FS_DRAINS), g(&FS_BREADS),
    );
    crate::println!(
        "FSSUB wac: ri={} fw={} wi={} pop={} js={} walkprobes={} walkdirty={} itbN={}",
        line(g(&FS_N), &FS_WAC_RI_CYCLES), line(g(&FS_N), &FS_WAC_FW_CYCLES),
        line(g(&FS_N), &FS_WAC_WI_CYCLES), line(g(&FS_N), &FS_WAC_POP_CYCLES),
        line(g(&FS_N), &FS_WAC_JS_CYCLES), g(&FS_WALK_PROBES), g(&FS_WALK_DIRTY), g(&FS_ITB_N),
    );
    crate::println!(
        "WRGATE nobatch={} noextent={} extentfail={} cached={} submitno={} hit={}",
        g(&WR_GATE_NOBATCH), g(&WR_GATE_NOEXTENT), g(&WR_GATE_EXTENT), g(&WR_GATE_CACHED),
        g(&WR_GATE_SUBMIT_NO), g(&WR_GATE_HIT),
    );
    crate::println!("BW n={} tot={}", g(&BW_N), line(g(&BW_N), &BW_CYCLES));
}
