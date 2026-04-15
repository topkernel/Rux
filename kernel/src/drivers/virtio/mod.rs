//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO block device driver

use crate::sync::spinlock::Spinlock;

use crate::drivers::blkdev::{GenDisk, Request, BlockDeviceOps};

pub mod queue;
pub mod probe;
pub mod offset;
pub mod virtio_pci;

/// VirtIO device register layout (compliant with VirtIO 1.0 specification)
#[repr(C)]
pub struct VirtIOBlkRegs {
    /// Magic number (0x00)
    pub magic_value: u32,
    /// Version (0x04)
    pub version: u32,
    /// Device ID (0x08)
    pub device_id: u32,
    /// Vendor ID (0x0C)
    pub vendor: u32,
    /// Device features (0x10)
    pub device_features: u32,
    /// _reserved (0x14)
    _reserved1: u32,
    /// Driver-selected features (0x20)
    pub driver_features: u32,
    /// _reserved (0x24)
    _reserved2: u32,
    /// Queue select (0x30)
    pub queue_sel: u32,
    /// Queue max count (0x34)
    pub queue_num_max: u32,
    /// Queue count (0x38)
    pub queue_num: u32,
    /// _reserved (0x3C)
    _reserved3: u32,
    /// _reserved (0x40)
    _reserved4: u32,
    /// Queue ready (0x44) - Modern VirtIO Queue Enable
    pub queue_ready: u32,
    /// _reserved (0x48)
    _reserved5: u32,
    /// _reserved (0x4C)
    _reserved6: u32,
    /// Queue notify (0x50)
    pub queue_notify: u32,
    /// _reserved (0x54-0x5C)
    _reserved7: [u32; 3],
    /// Interrupt status (0x60)
    pub interrupt_status: u32,
    /// Interrupt acknowledge (0x64)
    pub interrupt_ack: u32,
    /// _reserved (0x68-0x6C)
    _reserved8: [u32; 2],
    /// Driver status (0x70)
    pub status: u32,
    /// _reserved (0x74+)
    _reserved9: [u32; 4],
}

/// VirtIO block device
pub struct VirtIOBlkDevice {
    /// MMIO base address
    base_addr: u64,
    /// Block device
    pub disk: GenDisk,
    /// Capacity (sectors)
    capacity: u64,
    /// Block size
    block_size: u32,
    /// Initialization status
    initialized: Spinlock<bool>,
    /// VirtQueue (for I/O operations)
    virtqueue: Spinlock<Option<queue::VirtQueue>>,
    /// Queue size
    queue_size: u16,
    /// IRQ number
    irq: u32,
}

// SAFETY: VirtIOBlkDevice is only accessed from kernel context; internal Spinlocks
// serialize all mutable access to shared fields.
unsafe impl Send for VirtIOBlkDevice {}
// SAFETY: All shared state is protected by Spinlocks (irqsafe where needed),
// ensuring no data races across threads/CPUs.
unsafe impl Sync for VirtIOBlkDevice {}

impl VirtIOBlkDevice {
    /// Create new VirtIO block device
    pub fn new(base_addr: u64) -> Self {
        Self {
            base_addr,
            disk: GenDisk::new("virtblk", 0, 1, 512, None as Option<&BlockDeviceOps>),
            capacity: 0,
            block_size: 512,
            initialized: Spinlock::new(false),
            virtqueue: Spinlock::new(None),
            queue_size: 0,
            irq: 1,  // Default IRQ 1 (first VirtIO device)
        }
    }

    /// Initialize device
    pub fn init(&mut self) -> Result<(), &'static str> {
        // VirtIO MMIO register offsets
        const MAGIC_VALUE_OFFSET: u64 = 0x000;
        const VERSION_OFFSET: u64 = 0x004;
        const DEVICE_ID_OFFSET: u64 = 0x008;
        const STATUS_OFFSET: u64 = 0x070;
        const GUEST_PAGE_SIZE_OFFSET: u64 = 0x028;
        const DEVICE_FEATURES_OFFSET: u64 = 0x010;
        const DRIVER_FEATURES_OFFSET: u64 = 0x020;
        const QUEUE_SEL_OFFSET: u64 = 0x030;
        const QUEUE_NUM_MAX_OFFSET: u64 = 0x034;
        const QUEUE_NUM_OFFSET: u64 = 0x038;

        // Helper macro: print register read/write
        macro_rules! read_reg {
            ($offset:expr, $name:expr) => {
                {
                    let ptr = (self.base_addr + $offset) as *const u32;
                    core::ptr::read_volatile(ptr)
                }
            };
        }

        macro_rules! write_reg {
            ($offset:expr, $name:expr, $val:expr) => {
                {
                    let ptr = (self.base_addr + $offset) as *mut u32;
                    core::ptr::write_volatile(ptr, $val);
                }
            };
        }

        unsafe {
            // 1. Verify magic number
            let magic = read_reg!(MAGIC_VALUE_OFFSET, "MAGIC_VALUE");
            if magic != 0x74726976 {
                return Err("Invalid VirtIO magic value");
            }

            // 2. Verify version (only support Modern VirtIO 1.0+)
            let version = read_reg!(VERSION_OFFSET, "VERSION");
            if version != 2 {
                return Err("Unsupported VirtIO version: only Modern VirtIO 1.0+ (version 2) is supported, Legacy VirtIO is not supported");
            }

            // 3. Verify device ID
            let device_id = read_reg!(DEVICE_ID_OFFSET, "DEVICE_ID");
            if device_id != 2 {
                return Err("Not a VirtIO block device");
            }

            // SAFETY: MMIO base_addr points to valid device registers; all register
            // offsets follow the VirtIO MMIO device spec. The `read_reg!` and
            // `write_reg!` macros use volatile reads/writes at the correct offsets.
            // 4. State machine: Reset device
            write_reg!(STATUS_OFFSET, "STATUS", 0x00);

            // 5. State machine: ACKNOWLEDGE (0x01)
            write_reg!(STATUS_OFFSET, "STATUS", 0x01);
            let status = read_reg!(STATUS_OFFSET, "STATUS");

            // 6. State machine: DRIVER (0x02)
            write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02);
            let status = read_reg!(STATUS_OFFSET, "STATUS");

