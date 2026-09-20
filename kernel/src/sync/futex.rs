//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Futex Implementation - Fast Userspace Mutex
//!
//! # Design
//! - Static waiter pool (spinlock-protected slots) for zero-allocation futex
//! - Static hash table mapping FutexKey → waiter chain (singly-linked list)
//! - All locks use `lock_irqsave()` for interrupt safety
//! - Wake uses `Task::wake_up()` (enqueue + resched) for correct scheduling
//! - Wait inserts into chain then sets INTERRUPTIBLE under lock to prevent lost wakeup

use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::process::Task;
use crate::process::task::TaskState;
use crate::syscall::errno::{EINVAL, EFAULT, EAGAIN, ENOSYS, ETIMEDOUT};

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
        if !(self.flags & FLAGS_SHARED != 0) {
            self.uaddr == other.uaddr && self.pid == other.pid
        } else {
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
    /// Next waiter in hash chain
    next: Option<usize>,
}

// SAFETY: Waiter is only ever accessed while the per-slot Spinlock is held,
// serialising all reads and writes.  The `task` raw pointer is only
// dereferenced by the waker after validating it is non-null.
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
        let mut slot = WAITER_POOL[i].lock_irqsave();
        if slot.is_none() {
            // Reserve INSIDE the critical section: leaving the slot empty
            // until the caller initializes it let two CPUs hand out the
            // same index (review IPC-C2). A placeholder (task == null)
            // makes the slot visibly occupied; chain walkers skip null-task
            // waiters via the key/task checks below.
            *slot = Some(Waiter {
                key: FutexKey::new(0, 0, 0),
                task: core::ptr::null_mut(),
                bitset: 0,
                woken: false,
                next: None,
            });
            return Some(i);
        }
    }
    None
}

/// Free a waiter slot
fn free_waiter(index: usize) {
    let mut slot = WAITER_POOL[index].lock_irqsave();
    *slot = None;
}

/// Calculate futex hash value
fn futex_hash(key: &FutexKey) -> usize {
    // R31-9: shared futexes match on uaddr alone (matches() ignores pid)
    // — including pid in the hash put the waiter and waker in different
    // buckets (cross-process lost wakeup over SysV shm / MAP_SHARED).
    if key.flags & FLAGS_SHARED != 0 {
        key.uaddr % HASH_SIZE
    } else {
        (key.uaddr.wrapping_add(key.pid as usize)) % HASH_SIZE
    }
}

