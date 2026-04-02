//! Read-Write Spinlock
//!
//! Allows concurrent readers or a single exclusive writer.

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};

const WRITER_BIT: u32 = 1u32 << 31;
const READER_MASK: u32 = !WRITER_BIT;

/// Raw read-write spinlock.
///
/// Bit layout of the `state` word:
/// - `[31]`    — writer held flag
/// - `[30:0]`  — active reader count
pub struct RawRwSpinlock {
    state: AtomicU32,
}

impl RawRwSpinlock {
    #[inline]
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
        }
    }

    /// Acquire a shared (reader) lock.
    #[inline]
    pub fn read(&self) {
        loop {
            let s = self.state.load(Ordering::Acquire);
            // Writer held — spin
            if s & WRITER_BIT != 0 {
                core::hint::spin_loop();
                continue;
            }
            // Attempt to increment reader count
            if self
                .state
                .compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Try to acquire a shared (reader) lock.
    #[inline]
    pub fn try_read(&self) -> bool {
        let s = self.state.load(Ordering::Acquire);
        if s & WRITER_BIT != 0 {
            return false;
        }
        self.state
            .compare_exchange(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Release a shared (reader) lock.
    #[inline]
    pub fn read_unlock(&self) {
        self.state.fetch_sub(1, Ordering::Release);
    }

    /// Acquire an exclusive (writer) lock.
    #[inline]
    pub fn write(&self) {
        loop {
            let s = self.state.load(Ordering::Acquire);
            // Readers or writer held — spin
            if s != 0 {
                core::hint::spin_loop();
                continue;
            }
            // Attempt to set writer bit
            if self
                .state
                .compare_exchange_weak(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Try to acquire an exclusive (writer) lock.
    #[inline]
    pub fn try_write(&self) -> bool {
        self.state
            .compare_exchange(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Release an exclusive (writer) lock.
    #[inline]
    pub fn write_unlock(&self) {
        self.state.fetch_and(!WRITER_BIT, Ordering::Release);
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.state.load(Ordering::Acquire) != 0
    }
}

// ==================== Generic RwSpinlock<T> ====================

pub struct RwSpinlock<T: ?Sized> {
    raw: RawRwSpinlock,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for RwSpinlock<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for RwSpinlock<T> {}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwSpinlock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RwSpinlock")
            .field("locked", &self.raw.is_locked())
            .finish_non_exhaustive()
    }
}

impl<T> RwSpinlock<T> {
    #[inline]
    pub const fn new(data: T) -> Self {
        Self {
            raw: RawRwSpinlock::new(),
            data: UnsafeCell::new(data),
        }
    }

    #[inline]
    pub fn read(&self) -> RwSpinlockReadGuard<'_, T> {
        self.raw.read();
        RwSpinlockReadGuard { lock: self }
    }

    #[inline]
    pub fn try_read(&self) -> Option<RwSpinlockReadGuard<'_, T>> {
        if self.raw.try_read() {
            Some(RwSpinlockReadGuard { lock: self })
        } else {
            None
        }
    }

    #[inline]
    pub fn write(&self) -> RwSpinlockWriteGuard<'_, T> {
        self.raw.write();
        RwSpinlockWriteGuard { lock: self }
    }

    #[inline]
    pub fn try_write(&self) -> Option<RwSpinlockWriteGuard<'_, T>> {
        if self.raw.try_write() {
            Some(RwSpinlockWriteGuard { lock: self })
        } else {
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    #[inline]
    pub unsafe fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

// ==================== Read Guard ====================

pub struct RwSpinlockReadGuard<'a, T: ?Sized> {
    lock: &'a RwSpinlock<T>,
}

unsafe impl<T: ?Sized + Sync> Send for RwSpinlockReadGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.read_unlock();
    }
}

// ==================== Write Guard ====================

pub struct RwSpinlockWriteGuard<'a, T: ?Sized> {
    lock: &'a RwSpinlock<T>,
}

unsafe impl<T: ?Sized + Send> Send for RwSpinlockWriteGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwSpinlockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.write_unlock();
    }
}
