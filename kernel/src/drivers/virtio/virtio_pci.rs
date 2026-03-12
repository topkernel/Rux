//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO PCI transport layer
//!
//! Implements VirtIO device PCI transport (Modern VirtIO 1.0+)

use crate::drivers::pci::{PCIConfig, vendor, virtio_device, BARType};
use crate::drivers::virtio::queue;
use crate::drivers::virtio::offset;
use alloc::collections::btree_map::BTreeMap;
use alloc::vec::Vec;

/// VirtIO PCI Capability types
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtIOCapType {
    CommonCfg = 1,     // VIRTIO_PCI_CAP_COMMON_CFG
    NotifyCfg = 2,     // VIRTIO_PCI_CAP_NOTIFY_CFG
    IsrCfg = 3,        // VIRTIO_PCI_CAP_ISR_CFG
    DeviceCfg = 4,       // VIRTIO_PCI_CAP_DEVICE_CFG
    PciCfg = 5,        // VIRTIO_PCI_CAP_PCI_CFG
}

/// VirtIO PCI Capability structure
#[repr(C)]
#[derive(Debug)]
struct VirtioPCICap {
    cap_vndr: u8,   // Generic PCI field: PCI_CAP_ID_VNDR
    cap_next: u8,    // Generic PCI field: next ptr
    cap_len: u8,     // Generic PCI field: capability length
    cfg_type: u8,    // Identifies the structure (VirtIOCapType)
    bar: u8,         // Where to find it
    id: u8,          // Multiple capabilities of same type
    padding: [u8; 2], // Pad to full dword
    offset: u32,     // Offset within bar (little-endian)
    length: u32,     // Length of structure in bytes (little-endian)
}

/// VirtIO PCI Notify Capability structure (extended)
#[repr(C)]
#[derive(Debug)]
struct VirtioPCINotifyCap {
    cap: VirtioPCICap,
    notify_off_multiplier: u32,  // Queue notification offset multiplier
}

/// PCI Capability list pointer
const PCI_CAPABILITY_LIST: u8 = 0x34;
const PCI_CAP_ID_VNDR: u8 = 0x09;  // Vendor-specific capability

/// VirtIO device status bits
pub mod status {
    pub const ACKNOWLEDGE: u32 = 0x01;
    pub const DRIVER: u32 = 0x02;
    pub const FAILED: u32 = 0x80;
    pub const FEATURES_OK: u32 = 0x08;
    pub const DRIVER_OK: u32 = 0x04;
    pub const DEVICE_NEEDS_RESET: u32 = 0x40;
}

/// VirtIO PCI device
pub struct VirtIOPCI {
    /// PCI configuration space
    pub pci_config: PCIConfig,
    /// PCI slot number (used for IRQ calculation)
    pub pci_slot: u8,
    /// Common CFG BAR base address
    pub common_cfg_bar: u64,
    /// Common CFG BAR offset
    pub common_cfg_offset: u32,
    /// Device CFG BAR base address
    pub device_cfg_bar: u64,
    /// Device CFG BAR offset
    pub device_cfg_offset: u32,
    /// Notify CFG BAR base address
    pub notify_cfg_bar: u64,
    /// Notify CFG BAR offset
    pub notify_cfg_offset: u32,
    /// Notify offset multiplier
    pub notify_off_multiplier: u32,
    /// ISR CFG BAR base address (critical for interrupt status reading)
    pub isr_cfg_bar: u64,
    /// ISR CFG BAR offset
    pub isr_cfg_offset: u32,
    /// Device base address
    pub base_addr: u64,
}

