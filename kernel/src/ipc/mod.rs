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

pub mod util;
// The subsystem's ONE audited unsafe file (mainline minimal-first
// consolidation, 2026-11-27): file named `unsafe.rs` per the mainline
// naming decision, module named `edges` per the sibling convention
// (fs/unsafe.rs, process/unsafe.rs, syscall/edges.rs, sync/edges.rs).
// The #[path] keeps the FILE name the mainline gate requires while
// call sites stay on the plain `edges` identifier.
#[path = "unsafe.rs"]
pub mod edges;
pub mod sysv_sem;
pub mod sysv_msg;
pub mod sysv_shm;
pub mod posix_mq;
pub mod binder;
pub mod access_token;
pub mod keys;

// Re-export IPC constants for use by dispatch and other modules
pub use util::*;

/// Initialize the IPC subsystem.
/// Called once during kernel boot, after the scheduler is initialized.
pub fn init() {
    // Static IPC registries (IpcIds) are const-initialized via Spinlock::new,
    // so no explicit initialization is needed.
    // POSIX MQ table is also const-initialized.
    // Wait queues in message queues are initialized when queues are created.
}
