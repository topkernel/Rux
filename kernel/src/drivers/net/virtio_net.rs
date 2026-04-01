//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO network device driver

use crate::drivers::virtio::queue;
use crate::drivers::net::space::{NetDevice, NetDeviceOps, DeviceStats, ArpHrdType, dev_flags};
use crate::net::buffer::SkBuff;
use spin::Mutex;

/// VirtIO network device register layout
///
/// Corresponds to VirtIO network device MMIO registers
/// VirtIO Legacy MMIO Register Layout
#[repr(C)]
pub struct VirtIONetRegs {
    _padding0: [u8; 0x00],  // 0x00
    /// Magic number (0x74726976 "virt")
    pub magic_value: u32,   // 0x00
    /// Version
    pub version: u32,        // 0x04
    /// Device ID (network device = 1)
    pub device_id: u32,      // 0x08
    /// Vendor ID
    pub vendor: u32,         // 0x0C
    _padding1: [u8; 0x04],  // 0x10-0x13
    /// Device features
    pub device_features: u32, // 0x14
    _padding2: [u8; 0x18],  // 0x18-0x2F
    /// Queue select
    pub queue_sel: u32,      // 0x30
    /// Queue max count
    pub queue_num_max: u32, // 0x34
    /// Queue count
    pub queue_num: u32,      // 0x38
    /// Queue ready
    pub queue_ready: u32,    // 0x3C
    /// Queue notify
    pub queue_notify: u32,  // 0x40
    _padding3: [u8; 0x0C],  // 0x44-0x4F
    /// Driver status
    pub status: u32,         // 0x50
    _padding4: [u8; 0x4C],  // 0x54-0x9F
    /// Queue descriptor table address
    pub queue_desc: u64,     // 0xA0
    /// Queue available ring address
    pub queue_driver: u64,   // 0xA8
    /// Queue used ring address
    pub queue_device: u64,   // 0xB0
}

/// VirtIO network device configuration
///
/// Corresponds to VirtIO network device configuration space
#[repr(C)]
pub struct VirtIONetConfig {
    /// MAC address
    pub mac: [u8; 6],
    /// Device status
    pub status: u16,
    /// Maximum VIRTIO packet size
    pub mtu: u16,
}

/// VirtIO network packet header
///
/// Corresponds to VirtIO network device packet header format
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtIONetHdr {
    /// Flags
    pub flags: u8,
    /// GSO type
    pub gso_type: u8,
    /// Header length
    pub hdr_len: u16,
    /// GSO size
    pub gso_size: u16,
    /// Checksum start position
    pub csum_start: u16,
    /// Checksum offset
    pub csum_offset: u16,
    /// Buffer count
    pub num_buffers: u16,
}

/// VirtIO network device
pub struct VirtIONetDevice {
    /// MMIO base address
    base_addr: u64,
    /// MAC address
    mac: [u8; 6],
    /// MTU
    mtu: u16,
    /// Initialization status
    initialized: Mutex<bool>,
    /// Transmit queue (TX Queue - Queue 0)
    tx_queue: Mutex<Option<queue::VirtQueue>>,
    /// Receive queue (RX Queue - Queue 1)
    rx_queue: Mutex<Option<queue::VirtQueue>>,
    /// Queue size
    queue_size: u16,
    /// Statistics
    stats: Mutex<DeviceStats>,
    /// RX buffer address list
    rx_buffers: Mutex<alloc::vec::Vec<u64>>,
    /// Last processed RX used index
    rx_last_used: Mutex<u16>,
}

unsafe impl Send for VirtIONetDevice {}

impl VirtIONetDevice {
    /// Create new VirtIO network device
    pub fn new(base_addr: u64) -> Self {
        Self {
            base_addr,
            mac: [0; 6],
            mtu: 1500,
            initialized: Mutex::new(false),
            tx_queue: Mutex::new(None),
            rx_queue: Mutex::new(None),
            queue_size: 0,
            stats: Mutex::new(DeviceStats::default()),
            rx_buffers: Mutex::new(alloc::vec::Vec::new()),
            rx_last_used: Mutex::new(0),
        }
    }

