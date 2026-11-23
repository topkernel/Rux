//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Inter-Process Communication (IPC)
//!
//! This module implements System V IPC (semaphores, message queues, shared memory)
//! and POSIX message queues, following the standard kernel design.
//!
//! ## Submodules
//! - `util` — Core IPC infrastructure (ipc_ids registry, permissions, ID encoding)
//! - `sysv_sem` — System V semaphores (semget, semctl, semop, semtimedop)
//! - `sysv_msg` — System V message queues (msgget, msgctl, msgsnd, msgrcv)
//! - `sysv_shm` — System V shared memory (shmget, shmctl, shmat, shmdt)
//! - `posix_mq` — POSIX message queues (mq_open, mq_unlink, mq_timedsend, mq_timedreceive, mq_notify, mq_getsetattr)
//! - `keys` — Kernel key retention service (add_key, request_key, keyctl)
//!
//! ## Crate-flip rung (rux-ipc)
//! - `ipc_hooks` — the framework-side hook table (every lib consumer
//!   of the movable policy resolves through it)
//! - `edges` — the framework-side audited zone (INV-IPC-PAGEOP-1)
//! - `binder` — STAYS framework-side (its block-view windows, Arc
//!   adoption and diagnostic walkers are floor-shaped; the rung's
//!   verdict). Every binder call site keeps its direct path.
//! - `SemUndoEntry` — the framework-side TYPE substrate (Task's
//!   sem_undo field names it; the vma/mm_struct shelving lesson:
//!   inherent data types stay with their framework consumers, the
//!   policy re-exports).

pub mod util;
pub mod sysv_sem;
pub mod sysv_msg;
pub mod sysv_shm;
pub mod posix_mq;
pub mod binder;
pub mod access_token;
pub mod keys;
pub mod edges;
pub mod ipc_hooks;

// Re-export IPC constants for use by dispatch and other modules
pub use util::*;

/// Per-process semaphore undo entry.
/// Records an adjustment made to a semaphore so it can be reversed on process exit.
/// Framework-side type substrate: Task's `sem_undo` field names this
/// type, so the definition must stay in the framework crate (util
/// re-exports it; the sysv_sem policy constructs through that path).
#[derive(Clone, Copy)]
pub struct SemUndoEntry {
    /// Semaphore set ID.
    pub semid: i32,
    /// Semaphore number within the set.
    pub sem_num: u16,
    /// Adjustment value (the sem_op that was applied).
    pub adjustment: i32,
}

/// Initialize the IPC subsystem.
/// Called once during kernel boot, after the scheduler is initialized.
pub fn init() {
    // Static IPC registries (IpcIds) are const-initialized via Spinlock::new,
    // so no explicit initialization is needed.
    // POSIX MQ table is also const-initialized.
    // Wait queues in message queues are initialized when queues are created.
}

/// Fill the IpcHooks table (called by the composition bin's rust_main
/// BEFORE the framework impl — the devfs populate and any syscall ride
/// the rows from the first use; binder's access_token consult included).
pub fn register_hooks() {
    ipc_hooks::ipc_register_hooks(ipc_hooks::IpcHooks {
        sem_undo_exit: sysv_sem::sem_undo_exit,
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
        sem_proc_show: sysv_sem::proc_show,
        msg_proc_show: sysv_msg::proc_show,
        shm_proc_show: sysv_shm::proc_show,
        shm_attach_vma: sysv_shm::shm_attach_vma,
        shm_detach_vma: sysv_shm::shm_detach_vma,
        at_char_device: access_token::char_device,
        at_token_of: access_token::token_of,
        init,
    });
}