            // Check if reset is needed
            if status & 0x40 != 0 {
                write_reg!(STATUS_OFFSET, "STATUS", 0x00);
                write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02);
            }

            // 7. Read device features
            let _device_features = read_reg!(DEVICE_FEATURES_OFFSET, "DEVICE_FEATURES");

            // 9. Feature negotiation (Modern VirtIO)
            // Write DRIVER_FEATURES register
            // Set FEATURES_OK bit (indicating feature negotiation complete)
            write_reg!(DRIVER_FEATURES_OFFSET, "DRIVER_FEATURES", 0);

            // 9.5. Set FEATURES_OK bit
            write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02 | 0x08);

            // ========== VirtQueue setup ==========

            // 10. Select queue 0
            write_reg!(QUEUE_SEL_OFFSET, "QUEUE_SEL", 0);

            // 11. Read max queue size
            let max_queue_size = read_reg!(QUEUE_NUM_MAX_OFFSET, "QUEUE_NUM_MAX");

            if max_queue_size == 0 {
                return Err("VirtIO device has zero queue size");
            }

            self.queue_size = if max_queue_size < 8 { 4 } else { 8 };

            // 12. Set queue count
            write_reg!(QUEUE_NUM_OFFSET, "QUEUE_NUM", self.queue_size as u32);

            // 13. Create VirtQueue (allocate vring memory)
            let virtqueue = match queue::VirtQueue::new(
                self.queue_size,
                0,  // queue_index: block device only uses queue 0
                self.base_addr + 0x50,  // queue_notify
                self.base_addr + 0x60,  // interrupt_status
                self.base_addr + 0x64,  // interrupt_ack
            ) {
                Some(vq) => vq,
                None => return Err("Failed to allocate VirtQueue"),
            };

            let desc_addr = virtqueue.get_desc_addr();
            let avail_addr = virtqueue.get_avail_addr();
            let used_addr = virtqueue.get_used_addr();
            // 14. Modern VirtIO: Set queue addresses (64-bit, split into high/low)
            // Modern VirtIO uses three separate address register pairs to set queue
            use crate::drivers::virtio::offset;
            const QUEUE_DESC_LO_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DESC_LO as u64;
            const QUEUE_DESC_HI_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DESC_HI as u64;
            const QUEUE_DRIVER_LO_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DRIVER_LO as u64;
            const QUEUE_DRIVER_HI_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DRIVER_HI as u64;
            const QUEUE_DEVICE_LO_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DEVICE_LO as u64;
            const QUEUE_DEVICE_HI_OFFSET: u64 = offset::COMMON_CFG_QUEUE_DEVICE_HI as u64;
            const QUEUE_READY_OFFSET: u64 = offset::COMMON_CFG_QUEUE_ENABLE as u64;

            // Convert virtual addresses to physical addresses
            let desc_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(desc_addr)
            ).0;
            let avail_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(avail_addr)
            ).0;
            let used_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(used_addr)
            ).0;

            // Write descriptor table address (low 32 bits)
            write_reg!(QUEUE_DESC_LO_OFFSET, "QUEUE_DESC_LO", (desc_phys_addr & 0xFFFFFFFF) as u32);
            // Write descriptor table address (high 32 bits)
            write_reg!(QUEUE_DESC_HI_OFFSET, "QUEUE_DESC_HI", (desc_phys_addr >> 32) as u32);

            // Write available ring address (low 32 bits)
            write_reg!(QUEUE_DRIVER_LO_OFFSET, "QUEUE_DRIVER_LO", (avail_phys_addr & 0xFFFFFFFF) as u32);
            // Write available ring address (high 32 bits)
            write_reg!(QUEUE_DRIVER_HI_OFFSET, "QUEUE_DRIVER_HI", (avail_phys_addr >> 32) as u32);

            // Write used ring address (low 32 bits)
            write_reg!(QUEUE_DEVICE_LO_OFFSET, "QUEUE_DEVICE_LO", (used_phys_addr & 0xFFFFFFFF) as u32);
            // Write used ring address (high 32 bits)
            write_reg!(QUEUE_DEVICE_HI_OFFSET, "QUEUE_DEVICE_HI", (used_phys_addr >> 32) as u32);

            // Set queue ready bit
            write_reg!(QUEUE_READY_OFFSET, "QUEUE_READY", 1);

            // 15. Read device capacity
            const VIRTIO_BLK_CONFIG_CAPACITY: u64 = 0x100;
            let cap_ptr = (self.base_addr + VIRTIO_BLK_CONFIG_CAPACITY) as *const u64;
            self.capacity = *cap_ptr;

            // 16. Update block device info
            self.disk.set_capacity(self.capacity as u64);
            self.disk.set_request_fn(Self::handle_request);
            self.disk.set_async_read_fn(Self::async_read_fn);
            *self.virtqueue.lock() = Some(virtqueue);

            // 17. State machine: DRIVER_OK (0x04)
            write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02 | 0x08 | 0x04);

            // Memory barrier
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);

            // Mark as initialized
            *self.initialized.lock() = true;

            Ok(())
        }
    }

    /// Get capacity
    pub fn get_capacity(&self) -> u64 {
        self.capacity
    }

    /// Handle I/O request
    ///
    /// SAFETY: `req.device` points to a valid GenDisk whose `private_data` contains
    /// a valid pointer to a VirtIOBlkDevice. Called only from the block layer
    /// for registered devices.
    unsafe extern "C" fn handle_request(req: &mut Request) {
        // Get VirtIOBlkDevice pointer from private_data
        let gd = &*req.device;
        let device_ptr = match gd.private_data {
            Some(ptr) => ptr as *const VirtIOBlkDevice,
            None => {
                if let Some(end_io) = req.end_io {
                    end_io(req, -5);  // EIO
                }
                return;
            }
        };

        let device = &*device_ptr;

        // Execute operation based on command type
        let result = match req.cmd_type {
            crate::drivers::blkdev::ReqCmd::Read => {
                // Read block
                device.read_block(req.sector, &mut req.buffer)
            }
            crate::drivers::blkdev::ReqCmd::Write => {
                // Write block
                device.write_block(req.sector, &req.buffer)
            }
            crate::drivers::blkdev::ReqCmd::Flush => {
                // Flush operation (return success for now)
                Ok(())
            }
        };

        // Call completion callback
        match result {
            Ok(()) => {
                if let Some(end_io) = req.end_io {
                    end_io(req, 0);  // Success
                }
            }
            Err(err) => {
                crate::pr_err!("virtio-blk: I/O error: {}", err);
                if let Some(end_io) = req.end_io {
                    end_io(req, err);
                }
            }
        }
    }

    /// Read block
    pub fn read_block(&self, sector: u64, buf: &mut [u8]) -> Result<(), i32> {
        if !*self.initialized.lock_irqsave() {
            return Err(-5);  // EIO
        }

        // Phase 1: Set up and submit request (under queue lock)
        let (used_ring_ptr, prev_used, header_ptr, header_layout, resp_ptr, resp_layout) = {
            // Get VirtQueue (irqsafe: IRQ handler also takes this lock)
            let mut queue_guard = self.virtqueue.lock_irqsave();
            let queue = match queue_guard.as_mut() {
                Some(q) => q,
                None => return Err(-5),
            };

            use queue::{VirtIOBlkReqHeader, VirtIOBlkResp};

            // Construct VirtIO block request header
            let req_header = VirtIOBlkReqHeader {
                type_: queue::req_type::VIRTIO_BLK_T_IN,
                reserved: 0,
                sector,
            };

            // Allocate request header buffer (needs to persist until request completes)
            let header_layout = alloc::alloc::Layout::new::<VirtIOBlkReqHeader>();
            let header_ptr: *mut VirtIOBlkReqHeader;
            // SAFETY: Layout is non-zero-sized; null check follows immediately.
            unsafe {
                header_ptr = alloc::alloc::alloc(header_layout) as *mut VirtIOBlkReqHeader;
            }
            if header_ptr.is_null() {
                return Err(-12);  // ENOMEM
            }
            // SAFETY: header_ptr is non-null and properly aligned.
            unsafe {
                *header_ptr = req_header;
            }

            // Allocate response buffer
            let resp_layout = alloc::alloc::Layout::new::<VirtIOBlkResp>();
            let resp_ptr: *mut VirtIOBlkResp;
            // SAFETY: Layout is non-zero-sized; null check follows immediately.
            unsafe {
                resp_ptr = alloc::alloc::alloc(resp_layout) as *mut VirtIOBlkResp;
            }
            if resp_ptr.is_null() {
                // SAFETY: header_ptr was allocated with header_layout and is still valid.
                unsafe {
                    alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
                }
                return Err(-12);  // ENOMEM
            }
            // SAFETY: resp_ptr is non-null and properly aligned.
            unsafe {
                (*resp_ptr).status = 0xFF;  // Initialize to invalid state
            }

            // VirtIO descriptor flags
            const VIRTQ_DESC_F_NEXT: u16 = 1;
            const VIRTQ_DESC_F_WRITE: u16 = 2;

            // Convert virtual addresses to physical addresses (VirtIO devices need physical addresses for DMA)
            let header_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(header_ptr as u64)
            ).0;
            let data_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(buf.as_ptr() as u64)
            ).0;
            let resp_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(resp_ptr as u64)
            ).0;

            // Allocate three descriptors
            let header_desc_idx = match queue.alloc_desc() {
                Some(idx) => idx,
                None => return Err(-5),
            };
            let data_desc_idx = match queue.alloc_desc() {
                Some(idx) => idx,
                None => return Err(-5),
            };
            let resp_desc_idx = match queue.alloc_desc() {
                Some(idx) => idx,
                None => return Err(-5),
            };

            // Set request header descriptor (read-only, device reads) - use physical address
            queue.set_desc(
                header_desc_idx,
                header_phys_addr,
                core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
                VIRTQ_DESC_F_NEXT,
                data_desc_idx,
            );

            // Set data buffer descriptor (write-only, device writes) - use physical address
            queue.set_desc(
                data_desc_idx,
                data_phys_addr,
                buf.len() as u32,
                VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,  // WRITE + NEXT
                resp_desc_idx,
            );

            // Set response descriptor (write-only, device writes) - use physical address
            queue.set_desc(
                resp_desc_idx,
                resp_phys_addr,
                core::mem::size_of::<VirtIOBlkResp>() as u32,
                0,  // Last descriptor
                0,
            );

            // Snapshot expected used index BEFORE submit (under queue lock)
            let prev = get_mmio_expected_used_idx();

            // Submit to available ring
            queue.submit(header_desc_idx);

            // Notify device
            queue.notify();

            // Advance expected used index AFTER submit (under queue lock)
            increment_mmio_expected_used_idx();

            let used_ptr = queue.used_ring_ptr();

            (used_ptr, prev, header_ptr, header_layout, resp_ptr, resp_layout)
        };
        // queue_guard dropped here — VirtQueue spinlock released

        // Phase 2: Wait for completion (interrupt-driven, releases BKL during sleep)
        let used = queue::VirtQueue::wait_for_used_interruptible(
            used_ring_ptr,
            &VIRTIO_BLK_WAIT_QUEUE,
            prev_used,
        );

        // Phase 3: Check response
        if used == prev_used {
            // Timeout — device did not update used ring
            // SAFETY: Both pointers were allocated above and are still valid.
            unsafe {
                alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
                alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
            }
            return Err(-5);  // EIO
        }

        // SAFETY: resp_ptr was allocated above; device has completed the response.
        unsafe {
            let status = (*resp_ptr).status;
            alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);

            if status == queue::status::VIRTIO_BLK_S_OK {
                Ok(())
            } else {
                Err(-5)  // EIO
            }
        }
    }

    /// Write block
    pub fn write_block(&self, sector: u64, buf: &[u8]) -> Result<(), i32> {
        if !*self.initialized.lock_irqsave() {
            return Err(-5);  // EIO
        }

        // Phase 1: Set up and submit request (under queue lock)
        let (used_ring_ptr, prev_used, header_ptr, header_layout, resp_ptr, resp_layout) = {
            // Get VirtQueue (irqsafe: IRQ handler also takes this lock)
            let mut queue_guard = self.virtqueue.lock_irqsave();
            let queue = queue_guard.as_mut().ok_or(-5)?;

            use queue::{VirtIOBlkReqHeader, VirtIOBlkResp};

            // Construct VirtIO block request header
            let req_header = VirtIOBlkReqHeader {
                type_: queue::req_type::VIRTIO_BLK_T_OUT,
                reserved: 0,
                sector,
            };

            // Allocate request header buffer (needs to persist until request completes)
            let header_layout = alloc::alloc::Layout::new::<VirtIOBlkReqHeader>();
            let header_ptr: *mut VirtIOBlkReqHeader;
            // SAFETY: Layout is non-zero-sized; null check follows immediately.
            unsafe {
                header_ptr = alloc::alloc::alloc(header_layout) as *mut VirtIOBlkReqHeader;
            }
            if header_ptr.is_null() {
                return Err(-12);  // ENOMEM
            }
            // SAFETY: header_ptr is non-null and properly aligned.
            unsafe {
                *header_ptr = req_header;
            }

            // Allocate response buffer
            let resp_layout = alloc::alloc::Layout::new::<VirtIOBlkResp>();
            let resp_ptr: *mut VirtIOBlkResp;
            // SAFETY: Layout is non-zero-sized; null check follows immediately.
            unsafe {
                resp_ptr = alloc::alloc::alloc(resp_layout) as *mut VirtIOBlkResp;
            }
            if resp_ptr.is_null() {
                // SAFETY: header_ptr was allocated with header_layout and is still valid.
                unsafe {
                    alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
                }
                return Err(-12);  // ENOMEM
            }
            // SAFETY: resp_ptr is non-null and properly aligned.
            unsafe {
                (*resp_ptr).status = 0xFF;  // Initialize to invalid state
            }

            // VirtIO descriptor flags
            const VIRTQ_DESC_F_NEXT: u16 = 1;
            const VIRTQ_DESC_F_WRITE: u16 = 2;

            // Convert virtual addresses to physical addresses (VirtIO devices need physical addresses for DMA)
            let header_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(header_ptr as u64)
            ).0;
            let data_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(buf.as_ptr() as u64)
            ).0;
            let resp_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(resp_ptr as u64)
            ).0;

            // Allocate three descriptors
            let header_desc_idx = queue.alloc_desc().ok_or(-5)?;
            let data_desc_idx = queue.alloc_desc().ok_or(-5)?;
            let resp_desc_idx = queue.alloc_desc().ok_or(-5)?;

            // Set request header descriptor (read-only, device reads) - use physical address
            queue.set_desc(
                header_desc_idx,
                header_phys_addr,
                core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
                VIRTQ_DESC_F_NEXT,
                data_desc_idx,
            );

            // Set data buffer descriptor (read-only, device reads) - use physical address
            queue.set_desc(
                data_desc_idx,
                data_phys_addr,
                buf.len() as u32,
                VIRTQ_DESC_F_NEXT,
                resp_desc_idx,
            );

            // Set response descriptor (write-only, device writes) - use physical address
            queue.set_desc(
                resp_desc_idx,
                resp_phys_addr,
                core::mem::size_of::<VirtIOBlkResp>() as u32,
                VIRTQ_DESC_F_WRITE,
                0,
            );

            // Snapshot expected used index BEFORE submit (under queue lock)
            let prev = get_mmio_expected_used_idx();

            // Submit to available ring
            queue.submit(header_desc_idx);

            // Notify device
            queue.notify();

            // Advance expected used index AFTER submit (under queue lock)
            increment_mmio_expected_used_idx();

            let used_ptr = queue.used_ring_ptr();

            (used_ptr, prev, header_ptr, header_layout, resp_ptr, resp_layout)
        };
        // queue_guard dropped here — VirtQueue spinlock released

        // Phase 2: Wait for completion (interrupt-driven, releases BKL during sleep)
        let used = queue::VirtQueue::wait_for_used_interruptible(
            used_ring_ptr,
            &VIRTIO_BLK_WAIT_QUEUE,
            prev_used,
        );

        // Phase 3: Check response
        if used == prev_used {
            // Timeout — device did not update used ring
            // SAFETY: Both pointers were allocated above and are still valid.
            unsafe {
                alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
                alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
            }
            return Err(-5);  // EIO
        }

        // SAFETY: resp_ptr was allocated above; device has completed the response.
        unsafe {
            let status = (*resp_ptr).status;
            alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);

            if status == queue::status::VIRTIO_BLK_S_OK {
                Ok(())
            } else {
                Err(-5)  // EIO
            }
        }
    }
}

