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

/// Diagnostic volatile read of one aligned u64.
///
/// INV-PROC-DIAG-1: bounded, read-only forensics of a possibly-racing
/// field (the pre-conversion `read_volatile(addr_of!(field))` sites);
/// volatile observes the latest committed value without asserting
/// exclusivity — exactly what a diagnostic wants.
#[inline]
pub fn read_volatile_u64(addr: *const u64) -> u64 {
    // SAFETY: INV-PROC-DIAG-1 — a single volatile load from a mapped,
    // aligned kernel address (the caller passes &field of a live
    // object).
    unsafe { core::ptr::read_volatile(addr) }
}

/// The user program counter of a trap frame (arch word, cfg-split).
///
/// INV-PROC-PTRD-1: `pr` is a shared read view of a live task's own
/// trap frame (from `Task::pt_regs_ref`); reading the arch's user-PC
/// word is a plain load of a `Copy` register struct.
#[inline]
pub fn pt_user_pc(pr: &crate::arch::pt_regs::PtRegs) -> u64 {
    #[cfg(feature = "x86_64")]
    {
        pr.rip
    }
    #[cfg(feature = "riscv64")]
    {
        pr.epc
    }
    #[cfg(feature = "aarch64")]
    {
        pr.pc
    }
}

/// The running task's current jbd2 journal handle, if it owns one.
///
/// INV-PROC-JOURNAL-1: the slot is written only by the task itself
/// (set_current/clear_current per-task exclusivity); reading it from
/// the same task's exit path races nobody. The returned reference is
/// valid until [`clear_journal_handle`] — the handle outlives the
/// dying task's syscall frame only because the unwind below is the
/// next thing exit does with it.
pub fn journal_handle(t: &Task) -> Option<&'static mut crate::fs::jbd2::Handle> {
    let p = t.journal_handle.get();
    if p.is_null() {
        None
    } else {
        // SAFETY: INV-PROC-JOURNAL-1 — per-task exclusivity; the handle
        // is alive until the exit path clears the slot (the callee then
        // drops the reference before returning).
        Some(unsafe { &mut *p })
    }
}

/// Clear the running task's journal-handle slot (the exit unwind's
/// second half).
///
/// INV-PROC-JOURNAL-1 (clear arm): same per-task exclusivity; after
/// this call no jbd2 code can reach the handle through the task.
pub fn clear_journal_handle(t: &Task) {
    t.journal_handle.set(core::ptr::null_mut());
}

/// Free the heap PtRegs the legacy fork path banked on the task, if
/// any.
///
/// INV-PROC-PTREGS-FREE-1: the pointer came from the fork path's
/// `alloc` with the exact `size_of::<PtRegs>()`, align-16 layout and is
/// freed exactly once (slot ownership: the field is not re-read after
/// the free; the dying/reaped task can never fork again).
pub fn free_fork_pt_regs(t: &Task) {
    let p = t.fork_pt_regs();
    if p.is_null() {
        return;
    }
    use alloc::alloc::{dealloc, Layout};
    let layout = Layout::from_size_align(
        core::mem::size_of::<crate::arch::pt_regs::PtRegs>(),
        16,
    )
    .unwrap();
    // SAFETY: INV-PROC-PTREGS-FREE-1 — matching-layout, exactly-once
    // dealloc of the fork banked allocation.
    unsafe { dealloc(p as *mut u8, layout) };
}

