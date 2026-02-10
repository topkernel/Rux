//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 网络设备驱动
//!
//! 完全遵循 Linux 内核的网络设备驱动设计
//! 参考: drivers/net/

pub mod space;
pub mod loopback;

pub use space::{
    NetDevice, NetDeviceOps, DeviceStats,
    ArpHrdType, dev_flags,
    register_netdevice, unregister_netdevice,
    get_netdevice_by_index, get_netdevice_by_name,
    get_netdevice_count,
};

pub use loopback::{
    loopback_init, get_loopback_device, loopback_send,
};

// VirtIO 网络设备驱动 (待实现)
// pub mod virtio_net;
