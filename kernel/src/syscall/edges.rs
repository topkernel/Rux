//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The syscall subsystem's audited unsafe edges (framework zone).
//!
//! unsafe-slim round L7 (2026-11-20), framekernel doctrine: the syscall
//! SERVICE files (`misc.rs`, `process.rs`, `file.rs`, `memory.rs`,
//! `signal.rs`, ...) end with ZERO `unsafe` keywords — checkable by
//! grep, ready for a crate-level `forbid(unsafe_code)`. Every
//! genuinely-unsafe operation they need lives HERE as a named edge with
//! an INV contract, in the shape of `kernel/src/sync/edges.rs` and
//! `kernel/src/process/unsafe_edges.rs` (one audit per edge, not per
//! call site). User-memory copies do not live here at all: they route
//! through the already-safe `crate::arch::uaccess_safe` entries, and
//! task-pointer borrows reuse
//! [`crate::process::unsafe_edges::borrow_task`].
//!
//! INV registry:
//!
//! - INV-SYSC-1 (intrusive interest/wake lists) — the epoll family's
//!   `ListHead` nodes (`EpollEntry`, `EpollWakeNode`): the `list` field
//!   is the FIRST field of the node type, so a node pointer doubles as
//!   its `ListHead` pointer and the container_of cast is the pointer
//!   cast itself. Every node is Box-allocated, linked/unlinked only
//!   under the lock that owns the list (the per-epoll `entries` lock or
//!   the wake-shard irqsave lock), and freed only after `unlink` (or
//!   when never linked). Walks capture `next` before the body touches
//!   the node — the unlink-during-walk discipline the pre-conversion
//!   loops documented inline.
//! - INV-SYSC-2 (unshared box + exactly-once reclaim) — constructor-site
//!   raw pointers produced by `Box::into_raw` in the same function:
//!   either the box is still uniquely owned (created, not yet installed
//!   into any shared structure), or it is the reclaim drop matching
//!   exactly that `into_raw`. `boxed_drop` is the pairing release for
//!   every reclaim site; a pointer is dropped exactly once per
//!   successful `into_raw` (error paths drop when the install failed,
//!   success paths hand ownership to the close op).
//! - INV-SYSC-3 (current-task park/unwind) — the repeated
//!   set_state(RUNNING) + dequeue_task pair on a raw current-task
//!   pointer (the NEW-C2 racing-wake-undo idiom): `task` is the calling
//!   CPU's current task or a waiter this CPU just parked; the pair must
//!   run together so a wake that enqueued us between the state check
//!   and the sleep is undone before the caller re-runs.

use crate::list::ListHead;
use crate::process::task::Task;

// ---------------------------------------------------------------------------
// INV-SYSC-1: epoll intrusive-list family
// ---------------------------------------------------------------------------

/// The first node pointer after the sentinel `head`
/// (INV-SYSC-1: `head` is a live, initialized sentinel under its lock).
#[inline]
pub fn list_first(head: *mut ListHead) -> *mut ListHead {
    debug_assert!(!head.is_null());
    // SAFETY: INV-SYSC-1 — plain load of the sentinel's next field.
    unsafe { (*head).next }
}

/// The node after `cur` (INV-SYSC-1: `cur` is a linked node's ListHead;
/// capture-before-body discipline — call this BEFORE unlinking `cur`).
#[inline]
pub fn list_next(cur: *mut ListHead) -> *mut ListHead {
    debug_assert!(!cur.is_null());
    // SAFETY: INV-SYSC-1 — plain load of a linked node's next field.
    unsafe { (*cur).next }
}

/// The node object whose FIRST field is the ListHead `cur`
/// (INV-SYSC-1 shared arm: read-only walk body under the owning lock).
#[inline]
pub fn list_entry<T>(cur: *mut ListHead) -> &'static T {
    debug_assert!(!cur.is_null());
    debug_assert!(cur as usize % core::mem::align_of::<T>() == 0);
    // SAFETY: INV-SYSC-1 — cur is a linked node's ListHead and T's
    // first field (offset 0), so the cast IS the container_of; the
    // node is alive and unlinked only under the caller-held lock.
    unsafe { &*(cur as *const T) }
}

