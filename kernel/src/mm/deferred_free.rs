//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Time-deferred heap frees for teardown-raced objects.
//!
//! Rationale (exitwalk probe, exit/teardown family): procfs walkers
//! (generate_maps/smaps/status) take `task.address_space()` — an
//! unaccounted `&MmStruct` borrow of a foreign task. Even with the
//! teardown's vma_write walker-exclusion and the RCU grace-period wait,
//! a reader preempted mid-syscall can resume AFTER the last Arc drop
//! freed the MmStruct and the heap reused its memory: the reader then
//! locks a garbage rwlock word and iterates a garbage tree pointer —
//! the #GP-in-btree-navigate family (reference OH log pid 554,
//! exitwalk pid 314/315: atomic_load inside the node comparison
//! chasing a non-canonical child pointer).
//!
//! Mitigation: frees of allocations whose size matches a registered
//! "deferred size" are held back for DEFER_MS before returning to the
//! heap. By then the teardown has left the object in a coherent
//! post-teardown state (empty VMA tree — late readers navigate a valid
//! EMPTY BTreeMap, not freed memory) and any in-flight borrower of the
//! old object has long finished. This is the same idea as Linux freeing
//! mm_struct via call_rcu, but wall-clock based so kernel-preempted
//! readers are covered too (our RCU counts preemption points as
//! quiescent states, which a mid-syscall borrower is not).
//!
//! The remaining window (a reader parked >DEFER_MS mid-syscall) is
//! negligible; the proper fix — pinning the mm with address_space_arc()
//! in the procfs generators — is a one-line-per-site change in fs/ that
//! this lane does not touch.
//!
//! Bounded: the pool is a fixed ring; on overflow the free is immediate
//! (normal semantics). Reclamation is driven opportunistically from the
//! alloc path (the same churn that fills the pool drains it), so an
//! idle system never accumulates entries.

use core::alloc::Layout;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Pool capacity — bounded memory exposure (entries are 32 bytes each).
const POOL_CAP: usize = 16384;
/// How long a matched free is held back (ms of jiffies).
const DEFER_MS: u64 = 1500;

#[derive(Clone, Copy)]
struct Entry {
    ptr: usize,
    size: usize,
    align: usize,
    deadline: u64,
}

/// Registered size classes whose frees are deferred (first writer wins
/// per slot). Two live classes: the MmStruct ArcInner block and the Task
/// page itself (procfs walkers also borrow the TASK unpinned — the reaper
/// on another CPU can free it mid-read; free_task_slot poisons the page
/// before dealloc, so late readers see poison values, never heap-reuse
/// garbage pointers).
static DEFER_SIZES: [AtomicUsize; 4] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];
static mut POOL: [Entry; POOL_CAP] = [Entry {
    ptr: 0,
    size: 0,
    align: 0,
    deadline: 0,
}; POOL_CAP];
static POOL_HEAD: AtomicUsize = AtomicUsize::new(0); // next slot to fill
static POOL_DRAINED: AtomicUsize = AtomicUsize::new(0); // oldest live entry
/// Earliest deadline currently in the pool — relaxed peek for the alloc
/// path so the common case costs one load, no lock.
static EARLIEST_DEADLINE: AtomicU64 = AtomicU64::new(u64::MAX);
static POOL_LOCK: crate::sync::spinlock::Spinlock<()> = crate::sync::spinlock::Spinlock::new(());

/// Register the size class whose frees are deferred (first writer wins;
/// call from the type's constructor). Only ONE size class is supported —
/// enough for the MmStruct teardown race this exists for.
pub fn register_deferred_size(size: usize) {
    if size == 0 {
        return;
    }
    // NOTE: no printk here — registration runs at the FIRST raw Task-slot
    // init (idle-task creation), earlier than printk bring-up on riscv64;
    // a pr_err there hung the boot at the banner (final2/final3 runs).
    // Engagement (first successful defer) is witnessed later, post-boot.
    for slot in DEFER_SIZES.iter() {
        match slot.compare_exchange(0, size, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return,
            Err(v) if v == size => return, // already registered
            Err(_) => continue,
        }
    }
}

