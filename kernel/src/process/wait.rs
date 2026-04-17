//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Wait Queue mechanism
//!
//! Core concepts:
//! - Wait queues implement process blocking and waking
//! - When a process needs to wait for a condition, it joins the wait queue and calls schedule()
//! - When the condition is met, wake_up() wakes waiting processes

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use crate::sync::spinlock::Spinlock;

use super::Task;

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum WakeUpHint {
    /// Normal wake up
    Normal = 0,
    /// Async wake up (don't actually wake process, just mark)
    Async = 1,
}

#[repr(C)]
pub struct WaitQueueEntry {
    /// Associated task
    task: *mut Task,
    /// Exclusive flag (WQ_FLAG_EXCLUSIVE)
    exclusive: bool,
    /// Whether woken
    woken: AtomicBool,
}

impl WaitQueueEntry {
    /// Create new wait queue entry
    ///
    /// # Arguments
    /// * `task` - Associated task
    /// * `exclusive` - Whether exclusive mode (mutual exclusion)
    pub fn new(task: *mut Task, exclusive: bool) -> Self {
        Self {
            task,
            exclusive,
            woken: AtomicBool::new(false),
        }
    }

    /// Check if woken
    pub fn is_woken(&self) -> bool {
        self.woken.load(Ordering::Acquire)
    }

    /// Mark as woken
    pub fn set_woken(&self) {
        self.woken.store(true, Ordering::Release);
    }

    /// Get associated task
    pub fn task(&self) -> *mut Task {
        self.task
    }

    /// Check if exclusive mode
    pub fn is_exclusive(&self) -> bool {
        self.exclusive
    }
}

#[repr(C)]
pub struct WaitQueueHead {
    /// Wait queue list
    /// Uses Vec to store waiting processes
    list: Spinlock<Vec<WaitQueueEntry>>,
}

// Safety: WaitQueueHead uses internal Mutex for synchronization.
// The *mut Task pointer is only dereferenced under scheduler lock protection.
unsafe impl Send for WaitQueueHead {}
unsafe impl Sync for WaitQueueHead {}

impl WaitQueueHead {
    /// Create new wait queue head
    pub const fn new() -> Self {
        Self {
            list: Spinlock::new(Vec::new()),
        }
    }

    /// Initialize wait queue head (runtime initialization)
    pub fn init(&self) {
        // Vec is automatically initialized
    }

    /// Add to wait queue
    ///
    /// # Arguments
    /// * `entry` - Wait queue entry
    pub fn add(&self, entry: WaitQueueEntry) {
        // Use lock_irqsave: wake_up is called from IRQ handlers
        let mut list = self.list.lock_irqsave();
        // Non-exclusive entries added to head, exclusive entries added to tail
        if entry.is_exclusive() {
            list.push(entry);
        } else {
            list.insert(0, entry);
        }
    }

    /// Remove from wait queue
    ///
    /// # Arguments
    /// * `task` - Task to remove
    pub fn remove(&self, task: *mut Task) {
        let mut list = self.list.lock_irqsave();
        list.retain(|entry| entry.task() != task);
    }

    /// Wake up processes in wait queue
    ///
    /// # Arguments
    /// * `mode` - Wake mode
    /// * `nr` - Number of processes to wake (0 means wake all)
    ///
    /// # Returns
    /// Actual number of processes woken
    pub fn wake_up(&self, _mode: WakeUpHint, nr: usize) -> usize {
        // Use lock_irqsave: this is called from interrupt handlers.
        // Collect task pointers under the lock, then drop the lock before
        // calling wake_up_process to avoid ABBA deadlock with the GRQ lock
        // (waitqueue lock -> GRQ lock vs. GRQ lock -> waitqueue interaction).
        let list = self.list.lock_irqsave();
        let mut awakened = 0;
        let max_wake = if nr == 0 { usize::MAX } else { nr };

        // Collect tasks to wake while holding the waitqueue lock.
        let mut wake_list: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();

        for entry in list.iter() {
            if awakened >= max_wake {
                break;
            }

            if !entry.is_woken() {
                entry.set_woken();

                let task = entry.task();
                if !task.is_null() {
                    wake_list.push(task);
                }

                awakened += 1;

                if entry.is_exclusive() {
                    break;
                }
            }
        }

        // Drop the waitqueue lock before waking tasks.
        drop(list);

        // Now safely wake each task outside the waitqueue lock.
        for task in wake_list {
            crate::sched::wake_up_process(task);
        }

        awakened
    }

    /// Wake all waiting processes (non-exclusive)
    pub fn wake_up_all(&self) -> usize {
        self.wake_up(WakeUpHint::Normal, 0)
    }

    /// Wake one process (exclusive)
    pub fn wake_up_one(&self) -> usize {
        self.wake_up(WakeUpHint::Normal, 1)
    }