impl VirtIOPCI {
    /// Find VirtIO PCI capability
    ///
    /// # Parameters
    /// - `cap_type`: Capability type to find
    ///
    /// # Returns
    /// Returns capability offset position, or 0 if not found
    fn find_virtio_capability(&self, cap_type: VirtIOCapType) -> Option<u8> {
        unsafe {
            // Start from capabilities list pointer
            let mut cap_ptr = self.pci_config.read_config_byte(PCI_CAPABILITY_LIST);
            let mut iterations = 0;
            const MAX_ITERATIONS: u8 = crate::config::VIRTIO_PCI_MAX_CAPABILITIES as u8;  // From config

            while cap_ptr != 0 && iterations < MAX_ITERATIONS {
                // Read capability ID
                let cap_id = self.pci_config.read_config_byte(cap_ptr);

                if cap_id == PCI_CAP_ID_VNDR {
                    // This is a vendor-specific capability, check type
                    let cfg_type = self.pci_config.read_config_byte(cap_ptr + 3);

                    if cfg_type == cap_type as u8 {
                        return Some(cap_ptr);
                    }
                }

                // Move to next capability
                let next_ptr = self.pci_config.read_config_byte(cap_ptr + 1);
                if next_ptr == cap_ptr {
                    // Loop detected, exit
                    crate::println!("virtio-pci: WARNING - capability loop detected at {}", cap_ptr);
                    break;
                }
                cap_ptr = next_ptr;
                iterations += 1;
            }

            if iterations >= MAX_ITERATIONS {
                crate::println!("virtio-pci: WARNING - too many capability iterations");
            }
        }

        None
    }

    /// Read VirtIO PCI capability info
    ///
    /// # Parameters
    /// - `cap_offset`: Capability offset in PCI configuration space
    ///
    /// # Returns
    /// (bar_index, bar_offset, length)
    fn read_virtio_cap(&self, cap_offset: u8) -> Option<(u8, u32, u32)> {
        unsafe {
            // Read capability fields
            let bar = self.pci_config.read_config_byte(cap_offset + 4);

            // Read offset and length (little-endian)
            let offset_lo = self.pci_config.read_config_byte(cap_offset + 8) as u32;
            let offset_hi = self.pci_config.read_config_byte(cap_offset + 9) as u32;
            let offset = offset_lo | (offset_hi << 8);

            let len_lo = self.pci_config.read_config_byte(cap_offset + 12) as u32;
            let len_hi = self.pci_config.read_config_byte(cap_offset + 13) as u32;
            let length = len_lo | (len_hi << 8);

            if bar >= 6 {
                // Reserved BAR value
                return None;
            }

            Some((bar, offset, length))
        }
    }
    /// Create new VirtIO PCI device
    ///
    /// # Parameters
    /// - `pci_base`: PCI configuration space base address (ECAM)
    pub fn new(pci_base: u64) -> Result<Self, &'static str> {
        let pci_config = PCIConfig::new(pci_base);

        // Calculate PCI slot number (for IRQ calculation)
        let pci_slot = ((pci_base - crate::drivers::pci::RISCV_PCIE_ECAM_BASE) / crate::drivers::pci::PCIE_ECAM_SIZE) as u8;

        // Verify vendor ID and device ID
        let vendor_id = pci_config.vendor_id();
        let device_id = pci_config.device_id();

        if vendor_id != vendor::RED_HAT {
            return Err("Not a VirtIO device (wrong vendor)");
        }

        match device_id {
            virtio_device::VIRTIO_BLK_MODERN => {
                // VirtIO block device (Modern VirtIO 1.0+)
            }
            virtio_device::VIRTIO_BLK => {
                // VirtIO block device (Legacy/Transitional)
                // Try using modern VirtIO 1.0 interface (transitional devices support both modes)
            }
            virtio_device::VIRTIO_NET => {
                // VirtIO network device
            }
            virtio_device::VIRTIO_GPU => {
                // VirtIO GPU device
            }
            _ => {
                if device_id != 0 {
                    return Err("Unknown VirtIO device");
                }
            }
        }

        // Enable bus master and memory space access
        pci_config.enable_bus_master();

        // Create temporary instance to use capability scanning methods
        let temp_device = Self {
            pci_config,
            pci_slot,  // Add pci_slot here
            common_cfg_bar: 0,
            common_cfg_offset: 0,
            device_cfg_bar: 0,
            device_cfg_offset: 0,
            notify_cfg_bar: 0,
            notify_cfg_offset: 0,
            notify_off_multiplier: 0,
            isr_cfg_bar: 0,
            isr_cfg_offset: 0,
            base_addr: 0,
        };

