//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The net subsystem's type definitions (seed-core-design §2c.3).
//!
//! Definitions only. Every `struct` and `enum` of the subsystem lives
//! here; every `impl` block lives in the topic file beside the functions
//! that use it (buffer.rs today, a section of `hotpath.rs` after step 2)
//! — an impl is code, a definition is data layout.
//!
//! No constant lives here: none of these definitions names one.
//!
//! This file holds no `unsafe`: the definitions are plain data layout
//! (the raw-pointer fields of `SkBuff` are safe to declare; every
//! dereference of them is an audited edge of `buffer.rs`/`unsafe.rs`).
//! A field that was private to its old file would be `pub(super)` here;
//! none was — all four definitions and all their fields were `pub`.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this file must stay Safe Rust.
#![forbid(unsafe_code)]

/// Packet types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// Packet sent to this host
    Host = 0,
    /// Packet sent to another host
    Otherhost = 1,
    /// Broadcast packet
    Broadcast = 2,
    /// Multicast packet
    Multicast = 3,
}

/// Ethernet protocol types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum EthProtocol {
    /// IPv4
    ETH_P_IP = 0x0800,
    /// ARP
    ETH_P_ARP = 0x0806,
    /// IPv6
    ETH_P_IPV6 = 0x86DD,
    /// 802.1Q VLAN
    ETH_P_8021Q = 0x8100,
}

/// IP protocol types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum IpProtocol {
    /// IP
    IPPROTO_IP = 0,
    /// ICMP
    IPPROTO_ICMP = 1,
    /// TCP
    IPPROTO_TCP = 6,
    /// UDP
    IPPROTO_UDP = 17,
    /// IPv6
    IPPROTO_IPV6 = 41,
}

/// Network buffer (SkBuff)
///
/// # Memory Layout
/// ```text
/// |<- head                 ->|<- data       ->|<- tail ->|<- end ->|
/// |  (headroom)             |  (actual data) | (tailroom) |
/// ```
#[repr(C)]
pub struct SkBuff {
    /// Protocol type (ETH_P_IP, ETH_P_ARP, etc.)
    pub protocol: u16,
    /// Packet length
    pub len: u32,
    /// Data pointer (points to current protocol layer data start)
    pub data: *mut u8,
    /// Tail pointer (points to data end)
    pub tail: *mut u8,
    /// Buffer end pointer
    pub end: *mut u8,
    /// Buffer start pointer
    pub head: *mut u8,
    /// Packet type
    pub pkt_type: PacketType,
    /// Timestamp
    pub tstamp: u64,
    /// MAC address length (for Ethernet)
    pub mac_len: u8,
    /// MAC header pointer
    pub mac_header: *mut u8,
    /// Network layer header pointer
    pub network_header: *mut u8,
    /// Transport layer header pointer
    pub transport_header: *mut u8,
}
