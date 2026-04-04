//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Spinlock with preempt / IRQ / BH variants.
//!
//! API:
//!   lock()          — preempt disable + lock
//!   lock_irq()      — disable interrupts + preempt disable + lock
//!   lock_irqsave()  — save interrupt state + disable + preempt disable + lock
//!   lock_bh()       — disable bottom-half (softirq) + lock
//!
//! Backend: TAS (test-and-set) via compare_exchange.
//! Ticket lock causes interactive-input deadlock on QEMU
//! (likely QEMU's amoadd.w emulation bug), so TAS is used for now.

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

    /// Spinlock deadlock threshold (iterations before warning).
    /// On SMP with QEMU emulation, brief contention is normal — PLIC IRQ
    /// claim/release, GRQ lock, etc. can take 10-100ms of spin time.
    /// 100M iterations ≈ 100-500ms depending on CAS latency.
    const DEADLOCK_WARN_ITERS: u32 = 100_000_000;

    #[inline(never)]
    pub fn lock(&self) {
        // Capture caller's return address before spinning
        let caller_ra: usize;
        unsafe { core::arch::asm!("mv {}, ra", out(reg) caller_ra, options(nomem, nostack)); }
        let mut spins: u32 = 0;
        while self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_err() {
            spins = spins.wrapping_add(1);
            if spins == Self::DEADLOCK_WARN_ITERS {
                Self::deadlock_warn(self as *const Self, caller_ra);
                spins = 0; // continue spinning (might resolve)
            }
            core::hint::spin_loop();
        }
    }

    /// Print deadlock warning via SBI (works even with interrupts disabled).
    fn deadlock_warn(lock_addr: *const Self, caller_ra: usize) {
        // Use SBI putchar directly — printk might need locks we're spinning on
        let cpu = crate::arch::riscv64::smp::cpu_id();
        let msg = b"DEADLOCK: spinlock stuck cpu=";
        for &b in msg {
            unsafe { sbi_rt::legacy::console_putchar(b as usize); }
        }
        // Print CPU id as decimal digit
        if cpu < 10 {
            unsafe { sbi_rt::legacy::console_putchar(b'0' as usize + cpu); }
        }
        // Print lock address in hex
        let msg2 = b" lock=0x";
        for &b in msg2 {
            unsafe { sbi_rt::legacy::console_putchar(b as usize); }
        }
        let addr = lock_addr as usize;
        let mut shift = (core::mem::size_of::<usize>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (addr >> (shift as usize)) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            unsafe { sbi_rt::legacy::console_putchar(c as usize); }
        }

        // Print caller return address (ra) for debugging
        let msg3 = b" ra=0x";
        for &b in msg3 {
            unsafe { sbi_rt::legacy::console_putchar(b as usize); }
        }
        let mut shift = (core::mem::size_of::<usize>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (caller_ra >> shift) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            unsafe { sbi_rt::legacy::console_putchar(c as usize); }
        }

        unsafe { sbi_rt::legacy::console_putchar(b'\n' as usize); }
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
        Self { raw: RawSpinlock::new(), data: UnsafeCell::new(data) }
    }

    /// Preempt disable + lock.
    /// Guard drop: unlock + preempt enable.
    #[inline]
    pub fn lock(&self) -> SpinlockGuard<'_, T> {
        preempt_disable();
        self.raw.lock();
        SpinlockGuard { lock: self }
    }

    /// Disable interrupts + preempt disable + lock.
    /// Guard drop: unlock + preempt enable + restore interrupts.
    #[inline]
    pub fn lock_irq(&self) -> SpinlockIrqGuard<'_, T> {
        let flags = irq_save();
        preempt_disable();
        self.raw.lock();
        SpinlockIrqGuard { lock: self, flags }
    }

    /// Save interrupt state + disable + preempt disable + lock.
    /// Guard drop: unlock + preempt enable + restore interrupt state.
    #[inline]
    pub fn lock_irqsave(&self) -> SpinlockIrqGuard<'_, T> {
        let flags = irq_save();
        preempt_disable();
        self.raw.lock();
        SpinlockIrqGuard { lock: self, flags }
    }

    /// Disable bottom-half (softirq) + lock.
    /// bh_disable() increments preempt_count by SOFTIRQ_OFFSET,
    /// which also disables preemption.
    /// Guard drop: unlock + bh_enable (decrements preempt_count).
    #[inline]
    pub fn lock_bh(&self) -> SpinlockBhGuard<'_, T> {
        bh_disable();
        self.raw.lock();
        SpinlockBhGuard { lock: self }
    }

    #[inline]
    pub fn try_lock(&self) -> Option<SpinlockGuard<'_, T>> {
        preempt_disable();
        if self.raw.try_lock() {
            Some(SpinlockGuard { lock: self })
        } else {
            preempt_enable();
            None
        }
    }

    #[inline]
    pub fn try_lock_irqsave(&self) -> Option<SpinlockIrqGuard<'_, T>> {
        let flags = irq_save();
        preempt_disable();
        if self.raw.try_lock() {
            Some(SpinlockIrqGuard { lock: self, flags })
        } else {
            preempt_enable();
            irq_restore(flags);
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool { self.raw.is_locked() }

    /// Get a shared reference to the inner data without locking.
    ///
    /// # Safety
    /// Caller must ensure no concurrent mutable access (e.g., data is
    /// write-once during boot and only read thereafter).
    #[inline]
    pub unsafe fn get_ref(&self) -> &T {
        &*self.data.get()
    }

    #[inline]
    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        &mut *self.data.get()
    }

    #[inline]
    pub unsafe fn into_inner(self) -> T { self.data.into_inner() }
}

