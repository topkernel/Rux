//! IPC-lane framework edges — the audited unsafe boundary
//! (mainline minimal-first consolidation, 2026-11-27).
//!
//! Doctrine (the FRAMEWORK/SERVICE split, same as fs/unsafe.rs and
//! process/unsafe.rs): every service file of this subsystem (binder.rs,
//! sysv_*.rs, posix_mq.rs, keys.rs, util.rs, access_token.rs) carries
//! ZERO `unsafe` keywords — checkable by grep, enforced by a file-level
//! `#![forbid(unsafe_code)]`. Every genuinely-unsafe operation they
//! need lives HERE as a named edge with an INV contract, in the shape
//! of `kernel/src/safety/` (one audit per edge FAMILY, not per call
//! site).
//!
//! Edge inventory (families):
//!
//! * User-memory copies — INV-IPC-UACCESS-1: thin forwarding edges over
//!   the arch `uaccess` exception-table primitives. The caller proves
//!   the access_ok validation; the edge only removes the per-call-site
//!   `unsafe` keyword. Returns the raw primitive's own result code so
//!   the call sites' fault handling is byte-identical to the
//!   pre-conversion code.
//! * Binder block views — INV-BINDER-BLOCKVIEW: volatile reads/writes,
//!   typed POD views and byte windows into an exclusively-owned
//!   transaction block. The caller proves `span_ok` (bounds) and the
//!   alignment check before calling; the edge performs the raw access.
//! * Binder held-cells — INV-BINDER-HELD: UnsafeCell accessors whose
//!   only mutator discipline is the WORLD spinlock. The call sites keep
//!   passing the `&World` token whose borrow is the compiler-checked
//!   proof; the edge states the obligation once.
//! * Binder private-data — INV-BINDER-PRIV: the single
//!   `Arc::into_raw(BinderProc)` window installed at open, its borrow
//!   twin (ManuallyDrop, count unchanged) and its exactly-once take.
//! * Binder diagnostics — INV-BINDER-ENTRYFRAME / INV-BINDER-PROBE /
//!   INV-BINDER-STACKWIN / INV-BINDER-WORLDSEC / INV-BINDER-DIAG-COMM:
//!   the read-only diagnostic helpers moved here wholesale (they were
//!   file-private fns); call sites are unchanged apart from the module
//!   path.
//! * Kernel-memory primitives: zeroing a fresh frame, raw slice views,
//!   shared/exclusive raw borrows, page-table walks and the two
//!   eager-map+refcount edges (binder mmap, sysv shmat) with their
//!   PTE-lock obligations.
//! * Send/Sync commitments for the five binder shared types (moved
//!   verbatim from binder.rs; each carries its original SAFETY note).
//!
//! This module is Framework: it may use `unsafe` and is part of the
//! audited TCB. Nothing here grows domain logic — IPC semantics stay
//! in the service files; only the pointer discipline lives here.

use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::mem::ManuallyDrop;

// ===========================================================================
// INV-IPC-UACCESS-1: user-memory copies (exception-table primitives)
// ===========================================================================
//
// The caller proves: the user pointer/length pair was access_ok-validated
// for exactly this transfer before the call. The edges forward to the
// arch raw primitives and return the primitive's own result code
// (0 = fully copied; nonzero = bytes NOT copied / fault), so call-site
// fault handling is unchanged.

/// Copy `len` bytes to user `dst_user` from kernel `src`.
#[inline]
pub fn copy_to_user_bytes(dst_user: *mut u8, src: *const u8, len: usize) -> usize {
    // SAFETY: INV-IPC-UACCESS-1 — caller validated the user range.
    unsafe { crate::arch::uaccess::copy_to_user(dst_user, src, len) }
}

/// Copy `len` bytes from user `src_user` into kernel `dst`.
#[inline]
pub fn copy_from_user_bytes(dst: *mut u8, src_user: *const u8, len: usize) -> usize {
    // SAFETY: INV-IPC-UACCESS-1 — caller validated the user range.
    unsafe { crate::arch::uaccess::copy_from_user(dst, src_user, len) }
}

