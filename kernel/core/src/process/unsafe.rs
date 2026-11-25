//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Process-lane framework edges — the audited unsafe boundary
//! (industrial-reduction round, lane L2, 2026-11-20).
//!
//! Doctrine (unsafe-5000-plan Part I + the framework/service split):
//! the service files of this lane (`task.rs`, `exit.rs`, `coredump.rs`,
//! `ns.rs`) end with ZERO `unsafe` keywords — checkable by grep, ready
//! for a crate-level `forbid(unsafe_code)`. Every genuinely-unsafe
//! operation they need lives HERE as a named edge with an INV contract,
//! in the shape of `kernel/core/src/safety/` (one audit per edge, not per
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
///
/// INV-PROC-PLACE-2 (self-reference rule, added by the slim4-fix lane
/// 2026-11-20, hardened same day): the placed value must NOT carry
/// addresses of itself. Build-then-place copies the struct to the slot,
/// so a self-referential field initialized inside the constructor
/// (ListHead::init() pointing children/sibling/rt_run_list at the
/// TEMPORARY's own address) dangles at the dead temporary after the
/// copy — every empty-list head then dereferences freed stack memory on
/// the next list op (the Round-B fio null-walk corruption family,
/// convicted beeb17524). The re-anchor is ENFORCED HERE, at the single
/// placement choke point: every constructor route goes through this
/// edge, so the rule holds by construction and a future constructor
/// cannot forget it. Self-referential fields that a delta legitimately
/// overwrites with a FINAL address (group_leader/next_thread) are
/// simply re-set after placement, which also re-anchors them.
#[inline]
pub fn place_task(ptr: *mut Task, task: Task) {
    debug_assert!(!ptr.is_null());
    debug_assert!(ptr as usize % core::mem::align_of::<Task>() == 0);
    // SAFETY: INV-PROC-PLACE-1 — fresh, aligned, unaliased,
    // Task-sized-and-larger slot; `write` places the value without
    // dropping the (uninitialized) destination.
    unsafe { core::ptr::write(ptr, task) }
    // INV-PROC-PLACE-2: re-anchor the self-referential list heads AT
    // THE PLACED ADDRESS (init() is idempotent; the constructor-side
    // anchors were redundant once the edge owns the rule).
    let t = borrow_task_mut(ptr);
    t.children.init();
    t.sibling.init();
    t.rt_run_list.init();
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
pub fn dump_read_page(mm: &crate::mm::types::MmStruct, va: u64, out: &mut [u8]) {
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

/// Read a File's `private_data` slot as a MUTABLE reference to `T`.
///
/// INV-PROC-NSFILE-1 (mutable arm): the caller has verified the File's
/// type identity (the ops-table check that precedes every downcast) and
/// holds the serialization that proves no other alias is live — the
/// owning fd-table Arc pin plus the type's own lock discipline (the
/// epoll `entries` lock guards every mutable touch of an EpollFile's
/// lists). Same slot-population contract as the shared arm.
#[inline]
pub fn file_private_data_mut<T>(file: &crate::fs::File) -> Option<&'static mut T> {
    // SAFETY: INV-PROC-NSFILE-1 mutable arm — identity-checked slot
    // read; the caller's lock discipline excludes concurrent aliases.
    let p = unsafe { *file.private_data.get() }?;
    Some(unsafe { &mut *(p as *mut T) })
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
pub fn journal_handle<H>(t: &Task) -> Option<&'static mut H> {
    let p = t.journal_handle.get() as *mut H;
    if p.is_null() {
        None
    } else {
        // SAFETY: INV-PROC-JOURNAL-1 — per-task exclusivity; the handle
        // is alive until the exit path clears the slot (the callee then
        // drops the reference before returning). The caller names the
        // handle type (service-crate-owned since rung-5).
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
            crate::mm::hotpath::switch_address_space(kernel_ppn, 0);
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
pub fn borrow_active_mm<'a>(mm_ptr: *const crate::mm::types::AddressSpace) -> &'a crate::mm::types::AddressSpace {
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

// ---- exec loader edges --------------------------------------------------
// The round-3b split left exec.rs with local "one audited edge" wrappers
// plus the raw walk/copy sites; the framework/service doctrine moves the
// whole family here (INV-PROC-EXEC-*). exec.rs keeps only loader logic.

/// Copy an image block into the fresh user address space through the
/// page-table walk (page-by-page, physical contiguity NOT assumed).
///
/// INV-PROC-EXEC-COPY-1: `root_ppn` is a valid user page-table root of
/// the in-construction address space; the range
/// `[vaddr, vaddr+src.len())` was just mapped by THIS loader and is
/// exclusively owned by it (unpublished — INV-LOCK-7). Returns false
/// when a page of the range has no mapping.
pub fn exec_copy_user_image(root_ppn: u64, vaddr: u64, src: &[u8]) -> bool {
    use crate::arch::mm::mm_ops::PageTableWalker;
    use crate::arch::mm::{phys_to_virt, PhysAddr, PAGE_SHIFT, PAGE_SIZE};

    let mut done = 0usize;
    while done < src.len() {
        let va = vaddr + done as u64;
        let in_page = (va as usize) & (PAGE_SIZE as usize - 1);
        // SAFETY: INV-PROC-EXEC-COPY-1 — the arch walker over this
        // loader's own fresh tree.
        let Some((ppn, pte)) = (unsafe { PageTableWalker::walk(root_ppn, va) }) else {
            return false;
        };
        if pte & 1 == 0 {
            return false;
        }
        let chunk = core::cmp::min(src.len() - done, PAGE_SIZE as usize - in_page);
        let kva = phys_to_virt(PhysAddr::new(ppn << PAGE_SHIFT)).bits() as usize + in_page;
        // SAFETY: INV-PROC-EXEC-COPY-1 — kva is the linear-map address
        // of a page-table-resolved frame of this address space; chunk is
        // bounded by the page.
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr().add(done), kva as *mut u8, chunk);
        }
        done += chunk;
    }
    true
}

/// One audited edge for the ELF program-header lookups (fs/elf.rs owns
/// the table contract).
///
/// INV-PROC-EXEC-ELF-1: `ehdr`/`data` come from a validated ELF image
/// and `index < e_phnum` at every call site (loops are bounded by
/// e_phnum or its derived phdr_count).
#[inline]
pub fn exec_phdr_at(
    ehdr: &crate::fs::elf::Elf64Ehdr,
    data: &[u8],
    index: usize,
) -> Option<crate::fs::elf::Elf64Phdr> {
    // SAFETY: INV-PROC-EXEC-ELF-1 — validated image, bounded index.
    unsafe { ehdr.get_program_header(data, index) }
}

/// One audited edge for the ELF-header validation of a file slice.
///
/// INV-PROC-EXEC-ELF-1 (header arm): `from_bytes` validates
/// magic/layout internally; None for malformed images.
#[inline]
pub fn exec_ehdr_from(data: &[u8]) -> Option<crate::fs::elf::Elf64Ehdr> {
    // SAFETY: INV-PROC-EXEC-ELF-1 — self-validating parser.
    unsafe { crate::fs::elf::Elf64Ehdr::from_bytes(data) }
}

/// The fresh-tree mapping allocator (mm/mod.rs).
///
/// INV-PROC-EXEC-MAP-1: `root` is a freshly allocated page-table root
/// owned by this loader; va/len describe a user range nothing else has
/// mapped; flags are valid user PTE bits. Returns the block's physical
/// base (never for linear writes — content goes through the walker).
#[inline]
pub fn exec_map_user_range(root: u64, va: u64, len: u64, flags: u64) -> Option<u64> {
    // SAFETY: INV-PROC-EXEC-MAP-1 — fresh unpublished tree, owned range.
    unsafe { crate::mm::hotpath::alloc_and_map_user_table(root, va, len, flags) }
}

/// Map a static physical frame (vDSO data/code) into the fresh tree.
///
/// INV-PROC-EXEC-VDSO-1: `root` is the freshly created page-table root;
/// `paddr..paddr+len` are the aligned static vDSO frames; flags are
/// valid user PTE bits.
#[inline]
pub fn exec_map_user_region_static(
    root: u64,
    va: u64,
    paddr: u64,
    len: u64,
    flags: u64,
) {
    // SAFETY: INV-PROC-EXEC-VDSO-1 — fresh root, aligned static frames.
    // Some arches return bool (mapped?), others () — the loader sites
    // ignore the result (the READBACK probes verify).
    let _ = unsafe { crate::arch::mm::mm_ops::map_user_region(root, va, paddr, len, flags) };
}

/// Debug readback of one user word through the page-table walk.
///
/// INV-PROC-EXEC-PROBE-1: `root` is the loader's fresh tree and `va` a
/// range it just mapped; the volatile load stays inside the resolved
/// page.
pub fn exec_probe_read_u64(root: u64, va: u64) -> Option<(u64, u64)> {
    use crate::arch::mm::mm_ops::PageTableWalker;
    // SAFETY: INV-PROC-EXEC-PROBE-1 — the loader's own fresh tree.
    let (ppn, _) = unsafe { PageTableWalker::walk(root, va)? };
    let kva = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(ppn as u64 * 4096))
        .bits() as usize;
    // SAFETY: INV-PROC-EXEC-PROBE-1 — the linear-map byte of the frame
    // resolved above, within the same page as va.
    let word = unsafe { core::ptr::read_volatile((kva + (va & 0xFFF) as usize) as *const u64) };
    Some((word, ppn))
}

/// Free the fresh, still-unpublished user page-table tree (the error
/// path's RAII guard).
///
/// INV-PROC-EXEC-FREETREE-1: `root_ppn` was allocated by
/// create_user_address_space and no MmStruct owns it yet (INV-LOCK-7:
/// no mm-side writers exist; the FRESH_TREE_PTE_LOCK serializes
/// munmap-side walkers).
pub fn exec_free_user_page_trees(root_ppn: u64) {
    // SAFETY: INV-PROC-EXEC-FREETREE-1 — unpublished tree, exactly-once
    // free from the owning guard.
    unsafe {
        crate::arch::mm::mmu_init::free_user_page_tables(
            &crate::mm::hotpath::FRESH_TREE_PTE_LOCK,
            root_ppn,
            &[],
        );
    }
}

/// Write the two-instruction signal trampoline words into the freshly
/// mapped trampoline page.
///
/// INV-PROC-EXEC-TRAMP-1: `kva` is the linear-map address of a frame
/// just mapped by this loader into the fresh tree; the two u32 stores
/// stay inside the page.
#[inline]
pub fn exec_write_sigtramp(kva: usize, w0: u32, w1: u32) {
    // SAFETY: INV-PROC-EXEC-TRAMP-1 — owned fresh frame, in-page stores.
    unsafe {
        core::ptr::write_volatile(kva as *mut u32, w0);
        core::ptr::write_volatile((kva + 4) as *mut u32, w1);
    }
}

/// Create the MmStruct for the finished image.
///
/// INV-PROC-EXEC-MMNEW-1: `user_ppn` is a freshly allocated page-table
/// root with no prior users; ownership transfers to the returned
/// MmStruct (its Drop tears the tree down exactly once).
#[inline]
pub fn exec_mm_new_user(user_ppn: u64) -> crate::mm::types::MmStruct {
    // SAFETY: INV-PROC-EXEC-MMNEW-1 — fresh root, ownership transfer.
    unsafe { crate::mm::types::MmStruct::new_user(user_ppn) }
}

/// The exec tail's satp/CR3 switch onto the new image.
///
/// INV-PROC-EXEC-SWITCH-1: `user_ppn` is the new image's page-table
/// root with the kernel half mapped (create_user_address_space links
/// it) and `asid` was drawn for this mm; runs before the old mm's Arc
/// drop so freeing the old tree is safe for this hart.
#[inline]
pub fn exec_switch_address_space(user_ppn: u64, asid: u16) {
    // SAFETY: INV-PROC-EXEC-SWITCH-1 — kernel half present on the new
    // root; ASID paired.
    unsafe { crate::mm::hotpath::switch_address_space(user_ppn, asid) }
}

/// Tighten the leaf PTE permission bits of every present leaf covering
/// [seg_start, seg_end) in the fresh, still-unpublished user tree.
///
/// INV-PROC-EXEC-PTE-1: the walk descends the fresh tree's own tables
/// (root allocated by this loader, unpublished — INV-LOCK-7); each
/// level's validity is checked before descending and only the leaf
/// permission bits are rewritten (PPN preserved).
/// Tighten the leaf PTE permission bits of every present leaf covering
/// [seg_start, seg_end) in the fresh, still-unpublished user tree
/// (INV-LOCK-7: no MmStruct owns it yet, so no TLB entries can exist).
/// PPNs are preserved — permission bits only. `seg_exec=false` sets NX
/// on the arches that express non-exec via an NX alias.
///
/// One audited edge replacing the four hand-rolled per-level walks that
/// used to live inline in the main-image and interpreter tightening
/// loops (same walk, same invariants, one place).
pub fn exec_set_leaf_flags_range(root: u64, seg_start: u64, seg_end: u64, seg_flags: u64, seg_exec: bool) {
    // seg_exec only carries meaning on arches that express non-exec via
    // an NX alias (x86_64); the riscv64 PTE layout has no NX bit.
    #[cfg(not(feature = "x86_64"))]
    let _ = seg_exec;
    #[cfg(feature = "riscv64")]
    {
        let mut vaddr = seg_start;
        while vaddr < seg_end {
            let vpn2 = ((vaddr >> 30) & 0x1FF) as usize;
            let vpn1 = ((vaddr >> 21) & 0x1FF) as usize;
            let vpn0 = ((vaddr >> 12) & 0x1FF) as usize;
            // SAFETY: the walk descends the fresh tree's own tables
            // (root allocated by this loader, unpublished — INV-LOCK-7);
            // each level's validity is checked before descending and only
            // the leaf permission bits are rewritten (PPN preserved).
            unsafe {
                let root_table = crate::arch::mm::mmu_init::get_page_table_virt(
                    root << crate::arch::mm::PAGE_SHIFT,
                );
                let pte2 = (*root_table).get(vpn2);
                if !pte2.is_valid() {
                    vaddr += crate::arch::mm::PAGE_SIZE as u64;
                    continue;
                }
                let table1 = crate::arch::mm::mmu_init::get_page_table_virt(
                    pte2.ppn() << crate::arch::mm::PAGE_SHIFT,
                );
                let pte1 = (*table1).get(vpn1);
                if !pte1.is_valid() {
                    vaddr += crate::arch::mm::PAGE_SIZE as u64;
                    continue;
                }
                let table0 = crate::arch::mm::mmu_init::get_page_table_virt(
                    pte1.ppn() << crate::arch::mm::PAGE_SHIFT,
                );
                let old_pte = (*table0).get(vpn0);
                if !old_pte.is_valid() {
                    vaddr += crate::arch::mm::PAGE_SIZE as u64;
                    continue;
                }
                let new_bits = (old_pte.bits() & !(0xFFu64)) | seg_flags;
                (*table0).set(
                    vpn0,
                    crate::arch::mm::PageTableEntry::from_bits(new_bits),
                );
            }
            vaddr += crate::arch::mm::PAGE_SIZE as u64;
        }
    }
    #[cfg(feature = "x86_64")]
    {
        let mut vaddr = seg_start;
        while vaddr < seg_end {
            // 4-level x86_64 walk (PML4/PDPT/PD/PT)
            let i4 = ((vaddr >> 39) & 0x1FF) as usize;
            let i3 = ((vaddr >> 30) & 0x1FF) as usize;
            let i2 = ((vaddr >> 21) & 0x1FF) as usize;
            let i1 = ((vaddr >> 12) & 0x1FF) as usize;
            // SAFETY: the walk descends the fresh tree's own tables
            // (root allocated by this loader, unpublished — INV-LOCK-7);
            // each level's validity is checked before descending and only
            // the leaf permission bits are rewritten (PPN preserved).
            unsafe {
                let root_table = crate::arch::mm::mmu_init::get_page_table_virt(
                    root << crate::arch::mm::PAGE_SHIFT,
                );
                let pte4 = (*root_table).get(i4);
                if !pte4.is_valid() {
                    vaddr += crate::arch::mm::PAGE_SIZE as u64;
                    continue;
                }
                let table3 = crate::arch::mm::mmu_init::get_page_table_virt(
                    pte4.ppn() << crate::arch::mm::PAGE_SHIFT,
                );
                let pte3 = (*table3).get(i3);
                if !pte3.is_valid() {
                    vaddr += crate::arch::mm::PAGE_SIZE as u64;
                    continue;
                }
                let table2 = crate::arch::mm::mmu_init::get_page_table_virt(
                    pte3.ppn() << crate::arch::mm::PAGE_SHIFT,
                );
                let pte2 = (*table2).get(i2);
                if !pte2.is_valid() || pte2.is_leaf() {
                    vaddr += crate::arch::mm::PAGE_SIZE as u64;
                    continue;
                }
                let table1 = crate::arch::mm::mmu_init::get_page_table_virt(
                    pte2.ppn() << crate::arch::mm::PAGE_SHIFT,
                );
                let old_pte = (*table1).get(i1);
                if !old_pte.is_valid() {
                    vaddr += crate::arch::mm::PAGE_SIZE as u64;
                    continue;
                }
                // x86 permission bits: clear W|U|NX, then set from the
                // semantic aliases (V=P, W=RW, U=US; R/X are no-ops —
                // non-exec is expressed via NX).
                let clear =
                    crate::arch::mm::PageTableEntry::W
                        | crate::arch::mm::PageTableEntry::U
                        | crate::arch::mm::PageTableEntry::NX;
                let mut new_bits = (old_pte.bits() & !clear) | seg_flags;
                if !seg_exec {
                    new_bits |= crate::arch::mm::PageTableEntry::NX;
                }
                (*table1).set(
                    i1,
                    crate::arch::mm::PageTableEntry::from_bits(new_bits),
                );
            }
            vaddr += crate::arch::mm::PAGE_SIZE as u64;
        }
    }
    #[cfg(feature = "aarch64")]
    {
        let mut vaddr = seg_start;
        while vaddr < seg_end {
            // SAFETY: user_ppn is the freshly created user root (unpublished
            // — INV-LOCK-7); the arch helper rewrites only present L3
            // leaves' permission bits, preserving the output address.
            unsafe {
                crate::arch::mm::mmu_init::set_user_leaf_flags(root, vaddr, seg_flags);
            }
            vaddr += crate::arch::mm::PAGE_SIZE as u64;
        }
    }
}

// ---- fork edges ----------------------------------------------------------
// The fork path's arch/FPU and page-table-backend edges (the CLONE_FPUs
// bank, the CLONE_CHILD_SETTID resolved write, the diagnostic COW walk).

/// Bank the CURRENT task's live FPU image (riscv64 save_fpu).
///
/// INV-PROC-FORK-FPU-1: called on the live current task in syscall
/// context — the callee reads+stores the running register image.
#[inline]
#[cfg(feature = "riscv64")]
pub fn fork_save_parent_fpu(t: &mut Task) {
    // SAFETY: INV-PROC-FORK-FPU-1 — current task, syscall context.
    unsafe { t.thread_mut().save_fpu() }
}

/// Bank the CURRENT task's live vector state (riscv64 save_vstate).
///
/// INV-PROC-FORK-FPU-1 (vector arm): same current-task contract; the
/// child observes the parent's vector values (POSIX).
#[inline]
#[cfg(feature = "riscv64")]
pub fn fork_save_parent_vstate(t: &mut Task) {
    // SAFETY: INV-PROC-FORK-FPU-1 — current task, syscall context.
    unsafe { t.thread_mut().save_vstate() }
}

/// xsave the current task's live FPU image into its own bank.
///
/// INV-PROC-FORK-FPU-1 (x86 arm): TS is clear, so the live registers
/// belong to `t` (the current task).
#[inline]
#[cfg(feature = "x86_64")]
pub fn fork_save_parent_fpu(t: &mut Task) {
    // SAFETY: INV-PROC-FORK-FPU-1 — current task, TS clear.
    unsafe { t.thread_mut().fpu_save_for_switch() }
}

/// riscv64's fpu_save_for_switch twin (the riscv arm of the same
/// contract).
#[inline]
#[cfg(feature = "riscv64")]
pub fn fork_fpu_save_for_switch(t: &mut Task) {
    // SAFETY: INV-PROC-FORK-FPU-1 — current task's own image.
    unsafe { t.thread_mut().fpu_save_for_switch() }
}

/// The arch page-table walk backend (fork's CoW/resolved-write paths).
///
/// INV-PROC-FORK-WALK-1: `root` is the child's fresh root ppn and `va`
/// a validated user address of the in-construction child (nothing else
/// observes the tree).
#[inline]
pub fn fork_pt_walk(root: u64, va: u64) -> Option<(u64, u64)> {
    // SAFETY: INV-PROC-FORK-WALK-1 — fresh child tree, validated va.
    unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) }
}

