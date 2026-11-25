//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Inter-Process Communication (IPC) — the FRAMEWORK zone after the
//! ipc crate flip.
//!
//! What remains here (and why):
//! - `binder` — the binder driver STAYS framework-side (the rung's
//!   verdict: 46 unsafe blocks + 10 unsafe impls in 7 clusters — the
//!   TxnBlock block-view windows (INV-BINDER-BLOCKVIEW), the
//!   file->Arc adoption, the UnsafeCell interiors, the Send/Sync
//!   markers on raw-pointer object types, and the trap-regs/page-table
//!   diagnostic walkers are mechanism-floor material; flipping it
//!   needs a binder_substrate extraction campaign of its own).
//! - `sysv_sem` — SHELVED framework-side with evidence (the rung's
//!   second verdict): with the file compiled in the ipc crate,
//!   the sysv_sem_block guest test's cross-task handoff deadlocks —
//!   the helper kthread is enqueued (linked=1, on_rq=1, vruntime=0)
//!   but never scheduled, while the boot task's park loop cycles
//!   register→schedule→instant-return forever. Identical source
//!   in-tree (seam state) passes 809/809 with the same breadcrumbs;
//!   fork-point base passes 801/801. The delta is the crate packaging
//!   alone; root cause not run down in this rung (taskdump evidence
//!   in the lane log). The file keeps its zero-unsafe conversions.
//! - `util` — the shared ipc substrate (IpcIds registry, perms, the
//!   SemUndoEntry type of Task's sem_undo field). Consumed by BOTH
//!   the framework sysv_sem and the crate's service files — the crate
//!   re-exports it (the binder pattern).
//! - `ipc_hooks` — the 24-row seam table (every lib consumer of the
//!   movable policy resolves through it; ipc::register_all fills
//!   it from the composition bin).
//! - `edges` — the audited page-refcount zone (INV-IPC-PAGEOP-1).
//!
//! The movable policy (sysv_msg, sysv_shm, posix_mq, keys,
//! access_token) lives in kernel/ipc under
//! #![forbid(unsafe_code)] — the sole copy.

pub mod binder;
pub mod binder_diag_hooks;
pub mod ipc_hooks;
pub mod r#unsafe;
pub mod sysv_sem;
pub mod util;

// Re-export IPC constants for use by dispatch and other modules
pub use util::*;

// pub mod sysv_msg; — MOVED to the ipc service crate (this rung)
// pub mod sysv_shm; — MOVED to the ipc service crate (this rung)
// pub mod posix_mq; — MOVED to the ipc service crate (this rung)
// pub mod keys; — MOVED to the ipc service crate (this rung)
// pub mod access_token; — MOVED to the ipc service crate (this rung)