/// The node object mutably (INV-SYSC-1 exclusive arm: read-modify walk
/// body under the owning lock; no other alias is live).
#[inline]
pub fn list_entry_mut<T>(cur: *mut ListHead) -> &'static mut T {
    debug_assert!(!cur.is_null());
    debug_assert!(cur as usize % core::mem::align_of::<T>() == 0);
    // SAFETY: INV-SYSC-1 — same container_of as list_entry, made
    // exclusive: the caller holds the lock that serializes every
    // mutable touch of this list, so no alias can be live.
    unsafe { &mut *(cur as *mut T) }
}

/// Link a fresh, unlinked node at the tail of `head`'s list
/// (INV-SYSC-1 link arm: node was Box::into_raw'd and never linked;
/// caller holds the list's lock).
#[inline]
pub fn list_link_tail(node: *mut ListHead, head: *mut ListHead) {
    debug_assert!(!node.is_null() && !head.is_null());
    // SAFETY: INV-SYSC-1 — node's list is initialized (fresh) and
    // unlinked; head is the live sentinel held under the lock.
    unsafe { (*node).add_tail(head) }
}

/// Unlink a linked node (INV-SYSC-1 unlink arm: caller holds the list's
/// lock; the node is currently linked and its neighbors are live).
#[inline]
pub fn list_unlink(node: &mut ListHead) {
    // SAFETY: INV-SYSC-1 — lock-held unlink of a linked node.
    unsafe { node.del() }
}

// ---------------------------------------------------------------------------
// The `Send` proofs for the raw-pointer registry types (moved from
// misc.rs so the service file carries no `unsafe` keywords; same
// contracts the inline impls documented).
// ---------------------------------------------------------------------------

// SAFETY: the registry's epoll pointers are only dereferenced under the
// registry/shard lock and are removed in the owning EpollFile's close
// op, which runs before the box is freed (INV-SYSC-1/2 serialization).
unsafe impl Send for crate::syscall::misc::EpollWakeRegistry {}

// SAFETY: same contract as EpollWakeRegistry: `ep` is only dereferenced
// while the binding is linked (shard lock held or pinned per
// notify_pins), and the node is unlinked before the epoll's close op
// frees the box (INV-SYSC-1/2).
unsafe impl Send for crate::syscall::misc::EpollWakeNode {}

// SAFETY: same contract as EpollWakeRegistry (the per-shard type).
unsafe impl Send for crate::syscall::misc::EpollWakeShard {}

// SAFETY: task pointers are only compared for identity; queue pointers
// are unregistered in the owning fd's close op before the box is freed.
unsafe impl Send for crate::syscall::misc::SignalfdWakeRegistry {}

// ---------------------------------------------------------------------------
// INV-SYSC-2: unshared-box borrows + the exactly-once reclaim
// ---------------------------------------------------------------------------

/// Shared borrow of a `Box::into_raw` object the caller uniquely owns
/// (INV-SYSC-2 shared arm: created in this call path, not yet installed
/// into any shared structure — or reached through a discipline that
/// proves it alive, such as the File identity check that precedes every
/// private_data downcast).
#[inline]
pub fn boxed_ref<T>(p: *const T) -> &'static T {
    debug_assert!(!p.is_null());
    // SAFETY: INV-SYSC-2 — non-null into_raw pointer whose box the
    // caller owns or has proven alive; the borrow ends before any drop.
    unsafe { &*p }
}

/// Mutable borrow of an unshared `Box::into_raw` object
/// (INV-SYSC-2 exclusive arm: the box is uniquely owned — created here,
/// or the close path's last-reference reclaim where nothing else can
/// alias it).
#[inline]
pub fn boxed_mut<T>(p: *mut T) -> &'static mut T {
    debug_assert!(!p.is_null());
    // SAFETY: INV-SYSC-2 — non-null into_raw pointer, uniquely owned:
    // no alias can be live while this reference exists.
    unsafe { &mut *p }
}

/// Reclaim a `Box::into_raw` allocation (INV-SYSC-2 reclaim arm: the
/// exactly-once drop pairing the constructor's `into_raw` — error paths
/// of the install, or the close op running on the last reference).
#[inline]
pub fn boxed_drop<T>(p: *mut T) {
    debug_assert!(!p.is_null());
    // SAFETY: INV-SYSC-2 — the pointer came from Box::into_raw, is
    // unlinked/unregistered from every reachable structure, and is
    // dropped exactly once on this path.
    unsafe { drop(alloc::boxed::Box::from_raw(p)) }
}

// ---------------------------------------------------------------------------
// INV-SYSC-3: current-task park/unwind
// ---------------------------------------------------------------------------

