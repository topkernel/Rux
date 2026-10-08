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
            if status & 0x01 == 0 {
                return Err("Device rejected ACKNOWLEDGE status");
            }

            // 6. State machine: DRIVER (0x02)
            write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02);
            let status = read_reg!(STATUS_OFFSET, "STATUS");
            if status & 0x02 == 0 {
                return Err("Device rejected DRIVER status");
            }

            // Check if device needs reset (NEEDS_RESET bit)
            if status & 0x40 != 0 {
                write_reg!(STATUS_OFFSET, "STATUS", 0x00);
                write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02);
            }

            // 7. Feature negotiation (R35, modeled on the R34 virtio_net fix):
            // a modern (v2) MMIO device MUST have VIRTIO_F_VERSION_1 accepted
            // by the driver. The old code wrote DRIVER_FEATURES=0 through the
            // reset-default selector (word 0) and never touched
            // DRIVER_FEATURES_SEL — word 1 stayed 0, so VERSION_1 was never
            // acked and QEMU marked the device FAILED once FEATURES_OK got
            // set. Nothing beyond VERSION_1 is implemented (RO/FLUSH/
            // BLK_SIZE/... all deliberately not accepted).
            const DEVICE_FEATURES_SEL_OFFSET: u64 = 0x014;
            const DRIVER_FEATURES_SEL_OFFSET: u64 = 0x024;

            // VIRTIO_F_VERSION_1 = bit 32 = word 1, bit 0
            const F_VERSION_1: u32 = 1 << 0;

            // Read word 1 and require VERSION_1
            write_reg!(DEVICE_FEATURES_SEL_OFFSET, "DEVICE_FEATURES_SEL", 1);
            let feats_hi = read_reg!(DEVICE_FEATURES_OFFSET, "DEVICE_FEATURES");
            if feats_hi & F_VERSION_1 == 0 {
                return Err("Device does not offer VIRTIO_F_VERSION_1");
            }
            write_reg!(DEVICE_FEATURES_SEL_OFFSET, "DEVICE_FEATURES_SEL", 0);
            let _feats_lo = read_reg!(DEVICE_FEATURES_OFFSET, "DEVICE_FEATURES");

            // Write back: word 0 = 0 (nothing implemented), word 1 = VERSION_1
            write_reg!(DRIVER_FEATURES_SEL_OFFSET, "DRIVER_FEATURES_SEL", 0);
            write_reg!(DRIVER_FEATURES_OFFSET, "DRIVER_FEATURES", 0);
            write_reg!(DRIVER_FEATURES_SEL_OFFSET, "DRIVER_FEATURES_SEL", 1);
            write_reg!(DRIVER_FEATURES_OFFSET, "DRIVER_FEATURES", F_VERSION_1);

            // 9.5. Set FEATURES_OK bit and verify the device accepted the
            // negotiated set (a modern device clears the bit on rejection).
            write_reg!(STATUS_OFFSET, "STATUS", 0x01 | 0x02 | 0x08);
            let status = read_reg!(STATUS_OFFSET, "STATUS");
            if status & 0x08 == 0 {
                return Err("Device rejected negotiated features (features_ok cleared)");
            }

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
            // R35 (W32): virtio-mmio drops any register access whose size
            // != 4 ("wrong size access" in QEMU) — VirtQueue::new's W16
            // default made every MMIO blk notify a silent no-op, so I/O
            // never started. Use with_notify_width like the R34 net driver.
            let virtqueue = match queue::VirtQueue::with_notify_width(
                self.queue_size,
                0,  // queue_index: block device only uses queue 0
                self.base_addr + 0x50,  // queue_notify
                self.base_addr + 0x60,  // interrupt_status
                self.base_addr + 0x64,  // interrupt_ack
                queue::NotifyWidth::W32,
            ) {
                Some(vq) => vq,
                None => return Err("Failed to allocate VirtQueue"),
            };

            let desc_addr = virtqueue.get_desc_addr();
            let avail_addr = virtqueue.get_avail_addr();
            let used_addr = virtqueue.get_used_addr();
            // 14. Modern virtio-mmio (v2) split queue-address registers (the
            // same layout QEMU virt implements): QueueDesc 0x80/0x84,
            // QueueAvail 0x90/0x94, QueueUsed 0xa0/0xa4, QueueReady 0x44.
            // The previous code reused the PCI common-cfg offsets
            // (0x20-0x34, enable 0x1c): the address writes landed on
            // GuestFeaturesSel/GuestFeatures/QueueSel and QueueReady was
            // never actually set — the device never used the rings this
            // driver submits on (queue setup silently no-op'd).
            const QUEUE_DESC_LO_OFFSET: u64 = 0x80;
            const QUEUE_DESC_HI_OFFSET: u64 = 0x84;
            const QUEUE_DRIVER_LO_OFFSET: u64 = 0x90;
            const QUEUE_DRIVER_HI_OFFSET: u64 = 0x94;
            const QUEUE_DEVICE_LO_OFFSET: u64 = 0xA0;
            const QUEUE_DEVICE_HI_OFFSET: u64 = 0xA4;
            const QUEUE_READY_OFFSET: u64 = 0x44;

            // Convert virtual addresses to physical addresses
            let desc_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(desc_addr)
            ).0;
            let avail_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(avail_addr)
            ).0;
            let used_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(used_addr)
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
            // SAFETY: MMIO config space read; must use volatile to prevent
            // compiler from optimizing out or reordering the device access.
            self.capacity = unsafe { core::ptr::read_volatile(cap_ptr) };

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
                req.error.store(-5, core::sync::atomic::Ordering::Release);
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

        // R8-M2: record the device status on the Request itself —
        // submit_request returns 0 whenever a request_fn exists, so the
        // caller never saw these errors (blkdev_read copied a zero buffer
        // and returned success, which bio then cached as BH_Uptodate).
        match result {
            Ok(()) => {
                req.error.store(0, core::sync::atomic::Ordering::Release);
                if let Some(end_io) = req.end_io {
                    end_io(req, 0);  // Success
                }
            }
            Err(err) => {
                crate::pr_err!("virtio-blk: I/O error: {}", err);
                req.error.store(err, core::sync::atomic::Ordering::Release);
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
        let (used_ring_ptr, prev_used, submitted_desc_id, queue_sz, header_ptr, header_layout, resp_ptr, resp_layout) = {
            // Get VirtQueue (irqsafe: IRQ handler also takes this lock)
            let mut queue_guard = self.virtqueue.lock_irqsave();
            let _nest = VirtioLockNest::new();
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
            let header_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(header_ptr as u64)
            ).0;
            let data_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(buf.as_ptr() as u64)
            ).0;
            let resp_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(resp_ptr as u64)
            ).0;

            // Allocate three descriptors — R22-6: dealloc header/resp on
            // failure (write_block already had this; read_block leaked).
            let header_desc_idx = match queue.alloc_desc() {
                Some(idx) => idx,
                None => {
                    unsafe {
                        alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
                        alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
                    }
                    return Err(-5);
                }
            };
            let data_desc_idx = match queue.alloc_desc() {
                Some(idx) => idx,
                None => {
                    unsafe {
                        alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
                        alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
                    }
                    return Err(-5);
                }
            };
            let resp_desc_idx = match queue.alloc_desc() {
                Some(idx) => idx,
                None => {
                    unsafe {
                        alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
                        alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
                    }
                    return Err(-5);
                }
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
                VIRTQ_DESC_F_WRITE,  // Device writes status byte
                0,
            );

            // Snapshot actual used ring index for per-desc matching
            let prev = queue.get_used();

            // Submit to available ring
            queue.submit(header_desc_idx);

            // Notify device
            queue.notify();

            // Keep global counter in sync for async pending slot tracking
            increment_mmio_expected_used_idx();

            let used_ptr = queue.used_ring_ptr();
            let q_size = queue.queue_size;

            (used_ptr, prev, header_desc_idx as u32, q_size, header_ptr, header_layout, resp_ptr, resp_layout)
        };
        // queue_guard dropped here — VirtQueue spinlock released

        // Phase 2: Wait for THIS descriptor's completion (interrupt-driven)
        let completed = queue::VirtQueue::wait_for_desc_completion(
            used_ring_ptr,
            &VIRTIO_BLK_WAIT_QUEUE,
            prev_used,
            submitted_desc_id,
            queue_sz,
        );

        // Phase 3: Check response
        if !completed {
            // R8-M2 (NEW2 mechanism 2): the descriptors are STILL SUBMITTED
            // — freeing header/resp here (and the caller later dropping its
            // data buffer) let the device DMA 4KB into freed, reallocated
            // memory: the wandering corruption behind fdtable/BufferHead
            // damage. Drain the used ring for THIS descriptor with a long
            // bounded spin; if the device truly never completes, LEAK the
            // header/resp bytes (integrity over a leak) and return EIO.
            // SAFETY: used_ring_ptr is the MMIO-mapped used ring captured
            // under the queue lock in Phase 1; read_volatile is the I/O
            // access pattern the completion path already uses.
            let mut late = false;
            unsafe {
                // Same ring-scan discipline as wait_for_desc_completion:
                // UsedRing is [flags:u16, idx:u16, ring:UsedElem[]].
                for _ in 0..50_000_000u64 {
                    core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
                    let used_idx = core::ptr::read_volatile((used_ring_ptr as usize + 2) as *const u16);
                    let mut scan = prev_used;
                    while scan != used_idx {
                        let slot = scan as usize % queue_sz as usize;
                        let entry_id = core::ptr::read_volatile((used_ring_ptr as usize + 4 + slot * 8) as *const u32);
                        if entry_id == submitted_desc_id {
                            late = true;
                            break;
                        }
                        scan = scan.wrapping_add(1);
                    }
                    if late {
                        break;
                    }
                    core::hint::spin_loop();
                }
            }
            if !late {
                return Err(-5); // EIO — header/resp deliberately leaked
            }
            // Completed late — fall through to the normal status handling.
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
        let (used_ring_ptr, prev_used, submitted_desc_id, queue_sz, header_ptr, header_layout, resp_ptr, resp_layout) = {
            // Get VirtQueue (irqsafe: IRQ handler also takes this lock)
            let mut queue_guard = self.virtqueue.lock_irqsave();
            let _nest = VirtioLockNest::new();
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
            let header_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(header_ptr as u64)
            ).0;
            let data_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(buf.as_ptr() as u64)
            ).0;
            let resp_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(resp_ptr as u64)
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

            // Snapshot actual used ring index for per-desc matching
            let prev = queue.get_used();

            // Submit to available ring
            queue.submit(header_desc_idx);

            // Notify device
            queue.notify();

            // Keep global counter in sync for async pending slot tracking
            increment_mmio_expected_used_idx();

            let used_ptr = queue.used_ring_ptr();
            let q_size = queue.queue_size;

            (used_ptr, prev, header_desc_idx as u32, q_size, header_ptr, header_layout, resp_ptr, resp_layout)
        };
        // queue_guard dropped here — VirtQueue spinlock released

        // Phase 2: Wait for THIS descriptor's completion (interrupt-driven)
        let completed = queue::VirtQueue::wait_for_desc_completion(
            used_ring_ptr,
            &VIRTIO_BLK_WAIT_QUEUE,
            prev_used,
            submitted_desc_id,
            queue_sz,
        );

        // Phase 3: Check response
        if !completed {
            // R8-M2 (NEW2 mechanism 2): the descriptors are STILL SUBMITTED
            // — freeing header/resp here (and the caller later dropping its
            // data buffer) let the device DMA 4KB into freed, reallocated
            // memory: the wandering corruption behind fdtable/BufferHead
            // damage. Drain the used ring for THIS descriptor with a long
            // bounded spin; if the device truly never completes, LEAK the
            // header/resp bytes (integrity over a leak) and return EIO.
            // SAFETY: used_ring_ptr is the MMIO-mapped used ring captured
            // under the queue lock in Phase 1; read_volatile is the I/O
            // access pattern the completion path already uses.
            let mut late = false;
            unsafe {
                // Same ring-scan discipline as wait_for_desc_completion:
                // UsedRing is [flags:u16, idx:u16, ring:UsedElem[]].
                for _ in 0..50_000_000u64 {
                    core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
                    let used_idx = core::ptr::read_volatile((used_ring_ptr as usize + 2) as *const u16);
                    let mut scan = prev_used;
                    while scan != used_idx {
                        let slot = scan as usize % queue_sz as usize;
                        let entry_id = core::ptr::read_volatile((used_ring_ptr as usize + 4 + slot * 8) as *const u32);
                        if entry_id == submitted_desc_id {
                            late = true;
                            break;
                        }
                        scan = scan.wrapping_add(1);
                    }
                    if late {
                        break;
                    }
                    core::hint::spin_loop();
                }
            }
            if !late {
                return Err(-5); // EIO — header/resp deliberately leaked
            }
            // Completed late — fall through to the normal status handling.
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
        let _nest = VirtioLockNest::new();
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

        let header_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(header_ptr as u64),
        ).0;
        let data_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(buf.as_ptr() as u64),
        ).0;
        let resp_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(resp_ptr as u64),
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
            VIRTQ_DESC_F_WRITE, 0); // Device writes the status byte (DRIV-H4)

        let prev = get_mmio_expected_used_idx();
        let slot = prev as usize % MAX_PENDING_IO;
        // Occupancy check BEFORE submit: an entry still sitting at this
        // ordinal is a TOMBSTONE (timed-out chain whose used-ring entry the
        // walker has not consumed — the MMIO queue is single-in-flight, so
        // ordinals and completion order coincide). Overwriting it would
        // silently drop the tombstone's reservation and let the walker fire
        // this new pending with the OLD chain's used-ring entry. Refuse
        // instead; the caller falls back and the tombstone retires shortly.
        {
            let table = VIRTIO_MMIO_PENDING.lock_irqsave();
            if table[slot].is_some() {
                // SAFETY: both buffers were allocated above and are still
                // owned by this frame; no chain was submitted for them.
                unsafe {
                    alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
                    alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
                }
                return Err(-5);
            }
        }
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
            // Unused on the MMIO path: its queue admits ONE in-flight chain
            // (queue_size 8 / 3 descs), so completion order is trivially
            // submission order and the ordinal-indexed walk below is sound.
            head_desc: 0,
            ordinal: 0,
            timed_out: false,
        };
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