    /// Initialize device
    pub fn init(&mut self) -> Result<(), &'static str> {
        unsafe {
            // VirtIO MMIO register offsets
            const MAGIC_VALUE: u64 = 0x00;
            const VERSION: u64 = 0x04;
            const DEVICE_ID: u64 = 0x08;
            const VENDOR: u64 = 0x0C;
            const DEVICE_FEATURES: u64 = 0x14;
            const QUEUE_SEL: u64 = 0x30;
            const QUEUE_NUM_MAX: u64 = 0x34;
            const QUEUE_NUM: u64 = 0x38;
            const QUEUE_READY: u64 = 0x3C;
            const QUEUE_NOTIFY: u64 = 0x40;
            const STATUS: u64 = 0x50;
            const QUEUE_DESC: u64 = 0xA0;
            const QUEUE_DRIVER: u64 = 0xA8;
            const QUEUE_DEVICE: u64 = 0xB0;

            // Verify magic number
            let magic = core::ptr::read_volatile((self.base_addr + MAGIC_VALUE) as *const u32);
            if magic != 0x74726976 {
                return Err("Invalid VirtIO magic value");
            }

            // Verify version
            let version = core::ptr::read_volatile((self.base_addr + VERSION) as *const u32);
            if version != 1 && version != 2 {
                return Err("Unsupported VirtIO version");
            }

            // Verify device ID (network device = 1)
            let device_id = core::ptr::read_volatile((self.base_addr + DEVICE_ID) as *const u32);
            if device_id != 1 {
                return Err("Not a VirtIO network device");
            }

            // Set driver status: ACKNOWLEDGE
            core::ptr::write_volatile((self.base_addr + STATUS) as *mut u32, 0x01);

            // Set driver status: DRIVER
            core::ptr::write_volatile((self.base_addr + STATUS) as *mut u32, 0x03);

            // Read MAC address (from config space, offset 0x100)
            // In QEMU virt platform, MAC address is at offset 0 in config space
            let config_ptr = (self.base_addr + 0x100) as *const u8;
            for i in 0..6 {
                self.mac[i] = *config_ptr.add(i);
            }

            // Read MTU (from offset 0x106)
            let mtu_ptr = (self.base_addr + 0x106) as *const u16;
            self.mtu = core::ptr::read_volatile(mtu_ptr);
            if self.mtu == 0 {
                self.mtu = 1500; // Default MTU
            }

            // ========== Setup TX queue (Queue 0) ==========
            // Select queue 0
            core::ptr::write_volatile((self.base_addr + QUEUE_SEL) as *mut u32, 0);

            // Read max queue size
            let max_queue_size = core::ptr::read_volatile((self.base_addr + QUEUE_NUM_MAX) as *const u32);
            if max_queue_size == 0 {
                return Err("VirtIO device has zero queue size");
            }

            // Set queue size
            self.queue_size = if max_queue_size < 8 { 4 } else { 8 };

            // Allocate descriptor table
            let desc_size = self.queue_size as usize * core::mem::size_of::<queue::Desc>();
            let desc_layout = alloc::alloc::Layout::from_size_align(desc_size, 16)
                .map_err(|_| "Failed to create descriptor layout")?;
            let desc_ptr = alloc::alloc::alloc(desc_layout) as *mut queue::Desc;
            if desc_ptr.is_null() {
                return Err("Failed to allocate TX descriptor table");
            }

            // Initialize descriptor table
            let desc_slice = core::slice::from_raw_parts_mut(desc_ptr, self.queue_size as usize);
            for desc in desc_slice.iter_mut() {
                *desc = queue::Desc {
                    addr: 0,
                    len: 0,
                    flags: 0,
                    next: 0,
                };
            }

            // Set queue addresses
            core::ptr::write_volatile((self.base_addr + QUEUE_DESC) as *mut u64, desc_ptr as u64);
            core::ptr::write_volatile((self.base_addr + QUEUE_DRIVER) as *mut u64, 0);
            core::ptr::write_volatile((self.base_addr + QUEUE_DEVICE) as *mut u64, 0);

            // Set queue count
            core::ptr::write_volatile((self.base_addr + QUEUE_NUM) as *mut u32, self.queue_size as u32);

            // Set queue ready
            core::ptr::write_volatile((self.base_addr + QUEUE_READY) as *mut u32, 1);

            // Create VirtQueue
            let tx_queue = match queue::VirtQueue::new(
                self.queue_size,
                0,  // queue_index: TX queue is queue 0
                self.base_addr + QUEUE_NOTIFY,
                self.base_addr + 0x60,  // interrupt_status offset
                self.base_addr + 0x64,  // interrupt_ack offset
            ) {
                Some(q) => q,
                None => {
                    alloc::alloc::dealloc(desc_ptr as *mut u8, desc_layout);
                    return Err("Failed to create TX VirtQueue");
                }
            };
            *self.tx_queue.lock() = Some(tx_queue);

            // ========== Setup RX queue (Queue 1) ==========
            // Select queue 1
            core::ptr::write_volatile((self.base_addr + QUEUE_SEL) as *mut u32, 1);

            // Allocate descriptor table
            let desc_ptr_rx = alloc::alloc::alloc(desc_layout) as *mut queue::Desc;
            if desc_ptr_rx.is_null() {
                alloc::alloc::dealloc(desc_ptr as *mut u8, desc_layout);
                return Err("Failed to allocate RX descriptor table");
            }

            // Initialize descriptor table
            let desc_slice_rx = core::slice::from_raw_parts_mut(desc_ptr_rx, self.queue_size as usize);
            for desc in desc_slice_rx.iter_mut() {
                *desc = queue::Desc {
                    addr: 0,
                    len: 0,
                    flags: 0,
                    next: 0,
                };
            }

            // Set queue addresses
            core::ptr::write_volatile((self.base_addr + QUEUE_DESC) as *mut u64, desc_ptr_rx as u64);
            core::ptr::write_volatile((self.base_addr + QUEUE_DRIVER) as *mut u64, 0);
            core::ptr::write_volatile((self.base_addr + QUEUE_DEVICE) as *mut u64, 0);

            // Set queue count
            core::ptr::write_volatile((self.base_addr + QUEUE_NUM) as *mut u32, self.queue_size as u32);

            // Set queue ready
            core::ptr::write_volatile((self.base_addr + QUEUE_READY) as *mut u32, 1);

            // Create VirtQueue
            let rx_queue = match queue::VirtQueue::new(
                self.queue_size,
                1,  // queue_index: RX queue is queue 1
                self.base_addr + QUEUE_NOTIFY,
                self.base_addr + 0x60,  // interrupt_status offset
                self.base_addr + 0x64,  // interrupt_ack offset
            ) {
                Some(q) => q,
                None => {
                    alloc::alloc::dealloc(desc_ptr as *mut u8, desc_layout);
                    alloc::alloc::dealloc(desc_ptr_rx as *mut u8, desc_layout);
                    return Err("Failed to create RX VirtQueue");
                }
            };
            *self.rx_queue.lock() = Some(rx_queue);

            // Set driver status: DRIVER_OK
            core::ptr::write_volatile((self.base_addr + STATUS) as *mut u32, 0x07);

            // Mark as initialized
            *self.initialized.lock() = true;

            // Fill initial RX buffers
            drop(());  // Release all locks
            self.refill_rx_buffers();

            Ok(())
        }
    }

    /// Get MAC address
    pub fn get_mac(&self) -> [u8; 6] {
        self.mac
    }

    /// Get MTU
    pub fn get_mtu(&self) -> u16 {
        self.mtu
    }

    /// Transmit packet
    ///
    /// # Parameters
    /// - `skb`: Packet to transmit
    ///
    /// # Returns
    /// 0 on success, negative error code on failure
    pub fn xmit(&self, skb: SkBuff) -> i32 {
        if !*self.initialized.lock() {
            return -5; // EIO
        }

        // Get TX queue
        let mut queue_guard = self.tx_queue.lock();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return -5, // EIO
        };

        // Allocate VirtIO network packet header
        let hdr_layout = alloc::alloc::Layout::new::<VirtIONetHdr>();
        let hdr_ptr: *mut VirtIONetHdr;
        unsafe {
            hdr_ptr = alloc::alloc::alloc(hdr_layout) as *mut VirtIONetHdr;
        }
        if hdr_ptr.is_null() {
            return -12; // ENOMEM
        }
        unsafe {
            *hdr_ptr = VirtIONetHdr {
                flags: 0,
                gso_type: 0,
                hdr_len: 0,
                gso_size: 0,
                csum_start: 0,
                csum_offset: 0,
                num_buffers: 1,
            };
        }

        // VirtIO descriptor flags
        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        // Allocate two descriptors
        let header_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => return -5,  // EIO
        };
        let data_desc_idx = match queue.alloc_desc() {
            Some(idx) => idx,
            None => return -5,  // EIO
        };

        // Set packet header descriptor
        queue.set_desc(
            header_desc_idx,
            hdr_ptr as u64,
            core::mem::size_of::<VirtIONetHdr>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // Set data descriptor
        queue.set_desc(
            data_desc_idx,
            skb.data as u64,
            skb.len,
            0,  // Last descriptor
            0,
        );

        // Submit to available ring
        queue.submit(header_desc_idx);

        // Notify device
        queue.notify();

        // Wait for completion
        let prev_used = queue.get_used();
        let _used = queue.wait_for_completion(prev_used);

        // Free packet header
        unsafe {
            alloc::alloc::dealloc(hdr_ptr as *mut u8, hdr_layout);
        }

        // Update statistics
        let mut stats = self.stats.lock();
        stats.tx_packets += 1;
        stats.tx_bytes += skb.len as u64;

        // Free skb
        skb.free();

        0
    }

    /// Receive packet
    ///
    /// # Returns
    /// Received packet, or None if no packet available
    pub fn poll(&self) -> Option<SkBuff> {
        if !*self.initialized.lock() {
            return None;
        }

        // Get RX queue
        let mut queue_guard = self.rx_queue.lock();
        let queue = queue_guard.as_mut()?;

        // Get last processed index
        let mut last_used = *self.rx_last_used.lock();
        let current_used = queue.get_used();

        if last_used == current_used {
            return None; // No new packets
        }

        // Get completed descriptor from used ring
        let used_elem = queue.get_used_elem(last_used)?;

        // Update last_used
        last_used = last_used.wrapping_add(1);
        *self.rx_last_used.lock() = last_used;

        let desc_idx = used_elem.id as u16;
        let desc = queue.get_desc(desc_idx)?;

        // VirtIO-Net packet structure:
        // - 12 bytes VirtIONetHdr
        // - Followed by Ethernet frame data
        let total_len = used_elem.len as usize;
        if total_len <= core::mem::size_of::<VirtIONetHdr>() {
            return None; // Data too short
        }

        let pkt_data_len = total_len - core::mem::size_of::<VirtIONetHdr>();
        let hdr_and_data = unsafe {
            core::slice::from_raw_parts(desc.addr as *const u8, total_len)
        };

        // Skip VirtIO-Net header, keep only Ethernet frame
        let eth_data = &hdr_and_data[core::mem::size_of::<VirtIONetHdr>()..];

        // Create SkBuff
        let mut skb = crate::net::buffer::alloc_skb(pkt_data_len as u32 + 64)?;
        skb.skb_put_data(eth_data).ok()?;

        // Update statistics
        let mut stats = self.stats.lock();
        stats.rx_packets += 1;
        stats.rx_bytes += pkt_data_len as u64;

        // Free old RX buffer
        // Use the SAME layout as refill_rx_buffers() allocation:
        //   buf_size = size_of::<VirtIONetHdr>() + mtu + 64, align = 64
        let buf_size = core::mem::size_of::<VirtIONetHdr>() + self.mtu as usize + 64;
        unsafe {
            if let Ok(layout) = alloc::alloc::Layout::from_size_align(buf_size, 64) {
                alloc::alloc::dealloc(desc.addr as *mut u8, layout);
            } else {
                crate::pr_err!("virtio_net: invalid RX dealloc layout buf_size={}", buf_size);
            }
        }

        // Try to refill RX buffers
        drop(queue_guard);
        self.refill_rx_buffers();

        Some(skb)
    }

    /// Refill RX buffers
    fn refill_rx_buffers(&self) {
        let mut queue_guard = self.rx_queue.lock();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return,
        };

        let mut rx_buffers = self.rx_buffers.lock();

        // Check how many buffers need to be filled
        let need_refill = self.queue_size as usize - rx_buffers.len();

        for _ in 0..need_refill.min(4) {  // Fill at most 4 at a time
            // Allocate RX buffer (VirtIO-Net header + MTU + some margin)
            let buf_size = core::mem::size_of::<VirtIONetHdr>() + self.mtu as usize + 64;
            let layout = alloc::alloc::Layout::from_size_align(buf_size, 64);
            let layout = match layout {
                Ok(l) => l,
                Err(_) => continue,
            };

            let buf_ptr = unsafe { alloc::alloc::alloc(layout) as *mut u8 };
            if buf_ptr.is_null() {
                continue;
            }

            // Allocate descriptor
            let desc_idx = match queue.alloc_desc() {
                Some(idx) => idx,
                None => {
                    unsafe { alloc::alloc::dealloc(buf_ptr, layout); }
                    continue;
                }
            };

            // Set descriptor
            // VIRTQ_DESC_F_WRITE = 2 means device can write
            queue.set_desc(desc_idx, buf_ptr as u64, buf_size as u32, 2, 0);

            // Record buffer address
            rx_buffers.push(buf_ptr as u64);

            // Submit to available ring
            queue.submit(desc_idx);
        }
    }

    /// Get statistics
    pub fn get_stats(&self) -> DeviceStats {
        *self.stats.lock()
    }
}