        // ========== Scan VirtIO PCI capabilities ==========
        // 1. Find Common CFG capability
        let (common_bar, common_offset, _) = match temp_device.find_virtio_capability(VirtIOCapType::CommonCfg) {
            Some(cap_offset) => {
                match temp_device.read_virtio_cap(cap_offset) {
                    Some(info) => info,
                    None => return Err("Failed to read Common CFG capability"),
                }
            }
            None => return Err("Common CFG capability not found (not a Modern VirtIO device)"),
        };

        // 2. Find Notify CFG capability
        let (notify_bar, notify_offset, _) = match temp_device.find_virtio_capability(VirtIOCapType::NotifyCfg) {
            Some(cap_offset) => {
                match temp_device.read_virtio_cap(cap_offset) {
                    Some(info) => info,
                    None => return Err("Failed to read Notify CFG capability"),
                }
            }
            None => return Err("Notify CFG capability not found"),
        };

        // 2.5. Find ISR CFG capability (required for interrupt status)
        let (isr_bar, isr_offset, _) = match temp_device.find_virtio_capability(VirtIOCapType::IsrCfg) {
            Some(cap_offset) => {
                match temp_device.read_virtio_cap(cap_offset) {
                    Some(info) => info,
                    None => return Err("Failed to read ISR CFG capability"),
                }
            }
            None => return Err("ISR CFG capability not found"),
        };

        // 3. Find Device CFG capability (optional)
        let (device_bar, device_offset, _) = temp_device.find_virtio_capability(VirtIOCapType::DeviceCfg)
            .and_then(|cap_offset| temp_device.read_virtio_cap(cap_offset))
            .unwrap_or((0xFF, 0, 0));  // 0xFF indicates not present

        // ========== PCI BAR address assignment ==========
        // VirtIO PCI devices require kernel to assign BAR addresses
        // Use fixed MMIO region: 0x40000000 - 0x50000000 (256MB)
        const PCI_MMIO_BASE: u64 = 0x40000000;

        // Use global static variable to track MMIO offset, avoiding address conflicts between devices
        use core::sync::atomic::{AtomicU64, Ordering};
        static MMIO_OFFSET: AtomicU64 = AtomicU64::new(0);

        let mut mmio_offset = MMIO_OFFSET.load(Ordering::SeqCst);

        // Collect BAR indices to assign (deduplicated)
        let mut bars_to_assign = alloc::vec::Vec::new();
        bars_to_assign.push(common_bar);
        if notify_bar != common_bar {
            bars_to_assign.push(notify_bar);
        }
        if isr_bar != common_bar && isr_bar != notify_bar {
            bars_to_assign.push(isr_bar);
        }
        if device_bar != 0xFF && device_bar != common_bar && device_bar != notify_bar && device_bar != isr_bar {
            bars_to_assign.push(device_bar);
        }

        // Store assigned BAR info
        let mut assigned_bars = alloc::collections::btree_map::BTreeMap::new();

        // Assign address for each BAR
        for &bar_idx in &bars_to_assign {
            // Probe BAR size
            let bar_size = pci_config.probe_bar_size(bar_idx);

            // Calculate aligned address
            let aligned_addr = if mmio_offset % bar_size != 0 {
                ((mmio_offset / bar_size) + 1) * bar_size
            } else {
                mmio_offset
            };

            let bar_addr = PCI_MMIO_BASE + aligned_addr;

            // Write BAR address and store returned PCIBAR object
            match pci_config.assign_bar(bar_idx, bar_addr) {
                Ok(bar_obj) => {
                    mmio_offset = aligned_addr + bar_size;
                    assigned_bars.insert(bar_idx, bar_obj);
                }
                Err(e) => {
                    crate::println!("virtio-pci: ERROR - Failed to assign BAR{}: {}", bar_idx, e);
                    return Err("Failed to assign PCI BAR");
                }
            }
        }

        // Update global MMIO offset, avoiding next device address conflict
        MMIO_OFFSET.store(mmio_offset, Ordering::SeqCst);

