//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv4 Protocol

pub mod route;
pub mod checksum;
mod defrag;

use crate::net::buffer::SkBuff;
use crate::net::ethernet::ETH_ALEN;

/// W3: IPv4 identification counter for transmitted (fragmented) packets.
static IP_ID_COUNTER: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// IPv4 address length
pub const IP_ALEN: usize = 4;

/// IPv4 header length
pub const IPHDR_LEN: usize = 20;

/// IPv4 minimum MTU
pub const IP_MIN_MTU: u16 = 68;

/// IPv4 maximum MTU
pub const IP_MAX_MTU: u16 = 65535;

/// IPv4 default TTL (using configuration value)
pub use crate::config::IP_DEFAULT_TTL;

/// IPv4 fragment flags
pub mod ip_frag_flags {
    /// Reserved bit
    pub const RB: u16 = 0x8000;
    /// Don't Fragment
    pub const DF: u16 = 0x4000;
    /// More Fragments
    pub const MF: u16 = 0x2000;
    /// Fragment offset mask
    pub const OFFSET_MASK: u16 = 0x1FFF;
}

/// IPv4 header
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IpHdr {
    /// Version (4 bits) + Header length (4 bits)
    pub version_ihl: u8,
    /// Type of service
    pub tos: u8,
    /// Total length
    pub tot_len: u16,
    /// Identification
    pub id: u16,
    /// Fragment flags + Fragment offset
    pub frag_off: u16,
    /// TTL
    pub ttl: u8,
    /// Protocol
    pub protocol: u8,
    /// Header checksum
    pub check: u16,
    /// Source IP address
    pub saddr: u32,
    /// Destination IP address
    pub daddr: u32,
}

impl IpHdr {
    /// Create IP header from byte slice (the honest borrow rides
    /// INH-BUF-3 on the shared header-view edge — no invented
    /// lifetimes; round-5 also fixed the old `&'static Self` lie).
    pub fn from_bytes(data: &[u8]) -> Option<&Self> {
        crate::net::buffer::hdr_view::<IpHdr>(data)
    }

    /// Calculate checksum
    pub fn compute_checksum(&self) -> u16 {
        let header = crate::safety::bytes::bytes_of(self);
        checksum::ip_checksum(&header[..IPHDR_LEN])
    }

    /// Verify checksum over the first 20 header bytes. Only correct for
    /// IHL == 5 packets — the RX path (ip_rcv) verifies over IHL*4 bytes;
    /// this helper remains for the (option-less) TX build path.
    pub fn is_valid_checksum(&self) -> bool {
        self.compute_checksum() == 0
    }
}

/// Build IPv4 header
///
/// # Arguments
/// - `skb`: SkBuff
/// - `saddr`: Source IP address (network byte order)
/// - `daddr`: Destination IP address (network byte order)
/// - `protocol`: Protocol type
/// - `tot_len`: Total length
/// - `ttl`: Time To Live (P2 IP_TTL; 0 = system default IP_DEFAULT_TTL)
///
/// # Notes
/// Adds IPv4 header at the front of SkBuff
pub fn ip_push_header(
    skb: &mut SkBuff,
    saddr: u32,
    daddr: u32,
    protocol: u8,
    tot_len: u16,
    ttl: u8,
) -> Result<(), ()> {
    skb.push_hdr::<IpHdr>(|ip_hdr| {
        ip_hdr.version_ihl = (4 << 4) | 5;
        ip_hdr.tos = 0;
        ip_hdr.tot_len = tot_len.to_be();
        ip_hdr.id = 0;
        ip_hdr.frag_off = 0;
        // P2 IP_TTL: the socket's per-connection TTL (0 = default).
        ip_hdr.ttl = if ttl == 0 { IP_DEFAULT_TTL } else { ttl };
        ip_hdr.protocol = protocol;
        ip_hdr.check = 0;
        ip_hdr.saddr = saddr.to_be();
        ip_hdr.daddr = daddr.to_be();
        ip_hdr.check = ip_hdr.compute_checksum().to_be();
    })
    .ok_or(())?;

    Ok(())
}

/// Send IPv4 packet (for upper layer protocols)
///
/// # Arguments
/// - `skb`: SkBuff (containing TCP/UDP or other upper layer protocol data)
/// - `dest_ip`: Destination IP address
/// - `protocol`: Upper layer protocol number (IPPROTO_TCP = 6, IPPROTO_UDP = 17)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
/// Send with the socket's source address (0 = device address).
/// Transport layers must pass their bound local_ip — a fixed device
/// address broke 4-tuple matching for any non-device source (loopback in
/// particular; found via the nettest E2E run, NET-H5 family).
pub fn ipv4_send_src(skb: SkBuff, src_ip: u32, dest_ip: u32, protocol: u8) -> Result<(), ()> {
    ipv4_send_src_ttl(skb, src_ip, dest_ip, protocol, 0)
}

/// ipv4_send_src with an explicit TTL (P2 IP_TTL): `ttl` 0 uses the system
/// default (64); the socket layers pass their mirrored per-socket value.
pub fn ipv4_send_src_ttl(
    mut skb: SkBuff,
    src_ip: u32,
    dest_ip: u32,
    protocol: u8,
    ttl: u8,
) -> Result<(), ()> {
    let total_len = skb.len as usize; // includes the pushed header
    if total_len > u16::MAX as usize {
        return Err(()); // Packet too large for IPv4
    }
    skb.push_hdr::<IpHdr>(|ip_hdr| {
        ip_hdr.version_ihl = 0x45;
        ip_hdr.tos = 0;
        ip_hdr.tot_len = (total_len as u16).to_be();
        // W3: assign a datagram ID (required for fragment correlation; the
        // old code always sent 0, which collides on reassembly peers).
        ip_hdr.id = (IP_ID_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed) as u16).to_be();
        ip_hdr.frag_off = 0;
        // P2 IP_TTL: per-socket value (0 = system default).
        ip_hdr.ttl = if ttl == 0 { IP_DEFAULT_TTL } else { ttl };
        ip_hdr.protocol = protocol;
        let src = if src_ip == 0 {
            crate::net::arp::get_local_ip()
        } else {
            src_ip
        };
        ip_hdr.saddr = src.to_be();
        ip_hdr.daddr = dest_ip.to_be();
        ip_hdr.check = 0;
        let hdr_bytes = crate::safety::bytes::bytes_of(ip_hdr);
        ip_hdr.check = checksum::ip_checksum(&hdr_bytes[..IPHDR_LEN]).to_be();
    })
    .ok_or(())?;

    Ok(())
}