/// Undo a racing wake enqueue: set the task RUNNING and dequeue it if a
/// concurrent wake had re-queued us while we were still executing
/// (INV-SYSC-3: `task` is this CPU's current task — either parked by
/// this thread and re-taken, or never parked).
#[inline]
pub fn task_park_unwind(task: *mut Task) {
    let t = crate::process::unsafe_edges::borrow_task(task);
    t.set_state(crate::process::task::TaskState::new(
        crate::process::task::TaskState::RUNNING,
    ));
    crate::sched::dequeue_task(t);
}

// ---------------------------------------------------------------------------
// Arch reset edges (INV-SYSC-4: the reboot/panic ladder's port I/O and
// bare asm — deliberately-faulting or never-returning reset sequences
// that have no safe shape; reached only behind sys_reboot's CAP_SYS_BOOT
// gate or the panic timeout).
// ---------------------------------------------------------------------------

/// x86_64 panic reset: drain the 8042, pulse its reset line, then force
/// a triple fault through a null IDT (INV-SYSC-4; never returns to a
/// working machine — the CPU resets or QEMU dies).
#[cfg(feature = "x86_64")]
pub fn x86_panic_reset() {
    // SAFETY: INV-SYSC-4 — bare port I/O on the reset port and a
    // deliberate faulting instruction; neither returns.
    unsafe {
        // Drain the 8042 input buffer first (writing the command
        // while it is full is dropped on real hardware).
        for _ in 0..100_000 {
            if crate::arch::cpu::inb(0x64) & 0x02 == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        crate::arch::cpu::outb(0x64, 0xFE);
        // Give the pulse time to land.
        for _ in 0..10_000_000 {
            core::hint::spin_loop();
        }
        // Triple fault: load a null/limit-0 IDT and int3 — the
        // vector lookup must fail, the double fault must fail the
        // same way, and the CPU (QEMU included) resets.
        static NULL_IDT: [u64; 2] = [0; 2];
        core::arch::asm!(
            "lidt [{}]",
            in(reg) &NULL_IDT,
            options(nomem, nostack)
        );
        core::arch::asm!("int3", options(nomem, nostack));
    }
}

/// x86_64 reboot pulse: the 8042 keyboard-controller reset line, then
/// the PCI/ATX reset port 0xcf9 (INV-SYSC-4; the caller parks after if
/// the platform ignores both).
#[cfg(feature = "x86_64")]
pub fn x86_reboot_pulse() {
    // SAFETY: INV-SYSC-4 — the two reset-port writes; reached only
    // behind the CAP_SYS_BOOT gate in sys_reboot.
    unsafe {
        crate::arch::cpu::outb(0x64, 0xFE);
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
        crate::arch::cpu::outb(0xcf9, 0x0E);
    }
}

/// riscv64 SBI legacy shutdown ecall (FID 0x8) — the pre-0.3 power-off
/// path (INV-SYSC-4: the privileged ecall itself).
#[cfg(feature = "riscv64")]
pub fn sbi_shutdown_ecall() {
    // SAFETY: INV-SYSC-4 — privileged SBI ecall; reached only after the
    // CAP_SYS_BOOT gate in sys_reboot (or the panic fallback).
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") 0x8u64,
            out("a0") _,
            out("a1") _,
            options(nomem)
        );
    }
}

// ---------------------------------------------------------------------------
// Filetyped subsystem calls that are still `unsafe fn` in their home
// crates: thin named wrappers restating the call-site discipline once.
// ---------------------------------------------------------------------------

/// Resolve a file's pidfd identity (pidfd_open and /proc/<pid> dir fds).
///
/// INV-SYSC-5: the caller has verified the File's ops identity
/// (PIDFD_OPS / PROC_PIDDIR_OPS) before this call — the slot was
/// populated by `pidfd::attach` (Box::into_raw) and stays owned by the
/// file for its lifetime, so the returned reference is valid while the
/// caller holds its fd-table Arc.
#[inline]
pub fn pidfd_data_of(file: &crate::fs::File) -> Option<&'static crate::fs::pidfd::PidFdData> {
    // SAFETY: INV-SYSC-5 — identity-checked private_data downcast; the
    // box lives as long as the File.
    unsafe { crate::fs::pidfd::data_of(file) }
}

