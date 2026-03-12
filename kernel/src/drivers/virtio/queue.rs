//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO virtual queue
//!
//! Queue implementation fully compliant with VirtIO specification

use core::sync::atomic::{AtomicU16, Ordering};

/// VirtIO descriptor (16-byte aligned)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Desc {
    /// Address (64-bit)
    pub addr: u64,
    /// Length (32-bit)
    pub len: u32,
    /// Flags (16-bit)
    pub flags: u16,
    /// Next (16-bit)
    pub next: u16,
}

/// Available Ring (2-byte aligned)
#[repr(C)]
pub struct AvailRing {
    /// Flags
    pub flags: u16,
    /// Driver writes next available descriptor index (volatile read/write)
    pub idx: u16,
    // Descriptor index array starts here
    // Array followed by used_event_idx
}

/// Used Ring element (4-byte aligned)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UsedElem {
    /// Descriptor index
    pub id: u32,
    /// Bytes written
    pub len: u32,
}

/// Used Ring (4-byte aligned)
#[repr(C)]
pub struct UsedRing {
    /// Flags
    pub flags: u16,
    /// Device writes next available descriptor index (volatile read/write)
    pub idx: u16,
    // Element array starts here
    // Array followed by avail_event_idx
}

/// VirtIO virtual queue
///
/// Uses Modern VirtIO (v1.0+) layout
pub struct VirtQueue {
    /// Queue size
    pub queue_size: u16,
    /// Queue index (used for notifying device)
    queue_index: u16,
    /// Queue notification address
    queue_notify: u64,
    /// Interrupt status address (VIRTIO_MMIO_INTERRUPT_STATUS - Read Only)
    interrupt_status: u64,
    /// Interrupt acknowledge address (VIRTIO_MMIO_INTERRUPT_ACK - Write Only)
    interrupt_ack: u64,
    /// Descriptor table pointer (at start of contiguous memory block)
    pub(crate) desc: *mut Desc,
    /// Available Ring pointer
    pub(crate) avail: *mut AvailRing,
    /// Used Ring pointer
    pub(crate) used: *mut UsedRing,
    /// vring address
    vring_addr: u64,
    /// Next descriptor index to allocate
    next_desc: AtomicU16,
}

unsafe impl Send for VirtQueue {}
unsafe impl Sync for VirtQueue {}

