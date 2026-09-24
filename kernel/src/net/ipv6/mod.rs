//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv6 Protocol (P1 minimal stack)
//!
//! Scope:
//! - 40-byte base header parse/build (RFC 8200)
//! - SLAAC-style link-local address (fe80::/64 + EUI-64 from the device MAC)
//! - Neighbor Discovery replaces ARP (NS/NA live in icmpv6.rs; the neighbor
//!   cache lives here)
//! - Upper-layer dispatch to UDP/TCP/ICMPv6 with the RFC 8200 §8.1
//!   pseudo-header checksum (128-bit addresses)
//! - v4-mapped (::ffff:x.x.x.x) translation helpers for the dual-stack
//!   socket layer

pub mod icmpv6;

use crate::net::buffer::SkBuff;
use crate::sync::spinlock::Spinlock;
use crate::drivers::timer::HZ;

/// IPv6 address (wire/network byte order)
pub type Ipv6Addr = [u8; 16];

/// Address family: AF_INET6 (Linux value 10)
pub const AF_INET6: i32 = 10;

/// IPv6 base header length
pub const IPV6_HDR_LEN: usize = 40;

/// Next Header values used by this stack
pub mod next_header {
    /// Hop-by-Hop options
    pub const HOPBYHOP: u8 = 0;
    /// TCP
    pub const TCP: u8 = 6;
    /// UDP
    pub const UDP: u8 = 17;
    /// ICMPv6
    pub const ICMPV6: u8 = 58;
    /// No next header
    pub const NONE: u8 = 59;
}

/// Default hop limit (RFC 8200 recommends 64 for source)
pub const IPV6_DEFAULT_HOP_LIMIT: u8 = 64;

/// Unspecified address (::)
pub const IPV6_ADDR_UNSPECIFIED: Ipv6Addr = [0u8; 16];

/// Loopback address (::1)
pub const IPV6_ADDR_LOOPBACK: Ipv6Addr = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
];

/// All-nodes link-local multicast (ff02::1)
pub const IPV6_ADDR_ALL_NODES: Ipv6Addr = [
    0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
];

/// All-routers link-local multicast (ff02::2)
pub const IPV6_ADDR_ALL_ROUTERS: Ipv6Addr = [
    0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2,
];

// ============================================================================
// Address predicates / conversions
// ============================================================================

/// IP address family union used across the VFS socket layer: a v4 address,
/// or a pure v6 address. v4-mapped v6 addresses are normalized to
/// `IpAddr::V4` at the syscall boundary so the protocol layers keep their
/// u32 fast path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpAddr {
    /// IPv4 (host byte order)
    V4(u32),
    /// IPv6 (network byte order)
    V6(Ipv6Addr),
}

impl IpAddr {
    /// v4 view (None for a pure v6 address)
    pub fn as_v4(&self) -> Option<u32> {
        match self {
            IpAddr::V4(v) => Some(*v),
            IpAddr::V6(_) => None,
        }
    }

    /// v6 view: pure v6 stays, v4 becomes v4-mapped (::ffff:a.b.c.d)
    pub fn as_v6(&self) -> Ipv6Addr {
        match self {
            IpAddr::V6(v) => *v,
            IpAddr::V4(v) => v4_to_mapped(*v),
        }
    }

    /// True when this is a pure (non-v4-mapped) v6 address
    pub fn is_pure_v6(&self) -> bool {
        matches!(self, IpAddr::V6(_))
    }
}

/// Build the v4-mapped representation ::ffff:a.b.c.d (network byte order
/// layout, i.e. bytes 10..11 == 0xff 0xff followed by the 4 v4 bytes).
pub fn v4_to_mapped(v4: u32) -> Ipv6Addr {
    let mut a = [0u8; 16];
    a[10] = 0xff;
    a[11] = 0xff;
    a[12] = (v4 >> 24) as u8;
    a[13] = (v4 >> 16) as u8;
    a[14] = (v4 >> 8) as u8;
    a[15] = v4 as u8;
    a
}