/// Offset a raw pointer by `n` elements (the msgp+8 family). Caller
/// obligation: the result stays within the same validated buffer
/// (access_ok covered msgsz+8, so +8 is in the same user region).
#[inline]
pub fn ptr_offset<T>(p: *mut T, n: usize) -> *mut T {
    // SAFETY: INV-IPC-UACCESS-1 — caller-proven in-buffer offset.
    unsafe { p.add(n) }
}

/// Store one `Copy` value to user memory (put_user twin).
#[inline]
pub fn put_user_val<T: Copy>(dst_user: *mut T, value: T) -> bool {
    // SAFETY: INV-IPC-UACCESS-1 — caller validated the user range.
    unsafe { crate::arch::uaccess::put_user(dst_user, value) }
}

// ===========================================================================
// Generic raw-borrow / cell / Arc family
// ===========================================================================

/// INV-IPC-RAWREF-1: shared reborrow of a raw pointer. The caller
/// proves the pointee is alive and not being mutated for the borrow's
/// duration (fd-backed private data, a pinned page descriptor, a
/// pinned task page).
#[inline]
pub fn raw_ref<'a, T>(p: *const T) -> &'a T {
    debug_assert!(!p.is_null());
    // SAFETY: INV-IPC-RAWREF-1 (caller-proven liveness/aliasing).
    unsafe { &*p }
}

/// INV-IPC-RAWREF-1 (exclusive half): mutable reborrow of a raw
/// pointer. As [`raw_ref`], plus the caller's exclusion argument (WORLD
/// held, single owner, or &mut provenance).
#[inline]
pub fn raw_mut<'a, T>(p: *mut T) -> &'a mut T {
    debug_assert!(!p.is_null());
    // SAFETY: INV-IPC-RAWREF-1 exclusive half.
    unsafe { &mut *p }
}

/// INV-BINDER-HELD edge: mutable access to a WORLD-disciplined
/// UnsafeCell. Caller obligation: the WORLD spinlock is held — the call
/// sites pass the `&World` token (`_held`) whose borrow is that proof;
/// this edge states the discipline once for all three st()/im() sites.
#[inline]
#[allow(clippy::mut_from_ref)]
pub fn cell_mut<'a, T>(c: &UnsafeCell<T>) -> &'a mut T {
    // SAFETY: INV-BINDER-HELD — WORLD held (token proof at the call
    // site); single mutator at a time.
    unsafe { &mut *c.get() }
}

/// INV-BINDER-HELD edge: copy out of a WORLD-disciplined UnsafeCell
/// holding a `Copy` value (the private_data Option slots).
#[inline]
pub fn cell_copy<T: Copy>(c: &UnsafeCell<T>) -> T {
    // SAFETY: INV-BINDER-HELD — the read needs no exclusivity for a
    // Copy load; the slot is only written under WORLD/open-close paths.
    unsafe { *c.get() }
}

/// INV-BINDER-HELD edge: store into a WORLD-disciplined UnsafeCell
/// (the close path clearing private_data).
#[inline]
pub fn cell_set<T>(c: &UnsafeCell<T>, v: T) {
    // SAFETY: INV-BINDER-HELD — single mutator (the close path owns the
    // slot; no other reference observes the intermediate state).
    unsafe { *c.get() = v }
}

/// INV-BINDER-PRIV edge: adopt a raw Arc pointer WITHOUT changing the
/// reference count (ManuallyDrop wrapper; never dropped). The pointer
/// must be an `Arc::into_raw` product whose count the caller's clone
/// (or the fd's ownership) keeps alive for the borrow.
#[inline]
pub fn arc_borrow<T>(p: *const T) -> ManuallyDrop<Arc<T>> {
    // SAFETY: INV-BINDER-PRIV — Arc::into_raw product; ManuallyDrop
    // never drops, so the count is unchanged.
    unsafe { ManuallyDrop::new(Arc::from_raw(p)) }
}

/// INV-BINDER-PRIV edge: reclaim the one strong reference behind an
/// `Arc::into_raw` pointer (exactly-once take; the caller retires the
/// slot so no second take can observe it).
#[inline]
pub fn arc_take<T>(p: *const T) -> Arc<T> {
    // SAFETY: INV-BINDER-PRIV — the into_raw product's single strong
    // reference being reclaimed exactly once.
    unsafe { Arc::from_raw(p) }
}

