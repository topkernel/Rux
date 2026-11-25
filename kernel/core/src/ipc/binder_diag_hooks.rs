//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! BINDER DIAGNOSTIC SEAM (cold-four lane): the hook rows the FRAMEWORK
//! uses to reach the binder cold diagnostic halves that moved to the
//! ipc crate (src/ipc/binder_diag.rs). Every framework call site
//! routed here is measured cold:
//! - the timer softirq's ~1 Hz heartbeat slot (txn_hang_audit /
//!   bwr_park_audit / ams46_audit — self-gated + rate-capped inside,
//!   the slot itself CAS-gated to once per second);
//! - taskdump / hung_task / schb (sleep_snapshot, stall_report,
//!   schb_beat, schb_waiter_census, dump_latched_maps_taskctx) —
//!   demand-driven dumps;
//! - rux-obs and syscall reach the moved functions DIRECTLY (the
//!   allowed downward service edge; no row needed).
//! The unsafe walkers the moved halves consume stay framework-side in
//! binder_edges.rs (the audited zone).
//!
//! Registration runs from rust_main via ipc::register_all —
//! OnceSlot-only (no Spinlock, no per-CPU state; the x86_64 unit-test
//! kernel lesson).

#![forbid(unsafe_code)]
/// The binder diagnostic rows. Signatures carry the native types
/// (crate-plan §3a rule 3): no adapter logic.
#[derive(Clone, Copy)]
pub struct BinderDiagHooks {
    /// rux:txnhunt: the in-flight txn hang audit (1 Hz slot, self-gated).
    pub txn_hang_audit: fn(),
    /// rux:txnhunt: the BWR-park holder-chain audit (1 Hz slot, self-gated).
    pub bwr_park_audit: fn(),
    /// rux:ams46: the latched AMS node census (1 Hz slot, self-gated).
    pub ams46_audit: fn(),
    /// rux:sleepsnap: the parked-task sleep census (taskdump/hung_task/schb).
    pub sleep_snapshot: fn(),
    /// The schb stall report (hung_task/schb).
    pub stall_report: fn(),
    /// The scheduler-beat binder counters (schb).
    pub schb_beat: fn() -> (usize, usize, usize),
    /// The schb waiter census append (schb).
    pub schb_waiter_census: fn(&mut alloc::string::String),
    /// The latched-maps vma dump (taskdump).
    pub dump_latched_maps_taskctx: fn(),
}

impl BinderDiagHooks {
    pub const EMPTY: BinderDiagHooks = BinderDiagHooks {
        txn_hang_audit: || {},
        bwr_park_audit: || {},
        ams46_audit: || {},
        sleep_snapshot: || {},
        stall_report: || {},
        schb_beat: || (0, 0, 0),
        schb_waiter_census: |_| {},
        dump_latched_maps_taskctx: || {},
    };
}

pub static BINDER_DIAG_HOOKS: crate::safety::slot::OnceSlot<BinderDiagHooks> =
    crate::safety::slot::OnceSlot::new();

pub fn binder_diag_register_hooks(hooks: BinderDiagHooks) {
    let _ = BINDER_DIAG_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> &'static BinderDiagHooks {
    BINDER_DIAG_HOOKS.get().unwrap_or(&BinderDiagHooks::EMPTY)
}

#[inline]
pub fn txn_hang_audit() { (hooks().txn_hang_audit)() }
#[inline]
pub fn bwr_park_audit() { (hooks().bwr_park_audit)() }
#[inline]
pub fn ams46_audit() { (hooks().ams46_audit)() }
#[inline]
pub fn sleep_snapshot() { (hooks().sleep_snapshot)() }
#[inline]
pub fn stall_report() { (hooks().stall_report)() }
#[inline]
pub fn schb_beat() -> (usize, usize, usize) { (hooks().schb_beat)() }
#[inline]
pub fn schb_waiter_census(out: &mut alloc::string::String) {
    (hooks().schb_waiter_census)(out)
}
#[inline]
pub fn dump_latched_maps_taskctx() { (hooks().dump_latched_maps_taskctx)() }
