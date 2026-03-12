//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network device driver

pub mod space;
pub mod loopback;
pub mod virtio_net;

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

pub use virtio_net::{
    init as virtio_net_init,
    get_device as get_virtio_net_device,
    get_net_device as get_virtio_net_device_net,
};
