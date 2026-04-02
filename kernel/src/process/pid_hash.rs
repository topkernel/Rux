//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! PID hash table for O(1) task lookup by PID.
//!
//! All tasks (running, sleeping, zombie) are registered here,
//! enabling fast lookup regardless of task state.

use alloc::collections::BTreeMap;
use crate::sync::spinlock::Spinlock;

use crate::process::task::Task;

/// Number of hash buckets.
const PID_HASH_BUCKETS: usize = 256;

/// PID hash table using per-bucket BTreeMap.
pub struct PidHashTable {
    buckets: [BTreeMap<u32, *mut Task>; PID_HASH_BUCKETS],
}

// SAFETY: The PID hash table is protected by a Mutex and only accessed
// from kernel code. Raw pointers to Task are only inserted/removed
// while holding the lock.
unsafe impl Send for PidHashTable {}
unsafe impl Sync for PidHashTable {}

impl PidHashTable {
    const fn new() -> Self {
        // SAFETY: BTreeMap::new() is const since Rust 1.66
        const EMPTY: BTreeMap<u32, *mut Task> = BTreeMap::new();
        Self {
            buckets: [EMPTY; PID_HASH_BUCKETS],
        }
    }

    fn bucket_index(pid: u32) -> usize {
        (pid as usize) % PID_HASH_BUCKETS
    }
}

static PID_HASH_TABLE: Spinlock<PidHashTable> = Spinlock::new(PidHashTable::new());

/// Insert a task into the PID hash table.
///
/// Called from `alloc_task_slot()` after task initialization.
pub fn pid_hash_insert(task: *mut Task) {
    unsafe {
        let pid = (*task).pid();
        let mut table = PID_HASH_TABLE.lock();
        let idx = PidHashTable::bucket_index(pid);
        table.buckets[idx].insert(pid, task);
    }
}

/// Remove a task from the PID hash table.
///
/// Called from `release_task()` before freeing resources.
pub fn pid_hash_remove(pid: u32) {
    let mut table = PID_HASH_TABLE.lock();
    let idx = PidHashTable::bucket_index(pid);
    table.buckets[idx].remove(&pid);
}

/// Look up a task by PID.
///
/// Returns a raw pointer to the Task, or null if not found.
/// Works for all task states (running, sleeping, zombie).
pub fn pid_hash_lookup(pid: u32) -> *mut Task {
    let table = PID_HASH_TABLE.lock();
    let idx = PidHashTable::bucket_index(pid);
    table
        .buckets[idx]
        .get(&pid)
        .copied()
        .unwrap_or(core::ptr::null_mut())
}