/// The arch CoW fault backend (fork's CLONE_CHILD_SETTID pre-resolve).
///
/// INV-PROC-FORK-COW-1: child_root is the child's fresh root ppn; va is
/// access_ok-validated (the child's CLONE_CHILD_SETTID argument).
pub fn fork_handle_cow_fault(
    mm: &crate::mm::types::MmStruct,
    root: u64,
    va: u64,
) -> crate::arch::mm::mm_ops::CowFaultResult {
    // SAFETY: INV-PROC-FORK-COW-1 — fresh root, validated user va.
    unsafe {
        crate::arch::mm::mm_ops::handle_cow_fault(
            mm,
            root,
            // The arch memory-layout VirtAddr twin (Sv39 sign-extension on
            // riscv64, plain on x86_64/aarch64).
            crate::arch::mm::memory_layout::VirtAddr::new(va),
        )
    }
}

/// The resolved child-tid write through the linear map.
///
/// INV-PROC-FORK-TIDWRITE-1: `kva` is the linear-map address of the
/// child's page holding child_tid, resolved by fork_pt_walk; the child
/// has never run, so no concurrent writer exists and no TLB can hold a
/// stale entry.
#[inline]
pub fn fork_write_child_tid(kva: usize, tid: i32) {
    // SAFETY: INV-PROC-FORK-TIDWRITE-1 — owned unrun child page.
    unsafe { core::ptr::write_volatile(kva as *mut i32, tid) }
}

