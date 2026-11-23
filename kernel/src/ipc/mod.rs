//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Inter-Process Communication (IPC) — the FRAMEWORK zone after the
//! rux-ipc crate flip.
//!
//! What remains here (and why):
//! - `binder` — the binder driver STAYS framework-side (the rung's
//!   verdict: 46 unsafe blocks + 10 unsafe impls in 7 clusters — the
//!   TxnBlock block-view windows (INV-BINDER-BLOCKVIEW), the
//!   file->Arc adoption, the UnsafeCell interiors, the Send/Sync
//!   markers on raw-pointer object types, and the trap-regs/page-table
//!   diagnostic walkers are mechanism-floor material; flipping it
//!   needs a binder_substrate extraction campaign of its own).
//! - `ipc_hooks` — the 26-row seam table (every lib consumer of the
//!   movable policy resolves through it; rux-ipc::register_all fills
//!   it from the composition bin).
//! - `edges` — the audited page-refcount zone (INV-IPC-PAGEOP-1).
//! - `SemUndoEntry` — the type substrate of Task's sem_undo field
//!   (inherent data types stay with their framework consumers; the
//!   service re-exports).
//!
//! The movable policy (util, sysv_sem, sysv_msg, sysv_shm, posix_mq,
//! keys, access_token) lives in kernel/rux-ipc under
//! #![forbid(unsafe_code)] — the sole copy.

pub mod binder;
pub mod ipc_hooks;
pub mod edges;

// pub mod util; — MOVED to the rux-ipc service crate (this rung)
// pub mod sysv_sem; — MOVED to the rux-ipc service crate (this rung)
// pub mod sysv_msg; — MOVED to the rux-ipc service crate (this rung)
// pub mod sysv_shm; — MOVED to the rux-ipc service crate (this rung)
// pub mod posix_mq; — MOVED to the rux-ipc service crate (this rung)
// pub mod keys; — MOVED to the rux-ipc service crate (this rung)
// pub mod access_token; — MOVED to the rux-ipc service crate (this rung)

/// Per-process semaphore undo entry.
/// Records an adjustment made to a semaphore so it can be reversed on process exit.
/// Framework-side type substrate: Task's `sem_undo` field names this
/// type, so the definition must stay in the framework crate (the
/// vma/mm_struct shelving lesson: inherent data stays with its
/// framework consumer; the service re-exports it).
#[derive(Clone, Copy)]
pub struct SemUndoEntry {
    /// Semaphore set ID.
    pub semid: i32,
    /// Semaphore number within the set.
    pub sem_num: u16,
    /// Adjustment value (the sem_op that was applied).
    pub adjustment: i32,
}
