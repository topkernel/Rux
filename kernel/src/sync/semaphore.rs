//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Semaphore Mechanism
//!
//! Core concepts:
//! - Semaphores are used for process synchronization and mutual exclusion
//! - P operation (down/down_interruptible): acquire semaphore, may block
//! - V operation (up): release semaphore, wake up waiting processes

use core::sync::atomic::{AtomicI32, Ordering};
use crate::process::wait::WaitQueueHead;

/// Semaphore
///
/// A semaphore is a non-negative integer used for process synchronization:
/// - Initialized to some positive integer
/// - P operation (down): decrement value, if 0 then block and wait
/// - V operation (up): increment value, if processes are waiting then wake one
#[repr(C)]
pub struct Semaphore {
    /// Semaphore count value
    /// Use atomic integer to ensure thread safety
    count: AtomicI32,
    /// Wait queue
    /// When semaphore is 0, waiting processes join this queue
    wait: WaitQueueHead,
}

impl Semaphore {
    /// Create a new semaphore
    ///
    /// # Arguments
    /// * `value` - Initial value
    ///
    /// # Example
    /// ```
    /// // Mutex semaphore (binary semaphore)
    /// let mutex = Semaphore::new(1);
    ///
    /// // Counting semaphore (resource pool)
    /// let pool = Semaphore::new(10);
    /// ```
    pub const fn new(value: i32) -> Self {
        Self {
            count: AtomicI32::new(value),
            wait: WaitQueueHead::new(),
        }
    }

    /// Initialize semaphore (runtime initialization)
    ///
    /// # Arguments
    /// * `value` - Initial value
    pub fn init(&self, value: i32) {
        self.count.store(value, Ordering::Release);
        // WaitQueueHead is already automatically initialized
    }

    /// P operation (non-interruptible)
    ///
    /// Also called down operation or wait operation
    ///
    /// # Behavior
    /// - Decrement semaphore value by 1
    /// - If value >= 0, return immediately
    /// - If value < 0, block and wait until value becomes positive
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) {
    /// sem.down();  // Acquire semaphore
    /// // ... critical section ...
    /// sem.up();    // Release semaphore
    /// # }
    /// ```
    pub fn down(&self) {
        // Atomic decrement by 1
        let old = self.count.fetch_sub(1, Ordering::Acquire);

        if old > 0 {
            // Successfully acquired semaphore
            return;
        }

        // Semaphore not available, need to wait
        // Check condition: semaphore value > 0
        let has_semaphore = || self.count.load(Ordering::Acquire) > 0;

        loop {
            if has_semaphore() {
                // Retry acquisition
                let old = self.count.fetch_sub(1, Ordering::Acquire);
                if old > 0 {
                    return;
                }
                // Still failed, continue waiting
                self.count.fetch_add(1, Ordering::Release);
            }

            // Add to wait queue
            let current = match crate::sched::current() {
                Some(task) => task,
                None => return, // Cannot get current task, return directly
            };

            let entry = crate::process::wait::WaitQueueEntry::new(current, false);
            self.wait.add(entry);

            // Set task to UNINTERRUPTIBLE before yielding CPU
            unsafe {
                (*current).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::UNINTERRUPTIBLE));
            }

            // Yield CPU — task removed from runqueue by __schedule()
            crate::arch::riscv64::cpu::restore_irq(true);
            crate::sched::schedule();

            // After waking up, remove from wait queue
            self.wait.remove(current);
        }
    }

    /// P operation (interruptible)
    ///
    /// Also called down_interruptible operation
    ///
    /// # Behavior
    /// - Decrement semaphore value by 1
    /// - If value >= 0, return Ok(()) immediately
    /// - If value < 0, block and wait until value becomes positive or interrupted by signal
    ///
    /// # Returns
    /// - `Ok(())` - Successfully acquired semaphore
    /// - `Err(())` - Interrupted by signal
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) -> Result<(), ()> {
    /// match sem.down_interruptible() {
    ///     Ok(()) => {
    ///         // Successfully acquired semaphore
    ///         // ... critical section ...
    ///         sem.up();
    ///     }
    ///     Err(()) => {
    ///         // Interrupted by signal
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn down_interruptible(&self) -> Result<(), ()> {
        // Atomic decrement by 1
        let old = self.count.fetch_sub(1, Ordering::Acquire);

        if old > 0 {
            // Successfully acquired semaphore
            return Ok(());
        }

        // Semaphore not available, need to wait
        // TODO: Implement signal interruption check
        // Current simplified implementation: call down()
        self.down();
        Ok(())
    }

    /// Try P operation (non-blocking)
    ///
    /// Also called try_down or down_trylock operation
    ///
    /// # Behavior
    /// - Decrement semaphore value by 1
    /// - If value >= 0, return Ok(())
    /// - If value < 0, immediately return Err(()), does not block
    ///
    /// # Returns
    /// - `Ok(())` - Successfully acquired semaphore
    /// - `Err(())` - Semaphore not available
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) -> Result<(), ()> {
    /// match sem.down_trylock() {
    ///     Ok(()) => {
    ///         // Successfully acquired semaphore
    ///         // ... critical section ...
    ///         sem.up();
    ///     }
    ///     Err(()) => {
    ///         // Semaphore not available
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn down_trylock(&self) -> Result<(), ()> {
        // Atomic decrement by 1
        let old = self.count.fetch_sub(1, Ordering::Acquire);

        if old > 0 {
            // Successfully acquired semaphore
            Ok(())
        } else {
            // Semaphore not available, restore value
            self.count.fetch_add(1, Ordering::Release);
            Err(())
        }
    }

    /// V operation (release semaphore)
    ///
    /// Also called up operation or signal operation
    ///
    /// # Behavior
    /// - Increment semaphore value by 1
    /// - If processes are waiting, wake one process
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Semaphore;
    /// # fn test(sem: &Semaphore) {
    /// sem.down();
    /// // ... critical section ...
    /// sem.up();  // Release semaphore, wake waiting process
    /// # }
    /// ```
    pub fn up(&self) {
        // Atomic increment by 1
        let old = self.count.fetch_add(1, Ordering::Release);

        if old < 0 {
            // Previously had processes waiting, wake one
            // Use exclusive mode, only wake one process
            self.wait.wake_up_one();
        }
    }

    /// Get current semaphore value
    ///
    /// # Returns
    /// Current semaphore value
    ///
    /// # Note
    /// This value is for reference only, actual value may change immediately after call
    pub fn count(&self) -> i32 {
        self.count.load(Ordering::Acquire)
    }
}