/// Read the user TP register (aarch64's tpidr_el0).
///
/// INV-PROC-DIAG-1 (reg arm): pure register read; no memory or flags
/// effects.
#[inline]
#[cfg(feature = "aarch64")]
pub fn read_user_tp_register() -> u64 {
    let live_tp: u64;
    // SAFETY: INV-PROC-DIAG-1 — a single mrs with nomem/nostack/
    // preserves_flags.
    unsafe {
        core::arch::asm!(
            "mrs {}, tpidr_el0",
            out(reg) live_tp,
            options(nomem, nostack, preserves_flags)
        );
    }
    live_tp
}

/// Read a page descriptor's refcount (the COW-share diagnostic).
///
/// INV-PROC-DIAG-1 (page arm): `pg` is a page-descriptor pointer from
/// pfn_to_page of a walk-resolved frame (or null — None); read-only.
#[inline]
pub fn page_refcount(pg: *mut crate::mm::types::Page) -> Option<i32> {
    if pg.is_null() {
        return None;
    }
    // SAFETY: INV-PROC-DIAG-1 — non-null page descriptor, plain read.
    Some(unsafe { (*pg).refcount() })
}

// ---- wait-queue statics -------------------------------------------------
// WaitQueueHead's raw *mut Task entries are lock-guarded; the Send/Sync
// proofs live here so wait.rs carries zero unsafe.

