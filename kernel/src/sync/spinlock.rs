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
//!
//! # Safety Invariants — Lock Ordering & Deadlock Prevention
//!
//! Lock nesting must always follow this outermost-to-innermost order.
//! Violating this order risks deadlock.
//!
//! ```text
//! Level 0 (outermost): IRQ disable (irq_save / irq)
//!   └── Level 1: preempt_disable
//!         ├── Level 2a: GRQ lock (sched/sched.rs)
//!         │     └── Level 3a: per-zone lock (mm/zone.rs)
//!         │     └── Level 3b: futex hash bucket lock (sync/futex.rs)
//!         │           └── Level 4: waiter slot lock
//!         ├── Level 2b: process tree lock (process/task.rs)
//!         ├── Level 2c: inode lock (fs/)
//!         └── Level 2d: dentry cache lock (fs/)
//! ```
//!
//! - **INV-LOCK-1**: `preempt_disable` must precede any spinlock acquire.
//!
//! - **INV-LOCK-2**: `irq_save` (or `irq`) must precede `preempt_disable`
//!   when both are needed.
//!
//! - **INV-LOCK-3**: Release order is the reverse: unlock → preempt_enable →
//!   irq_restore.
//!
//! - **INV-LOCK-4**: No lock acquisition cycles across levels (deadlock freedom).
//!   In particular, a Level-3 lock must never be acquired while holding a
//!   different Level-2 lock.
//!
//! - **INV-LOCK-5**: GRQ lock and futex hash bucket lock nesting direction is
//!   consistent: GRQ may nest inside futex bucket, but never the reverse.
//!
//! - **INV-LOCK-6** (virtio-blk ABBA fix): a wait-queue lock
//!   (`WaitQueueHead`'s internal `Spinlock<Vec<WaitQueueEntry>>`) must NEVER
//!   be acquired while holding a virtio driver lock (`VIRTIO_PCI_BLK_LOCK`,
//!   the MMIO device's `virtqueue` lock, `VIRTIO_*_PENDING`). Wait-queue
//!   wakeups run `sched::wake_up_process` (GRQ lock) per waiter while
//!   holding the wait-queue lock, so nesting it under a virtio lock builds
//!   the BLK→waitqueue convoy that froze GNOME final6 (deadlock watchdog:
//!   one CPU stuck on the virtio BSS lock, another on a heap wait-queue
//!   lock, timer IRQs stopped). Drivers must COLLECT completed work under
//!   their locks and deliver wakes OUTSIDE them (timer.rs R12-3 pattern);
//!   see drivers/virtio/mod.rs and `assert_no_virtio_lock`.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};
#[cfg(feature = "dfx-lock-owner")]
use core::sync::atomic::AtomicUsize;

// ==================== RawSpinlock (TAS) ====================