/// INV-BINDER-WORLDSEC edge: rebuild the `&'static Location` behind a
/// WORLD_SEC table key. The table only ever stores `&'static Location`
/// references captured at section open (`Location::caller()`), so the
/// key is a valid static reference for the process lifetime.
#[inline]
pub fn location_from_key(key: usize) -> &'static core::panic::Location<'static> {
    // SAFETY: INV-BINDER-WORLDSEC — keys are &'static Location refs
    // stored at open time; the table holds no other values.
    unsafe { &*(key as *const core::panic::Location<'static>) }
}

/// Zero a fresh kernel-memory region. Caller obligation: `p` denotes
/// `len` writable bytes of a freshly allocated (unaliased) region.
#[inline]
pub fn mem_zero(p: *mut u8, len: usize) {
    // SAFETY: caller-proven fresh writable region (buddy allocation).
    unsafe { core::ptr::write_bytes(p, 0, len) }
}

/// INV-IPC-RAWREF-1 edge: reinterpret the first `len` elements behind a
/// raw pointer as a shared slice. Caller proves the whole range is
/// inside one live allocation (here: the direct-map view of one page).
#[inline]
pub fn slice_from_raw<'a, T>(p: *const T, len: usize) -> &'a [T] {
    // SAFETY: caller-proven in-allocation range.
    unsafe { core::slice::from_raw_parts(p, len) }
}

/// Bytewise view of a `Copy` repr(C) POD for staging into a read
/// buffer (INV-BINDER-STAGE-POD): reading size_of::<T>() bytes through
/// a valid &T is a view of the object's own representation; T has no
/// drop glue. (Call sites assert the layout with const asserts.)
#[inline]
pub fn pod_bytes<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: INV-BINDER-STAGE-POD — T is Copy POD; the byte range is
    // the object's own representation behind a live reference.
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) }
}

// ===========================================================================
// INV-BINDER-BLOCKVIEW: the transaction-block access family
// ===========================================================================
//
// A TxnBlock owns `base: *mut u8` — the exclusively-owned in-flight
// block carved from the binder mmap. Every edge here requires the
// caller to have proven, before calling:
//   (a) `abs .. abs+width` is inside the block (`span_ok`), and
//   (b) the access alignment (native or caller-checked), and
//   (c) no other live reference aliases the range (the TxnBlock is the
//       sole window; mutation goes through &mut self or WORLD).
// The edges perform the raw access; the audit is this block, once.

/// Mutable byte window `[abs, abs+len)` into the block (caller-supplied
/// copy destination; folded into uaccess_safe::copy_in by the caller).
#[inline]
pub fn block_window<'a>(base: *mut u8, abs: usize, len: usize) -> &'a mut [u8] {
    // SAFETY: INV-BINDER-BLOCKVIEW (a)+(c) — in-bounds sole window.
    unsafe { core::slice::from_raw_parts_mut(base.add(abs), len) }
}

/// Volatile-read one u32 at block offset `abs`.
#[inline]
pub fn block_read_u32(base: *const u8, abs: usize) -> u32 {
    // SAFETY: INV-BINDER-BLOCKVIEW (a)+(b): span_ok + 4-alignment.
    unsafe { core::ptr::read_volatile(base.add(abs) as *const u32) }
}

/// Volatile-write one u32 at block offset `abs`.
#[inline]
pub fn block_write_u32(base: *mut u8, abs: usize, v: u32) {
    // SAFETY: INV-BINDER-BLOCKVIEW (a)+(b): span_ok + 4-alignment.
    unsafe { core::ptr::write_volatile(base.add(abs) as *mut u32, v) }
}

/// Volatile-read one u64 at block offset `abs`.
#[inline]
pub fn block_read_u64(base: *const u8, abs: usize) -> u64 {
    // SAFETY: INV-BINDER-BLOCKVIEW (a)+(b): span_ok + 8-alignment.
    unsafe { core::ptr::read_volatile(base.add(abs) as *const u64) }
}

/// Volatile-write one u64 at block offset `abs`.
#[inline]
pub fn block_write_u64(base: *mut u8, abs: usize, v: u64) {
    // SAFETY: INV-BINDER-BLOCKVIEW (a)+(b): span_ok + 8-alignment.
    unsafe { core::ptr::write_volatile(base.add(abs) as *mut u64, v) }
}

