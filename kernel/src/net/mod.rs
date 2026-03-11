//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 网络子系统
//!
//! 完全...
//! 参考: net/

pub mod buffer;
pub mod ethernet;
pub mod arp;
pub mod ipv4;
pub mod udp;
pub mod tcp;
pub mod tcp_timer;
pub mod socket;

pub use buffer::{
    SkBuff, PacketType, EthProtocol, IpProtocol,
    alloc_skb, kfree_skb,
};

// 导出 socket 相关类型
pub use socket::{
    Socket, SocketType, SocketState, RecvPacket,
    SockAddrIn, AF_INET, SOCK_STREAM, SOCK_DGRAM, IPPROTO_TCP, IPPROTO_UDP,
    get_socket, get_socket_from_fd,
};
