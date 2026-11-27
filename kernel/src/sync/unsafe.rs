//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The sync subsystem's ONE audited unsafe file (framework zone).
//!
//! Mainline layout (2026-11-27): this is the former edges.rs renamed
//! per the 2026-11-25 edges->unsafe.rs ruling, now holding the whole
//! subsystem's unsafe — the L1 INV-SY edges, the guard-cell family
//! (INV-SY-4), the lock debug-probe family (INV-SY-5), the percpu row
//! family (INV-SY-6), the RCU list family (INV-SY-7), the seqlock
//! volatile snapshot (INV-SY-8), and every Send/Sync commitment.
//! Every OTHER file in the subsystem is safe (file-level forbid).
//!
//! unsafe-slim round L1 (2026-11-20), framekernel doctrine: the sync
//! POLICY files (futex.rs, futex_pi.rs) are safe by construction; the
//! genuinely-unsafe memory edges they need live here with INV-SY
//! contracts. Task pointer→reference derivations reuse
//! [`crate::sched::edges::task_shared`] (same INV-SCE-2 contract).
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

use crate::process::task::Task;

// ---------------------------------------------------------------------------
// INV-SY-1: cross-task user-memory diagnostic reads
// ---------------------------------------------------------------------------

/// Read up to `buf.len()` bytes in `task`'s user address space via its
/// page tables, stopping at the first unmapped page (KILLDUMP sc97:
/// iovec arrays, path strings, log payload heads of the wedged task).
/// Returns how many bytes were actually read.
pub fn read_bytes_via_task(task: *const Task, va: usize, buf: &mut [u8]) -> usize {
    let tk = crate::sched::edges::task_shared(task);
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
    let tk = crate::sched::edges::task_shared(task);
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



// ===========================================================================
// INV-SY-4: the guard-cell family (mainline consolidation, 2026-11-27).
// The lock guards (SpinlockGuard and its irq/bh twins, the rwlock/rwsem/
// seqlock guards) deref their protected cell exactly while the lock is
// held — the guard's own lifetime is the exclusivity proof. The guard
// impls live in the home files; every raw crossing is these two fns.
// ===========================================================================

#[inline]
pub fn cell_ref<T: ?Sized>(c: &core::cell::UnsafeCell<T>) -> &T {
    // SAFETY: INV-SY-4 — the caller holds the owning lock's guard (or
    // the documented boot write-once contract); no concurrent &mut can
    // exist while that guard is alive.
    unsafe { &*c.get() }
}

#[inline]
pub fn cell_mut<T: ?Sized>(c: &core::cell::UnsafeCell<T>) -> &mut T {
    // SAFETY: INV-SY-4 — the caller holds the owning lock's WRITE
    // guard; the guard protocol makes the &mut exclusive.
    unsafe { &mut *c.get() }
}

// ===========================================================================
// INV-SY-5: the lock debug-probe family (mainline consolidation,
// 2026-11-27). Cold-path deadlock/wedge reports only: raw views of a
// wedged holder's lock word, owner stamp, and heap span, plus the
// call-site capture register read. `lock_addr` is always an address
// captured by lock() itself, so the lock is live (wedged) while read.
// ===========================================================================

/// Capture this function's return address (the spinlock call site) for
/// deadlock diagnostics. Moved whole from spinlock.rs.
#[inline]
pub fn caller_return_address() -> usize {
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

/// Raw view of a wedged RawSpinlock (INV-SY-5). The borrow is confined
/// to the report scope; the lock is live (wedged) for the whole report.
pub fn rawspin_ref<'a>(
    lock_addr: *const crate::sync::spinlock::RawSpinlock,
) -> &'a crate::sync::spinlock::RawSpinlock {
    // SAFETY: INV-SY-5 — lock_addr is a valid RawSpinlock pointer (from
    // lock(), the wedged holder); diagnostic read-only view.
    unsafe { &*lock_addr }
}

/// The lock WORD value of a wedged lock (INV-SY-5, the R11 unblinding).
#[inline]
pub fn rawspin_locked_word(lock_addr: *const crate::sync::spinlock::RawSpinlock) -> u32 {
    rawspin_ref(lock_addr).locked_word()
}

/// The owner stamp of a wedged lock (INV-SY-5, the holder hunt).
#[cfg(feature = "dfx-lock-owner")]
#[inline]
pub fn rawspin_owner(lock_addr: *const crate::sync::spinlock::RawSpinlock) -> usize {
    rawspin_ref(lock_addr).owner.load(core::sync::atomic::Ordering::Relaxed)
}

