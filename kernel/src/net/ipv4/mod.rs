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
    /// Create IP header from byte slice
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < IPHDR_LEN {
            return None;
        }

        // SAFETY: data has at least IPHDR_LEN bytes; lifetime is 'static because
        // it aliases skb data which lives until the packet is freed.
        unsafe {
            Some(&*(data.as_ptr() as *const IpHdr))
        }
    }

    /// Calculate checksum
    pub fn compute_checksum(&self) -> u16 {
        let mut header = [0u8; IPHDR_LEN];
        // SAFETY: self is a valid IpHdr; copying IPHDR_LEN bytes is safe since
        // IpHdr is repr(C) and at least IPHDR_LEN bytes in size.
        unsafe {
            core::ptr::copy_nonoverlapping(
                (self as *const IpHdr) as *const u8,
                header.as_mut_ptr(),
                IPHDR_LEN,
            );
        }

        checksum::ip_checksum(&header)
    }

    /// Verify checksum
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
    let ptr = skb.skb_push(IPHDR_LEN as u32).ok_or(())?;

    // SAFETY: skb_push returned a valid, properly aligned pointer of at least
    // IPHDR_LEN bytes; writing fields of repr(C) IpHdr is well-defined.
    unsafe {
        let ip_hdr = &mut *(ptr as *mut IpHdr);

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
    }

    Ok(())
}

/// Parse IPv4 header
///
/// # Arguments
/// - `skb`: SkBuff
///
/// # Returns
/// IP header reference, or None if parsing fails
pub fn ip_pull_header(skb: &mut SkBuff) -> Option<&'static IpHdr> {
    // SAFETY: skb.data and skb.len describe a valid byte range in the skb buffer.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < IPHDR_LEN {
        return None;
    }

    let ip_hdr = IpHdr::from_bytes(data)?;

    let version = ip_hdr.version_ihl >> 4;
    if version != 4 {
        return None;
    }

    let ihl = ip_hdr.version_ihl & 0x0F;
    if ihl < 5 {
        return None;
    }

    let header_len = (ihl as usize) * 4;

    let tot_len = u16::from_be(ip_hdr.tot_len);
    if tot_len < (header_len as u16) {
        return None;
    }

    skb.skb_pull(header_len as u32);

    Some(ip_hdr)
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
    let ip_ptr = skb.skb_push(IPHDR_LEN as u32).ok_or(())?;

    // SAFETY: skb_push returned a valid, properly aligned pointer of at least
    // IPHDR_LEN bytes; writing fields of repr(C) IpHdr is well-defined.
    unsafe {
        let ip_hdr = &mut *(ip_ptr as *mut IpHdr);

        ip_hdr.version_ihl = 0x45;

        ip_hdr.tos = 0;

        let total_len = skb.len as usize; // skb_push(IPHDR_LEN) already included it
        if total_len > u16::MAX as usize {
            return Err(()); // Packet too large for IPv4
        }
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

        // SAFETY: ip_hdr is a valid IpHdr pointer; reading size_of::<IpHdr>() bytes
        // from its repr(C) layout is well-defined.
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(
                (ip_hdr as *const IpHdr) as *const u8,
                core::mem::size_of::<IpHdr>()
            )
        };
        ip_hdr.check = checksum::ip_checksum(hdr_bytes).to_be();
    }

    // W3: TX fragmentation — an IP packet above the Ethernet MTU (loopback
    // excepted: its MTU is effectively 64KB and loopback_send frames are
    // never wire-bound) is split into MTU-sized fragments at 8-byte
    // boundaries with MF/frag_off set. The old code handed the oversized
    // packet to the driver, which dropped it (UDP >1472B sendto always
    // failed with EIO).
    if (dest_ip >> 24) != 127 && skb.len as usize > crate::config::ETH_MTU {
        return ip_fragment_output(skb);
    }

    ip_output(skb)
}