/// Extract the v4 address from a ::ffff:a.b.c.d address (host byte order),
/// or None when `a` is a pure v6 address.
pub fn v6_to_v4_mapped(a: &Ipv6Addr) -> Option<u32> {
    if a[0..10].iter().all(|&b| b == 0) && a[10] == 0xff && a[11] == 0xff {
        Some(
            ((a[12] as u32) << 24)
                | ((a[13] as u32) << 16)
                | ((a[14] as u32) << 8)
                | (a[15] as u32),
        )
    } else {
        None
    }
}

/// :: check
pub fn is_unspecified(a: &Ipv6Addr) -> bool {
    a.iter().all(|&b| b == 0)
}

/// fe80::/10 check
pub fn is_link_local(a: &Ipv6Addr) -> bool {
    a[0] == 0xfe && (a[1] & 0xc0) == 0x80
}

/// ff00::/8 check
pub fn is_multicast(a: &Ipv6Addr) -> bool {
    a[0] == 0xff
}

/// Solicited-node multicast address (ff02::1:ffXX:XXXX) for NDP (RFC 4291)
pub fn solicited_node_multicast(target: &Ipv6Addr) -> Ipv6Addr {
    let mut a = [0u8; 16];
    a[0] = 0xff;
    a[1] = 0x02;
    a[11] = 0x01;
    a[12] = 0xff;
    a[13] = target[13];
    a[14] = target[14];
    a[15] = target[15];
    a
}

/// Multicast IPv6 address -> Ethernet multicast MAC (33:33: + low 32 bits)
pub fn multicast_mac(a: &Ipv6Addr) -> [u8; 6] {
    [0x33, 0x33, a[12], a[13], a[14], a[15]]
}

/// SLAAC link-local address: fe80::/64 + EUI-64 from the MAC
/// (flip the U/L bit of the first byte, insert 0xff:0xfe in the middle).
pub fn eui64_link_local(mac: &[u8; 6]) -> Ipv6Addr {
    let mut a = [0u8; 16];
    a[0] = 0xfe;
    a[1] = 0x80;
    a[8] = mac[0] ^ 0x02;
    a[9] = mac[1];
    a[10] = mac[2];
    a[11] = 0xff;
    a[12] = 0xfe;
    a[13] = mac[3];
    a[14] = mac[4];
    a[15] = mac[5];
    a
}

// ============================================================================
// Link-local address (SLAAC)
// ============================================================================

/// Our link-local address, valid once ipv6_init() has run.
static LINK_LOCAL: Spinlock<Ipv6Addr> = Spinlock::new([0u8; 16]);
static LINK_LOCAL_VALID: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Get our configured link-local address (None before ipv6_init)
pub fn get_link_local() -> Option<Ipv6Addr> {
    if !LINK_LOCAL_VALID.load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    Some(*LINK_LOCAL.lock_irqsave())
}

/// Is `a` one of our own addresses (link-local, ::1, or a v4-mapped form of
/// the local IPv4 address)?
pub fn is_local_addr(a: &Ipv6Addr) -> bool {
    if *a == IPV6_ADDR_LOOPBACK {
        return true;
    }
    if let Some(our) = get_link_local() {
        if our == *a {
            return true;
        }
    }
    if let Some(v4) = v6_to_v4_mapped(a) {
        if v4 == crate::net::arp::get_local_ip() || (v4 >> 24) == 127 {
            return true;
        }
    }
    false
}

/// Initialize the IPv6 stack: derive the SLAAC link-local address from the
/// device MAC, then emit a Router Solicitation (best effort — a slirp
/// network has no routers, which is fine).
pub fn ipv6_init() {
    let mac = match crate::drivers::net::virtio_net::get_device() {
        Some(d) => d.get_mac(),
        None => return, // no NIC: no link-local
    };
    let ll = eui64_link_local(&mac);
    *LINK_LOCAL.lock_irqsave() = ll;
    LINK_LOCAL_VALID.store(true, core::sync::atomic::Ordering::Release);

    crate::pr_info!(
        "ipv6: link-local {:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x} (SLAAC EUI-64)",
        ll[0], ll[1], ll[2], ll[3], ll[4], ll[5], ll[6], ll[7]
    );

    // Optional per RFC 4861: solicit routers on startup.
    icmpv6::send_rs();
}

