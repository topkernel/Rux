//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The sync subsystem's audited unsafe edges (framework zone).
//!
//! unsafe-slim round L1 (2026-11-20), framekernel doctrine: the sync
//! POLICY files (futex.rs, futex_pi.rs) are safe by construction; the
//! genuinely-unsafe memory edges they need live here with INV-SY
//! contracts. Task pointer→reference derivations reuse
//! [`crate::sched::unsafe::task_shared`] (same INV-SCE-2 contract).
//!
//! INV registry:
//! - INV-SY-1 `*_via_task` — cross-task USER-memory diagnostic reads:
//!   walk the (foreign) task's page tables, translate, read volatile.
//!   `task` is live (walker pin / GRQ / census caller); the reads are
//!   diagnostic (racy by nature, exactly like the raw reads replaced);
//!   the mapped user page cannot be unmapped for the duration of the
//!   single volatile read that follows the walk (the owning task is
//!   parked on the futex being diagnosed).
//! - INV-SY-2 `entry_frame_*` — the task's trap frame at the top of
//!   its own kernel stack (the pt_regs window), read-only: same layout
//!   contract as `Task::pt_regs_ref`.
//! - INV-SY-3 `word_read`/`word_cas` — the futex user word through the
//!   arch uaccess accessors (fault-isolated); caller holds the hash
//!   bucket that serializes kernel-side accessors of the word.
//! - INV-SY-4 `guard_shared`/`guard_exclusive` — the lock-guard
//!   Deref/DerefMut edge: the caller holds a live guard of the lock
//!   owning the cell, proving the read/exclusivity for the returned
//!   reference's lifetime (pass 3: the 20 hotpath.rs guard derefs).
//! - INV-SY-5 `keyed_guard_unlock` — KeyedGuard::drop's raw-pointer
//!   deref: the live keyed guard owns the lock pointer exclusively
//!   (pass 3).

use crate::process::types::Task;
use super::hotpath::NestKey;
use super::types::KeyedRawSpinlock;
use core::cell::UnsafeCell;
use super::hotpath::{
    GP_CONSEC_FAIL, GP_DUMPED, RCU_CBS, RCU_QS_GEN, RCU_WAITERS,
    REENTRY_EVENTS, all_online_cpus_reported, cpu_quiescent,
    shared_grace_done,
};
use super::types::{
    CpuLocal, CpuLocalRef, KeyedSpinlock, OptimisticProbe, PiWaiter,
    RawSpinlock, Row, RcuHead, RwSemaphore, RwSpinlock,
    RwSpinlockBhReadGuard, RwSpinlockBhWriteGuard,
    RwSpinlockIrqReadGuard, RwSpinlockIrqWriteGuard, RwSpinlockReadGuard,
    RwSpinlockWriteGuard, Rwsem, RwsemReadGuard, RwsemWriteGuard,
    SeqLock, SeqLockWriteGuard, SleepMutex, SleepMutexGuard, Spinlock,
    SpinlockBhGuard, SpinlockGuard, SpinlockIrqGuard, Waiter,
};
use crate::list::ListHead;
use crate::config::MAX_CPUS;
use core::marker::PhantomData;
use core::sync::atomic::Ordering;

// ---------------------------------------------------------------------------
// INV-SY-4: guard-held data dereference (the lock-guard Deref/DerefMut edge)
//
// The call sites are the guard Deref/DerefMut impls of hotpath.rs: each
// holds a live guard proving the lock's own discipline, so the data cell
// behind the guard may be reached with exactly the guard's own exclusivity.
// These fns exist so the deref SITES carry no inline `unsafe` — the edge is
// named once, here, with the contract below.
// ---------------------------------------------------------------------------

/// Shared (read) dereference of a lock's data cell (pass-3 collection).
///
/// # Contract (INV-SY-4)
///
/// `cell` is the `data` field of a lock whose SHARED guard the caller
/// currently holds (SpinlockGuard / RwSpinlock read-family / Rwsem read /
/// SeqLock write guard serialized by the write phase) — a live shared
/// guard excludes writers for the returned reference's lifetime, which
/// the borrow of `cell` (reached through the guard) ties down.
#[inline(always)]
pub(crate) fn guard_shared<'a, T: ?Sized>(cell: &'a UnsafeCell<T>) -> &'a T {
    // SAFETY: INV-SY-4 — the caller's live shared guard excludes writes
    // for 'a; UnsafeCell::get is the documented interior-mutability edge.
    unsafe { &*cell.get() }
}

/// Exclusive (mutable) dereference of a lock's data cell (pass-3
/// collection).
///
/// # Contract (INV-SY-4)
///
/// `cell` is the `data` field of a lock whose EXCLUSIVE guard the caller
/// currently holds (SpinlockGuard / SpinlockIrqGuard / SpinlockBhGuard /
/// RwSpinlock write-family / Rwsem write / SeqLock write guard — each
/// proof at its call site): no other reference to the data can exist
/// while the guard is live, so the `&mut` is exclusive for 'a.
#[inline(always)]
pub(crate) fn guard_exclusive<'a, T: ?Sized>(cell: &'a UnsafeCell<T>) -> &'a mut T {
    // SAFETY: INV-SY-4 — the caller's live exclusive guard excludes every
    // other access for 'a (proof named per call site).
    unsafe { &mut *cell.get() }
}

// ---------------------------------------------------------------------------
// INV-SY-5: the keyed-guard release edge (KeyedGuard::drop)
// ---------------------------------------------------------------------------

/// Release the raw lock a live [`KeyedRawSpinlock`] guard owns (pass-3
/// collection of KeyedGuard::drop's raw-pointer deref).
///
/// # Contract (INV-SY-5)
///
/// `lock` is the pointer a live KeyedGuard was minted with (KeyedGuard
/// holds it exclusively for its lifetime — the guard is `!Send` and
/// cannot be duplicated); the deref is in-bounds for the guard's whole
/// life and the release is RawSpinlock::unlock's audited owner-in-word
/// protocol.
#[inline(always)]
pub(crate) fn keyed_guard_unlock<K: NestKey>(lock: *mut KeyedRawSpinlock<K>) {
    // SAFETY: INV-SY-5 — the live guard owns `lock` exclusively.
    unsafe { (*lock).raw.unlock() }
}

// ---------------------------------------------------------------------------
// INV-SY-1: cross-task user-memory diagnostic reads
// ---------------------------------------------------------------------------

/// Read up to `buf.len()` bytes in `task`'s user address space via its
/// page tables, stopping at the first unmapped page (KILLDUMP sc97:
/// iovec arrays, path strings, log payload heads of the wedged task).
/// Returns how many bytes were actually read.
pub fn read_bytes_via_task(task: *const Task, va: usize, buf: &mut [u8]) -> usize {
    let tk = crate::sched::r#unsafe::task_shared(task);
    let aspace = match tk.address_space() {
        Some(a) => a,
        None => return 0,
    };
    let root = aspace.root_ppn();
    let mut done = 0usize;
    while done < buf.len() {
        let cur = va + done;
        // SAFETY: INV-SY-1 — the page-table walk is read-only.
        let (ppn, _) = match unsafe {
            crate::arch::mm::mm_ops::PageTableWalker::walk(root, cur as u64)
        } {
            Some(p) => p,
            None => break,
        };
        let pa = (ppn as usize) << 12;
        let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64))
            .bits() as usize
            + (cur & 0xfff);
        let chunk = core::cmp::min(buf.len() - done, 0x1000 - (cur & 0xfff));
        for i in 0..chunk {
            // SAFETY: INV-SY-1 — one volatile byte read of the mapped
            // user page the walk just resolved.
            buf[done + i] = unsafe { core::ptr::read_volatile((kv + i) as *const u8) };
        }
        done += chunk;
    }
    done
}

