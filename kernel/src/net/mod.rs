//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network Subsystem

// The subsystem's ONE audited unsafe file (mainline minimal-first
// consolidation, 2026-11-27): file named `unsafe.rs` per the mainline
// naming decision, module named `edges` per the sibling convention
// (fs/unsafe.rs, process/unsafe.rs, ipc/unsafe.rs, syscall/edges.rs).
// The #[path] keeps the FILE name the mainline gate requires while
// call sites stay on the plain `edges` identifier.
#[path = "unsafe.rs"]
pub mod edges;
pub mod buffer;
pub mod ethernet;
pub mod arp;
pub mod icmp;
pub mod ipv4;
pub mod ipv6;
pub mod udp;
pub mod tcp;
pub mod tcp_timer;
pub mod socket;
pub mod unix;
pub mod netlink;
pub mod raw;

pub use buffer::{
    SkBuff, PacketType, EthProtocol, IpProtocol,
    alloc_skb, kfree_skb,
};

pub use socket::{
    Socket, SocketType, SocketState, RecvPacket,
    SockAddrIn, AF_INET, SOCK_STREAM, SOCK_DGRAM, IPPROTO_TCP, IPPROTO_UDP,
    get_socket, get_socket_from_fd,
};

pub use ipv6::{AF_INET6, IpAddr};

// ==================== Socket-file Arc edges (round-5) ====================

/// Recover a strong `Arc<T>` from a socket File's private_data slot —
/// the ONE audited Arc-recovery edge for the net family (INV-NETARC-1).
///
/// INV-NETARC-1 (provenance + liveness): the File's private_data holds
// file_arc / take_file_arc (INV-NETARC-1) moved into the audited
// net edges file (unsafe.rs, 2026-11-27 consolidation); the re-export
// below keeps every `crate::net::file_arc` call site stable.
pub use edges::{file_arc, take_file_arc};