pub fn ip_output(skb: SkBuff) -> Result<(), ()> {
    crate::net::ethernet::ethernet_send(skb)
}

/// Receive and process IPv4 packet
///
/// # Arguments
/// - `skb`: SkBuff (containing IP packet)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn ip_rcv(skb: &mut SkBuff) -> Result<(), ()> {
    // Copy the wire header out by value (round-5: the old raw slice
    // stayed alive across the skb.len trim below — a borrow-tied view
    // cannot express that, which is the point).
    let ip_hdr: IpHdr = {
        let data = skb.data_all();
        *crate::net::buffer::hdr_view::<IpHdr>(data).ok_or(())?
    };

    // net:pkt_drop (r13 hunt): every silent early-return below is a
    // candidate for the vanishing-datagram class — record the skb and
    // the reason so a strand boot names the exact gate.
    let drop = |reason: u8| {
        crate::trace::trace_net_pkt_drop(skb.data as u64, 1, reason, 0, 0);
    };

    let version = ip_hdr.version_ihl >> 4;
    if version != 4 {
        drop(1);
        return Ok(());
    }

    // The internet checksum covers IHL*4 header bytes (RFC 791 §3.1 —
    // options included). The old fixed 20-byte verification failed every
    // well-formed packet carrying options (record route, LSRR...), so any
    // options-bearing datagram was silently dropped at this gate.
    let ihl = ip_hdr.version_ihl & 0x0F;
    if ihl < 5 {
        drop(2);
        return Ok(());
    }
    let hdr_len = (ihl as usize) * 4;
    {
        let wire = skb.data_all();
        if hdr_len > wire.len() {
            drop(3);
            return Ok(());
        }
        if checksum::ip_checksum(&wire[..hdr_len]) != 0 {
            drop(4);
            return Ok(());
        }
    }

    // Trim skb to IP-reported length (matches the reference ip_rcv:
    // skb_trim(skb, ntohs(iph->tot_len)))
    let ip_total_len = u16::from_be(ip_hdr.tot_len) as u32;
    if ip_total_len < 20 || ip_total_len > skb.len {
        drop(5); // Invalid tot_len
        return Ok(());
    }
    skb.len = ip_total_len;

    let src_ip = u32::from_be(ip_hdr.saddr);
    let dest_ip = u32::from_be(ip_hdr.daddr);

    // Advance skb past IP header so upper layers see only the transport payload
    // Validate IHL against packet length (per the reference ip_rcv)
    if hdr_len > ip_total_len as usize {
        drop(6);
        return Ok(());
    }

    // W3: fragment handling — a fragment (MF set or offset > 0) goes to
    // reassembly; the reassembled datagram re-enters dispatch below.
    // The old code parsed the transport header out of every fragment
    // (garbage for offset > 0 — UDP length checks happened to drop them).
    let frag_raw = u16::from_be(ip_hdr.frag_off);
    let mf = (frag_raw & ip_frag_flags::MF) != 0;
    let frag_off = ((frag_raw & ip_frag_flags::OFFSET_MASK) as usize) * 8;
    if mf || frag_off > 0 {
        // The payload slice [hdr_len, ip_total_len) is in-bounds
        // (checked above) — the safe clamped view.
        let payload = skb.data_slice_at(hdr_len as u32, ip_total_len - hdr_len as u32);
        defrag::ip_defrag(&ip_hdr, payload, frag_off as u32, mf, src_ip, dest_ip);
        return Ok(());
    }

    ip_dispatch(skb, &ip_hdr, src_ip, dest_ip);

    Ok(())
}

/// Protocol dispatch on a de-fragmented, header-validated packet: pull the
/// IP header and hand the payload to TCP/UDP/ICMP.
///
/// W3: extracted from ip_rcv so the reassembly completion path can re-enter
/// it with a rebuilt datagram.
pub fn ip_dispatch(skb: &mut SkBuff, ip_hdr: &IpHdr, src_ip: u32, dest_ip: u32) {
    let ihl = ip_hdr.version_ihl & 0x0F;
    let hdr_len = (ihl as usize) * 4;
    if hdr_len as u32 > skb.len {
        return;
    }
    // Advance past the IP header (ihl >= 5 was validated at parse;
    // skb_pull owns the pointer arithmetic).
    skb.skb_pull(hdr_len as u32);

    match ip_hdr.protocol {
        6 => {
            let _ = crate::net::tcp::tcp_rcv(skb, src_ip, dest_ip);
        }
        17 => {
            let _ = crate::net::udp::udp_rcv(skb, src_ip, dest_ip);
        }
        1 => {
            let _ = crate::net::icmp::icmp_rcv(skb, src_ip, dest_ip);
        }
        _ => {
        }
    }
}


/// Convenience wrapper: source = device address.
pub fn ipv4_send(skb: SkBuff, dest_ip: u32, protocol: u8) -> Result<(), ()> {
    ipv4_send_src(skb, 0, dest_ip, protocol)
}
