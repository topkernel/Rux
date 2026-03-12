//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO device probing
//!
//! Used to probe and initialize VirtIO devices

use crate::println;
use crate::config::ENABLE_VIRTIO_NET_PROBE;

/// VirtIO device IDs
///
/// Corresponds to device types in VirtIO specification
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtIODeviceId {
    /// Network device
    VirtioNet = 1,
    /// Block device
    VirtioBlk = 2,
    /// Console
    VirtioConsole = 3,
    /// Entropy
    VirtioRng = 4,
    /// Balloon device
    VirtioBalloon = 5,
    /// I/O memory
    VirtioScsi = 8,
    /// GPU
    VirtioGpu = 16,
}

/// VirtIO device MMIO base addresses
///
/// VirtIO device address range for QEMU virt platform
/// Uses identity mapping: VIRTIO_MMIO_BASE near 0x10000000
const VIRTIO_MMIO_BASE: u64 = 0x10001000;
const VIRTIO_MMIO_SIZE: u64 = 0x1000;

/// Number of VirtIO devices
const VIRTIO_MAX_DEVICES: usize = 8;

/// Probe all VirtIO devices
///
/// # Returns
/// Number of devices found
///
/// # Notes
/// Scans all 8 VirtIO device slots
pub fn virtio_probe_devices() -> usize {
    let mut device_count = 0;

    // Scan all VirtIO device slots
    for device_index in 0..VIRTIO_MAX_DEVICES {
        let base_addr = VIRTIO_MMIO_BASE + (device_index as u64 * VIRTIO_MMIO_SIZE);

        // Quick read magic number
        let magic = unsafe {
            let magic_ptr = base_addr as *const u32;
            core::ptr::read_volatile(magic_ptr)
        };

        // Check magic number ("virt" = 0x74726976)
        if magic == 0x74726976 {
            // Found VirtIO device, read more info
            let (version, device_id, _vendor, _device_features) = unsafe {
                let version_ptr = (base_addr + 4) as *const u32;
                let device_id_ptr = (base_addr + 8) as *const u32;
                let vendor_ptr = (base_addr + 12) as *const u32;
                let features_ptr = (base_addr + 16) as *const u32;
                (
                    core::ptr::read_volatile(version_ptr),
                    core::ptr::read_volatile(device_id_ptr),
                    core::ptr::read_volatile(vendor_ptr),
                    core::ptr::read_volatile(features_ptr),
                )
            };

            // Check version
            if version == 1 || version == 2 {
                // Identify device type and initialize
                match device_id {
                    1 => {
                        if init_virtio_net(base_addr).is_ok() {
                            device_count += 1;
                        }
                    }
                    2 => {
                        if init_virtio_blk(base_addr).is_ok() {
                            device_count += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    device_count
}

/// Initialize VirtIO-Net device
///
/// # Parameters
/// - `base_addr`: Device MMIO base address
///
/// # Returns
/// Ok(()) on success, Err(&str) on failure
fn init_virtio_net(base_addr: u64) -> Result<(), &'static str> {
    crate::drivers::net::virtio_net::init(base_addr)?;
    // Enable device interrupt
    crate::drivers::net::virtio_net::enable_device_interrupt(base_addr);
    Ok(())
}

/// Initialize VirtIO-Blk device
///
/// # Parameters
/// - `base_addr`: Device MMIO base address
///
/// # Returns
/// Ok(()) on success, Err(&str) on failure
fn init_virtio_blk(base_addr: u64) -> Result<(), &'static str> {
    crate::drivers::virtio::init(base_addr)?;
    // Enable device interrupt
    crate::drivers::virtio::enable_device_interrupt(base_addr);
    Ok(())
}

/// Initialize loopback network device
///
/// # Returns
/// true on success, false on failure
///
/// # Notes
/// Loopback device is always available as a fallback network device
fn init_loopback_device() -> bool {
    crate::drivers::net::loopback::loopback_init().is_some()
}

/// Initialize all network devices
///
/// # Notes
/// Initializes in order:
/// 1. Loopback device (always available)
/// 2. VirtIO-Net device (if present)
///
/// # Returns
/// Number of initialized devices
pub fn init_network_devices() -> usize {
    let mut device_count = 0;

    // 1. Initialize loopback device (always available)
    if init_loopback_device() {
        device_count += 1;
    }

    // 2. VirtIO device probing (controlled by menuconfig)
    if ENABLE_VIRTIO_NET_PROBE {
        let virtio_count = virtio_probe_devices();
        device_count += virtio_count;
    }

    device_count
}

/// Initialize all block devices
///
/// # Notes
/// Probes and initializes VirtIO-Blk devices
///
/// # Returns
/// Number of initialized devices
pub fn init_block_devices() -> usize {
    let mut device_count = 0;

    // Scan all VirtIO device slots
    for device_index in 0..VIRTIO_MAX_DEVICES {
        let base_addr = VIRTIO_MMIO_BASE + (device_index as u64 * VIRTIO_MMIO_SIZE);

        // Quick read magic number
        let magic = unsafe {
            let magic_ptr = base_addr as *const u32;
            core::ptr::read_volatile(magic_ptr)
        };

        // Check magic number ("virt" = 0x74726976)
        if magic == 0x74726976 {
            // Read device ID
            let device_id = unsafe {
                let device_id_ptr = (base_addr + 8) as *const u32;
                core::ptr::read_volatile(device_id_ptr)
            };

            // Check if block device
            if device_id == 2 {
                if init_virtio_blk(base_addr).is_ok() {
                    device_count += 1;
                }
            }
        }
    }

    device_count
}

/// Initialize PCI block devices
///
/// # Notes
/// Probes and initializes VirtIO-Blk devices via PCI bus
///
/// # Returns
/// Number of initialized devices
pub fn init_pci_block_devices() -> usize {
    let mut device_count = 0;

    // Scan PCIe bus (QEMU virt platform)
    const MAX_DEVICES: u8 = 32;

    for device in 0..MAX_DEVICES {
        let ecam_addr = crate::drivers::pci::RISCV_PCIE_ECAM_BASE + (device as u64 * crate::drivers::pci::PCIE_ECAM_SIZE);
        let config = crate::drivers::pci::PCIConfig::new(ecam_addr);

        let vendor_id = config.vendor_id();

        // Skip empty devices
        if vendor_id == 0xFFFF {
            continue;
        }

        let device_id = config.device_id();

        // Check if VirtIO block device
        if vendor_id == crate::drivers::pci::vendor::RED_HAT &&
           (device_id == crate::drivers::pci::virtio_device::VIRTIO_BLK ||
            device_id == crate::drivers::pci::virtio_device::VIRTIO_BLK_MODERN) {

            match crate::drivers::virtio::virtio_pci::VirtIOPCI::new(ecam_addr) {
                Ok(mut virtio_dev) => {
                    // Reset device
                    virtio_dev.reset_device();

                    // Wait for device reset to complete (status becomes 0)
                    let mut reset_timeout = 100000;
                    while virtio_dev.get_status() != 0 && reset_timeout > 0 {
                        core::hint::spin_loop();
                        reset_timeout -= 1;
                    }

                    // Set status to ACKNOWLEDGE | DRIVER
                    virtio_dev.set_status(crate::drivers::virtio::offset::status::ACKNOWLEDGE | crate::drivers::virtio::offset::status::DRIVER);

                    // Read device features
                    let features = virtio_dev.read_device_features();

                    // Write driver features
                    virtio_dev.write_driver_features(features);

                    // Set FEATURES_OK
                    virtio_dev.set_status(
                        crate::drivers::virtio::offset::status::ACKNOWLEDGE |
                        crate::drivers::virtio::offset::status::DRIVER |
                        crate::drivers::virtio::offset::status::FEATURES_OK
                    );

                    // Verify FEATURES_OK was accepted by device
                    let status_after_features = virtio_dev.get_status();
                    if status_after_features & crate::drivers::virtio::offset::status::FEATURES_OK == 0 {
                        continue;
                    }

                    // Select queue 0 and read queue size
                    unsafe {
                        let queue_select_ptr = (virtio_dev.common_cfg_bar + crate::drivers::virtio::offset::COMMON_CFG_QUEUE_SELECT as u64) as *mut u16;
                        core::ptr::write_volatile(queue_select_ptr, 0u16);
                    }

                    let queue_max = unsafe {
                        let queue_size_max_ptr = (virtio_dev.common_cfg_bar + crate::drivers::virtio::offset::COMMON_CFG_QUEUE_SIZE as u64) as *const u16;
                        core::ptr::read_volatile(queue_size_max_ptr)
                    };

                    // Create VirtQueue
                    let dummy_isr_addr = virtio_dev.common_cfg_bar;
                    match crate::drivers::virtio::queue::VirtQueue::new(queue_max,
                        0,  // queue_index
                        virtio_dev.get_notify_addr(0),
                        dummy_isr_addr,
                        dummy_isr_addr) {
                        None => {}
                        Some(virt_queue) => {
                            match virtio_dev.setup_queue(0, &virt_queue) {
                                Ok(()) => {
                                    // Store configured VirtQueue to global storage
                                    crate::drivers::virtio::set_pci_device_queue(virt_queue);

                                    // Enable device interrupt
                                    virtio_dev.enable_device_interrupt();

                                    // Set DRIVER_OK
                                    virtio_dev.set_status(
                                        crate::drivers::virtio::offset::status::ACKNOWLEDGE |
                                        crate::drivers::virtio::offset::status::DRIVER |
                                        crate::drivers::virtio::offset::status::FEATURES_OK |
                                        crate::drivers::virtio::offset::status::DRIVER_OK
                                    );

                                    // Register PCI VirtIO device to global storage
                                    crate::drivers::virtio::register_pci_device(virtio_dev);

                                    // Register GenDisk wrapper (so ext4 driver can access)
                                    crate::drivers::virtio::register_pci_gen_disk();

                                    device_count += 1;
                                }
                                Err(_) => {}
                            }
                        }
                    }
                }
                Err(_) => {}
            }
        }
    }

    device_count
}
