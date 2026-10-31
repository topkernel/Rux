//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Read-Write Spinlock
//!
//! Allows concurrent readers or a single exclusive writer.
//!
//! API:
//!   read()           — preempt disable + read lock
//!   write()          — preempt disable + write lock
//!   read_irqsave()   — save IRQ + preempt disable + read lock
//!   write_irqsave()  — save IRQ + preempt disable + write lock
//!   read_bh()        — disable BH + read lock
//!   write_bh()       — disable BH + write lock

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
            if s & WRITER_BIT != 0 {
                core::hint::spin_loop();
                continue;
            }
            // Guard against reader count overflow into writer bit.
            if (s & READER_MASK) >= READER_MASK {
                core::hint::spin_loop();
                continue;
            }
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
        if (s & READER_MASK) >= READER_MASK {
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
            if s != 0 {
                core::hint::spin_loop();
                continue;
            }
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

    /// A writer holds (or is acquiring) — readers may still enter.
    /// Used by the preemptible read-wait path (preempt-hunt): a reader
    /// only ever blocks on the writer bit, so waiting for full
    /// `is_locked() == false` would stall behind unrelated readers.
    #[inline]
    pub fn has_writer(&self) -> bool {
        self.state.load(Ordering::Acquire) & WRITER_BIT != 0
    }
}

// ==================== Generic RwSpinlock<T> ====================

pub struct RwSpinlock<T: ?Sized> {
    raw: RawRwSpinlock,
    data: UnsafeCell<T>,
}