/// Write a FRESH File's inode cell before the File is published to any
/// fd table (INV-SYSC-7 pre-publish arm).
///
/// This is the raw-cell write the pre-conversion sites performed — it
/// deliberately does NOT take `File::set_inode`'s lease registration
/// (an O_PATH tree descriptor holds no read/write lease on the inode,
/// and registering one would change close-path behavior). Contract:
/// `file` was constructed in this call path, has no inode cell value
/// yet, and is not reachable from any other thread until the following
/// install.
#[inline]
pub fn file_inode_set_pre_publish(
    file: &crate::fs::File,
    inode: alloc::sync::Arc<crate::fs::inode::Inode>,
) {
    // SAFETY: INV-SYSC-7 — fresh, unpublished File; the plain cell
    // store races nothing and skips lease bookkeeping on purpose.
    unsafe { *file.inode.get() = Some(inode) }
}

// ---------------------------------------------------------------------------
// INV-SYSC-8: the mmap/mremap/msync/mprotect page-table family. The
// arch walkers/leaf rewrites are Round-D irreducibles (page-table
// mutation); they live behind these named edges with the caller-side
// discipline (PTE lock held, current task's own root) stated once.
// ---------------------------------------------------------------------------

/// Map one fresh page into a USER page table (map_page).
///
/// INV-SYSC-8 (map arm): `root_ppn` is the current task's live user
/// root; the PTE section is held by the caller; `phys` is a freshly
/// allocated frame exclusively owned by this path. aarch64 declares the
/// primitive unsafe (root validity); riscv64/x86_64 are safe fns — the
/// cfg split keeps one call site.
#[inline]
pub fn arch_map_page(
    root_ppn: u64,
    virt: crate::arch::mm::VirtAddr,
    phys: crate::arch::mm::PhysAddr,
    flags: u64,
) -> bool {
    #[cfg(feature = "aarch64")]
    {
        // SAFETY: INV-SYSC-8 — live user root (aarch64's only proof
        // obligation).
        unsafe { crate::arch::mm::map_page(root_ppn, virt, phys, flags) }
    }
    #[cfg(not(feature = "aarch64"))]
    {
        crate::arch::mm::map_page(root_ppn, virt, phys, flags)
    }
}

/// Tear down one user leaf with rmap/refcount accounting
/// (unmap_one_user_leaf_locked).
///
/// INV-SYSC-8 (unmap arm): the caller holds the mm's PTE lock and `mm`
/// is the current task's address space — the proof aarch64's unsafe fn
/// demands; the other arches' fns carry the same discipline in their
/// names.
#[inline]
pub fn unmap_user_leaf_locked(mm: &crate::mm::AddressSpace, va: usize) -> bool {
    #[cfg(feature = "aarch64")]
    {
        // SAFETY: INV-SYSC-8 — PTE lock held by the caller.
        unsafe { mm.unmap_one_user_leaf_locked(va) }
    }
    #[cfg(not(feature = "aarch64"))]
    {
        mm.unmap_one_user_leaf_locked(va)
    }
}

/// Copy one whole page between two physical frames through the kernel
/// linear map (INV-SYSC-8 copy arm): both frames' liveness is held by
/// the caller's path (PTE section + refcounts, exactly the
/// copy_old_to_new_pages discipline).
#[inline]
pub fn copy_page_linear(src_phys: u64, dst_phys: u64) {
    #[cfg(feature = "riscv64")]
    {
        crate::arch::mm::unsafe_edges::copy_page(src_phys, dst_phys);
    }
    #[cfg(not(feature = "riscv64"))]
    {
        let src = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(src_phys))
            .bits() as *const u8;
        let dst = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(dst_phys))
            .bits() as *mut u8;
        // SAFETY: INV-SYSC-8 — both frames are live for the call and
        // PAGE_SIZE bytes each; the regions are distinct pages.
        unsafe {
            core::ptr::copy_nonoverlapping(
                src,
                dst,
                crate::arch::mm::PAGE_SIZE as usize,
            )
        };
    }
}

/// Borrow a page descriptor from the vmemmap (INV-SYSC-8 desc arm):
/// page descriptors are machine-lifetime statics; `pfn` was validated
/// by the allocation/walk that produced it. `None` mirrors the raw
/// null-check the call sites performed inline.
#[inline]
pub fn page_desc_of(pfn: usize) -> Option<&'static mut crate::mm::page_desc::Page> {
    let p = crate::mm::page_desc::pfn_to_page_mut(pfn);
    if p.is_null() {
        None
    } else {
        // SAFETY: INV-SYSC-8 — non-null vmemmap descriptor of a frame
        // this path owns or holds a refcount on; machine-lifetime
        // storage.
        Some(unsafe { &mut *p })
    }
}

