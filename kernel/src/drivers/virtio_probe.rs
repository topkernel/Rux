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
///
/// # 说明
/// 快速探测 VirtIO 设备，使用最小化的 MMIO 访问
/// 只检查第一个设备槽位，快速返回
pub fn virtio_probe_devices() -> usize {
    let mut device_count = 0;

    println!("drivers: Quick VirtIO device scan...");

    // 只检查第一个 VirtIO 设备槽位（最快）
    let base_addr = VIRTIO_MMIO_BASE;

    // 快速读取魔数
    let magic = unsafe {
        let magic_ptr = base_addr as *const u32;
        core::ptr::read_volatile(magic_ptr)
    };

    // 检查魔数（"virt" = 0x74726976）
    if magic == 0x74726976 {
        // 找到了 VirtIO 设备，读取更多信息
        let (version, device_id) = unsafe {
            let version_ptr = (base_addr + 4) as *const u32;
            let device_id_ptr = (base_addr + 8) as *const u32;
            (
                core::ptr::read_volatile(version_ptr),
                core::ptr::read_volatile(device_id_ptr),
            )
        };

        // 检查版本
        if version == 1 || version == 2 {
            // 识别设备类型
            match device_id {
                1 => {
                    println!("drivers:   VirtIO-Net device detected (eth0)");
                    device_count += 1;
                }
                2 => {
                    println!("drivers:   VirtIO-Blk device detected (virtblk0)");
                    device_count += 1;
                }
                _ => {
                    println!("drivers:   VirtIO device (ID={}) detected", device_id);
                    device_count += 1;
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

    // 1. 初始化回环设备（总是可用）
    init_loopback_device();
    device_count += 1;

    // 2. VirtIO 设备探测（通过 feature flag 控制）
    // 默认禁用以确保系统在没有 VirtIO 设备时也能正常运行
    // 要启用探测：编译时添加 --features virtio-net-probe
    #[cfg(feature = "virtio-net-probe")]
    {
        println!("drivers: VirtIO device probe enabled (feature flag set)");
        let virtio_count = virtio_probe_devices();
        device_count += virtio_count;
    }

    #[cfg(not(feature = "virtio-net-probe"))]
    {
        println!("drivers: VirtIO device probe: disabled (default)");
        println!("drivers:   To enable: cargo build --features virtio-net-probe");
        println!("drivers:   Then add to QEMU: -device virtio-net,netdev=user");
    }

    println!("drivers: Network device initialization completed, {} device(s) ready", device_count);
    device_count
}
