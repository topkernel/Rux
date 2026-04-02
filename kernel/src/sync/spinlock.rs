//! Simple TAS spinlock matching spin::Mutex behavior.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};

pub struct RawSpinlock { locked: AtomicU32 }

impl RawSpinlock {
    #[inline] pub const fn new() -> Self { Self { locked: AtomicU32::new(0) } }
    #[inline] pub fn lock(&self) {
        while self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_err() {
            core::hint::spin_loop();
        }
    }
    #[inline] pub fn try_lock(&self) -> bool {
        self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok()
    }
    #[inline] pub fn unlock(&self) { self.locked.store(0, Ordering::Release); }
    #[inline] pub fn is_locked(&self) -> bool { self.locked.load(Ordering::Acquire) != 0 }
    #[inline] pub unsafe fn reset(&mut self) { self.locked.store(0, Ordering::Release); }
}

pub struct Spinlock<T: ?Sized> { raw: RawSpinlock, data: UnsafeCell<T> }
unsafe impl<T: ?Sized + Send> Send for Spinlock<T> {}
unsafe impl<T: ?Sized + Send> Sync for Spinlock<T> {}

impl<T> Spinlock<T> {
    #[inline] pub const fn new(data: T) -> Self { Self { raw: RawSpinlock::new(), data: UnsafeCell::new(data) } }
    #[inline] pub fn lock(&self) -> SpinlockGuard<'_, T> { self.raw.lock(); SpinlockGuard { lock: self } }
    #[inline] pub fn try_lock(&self) -> Option<SpinlockGuard<'_, T>> {
        if self.raw.try_lock() { Some(SpinlockGuard { lock: self }) } else { None }
    }
    #[inline] pub fn is_locked(&self) -> bool { self.raw.is_locked() }
    #[inline] pub unsafe fn get_mut_unchecked(&self) -> &mut T { &mut *self.data.get() }
    #[inline] pub unsafe fn into_inner(self) -> T { self.data.into_inner() }
}

pub struct SpinlockGuard<'a, T: ?Sized> { lock: &'a Spinlock<T> }
unsafe impl<T: ?Sized + Send> Send for SpinlockGuard<'_, T> {}
impl<T: ?Sized> Deref for SpinlockGuard<'_, T> {
    type Target = T;
    #[inline] fn deref(&self) -> &Self::Target { unsafe { &*self.lock.data.get() } }
}
impl<T: ?Sized> DerefMut for SpinlockGuard<'_, T> {
    #[inline] fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.lock.data.get() } }
}
impl<T: ?Sized> Drop for SpinlockGuard<'_, T> {
    #[inline] fn drop(&mut self) { self.lock.raw.unlock(); }
}

#[inline(always)] pub fn raw_spin_lock(l: &RawSpinlock) { l.lock(); }
#[inline(always)] pub fn raw_spin_unlock(l: &RawSpinlock) { l.unlock(); }
#[inline(always)] pub fn raw_spin_is_locked(l: &RawSpinlock) -> bool { l.is_locked() }
#[inline(always)] pub fn raw_spin_trylock(l: &mut RawSpinlock) -> bool { l.try_lock() }