/// VirtIO block device operations
static VIRTIO_BLK_OPS: BlockDeviceOps = BlockDeviceOps {
    open: None,
    release: None,
    getgeo: None,
};

// Async I/O methods (added in a separate impl block)
impl VirtIOBlkDevice {
    // ========================================================================
    // Async I/O submission (fire-and-forget, completion via interrupt)
    // ========================================================================

    /// Submit an async read request. Does NOT wait for completion.
    ///
    /// The caller must ensure `buf` remains valid until `completion.complete()` is called
    /// (from interrupt context). The completion is stored in the pending-I/O table
    /// and signaled by the interrupt handler.
    ///
    /// # Returns
    /// Ok(()) on successful submission, Err(i32) on failure.
    fn submit_read_async(
        &self,
        sector: u64,
        buf: &mut [u8],
        completion: &crate::fs::io_completion::IoCompletion,
    ) -> Result<(), i32> {
        if !*self.initialized.lock_irqsave() {
            return Err(-5);  // EIO
        }

        use queue::{VirtIOBlkReqHeader, VirtIOBlkResp};

        let mut queue_guard = self.virtqueue.lock_irqsave();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return Err(-5),
        };

        // Allocate header and response buffers
        let header_layout = alloc::alloc::Layout::new::<VirtIOBlkReqHeader>();
        let header_ptr: *mut u8;
        // SAFETY: Layout is non-zero-sized; null check follows immediately.
        unsafe {
            header_ptr = alloc::alloc::alloc(header_layout);
        }
        if header_ptr.is_null() {
            return Err(-12);
        }
        // SAFETY: header_ptr is non-null and properly aligned for VirtIOBlkReqHeader.
        unsafe {
            let header = header_ptr as *mut VirtIOBlkReqHeader;
            (*header) = VirtIOBlkReqHeader {
                type_: queue::req_type::VIRTIO_BLK_T_IN,
                reserved: 0,
                sector,
            };
        }

