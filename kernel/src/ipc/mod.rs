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

pub mod util;
pub mod sysv_sem;
pub mod sysv_msg;
pub mod sysv_shm;
pub mod posix_mq;

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
