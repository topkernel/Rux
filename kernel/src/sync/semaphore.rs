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
        // Attempt fast-path acquisition first.
        let old = self.count.fetch_sub(1, Ordering::Acquire);
        if old > 0 {
            return;
        }

        // Slow path: semaphore not available, need to wait.
        // Use prepare_to_wait to atomically add to wait queue AND set state,
        // preventing the lost-wakeup race (see wait_event_interruptible! macro).
        loop {
            let current = match crate::sched::current() {
                Some(task) => task,
                None => {
                    // Cannot get current task — undo the decrement and return.
                    self.count.fetch_add(1, Ordering::Release);
                    return;
                }
            };

            self.wait.prepare_to_wait(current, false, false);

            // Re-check after prepare_to_wait (state is now UNINTERRUPTIBLE).
            // If count went positive, our initial decrement is balanced — return.
            if self.count.load(Ordering::Acquire) > 0 {
                self.wait.finish_wait(current);
                return;
            }

            // Yield CPU — task removed from runqueue by __schedule()
            crate::arch::riscv64::cpu::restore_irq(true);
            crate::sched::schedule();

            // Woken up — finish_wait restores RUNNING and removes from queue.
            self.wait.finish_wait(current);

            // Our initial fetch_sub(1) already reserved a slot.  The up() that
            // woke us incremented count by one, so the count is now correct.
            // No need to re-acquire — just return.
            return;
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
        // Fast path.
        let old = self.count.fetch_sub(1, Ordering::Acquire);
        if old > 0 {
            return Ok(());
        }

        // Slow path with signal checking.
        loop {
            let current = match crate::sched::current() {
                Some(task) => task,
                None => {
                    self.count.fetch_add(1, Ordering::Release);
                    return Err(());
                }
            };

            self.wait.prepare_to_wait(current, false, true);

            if self.count.load(Ordering::Acquire) > 0 {
                self.wait.finish_wait(current);
                return Ok(());
            }

            if crate::signal::signal_pending() {
                self.wait.finish_wait(current);
                // Undo our initial fetch_sub.
                self.count.fetch_add(1, Ordering::Release);
                return Err(());
            }

            crate::arch::riscv64::cpu::restore_irq(true);
            crate::sched::schedule();

            self.wait.finish_wait(current);

            // Check if woken by signal rather than up().
            if crate::signal::signal_pending() {
                // Interrupted — undo our initial fetch_sub.
                self.count.fetch_add(1, Ordering::Release);
                return Err(());
            }

            // Woken by up(): our initial fetch_sub already reserved a slot.
            // The up() that woke us incremented count, so it is correct.
            return Ok(());
        }
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
        // Linux __up() semantics: wake a waiter if one is waiting,
        // otherwise increment count.  This avoids the double-count bug
        // where up() both increments count AND wakes a waiter (the waiter
        // then also sees count > 0 and doesn't re-decrement, leaking count).
        //
        // We increment count first to signal that a slot is available,
        // then wake one exclusive waiter.  The woken waiter's down() path
        // will consume this count via its CAS loop.
        let old = self.count.fetch_add(1, Ordering::Release);

        if old < 0 {
            // Waiters exist — wake one exclusive waiter.
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
