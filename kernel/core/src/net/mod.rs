//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network Subsystem

pub mod net_hooks;
pub mod file_arc;
pub mod r#unsafe;
pub mod buffer;
// pub mod ethernet; — MOVED to the net service crate (rung 2)
// pub mod arp; — MOVED to the net service crate (rung 2)
// pub mod icmp; — MOVED to the net service crate (rung 2)
// pub mod ipv4; — MOVED to the net service crate (rung 2)
// pub mod ipv6; — MOVED to the net service crate (rung 2)
// pub mod udp; — MOVED to the net service crate (rung 2)
// pub mod tcp; — MOVED to the net service crate (rung 2)
// pub mod tcp_timer; — MOVED to the net service crate (rung 2)
// pub mod socket; — MOVED to the net service crate (rung 2)
// pub mod unix; — MOVED to the net service crate (rung 2)
// pub mod netlink; — MOVED to the net service crate (rung 2)
// pub mod raw; — MOVED to the net service crate (rung 2)

pub use buffer::{
    SkBuff, PacketType, EthProtocol, IpProtocol,
    alloc_skb, kfree_skb,
};

// pub use socket::{...}; — MOVED to the net service crate (rung 2)

// pub use ipv6::{AF_INET6, IpAddr}; — MOVED to the net service crate (rung 2)

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
pub(crate) fn file_arc<T>(file: &crate::fs::file::File) -> Option<alloc::sync::Arc<T>> {
    let ptr = file.private_data_ptr()? as *const T;
    // SAFETY: INV-NETARC-1 — the pointer is a live Arc::into_raw value
    // of this type (ops identity checked by the caller); the increment
    // balances the from_raw construction below.
    unsafe {
        alloc::sync::Arc::increment_strong_count(ptr);
        Some(alloc::sync::Arc::from_raw(ptr))
    }
}

/// Take the Arc OUT of a closing socket File's private_data slot and
/// HAND IT BACK (the close paths that must USE the socket before the
/// drop — flush held data, protocol close; INV-NETARC-1 + the caller's
/// ops identity check). The cell is emptied first, making a second
/// reclaim impossible.
pub(crate) fn reclaim_file_arc<T>(file: &crate::fs::file::File) -> Option<alloc::sync::Arc<T>> {
    let ptr = file.private_data_ptr()? as *const T;
    file.clear_private_data();
    // SAFETY: INV-NETARC-1 — reconstructing consumes the slot's single
    // reference; the emptied cell makes this exactly-once.
    unsafe { Some(alloc::sync::Arc::from_raw(ptr)) }
}

/// Take the Arc OUT of a closing socket File's private_data slot and
/// drop it (the close path — INV-NETARC-1 + the caller's ops identity
/// check; the slot is cleared under the file's own close serialization).
pub(crate) fn take_file_arc<T>(file: &crate::fs::file::File) -> Option<()> {
    let ptr = file.private_data_ptr()? as *const T;
    // SAFETY: INV-NETARC-1 — reconstructing the stored Arc consumes the
    // slot's single reference (close is the last user of the file).
    unsafe {
        drop(alloc::sync::Arc::from_raw(ptr));
    }
    file.clear_private_data();
    Some(())
}