/// Printable snapshot of a NUL-terminated user string in `task`'s space.
pub fn read_cstr_via_task(task: *const Task, va: usize, max: usize) -> alloc::string::String {
    if va == 0 {
        return alloc::string::String::from("(null)");
    }
    let mut raw = alloc::vec![0u8; max];
    let n = read_bytes_via_task(task, va, &mut raw);
    let end = raw[..n].iter().position(|&b| b == 0).unwrap_or(n);
    let mut s = alloc::string::String::new();
    for &b in &raw[..end] {
        if (0x20..0x7f).contains(&b) {
            s.push(b as char);
        } else {
            s.push('.');
        }
    }
    s
}

/// Read one u32 in `task`'s user address space via its page tables
/// (khungtaskd's get_user would read through the wrong mm).
pub fn read_u32_via_task(task: *const Task, va: usize) -> Option<u32> {
    let tk = crate::sched::r#unsafe::task_shared(task);
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    // SAFETY: INV-SY-1 — read-only page-table walk.
    let (ppn, _) = unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va as u64)? };
    let pa = (ppn as usize) << 12;
    let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64)).bits()
        as usize
        + (va & 0xfff);
    // SAFETY: INV-SY-1 — one volatile u32 read of the mapped page.
    Some(unsafe { core::ptr::read_volatile(kv as *const u32) })
}

// ---------------------------------------------------------------------------
// INV-SY-3: the futex user word
// ---------------------------------------------------------------------------

/// Read the futex user word at `uaddr` (fault-isolated; `None` on a bad
/// pointer). Caller holds the hash bucket that serializes kernel-side
/// accessors of the word (the plain side's WAKE_OP contract).
pub fn word_read(uaddr: usize) -> Option<u32> {
    // SAFETY: INV-SY-3 — uaddr came from the futex syscall's validated
    // user pointer; get_user is fault-isolated.
    unsafe { crate::arch::uaccess::get_user(uaddr as *const u32) }
}

/// Compare-and-swap the user word: writes `new` only when it reads
/// `expect`. Returns the observed pre-value; callers compare against
/// `expect` to learn whether the swap happened.
pub fn word_cas(uaddr: usize, expect: u32, new: u32) -> Option<u32> {
    let cur = word_read(uaddr)?;
    // SAFETY: INV-SY-3 — the fault-isolated store; a `false` return
    // (write faulted) is treated as a lost CAS.
    if cur == expect && !unsafe { crate::arch::uaccess::put_user(uaddr as *mut u32, new) } {
        return None;
    }
    Some(cur)
}

/// Walk a (foreign) task's page tables (INV-SY-1 walk edge): read-only
/// translation; `root` is the aspace's root_ppn of a live task.
pub fn pt_walk(root: u64, va: u64) -> Option<(u64, u64)> {
    // SAFETY: INV-SY-1 — the page-table walk is read-only.
    unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) }
}

