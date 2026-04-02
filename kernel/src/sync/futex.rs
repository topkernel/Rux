//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Futex Implementation - Fast Userspace Mutex

use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use crate::process::Task;
use crate::process::task::TaskState;
use crate::syscall::errno::{EINVAL, EFAULT, EAGAIN, ENOSYS};

/// FUTEX opcodes
pub const FUTEX_WAIT: i32 = 0;
pub const FUTEX_WAKE: i32 = 1;
pub const FUTEX_FD: i32 = 2;
pub const FUTEX_REQUEUE: i32 = 3;
pub const FUTEX_CMP_REQUEUE: i32 = 4;
pub const FUTEX_WAKE_OP: i32 = 5;
pub const FUTEX_LOCK_PI: i32 = 6;
pub const FUTEX_UNLOCK_PI: i32 = 7;
pub const FUTEX_TRYLOCK_PI: i32 = 8;
pub const FUTEX_WAIT_BITSET: i32 = 9;
pub const FUTEX_WAKE_BITSET: i32 = 10;
pub const FUTEX_WAIT_REQUEUE_PI: i32 = 11;
pub const FUTEX_CMP_REQUEUE_PI: i32 = 12;
pub const FUTEX_LOCK_PI2: i32 = 13;

pub const FUTEX_PRIVATE_FLAG: i32 = 128;
pub const FUTEX_CLOCK_REALTIME: i32 = 256;
pub const FUTEX_CMD_MASK: i32 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

pub const FUTEX_BITSET_MATCH_ANY: u32 = 0xffffffff;

// Internal flags
pub const FLAGS_SHARED: u32 = 0x0010;
pub const FLAGS_CLOCKRT: u32 = 0x0020;

/// Futex key - uniquely identifies a futex
#[derive(Clone, Copy, Debug)]
pub struct FutexKey {
    /// Userspace address
    pub uaddr: usize,
    /// Process ID (for private futex)
    pub pid: u32,
    /// Flags
    pub flags: u32,
}

impl FutexKey {
    pub fn new(uaddr: usize, pid: u32, flags: u32) -> Self {
        Self { uaddr, pid, flags }
    }

    /// Check if two keys match
    pub fn matches(&self, other: &FutexKey) -> bool {
        // For private futex, compare address and PID
        if !(self.flags & FLAGS_SHARED != 0) {
            self.uaddr == other.uaddr && self.pid == other.pid
        } else {
            // For shared futex, only compare address
            self.uaddr == other.uaddr
        }
    }
}

/// Waiter information
struct Waiter {
    /// Futex key
    key: FutexKey,
    /// Waiting task
    task: *mut Task,
    /// bitset
    bitset: u32,
    /// Whether already woken
    woken: bool,
    /// Next waiter
    next: Option<usize>,
}

// Waiter can be sent across threads because we use Mutex to protect access
unsafe impl Send for Waiter {}
unsafe impl Sync for Waiter {}

/// Waiter pool size - from config
const WAITER_POOL_SIZE: usize = crate::config::FUTEX_WAITER_POOL_SIZE;

/// Waiter pool
static WAITER_POOL: [Spinlock<Option<Waiter>>; WAITER_POOL_SIZE] = {
    const INIT: Spinlock<Option<Waiter>> = Spinlock::new(None);
    [INIT; WAITER_POOL_SIZE]
};

/// Hash bucket count - from config
const HASH_SIZE: usize = crate::config::FUTEX_HASH_SIZE;

/// Waiter list head for each bucket
static HASH_HEADS: [Spinlock<Option<usize>>; HASH_SIZE] = {
    const INIT: Spinlock<Option<usize>> = Spinlock::new(None);
    [INIT; HASH_SIZE]
};

/// Allocate a waiter slot
fn alloc_waiter() -> Option<usize> {
    for i in 0..WAITER_POOL_SIZE {
        let mut slot = WAITER_POOL[i].lock();
        if slot.is_none() {
            return Some(i);
        }
    }
    None
}