        let resp_layout = alloc::alloc::Layout::new::<VirtIOBlkResp>();
        let resp_ptr: *mut u8;
        // SAFETY: Layout is non-zero-sized; null check follows immediately.
        unsafe {
            resp_ptr = alloc::alloc::alloc(resp_layout);
        }
        if resp_ptr.is_null() {
            // SAFETY: header_ptr was allocated with header_layout and is still valid.
            unsafe { alloc::alloc::dealloc(header_ptr, header_layout); }
            return Err(-12);
        }
        // SAFETY: resp_ptr is non-null and properly aligned for VirtIOBlkResp.
        unsafe {
            *(resp_ptr as *mut VirtIOBlkResp) = VirtIOBlkResp { status: 0xFF };
        }

        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        let header_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(header_ptr as u64),
        ).0;
        let data_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(buf.as_ptr() as u64),
        ).0;
        let resp_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(resp_ptr as u64),
        ).0;

        let header_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => {
                // SAFETY: Both pointers were allocated above and are still valid.
                unsafe {
                    alloc::alloc::dealloc(header_ptr, header_layout);
                    alloc::alloc::dealloc(resp_ptr, resp_layout);
                }
                return Err(-5);
            }
        };
        let data_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => {
                // SAFETY: Both pointers were allocated above and are still valid.
                unsafe {
                    alloc::alloc::dealloc(header_ptr, header_layout);
                    alloc::alloc::dealloc(resp_ptr, resp_layout);
                }
                return Err(-5);
            }
        };
        let resp_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => {
                // SAFETY: Both pointers were allocated above and are still valid.
                unsafe {
                    alloc::alloc::dealloc(header_ptr, header_layout);
                    alloc::alloc::dealloc(resp_ptr, resp_layout);
                }
                return Err(-5);
            }
        };

        queue.set_desc(header_desc_idx, header_phys,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT, data_desc_idx);
        queue.set_desc(data_desc_idx, data_phys, buf.len() as u32,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT, resp_desc_idx);
        queue.set_desc(resp_desc_idx, resp_phys,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            0, 0);

        let prev = get_mmio_expected_used_idx();
        queue.submit(header_desc_idx);
        queue.notify();
        increment_mmio_expected_used_idx();

        // Store in pending table
        let pending = PendingIo {
            completion: completion as *const _ as *mut _,
            resp_ptr,
            resp_layout,
            header_ptr,
            header_layout,
        };
        let slot = prev as usize % MAX_PENDING_IO;
        VIRTIO_MMIO_PENDING.lock_irqsave()[slot] = Some(pending);

        Ok(())
    }

    /// Static wrapper for async read — matches `async_read_fn` signature on GenDisk.
    ///
    /// Casts `*const GenDisk` back to `&VirtIOBlkDevice` and calls the
    /// instance method `submit_read_async`.
    /// SAFETY: `disk` must be a raw pointer to a VirtIOBlkDevice (cast from `self`),
    /// and `completion` must be a valid pointer to an IoCompletion. Called only
    /// via GenDisk's async_read_fn callback after device initialization.
    unsafe fn async_read_fn(
        disk: *const crate::drivers::blkdev::GenDisk,
        sector: u64,
        buf: &mut [u8],
        completion: *mut core::ffi::c_void,
    ) -> i32 {
        let device = &*(disk as *const VirtIOBlkDevice);
        let comp = &*(completion as *const crate::fs::io_completion::IoCompletion);
        match device.submit_read_async(sector, buf, comp) {
            Ok(()) => 0,
            Err(e) => e,
        }
    }
}

