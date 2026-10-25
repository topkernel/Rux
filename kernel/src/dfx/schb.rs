//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Soft-wedge heartbeat (`dfx=schb`, sc-series quiet-quiet hunt, session R).
//!
//! The quiet-quiet boot (quietcon + loglevel=3) has a second failure shape
//! the classic-race probes cannot see: the pid flow stalls in the
//! 900-1400 zone from ~min 10-12 with NO rwlock freeze word and NO kernel
//! evidence (sc196/sc197; sc195 passes it 1-in-3). The evidence gap is
//! structural: `printk_bytes` drops every record above CONSOLE_LOGLEVEL
//! BEFORE the ring, so at loglevel=3 all pr_warn! census output
//! (SACMP, binder-stall, stkfutex, RWFRZ) never existed — muted is the
//! wrong model; it was never recorded.
//!
//! This module buys visibility back with a budget far below the
//! loglevel-4 watermark (the sc198 lesson: probe traffic modulates the
//! console race):
//!
//! - ONE ~90-byte heartbeat line every `BEAT_SECS` (10s), emitted from
//!   this kthread's own context (never a racer thread) straight to the
//!   UART via `ConsoleWriter` — no printk ring write, no persistent-log
//!   disk I/O, nothing on any binder/futex critical path.
//! - `SC-PROC` lines name each new binder process as the heartbeat
//!   drains a ring that `binder_open` fills — the port-IO again lands
//!   in schbd, not the opener.
//! - When the pid flow plateaus for `PLATEAU_BEATS` beats (60s) past
//!   early boot, ONE census dump fires per episode: every binder
//!   read-wait thread regardless of age (stall_report only prints
//!   waiters older than 120s), the scheduler view of every runnable /
//!   D-state task, the kernel stall machinery with the console loglevel
//!   briefly lifted to 4 (binder-stall + freeze_scan + WAKE-RUN-LEAK
//!   then print), and the tail of the printk ring — quietcon stashes
//!   the userspace /dev/console stream there, so the records around
//!   the wedge are the userspace last-known-good boundary. The dump
//!   fires after the wedge has settled, so its burn cannot decide the
//!   race it reports on.

use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::dfx::backtrace::ConsoleWriter;
use crate::dfx::switches::{self, DfxSwitch};

extern crate alloc;

/// Heartbeat period (seconds). One line per beat is ~2 orders of
/// magnitude below the loglevel-4 probe stream that carried sc198's
/// freeze in at ~min 9.
const BEAT_SECS: u64 = 10;

/// Consecutive zero-flow beats that would arm a census on the freeze
/// shape alone — kept as documentation of the original sc197 shape;
/// the live trigger is the generalized STALL_BEATS below.
const PLATEAU_BEATS: u32 = 6;
#[allow(dead_code)]
const _PLATEAU_DOC: u32 = PLATEAU_BEATS;

/// sc200 (11th leg): the wedge also comes as a CRAWL, not a freeze —
/// the pid frontier advances 1-9 pids per beat forever (retry-loop
/// churn: DB_TaskExecutor threads), so flow==0 never happens. A beat
/// counts as stalled when ANY of: flow == 0, flow <= CRAWL_FLOW_MAX,
/// or bw15 >= WAITER15_FLOOR (the binder world that parked). Census
/// after STALL_BEATS consecutive stalled beats (120s).
const CRAWL_FLOW_MAX: u32 = 5;
const WAITER15_FLOOR: usize = 150;
const STALL_BEATS: u32 = 12;

/// Minimum boot age (seconds) before a census may fire — early-boot
/// lulls before the service storm are not the wedge.
const MIN_BOOT_SECS: u64 = 240;

/// Minimum live tasks before a census may fire (past early init).
const MIN_TASKS: usize = 50;