/// Maximum number of PCI virtio-blk disks (OH boots six: updater/system/
/// vendor/sys_prod/chip_prod/userdata as vda..vdf; two spare letters).
pub const MAX_PCI_BLK_DISKS: usize = 8;

/// PCI virtio-blk devices (one per slot; slot 0 is the boot disk when the
/// boot probe found PCI disks). Each slot owns its complete I/O engine:
/// device, vring, BLK lock, sync wait queue, expected-used counter, pending
/// table and unked counter — the singletons the pre-1b kernel had, per disk.
static mut PCI_BLK_DEVICES: [Option<crate::drivers::virtio::virtio_pci::VirtIOPCI>; MAX_PCI_BLK_DISKS] =
    [const { None }; MAX_PCI_BLK_DISKS];

/// Configured VirtQueues, one per PCI virtio-blk slot.
static mut PCI_BLK_QUEUES: [Option<queue::VirtQueue>; MAX_PCI_BLK_DISKS] =
    [const { None }; MAX_PCI_BLK_DISKS];

/// Per-slot BLK lock: serializes all I/O operations on ONE disk (submit +
/// pending-store under the lock, completion collection under the same
/// lock). Disks never share a lock, and no path holds two disk locks
/// nested (slot-loops take them strictly sequentially).
pub(crate) static PCI_BLK_LOCKS: [Spinlock<()>; MAX_PCI_BLK_DISKS] =
    [const { Spinlock::new(()) }; MAX_PCI_BLK_DISKS];

/// Per-slot wait queue for PCI VirtIO block I/O completion (interrupt-driven wakeup).
static PCI_BLK_WAIT_QUEUES: [crate::process::wait::WaitQueueHead; MAX_PCI_BLK_DISKS] =
    [const { crate::process::wait::WaitQueueHead::new() }; MAX_PCI_BLK_DISKS];

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
    /// Completion token to signal when done. NULL = TOMBSTONE: the entry
    /// exists only to keep the chain's descriptor window reserved until
    /// the completion walker consumes the chain's used-ring entry. Used
    /// for (a) synchronous chains (their waiter polls its own response
    /// byte and owns the io_buf) and (b) timed-out async chains whose
    /// waiter has unwound (the io_buf is deliberately leaked — the device
    /// may still DMA its status byte — and the completion must never be
    /// fired into memory its owner stopped owning).
    completion: *mut crate::fs::io_completion::IoCompletion,
    /// Pointer to response buffer (allocated during submit, freed on
    /// completion). NULL in tombstones (nothing to free or read).
    resp_ptr: *mut u8,
    /// Layout of response buffer for deallocation.
    resp_layout: alloc::alloc::Layout,
    /// Pointer to request header buffer (freed on completion). NULL in
    /// tombstones.
    header_ptr: *mut u8,
    /// Layout of request header buffer for deallocation.
    header_layout: alloc::alloc::Layout,
    /// Head descriptor id of this request's chain (PCI only): the key the
    /// completion walker matches against the used ring's UsedElem.id.
    /// A virtio-blk chain occupies the three consecutive descriptors
    /// [head_desc, head_desc+2]; the publish-time window check (see
    /// pci_pending_slot_reservable) guarantees this id is UNIQUE among live
    /// table entries, so a used-ring entry can never be misattributed.
    head_desc: u32,
    /// Submission ordinal of this chain (PCI only): used-ring entry i maps
    /// to slot i % MAX_PENDING_IO_PCI, and the walker's positional fast
    /// path expects THIS entry there. Also the instrumentation key: a fired
    /// entry whose ordinal != its used-ring index proves the device
    /// completed out of submission order (VIRTIO_PCI_REORDER_EVENTS).
    ordinal: u16,
    /// Tombstone bookkeeping: true when the chain's waiter has TIMED OUT
    /// (sync deadline or async abandon) and already ran note_timed_out_chain
    /// on the queue. When the walker finally consumes this tombstone's
    /// used-ring entry it calls resolve_leaked_chain(), pairing that
    /// increment — leaked_chains then tracks only chains whose completion
    /// has genuinely not landed yet, instead of accumulating forever.
    timed_out: bool,
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

/// Per-slot expected used.idx (for tracking I/O completion status).
/// Incremented each time a request is submitted on that slot's queue
/// (under the slot's BLK lock); each submitter reads the value before
/// submit to know which used-ring slot to wait for.
static PCI_BLK_EXPECTED_USED_IDX: [core::sync::atomic::AtomicU16; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU16::new(0) }; MAX_PCI_BLK_DISKS];

// ============================================================================
// LOCK ORDER (virtio-blk ABBA fix — the GNOME final6 deadlock family)
//
// INVARIANT VIRTIO-WQ-1: a wait-queue lock (WaitQueueHead's internal
// Spinlock<Vec<WaitQueueEntry>>, taken by wake_up_all / prepare_to_wait /
// finish_wait / add / remove) must NEVER be acquired while holding a virtio
// driver lock (VIRTIO_PCI_BLK_LOCK, the MMIO device's `virtqueue` lock, or a
// VIRTIO_*_PENDING table lock).
//
// Why: WaitQueueHead::wake_up() holds the wait-queue lock across
// sched::wake_up_process() (→ GRQ lock, per-waiter, slow under TCG), and
// IoCompletion::wait() waiters run prepare_to_wait() on their OWN stack
// completion (fill_page_cache_batch keeps [IoCompletion; 128] there) while
// other CPUs submit block I/O. The old completion walker ran
// `(*pending.completion).complete()` (→ wake_up_all → wait-queue lock) while
// STILL HOLDING VIRTIO_PCI_BLK_LOCK with IRQs off; every submitter then piled
// onto VIRTIO_PCI_BLK_LOCK (deadlock watchdog: cpu1/cpu3 stuck on
// VIRTIO_PCI_BLK_LOCK @BSS, cpu0 stuck on a heap wait-queue lock inside
// `Spinlock<Vec<WaitQueueEntry>>` lock_irqsave — fire-gnome-final6
// attempt2/k7542fc3 serial logs) and timer IRQs stopped system-wide.
//
// The fix is the timer.rs R12-3 pattern: completion DELIVERY (dealloc +
// IoCompletion::complete + wake_up_all) happens strictly OUTSIDE the virtio
// locks. The walkers collect finished PendingIo entries under the locks, drop
// the locks, then deliver. `VirtioLockNest` below arms a per-CPU counter so
// debug builds assert the invariant at every wake entry point
// (IoCompletion::complete and the two sync-queue wake_up_all sites).
// ============================================================================

/// Per-CPU depth of held virtio driver locks (debug nesting guard for
/// INVARIANT VIRTIO-WQ-1). 0 on release builds.
#[cfg(debug_assertions)]
static VIRTIO_LOCK_DEPTH: [core::sync::atomic::AtomicUsize; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; crate::config::MAX_CPUS];

/// True while the current CPU holds any virtio driver lock (debug only).
#[cfg(debug_assertions)]
pub fn virtio_lock_held() -> bool {
    let cpu = crate::arch::smp::cpu_id() as usize;
    VIRTIO_LOCK_DEPTH[cpu.min(crate::config::MAX_CPUS - 1)]
        .load(core::sync::atomic::Ordering::Acquire)
        > 0
}

/// RAII marker armed around every virtio driver-lock critical section
/// (debug builds only). IoCompletion::complete() and the wait-queue wake
/// sites assert !virtio_lock_held() so any reintroduced nesting panics at
/// the violation point instead of deadlocking the machine.
#[cfg(debug_assertions)]
pub(crate) struct VirtioLockNest;

#[cfg(debug_assertions)]
impl VirtioLockNest {
    #[inline]
    pub(crate) fn new() -> Self {
        let cpu = (crate::arch::smp::cpu_id() as usize).min(crate::config::MAX_CPUS - 1);
        VIRTIO_LOCK_DEPTH[cpu].fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        Self
    }
}

#[cfg(debug_assertions)]
impl core::ops::Drop for VirtioLockNest {
    #[inline]
    fn drop(&mut self) {
        let cpu = (crate::arch::smp::cpu_id() as usize).min(crate::config::MAX_CPUS - 1);
        VIRTIO_LOCK_DEPTH[cpu].fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
    }
}

/// Compile-time no-op stand-in for release builds.
#[cfg(not(debug_assertions))]
pub(crate) struct VirtioLockNest;