/// Wake up waiters on a futex
///
/// Walks the hash chain for the given futex, waking up to `nr_wake` tasks
/// whose bitset intersects with the requested bitset.  Uses
/// `Task::wake_up()` which properly enqueues the task on the run queue
/// and triggers rescheduling on the target CPU.
pub fn futex_wake(uaddr: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    if bitset == 0 {
        return -EINVAL as i64;
    }

    let pid = match crate::sched::current() {
        // SAFETY: sched::current() returns the current task's raw pointer,
        // valid for the duration of this syscall.
        Some(t) => unsafe { (*t).pid() },
        None => return -EFAULT as i64,
    };

    let key = FutexKey::new(uaddr, pid, flags);
    let bucket_idx = futex_hash(&key);

    let mut ret = 0i64;
    let mut prev_idx: Option<usize> = None;
    // Collect tasks to wake after releasing the bucket lock, like
    // kernel/futex/waitwake.c wake_futex() + wake_q_add(). Dynamically
    // sized: a fixed 8-entry array silently stranded every waiter past the
    // 8th (pthread_cond_broadcast) — review IPC-C3.
    let mut wake_list: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();

    // Hold the hash bucket lock for the entire traversal so no
    // concurrent futex_wait can insert/remove while we walk.
    let mut head = HASH_HEADS[bucket_idx].lock_irqsave();

    let mut current_idx = *head;
    while let Some(idx) = current_idx {
        if ret >= nr_wake as i64 {
            break;
        }

        let mut waiter_slot = WAITER_POOL[idx].lock_irqsave();
        let should_wake = match *waiter_slot {
            Some(ref waiter) => {
                waiter.key.matches(&key) && (waiter.bitset & bitset) != 0
            }
            None => break,
        };

        if should_wake {
            let woken_task = match *waiter_slot {
                Some(ref waiter) => waiter.task,
                None => break,
            };
            let next_idx = match *waiter_slot {
                Some(ref waiter) => waiter.next,
                None => break,
            };

            // Mark woken so futex_wait knows it was explicitly woken.
            if let Some(ref mut w) = *waiter_slot {
                w.woken = true;
            }
            drop(waiter_slot);

            // Unlink from chain.
            if prev_idx.is_none() {
                *head = next_idx;
            } else if let Some(prev) = prev_idx {
                let mut prev_slot = WAITER_POOL[prev].lock_irqsave();
                if let Some(ref mut pw) = *prev_slot {
                    pw.next = next_idx;
                }
            }

            // Do NOT free the waiter slot here: the sleeping task still
            // inspects it after waking and frees it itself (review IPC-H4).
            // Defer wakeup — collect task pointer, wake after dropping lock.
            if !woken_task.is_null() {
                wake_list.push(woken_task);
            }

            ret += 1;
            current_idx = next_idx;
        } else {
            prev_idx = Some(idx);
            current_idx = match *waiter_slot {
                Some(ref waiter) => waiter.next,
                None => break,
            };
        }
    }

    // R12-2: wake WHILE STILL HOLDING the bucket lock — same reasoning as
    // WaitQueueHead::wake_up (R12-1): the waiter is still linked here, so
    // it cannot have passed its unlink-under-this-lock and exited. Bucket
    // -> GRQ order is safe (no GRQ-held path takes a futex bucket). The
    // old drop-then-wake window was the deferred-wake UAF.
    for task in wake_list {
        if !task.is_null() {
            Task::wake_up(task);
        }
    }
    drop(head);

    ret
}

/// Wait for a futex
///
/// Checks `*uaddr` under the hash bucket lock.  If it still equals `val`,
/// inserts a waiter into the chain, sets state to INTERRUPTIBLE (still under
/// the lock), then drops the lock and schedules.  The "insert + set state
/// under lock" ordering prevents the lost-wakeup race: by the time
/// futex_wake sees the waiter, the task is already in INTERRUPTIBLE state
/// so `Task::wake_up()` (which checks `is_sleeping()`) can succeed.
pub fn futex_wait(uaddr: usize, flags: u32, val: u32, bitset: u32) -> i64 {
    futex_wait_timeout(uaddr, flags, val, bitset, None)
}