/// One unaligned u64 read of a mapped kernel window (INV-SY-5): the
/// hexdump of the heap allocation containing a wedged lock (offline
/// symbolization). Races cost at worst a torn-but-bounded word.
#[inline]
pub fn read_unaligned_u64(addr: usize) -> u64 {
    // SAFETY: INV-SY-5 — diagnostic read within a mapped kernel span.
    unsafe { (addr as *const u64).read_unaligned() }
}

// ===========================================================================
// INV-SY-6: the percpu row family (mainline consolidation, 2026-11-27).
// CpuLocal's raw row walk: cpu is bounds-checked by the caller; the row
// is private to this cpu while IRQs (or preemption) stay disabled, and
// the in_use flag refuses a second live guard, so the handed-out &mut
// is exclusive. All access through raw pointers derived from the
// UnsafeCell — never through a &-path (the D-7 incident family).
// ===========================================================================

#[inline]
pub(crate) fn cpu_local_row_ptr<T, const N: usize>(
    rows: *mut [crate::sync::percpu::Row<T>; N],
    cpu: usize,
) -> *mut crate::sync::percpu::Row<T> {
    // SAFETY: INV-SY-6 — cpu bounds-checked by the caller; the row is
    // this cpu's private slot for the guard's lifetime.
    unsafe { rows.cast::<crate::sync::percpu::Row<T>>().add(cpu) }
}

/// Field address derivation for a row (no deref; INV-SY-6).
#[inline]
pub(crate) fn cpu_local_field_ptrs<T>(
    row: *mut crate::sync::percpu::Row<T>,
) -> (*mut T, *mut bool) {
    // SAFETY: INV-SY-6 — addr_of_mut forms no reference to the fields.
    unsafe { (
        core::ptr::addr_of_mut!((*row).val),
        core::ptr::addr_of_mut!((*row).in_use),
    ) }
}

/// The row's in_use flag read (INV-SY-6): owner-cpu-only plain bool.
#[inline]
pub(crate) fn cpu_local_in_use(p: *mut bool) -> bool {
    // SAFETY: INV-SY-6 — only the owning cpu reads/writes this flag.
    unsafe { *p }
}

/// The row's in_use flag store (INV-SY-6): owner-cpu-only plain bool.
#[inline]
pub(crate) fn cpu_local_in_use_set(p: *mut bool, v: bool) {
    // SAFETY: INV-SY-6 — only the owning cpu reads/writes this flag.
    unsafe { *p = v }
}

/// The row's value slot (INV-SY-6): exclusive by the in_use protocol.
#[inline]
pub(crate) fn cpu_local_val<'a, T>(p: *mut T) -> &'a mut T {
    // SAFETY: INV-SY-6 — the in_use handshake refused every alias, so
    // the &mut is exclusive while the guard lives.
    unsafe { &mut *p }
}

// ===========================================================================
// INV-SY-7: the RCU callback-list family (mainline consolidation,
// 2026-11-27). The cb_list lock for the cpu is held at every crossing;
// both list nodes were init()'d via the RcuHead/ListHead protocol.
// ===========================================================================

/// The intrusive rewire (the reference's blist_add_tail counterpart) —
/// it mutates THIS node's links. Caller holds the cb_list lock.
pub fn list_add_tail(node: &mut crate::list::ListHead, tail: *mut crate::list::ListHead) {
    // INV-SY-7 via INV-LIST-RAW-1 (list.rs's safe API): lock held,
    // both nodes initialized.
    node.add_tail(tail)
}

/// Enqueue `head`'s list node onto cpu's callback list (INV-SY-7): the
/// caller holds the cb_list lock for `cpu`; head.list was initialised
/// via init() (the RcuHead protocol).
pub fn rcu_call_enqueue(head: &mut crate::sync::rcu::RcuHead, cpu: usize) {
    // SAFETY: INV-SY-7 — cb_list lock held; the &mut to the static-mut
    // element is the lock's own exclusivity.
    unsafe { list_add_tail(&mut head.list, &mut crate::sync::rcu::RCU_CBS[cpu]) }
}

/// Re-initialize cpu's callback-list head (INV-SY-7: boot-time init,
/// or under the cb_list lock).
pub fn rcu_cbs_init(cpu: usize) {
    // SAFETY: INV-SY-7 — the cb_list lock protocol serializes this.
    unsafe { crate::sync::rcu::RCU_CBS[cpu].init() }
}