/// Volatile-read one `Copy` POD at block offset `abs` (binder object
/// headers; size/alignment caller-checked against the object size).
#[inline]
pub fn block_read_obj<T: Copy>(base: *const u8, abs: usize) -> T {
    // SAFETY: INV-BINDER-BLOCKVIEW (a)+(b); T is Copy (no drop glue
    // half-value hazard on a by-value volatile read).
    unsafe { core::ptr::read_volatile(base.add(abs) as *const T) }
}

/// Volatile-write one `Copy` POD at block offset `abs`.
#[inline]
pub fn block_write_obj<T: Copy>(base: *mut u8, abs: usize, v: T) {
    // SAFETY: INV-BINDER-BLOCKVIEW (a)+(b)+(c): exclusive block.
    unsafe { core::ptr::write_volatile(base.add(abs) as *mut T, v) }
}

/// Materialize `&mut T` at block offset `abs` for field rewrites
/// (FlatBinderObject translation). T is a plain repr(C) struct (any bit
/// pattern valid).
#[inline]
pub fn block_ref_mut<'a, T>(base: *mut u8, abs: usize) -> &'a mut T {
    // SAFETY: INV-BINDER-BLOCKVIEW (a)+(b)+(c); sole reference.
    unsafe { &mut *(base.add(abs) as *mut T) }
}

/// Copy `bytes` (+ NUL) into the block at offset `abs`. Caller proved
/// `abs + bytes.len() + 1` in-bounds (the secctx reservation).
#[inline]
pub fn block_copy_bytes_nul(base: *mut u8, abs: usize, bytes: &[u8]) {
    // SAFETY: INV-BINDER-BLOCKVIEW (a)+(c) — dst in the exclusive
    // block, src a shared slice, ranges disjoint.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(abs), bytes.len());
        core::ptr::write_volatile(base.add(abs + bytes.len()), 0u8);
    }
}

// ===========================================================================
// Kernel-memory / page-table edges
// ===========================================================================

/// INV-BINDER-PROBE edge: read-only walk of live user page tables.
/// Returns (ppn, pte-bits) of the leaf, None when unmapped. The walk
/// never writes; callers use it only for diagnostics.
#[inline]
pub fn pt_walk(root: u64, va: u64) -> Option<(u64, u64)> {
    // SAFETY: read-only diagnostic walk of live user page tables
    // (INV-BINDER-PROBE); the walker keeps its own locking contract.
    unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) }
}

/// Binder mmap eager-install edge: map one frame into the reserved VMA
/// and take the mapping reference on its descriptor. Caller obligation
/// (INV-BINDER-MMAP-1): PTE_MODIFY_LOCK is held (the caller's guard),
/// `phys` is a fresh contiguous buddy frame this VMA owns, and
/// (va, pa) are page-aligned.
pub fn map_frame_take_ref(root: u64, va: u64, pa: u64, flags: u64) {
    // SAFETY: root is the task's page-table root; phys..phys+PAGE is
    // our fresh contiguous allocation, mapped page-aligned under the
    // PTE lock held by the caller. (map_page is an unsafe fn only on
    // aarch64; the other arches expose it as a safe wrapper of this
    // same contract.)
    #[cfg(feature = "aarch64")]
    let _ = unsafe {
        crate::arch::mm::map_page(
            root,
            crate::arch::mm::VirtAddr::new(va),
            crate::arch::mm::PhysAddr::new(pa),
            flags,
        )
    };
    #[cfg(not(feature = "aarch64"))]
    let _ = crate::arch::mm::map_page(
        root,
        crate::arch::mm::VirtAddr::new(va),
        crate::arch::mm::PhysAddr::new(pa),
        flags,
    );
    let page = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(pa as usize));
    if !page.is_null() {
        // SAFETY: descriptor exists for this in-RAM frame (INV-FRAME-1).
        unsafe { (*page).get_page() };
    }
}