// SAFETY: WaitQueueHead uses internal Mutex/Spinlock synchronization;
// the *mut Task entries are only dereferenced under the queue lock and
// the scheduler's park/wake protocol.
unsafe impl Send for crate::process::wait::WaitQueueHead {}
// SAFETY: same lock discipline across CPUs.
unsafe impl Sync for crate::process::wait::WaitQueueHead {}

// ---- pid-hash chain edges -----------------------------------------------
// The bucket chains are raw singly-linked Task lists whose link words
// alias the bucket-head AtomicPtr on the first hop; every mutation
// runs under the per-bucket irqsave lock.

/// The RCU-style unlink store through a previous link slot.
///
/// INV-PROC-PIDHASH-1: the caller holds the bucket's irqsave lock (the
/// only writer); `prev_link` aliases either the bucket-head AtomicPtr
/// or a chained task's plain `pid_hash_next` word — the single-writer
/// store under the lock is the documented protocol, and concurrent RCU
/// readers observe either the old or the new aligned word, never a
/// torn one.
#[inline]
pub fn pid_hash_unlink_store(prev_link: *mut *mut Task, next: *mut Task) {
    // SAFETY: INV-PROC-PIDHASH-1 — bucket-lock-held single-writer store
    // to an aligned link word.
    unsafe { *prev_link = next }
}

