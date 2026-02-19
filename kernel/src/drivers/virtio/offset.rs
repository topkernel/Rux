//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! VirtIO PCI 寄存器偏移

/// VirtIO Common CFG 寄存器偏移（Modern VirtIO 1.0+）
pub const DEVICE_FEATURE_SELECT: u32 = 0;    // VIRTIO_PCI_COMMON_DFSELECT
pub const DEVICE_FEATURES: u32 = 4;          // VIRTIO_PCI_COMMON_DF
pub const DRIVER_FEATURE_SELECT: u32 = 8;    // VIRTIO_PCI_COMMON_GFSELECT
pub const DRIVER_FEATURES: u32 = 12;         // VIRTIO_PCI_COMMON_GF
pub const CONFIG_MSIX_VECTOR: u32 = 16;      // VIRTIO_PCI_COMMON_MSIX
pub const NUM_QUEUES: u32 = 18;              // VIRTIO_PCI_COMMON_NUMQ
/// 设备状态寄存器
pub const DEVICE_STATUS: u32 = 20;           // VIRTIO_PCI_COMMON_STATUS
pub const CONFIG_GENERATION: u32 = 21;       // VIRTIO_PCI_COMMON_CFGGENERATION

/// 队列相关寄存器
pub const COMMON_CFG_QUEUE_SELECT: u32 = 22; // VIRTIO_PCI_COMMON_Q_SELECT
pub const COMMON_CFG_QUEUE_SIZE: u32 = 24;   // VIRTIO_PCI_COMMON_Q_SIZE
pub const COMMON_CFG_QUEUE_MSIX_VECTOR: u32 = 26;  // VIRTIO_PCI_COMMON_Q_MSIX
pub const COMMON_CFG_QUEUE_ENABLE: u32 = 28; // VIRTIO_PCI_COMMON_Q_ENABLE
pub const COMMON_CFG_QUEUE_NOTIFY_OFF: u32 = 30;   // VIRTIO_PCI_COMMON_Q_NOFF

/// 队列地址寄存器 (64-bit, split into lo/hi)
pub const COMMON_CFG_QUEUE_DESC_LO: u32 = 32;  // VIRTIO_PCI_COMMON_Q_DESCLO
pub const COMMON_CFG_QUEUE_DESC_HI: u32 = 36;  // VIRTIO_PCI_COMMON_Q_DESCHI
pub const COMMON_CFG_QUEUE_DRIVER_LO: u32 = 40; // VIRTIO_PCI_COMMON_Q_AVAILLO
pub const COMMON_CFG_QUEUE_DRIVER_HI: u32 = 44; // VIRTIO_PCI_COMMON_Q_AVAILHI
pub const COMMON_CFG_QUEUE_DEVICE_LO: u32 = 48; // VIRTIO_PCI_COMMON_Q_USEDLO
pub const COMMON_CFG_QUEUE_DEVICE_HI: u32 = 52; // VIRTIO_PCI_COMMON_Q_USEDHI

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