/// sysv shmat install edge: map one zeroed segment page into the
/// attaching address space under the PTE lock and take the
/// per-attachment reference. Caller obligation (INV-SHM-ATTACH-1):
/// `vaddr` is page-aligned and inside the newly allocated VMA range;
/// `phys` is a valid page from get_zeroed_page; `root` is the current
/// process's page-table root.
pub fn shm_map_page_take_ref(root: u64, vaddr: u64, phys: u64, flags: u64) {
    // R7-A5: map+refcount under the PTE lock (same discipline as the
    // demand-fault paths).
    crate::arch::mm::mm_ops::MM_CTX.store(1, core::sync::atomic::Ordering::Relaxed);
    #[cfg(feature = "x86_64")]
    let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
    #[cfg(not(feature = "x86_64"))]
    let _pte_guard = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock_irqsave();
    // SAFETY: INV-SHM-ATTACH-1 — caller-proven alignment/VMA/frame.
    let _ = crate::arch::mm::map_user_page(
        root,
        crate::arch::mm::memory_layout::VirtAddr::new(vaddr),
        crate::arch::mm::memory_layout::PhysAddr::new(phys),
        flags,
    );
    // The new PTE references the segment page: take a per-attachment
    // reference so shmdt/exit's put_page cannot free a page the segment
    // still owns (review 2R.6).
    let page = crate::mm::pfn_to_page_mut(crate::mm::phys_to_pfn(phys as usize));
    if !page.is_null() {
        // SAFETY: descriptor exists for this in-RAM page.
        unsafe { (*page).get_page() };
    }
    drop(_pte_guard);
}

// ===========================================================================
// Binder diagnostics (moved wholesale from binder.rs; call sites only
// changed module path). INV-BINDER-ENTRYFRAME / -PROBE / -STACKWIN.
// ===========================================================================

/// Diagnostic entry-frame read: (pc, syscall-no, a0, a1, a2).
/// Zeros when `top == 0` (see the INV-BINDER-ENTRYFRAME header).
#[inline]
pub fn entry_frame(top: usize) -> (u64, u64, u64, u64, u64) {
    if top == 0 {
        return (0, 0, 0, 0, 0);
    }
    #[cfg(feature = "x86_64")]
    {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: diagnostic read of this task's own entry frame; the
        // caller obtained `top` from kernel_stack_top_checked() (or the
        // owns_kernel_stack() gate), so the frame pages are mapped.
        unsafe { ((*regs).rip, (*regs).orig_rax, (*regs).rdi, (*regs).rsi, (*regs).rdx) }
    }
    #[cfg(feature = "riscv64")]
    {
        let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::riscv64::pt_regs::PtRegs;
        // SAFETY: same contract as the x86_64 arm.
        unsafe { ((*regs).epc, (*regs).a7, (*regs).a0, (*regs).a1, (*regs).a2) }
    }
    #[cfg(feature = "aarch64")]
    {
        let regs = (top.wrapping_sub(crate::arch::aarch64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::aarch64::pt_regs::PtRegs;
        // SAFETY: same contract as the x86_64 arm.
        unsafe { ((*regs).pc, (*regs).syscallno, (*regs).regs[0], (*regs).regs[1], (*regs).regs[2]) }
    }
    #[cfg(not(any(feature = "x86_64", feature = "riscv64", feature = "aarch64")))]
    (0, 0, 0, 0, 0)
}

/// Diagnostic entry-frame read of the user stack pointer + pc: (sp, pc).
/// Zeros when `top == 0` (see the INV-BINDER-ENTRYFRAME header).
#[inline]
pub fn entry_frame_sp(top: usize) -> (u64, u64) {
    if top == 0 {
        return (0, 0);
    }
    #[cfg(feature = "x86_64")]
    {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: as entry_frame.
        unsafe { ((*regs).rsp, (*regs).rip) }
    }
    #[cfg(feature = "riscv64")]
    {
        let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::riscv64::pt_regs::PtRegs;
        // SAFETY: as entry_frame.
        unsafe { ((*regs).sp, (*regs).epc) }
    }
    #[cfg(feature = "aarch64")]
    {
        let regs = (top.wrapping_sub(crate::arch::aarch64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::aarch64::pt_regs::PtRegs;
        // SAFETY: as entry_frame.
        unsafe { ((*regs).sp, (*regs).pc) }
    }
    #[cfg(not(any(feature = "x86_64", feature = "riscv64", feature = "aarch64")))]
    (0, 0)
}

/// x86-only entry-frame triple the futex decode needs: (rbp, rsp, r10).
/// INV-BINDER-ENTRYFRAME arm; zeros when `top == 0`.
#[cfg(feature = "x86_64")]
#[inline]
pub fn entry_frame_futex_x86(top: usize) -> (u64, u64, u64) {
    if top == 0 {
        return (0, 0, 0);
    }
    let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
        as *const crate::arch::x86_64::pt_regs::PtRegs;
    // SAFETY: diagnostic read of this task's own entry frame; the
    // caller gated the stack via owns_kernel_stack/kernel_stack_top_checked.
    unsafe { ((*regs).rbp, (*regs).rsp, (*regs).r10) }
}

/// Diagnostic leaf walk: (ppn, pte-bits) of the mapping at `va` in the
/// target's live user page tables. INV-BINDER-PROBE: read-only, the one
/// audited walk span all probes build on.
#[inline]
pub fn probe_leaf(root: u64, va: u64) -> Option<(u64, u64)> {
    pt_walk(root, va)
}

/// Diagnostic cross-address-space probe: walk the target's live user
/// page tables and volatile-read one u32 from the mapped page. Returns
/// None when the page is unmapped. INV-BINDER-PROBE: read-only and
/// fault-free — the leaf is walked first and the read happens only
/// through the walked, present mapping.
#[inline]
pub fn probe_u32(root: u64, addr: u64) -> Option<u32> {
    pt_walk(root, addr).map(|(ppn, _)| {
        let pa = (ppn as usize) << 12;
        let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64)).bits()
            as usize
            + (addr as usize & 0xfff);
        // SAFETY: the walked leaf maps this user word.
        unsafe { core::ptr::read_volatile(kv as *const u32) }
    })
}

/// u64 twin of [`probe_u32`] (INV-BINDER-PROBE).
#[inline]
pub fn probe_u64(root: u64, addr: u64) -> Option<u64> {
    pt_walk(root, addr).map(|(ppn, _)| {
        let pa = (ppn as usize) << 12;
        let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64)).bits()
            as usize
            + (addr as usize & 0xfff);
        // SAFETY: the walked leaf maps this user word.
        unsafe { core::ptr::read_volatile(kv as *const u64) }
    })
}

