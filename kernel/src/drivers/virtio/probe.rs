//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO device probing
//!
//! Used to probe and initialize VirtIO devices

use crate::println;
use crate::config::ENABLE_VIRTIO_NET_PROBE;
use crate::sync::spinlock::Spinlock;
use alloc::vec::Vec;

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

/// Number of VirtIO devices - from config
const VIRTIO_MAX_DEVICES: usize = crate::config::VIRTIO_MAX_DEVICES;

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
                        match init_virtio_net(base_addr) {
                            Ok(()) => device_count += 1,
                            Err(e) => crate::pr_err!("virtio-net: init failed at 0x{:x}: {}", base_addr, e),
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
    // U3: registration-time uevent — notify udev/mdev the netdev exists.
    // virtio_net::init names the device "eth0" (fixed, single-function).
    crate::fs::sysfs::netdev_uevent("eth0", "add");
    Ok(())
}

/// Initialize VirtIO-Blk device
///
/// # Parameters
/// - `base_addr`: Device MMIO base address
///
/// # Returns
/// Ok(()) on success, Err(&'static str) on failure
fn init_virtio_blk(base_addr: u64) -> Result<(), &'static str> {
    crate::drivers::virtio::init(base_addr)?;
    // Enable device interrupt
    crate::drivers::virtio::enable_device_interrupt(base_addr);
    // U3: registration-time uevent for the (first) virtio-blk disk. The
    // MMIO GenDisk is "virtblk" (minor 0) and surfaces as /dev/vda.
    // Linux device-model shape (OH Phase 1b): virtio-mmio functions are
    // platform devices named "<addr>.virtio_mmio" (DT node form), and the
    // disk lives at /devices/platform/<addr>.virtio_mmio/virtioN/block/vda
    // — ueventd's /dev/block/by-name walk requires the platform ancestor.
    let capacity = crate::drivers::virtio::get_device()
        .map(|d| d.disk.get_capacity())
        .unwrap_or(0);
    let mmio_host = alloc::format!("{:x}.virtio_mmio", base_addr);
    crate::fs::sysfs::register_block_disk_at(
        "vda",
        crate::fs::devfs::VIRTIO_BLK_MAJOR,
        0,
        capacity,
        &[("platform", "platform"), (mmio_host.as_str(), "platform")],
    );
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
    if crate::drivers::net::loopback::loopback_init().is_some() {
        // U3: registration-time uevent for "lo".
        crate::fs::sysfs::netdev_uevent("lo", "add");
        true
    } else {
        false
    }
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

    // 3. PCI virtio-net (virtio-net-pci). Runs after the MMIO probe: the
    // eth0 singleton belongs to whichever transport found a device first
    // (on QEMU virt/riscv64 that is the MMIO function; on x86_64/q35 there
    // is no virtio-mmio and this is the only path).
    device_count += init_pci_net_devices();

    device_count
}

/// Initialize PCI network devices (virtio-net-pci)
///
/// # Notes
/// Probes and initializes the first VirtIO-Net function on the PCI bus
/// through the shared ECAM walker. The virtio-net layer owns a single
/// global device/NetDevice pair (same policy as virtio-blk's boot disk).
///
/// # Returns
/// Number of initialized devices
pub fn init_pci_net_devices() -> usize {
    // The MMIO probe already published eth0 — leave the singleton alone.
    if crate::drivers::net::virtio_net::get_device().is_some() {
        return 0;
    }

    let ecam_addresses = crate::drivers::pci::find_ecam_devices(
        crate::drivers::pci::vendor::RED_HAT,
        &[
            crate::drivers::pci::virtio_device::VIRTIO_NET,
            crate::drivers::pci::virtio_device::VIRTIO_NET_MODERN,
        ],
    );

    let mut device_count = 0;
    for ecam_addr in ecam_addresses {
        match crate::drivers::virtio::virtio_pci::VirtIOPCI::new(ecam_addr) {
            Ok(mut virtio_dev) => {
                match crate::drivers::net::virtio_net::init_pci(&mut virtio_dev) {
                    Ok(()) => {
                        device_count += 1;
                        // U3: registration-time uevent — notify udev/mdev
                        // the netdev exists (device is named "eth0").
                        crate::fs::sysfs::netdev_uevent("eth0", "add");
                        break; // single global net device
                    }
                    Err(e) => {
                        crate::pr_err!(
                            "virtio-net: PCI init failed at {:#x}: {}",
                            ecam_addr,
                            e
                        );
                    }
                }
            }
            Err(e) => {
                crate::pr_err!("virtio-net: PCI probe failed at {:#x}: {}", ecam_addr, e);
            }
        }
    }

    device_count
}