        // ========== Use assigned BAR info ==========
        let common_bar_obj = assigned_bars.get(&common_bar)
            .ok_or("Common CFG BAR not assigned")?;
        if common_bar_obj.bar_type != BARType::MemoryMapped {
            return Err("Common CFG BAR is not memory mapped");
        }
        let common_cfg_bar = common_bar_obj.base_addr;

        let notify_bar_obj = assigned_bars.get(&notify_bar)
            .ok_or("Notify CFG BAR not assigned")?;
        if notify_bar_obj.bar_type != BARType::MemoryMapped {
            return Err("Notify CFG BAR is not memory mapped");
        }
        let notify_cfg_bar = notify_bar_obj.base_addr;

        let device_cfg_bar = if device_bar != 0xFF {
            match assigned_bars.get(&device_bar) {
                Some(bar_obj) if bar_obj.bar_type == BARType::MemoryMapped => bar_obj.base_addr,
                _ => 0,
            }
        } else {
            0
        };

        // Extract ISR CFG BAR (critical for interrupt status reading)
        let isr_cfg_bar = match assigned_bars.get(&isr_bar) {
            Some(bar_obj) if bar_obj.bar_type == BARType::MemoryMapped => bar_obj.base_addr,
            _ => return Err("ISR CFG BAR not assigned or not memory mapped"),
        };

        // ========== Read notify_off_multiplier ==========
        // From Notify CFG capability offset 16 (notify_off_multiplier field)
        // notify_off_multiplier is part of Notify CFG capability structure, located in PCI configuration space
        let notify_off_multiplier = match temp_device.find_virtio_capability(VirtIOCapType::NotifyCfg) {
            Some(cap_offset) => {
                // notify_off_multiplier at capability structure offset 16
                pci_config.read_config_dword(cap_offset + 16)
            }
            None => 0,
        };