/// Free a waiter slot
fn free_waiter(index: usize) {
    let mut slot = WAITER_POOL[index].lock();
    *slot = None;
}

/// Calculate futex hash value
fn futex_hash(key: &FutexKey) -> usize {
    let hash = key.uaddr.wrapping_add(key.pid as usize);
    hash % HASH_SIZE
}

/// Wake up waiters
pub fn futex_wake(uaddr: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    if bitset == 0 {
        return -EINVAL as i64;
    }

    // Get current process PID
    let pid = match crate::sched::current() {
        Some(t) => unsafe { (*t).pid() },
        None => return -EFAULT as i64,
    };

    // Create futex key
    let key = FutexKey::new(uaddr, pid, flags);

    // Get hash bucket index
    let bucket_idx = futex_hash(&key);

    let mut ret = 0i64;
    let mut prev_idx: Option<usize> = None;
    let mut current_idx = *HASH_HEADS[bucket_idx].lock();

    while let Some(idx) = current_idx {
        if ret >= nr_wake as i64 {
            break;
        }

        let waiter_slot = WAITER_POOL[idx].lock();
        if let Some(ref waiter) = *waiter_slot {
            if waiter.key.matches(&key) && (waiter.bitset & bitset) != 0 {
                // Mark as woken
                let woken_task = waiter.task;
                let next_idx = waiter.next;

                // Release lock before operating
                drop(waiter_slot);

                // Mark woken
                {
                    let mut w = WAITER_POOL[idx].lock();
                    if let Some(ref mut w) = *w {
                        w.woken = true;
                    }
                }

                // Set task state to ready
                if !woken_task.is_null() {
                    unsafe {
                        (*woken_task).set_state(TaskState::new(TaskState::RUNNING));
                    }
                }

                // Remove from list
                if prev_idx.is_none() {
                    *HASH_HEADS[bucket_idx].lock() = next_idx;
                } else if let Some(prev) = prev_idx {
                    let mut prev_slot = WAITER_POOL[prev].lock();
                    if let Some(ref mut prev_waiter) = *prev_slot {
                        prev_waiter.next = next_idx;
                    }
                }

                // Free waiter slot
                free_waiter(idx);

                ret += 1;
                current_idx = next_idx;
                continue;
            }
            prev_idx = Some(idx);
            current_idx = waiter.next;
        } else {
            break;
        }
    }

    ret
}

/// Wait for futex
pub fn futex_wait(uaddr: usize, flags: u32, val: u32, bitset: u32) -> i64 {
    if bitset == 0 {
        return -EINVAL as i64;
    }

    let uaddr_ptr = uaddr as *const AtomicU32;

    if uaddr_ptr.is_null() {
        return -EINVAL as i64;
    }

    // Get current process
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -EFAULT as i64,
    };

    let pid = unsafe { (*current).pid() };

    // Create futex key
    let key = FutexKey::new(uaddr, pid, flags);

    // Get hash bucket index
    let bucket_idx = futex_hash(&key);

    // Lock bucket head
    let mut head = HASH_HEADS[bucket_idx].lock();

    // Check value again (while holding lock)
    let uval = unsafe { (*uaddr_ptr).load(Ordering::SeqCst) };

    if uval != val {
        return -EAGAIN as i64;
    }

    // Allocate waiter slot
    let waiter_idx = match alloc_waiter() {
        Some(idx) => idx,
        None => return -ENOMEM as i64,
    };

    // Initialize waiter
    {
        let mut slot = WAITER_POOL[waiter_idx].lock();
        *slot = Some(Waiter {
            key,
            task: current,
            bitset,
            woken: false,
            next: *head,
        });
    }

    // Update list head
    *head = Some(waiter_idx);
    drop(head);

    // Set task state to blocked
    unsafe {
        (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    }

    // Release kernel big lock (must release before sleeping)
    crate::sync::kernel_lock_release();

    // Schedule to yield CPU
    crate::sched::schedule();

    // Re-acquire kernel big lock after waking up
    crate::sync::kernel_lock_acquire();

    // After waking up, check if cleanup is needed
    {
        let slot = WAITER_POOL[waiter_idx].lock();
        if let Some(ref waiter) = *slot {
            if !waiter.woken {
                // Not yet woken, need to remove from list
                drop(slot);
                remove_waiter(bucket_idx, waiter_idx);
            }
        }
    }

    0
}