#[cfg(not(debug_assertions))]
impl VirtioLockNest {
    #[inline]
    pub(crate) fn new() -> Self { Self }
}

/// Panic-at-the-violation-point check for INVARIANT VIRTIO-WQ-1 (debug
/// builds only): callers must not be inside a virtio driver-lock critical
/// section when they reach a wait-queue wake.
#[cfg(debug_assertions)]
pub fn assert_no_virtio_lock(what: &str) {
    if virtio_lock_held() {
        panic!(
            "virtio: {} called with a virtio driver lock held — \
             VIRTIO-WQ-1 lock-order violation (wait-queue wake inside the \
             virtio lock; see drivers/virtio/mod.rs)",
            what
        );
    }
}

// ============================================================================
// PCI VirtIO-Blk async read support
//
// The PCI device is the boot/root disk on QEMU virt (-device virtio-blk-pci),
// but only the MMIO device ever registered an `async_read_fn`. bio::bread_async
// therefore returned ENXIO on the root disk and the ext4 read-ahead path was
// silently dead there: every page-cache miss paid a fully synchronous
// submit/sleep/wake virtio round trip (~1.8 ms under TCG), capping sequential
// file reads at ~2 MB/s. This table + submit function give the PCI disk the
// same fire-and-forget read path the MMIO device already had.
// ============================================================================

/// Pending async I/O slots for the PCI VirtIO block device.
/// Must exceed the maximum in-flight chains (queue_size / 3 descriptors);
/// QEMU's virtio-blk-pci default queue size is 128 → 41 chains max.
const MAX_PENDING_IO_PCI: usize = 64;

/// Pending async I/O requests for the PCI VirtIO block device.
///
/// ORDINAL-SLOT DISPATCH + DEVICE-TRUTHED MATCHING: every chain — async
/// read, synchronous read/write/flush — takes one submission ordinal from
/// VIRTIO_PCI_EXPECTED_USED_IDX (under the BLK lock) and publishes at
/// `ordinal % MAX_PENDING_IO_PCI`. The completion walker tries that
/// positional mapping first (used-ring entry i → slot i % 64, exact when
/// the device completes in submission order) and falls back to matching
/// the used-ring entry's actual `UsedElem.id` against each entry's
/// `head_desc`. The fallback is what makes reordering harmless:
///
/// The pure ordinal design (used-ring index == submission ordinal, slot
/// i for entry i) only holds when the device completes chains exactly in
/// submission order. QEMU virtio-blk completes mixed-size chains OUT OF
/// ORDER (a 4 KiB single-block read routinely overtakes an in-flight
/// 256 KiB coalesced readahead chain — counted live in
/// VIRTIO_PCI_REORDER_EVENTS), and the positional-only walker then fired
/// the WRONG pending on every reordering:
///   - a pending whose chain was still in flight fired EARLY with a
///     fabricated -EIO (its response byte still 0xFF); the waiter returned,
///     the caller freed the DMA target (`bfree_multi`) and could exit while
///     the device still wrote into it, and the late used-ring entry then
///     fired yet another innocent pending — a cascade (the wandering heap
///     corruption behind the r1/r3/r4 families and AC-2's 263 mis-matched
///     multi-block-read EIOs), and
///   - when the walker lagged >= 64 entries (Block-softirq starvation), the
///     submit path's stale-slot takeover fired still-waiting completions
///     with a fabricated -EIO, and any waiter that left via the 10s
///     deadline left a pending pointing at its kernel-stack IoCompletion —
///     the walker later fired it into the freed (recycled-to-userspace)
///     stack page: the r2 WAKE-WILD-PTR panic.
/// Matching the device's own UsedElem.id makes completion order
/// irrelevant: a pending fires exactly when ITS chain's used-ring entry
/// appears, never earlier. UNIQUENESS of head_desc among live entries is
/// enforced at publish time (pci_pending_slot_reservable's window check):
/// the free-running descriptor allocator would otherwise recycle a head
/// id every queue_size descriptors (~43 chains on a 128-desc queue) while
/// its previous chain was still in flight, and two live pendings sharing
/// head_desc would make a used-ring entry ambiguous again.
///
/// Synchronous chains publish NULL-completion reservations ("tombstones")
/// at their own ordinal slot so their windows and slots are equally
/// protected and their used-ring entries cannot be misattributed to a
/// later async pending; their waiter polls its own response byte and owns
/// the io_buf. A timing-out waiter flags its tombstone and converts async
/// entries into tombstones via abandon_pending_completion, so a late
/// completion can never touch memory its owner has stopped owning.
///
/// LIVENESS: a still-occupied slot or overlapping window is walker lag,
/// never fatal — submitters drain + retry (see pci_submit_read_async), and
/// because ordinal N+64 cannot publish until entry N was walked, the
/// walker can never trail far enough for the used ring (queue_size deep)
/// to overwrite an unconsumed entry.
static PCI_BLK_PENDING: [Spinlock<[Option<PendingIo>; MAX_PENDING_IO_PCI]>; MAX_PCI_BLK_DISKS] =
    [const { Spinlock::new([const { None }; MAX_PENDING_IO_PCI]) }; MAX_PCI_BLK_DISKS];

/// Last used-ring index processed by the async completion walker, per slot.
static PCI_BLK_PENDING_LAST: [core::sync::atomic::AtomicU16; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU16::new(0) }; MAX_PCI_BLK_DISKS];

/// Chains published to a slot's avail ring since the last device kick.
/// Batch submitters publish quietly and the waiter kicks once — see
/// pci_submit_read_async.
static PCI_BLK_UNKICKED: [core::sync::atomic::AtomicUsize; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; MAX_PCI_BLK_DISKS];

/// Kick the PCI virtio-blk disk in `slot` if any quietly-submitted chains
/// are pending. Callers must invoke this BEFORE sleeping on an async
/// completion (drain paths); it is idempotent and cheap when nothing is
/// unked.
pub fn pci_blk_kick(slot: usize) {
    if slot >= MAX_PCI_BLK_DISKS {
        return;
    }
    if PCI_BLK_UNKICKED[slot].swap(0, core::sync::atomic::Ordering::AcqRel) > 0 {
        if let Some(q) = get_pci_device_queue_at(slot) {
            q.notify();
        }
    }
}

/// Kick EVERY PCI virtio-blk disk with quietly-submitted chains. For
/// disk-agnostic recovery paths (batch drains, lost-completion
/// compensation) where the owning disk is not known; notify() on an idle
/// queue is a harmless MMIO write.
pub fn pci_blk_kick_all() {
    for slot in 0..MAX_PCI_BLK_DISKS {
        if PCI_BLK_UNKICKED[slot].swap(0, core::sync::atomic::Ordering::AcqRel) > 0 {
            if let Some(q) = get_pci_device_queue_at(slot) {
                q.notify();
            }
        }
    }
}

/// True if the 3-descriptor windows of two chains (heads `a` and `b` on a
/// queue of `q` descriptors) share any descriptor. Chains always occupy
/// [head, head+1, head+2] consecutively (header/data/resp).
fn chain_windows_overlap(a: u32, b: u32, q: u32) -> bool {
    if q == 0 {
        return false;
    }
    let d = (a as i64 - b as i64).rem_euclid(q as i64);
    d < 3 || (q as i64 - d) < 3
}

/// Check that a chain CAN be published to the pending table: its ordinal
/// slot must be free and its descriptor window [head, head+2] must not
/// overlap any live entry's window (async pendings, synchronous-chain
/// tombstones, and timeout tombstones alike).
///
/// ORDINAL-SLOT DISPATCH (the 7b7e847 skeleton): every chain — async or
/// synchronous — takes one submission ordinal from
/// VIRTIO_PCI_EXPECTED_USED_IDX and publishes at `ordinal %
/// MAX_PENDING_IO_PCI`. A still-occupied slot means the walker has not
/// consumed the entry from 64 ordinals ago; a window overlap means the
/// free-running descriptor allocator (id mod queue_size, recycled every
/// ~queue_size/3 chains) has wrapped onto a live entry. BOTH are the same
/// condition — the completion walker is lagging — and neither is fatal:
/// unlike the previous design, which BURNED colliding windows and could
/// exhaust the whole descriptor ring into a permanent -EIO, the caller
/// treats "not reservable" as a RETRYABLE error: drop the (BLK) lock,
/// kick + drain completions, and try again. Draining retires finished
/// entries, freeing slots and windows; the steady state (walker keeping
/// up) never hits this path at all.
///
/// Head-descriptor uniqueness among live entries is the invariant the
/// walker's device-truthed matching rests on: a used-ring entry carries
/// only the chain's head id, and with recycled-but-live duplicates the
/// first match could fire the WRONG pending (premature -EIO with the DMA
/// target freed under the device — the AC-2 corruption family).
///
/// Caller must hold the PCI BLK lock (the walker collects under the same
/// lock, so the reservation cannot race it).
pub fn pci_pending_slot_reservable(disk: usize, queue_size: u32, ordinal: u16, head_desc: u16) -> bool {
    if disk >= MAX_PCI_BLK_DISKS {
        return false;
    }
    let slot = ordinal as usize % MAX_PENDING_IO_PCI;
    let table = PCI_BLK_PENDING[disk].lock_irqsave();
    if table[slot].is_some() {
        return false;
    }
    !table.iter().any(|e| match e {
        Some(p) => chain_windows_overlap(p.head_desc, head_desc as u32, queue_size),
        None => false,
    })
}

/// Publish a NULL-completion reservation ("tombstone") for a synchronous
/// PCI chain (read_block_once / write_block_once / flush_block_once), at
/// the chain's own ordinal slot. Keeps the chain's descriptor window
/// reserved until the completion walker consumes its used-ring entry, and
/// ensures the chain's eventual used-ring entry is matched (and discarded)
/// instead of being misattributed to a later async pending that recycled
/// the head descriptor id. Their waiter polls its own response byte and
/// owns the io_buf — the walker must never fire or free anything for them.
///
/// Caller must hold the PCI BLK lock, have passed
/// pci_pending_slot_reservable for this (ordinal, head_desc) under the
/// same lock hold, and have already submitted the chain. Between the
/// reservation check and this publish only abandon_pending_completion can
/// touch the table, and it never frees a slot — so the publish cannot
/// fail; the debug assert documents that invariant.
pub fn pci_publish_sync_chain(disk: usize, queue_size: u32, ordinal: u16, head_desc: u16) {
    if disk >= MAX_PCI_BLK_DISKS {
        return;
    }
    let slot = ordinal as usize % MAX_PENDING_IO_PCI;
    let mut table = PCI_BLK_PENDING[disk].lock_irqsave();
    debug_assert!(
        table[slot].is_none()
            || !table.iter().any(|e| match e {
                Some(p) => chain_windows_overlap(p.head_desc, head_desc as u32, queue_size),
                None => false,
            }),
        "virtio: sync-chain publish lost its reserved slot"
    );
    table[slot] = Some(PendingIo {
        completion: core::ptr::null_mut(),
        resp_ptr: core::ptr::null_mut(),
        resp_layout: alloc::alloc::Layout::from_size_align(1, 1).unwrap(),
        header_ptr: core::ptr::null_mut(),
        header_layout: alloc::alloc::Layout::from_size_align(1, 1).unwrap(),
        head_desc: head_desc as u32,
        ordinal,
        timed_out: false,
    });
}