// ==================== SpinlockGuard (plain) ====================

pub struct SpinlockGuard<'a, T: ?Sized> {
    lock: &'a Spinlock<T>,
}

unsafe impl<T: ?Sized + Send> Send for SpinlockGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target { unsafe { &*self.lock.data.get() } }
}

impl<T: ?Sized> DerefMut for SpinlockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.lock.data.get() } }
}

impl<T: ?Sized> Drop for SpinlockGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.unlock();
        preempt_enable();
    }
}

// ==================== SpinlockIrqGuard (irqsave) ====================

pub struct SpinlockIrqGuard<'a, T: ?Sized> {
    lock: &'a Spinlock<T>,
    flags: bool,
}

unsafe impl<T: ?Sized + Send> Send for SpinlockIrqGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockIrqGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target { unsafe { &*self.lock.data.get() } }
}

impl<T: ?Sized> DerefMut for SpinlockIrqGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.lock.data.get() } }
}

impl<T: ?Sized> Drop for SpinlockIrqGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.unlock();
        preempt_enable();
        irq_restore(self.flags);
    }
}

impl<T: ?Sized> SpinlockIrqGuard<'_, T> {
    /// Release only the spinlock (unlock + preempt_enable), returning
    /// the saved IRQ flags. The caller must later call `irq_restore(flags)`
    /// to restore interrupt state.
    ///
    /// This is used by the scheduler: we must drop the rq lock before
    /// context_switch but keep interrupts disabled until after
    /// context_switch returns (following Linux's pattern where
    /// finish_task_switch releases the lock).
    #[inline]
    pub fn unlock_irqretain(self) -> bool {
        let flags = self.flags;
        self.lock.raw.unlock();
        preempt_enable();
        core::mem::forget(self); // prevent Drop from running
        flags
    }
}

// ==================== SpinlockBhGuard (bottom-half) ====================

pub struct SpinlockBhGuard<'a, T: ?Sized> {
    lock: &'a Spinlock<T>,
}

unsafe impl<T: ?Sized + Send> Send for SpinlockBhGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockBhGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target { unsafe { &*self.lock.data.get() } }
}

impl<T: ?Sized> DerefMut for SpinlockBhGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.lock.data.get() } }
}

impl<T: ?Sized> Drop for SpinlockBhGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.unlock();
        bh_enable();
    }
}

// ==================== Inline helpers ====================

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

#[inline]
fn bh_enable() {
    crate::interrupt::preempt::preempt_count_sub(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
    );
}

// ==================== Free functions (C-style API) ====================

#[inline(always)]
pub fn raw_spin_lock(l: &RawSpinlock) { l.lock(); }
#[inline(always)]
pub fn raw_spin_unlock(l: &RawSpinlock) { l.unlock(); }
#[inline(always)]
pub fn raw_spin_is_locked(l: &RawSpinlock) -> bool { l.is_locked() }
#[inline(always)]
pub fn raw_spin_trylock(l: &mut RawSpinlock) -> bool { l.try_lock() }
