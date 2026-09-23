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

// Opcodes
const IORING_OP_NOP:     u8 = 0;
const IORING_OP_FSYNC:   u8 = 3;
const IORING_OP_CLOSE:   u8 = 14;
const IORING_OP_READ:    u8 = 22;
const IORING_OP_WRITE:   u8 = 23;
const IORING_OP_FADVISE: u8 = 28;

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

// io_uring_enter flags
const IORING_ENTER_GETEVENTS: u32 = 1 << 0;

// Register opcodes
const IORING_REGISTER_EVENTFD:   u32 = 4;
const IORING_UNREGISTER_EVENTFD: u32 = 5;

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
    fn try_ref(&self) -> bool {
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
        crate::arch::riscv64::mm::phys_to_virt(
            crate::arch::riscv64::mm::PhysAddr::new(phys as u64),
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

/// Create a new io_uring instance.
fn io_uring_create(entries: u32, params: &mut IoUringParams) -> Result<Box<IoUring>, i32> {
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

    // Write SQ ring header
    unsafe {
        let base = sq_ring.kvirt;
        core::ptr::write_volatile(base.add(0) as *mut u32, 0);   // head
        core::ptr::write_volatile(base.add(4) as *mut u32, 0);   // tail
        core::ptr::write_volatile(base.add(8) as *mut u32, sq_entries - 1); // ring_mask
        core::ptr::write_volatile(base.add(12) as *mut u32, sq_entries);     // ring_entries
        core::ptr::write_volatile(base.add(16) as *mut u32, 0);  // flags
        core::ptr::write_volatile(base.add(20) as *mut u32, 0);  // dropped
    }

    // Write CQ ring header
    unsafe {
        let base = cq_ring.kvirt;
        core::ptr::write_volatile(base.add(0) as *mut u32, 0);   // head
        core::ptr::write_volatile(base.add(4) as *mut u32, 0);   // tail
        core::ptr::write_volatile(base.add(8) as *mut u32, cq_entries - 1); // ring_mask
        core::ptr::write_volatile(base.add(12) as *mut u32, cq_entries);     // ring_entries
        core::ptr::write_volatile(base.add(16) as *mut u32, 0);  // overflow
        core::ptr::write_volatile(base.add(20) as *mut u32, 0);  // flags
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

/// Serializes the private_data lifecycle: close() clears the pointer and
/// drops the installation reference under this lock, while enter/register/
/// mmap read the pointer and try_ref under the same lock. The lock lives
/// outside the IoUring because close's final unref may free the ring.
static LIFECYCLE_LOCK: Spinlock<()> = Spinlock::new(());

/// Acquire a pinned reference to the ring of an ops-verified io_uring file.
/// Returns None if the ring is absent or being torn down.
fn pin_ring(file: &File) -> Option<RingRef> {
    let _guard = LIFECYCLE_LOCK.lock();
    let ptr = unsafe { *file.private_data.get() }? as *const IoUring;
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
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        // Clear FIRST so no new reference can be acquired, then drop ours;
        // the final unref (which may free the ring) runs under the lock.
        // SAFETY: ptr came from Box::into_raw at install time.
        unsafe { *file.private_data.get() = None; }
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

pub static IO_URING_OPS: FileOps = FileOps {
    read: None,
    write: None,
    lseek: None,
    close: Some(io_uring_close),
    poll: Some(io_uring_poll),
};

// ==================== mmap Handler ====================

/// Handle mmap on an io_uring fd — maps ring buffers to userspace.
/// `file` must already be ops-verified as an io_uring file by the caller
/// (passing the fd here again would re-fetch it — a TOCTOU type confusion).
pub fn io_uring_mmap_handler(
    file: &File, addr: usize, length: usize, offset: u64, prot: u32,
) -> Result<usize, i32> {
    use crate::arch::riscv64::mm::{PageTableEntry, VirtAddr, PhysAddr, map_page};
    use crate::mm::vma::{Vma, VmaFlags};
    use crate::mm::page::VirtAddr as PageVirtAddr;

    // R7-C4: read the pointer INSIDE the lock (was read before, deref
    // after — a concurrent close() could free the ring in between).
    let (ring, _ring_guard) = {
        let _guard = LIFECYCLE_LOCK.lock();
        let ring_ptr = unsafe { *file.private_data.get() }.ok_or(-9)?;
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
        match addr_space.find_free_area(region.size) {
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
        let user_end = crate::arch::riscv64::mm::user_addr::USER_END;
        let end = vaddr.checked_add(region.size).ok_or(-22)?;
        if end > user_end {
            return Err(-22);
        }
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
        core::arch::asm!("sfence.vma");
    }

    let mut vma_flags = VmaFlags::new();
    vma_flags.insert(VmaFlags::READ);
    vma_flags.insert(VmaFlags::WRITE);
    vma_flags.insert(VmaFlags::SHARED);

    let vma = Vma::new(
        PageVirtAddr::new(vaddr),
        PageVirtAddr::new(vaddr + region.size),
        vma_flags,
    );
    if addr_space.vma_write().add(vma).is_err() {
        return Err(-12);
    }

    Ok(vaddr)
}

// ==================== Submission Processing ====================

/// Submit and process SQEs from the submission queue.
fn submit_sqes(ring: &IoUring, to_submit: u32) -> u32 {
    let mut submitted = 0u32;

    // Read head and tail ONCE (per Linux io_uring_submit_sqes).  A malicious
    // userspace could modify the shared ring between iterations (TOCTOU),
    // causing double-processing or skipping of SQEs.  We advance a local
    // counter and write back only after the loop finishes.
    let mut head = unsafe {
        core::ptr::read_volatile(ring.sq_ring.kvirt.add(ring.sq_head_off) as *const u32)
    };
    let tail = unsafe {
        core::ptr::read_volatile(ring.sq_ring.kvirt.add(ring.sq_tail_off) as *const u32)
    };

    // Cap to actual available SQEs
    let available = tail.wrapping_sub(head);
    let batch = core::cmp::min(to_submit, available);

    for _ in 0..batch {
        // Read SQE index from the array
        let array_base = unsafe { ring.sq_ring.kvirt.add(ring.sq_array_off) as *const u32 };
        let sqe_idx = unsafe {
            core::ptr::read_volatile(array_base.add((head & ring.sq_ring_mask) as usize))
        };

        // Validate SQE index against sq_entries (per Linux:
        // READ_ONCE(ring->array[i]) < ctx->sq_entries).
        if sqe_idx >= ring.sq_entries {
            break; // Invalid index — stop processing
        }

        // Read the SQE
        let sqe_ptr = unsafe { ring.sqes.kvirt.add(sqe_idx as usize * 64) as *const IoUringSqe };
        let sqe = unsafe { core::ptr::read_volatile(sqe_ptr) };

        // Advance local head
        head = head.wrapping_add(1);

        // Execute operation synchronously
        let res = io_uring_dispatch_op(&sqe);

        // Post CQE
        io_uring_post_cqe(ring, sqe.user_data, res, 0);

        submitted += 1;
    }

    // Write back the advanced head only after all submissions are complete
    if submitted > 0 {
        unsafe {
            core::ptr::write_volatile(
                ring.sq_ring.kvirt.add(ring.sq_head_off) as *mut u32,
                head,
            );
        }
    }

    submitted
}

// ==================== Operation Dispatch ====================

/// Dispatch a single SQE to the appropriate operation handler.
fn io_uring_dispatch_op(sqe: &IoUringSqe) -> i32 {
    match sqe.opcode {
        IORING_OP_NOP => 0,
        IORING_OP_READ => io_uring_op_read(sqe),
        IORING_OP_WRITE => io_uring_op_write(sqe),
        IORING_OP_FSYNC => io_uring_op_fsync(sqe),
        IORING_OP_CLOSE => io_uring_op_close(sqe),
        IORING_OP_FADVISE => io_uring_op_fadvise(sqe),
        _ => -22, // EINVAL
    }
}

/// IORING_OP_READ: read from fd into user buffer.
fn io_uring_op_read(sqe: &IoUringSqe) -> i32 {
    use crate::arch::riscv64::uaccess::access_ok;

    let fd = sqe.fd as usize;
    let buf = sqe.addr as usize;
    // Cap the transfer length: sqe.len is user-controlled (up to 4GB) and the
    // kernel heap cannot satisfy huge allocations (alloc failure would panic).
    let len = (sqe.len as usize).min(crate::syscall::io::MAX_RW_COUNT);
    let off = sqe.off as i64;

    if len == 0 { return 0; }
    if !access_ok(buf, len) { return -14; } // EFAULT

    let file = match unsafe { crate::fs::file::get_file_fd(fd) } {
        Some(f) => f,
        None => return -9, // EBADF
    };

    // Use file position if off == -1
    let use_file_pos = off == -1;

    if use_file_pos {
        // read_fn (e.g. file_read in fs/file.rs) reads from file.pos and
        // advances it by the number of bytes read. No manual pos update
        // needed — the old code double-counted by adding result again.
        do_read(&file, buf, len)
    } else {
        // pread: read from a specific offset without changing file position.
        let saved_pos = file.get_pos();
        let _ = file.set_pos(off as u64);
        let result = do_read(&file, buf, len);
        let _ = file.set_pos(saved_pos);
        result
    }
}

fn do_read(file: &Arc<File>, buf: usize, len: usize) -> i32 {
    let ops = match file.get_ops() {
        Some(o) => o,
        None => return -9, // EBADF
    };
    let read_fn = match ops.read {
        Some(f) => f,
        None => return -22, // EINVAL
    };

    // R20-FS9 (SYSA-C1 family, io_uring leftover): stage at most RW_CHUNK at
    // a time — a single `vec![0u8; len]` with user len up to MAX_RW_COUNT
    // (2GB) far exceeds the kernel heap and panics on allocation failure.
    // Matches the chunked staging the syscall read/write paths already use.
    const RW_CHUNK: usize = crate::syscall::io::RW_CHUNK;
    let mut kbuf = alloc::vec![0u8; len.min(RW_CHUNK)];
    let mut total = 0usize;

    while total < len {
        let chunk = core::cmp::min(len - total, kbuf.len());
        let n = read_fn(file, &mut kbuf[..chunk]);
        if n <= 0 {
            return if total > 0 { total as i32 } else { n as i32 };
        }
        let uncopied = unsafe {
            crate::arch::riscv64::uaccess::copy_to_user(
                (buf + total) as *mut u8,
                kbuf.as_ptr(),
                n as usize,
            )
        };
        if uncopied != 0 {
            return if total > 0 { total as i32 } else { -14 }; // EFAULT
        }
        total += n as usize;
        // R24: short chunk means EOF or "nothing more available now" (pipe
        // drained / socket drained) — sys_read's chunk loop breaks here;
        // without the break an io_uring read larger than RW_CHUNK from a
        // pipe blocks for a second fill instead of returning the partial
        // data (do_write already had the matching short-write break).
        if (n as usize) < chunk {
            break;
        }
    }

    total as i32
}

/// IORING_OP_WRITE: write from user buffer to fd.
fn io_uring_op_write(sqe: &IoUringSqe) -> i32 {
    use crate::arch::riscv64::uaccess::access_ok;

    let fd = sqe.fd as usize;
    let buf = sqe.addr as usize;
    // Cap the transfer length: sqe.len is user-controlled (up to 4GB) and the
    // kernel heap cannot satisfy huge allocations (alloc failure would panic).
    let len = (sqe.len as usize).min(crate::syscall::io::MAX_RW_COUNT);
    let off = sqe.off as i64;

    if len == 0 { return 0; }
    if !access_ok(buf, len) { return -14; } // EFAULT

    let file = match unsafe { crate::fs::file::get_file_fd(fd) } {
        Some(f) => f,
        None => return -9, // EBADF
    };

    let use_file_pos = off == -1;

    if use_file_pos {
        // write_fn (e.g. file_write in fs/file.rs) writes from file.pos and
        // advances it. No manual pos update needed — the old code
        // double-counted by adding result again.
        do_write(&file, buf, len)
    } else {
        // pwrite: write at a specific offset without changing file position.
        let saved_pos = file.get_pos();
        let _ = file.set_pos(off as u64);
        let result = do_write(&file, buf, len);
        let _ = file.set_pos(saved_pos);
        result
    }
}

fn do_write(file: &Arc<File>, buf: usize, len: usize) -> i32 {
    let ops = match file.get_ops() {
        Some(o) => o,
        None => return -9,
    };
    let write_fn = match ops.write {
        Some(f) => f,
        None => return -22,
    };

    // R20-FS9: chunked staging, same rationale as do_read.
    const RW_CHUNK: usize = crate::syscall::io::RW_CHUNK;
    let mut kbuf = alloc::vec![0u8; len.min(RW_CHUNK)];
    let mut total = 0usize;

    while total < len {
        let chunk = core::cmp::min(len - total, kbuf.len());
        let uncopied = unsafe {
            crate::arch::riscv64::uaccess::copy_from_user(
                kbuf.as_mut_ptr(),
                (buf + total) as *const u8,
                chunk,
            )
        };
        if uncopied != 0 {
            return if total > 0 { total as i32 } else { -14 }; // EFAULT
        }
        let n = write_fn(file, &kbuf[..chunk]);
        if n <= 0 {
            return if total > 0 { total as i32 } else { n as i32 };
        }
        total += n as usize;
        if (n as usize) < chunk {
            break; // short write — stop
        }
    }

    total as i32
}

/// IORING_OP_FSYNC: sync file to disk.
fn io_uring_op_fsync(_sqe: &IoUringSqe) -> i32 {
    // No real fsync in Rux (ramdisk/ramfs)
    0
}

/// IORING_OP_CLOSE: close a file descriptor.
///
/// Rejects attempts to close the io_uring ring fd itself to prevent
/// use-after-free of the ring structures.
fn io_uring_op_close(sqe: &IoUringSqe) -> i32 {
    let fd = sqe.fd as usize;

    // Guard: refuse to close a file whose ops match IO_URING_OPS, as that
    // would be the ring fd itself. Closing it would leave the ring mapped
    // in user memory but with freed kernel structures — a UAF.
    if let Some(file) = unsafe { crate::fs::file::get_file_fd(fd) } {
        if let Some(ops) = file.get_ops() {
            if core::ptr::eq(ops, &IO_URING_OPS as *const FileOps) {
                return -22; // EINVAL
            }
        }
    }

    match unsafe { crate::fs::file::close_file_fd(fd) } {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// IORING_OP_FADVISE: advise on file access pattern (ignored).
fn io_uring_op_fadvise(_sqe: &IoUringSqe) -> i32 {
    0
}

// ==================== CQE Posting ====================

/// Post a completion queue entry.
fn io_uring_post_cqe(ring: &IoUring, user_data: u64, res: i32, flags: u32) {
    let _lock = ring.cq_lock.lock();

    let head = unsafe {
        core::ptr::read_volatile(ring.cq_ring.kvirt.add(ring.cq_head_off) as *const u32)
    };
    let tail = unsafe {
        core::ptr::read_volatile(ring.cq_ring.kvirt.add(ring.cq_tail_off) as *const u32)
    };

    // Check if CQ is full (head == tail means empty, so capacity = cq_entries - 1)
    if tail.wrapping_sub(head) >= ring.cq_entries {
        // CQ overflow: increment overflow counter visible to userspace
        unsafe {
            let overflow_ptr = ring.cq_ring.kvirt.add(ring.cq_overflow_off) as *mut u32;
            let count = core::ptr::read_volatile(overflow_ptr);
            core::ptr::write_volatile(overflow_ptr, count + 1);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        return;
    }

    // Write CQE at tail position
    let cqe_offset = ring.cq_cqes_off + (tail as usize & ring.cq_ring_mask as usize) * 16;
    let cqe_ptr = unsafe { ring.cq_ring.kvirt.add(cqe_offset) as *mut IoUringCqe };
    unsafe {
        core::ptr::write_volatile(cqe_ptr, IoUringCqe { user_data, res, flags });
    }

    // Advance cq_tail
    let new_tail = tail.wrapping_add(1);
    unsafe {
        core::ptr::write_volatile(
            ring.cq_ring.kvirt.add(ring.cq_tail_off) as *mut u32,
            new_tail,
        );
    }
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

// ==================== CQ Wait ====================

/// Wait for at least `min_complete` CQEs.
///
/// Since operations execute synchronously, CQEs are already posted
/// before this is called. The first check will always succeed.
fn wait_for_cqes(ring: &IoUring, min_complete: u32) -> i32 {
    for _ in 0..1000 {
        let head = unsafe {
            core::ptr::read_volatile(ring.cq_ring.kvirt.add(ring.cq_head_off) as *const u32)
        };
        let tail = unsafe {
            core::ptr::read_volatile(ring.cq_ring.kvirt.add(ring.cq_tail_off) as *const u32)
        };

        let completed = tail.wrapping_sub(head);
        if completed >= min_complete {
            return completed as i32;
        }

        if crate::signal::signal_pending() {
            return -4; // EINTR
        }

        crate::sched::yield_cpu();
    }

    // Timeout (should not happen with sync ops)
    -110 // ETIMEDOUT
}

// ==================== Public Syscall API ====================

/// sys_io_uring_setup — create a new io_uring instance (NR 425).
pub fn sys_io_uring_setup(args: [u64; 6]) -> u64 {
    use crate::arch::riscv64::uaccess::{access_ok, get_user, put_user};

    let entries = args[0] as u32;
    let params_ptr = args[1] as *mut IoUringParams;

    if params_ptr.is_null() {
        return -(22i64) as u64; // EINVAL
    }
    if !access_ok(params_ptr as usize, core::mem::size_of::<IoUringParams>()) {
        return -(14i64) as u64; // EFAULT
    }

    let mut params = match unsafe { get_user::<IoUringParams>(params_ptr) } {
        Some(p) => p,
        None => return -(14i64) as u64,
    };

    let ring = match io_uring_create(entries, &mut params) {
        Ok(r) => r,
        // `e` is already a NEGATIVE errno (io_uring_create returns Err(-12)
        // etc.) — do NOT negate again (review批次6 P2: the double negation
        // turned ENOMEM into a fake "returned fd 12").
        Err(e) => return (e as i64) as u64,
    };

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(12i64) as u64, // ENOMEM
    };

    let ring_ptr = Box::into_raw(ring) as *mut u8;
    let file = Arc::new(File::new(FileFlags::new(FileFlags::O_RDWR)));
    file.set_ops(&IO_URING_OPS);
    file.set_private_data(ring_ptr);

    let fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            unsafe { let _ = Box::from_raw(ring_ptr as *mut IoUring); }
            return -(24i64) as u64; // EMFILE
        }
    };

    match fdtable.install_fd(fd, file) {
        Ok(()) => {}
        Err(_) => {
            unsafe { let _ = Box::from_raw(ring_ptr as *mut IoUring); }
            return -(12i64) as u64;
        }
    }

    // Copy updated params back
    unsafe { let _ = put_user(params_ptr, params); }

    fd as u64
}

/// sys_io_uring_enter — submit SQEs and/or wait for CQEs (NR 426).
pub fn sys_io_uring_enter(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let to_submit = args[1] as u32;
    let min_complete = args[2] as u32;
    let flags = args[3] as u32;

    // Only IORING_ENTER_GETEVENTS is supported
    if flags & !IORING_ENTER_GETEVENTS != 0 {
        return -(22i64) as u64;
    }

    let file = match unsafe { crate::fs::file::get_file_fd(fd as usize) } {
        Some(f) => f,
        None => return -(9i64) as u64, // EBADF
    };

    // Verify the fd really is an io_uring instance before treating its
    // private_data as a ring pointer — any other file type here would be a
    // type confusion (arbitrary kernel read/write).
    if !file.get_ops().is_some_and(|o| core::ptr::eq(o, &IO_URING_OPS as *const _)) {
        return -(9i64) as u64; // EBADF
    }

    // R7-C4 (IOU-H1 completion): the private_data pointer must be read
    // INSIDE the lifecycle lock — reading it before and dereferencing
    // after let a concurrent close() free the ring in between. Same
    // discipline as pin_ring().
    let (ring, _ring_guard) = {
        let _guard = LIFECYCLE_LOCK.lock();
        let ring_ptr = match unsafe { *file.private_data.get() } {
            Some(p) => p as *const IoUring,
            None => return -(9i64) as u64, // EBADF
        };
        // SAFETY: under the lifecycle lock, close() cannot clear+free the
        // ring between the read and try_ref.
        let ring = unsafe { &*ring_ptr };
        if !ring.try_ref() {
            return -(9i64) as u64; // EBADF: ring being torn down
        }
        (ring, RingRef(ring_ptr))
    };

    // Submit SQEs
    let submitted = submit_sqes(ring, to_submit);

    // Wait for completions if requested
    if flags & IORING_ENTER_GETEVENTS != 0 && min_complete > 0 {
        let result = wait_for_cqes(ring, min_complete);
        if result < 0 {
            // result is already a negative errno — no double negation
            // (review批次6 P2).
            return (result as i64) as u64;
        }
    }

    // Linux io_uring_enter returns the number of CQEs ready (post-wait)
    // when nothing was submitted — previously a pure-enter wait call
    // always returned 0 and callers could not distinguish "woke with N
    // completions" from "timeout/nothing" (review批次6 M).
    if submitted == 0 {
        let head = unsafe {
            core::ptr::read_volatile(ring.cq_ring.kvirt.add(ring.cq_head_off) as *const u32)
        };
        let tail = unsafe {
            core::ptr::read_volatile(ring.cq_ring.kvirt.add(ring.cq_tail_off) as *const u32)
        };
        (tail.wrapping_sub(head)) as u64
    } else {
        submitted as u64
    }
}

/// sys_io_uring_register — register buffers/files/eventfd (NR 427).
pub fn sys_io_uring_register(args: [u64; 6]) -> u64 {
    let fd = args[0] as i32;
    let opcode = args[1] as u32;
    let arg = args[2] as u64;
    let nr_args = args[3] as u32;

    let file = match unsafe { crate::fs::file::get_file_fd(fd as usize) } {
        Some(f) => f,
        None => return -(9i64) as u64, // EBADF
    };

    // Verify the fd really is an io_uring instance (type-confusion guard,
    // same as sys_io_uring_enter).
    if !file.get_ops().is_some_and(|o| core::ptr::eq(o, &IO_URING_OPS as *const _)) {
        return -(9i64) as u64; // EBADF
    }

    // R7-C4 (IOU-H1 completion): the private_data pointer must be read
    // INSIDE the lifecycle lock — reading it before and dereferencing
    // after let a concurrent close() free the ring in between. Same
    // discipline as pin_ring().
    let (ring, _ring_guard) = {
        let _guard = LIFECYCLE_LOCK.lock();
        let ring_ptr = match unsafe { *file.private_data.get() } {
            Some(p) => p as *const IoUring,
            None => return -(9i64) as u64, // EBADF
        };
        // SAFETY: under the lifecycle lock, close() cannot clear+free the
        // ring between the read and try_ref.
        let ring = unsafe { &*ring_ptr };
        if !ring.try_ref() {
            return -(9i64) as u64; // EBADF: ring being torn down
        }
        (ring, RingRef(ring_ptr))
    };

    
    match opcode {
        IORING_REGISTER_EVENTFD => {
            if nr_args != 1 { return -(22i64) as u64; }
            let eventfd_fd = arg as i32;
            if eventfd_fd < 0 { return -(9i64) as u64; }
            // Hold the actual file reference (immune to fd reuse) and verify
            // it really is an eventfd — anything else would make completion
            // notification write garbage into an unrelated file.
            let efile = match unsafe { crate::fs::file::get_file_fd(eventfd_fd as usize) } {
                Some(f) => f,
                None => return -(9i64) as u64,
            };
            if !efile.get_ops().is_some_and(|o| core::ptr::eq(
                o, &crate::syscall::misc::EVENTFD_OPS as *const _,
            )) {
                return -(22i64) as u64; // EINVAL
            }
            ring.eventfd_fd.store(eventfd_fd, core::sync::atomic::Ordering::Release);
            *ring.eventfd_file.lock() = Some(efile);
            0
        }
        IORING_UNREGISTER_EVENTFD => {
            ring.eventfd_fd.store(-1, core::sync::atomic::Ordering::Release);
            *ring.eventfd_file.lock() = None;
            0
        }
        _ => -(22i64) as u64, // EINVAL
    }
}