/// Rewrite one Sv39 leaf's permission bits in place (the riscv64 arm of
/// mprotect; x86_64/aarch64 use the safe arch mprotect_pte_range).
///
/// INV-SYSC-8 (mprotect arm): `root_ppn` is the current task's live
/// user root, the caller holds the mm's PTE lock for the whole loop,
/// and only present leaves are rewritten (non-present pages are
/// skipped). COW/PCACHE software bits and the PPN are preserved per
/// the MM-H7/session-25 rules the body documents.
#[cfg(feature = "riscv64")]
pub fn mprotect_rewrite_leaf_sv39(root_ppn: u64, virt: u64, new_flags: u64) {
    use crate::arch::mm::mm_ops::cow_flags;
    use crate::arch::mm::PageTableEntry;

    let virt_addr = crate::arch::mm::VirtAddr(virt);
    let vpn2 = virt_addr.vpn(2) as usize;
    let vpn1 = virt_addr.vpn(1) as usize;
    let vpn0 = virt_addr.vpn(0) as usize;

    // The walk runs under the caller's PTE lock (INV-RE-PT live arm).
    let root_table = match crate::arch::mm::unsafe_edges::PtTable::from_phys_live(
        root_ppn << crate::arch::mm::PAGE_SHIFT,
    ) {
        Some(t) => t,
        None => return,
    };
    let pte2 = root_table.get(vpn2);
    if !pte2.is_valid() {
        return; // Page not mapped, skip
    }
    let table1 = match crate::arch::mm::unsafe_edges::PtTable::from_phys_live(
        pte2.ppn() << crate::arch::mm::PAGE_SHIFT,
    ) {
        Some(t) => t,
        None => return,
    };
    let pte1 = table1.get(vpn1);
    if !pte1.is_valid() {
        return; // Page not mapped, skip
    }
    let table0 = match crate::arch::mm::unsafe_edges::PtTable::from_phys_live(
        pte1.ppn() << crate::arch::mm::PAGE_SHIFT,
    ) {
        Some(t) => t,
        None => return,
    };
    let pte0 = table0.get(vpn0);
    if !pte0.is_valid() {
        return;
    }

    // Preserve PPN and any COW software bit (bit 8): clearing COW here
    // let two processes that share a forked page write straight through
    // after mprotect(PROT_WRITE), bypassing the COW copy (review
    // MM-H7). Also never raise W on a page whose COW bit is set — the
    // write fault path owns the copy.
    //
    // PCACHE (RSW bit 9, riscv64 filemap_fault port): a
    // mapping-ownership bit, preserved like COW — dropping it would
    // leak the page-cache pin forever (frame unevictable). A PRIVATE
    // cache-backed leaf NEVER gains W in place: the frame is shared
    // with the page cache, so a W-upgrade diverts the first write
    // through COW (Linux writenotify; x86_64 mprotect_pte_range twin).
    // A SHARED cache leaf (PCACHE with no COW — cow_flags::
    // pte_is_shared_cache; Sv39 has no third software bit) keeps W in
    // place — its stores must land in the cache frame (session 25), and
    // gaining W here (an RO-faulted shared leaf being upgraded) is a
    // W-install: mark the cache page dirty so the later msync/fsync
    // persists such stores.
    let ppn = pte0.ppn();
    let cow_bit = pte0.bits() & cow_flags::COW;
    let pcache_bit = pte0.bits() & cow_flags::PCACHE;
    let mut flags = new_flags;
    if cow_bit != 0 {
        flags |= cow_bit;
        flags &= !PageTableEntry::W; // defer W to COW fault
    }
    if pcache_bit != 0 {
        flags |= pcache_bit;
        if flags & PageTableEntry::W != 0 {
            if cow_flags::pte_is_shared_cache(pte0.bits()) {
                crate::fs::page_cache::get_page_cache().mark_dirty_pfn(ppn as usize);
            } else {
                flags |= cow_flags::COW;
                flags &= !PageTableEntry::W;
            }
        }
    }
    let new_pte = PageTableEntry::from_bits((ppn << 10) | flags);
    table0.set(vpn0, new_pte);
}

