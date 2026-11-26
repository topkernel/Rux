//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! SIGNAL POLICY SEAM (crate-flip, the signal rung): the framework's
//! signal entry points that are POLICY — the send-path queueing core
//! (coalescing, sig_ignored ordering, RLIMIT_SIGPENDING, wake decisions),
//! the default-action table, the pid-namespace init delivery filter, the
//! exec/fork handler-reset rules, the /proc SigIgn/SigCgt pair, and the
//! user-originated sender-stamping entries — dispatch through THIS
//! nullable table. Until the carve lands in the `signal` crate, the
//! framework registers its own in-tree policy (fn pointers, behavior
//! identical); the crate's rows replace them (OnceSlot first-wins).
//!
//! HOT-PATH SHAPE (measured before this table was drawn, per crate-plan
//! §3a rule 2): the per-CHECK path (check_and_deliver_signals on every
//! syscall exit / interrupt return) makes ZERO calls through this table —
//! the pending test and first_unmasked stay direct methods on the
//! framework's SigPending. Rows fire only per DELIVERED signal
//! (default_action_class on the Default arm, init_delivery_filter for
//! pid-1/ns-1 tasks) and on the send syscalls — one indirect call per
//! kill(2) — all cold relative to the frame build that follows.
//!
//! THE SELF-RECURSION RULE (the S1 lesson, binding): a registered row
//! body must NEVER forward back into the accessor that dispatches the
//! same slot. The moved bodies call the framework's MECHANISM fns
//! directly (signal_wake_up, signalfd_notify_task, the pinned lookups
//! stay plain framework fns, not accessors of this table).

#![forbid(unsafe_code)]
use crate::process::types::Task;
use crate::signal::{SigInfo, SignalStruct};

// ==================== The seam vocabulary ====================

/// The default-action class for a signal number (the policy half of the
/// old handle_default_signal match): WHICH class of default behavior a
/// disposition-less signal gets. The executor (mechanism: state changes,
/// parent notification, do_exit_group) stays framework-side and matches
/// on this — the table itself is policy and lives in the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultActionClass {
    /// Default ignore (SIGCHLD/SIGURG/SIGWINCH and unknown numbers)
    Ignore,
    /// SIGCONT: resume a job-control stop + CLD_CONTINUED notify
    Continue,
    /// SIGSTOP/SIGTSTP/SIGTTIN/SIGTTOU: group stop + CLD_STOPPED notify
    GroupStop,
    /// Terminate the whole thread group (core or plain)
    Terminate,
}

/// The signal-policy hook set (one row per framework->policy entry
/// point; the reverse surface measured before the carve: 11 rows cover
/// every framework->policy edge, all cold).
#[derive(Clone, Copy)]
pub struct SignalHooks {
    /// The send-path queueing core (was send_signal_locked_info):
    /// legacy coalescing, sig_ignored ordering (blocked-never-ignored,
    /// traced-task exemption), RLIMIT_SIGPENDING, the wake decisions.
    pub send_signal_core: fn(&Task, i32, Option<SigInfo>, i32) -> Result<(), i32>,
    /// User-originated kill(2) (SI_USER sender stamping).
    pub send_from_user: fn(u32, i32) -> Result<(), i32>,
    /// tkill/tgkill (SI_TKILL sender stamping; glibc raise()).
    pub send_from_user_tkill: fn(u32, i32) -> Result<(), i32>,
    /// rt_sigqueueinfo family entry (user si_code).
    pub send_info: fn(u32, i32, i32) -> Result<(), i32>,
    /// Process-group broadcast (tty ISIG) with kill permission checks.
    pub send_to_pgid: fn(u32, i32),
    /// force_sig parity: unblock a synchronous signal on the faulting
    /// task (blocked-SIGSEGV livelock fix).
    pub force_unblock_current: fn(i32),
    /// The default-action table: signal number -> behavior class.
    pub default_action_class: fn(i32) -> DefaultActionClass,
    /// pid-namespace init delivery filter (SIGNAL_UNKILLABLE discard
    /// decision; the framework keeps the is_init predicate itself).
    pub init_delivery_filter: fn(sig: i32, handled: bool, code: i32, pid: i32) -> bool,
    /// POSIX execve handler reset (caught -> SIG_DFL, SIG_IGN survives
    /// unless forced; SIGKILL/SIGSTOP untouched).
    pub flush_handlers: fn(&SignalStruct, bool),
    /// CLONE_CLEAR_SIGHAND (posix_spawn): reset non-SIG_IGN to SIG_DFL.
    pub reset_all_handlers_default: fn(&SignalStruct),
    /// /proc/[pid]/status SigIgn/SigCgt mask pair.
    pub ign_cgt_masks: fn(&SignalStruct) -> (u64, u64),
}

/// An unfilled row is a kernel misbuild, not a degraded mode: kill(2)
/// and every fatal-signal path route through this table from the first
/// user process. Name it loudly instead of failing softly.
impl SignalHooks {
    pub const EMPTY: SignalHooks = SignalHooks {
        send_signal_core: no_policy_send_core,
        send_from_user: no_policy_send_from_user,
        send_from_user_tkill: no_policy_send_from_user_tkill,
        send_info: no_policy_send_info,
        send_to_pgid: no_policy_send_to_pgid,
        force_unblock_current: no_policy_force_unblock,
        default_action_class: no_policy_default_action_class,
        init_delivery_filter: no_policy_init_filter,
        flush_handlers: no_policy_flush_handlers,
        reset_all_handlers_default: no_policy_reset_all,
        ign_cgt_masks: no_policy_ign_cgt_masks,
    };
}