/// Flag a synchronous chain's tombstone as timed out.
///
/// Called by read_block_once / write_block_once / flush_block_once when
/// their 10s deadline expires (after note_timed_out_chain bumped the
/// queue's leaked counter): when the walker later consumes this
/// tombstone's used-ring entry it will pair the increment with
/// resolve_leaked_chain instead of letting leaked_chains accumulate
/// forever.
pub fn pci_flag_sync_tombstone_timed_out(disk: usize, ordinal: u16) {
    if disk >= MAX_PCI_BLK_DISKS {
        return;
    }
    let slot = ordinal as usize % MAX_PENDING_IO_PCI;
    let mut table = PCI_BLK_PENDING[disk].lock_irqsave();
    if let Some(p) = table[slot].as_mut() {
        if p.completion.is_null() {
            p.timed_out = true;
        }
    }
}

/// Abandon in-flight async I/O bound to one IoCompletion pointer.
///
/// GSD fix (10s-deadline UAF): `IoCompletion::wait` bounds itself at 10s
/// and returns -ETIMEDOUT to escape wedged I/O. That waiter's
/// `IoCompletion` usually lives on its KERNEL STACK
/// (fill_page_cache_batch), so once wait() returns, the stack frame —
/// and the completion with it — is gone. But the pending tables kept the
/// raw pointer, and every later completion walker
/// (`pci_process_async_completions`, the MMIO IRQ path) would have
/// called `complete()` through it: a wake_up_all() over a wait queue
/// that no longer exists, corrupting whatever now owns that stack. The
/// resulting wild `wake_up_process` on a garbage Task pointer was the
/// recurring `KERNPANIC pfault badaddr=0x5e` under gnome-session load.
///
/// Matching entries are converted into TOMBSTONES (null completion, null
/// io_buf pointers, head_desc kept) instead of being removed: nothing
/// dereferences the abandoning waiter's memory after it unwinds, but the
/// chain's descriptor window stays reserved until the walker consumes
/// its used-ring entry — otherwise the free-running allocator could hand
/// the head id to a NEW chain, and the abandoned chain's late used-ring
/// entry would fire that new pending prematurely. The tombstone is
/// flagged timed_out and the queue's leaked counter is bumped
/// (note_timed_out_chain): the admission guard stops counting the chain
/// as in-flight, and when the walker finally consumes the tombstone the
/// decrement pairs again (resolve_leaked_chain). The device may still
/// finish the abandoned chain and DMA into the request buffer — the
/// waiter must therefore NOT free its DMA buffers on the WAIT_TIMED_OUT
/// path (see fill_page_cache_batch / bread_wait callers).
///
/// The chain's header/resp io_buf is deliberately LEAKED, not freed: the
/// device writes the response byte (inside that block) exactly when the
/// chain completes — possibly long after this abandon — so freeing it
/// would hand a late 1-byte DMA a freed heap block (R21-N2 discipline:
/// integrity over a bounded leak; the window reservation additionally
/// guarantees no descriptor of the chain is reused before that moment).
///
/// Returns the number of entries abandoned.
pub fn abandon_pending_completion(
    comp: *mut crate::fs::io_completion::IoCompletion,
) -> usize {
    let mut abandoned = 0usize;
    // Per-disk tally so the admission-guard decrements land on the OWNING
    // queue (note_timed_out_chain pairs with resolve_leaked_chain there).
    let mut abandoned_per_disk = [0usize; MAX_PCI_BLK_DISKS];

    // PCI tables: tombstone (window stays reserved; nothing left to fire).
    // The completion may be in flight on ANY disk — sweep every slot.
    for disk in 0..MAX_PCI_BLK_DISKS {
        let mut table = PCI_BLK_PENDING[disk].lock_irqsave();
        for slot in table.iter_mut() {
            if let Some(p) = slot {
                if p.completion == comp && !p.completion.is_null() {
                    abandoned_per_disk[disk] += 1;
                    *slot = Some(PendingIo {
                        completion: core::ptr::null_mut(),
                        resp_ptr: core::ptr::null_mut(),
                        resp_layout: p.resp_layout,
                        header_ptr: core::ptr::null_mut(),
                        header_layout: p.header_layout,
                        head_desc: p.head_desc,
                        ordinal: p.ordinal,
                        timed_out: true,
                    });
                    // io_buf intentionally leaked (see above).
                    abandoned += 1;
                }
            }
        }
    }
    if abandoned > 0 {
        // Discount the abandoned chains from the in-flight admission
        // guard: their used-ring entries may land arbitrarily late (or
        // never). The walker pairs this with resolve_leaked_chain when
        // the tombstones' entries are consumed. No next_desc skipping:
        // the tombstones' window reservations already keep every
        // descriptor of these chains out of new allocations' reach, and
        // skipping from process context could interleave with a
        // concurrent chain build under the BLK lock (its three
        // descriptors must stay consecutive for the window math).
        // Multi-disk: bump each disk's queue for the chains abandoned from
        // ITS table (tallied per disk above).
        for disk in 0..MAX_PCI_BLK_DISKS {
            let n = abandoned_per_disk[disk];
            if n == 0 {
                continue;
            }
            if let Some(vq) = get_pci_device_queue_at(disk) {
                for _ in 0..n {
                    vq.note_timed_out_chain();
                }
            }
        }
    }
    // MMIO table: same tombstone discipline (ordinal-indexed table).
    {
        let mut table = VIRTIO_MMIO_PENDING.lock_irqsave();
        for slot in table.iter_mut() {
            if let Some(p) = slot {
                if p.completion == comp && !p.completion.is_null() {
                    *slot = Some(PendingIo {
                        completion: core::ptr::null_mut(),
                        resp_ptr: core::ptr::null_mut(),
                        resp_layout: p.resp_layout,
                        header_ptr: core::ptr::null_mut(),
                        header_layout: p.header_layout,
                        head_desc: p.head_desc,
                        ordinal: p.ordinal,
                        timed_out: false,
                    });
                    // header + resp intentionally leaked (same discipline).
                    abandoned += 1;
                }
            }
        }
    }
    if abandoned > 0 {
        crate::pr_err!(
            "virtio: abandoned {} in-flight I/O(s) of a timed-out waiter",
            abandoned
        );
    }
    // Wait out any walker delivery of THIS completion that is still in
    // flight (the entry was taken from a table before our scan, so the
    // tombstone pass above could not retire it — see IoCompletion's
    // `delivering` protocol). The waiter returns as soon as we do, and its
    // IoCompletion usually lives on its kernel stack: without this
    // handoff, a late complete() fired into the recycled stack page was
    // the wandering corruption behind ftest01's cross-file bad-verify.
    if !unsafe { (*comp).wait_deliveries() } {
        crate::pr_err!(
            "virtio: delivery handoff overran spin budget during abandon"
        );
    }
    abandoned
}

/// Submit an async read on the PCI VirtIO block device (no waiting).
///
/// SAFETY: `disk` must be the PCI virtio-blk GenDisk (major 8); `buf` must
/// remain valid and unmodified until the completion fires; `completion` must
/// outlive the I/O. Mirrors `VirtIOBlkDevice::submit_read_async`.
unsafe fn pci_submit_read_async(
    disk: *const crate::drivers::blkdev::GenDisk,
    sector: u64,
    buf: &mut [u8],
    completion: &crate::fs::io_completion::IoCompletion,
) -> Result<(), i32> {
    use queue::{VirtIOBlkReqHeader, VirtIOBlkResp};

    // Route to the disk's own slot: register_pci_gen_disk stashes the
    // slot index in GenDisk.private_data.
    let slot = (*disk).private_data.map(|p| p as usize).unwrap_or(0);
    if slot >= MAX_PCI_BLK_DISKS
        || !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire)
    {
        return Err(-5); // EIO
    }

    // One 64-byte block carries header (offset 0) + response (offset 48) —
    // the same R17-C combined allocation the sync path uses.
    let io_layout = alloc::alloc::Layout::from_size_align(64, 16).unwrap();
    // SAFETY: Layout is non-zero-sized; null check follows immediately.
    let io_buf = unsafe { alloc::alloc::alloc(io_layout) };
    if io_buf.is_null() {
        return Err(-12); // ENOMEM
    }
    let header_ptr = io_buf as *mut VirtIOBlkReqHeader;
    // SAFETY: io_buf is non-null, 64 bytes, 16-byte aligned; +48 is in bounds.
    let resp_ptr = unsafe { io_buf.add(48) } as *mut VirtIOBlkResp;
    // SAFETY: pointers derived above are valid and aligned.
    unsafe {
        *header_ptr = VirtIOBlkReqHeader {
            type_: queue::req_type::VIRTIO_BLK_T_IN,
            reserved: 0,
            sector,
        };
        (*resp_ptr).status = 0xFF;
    }

    const VIRTQ_DESC_F_NEXT: u16 = 1;
    const VIRTQ_DESC_F_WRITE: u16 = 2;

    let header_phys = crate::arch::mm::virt_to_phys(
        crate::arch::mm::VirtAddr::new(header_ptr as u64),
    ).0;
    let data_phys = crate::arch::mm::virt_to_phys(
        crate::arch::mm::VirtAddr::new(buf.as_ptr() as u64),
    ).0;
    let resp_phys = crate::arch::mm::virt_to_phys(
        crate::arch::mm::VirtAddr::new(resp_ptr as u64),
    ).0;

    // Submission attempts. "Walker lag" (ordinal slot still occupied or
    // descriptor window overlapping a live entry) and a full in-flight
    // guard are both SOFT conditions: the completion walker retires
    // finished entries as soon as it runs, so the right response is to
    // drop every lock, kick + drain once, and try again — NOT to burn
    // descriptor windows or fail outright (the old design converted
    // Block-softirq starvation into permanent submission failure, and
    // the sync-read fallback storm that followed piled waiters onto the
    // sync wait queue). The steady state never enters the retry path.
    let mut submitted = false;
    for _attempt in 0..3 {
        let mut lagging = false;
        {
            // The BLK lock is held across submit + pending-store so the
            // completion walker (Block softirq, same lock) can never
            // observe the used-ring advance before the pending entry is
            // published — the lost-completion race documented on the MMIO
            // path.
            let _guard = PCI_BLK_LOCKS[slot].lock_irqsave();
            let _nest = VirtioLockNest::new();

            let virt_queue = match get_pci_device_queue_mut_at(slot) {
                Some(q) => q,
                None => {
                    // SAFETY: io_buf was allocated with io_layout and is
                    // unreferenced; no chain was built for it.
                    unsafe { alloc::alloc::dealloc(io_buf, io_layout); }
                    return Err(-5);
                }
            };
            let q = virt_queue.queue_size as u32;

            let (header_desc_idx, data_desc_idx, resp_desc_idx) =
                match (virt_queue.alloc_desc(), virt_queue.alloc_desc(), virt_queue.alloc_desc()) {
                    (Some(h), Some(d), Some(r)) => (h, d, r),
                    _ => {
                        // In-flight guard exhausted. Draining completions
                        // advances the used ring and lowers the guard, so
                        // retry after the drain below; the burned
                        // descriptor ids are harmless (free-running
                        // counter, nothing submitted through them).
                        lagging = true;
                        (0, 0, 0)
                    }
                };

            if !lagging {
                // Ordinal slot BEFORE submit: every chain — async or
                // synchronous — takes one submission ordinal and
                // publishes at `ordinal % MAX_PENDING_IO_PCI`; the
                // walker's positional fast path expects it there.
                let prev_expected = get_expected_used_idx_at(slot);
                if !pci_pending_slot_reservable(slot, q, prev_expected, header_desc_idx) {
                    // Walker lag: retry after drain below.
                    lagging = true;
                } else {
                    virt_queue.set_desc(header_desc_idx, header_phys,
                        core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
                        VIRTQ_DESC_F_NEXT, data_desc_idx);
                    virt_queue.set_desc(data_desc_idx, data_phys, buf.len() as u32,
                        VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT, resp_desc_idx);
                    virt_queue.set_desc(resp_desc_idx, resp_phys,
                        core::mem::size_of::<VirtIOBlkResp>() as u32,
                        VIRTQ_DESC_F_WRITE, 0);

                    // Batch submission discipline: publish quietly and
                    // count. Every notify is an MMIO trap (device
                    // emulation under TCG, ~hundreds of µs), so a
                    // 128-block read-ahead window kicks once
                    // (pci_blk_kick from the waiter) instead of 128
                    // times. The bound below is a safety kick: past it
                    // the virtqueue is nearly exhausted anyway and a
                    // lost waiter must never depend on someone else's
                    // kick.
                    virt_queue.submit_quiet(header_desc_idx);
                    // Keep the submission counter aligned with the used
                    // ring: the walker's positional fast path maps
                    // used-ring entry i to slot i % MAX_PENDING_IO_PCI.
                    increment_expected_used_idx_at(slot);
                    let unkicked = PCI_BLK_UNKICKED[slot].fetch_add(1, core::sync::atomic::Ordering::AcqRel) + 1;
                    if unkicked >= 32 {
                        PCI_BLK_UNKICKED[slot].store(0, core::sync::atomic::Ordering::Release);
                        virt_queue.notify();
                    }

                    // Publish the pending entry at its ordinal slot,
                    // carrying the chain's head descriptor id for
                    // device-truthed matching. The BLK lock is still
                    // held, so the completion walker (same lock) cannot
                    // observe the used-ring advance before this entry is
                    // visible — no matter how fast the device completes.
                    PCI_BLK_PENDING[slot].lock_irqsave()
                        [prev_expected as usize % MAX_PENDING_IO_PCI] = Some(PendingIo {
                        completion: completion as *const _ as *mut _,
                        resp_ptr: resp_ptr as *mut u8,
                        resp_layout: io_layout,
                        header_ptr: header_ptr as *mut u8,
                        header_layout: io_layout,
                        head_desc: header_desc_idx as u32,
                        ordinal: prev_expected,
                        timed_out: false,
                    });

                    submitted = true;
                }
            }
        }

        if submitted {
            return Ok(());
        }
        if !lagging {
            break;
        }
        // Outside every virtio lock: kick the device and run the walker
        // once so finished entries retire and slots/windows free up.
        pci_blk_kick(slot);
        pci_process_async_completions_slot(slot);
    }

    // Still lagging after the retries — fail; the caller (bread_async)
    // falls back to a synchronous read, which is correct if slower.
    // SAFETY: io_buf was allocated with io_layout and no chain was built
    // from it (every attempt bailed before submit).
    unsafe { alloc::alloc::dealloc(io_buf, io_layout); }
    Err(-5)
}