// ---- ptrace edges --------------------------------------------------------
// The debugger's target-memory access family: page-table translate
// under the target mm's PTE lock, linear-map word/chunk moves, the
// icache coherence sequence, and the gregset frame copies.

/// Walk the TARGET mm's page table for `va` (under its PTE lock).
///
/// INV-PROC-PTRACE-WALK-1: `mm` is the pinned target mm whose root the
/// walk uses; the lock serializes against same-mm munmap leaf-table
/// unlinks (INV-LOCK-7 — the walker dereferences PMD-resolved leaf
/// tables). Returns (phys, page_len) or None when unmapped.
pub fn ptrace_translate(mm: &crate::mm::types::MmStruct, va: u64) -> Option<(u64, u64)> {
    use crate::arch::mm::mm_ops::PageTableWalker;
    let root_ppn = mm.root_ppn();
    let _pte_guard = mm.pte_lock().lock();
    // SAFETY: INV-PROC-PTRACE-WALK-1 — pinned target root; the walker
    // bounds-checks every entry.
    let (ppn, _) = unsafe { PageTableWalker::walk(root_ppn, va) }?;
    Some((ppn << 12, 4096))
}

/// Volatile u64 read from the kernel linear map of a target user page.
///
/// INV-PROC-PTRACE-MEM-1: `kva` was resolved by [`ptrace_translate`]
/// for an in-page, word-aligned offset of the pinned target.
#[inline]
pub fn ptrace_read_word(kva: usize) -> u64 {
    // SAFETY: INV-PROC-PTRACE-MEM-1 — resolved in-page linear address.
    unsafe { core::ptr::read_volatile(kva as *const u64) }
}