/// Detach the per-cpu callback list into `batch` (the splice). Caller
/// holds the cb_list lock for `cpu` (RCU_CBS[cpu] is only accessed by
/// us); the list pointers were initialised in init().
pub fn rcu_detach_batch(cpu: usize, batch: &mut crate::list::ListHead) {
    // SAFETY: INV-SY-7 — cb_list lock held; raw list surgery on
    // initialized nodes.
    unsafe {
        let cbs = &mut crate::sync::rcu::RCU_CBS[cpu];
        if !cbs.is_empty() {
            // Splice the list into `batch`.
            batch.next = cbs.next;
            batch.prev = cbs.prev;
            (*batch.next).prev = batch;
            (*batch.prev).next = batch;
            cbs.init();
        }
    }
}

/// Invoke every callback of a detached batch (the caller owns the list
/// exclusively after rcu_detach_batch). Each RcuHead was registered via
/// call_rcu with a valid func pointer and remains valid until the
/// callback executes.
pub fn rcu_invoke_batch(batch: &mut crate::list::ListHead) {
    // SAFETY: INV-SY-7 — exclusive ownership of the detached batch.
    unsafe {
        let sentinel = batch as *mut crate::list::ListHead;
        let mut node = batch.next;
        while node != sentinel {
            let next = (*node).next;
            let head = node as *mut crate::sync::rcu::RcuHead;
            let func = (*head).func;
            func(head as *mut core::ffi::c_void);
            node = next;
        }
    }
}

// ===========================================================================
// INV-SY-8: the seqlock snapshot (mainline consolidation, 2026-11-27).
// One volatile read of the protected cell while the sequence is even
// (no writer) and preemption is disabled by the caller.
// ===========================================================================

#[inline]
pub fn volatile_read_cell<T: Copy>(c: *mut T) -> T {
    // SAFETY: INV-SY-8 — single volatile read of the seqlock cell.
    unsafe { core::ptr::read_volatile(c) }
}

// ===========================================================================
// The keyed-raw unlock edge (mainline consolidation, 2026-11-27): the
// KeyedGuard drop path releases through the raw pointer it holds; the
// nest-key protocol (the caller's own guard) proves the lock is held.
// ===========================================================================

#[inline]
pub fn keyed_raw_unlock<K: crate::sync::lockkey::NestKey>(
    lock: *mut crate::sync::lockkey::KeyedRawSpinlock<K>,
) {
    // SAFETY: the KeyedGuard protocol — the guard exists only while the
    // keyed lock is held, so this is the single legitimate release.
    unsafe { (*lock).raw.unlock() }
}

// ===========================================================================
// Send/Sync commitments (moved verbatim from their home files by the
// 2026-11-27 mainline consolidation; each carries its original SAFETY
// note — the per-field lock/cell discipline arguments live with the
// type definitions, and these impls add no field access of their own).
// ===========================================================================

// SAFETY: Spinlock<T> only allows &mut T access via the guard, which exists
// only while the inner lock is held.  Send is required so the lock (and its
// data) can be moved between threads; Sync is safe because the lock mediates
// all access.
unsafe impl<T: ?Sized + Send> Send for crate::sync::spinlock::Spinlock<T> {}
unsafe impl<T: ?Sized + Send> Sync for crate::sync::spinlock::Spinlock<T> {}