/// Static wrapper matching GenDisk's `async_read_fn` signature for the PCI disk.
///
/// SAFETY: `disk` is the registered PCI virtio-blk GenDisk; `completion` is a
/// valid IoCompletion pointer that outlives the I/O.
unsafe fn pci_async_read_fn(
    disk: *const crate::drivers::blkdev::GenDisk,
    sector: u64,
    buf: &mut [u8],
    completion: *mut core::ffi::c_void,
) -> i32 {
    let comp = &*(completion as *const crate::fs::io_completion::IoCompletion);
    match pci_submit_read_async(disk, sector, buf, comp) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Out-of-order completion instrumentation: counts used-ring entries whose
/// head descriptor id does NOT match the pending published at the entry's
/// positional slot (`i % MAX_PENDING_IO_PCI`). Zero for the whole boot =
/// the device completed every chain in submission order and the pure
/// ordinal mapping of the 7b7e847 design would have sufficed; a large
/// count proves QEMU virtio-blk reorders (mixed-size chains: a 4 KiB read
/// overtakes an in-flight 256 KiB readahead chain) and justifies the
/// device-truthed fallback match. Rate-limited log so the fact shows up
/// in ordinary serial logs without flooding.
static VIRTIO_PCI_REORDER_EVENTS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Used-ring entries the walker consumed that matched NO live pending
/// (neither positionally nor by head_desc). Under the ordinal+window
/// publish discipline every entry must match — a nonzero count means a
/// submit path published a chain without a pending (or a pending was
/// consumed twice), and the entry's chain has no completion to fire.
/// Counted loudly (rate-limited) so a dispatch bug cannot hide.
static VIRTIO_PCI_UNMATCHED_EVENTS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

// ---- VW forensic instrumentation (temporary; wedge hunt r3) ----------
// Counters that make the ftest01 wedge self-describing: which exit the
// completion walker last took, how many PCI IRQs actually fired per disk,
// and a timer-cadence watchdog that prints the full ring/walker state the
// moment PENDING_LAST lags the used ring with no scheduled consumer.
pub static VW_IRQS: [core::sync::atomic::AtomicU64; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_PCI_BLK_DISKS];
pub static VW_WALKS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
/// Exit-kind tally: 0=fast-path caught up, 1=loop caught up,
/// 2=watermark moved by other, 3=budget exhaustion (re-armed),
/// 4=collected==0 (loop re-check).
pub static VW_EXITS: [core::sync::atomic::AtomicU64; 5] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 5];
/// Jiffy when the walker last exited WITH residual lag (0 = never).
pub static VW_LAST_LAG_EXIT_AT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

#[inline]
fn vw_exit(kind: usize, lag: bool) {
    VW_EXITS[kind].fetch_add(1, core::sync::atomic::Ordering::AcqRel);
    if lag {
        VW_LAST_LAG_EXIT_AT.store(
            crate::drivers::timer::get_jiffies(),
            core::sync::atomic::Ordering::Release,
        );
    }
}

/// On-demand VW forensic report (called from waiter deadline paths and
/// the DUMP! handler — contexts that run even when timers die).
pub fn vw_report(tag: &str) {
    // EXT4 big lock census first (VW forensic): the f05rep2 wedge leaves
    // children D-sleeping while the virtio rings are fully drained.
    {
        let (locked, owner, depth, queued, woken) =
            crate::fs::ext4::EXT4_BIG_LOCK.vw_state();
        crate::pr_err!(
            "VW-EXT4LOCK {} locked={} owner={} depth={} queued={} wokenflag={}",
            tag,
            locked,
            owner,
            depth,
            queued,
            woken
        );
    }
    for disk in 0..MAX_PCI_BLK_DISKS {
        if !PCI_BLK_READY[disk].load(core::sync::atomic::Ordering::Acquire) {
            continue;
        }
        let (used_ring, queue_sz, avail_shadow) = match get_pci_device_queue_at(disk) {
            Some(q) => (q.used_ring_ptr(), q.queue_size, q.avail_shadow_snapshot()),
            None => continue,
        };
        if queue_sz == 0 {
            continue;
        }
        // SAFETY: same idx read as the walker's fast path.
        let used_idx = unsafe {
            core::ptr::read_volatile((used_ring as usize + 2) as *const u16)
        };
        let last = PCI_BLK_PENDING_LAST[disk].load(core::sync::atomic::Ordering::Acquire);
        crate::pr_err!(
            "VW-REPORT {} disk={} used={} last={} lag={} expected={} \
             unkicked={} at_device={} walks={} exits fp={} cu={} wm={} \
             budget={} zero={} last_lag_exit={} softirq=0x{:x}/0x{:x}",
            tag,
            disk,
            used_idx,
            last,
            (used_idx.wrapping_sub(last)) & 0xFFFF,
            PCI_BLK_EXPECTED_USED_IDX[disk]
                .load(core::sync::atomic::Ordering::Acquire),
            PCI_BLK_UNKICKED[disk]
                .load(core::sync::atomic::Ordering::Acquire),
            (avail_shadow.wrapping_sub(used_idx)) & 0xFFFF,
            VW_WALKS.load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[0].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[1].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[2].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[3].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[4].load(core::sync::atomic::Ordering::Acquire),
            VW_LAST_LAG_EXIT_AT.load(core::sync::atomic::Ordering::Acquire),
            crate::interrupt::softirq::softirq_pending_raw(0),
            crate::interrupt::softirq::softirq_pending_raw(1),
        );
    }
}

/// Timer-cadence lag watchdog: called once per jiffy from the Timer
/// softirq (the context that keeps running when everything else wedges).
/// When a ready disk's used ring runs ahead of the walker for >= 3s,
/// print the full forensic snapshot (rate-limited to one per 10s).
pub fn vw_lag_watchdog(now: u64) {
    static FIRST_LAG: [core::sync::atomic::AtomicU64; MAX_PCI_BLK_DISKS] =
        [const { core::sync::atomic::AtomicU64::new(0) }; MAX_PCI_BLK_DISKS];
    static LAST_PRINT: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    for disk in 0..MAX_PCI_BLK_DISKS {
        if !PCI_BLK_READY[disk].load(core::sync::atomic::Ordering::Acquire) {
            continue;
        }
        let (used_ring, queue_sz) = match get_pci_device_queue_at(disk) {
            Some(q) => (q.used_ring_ptr(), q.queue_size),
            None => continue,
        };
        if queue_sz == 0 {
            continue;
        }
        // SAFETY: same idx read as the walker's fast path.
        let used_idx = unsafe {
            core::ptr::read_volatile((used_ring as usize + 2) as *const u16)
        };
        let last = PCI_BLK_PENDING_LAST[disk].load(core::sync::atomic::Ordering::Acquire);
        // Device-side stall component: chains published to the avail ring
        // that the device has NOT completed (avail_shadow - used). Combined
        // with unkicked, separates "device working" from "LOST KICK"
        // (quiet-submitted chains nobody ever notified).
        let _ = &used_ring; // keep the ptr alive for both branches
        let lag_used = used_idx != last;
        // avail_shadow lives in the VirtQueue; get it via the queue helper.
        let avail_shadow = get_pci_device_queue_at(disk)
            .map(|q| q.avail_shadow_snapshot())
            .unwrap_or(0);
        let at_device = (avail_shadow.wrapping_sub(used_idx)) & 0xFFFF;
        let unkicked_now = PCI_BLK_UNKICKED[disk]
            .load(core::sync::atomic::Ordering::Acquire);
        let stalled = lag_used || (at_device != 0 && unkicked_now != 0);
        if !stalled {
            FIRST_LAG[disk].store(0, core::sync::atomic::Ordering::Release);
            continue;
        }
        let first = FIRST_LAG[disk].load(core::sync::atomic::Ordering::Acquire);
        if first == 0 {
            FIRST_LAG[disk].store(now, core::sync::atomic::Ordering::Release);
            continue;
        }
        if now.saturating_sub(first) < 300 {
            continue; // < 3s of lag: not yet a wedge
        }
        let last_print = LAST_PRINT.load(core::sync::atomic::Ordering::Acquire);
        if now.saturating_sub(last_print) < 1000 {
            continue; // rate limit 10s
        }
        LAST_PRINT.store(now, core::sync::atomic::Ordering::Release);
        let expected = PCI_BLK_EXPECTED_USED_IDX[disk]
            .load(core::sync::atomic::Ordering::Acquire);
        let unkicked = PCI_BLK_UNKICKED[disk]
            .load(core::sync::atomic::Ordering::Acquire);
        crate::pr_err!(
            "VW-WEDGE disk={} used={} last={} lag={} expected={} unkicked={} \
             at_device={} avail_shadow={} \
             irqs={} walks={} exits fp={} cu={} wm={} budget={} zero={} \
             last_lag_exit={} softirq_pending=0x{:x}/0x{:x}",
            disk,
            used_idx,
            last,
            (used_idx.wrapping_sub(last)) & 0xFFFF,
            expected,
            unkicked,
            at_device,
            avail_shadow,
            VW_IRQS[disk].load(core::sync::atomic::Ordering::Acquire),
            VW_WALKS.load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[0].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[1].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[2].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[3].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[4].load(core::sync::atomic::Ordering::Acquire),
            VW_LAST_LAG_EXIT_AT.load(core::sync::atomic::Ordering::Acquire),
            crate::interrupt::softirq::softirq_pending_raw(0),
            crate::interrupt::softirq::softirq_pending_raw(1),
        );
    }
}