#[inline]
pub fn deferred_size() -> usize {
    DEFER_SIZES[0].load(Ordering::Acquire)
}

/// Maybe hold back this free. Returns true when the entry was queued
/// (the caller must NOT free); false when it should free normally
/// (size mismatch or pool full).
///
/// Safe in any context (irqsave spinlock; no allocation).
pub fn try_defer(ptr: *mut u8, layout: &Layout) -> bool {
    let size = layout.size();
    if size == 0 {
        return false;
    }
    let mut matched = false;
    for slot in DEFER_SIZES.iter() {
        if slot.load(Ordering::Acquire) == size {
            matched = true;
            break;
        }
    }
    if !matched {
        return false;
    }
    let now = crate::drivers::timer::get_jiffies();
    let deadline = now.saturating_add(DEFER_MS);
    let _g = POOL_LOCK.lock_irqsave();
    let pool = unsafe { &mut *core::ptr::addr_of_mut!(POOL) };
    let head = POOL_HEAD.load(Ordering::Relaxed);
    let drained = POOL_DRAINED.load(Ordering::Relaxed);
    let used = head.wrapping_sub(drained);
    if used >= POOL_CAP {
        return false; // pool full — fall back to immediate free
    }
    let idx = head % POOL_CAP;
    pool[idx] = Entry {
        ptr: ptr as usize,
        size,
        align: layout.align(),
        deadline,
    };
    POOL_HEAD.store(head.wrapping_add(1), Ordering::Relaxed);
    let earliest = EARLIEST_DEADLINE.load(Ordering::Relaxed);
    if deadline < earliest {
        EARLIEST_DEADLINE.store(deadline, Ordering::Relaxed);
    }
    // One-time engagement witness: proves the size class matched a real
    // free (the first iteration of this guard registered the WRONG size
    // — bare size_of::<MmStruct> instead of the ArcInner block — and
    // silently deferred nothing).
    static ENGAGED: AtomicUsize = AtomicUsize::new(0);
    if ENGAGED.fetch_add(1, Ordering::Relaxed) == 0 {
        crate::pr_err!("DEFERFREE: engaged (first MmStruct free held back {}ms)", DEFER_MS);
    }
    true
}

/// Cheap alloc-path hook: drain only when the earliest pending deadline
/// has passed (one relaxed load otherwise).
#[inline]
pub fn drain_if_due() {
    let now = crate::drivers::timer::get_jiffies();
    if now >= EARLIEST_DEADLINE.load(Ordering::Relaxed) {
        drain(now);
    }
}

/// Free every entry whose deadline has passed. Returns the number freed.
/// Entry payloads are copied out under the lock; the actual dealloc runs
/// OUTSIDE the pool lock and calls `dealloc_direct` (not the intercepted
/// GlobalAlloc path — no re-defer).
pub fn drain(now: u64) -> usize {
    let mut batch: [(usize, usize, usize); 64] = [(0, 0, 0); 64]; // ptr,size,align
    let mut batch_n = 0usize;
    let mut new_earliest = u64::MAX;
    {
        let _g = POOL_LOCK.lock_irqsave();
        let pool = unsafe { &mut *core::ptr::addr_of_mut!(POOL) };
        let mut drained = POOL_DRAINED.load(Ordering::Relaxed);
        let head = POOL_HEAD.load(Ordering::Relaxed);
        while drained < head && batch_n < batch.len() {
            let idx = drained % POOL_CAP;
            if pool[idx].deadline > now {
                new_earliest = new_earliest.min(pool[idx].deadline);
                break; // ring is deadline-ordered by construction
            }
            batch[batch_n] = (pool[idx].ptr, pool[idx].size, pool[idx].align);
            batch_n += 1;
            drained = drained.wrapping_add(1);
        }
        POOL_DRAINED.store(drained, Ordering::Relaxed);
        EARLIEST_DEADLINE.store(new_earliest, Ordering::Relaxed);
    }
    for i in 0..batch_n {
        let (ptr, size, align) = batch[i];
        if let Ok(layout) = Layout::from_size_align(size, align) {
            unsafe {
                crate::mm::buddy_allocator::dealloc_untracked_pub(ptr as *mut u8, layout);
            }
        }
    }
    batch_n
}