/// Global VirtIO block device (MMIO)
static mut VIRTIO_BLK: Option<VirtIOBlkDevice> = None;

/// Global VirtIO PCI block device (using raw pointer storage)
static mut VIRTIO_PCI_BLK: Option<crate::drivers::virtio::virtio_pci::VirtIOPCI> = None;

/// Global VirtIO PCI block device VirtQueue (configured queue)
static mut VIRTIO_PCI_BLK_QUEUE: Option<queue::VirtQueue> = None;

/// Spinlock to serialize all PCI VirtIO block I/O operations.
pub(crate) static VIRTIO_PCI_BLK_LOCK: Spinlock<()> = Spinlock::new(());

/// Wait queue for PCI VirtIO block I/O completion (interrupt-driven wakeup)
static VIRTIO_PCI_BLK_WAIT_QUEUE: crate::process::wait::WaitQueueHead =
    crate::process::wait::WaitQueueHead::new();

/// Wait queue for MMIO VirtIO block I/O completion (interrupt-driven wakeup)
static VIRTIO_BLK_WAIT_QUEUE: crate::process::wait::WaitQueueHead =
    crate::process::wait::WaitQueueHead::new();

/// Global VirtIO MMIO block device expected used.idx (for tracking I/O completion status)
/// Incremented each time a request is submitted under the queue lock.
/// Each caller reads the value before submit to know which used ring slot to wait for,
/// avoiding the race where two cores read the same queue.get_used() value.
static VIRTIO_MMIO_EXPECTED_USED_IDX: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