        Ok(Self {
            pci_config,
            pci_slot,
            common_cfg_bar: common_cfg_bar + common_offset as u64,
            common_cfg_offset: common_offset,
            device_cfg_bar: device_cfg_bar + device_offset as u64,
            device_cfg_offset: device_offset,
            // Critical fix: notify_cfg_bar should be pure BAR base address, without offset
            // get_notify_addr will add queue_index * multiplier when used
            notify_cfg_bar: notify_cfg_bar,
            notify_cfg_offset: notify_offset,
            notify_off_multiplier,
            isr_cfg_bar: isr_cfg_bar + isr_offset as u64,
            isr_cfg_offset: isr_offset,
            base_addr: common_cfg_bar + common_offset as u64,  // Use Common CFG as primary access address
        })
    }

    /// Reset device
    pub fn reset_device(&self) {
        unsafe {
            let status_ptr = (self.common_cfg_bar + 0x14) as *mut u32;
            core::ptr::write_volatile(status_ptr, 0);
        }
    }

    /// Set device status
    pub fn set_status(&self, status: u32) {
        unsafe {
            let status_ptr = (self.common_cfg_bar + 0x14) as *mut u32;
            core::ptr::write_volatile(status_ptr, status);
        }
    }

    /// Get device status
    pub fn get_status(&self) -> u32 {
        unsafe {
            let status_ptr = (self.common_cfg_bar + 0x14) as *const u32;
            core::ptr::read_volatile(status_ptr)
        }
    }

    /// Read device features
    ///
    /// VirtIO 1.0 PCI specification:
    /// - 0x00: device_feature_select (write-only) - select feature bit set
    /// - 0x04: device_feature (read-only) - actual feature bits
    pub fn read_device_features(&self) -> u32 {
        unsafe {
            // First write 0 to device_feature_select to select feature bits 0-31
            let select_ptr = (self.common_cfg_bar + 0x00) as *mut u32;
            core::ptr::write_volatile(select_ptr, 0u32);

            // Then read actual feature bits from device_feature
            let features_ptr = (self.common_cfg_bar + 0x04) as *const u32;
            core::ptr::read_volatile(features_ptr)
        }
    }

    /// Write driver features
    ///
    /// VirtIO 1.0 PCI specification:
    /// - 0x08: driver_feature_select (write-only) - select feature bit set
    /// - 0x0C: driver_feature (write-only) - actual feature bits
    pub fn write_driver_features(&self, features: u32) {
        unsafe {
            // First write 0 to driver_feature_select to select feature bits 0-31
            let select_ptr = (self.common_cfg_bar + 0x08) as *mut u32;
            core::ptr::write_volatile(select_ptr, 0u32);

            // Then write to driver_feature
            let features_ptr = (self.common_cfg_bar + 0x0C) as *mut u32;
            core::ptr::write_volatile(features_ptr, features);
        }
    }

    /// Setup queue
    pub fn setup_queue(&self, queue_index: u16, virt_queue: &queue::VirtQueue) -> Result<(), &'static str> {
        // Select queue
        unsafe {
            let queue_select_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_SELECT as u64) as *mut u16;
            core::ptr::write_volatile(queue_select_ptr, queue_index);
        }

        // Get queue size
        unsafe {
            let queue_size_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_SIZE as u64) as *const u16;
            let queue_max_size = core::ptr::read_volatile(queue_size_ptr);

            if queue_max_size == 0 {
                return Err("Queue not available");
            }

            // Use maximum queue size supported by device
            let _queue_size = queue_max_size;
        }

        // Get descriptor table, available ring, used ring physical addresses
        let desc_addr = virt_queue.get_desc_addr();
        let avail_addr = virt_queue.get_avail_addr();
        let used_addr = virt_queue.get_used_addr();

        // Convert to physical addresses
        #[cfg(feature = "riscv64")]
        let desc_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(desc_addr)
        ).0;
        #[cfg(feature = "riscv64")]
        let avail_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(avail_addr)
        ).0;
        #[cfg(feature = "riscv64")]
        let used_phys = crate::arch::riscv64::mm::virt_to_phys(
            crate::arch::riscv64::mm::VirtAddr::new(used_addr)
        ).0;

        #[cfg(not(feature = "riscv64"))]
        let desc_phys = desc_addr;
        #[cfg(not(feature = "riscv64"))]
        let avail_phys = avail_addr;
        #[cfg(not(feature = "riscv64"))]
        let used_phys = used_addr;

        // Write descriptor table address (64-bit)
        unsafe {
            let desc_lo_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DESC_LO as u64) as *mut u32;
            let desc_hi_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DESC_HI as u64) as *mut u32;
            core::ptr::write_volatile(desc_lo_ptr, (desc_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile(desc_hi_ptr, (desc_phys >> 32) as u32);
        }

        // Write available ring address (64-bit)
        unsafe {
            let driver_lo_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DRIVER_LO as u64) as *mut u32;
            let driver_hi_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DRIVER_HI as u64) as *mut u32;
            core::ptr::write_volatile(driver_lo_ptr, (avail_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile(driver_hi_ptr, (avail_phys >> 32) as u32);
        }

        // Write used ring address (64-bit)
        unsafe {
            let device_lo_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DEVICE_LO as u64) as *mut u32;
            let device_hi_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_DEVICE_HI as u64) as *mut u32;
            core::ptr::write_volatile(device_lo_ptr, (used_phys & 0xFFFFFFFF) as u32);
            core::ptr::write_volatile(device_hi_ptr, (used_phys >> 32) as u32);
        }

        // Enable queue
        unsafe {
            let queue_enable_ptr = (self.common_cfg_bar + offset::COMMON_CFG_QUEUE_ENABLE as u64) as *mut u16;
            core::ptr::write_volatile(queue_enable_ptr, 1);
        }

        Ok(())
    }

    /// Get notification address
    pub fn get_notify_addr(&self, queue_index: u16) -> u64 {
        // Critical fix: per VirtIO 1.0 specification 4.1.4.4,
        // notification address = notify_offset + 2 * (queue_index * notify_off_multiplier)
        // i.e.: multiply notify_off_multiplier by 2 (since it's in 16-bit units, need to multiply by 2 to convert to bytes)
        let queue_offset = (queue_index as u64 * self.notify_off_multiplier as u64) * 2;
        self.notify_cfg_bar + self.notify_cfg_offset as u64 + queue_offset
    }

    /// Notify device
    pub fn notify(&self, queue_index: u16) {
        let notify_addr = self.get_notify_addr(queue_index);
        unsafe {
            let notify_ptr = notify_addr as *mut u16;
            // VirtIO 1.0 specification: write queue index (16-bit) to notification register
            core::ptr::write_volatile(notify_ptr, queue_index);
        }
    }

    /// Enable device interrupt
    ///
    /// RISC-V QEMU virt platform PCIe IRQ routing:
    /// PCIE_IRQ base = 32, total 4 IRQs (32-35)
    /// Formula: IRQ = 32 + ((INT_PIN + PCI_slot) % 4)
    pub fn enable_device_interrupt(&self) {
        // Read INT_PIN to determine IRQ offset
        let int_pin = self.pci_config.read_config_byte(0x3D);

        // PCIe IRQ calculation formula (QEMU RISC-V virt platform)
        // Note: INT_PIN starts from 1 (INTA=1, INTB=2, INTC=3, INTD=4)
        let irq = 32 + ((int_pin as u32 + self.pci_slot as u32) % 4);

        // Enable IRQ (on current boot hart)
        #[cfg(feature = "riscv64")]
        {
            let boot_hart = crate::arch::riscv64::smp::cpu_id();
            crate::drivers::intc::plic::enable_interrupt(boot_hart, irq as usize);
        }
    }

    /// Set queue MSI-X vector
    ///
    /// VirtIO 1.0 specification requires setting MSI-X vector before queue_enable
    /// This tells the device which MSI-X vector to use for queue completion interrupts
    ///
    /// # Parameters
    /// - `queue_index`: Queue index (0 for first queue)
    /// - `vector`: MSI-X vector number (0 means don't use MSI-X, use legacy INTx)
    pub fn set_queue_vector(&self, queue_index: u16, vector: u16) {
        // VirtIO Common CFG offset 0x1C: queue_msix_vector
        unsafe {
            let vector_ptr = (self.common_cfg_bar + 0x1C) as *mut u16;
            core::ptr::write_volatile(vector_ptr, vector);
        }
        let _ = queue_index; // Avoid unused warning
    }

    /// Read data from block device
    ///
    /// # Parameters
    /// - `sector`: Starting sector number
    /// - `buf`: Data buffer
    ///
    /// # Returns
    /// Returns bytes read on success, error code on failure
    pub fn read_block(&self, sector: u64, buf: &mut [u8]) -> Result<usize, &'static str> {
        use crate::drivers::virtio::queue::{VirtIOBlkReqHeader, VirtIOBlkResp, req_type};
        use crate::arch::riscv64::mm::VirtAddr;

        // Allocate three descriptors
        let virt_queue_opt: Option<queue::VirtQueue> = queue::VirtQueue::new(8u16,
            0,  // queue_index
            self.notify_cfg_bar + offset::QUEUE_NOTIFY as u64,
            self.common_cfg_bar + offset::INTERRUPT_STATUS as u64,
            self.common_cfg_bar + offset::INTERRUPT_ACK as u64);
        let mut virt_queue = match virt_queue_opt {
            None => return Err("Failed to create VirtQueue"),
            Some(q) => q,
        };

        let header_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err("Failed to alloc header descriptor"),
        };
        let data_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err("Failed to alloc data descriptor"),
        };
        let resp_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err("Failed to alloc response descriptor"),
        };

        // Construct VirtIO block request header
        let req_header = VirtIOBlkReqHeader {
            type_: req_type::VIRTIO_BLK_T_IN,
            reserved: 0,
            sector,
        };

        // Allocate request header buffer
        let header_layout = alloc::alloc::Layout::new::<VirtIOBlkReqHeader>();
        let header_ptr: *mut VirtIOBlkReqHeader;
        unsafe {
            header_ptr = alloc::alloc::alloc(header_layout) as *mut VirtIOBlkReqHeader;
        }
        if header_ptr.is_null() {
            return Err("Failed to allocate header");
        }
        unsafe {
            *header_ptr = req_header;
        }

        // Allocate response buffer
        let resp_layout = alloc::alloc::Layout::new::<VirtIOBlkResp>();
        let resp_ptr: *mut VirtIOBlkResp;
        unsafe {
            resp_ptr = alloc::alloc::alloc(resp_layout) as *mut VirtIOBlkResp;
        }
        if resp_ptr.is_null() {
            unsafe {
                alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            }
            return Err("Failed to allocate response");
        }
        unsafe {
            (*resp_ptr).status = 0xFF;  // Initialize to invalid status
        }

        // VirtIO descriptor flags
        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        // Convert virtual addresses to physical addresses
        #[cfg(feature = "riscv64")]
        let header_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
            VirtAddr::new(header_ptr as u64)
        ).0;
        #[cfg(feature = "riscv64")]
        let resp_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
            VirtAddr::new(resp_ptr as u64)
        ).0;

        // Set request header descriptor
        virt_queue.set_desc(
            header_desc_idx,
            header_phys_addr,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // Set data buffer descriptor (device writes)
        // For PCI VirtIO, we need to ensure buffer is accessible in physical memory
        #[cfg(feature = "riscv64")]
        let data_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
            VirtAddr::new(buf.as_ptr() as u64)
        ).0;
        #[cfg(not(feature = "riscv64"))]
        let data_phys_addr = buf.as_ptr() as u64;

        virt_queue.set_desc(
            data_desc_idx,
            data_phys_addr,
            buf.len() as u32,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            resp_desc_idx,
        );

        // Set response descriptor
        virt_queue.set_desc(
            resp_desc_idx,
            resp_phys_addr,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            VIRTQ_DESC_F_WRITE,
            0,
        );

        // Submit to available ring
        virt_queue.submit(header_desc_idx);

        // Notify device
        virt_queue.notify();

        // Wait for completion
        let prev_used = virt_queue.get_used();
        let new_used = virt_queue.wait_for_completion(prev_used);

        if new_used == prev_used {
            // Request failed, device did not update used ring
            unsafe {
                alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
                alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
            }
            return Err("VirtIO request timeout");
        }

        // Read response status
        let status = unsafe { *resp_ptr };

        // Cleanup buffers
        unsafe {
            alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
        }

        match status.status {
            crate::drivers::virtio::queue::status::VIRTIO_BLK_S_OK => Ok(buf.len()),
            _ => Err("VirtIO block I/O error"),
        }
    }
}

