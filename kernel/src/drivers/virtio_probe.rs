//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO 设备探测
//!
//! 用于探测和初始化 VirtIO 设备
//! 参考: Linux kernel virtio device probing

use crate::println;

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
const VIRTIO_MMIO_BASE: u64 = 0x10001000;
const VIRTIO_MMIO_SIZE: u64 = 0x1000;

/// VirtIO 设备数量
const VIRTIO_MAX_DEVICES: usize = 8;

/// 探测所有 VirtIO 设备
///
/// # 返回
/// 返回找到的设备数量
pub fn virtio_probe_devices() -> usize {
    let mut device_count = 0;

    println!("drivers: Probing VirtIO devices...");

    // 扫描所有可能的 VirtIO 设备槽位
    for i in 0..VIRTIO_MAX_DEVICES {
        let base_addr = VIRTIO_MMIO_BASE + (i as u64 * VIRTIO_MMIO_SIZE);

        // 读取魔数和版本
        let (magic, version, device_id) = unsafe {
            let magic_ptr = base_addr as *const u32;
            let version_ptr = (base_addr + 4) as *const u32;
            let device_id_ptr = (base_addr + 8) as *const u32;

            (
                core::ptr::read_volatile(magic_ptr),
                core::ptr::read_volatile(version_ptr),
                core::ptr::read_volatile(device_id_ptr),
            )
        };

        // 检查魔数（"virt" = 0x74726976）
        if magic != 0x74726976 {
            continue; // 没有设备
        }

        // 检查版本
        if version != 1 && version != 2 {
            println!("drivers:   Device at 0x{:x}: unsupported version {}", base_addr, version);
            continue;
        }

        // 识别设备类型
        match device_id {
            1 => {
                // VirtIO-Net 网络设备
                println!("drivers:   Found VirtIO-Net device at 0x{:x}", base_addr);
                // 暂时跳过实际初始化，只记录设备
                println!("drivers:     VirtIO-Net device detected (not initializing in test mode)");
                device_count += 1;
            }
            2 => {
                // VirtIO-Blk 块设备
                println!("drivers:   Found VirtIO-Blk device at 0x{:x}", base_addr);
                // 暂时跳过实际初始化，只记录设备
                println!("drivers:     VirtIO-Blk device detected (not initializing in test mode)");
                device_count += 1;
            }
            0 => {
                // 设备不存在
            }
            _ => {
                println!("drivers:   Unknown VirtIO device (ID={}) at 0x{:x}", device_id, base_addr);
            }
        }
    }

    println!("drivers: VirtIO probe completed, found {} device(s)", device_count);

    if device_count == 0 {
        println!("drivers:   No VirtIO devices found (this is expected if QEMU doesn't have VirtIO devices enabled)");
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
        crate::drivers::virtio::init(base_addr)
    }

    #[cfg(not(feature = "riscv64"))]
    {
        let _ = base_addr;
        Err("VirtIO-Blk not supported on this platform")
    }
}

/// 初始化回环网络设备
///
/// # 说明
/// 回环设备总是可用，作为后备网络设备
pub fn init_loopback_device() {
    println!("drivers: Initializing loopback network device...");

    if let Some(_device) = crate::drivers::net::loopback::loopback_init() {
        println!("drivers: Loopback device initialized successfully");
    } else {
        println!("drivers: Loopback device initialization failed");
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

    // 1. 初始化回环设备
    init_loopback_device();
    device_count += 1;

    // 2. 探测并初始化 VirtIO-Net 设备
    // 注意：暂时禁用 VirtIO 探测，因为它可能导致 QEMU 挂起
    // 在实际使用时，可以通过 QEMU 参数启用 VirtIO-Net 设备
    #[cfg(feature = "virtio-net-probe")]
    {
        let virtio_count = virtio_probe_devices();
        device_count += virtio_count;
    }

    #[cfg(not(feature = "virtio-net-probe"))]
    {
        println!("drivers: VirtIO device probe disabled (enable with 'virtio-net-probe' feature)");
    }

    println!("drivers: Network device initialization completed, {} device(s) ready", device_count);
    device_count
}
