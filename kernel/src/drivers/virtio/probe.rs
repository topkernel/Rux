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

            println!("drivers:   Slot {}: magic=0x{:08x}, version={}, device_id={}, vendor=0x{:08x}, features=0x{:08x}",
                device_index, magic, version, device_id, vendor, device_features);

            // 检查版本
            if version == 1 || version == 2 {
                // 识别设备类型并初始化
                match device_id {
                    1 => {
                        println!("drivers:     VirtIO-Net device detected (eth{})", device_index);
                        // 初始化 VirtIO-Net 设备
                        match init_virtio_net(base_addr) {
                            Ok(()) => {
                                println!("drivers:     VirtIO-Net device initialized successfully");
                                device_count += 1;
                            }
                            Err(e) => {
                                println!("drivers:     VirtIO-Net device initialization failed: {}", e);
                            }
                        }
                    }
                    2 => {
                        println!("drivers:     VirtIO-Blk device detected (virtblk{})", device_index);
                        // 初始化 VirtIO-Blk 设备
                        match init_virtio_blk(base_addr) {
                            Ok(()) => {
                                println!("drivers:     VirtIO-Blk device initialized successfully");
                                device_count += 1;
                            }
                            Err(e) => {
                                println!("drivers:     VirtIO-Blk device initialization failed: {}", e);
                            }
                        }
                    }
                    _ => {
                        if device_id != 0 {
                            println!("drivers:     VirtIO device (ID={}) detected at slot {}", device_id, device_index);
                            println!("drivers:     Device type not supported, skipping initialization");
                        }
                        // device_id=0 表示空槽位，不计数
                    }
                }
            }
        }
    }

    println!("drivers: Scan completed, found {} VirtIO device(s)", device_count);

    if device_count == 0 {
        println!("drivers:   No VirtIO devices detected (use QEMU -device virtio-net to enable)");
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
        println!("drivers: Loopback device (lo) initialized successfully");
        true
    } else {
        println!("drivers: Loopback device initialization failed");
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
    // 默认启用，如需禁用：在 Kernel.toml 中设置 enable_virtio_net_probe = false
    if ENABLE_VIRTIO_NET_PROBE {
        println!("drivers: VirtIO device probe enabled (default)");
        let virtio_count = virtio_probe_devices();
        device_count += virtio_count;
    } else {
        println!("drivers: VirtIO device probe: disabled (config setting)");
        println!("drivers:   To enable: Set enable_virtio_net_probe = true in Kernel.toml");
        println!("drivers:   Then add to QEMU: -device virtio-net,netdev=user -netdev user,id=user");
    }

    println!("drivers: Network device initialization completed, {} device(s) ready", device_count);
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
        println!("drivers: Scanning slot {}...", device_index);
        let base_addr = VIRTIO_MMIO_BASE + (device_index as u64 * VIRTIO_MMIO_SIZE);

        // 快速读取魔数
        let magic = unsafe {
            let magic_ptr = base_addr as *const u32;
            core::ptr::read_volatile(magic_ptr)
        };

        // 检查魔数（"virt" = 0x74726976）
        if magic == 0x74726976 {
            println!("drivers:   Found VirtIO device at slot {}", device_index);
            // 读取设备 ID
            let device_id = unsafe {
                let device_id_ptr = (base_addr + 8) as *const u32;
                core::ptr::read_volatile(device_id_ptr)
            };

            println!("drivers:   Device ID = {}", device_id);

            // 检查是否为块设备
            if device_id == 2 {
                println!("drivers:   VirtIO-Blk device detected at slot {}", device_index);
                println!("drivers:   Calling init_virtio_blk...");
                match init_virtio_blk(base_addr) {
                    Ok(()) => {
                        println!("drivers:   VirtIO-Blk device initialized successfully");
                        device_count += 1;
                    }
                    Err(e) => {
                        println!("drivers:   VirtIO-Blk device initialization failed: {}", e);
                    }
                }
            }
        }
    }

    println!("drivers: Block device initialization completed, {} device(s) ready", device_count);
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
                println!("drivers: Found VirtIO block device: vendor=0x{:04x}, device=0x{:04x} at slot {}",
                    vendor_id, device_id, device);

                println!("drivers:   Using PCI ECAM address: 0x{:x}", ecam_addr);

                // 初始化 VirtIO-PCI 设备（使用 PCI ECAM 地址）
                // VirtIOPCI::new() 会读取 BAR 来获取实际的 MMIO 地址
                println!("drivers:   Initializing VirtIO-PCI device...");
                match crate::drivers::virtio::virtio_pci::VirtIOPCI::new(ecam_addr) {
                    Ok(mut virtio_dev) => {
                        println!("drivers:   VirtIO-PCI device created successfully");

                        // 重置设备
                        println!("drivers:   About to call reset_device()...");
                        virtio_dev.reset_device();
                        println!("drivers:   reset_device() returned");

                        // 设置状态为 ACKNOWLEDGE | DRIVER
                        println!("drivers:   About to call set_status()...");
                        virtio_dev.set_status(crate::drivers::virtio::offset::status::ACKNOWLEDGE | crate::drivers::virtio::offset::status::DRIVER);
                        println!("drivers:   set_status() returned");

                        // 读取设备特征
                        let features = virtio_dev.read_device_features();
                        println!("drivers:   Device features offered: 0x{:08x}", features);

                        // 读取设备容量（从 Device Config space，偏移 0x2000）
                        // VirtIO Block Device Configuration:
                        // - 0x00: capacity (64-bit)
                        // - 0x08: size_max
                        // - 0x0C: seg_max
                        let device_cfg_addr = virtio_dev.common_cfg_bar + 0x2000;
                        unsafe {
                            let capacity_ptr = device_cfg_addr as *const u64;
                            let capacity = core::ptr::read_volatile(capacity_ptr);
                            println!("drivers:   Device capacity: {} sectors ({} MB)",
                                capacity, capacity * 512 / (1024 * 1024));
                        }

                        // 写入驱动特征（接受设备提供的特性）
                        let device_features = virtio_dev.read_device_features();
                        println!("drivers:   Writing driver features: 0x{:08x}", device_features);
                        virtio_dev.write_driver_features(device_features);

                        // VirtIO 1.0 规范要求：在设置队列之前设置 FEATURES_OK
                        // 初始化顺序：
                        // 1. reset_device()
                        // 2. set_status(ACKNOWLEDGE | DRIVER)
                        // 3. read_device_features()
                        // 4. write_driver_features()
                        // 5. set_status(FEATURES_OK) ← 现在！
                        // 6. 验证 FEATURES_OK 被设备接受
                        // 7. setup_queue()
                        // 8. set_status(DRIVER_OK)
                        virtio_dev.set_status(
                            crate::drivers::virtio::offset::status::ACKNOWLEDGE |
                            crate::drivers::virtio::offset::status::DRIVER |
                            crate::drivers::virtio::offset::status::FEATURES_OK
                        );

                        // 关键：验证 FEATURES_OK 被设备接受
                        let status_after_features = virtio_dev.get_status();
                        if status_after_features & crate::drivers::virtio::offset::status::FEATURES_OK == 0 {
                            println!("drivers:   ERROR: Device rejected FEATURES_OK! status = 0x{:02x}", status_after_features);
                            // Don't continue with queue setup if device rejects FEATURES_OK
                            continue;
                        }
                        println!("drivers:   FEATURES_OK set and verified (0x{:02x})", status_after_features);

                        // 关键修复：在创建 VirtQueue 之前读取设备支持的最大队列大小
                        // 选择队列 0
                        unsafe {
                            let queue_select_ptr = (virtio_dev.common_cfg_bar + crate::drivers::virtio::offset::COMMON_CFG_QUEUE_SELECT as u64) as *mut u16;
                            core::ptr::write_volatile(queue_select_ptr, 0u16);
                        }

                        // 读取队列最大大小
                        let queue_max = unsafe {
                            let queue_size_max_ptr = (virtio_dev.common_cfg_bar + crate::drivers::virtio::offset::COMMON_CFG_QUEUE_SIZE as u64) as *const u16;
                            core::ptr::read_volatile(queue_size_max_ptr)
                        };

                        println!("drivers:   Device supports queue size: {}", queue_max);

                        // 创建 VirtQueue（使用设备支持的最大队列大小）
                        println!("drivers:   Creating VirtQueue (queue_size={})...", queue_max);
                        // VirtIO PCI 使用 PLIC 中断，不轮询 ISR
                        // 传递 dummy 地址给 VirtQueue（实际不会使用）
                        // VirtQueue 的 interrupt_status/interrupt_ack 仅用于 MMIO VirtIO
                        let dummy_isr_addr = virtio_dev.common_cfg_bar;
                        println!("drivers:   VirtQueue will use PLIC interrupts (not ISR polling)");
                        match crate::drivers::virtio::queue::VirtQueue::new(queue_max,
                            virtio_dev.get_notify_addr(0),
                            dummy_isr_addr,
                            dummy_isr_addr) {
                            None => {
                                println!("drivers:   VirtQueue creation failed");
                            }
                            Some(mut virt_queue) => {
                                println!("drivers:   VirtQueue created successfully");

                                // 设置队列
                                match virtio_dev.setup_queue(0, &virt_queue) {
                                    Ok(()) => {
                                        println!("drivers:   VirtQueue setup complete");

                                        // VirtIO 设备已经正确初始化，使用恒等映射
                                        // I/O 测试留待文件系统驱动验证
                                        println!("drivers:   Device registered successfully (using identity-mapped physical addresses)");

                                        // 存储已配置的 VirtQueue 到全局存储
                                        // 这样 read_block() 等函数可以重用这个队列，而不是创建新的
                                        crate::drivers::virtio::set_pci_device_queue(virt_queue);
                                        println!("drivers:   VirtQueue stored to global storage for I/O reuse");

                                        // VirtIO 规范要求：队列设置完成后设置 DRIVER_OK
                                        // 必须在 setup_queue 成功后才能设置 DRIVER_OK
                                        virtio_dev.set_status(
                                            crate::drivers::virtio::offset::status::ACKNOWLEDGE |
                                            crate::drivers::virtio::offset::status::DRIVER |
                                            crate::drivers::virtio::offset::status::FEATURES_OK |
                                            crate::drivers::virtio::offset::status::DRIVER_OK
                                        );
                                        println!("drivers:   Device status set to DRIVER_OK (0x0F)");

                                        // 注册 PCI VirtIO 设备到全局存储
                                        crate::drivers::virtio::register_pci_device(virtio_dev);

                                        device_count += 1;
                                    }
                                    Err(e) => {
                                        println!("drivers:   VirtQueue setup failed: {}", e);
                                    }
                                }
                            }
                        }

                        println!("drivers:   VirtIO-PCI device initialization complete");
                    }
                    Err(e) => {
                        println!("drivers:   VirtIO-PCI device creation failed: {}", e);
                    }
                }
            }
        }

        println!("drivers: PCI block device initialization completed, {} device(s) ready", device_count);
        device_count
    }

    #[cfg(not(feature = "riscv64"))]
    {
        println!("drivers: PCI block devices not supported on this platform");
        0
    }
}