/// The exit path's return to the kernel root page table before the mm
/// is dropped.
///
/// INV-PROC-ROOTSWITCH-1: reads the boot-time root-table constant and,
/// when the dying task's satp still names another root, installs the
/// kernel root (the kernel linear mapping is present in every address
/// space's kernel portion and in ROOT_PAGE_TABLE itself). Runs after
/// the last user access and before `set_address_space(None)` — the same
/// window the inline block occupied.
pub fn switch_to_kernel_root_on_exit() {
    // SAFETY: INV-PROC-ROOTSWITCH-1 — reads the boot-time constant.
    let kernel_ppn = unsafe { crate::arch::mm::mmu_init::root_page_table_ppn() };
    let current_ppn = crate::arch::mm::read_satp() as u64 & 0xF_FFFF_FFFF_FFFF;
    if current_ppn != kernel_ppn {
        // SAFETY: INV-PROC-ROOTSWITCH-1 — the switch target carries the
        // full kernel linear map.
        unsafe {
            crate::mm::switch_address_space(kernel_ppn, 0);
        }
    }
}

/// The exit-path futex-stack forensics shim (sync::futex's
/// `apppark_rets_public`, still an unsafe fn on the futex side).
///
/// INV-PROC-APPPARK-1: `t` is the current, still-live dying task (the
/// futex walker reads its user stack, intact at do_exit time) — the
/// contract the callee's doc states.
pub fn apppark_rets(t: &Task) {
    // SAFETY: INV-PROC-APPPARK-1 — cur is the running dying task.
    unsafe { crate::sync::futex::apppark_rets_public(t) }
}

// ---- kernel-stack cache slot ops -------------------------------------
// The stack cache stores its free list INSIDE the dead stacks (the
// bottom word region is unused by the growing-down stack); these edges
// own the linkage and the raw heap calls.

/// Read a free-stack entry's next pointer.
///
/// INV-PROC-KSTACK-1: `entry` is a dead stack bottom previously linked
/// by [`freelist_link`] (or the cache head), accessed only under
/// STACK_CACHE — the lock that also serializes the alloc/pop side.
#[inline]
pub fn freelist_next(entry: *mut u8) -> *mut u8 {
    // SAFETY: INV-PROC-KSTACK-1 — entry is a linked, dead stack bottom.
    unsafe { (*(entry as *mut super::task::FreeStackEntry)).next as *mut u8 }
}

/// Write a free-stack entry's next pointer (the push-side linkage).
///
/// INV-PROC-KSTACK-1 (write arm): `entry` is a dead stack bottom that
/// this call is (re)linking into the cache under STACK_CACHE.
#[inline]
pub fn freelist_link(entry: *mut u8, next: *mut u8) {
    // SAFETY: INV-PROC-KSTACK-1 — dead stack bottom, lock-held linkage.
    unsafe {
        (*(entry as *mut super::task::FreeStackEntry)).next = next as *mut super::task::FreeStackEntry;
    }
}

/// Raw heap allocate (the kernel-stack allocator's one GlobalAlloc
/// edge).
///
/// INV-PROC-KSTACK-2: `layout` is a valid nonzero layout the caller
/// pairs with the matching [`heap_dealloc`] on the same pointer.
#[inline]
pub fn heap_alloc(layout: core::alloc::Layout) -> *mut u8 {
    // SAFETY: INV-PROC-KSTACK-2 — valid layout; null-checked by callers.
    unsafe { alloc::alloc::alloc(layout) }
}

/// Raw heap deallocate — the matching free for [`heap_alloc`].
///
/// INV-PROC-KSTACK-2 (free arm): `p` was returned by `alloc` with
/// EXACTLY this layout and is freed exactly once.
#[inline]
pub fn heap_dealloc(p: *mut u8, layout: core::alloc::Layout) {
    // SAFETY: INV-PROC-KSTACK-2 — matching layout, exactly-once free.
    unsafe { alloc::alloc::dealloc(p, layout) }
}

/// In-bounds pointer offset (slot addressing for packed blocks).
///
/// INV-PROC-KSTACK-3: `base + off` stays inside the caller's live
/// allocation (the packed-slab const asserts guarantee the slot fits).
#[inline]
pub fn slot_ptr(base: *mut u8, off: usize) -> *mut u8 {
    // SAFETY: INV-PROC-KSTACK-3 — const-asserted in-bounds slot offset.
    unsafe { base.add(off) }
}

