//! The IPC SERVICE CRATE (crate-flip rung: ipc): the movable ipc
//! policy — System V semaphores/message queues/shared memory, POSIX
//! message queues, the kernel key retention service and the
//! access_token registry — under a crate-level unsafe-deny boundary.
//! The framework keeps ipc/binder.rs (the rung's verdict: its
//! block-view windows, Arc adoption and diagnostic walkers are
//! floor-shaped), ipc/ipc_hooks.rs (the 26-row seam), ipc/edges.rs
//! (INV-IPC-PAGEOP-1) and the SemUndoEntry type substrate.
//!
//! Sole-copy state (the rung-1 recipe's final step): the service files
//! LIVE in this crate (src/), flat under its root; the ipc guest-test
//! followed the files.

// FORBID since the flip: the eleven unsafe blocks the files carried
// were converted in the pre-flip commit — raw user copies ride the
// uaccess_safe typed twins, the exit cleanups take live &Task
// references, and the two vmemmap page-refcount touches ride the
// framework's ipc/edges.rs (INV-IPC-PAGEOP-1).
#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the service files' crate::X paths resolve unchanged (the net recipe).
pub use rux::{
    arch, cmdline, config, console, debug, drivers, errno, fs, interrupt, mm,
    process, safety, sched, security, signal, syscall, timer, trace,
};
// The framework's sync module plus the moved futex diagnostics, merged
// at the sync crate's root (crate::sync::futex::X paths).
pub use ::sync;
pub use rux::net;
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

// The service files (THIS crate's own — the sole copies), flat at the
// crate root (seed-core-design 2c.2).
pub mod sysv_msg;
pub mod sysv_shm;
pub mod posix_mq;
pub mod keys;
pub mod access_token;
// the binder COLD DIAGNOSTIC halves (cold-four lane; the txn
// mechanism stays framework-side in rux::ipc::binder — the rung's
// shelving verdict stands).
pub mod binder_diag;

// The old framework mod.rs's `pub use util::*` — the IPC constants
// (IPC_NOWAIT, SHM_REMAP, ...) re-exported at the ipc root so the
// service files' `super::X` paths resolve (the historical shape).
pub use rux::ipc::util::*;

// binder, sysv_sem and util stay FRAMEWORK-side (the rung's
// verdicts — see the shelving records); re-exported so
// `crate::ipc::X` paths keep resolving (syscall's dispatch
// rows, devfs, futex, the debug audits, the sibling service files'
// `super::util` uses).
pub use rux::ipc::binder;
pub use rux::ipc::binder_edges;
pub use rux::ipc::sysv_sem;
pub use rux::ipc::util;
// the seam zones the service files consume (the hook table's
// KeyQuota type; the audited page-refcount edges).
pub use rux::ipc::ipc_hooks;
pub use rux::ipc::edges;
// the type substrate that stayed with Task's sem_undo field.
pub use rux::ipc::SemUndoEntry;

// the guest-test module that followed the files (the framework's
// tests/mod.rs rows are gated with the move note).
// Service-internal test lives CENTRALLY in kernel/core/src/tests/services/.

// The framework's test counters + assert macros, re-exported at the
// crate root where the test module's imports expect them (the
// service-test seam contract: same pass/fail counters as the
// in-framework suite).
#[cfg(feature = "unit-test")]
pub use rux::tests::{test_pass, test_fail, test_skip, test_group_start};
#[cfg(feature = "unit-test")]
pub use rux::test_assert_eq;
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/ipc_sysv.rs"]
pub mod ipc_sysv;

/// The boot-time registration (bin calls this before kmain): the
/// IpcHooks table's rows. THE composition contract.
pub fn register_all() {
    rux::ipc::ipc_hooks::ipc_register_hooks(rux::ipc::ipc_hooks::IpcHooks {
        mq_fds_cleanup: posix_mq::mq_fds_cleanup,
        exec_clear_thread_keyring: keys::exec_clear_thread_keyring,
        at_registered_token_of: access_token::registered_token_of,
        at_mirror_set: access_token::mirror_set,
        msgmni: sysv_msg::msgmni,
        set_msgmni: sysv_msg::set_msgmni,
        shm_ctlmax: sysv_shm::shm_ctlmax,
        set_shm_ctlmax: sysv_shm::set_shm_ctlmax,
        shm_ctlni: sysv_shm::shm_ctlni,
        set_shm_ctlni: sysv_shm::set_shm_ctlni,
        shm_ctlall: sysv_shm::shm_ctlall,
        set_shm_ctlall: sysv_shm::set_shm_ctlall,
        ipc_ids_max: util::ipc_ids_max,
        queues_max: posix_mq::queues_max,
        set_queues_max: posix_mq::set_queues_max,
        key_quota: keys::key_quota,
        set_key_quota: keys::set_key_quota,
        msg_proc_show: sysv_msg::proc_show,
        shm_proc_show: sysv_shm::proc_show,
        shm_attach_vma: sysv_shm::shm_attach_vma,
        shm_detach_vma: sysv_shm::shm_detach_vma,
        at_char_device: access_token::char_device,
        at_token_of: access_token::token_of,
        init: || {},
    });
    // The binder diagnostic rows (cold-four lane): the moved audits and
    // census reports the framework's heartbeat slot / taskdump paths
    // reach through the seam (OnceSlot-only publication).
    rux::ipc::binder_diag_hooks::binder_diag_register_hooks(
        rux::ipc::binder_diag_hooks::BinderDiagHooks {
            txn_hang_audit: binder_diag::txn_hang_audit,
            bwr_park_audit: binder_diag::bwr_park_audit,
            ams46_audit: binder_diag::ams46_audit,
            sleep_snapshot: binder_diag::sleep_snapshot,
            stall_report: binder_diag::stall_report,
            schb_beat: binder_diag::schb_beat,
            schb_waiter_census: binder_diag::schb_waiter_census,
            dump_latched_maps_taskctx: binder_diag::dump_latched_maps_taskctx,
        },
    );
}

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (bin registers it at
/// boot). Reporting goes through the framework's counters, so pass/fail
/// lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    rux::tests::test_group_start("ipc_sysv");
    ipc_sysv::test_ipc_sysv();
}