/// Diagnostic kernel-stack word read: bounds-check `addr` inside the
/// task's own kernel-stack window [base, top) and volatile-read one u64.
/// INV-BINDER-STACKWIN: the only raw kernel-stack-window reads in the
/// diagnostics. (The pre-conversion scans bounds-checked only the
/// FIRST word and then walked up to 640 more — reads past `top` were
/// never part of the documented "inside the task's own kernel-stack
/// window" contract; the per-word check enforces it.)
#[inline]
pub fn stack_word(addr: usize, base: usize, top: usize) -> Option<u64> {
    if addr < base || addr + 8 > top || addr & 7 != 0 {
        return None;
    }
    // SAFETY: addr is inside the task's own kernel-stack window,
    // 8-byte aligned, and the full word fits below `top`.
    Some(unsafe { core::ptr::read_volatile(addr as *const u64) })
}

// ===========================================================================
// Send/Sync commitments (moved verbatim from binder.rs by the 2026-11-27
// mainline consolidation; each carries its original SAFETY note — the
// per-field lock discipline arguments live with the type definitions,
// and these impls add no field access of their own).
// ===========================================================================

// SAFETY: binder state is shared across CPUs strictly under the WORLD
// spinlock; the wait queues are internally synchronized; the kvirt raw
// pointer is only dereferenced under WORLD while the mmap is alive.
unsafe impl Send for crate::ipc::binder::BinderAlloc {}
unsafe impl Sync for crate::ipc::binder::BinderAlloc {}
unsafe impl Send for crate::ipc::binder::BinderNode {}
unsafe impl Sync for crate::ipc::binder::BinderNode {}
unsafe impl Send for crate::ipc::binder::BinderThread {}
unsafe impl Sync for crate::ipc::binder::BinderThread {}
unsafe impl Send for crate::ipc::binder::BinderTxn {}
unsafe impl Sync for crate::ipc::binder::BinderTxn {}
unsafe impl Send for crate::ipc::binder::BinderProc {}
unsafe impl Sync for crate::ipc::binder::BinderProc {}