/// Initialize all block devices
///
/// # Notes
/// Block devices are already initialized by virtio_probe_devices() in init_network_devices().
/// This function is a no-op to prevent double initialization.
///
/// # Returns
/// 0 (devices already initialized)
pub fn init_block_devices() -> usize {
    0
}

/// ECAM addresses of virtio-blk functions already claimed by the boot
/// probe or a hotplug rescan (U4). Rescans only consider new functions.
static CLAIMED_BLK_ECAM: Spinlock<Vec<u64>> = Spinlock::new(Vec::new());

/// Next minor for a hotplug virtio-blk disk. vda (the boot disk, if any)
/// holds minor 0; hotplug disks take 16, 32, ... matching the legacy
/// per-disk minor stride of the 254 major.
static HOTPLUG_BLK_MINOR: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(16);

/// Initialize PCI block devices
///
/// # Notes
/// Probes and initializes VirtIO-Blk devices via PCI bus
///
/// # Returns
/// Number of initialized devices
pub fn init_pci_block_devices() -> usize {
    let mut device_count = 0;

    // Scan PCIe bus through the shared ECAM walker: 0x8000 stride per slot
    // + all 8 functions. The old local scan used a 0x1000 stride, reading
    // the FUNCTION bits as device numbers — every slot >= 4 was invisible
    // (review BUG "PCI 探测步长 0x1000 错，slot≥4 全漏").
    let ecam_addresses = crate::drivers::pci::find_ecam_devices(
        crate::drivers::pci::vendor::RED_HAT,
        &[
            crate::drivers::pci::virtio_device::VIRTIO_BLK,
            crate::drivers::pci::virtio_device::VIRTIO_BLK_MODERN,
        ],
    );

    for ecam_addr in ecam_addresses {
        // The virtio-blk layer owns ONE vring/GenDisk pair (single global
        // queue + single GenDisk registry slot). A second full init would
        // clobber those singletons and silently reroute the ROOT DISK's
        // I/O to the new device — claim only the first function at boot;
        // additional functions stay for the runtime rescan path
        // (pci_rescan_block_hotplug: metadata bring-up + sysfs + uevent).
        if device_count > 0 {
            crate::pr_info!(
                "virtio-blk: extra function at {:#x} deferred to PCI rescan",
                ecam_addr
            );
            break;
        }
        {
            match crate::drivers::virtio::virtio_pci::VirtIOPCI::new(ecam_addr) {
                Ok(mut virtio_dev) => {
                    // Reset device
                    virtio_dev.reset_device();

                    // Wait for device reset to complete (status becomes 0)
                    // Timeout from config (in loop iterations)
                    let mut reset_timeout = crate::config::VIRTIO_RESET_TIMEOUT_TICKS;
                    while virtio_dev.get_status() != 0 && reset_timeout > 0 {
                        core::hint::spin_loop();
                        reset_timeout -= 1;
                    }

                    // Set status to ACKNOWLEDGE | DRIVER
                    virtio_dev.set_status(crate::drivers::virtio::offset::status::ACKNOWLEDGE | crate::drivers::virtio::offset::status::DRIVER);

                    // Read device features
                    let features = virtio_dev.read_device_features();

                    // Feature negotiation narrowed to the implemented set
                    // (review BUG: "feature 协商三路径三套口径" — the old
                    // blacklist still accepted every other device feature
                    // the driver never honors). Word 0 accept-set:
                    //   SEG_MAX / GEOMETRY / BLK_SIZE — passed through to
                    //       the block layer without interpretation;
                    //   RO — honored implicitly: the disk is treated as
                    //       read-only when the device offers it;
                    //   FLUSH — now implemented (VIRTIO_BLK_T_FLUSH via
                    //       flush_block_using_configured_queue).
                    // EVENT_IDX stays rejected: the avail-ring used_event
                    // slot is never initialized, so the device would read a
                    // garbage value and could suppress completion
                    // notifications indefinitely (review DRIV-H2).
                    // INDIRECT/ANY_LAYOUT remain unimplemented.
                    const F_SEG_MAX: u32 = 1 << 1;
                    const F_GEOMETRY: u32 = 1 << 2;
                    const F_RO: u32 = 1 << 3;
                    const F_BLK_SIZE: u32 = 1 << 4;
                    const F_FLUSH: u32 = 1 << 6;
                    const SUPPORTED_W0: u32 =
                        F_SEG_MAX | F_GEOMETRY | F_RO | F_BLK_SIZE | F_FLUSH;
                    let masked = features & SUPPORTED_W0;

                    // Write driver features (word 0 + VIRTIO_F_VERSION_1 in
                    // word 1, required for modern-only devices)
                    virtio_dev.write_driver_features(masked);

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

                                    // U3: registration-time sysfs entry + "add"
                                    // uevent (the boot disk is /dev/vda).
                                    // /devices platform shape — OH Phase 1b.
                                    let capacity = crate::drivers::virtio::get_pci_gen_disk()
                                        .map(|d| d.get_capacity())
                                        .unwrap_or(0);
                                    let hier = pci_dev_hierarchy(ecam_addr);
                                    let chain: alloc::vec::Vec<(&str, &str)> = hier
                                        .iter()
                                        .map(|(n, b)| (n.as_str(), *b))
                                        .collect();
                                    crate::fs::sysfs::register_block_disk_at(
                                        "vda",
                                        crate::fs::devfs::VIRTIO_BLK_MAJOR,
                                        0,
                                        capacity,
                                        &chain,
                                    );

                                    // U4: remember the function for rescan diffing.
                                    CLAIMED_BLK_ECAM.lock().push(ecam_addr);

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

/// Linux device-model hierarchy for a virtio PCI function at `ecam_addr`
/// (the sysfs DEVPATH shape — see sysfs::register_block_disk_at). ECAM
/// layout: addr = ECAM | bus<<20 | dev<<15 | fn<<12.
///
/// riscv64 virt: the gpex PCI host controller is a PLATFORM device (DT
/// node pci@<ecam-base> → "<hex-addr>.pci") and the root bus hangs below
/// it — matching the real Linux tree
/// /sys/devices/platform/30000000.pci/pci0000:00/0000:00:0X.0/virtioN.
/// x86_64 q35: no platform wrapper (/sys/devices/pci0000:00/...), like
/// real x86 hardware.
fn pci_dev_hierarchy(ecam_addr: u64) -> alloc::vec::Vec<(alloc::string::String, &'static str)> {
    let bus = (ecam_addr >> 20) & 0xFF;
    let dev = (ecam_addr >> 15) & 0x1F;
    let func = (ecam_addr >> 12) & 0x7;
    let fn_name = alloc::format!("0000:{:02x}:{:02x}.{}", bus, dev, func);
    let mut chain: alloc::vec::Vec<(alloc::string::String, &'static str)> =
        alloc::vec::Vec::new();
    #[cfg(feature = "riscv64")]
    {
        let host = alloc::format!(
            "{:x}.pci",
            crate::arch::mm::memory_layout::PCIE_ECAM_BASE
        );
        chain.push((alloc::string::String::from("platform"), "platform"));
        chain.push((host, "platform"));
        chain.push((alloc::string::String::from("pci0000:00"), "pci"));
    }
    #[cfg(not(feature = "riscv64"))]
    {
        chain.push((alloc::string::String::from("pci0000:00"), "pci"));
    }
    chain.push((fn_name, "pci"));
    chain
}

/// Hotplug PCI rescan (U4): `echo 1 > /sys/bus/pci/rescan`.
///
/// Re-enumerates the ECAM space for virtio-blk functions not yet claimed
/// (attached after boot, or deferred by the boot probe — see
/// init_pci_block_devices). The riscv/virt platform has no ACPI/PCIe
/// hotplug interrupt AND QEMU's gpex host does not decode ECAM cycles for
/// secondary buses behind pcie-root-ports (verified on QEMU 8.2.2 and
/// 10.2.2: full guest-side bridge programming — bus numbers, memory
/// window, command, slot power — still leaves bus-1 config reads at
/// 0xFFFFFFFF while QEMU's own info pci sees the device), so userland
/// drives the rescan and only root-bus functions are discoverable.
///
/// Each NEW function gets a minimal virtio bring-up (reset → ACK|DRIVER →
/// FEATURES_OK → DRIVER_OK) and a config-space capacity read; the disk is
/// then registered in sysfs (/sys/class/block/vdX) and an "add" uevent
/// with MAJOR/MINOR/DEVNAME is broadcast for udev/mdev to create the
/// /dev/vdX node.
///
/// NOTE (scope): the hotplug disk is metadata-complete (sysfs + uevent +
/// node), not I/O-wired — the virtio-blk layer has a single global
/// vring/GenDisk pair owned by the boot disk; hot disks do not steal it.
pub fn pci_rescan_block_hotplug() -> usize {
    use crate::drivers::virtio::offset::status;

    let ecam_addresses = crate::drivers::pci::find_ecam_devices(
        crate::drivers::pci::vendor::RED_HAT,
        &[
            crate::drivers::pci::virtio_device::VIRTIO_BLK,
            crate::drivers::pci::virtio_device::VIRTIO_BLK_MODERN,
        ],
    );

    let mut found = 0usize;
    for ecam_addr in ecam_addresses {
        // Skip functions the boot probe already claimed.
        if CLAIMED_BLK_ECAM.lock().contains(&ecam_addr) {
            continue;
        }

        let dev = match crate::drivers::virtio::virtio_pci::VirtIOPCI::new(ecam_addr) {
            Ok(d) => d,
            Err(_) => continue,
        };

        // Minimal bring-up: reset, ACK|DRIVER, no feature the driver must
        // then honor, FEATURES_OK, DRIVER_OK. No queue setup — config
        // space (capacity) is readable once DRIVER_OK is set.
        dev.reset_device();
        let mut reset_timeout = crate::config::VIRTIO_RESET_TIMEOUT_TICKS;
        while dev.get_status() != 0 && reset_timeout > 0 {
            core::hint::spin_loop();
            reset_timeout -= 1;
        }
        if dev.get_status() != 0 {
            continue;
        }
        dev.set_status(status::ACKNOWLEDGE | status::DRIVER);
        dev.write_driver_features(0); // word 0 none; word 1 VIRTIO_F_VERSION_1
        dev.set_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);
        if dev.get_status() & status::FEATURES_OK == 0 {
            continue;
        }
        dev.set_status(
            status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK,
        );

        // Capacity: virtio-blk device config is a u64 sector count at
        // offset 0 of the device cfg region. register_pci_gen_disk reads
        // it at common_cfg + 0x2000 (same BAR, capability layout); use
        // the parsed device_cfg BAR when present, else the known quirk.
        let cfg_base = if dev.device_cfg_bar != 0 {
            dev.device_cfg_bar
        } else {
            dev.common_cfg_bar + 0x2000
        };
        // SAFETY: cfg_base is a valid MMIO-mapped virtio-blk device
        // config region (parsed from the device's PCI capabilities).
        let capacity = unsafe { core::ptr::read_volatile(cfg_base as *const u64) };

        // Assign identity: vdX with minor stride 16 (vda holds minor 0).
        let minor = HOTPLUG_BLK_MINOR.fetch_add(16, core::sync::atomic::Ordering::SeqCst);
        if minor > 16 * 25 {
            break; // cap at vdz
        }
        let letter = b'a' + (minor / 16) as u8;
        let name = alloc::format!("vd{}", letter as char);

        let hier = pci_dev_hierarchy(ecam_addr);
        let chain: alloc::vec::Vec<(&str, &str)> =
            hier.iter().map(|(n, b)| (n.as_str(), *b)).collect();
        let seq = crate::fs::sysfs::register_block_disk_at(
            &name,
            crate::fs::devfs::VIRTIO_BLK_MAJOR,
            minor,
            capacity,
            &chain,
        );
        CLAIMED_BLK_ECAM.lock().push(ecam_addr);
        found += 1;
        crate::pr_info!(
            "virtio-blk hotplug: {} ({}:{}) {} sectors, uevent seq {}",
            name,
            crate::fs::devfs::VIRTIO_BLK_MAJOR,
            minor,
            capacity,
            seq
        );
    }

    if found == 0 {
        crate::pr_info!("pci rescan: no new virtio-blk functions");
    }

    found
}
