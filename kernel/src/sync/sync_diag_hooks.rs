//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! SYNC DIAGNOSTIC SEAM (sync-upper lane): the hook rows the FRAMEWORK
//! uses to reach the futex cold diagnostic family that moved to the
//! sync crate (src/sync/futex_diag.rs). Every framework call site
//! routed here is measured cold:
//! - the do_futex / futex_stuck_census latch sections — inside
//!   trace-event armed() gates and the census's own 5 s rate cap
//!   (the S1 arm-gated-row pattern; disarmed boots never touch the
//!   table);
//! - exit / pid_hash (rwfz_exit_watch_tgid, rwfrz_tgid_frozen,
//!   note_task_unlink) — thread-exit forensics;
//! - signal_policy / task (kill_dump_tgid) — the SIGKILL wedge dump;
//! - taskdump / schb / debug switches (sqwalk, ffrt heap map,
//!   freeze_scan, fuxwatch cmdline parse) — demand-driven dumps;
//! - binder.rs's hdf_devmgr latch (fxdv_latch_devmgr).
//! Sibling service crates (ipc binder_diag, process) reach the
//! moved functions DIRECTLY at their historical crate::sync::futex::X
//! paths (the union module in sync's lib.rs — the allowed downward
//! service edge; no row needed).
//! The futex MECHANISM, futex_stuck_census itself, and the fuxwatch
//! watchpoint machinery the x86 trap paths call by name stay
//! framework-side in futex.rs (the lane's stay verdicts).
//!
//! Registration runs from rust_main via sync::register_all —
//! OnceSlot-only (no Spinlock, no per-CPU state; the x86_64 unit-test
//! kernel lesson).

/// The sync diagnostic rows. Signatures carry the native types
/// (crate-plan §3a rule 3): no adapter logic.
#[derive(Clone, Copy)]
pub struct SyncDiagHooks {
    /// sc184: latch the hdf_devmgr tgid for the FXDV tracer (binder.rs
    /// SA-5100 site + do_futex's name-latch backstop, armed-gated).
    pub fxdv_latch_devmgr: fn(u32),
    /// rwlock-livelock: the exit-watch tgid the do_futex latch set
    /// (process exit path).
    pub rwfz_exit_watch_tgid: fn() -> u32,
    /// Whether this tgid owns the frozen word (process exit path).
    pub rwfrz_tgid_frozen: fn(u32) -> bool,
    /// The thread-exit ring append (pid_hash unlink, under bucket lock).
    pub note_task_unlink: fn(u32, u32),
    /// The SIGKILL-time wedge dump (signal_policy, process/task).
    pub kill_dump_tgid: fn(u32),
    /// Resume the sqwalk single-word walk (taskdump).
    pub sqwalk_resume: fn(),
    /// The ffrt mutex heap-map poll (taskdump).
    pub ffrt_mutex_heap_map_done: fn() -> bool,
    /// The ffrt mutex heap-map re-arm (taskdump).
    pub ffrt_mutex_heap_map_rearm: fn(),
    /// The ffrt mutex heap map walk (taskdump).
    pub ffrt_mutex_heap_map: fn(u32, u64) -> alloc::string::String,
    /// fuxwatch=0xADDR[,DELAY] boot cmdline parse (debug switches).
    pub fuxwatch_parse: fn(),
    /// fuxwatch2=0xADDR boot cmdline parse (debug switches).
    pub fuxwatch2_parse: fn(),
    /// The malloc-lock holder hunt (do_futex CTDE + census latch arms).
    pub malloc_holder_scan: fn(u32, u64),
    /// The CoTask saved-stack decoder (do_futex CTDE + census adoptions).
    pub ctd_decode: fn(u32, u64, &str),
    /// The first-writer park latch (do_futex CTDE arm).
    pub fuxpark_latch: fn(u64, u32, u32),
    /// The frozen-word reader census (census adoption).
    pub rwfrz_reader_scan: fn(u32, u64),
}

impl SyncDiagHooks {
    pub const EMPTY: SyncDiagHooks = SyncDiagHooks {
        fxdv_latch_devmgr: |_| {},
        rwfz_exit_watch_tgid: || 0,
        rwfrz_tgid_frozen: |_| false,
        note_task_unlink: |_, _| {},
        kill_dump_tgid: |_| {},
        sqwalk_resume: || {},
        ffrt_mutex_heap_map_done: || false,
        ffrt_mutex_heap_map_rearm: || {},
        ffrt_mutex_heap_map: |_, _| alloc::string::String::new(),
        fuxwatch_parse: || {},
        fuxwatch2_parse: || {},
        malloc_holder_scan: |_, _| {},
        ctd_decode: |_, _, _| {},
        fuxpark_latch: |_, _, _| {},
        rwfrz_reader_scan: |_, _| {},
    };
}

pub static SYNC_DIAG_HOOKS: crate::safety::slot::OnceSlot<SyncDiagHooks> =
    crate::safety::slot::OnceSlot::new();

pub fn sync_diag_register_hooks(hooks: SyncDiagHooks) {
    let _ = SYNC_DIAG_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> &'static SyncDiagHooks {
    SYNC_DIAG_HOOKS.get().unwrap_or(&SyncDiagHooks::EMPTY)
}

#[inline]
pub fn fxdv_latch_devmgr(pid: u32) { (hooks().fxdv_latch_devmgr)(pid) }
#[inline]
pub fn rwfz_exit_watch_tgid() -> u32 { (hooks().rwfz_exit_watch_tgid)() }
#[inline]
pub fn rwfrz_tgid_frozen(tgid: u32) -> bool { (hooks().rwfrz_tgid_frozen)(tgid) }
#[inline]
pub fn note_task_unlink(pid: u32, tgid: u32) { (hooks().note_task_unlink)(pid, tgid) }
#[inline]
pub fn kill_dump_tgid(tgid: u32) { (hooks().kill_dump_tgid)(tgid) }
#[inline]
pub fn sqwalk_resume() { (hooks().sqwalk_resume)() }
#[inline]
pub fn ffrt_mutex_heap_map_done() -> bool { (hooks().ffrt_mutex_heap_map_done)() }
#[inline]
pub fn ffrt_mutex_heap_map_rearm() { (hooks().ffrt_mutex_heap_map_rearm)() }
#[inline]
pub fn ffrt_mutex_heap_map(tgid: u32, known_lock: u64) -> alloc::string::String {
    (hooks().ffrt_mutex_heap_map)(tgid, known_lock)
}
#[inline]
pub fn fuxwatch_parse() { (hooks().fuxwatch_parse)() }
#[inline]
pub fn fuxwatch2_parse() { (hooks().fuxwatch2_parse)() }
#[inline]
pub fn malloc_holder_scan(tgid: u32, word: u64) { (hooks().malloc_holder_scan)(tgid, word) }
#[inline]
pub fn ctd_decode(tgid: u32, word: u64, tag: &str) { (hooks().ctd_decode)(tgid, word, tag) }
#[inline]
pub fn fuxpark_latch(word: u64, tid: u32, tgid: u32) { (hooks().fuxpark_latch)(word, tid, tgid) }
#[inline]
pub fn rwfrz_reader_scan(tgid: u32, word: u64) { (hooks().rwfrz_reader_scan)(tgid, word) }