// ============================================================================
// Neighbor cache (NDP replacement for ARP)
// ============================================================================

/// Neighbor cache capacity
const NEIGHBOR_CACHE_SIZE: usize = 32;

/// One NDP neighbor entry
#[derive(Debug, Clone, Copy)]
pub struct NeighborEntry {
    /// Neighbor IPv6 address
    pub ip: Ipv6Addr,
    /// Neighbor link-layer address
    pub mac: [u8; 6],
    /// Jiffies of the last confirmation (NS answer / NA / RX hint)
    pub last_updated: u64,
    /// Entry in use
    pub valid: bool,
}

struct NeighborCache {
    entries: [NeighborEntry; NEIGHBOR_CACHE_SIZE],
    count: usize,
}

impl NeighborCache {
    const fn new() -> Self {
        const EMPTY: NeighborEntry = NeighborEntry {
            ip: [0u8; 16],
            mac: [0u8; 6],
            last_updated: 0,
            valid: false,
        };
        Self {
            entries: [EMPTY; NEIGHBOR_CACHE_SIZE],
            count: 0,
        }
    }
}

static NEIGHBOR_CACHE: Spinlock<NeighborCache> = Spinlock::new(NeighborCache::new());

/// Neighbor entry timeout (seconds), mirroring the ARP cache default.
const NEIGHBOR_TIMEOUT_SECS: u64 = 300;

fn jiffies_now() -> u64 {
    crate::drivers::timer::get_jiffies()
}

/// Look up a neighbor's MAC (None = unresolved)
pub fn neigh_lookup(ip: &Ipv6Addr) -> Option<[u8; 6]> {
    let cache = NEIGHBOR_CACHE.lock_irqsave();
    let now = jiffies_now();
    for e in cache.entries.iter() {
        if e.valid && &e.ip == ip {
            let expired =
                now.saturating_sub(e.last_updated) > NEIGHBOR_TIMEOUT_SECS * HZ;
            if !expired {
                return Some(e.mac);
            }
            return None;
        }
    }
    None
}

/// Add or refresh a neighbor entry (NA, NS answer, or an RX-source hint)
pub fn neigh_update(ip: Ipv6Addr, mac: [u8; 6]) {
    let mut cache = NEIGHBOR_CACHE.lock_irqsave();
    let now = jiffies_now();
    for e in cache.entries.iter_mut() {
        if e.valid && e.ip == ip {
            e.mac = mac;
            e.last_updated = now;
            return;
        }
    }
    if cache.count < NEIGHBOR_CACHE_SIZE {
        let idx = cache.count;
        cache.entries[idx] = NeighborEntry {
            ip,
            mac,
            last_updated: now,
            valid: true,
        };
        cache.count += 1;
    } else {
        // Full: replace the oldest entry
        let mut idx = 0usize;
        let mut oldest = u64::MAX;
        for (i, e) in cache.entries.iter().enumerate() {
            if e.valid && e.last_updated < oldest {
                oldest = e.last_updated;
                idx = i;
            }
        }
        cache.entries[idx] = NeighborEntry {
            ip,
            mac,
            last_updated: now,
            valid: true,
        };
    }
}

/// Snapshot of live neighbor entries (procfs / netstat)
pub fn neigh_dump() -> alloc::vec::Vec<NeighborEntry> {
    let cache = NEIGHBOR_CACHE.lock_irqsave();
    cache
        .entries
        .iter()
        .filter(|e| e.valid)
        .copied()
        .collect()
}

// ============================================================================
// Header
// ============================================================================

/// IPv6 fixed base header (40 bytes, no extension headers in this stack)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ipv6Hdr {
    /// Version (4 bits) + Traffic Class (8 bits) + Flow Label (20 bits)
    pub ver_tc_flow: u32,
    /// Payload length (header + data after the 40-byte base header)
    pub payload_len: u16,
    /// Next header (transport protocol number)
    pub next_header: u8,
    /// Hop limit
    pub hop_limit: u8,
    /// Source address
    pub saddr: Ipv6Addr,
    /// Destination address
    pub daddr: Ipv6Addr,
}

