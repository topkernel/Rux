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
use spin::Mutex;

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
    list: Mutex<Vec<WaitQueueEntry>>,
}

impl WaitQueueHead {
    /// Create new wait queue head
    pub const fn new() -> Self {
        Self {
            list: Mutex::new(Vec::new()),
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
        let mut list = self.list.lock();
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
        let mut list = self.list.lock();
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
        let list = self.list.lock();
        let mut awakened = 0;

        // Determine max wake count
        let max_wake = if nr == 0 { usize::MAX } else { nr };

        // Wake from list head
        for entry in list.iter() {
            if awakened >= max_wake {
                break;
            }

            if !entry.is_woken() {
                entry.set_woken();

                // Actually wake the process by adding it to run queue
                let task = entry.task();
                if !task.is_null() {
                    crate::sched::wake_up_process(task);
                }

                awakened += 1;

                // Exclusive mode: only wake one
                if entry.is_exclusive() {
                    break;
                }
            }
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

            // Condition not met, add to wait queue
            let current = match crate::sched::current() {
                Some(task) => task,
                None => break,
            };

            let entry = $crate::process::wait::WaitQueueEntry::new(current, false);

            // Add to wait queue
            wq_head.add(entry);

            // Release kernel lock (must release before sleep)
            crate::sync::kernel_lock_release();

            // Yield CPU
            crate::sched::schedule();

            // Re-acquire kernel lock after wakeup
            crate::sync::kernel_lock_acquire();

            // Remove from wait queue after wakeup
            wq_head.remove(current);

            // Re-check condition
        }
    }};
}

#[macro_export]
macro_rules! wait_event_interruptible {
    ($wq_head:expr, $condition:expr) => {{
        let wq_head = $wq_head;
        loop {
            // Check condition
            if $condition {
                break true;
            }

            // Check for pending signals
            if crate::signal::signal_pending() {
                break false;
            }

            // Condition not met, add to wait queue
            let current = match crate::sched::current() {
                Some(task) => task,
                None => break true,
            };

            let entry = $crate::process::wait::WaitQueueEntry::new(current, false);

            // Add to wait queue
            wq_head.add(entry);

            // Release kernel lock (must release before sleep)
            crate::sync::kernel_lock_release();

            // Yield CPU
            crate::sched::schedule();

            // Re-acquire kernel lock after wakeup
            crate::sync::kernel_lock_acquire();

            // Remove from wait queue after wakeup
            wq_head.remove(current);

            // Re-check condition
        }
    }};
}