/// VirtIO network device transmit function (for NetDevice calls)
fn virtio_net_xmit(skb: SkBuff) -> i32 {
    // Get global VirtIO network device
    unsafe {
        if let Some(device) = VIRTIO_NET.as_ref() {
            device.xmit(skb)
        } else {
            skb.free();
            -5 // EIO
        }
    }
}

/// VirtIO network device statistics function
fn virtio_net_get_stats() -> DeviceStats {
    unsafe {
        if let Some(device) = VIRTIO_NET.as_ref() {
            device.get_stats()
        } else {
            DeviceStats::default()
        }
    }
}

/// VirtIO network device operation interface
static VIRTIO_NET_OPS: NetDeviceOps = NetDeviceOps {
    xmit: virtio_net_xmit,
    init: None,
    uninit: None,
    get_stats: Some(virtio_net_get_stats),
};

/// Global VirtIO network device
static mut VIRTIO_NET: Option<VirtIONetDevice> = None;
static mut VIRTIO_NET_DEVICE: Option<NetDevice> = None;

/// Initialize VirtIO network device
///
/// # Parameters
/// - `base_addr`: MMIO base address (QEMU virt platform typically 0x10001000)
pub fn init(base_addr: u64) -> Result<(), &'static str> {
    unsafe {
        let mut device = VirtIONetDevice::new(base_addr);

        device.init()?;

        // Get MAC address
        let mac = device.get_mac();

        // Create NetDevice
        let mut net_device = NetDevice {
            name: [0u8; 16],
            ifindex: 0,
            mtu: device.get_mtu() as u32,
            type_: ArpHrdType::ARPHRD_ETHER,
            addr: [0u8; 32],
            addr_len: 6,
            netdev_ops: &VIRTIO_NET_OPS,
            priv_: core::ptr::null_mut(),
            stats: DeviceStats::default(),
            flags: dev_flags::IFF_UP | dev_flags::IFF_RUNNING | dev_flags::IFF_BROADCAST,
            rx_queue_len: 0,
        };

        // Set device name
        let name = b"eth0\0";
        net_device.name[..name.len()].copy_from_slice(name);

        // Set MAC address
        net_device.set_address(&mac, 6);

        // Store device
        VIRTIO_NET = Some(device);
        VIRTIO_NET_DEVICE = Some(net_device);

        // Register network device
        if let Some(ref mut dev) = VIRTIO_NET_DEVICE {
            crate::drivers::net::register_netdevice(dev);
        }

        Ok(())
    }
}

