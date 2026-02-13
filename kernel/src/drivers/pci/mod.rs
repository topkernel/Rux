//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! PCI 配置空间访问层
//!
//! 实现 PCI 配置空间访问和设备枚举
//! 参考: Linux kernel drivers/pci/

/// PCI 配置空间寄存器偏移
pub mod offset {
    pub const VENDOR_ID: u8 = 0x00;
    pub const DEVICE_ID: u8 = 0x02;
    pub const COMMAND: u8 = 0x04;
    pub const STATUS: u8 = 0x06;
    pub const REVISION: u8 = 0x08;
    pub const PROG_IF: u8 = 0x09;
    pub const SUBCLASS: u8 = 0x0A;
    pub const CLASS: u8 = 0x0B;
    pub const CACHE_LINE_SIZE: u8 = 0x0C;
    pub const LATENCY_TIMER: u8 = 0x0D;
    pub const HEADER_TYPE: u8 = 0x0E;
    pub const BIST: u8 = 0x0F;
    pub const BAR0: u8 = 0x10;
    pub const BAR1: u8 = 0x14;
    pub const BAR2: u8 = 0x18;
    pub const BAR3: u8 = 0x1C;
    pub const BAR4: u8 = 0x20;
    pub const BAR5: u8 = 0x24;
    pub const CARDBUS_CIS_PTR: u8 = 0x28;
    pub const SUBSYSTEM_VENDOR_ID: u8 = 0x2C;
    pub const SUBSYSTEM_ID: u8 = 0x2E;
    pub const EXP_ROM_BASE_ADDR: u8 = 0x30;
    pub const CAPABILITIES_PTR: u8 = 0x34;
    pub const RESERVED0: u8 = 0x35;
    pub const RESERVED1: u8 = 0x38;
    pub const INT_LINE: u8 = 0x3C;
    pub const INT_PIN: u8 = 0x3D;
    pub const MIN_GNT: u8 = 0x3E;
    pub const MAX_LAT: u8 = 0x3F;
}

/// PCI 命令寄存器位
pub mod command {
    pub const IO_SPACE: u16 = 0x0001;
    pub const MEMORY_SPACE: u16 = 0x0002;
    pub const BUS_MASTER: u16 = 0x0004;
    pub const SPECIAL_CYCLES: u16 = 0x0008;
    pub const MEM_WR_INV: u16 = 0x0010;
    pub const VGA_PALETTE: u16 = 0x0020;
    pub const PARITY_ERR_RESP: u16 = 0x0040;
    pub const SERR_ENABLE: u16 = 0x0080;
    pub const FAST_BACK_TO_BACK: u16 = 0x0100;
    pub const INT_DISABLE: u16 = 0x0400;
}

/// PCI 状态寄存器位
pub mod status {
    pub const CAPABILITIES_LIST: u16 = 0x0010;
    pub const IRQ_STATUS: u16 = 0x0008;
    pub const CAPABILITIES_LIST_66MHZ: u16 = 0x0200;
    pub const FAST_BACK_TO_BACK: u16 = 0x0080;
    pub const DEVSEL_TIMING: u16 = 0x0600;
}

/// PCI BAR 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BARType {
    MemoryMapped,
    IOMapped,
    None,
}

/// PCI Base Address Register (BAR)
#[derive(Debug, Clone, Copy)]
pub struct PCIBAR {
    pub base_addr: u64,
    pub size: u64,
    pub bar_type: BARType,
    pub prefetchable: bool,
}

impl PCIBAR {
    pub const fn empty() -> Self {
        Self {
            base_addr: 0,
            size: 0,
            bar_type: BARType::None,
            prefetchable: false,
        }
    }
}

/// PCI 配置空间访问结构体
#[derive(Debug, Clone, Copy)]
pub struct PCIConfig {
    pub base_addr: u64,
}

impl PCIConfig {
    /// 创建新的 PCI 配置空间访问
    pub const fn new(base_addr: u64) -> Self {
        Self { base_addr }
    }

    /// 读取 32 位配置空间寄存器
    pub fn read_config_dword(&self, offset: u8) -> u32 {
        unsafe {
            let ptr = (self.base_addr + offset as u64) as *const u32;
            core::ptr::read_volatile(ptr)
        }
    }

    /// 写入 32 位配置空间寄存器
    pub fn write_config_dword(&self, offset: u8, value: u32) {
        unsafe {
            let ptr = (self.base_addr + offset as u64) as *mut u32;
            core::ptr::write_volatile(ptr, value);
        }
    }

    /// 读取 16 位配置空间寄存器
    pub fn read_config_word(&self, offset: u8) -> u16 {
        unsafe {
            let ptr = (self.base_addr + offset as u64) as *const u16;
            core::ptr::read_volatile(ptr)
        }
    }

    /// 读取 8 位配置空间寄存器
    pub fn read_config_byte(&self, offset: u8) -> u8 {
        unsafe {
            let ptr = (self.base_addr + offset as u64) as *const u8;
            core::ptr::read_volatile(ptr)
        }
    }

    /// 读取 BAR
    pub fn read_bar(&self, bar_index: u8) -> PCIBAR {
        if bar_index > 5 {
            return PCIBAR::empty();
        }

        let bar_offset = offset::BAR0 + (bar_index * 4);
        let bar_value = self.read_config_dword(bar_offset);

        // 判断 BAR 类型
        if bar_value & 0x00000001 == 0x00000001 {
            // I/O mapped BAR
            PCIBAR {
                base_addr: (bar_value & 0xFFFFFFFC) as u64,
                size: 0,
                bar_type: BARType::IOMapped,
                prefetchable: false,
            }
        } else {
            // Memory mapped BAR
            let base = bar_value & 0xFFFFFFF0;
            PCIBAR {
                base_addr: base as u64,
                size: 0,
                bar_type: BARType::MemoryMapped,
                prefetchable: (bar_value & 0x00000008) != 0,
            }
        }
    }

