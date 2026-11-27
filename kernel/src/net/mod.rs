//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network Subsystem

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
/// an `Arc::into_raw` pointer installed by the socket's own file-ops
/// installer, and the caller has ALREADY identity-checked the file
/// against that ops table (`*file.ops.get()` equality — the per-family
/// `*_socket_from_fd` checks). The File's lifetime keeps the Arc alive
/// (close() is the only drop path); increment-before-clone keeps the
/// returned Arc independent of the slot.
// The subsystem's ONE audited unsafe file (mainline minimal-first
// consolidation, 2026-11-27): module named `edges` per the sibling
// convention; the #[path] keeps the FILE name the gate requires.
#[path = "unsafe.rs"]
pub(crate) mod edges;

pub(crate) fn file_arc<T>(file: &crate::fs::file::File) -> Option<alloc::sync::Arc<T>> {
    let ptr = file.private_data_ptr()? as *const T;
    // INV-NETARC-1 (live Arc::into_raw value of this type, ops identity
    // checked by the caller; the increment balances the from_raw
    // construction). Edge lives in the audited unsafe.rs.
    crate::net::edges::arc_increment_strong(ptr);
    Some(crate::net::edges::arc_from_raw(ptr))
}

/// Take the Arc OUT of a closing socket File's private_data slot and
/// drop it (the close path — INV-NETARC-1 + the caller's ops identity
/// check; the slot is cleared under the file's own close serialization).
pub(crate) fn take_file_arc<T>(file: &crate::fs::file::File) -> Option<()> {
    let ptr = file.private_data_ptr()? as *const T;
    // INV-NETARC-1 (reconstructing the stored Arc consumes the slot's
    // single reference; close is the last user of the file). Edge in
    // the audited unsafe.rs.
    drop(crate::net::edges::arc_from_raw(ptr));
    file.clear_private_data();
    Some(())
}