// SAFETY: RwSpinlock<T> mediates all access to the inner data: shared (&T) via
// read-locked guards, exclusive (&mut T) via write-locked guards.  Send allows
// moving between threads; Sync allows shared references across threads because
// the rwlock serialises access.
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

    /// Preempt disable + read lock.
    /// Guard drop: read unlock + preempt enable.
    /// track_caller: leakorigin's outermost-acquire shadow records the
    /// LOCK CALLER's site (preempt-hunt), not this helper's line.
    ///
    /// preempt-hunt (bounded preemptible wait — the twin of
    /// Spinlock::lock's): a reader contending a writer that runs a
    /// multi-second section used to spin preempt-disabled for the whole
    /// hold (the pc=1 incumbent family; the tick-heal then stripped a
    /// LIVE spin booking and the guard's Drop underflowed). At the
    /// outermost depth in task context, wait preemptibly and yield to
    /// the writer after a spin budget; nested/IRQ callers keep the
    /// legacy preempt-off acquire.
    #[inline]
    #[track_caller]
    pub fn read(&self) -> RwSpinlockReadGuard<'_, T> {
        let pc0 = crate::interrupt::preempt::preempt_count();
        let yieldable = (pc0 & crate::interrupt::preempt::PREEMPT_MASK) == 0
            && (pc0
                & (crate::interrupt::preempt::HARDIRQ_MASK
                    | crate::interrupt::preempt::SOFTIRQ_MASK
                    | crate::interrupt::preempt::NMI_MASK))
                == 0
            && crate::sched::current().is_some();
        preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_read() {
            return RwSpinlockReadGuard { lock: self };
        }
        if !yieldable {
            self.raw.read();
            return RwSpinlockReadGuard { lock: self };
        }
        preempt_enable();
        const SPIN_BUDGET: u32 = 20_000;
        loop {
            // Only a WRITER blocks a reader — do not wait for unrelated
            // readers to drain (a reader-heavy lock would otherwise
            // livelock the yield loop).
            let mut spins: u32 = 0;
            while self.raw.has_writer() && spins < SPIN_BUDGET {
                spins = spins.wrapping_add(1);
                core::hint::spin_loop();
            }
            preempt_disable();
            if self.raw.try_read() {
                return RwSpinlockReadGuard { lock: self };
            }
            preempt_enable();
            // Writer still holds (or a racing reader/writer slipped in):
            // hand the CPU over — we hold nothing, so switching is safe.
            crate::sched::schedule();
        }
    }

    /// Preempt disable + write lock.
    /// Guard drop: write unlock + preempt enable.
    /// Same preemptible-wait discipline as `read` (see there).
    #[inline]
    #[track_caller]
    pub fn write(&self) -> RwSpinlockWriteGuard<'_, T> {
        let pc0 = crate::interrupt::preempt::preempt_count();
        let yieldable = (pc0 & crate::interrupt::preempt::PREEMPT_MASK) == 0
            && (pc0
                & (crate::interrupt::preempt::HARDIRQ_MASK
                    | crate::interrupt::preempt::SOFTIRQ_MASK
                    | crate::interrupt::preempt::NMI_MASK))
                == 0
            && crate::sched::current().is_some();
        preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_write() {
            return RwSpinlockWriteGuard { lock: self };
        }
        if !yieldable {
            self.raw.write();
            return RwSpinlockWriteGuard { lock: self };
        }
        preempt_enable();
        const SPIN_BUDGET: u32 = 20_000;
        loop {
            let mut spins: u32 = 0;
            while self.raw.is_locked() && spins < SPIN_BUDGET {
                spins = spins.wrapping_add(1);
                core::hint::spin_loop();
            }
            preempt_disable();
            if self.raw.try_write() {
                return RwSpinlockWriteGuard { lock: self };
            }
            preempt_enable();
            crate::sched::schedule();
        }
    }

    /// Save IRQ + preempt disable + read lock.
    /// Guard drop: read unlock + preempt enable + restore IRQ.
    #[inline]
    #[track_caller]
    pub fn read_irqsave(&self) -> RwSpinlockIrqReadGuard<'_, T> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
        self.raw.read();
        RwSpinlockIrqReadGuard { lock: self, flags }
    }

    /// Save IRQ + preempt disable + write lock.
    /// Guard drop: write unlock + preempt enable + restore IRQ.
    #[inline]
    #[track_caller]
    pub fn write_irqsave(&self) -> RwSpinlockIrqWriteGuard<'_, T> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
        self.raw.write();
        RwSpinlockIrqWriteGuard { lock: self, flags }
    }

    /// Disable BH + read lock.
    /// bh_disable() increments preempt_count by SOFTIRQ_OFFSET.
    /// Guard drop: read unlock + bh_enable.
    #[inline]
    pub fn read_bh(&self) -> RwSpinlockBhReadGuard<'_, T> {
        bh_disable();
        self.raw.read();
        RwSpinlockBhReadGuard { lock: self }
    }

    /// Disable BH + write lock.
    /// bh_disable() increments preempt_count by SOFTIRQ_OFFSET.
    /// Guard drop: write unlock + bh_enable.
    #[inline]
    pub fn write_bh(&self) -> RwSpinlockBhWriteGuard<'_, T> {
        bh_disable();
        self.raw.write();
        RwSpinlockBhWriteGuard { lock: self }
    }

    #[inline]
    #[track_caller]
    pub fn try_read(&self) -> Option<RwSpinlockReadGuard<'_, T>> {
        preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_read() {
            Some(RwSpinlockReadGuard { lock: self })
        } else {
            preempt_enable();
            None
        }
    }

    #[inline]
    #[track_caller]
    pub fn try_write(&self) -> Option<RwSpinlockWriteGuard<'_, T>> {
        preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_write() {
            Some(RwSpinlockWriteGuard { lock: self })
        } else {
            preempt_enable();
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    /// Consume the lock and return the inner data.
    ///
    /// # Safety
    /// Caller must ensure no other thread holds a reference to the lock or data.
    #[inline]
    pub unsafe fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

// ==================== Read Guard (plain — preempt) ====================

pub struct RwSpinlockReadGuard<'a, T: ?Sized> {
    lock: &'a RwSpinlock<T>,
}

// SAFETY: ReadGuard exists only while a read lock is held, preventing any
// writer from obtaining exclusive access to the data.
unsafe impl<T: ?Sized + Sync> Send for RwSpinlockReadGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Read lock is held — no &mut access can occur concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.read_unlock();
        preempt_enable();
    }
}

// ==================== Write Guard (plain — preempt) ====================