/// FUTEX_WAIT with an optional jiffies deadline (see do_futex).
pub fn futex_wait_timeout(uaddr: usize, flags: u32, val: u32, bitset: u32, deadline: Option<u64>) -> i64 {
    if bitset == 0 {
        return -EINVAL as i64;
    }

    let uaddr_ptr = uaddr as *const AtomicU32;
    if uaddr_ptr.is_null() {
        return -EINVAL as i64;
    }

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -EFAULT as i64,
    };
    // SAFETY: current is the current task's raw pointer from sched::current(),
    // valid for the duration of this syscall.
    let pid = unsafe { (*current).pid() };

    let key = FutexKey::new(uaddr, pid, flags);
    let bucket_idx = futex_hash(&key);

    // Lock the hash bucket.  All subsequent operations (value check,
    // waiter insertion, state change) happen under this lock.
    let mut head = HASH_HEADS[bucket_idx].lock_irqsave();

    // Re-check value under lock (prevents lost wakeup).
    // SAFETY: get_user goes through the exception-table copy path; an
    // unmapped user address yields EFAULT instead of a kernel page fault.
    let uval = match unsafe {
        crate::arch::riscv64::uaccess::get_user(uaddr_ptr as *const u32)
    } {
        Some(v) => v,
        None => return -EFAULT as i64,
    };
    if uval != val {
        return -EAGAIN as i64;
    }

    // Allocate waiter slot.
    let waiter_idx = match alloc_waiter() {
        Some(idx) => idx,
        None => return -ENOMEM as i64,
    };

    // Initialize and insert waiter into hash chain (fill the placeholder
    // reserved by alloc_waiter in place).
    {
        let mut slot = WAITER_POOL[waiter_idx].lock_irqsave();
        if let Some(ref mut w) = *slot {
            w.key = key;
            w.task = current;
            w.bitset = bitset;
            w.woken = false;
            w.next = *head;
        }
    }

    // Update chain head.
    *head = Some(waiter_idx);

    // Set task state to INTERRUPTIBLE while still holding the hash lock.
    // This guarantees that any futex_wake that sees the waiter in the chain
    // will also see the task in INTERRUPTIBLE state, preventing the
    // lost-wakeup race.
    // SAFETY: current is the current task, valid for the duration of this
    // function.  We hold the hash bucket lock so futex_wake will see the
    // state transition before checking is_sleeping().
    unsafe {
        (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    }

    // Release the hash bucket lock.  The Release semantics ensure that
    // the waiter entry (chain + INTERRUPTIBLE state) is visible to other
    // CPUs before they can observe the lock is free.
    drop(head);

    // Schedule — yields the CPU.  The task will be re-enqueued by
    // Task::wake_up() when futex_wake (or a signal) wakes it.  Arm a
    // wakeup timer when a deadline is set: nothing else would wake a
    // futex that is never signaled (pthread_cond_timedwait would hang).
    crate::arch::riscv64::cpu::restore_irq(true);
    let timer_id = deadline
        .map(|dl| crate::timer::add_timer_wakeup(
            dl, crate::sched::get_current_pid(),
        ))
        .unwrap_or(0);
    // R32 (NEW-3): if a deadline was requested but the timer pool is
    // exhausted (add_timer_wakeup → 0), schedule() below would sleep
    // FOREVER — nothing else wakes an un-signaled futex, so the timed
    // wait degenerated into an untimed hang. Unlink and fail instead.
    if deadline.is_some() && timer_id == 0 {
        remove_waiter(bucket_idx, waiter_idx);
        return -ENOMEM as i64;
    }
    crate::sched::schedule();
    if timer_id != 0 {
        crate::timer::del_timer(timer_id);
    }

    // Check for signal interruption (EINTR). Ownership guard: only act on
    // the slot if it still belongs to us (task pointer matches).
    {
        let slot = WAITER_POOL[waiter_idx].lock_irqsave();
        let mine = slot.as_ref().map(|w| w.task == current).unwrap_or(false);
        drop(slot);
        if !mine {
            // Slot was recycled underneath us (should not happen now that
            // the waker never frees slots, but stay defensive).
            return 0;
        }
        if crate::signal::signal_pending() {
            let woken = {
                let slot = WAITER_POOL[waiter_idx].lock_irqsave();
                slot.as_ref().map(|w| w.woken).unwrap_or(false)
            };
            if !woken {
                remove_waiter(bucket_idx, waiter_idx);
                return -crate::syscall::errno::EINTR as i64;
            }
        }
    }

    // After waking up, check if we were explicitly woken. The waiter owns
    // its slot: unlink paths that did not wake us leave it in the chain
    // (remove), the waker unlinked it already (just free the slot).
    {
        let mut slot = WAITER_POOL[waiter_idx].lock_irqsave();
        let mine = slot.as_ref().map(|w| w.task == current).unwrap_or(false);
        if mine {
            let was_woken = slot.as_ref().map(|w| w.woken).unwrap_or(false);
            if !was_woken {
                // Not explicitly woken (spurious wakeup or the timeout
                // timer): still in the chain.
                drop(slot);
                remove_waiter(bucket_idx, waiter_idx);
                // Timeout semantics (review 2R.9): if we were not woken and
                // the deadline has passed, this is a genuine ETIMEDOUT —
                // returning success here broke every timed waiter.
                if let Some(dl) = deadline {
                    if crate::drivers::timer::get_jiffies() >= dl {
                        return -ETIMEDOUT as i64;
                    }
                }
            } else {
                // Woken: the waker unlinked us from the chain and left the
                // slot for us to free.
                *slot = None;
            }
        }
    }

    0
}