fn no_policy_send_core(_: &Task, _: i32, _: Option<SigInfo>, _: i32) -> Result<(), i32> {
    panic!("signal seam: policy not registered (send core)")
}
fn no_policy_send_from_user(_: u32, _: i32) -> Result<(), i32> {
    panic!("signal seam: policy not registered (send_from_user)")
}
fn no_policy_send_from_user_tkill(_: u32, _: i32) -> Result<(), i32> {
    panic!("signal seam: policy not registered (send_from_user_tkill)")
}
fn no_policy_send_info(_: u32, _: i32, _: i32) -> Result<(), i32> {
    panic!("signal seam: policy not registered (send_info)")
}
fn no_policy_send_to_pgid(_: u32, _: i32) {
    panic!("signal seam: policy not registered (send_to_pgid)")
}
fn no_policy_force_unblock(_: i32) {
    panic!("signal seam: policy not registered (force_unblock_current)")
}
fn no_policy_default_action_class(_: i32) -> DefaultActionClass {
    panic!("signal seam: policy not registered (default_action_class)")
}
fn no_policy_init_filter(_: i32, _: bool, _: i32, _: i32) -> bool {
    panic!("signal seam: policy not registered (init_delivery_filter)")
}
fn no_policy_flush_handlers(_: &SignalStruct, _: bool) {
    panic!("signal seam: policy not registered (flush_handlers)")
}
fn no_policy_reset_all(_: &SignalStruct) {
    panic!("signal seam: policy not registered (reset_all_handlers_default)")
}
fn no_policy_ign_cgt_masks(_: &SignalStruct) -> (u64, u64) {
    panic!("signal seam: policy not registered (ign_cgt_masks)")
}

pub static SIGNAL_HOOKS: crate::safety::slot::OnceSlot<SignalHooks> =
    crate::safety::slot::OnceSlot::new();

/// The service crate's registration entry (idempotent: first wins —
/// the composition bin's `signal::register_all` runs before any
/// framework use, so the crate's rows prevail over any later in-tree
/// registration attempt). Publication is a OnceSlot store (plain
/// atomics, no lock): registration runs from rust_main, before
/// arch::smp::init() establishes the per-CPU area, and every Spinlock
/// in this kernel reads per-CPU state on acquisition.
pub fn signal_register_hooks(hooks: SignalHooks) {
    let _ = SIGNAL_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> &'static SignalHooks {
    SIGNAL_HOOKS.get().unwrap_or(&SignalHooks::EMPTY)
}

// ---- accessors: the framework's only door into signal policy ----

/// The send-path queueing core. Called by the framework's pinned-lookup
/// wrappers (send_signal_g / send_signal_with_info_g) — one indirect
/// call per kill(2)-class send, on the syscall path.
#[inline]
pub fn send_signal_core(
    task: &Task,
    sig: i32,
    si: Option<SigInfo>,
    group: i32,
) -> Result<(), i32> {
    (hooks().send_signal_core)(task, sig, si, group)
}

#[inline]
pub fn send_from_user(pid: u32, sig: i32) -> Result<(), i32> {
    (hooks().send_from_user)(pid, sig)
}

#[inline]
pub fn send_from_user_tkill(tid: u32, sig: i32) -> Result<(), i32> {
    (hooks().send_from_user_tkill)(tid, sig)
}

#[inline]
pub fn send_info(pid: u32, sig: i32, si_code: i32) -> Result<(), i32> {
    (hooks().send_info)(pid, sig, si_code)
}

#[inline]
pub fn send_to_pgid(pgid: u32, sig: i32) {
    (hooks().send_to_pgid)(pgid, sig)
}

#[inline]
pub fn force_unblock_current(sig: i32) {
    (hooks().force_unblock_current)(sig)
}

/// Default-action table lookup — fires once per DELIVERED signal whose
/// disposition is SIG_DFL (never on the pending check itself).
#[inline]
pub fn default_action_class(sig: i32) -> DefaultActionClass {
    (hooks().default_action_class)(sig)
}

/// pid-ns init delivery filter — fires only for tasks where the
/// framework-side is_unkillable_init predicate already matched.
#[inline]
pub fn init_delivery_filter(sig: i32, handled: bool, code: i32, pid: i32) -> bool {
    (hooks().init_delivery_filter)(sig, handled, code, pid)
}

#[inline]
pub fn flush_handlers(ss: &SignalStruct, force_default: bool) {
    (hooks().flush_handlers)(ss, force_default)
}

#[inline]
pub fn reset_all_handlers_default(ss: &SignalStruct) {
    (hooks().reset_all_handlers_default)(ss)
}

#[inline]
pub fn ign_cgt_masks(ss: &SignalStruct) -> (u64, u64) {
    (hooks().ign_cgt_masks)(ss)
}