/// Get VirtIO network device
pub fn get_device() -> Option<&'static VirtIONetDevice> {
    unsafe { VIRTIO_NET.as_ref() }
}

/// Get VirtIO network device's NetDevice
pub fn get_net_device() -> Option<&'static mut NetDevice> {
    unsafe { VIRTIO_NET_DEVICE.as_mut() }
}

/// Get VirtIO network device's base address
fn get_device_base_addr() -> Option<u64> {
    unsafe { VIRTIO_NET.as_ref().map(|dev| dev.base_addr) }
}

/// VirtIO-Net interrupt handler
///
/// Called when VirtIO-Net device generates interrupt.
/// Registered via request_irq. EOI is done by the IRQ framework.
pub fn interrupt_handler(_irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    // Get device base address
    let base_addr = match get_device_base_addr() {
        Some(addr) => addr,
        None => return crate::interrupt::IrqReturn::None,
    };

    unsafe {
        // Read interrupt status (INTERRUPT_STATUS at 0x60)
        let irq_status_ptr = (base_addr + 0x60) as *const u32;
        let irq_status = core::ptr::read_volatile(irq_status_ptr);

        if irq_status != 0 {
            // Clear interrupt (INTERRUPT_ACK at 0x64)
            let irq_ack_ptr = (base_addr + 0x64) as *mut u32;
            core::ptr::write_volatile(irq_ack_ptr, irq_status);

            // Poll received packets
            crate::net::ethernet::ethernet_poll();
            return crate::interrupt::IrqReturn::Handled;
        }
    }
    crate::interrupt::IrqReturn::None
}

/// Enable VirtIO-Net device interrupt
///
/// Registers the handler via request_irq.
pub fn enable_device_interrupt(base_addr: u64) {
    const VIRTIO_MMIO_BASE: u64 = 0x10001000;
    const VIRTIO_MMIO_SIZE: u64 = 0x1000;

    let slot = ((base_addr - VIRTIO_MMIO_BASE) / VIRTIO_MMIO_SIZE) as u32;
    let irq = (slot + 1) as u32;  // IRQ 1-8

    crate::pr_info!("virtio-net: Registering IRQ {} for device at 0x{:x} (slot {})", irq, base_addr, slot);

    // Register handler via IRQ framework (unmasks automatically)
    crate::interrupt::request_irq(
        irq,
        interrupt_handler,
        0,
        "virtio-net",
        base_addr as usize,
    ).ok();
}
