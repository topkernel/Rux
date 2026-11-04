//! pidfd — file descriptors that name a process (reference pidfd_open /
//! pidfd_send_signal / pidfd_getfd semantics).
//!
//! Identity model (the divergence-safe core): a pidfd pins the target's
//! thread-group LEADER (`task_refcnt`, the same pin the wake/notify paths
//! use) and records (pid, leader-ptr). The reference kernel hands out a
//! refcounted `struct pid`; our Task pages are recycled, so a pinned
//! leader pointer plus the pid-hash equality check gives the same
//! guarantee: when `find_task_by_pid(pid)` returns a DIFFERENT pointer
//! than the pinned one, the pid was reused by a new process and the old
//! pidfd resolves to nothing (ESRCH) — it can never target the new
//! process (LTP pidfd_send_signal03's forced-reuse check).
//!
//! Two file shapes carry pidfd identity:
//! - `PIDFD_OPS` files created by pidfd_open(2);
//! - /proc/<pid> DIRECTORY fds opened before the process went away
//!   (the pre-pidfd_open(2) mechanism the reference still supports:
//!   pidfd_send_signal accepts both). The procfs open path attaches the
//!   same `PidFdData` via `attach()`; `PROC_PIDDIR_OPS` (procfs) owns the
//!   free path for that variant.
//!
//! Poll: EPOLLIN when the thread group has exited. The reference hangs a
//! waitqueue off `struct pid`; a single global queue woken once per
//! do_exit (`exit_notify_all_waiters`) is the same observable behavior —
//! spurious wakes are handled by the poll round's readiness re-check.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust except for the documented task-pin
// accessors, which follow the same SAFETY contracts as the wake paths.
#![allow(clippy::missing_safety_doc)]

use crate::fs::file::FileOps;
use crate::process::task::Task;

/// EPOLLIN — the only bit a pidfd ever reports (exit readiness).
const EPOLLIN_U16: u16 = 0x001;

/// Global pidfd poller queue: every poll() over a pidfd registers here
/// and `exit_notify_all_waiters` (called from do_exit) wakes them all.
/// Spurious wakes re-check their own target's state and re-sleep.
static PIDFD_EXIT_WAITQ: crate::process::wait::WaitQueueHead =
    crate::process::wait::WaitQueueHead::new();

/// Wake every pidfd poller. Called from do_exit after the ZOMBIE state
/// store (the reference's wake_up_poll on the pid's waiters).
pub fn exit_notify_all_waiters() {
    let _n = PIDFD_EXIT_WAITQ.wake_up_all();
}

/// The pinned identity carried by every pidfd-shaped file.
pub struct PidFdData {
    /// The process id (thread-group id) at open time.
    pub pid: u32,
    /// PINNED thread-group leader. Valid until `close` drops the pin;
    /// never dereferenced beyond pid/state/cred reads.
    pub task: *mut Task,
}

/// Pin the thread-group leader of `pid` (pidfd identity). Returns None
/// when the pid is absent.
fn pin_leader(pid: u32) -> Option<*mut Task> {
    // SAFETY: pid_hash_lookup_pinned returns a pinned, valid Task.
    let t = unsafe { crate::process::pid_hash::pid_hash_lookup_pinned(pid) };
    if t.is_null() {
        return None;
    }
    // SAFETY: t is pinned for the duration of this function.
    let leader = unsafe { (*t).group_leader_ptr() };
    if (leader as *const Task) != (t as *const Task) && !leader.is_null() {
        // Pin the leader too; drop the intermediate pin.
        // SAFETY: group_leader_ptr() of a pinned task is in the same
        // thread ring (kept alive by the ring linkage discipline).
        unsafe {
            (*leader)
                .task_refcnt
                .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
            Task::task_put(t);
        }
    }
    Some(leader)
}

/// Allocate the pinned identity for `pid`. Caller owns the returned
/// box; hand it to `attach()` or free it with `free_pidfd_data`.
pub fn make_pidfd_data(pid: u32) -> Option<alloc::boxed::Box<PidFdData>> {
    let task = pin_leader(pid)?;
    Some(alloc::boxed::Box::new(PidFdData { pid, task }))
}

/// Drop a PidFdData and its task pin.
pub fn free_pidfd_data(data: *mut PidFdData) {
    if data.is_null() {
        return;
    }
    // SAFETY: the box was created by make_pidfd_data and is owned by the
    // caller (the file's private_data slot, single-owner).
    let boxed = unsafe { alloc::boxed::Box::from_raw(data) };
    Task::task_put(boxed.task);
}

