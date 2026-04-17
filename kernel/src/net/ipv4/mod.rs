//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv4 Protocol

pub mod route;
pub mod checksum;

use crate::net::buffer::SkBuff;
use crate::net::ethernet::ETH_ALEN;

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
///
/// # Notes
/// Adds IPv4 header at the front of SkBuff
pub fn ip_push_header(
    skb: &mut SkBuff,
    saddr: u32,
    daddr: u32,
    protocol: u8,
    tot_len: u16,
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

        ip_hdr.ttl = IP_DEFAULT_TTL;

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
pub fn ipv4_send(mut skb: SkBuff, dest_ip: u32, protocol: u8) -> Result<(), ()> {
    let ip_ptr = skb.skb_push(IPHDR_LEN as u32).ok_or(())?;

    // SAFETY: skb_push returned a valid, properly aligned pointer of at least
    // IPHDR_LEN bytes; writing fields of repr(C) IpHdr is well-defined.
    unsafe {
        let ip_hdr = &mut *(ip_ptr as *mut IpHdr);

        ip_hdr.version_ihl = 0x45;

        ip_hdr.tos = 0;

        let total_len = IPHDR_LEN + skb.len as usize;
        if total_len > u16::MAX as usize {
            return Err(()); // Packet too large for IPv4
        }
        ip_hdr.tot_len = (total_len as u16).to_be();

        ip_hdr.id = 0;

        ip_hdr.frag_off = 0;

        ip_hdr.ttl = IP_DEFAULT_TTL;

        ip_hdr.protocol = protocol;

        ip_hdr.saddr = crate::net::arp::get_local_ip().to_be();

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

    ip_output(skb)
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

    Ok(())
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
