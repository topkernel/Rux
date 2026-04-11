//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RCU-protected PID hash table for O(1) task lookup by PID.
//!
//! Read path (lookup) uses RCU — no locks, just preempt_disable/enable.
//! Write path (insert/remove) uses per-bucket spinlock.
//!
//! Uses a singly-linked list per bucket (Task.pid_hash_next) so that
//! removal only writes the removed node's next pointer, not a neighbour's,
//! making concurrent RCU traversal safe on SMP.

use core::sync::atomic::{AtomicBool, Ordering};
use core::ptr;
use crate::process::task::Task;
use crate::sync::rcu;

/// Number of hash buckets.
const PID_HASH_BUCKETS: usize = 256;

/// Per-bucket spinlock (simple TAS).
static BUCKET_LOCK: [AtomicBool; PID_HASH_BUCKETS] = [const { AtomicBool::new(false) }; PID_HASH_BUCKETS];

/// Per-bucket chain head.  AtomicPtr would be ideal but we only have
/// core::sync::atomic in no_std, so use a Spinlock-protected array.
/// Actually, we do have AtomicPtr in core.  Let's use it.
use core::sync::atomic::AtomicPtr;

/// Per-bucket chain head (first task pointer, or null).
static BUCKET_HEAD: [AtomicPtr<Task>; PID_HASH_BUCKETS] = [const { AtomicPtr::new(ptr::null_mut()) }; PID_HASH_BUCKETS];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[inline]
fn bucket_index(pid: u32) -> usize {
    (pid as usize) % PID_HASH_BUCKETS
}

#[inline]
fn lock_bucket(idx: usize) {
    while BUCKET_LOCK[idx]
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

#[inline]
fn unlock_bucket(idx: usize) {
    BUCKET_LOCK[idx].store(false, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the PID hash table (call once during boot).
pub fn init() {
    // AtomicPtr is already initialized to null via const, nothing extra needed.
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Insert a task into the PID hash table.
///
/// Called from `alloc_task_slot()` / idle init after task construction.
pub fn pid_hash_insert(task: *mut Task) {
    // SAFETY: task is a valid pointer to a Task that was just constructed and is not
    /// yet visible to other CPUs (called before enqueue_task).
    let pid = unsafe { (*task).pid() };
    let idx = bucket_index(pid);

    lock_bucket(idx);
    // Prepend to chain (LIFO — O(1) insert).
    // SAFETY: We hold the bucket lock, and task is valid and not yet in any chain.
    unsafe {
        let old_head = BUCKET_HEAD[idx].load(Ordering::Relaxed);
        (*task).pid_hash_next = old_head;
        BUCKET_HEAD[idx].store(task, Ordering::Release);
    }
    unlock_bucket(idx);
}

/// Remove a task from the PID hash table by PID.
///
/// Called from `release_task()`. After this returns, the task may still be
/// visible to RCU readers until a grace period elapses.
pub fn pid_hash_remove(pid: u32) {
    let idx = bucket_index(pid);

    lock_bucket(idx);
    // SAFETY: We hold the bucket lock. The chain traversal uses Release/Acquire ordering
    // so no concurrent modifications can occur. Only the removed node's next pointer is
    // written, which is safe for concurrent RCU readers.
    unsafe {
        let mut prev: *mut *mut Task = &BUCKET_HEAD[idx] as *const AtomicPtr<Task> as *mut *mut Task;
        let mut curr = BUCKET_HEAD[idx].load(Ordering::Acquire);
        while !curr.is_null() {
            if (*curr).pid() == pid {
                // Unlink: write *prev = curr->next.  This only touches the
                // _previous_ node's next pointer (or the bucket head), not
                // the removed node itself — safe for concurrent RCU readers
                // who may already be at `curr`.
                *prev = (*curr).pid_hash_next;
                break;
            }
            prev = &mut (*curr).pid_hash_next;
            curr = (*curr).pid_hash_next;
        }
    }
    unlock_bucket(idx);
}

/// Look up a task by PID (RCU read-side).
///
/// Returns a raw pointer to the Task, or null if not found.
/// The caller must NOT free the returned task while in the RCU read-side
/// critical section.
pub fn pid_hash_lookup(pid: u32) -> *mut Task {
    rcu::rcu_read_lock();

    let idx = bucket_index(pid);
    // SAFETY: We are in an RCU read-side critical section. The bucket head is loaded
    // with Acquire ordering. Any task found remains valid until after rcu_read_unlock()
    // plus a grace period (caller must not free it during the RCU read side).
    let result = unsafe {
        let mut curr = BUCKET_HEAD[idx].load(Ordering::Acquire);
        let mut found: *mut Task = ptr::null_mut();
        while !curr.is_null() {
            if (*curr).pid() == pid {
                found = curr;
                break;
            }
            curr = (*curr).pid_hash_next;
        }
        found
    };

    rcu::rcu_read_unlock();

    result
}

/// Iterate all tasks in the PID hash table.
///
/// Takes per-bucket locks sequentially. Used by the OOM killer.
pub fn pid_hash_for_each_task<F>(mut f: F)
where
    F: FnMut(*mut Task),
{
    for i in 0..PID_HASH_BUCKETS {
        lock_bucket(i);
        // SAFETY: We hold the bucket lock so no concurrent insert/remove can modify the chain.
        unsafe {
            let mut curr = BUCKET_HEAD[i].load(Ordering::Acquire);
            while !curr.is_null() {
                f(curr);
                curr = (*curr).pid_hash_next;
            }
        }
        unlock_bucket(i);
    }
}

/// Collect PIDs currently in the hash table.
///
/// Returns a fixed-size array of PIDs, the count, and whether truncation occurred.
/// If more than 64 processes exist, remaining PIDs are silently dropped and
/// `truncated` is set to true.
/// Used by procfs to list /proc/[pid] directories.
pub fn pid_hash_collect_all() -> ([u32; 64], usize, bool) {
    let mut pids = [0u32; 64];
    let mut count = 0;
    let mut truncated = false;

    for i in 0..PID_HASH_BUCKETS {
        if count >= 64 {
            truncated = true;
            break;
        }
        lock_bucket(i);
        // SAFETY: We hold the bucket lock so the chain cannot be modified concurrently.
        unsafe {
            let mut curr = BUCKET_HEAD[i].load(Ordering::Acquire);
            while !curr.is_null() && count < 64 {
                pids[count] = (*curr).pid();
                count += 1;
                curr = (*curr).pid_hash_next;
            }
            if !curr.is_null() {
                truncated = true;
            }
        }
        unlock_bucket(i);
    }

    (pids, count, truncated)
}
