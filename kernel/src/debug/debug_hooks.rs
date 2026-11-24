//! Framework-side observability hook table (crate-flip rung: rux-obs):
//! every framework-side consumer of the MOVABLE observation tools — the
//! syscost/fsw/mmprof report halves, the schbd heartbeat kthread, the
//! reaper print arms, the driftwatch/filerd report readers, the
//! softlockup lockup print, the lostpark ring dump, the spinwatch
//! compat arm, and the tracefs file publisher — resolves through these
//! rows (the IpcHooks/NetHooks pattern: nullable-default fns, no-op
//! until the service crate registers; the composition bin calls
//! rux_obs::register_all before rust_main_impl).
//!
//! Row discipline (crate-plan §3a): every row here is COLD — boot-time
//! arming, shutdown/procfs readers, cadence dumps (>= 1 Hz), or an
//! ARM-GATED event print whose call site already checked the probe's
//! own switch first (the reaper rows fire only on
//! EVENT_SCHEDTRACE-armed boots; the switch load, not the row, is the
//! per-event cost). The per-event RECORD fns (syscost enter/exit/tick,
//! fsw note_*, mmprof add/bump, allocwatch guard, softlockup
//! check/touch, cctx/forkprof/unixrd markers) stay framework-side as
//! direct calls — they are NOT behind this table (the S1 lesson: an
//! indirect call per event cost p2_ctxsw +25.1% until devirtualized).
//! The 18-row width is justified by that split: this table is exactly
//! the cold report surface measured for the flip (one row per
//! framework entry point into the moved code), not a convenience
//! mega-table.
//!
//! THE SELF-RECURSION RULE (binding, the MmHooks wording): a
//! registered row body must NEVER forward back into the accessor or
//! wrapper that dispatches the same slot — registered bodies are the
//! REAL report fns in the crate, and call sites read the table only
//! through these accessors / the historical-name wrappers in the
//! framework modules.

use alloc::vec::Vec;

/// The observability hook set (one row per cold framework entry point
/// into the rux-obs crate's report/init/publish surface).
#[derive(Clone, Copy)]
pub struct ObsHooks {
    // ---- periodic / shutdown reports ----
    /// syscost's 60 s cadence dump (called from framework syscost::tick
    /// once per dump window — the tick body itself stays direct).
    pub syscost_dump: fn(),
    /// fsw's offset-cadence dump + the shutdown cascade hook.
    pub fsw_dump: fn(),
    /// mmprof's rotating report line (shutdown calls variant 0).
    pub mmprof_report_line: fn(u64),

    // ---- boot-time init / arming ----
    /// Start the schbd heartbeat kthread (rux:schb-gated inside).
    pub schb_init: fn(),
    /// Spin the mmprofd dump kthread (idempotent).
    pub mmprof_ensure_dump_thread: fn(),
    /// The old `spinwatch=1` cmdline compat alias (arms trace events).
    pub spinwatch_compat_arm: fn(),
    /// Publish the tracefs control files.
    pub trace_files_register: fn(),
    /// Apply the `trace_event=` boot parameter.
    pub trace_files_init_from_cmdline: fn(),

    // ---- report readers ----
    /// driftwatch's per-site counter lines (ledger context).
    pub driftwatch_report: fn(),
    /// /proc/filerd content.
    pub filerd_generate: fn() -> Vec<u8>,
    /// The one-shot softlockup lockup report (fires at most once per
    /// CPU per occurrence from the timer-interrupt check).
    pub softlockup_report: fn(usize, u64, u64),
    /// The one-shot lostpark ring dump (wakewatch frozen branch).
    pub lostpark_dump_once: fn(&[u8]),

    // ---- one-shot diagnostics ----
    /// The allocator anatomy at the fake-OOM floor (mmprof family's
    /// sibling; called from the OOM killer's no-killable path).
    pub schb_oom_anat_dump: fn(),

    // ---- reaper print arms (call sites pre-gate on reaper::on()) ----
    pub reaper_note_nameset: fn(u32, &[u8]),
    pub reaper_on_zombie: fn(u32, i32, u32, &[u8; 16], u32, u64),
    pub reaper_on_reap_wait4: fn(u32, u32, i32, bool),
    pub reaper_on_reap_waitid: fn(u32, u32, i32, i32, i32, i32, i32, bool, bool),
    pub reaper_poll_note: fn(u32, i32, i32, i32),
}