    /// 获取厂商 ID
    pub fn vendor_id(&self) -> u16 {
        self.read_config_word(offset::VENDOR_ID)
    }

    /// 获取设备 ID
    pub fn device_id(&self) -> u16 {
        self.read_config_word(offset::DEVICE_ID)
    }

    /// 获取类代码
    pub fn class_code(&self) -> u8 {
        self.read_config_byte(offset::CLASS)
    }

    /// 获取子类代码
    pub fn subclass(&self) -> u8 {
        self.read_config_byte(offset::SUBCLASS)
    }

    /// 获取编程接口
    pub fn prog_if(&self) -> u8 {
        self.read_config_byte(offset::PROG_IF)
    }

    /// 获取修订 ID
    pub fn revision_id(&self) -> u8 {
        self.read_config_byte(offset::REVISION)
    }

    /// 获取中断引脚
    pub fn interrupt_pin(&self) -> u8 {
        self.read_config_byte(offset::INT_PIN)
    }

    /// 获取中断线
    pub fn interrupt_line(&self) -> u8 {
        self.read_config_byte(offset::INT_LINE)
    }

    /// 设置命令寄存器
    pub fn set_command(&self, cmd: u16) {
        self.write_config_dword(offset::COMMAND, cmd as u32);
    }

    /// 获取命令寄存器
    pub fn command(&self) -> u16 {
        self.read_config_word(offset::COMMAND)
    }

    /// 使能总线 mastering
    pub fn enable_bus_master(&self) {
        let current = self.command();
        self.set_command(current | command::BUS_MASTER | command::MEMORY_SPACE);
    }
}

/// 已知厂商 ID
pub mod vendor {
    pub const RED_HAT: u16 = 0x1AF4;  // QEMU VirtIO 厂商
}

/// VirtIO 设备 ID (PCI)
pub mod virtio_device {
    pub const VIRTIO_NET: u16 = 0x1000;  // VirtIO 网络设备
    pub const VIRTIO_BLK: u16 = 0x1001;  // VirtIO 块设备 (Legacy)
    pub const VIRTIO_BLK_MODERN: u16 = 0x1042;  // VirtIO 块设备 (Modern/Transitional)
}

/// RISC-V PCIe ECAM 基地址
#[cfg(feature = "riscv64")]
pub const RISCV_PCIE_ECAM_BASE: u64 = 0x30000000;

/// PCIe ECAM 配置空间大小
pub const PCIE_ECAM_SIZE: u64 = 0x1000;

/// 枚举 PCI 总线上的 VirtIO 设备
///
/// # 返回
/// 返回找到的 VirtIO 设备数量
pub fn enumerate_virtio_devices() -> usize {
    crate::println!("pci: Scanning PCIe bus for VirtIO devices...");

    #[cfg(feature = "riscv64")]
    {
        let mut device_count = 0;

        // RISC-V: 扫描 PCIe ECAM 空间
        // QEMU virt 平台: PCIe ECAM 从 0x30000000 开始
        const MAX_DEVICES: u8 = 32;

        for device in 0..MAX_DEVICES {
            let ecam_addr = RISCV_PCIE_ECAM_BASE + (device as u64 * PCIE_ECAM_SIZE);
            let config = PCIConfig::new(ecam_addr);

            let vendor_id = config.vendor_id();
            let device_id = config.device_id();

            // 检查设备是否存在
            if vendor_id == 0xFFFF {
                continue;
            }

            // 检查是否为 VirtIO 设备 (Red Hat)
            if vendor_id == vendor::RED_HAT {
                crate::println!("pci: Found VirtIO device: vendor=0x{:04x}, device=0x{:04x} at slot {}",
                    vendor_id, device_id, device);

                // 识别 VirtIO 设备类型
                match device_id {
                    virtio_device::VIRTIO_BLK => {
                        crate::println!("pci:   VirtIO-Blk device detected");
                        device_count += 1;
                    }
                    virtio_device::VIRTIO_NET => {
                        crate::println!("pci:   VirtIO-Net device detected");
                        device_count += 1;
                    }
                    _ => {
                        crate::println!("pci:   VirtIO device (ID=0x{:04x}), type not supported", device_id);
                    }
                }

                // 打印设备信息
                let class = config.class_code();
                let subclass = config.subclass();
                let prog_if = config.prog_if();
                let irq_pin = config.interrupt_pin();

                crate::println!("pci:   Class: 0x{:02x}, Subclass: 0x{:02x}, Prog IF: 0x{:02x}",
                    class, subclass, prog_if);
                crate::println!("pci:   IRQ Pin: {}", irq_pin);

                // 读取 BAR
                for bar_idx in 0..6 {
                    let bar = config.read_bar(bar_idx);
                    if bar.bar_type != BARType::None {
                        crate::println!("pci:   BAR{}: base=0x{:x}, size=0x{:x}, type={:?}",
                            bar_idx, bar.base_addr, bar.size, bar.bar_type);
                    }
                }
            }
        }

        crate::println!("pci: Scan completed, found {} VirtIO device(s)", device_count);
        device_count
    }

    #[cfg(not(feature = "riscv64"))]
    {
        crate::println!("pci: PCI not supported on this platform");
        0
    }
}