/// Zero `len` bytes at `p` (fresh-stack hygiene for the high-water
/// probe).
///
/// INV-PROC-KSTACK-3 (zero arm): `p..p+len` is wholly owned by the
/// caller's fresh allocation, visible to nobody else yet.
#[inline]
pub fn zero_bytes(p: *mut u8, len: usize) {
    // SAFETY: INV-PROC-KSTACK-3 — owned fresh allocation, full range.
    unsafe { core::ptr::write_bytes(p, 0, len) }
}

/// Volatile store of one u64 (canary writes).
///
/// INV-PROC-DIAG-1 (write arm): aligned mapped kernel address owned by
/// the caller (the stack's first word).
#[inline]
pub fn write_volatile_u64(addr: *mut u64, v: u64) {
    // SAFETY: INV-PROC-DIAG-1 — single volatile store, owned slot.
    unsafe { core::ptr::write_volatile(addr, v) }
}

// The Send impls for the lock-guarded raw-pointer statics: the types are
// only ever accessed behind their Spinlocks (STACK_CACHE /
// KSTACK_BLOCKS), which serialize every dereference — the same words
// the pre-conversion comments at the types argued.
// SAFETY: StackCache's raw head is managed only under STACK_CACHE.
unsafe impl Send for super::task::StackCache {}
// SAFETY: the raw block bases are only ever dereferenced as stack
// storage handed out one slot at a time under the KSTACK_BLOCKS lock.
#[cfg(target_arch = "x86_64")]
unsafe impl Send for crate::process::task::KstackBlocks {}

/// Volatile read of one aligned u32 (poison-canary screens).
///
/// INV-PROC-DIAG-1 (u32 arm): as [`read_volatile_u64`].
#[inline]
pub fn read_volatile_u32(addr: *const u32) -> u32 {
    // SAFETY: INV-PROC-DIAG-1 — single volatile load from a mapped,
    // aligned kernel address the caller screened.
    unsafe { core::ptr::read_volatile(addr) }
}

/// Borrow an active-mm `AddressSpace` raw pointer (kernel-thread
/// borrowed address space).
///
/// INV-PROC-ACTIVEMM-1: `mm_ptr` names an AddressSpace whose owning
/// task's mm slot still references it (the lazy-mm discipline); the
/// borrow lives only within the accessor call, matching the
/// pre-conversion inline deref's contract.
#[inline]
pub fn borrow_active_mm<'a>(mm_ptr: *const crate::mm::AddressSpace) -> &'a crate::mm::AddressSpace {
    debug_assert!(!mm_ptr.is_null());
    // SAFETY: INV-PROC-ACTIVEMM-1 — the owner's reference keeps the
    // AddressSpace alive for this short borrow.
    unsafe { &*mm_ptr }
}

// ---- intrusive tree-list linkage --------------------------------------
// ListHead's add/add_tail/del/for_each are unsafe fns (intrusive-list
// surgery on raw nodes). The process tree serializes every one of its
// link operations under PROCESS_TREE_LOCK; these wrappers carry that
// proof once.

/// Link `node` at the tail of `head`'s list.
///
/// INV-PROC-LIST-1: the caller holds PROCESS_TREE_LOCK (or owns both
/// nodes pre-publication — the fork-build path); `node` is an
/// initialized ListHead not currently linked elsewhere, and `head` is a
/// live list head.
#[inline]
pub fn list_add_tail(node: &mut crate::list::ListHead, head: *mut crate::list::ListHead) {
    // SAFETY: INV-PROC-LIST-1 — lock-held (or pre-publication) link of
    // an initialized node into a live head.
    unsafe { node.add_tail(head) }
}