/// Maximum number of in-flight async I/O requests per device.
const MAX_PENDING_IO: usize = 16;

/// Pending async I/O request for MMIO VirtIO.
struct PendingIo {
    /// Completion token to signal when done.
    completion: *mut crate::fs::io_completion::IoCompletion,
    /// Pointer to response buffer (allocated during submit, freed on completion).
    resp_ptr: *mut u8,
    /// Layout of response buffer for deallocation.
    resp_layout: alloc::alloc::Layout,
    /// Pointer to request header buffer (freed on completion).
    header_ptr: *mut u8,
    /// Layout of request header buffer for deallocation.
    header_layout: alloc::alloc::Layout,
}

// SAFETY: PendingIo is stored in a Spinlock-protected table and only accessed
// from IRQ/softirq context; raw pointers within are not shared across threads.
unsafe impl Send for PendingIo {}

/// Pending async I/O requests for MMIO VirtIO block device.
/// Indexed by (expected_used_idx % MAX_PENDING_IO).
static VIRTIO_MMIO_PENDING: Spinlock<[Option<PendingIo>; MAX_PENDING_IO]> =
    Spinlock::new([const { None }; MAX_PENDING_IO]);

/// Last processed used index for async completions (MMIO).
static VIRTIO_MMIO_LAST_PROCESSED: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

/// Get current MMIO expected used index (call before submitting request, under queue lock)
#[inline]
fn get_mmio_expected_used_idx() -> u16 {
    VIRTIO_MMIO_EXPECTED_USED_IDX.load(core::sync::atomic::Ordering::Acquire)
}

/// Increment MMIO expected used index (call after submitting request, under queue lock)
#[inline]
fn increment_mmio_expected_used_idx() {
    VIRTIO_MMIO_EXPECTED_USED_IDX.fetch_add(1, core::sync::atomic::Ordering::Release);
}

/// Global VirtIO PCI block device expected used.idx (for tracking I/O completion status)
/// Incremented each time request is submitted, used to detect if device completed request
static VIRTIO_PCI_EXPECTED_USED_IDX: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

/// PCI device ready flag (using atomic type to ensure multi-core visibility)
static VIRTIO_PCI_READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Initialize VirtIO block device
///
/// # Parameters
/// - `base_addr`: MMIO base address (QEMU virt platform typically 0x10001000)
pub fn init(base_addr: u64) -> Result<(), &'static str> {
    // SAFETY: Called once during kernel init; VIRTIO_BLK is a global static
    // that is not accessed concurrently at this point.
    unsafe {
        let mut device = VirtIOBlkDevice::new(base_addr);

        device.init()?;

        // Store device to static variable
        VIRTIO_BLK = Some(device);

        // Device is now in static storage, update private_data pointer
        if let Some(ref mut dev) = VIRTIO_BLK {
            let device_ptr = dev as *const VirtIOBlkDevice as *mut u8;
            dev.disk.private_data = Some(device_ptr);
        }

        Ok(())
    }
}

/// Register PCI VirtIO device
///
/// # Parameters
/// - `device`: PCI VirtIO device
pub fn register_pci_device(device: crate::drivers::virtio::virtio_pci::VirtIOPCI) {
    // SAFETY: Called once during device probe before any I/O requests;
    // SeqCst fence ensures write visibility before ready flag is set.
    unsafe {
        VIRTIO_PCI_BLK = Some(device);
        // Ensure device write is visible to all CPUs
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        // Set ready flag (must be set after writing device)
        VIRTIO_PCI_READY.store(true, core::sync::atomic::Ordering::SeqCst);
    }
}

/// Get VirtIO block device
///
/// Returns PCI VirtIO device first, or MMIO device if unavailable
pub fn get_device() -> Option<&'static VirtIOBlkDevice> {
    // SAFETY: VIRTIO_BLK is initialized before any caller; we return an immutable
    // reference and the device's internal locks protect mutable state.
    unsafe {
        // If PCI device exists, use it for I/O
        // Note: Currently PCI device uses separate I/O interface, returning MMIO device as fallback
        VIRTIO_BLK.as_ref()
    }
}

/// Get PCI VirtIO device
pub fn get_pci_device() -> Option<&'static crate::drivers::virtio::virtio_pci::VirtIOPCI> {
    // Check if device is ready
    if !VIRTIO_PCI_READY.load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    // SAFETY: Ready flag guarantees VIRTIO_PCI_BLK was written; returning
    // immutable reference while device is initialized and not being mutated.
    unsafe {
        VIRTIO_PCI_BLK.as_ref()
    }
}

/// Set PCI VirtIO block device's VirtQueue
///
/// # Parameters
/// - `queue`: Configured VirtQueue
pub fn set_pci_device_queue(queue: queue::VirtQueue) {
    // SAFETY: Called once during device init before any I/O; stores
    // the configured VirtQueue into the global static.
    unsafe {
        // Store reference instead of moving queue
        VIRTIO_PCI_BLK_QUEUE = Some(queue);
        // Initialize expected used.idx to 0 (new queue starts at 0)
        VIRTIO_PCI_EXPECTED_USED_IDX.store(0, core::sync::atomic::Ordering::Release);
    }
}

