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
//! Backend: TTAS (test-and-test-and-set) via compare_exchange.
//! Ticket lock causes interactive-input deadlock on QEMU
//! (likely QEMU's amoadd.w emulation bug), so TTAS is used for now.
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
use core::sync::atomic::{AtomicU64, AtomicUsize};

// ==================== RawSpinlock (TTAS) ====================

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
    /// oh-crash 91st (leak hunt): LAST-acquire forensics — pid + call
    /// site + jiffies of the most recent acquire. NOT cleared on unlock
    /// (they name the acquirer even after a leaked/dropped unlock), at
    /// one relaxed store each on the acquire path.
    #[cfg(feature = "dfx-lock-owner")]
    acq_pid: AtomicU32,
    #[cfg(feature = "dfx-lock-owner")]
    acq_site: AtomicUsize,
    #[cfg(feature = "dfx-lock-owner")]
    acq_jiffies: AtomicU64,
}

impl RawSpinlock {
    #[inline]
    pub const fn new() -> Self {
        Self {
            locked: AtomicU32::new(0),
            #[cfg(feature = "dfx-lock-owner")]
            owner: AtomicUsize::new(0),
            #[cfg(feature = "dfx-lock-owner")]
            acq_pid: AtomicU32::new(0),
            #[cfg(feature = "dfx-lock-owner")]
            acq_site: AtomicUsize::new(0),
            #[cfg(feature = "dfx-lock-owner")]
            acq_jiffies: AtomicU64::new(0),
        }
    }

    /// Record the holder (dfx-lock-owner feature only).
    #[inline]
    #[cfg(feature = "dfx-lock-owner")]
    fn record_owner(&self, ra: usize) {
        // cpu_id() is u64 on x86_64 / usize elsewhere — cast to the
        // owner slot's usize.
        self.owner.store((crate::arch::smp::cpu_id() + 1) as usize, Ordering::Relaxed);
        // Leak forensics (see field docs): last acquirer's pid + site.
        self.acq_pid.store(crate::process::current_pid(), Ordering::Relaxed);
        self.acq_site.store(ra, Ordering::Relaxed);
        self.acq_jiffies
            .store(crate::drivers::timer::get_jiffies(), Ordering::Relaxed);
    }

    /// Clear the holder (dfx-lock-owner feature only).
    #[inline]
    #[cfg(feature = "dfx-lock-owner")]
    fn clear_owner(&self) {
        self.owner.store(0, Ordering::Relaxed);
    }

    #[inline]
    #[cfg(not(feature = "dfx-lock-owner"))]
    fn record_owner(&self, _ra: usize) {}

    #[inline]
    #[cfg(not(feature = "dfx-lock-owner"))]
    fn clear_owner(&self) {}

    /// Spinlock deadlock threshold (iterations before warning).
    /// On SMP with QEMU emulation, brief contention is normal — PLIC IRQ
    /// claim/release, GRQ lock, etc. can take 10-100ms of spin time.
    /// 100M iterations ≈ 100-500ms depending on load/CAS latency.
    const DEADLOCK_WARN_ITERS: u32 = 100_000_000;

