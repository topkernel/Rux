//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO 设备探测
//!
//! 用于探测和初始化 VirtIO 设备
//! 参考: Linux kernel virtio device probing

use crate::println;
use crate::config::ENABLE_VIRTIO_NET_PROBE;

/// VirtIO 设备 ID
///
/// 对应 VirtIO 规范中的设备类型
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtIODeviceId {
    /// 网络设备
    VirtioNet = 1,
    /// 块设备
    VirtioBlk = 2,
    /// 控制台
    VirtioConsole = 3,
    ///_entropy
    VirtioRng = 4,
    /// 气球设备
    VirtioBalloon = 5,
    /// I/O 内存
    VirtioScsi = 8,
    /// GPU
    VirtioGpu = 16,
}

/// VirtIO 设备 MMIO 基地址
///
/// QEMU virt 平台的 VirtIO 设备地址范围
/// 参考: QEMU virt 平台文档
/// 使用恒等映射：VIRTIO_MMIO_BASE 在 0x10000000 附近
const VIRTIO_MMIO_BASE: u64 = 0x10001000;
const VIRTIO_MMIO_SIZE: u64 = 0x1000;

/// VirtIO 设备数量
const VIRTIO_MAX_DEVICES: usize = 8;

/// 探测所有 VirtIO 设备
///
/// # 返回
/// 返回找到的设备数量
///
/// # 说明
/// 扫描所有 8 个 VirtIO 设备槽位
pub fn virtio_probe_devices() -> usize {
    let mut device_count = 0;

    println!("drivers: Scanning VirtIO devices...");

    // 扫描所有 VirtIO 设备槽位
    for device_index in 0..VIRTIO_MAX_DEVICES {
        let base_addr = VIRTIO_MMIO_BASE + (device_index as u64 * VIRTIO_MMIO_SIZE);

        // 快速读取魔数
        let magic = unsafe {
            let magic_ptr = base_addr as *const u32;
            core::ptr::read_volatile(magic_ptr)
        };

        // 检查魔数（"virt" = 0x74726976）
        if magic == 0x74726976 {
            // 找到了 VirtIO 设备，读取更多信息
            let (version, device_id, vendor, device_features) = unsafe {
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

            // 检查版本
            if version == 1 || version == 2 {
                // 识别设备类型并初始化
                match device_id {
                    1 => {
                        println!("drivers:   VirtIO-Net device detected at slot {}", device_index);
                        match init_virtio_net(base_addr) {
                            Ok(()) => {
                                println!("drivers:   VirtIO-Net initialized successfully");
                                device_count += 1;
                            }
                            Err(e) => {
                                println!("drivers:   VirtIO-Net init failed: {}", e);
                            }
                        }
                    }
                    2 => {
                        println!("drivers:   VirtIO-Blk device detected at slot {}", device_index);
                        match init_virtio_blk(base_addr) {
                            Ok(()) => {
                                println!("drivers:   VirtIO-Blk initialized successfully");
                                device_count += 1;
                            }
                            Err(e) => {
                                println!("drivers:   VirtIO-Blk init failed: {}", e);
                            }
                        }
                    }
                    _ => {
                        if device_id != 0 {
                            println!("drivers:   Unsupported VirtIO device (ID={}) at slot {}", device_id, device_index);
                        }
                    }
                }
            }
        }
    }

    if device_count == 0 {
        println!("drivers: No VirtIO devices found");
    }

    device_count
}

/// 初始化 VirtIO-Net 设备
///
/// # 参数
/// - `base_addr`: 设备 MMIO 基地址
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(&str)
fn init_virtio_net(base_addr: u64) -> Result<(), &'static str> {
    #[cfg(feature = "riscv64")]
    {
        crate::drivers::net::virtio_net::init(base_addr)
    }

    #[cfg(not(feature = "riscv64"))]
    {
        let _ = base_addr;
        Err("VirtIO-Net not supported on this platform")
    }
}

/// 初始化 VirtIO-Blk 设备
///
/// # 参数
/// - `base_addr`: 设备 MMIO 基地址
///
/// # 返回
/// 成功返回 Ok(())，失败返回 Err(&str)
fn init_virtio_blk(base_addr: u64) -> Result<(), &'static str> {
    #[cfg(feature = "riscv64")]
    {
        crate::drivers::virtio::init(base_addr)?;
        // 使能设备中断
        crate::drivers::virtio::enable_device_interrupt(base_addr);
        Ok(())
    }

    #[cfg(not(feature = "riscv64"))]
    {
        let _ = base_addr;
        Err("VirtIO-Blk not supported on this platform")
    }
}