/// Leg-17: deterministic census timing (`censusat=<secs>` boot param).
/// The 80%-heap trigger inside the beat cannot fire on a boot parked at
/// the render_service lottery (heap stalls ~300MB of the ~1.1G window),
/// and tonight 3-of-3 rolls parked — the type-attribution mission was
/// hostage to a userspace race. This param fires the SAME one-shot
/// `oom_anat_dump()` at a fixed boot age instead, so even a parked boot
/// banks the census + peek histograms. u64::MAX = disabled (default);
/// the DONE guard inside `oom_anat_dump` dedupes against the 80% trigger
/// (whichever fires first wins).
static CENSUS_AT_SECS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Ring records dumped at the census (quietcon's userspace console
/// stream lives in the printk ring; its tail around the wedge is the
/// userspace last-known-good).
const RING_TAIL: usize = 192;

/// Depth of the binder-proc open ring (one slot per binder_open; an OH
/// boot opens on the order of a hundred).
const PROC_RING_LEN: usize = 128;

static RUNNING: AtomicBool = AtomicBool::new(false);

/// binder_open trace ring: (seq, pid, jiffies). Filled by `note_proc_open`
/// from the opener's context (spinlock push only — no I/O there); drained
/// and printed by schbd so every byte of UART cost stays off racer paths.
static PROC_RING: crate::sync::spinlock::Spinlock<[(u64, u32, u64); PROC_RING_LEN]> =
    crate::sync::spinlock::Spinlock::new([(0, 0, 0); PROC_RING_LEN]);
static PROC_SEQ: AtomicU64 = AtomicU64::new(0);

/// Record a binder-proc creation (called from `binder_open`). No-op
/// unless `dfx=schb`. Bounded cost: one switch load + one spinlock store.
pub fn note_proc_open(pid: u32) {
    if !switches::enabled(DfxSwitch::Schb) {
        return;
    }
    let seq = PROC_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let j = crate::drivers::timer::get_jiffies();
    let mut ring = PROC_RING.lock();
    ring[(seq as usize - 1) % PROC_RING_LEN] = (seq, pid, j);
}

// ===== Leg-15 pace counters (launcher round decomposition) =====
//
// sc210 measured ~11 s per launcher getOsAccountLocalId round = ~3000 COW
// faults + 40 forks, with the zone pinned at free~100 pages (wmark low
// 2451, in_direct_reclaim=1) — every COW fault's page alloc rides the
// serialized direct-reclaim slowpath. These cumulative counters decompose
// the round cost from the heartbeat alone: avg ms/fault = 1000/KERNEL_HZ *
// d(cowJ)/d(cow), reclaim share = d(drctJ)/beat-jiffies, regime =
// zfree/heapK trajectory. All sites are fault/alloc hot paths: ONE
// relaxed fetch_add per event, no locks held at any call site, no I/O —
// the sc198 lesson (probe traffic modulates the race) is about prints;
// the only UART cost stays here in schbd's beat.

static PACE_COW: AtomicU64 = AtomicU64::new(0);
static PACE_COW_J: AtomicU64 = AtomicU64::new(0);
static PACE_DRCT: AtomicU64 = AtomicU64::new(0);
static PACE_DRCT_J: AtomicU64 = AtomicU64::new(0);
static PACE_DRCT_MAXJ: AtomicU64 = AtomicU64::new(0);
static PACE_SLOW: AtomicU64 = AtomicU64::new(0);

/// Record one COW fault resolution attempt. `j0` is the jiffies value
/// captured at handle_cow_fault entry (callers pass it so the exit-side
/// cost stays one read + adds). No-op unless `dfx=schb`.
pub fn note_cow(j0: u64) {
    if !switches::enabled(DfxSwitch::Schb) {
        return;
    }
    PACE_COW.fetch_add(1, Ordering::Relaxed);
    let d = crate::drivers::timer::get_jiffies().saturating_sub(j0);
    PACE_COW_J.fetch_add(d, Ordering::Relaxed);
}

/// Record one direct-reclaim round (try_to_free_pages), same shape.
pub fn note_drct(j0: u64) {
    if !switches::enabled(DfxSwitch::Schb) {
        return;
    }
    PACE_DRCT.fetch_add(1, Ordering::Relaxed);
    let d = crate::drivers::timer::get_jiffies().saturating_sub(j0);
    PACE_DRCT_J.fetch_add(d, Ordering::Relaxed);
    PACE_DRCT_MAXJ.fetch_max(d, Ordering::Relaxed);
}

/// Record one page-alloc slowpath entry (zone fastpath failed).
pub fn note_slow() {
    if !switches::enabled(DfxSwitch::Schb) {
        return;
    }
    PACE_SLOW.fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the pace counters for the beat line (schbd context only).
fn pace_snapshot() -> (u64, u64, u64, u64, u64, u64) {
    (
        PACE_COW.load(Ordering::Relaxed),
        PACE_COW_J.load(Ordering::Relaxed),
        PACE_DRCT.load(Ordering::Relaxed),
        PACE_DRCT_J.load(Ordering::Relaxed),
        PACE_DRCT_MAXJ.load(Ordering::Relaxed),
        PACE_SLOW.load(Ordering::Relaxed),
    )
}

/// One-shot allocator anatomy at the first fake-OOM floor (leg-16).
/// Called from the oom killer's "no killable processes" path: raw UART,
/// allocation-free, counters only — safe in any context, once per boot
/// regardless of dfx switches (NET_PAGES is always live; the kstack
/// high-water needs dfx=schb to have armed its probe).
pub fn oom_anat_dump() {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let (kn, khw) = crate::process::task::kstack_anat();
    taskdump_raw_line(b"OOM-ANAT kstack live=");
    taskdump_dec(kn);
    taskdump_raw_line(b" hw=");
    taskdump_dec(khw);
    taskdump_raw_line(b" taskSZ=");
    taskdump_dec(core::mem::size_of::<crate::process::task::Task>() as u64);
    // Type sizes for the per-class census mapping below (bytes, the
    // small-alloc class is the next power of two >= size).
    taskdump_raw_line(b"\nOOM-ANAT sizes: vma=");
    taskdump_dec(core::mem::size_of::<crate::mm::vma::Vma>() as u64);
    taskdump_raw_line(b" file=");
    taskdump_dec(core::mem::size_of::<crate::fs::file::File>() as u64);
    taskdump_raw_line(b" dentry=");
    taskdump_dec(core::mem::size_of::<crate::fs::dentry::Dentry>() as u64);
    taskdump_raw_line(b" inode=");
    taskdump_dec(core::mem::size_of::<crate::fs::inode::Inode>() as u64);
    taskdump_raw_line(b" mm=");
    taskdump_dec(core::mem::size_of::<crate::mm::mm_struct::MmStruct>() as u64);
    taskdump_raw_line(b" ptregs=");
    taskdump_dec(core::mem::size_of::<crate::arch::pt_regs::PtRegs>() as u64);
    taskdump_raw_line(b" fdte=");
    taskdump_dec(
        core::mem::size_of::<[Option<alloc::sync::Arc<crate::fs::file::File>>; 1024]>() as u64
            + 128,
    );
    taskdump_raw_line(b"\nOOM-ANAT buddy live pages by order:\n");
    // Leg-17 leak test: live user mms = creations - drops. If live_mm
    // >> live procs at the census moment, every spawn strands its
    // AddressSpace (and its VMA tree + vma_files pins ride along).
    {
        let mn = crate::dfx::memwatch::MM_NEWS.load(Ordering::Relaxed) as u64;
        let md = crate::dfx::memwatch::MM_DROPS.load(Ordering::Relaxed) as u64;
        taskdump_raw_line(b"OOM-ANAT mm: new=");
        taskdump_dec(mn);
        taskdump_raw_line(b" drop=");
        taskdump_dec(md);
        taskdump_raw_line(b" live=");
        taskdump_dec(mn.saturating_sub(md));
        taskdump_raw_line(b"\n");
    }
    let np = &crate::mm::buddy_allocator::NET_PAGES;
    let mut o = 0usize;
    while o < np.len() {
        let v = np[o].load(Ordering::Relaxed);
        if v != 0 {
            taskdump_raw_line(b" o");
            taskdump_dec(o as u64);
            taskdump_raw_line(b"=");
            taskdump_dec(v as u64);
            taskdump_raw_line(b"p\n");
        }
        o += 1;
    }
    crate::mm::small_alloc::census_line();
    // Leg-16: type identification for the two heaviest classes at the
    // floor (c256 ~520K live, c1024 ~84K live) — dump object headers from
    // the active slab pages; vtables/magic are symbolisable offline.
    crate::mm::small_alloc::census_peek(5, 8);
    crate::mm::small_alloc::census_peek(7, 8);
    // Leg-17: structural buckets (the v2 histogram alone cannot name a
    // pointer-first heterogeneous population).
    crate::mm::small_alloc::census_peek3(5);
    crate::mm::small_alloc::census_peek3(7);
}

/// Start the schbd kthread. Gated on the `dfx=schb` switch; called from
/// the same late-boot slot as khungtaskd (secondaries online, timer
/// wakeups routable).
pub fn init() {
    if !switches::enabled(DfxSwitch::Schb) {
        return;
    }
    // Leg-17: `censusat=<secs>` — see CENSUS_AT_SECS above. Parsed here
    // (init runs once, late boot, cmdline is stable by then).
    if let Some(v) = crate::cmdline::get_param("censusat") {
        if let Ok(secs) = v.trim().parse::<u64>() {
            CENSUS_AT_SECS.store(secs, Ordering::Release);
            crate::pr_info!("dfx: schbd census timer armed at {}s", secs);
        }
    }
    if RUNNING.load(Ordering::Acquire) {
        return;
    }
    let started =
        crate::process::kthread::kthread_run(schbd_fn, core::ptr::null_mut(), "schbd").is_some();
    if started {
        RUNNING.store(true, Ordering::Release);
        crate::pr_err!(
            "dfx: schbd soft-wedge heartbeat armed ({}s beat, census at {}s stalled)",
            BEAT_SECS,
            STALL_BEATS as u64 * BEAT_SECS
        );
    } else {
        crate::pr_err!("dfx: schbd failed to start");
    }
}

/// Trim a comm byte array to a printable &str.
fn comm_str(c: &[u8; 16]) -> &str {
    let end = c.iter().position(|&b| b == 0).unwrap_or(c.len());
    core::str::from_utf8(&c[..end]).unwrap_or("?")
}

/// schbd main loop: sleep / beat / detect / census.
extern "C" fn schbd_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::task::TaskState;

    let hz = crate::config::KERNEL_HZ as u64;
    if hz == 0 {
        return 0;
    }
    let beat_jiffies = (BEAT_SECS * hz).max(1);
    let boot_jiffies = crate::drivers::timer::get_jiffies();

    let mut last_pidmax: u32 = 0;
    let mut plateau: u32 = 0;
    let mut dumped = false; // one census per plateau episode
    let mut drained_seq: u64 = 0;

    loop {
        if crate::process::kthread::kthread_should_stop() {
            break;
        }

        // --- Periodic sleep (khungtaskd discipline: arm a one-shot timer
        // as the only waker, mark INTERRUPTIBLE, re-check, then sleep). ---
        let current = match crate::sched::current() {
            Some(t) => t as *mut crate::process::task::Task,
            None => break,
        };
        // SAFETY: current is this kthread's own task pointer.
        let my_pid = unsafe { (*current).pid() };
        let target = crate::drivers::timer::get_jiffies() + beat_jiffies;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);
        if timer_id != 0 {
            // SAFETY: current is the running task's pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
            }
            if crate::drivers::timer::get_jiffies() >= target {
                // SAFETY: current is the running task's pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    // A racing wake may have enqueued us mid-exit — take
                    // ourselves back off (NEW-C2 discipline).
                    crate::sched::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
            } else {
                crate::arch::cpu::restore_irq(true);
                crate::sched::schedule();
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            // Timer table full: stay runnable, yield one round, retry.
            crate::sched::schedule();
        }

        // --- Beat: one task-table walk + one binder count walk. ---
        let mut max_pid: u32 = 0;
        let mut n_tasks: usize = 0;
        let mut n_procs: usize = 0; // leg-16: tgid leaders (processes vs threads)
        let mut n_run: usize = 0; // state bits clear: runnable or running
        let mut n_mark: usize = 0; // state clear + on_cpu: executing OR orphaned mark
        let mut n_s: usize = 0; // INTERRUPTIBLE
        let mut n_d: usize = 0; // UNINTERRUPTIBLE
        let mut young_pid: u32 = 0;
        let mut young_comm = [0u8; 16];
        crate::sched::for_each_task(|task| {
            // SAFETY: callback runs under the pid-hash bucket lock; reads
            // only plain fields (same contract as khungtaskd's check_tasks).
            unsafe {
                let t = &*task;
                let pid = t.pid();
                n_tasks += 1;
                if t.tgid() == pid {
                    n_procs += 1;
                }
                if pid > max_pid {
                    max_pid = pid;
                }
                if pid >= young_pid {
                    young_pid = pid;
                    young_comm = *t.comm();
                }
                let b = t.state().bits();
                if b & 0x37 == 0 {
                    if t.on_cpu() {
                        n_mark += 1;
                    } else {
                        n_run += 1;
                    }
                } else if b & 0x02 != 0 {
                    n_d += 1;
                } else if b & 0x01 != 0 {
                    n_s += 1;
                }
            }
        });
        let (bprocs, bwait, bwait15) = crate::ipc::binder::schb_beat();
        let flow = max_pid.saturating_sub(last_pidmax);
        let t_secs = crate::drivers::timer::get_jiffies().saturating_sub(boot_jiffies) / hz;

        {
            // Leg-15 pace snapshot (all reads relaxed, schbd context):
            // cumulative COW attempts + jiffies spent in handle_cow_fault,
            // direct-reclaim rounds + jiffies + max round, slowpath
            // entries, zone free pages, kernel heap used KB.
            let (cow, cowj, drct, drctj, drctmax, slow) = pace_snapshot();
            let zfree = crate::mm::pglist::first_online_node()
                .map(|n| n.free_pages() as u64)
                .unwrap_or(0);
            let heapk = crate::mm::buddy_allocator::buddy_stats().used_bytes / 1024;
            // Leg-16: fire the allocator anatomy dump when the heap is
            // ~80% full — comfortably before the floor/OOM would, so a
            // roll that never quite reaches the fake-OOM still banks the
            // census (one-shot; the OOM path reuses the same guard).
            {
                let st = crate::mm::buddy_allocator::buddy_stats();
                if st.used_bytes * 5 >= st.heap_size * 4 {
                    oom_anat_dump();
                }
            }
            // Leg-17: fixed-time census (see CENSUS_AT_SECS). Whichever
            // trigger wins first, the DONE guard makes it one-shot.
            if t_secs >= CENSUS_AT_SECS.load(Ordering::Acquire) {
                oom_anat_dump();
            }
            let mut w = ConsoleWriter::new();
            let _ = writeln!(
                w,
                "SC-HB t={}s pidmax={} flow={} young={} tasks={} R={} M={} S={} D={} bproc={} bw={} bw15={} pace: cow={} cowJ={} drct={} drctJ={} drctMaxJ={} slow={} zfree={} heapK={}",
                t_secs, max_pid, flow, comm_str(&young_comm), n_tasks, n_run, n_mark, n_s, n_d,
                bprocs, bwait, bwait15, cow, cowj, drct, drctj, drctmax, slow, zfree, heapk
            );
        }

        // Leg-16 anatomy line: decomposes heapK at the floor. procs =
        // tgid leaders; kN/kK/kHW = live kernel stacks / their bytes /
        // boot-max high-water (probe armed by dfx=schb); smallK vs
        // smallPgK = live small-object estimate vs slab pages actually
        // held (the delta is packing slack); oN = buddy per-order live
        // pages (o5 = the 128KB kernel-stack class on this config).
        {
            let (kn, khw) = crate::process::task::kstack_anat();
            // kb/ks = packed-slab blocks/live slots (x86_64 only; 0/0
            // reads as "no slab" on arches that keep plain order-5
            // stacks — see process/task.rs).
            let (kb, ks, _fb) = crate::process::task::kstack_slab_stats();
            let (small_k, small_pg_k) = crate::mm::small_alloc::live_stats_kib();
            let np = &crate::mm::buddy_allocator::NET_PAGES;
            let mut w = ConsoleWriter::new();
            let _ = writeln!(
                w,
                "SC-ANAT t={}s procs={} kN={} kK={} kHW={} kb={} ks={} smallK={} smallPgK={} o0={} o1={} o2={} o3={} o4={} o5={} o6={} o7={} o8={}",
                t_secs, n_procs, kn, kn * (crate::config::KERNEL_STACK_SIZE as u64) / 1024, khw,
                kb, ks,
                small_k, small_pg_k,
                np[0].load(Ordering::Relaxed), np[1].load(Ordering::Relaxed),
                np[2].load(Ordering::Relaxed), np[3].load(Ordering::Relaxed),
                np[4].load(Ordering::Relaxed), np[5].load(Ordering::Relaxed),
                np[6].load(Ordering::Relaxed), np[7].load(Ordering::Relaxed),
                np[8].load(Ordering::Relaxed)
            );
        }

        // Drain the proc-open ring (SC-PROC lines) after the beat line.
        {
            let cur = PROC_SEQ.load(Ordering::Relaxed);
            if cur > drained_seq {
                let ring = PROC_RING.lock();
                let mut w = ConsoleWriter::new();
                let mut seq = drained_seq;
                while seq < cur {
                    let (s, pid, j) = ring[(seq as usize) % PROC_RING_LEN];
                    // A wrapped/overwritten slot shows a stale seq: skip
                    // silently (bounded ring; beats drain faster than 128
                    // opens in practice).
                    if s == seq + 1 {
                        let _ = writeln!(w, "SC-PROC seq={} pid={} @{}j", s, pid, j);
                    }
                    seq += 1;
                }
                drop(ring);
                drained_seq = cur;
            }
        }

        // Re-arm on REAL recovery only: a beat with healthy flow (>= 20)
        // proves the frontier moves and any prior census's heal worked, so
        // a later re-freeze gets a fresh census. bw15 alone must NOT gate
        // the re-arm - sc202 recovered to flow 27-54 with bw15 still ~300
        // (the old parked cohort never drains), and bw15-gating kept the
        // plateau counter alive forever, permanently disarming the census.
        // sc206: the else-if had the same defect through branch ORDER -
        // after the first census the parked cohort keeps bw15 >=
        // WAITER15_FLOOR on every beat, so the first arm of the OR always
        // fired and the flow >= 20 re-arm below was dead code: sc206's
        // t=537 flow=80 burst never re-armed the census and a 410 s
        // qualifying plateau (t=547-957, bw15 223-244) got no census.
        // Test the recovery arm FIRST: a flow >= 20 beat is a real
        // frontier move regardless of the parked waiter count.
        if flow >= 20 {
            plateau = 0;
            dumped = false;
        } else if flow <= CRAWL_FLOW_MAX || bwait15 >= WAITER15_FLOOR {
            plateau = plateau.saturating_add(1);
        }
        last_pidmax = max_pid;

        if plateau >= STALL_BEATS && !dumped && n_tasks >= MIN_TASKS && t_secs >= MIN_BOOT_SECS {
            dumped = true;
            wedge_census(
                t_secs,
                max_pid,
                plateau as u64 * BEAT_SECS,
            );
        }
    }

    RUNNING.store(false, Ordering::Release);
    0
}

