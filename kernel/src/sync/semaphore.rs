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

/// R77 (semaphore CAS discipline): try to take one unit from `count` —
/// succeeds only by atomically swapping a POSITIVE value to value-1. A
/// failed attempt leaves the count untouched (no reservation, no restore),
/// so the count is always exactly "units available", never a mix of
/// availability and outstanding waiter reservations.
fn cas_take(count: &AtomicI32) -> bool {
    let mut c = count.load(Ordering::Acquire);
    while c > 0 {
        match count.compare_exchange_weak(
            c,
            c - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(v) => c = v,
        }
    }
    false
}

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
        // R77 (GNOME kill-storm wedge/corruption family — rewrite of the
        // reservation protocol): the old down() reserved with an
        // unconditional fetch_sub and, after schedule() returned, TRUSTED
        // that the wake was an up() handoff. But signal_wake_up
        // force-wakes UNINTERRUPTIBLE sleepers (kill -9 storms wake
        // D-state waiters — the gsd teardown churn), so a signal-woken
        // waiter returned WITHOUT owning the semaphore while its -1
        // reservation stayed queued forever: two tasks walked into one
        // critical section (the copy_user wild-pointer panics at
        // guest+360s/+960s) and the poisoned count made every later up()
        // pair against a ghost. The wake source cannot be distinguished
        // from inside down(), so stop reserving: take a slot only via
        // CAS on a POSITIVE count (no failed-attempt side effects), and
        // after ANY wake simply retry. A spurious or signal wake now
        // costs one extra prepare/sleep round instead of the mutex
        // invariant. The R7-B6b lost-wake window stays closed by the
        // post-registration recheck; R8-6's exclusive/FIFO registration
        // discipline is unchanged.
        let current = match crate::sched::current() {
            Some(task) => task,
            None => {
                // No current task (early boot / IRQ context): fall back to
                // the unconditional decrement — callers in this context
                // must guarantee the count is available.
                self.count.fetch_sub(1, Ordering::Acquire);
                return;
            }
        };

        loop {
            // Fast path: take a slot if one is visible.
            if cas_take(&self.count) {
                return;
            }

            // Slow path: register EXCLUSIVE (tail insert, FIFO) + mark
            // UNINTERRUPTIBLE atomically under the waitqueue lock.
            self.wait.prepare_to_wait(current, true, false);

            // Recheck after registering (R7-B6b): an up() between the
            // failed CAS and the registration must find us queued.
            if cas_take(&self.count) {
                self.wait.finish_wait(current);
                // finish_wait restored RUNNING; a concurrent up() may ALSO
                // have seen us queued+sleeping and enqueued us (NEW-C2
                // family) — undo that (per-class on_rq guards make it a
                // no-op otherwise).
                crate::sched::dequeue_task(&*current);
                return;
            }

            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            self.wait.finish_wait(current);
            // R77: loop and retry — do NOT trust the wake source.
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
        // R77: same CAS rewrite as down() — a returned-from-schedule wake
        // is not proof of an up() handoff (spurious wakes, signal wakes),
        // so retry the acquire instead of returning Ok(()) unowned. The
        // signal path keeps its exact semantics: pending signal after a
        // wake (or before sleeping) returns Err with NO count side
        // effects (there is no reservation to roll back anymore).
        let current = match crate::sched::current() {
            Some(task) => task,
            None => {
                // No current task (early boot / IRQ context): cannot block.
                // Do not touch the count — report failure to the caller.
                return Err(());
            }
        };

        loop {
            // Fast path: take a slot if one is visible.
            if cas_take(&self.count) {
                return Ok(());
            }

            // Register + mark INTERRUPTIBLE (atomically under the waitqueue
            // lock). EXCLUSIVE (tail insert, FIFO), matching down().
            self.wait.prepare_to_wait(current, true, true);

            // Recheck after registering (R32-F10 / R7-B6b lost-wake
            // discipline: any up() that pairs with this attempt must find
            // us queued).
            if cas_take(&self.count) {
                self.wait.finish_wait(current);
                crate::sched::dequeue_task(&*current);
                return Ok(());
            }

            // A signal that arrived before we sleep must interrupt
            // immediately (the caller maps Err to EINTR).
            if crate::signal::signal_pending() {
                self.wait.finish_wait(current);
                // R9-17: NEW-C2 discipline for the signal path too — undo a
                // concurrent wake enqueue before returning.
                crate::sched::dequeue_task(&*current);
                return Err(());
            }

            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            self.wait.finish_wait(current);

            // Interrupted by a signal rather than up(): report EINTR to
            // the caller. No count side effects to undo (R77: the CAS
            // discipline took nothing); if an up() raced the signal its +1
            // stays in the count for the next down — no token is lost.
            if crate::signal::signal_pending() {
                crate::sched::dequeue_task(&*current);
                return Err(());
            }
            // R77: otherwise loop and retry the acquire — the wake alone is
            // not proof an up() handed us the slot.
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
        // R77: pure CAS take — the old fetch_sub/fetch_add restore pair
        // briefly published a negative count that a concurrent up() could
        // misread as a waiter reservation.
        if cas_take(&self.count) {
            Ok(())
        } else {
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
        // R77: with the CAS discipline the count never goes negative on a
        // FAILED down() (no reservations), so "old < 0" can no longer
        // serve as the waiters-exist test. Publish the slot and always
        // attempt one exclusive wake; waking an empty (or all-woken)
        // queue is a cheap no-op scan under the queue lock. If a fast-path
        // down() steals the slot before the woken waiter runs, the waiter
        // simply re-sleeps — no token is lost.
        self.count.fetch_add(1, Ordering::Release);
        self.wait.wake_up_one();
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
