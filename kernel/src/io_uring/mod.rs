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

    cq_lock: Spinlock<()>,
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
    free_pages(region.phys, order);
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

    // Fill in params
    params.sq_entries = sq_entries;
    params.cq_entries = cq_entries;
    params.features = IORING_FEAT_SINGLE_MMAP
        | IORING_FEAT_SUBMIT_STABLE
        | IORING_FEAT_RW_CUR_POS;

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
        cq_lock: Spinlock::new(()),
    });

    Ok(ring)
}

// ==================== FileOps ====================

fn io_uring_close(file: &File) -> i32 {
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        unsafe {
            let _ = Box::from_raw(ptr as *mut IoUring);
        }
        unsafe { *file.private_data.get() = None; }
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
pub fn io_uring_mmap_handler(
    fd: i32, addr: usize, length: usize, offset: u64, prot: u32,
) -> Result<usize, i32> {
    use crate::arch::riscv64::mm::{PageTableEntry, VirtAddr, PhysAddr, map_page};
    use crate::mm::vma::{Vma, VmaFlags};
    use crate::mm::page::VirtAddr as PageVirtAddr;

    let file = unsafe { crate::fs::file::get_file_fd(fd as usize) }.ok_or(-9)?; // EBADF
    let ring_ptr = unsafe { *file.private_data.get() }.ok_or(-9)?;
    let ring = unsafe { &*(ring_ptr as *const IoUring) };

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

    let vaddr = if addr == 0 {
        crate::arch::riscv64::mm::user_addr::MMAP_START
    } else {
        addr & !(PAGE_SIZE - 1)
    };

    unsafe {
        let mut pte_flags = PageTableEntry::V | PageTableEntry::U
            | PageTableEntry::A | PageTableEntry::D;
        if prot & 0x1 != 0 { pte_flags |= PageTableEntry::R; }
        if prot & 0x2 != 0 { pte_flags |= PageTableEntry::R | PageTableEntry::W; }

        for i in 0..region.npages {
            let va = vaddr + i * PAGE_SIZE;
            let pa = region.phys + i * PAGE_SIZE;
            map_page(user_ppn, VirtAddr::new(va as u64), PhysAddr::new(pa as u64), pte_flags);
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

    for _ in 0..to_submit {
        // Read sq_head and sq_tail
        let head = unsafe {
            core::ptr::read_volatile(ring.sq_ring.kvirt.add(ring.sq_head_off) as *const u32)
        };
        let tail = unsafe {
            core::ptr::read_volatile(ring.sq_ring.kvirt.add(ring.sq_tail_off) as *const u32)
        };

        if head == tail { break; }

        // Read SQE index from the array
        let array_base = unsafe { ring.sq_ring.kvirt.add(ring.sq_array_off) as *const u32 };
        let sqe_idx = unsafe {
            core::ptr::read_volatile(array_base.add((head & ring.sq_ring_mask) as usize))
        };

        // Read the SQE
        let sqe_ptr = unsafe { ring.sqes.kvirt.add(sqe_idx as usize * 64) as *const IoUringSqe };
        let sqe = unsafe { core::ptr::read_volatile(sqe_ptr) };

        // Advance sq_head
        let new_head = head.wrapping_add(1);
        unsafe {
            core::ptr::write_volatile(
                ring.sq_ring.kvirt.add(ring.sq_head_off) as *mut u32,
                new_head,
            );
        }

        // Execute operation synchronously
        let res = io_uring_dispatch_op(&sqe);

        // Post CQE
        io_uring_post_cqe(ring, sqe.user_data, res, 0);

        submitted += 1;
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
    let len = sqe.len as usize;
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
        let saved_pos = file.get_pos();
        let result = do_read(&file, buf, len);
        if result > 0 {
            let _ = file.set_pos(saved_pos + result as u64);
        }
        result
    } else {
        let _ = file.set_pos(off as u64);
        do_read(&file, buf, len)
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

    // Use a stack buffer for small reads, heap for large
    let mut kbuf = alloc::vec![0u8; len];
    let n = read_fn(file, &mut kbuf);
    if n <= 0 {
        return n as i32;
    }

    let uncopied = unsafe { crate::arch::riscv64::uaccess::copy_to_user(buf as *mut u8, kbuf.as_ptr(), n as usize) };
    if uncopied != 0 {
        return -14; // EFAULT
    }

    n as i32
}

/// IORING_OP_WRITE: write from user buffer to fd.
fn io_uring_op_write(sqe: &IoUringSqe) -> i32 {
    use crate::arch::riscv64::uaccess::access_ok;

    let fd = sqe.fd as usize;
    let buf = sqe.addr as usize;
    let len = sqe.len as usize;
    let off = sqe.off as i64;

    if len == 0 { return 0; }
    if !access_ok(buf, len) { return -14; } // EFAULT

    let file = match unsafe { crate::fs::file::get_file_fd(fd) } {
        Some(f) => f,
        None => return -9, // EBADF
    };

    let use_file_pos = off == -1;

    // Copy user data to kernel buffer
    let mut kbuf = alloc::vec![0u8; len];
    let uncopied = unsafe { crate::arch::riscv64::uaccess::copy_from_user(kbuf.as_mut_ptr(), buf as *const u8, len) };
    if uncopied != 0 { return -14; }

    if use_file_pos {
        let saved_pos = file.get_pos();
        let result = do_write(&file, &kbuf);
        if result > 0 {
            let _ = file.set_pos(saved_pos + result as u64);
        } else {
            let _ = file.set_pos(saved_pos);
        }
        result
    } else {
        let _ = file.set_pos(off as u64);
        do_write(&file, &kbuf)
    }
}

fn do_write(file: &Arc<File>, kbuf: &[u8]) -> i32 {
    let ops = match file.get_ops() {
        Some(o) => o,
        None => return -9,
    };
    let write_fn = match ops.write {
        Some(f) => f,
        None => return -22,
    };

    write_fn(file, kbuf) as i32
}

/// IORING_OP_FSYNC: sync file to disk.
fn io_uring_op_fsync(_sqe: &IoUringSqe) -> i32 {
    // No real fsync in Rux (ramdisk/ramfs)
    0
}

/// IORING_OP_CLOSE: close a file descriptor.
fn io_uring_op_close(sqe: &IoUringSqe) -> i32 {
    let fd = sqe.fd as usize;
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

    // Signal eventfd if registered
    let efd = ring.eventfd_fd.load(core::sync::atomic::Ordering::Acquire);
    if efd >= 0 {
        signal_eventfd(efd as usize);
    }
}

/// Signal the registered eventfd by writing 1.
fn signal_eventfd(fd: usize) {
    if let Some(file) = unsafe { crate::fs::file::get_file_fd(fd) } {
        let one: [u8; 8] = 1u64.to_le_bytes();
        let ops = match file.get_ops() {
            Some(o) => o,
            None => return,
        };
        if let Some(write_fn) = ops.write {
            let _ = write_fn(&file, &one);
        }
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
        Err(e) => return -(e as i64) as u64,
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

    let ring_ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p as *const IoUring,
        None => return -(9i64) as u64,
    };
    let ring = unsafe { &*ring_ptr };

    // Submit SQEs
    let submitted = submit_sqes(ring, to_submit);

    // Wait for completions if requested
    if flags & IORING_ENTER_GETEVENTS != 0 && min_complete > 0 {
        let result = wait_for_cqes(ring, min_complete);
        if result < 0 {
            return -(result as i64) as u64;
        }
    }

    submitted as u64
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

    let ring_ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p as *const IoUring,
        None => return -(9i64) as u64,
    };
    let ring = unsafe { &*ring_ptr };

    match opcode {
        IORING_REGISTER_EVENTFD => {
            if nr_args != 1 { return -(22i64) as u64; }
            let eventfd_fd = arg as i32;
            if eventfd_fd < 0 { return -(9i64) as u64; }
            // Validate eventfd exists
            if unsafe { crate::fs::file::get_file_fd(eventfd_fd as usize) }.is_none() {
                return -(9i64) as u64;
            }
            ring.eventfd_fd.store(eventfd_fd, core::sync::atomic::Ordering::Release);
            0
        }
        IORING_UNREGISTER_EVENTFD => {
            ring.eventfd_fd.store(-1, core::sync::atomic::Ordering::Release);
            0
        }
        _ => -(22i64) as u64, // EINVAL
    }
}