/// Attach pidfd identity to a file's private_data slot (both the
/// pidfd_open and the /proc/<pid> dir variant).
pub fn attach(file: &crate::fs::File, data: alloc::boxed::Box<PidFdData>) {
    let ptr = alloc::boxed::Box::into_raw(data) as *mut u8;
    file.set_private_data(ptr);
}

/// Resolve a file's pidfd identity, if it has one.
///
/// SAFETY: `data` returned here is valid while the file holds it.
pub unsafe fn data_of(file: &crate::fs::File) -> Option<&'static PidFdData> {
    let p = (*file.private_data.get())?;
    // SAFETY: the pointer came from Box::into_raw(PidFdData) in attach()
    // and stays owned by the file for its lifetime.
    Some(&*(p as *const PidFdData))
}

/// Resolve a pidfd's process with pid-reuse protection.
///
/// Returns the pinned-at-open leader when it is STILL the task the pid
/// hash names for `pid` (i.e. alive and not recycled), else None.
pub fn resolve_alive(data: &PidFdData) -> Option<&'static Task> {
    // SAFETY: lookup runs under RCU; equality with the pinned pointer
    // proves identity without dereferencing the result.
    let now = unsafe { crate::process::pid_hash::pid_hash_lookup(data.pid) };
    if now.is_null() || (now as *const Task) != (data.task as *const Task) {
        return None;
    }
    // SAFETY: equal to the pinned leader pointer, which stays valid
    // until the pidfd's close drops the pin.
    Some(unsafe { &*(data.task as *const Task) })
}

/// Has the pidfd's thread group exited? (ZOMBIE/DEAD leader, or the
/// leader left the pid hash — reaped.)
pub fn target_exited(data: &PidFdData) -> bool {
    // SAFETY: the pinned leader's Task page is valid; state() is atomic.
    let st = unsafe { (*data.task).state() };
    if st.is_dead() || st.contains(crate::process::task::TaskState::ZOMBIE) {
        return true;
    }
    // Reaped and hash-removed: exited by definition.
    unsafe { crate::process::pid_hash::pid_hash_lookup(data.pid).is_null() }
}

// ---------------------------------------------------------------------------
// pidfd file ops (pidfd_open files)
// ---------------------------------------------------------------------------

fn pidfd_poll(_file: &crate::fs::File, _events: u16) -> u16 {
    // SAFETY: identity-checked dispatcher contract (poll only installed
    // on PIDFD_OPS files, whose private_data came from attach()).
    match unsafe { data_of(_file) } {
        Some(d) if target_exited(d) => EPOLLIN_U16,
        _ => 0,
    }
}

fn pidfd_close(file: &crate::fs::File) -> i32 {
    // SAFETY: same contract as pidfd_poll.
    if let Some(p) = unsafe { *file.private_data.get() } {
        free_pidfd_data(p as *mut PidFdData);
        // Null the slot so a racing second close cannot double-free.
        unsafe { *file.private_data.get() = None };
    }
    0
}

/// File operations for pidfd_open(2) descriptors.
pub static PIDFD_OPS: FileOps = FileOps::builder()
    .poll(pidfd_poll)
    .close(pidfd_close)
    .build();

/// poll(2) wait registration (event-driven exit wakeups).
pub fn pidfd_poll_wait(
    _file: &crate::fs::File,
    task: *mut Task,
) -> bool {
    PIDFD_EXIT_WAITQ.add(crate::process::wait::WaitQueueEntry::new(task, false));
    true
}

/// Remove a registration added by pidfd_poll_wait.
pub fn pidfd_poll_unwait(_file: &crate::fs::File, task: *mut Task) {
    PIDFD_EXIT_WAITQ.remove(task);
}

// ---------------------------------------------------------------------------
// /proc/<pid> directory variant (PROC_PIDDIR_OPS hooks)
// ---------------------------------------------------------------------------

/// Close hook for /proc/<pid> dir fds: drop the pinned identity.
pub fn pidfd_dir_close(file: &crate::fs::File) -> i32 {
    pidfd_close(file)
}

/// Poll hook for /proc/<pid> dir fds: exit readiness, same as pidfds.
pub fn pidfd_dir_poll(file: &crate::fs::File, _events: u16) -> u16 {
    pidfd_poll(file, _events)
}
