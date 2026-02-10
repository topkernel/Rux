//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 网络子系统
//!
//! 完全遵循 Linux 内核的网络子系统设计
//! 参考: net/

pub mod buffer;

pub use buffer::{
    SkBuff, PacketType, EthProtocol, IpProtocol,
    alloc_skb, kfree_skb,
};

// Socket 层 (待实现)
// pub mod socket;

// 以太网层 (待实现)
// pub mod ethernet;

// ARP 协议 (待实现)
// pub mod arp;

// IPv4 协议 (待实现)
// pub mod ipv4;

// TCP 协议 (待实现)
// pub mod tcp;

// UDP 协议 (待实现)
// pub mod udp;