/// Capture this function's return address (the spinlock call site) for
/// deadlock diagnostics.
#[inline]
fn caller_return_address() -> usize {
    let ra: usize;
    #[cfg(feature = "riscv64")]
    // SAFETY: pure register read, no memory access or side effects.
    unsafe {
        core::arch::asm!("mv {}, ra", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(feature = "x86_64")]
    // SAFETY: frame pointers are forced on, so [rbp+8] is the return
    // address of this inlined function's caller.
    unsafe {
        core::arch::asm!("mov {}, qword ptr [rbp + 8]", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    {
        ra = 0;
    }
    ra
}

pub struct RawSpinlock {
    locked: AtomicU32,
    /// Deadlock diagnostics: holder's hart id + 1 (0 = free).  Written
    /// AFTER acquiring / cleared BEFORE releasing, so a concurrent reader
    /// may briefly see a stale value — fine for the watchdog print, which
    /// only needs "who held it when everything wedged".
    #[cfg(feature = "dfx-lock-owner")]
    owner: AtomicUsize,
}

impl RawSpinlock {
    #[inline]
    pub const fn new() -> Self {
        Self {
            locked: AtomicU32::new(0),
            #[cfg(feature = "dfx-lock-owner")]
            owner: AtomicUsize::new(0),
        }
    }

    /// Record the holder (dfx-lock-owner feature only).
    #[inline]
    #[cfg(feature = "dfx-lock-owner")]
    fn record_owner(&self) {
        self.owner.store(crate::arch::smp::cpu_id() + 1, Ordering::Relaxed);
    }

    /// Clear the holder (dfx-lock-owner feature only).
    #[inline]
    #[cfg(feature = "dfx-lock-owner")]
    fn clear_owner(&self) {
        self.owner.store(0, Ordering::Relaxed);
    }

    #[inline]
    #[cfg(not(feature = "dfx-lock-owner"))]
    fn record_owner(&self) {}

    #[inline]
    #[cfg(not(feature = "dfx-lock-owner"))]
    fn clear_owner(&self) {}

    /// Spinlock deadlock threshold (iterations before warning).
    /// On SMP with QEMU emulation, brief contention is normal — PLIC IRQ
    /// claim/release, GRQ lock, etc. can take 10-100ms of spin time.
    /// 100M iterations ≈ 100-500ms depending on CAS latency.
    const DEADLOCK_WARN_ITERS: u32 = 100_000_000;

    #[inline(never)]
    pub fn lock(&self) {
        // Capture caller's return address before spinning
        let caller_ra = caller_return_address();
        let mut spins: u32 = 0;
        while self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_err() {
            spins = spins.wrapping_add(1);
            if spins == Self::DEADLOCK_WARN_ITERS {
                Self::deadlock_warn(self as *const Self, caller_ra);
                spins = 0; // continue spinning (might resolve)
            }
            core::hint::spin_loop();
        }
        // Diagnostics: record the holder AFTER the acquire (a spinner's
        // watchdog may read a stale 0 in the tiny window — acceptable).
        self.record_owner();
    }

    /// Print deadlock warning via SBI (works even with interrupts disabled).
    fn deadlock_warn(lock_addr: *const Self, caller_ra: usize) {
        // Use SBI putchar directly — printk might need locks we're spinning on
        let cpu = crate::arch::smp::cpu_id();
        let msg = b"DEADLOCK: spinlock stuck cpu=";
        for &b in msg {
            unsafe { crate::console::putchar_no_lock(b); }
        }
        // Print CPU id as decimal digit
        if cpu < 10 {
            unsafe { crate::console::putchar_no_lock(b'0' + cpu as u8); }
        }
        // Print lock address in hex
        let msg2 = b" lock=0x";
        for &b in msg2 {
            unsafe { crate::console::putchar_no_lock(b); }
        }
        let addr = lock_addr as usize;
        let mut shift = (core::mem::size_of::<usize>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (addr >> (shift as usize)) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            unsafe { crate::console::putchar_no_lock(c); }
        }

        // Print caller return address (ra) for debugging
        let msg3 = b" ra=0x";
        for &b in msg3 {
            unsafe { crate::console::putchar_no_lock(b); }
        }
        let mut shift = (core::mem::size_of::<usize>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (caller_ra >> shift) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            unsafe { crate::console::putchar_no_lock(c); }
        }

        // Holder identity: owner = hart+1 (0 = free / mid-handoff).
        #[cfg(feature = "dfx-lock-owner")]
        {
            let msg4 = b" holder=";
            for &b in msg4 {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            // SAFETY: lock_addr is a valid RawSpinlock pointer (from lock()).
            let holder = unsafe { (&*lock_addr).owner.load(Ordering::Relaxed) };
            if holder == 0 {
                let msg5 = b"switching";
                for &b in msg5 {
                    unsafe { crate::console::putchar_no_lock(b); }
                }
            } else {
                let h = holder - 1;
                if h < 10 {
                    unsafe { crate::console::putchar_no_lock(b'0' + h as u8); }
                }
            }
        }

        unsafe { crate::console::putchar_no_lock(b'\n'); }

        // dfx=watchdog: follow the warning with a full task-state snapshot
        // (silent-wedge diagnosis — CPUs idle + sleepers never woken).
        if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::WatchdogDump) {
            crate::dfx::taskdump::dump_all_tasks("deadlock-watchdog");
        }
    }

    #[inline]
    pub fn try_lock(&self) -> bool {
        let ok = self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok();
        if ok {
            self.record_owner();
        }
        ok
    }

    #[inline]
    pub fn unlock(&self) {
        self.clear_owner();
        self.locked.store(0, Ordering::Release);
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Acquire) != 0
    }

    #[inline]
    pub unsafe fn reset(&mut self) {
        self.clear_owner();
        self.locked.store(0, Ordering::Release);
    }
}

// ==================== Spinlock<T> ====================

pub struct Spinlock<T: ?Sized> {
    raw: RawSpinlock,
    data: UnsafeCell<T>,
}

// SAFETY: Spinlock<T> only allows &mut T access via the guard, which exists
// only while the inner lock is held.  Send is required so the lock (and its
// data) can be moved between threads; Sync is safe because the lock mediates
// all access.
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

    /// Bounded-spin + yield acquire for COARSE, long-held global locks
    /// (VFS_MUTATION_LOCK, EXT4_BIG_LOCK — held across synchronous block
    /// I/O, i.e. for milliseconds at a time).
    ///
    /// Wedge background (LTP r2 WEDGE family: inode02, ftest02/04/06,
    /// creat09, sendmsg02): when enough tasks contend a plain lock(),
    /// every CPU ends up spinning preempt-disabled; the lock holder —
    /// which may be BLOCKED in virtio I/O or simply not resident on any
    /// CPU — can then never be rescheduled (wake IPIs cannot preempt a
    /// preempt-disabled spinner), and the machine freezes with all CPUs
    /// pinned. This acquire spins a bounded budget like lock(); if the
    /// lock still isn't free it re-enables preemption and yields the CPU
    /// (schedule()) so the holder can run HERE, then retries. Waiters
    /// therefore never starve the system of a schedulable CPU.
    ///
    /// NOT for IRQ context or short critical sections — schedule() from
    /// interrupt context is illegal; use plain lock()/lock_irq() there.
    pub fn lock_fair(&self) -> SpinlockGuard<'_, T> {
        const FAIR_SPIN_BUDGET: u32 = 1_000_000;
        loop {
            preempt_disable();
            let mut spins: u32 = 0;
            loop {
                if self.raw.try_lock() {
                    return SpinlockGuard { lock: self };
                }
                spins += 1;
                if spins >= FAIR_SPIN_BUDGET {
                    break;
                }
                core::hint::spin_loop();
            }
            // Budget exhausted: hand the CPU to the holder instead of
            // pinning it. The schedule() picks another task (likely the
            // holder); we come back and retry afterwards.
            preempt_enable();
            crate::sched::schedule();
        }
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

    /// Get a mutable reference to the inner data without locking.
    ///
    /// # Safety
    /// Caller must ensure exclusive access (e.g., no concurrent readers or writers).
    #[inline]
    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        &mut *self.data.get()
    }

    /// Consume the lock and return the inner data.
    ///
    /// # Safety
    /// Caller must ensure no other thread holds a reference to the lock or data.
    #[inline]
    pub unsafe fn into_inner(self) -> T { self.data.into_inner() }
}