    #[inline(never)]
    pub fn lock(&self) {
        // Capture caller's return address before spinning
        let caller_ra = caller_return_address();
        let mut spins: u32 = 0;
        // TTAS (test-and-test-and-set, C7): one CAS to acquire; while
        // contended, spin on a plain Relaxed load until the word reads
        // free, then retry the CAS. Relaxed is sufficient for the spin
        // load: it only decides WHEN to retry — the acquiring CAS pairs
        // with the holder's Release store in unlock() and provides all
        // necessary synchronization. The old tight CAS loop kept issuing
        // an atomic RMW on every iteration, hammering the cacheline (and
        // the QEMU TCG atomic slow path) for the whole hold duration.
        loop {
            if self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
                break;
            }
            while self.locked.load(Ordering::Relaxed) != 0 {
                spins = spins.wrapping_add(1);
                if spins == Self::DEADLOCK_WARN_ITERS {
                    Self::deadlock_warn(self as *const Self, caller_ra);
                    spins = 0; // continue spinning (might resolve)
                }
                core::hint::spin_loop();
            }
        }
        // Diagnostics: record the holder AFTER the acquire (a spinner's
        // watchdog may read a stale 0 in the tiny window — acceptable).
        self.record_owner(caller_ra);
    }

    /// Print deadlock warning. pr_err! (an error — stays active in
    /// release builds): printk's pre-ring filter is a single atomic load,
    /// and the per-CPU re-entrancy guard turns a call from inside
    /// printk's own path into a silent drop, never a recursion — so this
    /// is safe even with interrupts disabled or while spinning on a lock
    /// printk itself might touch.
    fn deadlock_warn(lock_addr: *const Self, caller_ra: usize) {
        use core::fmt::Write as _;
        let cpu = crate::arch::smp::cpu_id();
        let addr = lock_addr as usize;

        // Holder identity: owner = hart+1 (0 = free / mid-handoff).
        // oh-crash 91st extension (the meltdown holder hunt): the owner
        // CPU's current task (pid/comm), the heap allocation CONTAINING
        // the lock (small-object span + first words for offline
        // symbolization of any ops/vtable pointers), and one REMOTE-BT
        // of the holder per unique lock address per boot — the IPI
        // handler on the holder captures its live interrupted stack.
        #[allow(unused_mut)]
        let mut holder_cpu: Option<usize> = None;
        #[cfg(feature = "dfx-lock-owner")]
        {
            // SAFETY: lock_addr is a valid RawSpinlock pointer (from lock()).
            let holder = unsafe { (&*lock_addr).owner.load(Ordering::Relaxed) };
            if holder == 0 {
                crate::pr_err!(
                    "DEADLOCK: spinlock stuck cpu={} lock={:#x} ra={:#x} holder=switching",
                    cpu, addr, caller_ra
                );
            } else {
                let h = holder - 1;
                holder_cpu = Some(h);
                // Holder CPU's current task: pid + comm (best-effort —
                // an irqsave holder cannot migrate, so this is the task
                // that acquired; plain lock() holders may have been
                // preempted, in which case the remote-BT below shows
                // what the CPU runs NOW and the pid names the victim).
                // SAFETY: cpu_state(h).current is the scheduler's own
                // published pointer; read-only access.
                let cur = unsafe { crate::sched::sched::cpu_state(h).current };
                if !cur.is_null() {
                    // SAFETY: read-only access to a live Task; comm is a
                    // fixed [u8;16], no allocation.
                    let pid = unsafe { (*cur).pid() };
                    let comm = unsafe { (*cur).comm() };
                    let mut j = 0;
                    while j < comm.len() && comm[j] != 0 {
                        j += 1;
                    }
                    let comm_str = core::str::from_utf8(&comm[..j]).unwrap_or("?");
                    crate::pr_err!(
                        "DEADLOCK: spinlock stuck cpu={} lock={:#x} ra={:#x} holder={} task={}({})",
                        cpu, addr, caller_ra, h, pid, comm_str
                    );
                } else {
                    crate::pr_err!(
                        "DEADLOCK: spinlock stuck cpu={} lock={:#x} ra={:#x} holder={}",
                        cpu, addr, caller_ra, h
                    );
                }
            }

            // Leak forensics (dfx-lock-owner): the LAST acquirer's pid,
            // call site and acquire age — these persist past a cleared
            // owner word, so a leaked lock names who took it and when
            // even when the owner stamp is mid-handoff. Plus the raw
            // locked word (1 = genuinely held; anything else is a torn
            // or corrupted state worth seeing).
            {
                // SAFETY: lock_addr is a valid RawSpinlock pointer (from lock()).
                let l = unsafe { &*lock_addr };
                crate::pr_err!(
                    "LOCK-ACQ: word={:#x} pid={} site={:#x} age={}j",
                    l.locked.load(Ordering::Relaxed),
                    l.acq_pid.load(Ordering::Relaxed),
                    l.acq_site.load(Ordering::Relaxed),
                    crate::drivers::timer::get_jiffies()
                        .wrapping_sub(l.acq_jiffies.load(Ordering::Relaxed))
                );
            }
        }
        #[cfg(not(feature = "dfx-lock-owner"))]
        {
            crate::pr_err!(
                "DEADLOCK: spinlock stuck cpu={} lock={:#x} ra={:#x}",
                cpu, addr, caller_ra
            );
        }

        // Containing heap object (works with or without a known holder):
        // slab span + the first 6 words of the object. Any word landing
        // in kernel text (0xffffffff8...) symbolizes offline against
        // gen-ksyms output — that names the lock's OWNER STRUCT (ops
        // table / vtable / function pointer members).
        #[allow(unused_mut)]
        if let Some((base, sz)) = crate::mm::small_alloc::object_span(lock_addr as usize) {
            let mut line = [0u8; 160];
            let mut cur = crate::printk::BufferWriter::new(&mut line);
            let _ = write!(
                cur,
                "LOCK-OBJ: small kmalloc-{} base={:#x} words=",
                sz, base
            );
            for i in 0..6 {
                // SAFETY: diagnostic read of the object's first words;
                // the span was validated by object_span (inside a live
                // slab page, in-bounds of the object's slot).
                let w = unsafe { ((base + i * 8) as *const u64).read_unaligned() };
                let _ = write!(cur, "{}{:#x}", if i == 0 { "" } else { "," }, w);
            }
            let len = cur.pos();
            drop(cur);
            let text = core::str::from_utf8(&line[..len]).unwrap_or("");
            crate::pr_err!("{}", text);
        } else if let Some((base, order, free)) =
            crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.buddy_block_span(lock_addr as usize)
        {
            // oh-crash 98th: DIRECT BUDDY blocks (>1KB kmalloc — Task
            // structs, large fs/ipc objects). Same offline-symbolization
            // trick on the first words; `free!=0` (a lock inside a FREE
            // block) is itself evidence of a freed-with-lock-reference
            // object, printed as words=FREE.
            if free != 0 {
                crate::pr_err!(
                    "LOCK-OBJ: buddy order-{} base={:#x} free={} words=FREE",
                    order, base, free
                );
            } else {
                let mut line = [0u8; 160];
                let mut cur = crate::printk::BufferWriter::new(&mut line);
                let _ = write!(
                    cur,
                    "LOCK-OBJ: buddy order-{} base={:#x} free={} words=",
                    order, base, free
                );
                for i in 0..6 {
                    // SAFETY: diagnostic read of the block's first words;
                    // buddy_block_span validated the address against the
                    // heap window and the block's base alignment.
                    let w = unsafe { ((base + i * 8) as *const u64).read_unaligned() };
                    let _ = write!(cur, "{}{:#x}", if i == 0 { "" } else { "," }, w);
                }
                let len = cur.pos();
                drop(cur);
                let text = core::str::from_utf8(&line[..len]).unwrap_or("");
                crate::pr_err!("{}", text);
            }
        }

        // One REMOTE-BT per unique lock address per boot: the IPI
        // handler on the holder CPU captures the live interrupted
        // stack. (x86_64 only — the riscv64/aarch64 twins have no
        // IRQ_WORK remote capture yet.)
        #[cfg(feature = "x86_64")]
        {
            static LAST_BT_LOCK: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let la = lock_addr as usize;
            let prev = LAST_BT_LOCK.load(Ordering::Acquire);
            if prev != la
                && LAST_BT_LOCK
                    .compare_exchange(prev, la, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                if let Some(h) = holder_cpu {
                    let _ = crate::arch::ipi::remote_backtrace(h, "deadlock-holder");
                }
            }
        }

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
            self.record_owner(caller_return_address());
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
    /// track_caller: leakorigin's outermost-acquire shadow records the
    /// LOCK CALLER's site (preempt-hunt), not this helper's line.
    ///
    /// Reference discipline (sync-realign, 2026-11): a spinlock NEVER
    /// sleeps. The acquire is preempt_disable + pure bounded spinning
    /// (spin until acquired — the DEADLOCK_WARN threshold only prints,
    /// it never gives up); no task-state change and no schedule()
    /// anywhere in this path. This is sound ONLY because every lock
    /// that may be held across blocking I/O is a SleepMutex
    /// (kernel/src/sync/mutex.rs), never a Spinlock: a preempt-disabled
    /// spinner can starve the holder's wake IPI, so a Spinlock section
    /// must stay short by construction. The former "preemptible
    /// bounded wait" that yielded via schedule() here (preempt-hunt,
    /// then the lost-park-patched handoff) was a divergence from the
    /// reference design and is deleted — the lost-park hazard class
    /// (a caller inside a wait path's prepared window parked inside a
    /// spinlock acquire) is structurally impossible now.
    #[inline]
    #[track_caller]
    pub fn lock(&self) -> SpinlockGuard<'_, T> {
        preempt_disable();
        self.raw.lock();
        SpinlockGuard { lock: self }
    }

    /// Disable interrupts + preempt disable + lock.
    /// Guard drop: unlock + preempt enable + restore interrupts.
    #[inline]
    #[track_caller]
    pub fn lock_irq(&self) -> SpinlockIrqGuard<'_, T> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
        self.raw.lock();
        SpinlockIrqGuard { lock: self, flags }
    }

    /// Save interrupt state + disable + preempt disable + lock.
    /// Guard drop: unlock + preempt enable + restore interrupt state.
    #[inline]
    #[track_caller]
    pub fn lock_irqsave(&self) -> SpinlockIrqGuard<'_, T> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
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
    #[track_caller]
    pub fn try_lock(&self) -> Option<SpinlockGuard<'_, T>> {
        preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_lock() {
            Some(SpinlockGuard { lock: self })
        } else {
            preempt_enable();
            None
        }
    }

    #[inline]
    #[track_caller]
    pub fn try_lock_irqsave(&self) -> Option<SpinlockIrqGuard<'_, T>> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
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
        // Leak-hunt: pass the lock instance so the underflow report can
        // name WHICH Spinlock double-dropped (static locks symbolize
        // against the ELF; heap locks identify the owning object).
            crate::interrupt::preempt::preempt_count_sub2_at(
                crate::interrupt::preempt::PREEMPT_OFFSET,
                core::panic::Location::caller(),
                // RawSpinlock is Spinlock's first field, so its address
                // IS the Spinlock instance's address (T may be ?Sized,
                // making &Spinlock<T> a fat reference — go through the
                // thin raw field pointer).
                &self.lock.raw as *const RawSpinlock as usize,
            );
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
#[track_caller]
fn preempt_disable() {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        core::panic::Location::caller(),
    );
}

/// As `preempt_disable`, booking the CALLER-of-the-lock's location (the
/// #[track_caller] lock helpers forward their own caller down so the
/// leakorigin shadow names the real acquire site).
#[inline]
fn preempt_disable_at(loc: &'static core::panic::Location<'static>) {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        loc,
    );
}

#[inline]
#[track_caller]
fn preempt_enable() {
    crate::interrupt::preempt::preempt_count_sub_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        core::panic::Location::caller(),
    );
}

#[inline]
#[track_caller]
fn irq_save() -> bool {
    // track_caller: syscost's REST histogram sees the outermost lock
    // user instead of this line (no-op unless syscost is armed).
    crate::arch::cpu::save_and_disable_irq()
}

#[inline]
#[track_caller]
fn irq_restore(flags: bool) {
    crate::arch::cpu::restore_irq(flags);
}

#[inline]
#[track_caller]
fn bh_disable() {
    crate::interrupt::preempt::preempt_count_add(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
    );
}

#[inline]
#[track_caller]
fn bh_enable() {
    crate::interrupt::preempt::preempt_count_sub_at(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
        core::panic::Location::caller(),
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