/// 初始化回环网络设备
///
/// # 返回
/// 成功返回 true，失败返回 false
///
/// # 说明
/// 回环设备总是可用，作为后备网络设备
fn init_loopback_device() -> bool {
    if let Some(_device) = crate::drivers::net::loopback::loopback_init() {
        println!("drivers:   Loopback device (lo) initialized");
        true
    } else {
        println!("drivers:   Loopback device init failed");
        false
    }
}

/// 初始化所有网络设备
///
/// # 说明
/// 按顺序初始化：
/// 1. 回环设备（总是可用）
/// 2. VirtIO-Net 设备（如果存在）
///
/// # 返回
/// 返回初始化的设备数量
pub fn init_network_devices() -> usize {
    let mut device_count = 0;

    println!("drivers: Initializing network devices...");

    // 1. 初始化回环设备（总是可用）
    if init_loopback_device() {
        device_count += 1;
    }

    // 2. VirtIO 设备探测（通过 menuconfig 配置控制）
    if ENABLE_VIRTIO_NET_PROBE {
        let virtio_count = virtio_probe_devices();
        device_count += virtio_count;
    }

    println!("drivers: {} network device(s) ready", device_count);
    device_count
}

/// 初始化所有块设备
///
/// # 说明
/// 探测并初始化 VirtIO-Blk 设备
///
/// # 返回
/// 返回初始化的设备数量
pub fn init_block_devices() -> usize {
    let mut device_count = 0;

    println!("drivers: Initializing block devices...");

    // 扫描所有 VirtIO 设备槽位
    for device_index in 0..VIRTIO_MAX_DEVICES {
        let base_addr = VIRTIO_MMIO_BASE + (device_index as u64 * VIRTIO_MMIO_SIZE);

        // 快速读取魔数
        let magic = unsafe {
            let magic_ptr = base_addr as *const u32;
            core::ptr::read_volatile(magic_ptr)
        };

        // 检查魔数（"virt" = 0x74726976）
        if magic == 0x74726976 {
            // 读取设备 ID
            let device_id = unsafe {
                let device_id_ptr = (base_addr + 8) as *const u32;
                core::ptr::read_volatile(device_id_ptr)
            };

            // 检查是否为块设备
            if device_id == 2 {
                println!("drivers:   VirtIO-Blk device detected at slot {}", device_index);
                match init_virtio_blk(base_addr) {
                    Ok(()) => {
                        println!("drivers:   VirtIO-Blk initialized successfully");
                        device_count += 1;
                    }
                    Err(e) => {
                        println!("drivers:   VirtIO-Blk init failed: {}", e);
                    }
                }
            }
        }
    }

    if device_count == 0 {
        println!("drivers: No VirtIO block devices found");
    }

    device_count
}

