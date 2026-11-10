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
//! - **INV-LOCK-9** (virtio-blk ABBA fix; renumbered 2026-11-07 from a
//!   duplicated INV-LOCK-6 that collided with the virtio-net pair): a wait-queue lock
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
#[cfg(feature = "debug-lock-owner")]
use core::sync::atomic::AtomicUsize;

/// LCB_F_SPIN — the contention_begin flag of a plain exclusive spin
/// lock (include/trace/events/lock.h).
const LCB_F_SPIN: u32 = 1;

// ==================== RawSpinlock (TTAS) ====================

/// Capture this function's return address (the spinlock call site) for
/// deadlock diagnostics. Also the acquire-site source for lockdep-lite
/// P1's keyed hooks (symbolized through debug::ksyms at splat time).
#[inline]
pub(crate) fn caller_return_address() -> usize {
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

// r18 flame refinement (`nextgate` feature, hunt kernels only):
// contended-lock witness histogram keyed by lock ADDRESS. The GB
// flamegraph's kernel-side top bucket (22%) is IP-only — every
// contended lock lands on the same pause instruction — so this table
// records WHICH lock the spin time went to. One witness per ~1M spins
// (the TTAS inner loop), fixed 24-slot table, linear probe with CAS
// claim; entry 0 of the count marks the address slot. Adversarial
// collisions only merge two locks' counts — fine for target ranking.
#[cfg(feature = "nextgate")]
mod lockhist {
    pub const SLOTS: usize = 24;
    pub static ADDR: [core::sync::atomic::AtomicUsize; SLOTS] =
        [const { core::sync::atomic::AtomicUsize::new(0) }; SLOTS];
    pub static N: [core::sync::atomic::AtomicU32; SLOTS] =
        [const { core::sync::atomic::AtomicU32::new(0) }; SLOTS];
    /// r19: the first witness's caller RA per slot — names the acquire
    /// site of heap-allocated locks (their addresses cannot be
    /// symbolized against the ELF).
    pub static RA: [core::sync::atomic::AtomicUsize; SLOTS] =
        [const { core::sync::atomic::AtomicUsize::new(0) }; SLOTS];
}

/// r18 flame refinement: record one contention witness for this lock
/// address (see `lockhist`). Called from the spin loop with IRQ state
/// whatever the caller had — atomics only.
#[cfg(feature = "nextgate")]
fn lockhist_witness(addr: usize, ra: usize) {
    for s in 0..lockhist::SLOTS {
        let a = &lockhist::ADDR[s];
        let cur = a.load(core::sync::atomic::Ordering::Relaxed);
        if cur == addr {
            lockhist::N[s].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            return;
        }
        if cur == 0
            && a
                .compare_exchange(0, addr, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed)
                .is_ok()
        {
            lockhist::N[s].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // r19: the claimant stamps the acquire site once.
            lockhist::RA[s].store(ra, core::sync::atomic::Ordering::Relaxed);
            return;
        }
    }
    // table full: count into the last slot's address-less overflow by
    // incrementing nothing — dropping witnesses is acceptable (ranking
    // only).
}

/// r18 flame refinement: print the contention witness table
/// (putchar_no_lock discipline; safe from any context).
#[cfg(feature = "nextgate")]
pub fn lockhist_print() {
    use crate::console::putchar_no_lock as putchar;
    for s in 0..lockhist::SLOTS {
        let n = lockhist::N[s].load(core::sync::atomic::Ordering::Relaxed);
        if n == 0 {
            continue;
        }
        let a = lockhist::ADDR[s].load(core::sync::atomic::Ordering::Relaxed);
        let ra = lockhist::RA[s].load(core::sync::atomic::Ordering::Relaxed);
        const M: &[u8] = b"\nLOCKHIST addr=0x";
        for &b in M {
            putchar(b);
        }
        let mut v = a as u64;
        for _ in 0..16 {
            let nb = (v >> 60) as u8;
            putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
            v <<= 4;
        }
        const M2: &[u8] = b" n=";
        for &b in M2 {
            putchar(b);
        }
        let mut buf = [0u8; 10];
        let mut k = 0;
        let mut w = n as u64;
        if w == 0 {
            buf[0] = b'0';
            k = 1;
        }
        while w > 0 {
            buf[k] = b'0' + (w % 10) as u8;
            w /= 10;
            k += 1;
        }
        for i in (0..k).rev() {
            putchar(buf[i]);
        }
        // r19: the first witness's acquire site (heap locks' identity).
        // r19b: for heap-allocated locks (physmap range) the address
        // cannot be symbolized — dump the surrounding qwords instead;
        // long-lived containers self-identify (inode names, pool
        // counters). Advisory read of maybe-freed memory is acceptable
        // for a hunt print.
        if a >= 0xffff_8880_0000_0000 && a < 0xffff_9000_0000_0000 {
            const M4: &[u8] = b" ctx=";
            for &b in M4 {
                putchar(b);
            }
            for off in [-2i64, -1, 0, 1, 2, 3, 4, 5] {
                let p = (a as i64 + off * 8) as *const u64;
                // SAFETY: advisory hunt print; may read freed memory.
                let v = unsafe { core::ptr::read_volatile(p) };
                let mut u = v;
                if off != -2 {
                    putchar(b' ');
                }
                for _ in 0..16 {
                    let nb = (u >> 60) as u8;
                    putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                    u <<= 4;
                }
            }
        }
        const M3: &[u8] = b" ra=0x";
        for &b in M3 {
            putchar(b);
        }
        let mut u = ra as u64;
        for _ in 0..16 {
            let nb = (u >> 60) as u8;
            putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
            u <<= 4;
        }
    }
    putchar(b'\n');
}

pub struct RawSpinlock {
    locked: AtomicU32,
    /// Deadlock diagnostics: holder's hart id + 1 (0 = free).  Written
    /// AFTER acquiring / cleared BEFORE releasing, so a concurrent reader
    /// may briefly see a stale value — fine for the watchdog print, which
    /// only needs "who held it when everything wedged".
    #[cfg(feature = "debug-lock-owner")]
    owner: AtomicUsize,
}

impl RawSpinlock {
    #[inline]
    pub const fn new() -> Self {
        Self {
            locked: AtomicU32::new(0),
            #[cfg(feature = "debug-lock-owner")]
            owner: AtomicUsize::new(0),
        }
    }

    /// Record the holder (debug-lock-owner feature only). r12: the
    /// owner word now stores the acquirer's RETURN ADDRESS (cpu+1 in
    /// the low bits kept for the pre-r12 readers) — the GRQ stall probe
    /// needs the holder's call site, not just its CPU.
    ///
    /// Boot-gated inside the feature (debug-dlspin, 2026-11-07): the
    /// stores run only when `spinowner=1` (or forkprof) armed recording
    /// — one relaxed load per acquire in feature builds, nothing in
    /// default builds (zero-instruction mandate on the uncontended
    /// fast path).
    #[inline]
    #[cfg(feature = "debug-lock-owner")]
    fn record_owner(&self, ra: usize) {
        if crate::debug::dlspin::spinowner_armed() {
            self.owner.store(
                (ra & !0xFusize) | ((crate::arch::smp::cpu_id() as usize) + 1),
                Ordering::Relaxed,
            );
        }
    }

    /// Clear the holder (debug-lock-owner feature only).
    #[inline]
    #[cfg(feature = "debug-lock-owner")]
    fn clear_owner(&self) {
        self.owner.store(0, Ordering::Relaxed);
    }

    #[inline]
    #[cfg(not(feature = "debug-lock-owner"))]
    fn record_owner(&self, _ra: usize) {}

    /// Last recorded holder's call site (debug-lock-owner builds; 0
    /// otherwise). Advisory only: read without holding this lock, so
    /// the value may be stale or already overwritten by the reader's
    /// own acquire. For the GRQ stall probe this is fine — a waiter
    /// that spins 4ms+ names the holder that was present at entry.
    #[inline]
    #[cfg(feature = "debug-lock-owner")]
    pub fn owner_hint(&self) -> usize {
        self.owner.load(Ordering::Relaxed) & !0xF
    }

    /// Overwrite the owner word's RA with a DEEPER caller's return
    /// address (debug-lock-owner only). Used by wrapper locks (the GRQ
    /// acquisition wrappers) whose frame is the only one RawSpinlock's
    /// own caller capture sees: after acquiring, the wrapper restamps
    /// so a lost release names the wrapper's CALLER — the true acquire
    /// site — in the deadlock warn's ow_ra field. The cpu nibble is
    /// preserved from the wrapper's own record_owner.
    #[inline]
    #[cfg(feature = "debug-lock-owner")]
    pub fn restamp_owner_ra(&self, ra: usize) {
        let cur = self.owner.load(Ordering::Relaxed);
        self.owner
            .store((ra & !0xFusize) | (cur & 0xF), Ordering::Relaxed);
    }

    #[inline]
    #[cfg(not(feature = "debug-lock-owner"))]
    pub fn restamp_owner_ra(&self, _ra: usize) {}

    #[inline]
    #[cfg(not(feature = "debug-lock-owner"))]
    pub fn owner_hint(&self) -> usize {
        0
    }

    /// r17 mechanism-C hunt (`nextgate` feature): the address of the
    /// lock word, for the DR-register write watchpoint that catches a
    /// wild write onto it red-handed (a lost release leaves the word at
    /// exactly 1; a scribble leaves anything else).
    #[inline]
    #[cfg(feature = "nextgate")]
    pub fn locked_addr(&self) -> usize {
        &self.locked as *const _ as usize
    }

    /// r17 mechanism-C hunt: the address of the owner word (requires
    /// debug-lock-owner). A legitimate stamp always has cpu+1 in the
    /// low nibble and kernel text in the high bits; anything else is a
    /// wild write.
    #[inline]
    #[cfg(all(feature = "nextgate", feature = "debug-lock-owner"))]
    pub fn owner_addr(&self) -> usize {
        &self.owner as *const _ as usize
    }

    #[inline]
    #[cfg(not(feature = "debug-lock-owner"))]
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
        // Tracepoint bracket (round 4, lock:contention_begin/end): the
        // SITES LIVE IN THE CONTENDED BRANCH ONLY — an uncontended
        // acquire (first CAS succeeds) executes neither the flag nor
        // the event check, so the spinlock FAST PATH gained NOTHING
        // (the lane charter's rule). One begin/end pair per acquire
        // that saw contention, regardless of how many TTAS rounds it
        // took. Record path is IRQ-safe (per-CPU SPSC ring, no locks)
        // — the slow path may run with interrupts disabled
        // (lock_irq/lock_irqsave).
        let mut contended = false;
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
            if !contended {
                contended = true;
                crate::trace::trace_contention_begin(
                    self as *const RawSpinlock as u64,
                    LCB_F_SPIN,
                );
            }
            while self.locked.load(Ordering::Relaxed) != 0 {
                spins = spins.wrapping_add(1);
                #[cfg(feature = "nextgate")]
                if spins & 0xFFFF == 0 {
                    // r18 flame refinement: one witness per 64K spins
                    // (≈60-250 µs contended; the spins counter RESETS per
                    // lock() attempt, so a 1M threshold never fires on
                    // ordinary contention — only wedges, which never
                    // reach the shutdown dump) — a cheap per-lock
                    // contention volume histogram for hunt kernels
                    // (IP-only samples cannot name WHICH lock the 22%
                    // bucket is).
                    lockhist_witness(self as *const Self as usize, caller_ra);
                }
                if spins == Self::DEADLOCK_WARN_ITERS {
                    Self::deadlock_warn(self as *const Self, caller_ra);
                    spins = 0; // continue spinning (might resolve)
                }
                core::hint::spin_loop();
            }
        }
        if contended {
            // ret is always 0 — the TTAS acquire cannot fail.
            crate::trace::trace_contention_end(self as *const RawSpinlock as u64, 0);
        }
        // Diagnostics: record the holder AFTER the acquire (a spinner's
        // watchdog may read a stale 0 in the tiny window — acceptable).
        self.record_owner(caller_ra);
    }

    /// Deadlock warn boundary (every DEADLOCK_WARN_ITERS spin rounds):
    /// hand off to the debug-dlspin last-gasp bundle. All printing is
    /// putchar_no_lock-family (IRQ-off safe, no locks, no allocation);
    /// debug-dlspin keeps the one-line warn format and adds the holder
    /// attribution, cross-CPU peek, all-CPU stacks and the task dump.
    /// COLD PATH ONLY — the spin loop's per-iteration work is unchanged.
    fn deadlock_warn(lock_addr: *const Self, caller_ra: usize) {
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
        // r17 (mechanism-C hunt): the lock word's VALUE discriminates the
        // lost-release shapes — 1 = a real acquire that never unlocked,
        // any other nonzero (a stack-pointer-shaped 0xffff8880.. word,
        // ASCII, poison) = a wild write landed on the word (e.g. context
        // -switch register saves through a stale task pointer).
        let msg2b = b" val=0x";
        for &b in msg2b {
            unsafe { crate::console::putchar_no_lock(b); }
        }
        let lval = unsafe { (&*lock_addr).locked.load(Ordering::Relaxed) };
        // u32 word: 8 nibbles (the inherited 64-bit loop shifted a u32
        // by up to 60 — debug builds panic on the overflow, which ate
        // the first owner5 capture).
        let mut shift = (core::mem::size_of::<u32>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (lval >> (shift as usize)) & 0xF;
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
        #[cfg(feature = "debug-lock-owner")]
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
                // r18: the owner word has been RA-stamped since r12, so
                // the old hart-id decode printed nothing; the raw low
                // nibble is the stamping CPU+1 (a legitimate stamp) or
                // whatever a wild write left there — print it raw.
                let nib = holder & 0xF;
                unsafe {
                    crate::console::putchar_no_lock(if nib < 10 {
                        b'0' + nib as u8
                    } else {
                        b'a' + (nib - 10) as u8
                    });
                }
            }
            // r16 (GB GRQ lost-release deadlock): the owner word's HIGH
            // bits are the LAST ACQUIRER'S RETURN ADDRESS (the r12
            // shape) — a lock stuck "locked" with no CPU inside any
            // longer names its lost-release site right here. Print it.
            let msg5b = b" ow_ra=0x";
            for &b in msg5b {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            let ra = holder & !0xFusize;
            let mut shift2 = (core::mem::size_of::<usize>() * 8) as i32;
            while shift2 > 0 {
                shift2 -= 4;
                let nibble = (ra >> (shift2 as usize)) & 0xF;
                let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
                unsafe { crate::console::putchar_no_lock(c); }
            }
        }

        // r18 mechanism-C hunt: the per-CPU GRQ hold table discriminates
        // "a real acquire leaked the section" (slot names the site) from
        // "a wild write put the lock word at 1" (all slots clear).
        #[cfg(feature = "nextgate")]
        {
            crate::sched::sched::grq_hold_table_print();
        }

        unsafe { crate::console::putchar_no_lock(b'\n'); }

        // debug=watchdog: follow the warning with a full task-state snapshot
        // (silent-wedge diagnosis — CPUs idle + sleepers never woken).
        if crate::debug::switches::enabled(crate::debug::switches::DebugSwitch::WatchdogDump) {
            crate::debug::taskdump::dump_all_tasks("deadlock-watchdog");
        }
        // dlspin handoff (branch round-20): the last-gasp bundle owns
        // the WARN_COUNT bookkeeping the dlspin verdict test asserts on
        // (waiter must see warn fired) plus the cross-CPU peek, all-CPU
        // stacks and the one-time task dump. The inline print above
        // keeps the scanner-compatible one-line format.
        // SAFETY: lock_addr is a valid RawSpinlock pointer (from lock()).
        #[cfg(feature = "debug-lock-owner")]
        let owner_word = unsafe { (&*lock_addr).owner.load(Ordering::Relaxed) };
        #[cfg(not(feature = "debug-lock-owner"))]
        let owner_word = 0usize;
        crate::debug::dlspin::deadlock_warn(
            cpu as usize,
            addr,
            caller_ra,
            owner_word,
        );
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
