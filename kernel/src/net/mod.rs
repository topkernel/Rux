//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 网络子系统
//!
//! 完全遵循 Linux 内核的网络子系统设计
//! 参考: net/

pub mod buffer;
pub mod ethernet;
pub mod arp;
pub mod ipv4;
pub mod udp;
pub mod tcp;

pub use buffer::{
    SkBuff, PacketType, EthProtocol, IpProtocol,
    alloc_skb, kfree_skb,
};

// Socket 层 (待实现)
// pub mod socket;