/// Process completed PCI async reads: walk the used ring from the last
/// processed index and fire the pending I/O matching each entry's chain.
///
/// ORDINAL FAST PATH + DEVICE-TRUTHED FALLBACK: used-ring entry i is first
/// checked against the pending at its positional slot
/// (`i % MAX_PENDING_IO_PCI`) — the submission-ordinal mapping, which is
/// correct whenever the device completes chains in submission order. On a
/// mismatch the walker scans the table for the entry whose `head_desc`
/// equals the used-ring entry's UsedElem.id — the chain the device
/// ACTUALLY completed — so out-of-order completion (QEMU virtio-blk
/// reorders mixed-size chains; counted in VIRTIO_PCI_REORDER_EVENTS)
/// cannot fire the wrong pending: a pending fires exactly when ITS
/// chain's used-ring entry appears, never earlier. Head-descriptor
/// uniqueness among live entries is guaranteed by the publish-time window
/// check (pci_pending_slot_reservable).
///
/// Runs in the Block softirq (raised by the PCI IRQ handler) — never in
/// hard-IRQ context — and from process-context drain/rescue paths.
/// VIRTIO-WQ-1 discipline (the virtio-blk ABBA fix): the walk COLLECTS
/// finished PendingIo entries under the BLK lock, then drops every lock
/// before delivering (dealloc + IoCompletion::complete, which takes a
/// wait-queue lock and calls wake_up_process per waiter).
///
/// LOSSLESS DISPATCH (the ftest01 lost-wakeup fix): the walk loops until
/// it is caught up with the used ring (or its one-window budget is
/// spent), and on EVERY exit with residual lag — budget exhaustion, or a
/// completion that landed mid-pass — it re-raises the Block softirq. The
/// old walker could return with used.idx ahead of PENDING_LAST and NO
/// interrupt scheduled: the device was idle (nothing will interrupt
/// again), the softirq pending bits were clear, and the waiters of the
/// unconsumed entries slept until their 10s deadlines — the ftest01
/// wedge (freeze-dumped live: PENDING_LAST one behind, everything else
/// quiescent, six children asleep).
///
/// The sync-queue wake is GATED on real progress: wake_up_all on the
/// disk's sync queue costs a wake_up_process (GRQ lock) per waiter, and
/// firing it from every Block softirq — including timer-driven passes
/// that consumed nothing — put the six ftest01 children into a permanent
/// wake/re-queue herd that starved the very completions they waited for
/// (freeze-dumps show both CPUs grinding in the GRQ/CFS btree while the
/// used ring runs 1-8 entries ahead of the walker).
///
/// Returns the number of used-ring entries consumed (matched or skipped).
pub fn pci_process_async_completions() {
    for disk in 0..MAX_PCI_BLK_DISKS {
        if PCI_BLK_READY[disk].load(core::sync::atomic::Ordering::Acquire) {
            pci_process_async_completions_slot(disk);
        }
    }
}

/// Per-disk completion walker (see pci_process_async_completions for the
/// design notes; every table/lock reference below is disk-local).
pub fn pci_process_async_completions_slot(slot: usize) -> usize {
    /// Collected-per-lock-pass bound. 16 × sizeof(PendingIo) ≈ 1.3 KiB of
    /// stack per pass; the outer loop repeats until caught up or the total
    /// budget (one queue window) is spent.
    const CHUNK: usize = 16;

    // Fast path: nothing pending. Read the used ring first; if the walker is
    // already caught up, skip the lock entirely.
    let (used_ring, queue_sz) = match get_pci_device_queue_at(slot) {
        Some(q) => (q.used_ring_ptr(), q.queue_size),
        None => return 0,
    };
    if queue_sz == 0 {
        return 0;
    }
    // SAFETY: used ring offset 2 is the idx field (u16); the queue is alive
    // (PCI_BLK_READY was checked by get_pci_device_queue_at).
    let used_idx = unsafe {
        core::ptr::read_volatile((used_ring as usize + 2) as *const u16)
    };
    let last = PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire);
    if used_idx == last {
        vw_exit(0, false);
        return 0;
    }
    VW_WALKS.fetch_add(1, core::sync::atomic::Ordering::AcqRel);

    // Bounded by one queue window per call; u16 wrap-safe walk.
    let mut budget = MAX_PENDING_IO_PCI as u16;
    let mut consumed_total = 0usize;
    loop {
        // ---- Phase 1: collect under the BLK lock (short irqsave section) --
        let mut done: [Option<PendingIo>; CHUNK] = [const { None }; CHUNK];
        let mut collected = 0usize;
        let mut walked = 0usize; // entries consumed this pass (matched+skipped)
        let mut unmatched = 0usize;
        let i_end;
        {
            let _guard = PCI_BLK_LOCKS[slot].lock_irqsave();
            let _nest = VirtioLockNest::new();
            // Re-read under the lock (a submission may have landed since).
            // SAFETY: same field, queue alive.
            let used_idx = unsafe {
                core::ptr::read_volatile((used_ring as usize + 2) as *const u16)
            };
            let mut i = PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire);
            while i != used_idx && budget > 0 && collected < CHUNK {
                budget -= 1;
                core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
                // The completed chain's head descriptor id (UsedElem.id at
                // used_ring + 4 + pos*8; the device publishes it only after
                // the chain — including its DMA writes — is done).
                // SAFETY: ring position `i % queue_sz` is within the used ring.
                let ring_pos = (i as usize) % queue_sz as usize;
                let entry_id = unsafe {
                    core::ptr::read_volatile(
                        (used_ring as usize + 4 + ring_pos * 8) as *const u32
                    )
                };
                // Positional fast path: entry i belongs at ordinal slot
                // i % MAX_PENDING_IO_PCI when completion order equals
                // submission order. (NOTE: `slot` is the DISK; the pending
                // table index below is `ordinal_slot` — do not conflate.)
                let ordinal_slot = i as usize % MAX_PENDING_IO_PCI;
                let mut fired: Option<PendingIo> = None;
                {
                    let mut table = PCI_BLK_PENDING[slot].lock_irqsave();
                    let positional_match = match table[ordinal_slot].as_ref() {
                        Some(p) => p.head_desc == entry_id,
                        None => false,
                    };
                    if positional_match {
                        fired = table[ordinal_slot].take();
                    } else {
                        // Out-of-order (or the positional slot holds a
                        // different live chain): scan for the entry whose
                        // chain the device ACTUALLY completed. The
                        // publish-time window check makes head_desc unique
                        // among live entries, so the match is unambiguous.
                        // Tombstones match too (their used-ring entry
                        // releases the descriptor window and ordinal slot).
                        for e in table.iter_mut() {
                            if let Some(p) = e {
                                if p.head_desc == entry_id {
                                    fired = e.take();
                                    break;
                                }
                            }
                        }
                    }
                    // ABANDON-vs-DELIVERY protocol (the bad-verify fix):
                    // mark the waiter's completion as under delivery BEFORE
                    // the table lock drops. abandon_pending_completion
                    // (the 10s-deadline retire path) scans the tables under
                    // this same lock; if it does NOT find the entry it
                    // knows a walker is mid-delivery and waits for this
                    // count to drain — so a walker can never fire into a
                    // waiter's kernel stack after its wait() returned (the
                    // ftest01 "2048*25 bad verify" wandering corruption).
                    if let Some(p) = fired.as_ref() {
                        if !p.completion.is_null() {
                            unsafe {
                                (*p.completion).begin_delivery();
                            }
                        }
                    }
                }
                match fired.as_ref() {
                    Some(p) => {
                        // Exact per-entry disorder measurement: the entry
                        // was submitted at ordinal p.ordinal but completed
                        // at used-ring index i.
                        if p.ordinal != i {
                            let n = VIRTIO_PCI_REORDER_EVENTS
                                .fetch_add(1, core::sync::atomic::Ordering::AcqRel)
                                + 1;
                            if n == 1 || n % 8192 == 0 {
                                crate::pr_info!(
                                    "virtio-blk: out-of-order completion #{} \
                                     (used-ring order != submission order)",
                                    n
                                );
                            }
                        }
                        done[collected] = fired.take();
                        collected += 1;
                    }
                    None => {
                        // No live pending claims this used-ring entry. Every
                        // submitted chain publishes a pending or tombstone,
                        // so this is a dispatch invariant violation — count
                        // it loudly. The entry is consumed (PENDING_LAST
                        // advances) so one bad entry cannot stall the whole
                        // queue; the log makes the accounting hole visible.
                        unmatched += 1;
                        let n = VIRTIO_PCI_UNMATCHED_EVENTS
                            .fetch_add(1, core::sync::atomic::Ordering::AcqRel)
                            + 1;
                        if n == 1 || n % 4096 == 0 {
                            crate::pr_err!(
                                "virtio-blk: used entry {} (head {}) matched \
                                 no pending — dispatch accounting hole #{}",
                                i,
                                entry_id,
                                n
                            );
                        }
                    }
                }
                walked += 1;
                i = i.wrapping_add(1);
            }
            i_end = i;
            PCI_BLK_PENDING_LAST[slot].store(i, core::sync::atomic::Ordering::Release);
            // _nest, table guards and the BLK lock all drop HERE.
        }
        consumed_total += walked;

        // ---- Phase 2: deliver OUTSIDE every virtio lock (R12-3) -----------
        // Each complete() takes the waiter's wait-queue lock and runs
        // wake_up_process per waiter; doing that under the BLK lock is the
        // ABBA this walker must never reintroduce.
        for k in 0..collected {
            let pending = done[k].take().unwrap();
            // NULL completion = TOMBSTONE (synchronous chain or timed-out
            // abandon): its used-ring entry just released the descriptor
            // window — nothing to read, free, or fire. Sync chains' io_buf
            // is owned and freed by their own waiter; abandoned io_bufs
            // were deliberately leaked at abandon time.
            if pending.completion.is_null() {
                // A TIMED-OUT tombstone's completion finally landed: pair
                // the note_timed_out_chain increment from its deadline path
                // so leaked_chains tracks only genuinely unresolved chains
                // (an ever-growing counter permanently shrank the
                // in-flight admission guard).
                if pending.timed_out {
                    if let Some(vq) = get_pci_device_queue_at(slot) {
                        vq.resolve_leaked_chain();
                    }
                }
                continue;
            }
            let status = unsafe { *(pending.resp_ptr as *mut u8) };
            let io_status = if status == 0 { 0 } else { -5i32 };
            // SAFETY: the combined R17-C block: header at base, resp at
            // +48, both inside the single 64-byte allocation
            // (resp_layout == base).
            unsafe {
                alloc::alloc::dealloc(pending.header_ptr, pending.header_layout);
            }
            // SAFETY: the delivering-protocol count taken under the table
            // lock keeps the waiter (and its stack) alive across this call
            // — abandon waits for it before unwinding.
            unsafe { (*pending.completion).complete(io_status); }
            unsafe { (*pending.completion).end_delivery(); }
        }

        if i_end != PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire) {
            // Another CPU (or our own nested context) already moved the
            // watermark — it owns the remaining entries.
            vw_exit(2, true);
            break;
        }
        // Caught up? Re-read the ring: completions may have landed while we
        // delivered (budget re-check keeps the loop bounded either way).
        // SAFETY: same field read as the fast path.
        let fresh_used = unsafe {
            core::ptr::read_volatile((used_ring as usize + 2) as *const u16)
        };
        if fresh_used == i_end {
            vw_exit(1, false);
            break;
        }
        if budget == 0 {
            // One queue window consumed; more remain. Re-arm the Block
            // softirq so a scheduled consumer always exists for the rest.
            vw_exit(3, true);
            crate::interrupt::softirq::raise_softirq(
                crate::interrupt::softirq::SoftirqIndex::Block as usize,
            );
            break;
        }
        vw_exit(4, true);
    }

    // GATED sync-queue wake: only when this pass actually consumed
    // completions on THIS disk. The unconditional wake (every Block
    // softirq pass, including timer-driven no-op passes) was the wake-all
    // herd that turned six concurrent waiters into a GRQ convoy under
    // TCG — the completion delivery starvation behind the ftest01 wedge.
    // Waking here (rather than only in the BH) also covers the
    // process-context drain/rescue callers: a sync waiter is why they ran.
    if consumed_total > 0 {
        #[cfg(debug_assertions)]
        crate::drivers::virtio::assert_no_virtio_lock(
            "PCI_BLK_WAIT_QUEUES wake (walker)",
        );
        PCI_BLK_WAIT_QUEUES[slot].wake_up_all();
    }
    consumed_total
}