/// Read block device using configured VirtQueue
///
/// # Parameters
/// - `pci_dev`: VirtIO PCI device
/// - `sector`: Starting sector number
/// - `buf`: Data buffer
///
/// # Returns
/// Returns bytes read on success, error code on failure
pub fn read_block_using_configured_queue(
    pci_dev: &VirtIOPCI,
    sector: u64,
    buf: &mut [u8]
) -> Result<usize, &'static str> {
    // Add retry mechanism to resolve VirtIO block device random timeout issues
    const MAX_RETRIES: usize = 5;
    let mut retries = 0;

    loop {
        match read_block_once(pci_dev, sector, buf) {
            Ok(size) => return Ok(size),
            Err(e) => {
                retries += 1;
                if retries >= MAX_RETRIES {
                    return Err(e);
                }
                // Short delay before retry
                for _ in 0..10000 {
                    core::hint::spin_loop();
                }
            }
        }
    }
}

/// Single read attempt
fn read_block_once(
    pci_dev: &VirtIOPCI,
    sector: u64,
    buf: &mut [u8]
) -> Result<usize, &'static str> {
    use crate::drivers::virtio::queue::{VirtIOBlkReqHeader, VirtIOBlkResp, req_type};

    // Get configured VirtQueue (mutable reference)
    let virt_queue = match crate::drivers::virtio::get_pci_device_queue_mut() {
        Some(q) => q,
        None => return Err("No configured VirtQueue found"),
    };

    // Reset descriptor allocator to reuse descriptors
    virt_queue.reset_desc_allocator();

    // Allocate three descriptors
    let header_desc_idx = match virt_queue.alloc_desc() {
        Some(idx) => idx,
        None => return Err("Failed to alloc header descriptor"),
    };
    let data_desc_idx = match virt_queue.alloc_desc() {
        Some(idx) => idx,
        None => return Err("Failed to alloc data descriptor"),
    };
    let resp_desc_idx = match virt_queue.alloc_desc() {
        Some(idx) => idx,
        None => return Err("Failed to alloc response descriptor"),
    };

    // Construct VirtIO block request header
    let req_header = VirtIOBlkReqHeader {
        type_: req_type::VIRTIO_BLK_T_IN,
        reserved: 0,
        sector,
    };

    // Allocate request header buffer
    let header_layout = alloc::alloc::Layout::new::<VirtIOBlkReqHeader>();
    let header_ptr: *mut VirtIOBlkReqHeader;
    unsafe {
        header_ptr = alloc::alloc::alloc(header_layout) as *mut VirtIOBlkReqHeader;
    }
    if header_ptr.is_null() {
        return Err("Failed to allocate header");
    }
    unsafe {
        *header_ptr = req_header;
    }

    // Allocate response buffer
    let resp_layout = alloc::alloc::Layout::new::<VirtIOBlkResp>();
    let resp_ptr: *mut VirtIOBlkResp;
    unsafe {
        resp_ptr = alloc::alloc::alloc(resp_layout) as *mut VirtIOBlkResp;
    }
    if resp_ptr.is_null() {
        unsafe {
            alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
        }
        return Err("Failed to allocate response");
    }
    unsafe {
        (*resp_ptr).status = 0xFF;  // Initialize to invalid status
    }

    // VirtIO descriptor flags
    const VIRTQ_DESC_F_NEXT: u16 = 1;
    const VIRTQ_DESC_F_WRITE: u16 = 2;

    // Convert virtual addresses to physical addresses
    #[cfg(feature = "riscv64")]
    let header_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
        crate::arch::riscv64::mm::VirtAddr::new(header_ptr as u64)
    ).0;
    #[cfg(feature = "riscv64")]
    let resp_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
        crate::arch::riscv64::mm::VirtAddr::new(resp_ptr as u64)
    ).0;

    // For PCI VirtIO, we need to ensure buffer is accessible in physical memory
    #[cfg(feature = "riscv64")]
    let data_phys_addr = crate::arch::riscv64::mm::virt_to_phys(
        crate::arch::riscv64::mm::VirtAddr::new(buf.as_ptr() as u64)
    ).0;
    #[cfg(not(feature = "riscv64"))]
    let data_phys_addr = buf.as_ptr() as u64;

    // Set request header descriptor
    virt_queue.set_desc(
        header_desc_idx,
        header_phys_addr,
        core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
        VIRTQ_DESC_F_NEXT,
        data_desc_idx,
    );

    // Set data buffer descriptor (device writes)
    virt_queue.set_desc(
        data_desc_idx,
        data_phys_addr,
        buf.len() as u32,
        VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
        resp_desc_idx,
    );

    // Set response descriptor
    virt_queue.set_desc(
        resp_desc_idx,
        resp_phys_addr,
        core::mem::size_of::<VirtIOBlkResp>() as u32,
        VIRTQ_DESC_F_WRITE,
        0,
    );

    // Get current expected value (used.idx expected value before submit)
    let prev_expected = crate::drivers::virtio::get_expected_used_idx();

    // Submit to available ring (submit internally calls notify() and adds delay)
    virt_queue.submit(header_desc_idx);

    // Increment expected used.idx (track our expected completion count)
    crate::drivers::virtio::increment_expected_used_idx();

    // Wait for completion - wait for used.idx to reach expected value
    let new_used = virt_queue.wait_for_completion(prev_expected);

    if new_used == prev_expected {
        // Request failed, device did not update used ring
        unsafe {
            alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
            alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
        }
        return Err("VirtIO request timeout");
    }

    // Read response status
    let status = unsafe { *resp_ptr };

    // Cleanup buffers
    unsafe {
        alloc::alloc::dealloc(header_ptr as *mut u8, header_layout);
        alloc::alloc::dealloc(resp_ptr as *mut u8, resp_layout);
    }

    match status.status {
        crate::drivers::virtio::queue::status::VIRTIO_BLK_S_OK => Ok(buf.len()),
        _ => Err("VirtIO block I/O error"),
    }
}