/// Remove waiter from hash chain.
fn remove_waiter(bucket_idx: usize, target_idx: usize) {
    let mut head = HASH_HEADS[bucket_idx].lock_irqsave();

    if *head == Some(target_idx) {
        let next = {
            let slot = WAITER_POOL[target_idx].lock_irqsave();
            slot.as_ref().and_then(|w| w.next)
        };
        *head = next;
        free_waiter(target_idx);
        return;
    }

    let mut current_idx = *head;
    while let Some(idx) = current_idx {
        let next = {
            let slot = WAITER_POOL[idx].lock_irqsave();
            slot.as_ref().and_then(|w| w.next)
        };

        if next == Some(target_idx) {
            let target_next = {
                let target_slot = WAITER_POOL[target_idx].lock_irqsave();
                target_slot.as_ref().and_then(|w| w.next)
            };
            {
                let mut slot = WAITER_POOL[idx].lock_irqsave();
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

/// Clean up all futex waiters for a given task.
///
/// Called from `do_exit` so that no dangling waiter entries remain in
/// the hash chains after the task is freed.  Wakes the task (so it can
/// continue exiting) and frees all its waiter slots.
pub fn futex_cleanup(task: *mut Task) {
    if task.is_null() {
        return;
    }
    // SAFETY: task is non-null (checked above); caller (do_exit) guarantees
    // the task pointer is valid during cleanup.
    let task_pid = unsafe { (*task).pid() };

    for bucket_idx in 0..HASH_SIZE {
        let mut head = HASH_HEADS[bucket_idx].lock_irqsave();
        let mut prev_idx: Option<usize> = None;
        let mut current_idx = *head;

        while let Some(idx) = current_idx {
            let remove = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                slot.as_ref().map_or(false, |w| {
                    w.key.pid == task_pid && w.task == task
                })
            };

            if remove {
                // Unlink from chain.
                let next = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().and_then(|w| w.next)
                };
                if prev_idx.is_none() {
                    *head = next;
                } else if let Some(prev) = prev_idx {
                    let mut prev_slot = WAITER_POOL[prev].lock_irqsave();
                    if let Some(ref mut pw) = *prev_slot {
                        pw.next = next;
                    }
                }
                free_waiter(idx);
                current_idx = next;
            } else {
                prev_idx = Some(idx);
                current_idx = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().and_then(|w| w.next)
                };
            }
        }
        // Release bucket lock here (end of iteration — `head` dropped on next loop or at end).
        drop(head);
    }

    // Wake the task so it can continue the exit path — done outside all bucket locks.
    Task::wake_up(task);
}

/// ENOMEM
const ENOMEM: i32 = 12;

/// FUTEX_WAIT_BITSET implementation
pub fn futex_wait_bitset(uaddr: usize, flags: u32, val: u32, _timeout: u64, bitset: u32, deadline: Option<u64>) -> i64 {
    futex_wait_timeout(uaddr, flags, val, bitset, deadline)
}

/// Parse the futex ABI `struct timespec *timeout` (raw user pointer) into a
/// jiffies deadline. NULL and unreadable pointers yield None (= wait forever).
/// `absolute`: FUTEX_WAIT_BITSET (and FUTEX_WAIT|FUTEX_CLOCK_REALTIME) pass
/// an absolute timespec; CLOCK_REALTIME here counts from boot (CLINT cycles
/// / TIMER_CLOCK_FREQ_HZ) and so does jiffies, so the conversion needs no
/// offset. Plain FUTEX_WAIT passes a relative duration (round 6 MED: was
/// always relative, so every pthread_cond_timedwait fired instantly or
/// never).
fn futex_parse_timeout(timeout_ptr: u64, absolute: bool) -> Option<u64> {
    use crate::drivers::timer::{get_jiffies, HZ};
    if timeout_ptr == 0 {
        return None;
    }
    if !crate::arch::riscv64::uaccess::access_ok(timeout_ptr as usize, 16) {
        return None;
    }
    let mut buf = [0u8; 16];
    // SAFETY: access_ok-validated user pointer; exception-table copy.
    let uncopied = unsafe {
        crate::arch::riscv64::uaccess::copy_from_user(
            buf.as_mut_ptr(), timeout_ptr as *const u8, 16,
        )
    };
    if uncopied > 0 {
        return None;
    }
    let sec = i64::from_le_bytes(buf[0..8].try_into().unwrap());
    let nsec = i64::from_le_bytes(buf[8..16].try_into().unwrap());
    if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
        return None;
    }
    let jiffies = (sec as u64).saturating_mul(HZ)
        .saturating_add((nsec as u64 * HZ) / 1_000_000_000);
    if absolute {
        // Absolute CLOCK_REALTIME value; if already past, the min-1 clamp
        // arms an immediately-expiring timer → ETIMEDOUT on wake check.
        Some(jiffies.max(1))
    } else {
        Some(get_jiffies().saturating_add(jiffies.max(1)))
    }
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

/// FUTEX_REQUEUE / FUTEX_CMP_REQUEUE implementation.
///
/// Wakes up to `nr_wake` waiters on `uaddr`, then requeues up to `nr_requeue`
/// remaining waiters from `uaddr` to `uaddr2`.  For CMP_REQUEUE, verifies
/// `*uaddr == cmpval` first.
///
/// Returns the total number of waiters woken + requeued, or a negative errno.
pub fn futex_requeue(
    uaddr: usize,
    flags: u32,
    nr_wake: i32,
    nr_requeue: i32,
    uaddr2: usize,
    cmpval: u32,
    is_cmp: bool,
) -> i64 {
    let pid = match crate::sched::current() {
        Some(t) => unsafe { (*t).pid() },
        None => return -EFAULT as i64,
    };

    let key1 = FutexKey::new(uaddr, pid, flags);

    // For CMP_REQUEUE, verify *uaddr == cmpval
    if is_cmp {
        // SAFETY: exception-table protected read; unmapped address → EFAULT.
        let uaddr_ptr = uaddr as *const u32;
        let uval = match unsafe {
            crate::arch::riscv64::uaccess::get_user(uaddr_ptr)
        } {
            Some(v) => v,
            None => return -EFAULT as i64,
        };
        if uval != cmpval {
            return -EAGAIN as i64;
        }
    }

    // No requeue target or same address → just wake
    if uaddr2 == 0 || uaddr2 == uaddr || nr_requeue <= 0 {
        return futex_wake(uaddr, flags, nr_wake, FUTEX_BITSET_MATCH_ANY);
    }

    let key2 = FutexKey::new(uaddr2, pid, flags);
    let bucket1 = futex_hash(&key1);
    let bucket2 = futex_hash(&key2);

    let mut ret = 0i64;
    let mut woken = 0i32;

    // Collect tasks to wake and waiter indices to requeue. Dynamically
    // sized: fixed 8/32-entry arrays silently stranded waiters past their
    // capacity (pthread_cond_broadcast) — review IPC-C3/M7.
    let mut wake_list: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();
    let mut requeue_list: alloc::vec::Vec<usize> = alloc::vec::Vec::new();

    // Lock both buckets to prevent requeued entries from being in limbo.
    // Use address ordering (lower index first) to avoid ABBA deadlock.
    if bucket1 == bucket2 {
        // Same bucket — lock once.
        let mut head1 = HASH_HEADS[bucket1].lock_irqsave();

        let mut prev: Option<usize> = None;
        let mut cur = *head1;

        while let Some(idx) = cur {
            let (matches, next) = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                match *slot {
                    Some(ref w) => (w.key.matches(&key1), w.next),
                    None => break,
                }
            };

            if !matches {
                prev = Some(idx);
                cur = next;
                continue;
            }

            if woken < nr_wake {
                let task = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().map(|w| w.task).unwrap_or(core::ptr::null_mut())
                };
                if prev.is_none() {
                    *head1 = next;
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
                }
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot { w.woken = true; }
                }
                // The woken waiter frees its own slot (see futex_wait).

                if !task.is_null() {
                    wake_list.push(task);
                }
                woken += 1;
                ret += 1;
                cur = next;
            } else if (requeue_list.len() as i32) < nr_requeue
            {
                // Same bucket — just update the key, stay in chain.
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot {
                        w.key = key2;
                    }
                }
                requeue_list.push(idx);
                ret += 1;
                prev = Some(idx);
                cur = next;
            } else {
                break;
            }
        }
    } else {
        // Different buckets — lock both with deadlock-avoidance ordering.
        let (lo, hi) = if bucket1 < bucket2 {
            (bucket1, bucket2)
        } else {
            (bucket2, bucket1)
        };

        let mut guard_lo = HASH_HEADS[lo].lock_irqsave();
        let mut guard_hi = HASH_HEADS[hi].lock_irqsave();

        // Get references to the actual heads we need.
        let (head1_ref, head2_ref) = if bucket1 < bucket2 {
            (&mut *guard_lo, &mut *guard_hi)
        } else {
            (&mut *guard_hi, &mut *guard_lo)
        };

        let mut prev: Option<usize> = None;
        let mut cur = *head1_ref;

        while let Some(idx) = cur {
            let (matches, next) = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                match *slot {
                    Some(ref w) => (w.key.matches(&key1), w.next),
                    None => break,
                }
            };

            if !matches {
                prev = Some(idx);
                cur = next;
                continue;
            }

            if woken < nr_wake {
                let task = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().map(|w| w.task).unwrap_or(core::ptr::null_mut())
                };
                if prev.is_none() {
                    *head1_ref = next;
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
                }
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot { w.woken = true; }
                }
                // The woken waiter frees its own slot (see futex_wait).

                if !task.is_null() {
                    wake_list.push(task);
                }
                woken += 1;
                ret += 1;
                cur = next;
            } else if (requeue_list.len() as i32) < nr_requeue
            {
                // Unlink from source chain.
                if prev.is_none() {
                    *head1_ref = next;
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
                }
                // Update key and insert into destination chain immediately.
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot {
                        w.key = key2;
                        w.next = *head2_ref;
                    }
                }
                *head2_ref = Some(idx);

                requeue_list.push(idx);
                ret += 1;
                cur = next;
            } else {
                break;
            }
        }
        drop(guard_lo);
        drop(guard_hi);
    }

    // R12-2: the waiters were unlinked from their chains under the bucket
    // locks in the collecting pass; unlike futex_wake we are already past
    // those critical sections, so the PID revalidation stays as defense
    // in depth against the cross-CPU reap window.
    for task in wake_list {
        if !task.is_null() {
            let pid = unsafe { (*task).pid() };
            let fresh = crate::process::pid_hash::pid_hash_lookup(pid);
            if fresh == task {
                Task::wake_up(task);
            }
        }
    }

    ret
}