/// Volatile u64 write into the kernel linear map of a target user page
/// (the POKETEXT path — bypasses the target PTE write permission by
/// design, exactly what a debugger needs for R+X text).
///
/// INV-PROC-PTRACE-MEM-1 (write arm).
#[inline]
pub fn ptrace_write_word(kva: usize, value: u64) {
    // SAFETY: INV-PROC-PTRACE-MEM-1 — resolved in-page linear address.
    unsafe { core::ptr::write_volatile(kva as *mut u64, value) }
}

/// One chunk copy between a kernel buffer and the linear map of a
/// target user page (process_vm_readv/writev core).
///
/// INV-PROC-PTRACE-MEM-1 (chunk arm): `lin` is the resolved in-page
/// linear address; `kbuf..kbuf+chunk` is the caller's kernel buffer;
/// the two ranges cannot overlap (user mapping vs kernel buffer).
#[inline]
pub fn ptrace_copy_chunk(lin: *mut u8, kbuf: *mut u8, chunk: usize, to_remote: bool) {
    // SAFETY: INV-PROC-PTRACE-MEM-1 — disjoint resolved ranges, chunk
    // bounded by the page remainder.
    unsafe {
        if to_remote {
            core::ptr::copy_nonoverlapping(kbuf as *const u8, lin, chunk);
        } else {
            core::ptr::copy_nonoverlapping(lin as *const u8, kbuf, chunk);
        }
    }
}