    /// Prepare to wait: atomically add entry to queue AND set task state.
    ///
    /// This prevents the classic lost-wakeup race where a waker finds the
    /// entry on the queue but the task is still RUNNING (is_sleeping()=false),
    /// marks the entry as woken, then the task sets INTERRUPTIBLE and vanishes
    /// from the runqueue — never to be woken again.
    ///
    /// By holding the waitqueue lock across both operations, the waker (which
    /// also holds this lock) will always see a consistent state.
    pub fn prepare_to_wait(&self, task: *mut Task, exclusive: bool, interruptible: bool) {
        let entry = WaitQueueEntry::new(task, exclusive);
        let mut list = self.list.lock_irqsave();

        // Set task state BEFORE adding to queue (both under same lock).
        // Waker holds this same lock, so it will see either:
        //   - task still RUNNING, entry not on queue → no action (correct)
        //   - task INTERRUPTIBLE, entry on queue → wake_up succeeds (correct)
        if interruptible {
            unsafe {
                (*task).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::INTERRUPTIBLE));
            }
        } else {
            unsafe {
                (*task).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::UNINTERRUPTIBLE));
            }
        }

        // Add entry to queue
        if exclusive {
            list.push(entry);
        } else {
            list.insert(0, entry);
        }
        // Lock released here — now waker can see consistent state
    }

    /// Finish wait: remove entry from queue and restore task state to RUNNING.
    ///
    /// Called after schedule() returns (task was woken) or when condition is
    /// already met after prepare_to_wait.
    pub fn finish_wait(&self, task: *mut Task) {
        let mut list = self.list.lock_irqsave();
        list.retain(|entry| entry.task() != task);

        // If task is still sleeping (condition met before schedule, or
        // called from finish path), restore to RUNNING.
        unsafe {
            let state = (*task).state();
            if state.is_sleeping() {
                (*task).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::RUNNING));
            }
        }
    }
}

#[macro_export]
macro_rules! wait_event {
    ($wq_head:expr, $condition:expr) => {{
        let wq_head = $wq_head;
        loop {
            // Check condition
            if $condition {
                break;
            }

            // Condition not met, prepare to wait.
            let current = match crate::sched::current() {
                Some(task) => task,
                None => break,
            };

            // Atomically add to wait queue AND set UNINTERRUPTIBLE state
            // (both under the waitqueue lock) to prevent lost-wakeup race.
            wq_head.prepare_to_wait(current, false, false);

            // Re-check condition after prepare_to_wait (state is now UNINTERRUPTIBLE)
            if $condition {
                wq_head.finish_wait(current);
                break;
            }

            // Yield CPU — task removed from runqueue by __schedule()
            //
            // Enable interrupts before schedule(). We're in syscall context
            // (SIE=0). This ensures lock_irqsave in __schedule saves SIE=1,
            // so when the task is later switched back in, restore_irq restores
            // SIE=1 (via __schedule's unconditional restore_irq(true)).
            crate::arch::riscv64::cpu::restore_irq(true);
            crate::sched::schedule();

            // After wakeup, state is RUNNING (set by enqueue_task_locked)
            // Remove from wait queue and restore state
            wq_head.finish_wait(current);

            // Re-check condition
        }
    }};
}

/// Wait for a condition to become true, interruptible by signals.
///
/// Returns 0 on success (condition met), or -ERESTARTSYS if interrupted by a signal.
#[macro_export]
macro_rules! wait_event_interruptible {
    ($wq_head:expr, $condition:expr) => {{
        let wq_head = $wq_head;
        let _ret = loop {
            // Check condition first (fast path, no locking needed)
            if $condition {
                break 0i32;
            }

            // Check for pending signals
            if crate::signal::signal_pending() {
                break -512i32; // -ERESTARTSYS
            }

            let current = match crate::sched::current() {
                Some(task) => task,
                None => break 0i32,
            };

            // Atomically add to wait queue AND set INTERRUPTIBLE (under lock).
            // This prevents the lost-wakeup race where the waker finds the
            // entry on the queue but the task is still RUNNING, marks the
            // entry woken, then the task sets INTERRUPTIBLE and vanishes.
            wq_head.prepare_to_wait(current, false, true);

            // Re-check condition after prepare_to_wait (state is now INTERRUPTIBLE)
            if $condition {
                // Condition met — restore RUNNING and remove from queue
                wq_head.finish_wait(current);
                break 0i32;
            }

            // Re-check signals after prepare_to_wait
            if crate::signal::signal_pending() {
                wq_head.finish_wait(current);
                break -512i32; // -ERESTARTSYS
            }

            // Enable interrupts before schedule(). We're in syscall context
            // (SIE=0). This ensures lock_irqsave in __schedule saves SIE=1,
            // so when the task is later switched back in, restore_irq restores
            // SIE=1 (via __schedule's unconditional restore_irq(true)).
            crate::arch::riscv64::cpu::restore_irq(true);
            crate::sched::schedule();

            // After wakeup, state is RUNNING (set by enqueue_task_locked).
            // Remove from wait queue.
            wq_head.finish_wait(current);
        };
        _ret
    }};
}
