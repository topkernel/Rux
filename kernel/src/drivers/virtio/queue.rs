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
        // SAFETY: layout is non-zero, page-aligned; null check follows.
        let mem_ptr = unsafe { alloc::alloc::alloc(layout) as *mut u8 };
        if mem_ptr.is_null() {
            return None;
        }

        let addr = mem_ptr as usize;
        if addr & (PAGE_SIZE - 1) != 0 {
            crate::println!("virtio: ERROR: vring not page-aligned!");
            // SAFETY: mem_ptr was allocated above with the same layout but failed alignment; reclaim it.
            unsafe { alloc::alloc::dealloc(mem_ptr, layout) };
            return None;
        }

        let desc = mem_ptr as *mut Desc;
        // SAFETY: pointers are within the allocated region; offsets are computed from
        // page-aligned sizes and do not exceed total_size.
        let avail = unsafe { (mem_ptr as usize + desc_size_aligned) as *mut AvailRing };
        let used = unsafe { (mem_ptr as usize + desc_size_aligned + avail_size_aligned) as *mut UsedRing };

        // SAFETY: avail and used are within the allocated region; initializing ring fields.
        unsafe {
            (*avail).flags = 0;
            (*avail).idx = 0;
            (*used).flags = 0;
            (*used).idx = 0;
        }

        for i in 0..queue_size {
            // SAFETY: i < queue_size, and desc points to queue_size descriptors in the allocation.
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
    /// Get current available index
    pub fn get_avail(&self) -> u16 {
        // SAFETY: avail is set in new() and points to the VirtIO available ring in our allocation.
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*self.avail).idx)) }
    }

    /// Get current used index
    pub fn get_used(&self) -> u16 {
        // SAFETY: used is set in new() and points to the VirtIO used ring in our allocation.
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

        // SAFETY: idx is wrapped to queue_size; ring_base is within the used ring allocation.
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
        // Disable interrupts during MMIO operations
        #[cfg(feature = "riscv64")]
        let sie_backup: u64;
        #[cfg(feature = "riscv64")]
        // SAFETY: reading and clearing SEIE in the sie CSR is valid on S-mode RISC-V.
        unsafe {
            // Read current sie and disable external interrupts
            core::arch::asm!(
                "csrr {sie}, sie",
                "csrci sie, 9",  // Clear SEIE (bit 9) - disable external interrupts
                sie = out(reg) sie_backup,
            );
        }

        // RISC-V MMIO fence: fence w, o
        // This ensures all previous writes (to descriptor table, available ring)
        // are visible before the MMIO write to the notify register.
        // MMIO write fence: RISCV_FENCE(w, o)
        #[cfg(feature = "riscv64")]
        unsafe {
            core::arch::asm!("fence w, o");
        }

        // SAFETY: queue_notify is a valid MMIO address for the VirtIO notify register;
        // volatile write ensures the store reaches the device.
        unsafe {
            // Use 16-bit write as per VirtIO spec
            let queue_notify = self.queue_notify as *mut u16;
            // Write queue index to notify register
            core::ptr::write_volatile(queue_notify, self.queue_index);
        }

        // RISC-V MMIO fence after write: fence i, ir
        // This ensures the MMIO write completes before any subsequent reads.
        // MMIO read fence: RISCV_FENCE(i, ir)
        #[cfg(feature = "riscv64")]
        unsafe {
            core::arch::asm!("fence i, ir");
        }

        // Restore interrupts
        #[cfg(feature = "riscv64")]
        // SAFETY: writing sie CSR to restore the previous interrupt-enable state is valid on S-mode.
        unsafe {
            core::arch::asm!(
                "csrw sie, {sie}",
                sie = in(reg) sie_backup,
            );
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

            // SAFETY: used points to the VirtIO used ring in our allocation;
            // offset 2 is the idx field (u16), within the used ring structure.
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
                return used_idx;
            }
        }
    }

    /// Get raw pointer to used ring (for interrupt-driven wait outside queue lock)
    pub fn used_ring_ptr(&self) -> *const UsedRing {
        self.used
    }

    /// Wait for device to complete request using interrupt-driven sleep.
    ///
    /// Instead of busy-wait polling, the current task sleeps on a wait queue
    /// and is woken by the VirtIO interrupt handler when the device completes
    /// the request. The BKL is released during sleep and re-acquired on wakeup.
    ///
    /// # Safety
    /// - `used_ring` must point to a valid VirtIO used ring
    /// - `wait_queue` must be the correct wait queue for this device's interrupt
    pub fn wait_for_used_interruptible(
        used_ring: *const UsedRing,
        wait_queue: &crate::process::wait::WaitQueueHead,
        prev_used: u16,
    ) -> u16 {
        if used_ring.is_null() {
            return prev_used;
        }

        // Detect early boot phase: no current task or PID 0 (idle/boot thread).
        // During early boot (e.g., ext4 mount before scheduler/IRQ init),
        // interrupts are not enabled and there is no scheduler, so we must
        // use synchronous polling with a large timeout (matching the MMIO
        // path's budget). The normal path relies on interrupt-driven wakeup
        // with a small safety-net timeout.
        let is_early_boot = match crate::sched::current() {
            Some(task) if task.pid() != 0 => false,
            _ => true,
        };

        // Early boot: large timeout for reliable synchronous polling.
        // Normal: small timeout — relies on interrupt wakeup via wait queue.
        let max_iterations = if is_early_boot {
            1_000_000
        } else {
            5000
        };

        for _iteration in 0..max_iterations {
            // Early boot: pure spin-poll (no scheduler, no interrupts).
            if is_early_boot {
                // SAFETY: used_ring offset 2 is the idx field.
                let used_idx = unsafe {
                    let used_idx_ptr = (used_ring as usize + 2) as *const u16;
                    core::ptr::read_volatile(used_idx_ptr)
                };
                if used_idx != prev_used {
                    return used_idx;
                }
                core::hint::spin_loop();
                continue;
            }

            // Normal path: get current task and sleep on wait queue.
            // The idle task has SchedPolicy::Idle which enqueue_task() ignores,
            // so sleeping would be a permanent deadlock.
            let current = match crate::sched::current() {
                Some(task) if task.pid() != 0 => task,
                _ => {
                    core::hint::spin_loop();
                    continue;
                }
            };

            // Fast check: I/O may have completed already.
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
            let used_idx = unsafe {
                let used_idx_ptr = (used_ring as usize + 2) as *const u16;
                core::ptr::read_volatile(used_idx_ptr)
            };
            if used_idx != prev_used {
                return used_idx;
            }

            // Add to wait queue, then spin-wait briefly before sleeping.
            // This closes the lost-wakeup race window without changing
            // task state or disabling interrupts:
            //
            //   race: add → [interrupt fires, used_idx updated, wake returns
            //          false because task is RUNNING] → schedule → sleep forever
            //
            //   fix: add → spin-wait catches the updated used_idx → return
            //
            // The spin-wait is short (256 iterations ≈ few µs) so it
            // only activates when the race actually occurs; the normal
            // path (interrupt hasn't fired yet) quickly falls through
            // to schedule().
            let entry = crate::process::wait::WaitQueueEntry::new(current, false);
            wait_queue.add(entry);

            for _ in 0..256 {
                core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
                let used_idx = unsafe {
                    let used_idx_ptr = (used_ring as usize + 2) as *const u16;
                    core::ptr::read_volatile(used_idx_ptr)
                };
                if used_idx != prev_used {
                    wait_queue.remove(current);
                    return used_idx;
                }
                core::hint::spin_loop();
            }

            // Sleep until woken by interrupt
            crate::sched::schedule();

            // Remove from wait queue and loop back to re-check condition
            wait_queue.remove(current);
        }

        // Timeout: return current used_idx (caller treats as error)
        // SAFETY: used_ring is still valid; offset 2 is the idx field within the ring.
        unsafe {
            let used_idx_ptr = (used_ring as usize + 2) as *const u16;
            core::ptr::read_volatile(used_idx_ptr)
        }
    }

    /// Add descriptor chain to queue and notify device
    pub fn submit(&mut self, head_idx: u16) {
        // SAFETY: avail points to the available ring in our allocation; the ring
        // pointer at offset 4 is within the allocated region; all volatile
        // reads/writes target fields of the VirtIO vring we own.
        unsafe {
            let avail = &mut *self.avail;
            let idx = core::ptr::read_volatile(core::ptr::addr_of!(avail.idx)) as usize;
            let ring_idx = idx % self.queue_size as usize;

            // Memory barrier before writing to available ring
            core::sync::atomic::fence(Ordering::Release);

            // Write descriptor head index to available ring
            let ring_ptr = (self.avail as usize + 4) as *mut u16;
            core::ptr::write_volatile(ring_ptr.add(ring_idx), head_idx);

            // Memory barrier to ensure ring write completes before index update
            core::sync::atomic::fence(Ordering::Release);

            // Update available index (this signals to device that new request is ready)
            let new_idx = (idx as u16).wrapping_add(1);
            core::ptr::write_volatile(&mut (*avail).idx as *mut u16, new_idx);

            // Full memory barrier before notify
            core::sync::atomic::fence(Ordering::SeqCst);

            // Notify device
            Self::notify(self);
        }
    }

    /// Get descriptor
    pub fn get_desc(&mut self, idx: u16) -> Option<Desc> {
        if idx < self.queue_size {
            // SAFETY: idx < queue_size, desc points to queue_size descriptors in the allocation.
            unsafe { Some(*self.desc.add(idx as usize)) }
        } else {
            None
        }
    }

    /// Allocate new descriptor (reclaims from used ring when possible)
    pub fn alloc_desc(&mut self) -> Option<u16> {
        let used_idx = self.get_used();
        let avail_idx = self.get_avail();

        // Try to reclaim descriptors that the device has finished with.
        // We can reclaim all descriptors from (last_avail_base .. used_idx).
        if used_idx != avail_idx {
            // The available ring wraps around, so last_avail_base may not be
            // simply (avail_idx - 3).  Instead, find the base of the last
            // submitted chain by scanning back from avail_idx.
            // The simplest safe approach: reclaim everything up to used_idx
            // but only if used_idx has advanced past our current next_desc.
            //
            // Reclaim range: we know the device is done with descriptors
            // whose id < used_idx (the device has written them to used ring).
            // So advance next_desc to max(next_desc, used_idx).
            let used_idx_safe = used_idx;
            if self.next_desc.load(Ordering::Acquire) < used_idx_safe {
                self.next_desc.store(used_idx_safe, Ordering::Release);
            }
        }

        let idx = self.next_desc.fetch_add(1, Ordering::AcqRel) % self.queue_size;
        Some(idx)
    }

    /// Reclaim descriptors that the device has finished processing.
    ///
    /// Called after I/O completion to make descriptors available for reuse.
    pub fn reclaim_descs(&mut self) {
        let used_idx = self.get_used();
        if self.next_desc.load(Ordering::Acquire) < used_idx {
            self.next_desc.store(used_idx, Ordering::Release);
        }
    }

    /// Reset descriptor allocator
    ///
    /// Note: This is UNSAFE under concurrent I/O and should only be used
    /// during single-threaded initialization.
    pub fn reset_desc_allocator(&mut self) {
        self.next_desc.store(0, Ordering::Release);
    }

    /// Set descriptor content
    pub fn set_desc(&mut self, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
        if idx < self.queue_size {
            // SAFETY: idx < queue_size, desc points to queue_size descriptors in the allocation.
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
