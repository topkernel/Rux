//! The IPC SERVICE CRATE (crate-flip rung: rux-ipc): the movable ipc
//! policy — System V semaphores/message queues/shared memory, POSIX
//! message queues, the kernel key retention service and the
//! access_token registry — under a crate-level unsafe-deny boundary.
//! The framework keeps ipc/binder.rs (the rung's verdict: its
//! block-view windows, Arc adoption and diagnostic walkers are
//! floor-shaped), ipc/ipc_hooks.rs (the 26-row seam), ipc/edges.rs
//! (INV-IPC-PAGEOP-1) and the SemUndoEntry type substrate.
//!
//! Sole-copy state (the rung-1 recipe's final step): the service files
//! LIVE in this crate (src/ipc/); the two ipc guest-test modules
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
    arch, cmdline, config, console, dfx, drivers, errno, fs, interrupt, mm,
    process, safety, sched, security, signal, sync, syscall, timer, trace,
};
pub use rux::net;
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

/// The ipc tree, nested under crate::ipc (the historical paths): the
/// service files plus the framework's binder re-export (the union the
/// sibling services consumed when ipc was one framework module) and
/// the framework-side seam zones (ipc_hooks, edges).
pub mod ipc {
    // the service files (THIS crate's own — the sole copies)
    pub mod util;
    pub mod sysv_sem;
    pub mod sysv_msg;
    pub mod sysv_shm;
    pub mod posix_mq;
    pub mod keys;
    pub mod access_token;

    // The old framework mod.rs's `pub use util::*` — the IPC constants
    // (IPC_NOWAIT, SHM_REMAP, ...) re-exported at the ipc root so the
    // service files' `super::X` paths resolve (the historical shape).
    pub use util::*;

    // the guest-test modules that followed the files (the framework's
    // tests/mod.rs rows are gated with the move note).
    #[cfg(feature = "unit-test")]
    pub mod ipc_sysv;
    #[cfg(feature = "unit-test")]
    pub mod sysv_sem_block;

    // binder stays FRAMEWORK-side (the rung's verdict); re-exported so
    // `crate::ipc::binder` keeps resolving (rux-syscall's ioctl/mmap/
    // poll rows, devfs, futex, the dfx audits).
    pub use rux::ipc::binder;
    // the seam zones the service files consume (the hook table's
    // KeyQuota type; the audited page-refcount edges).
    pub use rux::ipc::ipc_hooks;
    pub use rux::ipc::edges;
    // the type substrate that stayed with Task's sem_undo field.
    pub use rux::ipc::SemUndoEntry;

    // The framework's whole-module globs (the service files'
    // crate::arch / crate::mm / ... paths resolve through the crate
    // root re-exports; this glob mirrors the old mod.rs's pub-use).
    pub use crate::arch::*;
    pub use crate::config;
    pub use crate::console;
    pub use crate::dfx::*;
    pub use crate::drivers::*;
    pub use crate::errno;
    pub use crate::fs::*;
    pub use crate::mm::*;
    pub use crate::net;
    pub use crate::process::*;
    pub use crate::safety::*;
    pub use crate::sched::*;
    pub use crate::security::*;
    pub use crate::signal::*;
    pub use crate::sync::*;
    pub use crate::syscall;
    pub use crate::timer::*;
    pub use crate::trace;
}

/// The boot-time registration (rux-bin calls this before kmain): the
/// IpcHooks table's rows. THE composition contract.
pub fn register_all() {
    rux::ipc::ipc_hooks::ipc_register_hooks(rux::ipc::ipc_hooks::IpcHooks {
        sem_undo_exit: ipc::sysv_sem::sem_undo_exit,
        mq_fds_cleanup: ipc::posix_mq::mq_fds_cleanup,
        exec_clear_thread_keyring: ipc::keys::exec_clear_thread_keyring,
        at_registered_token_of: ipc::access_token::registered_token_of,
        at_mirror_set: ipc::access_token::mirror_set,
        msgmni: ipc::sysv_msg::msgmni,
        set_msgmni: ipc::sysv_msg::set_msgmni,
        shm_ctlmax: ipc::sysv_shm::shm_ctlmax,
        set_shm_ctlmax: ipc::sysv_shm::set_shm_ctlmax,
        shm_ctlni: ipc::sysv_shm::shm_ctlni,
        set_shm_ctlni: ipc::sysv_shm::set_shm_ctlni,
        shm_ctlall: ipc::sysv_shm::shm_ctlall,
        set_shm_ctlall: ipc::sysv_shm::set_shm_ctlall,
        ipc_ids_max: ipc::util::ipc_ids_max,
        queues_max: ipc::posix_mq::queues_max,
        set_queues_max: ipc::posix_mq::set_queues_max,
        key_quota: ipc::keys::key_quota,
        set_key_quota: ipc::keys::set_key_quota,
        sem_proc_show: ipc::sysv_sem::proc_show,
        msg_proc_show: ipc::sysv_msg::proc_show,
        shm_proc_show: ipc::sysv_shm::proc_show,
        shm_attach_vma: ipc::sysv_shm::shm_attach_vma,
        shm_detach_vma: ipc::sysv_shm::shm_detach_vma,
        at_char_device: ipc::access_token::char_device,
        at_token_of: ipc::access_token::token_of,
        init: || {},
    });
}

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (rux-bin registers it at
/// boot). Reporting goes through the framework's counters, so pass/fail
/// lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    rux::tests::test_group_start("ipc_sysv");
    ipc::ipc_sysv::test_ipc_sysv();
    rux::tests::test_group_start("sysv_sem_block");
    ipc::sysv_sem_block::test_sysv_sem_block();
}