/// Mutex Semaphore (Mutex)
///
/// Binary semaphore, initial value is 1, used for mutual exclusion
///
/// # Example
/// ```no_run
/// # use kernel::sync::Mutex;
/// # fn test(mutex: &Mutex) {
/// mutex.lock();
/// // ... critical section ...
/// mutex.unlock();
/// # }
/// ```
#[repr(C)]
pub struct Mutex {
    /// Internal semaphore
    sem: Semaphore,
}

impl Mutex {
    /// Create a new mutex
    ///
    /// # Example
    /// ```
    /// let mutex = Mutex::new();
    /// ```
    pub const fn new() -> Self {
        Self {
            sem: Semaphore::new(1),
        }
    }

    /// Acquire lock
    ///
    /// If lock is already held, block and wait
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Mutex;
    /// # fn test(mutex: &Mutex) {
    /// mutex.lock();
    /// // ... critical section ...
    /// mutex.unlock();
    /// # }
    /// ```
    pub fn lock(&self) {
        self.sem.down();
    }

    /// Try to acquire lock (non-blocking)
    ///
    /// # Returns
    /// - `Ok(())` - Successfully acquired lock
    /// - `Err(())` - Lock is already held
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Mutex;
    /// # fn test(mutex: &Mutex) -> Result<(), ()> {
    /// match mutex.try_lock() {
    ///     Ok(()) => {
    ///         // Successfully acquired lock
    ///         // ... critical section ...
    ///         mutex.unlock();
    ///     }
    ///     Err(()) => {
    ///         // Lock is already held
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_lock(&self) -> Result<(), ()> {
        self.sem.down_trylock()
    }

    /// Release lock
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Mutex;
    /// # fn test(mutex: &Mutex) {
    /// mutex.lock();
    /// // ... critical section ...
    /// mutex.unlock();
    /// # }
    /// ```
    pub fn unlock(&self) {
        self.sem.up();
    }
}

/// Mutex Guard (RAII)
///
/// Automatically manages lock lifetime
///
/// # Example
/// ```no_run
/// # use kernel::sync::Mutex;
/// # fn test(mutex: &Mutex) {
/// {
///     let _guard = mutex.guard();
///     // ... critical section ...
/// } // Automatically release lock
/// # }
/// ```
pub struct MutexGuard<'a> {
    mutex: &'a Mutex,
}

impl<'a> MutexGuard<'a> {
    /// Create lock guard
    ///
    /// # Arguments
    /// * `mutex` - Associated mutex
    pub fn new(mutex: &'a Mutex) -> Self {
        mutex.lock();
        Self { mutex }
    }
}

impl<'a> Drop for MutexGuard<'a> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

impl Mutex {
    /// Get lock guard (RAII)
    ///
    /// Automatically manages lock lifetime, releases lock when guard goes out of scope
    ///
    /// # Returns
    /// MutexGuard guard object
    ///
    /// # Example
    /// ```no_run
    /// # use kernel::sync::Mutex;
    /// # fn test(mutex: &Mutex) {
    /// {
    ///     let _guard = mutex.guard();
    /// // ... critical section ...
    /// } // Automatically release lock
    /// # }
    /// ```
    pub fn guard(&self) -> MutexGuard<'_> {
        MutexGuard::new(self)
    }
}