/// Resolve the WRITABLE physical frame backing `va`, if any (the msync
/// writeback resolver).
///
/// INV-SYSC-8 (msync arm): `root_ppn` is the current task's live user
/// root; the walk only READS PTEs under the caller's PTE lock. Returns
/// the physical address of the writable leaf frame (PCACHE leaves are
/// skipped — their writeback is the page cache's dirty path).
pub fn pt_walk_writable_phys(root_ppn: u64, va: u64) -> Option<u64> {
    // The Sv39-shaped walk: three levels, leaf when R/W/X present —
    // the arch shims (`get_page_table_virt`, `PageTableEntry`) exist
    // on every arch this file builds for, same as the raw walk this
    // replaces.
    let vpn = [
        (va >> 12) & 0x1FF,
        (va >> 21) & 0x1FF,
        (va >> 30) & 0x1FF,
    ];
    let page_shift = crate::arch::mm::PAGE_SHIFT;
    // SAFETY: INV-SYSC-8 — read-only place projection; the root frame
    // is the caller-pinned live root and descent follows only valid
    // entries (the same argument the raw walk made inline).
    let mut pte_virt = unsafe {
        crate::arch::mm::get_page_table_virt(root_ppn << page_shift)
    } as *const crate::arch::mm::PageTableEntry;
    let mut phys: Option<u64> = None;
    for level in (0..3usize).rev() {
        // SAFETY: INV-SYSC-8 — read-only place projection of a live
        // table entry (the walk descends only valid entries).
        let pte = unsafe { &*pte_virt.add(vpn[level] as usize) };
        if !pte.is_valid() {
            break;
        }
        // PCACHE leaves (SHARED ext4, session 25): their writeback
        // state is the page cache's dirty bit — flushed by
        // ext4_flush_dirty_pages above. Writing the frame here as well
        // would be redundant I/O through the slow path.
        if pte.bits() & crate::arch::mm::mm_ops::cow_flags::PCACHE != 0 {
            break;
        }
        let is_leaf = pte.is_readable() || pte.is_writable() || pte.is_executable();
        if level == 0 || is_leaf {
            if pte.is_writable() {
                phys = Some(pte.ppn() << page_shift);
            }
            break;
        }
        // SAFETY: INV-SYSC-8 — same conversion for the next-level table
        // the valid parent entry names.
        pte_virt = unsafe {
            crate::arch::mm::get_page_table_virt(pte.ppn() << page_shift)
        } as *const crate::arch::mm::PageTableEntry;
    }
    phys
}

/// Write one kernel-linear page (or its EOF-clamped head) out through
/// `file`'s raw write path (the msync writeback store).
///
/// INV-SYSC-8 (writeback arm): `page_phys` is a present writable frame
/// resolved by [`pt_walk_writable_phys`] under the same PTE section;
/// `n <= PAGE_SIZE` is the EOF clamp. This is the one remaining raw
/// source pointer — `File::write_at`'s safe slice twin cannot describe
/// a page we do not own as a Rust slice.
pub fn file_write_page(
    file: &crate::fs::File,
    file_off: u64,
    page_phys: u64,
    n: usize,
) -> isize {
    let kva = crate::arch::mm::phys_to_virt(crate::arch::mm::memory_layout::PhysAddr(
        page_phys,
    ));
    // SAFETY: INV-SYSC-8 — n readable bytes of a present page through
    // the kernel linear map (the same source the raw write_at call
    // used).
    unsafe { file.write_at(file_off, kva.as_usize() as *const u8, n) }
}

/// Map one fresh user page (map_user_page).
///
/// INV-SYSC-8 (user-map arm): the VA is page-aligned inside a VMA this
/// path just added, `phys` is a fresh zeroed frame, and the caller
/// holds the mm's PTE lock — the demand-fault discipline. riscv64
/// declares the primitive unsafe; the other arches are safe fns.
#[inline]
pub fn arch_map_user_page(
    root_ppn: u64,
    virt: crate::arch::mm::memory_layout::VirtAddr,
    phys: crate::arch::mm::memory_layout::PhysAddr,
    flags: u64,
) -> bool {
    #[cfg(feature = "riscv64")]
    {
        // SAFETY: INV-SYSC-8 — live user root, PTE section held.
        unsafe { crate::arch::mm::map_user_page(root_ppn, virt, phys, flags) }
    }
    #[cfg(not(feature = "riscv64"))]
    {
        crate::arch::mm::map_user_page(root_ppn, virt, phys, flags)
    }
}