/// W3: split a fully-built IP packet (header + payload in `skb`) into
/// MTU-sized fragments and transmit each through the normal output path.
/// Consumes and frees `skb`.
fn ip_fragment_output(skb: SkBuff) -> Result<(), ()> {
    const FRAG_HDR_MAX: usize = IPHDR_LEN;

    // SAFETY: skb.data/skb.len describe the packet we just built.
    let pkt = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };
    if pkt.len() < IPHDR_LEN {
        skb.free();
        return Err(());
    }
    // SAFETY: length checked above; repr(C) IpHdr is exactly IPHDR_LEN.
    let hdr = unsafe { &*(pkt.as_ptr() as *const IpHdr) };
    let ihl = ((hdr.version_ihl & 0x0F) as usize) * 4;
    if ihl < IPHDR_LEN || ihl > pkt.len() {
        skb.free();
        return Err(());
    }
    let hdr_len = if ihl > FRAG_HDR_MAX { FRAG_HDR_MAX } else { ihl };
    let payload = &pkt[hdr_len..];

    // Payload bytes per fragment, 8-byte aligned per RFC 791.
    let frag_payload = (crate::config::ETH_MTU - hdr_len) & !7;
    if frag_payload == 0 {
        skb.free();
        return Err(());
    }

    // All fragments share the original datagram's ID.
    let id = u16::from_be(hdr.id);

    let mut off = 0usize;
    while off < payload.len() {
        let end = core::cmp::min(off + frag_payload, payload.len());
        let last = end == payload.len();

        let mut frag = match crate::net::buffer::alloc_skb(crate::config::ETH_MTU as u32) {
            Some(f) => f,
            None => {
                // Mid-datagram allocation failure: the already-sent
                // fragments will be discarded by the receiver on timeout.
                skb.free();
                return Err(());
            }
        };
        let flen = hdr_len + (end - off);
        // SAFETY: skb_put returned a valid pointer of flen bytes.
        let ptr = match frag.skb_put(flen as u32) {
            Some(p) => p,
            None => {
                frag.free();
                skb.free();
                return Err(());
            }
        };
        // SAFETY: ptr has flen >= hdr_len + frag bytes valid.
        unsafe {
            core::ptr::copy_nonoverlapping(pkt.as_ptr(), ptr, hdr_len);
            core::ptr::copy_nonoverlapping(
                payload.as_ptr().add(off),
                ptr.add(hdr_len),
                end - off,
            );
            let fh = &mut *(ptr as *mut IpHdr);
            fh.tot_len = (flen as u16).to_be();
            fh.id = id.to_be();
            let frag_bits = ((off / 8) as u16)
                | if last { 0 } else { ip_frag_flags::MF };
            fh.frag_off = frag_bits.to_be();
            fh.check = 0;
            let hdr_bytes = core::slice::from_raw_parts(ptr, hdr_len);
            fh.check = checksum::ip_checksum(hdr_bytes).to_be();
        }

        // Each fragment is its own Ethernet frame (ARP resolution per dest
        // is cached after the first).
        let _ = ip_output(frag);

        off = end;
    }

    skb.free();
    Ok(())
}

/// Send IPv4 packet
///
/// # Arguments
/// - `skb`: SkBuff (containing IP packet)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
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
    // SAFETY: skb.data and skb.len describe a valid byte range in the skb buffer.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    let ip_hdr = IpHdr::from_bytes(data).ok_or(())?;

    let version = ip_hdr.version_ihl >> 4;
    if version != 4 {
        return Ok(());
    }

    if !ip_hdr.is_valid_checksum() {
        return Ok(());
    }

    // Trim skb to IP-reported length (matches Linux ip_rcv: skb_trim(skb, ntohs(iph->tot_len)))
    let ip_total_len = u16::from_be(ip_hdr.tot_len) as u32;
    if ip_total_len < 20 || ip_total_len > skb.len {
        return Ok(()); // Invalid tot_len
    }
    skb.len = ip_total_len;

    let src_ip = u32::from_be(ip_hdr.saddr);
    let dest_ip = u32::from_be(ip_hdr.daddr);

    // Advance skb past IP header so upper layers see only the transport payload
    let ihl = ip_hdr.version_ihl & 0x0F;
    let hdr_len = (ihl as usize) * 4;
    // Validate IHL against packet length (per Linux ip_rcv)
    if hdr_len > ip_total_len as usize {
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
        // SAFETY: skb.data holds ip_total_len valid bytes; the payload
        // slice [hdr_len, ip_total_len) is in-bounds (checked above).
        let payload = unsafe {
            core::slice::from_raw_parts(skb.data.add(hdr_len), ip_total_len as usize - hdr_len)
        };
        defrag::ip_defrag(ip_hdr, payload, frag_off as u32, mf, src_ip, dest_ip);
        return Ok(());
    }

    ip_dispatch(skb, ip_hdr, src_ip, dest_ip);

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
    // SAFETY: ihl >= 5 was validated by IpHdr::from_bytes above; skb.data + hdr_len
    // is within the skb's valid data range.
    unsafe {
        skb.data = skb.data.add(hdr_len);
        skb.len -= hdr_len as u32;
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iphdr_size() {
        assert_eq!(core::mem::size_of::<IpHdr>(), 20);
    }

    #[test]
    fn test_iphdr_version_ihl() {
        let mut hdr = IpHdr::default();
        hdr.version_ihl = 0x45;

        assert_eq!(hdr.version_ihl >> 4, 4);
        assert_eq!(hdr.version_ihl & 0x0F, 5);
    }
}

/// Convenience wrapper: source = device address.
pub fn ipv4_send(skb: SkBuff, dest_ip: u32, protocol: u8) -> Result<(), ()> {
    ipv4_send_src(skb, 0, dest_ip, protocol)
}