// SAFETY: same contract as WaitQueueHead/SleepMutex — the raw task
// pointers inside the wait list are only ever passed to
// sched::wake_up_process (whose own guards handle stale targets);
// every list mutation is serialized by wait_lock; a queued waiter
// cannot exit (UNINTERRUPTIBLE), so entries never dangle.
unsafe impl Send for crate::sync::rwsem::RwSemaphore {}
unsafe impl Sync for crate::sync::rwsem::RwSemaphore {}
// SAFETY: Waiter is only ever accessed while the per-slot Spinlock is held,
// serialising all reads and writes.  The `task` raw pointer is only
// dereferenced by the waker after validating it is non-null.
unsafe impl Send for crate::sync::futex::Waiter {}
unsafe impl Sync for crate::sync::futex::Waiter {}
// SAFETY: PiWaiter slots are only accessed while the per-slot spinlock
// is held; the task raw pointer is only dereferenced by the waker after
// a non-null check — the same contract as the plain Waiter.
unsafe impl Send for crate::sync::futex_pi::PiWaiter {}
unsafe impl Sync for crate::sync::futex_pi::PiWaiter {}
// (SpinlockGuard Send commitment lives in crate::sync::edges —
// unsafe.rs; mainline consolidation.)
// SAFETY: SpinlockGuard exists only while the lock is held, so the guarded
// data cannot be accessed concurrently from another thread.
unsafe impl<T: ?Sized + Send> Send for crate::sync::spinlock::SpinlockGuard<'_, T> {}
// SAFETY: SpinlockIrqGuard exists only while the lock is held, so the
// guarded data cannot be accessed concurrently.
unsafe impl<T: ?Sized + Send> Send for crate::sync::spinlock::SpinlockIrqGuard<'_, T> {}
// SAFETY: SpinlockBhGuard exists only while the lock is held, so the
// guarded data cannot be accessed concurrently.
unsafe impl<T: ?Sized + Send> Send for crate::sync::spinlock::SpinlockBhGuard<'_, T> {}
// SAFETY: RwSpinlock<T> mediates all access to the inner data: shared (&T) via
// read-locked guards, exclusive (&mut T) via write-locked guards.  Send allows
// moving between threads; Sync allows shared references across threads because
// the rwlock serialises access.
unsafe impl<T: ?Sized + Send> Send for crate::sync::rwlock::RwSpinlock<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for crate::sync::rwlock::RwSpinlock<T> {}
// SAFETY: ReadGuard exists only while a read lock is held, preventing any
// writer from obtaining exclusive access to the data.
unsafe impl<T: ?Sized + Sync> Send for crate::sync::rwlock::RwSpinlockReadGuard<'_, T> {}
// SAFETY: WriteGuard exists only while the write lock is held exclusively,
// so no other thread can access the data.
unsafe impl<T: ?Sized + Send> Send for crate::sync::rwlock::RwSpinlockWriteGuard<'_, T> {}
// SAFETY: IrqReadGuard exists only while a read lock + IRQ disable is held,
// preventing any writer from accessing the data.
unsafe impl<T: ?Sized + Sync> Send for crate::sync::rwlock::RwSpinlockIrqReadGuard<'_, T> {}
// SAFETY: IrqWriteGuard exists only while the write lock + IRQ disable is
// held exclusively, so no other thread can access the data.
unsafe impl<T: ?Sized + Send> Send for crate::sync::rwlock::RwSpinlockIrqWriteGuard<'_, T> {}
// SAFETY: BhReadGuard exists only while a read lock + BH disable is held,
// preventing any writer from accessing the data.
unsafe impl<T: ?Sized + Sync> Send for crate::sync::rwlock::RwSpinlockBhReadGuard<'_, T> {}
// SAFETY: BhWriteGuard exists only while the write lock + BH disable is
// held exclusively, so no other thread can access the data.
unsafe impl<T: ?Sized + Send> Send for crate::sync::rwlock::RwSpinlockBhWriteGuard<'_, T> {}
// SAFETY: Rwsem<T> mediates all access to the inner data: shared (&T)
// via read-locked guards, exclusive (&mut T) via write-locked guards.
unsafe impl<T: ?Sized + Send> Send for crate::sync::rwsem::Rwsem<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for crate::sync::rwsem::Rwsem<T> {}
// SAFETY: guard exists only while the read lock is held, preventing
// any writer from obtaining exclusive access to the data.
unsafe impl<T: ?Sized + Sync> Send for crate::sync::rwsem::RwsemReadGuard<'_, T> {}
// SAFETY: guard exists only while the write lock is held exclusively.
unsafe impl<T: ?Sized + Send> Send for crate::sync::rwsem::RwsemWriteGuard<'_, T> {}
// Safety: writers have exclusive access; readers get a Copy snapshot.
unsafe impl<T: Copy + Send> Send for crate::sync::seqlock::SeqLock<T> {}
unsafe impl<T: Copy + Send> Sync for crate::sync::seqlock::SeqLock<T> {}
unsafe impl<T: Copy + Send> Send for crate::sync::seqlock::SeqLockWriteGuard<'_, T> {}
// SAFETY: RawSpinlock's atomic protocol is the synchronization; the
// key is zero-sized and carries no data.
unsafe impl<K: crate::sync::lockkey::NestKey> Sync for crate::sync::lockkey::KeyedRawSpinlock<K> {}
// SAFETY: same rule as Spinlock itself — the atomic protocol
// synchronizes; the key is zero-sized and carries no data.
unsafe impl<K: crate::sync::lockkey::NestKey, T: Send> Sync for crate::sync::lockkey::KeyedSpinlock<K, T> {}
// SAFETY: see the struct comment — rows are cpu-private by
// construction of `with`.
unsafe impl<T, const N: usize> Sync for crate::sync::percpu::CpuLocal<T, N> {}