/// Waiter-side rescue for lagged completions (companion to the walker's
/// own re-arm): if this disk's used ring is ahead of the walker and no
/// interrupt is coming (device idle, softirq bits clear), the sleeping
/// waiter is the only context that can make progress — drain the ring
/// ourselves before going back to sleep. Cheap when caught up (two
/// loads). No-op for out-of-range/unready slots.
pub fn pci_rescue_lagged_completions(slot: usize) {
    if slot >= MAX_PCI_BLK_DISKS
        || !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire)
    {
        return;
    }
    let used_ring = match get_pci_device_queue_at(slot) {
        Some(q) => q.used_ring_ptr(),
        None => return,
    };
    // SAFETY: same used-ring idx read as the walker's fast path.
    let used_idx = unsafe {
        core::ptr::read_volatile((used_ring as usize + 2) as *const u16)
    };
    let last = PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire);
    if used_idx != last {
        pci_process_async_completions_slot(slot);
    }
}

/// Per-slot ready flags (set after the device + queue + GenDisk are all in
/// place; multi-core visibility via SeqCst).
static PCI_BLK_READY: [core::sync::atomic::AtomicBool; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicBool::new(false) }; MAX_PCI_BLK_DISKS];

/// Number of PCI virtio-blk slots actually registered (boot probe order:
/// slot i is vd<'a'+i>). Exposed for devtmpfs node creation and letter->
/// disk resolution.
static PCI_BLK_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

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
pub fn register_pci_device(slot: usize, mut device: crate::drivers::virtio::virtio_pci::VirtIOPCI) {
    if slot >= MAX_PCI_BLK_DISKS {
        return;
    }
    // Stamp the slot into the device: every I/O path reaches its queue,
    // lock, wait queue and pending table THROUGH this number.
    device.blk_slot = slot;
    // SAFETY: Called once during device probe before any I/O requests;
    // SeqCst fence ensures write visibility before ready flag is set.
    unsafe {
        PCI_BLK_DEVICES[slot] = Some(device);
        // Ensure device write is visible to all CPUs
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        // Set ready flag (must be set after writing device)
        PCI_BLK_READY[slot].store(true, core::sync::atomic::Ordering::SeqCst);
    }
}

/// Number of registered PCI virtio-blk disks (slots 0..n).
pub fn pci_blk_disk_count() -> usize {
    PCI_BLK_COUNT.load(core::sync::atomic::Ordering::Acquire)
}

/// Register slot `slot` as a live disk (called by the probe after the
/// GenDisk + sysfs registration, once per disk).
pub fn pci_blk_slot_online(slot: usize) {
    PCI_BLK_COUNT.fetch_max(slot + 1, core::sync::atomic::Ordering::AcqRel);
}

/// Is the PCI virtio-blk disk in `slot` ready for I/O?
pub fn pci_blk_slot_ready(slot: usize) -> bool {
    slot < MAX_PCI_BLK_DISKS && PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire)
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

/// Get the BOOT PCI virtio-blk device (slot 0). Legacy single-disk
/// callers (flush, devfs presence checks) keep this view.
pub fn get_pci_device() -> Option<&'static crate::drivers::virtio::virtio_pci::VirtIOPCI> {
    get_pci_device_at(0)
}

/// Get the PCI virtio-blk device in `slot`.
pub fn get_pci_device_at(slot: usize) -> Option<&'static crate::drivers::virtio::virtio_pci::VirtIOPCI> {
    if slot >= MAX_PCI_BLK_DISKS {
        return None;
    }
    // Check if device is ready
    if !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    // SAFETY: The ready flag guarantees the slot was written; returning an
    // immutable reference while the device is initialized and not being
    // mutated.
    unsafe {
        PCI_BLK_DEVICES[slot].as_ref()
    }
}

/// Set a slot's configured VirtQueue.
pub fn set_pci_device_queue(slot: usize, queue: queue::VirtQueue) {
    if slot >= MAX_PCI_BLK_DISKS {
        return;
    }
    // SAFETY: Called once during device init before any I/O; stores
    // the configured VirtQueue into the slot's array cell.
    unsafe {
        PCI_BLK_QUEUES[slot] = Some(queue);
        // Initialize expected used.idx to 0 (new queue starts at 0)
        PCI_BLK_EXPECTED_USED_IDX[slot].store(0, core::sync::atomic::Ordering::Release);
    }
}

/// Get a slot's VirtQueue (mutable reference).
///
/// Caller must hold that slot's PCI_BLK_LOCKS[slot] for mutual exclusion.
pub fn get_pci_device_queue_mut_at(slot: usize) -> Option<&'static mut queue::VirtQueue> {
    if slot >= MAX_PCI_BLK_DISKS {
        return None;
    }
    // Check if device is ready
    if !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    // SAFETY: The ready flag guarantees the slot's queue was initialized;
    // exclusion comes from the slot's BLK lock (caller-held).
    unsafe {
        PCI_BLK_QUEUES[slot].as_mut()
    }
}

/// Get a slot's VirtQueue (read-only reference).
pub fn get_pci_device_queue_at(slot: usize) -> Option<&'static queue::VirtQueue> {
    if slot >= MAX_PCI_BLK_DISKS {
        return None;
    }
    // Check if device is ready
    if !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    // SAFETY: The ready flag guarantees the slot's queue was initialized.
    unsafe {
        PCI_BLK_QUEUES[slot].as_ref()
    }
}

/// Get a slot's expected used.idx (for waiting I/O completion).
pub fn get_expected_used_idx_at(slot: usize) -> u16 {
    if slot >= MAX_PCI_BLK_DISKS {
        return 0;
    }
    PCI_BLK_EXPECTED_USED_IDX[slot].load(core::sync::atomic::Ordering::Acquire)
}

/// Increment a slot's expected used.idx (called after submitting request,
/// under that slot's BLK lock).
pub fn increment_expected_used_idx_at(slot: usize) {
    if slot >= MAX_PCI_BLK_DISKS {
        return;
    }
    PCI_BLK_EXPECTED_USED_IDX[slot].fetch_update(
        core::sync::atomic::Ordering::Release,
        core::sync::atomic::Ordering::Relaxed,
        |v| Some(v.wrapping_add(1))
    ).ok();
}

/// Get a slot's sync-I/O wait queue (for interrupt handler / softirq wake).
pub fn get_pci_blk_wait_queue(slot: usize) -> &'static crate::process::wait::WaitQueueHead {
    &PCI_BLK_WAIT_QUEUES[slot.min(MAX_PCI_BLK_DISKS - 1)]
}

/// Get reference to MMIO VirtIO block wait queue (for interrupt handler)
pub fn get_mmio_blk_wait_queue() -> &'static crate::process::wait::WaitQueueHead {
    &VIRTIO_BLK_WAIT_QUEUE
}

/// Register PCI VirtIO device's GenDisk
///
/// Creates a GenDisk wrapper so ext4 driver can access PCI VirtIO device through standard block device interface
pub fn register_pci_gen_disk(slot: usize) {
    use alloc::boxed::Box;

    if slot >= MAX_PCI_BLK_DISKS {
        return;
    }

    // One GenDisk per PCI function. Majors are allocated 8+slot: the block
    // device manager keys disks by major alone, so every disk needs its
    // own. The slot index rides in GenDisk.private_data — request_fn and
    // async_read_fn route to the disk's own queue/lock/wait-queue/pending
    // table through it. (sysfs/devfs numbers stay Linux-shaped: the
    // virtio-blk major 254 with per-disk minors.)
    let mut disk = Box::new(GenDisk::new(
        "pci-virtblk",
        8 + slot as u32, // major number (unique per disk in the manager)
        1,               // minors
        512,             // block size
        None as Option<&BlockDeviceOps>,
    ));
    disk.set_private_data(slot as *mut u8);

    // Read device capacity from this slot's function.
    if let Some(pci_dev) = get_pci_device_at(slot) {
        let device_cfg_addr = if pci_dev.device_cfg_bar != 0 {
            pci_dev.device_cfg_bar
        } else {
            pci_dev.common_cfg_bar + 0x2000
        };
        let capacity_ptr = device_cfg_addr as *const u64;
        // SAFETY: the device cfg region is valid MMIO for this function.
        let capacity_sectors = unsafe { core::ptr::read_volatile(capacity_ptr) };
        disk.set_capacity(capacity_sectors as u64);
    }

    // Set request handler function
    disk.set_request_fn(pci_virtio_handle_request);
    // Async read path: without this, bio::bread_async failed with ENXIO
    // on the root disk and ext4 read-ahead was silently disabled there
    // (every page miss = one synchronous virtio round trip).
    disk.set_async_read_fn(pci_async_read_fn);

    // Register to block device manager
    let _ = crate::drivers::blkdev::register_disk(disk);
    pci_blk_slot_online(slot);
}