/// do_futex - main dispatch function
pub fn do_futex(uaddr: usize, op: i32, val: u32, _timeout: u64, uaddr2: usize, val2: u32, val3: u32) -> i64 {
    let flags = futex_to_flags(op as u32);
    let cmd = op & FUTEX_CMD_MASK;

    match cmd {
        FUTEX_WAIT => {
            // R7-A9: plain FUTEX_WAIT keeps a RELATIVE timeout even with
            // FUTEX_CLOCK_REALTIME set (Linux futex_init_timeout adds
            // ktime_get() for cmd == FUTEX_WAIT regardless of the flag);
            // only WAIT_BITSET interprets it absolutely.
            futex_wait_timeout(uaddr, flags, val, FUTEX_BITSET_MATCH_ANY, futex_parse_timeout(_timeout, false))
        }
        FUTEX_WAKE => {
            futex_wake(uaddr, flags, val as i32, FUTEX_BITSET_MATCH_ANY)
        }
        FUTEX_WAIT_BITSET => {
            // WAIT_BITSET always interprets timeout as absolute time.
            futex_wait_bitset(uaddr, flags, val, _timeout, val3, futex_parse_timeout(_timeout, true))
        }
        FUTEX_WAKE_BITSET => {
            futex_wake_bitset(uaddr, flags, val as i32, val3)
        }
        FUTEX_REQUEUE => {
            // _timeout is repurposed as nr_requeue in the futex ABI.
            let nr_requeue = _timeout as i32;
            futex_requeue(uaddr, flags, val as i32, nr_requeue, uaddr2, 0, false)
        }
        FUTEX_CMP_REQUEUE => {
            let nr_requeue = _timeout as i32;
            futex_requeue(uaddr, flags, val as i32, nr_requeue, uaddr2, val3, true)
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
