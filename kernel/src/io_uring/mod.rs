//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IO_uring — High-performance async I/O interface
//!
//! Implements the io_uring ABI (Linux 5.1+): submission/completion ring
//! buffers shared between kernel and userspace via mmap.  Operations
//! execute synchronously in the io_uring_enter syscall context, providing
//! batched submission with reduced syscall overhead.
//!
//! Supported opcodes: NOP, READ, WRITE, FSYNC, CLOSE, FADVISE.
//! Supported register ops: EVENTFD / UNREGISTER_EVENTFD.

extern crate alloc;

use alloc::sync::Arc;
use alloc::boxed::Box;

use crate::fs::{File, FileOps, FileFlags};
use crate::mm::page_alloc::{alloc_pages, free_pages};
use crate::mm::zone::GfpFlags;
use crate::sync::spinlock::Spinlock;

// ==================== UAPI Constants ====================

// Opcodes (pub since the policy flip: the io_uring crate's op
// dispatch and register paths match on them)
pub const IORING_OP_NOP:     u8 = 0;
pub const IORING_OP_FSYNC:   u8 = 3;
pub const IORING_OP_CLOSE:   u8 = 14;
pub const IORING_OP_READ:    u8 = 22;
pub const IORING_OP_WRITE:   u8 = 23;
pub const IORING_OP_FADVISE: u8 = 28;

// Features reported to userspace
const IORING_FEAT_SINGLE_MMAP:   u32 = 1 << 0;
const IORING_FEAT_NODROP:        u32 = 1 << 1;
const IORING_FEAT_SUBMIT_STABLE: u32 = 1 << 2;
const IORING_FEAT_RW_CUR_POS:    u32 = 1 << 3;

// mmap offsets
const IORING_OFF_SQ_RING:   u64 = 0x0000_0000;
const IORING_OFF_CQ_RING:   u64 = 0x8000_0000;
const IORING_OFF_SQES:      u64 = 0x1000_0000;
const IORING_OFF_MMAP_MASK: u64 = 0xf800_0000;

// io_uring_enter flags (pub since the policy flip)
pub const IORING_ENTER_GETEVENTS: u32 = 1 << 0;

// Register opcodes (pub since the policy flip)
pub const IORING_REGISTER_EVENTFD:   u32 = 4;
pub const IORING_UNREGISTER_EVENTFD: u32 = 5;

// Limits
const IORING_MAX_ENTRIES: u32 = 4096;
const IORING_MIN_ENTRIES: u32 = 1;

// ==================== UAPI Wire Structures ====================

/// io_uring Submission Queue Entry — 64 bytes, matches Linux UABI.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IoUringSqe {
    pub opcode:      u8,
    pub flags:       u8,
    pub ioprio:      u16,
    pub fd:          i32,
    pub off:         u64,
    pub addr:        u64,
    pub len:         u32,
    pub rw_flags:    u32,
    pub user_data:   u64,
    pub buf_index:   u16,
    pub personality: u16,
    pub splice_fd_in: i32,
    pub addr3:       u64,
    pub __pad2:      u64,
}
const _: () = assert!(core::mem::size_of::<IoUringSqe>() == 64);

/// io_uring Completion Queue Entry — 16 bytes, matches Linux UABI.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IoUringCqe {
    pub user_data: u64,
    pub res:       i32,
    pub flags:     u32,
}
const _: () = assert!(core::mem::size_of::<IoUringCqe>() == 16);