/// Make a text write coherent before subsequent execution.
///
/// INV-PROC-PTRACE-ICOH-1: the standard per-arch text-write coherence
/// sequence (fence/sfence.vma/fence.i; dc cvau/ic ivau/dsb/isb; x86 is
/// cache-coherent) — register operands only.
#[inline]
pub fn ptrace_flush_icache(addr: u64) {
    #[cfg(feature = "riscv64")]
    // SAFETY: INV-PROC-PTRACE-ICOH-1 — the riscv sequence.
    unsafe {
        core::arch::asm!(
            "fence rw, rw",
            "sfence.vma {0}, zero",
            "fence.i",
            in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
    #[cfg(feature = "x86_64")]
    {
        let _ = addr;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }
    #[cfg(feature = "aarch64")]
    // SAFETY: INV-PROC-PTRACE-ICOH-1 — the arm64 sequence.
    unsafe {
        core::arch::asm!(
            "dc cvau, {0}",
            "ic ivau, {0}",
            "dsb ish",
            "isb",
            in(reg) addr,
            options(nostack, preserves_flags)
        );
    }
}

/// Copy the tracee's gregset (32 u64 GP words) out of its saved trap
/// frame.
///
/// INV-PROC-PTRACE-GREG-1: `regs` is the pinned-and-stopped target's
/// live trap frame (quiescent while stopped); the first 256 bytes are
/// the 32 general-purpose words (the user_regs_struct layout).
#[inline]
pub fn ptrace_read_gregset(regs: *mut crate::arch::pt_regs::PtRegs, out: &mut [u8]) -> bool {
    if regs.is_null() {
        return false;
    }
    // SAFETY: INV-PROC-PTRACE-GREG-1 — quiescent frame, fixed 256-byte
    // head copy.
    unsafe {
        core::ptr::copy_nonoverlapping(regs as *const u8, out.as_mut_ptr(), 256);
    }
    true
}

/// Write the gregset back into the tracee's trap frame.
///
/// INV-PROC-PTRACE-GREG-1 (write arm): only GP words written; the
/// target is pinned and stopped.
#[inline]
pub fn ptrace_write_gregset(regs: *mut crate::arch::pt_regs::PtRegs, from: &[u8]) -> bool {
    if regs.is_null() {
        return false;
    }
    let n = from.len().min(256);
    // SAFETY: INV-PROC-PTRACE-GREG-1 — quiescent frame, GP-word head
    // write.
    unsafe {
        core::ptr::copy_nonoverlapping(from.as_ptr(), regs as *mut u8, n);
    }
    true
}

/// Zeroed `SigInfo` for the SETSIGINFO read-in path.
///
/// INV-PROC-DIAG-1 (zeroed arm): SigInfo is a plain-C layout of integer
/// words; an all-zero image is a valid (if uninformative) siginfo — the
/// same construction the reference kernel's copy_from_user target uses.
#[inline]
pub fn zeroed_siginfo() -> crate::signal::SigInfo {
    // SAFETY: INV-PROC-DIAG-1 — all-zero bit pattern is valid for the
    // integer-only C layout.
    unsafe { core::mem::zeroed() }
}
