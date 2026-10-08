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

        // C8 wake-ordering invariant (sleeper side), same for
        // wait_interruptible below: (1) `prepare_to_wait` links our entry
        // AND (2) sets the task state to INTERRUPTIBLE with Release — both
        // under the wait-queue lock — strictly BEFORE (3) we release the
        // mutex and call schedule(). Reordering loses wakeups: if the
        // state store happened after the unlock, a signal() arriving in
        // between would find the entry but read the task as still
        // RUNNING, consume the wake by marking the entry woken, and never
        // schedule us — we would then set INTERRUPTIBLE and sleep forever
        // (a store to our state word wakes no one). See
        // WaitQueueHead::prepare_to_wait / wake_up in process/wait.rs.
        //
        // 1. Atomically add to wait queue AND set INTERRUPTIBLE under the
        //    waitqueue lock, preventing the lost-wakeup race where signal()
        //    fires between unlock() and add()/set_state().
        self.wait.prepare_to_wait(current, false, true);

        // 2. Release mutex — any concurrent signal() will now see us in the
        //    waitqueue and wake us up.
        mutex.unlock();

        // 3. Yield CPU — task removed from runqueue by __schedule()
        crate::arch::cpu::restore_irq(true);
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

        // C8 wake-ordering invariant (sleeper side): identical ordering to
        // wait() above — prepare_to_wait links the entry and stores
        // INTERRUPTIBLE with Release under the wait-queue lock BEFORE the
        // mutex is released and schedule() runs; reordering any of it
        // loses wakeups (see wait() for the full argument).
        //
        // 1. Atomically add to wait queue AND set INTERRUPTIBLE under the
        //    waitqueue lock, preventing the lost-wakeup race where signal()
        //    fires between unlock() and add()/set_state().
        self.wait.prepare_to_wait(current, false, true);

        // 2. Release mutex — any concurrent signal() will now see us in the
        //    waitqueue and wake us up.
        mutex.unlock();

        // 3. Yield CPU — task removed from runqueue by __schedule()
        crate::arch::cpu::restore_irq(true);
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
        // C8 wake-ordering invariant (waker side), shared with broadcast():
        // the underlying WaitQueueHead::wake_up runs, under the wait-queue
        // lock, (1) extract the waiter's task pointer from its entry, then
        // (2) set the entry's `woken` flag with Release, then (3) invoke
        // wake_up_process — in that order. Reordering loses wakeups: the
        // moment wake_up_process runs, the waiter is schedulable; if the
        // flag store came after, the waiter could return from schedule(),
        // observe woken == false, treat the wake as spurious, re-check its
        // condition (still false — the signaler has not made it true for
        // us) and go back to sleep. The later flag store wakes no one: it
        // schedules nothing, and with no second signal the waiter sleeps
        // forever despite having been woken once.
        //
        // Application-level mirror of the same invariant (caller's duty):
        // establish the condition BEFORE signaling, both under the mutex.
        // Signal-then-mutate is the same lost wakeup one level up — the
        // woken waiter re-acquires the mutex, re-checks, sees the old
        // condition, and sleeps again.
        //
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
        // C8 wake-ordering invariant (waker side): same extract-waker →
        // set-woken(Release) → wake_up_process order as signal() above,
        // applied to every entry; see signal() for why reordering loses
        // wakeups.
        //
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