pub struct RwSpinlockWriteGuard<'a, T: ?Sized> {
    lock: &'a RwSpinlock<T>,
}

// SAFETY: WriteGuard exists only while the write lock is held exclusively,
// so no other thread can access the data.
unsafe impl<T: ?Sized + Send> Send for RwSpinlockWriteGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Write lock is held exclusively — no concurrent access.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwSpinlockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Write lock is held exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.write_unlock();
        preempt_enable();
    }
}

// ==================== IRQ Read Guard ====================

pub struct RwSpinlockIrqReadGuard<'a, T: ?Sized> {
    lock: &'a RwSpinlock<T>,
    flags: bool,
}

// SAFETY: IrqReadGuard exists only while a read lock + IRQ disable is held,
// preventing any writer from accessing the data.
unsafe impl<T: ?Sized + Sync> Send for RwSpinlockIrqReadGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockIrqReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Read lock is held — no &mut access can occur concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockIrqReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.read_unlock();
        preempt_enable();
        irq_restore(self.flags);
    }
}

// ==================== IRQ Write Guard ====================

pub struct RwSpinlockIrqWriteGuard<'a, T: ?Sized> {
    lock: &'a RwSpinlock<T>,
    flags: bool,
}

// SAFETY: IrqWriteGuard exists only while the write lock + IRQ disable is
// held exclusively, so no other thread can access the data.
unsafe impl<T: ?Sized + Send> Send for RwSpinlockIrqWriteGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockIrqWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Write lock is held exclusively — no concurrent access.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwSpinlockIrqWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Write lock is held exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockIrqWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.write_unlock();
        preempt_enable();
        irq_restore(self.flags);
    }
}

// ==================== BH Read Guard ====================

pub struct RwSpinlockBhReadGuard<'a, T: ?Sized> {
    lock: &'a RwSpinlock<T>,
}

// SAFETY: BhReadGuard exists only while a read lock + BH disable is held,
// preventing any writer from accessing the data.
unsafe impl<T: ?Sized + Sync> Send for RwSpinlockBhReadGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockBhReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Read lock is held — no &mut access can occur concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockBhReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.read_unlock();
        bh_enable();
    }
}

// ==================== BH Write Guard ====================

pub struct RwSpinlockBhWriteGuard<'a, T: ?Sized> {
    lock: &'a RwSpinlock<T>,
}

// SAFETY: BhWriteGuard exists only while the write lock + BH disable is
// held exclusively, so no other thread can access the data.
unsafe impl<T: ?Sized + Send> Send for RwSpinlockBhWriteGuard<'_, T> {}

impl<T: ?Sized> Deref for RwSpinlockBhWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Write lock is held exclusively — no concurrent access.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwSpinlockBhWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Write lock is held exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwSpinlockBhWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.write_unlock();
        bh_enable();
    }
}

// ==================== Inline helpers ====================

#[inline]
#[track_caller]
fn preempt_disable() {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        core::panic::Location::caller(),
    );
}

/// As `preempt_disable`, booking the caller-of-the-lock's location (the
/// #[track_caller] lock helpers forward their own caller down so the
/// leakorigin shadow names the real acquire site; preempt-hunt).
#[inline]
fn preempt_disable_at(loc: &'static core::panic::Location<'static>) {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        loc,
    );
}

#[inline]
fn preempt_enable() {
    crate::interrupt::preempt::preempt_count_sub(
        crate::interrupt::preempt::PREEMPT_OFFSET,
    );
}

#[inline]
#[track_caller]
fn irq_save() -> bool { // track_caller: syscost REST site attribution
    crate::arch::cpu::save_and_disable_irq()
}

#[inline]
#[track_caller]
fn irq_restore(flags: bool) {
    crate::arch::cpu::restore_irq(flags);
}

#[inline]
fn bh_disable() {
    crate::interrupt::preempt::preempt_count_add(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
    );
}

#[inline]
fn bh_enable() {
    crate::interrupt::preempt::preempt_count_sub(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
    );
}
