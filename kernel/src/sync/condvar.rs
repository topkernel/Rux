//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Condition Variable Mechanism
//!
//! Core concepts:
//! - Condition variables are used for inter-process synchronization
//! - Must be used together with a mutex
//! - wait() releases lock and waits for condition to be satisfied
//! - signal() wakes one waiting process
//! - broadcast() wakes all waiting processes

use crate::process::wait::WaitQueueHead;

/// Condition Variable
///
/// Condition variables are used for inter-process synchronization, typical use cases:
/// - Producer-consumer pattern
/// - Buffer full/empty notification
/// - Event completion notification
///
/// # Example
/// ```no_run
/// # use kernel::sync::{Mutex, ConditionVariable};
/// # fn test(mutex: &Mutex, cond: &ConditionVariable) {
/// // Acquire lock
/// mutex.lock();
///
/// // Check condition
/// while !condition_is_met() {
///     cond.wait(mutex);  // Release lock and wait
/// }
///
/// // ... critical section ...
///
/// // Release lock
/// mutex.unlock();
///
/// // In another thread:
/// mutex.lock();
/// // ... modify condition ...
/// cond.signal();  // or broadcast()
/// mutex.unlock();
/// # }
/// ```
#[repr(C)]
pub struct ConditionVariable {
    /// Wait queue
    wait: WaitQueueHead,
}

impl ConditionVariable {
    /// Create a new condition variable
    ///
    /// # Example
    /// ```
    /// let cond = ConditionVariable::new();
    /// ```
    pub const fn new() -> Self {
        Self {
            wait: WaitQueueHead::new(),
        }
    }

    /// Initialize condition variable (runtime initialization)
    pub fn init(&self) {
        // WaitQueueHead is already automatically initialized
    }

    /// Wait for condition to be satisfied (non-interruptible)
    ///
    /// # Arguments
    /// * `mutex` - Associated mutex
    ///
    /// # Behavior
    /// 1. Atomically release mutex
    /// 2. Add to wait queue
    /// 3. Yield CPU, go to sleep
    /// 4. Re-acquire mutex after being woken
    /// 5. Return
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::{Mutex, ConditionVariable};
    /// # fn test(mutex: &Mutex, cond: &ConditionVariable) {
    /// mutex.lock();
    /// while !condition_is_met() {
    ///     cond.wait(mutex);
    /// }
    /// // ... condition is met, can safely execute operations ...
    /// mutex.unlock();
    /// # }
    /// ```
    pub fn wait(&self, mutex: &super::Mutex) {
        let current = match crate::sched::current() {
            Some(task) => task,
            None => return,
        };

        // 1. Atomically add to wait queue AND set INTERRUPTIBLE under the
        //    waitqueue lock, preventing the lost-wakeup race where signal()
        //    fires between unlock() and add()/set_state().
        self.wait.prepare_to_wait(current, false, true);

        // 2. Release mutex — any concurrent signal() will now see us in the
        //    waitqueue and wake us up.
        mutex.unlock();

        // 3. Yield CPU — task removed from runqueue by __schedule()
        crate::arch::riscv64::cpu::restore_irq(true);
        crate::sched::schedule();

        // 4. After wakeup, finish_wait restores RUNNING and removes entry.
        self.wait.finish_wait(current);

        // 5. Re-acquire mutex
        mutex.lock();
    }

    /// Wait for condition to be satisfied (interruptible)
    ///
    /// # Arguments
    /// * `mutex` - Associated mutex
    ///
    /// # Returns
    /// * `Ok(())` - Condition satisfied
    /// * `Err(())` - Interrupted by signal
    ///
    /// # Behavior
    /// 1. Atomically release mutex
    /// 2. Add to wait queue
    /// 3. Yield CPU, go to sleep
    /// 4. Re-acquire mutex after being woken or interrupted by signal
    /// 5. Return result
    pub fn wait_interruptible(&self, mutex: &super::Mutex) -> Result<(), ()> {
        let current = match crate::sched::current() {
            Some(task) => task,
            None => return Ok(()),
        };

        // 1. Atomically add to wait queue AND set INTERRUPTIBLE under the
        //    waitqueue lock, preventing the lost-wakeup race where signal()
        //    fires between unlock() and add()/set_state().
        self.wait.prepare_to_wait(current, false, true);

        // 2. Release mutex — any concurrent signal() will now see us in the
        //    waitqueue and wake us up.
        mutex.unlock();

        // 3. Yield CPU — task removed from runqueue by __schedule()
        crate::arch::riscv64::cpu::restore_irq(true);
        crate::sched::schedule();

        // 4. After wakeup, finish_wait restores RUNNING and removes entry.
        self.wait.finish_wait(current);

        // Check for signal interruption
        if crate::signal::signal_pending() {
            mutex.lock();
            return Err(());
        }

        // 5. Re-acquire mutex
        mutex.lock();

        Ok(())
    }

    /// Wake one waiting process
    ///
    /// # Behavior
    /// Wake one process in the wait queue (if any)
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::{Mutex, ConditionVariable};
    /// # fn test(mutex: &Mutex, cond: &ConditionVariable) {
    /// // Modify condition
    /// mutex.lock();
    /// condition = true;
    /// cond.signal();  // Wake one waiter
    /// mutex.unlock();
    /// # }
    /// ```
    pub fn signal(&self) {
        // Wake one process (using exclusive mode)
        self.wait.wake_up_one();
    }

    /// Wake all waiting processes
    ///
    /// # Behavior
    /// Wake all processes in the wait queue
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::{Mutex, ConditionVariable};
    /// # fn test(mutex: &Mutex, cond: &ConditionVariable) {
    /// // Modify condition (may satisfy multiple waiters)
    /// mutex.lock();
    /// buffer.clear();
    /// cond.broadcast();  // Wake all waiters
    /// mutex.unlock();
    /// # }
    /// ```
    pub fn broadcast(&self) {
        // Wake all processes
        self.wait.wake_up_all();
    }
}

/// Default implementation
impl Default for ConditionVariable {
    fn default() -> Self {
        Self::new()
    }
}