// ==================== SpinlockGuard (plain) ====================

pub struct SpinlockGuard<'a, T: ?Sized> {
    lock: &'a Spinlock<T>,
}

// SAFETY: SpinlockGuard exists only while the lock is held, so the guarded
// data cannot be accessed concurrently from another thread.
unsafe impl<T: ?Sized + Send> Send for SpinlockGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Guard holds the lock — no concurrent &mut access possible.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinlockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Guard holds the lock exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
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

// SAFETY: SpinlockIrqGuard exists only while the lock is held, so the
// guarded data cannot be accessed concurrently.
unsafe impl<T: ?Sized + Send> Send for SpinlockIrqGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockIrqGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Guard holds the lock — no concurrent &mut access possible.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinlockIrqGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Guard holds the lock exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
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

// SAFETY: SpinlockBhGuard exists only while the lock is held, so the
// guarded data cannot be accessed concurrently.
unsafe impl<T: ?Sized + Send> Send for SpinlockBhGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockBhGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Guard holds the lock — no concurrent &mut access possible.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinlockBhGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Guard holds the lock exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
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
    crate::arch::cpu::save_and_disable_irq()
}

#[inline]
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

// ==================== Free functions (C-style API) ====================

#[inline(always)]
pub fn raw_spin_lock(l: &RawSpinlock) { l.lock(); }
#[inline(always)]
pub fn raw_spin_unlock(l: &RawSpinlock) { l.unlock(); }
#[inline(always)]
pub fn raw_spin_is_locked(l: &RawSpinlock) -> bool { l.is_locked() }
#[inline(always)]
pub fn raw_spin_trylock(l: &mut RawSpinlock) -> bool { l.try_lock() }