/// The one-shot wedge census (fires after the wedge has settled, so its
/// output burn cannot decide the race it reports on).
fn wedge_census(t_secs: u64, pidmax: u32, plateau_secs: u64) {
    {
        let mut w = ConsoleWriter::new();
        let _ = writeln!(
            w,
            "SC-WEDGE t={}s pidmax={} plateau={}s census begin",
            t_secs, pidmax, plateau_secs
        );
    }

    // (1) Every binder read-wait thread regardless of age. stall_report
    // only prints waiters older than 120s; at plateau+60s the wedge's
    // waiters are younger than that.
    {
        let mut s = alloc::string::String::new();
        crate::ipc::binder::schb_waiter_census(&mut s);
        let mut w = ConsoleWriter::new();
        let _ = write!(w, "{}", s);
    }

    // (2) Scheduler view of every runnable (not on_cpu) and D-state task.
    // Collect raw fields under the pid-hash walk; re-find each task for
    // the runqueue-linked diagnostic afterwards (never take a GRQ
    // diagnostic lock under a pid-hash bucket lock).
    struct Snap {
        pid: u32,
        comm: [u8; 16],
        bits: u32,
        on_cpu: bool,
        claim: i32,
        pk: u32,
        ti_cpu: i32,
    }
    let mut snaps: alloc::vec::Vec<Snap> = alloc::vec::Vec::new();
    crate::sched::for_each_task(|task| {
        // SAFETY: plain-field reads under the bucket lock (khungtaskd
        // contract).
        unsafe {
            let t = &*task;
            let b = t.state().bits();
            if (b & 0x37 == 0 && !t.on_cpu()) || (b & 0x02 != 0) {
                snaps.push(Snap {
                    pid: t.pid(),
                    comm: *t.comm(),
                    bits: b,
                    on_cpu: t.on_cpu(),
                    claim: t.running_on_cpu.load(Ordering::Acquire),
                    pk: t.pick_state(),
                    ti_cpu: t.ti_cpu(),
                });
            }
        }
    });
    {
        let mut s = alloc::string::String::new();
        for sn in &snaps {
            // SAFETY: diagnostic re-find; the pointer is only read for
            // the linked query and may be gone (exited between walk and
            // now) — find returns null then.
            let linked = unsafe { crate::sched::find_task_by_pid(sn.pid) };
            let linked_str = if linked.is_null() {
                "gone"
            } else {
                match crate::sched::sched::grq_diag_cfs_linked(linked) {
                    Some(true) => "1",
                    Some(false) => "0",
                    None => "n/a",
                }
            };
            let _ = writeln!(
                s,
                "SC-WEDGE-{} pid={} comm={} state={:#x} oncpu={} claim={} pk={} ti_cpu={} linked={}",
                if sn.bits & 0x02 != 0 { "D" } else { "R" },
                sn.pid,
                comm_str(&sn.comm),
                sn.bits,
                sn.on_cpu,
                sn.claim,
                sn.pk,
                sn.ti_cpu,
                linked_str
            );
        }
        let mut w = ConsoleWriter::new();
        let _ = write!(w, "{}", s);
    }

    // (3) Kernel stall machinery at warn level: lift the console loglevel
    // to 4 so binder-stall / freeze_scan / WAKE-RUN-LEAK print (at
    // loglevel=3 their records are dropped before the ring — see the
    // module doc), run one stall_report pass, restore verbatim. The pass
    // also re-kicks parked readers (idempotent heal khungtaskd already
    // runs every 60s); if that unwedges the system the next heartbeats
    // show the flow resuming — itself a diagnostic.
    {
        let mut w = ConsoleWriter::new();
        let _ = writeln!(w, "SC-WEDGE stall_report + freeze_scan (loglevel lifted to 4):");
    }
    let saved = crate::printk::get_console_loglevel();
    if saved < 4 {
        crate::printk::set_console_loglevel(4);
    }
    crate::ipc::binder::stall_report();
    // dfx=sleepsnap names the syscall every S-state task is parked in —
    // the wedge census's blind spot is a server parked OUTSIDE binder
    // (samgr in sc201: not runnable, not binder-waiting, clients owed
    // 200s+). sleep_snapshot is self-gated on its own dfx switch; boots
    // that arm dfx=schb,sleepsnap get its census inside this lifted
    // window (its khungtaskd-driven prints at loglevel 3 are dropped
    // pre-ring, so the 60s cadence costs no UART burn).
    crate::ipc::binder::sleep_snapshot();
    if saved < 4 {
        crate::printk::set_console_loglevel(saved);
    }

    // (4) Userspace tail: quietcon diverts the /dev/console stream into
    // the printk ring; the newest records around the wedge are the
    // userspace last-known-good boundary.
    {
        let tail = crate::printk::ring_tail(RING_TAIL);
        let mut w = ConsoleWriter::new();
        let _ = writeln!(w, "SC-WEDGE printk ring tail ({} records, oldest first):", RING_TAIL);
        let _ = write!(w, "{}", tail);
        let _ = writeln!(w, "SC-WEDGE census end");
    }
}