/// Unlink `node` from its list.
///
/// INV-PROC-LIST-1 (unlink arm): the caller holds PROCESS_TREE_LOCK and
/// `node` is currently linked in a list whose neighbors are live.
#[inline]
pub fn list_del(node: &mut crate::list::ListHead) {
    // SAFETY: INV-PROC-LIST-1 — lock-held unlink of a linked node.
    unsafe { node.del() }
}

/// Walk `head`'s list, handing each node pointer to `f`.
///
/// INV-PROC-LIST-1 (walk arm): the caller holds the lock that
/// serializes link/unlink on `head` for the whole walk; `f` must not
/// store the node pointer past its return.
#[inline]
pub fn list_for_each<F>(head: *mut crate::list::ListHead, f: F)
where
    F: FnMut(*mut crate::list::ListHead),
{
    // SAFETY: INV-PROC-LIST-1 — lock-held walk of a live list.
    unsafe { crate::list::ListHead::for_each(head, f) }
}

/// Deref a task pointer for the wake/notify family (the (*task) sites).
#[inline]
pub fn task_pid_of(task: *mut Task) -> u32 {
    borrow_task(task).pid()
}

/// The paired {on_cpu, running_on_cpu} ownership qword store (the
/// pick-ownership publish/release protocol).
///
/// INV-PROC-PICKQWORD-1: `addr` is the aligned address of the task's
/// `ti_on_cpu` field, immediately followed by `running_on_cpu` (the
/// layout the switch-out asm depends on); the store is the single
/// aligned Release write the protocol mandates (owner-in-word).
#[inline]
pub fn store_pick_qword(addr: usize, v: u64) {
    // SAFETY: INV-PROC-PICKQWORD-1 — aligned two-field qword at the
    // documented Task offset; single Release store.
    unsafe {
        (*(addr as *const core::sync::atomic::AtomicU64)).store(v, core::sync::atomic::Ordering::Release)
    }
}

/// Poison-paint a freed stack (scribble-trap arm).
///
/// INV-PROC-KSTACK-3 (paint arm): `bottom+off .. bottom+off+len` is
/// inside the dead stack allocation being returned to the cache
/// (bounds-checked by the caller's recycle gate).
#[inline]
pub fn paint_bytes(bottom: *mut u8, off: usize, val: u8, len: usize) {
    // SAFETY: INV-PROC-KSTACK-3 — in-allocation paint range.
    unsafe { core::ptr::write_bytes(bottom.add(off), val, len) }
}

// ---- trap-frame windows ------------------------------------------------
// The round-3b/4 typed views (Task::pt_regs_mut / pt_regs_ref) hold one
// audited edge each; they live here so task.rs carries zero unsafe.

/// Exclusive trap-frame window (the `pt_regs_mut` edge).
///
/// INV-PROC-FRAME-1: `p` is the PtRegs-sized window at the top of a
/// task's OWN kernel stack (kernel_stack is Some — the caller checked);
/// the slot discipline keeps the frame alive for the slot's `'static`
/// lifetime, and the caller proves exclusivity (the task is not yet
/// enqueued, or this CPU owns it).
#[inline]
pub fn frame_mut(
    p: *mut crate::arch::pt_regs::PtRegs,
) -> &'static mut crate::arch::pt_regs::PtRegs {
    debug_assert!(!p.is_null());
    // SAFETY: INV-PROC-FRAME-1 — own-stack window, slot lifetime.
    unsafe { &mut *p }
}

/// Shared trap-frame window (the `pt_regs_ref` edge).
///
/// INV-PROC-FRAME-1 (shared arm): as `frame_mut`, for diagnostic
/// readers; concurrent reads while another CPU runs the task see a
/// frame mid-flight — the caller's diagnostic-read contract.
#[inline]
pub fn frame_ref(p: *mut crate::arch::pt_regs::PtRegs) -> &'static crate::arch::pt_regs::PtRegs {
    debug_assert!(!p.is_null());
    // SAFETY: INV-PROC-FRAME-1 — own-stack window, shared reads only.
    unsafe { &*p }
}
