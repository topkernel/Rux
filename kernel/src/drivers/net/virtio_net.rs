//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO network device driver

use crate::drivers::virtio::queue;
use crate::drivers::net::space::{NetDevice, NetDeviceOps, DeviceStats, ArpHrdType, dev_flags};
use crate::net::buffer::SkBuff;
use crate::sync::spinlock::Spinlock;

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
    initialized: Spinlock<bool>,
    /// Transmit queue (TX Queue - Queue 0)
    tx_queue: Spinlock<Option<queue::VirtQueue>>,
    /// Receive queue (RX Queue - Queue 1)
    rx_queue: Spinlock<Option<queue::VirtQueue>>,
    /// Queue size
    queue_size: u16,
    /// Statistics
    stats: Spinlock<DeviceStats>,
    /// RX buffer address list
    rx_buffers: Spinlock<alloc::vec::Vec<u64>>,
    /// Last processed RX used index
    rx_last_used: Spinlock<u16>,
}

// SAFETY: All shared state is protected by Spinlocks (irqsave where needed),
// ensuring no data races across threads/CPUs.
unsafe impl Send for VirtIONetDevice {}

impl VirtIONetDevice {
    /// Create new VirtIO network device
    pub fn new(base_addr: u64) -> Self {
        Self {
            base_addr,
            mac: [0; 6],
            mtu: 1500,
            initialized: Spinlock::new(false),
            tx_queue: Spinlock::new(None),
            rx_queue: Spinlock::new(None),
            queue_size: 0,
            stats: Spinlock::new(DeviceStats::default()),
            rx_buffers: Spinlock::new(alloc::vec::Vec::new()),
            rx_last_used: Spinlock::new(0),
        }
    }