impl ObsHooks {
    /// Unregistered rows are silent no-ops (an uncomposed kernel prints
    /// nothing; the composed bin always registers the real bodies).
    pub const EMPTY: ObsHooks = ObsHooks {
        syscost_dump: || {},
        fsw_dump: || {},
        mmprof_report_line: |_| {},
        schb_init: || {},
        mmprof_ensure_dump_thread: || {},
        spinwatch_compat_arm: || {},
        trace_files_register: || {},
        trace_files_init_from_cmdline: || {},
        driftwatch_report: || {},
        filerd_generate: || Vec::new(),
        softlockup_report: |_, _, _| {},
        lostpark_dump_once: |_| {},
        schb_oom_anat_dump: || {},
        reaper_note_nameset: |_, _| {},
        reaper_on_zombie: |_, _, _, _, _, _| {},
        reaper_on_reap_wait4: |_, _, _, _| {},
        reaper_on_reap_waitid: |_, _, _, _, _, _, _, _, _| {},
        reaper_poll_note: |_, _, _, _| {},
    };
}

pub static OBS_HOOKS: crate::safety::slot::OnceSlot<ObsHooks> =
    crate::safety::slot::OnceSlot::new();

/// Registration entry (called by the rux-obs service crate at boot,
/// from rust_main — OnceSlot publication only, no locks, no per-CPU
/// state; INV-SVCTEST-1's pre-SMP discipline).
pub fn obs_register_hooks(hooks: ObsHooks) {
    let _ = OBS_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> ObsHooks {
    *OBS_HOOKS.get().unwrap_or(&ObsHooks::EMPTY)
}

// The lib-side call surface (one wrapper per row; the framework call
// sites and the historical-name wrappers in the debug modules name
// these, never the table).

#[inline]
pub fn syscost_dump() {
    (hooks().syscost_dump)()
}
#[inline]
pub fn fsw_dump() {
    (hooks().fsw_dump)()
}
#[inline]
pub fn mmprof_report_line(k: u64) {
    (hooks().mmprof_report_line)(k)
}
#[inline]
pub fn schb_init() {
    (hooks().schb_init)()
}
#[inline]
pub fn mmprof_ensure_dump_thread() {
    (hooks().mmprof_ensure_dump_thread)()
}
#[inline]
pub fn spinwatch_compat_arm() {
    (hooks().spinwatch_compat_arm)()
}
#[inline]
pub fn trace_files_register() {
    (hooks().trace_files_register)()
}
#[inline]
pub fn trace_files_init_from_cmdline() {
    (hooks().trace_files_init_from_cmdline)()
}
#[inline]
pub fn driftwatch_report() {
    (hooks().driftwatch_report)()
}
#[inline]
pub fn filerd_generate() -> Vec<u8> {
    (hooks().filerd_generate)()
}
#[inline]
pub fn softlockup_report(cpu: usize, elapsed_secs: u64, timer_cnt: u64) {
    (hooks().softlockup_report)(cpu, elapsed_secs, timer_cnt)
}
#[inline]
pub fn lostpark_dump_once(tag: &[u8]) {
    (hooks().lostpark_dump_once)(tag)
}
#[inline]
pub fn schb_oom_anat_dump() {
    (hooks().schb_oom_anat_dump)()
}
#[inline]
pub fn reaper_note_nameset(pid: u32, comm: &[u8]) {
    (hooks().reaper_note_nameset)(pid, comm)
}
#[inline]
pub fn reaper_on_zombie(
    pid: u32,
    code: i32,
    parent_pid: u32,
    comm: &[u8; 16],
    last_nr: u32,
    last_a0: u64,
) {
    (hooks().reaper_on_zombie)(pid, code, parent_pid, comm, last_nr, last_a0)
}
#[inline]
pub fn reaper_on_reap_wait4(reaper: u32, child: u32, wstatus: i32, nohang: bool) {
    (hooks().reaper_on_reap_wait4)(reaper, child, wstatus, nohang)
}
#[inline]
pub fn reaper_on_reap_waitid(
    reaper: u32,
    si_pid: u32,
    si_code: i32,
    si_status: i32,
    idtype: i32,
    id: i32,
    options: i32,
    wnowait: bool,
    via_pidfd: bool,
) {
    (hooks().reaper_on_reap_waitid)(
        reaper,
        si_pid,
        si_code,
        si_status,
        idtype,
        id,
        options,
        wnowait,
        via_pidfd,
    )
}
#[inline]
pub fn reaper_poll_note(reaper: u32, idtype: i32, id: i32, options: i32) {
    (hooks().reaper_poll_note)(reaper, idtype, id, options)
}