/// Remove waiter from list
fn remove_waiter(bucket_idx: usize, target_idx: usize) {
    let mut head = HASH_HEADS[bucket_idx].lock();

    if *head == Some(target_idx) {
        // Target is list head
        let next = {
            let slot = WAITER_POOL[target_idx].lock();
            slot.as_ref().and_then(|w| w.next)
        };
        *head = next;
        free_waiter(target_idx);
        return;
    }

    // Traverse list to find target
    let mut current_idx = *head;
    while let Some(idx) = current_idx {
        let next = {
            let slot = WAITER_POOL[idx].lock();
            slot.as_ref().and_then(|w| w.next)
        };

        if next == Some(target_idx) {
            // Found predecessor of target
            let target_next = {
                let target_slot = WAITER_POOL[target_idx].lock();
                target_slot.as_ref().and_then(|w| w.next)
            };
            {
                let mut slot = WAITER_POOL[idx].lock();
                if let Some(ref mut w) = *slot {
                    w.next = target_next;
                }
            }
            free_waiter(target_idx);
            return;
        }

        current_idx = next;
    }
}

/// ENOMEM
const ENOMEM: i32 = 12;

/// FUTEX_WAIT_BITSET implementation
pub fn futex_wait_bitset(uaddr: usize, flags: u32, val: u32, _timeout: u64, bitset: u32) -> i64 {
    futex_wait(uaddr, flags, val, bitset)
}

/// FUTEX_WAKE_BITSET implementation
pub fn futex_wake_bitset(uaddr: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    futex_wake(uaddr, flags, nr_wake, bitset)
}

/// Convert FUTEX opcode to internal flags
pub fn futex_to_flags(op: u32) -> u32 {
    let mut flags = 0u32;

    if (op & FUTEX_PRIVATE_FLAG as u32) == 0 {
        flags |= FLAGS_SHARED;
    }

    if (op & FUTEX_CLOCK_REALTIME as u32) != 0 {
        flags |= FLAGS_CLOCKRT;
    }

    flags
}

/// do_futex - main dispatch function
pub fn do_futex(uaddr: usize, op: i32, val: u32, _timeout: u64, uaddr2: usize, val2: u32, val3: u32) -> i64 {
    let flags = futex_to_flags(op as u32);
    let cmd = op & FUTEX_CMD_MASK;

    match cmd {
        FUTEX_WAIT => {
            futex_wait(uaddr, flags, val, FUTEX_BITSET_MATCH_ANY)
        }
        FUTEX_WAKE => {
            futex_wake(uaddr, flags, val as i32, FUTEX_BITSET_MATCH_ANY)
        }
        FUTEX_WAIT_BITSET => {
            futex_wait_bitset(uaddr, flags, val, _timeout, val3)
        }
        FUTEX_WAKE_BITSET => {
            futex_wake_bitset(uaddr, flags, val as i32, val3)
        }
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            // Simplified implementation: only wake, no requeue
            futex_wake(uaddr, flags, val as i32, FUTEX_BITSET_MATCH_ANY)
        }
        FUTEX_WAKE_OP => {
            // Simplified implementation
            futex_wake(uaddr, flags, val as i32, FUTEX_BITSET_MATCH_ANY)
        }
        _ => {
            // PI-related operations not yet supported
            -ENOSYS as i64
        }
    }
}

/// sys_futex system call entry point
pub fn sys_futex_handler(args: &[u64; 6]) -> i64 {
    let uaddr = args[0] as usize;
    let op = args[1] as i32;
    let val = args[2] as u32;
    let timeout = args[3];
    let uaddr2 = args[4] as usize;
    let val3 = args[5] as u32;

    do_futex(uaddr, op, val, timeout, uaddr2, 0, val3)
}