    /// Initialize device
    pub fn init(&mut self) -> Result<(), &'static str> {
        // SAFETY: base_addr points to valid VirtIO MMIO registers; all register
        // offsets follow the VirtIO MMIO device specification.
        unsafe {
            // VirtIO MMIO register offsets (linux/virtio_mmio.h — the
            // shared legacy/modern layout; queueSel=0x30, status=0x70,
            // ready=0x44). R34: the previous code was missing feature
            // negotiation entirely (DriverFeatures 0x20/0x24 were never
            // written, so QEMU's virtio-net ran with the legacy 10-byte
            // vnet header while this driver submits 12-byte headers).
            const MAGIC_VALUE: u64 = 0x00;
            const VERSION: u64 = 0x04;
            const DEVICE_ID: u64 = 0x08;
            const DEVICE_FEATURES: u64 = 0x10;
            const DEVICE_FEATURES_SEL: u64 = 0x14;
            const DRIVER_FEATURES: u64 = 0x20;
            const DRIVER_FEATURES_SEL: u64 = 0x24;
            const QUEUE_SEL: u64 = 0x30;
            const QUEUE_NUM_MAX: u64 = 0x34;
            const QUEUE_NUM: u64 = 0x38;
            const QUEUE_READY: u64 = 0x44;
            const QUEUE_NOTIFY: u64 = 0x50;
            const INTERRUPT_STATUS: u64 = 0x60;
            const INTERRUPT_ACK: u64 = 0x64;
            const STATUS: u64 = 0x70;
            const QUEUE_DESC_LO: u64 = 0x80;
            const QUEUE_DESC_HI: u64 = 0x84;
            const QUEUE_DRIVER_LO: u64 = 0x90;
            const QUEUE_DRIVER_HI: u64 = 0x94;
            const QUEUE_DEVICE_LO: u64 = 0xA0;
            const QUEUE_DEVICE_HI: u64 = 0xA4;
            const CONFIG: u64 = 0x100;

            // Feature bits (word << 5 | bit within word)
            const F_NET_MAC: u32 = 1 << 5;          // word 0: device MAC in config
            const F_VERSION_1: u32 = 1 << 0;        // word 1: VIRTIO_F_VERSION_1

            // Status bits
            const S_ACKNOWLEDGE: u32 = 0x01;
            const S_DRIVER: u32 = 0x02;
            const S_FEATURES_OK: u32 = 0x08;
            const S_DRIVER_OK: u32 = 0x04;

            // Verify magic number
            let magic = core::ptr::read_volatile((self.base_addr + MAGIC_VALUE) as *const u32);
            if magic != 0x74726976 {
                return Err("Invalid VirtIO magic value");
            }

            // Verify version — only Modern (v2) is supported. The register
            // block below uses the modern split-address layout (0x80/0x90/
            // 0xa0); legacy v1 devices need the QueuePFN model instead.
            let version = core::ptr::read_volatile((self.base_addr + VERSION) as *const u32);
            if version != 2 {
                return Err("Unsupported VirtIO version (only Modern v2)");
            }

            // Verify device ID (network device = 1)
            let device_id = core::ptr::read_volatile((self.base_addr + DEVICE_ID) as *const u32);
            if device_id != 1 {
                return Err("Not a VirtIO network device");
            }

            // Reset device, then walk the standard status sequence.
            core::ptr::write_volatile((self.base_addr + STATUS) as *mut u32, 0);
            for _ in 0..100_000u32 {
                if core::ptr::read_volatile((self.base_addr + STATUS) as *const u32) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            // Set driver status: ACKNOWLEDGE, then DRIVER
            core::ptr::write_volatile((self.base_addr + STATUS) as *mut u32, S_ACKNOWLEDGE);
            core::ptr::write_volatile((self.base_addr + STATUS) as *mut u32, S_ACKNOWLEDGE | S_DRIVER);

            // R34: feature negotiation was missing entirely. VIRTIO_F_VERSION_1
            // MUST be accepted on a modern (v2) device — without it QEMU's
            // virtio-net operates with the legacy 10-byte vnet header while
            // this driver submits 12-byte headers, corrupting every frame by
            // 2 bytes. Accept exactly VERSION_1 (+MAC); MRG_RXBUF/CTRL_VQ/
            // EVENT_IDX/GSO are deliberately NOT accepted.
            core::ptr::write_volatile((self.base_addr + DEVICE_FEATURES_SEL) as *mut u32, 1);
            let feats_hi = core::ptr::read_volatile((self.base_addr + DEVICE_FEATURES) as *const u32);
            if feats_hi & F_VERSION_1 == 0 {
                return Err("Device does not offer VIRTIO_F_VERSION_1");
            }
            core::ptr::write_volatile((self.base_addr + DEVICE_FEATURES_SEL) as *mut u32, 0);
            let feats_lo = core::ptr::read_volatile((self.base_addr + DEVICE_FEATURES) as *const u32);
            // Word 0: accept only VIRTIO_NET_F_MAC (informative — we read the
            // MAC from config space either way), and only if offered.
            let accept_lo = feats_lo & F_NET_MAC;
            // Word 1: accept VIRTIO_F_VERSION_1.
            let accept_hi = feats_hi & F_VERSION_1;
            core::ptr::write_volatile((self.base_addr + DRIVER_FEATURES_SEL) as *mut u32, 0);
            core::ptr::write_volatile((self.base_addr + DRIVER_FEATURES) as *mut u32, accept_lo);
            core::ptr::write_volatile((self.base_addr + DRIVER_FEATURES_SEL) as *mut u32, 1);
            core::ptr::write_volatile((self.base_addr + DRIVER_FEATURES) as *mut u32, accept_hi);

            core::ptr::write_volatile(
                (self.base_addr + STATUS) as *mut u32,
                S_ACKNOWLEDGE | S_DRIVER | S_FEATURES_OK,
            );
            let status = core::ptr::read_volatile((self.base_addr + STATUS) as *const u32);
            if status & S_FEATURES_OK == 0 {
                return Err("Device rejected negotiated features (features_ok cleared)");
            }

            // Read MAC address (from config space, offset 0x100).
            let config_ptr = (self.base_addr + CONFIG) as *const u8;
            for i in 0..6 {
                self.mac[i] = core::ptr::read_volatile(config_ptr.add(i));
            }

            // Read MTU — virtio-net config: mac[6], status u16 @6,
            // max_virtqueue_pairs u16 @8, mtu u16 @10 → MMIO 0x10a.
            // (0x106 was the old wrong offset: that is the STATUS field.)
            let mtu_ptr = (self.base_addr + CONFIG + 10) as *const u16;
            self.mtu = u16::from_le(core::ptr::read_volatile(mtu_ptr));
            if self.mtu == 0 || self.mtu > 1500 {
                self.mtu = 1500; // No VIRTIO_NET_F_MTU negotiated
            }

            // R34: queue roles were INVERTED. Virtio-net 1.x with a single
            // queue pair is queue 0 = receiveq1, queue 1 = transmitq1 (spec
            // 5.1.3). The old code posted RX buffers on the transmit queue
            // and submitted TX chains on the receive queue — with a real
            // device neither direction could ever work.
            //
            // ========== Setup RX queue (Queue 0) ==========
            core::ptr::write_volatile((self.base_addr + QUEUE_SEL) as *mut u32, 0);

            // Read max queue size
            let max_queue_size = core::ptr::read_volatile((self.base_addr + QUEUE_NUM_MAX) as *const u32);
            if max_queue_size == 0 {
                return Err("VirtIO device has zero queue size");
            }

            // Set queue size
            self.queue_size = if max_queue_size < 8 { 4 } else { 8 };

            // Create VirtQueue (single contiguous desc+avail+used allocation).
            // W32: virtio-mmio rejects notify writes that are not 32-bit.
            let rx_queue = match queue::VirtQueue::with_notify_width(
                self.queue_size,
                0,  // queue_index: RX queue is queue 0
                self.base_addr + QUEUE_NOTIFY,
                self.base_addr + INTERRUPT_STATUS,
                self.base_addr + INTERRUPT_ACK,
                queue::NotifyWidth::W32,
            ) {
                Some(q) => q,
                None => return Err("Failed to create RX VirtQueue"),
            };

            // Register the VirtQueue's OWN rings with the device (modern
            // virtio-mmio split-address layout). R32 fixed the addresses;
            // R34 fixed the register offsets they are written through.
            let rx_desc_phys = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(rx_queue.get_desc_addr())
            ).0;
            let rx_avail_phys = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(rx_queue.get_avail_addr())
            ).0;
            let rx_used_phys = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(rx_queue.get_used_addr())
            ).0;

            // Set queue count
            core::ptr::write_volatile((self.base_addr + QUEUE_NUM) as *mut u32, self.queue_size as u32);

            core::ptr::write_volatile((self.base_addr + QUEUE_DESC_LO) as *mut u32, (rx_desc_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile((self.base_addr + QUEUE_DESC_HI) as *mut u32, (rx_desc_phys >> 32) as u32);
            core::ptr::write_volatile((self.base_addr + QUEUE_DRIVER_LO) as *mut u32, (rx_avail_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile((self.base_addr + QUEUE_DRIVER_HI) as *mut u32, (rx_avail_phys >> 32) as u32);
            core::ptr::write_volatile((self.base_addr + QUEUE_DEVICE_LO) as *mut u32, (rx_used_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile((self.base_addr + QUEUE_DEVICE_HI) as *mut u32, (rx_used_phys >> 32) as u32);

            // Set queue ready
            core::ptr::write_volatile((self.base_addr + QUEUE_READY) as *mut u32, 1);

            *self.rx_queue.lock() = Some(rx_queue);

            // ========== Setup TX queue (Queue 1) ==========
            core::ptr::write_volatile((self.base_addr + QUEUE_SEL) as *mut u32, 1);

            let max_queue_size_tx = core::ptr::read_volatile((self.base_addr + QUEUE_NUM_MAX) as *const u32);
            if max_queue_size_tx < self.queue_size as u32 {
                return Err("VirtIO TX queue smaller than RX queue");
            }

            let tx_queue = match queue::VirtQueue::with_notify_width(
                self.queue_size,
                1,  // queue_index: TX queue is queue 1
                self.base_addr + QUEUE_NOTIFY,
                self.base_addr + INTERRUPT_STATUS,
                self.base_addr + INTERRUPT_ACK,
                queue::NotifyWidth::W32,
            ) {
                Some(q) => q,
                None => return Err("Failed to create TX VirtQueue"),
            };

            let tx_desc_phys = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(tx_queue.get_desc_addr())
            ).0;
            let tx_avail_phys = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(tx_queue.get_avail_addr())
            ).0;
            let tx_used_phys = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(tx_queue.get_used_addr())
            ).0;

            // Set queue count
            core::ptr::write_volatile((self.base_addr + QUEUE_NUM) as *mut u32, self.queue_size as u32);

            core::ptr::write_volatile((self.base_addr + QUEUE_DESC_LO) as *mut u32, (tx_desc_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile((self.base_addr + QUEUE_DESC_HI) as *mut u32, (tx_desc_phys >> 32) as u32);
            core::ptr::write_volatile((self.base_addr + QUEUE_DRIVER_LO) as *mut u32, (tx_avail_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile((self.base_addr + QUEUE_DRIVER_HI) as *mut u32, (tx_avail_phys >> 32) as u32);
            core::ptr::write_volatile((self.base_addr + QUEUE_DEVICE_LO) as *mut u32, (tx_used_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile((self.base_addr + QUEUE_DEVICE_HI) as *mut u32, (tx_used_phys >> 32) as u32);

            // Set queue ready
            core::ptr::write_volatile((self.base_addr + QUEUE_READY) as *mut u32, 1);

            *self.tx_queue.lock() = Some(tx_queue);

            // Set driver status: DRIVER_OK (acknowledge | driver |
            // features_ok | driver_ok)
            core::ptr::write_volatile(
                (self.base_addr + STATUS) as *mut u32,
                S_ACKNOWLEDGE | S_DRIVER | S_FEATURES_OK | S_DRIVER_OK,
            );

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
        if !*self.initialized.lock_irqsave() {
            return -5; // EIO
        }

        // Get TX queue (irqsafe: IRQ handler may access rx/tx queues)
        let mut queue_guard = self.tx_queue.lock_irqsave();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return -5, // EIO
        };

        // Allocate VirtIO network packet header
        let hdr_layout = alloc::alloc::Layout::new::<VirtIONetHdr>();
        let hdr_ptr: *mut VirtIONetHdr;
        // SAFETY: Layout is non-zero-sized; null check follows immediately.
        unsafe {
            hdr_ptr = alloc::alloc::alloc(hdr_layout) as *mut VirtIONetHdr;
        }
        if hdr_ptr.is_null() {
            return -12; // ENOMEM
        }
        // SAFETY: hdr_ptr is non-null and properly aligned for VirtIONetHdr.
        unsafe {
            *hdr_ptr = VirtIONetHdr {
                flags: 0,
                gso_type: 0,
                hdr_len: 0,
                gso_size: 0,
                csum_start: 0,
                csum_offset: 0,
                num_buffers: 0, // unused on TX (spec 5.1.6.1)
            };
        }

        // VirtIO descriptor flags
        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        // Allocate two descriptors (chain_len=2: this header+data pair —
        // R24: passing the real chain length lets the limiter account net
        // TX chains correctly instead of the blk-shaped 3-desc default)
        let header_desc_idx = match queue.alloc_desc_chain(2) {
            Some(idx) => idx,
            None => {
                // R24 (R23-6 completion): the FIRST allocation failure leaked
                // hdr_ptr too — only the second branch was fixed in R23-6.
                unsafe { alloc::alloc::dealloc(hdr_ptr as *mut u8, hdr_layout); }
                return -5;  // EIO
            }
        };
        let data_desc_idx = match queue.alloc_desc_chain(2) {
            Some(idx) => idx,
            None => {
                // R23-6: hdr leaks on desc exhaustion (R22-6 pattern).
                unsafe { alloc::alloc::dealloc(hdr_ptr as *mut u8, hdr_layout); }
                return -5;  // EIO
            }
        };

        // Set packet header descriptor (use physical address for DMA)
        let hdr_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(hdr_ptr as u64)
        ).0;
        queue.set_desc(
            header_desc_idx,
            hdr_phys,
            core::mem::size_of::<VirtIONetHdr>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // Set data descriptor (use physical address for DMA)
        let data_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(skb.data as u64)
        ).0;
        queue.set_desc(
            data_desc_idx,
            data_phys,
            skb.len,
            0,  // Last descriptor
            0,
        );

        // Snapshot used.idx BEFORE submit: QEMU's iothread can complete the
        // TX before a post-submit get_used() runs, and a snapshot that
        // already includes our completion makes the wait below time out —
        // reporting a false EIO for every fast completion.
        let prev_used = queue.get_used();

        // Submit to available ring
        queue.submit(header_desc_idx);

        // Notify device
        queue.notify();

        // Wait for completion
        let new_used = queue.wait_for_completion(prev_used);

        if new_used == prev_used {
            // R8-M2 / R21-N2 discipline (same as virtio-blk): the TX chain
            // is STILL SUBMITTED — the device may be DMA-reading hdr_ptr and
            // the skb data right now. Late-drain the used ring with a long
            // bounded spin; on a true timeout LEAK both buffers instead of
            // freeing in-flight DMA targets.
            let used_ring = queue.used_ring_ptr();
            let mut late = false;
            for _ in 0..50_000_000u64 {
                // SAFETY: used_ring points to this queue's used ring; offset 2
                // is the idx field (u16) within the ring structure.
                let idx = unsafe {
                    core::ptr::read_volatile((used_ring as usize + 2) as *const u16)
                };
                if idx != prev_used {
                    late = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !late {
                let mut stats = self.stats.lock_irqsave();
                stats.tx_errors += 1;
                drop(stats);
                // hdr and skb deliberately NOT freed — device still owns them.
                // SkBuff's Drop releases its buffer, so forget() it to make
                // the leak explicit (R21-N2: integrity over a leak).
                core::mem::forget(skb);
                return -5;  // EIO
            }
            // Completed late — fall through to the normal cleanup.
        }

        // Free packet header
        // SAFETY: hdr_ptr was allocated with hdr_layout above and is still valid.
        unsafe {
            alloc::alloc::dealloc(hdr_ptr as *mut u8, hdr_layout);
        }

        // Update statistics
        let mut stats = self.stats.lock_irqsave();
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
        if !*self.initialized.lock_irqsave() {
            return None;
        }

        // Get RX queue (irqsafe: IRQ handler may access these)
        let mut queue_guard = self.rx_queue.lock_irqsave();
        let queue = queue_guard.as_mut()?;

        // Get last processed index
        let mut last_used = *self.rx_last_used.lock_irqsave();
        let current_used = queue.get_used();

        if last_used == current_used {
            return None; // No new packets
        }

        // Get completed descriptor from used ring
        let used_elem = queue.get_used_elem(last_used)?;

        // Update last_used
        last_used = last_used.wrapping_add(1);
        *self.rx_last_used.lock_irqsave() = last_used;

        let desc_idx = used_elem.id as u16;
        let desc = match queue.get_desc(desc_idx) {
            Some(d) => d,
            None => {
                // R24: bogus descriptor id from the device — the used entry
                // is consumed, so still account the buffer and refill
                // (was a bare `?` that lost the slot forever).
                drop(queue_guard);
                let stale = self.rx_buffers.lock_irqsave().pop();
                if let Some(addr) = stale {
                    self.dealloc_rx_buffer(addr);
                }
                self.refill_rx_buffers();
                return None;
            }
        };

        // desc.addr is the PHYSICAL (DMA) address programmed into the
        // descriptor; the kernel must touch the buffer through the linear
        // mapping. The old code used desc.addr directly as a kernel pointer
        // AND dealloc'd it — a physical address dereference that only
        // "worked" while nothing arrived (the RX rings were never armed).
        let buf_virt = crate::arch::riscv64::mm::phys_to_virt(
            crate::arch::riscv64::mm::PhysAddr::new(desc.addr)
        ).bits();

        // VirtIO-Net packet structure:
        // - 12 bytes VirtIONetHdr
        // - Followed by Ethernet frame data
        let total_len = used_elem.len as usize;
        if total_len <= core::mem::size_of::<VirtIONetHdr>() {
            // R24 (R14 MED "poll 早退泄漏 RX 缓冲"): every early return after
            // this point has consumed a used entry — the buffer must be
            // recycled or the buffer AND its descriptor are lost forever
            // (enough of them wedge RX at zero posted buffers).
            drop(queue_guard);
            self.recycle_rx_buffer(buf_virt);
            return None; // Data too short
        }

        let pkt_data_len = total_len - core::mem::size_of::<VirtIONetHdr>();
        // SAFETY: buf_virt is the linear-mapping address of the device-completed
        // RX buffer of total_len bytes; the buffer remains valid until after
        // this function returns.
        let hdr_and_data = unsafe {
            core::slice::from_raw_parts(buf_virt as *const u8, total_len)
        };

        // Skip VirtIO-Net header, keep only Ethernet frame
        let eth_data = &hdr_and_data[core::mem::size_of::<VirtIONetHdr>()..];

        // Create SkBuff
        let mut skb = match crate::net::buffer::alloc_skb(pkt_data_len as u32 + 64) {
            Some(skb) => skb,
            None => {
                // R24: recycle on allocation failure (was a leak).
                drop(queue_guard);
                self.recycle_rx_buffer(buf_virt);
                return None;
            }
        };
        if skb.skb_put_data(eth_data).is_err() {
            // R24: recycle and free the just-allocated skb (was a leak).
            skb.free();
            drop(queue_guard);
            self.recycle_rx_buffer(buf_virt);
            return None;
        }

        // Update statistics
        let mut stats = self.stats.lock_irqsave();
        stats.rx_packets += 1;
        stats.rx_bytes += pkt_data_len as u64;

        // Free old RX buffer and post a replacement
        drop(queue_guard);
        self.recycle_rx_buffer(buf_virt);

        Some(skb)
    }

    /// R24: recycle one RX buffer — dealloc the DMA buffer (same layout as
    /// refill_rx_buffers allocates) and post a replacement. Every poll()
    /// path that consumes a used-ring entry must run this; previously only
    /// the full-success path did, so any early return permanently lost the
    /// buffer and its descriptor.
    ///
    /// `addr` is the buffer's KERNEL VIRTUAL address (as tracked in
    /// rx_buffers), not the physical address stored in the descriptor.
    fn recycle_rx_buffer(&self, addr: u64) {
        self.dealloc_rx_buffer(addr);
        // Pop one entry from rx_buffers to reflect the freed buffer,
        // so refill_rx_buffers() knows to allocate a replacement.
        self.rx_buffers.lock_irqsave().pop();
        self.refill_rx_buffers();
    }

    /// Dealloc one RX buffer with the layout refill_rx_buffers() allocates:
    ///   buf_size = size_of::<VirtIONetHdr>() + mtu + 64, align = 64
    fn dealloc_rx_buffer(&self, addr: u64) {
        let buf_size = core::mem::size_of::<VirtIONetHdr>() + self.mtu as usize + 64;
        // SAFETY: Buffer was allocated with identical layout in refill_rx_buffers();
        // from_size_align cannot fail because buf_size and align are the same constants.
        unsafe {
            if let Ok(layout) = alloc::alloc::Layout::from_size_align(buf_size, 64) {
                alloc::alloc::dealloc(addr as *mut u8, layout);
            } else {
                crate::pr_err!("virtio_net: invalid RX dealloc layout buf_size={}", buf_size);
            }
        }
    }

    /// Refill RX buffers
    fn refill_rx_buffers(&self) {
        let mut queue_guard = self.rx_queue.lock_irqsave();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return,
        };

        let mut rx_buffers = self.rx_buffers.lock_irqsave();

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

            // SAFETY: Layout is valid (checked above); null check follows immediately.
            let buf_ptr = unsafe { alloc::alloc::alloc(layout) as *mut u8 };
            if buf_ptr.is_null() {
                continue;
            }

            // Allocate descriptor (single-descriptor chain — R24 chain
            // limiter fix: 1-desc RX buffers now fill the whole ring
            // instead of stopping at 2 under the blk-shaped *3 guard).
            let desc_idx = match queue.alloc_desc_chain(1) {
                Some(idx) => idx,
                None => {
                    // SAFETY: buf_ptr was just allocated with layout; no submit occurred.
                    unsafe { alloc::alloc::dealloc(buf_ptr, layout); }
                    continue;
                }
            };

            // Set descriptor — DMA needs physical address
            // SAFETY: buf_ptr is a valid kernel virtual address from alloc; virt_to_phys
            // converts to the corresponding physical address for DMA.
            let buf_phys = crate::arch::riscv64::mm::virt_to_phys(
                crate::arch::riscv64::mm::VirtAddr::new(buf_ptr as u64)
            ).0 as u64;
            // VIRTQ_DESC_F_WRITE = 2 means device can write
            queue.set_desc(desc_idx, buf_phys, buf_size as u32, 2, 0);

            // Record buffer address
            rx_buffers.push(buf_ptr as u64);

            // Submit to available ring
            queue.submit(desc_idx);
        }
    }

    /// Get statistics
    pub fn get_stats(&self) -> DeviceStats {
        *self.stats.lock_irqsave()
    }
}

/// VirtIO network device transmit function (for NetDevice calls)
fn virtio_net_xmit(skb: SkBuff) -> i32 {
    // Get global VirtIO network device
    // SAFETY: VIRTIO_NET is initialized before any network I/O can occur.
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
    // SAFETY: VIRTIO_NET is initialized before any network I/O can occur.
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
    // SAFETY: Called once during kernel init; VIRTIO_NET and VIRTIO_NET_DEVICE
    // are not accessed concurrently at this point.
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
    // SAFETY: VIRTIO_NET is initialized before any caller accesses it.
    unsafe { VIRTIO_NET.as_ref() }
}

/// Get VirtIO network device's NetDevice
pub fn get_net_device() -> Option<&'static mut NetDevice> {
    // SAFETY: VIRTIO_NET_DEVICE is initialized before any caller accesses it.
    unsafe { VIRTIO_NET_DEVICE.as_mut() }
}

/// Get VirtIO network device's base address
fn get_device_base_addr() -> Option<u64> {
    // SAFETY: VIRTIO_NET is initialized before any IRQ handler is registered.
    unsafe { VIRTIO_NET.as_ref().map(|dev| dev.base_addr) }
}

/// VirtIO-Net interrupt handler (top half)
///
/// Called when VirtIO-Net device generates interrupt.
/// Only acknowledges the interrupt and defers packet processing
/// to NetRx softirq bottom half.
/// Registered via request_irq. EOI is done by the IRQ framework.
pub fn interrupt_handler(_irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    // Get device base address
    let base_addr = match get_device_base_addr() {
        Some(addr) => addr,
        None => return crate::interrupt::IrqReturn::None,
    };

    // SAFETY: base_addr is from a valid, initialized VirtIO device;
    // MMIO register reads/writes at correct offsets per VirtIO spec.
    unsafe {
        // Read interrupt status (INTERRUPT_STATUS at 0x60)
        let irq_status_ptr = (base_addr + 0x60) as *const u32;
        let irq_status = core::ptr::read_volatile(irq_status_ptr);

        if irq_status != 0 {
            // Clear interrupt (INTERRUPT_ACK at 0x64)
            let irq_ack_ptr = (base_addr + 0x64) as *mut u32;
            core::ptr::write_volatile(irq_ack_ptr, irq_status);

            // Defer packet processing to NetRx softirq bottom half
            crate::interrupt::softirq::raise_softirq_irqoff(
                crate::interrupt::softirq::SoftirqIndex::NetRx as usize,
            );
            return crate::interrupt::IrqReturn::Handled;
        }
    }
    crate::interrupt::IrqReturn::None
}

/// NetRx softirq handler (bottom half).
///
/// Processes received network packets deferred from the interrupt handler.
/// Runs in softirq context (at `irq_exit` time or from ksoftirqd).
pub fn net_rx_softirq_handler(_vec: usize) {
    crate::net::ethernet::ethernet_poll();
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