impl VirtQueue {
    /// Create new VirtQueue (using contiguous memory layout)
    ///
    /// # Parameters
    /// - `queue_size`: Queue size (must be power of 2)
    /// - `queue_index`: Queue index (written when notifying device)
    /// - `queue_notify`: Queue notification register address
    /// - `interrupt_status`: Interrupt status register address
    /// - `interrupt_ack`: Interrupt acknowledge register address
    pub fn new(queue_size: u16, queue_index: u16, queue_notify: u64, interrupt_status: u64, interrupt_ack: u64) -> Option<Self> {
        let desc_size = queue_size as usize * 16;
        let avail_size = 2 + 2 + queue_size as usize * 2 + 2;
        let used_size = 2 + 2 + queue_size as usize * 8 + 2;

        // VirtIO 1.0 specification: descriptor table, available ring, and used ring must be page-aligned (at least 4096 bytes)
        const PAGE_SIZE: usize = 4096;

        let desc_size_aligned = (desc_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let avail_size_aligned = (avail_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let used_size_aligned = (used_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        let total_size = desc_size_aligned + avail_size_aligned + used_size_aligned;

        let layout = alloc::alloc::Layout::from_size_align(total_size, PAGE_SIZE).ok()?;
        let mem_ptr = unsafe { alloc::alloc::alloc(layout) as *mut u8 };
        if mem_ptr.is_null() {
            return None;
        }

        let addr = mem_ptr as usize;
        if addr & (PAGE_SIZE - 1) != 0 {
            crate::println!("virtio: ERROR: vring not page-aligned!");
            unsafe { alloc::alloc::dealloc(mem_ptr, layout) };
            return None;
        }

        let desc = mem_ptr as *mut Desc;
        let avail = unsafe { (mem_ptr as usize + desc_size_aligned) as *mut AvailRing };
        let used = unsafe { (mem_ptr as usize + desc_size_aligned + avail_size_aligned) as *mut UsedRing };

        unsafe {
            (*avail).flags = 0;
            (*avail).idx = 0;
            (*used).flags = 0;
            (*used).idx = 0;
        }

        for i in 0..queue_size {
            unsafe {
                *desc.add(i as usize) = Desc { addr: 0, len: 0, flags: 0, next: 0 };
            }
        }

        Some(Self {
            queue_size,
            queue_index,
            queue_notify,
            interrupt_status,
            interrupt_ack,
            desc,
            avail,
            used,
            vring_addr: mem_ptr as u64,
            next_desc: AtomicU16::new(0),
        })
    }

    /// Get current available index
    pub fn get_avail(&self) -> u16 {
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.avail).idx)) }
    }

    /// Get current used index
    pub fn get_used(&self) -> u16 {
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.used).idx)) }
    }

    /// Get element from used ring
    ///
    /// # Parameters
    /// - `idx`: Index in used ring
    ///
    /// # Returns
    /// UsedElem containing descriptor ID and length
    pub fn get_used_elem(&self, idx: u16) -> Option<UsedElem> {
        if self.used.is_null() {
            return None;
        }

        unsafe {
            // Used ring structure: flags (2) + idx (2) + ring (queue_size * 8)
            let ring_base = (self.used as usize) + 4;
            let elem_ptr = (ring_base + (idx % self.queue_size) as usize * 8) as *const UsedElem;
            Some(core::ptr::read_volatile(elem_ptr))
        }
    }

    /// Get last processed used index (for tracking)
    pub fn get_last_used(&self) -> u16 {
        // This should be maintained by driver, simplified implementation here
        self.get_used()
    }

    /// Notify device of new request
    pub fn notify(&self) {
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        unsafe {
            let queue_notify = self.queue_notify as *mut u16;
            core::ptr::write_volatile(queue_notify, self.queue_index);
        }
    }

    /// Wait for device to complete request
    pub fn wait_for_completion(&self, prev_used: u16) -> u16 {
        // Timeout value from config (in loop iterations, approximately microseconds)
        let mut timeout = crate::config::VIRTIO_QUEUE_TIMEOUT_US;

        if self.used.is_null() {
            return prev_used;
        }

        loop {
            // Use memory barrier to ensure read ordering
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

            let used_idx = unsafe {
                let used_idx_ptr = (self.used as usize + 2) as *const u16;
                core::ptr::read_volatile(used_idx_ptr)
            };

            if used_idx != prev_used {
                return used_idx;
            }

            core::hint::spin_loop();

            timeout -= 1;
            if timeout == 0 {
                crate::println!("virtio: I/O timeout (prev={}, idx={})", prev_used, used_idx);
                return used_idx;
            }
        }
    }

    /// Add descriptor chain to queue and notify device
    pub fn submit(&mut self, head_idx: u16) {
        unsafe {
            let avail = &mut *self.avail;
            let idx = core::ptr::read_volatile(core::ptr::addr_of!(avail.idx)) as usize;

            core::sync::atomic::fence(Ordering::Release);

            let ring_ptr = (self.avail as usize + 4) as *mut u16;
            core::ptr::write_volatile(ring_ptr.add(idx % self.queue_size as usize), head_idx);

            let new_idx = (idx as u16) + 1;
            core::sync::atomic::fence(Ordering::Release);
            core::ptr::write_volatile(&mut (*avail).idx as *mut u16, new_idx);
            core::sync::atomic::fence(Ordering::SeqCst);

            Self::notify(self);

            // Delay: give QEMU VirtIO device time to process notification
            // Note: This delay is necessary because QEMU needs time to respond to MMIO writes
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
        }
    }

    /// Get descriptor
    pub fn get_desc(&mut self, idx: u16) -> Option<Desc> {
        if idx < self.queue_size {
            unsafe { Some(*self.desc.add(idx as usize)) }
        } else {
            None
        }
    }

    /// Allocate new descriptor
    pub fn alloc_desc(&mut self) -> Option<u16> {
        let idx = self.next_desc.fetch_add(1, Ordering::AcqRel);
        if idx < self.queue_size {
            Some(idx)
        } else {
            None
        }
    }

    /// Reset descriptor allocator
    ///
    /// Call before starting new I/O operation to reuse descriptors
    /// Note: This assumes no concurrent I/O operations
    pub fn reset_desc_allocator(&mut self) {
        self.next_desc.store(0, Ordering::Release);
    }

    /// Set descriptor content
    pub fn set_desc(&mut self, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
        if idx < self.queue_size {
            unsafe {
                *self.desc.add(idx as usize) = Desc { addr, len, flags, next };
            }
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        }
    }

    /// Get descriptor table address
    pub fn get_desc_addr(&self) -> u64 {
        self.desc as u64
    }

    /// Get Available Ring address
    pub fn get_avail_addr(&self) -> u64 {
        self.avail as u64
    }

    /// Get Used Ring address
    pub fn get_used_addr(&self) -> u64 {
        self.used as u64
    }

    /// Get vring base address
    pub fn get_vring_addr(&self) -> u64 {
        self.vring_addr
    }

    /// Get queue notification address
    pub fn get_notify_addr(&self) -> u64 {
        self.queue_notify
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtIOBlkReqHeader {
    /// Request type (0=read, 1=write, 2=flush)
    pub type_: u32,
    /// Reserved
    pub reserved: u32,
    /// Sector number
    pub sector: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtIOBlkResp {
    /// Status (0=OK, 1=IOERR, 2=UNSUPPORTED)
    pub status: u8,
}

impl core::fmt::Display for VirtIOBlkResp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.status {
            0 => write!(f, "OK"),
            1 => write!(f, "IOERR"),
            2 => write!(f, "UNSUPPORTED"),
            _ => write!(f, "UNKNOWN({})", self.status),
        }
    }
}

pub mod req_type {
    pub const VIRTIO_BLK_T_IN: u32 = 0;
    pub const VIRTIO_BLK_T_OUT: u32 = 1;
    pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
}

pub mod status {
    pub const VIRTIO_BLK_S_OK: u8 = 0;
    pub const VIRTIO_BLK_S_IOERR: u8 = 1;
    pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;
}