/// Get PCI VirtIO block device's VirtQueue (mutable reference)
pub fn get_pci_device_queue_mut() -> Option<&'static mut queue::VirtQueue> {
    // Check if device is ready
    if !VIRTIO_PCI_READY.load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    // SAFETY: Ready flag guarantees VIRTIO_PCI_BLK_QUEUE was initialized.
    // Caller must hold VIRTIO_PCI_BLK_LOCK for mutual exclusion.
    unsafe {
        VIRTIO_PCI_BLK_QUEUE.as_mut()
    }
}

/// Get PCI VirtIO block device's VirtQueue (read-only reference)
pub fn get_pci_device_queue() -> Option<&'static queue::VirtQueue> {
    // Check if device is ready
    if !VIRTIO_PCI_READY.load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    // SAFETY: Ready flag guarantees VIRTIO_PCI_BLK_QUEUE was initialized.
    unsafe {
        VIRTIO_PCI_BLK_QUEUE.as_ref()
    }
}

/// Get expected used.idx (for waiting I/O completion)
pub fn get_expected_used_idx() -> u16 {
    VIRTIO_PCI_EXPECTED_USED_IDX.load(core::sync::atomic::Ordering::Acquire)
}

/// Increment expected used.idx (called after submitting request)
pub fn increment_expected_used_idx() {
    VIRTIO_PCI_EXPECTED_USED_IDX.fetch_update(
        core::sync::atomic::Ordering::Release,
        core::sync::atomic::Ordering::Relaxed,
        |v| Some(v.wrapping_add(1))
    ).ok();
}

/// Get reference to PCI VirtIO block wait queue (for interrupt handler)
pub fn get_pci_blk_wait_queue() -> &'static crate::process::wait::WaitQueueHead {
    &VIRTIO_PCI_BLK_WAIT_QUEUE
}

/// Get reference to MMIO VirtIO block wait queue (for interrupt handler)
pub fn get_mmio_blk_wait_queue() -> &'static crate::process::wait::WaitQueueHead {
    &VIRTIO_BLK_WAIT_QUEUE
}

/// Register PCI VirtIO device's GenDisk
///
/// Creates a GenDisk wrapper so ext4 driver can access PCI VirtIO device through standard block device interface
pub fn register_pci_gen_disk() {
    use alloc::boxed::Box;

    // SAFETY: Called once during device initialization; VIRTIO_PCI_BLK is already initialized.
    unsafe {

        // Create GenDisk
        let mut disk = Box::new(GenDisk::new(
            "pci-virtblk",
            8,  // major number (arbitrary, but unique)
            1,  // minors
            512, // block size
            None as Option<&BlockDeviceOps>,
        ));

        // Read device capacity
        if let Some(pci_dev) = VIRTIO_PCI_BLK.as_ref() {
            let device_cfg_addr = pci_dev.common_cfg_bar + 0x2000;
            let capacity_ptr = device_cfg_addr as *const u64;
            let capacity_sectors = core::ptr::read_volatile(capacity_ptr);
            disk.set_capacity(capacity_sectors as u64);
        }

        // Set request handler function
        disk.set_request_fn(pci_virtio_handle_request);

        // Register to block device manager
        let _ = crate::drivers::blkdev::register_disk(disk);
    }
}

/// PCI VirtIO block device request handler
///
/// This function is called by block device layer to handle read/write requests.
///
/// SAFETY: `req.device` points to a valid GenDisk registered by register_pci_gen_disk.
unsafe extern "C" fn pci_virtio_handle_request(req: &mut Request) {
    use crate::drivers::blkdev::ReqCmd;

    // Check if device is ready (use SeqCst for strongest memory visibility)
    if !VIRTIO_PCI_READY.load(core::sync::atomic::Ordering::SeqCst) {
        crate::pr_err!("virtio: PCI device not ready");
        if let Some(end_io) = req.end_io {
            end_io(req, -6);  // ENXIO
        }
        return;
    }

    // Get PCI device
    let pci_dev = match VIRTIO_PCI_BLK.as_ref() {
        Some(dev) => dev,
        None => {
            crate::pr_err!("virtio: No PCI device for request");
            if let Some(end_io) = req.end_io {
                end_io(req, -6);  // ENXIO
            }
            return;
        }
    };

    // Execute operation based on command type
    let result = match req.cmd_type {
        ReqCmd::Read => {
            // Read block
            pci_virtio_read_block(pci_dev, req.sector, &mut req.buffer)
        }
        ReqCmd::Write => {
            // Write block
            pci_virtio_write_block(pci_dev, req.sector, &req.buffer)
        }
        ReqCmd::Flush => {
            // Flush operation (return success for now)
            Ok(())
        }
    };

    // Call completion callback
    match result {
        Ok(()) => {
            if let Some(end_io) = req.end_io {
                end_io(req, 0);
            }
        }
        Err(err) => {
            if let Some(end_io) = req.end_io {
                end_io(req, err);
            }
        }
    }
}

/// Read block using PCI VirtIO device
fn pci_virtio_read_block(
    pci_dev: &crate::drivers::virtio::virtio_pci::VirtIOPCI,
    sector: u64,
    buf: &mut [u8],
) -> Result<(), i32> {
    // PCI lock is now managed inside read_block_once()
    use virtio_pci::read_block_using_configured_queue;

    match read_block_using_configured_queue(pci_dev, sector, buf) {
        Ok(_) => Ok(()),
        Err(_) => Err(-5),  // EIO
    }
}

/// Write block using PCI VirtIO device
fn pci_virtio_write_block(
    pci_dev: &crate::drivers::virtio::virtio_pci::VirtIOPCI,
    sector: u64,
    buf: &[u8],
) -> Result<(), i32> {
    // PCI lock is now managed inside write_block_once()
    use virtio_pci::write_block_using_configured_queue;

    match write_block_using_configured_queue(pci_dev, sector, buf) {
        Ok(_) => Ok(()),
        Err(_) => Err(-5),  // EIO
    }
}