/// PCI VirtIO block device request handler
///
/// This function is called by block device layer to handle read/write requests.
///
/// SAFETY: `req.device` points to a valid GenDisk registered by register_pci_gen_disk.
unsafe extern "C" fn pci_virtio_handle_request(req: &mut Request) {
    use crate::drivers::blkdev::ReqCmd;

    // Route to the OWNING disk: GenDisk.private_data carries the slot.
    let slot = (*req.device).private_data.map(|p| p as usize).unwrap_or(0);

    // Check if device is ready (use SeqCst for strongest memory visibility)
    if slot >= MAX_PCI_BLK_DISKS || !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::SeqCst) {
        crate::pr_err!("virtio: PCI device not ready");
        req.error.store(-6, core::sync::atomic::Ordering::Release);
        if let Some(end_io) = req.end_io {
            end_io(req, -6);  // ENXIO
        }
        return;
    }

    // Get PCI device
    let pci_dev = match get_pci_device_at(slot) {
        Some(dev) => dev,
        None => {
            crate::pr_err!("virtio: No PCI device for request");
            req.error.store(-6, core::sync::atomic::Ordering::Release);
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
            req.error.store(0, core::sync::atomic::Ordering::Release);
            if let Some(end_io) = req.end_io {
                end_io(req, 0);
            }
        }
        Err(err) => {
            // R9-4: record the status — blkdev_read/write check it (the
            // PCI path previously swallowed errors entirely).
            req.error.store(err, core::sync::atomic::Ordering::Release);
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

/// Flush the PCI VirtIO block device's write cache (VIRTIO_BLK_T_FLUSH).
///
/// Persists the disk's volatile cache after the kernel's write-through
/// buffer cache has been drained. Returns Ok(()) when no PCI blk device
/// is registered (nothing to flush) so callers can invoke it blindly.
pub fn flush_pci_blk() -> Result<(), i32> {
    // Flush EVERY registered disk: callers (ext4 sync, fsync paths) want
    // persistence guarantees and a disk name is not threaded through the
    // generic paths. Sequential per-disk flushes — never nested locks.
    for slot in 0..MAX_PCI_BLK_DISKS {
        if let Some(pci_dev) = get_pci_device_at(slot) {
            use virtio_pci::flush_block_using_configured_queue;
            if flush_block_using_configured_queue(&pci_dev).is_err() {
                return Err(-5); // EIO
            }
        }
    }
    Ok(())
}

/// Get the BOOT PCI VirtIO GenDisk (slot 0, major 8).
pub fn get_pci_gen_disk() -> Option<&'static GenDisk> {
    get_pci_gen_disk_at(0)
}

/// Get the PCI VirtIO GenDisk in `slot` (major 8 + slot).
pub fn get_pci_gen_disk_at(slot: usize) -> Option<&'static GenDisk> {
    if slot >= MAX_PCI_BLK_DISKS || !pci_blk_slot_ready(slot) {
        return None;
    }
    // SAFETY: get_disk returns a valid raw pointer to a registered GenDisk.
    crate::drivers::blkdev::get_disk(8 + slot as u32).map(|ptr| unsafe { &*ptr })
}

/// Resolve a vdX disk NAME to its GenDisk ("vdb" -> slot 1). Returns None
/// for non-vdX names or slots with no registered disk.
pub fn get_pci_gen_disk_by_name(name: &str) -> Option<&'static GenDisk> {
    let b = name.as_bytes();
    if b.len() != 3 || b[0] != b'v' || b[1] != b'd' || !b[2].is_ascii_lowercase() {
        return None;
    }
    get_pci_gen_disk_at((b[2] - b'a') as usize)
}

/// PCI VirtIO-Blk interrupt handler (Modern VirtIO 1.0+)
///
/// Registered via request_irq. EOI (PLIC complete) is done by the IRQ framework.
///
/// Top half ONLY (the MMIO ISR discipline, now applied to PCI too): the ISR
/// acknowledges the device and raises the Block softirq. Every wait-queue
/// wakeup — async IoCompletion delivery AND the sync queues — happens in
/// `block_bh_handler` (softirq context, no virtio lock held). The old ISR
/// called wake_up_all() on both sync queues directly in hard-IRQ context:
/// each wake holds the wait-queue lock across wake_up_process (→ GRQ) for
/// EVERY waiter, a long IRQ-off window that fed the BLK↔waitqueue lock
/// convoy (VIRTIO-WQ-1) and stopped timer IRQs system-wide.
pub fn interrupt_handler_pci(_irq: u32, dev_id: usize) -> crate::interrupt::IrqReturn {
    // dev_id is the disk SLOT (registered with IRQF_SHARED so several
    // virtio-blk functions can share a swizzled INTx line; the framework
    // demuxes per dev_id and each instance checks its own ISR register).
    let slot = dev_id;
    if slot >= MAX_PCI_BLK_DISKS {
        return crate::interrupt::IrqReturn::None;
    }
    // SAFETY: the slot's device is initialized before its IRQ registration.
    unsafe {
        if let Some(pci_device) = get_pci_device_at(slot) {
            // Read ISR status FIRST: per the virtio 1.1 spec the read drops
            // the device's interrupt line (device-side EOI for level-
            // triggered INTx). Skipping it caused immediate re-entry
            // storms once the IRQ line actually fires (review DRIV-H3).
            // Bit 0 = queue used, bit 1 = configuration change.
            if pci_device.isr_cfg_bar != 0 {
                let isr = core::ptr::read_volatile(pci_device.isr_cfg_bar as *const u32);
                if isr == 0 {
                    // Not ours — spurious or another device on the shared line.
                    return crate::interrupt::IrqReturn::None;
                }
            }
            // Deferred-wake discipline (R12-3): collect in the top half,
            // deliver in the bottom half. The Block softirq runs at
            // irq_exit() time, so wakeup latency stays bounded while the
            // hard-IRQ section stays minimal.
            VW_IRQS[slot].fetch_add(1, core::sync::atomic::Ordering::AcqRel);
            crate::interrupt::softirq::raise_softirq_irqoff(
                crate::interrupt::softirq::SoftirqIndex::Block as usize,
            );
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
///
/// VIRTIO-WQ-1 discipline (virtio-blk ABBA fix): completed pendings are
/// COLLECTED under the MMIO virtqueue lock, the lock is dropped, and only
/// then are the IoCompletions delivered (complete() takes wait-queue locks
/// and wakes tasks). The final sync-queue wake_up_all()s also run here,
/// OUTSIDE every virtio lock — they moved out of the PCI hard-ISR top half
/// (see interrupt_handler_pci) and out from under the virtqueue guard.
pub fn block_bh_handler(_vec: usize) {
    // SAFETY: Runs in softirq context; VIRTIO_BLK is initialized. The irqsave
    // lock on virtqueue ensures mutual exclusion with hardirq handlers.
    unsafe {
        let mut mmio_collected = 0usize;
        if let Some(device) = VIRTIO_BLK.as_ref() {
            // MMIO pending table holds at most MAX_PENDING_IO live entries,
            // so one collection array covers a full catch-up pass.
            let mut done: [Option<PendingIo>; MAX_PENDING_IO] =
                [const { None }; MAX_PENDING_IO];
            let mut collected = 0usize;
            {
                // Read current used ring index (irqsafe: runs in softirq, can
                // be preempted by hard IRQ that also takes this lock)
                let queue_guard = device.virtqueue.lock_irqsave();
                let _nest = VirtioLockNest::new();
                if let Some(ref queue) = *queue_guard {
                    let used_ring = queue.used_ring_ptr();
                    let used_idx = core::ptr::read_volatile(
                        (used_ring as usize + 2) as *const u16
                    );
                    let last_processed = VIRTIO_MMIO_LAST_PROCESSED
                        .load(core::sync::atomic::Ordering::Acquire);

                    // Collect each newly completed descriptor's pending I/O.
                    // TOMBSTONEs (timed-out waiter / sync chain) are taken
                    // too — their used-ring entry releases the slot — but
                    // are skipped at delivery time below.
                    let mut i = last_processed;
                    while i != used_idx {
                        let slot = i as usize % MAX_PENDING_IO;
                        let mut pending = VIRTIO_MMIO_PENDING.lock_irqsave();
                        if let Some(pending) = pending[slot].take() {
                            // Same walker-vs-abandon delivery handshake as
                            // the PCI path (see IoCompletion::delivering).
                            if !pending.completion.is_null() {
                                (*pending.completion).begin_delivery();
                            }
                            if collected < MAX_PENDING_IO {
                                done[collected] = Some(pending);
                                collected += 1;
                            }
                            // Overflow cannot happen (table holds at most
                            // MAX_PENDING_IO Somes); keep the walker moving.
                        }
                        i = i.wrapping_add(1);
                    }
                    VIRTIO_MMIO_LAST_PROCESSED.store(used_idx,
                        core::sync::atomic::Ordering::Release);
                }
                // queue_guard drops HERE — no virtio lock is held below.
            }

            // Deliver outside the virtqueue lock (R12-3): free buffers and
            // complete, waking waiters without holding any virtio lock.
            for k in 0..collected {
                if let Some(pending) = done[k].take() {
                    // TOMBSTONE (timed-out waiter / sync chain): nothing to
                    // read, free (the io_buf was deliberately leaked), or
                    // fire into unwound memory.
                    if pending.completion.is_null() {
                        continue;
                    }
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
                    (*pending.completion).end_delivery();
                }
            }
            mmio_collected = collected;
        }
        // PCI async reads complete here too (raised by
        // interrupt_handler_pci); independent of the MMIO device so a
        // PCI-only boot still drains its pending table. The per-disk
        // walkers wake their own sync queues, gated on real progress.
        pci_process_async_completions();

        // FIX8 (lost-wakeup wedge), relocated from the PCI hard-ISR top
        // half: every sync waiter whose response byte landed between its
        // used-ring re-check and its schedule() needs a wake from the
        // completion path. Doing it here (softirq, no virtio lock held,
        // IRQs enabled) keeps the hard-IRQ section minimal and cannot
        // nest the wait-queue lock inside a virtio lock.
        //
        // GATED on real progress (the ftest01 herd fix): wake_up_all
        // costs a wake_up_process (GRQ lock) per waiter and every woken
        // waiter re-runs prepare_to_wait/finish_wait on this queue.
        // Firing the wake from EVERY Block softirq pass — including
        // passes raised by timer/other activity that consumed nothing —
        // turned six concurrent sync waiters into a permanent
        // wake/re-queue convoy that starved completion delivery itself
        // (freeze-dumped: both CPUs in the GRQ/CFS machinery, used ring
        // running 1-8 entries ahead of the walker, ftest01 wedged until
        // its 30s kill). The PCI queues' wakes live INSIDE
        // pci_process_async_completions_slot (gated on consumed > 0) so
        // process-context rescue walks wake sync waiters too.
        if mmio_collected > 0 {
            #[cfg(debug_assertions)]
            crate::drivers::virtio::assert_no_virtio_lock(
                "VIRTIO_BLK_WAIT_QUEUE wake (BH)",
            );
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
