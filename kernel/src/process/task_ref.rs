//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Typed task handles — the raw-`*mut Task` consolidation of the
//! unsafe-slim round (2026-11-19).
//!
//! The kernel's task lifecycle runs on raw `*mut Task` pointers stored
//! in the PID hash, runqueues and the tp register. Two disciplines are
//! documented at the sources (`pid_hash::pid_hash_lookup`'s doc and
//! `Task::task_put`'s R12-5 note):
//!
//! 1. **Syscall-context short read** — an unpinned lookup is valid for
//!    the duration of a short field read because the reaper (the
//!    task's parent) cannot run on THIS CPU while the reader executes.
//!    `TaskBorrow` is that discipline as a type.
//! 2. **Pinned handle** — `pid_hash_lookup_pinned` takes a
//!    `task_refcnt` pin that the caller must drop with `task_put` (the
//!    last put frees the slot). `TaskRef` owns the pin: `Drop` calls
//!    `task_put`, so no exit path can leak it.
//!
//! Both handle types `Deref` to `&Task` — the one consolidated
//! dereference with the contract written once, instead of one
//! `unsafe { (*t).field() }` per call site. No raw pointer escapes
//! except through the explicit `as_ptr` escape hatch (required by the
//! queue/hash internals that still live on raw pointers).
//!
//! Handing out `&Task`/`&mut Task` from a raw pointer is the same
//! operation the call sites performed inline; the aliasing
//! discipline is unchanged (a task is mutated only by the CPU that
//! currently owns it or under the locks the callers already hold —
//! exactly what the per-call-site SAFETY comments said before).

#![forbid(unsafe_code)]
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

use super::task::Task;

/// Unpinned borrowed handle: syscall-context short reads.
///
/// Contract (unchanged from `pid_hash_lookup`'s doc): the pointer stays
/// valid for a short read because the caller executes in syscall
/// context on this CPU and the reaper cannot run here mid-read. Do NOT
/// hold across `schedule()`, sleeps, or any path that yields the CPU;
/// do NOT store; use [`TaskRef`] when the handle must outlive such a
/// point.
pub struct TaskBorrow(NonNull<Task>);

impl TaskBorrow {
    /// Borrow the task with `pid`, if it is in the PID hash.
    pub fn lookup(pid: u32) -> Option<Self> {
        let p = crate::process::pid_hash::pid_hash_lookup(pid);
        NonNull::new(p).map(TaskBorrow)
    }

    /// Escape hatch for APIs still taking raw pointers (queue/hash
    /// internals). The borrow discipline above still applies to the
    /// caller's use of the returned pointer.
    /// (INV-TASKREF-1 hatch row: shared-borrow read/write, no store,
    /// no yield — see the safety-and-performance audit table.)
    /// (INV-TASKREF-1 hatch row: address-of under the live pin.)
    pub fn as_ptr(&self) -> *mut Task {
        self.0.as_ptr()
    }

    /// Adopt an already-validated raw task handle (round 4): the ONE
    /// audited edge for handles that predate this type (tree-ring
    /// handles, walk-callback pointers). The caller proves the same
    /// contract `lookup` documents: a short read/write in syscall
    /// context on this CPU, no yield, no store.
    /// (INV-TASKREF-1 hatch row: adopt-once under the caller's
    /// liveness proof.)
    ///
    /// The caller keeps the obligation `lookup` documents: a short
    /// read/write in syscall context on this CPU, no yield, no store —
    /// a short read/write in syscall context on this CPU, no yield, no
    /// store; the deref edges (`TaskBorrow`'s Deref/DerefMut) are the
    /// audited INV-PROC-BORROW-1 hatch in `process::edges`.
    pub fn from_raw(p: *mut Task) -> Option<Self> {
        NonNull::new(p).map(TaskBorrow)
    }
}

impl Deref for TaskBorrow {
    type Target = Task;

    fn deref(&self) -> &Task {
        // INV-PROC-BORROW-1 hatch row: syscall-context contract (type
        // doc) — the reaper cannot run on this CPU while the borrower
        // executes; the audited edge performs the borrow.
        crate::process::edges::borrow_task(self.0.as_ptr())
    }
}

impl DerefMut for TaskBorrow {
    fn deref_mut(&mut self) -> &mut Task {
        // INV-PROC-BORROW-1 mutable hatch row: same contract as Deref;
        // mutation exclusion is the caller's (this CPU owns the task,
        // or the caller holds the lock that serializes the write) — the
        // audited edge performs the borrow.
        crate::process::edges::borrow_task_mut(self.0.as_ptr())
    }
}

/// Pinned task handle owning a `task_refcnt` pin (R12-5).
///
/// `lookup` takes the pin; `Drop` releases it with `task_put` (the
/// last put frees the task slot). Valid across scheduling points and
/// cross-CPU wake/notify paths.
///
/// INV-TASKREF-1 (pin exclusivity, added by the slim4-fix lane
/// 2026-11-20): the handle owns its pin EXCLUSIVELY. Callers must NOT
/// call `task_put` on `as_ptr()` while the handle is alive — Drop
/// releases the same pin, so a manual put is a double release: the
/// receiver's `task_refcnt` erodes by one per call and `free_task_slot`
/// eventually fires on a task still linked in the pid hash, its
/// parent's child list, and the timer/runqueue structures (the slim4
/// fio-corruption regression: signal receivers freed early, wait4
/// walking a pid=0xdeadbeef page). A raw consumer that must release
/// the pin itself takes ownership with [`TaskRef::into_raw`] (which
/// `forget`s the handle and its Drop).
pub struct TaskRef(NonNull<Task>);

impl TaskRef {
    /// Look up and pin the task with `pid`; `None` if not found.
    pub fn lookup(pid: u32) -> Option<Self> {
        let p = crate::process::pid_hash::pid_hash_lookup_pinned(pid);
        NonNull::new(p).map(TaskRef)
    }

    /// Adopt an already-taken pin (e.g. one taken inline before this
    /// type existed). The returned handle owns it. `None` for null.
    pub fn from_pinned(p: *mut Task) -> Option<Self> {
        NonNull::new(p).map(TaskRef)
    }

    /// Escape hatch: keep the pin, hand out the raw pointer. The
    /// caller becomes responsible for the eventual `task_put`.
    /// (INV-TASKREF-1 hatch row: pin transfer, exactly-one task_put.)
    pub fn into_raw(self) -> *mut Task {
        let p = self.0.as_ptr();
        core::mem::forget(self);
        p
    }

    pub fn as_ptr(&self) -> *mut Task {
        self.0.as_ptr()
    }
}

impl Deref for TaskRef {
    type Target = Task;

    fn deref(&self) -> &Task {
        // INV-PROC-BORROW-1 hatch row: the pin defers `free_task_slot`
        // (task_put) until after this handle is dropped, so the task
        // page stays alive; the audited edge performs the borrow.
        crate::process::edges::borrow_task(self.0.as_ptr())
    }
}

impl DerefMut for TaskRef {
    fn deref_mut(&mut self) -> &mut Task {
        // INV-PROC-BORROW-1 mutable hatch row: same liveness as Deref;
        // mutation exclusion is the caller's (the pin serializes
        // freeing, not writing); the audited edge performs the borrow.
        crate::process::edges::borrow_task_mut(self.0.as_ptr())
    }
}

impl Drop for TaskRef {
    fn drop(&mut self) {
        // SAFETY: this handle owns exactly one pin (lookup or
        // from_pinned); task_put is the documented release.
        crate::process::task::Task::task_put(self.0.as_ptr());
    }
}
