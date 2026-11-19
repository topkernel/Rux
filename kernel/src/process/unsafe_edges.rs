//! Process-lane framework edges — the audited unsafe boundary
//! (industrial-reduction round, lane L2, 2026-11-20).
//!
//! Doctrine (unsafe-5000-plan Part I + the framework/service split):
//! the service files of this lane (`task.rs`, `exit.rs`, `coredump.rs`,
//! `ns.rs`) end with ZERO `unsafe` keywords — checkable by grep, ready
//! for a crate-level `forbid(unsafe_code)`. Every genuinely-unsafe
//! operation they need lives HERE as a named edge with an INV contract,
//! in the shape of `kernel/src/safety/` (one audit per edge, not per
//! call site).
//!
//! Edge inventory:
//!
//! * [`borrow_task`] / [`borrow_task_mut`] — INV-PROC-BORROW-1: turn a
//!   live raw task pointer into a reference for the duration of a
//!   guarded access. This is the same operation the pre-conversion call
//!   sites performed inline (`unsafe { (*p).field() }`,
//!   `unsafe { &mut *p }`); the aliasing discipline is unchanged and is
//!   restated once here instead of per site.
//! * [`place_task`] — INV-PROC-PLACE-1: placement-new of a whole `Task`
//!   value into a freshly allocated task slot (the constructor edge
//!   behind `Task::new_task_at` / `Task::new_idle_at`).
//! * [`dump_read_page`] — INV-PROC-COREDUMP-1: page-table walk + one
//!   linear-map copy for the ELF core dumper (the arch-walker read that
//!   coredump.rs previously owned inline).
//!
//! This module is Framework: it may use `unsafe` and is part of the
//! audited TCB. Nothing here grows domain logic — the process semantics
//! stay in the service files; only the pointer discipline lives here.

use super::task::Task;

/// Borrow a live task immutably.
///
/// INV-PROC-BORROW-1 (aliasing-borrow of a task page): `p` is non-null
/// and points at a live `Task` whose page cannot be freed for the
/// duration of the returned reference. The caller proves liveness with
/// one of the kernel's existing disciplines — a `task_refcnt` pin
/// (`TaskRef`), the process-tree lock for ring/parent hops, the pid-hash
/// walk pin for hash-walk readers, or "this CPU is the task" for
/// current-task paths (`do_exit` and friends). This is exactly the
/// contract every pre-conversion `unsafe { (*p).field() }` site argued
/// inline; stating it once makes the call sites safe code the compiler
/// checks.
#[inline]
pub fn borrow_task<'a>(p: *mut Task) -> &'a Task {
    debug_assert!(!p.is_null());
    // SAFETY: INV-PROC-BORROW-1 (caller-proven liveness; the reference
    // borrows the task page, which the pin/lock/CPU-ownership keeps
    // mapped).
    unsafe { &*p }
}

/// Borrow a live task exclusively.
///
/// INV-PROC-BORROW-1 (mutable half): as [`borrow_task`], plus the
/// exclusion argument — the caller's existing serialization proves no
/// other CPU aliases the task mutably while this reference is live (the
/// CPU that owns the task, the tree lock for child-list surgery, or a
/// pre-runnable fork child nobody else has seen). No two live
/// `borrow_task_mut` references may name the same task.
#[inline]
pub fn borrow_task_mut<'a>(p: *mut Task) -> &'a mut Task {
    debug_assert!(!p.is_null());
    // SAFETY: INV-PROC-BORROW-1 mutable half (caller-proven liveness and
    // exclusion — the same argument the pre-conversion `&mut *p` sites
    // made inline).
    unsafe { &mut *p }
}

/// Placement-new of a `Task` value into a task slot.
///
/// INV-PROC-PLACE-1: `ptr` is non-null, aligned to `align_of::<Task>()`
/// and denotes `size_of::<Task>()` writable bytes of a FRESH task slot
/// (from the task allocator) that does not overlap any live object and
/// is not aliased by any reference — the slot is uninitialized memory
/// until this call. After the write the slot holds an initialized
/// `Task` and every field-inclusion error class the offset_of ladder
/// carried (sc130/R9-1/R68a: fields silently missing from the hand list)
/// is impossible: the compiler checks the value's completeness.
#[inline]
pub fn place_task(ptr: *mut Task, task: Task) {
    debug_assert!(!ptr.is_null());
    debug_assert!(ptr as usize % core::mem::align_of::<Task>() == 0);
    // SAFETY: INV-PROC-PLACE-1 — fresh, aligned, unaliased,
    // Task-sized-and-larger slot; `write` places the value without
    // dropping the (uninitialized) destination.
    unsafe { core::ptr::write(ptr, task) }
}

