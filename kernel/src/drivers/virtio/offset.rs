//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO PCI 寄存器偏移

/// VirtIO Common CFG 寄存器偏移（Modern VirtIO 1.0+）
pub const DEVICE_FEATURES: u32 = 0x000;
pub const DRIVER_FEATURES: u32 = 0x004;
/// Modern VirtIO 队列选择
pub const COMMON_CFG_QUEUE_SELECT: u32 = 0x014;
/// Modern VirtIO 队列大小
pub const COMMON_CFG_QUEUE_SIZE: u32 = 0x018;
/// Modern VirtIO 队列描述符表地址（低32位）
pub const COMMON_CFG_QUEUE_DESC_LO: u32 = 0x028;
/// Modern VirtIO 队列描述符表地址（高32位）
pub const COMMON_CFG_QUEUE_DESC_HI: u32 = 0x02C;
/// Modern VirtIO 队列驱动环地址（低32位，Available Ring）
pub const COMMON_CFG_QUEUE_DRIVER_LO: u32 = 0x030;
/// Modern VirtIO 队列驱动环地址（高32位，Available Ring）
pub const COMMON_CFG_QUEUE_DRIVER_HI: u32 = 0x034;
/// Modern VirtIO 队列设备环地址（低32位，Used Ring）
pub const COMMON_CFG_QUEUE_DEVICE_LO: u32 = 0x038;
/// Modern VirtIO 队列设备环地址（高32位，Used Ring）
pub const COMMON_CFG_QUEUE_DEVICE_HI: u32 = 0x03C;
/// Modern VirtIO 队列就绪使能
pub const COMMON_CFG_QUEUE_ENABLE: u32 = 0x044;
pub const DEVICE_STATUS: u32 = 0x014;

/// VirtIO Interrupt CFG 寄存器偏移
pub const INTERRUPT_STATUS: u32 = 0x000;
pub const INTERRUPT_ACK: u32 = 0x004;

/// VirtIO Notify CFG 寄存器偏移
pub const QUEUE_NOTIFY: u32 = 0x000;

/// VirtIO 设备状态位
pub mod status {
    pub const ACKNOWLEDGE: u32 = 0x01;
    pub const DRIVER: u32 = 0x02;
    pub const FAILED: u32 = 0x80;
    pub const FEATURES_OK: u32 = 0x08;
    pub const DRIVER_OK: u32 = 0x04;
    pub const DEVICE_NEEDS_RESET: u32 = 0x40;
}