impl Ipv6Hdr {
    /// Parse from bytes (at least IPV6_HDR_LEN)
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < IPV6_HDR_LEN {
            return None;
        }
        if data[0] >> 4 != 6 {
            return None;
        }
        // SAFETY: data has at least IPV6_HDR_LEN bytes; the lifetime is
        // 'static because it aliases skb data which lives until the packet
        // is freed (same convention as IpHdr::from_bytes).
        unsafe {
            Some(&*(data.as_ptr() as *const Ipv6Hdr))
        }
    }

    /// Version nibble (host order view of ver_tc_flow)
    pub fn version(&self) -> u8 {
        (u32::from_be(self.ver_tc_flow) >> 28) as u8
    }

    /// Payload length (host order)
    pub fn payload_len(&self) -> u16 {
        u16::from_be(self.payload_len)
    }

    /// Source address (copy)
    pub fn saddr(&self) -> Ipv6Addr {
        self.saddr
    }

    /// Destination address (copy)
    pub fn daddr(&self) -> Ipv6Addr {
        self.daddr
    }
}

/// Prepend an IPv6 base header to `skb`. `payload_len` is the byte count
/// that follows this header; 0 hop_limit selects the system default.
pub fn ipv6_push_header(
    skb: &mut SkBuff,
    saddr: &Ipv6Addr,
    daddr: &Ipv6Addr,
    next_hdr: u8,
    hop_limit: u8,
) -> Result<(), ()> {
    let payload_len = skb.len as usize;
    if payload_len > u16::MAX as usize {
        return Err(());
    }
    let ptr = skb.skb_push(IPV6_HDR_LEN as u32).ok_or(())?;

    // SAFETY: skb_push returned a valid, properly aligned pointer covering
    // at least IPV6_HDR_LEN bytes; writing the repr(C) fields is
    // well-defined.
    unsafe {
        let hdr = &mut *(ptr as *mut Ipv6Hdr);
        // Version=6, traffic class 0, flow label 0.
        hdr.ver_tc_flow = (6u32 << 28).to_be();
        hdr.payload_len = (payload_len as u16).to_be();
        hdr.next_header = next_hdr;
        hdr.hop_limit = if hop_limit == 0 {
            IPV6_DEFAULT_HOP_LIMIT
        } else {
            hop_limit
        };
        hdr.saddr = *saddr;
        hdr.daddr = *daddr;
    }
    Ok(())
}

/// Receive and dispatch an IPv6 packet (called from ethernet_rcv with the
/// Ethernet header already pulled).
pub fn ipv6_rcv(skb: &mut SkBuff) -> Result<(), ()> {
    // SAFETY: skb.data/skb.len describe the packet's valid byte range.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };
    let hdr = match Ipv6Hdr::from_bytes(data) {
        Some(h) => h,
        None => return Ok(()),
    };

    let payload_len = hdr.payload_len() as usize;
    if payload_len > skb.len as usize {
        return Ok(()); // truncated frame
    }
    // Trim trailing Ethernet padding down to the IP-reported length.
    skb.len = payload_len as u32;

    let src = hdr.saddr();
    let dst = hdr.daddr();

    // Advance past the base header so the transport layer starts at its
    // own header (mirrors ip_dispatch).
    if (skb.skb_pull(IPV6_HDR_LEN as u32)).is_none() {
        return Ok(());
    }

    match hdr.next_header {
        next_header::ICMPV6 => {
            icmpv6::icmpv6_rcv(skb, &src, &dst);
        }
        next_header::UDP => {
            crate::net::udp::udp_rcv6(skb, &src, &dst);
        }
        next_header::TCP => {
            crate::net::tcp::tcp_rcv6(skb, &src, &dst);
        }
        _ => {
            // Hop-by-hop / routing / no-next-header ...: not supported.
        }
    }

    Ok(())
}