/// One volatile u64 read of an address the caller has validated
/// (INV-SY-1 diag): kernel-stack windows, translated user pages.
/// Volatile by contract — the data may be written concurrently by the
/// owning CPU; readers take what they see.
#[inline]
pub fn read_volatile_u64(addr: usize) -> u64 {
    // SAFETY: INV-SY-1 — single volatile load of a validated address.
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

/// One volatile u32 read (INV-SY-1 diag twin of read_volatile_u64).
#[inline]
pub fn read_volatile_u32(addr: usize) -> u32 {
    // SAFETY: INV-SY-1 — single volatile load of a validated address.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// copy_from_user with a byte count (INV-SY-3): fault-isolated user
/// read into kernel memory; returns the bytes NOT copied (Linux
/// convention — 0 means full success).
pub fn copy_from_user_bytes(dst: *mut u8, src: usize, len: usize) -> usize {
    // SAFETY: INV-SY-3 — the futex syscall's validated user pointer;
    // copy_from_user is fault-isolated.
    unsafe { crate::arch::uaccess::copy_from_user(dst, src as *const u8, len) }
}

/// Store the futex user word unconditionally (INV-SY-3): the
/// fault-isolated put; returns true when the write landed.
pub fn word_write(uaddr: usize, value: u32) -> bool {
    // SAFETY: INV-SY-3 — validated futex user pointer.
    unsafe { crate::arch::uaccess::put_user(uaddr as *mut u32, value) }
}


// ==========================================================================
// pass 3 (seed-core §2c.5): sync's collected unsafe — the inline
// blocks of the subsystem's mechanism files, one contract each.
// ==========================================================================

// ------------ auto-trait proofs (Send / Sync) ------------

// SAFETY: Waiter is only ever accessed while the per-slot Spinlock is held,
// serialising all reads and writes.  The `task` raw pointer is only
// dereferenced by the waker after validating it is non-null.
/// INV-SCE-2: the Waiter task raw pointer rides the sched task_shared read view; all other access is serialized by the per-slot lock.
unsafe impl Send for Waiter {}
/// INV-SCE-2: the Waiter task raw pointer rides the sched task_shared read view; all other access is serialized by the per-slot lock.
unsafe impl Sync for Waiter {}
// SAFETY: PiWaiter slots are only accessed while the per-slot spinlock
// is held; the task raw pointer is only dereferenced by the waker after
// a non-null check — the same contract as the plain Waiter.
/// INV-SENDSYNC-1: slots are only touched under their per-slot spinlock — the raw-lock Sync proof.
unsafe impl Send for PiWaiter {}
/// INV-SENDSYNC-1: slots are only touched under their per-slot spinlock — the raw-lock Sync proof.
unsafe impl Sync for PiWaiter {}
// SAFETY: Spinlock<T> only allows &mut T access via the guard, which exists
// only while the inner lock is held.  Send is required so the lock (and its
// data) can be moved between threads; Sync is safe because the lock mediates
// all access.
/// INV-SPIN-1: guard-gated access: &mut T exists only behind a live guard, the lock mediates every access (the impl's SAFETY note above).
unsafe impl<T: ?Sized + Send> Send for Spinlock<T> {}
/// INV-SPIN-1: guard-gated access: &mut T exists only behind a live guard, the lock mediates every access (the impl's SAFETY note above).
unsafe impl<T: ?Sized + Send> Sync for Spinlock<T> {}
// SAFETY: SpinlockGuard exists only while the lock is held, so the guarded
// data cannot be accessed concurrently from another thread.
/// INV-SPIN-1: guard-gated access: &mut T exists only behind a live guard, the lock mediates every access (the impl's SAFETY note above).
unsafe impl<T: ?Sized + Send> Send for SpinlockGuard<'_, T> {}
// SAFETY: SpinlockIrqGuard exists only while the lock is held, so the
// guarded data cannot be accessed concurrently.
/// INV-SPIN-1: guard-gated access: &mut T exists only behind a live guard, the lock mediates every access (the impl's SAFETY note above).
unsafe impl<T: ?Sized + Send> Send for SpinlockIrqGuard<'_, T> {}
// SAFETY: SpinlockBhGuard exists only while the lock is held, so the
// guarded data cannot be accessed concurrently.
/// INV-SPIN-1: guard-gated access: &mut T exists only behind a live guard, the lock mediates every access (the impl's SAFETY note above).
unsafe impl<T: ?Sized + Send> Send for SpinlockBhGuard<'_, T> {}
// SAFETY: RwSpinlock<T> mediates all access to the inner data: shared (&T) via
// read-locked guards, exclusive (&mut T) via write-locked guards.  Send allows
// moving between threads; Sync allows shared references across threads because
// the rwlock serialises access.
/// INV-SPIN-1: guard-gated access: &mut T exists only behind a live guard, the lock mediates every access (the impl's SAFETY note above).
unsafe impl<T: ?Sized + Send> Send for RwSpinlock<T> {}
/// INV-SPIN-1: guard-gated access: &mut T exists only behind a live guard, the lock mediates every access (the impl's SAFETY note above).
unsafe impl<T: ?Sized + Send + Sync> Sync for RwSpinlock<T> {}
// SAFETY: ReadGuard exists only while a read lock is held, preventing any
// writer from obtaining exclusive access to the data.
/// INV-RWLOCK-1: the rwlock family discipline: reader counts and the writer word pair acquire/release; guards ride the lock's own exclusivity.
unsafe impl<T: ?Sized + Sync> Send for RwSpinlockReadGuard<'_, T> {}
// SAFETY: WriteGuard exists only while the write lock is held exclusively,
// so no other thread can access the data.
/// INV-RWLOCK-1: the rwlock family discipline: reader counts and the writer word pair acquire/release; guards ride the lock's own exclusivity.
unsafe impl<T: ?Sized + Send> Send for RwSpinlockWriteGuard<'_, T> {}
// SAFETY: IrqReadGuard exists only while a read lock + IRQ disable is held,
// preventing any writer from accessing the data.
/// INV-RWLOCK-1: the rwlock family discipline: reader counts and the writer word pair acquire/release; guards ride the lock's own exclusivity.
unsafe impl<T: ?Sized + Sync> Send for RwSpinlockIrqReadGuard<'_, T> {}
// SAFETY: IrqWriteGuard exists only while the write lock + IRQ disable is
// held exclusively, so no other thread can access the data.
/// INV-RWLOCK-1: the rwlock family discipline: reader counts and the writer word pair acquire/release; guards ride the lock's own exclusivity.
unsafe impl<T: ?Sized + Send> Send for RwSpinlockIrqWriteGuard<'_, T> {}
// SAFETY: BhReadGuard exists only while a read lock + BH disable is held,
// preventing any writer from accessing the data.
/// INV-RWLOCK-1: the rwlock family discipline: reader counts and the writer word pair acquire/release; guards ride the lock's own exclusivity.
unsafe impl<T: ?Sized + Sync> Send for RwSpinlockBhReadGuard<'_, T> {}
// SAFETY: BhWriteGuard exists only while the write lock + BH disable is
// held exclusively, so no other thread can access the data.
/// INV-RWLOCK-1: the rwlock family discipline: reader counts and the writer word pair acquire/release; guards ride the lock's own exclusivity.
unsafe impl<T: ?Sized + Send> Send for RwSpinlockBhWriteGuard<'_, T> {}
// SAFETY: same contract as WaitQueueHead/SleepMutex — the raw task
// pointers inside the wait list are only ever passed to
// sched::wake_up_process (whose own guards handle stale targets);
// every list mutation is serialized by wait_lock; a queued waiter
// cannot exit (UNINTERRUPTIBLE), so entries never dangle.
/// INV-RWSEM-1: the rwsem discipline: guard existence proves the counted read or exclusive write phase.
unsafe impl Send for RwSemaphore {}
/// INV-RWSEM-1: the rwsem discipline: guard existence proves the counted read or exclusive write phase.
unsafe impl Sync for RwSemaphore {}
// SAFETY: Rwsem<T> mediates all access to the inner data: shared (&T)
// via read-locked guards, exclusive (&mut T) via write-locked guards.
/// INV-RWSEM-1: the rwsem discipline: guard existence proves the counted read or exclusive write phase.
unsafe impl<T: ?Sized + Send> Send for Rwsem<T> {}
/// INV-RWSEM-1: the rwsem discipline: guard existence proves the counted read or exclusive write phase.
unsafe impl<T: ?Sized + Send + Sync> Sync for Rwsem<T> {}
// SAFETY: guard exists only while the read lock is held, preventing
// any writer from obtaining exclusive access to the data.
/// INV-RWSEM-1: the rwsem discipline: guard existence proves the counted read or exclusive write phase.
unsafe impl<T: ?Sized + Sync> Send for RwsemReadGuard<'_, T> {}
// SAFETY: guard exists only while the write lock is held exclusively.
/// INV-RWSEM-1: the rwsem discipline: guard existence proves the counted read or exclusive write phase.
unsafe impl<T: ?Sized + Send> Send for RwsemWriteGuard<'_, T> {}
// Safety: writers have exclusive access; readers get a Copy snapshot.
/// INV-SEQLOCK-1: writers serialize on the sequence counter; readers snapshot under the even phase (T: Copy).
unsafe impl<T: Copy + Send> Send for SeqLock<T> {}
/// INV-SEQLOCK-1: writers serialize on the sequence counter; readers snapshot under the even phase (T: Copy).
unsafe impl<T: Copy + Send> Sync for SeqLock<T> {}
/// INV-SEQLOCK-1: writers serialize on the sequence counter; readers snapshot under the even phase (T: Copy).
unsafe impl<T: Copy + Send> Send for SeqLockWriteGuard<'_, T> {}
// SAFETY: see the struct comment — rows are cpu-private by
// construction of `with`.
/// INV-PERCPU-1: rows are cpu-private by construction of with/with_preempt (bounds + re-entry tripwire).
unsafe impl<T, const N: usize> Sync for CpuLocal<T, N> {}
// SAFETY: RawSpinlock's atomic protocol is the synchronization; the
// key is zero-sized and carries no data.
/// INV-LOCK-6: the atomic protocol of the inner RawSpinlock is the synchronization; the key carries no data.
unsafe impl<K: NestKey> Sync for KeyedRawSpinlock<K> {}
// SAFETY: same rule as Spinlock itself — the atomic protocol
// synchronizes; the key is zero-sized and carries no data.
/// INV-LOCK-6: the atomic protocol of the inner RawSpinlock is the synchronization; the key carries no data.
unsafe impl<K: NestKey, T: Send> Sync for KeyedSpinlock<K, T> {}

// ------------ per-CPU bank windows (INV-PERCPU-1) ------------

impl <T, const N: usize> CpuLocal<T, N> {
    /// Run `f` with this cpu's row, interrupts disabled for exactly
    /// the duration of `f`. Returns `None` — with interrupts restored
    /// — when the acting cpu id is out of range for this bank (the
    /// r8final divergence family; callers keep their locked fallback)
    /// or on a refused same-slot re-entry.
    /// INV-PERCPU-1: the D-7 window — bounds-checked cpu id, IF=0 exclusivity, in_use re-entry tripwire; the row is handed out only through the guard.
    #[inline]
    pub fn with<R>(&self, f: impl FnOnce(CpuLocalRef<'_, T>) -> R) -> Option<R> {
        let flags = crate::arch::cpu::save_and_disable_irq();
        let cpu = crate::arch::cpu_id() as usize;
        if cpu >= N {
            crate::arch::cpu::restore_irq(flags);
            return None;
        }
        let rows = self.rows.get(); // *mut [Row<T>; N], interior-mutable
        // SAFETY (pointer arithmetic and both field pointers): `cpu`
        // is bounds-checked above; the row is private to this cpu for
        // as long as interrupts stay disabled (they are — the guard
        // restores at drop); the in_use flag read/set below refuses a
        // second live guard, so the &mut handed out is exclusive. All
        // access goes through raw pointers derived from the UnsafeCell
        // — never through a `&`-path (the D-7 incident family).
        let row = unsafe { rows.cast::<Row<T>>().add(cpu) }; // *mut Row<T>
        let (val, in_use): (*mut T, *mut bool) = unsafe {
            (
                core::ptr::addr_of_mut!((*row).val),
                core::ptr::addr_of_mut!((*row).in_use),
            )
        };
        if unsafe { *in_use } {
            // Same slot, same cpu, a guard is already live (the outer
            // window still holds — flags were saved with IRQs already
            // off and false restores nothing). Refuse the alias,
            // count + taint: this is a call-site bug of the r8final
            // class, and the taint names it instead of absorbing it.
            REENTRY_EVENTS.fetch_add(1, Ordering::Relaxed);
            crate::debug::hotpath::add_taint(crate::debug::types::TaintFlags::BAD_PAGE);
            crate::arch::cpu::restore_irq(flags);
            return None;
        }
        unsafe { *in_use = true };
        let guard = CpuLocalRef {
            val: unsafe { &mut *val },
            cpu,
            flags,
            preempt_guard: false,
            in_use,
            _nosend: PhantomData,
        };
        Some(f(guard))
    }
}
impl <T, const N: usize> CpuLocal<T, N> {
    /// Run `f` with this cpu's row, PREEMPTION disabled but INTERRUPTS
    /// LEFT ENABLED, for exactly the duration of `f` (106th, the
    /// allocator irq-toggle restructure).
    ///
    /// The exclusivity this guard provides is strictly weaker than
    /// [`CpuLocal::with`]: it excludes a context switch (so the cpu id
    /// read inside cannot go stale — no migration between the read and
    /// the row access) but it does NOT exclude interrupt context on the
    /// same cpu. It is therefore only correct for banks whose IRQ-context
    /// access is ROUTED AWAY from the rows by the caller — the small_alloc
    /// magazines: interrupt-context allocs/frees take the class-lock path
    /// directly and never touch a row, so a task-context row access can be
    /// interrupted but never raced. `with_preempt` returns `None` (nothing
    /// touched) whenever the caller is NOT in task context
    /// (`in_interrupt()` — hardirq/softirq/NMI) or on the same
    /// out-of-range/re-entry refusals as `with`; callers keep their locked
    /// fallback for every `None`.
    ///
    /// Why this exists: `with` pays a cli/sti pair per access. On the
    /// allocation fast paths that is ~46M IF=0 windows per boot, the
    /// second-largest surviving interrupt-off shadow (the deferred-PMI
    /// `enable_irq+0x1d` sample class). A preempt-disable window keeps
    /// the tick and device IRQs flowing through the access.
    /// INV-PERCPU-1: the preempt-flavored window — task-context check, preempt-off exclusivity, IRQ-context access routed away by the callers.
    pub fn with_preempt<R>(&self, f: impl FnOnce(CpuLocalRef<'_, T>) -> R) -> Option<R> {
        if crate::interrupt::preempt::in_interrupt() {
            return None;
        }
        crate::interrupt::preempt::preempt_count_add(crate::interrupt::preempt::PREEMPT_OFFSET);
        let cpu = crate::arch::cpu_id() as usize;
        if cpu >= N {
            crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::PREEMPT_OFFSET);
            return None;
        }
        let rows = self.rows.get(); // *mut [Row<T>; N], interior-mutable
        // SAFETY: `cpu` is bounds-checked above; the row is private to
        // this cpu for as long as preemption stays disabled (no context
        // switch can migrate the task between the cpu_id read and the row
        // access — the r8final divergence family); interrupt context is
        // excluded by the caller-side routing documented on this method.
        // The in_use flag read/set refuses a second live guard.
        let row = unsafe { rows.cast::<Row<T>>().add(cpu) }; // *mut Row<T>
        let (val, in_use): (*mut T, *mut bool) = unsafe {
            (
                core::ptr::addr_of_mut!((*row).val),
                core::ptr::addr_of_mut!((*row).in_use),
            )
        };
        if unsafe { *in_use } {
            // Same refusal semantics as `with` (see there).
            REENTRY_EVENTS.fetch_add(1, Ordering::Relaxed);
            crate::debug::hotpath::add_taint(crate::debug::types::TaintFlags::BAD_PAGE);
            crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::PREEMPT_OFFSET);
            return None;
        }
        unsafe { *in_use = true };
        let guard = CpuLocalRef {
            val: unsafe { &mut *val },
            cpu,
            flags: false,
            preempt_guard: true,
            in_use,
            _nosend: PhantomData,
        };
        Some(f(guard))
    }
}
/// Release a per-CPU row's in_use latch (CpuLocalRef::drop,
/// collected by pass 3): the raw store is the only writer.
///
/// # Contract (INV-PERCPU-1)
/// same-cpu exclusivity still holds at drop (IF=0 or
/// preempt-off window intact until the restore below the
/// call); the guard owns `in_use` exclusively (INV-PERCPU-1).
#[inline(always)]
pub(crate) fn cpu_local_row_release(in_use: *mut bool) {
        unsafe { *in_use = false };
}


// ------------ rcu callback edges (INV-RCU-1) ------------

/// Queue `head` on cpu's callback list (collected by pass 3):
/// the intrusive add_tail under the cb_list lock.
///
/// # Contract (INV-RCU-1)
/// the cb_list lock is held for `cpu` (call_rcu's
/// lock_cb_list/unlock_cb_list bracket); head.list was
/// initialized via init() (the RcuHead protocol).
#[inline(always)]
pub(crate) fn rcu_link_tail(cpu: usize, head: &mut RcuHead) {
    unsafe {
        head.list.add_tail(&mut RCU_CBS[cpu]);
    }
}

/// Detach cpu's callback list into `batch` (collected by pass 3).
///
/// # Contract (INV-RCU-1)
/// the cb_list lock is held for `cpu` (the softirq handler's
/// bracket); RCU_CBS[cpu] is exclusively ours and was
/// initialized in init().
#[inline(always)]
pub(crate) fn rcu_splice_cbs(cpu: usize, batch: &mut ListHead) {
        unsafe {
            if !RCU_CBS[cpu].is_empty() {
                // Splice the list into `batch`.
                batch.next = RCU_CBS[cpu].next;
                batch.prev = RCU_CBS[cpu].prev;
                (*batch.next).prev = &mut *batch;
                (*batch.prev).next = &mut *batch;
                RCU_CBS[cpu].init();
            }
        }
}

/// Invoke the detached callbacks (collected by pass 3).
///
/// # Contract (INV-RCU-1)
/// `batch` was detached by rcu_splice_cbs: the list is
/// exclusively owned, each RcuHead registered via call_rcu
/// with a valid func pointer, alive until the callback runs.
#[inline(always)]
pub(crate) fn rcu_run_cbs(batch: &mut ListHead) {
    unsafe {
        let sentinel = &mut *batch as *mut ListHead;
        let mut node = batch.next;
        while node != sentinel {
            let next = (*node).next;
            let head = node as *mut RcuHead;
            let func = (*head).func;
            func(head as *mut core::ffi::c_void);
            node = next;
        }
    }
}

/// Boot-time init of cpu i's callback row (collected by pass 3).
///
/// # Contract (INV-RCU-1)
/// runs once during boot before any CPU uses RCU — no
/// concurrent access to RCU_CBS[i] (INV-RCU-1).
#[inline(always)]
pub(crate) fn rcu_cbs_row_init(i: usize) {
        unsafe { RCU_CBS[i].init(); }
}


// ------------ rcu grace-period wait (INV-RCU-1) ------------

/// The sleeping poll loop shared by the sleepy wait forms: check first
/// (fresh reporters pass without ever sleeping), then one-jiffy
/// INTERRUPTIBLE sleeps armed by a one-shot timer (the khungtaskd
/// idiom), until every online CPU reports at/after `gen` or the budget
/// expires. Returns whether the round completed.
/// INV-RCU-1: the sleepy wait's own-task derefs (state transitions, pid reads, stack scan) run on the waiter's own current pointer; the diagnostic reads are bounded volatile reads.
pub(crate) fn wait_qs_sleepy(gen: u32, max_ms: u64, site: &'static str) -> bool {
    use crate::process::types::TaskState;

    let hz = crate::drivers::hotpath::timer::HZ as u64;
    let deadline = crate::timer::get_jiffies() + max_ms * hz / 1000;

    loop {
        if all_online_cpus_reported(gen) {
            shared_grace_done();
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            GP_CONSEC_FAIL.store(0, Ordering::Relaxed);
            return true;
        }
        if crate::timer::get_jiffies() >= deadline {
            shared_grace_done();
            RCU_WAITERS.fetch_sub(1, Ordering::AcqRel);
            static SGP_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = SGP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // Print budget 4 (was 200): each report is a long IF=0
            // polled-UART line; under the close storm the reports
            // themselves pinned the console while the peer CPU's PID 1
            // sat blocked in write(1) — manufacturing the very "stuck
            // CPU" they were diagnosing (the bw_file_rd livelock
            // anatomy, 2026-11-19). Four lines name the class; the
            // rest stay silent.
            if n < 4 {
                // rwlock-livelock: the old print showed only qs[0..4] —
                // on the SMP8 ub2404 rig the non-quiescent CPU was
                // invisible (qs0-3 all == gen while the round failed),
                // so the monopolist was unnameable. Show all MAX_CPUS
                // (gen|on|idle triplets) and the first stuck CPU's
                // slot-current task inline (same fields the
                // frozen-CPU truth dump reads; no IPI here).
                let mut qs_desc = alloc::string::String::new();
                let mut stuck = usize::MAX;
                for i in 0..MAX_CPUS {
                    let on = crate::sched::hotpath::cpu_online(i);
                    let idle = crate::sched::hotpath::cpu_running_idle(i);
                    let q = RCU_QS_GEN[i].load(Ordering::Acquire);
                    use core::fmt::Write as _;
                    let _ = write!(
                        qs_desc,
                        "{}:{:x}/{}/{},",
                        i, q, on as u8, idle as u8
                    );
                    if on && stuck == usize::MAX && !cpu_quiescent(i, gen) {
                        stuck = i;
                    }
                }
                let mut victim = alloc::string::String::new();
                if stuck != usize::MAX {
                    let cur = crate::sched::hotpath::grq_diag_cpu_currents();
                    let t = cur[stuck].2 as *mut crate::process::types::Task;
                    if !t.is_null() {
                        // SAFETY: diagnostic read of stable fields on the
                        // per-CPU current Task (idle/static or heap; both
                        // stay valid while current on that CPU).
                        let tk = unsafe { &*t };
                        use core::fmt::Write as _;
                        let cb = tk.comm();
                        let cl = cb.iter().position(|&b| b == 0).unwrap_or(16);
                        let name = core::str::from_utf8(&cb[..cl]).unwrap_or("?");
                        let _ = write!(
                            victim,
                            " stuck-cpu{}-cur pid={} tgid={} {} preempt={:#x}",
                            stuck,
                            tk.pid(),
                            tk.tgid(),
                            name,
                            tk.preempt_count()
                        );
                        // rwlock-livelock: the victim's live kernel frames
                        // sit BELOW the sp saved at its last switch-in
                        // (same technique as the FROZEN-CPU IF=0 fallback,
                        // but at STALL TIME — mid-episode; the one-shot
                        // IPI dump kept arriving after rolling monopolies
                        // had already drained). Names the leaking loop.
                        let sp = tk.thread().sp;
                        if (sp >> 48) == 0xffff && (sp & 7) == 0 && sp > 0x1000 {
                            let base = sp as *const u64;
                            let mut shown = 0usize;
                            // SAFETY: diagnostic read within the current
                            // task's own kernel stack pages (mapped while
                            // the Task exists); races cost at worst a
                            // torn-but-bounded word.
                            for k in 0..96 {
                                let v = unsafe {
                                    core::ptr::read_volatile(base.wrapping_sub(k))
                                };
                                #[cfg(feature = "x86_64")]
                                let (lo, hi) = (0xffff_ffff_8020_0000u64, 0xffff_ffff_8200_0000u64);
                                #[cfg(not(feature = "x86_64"))]
                                let (lo, hi) = (0xffff_ffff_8000_0000u64, 0xffff_ffff_8020_0000u64);
                                if v >= lo && v < hi && (v & 1) == 0 {
                                    let _ = write!(victim, " {v:#x}");
                                    shown += 1;
                                    if shown >= 10 {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                crate::pr_warn!(
                    "RCU-GP-BOUND site={} waited>{}ms (sleepy): no full quiescent round gen={} per-cpu(cpu:gen/on/idle,...)={}:{}",
                    site,
                    max_ms,
                    gen,
                    qs_desc,
                    victim,
                );
            }
            // Comprehensive-fix residual hunt (2026-11-04, cgf1): a CPU
            // frozen non-quiescent across MANY consecutive grace budgets
            // is the wedge-family monopolist (IF=0/preempt-off holder —
            // its own tick cannot fire, so every tick-side instrument is
            // blind to it; the 13th/25th cross-CPU-dump prescription).
            // This waiter runs on ANOTHER CPU in task (syscall) context,
            // so it can walk the frozen CPU's tasks.
            //
            // 52nd session (oh-crash): the trigger bar moved 32 -> 4
            // consecutive failed rounds. The ohc10-19 frozen face took
            // 32 rounds to arm because grace rounds kept passing BETWEEN
            // rolling monopolies; ohc20 proved the pandemic forms with
            // NO terminal freeze at all — rolling preempt/softirq-leak
            // carriers (PREEMPT-LEAK pc=0x10001/0x100, LEAK-ORIGIN
            // slab.rs/unix.rs sites) starve the box while each monopoly
            // lasts only seconds. Four failed rounds (~200ms of a stuck
            // CPU at the 50ms fdreap budget) catches a carrier
            // mid-monopoly; a healthy boot never fails even one sleepy
            // round. One-shot per boot (GP_DUMPED), same latch as the
            // full dump below.
            let consec = GP_CONSEC_FAIL.fetch_add(1, Ordering::Relaxed) + 1;
            // rwlock-livelock: consec==2 added — the ub2404 boot showed
            // rolling monopolies where successes reset the counter
            // before 4 accumulated, leaving the monopolist unnameable.
            if (consec == 2 || consec == 4 || consec == 32)
                && !GP_DUMPED.swap(true, Ordering::AcqRel)
            {
                // FIRST name the stuck CPU's live producer — per-CPU
                // tick counts + last tick-entry RIP, then an IPI probe
                // whose callback answers with the percpu canary (gs cpu
                // id + magic vs LAPIC id) and the interrupted RIP + live
                // stack of whatever runs there (or stays silent =
                // IRQ-off monopolist) — BEFORE the full task dump
                // floods the log.
                let mut stuck = usize::MAX;
                for i in 0..MAX_CPUS {
                    if crate::sched::hotpath::cpu_online(i) && !cpu_quiescent(i, gen) {
                        stuck = i;
                        break;
                    }
                }
                if stuck != usize::MAX {
                    crate::debug::hotpath::dump_frozen_cpu_truth(stuck);
                }
                crate::debug::hotpath::dump_all_tasks("rcu-gp-frozen-cpu");
            }
            return false;
        }

        let current = match crate::sched::hotpath::current() {
            Some(t) => t,
            None => {
                // Early-boot / no-task context: no sleep possible —
                // plain yield rounds keep the wait alive.
                crate::sched::hotpath::schedule();
                continue;
            }
        };
        // SAFETY: current is this task's own pointer.
        let my_pid = unsafe { (*current).pid() };
        // r16 (mechanism B — the GB lost-wake-mid-exit, re-landed): the
        // one-shot wake below is keyed by PID, and a group member's
        // do_exit used to free its pid BEFORE its opportunistic sweep, so
        // this sleepy wait ran with a pid already returned to the
        // allocator. The expiry then resolved the pid to nothing
        // (WAKE-PID-MISS — silently dropped) or to a REUSED task — either
        // way this waiter parked at the schedule() below and the deadline
        // line under it never ran, because the bound is only re-checked
        // on wake: the "bounded" wait became permanent (GB worker
        // 312/313, both wedged mid-exit; the drain's rescue wake,
        // addressed by task pointer, is what finally moved them). The
        // exit path now frees the pid only AFTER the sweep, but keep the
        // independent guard here: validate the pid->task mapping before
        // arming, and when it is broken (any future pid-then-sleep
        // order), fall back to a yield-poll that enforces its own
        // deadline and never depends on a wake token. Rate-limited
        // report: every firing names a producer of the order.
        let pid_resolves = {
            let t = crate::process::hotpath::pid_hash_lookup_pinned(my_pid);
            if t.is_null() {
                false
            } else {
                let ok = t as *const crate::process::types::Task
                    == current as *const crate::process::types::Task;
                // Release the lookup pin in both cases (unmatched or
                // matched): the pinned refcount is the lookup's own.
                crate::process::types::Task::task_put(t);
                ok
            }
        };
        if !pid_resolves {
            static PIDFREE_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if PIDFREE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 16 {
                crate::pr_warn!(
                    "RCU-SLEEPY-PID-FREE pid={} site={} — yield-poll round (pid-keyed timer wake would be lost)",
                    my_pid,
                    site
                );
            }
            // Cooperative yield round: state stays RUNNING (no park, no
            // wake token), the schedule() itself reports this CPU's QS
            // and lets peers run; the loop top re-checks completion and
            // the deadline on every return.
            crate::arch::cpu::restore_irq(true);
            crate::sched::hotpath::schedule();
            continue;
        }
        let target = crate::timer::get_jiffies() + 1;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);
        if timer_id != 0 {
            // SAFETY: current is this task's own pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
            }
            // Re-check QS AFTER marking sleeping: the last missing
            // reporter typically reports in the microseconds between
            // the failed check at the loop top and here (the fd-close
            // completion loop yields through schedule(), which is a
            // report site). Aborting the sleep when it lands converts
            // what was otherwise a full 1-jiffy stall — measured
            // 2026-11-05: one close(2) per 10 ms tick stalled up to
            // 10 ms in fdreapd's sleepy wait (probe: open+close median
            // 1.07 us but mean 2.45 us, every stall ending at a jiffy
            // boundary) — back into the microsecond poll window.
            if all_online_cpus_reported(gen) {
                // SAFETY: current is this task's own pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    // NEW-C2: a racing wake may have enqueued us while
                    // still executing — take ourselves back off.
                    crate::sched::hotpath::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
                continue;
            }
            // Re-check the one-shot AFTER marking sleeping: it may have
            // fired between arming and set_state (khungtaskd lesson).
            if crate::timer::get_jiffies() >= target {
                // SAFETY: current is this task's own pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    // NEW-C2: a racing wake may have enqueued us while
                    // still executing — take ourselves back off.
                    crate::sched::hotpath::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
            } else {
                crate::arch::cpu::restore_irq(true);
                crate::sched::hotpath::schedule();
                // Spurious wake before the deadline: drop the timer and
                // loop (re-armed next iteration).
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            // Timer table full — stay runnable, yield one round.
            crate::sched::hotpath::schedule();
        }
    }
}

// ------------ unsafe accessor fns (unsafe fn relocations) ------------

impl RawSpinlock {
    /// INV-SPIN-1: reset releases the word without the debug ownership check; caller must provably own the lock's state.
    #[inline]
    pub unsafe fn reset(&mut self) {
        self.clear_owner();
        self.locked.store(0, Ordering::Release);
    }
}
impl <T> Spinlock<T> {
    /// Get a shared reference to the inner data without locking.
    ///
    /// # Safety
    /// Caller must ensure no concurrent mutable access (e.g., data is
    /// write-once during boot and only read thereafter).
    /// INV-SPIN-1: shared view without locking; caller must prove no concurrent mutable access (write-once boot data).
    #[inline]
    pub unsafe fn get_ref(&self) -> &T {
        &*self.data.get()
    }
}
impl <T> Spinlock<T> {
    /// Get a mutable reference to the inner data without locking.
    ///
    /// # Safety
    /// Caller must ensure exclusive access (e.g., no concurrent readers or writers).
    /// INV-SPIN-1: mutable view without locking; caller must hold exclusive access.
    #[inline]
    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        &mut *self.data.get()
    }
}
impl <T> Spinlock<T> {
    /// Consume the lock and return the inner data.
    ///
    /// # Safety
    /// Caller must ensure no other thread holds a reference to the lock or data.
    /// INV-SPIN-1: consumes the lock; caller must prove no other reference to lock or data.
    #[inline]
    pub unsafe fn into_inner(self) -> T { self.data.into_inner() }
}
impl <T> RwSpinlock<T> {
    /// Consume the lock and return the inner data.
    ///
    /// # Safety
    /// Caller must ensure no other thread holds a reference to the lock or data.
    /// INV-RWLOCK-1: consumes the lock; caller must prove no other reference to lock or data.
    #[inline]
    pub unsafe fn into_inner(self) -> T {
        self.data.into_inner()
    }
}
impl <T: Copy> SeqLock<T> {
    /// Read without disabling preemption.
    ///
    /// # Safety
    /// Caller must hold preemption disabled for the duration of this call.
    /// INV-SEQLOCK-1: the caller's disabled preemption is the read-window contract (read_inner checks the sequence).
    #[inline]
    pub unsafe fn read_preempt_disabled(&self) -> T {
        self.read_inner()
    }
}
impl <T: Copy> SeqLock<T> {
    /// Core read loop (no preemption management).
    /// INV-SEQLOCK-1: the volatile snapshot under the even-sequence validation (preemption disabled by the callers read()/read_preempt_disabled).
    #[inline]
    pub(crate) fn read_inner(&self) -> T {
        loop {
            let seq = self.raw.read_begin();
            if seq & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            // SAFETY: preemption is disabled and sequence is even (no writer).
            let snapshot = unsafe { core::ptr::read_volatile(self.data.get()) };
            if self.raw.read_retry(seq) {
                core::hint::spin_loop();
                continue;
            }
            return snapshot;
        }
    }
}

// ------------ spinlock deadlock diagnostics (INV-SPIN-1) ------------

impl RawSpinlock {
    /// Print deadlock warning via SBI (works even with interrupts disabled).
    /// INV-SPIN-1: cold diagnostic reads of a lock the reporter SPINS ON (locked_word/owner/current-task); every deref is bounded by the live-word protocol.
    pub(crate) fn deadlock_warn(lock_addr: *const Self, caller_ra: usize) {
        // Raw UART output — printk might need locks we're spinning on.
        // uart_putc_raw, NOT putchar_no_lock: the latter is compiled out
        // of release builds, so this warn printed NOTHING on every
        // production-config boot (the round-2 monopolist spun 32 s in
        // kill() with zero DEADLOCK lines for exactly this reason).
        // Once per (lock, site) per boot — see the call site.
        {
            static SEEN: [core::sync::atomic::AtomicUsize; 64] =
                [const { core::sync::atomic::AtomicUsize::new(0) }; 64];
            let key = (lock_addr as usize >> 4) ^ (caller_ra << 8);
            let slot = &SEEN[key % 64];
            if slot.load(Ordering::Relaxed) == key {
                return;
            }
            slot.store(key, Ordering::Relaxed);
        }
        let cpu = crate::arch::smp::cpu_id();
        let msg = b"DEADLOCK: spinlock stuck cpu=";
        for &b in msg {
            crate::console::uart_putc_raw(b);
        }
        // Print CPU id as decimal digit
        if cpu < 10 {
            crate::console::uart_putc_raw(b'0' + cpu as u8);
        }
        // Print lock address in hex
        let msg2 = b" lock=0x";
        for &b in msg2 {
            crate::console::uart_putc_raw(b);
        }
        let addr = lock_addr as usize;
        let mut shift = (core::mem::size_of::<usize>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (addr >> (shift as usize)) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            crate::console::uart_putc_raw(c);
        }

        // Print caller return address (ra) for debugging
        let msg3 = b" ra=0x";
        for &b in msg3 {
            crate::console::uart_putc_raw(b);
        }
        let mut shift = (core::mem::size_of::<usize>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (caller_ra >> shift) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            crate::console::uart_putc_raw(c);
        }

        // R11 (deadlock-layer unblinding, the seventh blind printer): the
        // lock WORD value, always. The holder-identity block below is
        // debug-lock-owner-gated, so on standard builds every stuck lock
        // printed with NO holder datum at all — and the one value that
        // discriminates "a real holder the tracker missed" (word == 1)
        // from "the word itself was scribbled" (word ∉ {0,1}) was never
        // printed. One relaxed load, cold path, raw uart.
        let msg_lw = b" locked=0x";
        for &b in msg_lw {
            crate::console::uart_putc_raw(b);
        }
        let lw = unsafe { (&*lock_addr).locked_word() };
        let mut shift_lw: i32 = 8;
        while shift_lw > 0 {
            shift_lw -= 4;
            let nibble = ((lw >> shift_lw) & 0xF) as u8;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            crate::console::uart_putc_raw(c);
        }

        // Holder identity: owner = hart+1 (0 = free / mid-handoff).
        // oh-crash 91st extension (the meltdown holder hunt): the owner
        // CPU's current task (pid/comm), the heap allocation CONTAINING
        // the lock (small-object span + first words for offline
        // symbolization of any ops/vtable pointers), and one REMOTE-BT
        // of the holder per unique lock address per boot — the IPI
        // handler on the holder captures its live interrupted stack.
        // grq-lock-1st: the GRQ crime scene. A GRQ holder runs with IRQs
        // OFF, so the REMOTE-BT capture below (an IPI) can never answer
        // on a GRQ wedge — that is why every GRQ wedge so far ended with
        // the holder UNNAMED. The GRQ site stamp is published BY the
        // holder, so this dump reads it without any interrupt.
        crate::sched::hotpath::grq_dump_holder("deadlock");

        #[cfg(feature = "debug-lock-owner")]
        {
            let msg4 = b" holder=";
            for &b in msg4 {
                crate::console::uart_putc_raw(b);
            }
            // SAFETY: lock_addr is a valid RawSpinlock pointer (from lock()).
            let holder = unsafe { (&*lock_addr).owner.load(Ordering::Relaxed) };
            let mut holder_cpu: Option<usize> = None;
            if holder == 0 {
                let msg5 = b"switching";
                for &b in msg5 {
                    crate::console::uart_putc_raw(b);
                }
            } else {
                let h = holder - 1;
                holder_cpu = Some(h);
                if h < 10 {
                    crate::console::uart_putc_raw(b'0' + h as u8);
                }
                // Holder CPU's current task: pid + comm (best-effort —
                // an irqsave holder cannot migrate, so this is the task
                // that acquired; plain lock() holders may have been
                // preempted, in which case the remote-BT below shows
                // what the CPU runs NOW and the pid names the victim).
                // SAFETY: cpu_state(h).current is the scheduler's own
                // published pointer; read-only access.
                let cur = unsafe { crate::sched::hotpath::cpu_state(h).current };
                if !cur.is_null() {
                    // SAFETY: read-only access to a live Task.
                    let pid = unsafe { (*cur).pid() };
                    let msg_pid = b" task=";
                    for &b in msg_pid {
                        crate::console::uart_putc_raw(b);
                    }
                    Self::putdec(pid as u64);
                    // SAFETY: comm is a fixed [u8;16], no allocation.
                    let comm = unsafe { (*cur).comm() };
                    crate::console::uart_putc_raw(b'(');
                    let mut any = false;
                    for &b in comm.iter() {
                        if b == 0 {
                            break;
                        }
                        if (0x20..0x7f).contains(&b) {
                            crate::console::uart_putc_raw(b);
                            any = true;
                        }
                    }
                    if !any {
                        crate::console::uart_putc_raw(b'?');
                    }
                    unsafe { crate::console::uart_putc_raw(b')'); }
                }
            }

        crate::console::uart_putc_raw(b'\n');

        // Leak forensics (debug-lock-owner): the LAST acquirer's pid,
        // call site and acquire age — these persist past a cleared
        // owner word, so a leaked lock names who took it and when
        // even when the owner stamp is mid-handoff. Plus the raw
        // locked word (1 = genuinely held; anything else is a torn
        // or corrupted state worth seeing).
        #[cfg(feature = "debug-lock-owner")]
        {
            // SAFETY: lock_addr is a valid RawSpinlock pointer (from lock()).
            let l = unsafe { &*lock_addr };
            let apid = l.acq_pid.load(Ordering::Relaxed);
            let asite = l.acq_site.load(Ordering::Relaxed);
            let ajif = l.acq_jiffies.load(Ordering::Relaxed);
            let wv = l.locked.load(Ordering::Relaxed);
            let now = crate::timer::get_jiffies();
            let msg = b"LOCK-ACQ: word=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::puthex(wv as u64);
            let msg = b" pid=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::putdec(apid as u64);
            let msg = b" site=0x";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::puthex(asite as u64);
            let msg = b" age=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::putdec(now.wrapping_sub(ajif));
            let msg = b"j\n";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
        }

        // Containing heap object (works with or without a known holder):
        // slab span + the first 6 words of the object. Any word landing
        // in kernel text (0xffffffff8...) symbolizes offline against
        // gen-ksyms output — that names the lock's OWNER STRUCT (ops
        // table / vtable / function pointer members).
        if let Some((base, sz)) = crate::mm::hotpath::object_span(lock_addr as usize) {
            let msg = b"LOCK-OBJ: small kmalloc-";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::putdec(sz as u64);
            let msg = b" base=0x";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::puthex(base as u64);
            let msg = b" words=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            for i in 0..6 {
                // SAFETY: diagnostic read of the object's first words;
                // the span was validated by object_span (inside a live
                // slab page, in-bounds of the object's slot).
                let w = unsafe { ((base + i * 8) as *const u64).read_unaligned() };
                Self::puthex(w);
                if i < 5 {
                    crate::console::uart_putc_raw(b',');
                }
            }
            crate::console::uart_putc_raw(b'\n');
        } else if let Some((base, order, free)) =
            crate::mm::hotpath::GLOBAL_ALLOCATOR.buddy_block_span(lock_addr as usize)
        {
            // oh-crash 98th: DIRECT BUDDY blocks (>1KB kmalloc — Task
            // structs, large fs/ipc objects). Same offline-symbolization
            // trick on the first words; `free!=0` (a lock inside a FREE
            // block) is itself evidence of a freed-with-lock-reference
            // object, printed as words=FREE.
            let msg = b"LOCK-OBJ: buddy order-";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::putdec(order as u64);
            let msg = b" base=0x";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::puthex(base as u64);
            let msg = b" free=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            Self::putdec(free as u64);
            let msg = b" words=";
            for &b in msg {
                crate::console::uart_putc_raw(b);
            }
            for i in 0..6 {
                // SAFETY: diagnostic read of the block's first words;
                // buddy_block_span validated the address against the
                // heap window and the block's base alignment.
                let w = unsafe { ((base + i * 8) as *const u64).read_unaligned() };
                Self::puthex(w);
                if i < 5 {
                    crate::console::uart_putc_raw(b',');
                }
            }
            crate::console::uart_putc_raw(b'\n');
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
        }
        #[cfg(not(feature = "debug-lock-owner"))]
        {
            crate::console::uart_putc_raw(b'\n');

            // The heap-object span dump is cheap and holder-independent:
            // keep it on default builds too (the meltdown hunts run
            // stock feature sets as controls).
            if let Some((base, sz)) = crate::mm::hotpath::object_span(lock_addr as usize) {
                let msg = b"LOCK-OBJ: small kmalloc-";
                for &b in msg {
                    crate::console::uart_putc_raw(b);
                }
                Self::putdec(sz as u64);
                let msg = b" base=0x";
                for &b in msg {
                    crate::console::uart_putc_raw(b);
                }
                Self::puthex(base as u64);
                crate::console::uart_putc_raw(b'\n');
            } else if let Some((base, order, free)) =
                crate::mm::hotpath::GLOBAL_ALLOCATOR.buddy_block_span(lock_addr as usize)
            {
                let msg = b"LOCK-OBJ: buddy order-";
                for &b in msg {
                    crate::console::uart_putc_raw(b);
                }
                Self::putdec(order as u64);
                let msg = b" base=0x";
                for &b in msg {
                    crate::console::uart_putc_raw(b);
                }
                Self::puthex(base as u64);
                let msg = b" free=";
                for &b in msg {
                    crate::console::uart_putc_raw(b);
                }
                Self::putdec(free as u64);
                crate::console::uart_putc_raw(b'\n');
            }
        }

        // rux:watchdog (retired debug=watchdog) (debug-migrate: rux:taskdump kind=WATCHDOG):
        // follow the warning with a full task-state snapshot
        // (silent-wedge diagnosis — CPUs idle + sleepers never woken).
        if crate::trace::EVENT_TASKDUMP.armed() {
            crate::trace::trace_taskdump(2, 0);
            crate::debug::hotpath::dump_all_tasks("deadlock-watchdog");
        }
    }
}
impl RawSpinlock {
    /// Lockless hex print (deadlock diagnostics — putchar path only).
    /// INV-SPIN-1: lockless diagnostic putchar (deadlock dumps run with locks wedged).
    pub(crate) fn puthex(v: u64) {
        let mut shift: i32 = 64;
        while shift > 0 {
            shift -= 4;
            let nibble = ((v >> shift) & 0xF) as u8;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            unsafe { crate::console::putchar_no_lock(c); }
        }
    }
}
impl RawSpinlock {
    /// Lockless decimal print (deadlock diagnostics).
    /// INV-SPIN-1: lockless diagnostic putchar (deadlock dumps run with locks wedged).
    pub(crate) fn putdec(mut v: u64) {
        let mut buf = [0u8; 20];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (v % 10) as u8;
            i += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while i > 0 {
            i -= 1;
            unsafe { crate::console::putchar_no_lock(buf[i]); }
        }
    }
}

// ------------ arch asm reads (caller return address) ------------

/// Capture this function's return address (the spinlock call site) for
/// deadlock diagnostics.
/// INV-SPIN-1: pure register/frame reads (nomem, nostack) for the deadlock call-site stamp; no memory access.
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

// ------------ sleep-mutex handoff edges (INV-LOCK-6) ------------

/// The woken handoff waiter's pid read (collected by pass 3):
/// one deref of the wake queue's task pointer.
///
/// # Contract (INV-LOCK-6)
/// `wtask` is the *mut Task just handed out by
/// wake_up_one_hiprio (still owned by the waiter entry);
/// the pid read is the handoff's own field read (INV-LOCK-6).
#[inline(always)]
pub(crate) fn wake_task_pid(wtask: *mut crate::process::types::Task) -> u32 {
unsafe { (*(wtask as *mut crate::process::types::Task)).pid() as u32 }
}


// ------------ sleep-mutex owner checks (INV-LOCK-6) ------------

/// R77-style stall tripwire support: is the recorded owner still a live
/// task? (pid-hash lookup; the stall tripwire only fires while the owner
/// lives — a DEAD owner means the lock leaked with its holder unless
/// release_if_owner ran.)
/// INV-LOCK-6: read-only state() probe of the pinned owner task (the R77 stall tripwire); the pin keeps the Task alive across the read.
pub(crate) fn locked_owner_alive_wait(owner: &u32) -> bool {
    if *owner == 0 {
        return false;
    }
    let t = crate::process::hotpath::pid_hash_lookup_pinned(*owner);
    if t.is_null() {
        return false;
    }
    let alive = unsafe { !(*t).state().is_dead() };
    crate::process::types::Task::task_put(t);
    alive
}
impl SleepMutex {
    /// Optimistic-spin probe (one state-lock section + owner on-CPU
    /// check). Reference discipline: mutex_optimistic_spin only spins
    /// while the OWNER IS RUNNING on a CPU (mutex_spin_on_owner reads
    /// owner->on_cpu); the moment the holder is descheduled the
    /// reference stops spinning and queues. Spinning against an
    /// off-CPU holder is pure waste — and worse than waste under a
    /// convoy: on a 2-CPU box with 5-6 contenders on VFS_MUTATION_LOCK
    /// / EXT4_BIG_LOCK (ftest03 shape), spinners burning their whole
    /// optimistic budget per round starve the very holder whose
    /// critical section gates everyone (measured: 10 probe iterations
    /// 14s -> ~95s, 7x, LTP ftest03 census FAIL via the 30s budget
    /// TERM -> child TBROK unlink exits).
    ///
    /// Outcomes:
    /// - `Acquired`: the fair try succeeded (free+unwaited, or
    ///   recursion) — same gates as try_acquire_or_recurse_fair.
    /// - `Spin`: lock held by ANOTHER task that is on-CPU right now —
    ///   optimistic spinning is profitable, keep going.
    /// - `Stop`: stop spinning and queue — holder off-CPU (or its task
    ///   is gone), or the lock is free behind counted waiters (the R84
    ///   handoff window: a fresh caller must not take it, so there is
    ///   nothing to spin FOR).
    /// INV-LOCK-6: one locked fair-try section plus the pinned owner's ti_on_cpu read; the pin keeps the Task alive across the read.
    pub(crate) fn optimistic_probe(&'static self, pid: u32) -> OptimisticProbe {
        // One locked section: the fair-try gates + owner snapshot.
        let owner_pid = {
            let mut st = self.state.lock();
            if !st.locked && st.waiters == 0 {
                st.locked = true;
                st.owner = pid;
                st.depth = 1;
                st.acquired_at = crate::timer::get_jiffies();
                return OptimisticProbe::Acquired(SleepMutexGuard { lock: self });
            } else if st.locked && st.owner == pid {
                st.depth += 1;
                return OptimisticProbe::Acquired(SleepMutexGuard { lock: self });
            } else if !st.locked {
                // Free behind counted waiters: the handoff window. The
                // fair gate refuses us; the woken token holder is on its
                // way. Spinning here only burns the CPU it needs.
                return OptimisticProbe::Stop;
            }
            st.owner
        };
        // Holder task on-CPU? Pinned lookup keeps the Task alive across
        // the read (a racing release+exit would otherwise free it).
        let t = crate::process::hotpath::pid_hash_lookup_pinned(owner_pid);
        if t.is_null() {
            // Owner pid unresolvable (exiting / hash moment): cannot
            // prove spinning is profitable — queue. The R77 stall
            // tripwire separately watches a dead-owner leak.
            return OptimisticProbe::Stop;
        }
        let on_cpu = unsafe {
            (*t).ti_on_cpu.load(core::sync::atomic::Ordering::Acquire)
        };
        crate::process::types::Task::task_put(t);
        if on_cpu {
            OptimisticProbe::Spin
        } else {
            OptimisticProbe::Stop
        }
    }
}