/// Get PCI VirtIO GenDisk
///
/// Get PCI VirtIO device's GenDisk from block device manager
pub fn get_pci_gen_disk() -> Option<&'static GenDisk> {
    // PCI VirtIO device uses major number 8
    // SAFETY: get_disk returns a valid raw pointer to a registered GenDisk.
    crate::drivers::blkdev::get_disk(8).map(|ptr| unsafe { &*ptr })
}

/// PCI VirtIO-Blk interrupt handler (Modern VirtIO 1.0+)
///
/// Registered via request_irq. EOI (PLIC complete) is done by the IRQ framework.
pub fn interrupt_handler_pci(_irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    // SAFETY: VIRTIO_PCI_BLK is initialized before IRQ registration;
    // waking wait queue is safe from IRQ context.
    unsafe {
        if let Some(_pci_device) = VIRTIO_PCI_BLK.as_ref() {
            VIRTIO_PCI_BLK_WAIT_QUEUE.wake_up_all();
        }
    }
    crate::interrupt::IrqReturn::Handled
}

/// VirtIO-Blk interrupt handler (Legacy MMIO VirtIO) — top half only.
///
/// Acknowledges the device interrupt and defers completion processing
/// to the Block softirq bottom half.
/// Registered via request_irq. EOI is done by the IRQ framework.
pub fn interrupt_handler(_irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    // SAFETY: VIRTIO_BLK is initialized before IRQ registration; MMIO register
    // reads/writes use volatile access at correct offsets per VirtIO spec.
    unsafe {
        // MMIO VirtIO device (Legacy VirtIO)
        if let Some(device) = VIRTIO_BLK.as_ref() {
            // Read interrupt status (INTERRUPT_STATUS at 0x60)
            let irq_status_ptr = (device.base_addr + 0x60) as *const u32;
            let irq_status = core::ptr::read_volatile(irq_status_ptr);

            if irq_status != 0 {
                // Clear interrupt (INTERRUPT_ACK at 0x64)
                let irq_ack_ptr = (device.base_addr + 0x64) as *mut u32;
                core::ptr::write_volatile(irq_ack_ptr, irq_status);

                // Defer completion processing to Block softirq bottom half
                crate::interrupt::softirq::raise_softirq_irqoff(
                    crate::interrupt::softirq::SoftirqIndex::Block as usize,
                );
                return crate::interrupt::IrqReturn::Handled;
            }
        }
    }
    crate::interrupt::IrqReturn::None
}

/// Block softirq bottom half handler.
///
/// Processes completed VirtIO Block I/O descriptors deferred from
/// the interrupt handler. Runs in softirq context.
pub fn block_bh_handler(_vec: usize) {
    // SAFETY: Runs in softirq context; VIRTIO_BLK is initialized. The irqsave
    // lock on virtqueue ensures mutual exclusion with hardirq handlers.
    unsafe {
        if let Some(device) = VIRTIO_BLK.as_ref() {
            // Read current used ring index (irqsafe: runs in softirq, can be
            // preempted by hard IRQ that also takes this lock)
            let queue_guard = device.virtqueue.lock_irqsave();
            if let Some(ref queue) = *queue_guard {
                let used_ring = queue.used_ring_ptr();
                let used_idx = core::ptr::read_volatile(
                    (used_ring as usize + 2) as *const u16
                );
                let last_processed = VIRTIO_MMIO_LAST_PROCESSED
                    .load(core::sync::atomic::Ordering::Acquire);

                // Process each newly completed descriptor
                let mut i = last_processed;
                while i != used_idx {
                    let slot = i as usize % MAX_PENDING_IO;
                    let mut pending = VIRTIO_MMIO_PENDING.lock_irqsave();
                    if let Some(pending) = pending[slot].take() {
                        // Read response status
                        let status = if !pending.resp_ptr.is_null() {
                            *(pending.resp_ptr as *mut u8)
                        } else {
                            0
                        };
                        let io_status = if status == 0 { 0 } else { -5i32 };

                        // Free allocated buffers
                        alloc::alloc::dealloc(
                            pending.header_ptr, pending.header_layout,
                        );
                        alloc::alloc::dealloc(
                            pending.resp_ptr, pending.resp_layout,
                        );

                        // Signal completion
                        (*pending.completion).complete(io_status);
                    }
                    i = i.wrapping_add(1);
                }
                VIRTIO_MMIO_LAST_PROCESSED.store(used_idx,
                    core::sync::atomic::Ordering::Release);
            }
            // Also wake synchronous waiters (backward compat)
            VIRTIO_BLK_WAIT_QUEUE.wake_up_all();
        }
    }
}

/// Enable VirtIO-Blk device interrupt
///
/// # Parameters
/// - `base_addr`: VirtIO device's MMIO base address
///
/// # Notes
/// Calculates corresponding IRQ number based on MMIO base address and enables it
pub fn enable_device_interrupt(base_addr: u64) {
    // QEMU RISC-V virt platform:
    // - VirtIO devices start at 0x10001000
    // - Each device occupies 0x1000 bytes
    // - IRQ starts at 1, one IRQ per device
    const VIRTIO_MMIO_BASE: u64 = 0x10001000;
    const VIRTIO_MMIO_SIZE: u64 = 0x1000;

    let slot = ((base_addr - VIRTIO_MMIO_BASE) / VIRTIO_MMIO_SIZE) as u32;
    let irq = (slot + 1) as u32;  // IRQ 1-8 correspond to slot 0-7

    crate::pr_info!("virtio-blk: Registering IRQ {} for device at 0x{:x} (slot {})", irq, base_addr, slot);

    // Register handler via IRQ framework (unmasks automatically)
    crate::interrupt::request_irq(
        irq,
        interrupt_handler,
        0,
        "virtio-blk",
        base_addr as usize,
    ).ok();

    // Also update IRQ number in device
    // SAFETY: VIRTIO_BLK is initialized before interrupt setup; writing irq field.
    unsafe {
        if let Some(ref mut dev) = VIRTIO_BLK {
            dev.irq = irq;
        }
    }
}