/// Send an IPv6 packet for the upper layers: prepend the base header and
/// hand to the v6 Ethernet output path. `skb` carries ONLY the transport
/// data (header + payload) when called.
pub fn ipv6_send(skb: SkBuff, saddr: &Ipv6Addr, daddr: &Ipv6Addr, next_hdr: u8) -> Result<(), ()> {
    ipv6_send_hops(skb, saddr, daddr, next_hdr, 0)
}

/// ipv6_send with an explicit hop limit (0 = system default 64).
pub fn ipv6_send_hops(
    mut skb: SkBuff,
    saddr: &Ipv6Addr,
    daddr: &Ipv6Addr,
    next_hdr: u8,
    hop_limit: u8,
) -> Result<(), ()> {
    ipv6_push_header(&mut skb, saddr, daddr, next_hdr, hop_limit)?;
    crate::net::ethernet::ethernet_send(skb)
}

// ============================================================================
// Pseudo-header checksum (RFC 8200 §8.1)
// ============================================================================

/// Transport-layer checksum over an IPv6 pseudo-header + the transport
/// segment bytes (`seg` = transport header INCLUDING the checksum field as
/// it appears on the wire or with the field zeroed for TX).
///
/// TX usage: zero the checksum field, call this, store the result.
/// RX usage: call with the segment as received; a valid segment yields 0.
///
/// The caller must keep the checksum field in whatever state it has — the
/// function is a plain sum, so including a stored checksum on RX gives the
/// standard "sum to zero" validation.
pub fn transport_checksum6(src: &Ipv6Addr, dst: &Ipv6Addr, next_hdr: u8, seg: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    // Pseudo-header: src (16B) + dst (16B) as big-endian u16 words...
    for i in (0..16).step_by(2) {
        sum += u16::from_be_bytes([src[i], src[i + 1]]) as u32;
        sum += u16::from_be_bytes([dst[i], dst[i + 1]]) as u32;
    }
    // ... upper-layer packet length (4 bytes) ...
    let len32 = seg.len() as u32;
    sum += (len32 >> 16) as u32;
    sum += (len32 & 0xFFFF) as u32;
    // ... 3 zero bytes + next header
    sum += next_hdr as u32;

    // Segment words
    let mut i = 0;
    while i + 1 < seg.len() {
        sum += u16::from_be_bytes([seg[i], seg[i + 1]]) as u32;
        i += 2;
    }
    if i < seg.len() {
        sum += (seg[i] as u32) << 8;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hdr_size() {
        assert_eq!(core::mem::size_of::<Ipv6Hdr>(), 40);
    }

    #[test]
    fn test_eui64_link_local() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let ll = eui64_link_local(&mac);
        assert!(is_link_local(&ll));
        // U/L bit flipped
        assert_eq!(ll[8], 0x50);
        assert_eq!(&ll[9..13], &[0x54, 0x00, 0xff, 0xfe]);
        assert_eq!(&ll[13..], &[0x12, 0x34, 0x56][..]);
    }

    #[test]
    fn test_v4_mapped() {
        let m = v4_to_mapped(0x0A00020F);
        assert_eq!(v6_to_v4_mapped(&m), Some(0x0A00020F));
        assert_eq!(v6_to_v4_mapped(&IPV6_ADDR_LOOPBACK), None);
    }

    #[test]
    fn test_checksum_roundtrip() {
        let src = eui64_link_local(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        let dst = IPV6_ADDR_ALL_NODES;
        let mut seg = [0u8; 12];
        seg[0] = 128; // echo request
        let csum = transport_checksum6(&src, &dst, next_header::ICMPV6, &seg);
        // Store big-endian and re-verify (sum-to-zero property).
        seg[2] = (csum >> 8) as u8;
        seg[3] = csum as u8;
        assert_eq!(transport_checksum6(&src, &dst, next_header::ICMPV6, &seg), 0);
    }

    #[test]
    fn test_solicited_node() {
        let target = eui64_link_local(&[1, 2, 3, 4, 5, 6]);
        let sn = solicited_node_multicast(&target);
        assert!(is_multicast(&sn));
        assert_eq!(sn[13..], target[13..]);
    }
}