/// io_uring_params — passed to io_uring_setup, returned with offsets.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IoUringParams {
    pub sq_entries:     u32,
    pub cq_entries:     u32,
    pub flags:          u32,
    pub sq_thread_cpu:  u32,
    pub sq_thread_idle: u32,
    pub features:       u32,
    pub wq_fd:          u32,
    pub resv:           [u32; 3],
    pub sq_off:         IoSqringOffsets,
    pub cq_off:         IoCqringOffsets,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IoSqringOffsets {
    pub head:          u32,
    pub tail:          u32,
    pub ring_mask:     u32,
    pub ring_entries:  u32,
    pub flags:         u32,
    pub dropped:       u32,
    pub array:         u32,
    pub resv1:         u32,
    pub user_addr:     u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IoCqringOffsets {
    pub head:          u32,
    pub tail:          u32,
    pub ring_mask:     u32,
    pub ring_entries:  u32,
    pub overflow:      u32,
    pub cqes:          u32,
    pub flags:         u32,
    pub resv1:         u32,
    pub user_addr:     u64,
}

// ==================== Ring Region ====================

/// A contiguous physically-backed memory region for one ring component.
struct RingRegion {
    /// Kernel virtual address (via phys_to_virt linear mapping)
    kvirt:  *mut u8,
    /// Physical address of the first page
    phys:   usize,
    /// Size in bytes (page-aligned)
    size:   usize,
    /// Number of pages
    npages: usize,
}

// ==================== IoUring Instance ====================

/// The io_uring ring instance.
pub struct IoUring {
    sq_entries:    u32,
    cq_entries:    u32,
    sq_ring_mask:  u32,
    cq_ring_mask:  u32,

    sq_ring:  RingRegion,
    cq_ring:  RingRegion,
    sqes:     RingRegion,

    // Cached byte offsets into sq_ring
    sq_head_off:          usize,
    sq_tail_off:          usize,
    sq_ring_mask_off:     usize,
    sq_ring_entries_off:  usize,
    sq_flags_off:         usize,
    sq_array_off:         usize,

    // Cached byte offsets into cq_ring
    cq_head_off:          usize,
    cq_tail_off:          usize,
    cq_ring_mask_off:     usize,
    cq_ring_entries_off:  usize,
    cq_overflow_off:      usize,
    cq_flags_off:         usize,
    cq_cqes_off:          usize,

    /// eventfd fd for completion notification (-1 = none)
    eventfd_fd: core::sync::atomic::AtomicI32,
    /// Registered eventfd file reference (holds the file alive across fd
    /// reuse; the raw fd number above is only used for reporting)
    eventfd_file: Spinlock<Option<alloc::sync::Arc<File>>>,

    /// Manual reference count: enter/mmap hold a reference while using the
    /// ring so a concurrent close() cannot free it underneath them. The
    /// final unref frees the ring regions (see Drop below).
    refs: core::sync::atomic::AtomicU32,

    cq_lock: Spinlock<()>,
}

impl IoUring {
    /// Try to acquire a reference; fails if the ring is being destroyed.
    /// `pub` since the io_uring POLICY flip: the io_uring service
    /// crate's sys_enter/sys_register call it under the lifecycle lock
    /// (direct cross-crate call — service -> core, inlinable).
    pub fn try_ref(&self) -> bool {
        let mut cur = self.refs.load(core::sync::atomic::Ordering::Acquire);
        loop {
            if cur == 0 {
                return false;
            }
            match self.refs.compare_exchange(
                cur,
                cur + 1,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(v) => cur = v,
            }
        }
    }

    fn unref(&self) {
        if self.refs.fetch_sub(1, core::sync::atomic::Ordering::AcqRel) == 1 {
            // Last reference: free the instance (and its ring regions via Drop).
            // SAFETY: this instance was created by Box::into_raw (io_uring_create
            // → into_raw at install) and no other references remain.
            unsafe { drop(alloc::boxed::Box::from_raw(self as *const IoUring as *mut IoUring)); }
        }
    }
}

impl Drop for IoUring {
    fn drop(&mut self) {
        free_ring_region(&self.sq_ring);
        free_ring_region(&self.cq_ring);
        free_ring_region(&self.sqes);
    }
}

/// Scoped reference to an IoUring: decrements on drop (all exit paths).
struct RingRef(*const IoUring);
impl Drop for RingRef {
    fn drop(&mut self) {
        // SAFETY: paired with a successful try_ref at construction.
        unsafe { (*self.0).unref() };
    }
}

// ==================== Ring Layout Helpers ====================

const PAGE_SIZE: usize = 4096;

/// SQ ring size: header (6 * 4 = 24 bytes) + index array (4 * entries).
fn sq_ring_size(entries: u32) -> usize {
    let total = 24 + 4 * entries as usize;
    page_align(total)
}

/// CQ ring size: header (8 * 4 = 32 bytes) + CQE array (16 * entries).
fn cq_ring_size(entries: u32) -> usize {
    let total = 32 + 16 * entries as usize;
    page_align(total)
}

/// SQE array size: 64 bytes per entry.
fn sqes_size(entries: u32) -> usize {
    page_align(64 * entries as usize)
}

fn page_align(size: usize) -> usize {
    (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

fn round_up_pow2(n: u32) -> u32 {
    if n <= 1 { return 1; }
    let mut v = n - 1;
    v |= v >> 1;
    v |= v >> 2;
    v |= v >> 4;
    v |= v >> 8;
    v |= v >> 16;
    v + 1
}

// ==================== Ring Region Allocation ====================

fn alloc_ring_region(size: usize) -> Option<RingRegion> {
    let npages = page_align(size) / PAGE_SIZE;
    let order = npages.next_power_of_two().trailing_zeros() as usize;

    let phys = alloc_pages(GfpFlags::GFP_KERNEL, order);
    if phys == 0 {
        return None;
    }

    let kvirt = unsafe {
        crate::arch::mm::phys_to_virt(
            crate::arch::mm::PhysAddr::new(phys as u64),
        ).bits() as *mut u8
    };

    // Zero the memory
    unsafe { core::ptr::write_bytes(kvirt, 0, npages * PAGE_SIZE); }

    Some(RingRegion {
        kvirt,
        phys,
        size: npages * PAGE_SIZE,
        npages,
    })
}

fn free_ring_region(region: &RingRegion) {
    let order = region.npages.next_power_of_two().trailing_zeros() as usize;
    // Drop the owner's per-page reference. User mappings hold their own
    // per-page references (taken at mmap); only free the block when every
    // page has been unpinned — otherwise a stray mapping's munmap will
    // release the last reference per page (review 2R.6).
    let mut still_pinned = false;
    let base_pfn = crate::mm::phys_to_pfn(region.phys);
    for i in 0..region.npages {
        let page = crate::mm::pfn_to_page_mut(base_pfn + i);
        if !page.is_null() {
            // SAFETY: page descriptor exists for this in-RAM page.
            let r = unsafe { (*page).put_page() };
            if r > 0 {
                still_pinned = true;
            }
        }
    }
    if !still_pinned {
        free_pages(region.phys, order);
    }
}

// ==================== Ring Creation ====================

/// Create a new io_uring instance. `pub` since the policy flip: the
/// io_uring crate's sys_io_uring_setup calls it (the ring layout
/// and region assembly stay mechanism-side — this fn constructs the
/// private-field IoUring and calls the unsafe region allocator).
pub fn io_uring_create(entries: u32, params: &mut IoUringParams) -> Result<Box<IoUring>, i32> {
    // Reject all setup flags (no SQPOLL, IOPOLL, etc.)
    if params.flags != 0 {
        return Err(-22); // EINVAL
    }

    // Validate reserved fields
    if params.resv[0] != 0 || params.resv[1] != 0 || params.resv[2] != 0 {
        return Err(-22);
    }

    // Clamp and round up entries
    let entries = if entries == 0 { 0 } else { entries.clamp(IORING_MIN_ENTRIES, IORING_MAX_ENTRIES) };
    if entries == 0 {
        return Err(-22);
    }
    let sq_entries = round_up_pow2(entries);
    let cq_entries = round_up_pow2(sq_entries * 2);

    // Allocate ring regions
    let sq_ring = alloc_ring_region(sq_ring_size(sq_entries)).ok_or(-12)?; // ENOMEM
    let cq_ring = alloc_ring_region(cq_ring_size(cq_entries)).ok_or_else(|| {
        free_ring_region(&sq_ring);
        -12
    })?;
    let sqes = alloc_ring_region(sqes_size(sq_entries)).ok_or_else(|| {
        free_ring_region(&cq_ring);
        free_ring_region(&sq_ring);
        -12
    })?;

    // Write SQ ring header (INV-IOURING-RING-1: the region is in hand,
    // pre-publication — the view's window check is defense-in-depth).
    {
        let sq = crate::safety::mmio::MmioRegion::new(sq_ring.kvirt as usize, sq_ring.size);
        sq.write_u32(0, 0);              // head
        sq.write_u32(4, 0);              // tail
        sq.write_u32(8, sq_entries - 1); // ring_mask
        sq.write_u32(12, sq_entries);    // ring_entries
        sq.write_u32(16, 0);             // flags
        sq.write_u32(20, 0);             // dropped
    }

    // Write CQ ring header (same region-in-hand shape).
    {
        let cq = crate::safety::mmio::MmioRegion::new(cq_ring.kvirt as usize, cq_ring.size);
        cq.write_u32(0, 0);              // head
        cq.write_u32(4, 0);              // tail
        cq.write_u32(8, cq_entries - 1); // ring_mask
        cq.write_u32(12, cq_entries);    // ring_entries
        cq.write_u32(16, 0);             // overflow
        cq.write_u32(20, 0);             // flags
    }

    // Fill in params. NOTE (review批次6 P1): do NOT advertise
    // IORING_FEAT_SINGLE_MMAP — this implementation maps the SQ ring, CQ
    // ring and SQEs as SEPARATE regions. With the bit set, liburing maps
    // one region for both rings and computes wrong ring pointers.
    params.sq_entries = sq_entries;
    params.cq_entries = cq_entries;
    params.features = IORING_FEAT_SUBMIT_STABLE | IORING_FEAT_RW_CUR_POS;

    // SQ offsets
    params.sq_off.head = 0;
    params.sq_off.tail = 4;
    params.sq_off.ring_mask = 8;
    params.sq_off.ring_entries = 12;
    params.sq_off.flags = 16;
    params.sq_off.dropped = 20;
    params.sq_off.array = 24;

    // CQ offsets
    params.cq_off.head = 0;
    params.cq_off.tail = 4;
    params.cq_off.ring_mask = 8;
    params.cq_off.ring_entries = 12;
    params.cq_off.overflow = 16;
    params.cq_off.cqes = 32;
    params.cq_off.flags = 20;

    let ring = Box::new(IoUring {
        sq_entries,
        cq_entries,
        sq_ring_mask: sq_entries - 1,
        cq_ring_mask: cq_entries - 1,

        sq_ring,
        cq_ring,
        sqes,

        sq_head_off: 0,
        sq_tail_off: 4,
        sq_ring_mask_off: 8,
        sq_ring_entries_off: 12,
        sq_flags_off: 16,
        sq_array_off: 24,

        cq_head_off: 0,
        cq_tail_off: 4,
        cq_ring_mask_off: 8,
        cq_ring_entries_off: 12,
        cq_overflow_off: 16,
        cq_flags_off: 20,
        cq_cqes_off: 32,

        eventfd_fd: core::sync::atomic::AtomicI32::new(-1),
        eventfd_file: Spinlock::new(None),
        refs: core::sync::atomic::AtomicU32::new(1),
        cq_lock: Spinlock::new(()),
    });

    Ok(ring)
}

// ==================== FileOps ====================

/// A pinned, refcounted ring reference for POLICY-side callers (the
/// io_uring crate's sys_enter/sys_register). Owns the RingRef, so
/// the shared `&IoUring` it hands out stays valid for the struct's
/// whole life (the final unref — which may free the ring — runs in
/// RingRef's Drop, i.e. only when this struct is gone).
/// SAFETY-CONTRACT (INV-IOURING-PIN): the interior &'static is sound
/// because the guard field keeps `refs > 0` for as long as the struct
/// exists; the raw ptr came from Box::into_raw at install time.
pub struct PinnedRing {
    ring: &'static IoUring,
    _guard: RingRef,
}

impl PinnedRing {
    /// The pinned ring (shared reference; valid while the guard lives).
    pub fn ring(&self) -> &IoUring {
        self.ring
    }
}

/// Pin the ring of an ops-VERIFIED io_uring file: read private_data and
/// try_ref INSIDE the lifecycle lock (R7-C4: the pointer read and the
/// deref must not straddle a close()). `pub` since the policy flip —
/// the crate's syscall sites call this directly (service -> core).
pub fn pin_verified_ring(file: &File) -> Option<PinnedRing> {
    let _guard = LIFECYCLE_LOCK.lock();
    let ptr = file.get_private_data()? as *const IoUring;
    // SAFETY: under the lifecycle lock close() cannot clear+free the
    // ring between the read and try_ref (INV-IOURING-PIN).
    let ring = unsafe { &*ptr };
    if !ring.try_ref() {
        return None;
    }
    Some(PinnedRing {
        ring,
        _guard: RingRef(ptr),
    })
}

/// Audited edge (INV-IOURING-RING-2): release a ring whose
/// Box::into_raw pointer could not be installed into the fd table
/// (alloc_fd / install_fd failure in sys_io_uring_setup). Called by the
/// io_uring crate, which cannot write unsafe.
pub fn ring_release_on_install_error(ring_ptr: *mut u8) {
    // SAFETY: paired with the Box::into_raw in sys_io_uring_setup; the
    // install failed, so no pin_ring reader can exist and no mapping
    // took a page reference yet.
    unsafe { drop(alloc::boxed::Box::from_raw(ring_ptr as *mut IoUring)); }
}

/// Serializes the private_data lifecycle: close() clears the pointer and
/// drops the installation reference under this lock, while enter/register/
/// mmap read the pointer and try_ref under the same lock. The lock lives
/// outside the IoUring because close's final unref may free the ring.
static LIFECYCLE_LOCK: Spinlock<()> = Spinlock::new(());

/// Acquire a pinned reference to the ring of an ops-verified io_uring file.
/// Returns None if the ring is absent or being torn down.
fn pin_ring(file: &File) -> Option<RingRef> {
    let _guard = LIFECYCLE_LOCK.lock();
    let ptr = file.get_private_data()? as *const IoUring;
    // SAFETY: ptr was installed by Box::into_raw for an IO_URING_OPS file
    // and cannot be freed while we hold the lifecycle lock + a reference.
    let ring = unsafe { &*ptr };
    if !ring.try_ref() {
        return None;
    }
    Some(RingRef(ptr))
}

fn io_uring_close(file: &File) -> i32 {
    let _guard = LIFECYCLE_LOCK.lock();
    // take_private_data clears FIRST so no new reference can be acquired
    // (the cell discipline's close-path take, INV-FS-FILEPRIV-1), then we
    // drop our reference; the final unref (which may free the ring) runs
    // under the lock.
    if let Some(ptr) = file.take_private_data() {
        // SAFETY: ptr came from Box::into_raw at install time; the
        // lifecycle lock serializes against every pin_ring reader.
        unsafe { (*(ptr as *const IoUring)).unref(); }
    }
    0
}

fn io_uring_poll(_file: &File, events: u16) -> u16 {
    // io_uring fd is always readable and writable
    const POLLIN: u16 = 0x001;
    const POLLOUT: u16 = 0x004;
    let mut ready = 0u16;
    if events & POLLIN != 0 { ready |= POLLIN; }
    if events & POLLOUT != 0 { ready |= POLLOUT; }
    ready
}

pub static IO_URING_OPS: FileOps = FileOps::builder()
    .close(io_uring_close)
    .poll(io_uring_poll)
    .build();

// ==================== mmap Handler ====================

/// Handle mmap on an io_uring fd — maps ring buffers to userspace.
/// `file` must already be ops-verified as an io_uring file by the caller
/// (passing the fd here again would re-fetch it — a TOCTOU type confusion).
pub fn io_uring_mmap_handler(
    file: &File, addr: usize, length: usize, offset: u64, prot: u32,
) -> Result<usize, i32> {
    use crate::arch::mm::{PageTableEntry, VirtAddr, PhysAddr, map_page};
    use crate::mm::vma::{Vma, VmaFlags};
    use crate::mm::page::VirtAddr as PageVirtAddr;

    // R7-C4: read the pointer INSIDE the lock (was read before, deref
    // after — a concurrent close() could free the ring in between).
    let (ring, _ring_guard) = {
        let _guard = LIFECYCLE_LOCK.lock();
        let ring_ptr = file.get_private_data().ok_or(-9)?;
        // SAFETY: under the lifecycle lock, close() cannot clear+free the
        // ring between the read and try_ref.
        let ring = unsafe { &*(ring_ptr as *const IoUring) };
        if !ring.try_ref() {
            return Err(-9); // EBADF: ring being torn down
        }
        (ring, RingRef(ring_ptr as *const IoUring))
    };

    let region = match offset & IORING_OFF_MMAP_MASK {
        IORING_OFF_SQ_RING => &ring.sq_ring,
        IORING_OFF_CQ_RING => &ring.cq_ring,
        IORING_OFF_SQES    => &ring.sqes,
        _ => return Err(-22), // EINVAL
    };

    if length != region.size {
        return Err(-22);
    }

    let current_task = crate::sched::current().ok_or(-12)?; // ENOMEM
    let addr_space = current_task.address_space().ok_or(-12)?;
    let user_ppn = addr_space.root_ppn();

    // addr == 0 asks the kernel to choose (review批次6 P1: the old fixed
    // MMAP_START made every SECOND ring mmap collide with the first).
    // find_free_area honors existing VMAs, so repeated NULL mappings stack.
    let vaddr = if addr == 0 {
        match crate::mm::mm_hooks::find_free_area(&addr_space, region.size) {
            Ok(v) => v.as_usize(),
            Err(_) => return Err(-12), // ENOMEM: no gap left
        }
    } else {
        addr & !(PAGE_SIZE - 1)
    };

    // Never allow a ring mapping at or above USER_END: map_page has no
    // address guard and the user root table shares the kernel PGD entries,
    // so this would overwrite kernel PTEs with user-writable ones
    // (review NEW-C1 — full privilege escalation).
    {
        let user_end = crate::arch::mm::user_addr::USER_END;
        let end = vaddr.checked_add(region.size).ok_or(-22)?;
        if end > user_end {
            return Err(-22);
        }
    }

    // ATOMIC RESERVE (b91 corruption fix): add the ring's VMA BEFORE any
    // eager PTE work — the old order (map loop, then vma_write().add)
    // raced same-mm mappers for the picked range: the loser's add failed
    // AFTER its PTEs were installed (leaked mapping over a foreign VMA),
    // or its map_page overwrote the winner's demand PTEs. Reserved first,
    // the map loop below can only install into our own VMA.
    let mut vma_flags = VmaFlags::new();
    vma_flags.insert(VmaFlags::READ);
    vma_flags.insert(VmaFlags::WRITE);
    vma_flags.insert(VmaFlags::SHARED);

    let mut vma = Vma::new(
        PageVirtAddr::new(vaddr),
        PageVirtAddr::new(vaddr + region.size),
        vma_flags,
    );
    vma.set_origin(crate::mm::vma::VmaOrigin::from_bits(
        crate::mm::vma::VmaOrigin::OTHER,
    ));
    if addr_space.vma_write().add(vma).is_err() {
        return Err(-12);
    }

    unsafe {
        let mut pte_flags = PageTableEntry::V | PageTableEntry::U
            | PageTableEntry::A | PageTableEntry::D;
        if prot & 0x1 != 0 { pte_flags |= PageTableEntry::R; }
        if prot & 0x2 != 0 { pte_flags |= PageTableEntry::R | PageTableEntry::W; }

        for i in 0..region.npages {
            let va = vaddr + i * PAGE_SIZE;
            let pa = region.phys + i * PAGE_SIZE;
            map_page(user_ppn, VirtAddr::new(va as u64), PhysAddr::new(pa as u64), pte_flags);
            // The user PTE now references the ring page: take a mapping
            // reference so munmap's put_page cannot free a page the ring
            // still owns (Wave-2 regression, review 2R.6). The final free
            // happens in Drop only when every mapping is gone.
            let page = crate::mm::pfn_to_page_mut(
                crate::mm::phys_to_pfn(pa),
            );
            if !page.is_null() {
                (*page).get_page();
            }
        }
        crate::arch::mm::asid::flush_tlb_all();
    }

    Ok(vaddr)
}

// ==================== Submission Processing ====================
// MOVED to the io_uring service crate (the cold-four lane): the
// submit loop, the op dispatch and the READ/WRITE/FSYNC/CLOSE/FADVISE
// handlers are syscall-context policy over the FileOps vtable — safe
// code, reached only from sys_io_uring_enter (syscall's wrapper
// re-exports the crate). The ring views they consume stay here (the
// INV-IOURING-RING-1 window protocol) and are reached through the pub
// accessor surface above; the completion path (post_cqe +
// signal_eventfd_file) stays here with the wakeup path.

// ==================== Ring views (INV-IOURING-RING-1) ====================

/// INV-IOURING-RING-1: the SQ/CQ ring headers and the SQE array are
/// USER-SHARED memory (mmap'd at setup, byte sizes fixed then and never
/// resized) — the kernel is one party of a shared-memory protocol, the
/// exact relationship MmioRegion types for device windows (the "device"
/// here is the user process). Every ring access goes through these
/// views: the MmioRegion window is the setup-time region size, so each
/// head/tail/array/SQE/CQE access is bounds-checked against it — the
/// unchecked `.add()` family (ledger row 21) is structurally impossible
/// through these (a firing window check is a kernel bug and panics,
/// never an OOB on user-controlled data).
impl IoUring {
    /// The SQ-ring header view. `pub` since the policy flip: the
    /// io_uring crate's submit path reads head/tail/array through
    /// it (INV-IOURING-RING-1 — the window bounds every access).
    pub fn sq_hdr(&self) -> crate::safety::mmio::MmioRegion {
        crate::safety::mmio::MmioRegion::new(self.sq_ring.kvirt as usize, self.sq_ring.size)
    }
    /// The CQ-ring header view (`pub` since the policy flip — the
    /// crate's wait/ready-count paths read head/tail through it).
    pub fn cq_hdr(&self) -> crate::safety::mmio::MmioRegion {
        crate::safety::mmio::MmioRegion::new(self.cq_ring.kvirt as usize, self.cq_ring.size)
    }
    /// SQE count (the bound submit validates array indices against).
    pub fn sq_entries(&self) -> u32 {
        self.sq_entries
    }
    /// Head/tail/array offset accessors (setup-time constants; `pub`
    /// since the policy flip for the crate-side submit loop).
    pub fn sq_head_off(&self) -> usize { self.sq_head_off }
    pub fn sq_tail_off(&self) -> usize { self.sq_tail_off }
    pub fn sq_array_off(&self) -> usize { self.sq_array_off }
    pub fn sq_ring_mask(&self) -> u32 { self.sq_ring_mask }
    /// Set or clear the completion-notification eventfd
    /// (sys_io_uring_register's policy side calls this; pub since the
    /// flip — the fields stay private).
    pub fn set_eventfd(&self, fd: i32, file: Option<alloc::sync::Arc<File>>) {
        self.eventfd_fd.store(fd, core::sync::atomic::Ordering::Release);
        *self.eventfd_file.lock() = file;
    }

    /// CQEs currently posted (tail - head through the window view).
    /// Replaces the raw `read_volatile` pair sys_io_uring_enter carried
    /// (same reads, now through the INV-IOURING-RING-1 bounds-checked
    /// view — the typed-twin conversion of the flip).
    pub fn cq_completed(&self) -> u32 {
        let cq = self.cq_hdr();
        let head = cq.read_u32(self.cq_head_off);
        let tail = cq.read_u32(self.cq_tail_off);
        tail.wrapping_sub(head)
    }
    /// Read one SQE by index (caller validated `idx < sq_entries`).
    /// `pub` since the policy flip (the crate's submit loop).
    pub fn sqe_at(&self, idx: u32) -> IoUringSqe {
        let r = crate::safety::mmio::MmioRegion::new(
            self.sqes.kvirt as usize,
            self.sqes.size,
        );
        // IoUringSqe is 64 bytes of POD (repr(Rust) plain u64/u32 fields,
        // no padding surprises — same layout liburing shares); read the
        // four u64 words and the tail u32 through the window.
        // SAFETY: INV-IOURING-RING-1 + idx < sq_entries (checked by every
        // caller) keeps 0..64 inside the SQE-array window.
        unsafe { core::ptr::read_volatile((r.base() + idx as usize * 64) as *const IoUringSqe) }
    }
    /// Write one CQE at slot `slot` (caller masked the slot).
    fn cqe_write(&self, slot: usize, cqe: IoUringCqe) {
        let r = self.cq_hdr();
        let off = self.cq_cqes_off + slot * 16;
        // SAFETY: INV-IOURING-RING-1; off+16 <= window by the setup-time
        // cq_entries sizing (the window check is the backstop).
        unsafe {
            core::ptr::write_volatile((r.base() + off) as *mut IoUringCqe, cqe);
        }
    }
}

// ==================== CQE Posting ====================

/// Post a completion queue entry. `pub` since the policy flip: the
/// io_uring crate's submit loop calls it (completion-queue
/// machinery — stays mechanism-side with the wakeup path).
pub fn io_uring_post_cqe(ring: &IoUring, user_data: u64, res: i32, flags: u32) {
    let _lock = ring.cq_lock.lock();

    let cq = ring.cq_hdr();
    let head = cq.read_u32(ring.cq_head_off);
    let tail = cq.read_u32(ring.cq_tail_off);

    // Check if CQ is full (head == tail means empty, so capacity = cq_entries - 1)
    if tail.wrapping_sub(head) >= ring.cq_entries {
        // CQ overflow: increment overflow counter visible to userspace
        let count = cq.read_u32(ring.cq_overflow_off);
        cq.write_u32(ring.cq_overflow_off, count + 1);
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        return;
    }

    // Write CQE at tail position
    ring.cqe_write((tail as usize & ring.cq_ring_mask as usize), IoUringCqe { user_data, res, flags });

    // Advance cq_tail
    let new_tail = tail.wrapping_add(1);
    cq.write_u32(ring.cq_tail_off, new_tail);
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);

    // Signal eventfd if registered (use the pinned file reference, not the
    // raw fd number — the fd may have been closed and reused in between)
    if let Some(efile) = ring.eventfd_file.lock().clone() {
        signal_eventfd_file(&efile);
    }
}

/// Signal the registered eventfd by writing 1.
fn signal_eventfd_file(file: &alloc::sync::Arc<File>) {
    let one: [u8; 8] = 1u64.to_le_bytes();
    let ops = match file.get_ops() {
        Some(o) => o,
        None => return,
    };
    if let Some(write_fn) = ops.write {
        let _ = write_fn(file, &one);
    }
}

// ==================== CQ Wait / Public Syscall API ====================
// MOVED to the io_uring service crate (the cold-four lane):
// wait_for_cqes, sys_io_uring_setup, sys_io_uring_enter and
// sys_io_uring_register are syscall-context policy — their only callers
// are syscall's NR 425/426/427 wrappers, which now reach them
// through THIS crate's re-export chain (syscall -> io_uring ->
// rux, the allowed downward edge; the worklist marked them hot only
// because the scanner cannot model the caller's import switch). The
// unsafe the moved functions carried was converted at the flip:
// - the error-path Box::from_raw pairs in sys_io_uring_setup ride the
//   framework's audited ring_release_on_install_error edge
//   (INV-IOURING-RING-2);
// - the private_data reads in enter/register ride pin_verified_ring
//   (the R7-C4 lock-discipline twin pin_ring already used);
// - the raw read_volatile head/tail pair rides the cq_completed()
//   window-checked accessor.
