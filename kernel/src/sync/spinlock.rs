//! Spinlock with IRQ-safe variants.
//!
//! Ticket lock with mandatory interrupt gating in lock() to prevent
//! deadlocks when an IRQ handler attempts to acquire a spinlock already
//! held on the same hart. Unlike TAS (compare_exchange), ticket lock's
//! fetch_add unconditionally modifies the the atomic state, If an IRQ
//! preempts a spinning hart and the IRQ handler fetch_adds the same lock,
//! neither the the hart nor the IRQ handler can ever proceed. Closing
//! interrupts before acquiring the lock prevents this.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};

// ==================== RawSpinlock (TAS) ====================

pub struct RawSpinlock {
    locked: AtomicU32,
}

impl RawSpinlock {
    #[inline]
    pub const fn new() -> Self {
        Self { locked: AtomicU32::new(0) }
    }

    #[inline]
    pub fn lock(&self) {
        while self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_err() {
            core::hint::spin_loop();
        }
    }

    #[inline]
    pub fn try_lock(&self) -> bool {
        self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok()
    }

    #[inline]
    pub fn unlock(&self) {
        self.locked.store(0, Ordering::Release);
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Acquire) != 0
    }

    #[inline]
    pub unsafe fn reset(&mut self) {
        self.locked.store(0, Ordering::Release);
    }
}

// ==================== Spinlock<T> ====================

pub struct Spinlock<T: ?Sized> {
    raw: RawSpinlock,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for Spinlock<T> {}
unsafe impl<T: ?Sized + Send> Sync for Spinlock<T> {}

impl<T> Spinlock<T> {
    #[inline]
    pub const fn new(data: T) -> Self {
        Self {
            raw: RawSpinlock::new(),
            data: UnsafeCell::new(data),
        }
    }

    /// Lock with interrupt gating (ticket lock requires this to prevent
    /// deadlocks on the same hart when an IRQ handler re-enters the same lock).
    #[inline]
    pub fn lock(&self) -> SpinlockGuard<'_, T> {
        let flags = irq_save();
        self.raw.lock();
        SpinlockGuard { lock: self, flags }
    }

    /// Lock + disable interrupts (same as lock() for ticket lock,
    /// kept for API compatibility).
    #[inline]
    pub fn lock_irq(&self) -> SpinlockGuard<'_, T> {
        let flags = irq_save();
        self.raw.lock();
        SpinlockGuard { lock: self, flags }
    }

    /// Lock + save and disable interrupts (explicit irqsave variant).
    #[inline]
    pub fn lock_irqsave(&self) -> SpinlockGuard<'_, T> {
        let flags = irq_save();
        self.raw.lock();
        SpinlockGuard { lock: self, flags }
    }

    /// Lock + disable bottom half (softirq).
    #[inline]
    pub fn lock_bh(&self) -> SpinlockGuard<'_, T> {
        bh_disable();
        let flags = irq_save();
        self.raw.lock();
        SpinlockGuard { lock: self, flags }
    }

    #[inline]
    pub fn try_lock(&self) -> Option<SpinlockGuard<'_, T>> {
        let flags = irq_save();
        if self.raw.try_lock() {
            Some(SpinlockGuard { lock: self, flags })
        } else {
            irq_restore(flags);
            None
        }
    }

    #[inline]
    pub fn try_lock_irqsave(&self) -> Option<SpinlockGuard<'_, T>> {
        let flags = irq_save();
        if self.raw.try_lock() {
            Some(SpinlockGuard { lock: self, flags })
        } else {
            irq_restore(flags);
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    #[inline]
    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        &mut *self.data.get()
    }

    #[inline]
    pub unsafe fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

// ==================== SpinlockGuard ====================

pub struct SpinlockGuard<'a, T: ?Sized> {
    lock: &'a Spinlock<T>,
    /// Saved interrupt state to restore on drop.
    flags: bool,
}

unsafe impl<T: ?Sized + Send> Send for SpinlockGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinlockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinlockGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.unlock();
        irq_restore(self.flags);
    }
}

// ==================== Helper functions ====================

#[inline]
fn preempt_disable() {
    crate::interrupt::preempt::preempt_count_add(
        crate::interrupt::preempt::PREEMPT_OFFSET,
    );
}

#[inline]
fn preempt_enable() {
    crate::interrupt::preempt::preempt_count_sub(
        crate::interrupt::preempt::PREEMPT_OFFSET,
    );
}

#[inline]
fn irq_save() -> bool {
    crate::arch::riscv64::cpu::save_and_disable_irq()
}

#[inline]
fn irq_restore(flags: bool) {
    crate::arch::riscv64::cpu::restore_irq(flags);
}

#[inline]
fn bh_disable() {
    crate::interrupt::preempt::preempt_count_add(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
    );
}

// ==================== Free functions (C-style API) ====================

#[inline(always)]
pub fn raw_spin_lock(l: &RawSpinlock) {
    l.lock();
}
#[inline(always)]
pub fn raw_spin_unlock(l: &RawSpinlock) {
    l.unlock();
}
#[inline(always)]
pub fn raw_spin_is_locked(l: &RawSpinlock) -> bool {
    l.is_locked()
}
#[inline(always)]
pub fn raw_spin_trylock(l: &mut RawSpinlock) -> bool {
    l.try_lock()
}
