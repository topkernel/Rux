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
    pub fn as_ptr(&self) -> *mut Task {
        self.0.as_ptr()
    }

    /// Adopt an already-validated raw task handle (round 4): the ONE
    /// audited edge for handles that predate this type (tree-ring
    /// handles, walk-callback pointers). The caller proves the same
    /// contract `lookup` documents: a short read/write in syscall
    /// context on this CPU, no yield, no store.
    ///
    /// # Safety
    /// `p` must point at a live task (the caller's borrow/pin/tree-lock
    /// keeps the slot alive for the borrow's duration) and must not be
    /// adopted twice into handles that outlive that proof.
    pub unsafe fn from_raw(p: *mut Task) -> Option<Self> {
        NonNull::new(p).map(TaskBorrow)
    }
}

impl Deref for TaskBorrow {
    type Target = Task;

    fn deref(&self) -> &Task {
        // SAFETY: syscall-context contract (type doc): the reaper
        // cannot run on this CPU while the borrower executes, so the
        // task page cannot be freed under this reference for the
        // duration of the short read.
        unsafe { self.0.as_ref() }
    }
}

impl DerefMut for TaskBorrow {
    fn deref_mut(&mut self) -> &mut Task {
        // SAFETY: same contract as Deref; mutation additionally relies
        // on the caller's existing exclusion (this CPU owns the task,
        // or the caller holds the lock that serializes the write) —
        // the same argument the pre-conversion call sites made.
        unsafe { self.0.as_mut() }
    }
}

/// Pinned task handle owning a `task_refcnt` pin (R12-5).
///
/// `lookup` takes the pin; `Drop` releases it with `task_put` (the
/// last put frees the task slot). Valid across scheduling points and
/// cross-CPU wake/notify paths.
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
        // SAFETY: the pin defers `free_task_slot` (task_put) until
        // after this handle is dropped, so the task page stays alive
        // and mapped for the whole lifetime of the handle.
        unsafe { self.0.as_ref() }
    }
}

impl DerefMut for TaskRef {
    fn deref_mut(&mut self) -> &mut Task {
        // SAFETY: same liveness as Deref; mutation exclusion is the
        // caller's (the pin serializes freeing, not writing).
        unsafe { self.0.as_mut() }
    }
}

impl Drop for TaskRef {
    fn drop(&mut self) {
        // SAFETY: this handle owns exactly one pin (lookup or
        // from_pinned); task_put is the documented release.
        crate::process::task::Task::task_put(self.0.as_ptr());
    }
}