/// Read one page of target user memory through its page tables into
/// `out` (the core-dump memory reader).
///
/// INV-PROC-COREDUMP-1: `mm` is a pinned (Arc-held) address space whose
/// root page table cannot be freed mid-walk; the caller holds the mm's
/// PTE lock across the walk+copy so same-mm munmap cannot unlink leaf
/// tables underneath us (a dying multithreaded process may still have a
/// thread in munmap). The arch walker bounds-checks the root and every
/// intermediate entry against machine memory; non-present pages read as
/// zeros (page-granular: callers cover whole VMAs with one PT_LOAD).
/// The final copy is bounded to the intersection of `out` and the page
/// containing `va`, from the kernel linear map of a present user page.
///
/// The hand-rolled SV39 walk this replaces misread x86 PTEs on x86_64
/// builds (LTP r21 kill11 KERNPANIC) — the edge stays on the arch
/// walker, and only here.
pub fn dump_read_page(mm: &crate::mm::mm_struct::MmStruct, va: u64, out: &mut [u8]) {
    use crate::arch::mm::mm_ops::PageTableWalker;
    use crate::arch::mm::{phys_to_virt, PhysAddr};

    for b in out.iter_mut() {
        *b = 0;
    }
    // Walk under the mm PTE lock (fork-frag lane discipline): x86_64
    // munmap can unlink+defer-free emptied leaf tables of live mms, and
    // this walk dereferences PMD-resolved leaf tables.
    let _pte_guard = mm.pte_lock().lock();
    let root_ppn = mm.root_ppn();
    // SAFETY: INV-PROC-COREDUMP-1 — root_ppn belongs to the pinned mm;
    // the walker bounds-checks the root and every intermediate entry
    // against machine memory.
    let (ppn, _) = match unsafe { PageTableWalker::walk(root_ppn, va) } {
        Some(r) => r,
        None => return, // non-present page: zero-filled PT_LOAD content
    };
    // The arch walkers fold huge-leaf offsets in: `ppn` is the 4 KiB
    // frame containing `va`.
    let off = (va & 0xFFF) as usize;
    let copy = core::cmp::min(out.len(), 4096 - off);
    // SAFETY: INV-PROC-COREDUMP-1 — kernel linear map of a present user
    // page; the copy is bounded to min(out.len(), page remainder).
    let src = phys_to_virt(PhysAddr::new((ppn << 12) + off as u64)).0 as *const u8;
    unsafe { core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), copy) };
}

/// Read a File's `private_data` slot as a shared reference to `T`.
///
/// INV-PROC-NSFILE-1: the slot was populated by this file type's
/// constructor with a `Box::into_raw(Box<T>)` pointer (write-once
/// before the File is published to an fd table), the value is not
/// mutably aliasable through the FileOps read path, and the borrow
/// lives only within the callback that owns `file` — the same
/// discipline every FileOps implementation argued inline before.
#[inline]
pub fn file_private_data_ref<T>(file: &crate::fs::File) -> Option<&T> {
    // SAFETY: INV-PROC-NSFILE-1 — write-once cell read through the
    // shared borrow the callback owns; T matches the constructor's Box.
    let p = unsafe { *file.private_data.get() }?;
    Some(unsafe { &*(p as *const T) })
}

/// Take a File's `private_data` slot back as the owned `Box<T>` it was
/// installed as.
///
/// INV-PROC-NSFILE-2: the exactly-once reclaim edge for the close path
/// — the cell swap makes the take idempotent (a second close sees None)
/// and `Box::from_raw` consumes the constructor's `Box::into_raw`
/// allocation, matching the ownership transfer documented there.
#[inline]
pub fn file_private_data_take_boxed<T>(file: &crate::fs::File) -> Option<alloc::boxed::Box<T>> {
    // SAFETY: INV-PROC-NSFILE-2 — the take is under the close path's
    // single-threaded-per-File discipline; None after the first take.
    let p = unsafe { &mut *file.private_data.get() }.take()?;
    Some(unsafe { alloc::boxed::Box::from_raw(p as *mut T) })
}
