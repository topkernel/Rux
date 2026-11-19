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
/// Writer-pending bit: set by the FIRST contended writer while readers
/// still hold the lock (the reference qrwlock's queued-writer discipline
/// — see `write()` below). New readers block on it, so a continuous
/// cross-CPU stream of short readers cannot starve a waiting writer out
/// of the lock (the post-realign hazard: with the yielding slowpath
/// deleted, a preempt-disabled writer could only spin while readers on
/// other CPUs kept re-acquiring forever).
const WRITER_PENDING_BIT: u32 = 1u32 << 30;
const READER_MASK: u32 = !(WRITER_BIT | WRITER_PENDING_BIT);

/// Raw read-write spinlock.
///
/// Bit layout of the `state` word:
/// - `[31]`    — writer held flag
/// - `[30]`    — writer pending flag (one waiting writer owns it)
/// - `[29:0]`  — active reader count
///
/// Writer fairness (reference qrwlock discipline, sync-realign followup):
/// a contended writer first registers WRITER_PENDING_BIT — readers still
/// holding the lock drain normally, but NEW readers block, so the writer
/// is handed the lock after at most one reader-generation instead of
/// competing forever against a cross-CPU reader stream.
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
            // A HELD writer excludes; a PENDING writer also blocks new
            // readers (writer fairness — see the struct doc).
            if s & (WRITER_BIT | WRITER_PENDING_BIT) != 0 {
                core::hint::spin_loop();
                continue;
            }
            // Guard against reader count overflow into the flag bits.
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
        if s & (WRITER_BIT | WRITER_PENDING_BIT) != 0 {
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

    /// Acquire an exclusive (writer) lock (two-phase, writer-fair).
    ///
    /// Phase 1 — register PENDING: exactly ONE waiting writer owns the
    /// pending bit at a time (a second writer sees it set and waits for
    /// it to clear, which its owner's phase-2 CAS guarantees). Setting
    /// PENDING is allowed while readers still hold the lock; it blocks
    /// only NEW readers, so the current reader generation drains.
    ///
    /// Phase 2 — wait for the reader count to reach zero, then flip
    /// PENDING → WRITER. Once we own PENDING, no new reader can enter
    /// (they block on it) and no other writer can take WRITER (they
    /// cannot reach phase 2 without owning PENDING), so the flip's CAS
    /// races nothing; the loop keeps its retry shape for the same
    /// defensive reason RawSpinlock::lock does.
    #[inline]
    pub fn write(&self) {
        // Phase 1: become the one pending writer.
        loop {
            let s = self.state.load(Ordering::Acquire);
            if s & WRITER_BIT != 0 {
                core::hint::spin_loop();
                continue;
            }
            if s & WRITER_PENDING_BIT != 0 {
                // Another writer already queued — wait for its turn to
                // complete (it WILL take the lock: readers drain and the
                // flip races nothing).
                core::hint::spin_loop();
                continue;
            }
            if self
                .state
                .compare_exchange_weak(
                    s,
                    s | WRITER_PENDING_BIT,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
            core::hint::spin_loop();
        }
        // Phase 2: drain wait + flip.
        loop {
            let s = self.state.load(Ordering::Acquire);
            if s & READER_MASK == 0 {
                if self
                    .state
                    .compare_exchange_weak(
                        s,
                        (s | WRITER_BIT) & !WRITER_PENDING_BIT,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return;
                }
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

    /// A writer holds the lock or has registered pending (readers may
    /// not enter either way). Diagnostic/probe helper (the deleted
    /// preemptible read-wait path was its only internal consumer).
    #[inline]
    pub fn has_writer(&self) -> bool {
        self.state.load(Ordering::Acquire) & (WRITER_BIT | WRITER_PENDING_BIT) != 0
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
    /// Reference discipline (sync-realign, 2026-11): a spin-family lock
    /// NEVER sleeps — no task-state change and no schedule() anywhere
    /// in the acquire; preempt stays disabled for the whole spin. The
    /// former "preemptible read-wait" that yielded via a bare
    /// schedule() here (preempt-hunt) was the same divergence deleted
    /// from Spinlock::lock, and worse: it could park a caller caught in
    /// a wait path's prepared window (the lost-park family). Locks whose
    /// holders block in I/O must be SleepMutex (sync/mutex.rs), not
    /// rwlocks.
    #[inline]
    #[track_caller]
    pub fn read(&self) -> RwSpinlockReadGuard<'_, T> {
        preempt_disable_at(core::panic::Location::caller());
        self.raw.read();
        RwSpinlockReadGuard { lock: self }
    }

    /// Preempt disable + write lock.
    /// Guard drop: write unlock + preempt enable.
    /// Same pure-spinning discipline as `read` (see there).
    #[inline]
    #[track_caller]
    pub fn write(&self) -> RwSpinlockWriteGuard<'_, T> {
        preempt_disable_at(core::panic::Location::caller());
        self.raw.write();
        RwSpinlockWriteGuard { lock: self }
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