/// 初始化 PCI 块设备
///
/// # 说明
/// 通过 PCI 总线探测并初始化 VirtIO-Blk 设备
///
/// # 返回
/// 返回初始化的设备数量
pub fn init_pci_block_devices() -> usize {
    println!("drivers: Initializing PCI block devices...");

    #[cfg(feature = "riscv64")]
    {
        let mut device_count = 0;

        // 扫描 PCIe 总线（QEMU virt 平台）
        const MAX_DEVICES: u8 = 32;

        for device in 0..MAX_DEVICES {
            let ecam_addr = crate::drivers::pci::RISCV_PCIE_ECAM_BASE + (device as u64 * crate::drivers::pci::PCIE_ECAM_SIZE);
            let config = crate::drivers::pci::PCIConfig::new(ecam_addr);

            let vendor_id = config.vendor_id();

            // 跳过空设备
            if vendor_id == 0xFFFF {
                continue;
            }

            let device_id = config.device_id();

            // 检查是否为 VirtIO 块设备
            if vendor_id == crate::drivers::pci::vendor::RED_HAT &&
               (device_id == crate::drivers::pci::virtio_device::VIRTIO_BLK ||
                device_id == crate::drivers::pci::virtio_device::VIRTIO_BLK_MODERN) {
                println!("drivers:   Found VirtIO block device (vendor=0x{:04x}, device=0x{:04x})",
                    vendor_id, device_id);

                match crate::drivers::virtio::virtio_pci::VirtIOPCI::new(ecam_addr) {
                    Ok(mut virtio_dev) => {
                        // 重置设备
                        virtio_dev.reset_device();

                        // 设置状态为 ACKNOWLEDGE | DRIVER
                        virtio_dev.set_status(crate::drivers::virtio::offset::status::ACKNOWLEDGE | crate::drivers::virtio::offset::status::DRIVER);

                        // 读取设备特征
                        let features = virtio_dev.read_device_features();

                        // 读取设备容量（从 Device Config space，偏移 0x2000）
                        let device_cfg_addr = virtio_dev.common_cfg_bar + 0x2000;
                        unsafe {
                            let capacity_ptr = device_cfg_addr as *const u64;
                            let capacity = core::ptr::read_volatile(capacity_ptr);
                            println!("drivers:   Device capacity: {} MB", capacity * 512 / (1024 * 1024));
                        }

                        // 写入驱动特征
                        virtio_dev.write_driver_features(features);

                        // 设置 FEATURES_OK
                        virtio_dev.set_status(
                            crate::drivers::virtio::offset::status::ACKNOWLEDGE |
                            crate::drivers::virtio::offset::status::DRIVER |
                            crate::drivers::virtio::offset::status::FEATURES_OK
                        );

                        // 验证 FEATURES_OK 被设备接受
                        let status_after_features = virtio_dev.get_status();
                        if status_after_features & crate::drivers::virtio::offset::status::FEATURES_OK == 0 {
                            println!("drivers:   ERROR: Device rejected FEATURES_OK");
                            continue;
                        }

                        // 选择队列 0 并读取队列大小
                        unsafe {
                            let queue_select_ptr = (virtio_dev.common_cfg_bar + crate::drivers::virtio::offset::COMMON_CFG_QUEUE_SELECT as u64) as *mut u16;
                            core::ptr::write_volatile(queue_select_ptr, 0u16);
                        }

                        let queue_max = unsafe {
                            let queue_size_max_ptr = (virtio_dev.common_cfg_bar + crate::drivers::virtio::offset::COMMON_CFG_QUEUE_SIZE as u64) as *const u16;
                            core::ptr::read_volatile(queue_size_max_ptr)
                        };

                        // 创建 VirtQueue
                        let dummy_isr_addr = virtio_dev.common_cfg_bar;
                        match crate::drivers::virtio::queue::VirtQueue::new(queue_max,
                            0,  // queue_index
                            virtio_dev.get_notify_addr(0),
                            dummy_isr_addr,
                            dummy_isr_addr) {
                            None => {
                                println!("drivers:   VirtQueue creation failed");
                            }
                            Some(virt_queue) => {
                                match virtio_dev.setup_queue(0, &virt_queue) {
                                    Ok(()) => {
                                        // 存储已配置的 VirtQueue 到全局存储
                                        crate::drivers::virtio::set_pci_device_queue(virt_queue);

                                        // 启用设备中断
                                        virtio_dev.enable_device_interrupt();

                                        // 设置 DRIVER_OK
                                        virtio_dev.set_status(
                                            crate::drivers::virtio::offset::status::ACKNOWLEDGE |
                                            crate::drivers::virtio::offset::status::DRIVER |
                                            crate::drivers::virtio::offset::status::FEATURES_OK |
                                            crate::drivers::virtio::offset::status::DRIVER_OK
                                        );

                                        // 注册 PCI VirtIO 设备到全局存储
                                        crate::drivers::virtio::register_pci_device(virtio_dev);

                                        // 注册 GenDisk 包装器（使 ext4 驱动可以访问）
                                        crate::drivers::virtio::register_pci_gen_disk();

                                        println!("drivers:   VirtIO-PCI block device initialized successfully");
                                        device_count += 1;
                                    }
                                    Err(e) => {
                                        println!("drivers:   VirtQueue setup failed: {}", e);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        println!("drivers:   VirtIO-PCI init failed: {}", e);
                    }
                }
            }
        }

        if device_count == 0 {
            println!("drivers: No VirtIO PCI block devices found");
        }

        device_count
    }

    #[cfg(not(feature = "riscv64"))]
    {
        println!("drivers: PCI block devices not supported on this platform");
        0
    }
}
