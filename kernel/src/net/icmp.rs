//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ICMP Protocol (RFC 792)

use crate::net::buffer::SkBuff;
use crate::net::ipv4::checksum::ip_checksum;

/// ICMP header length (minimum)
pub const ICMP_HDR_LEN: usize = 8;

/// ICMP types
pub mod icmp_type {
    pub const ECHO_REPLY: u8 = 0;
    pub const DEST_UNREACH: u8 = 3;
    pub const ECHO_REQUEST: u8 = 8;
    pub const TIME_EXCEEDED: u8 = 11;
}

/// ICMP destination unreachable codes
pub mod icmp_code {
    pub const NET_UNREACH: u8 = 0;
    pub const HOST_UNREACH: u8 = 1;
    pub const PROT_UNREACH: u8 = 2;
    pub const PORT_UNREACH: u8 = 3;
    pub const FRAG_NEEDED: u8 = 4;
    pub const SR_FAILED: u8 = 5;
}

/// ICMP header
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IcmpHdr {
    /// ICMP type
    pub type_: u8,
    /// ICMP code
    pub code: u8,
    /// Checksum
    pub checksum: u16,
    /// Identifier (for echo request/reply)
    pub id: u16,
    /// Sequence number (for echo request/reply)
    pub seq: u16,
}

impl IcmpHdr {
    /// Create ICMP header from byte slice
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < ICMP_HDR_LEN {
            return None;
        }
        // SAFETY: length checked above guarantees `data` has at least ICMP_HDR_LEN (8) bytes;
        // IcmpHdr is repr(C) with no padding so the cast covers exactly 8 aligned bytes.
        unsafe { Some(&*(data.as_ptr() as *const IcmpHdr)) }
    }

    /// Calculate ICMP checksum over header + data
    pub fn compute_checksum(hdr: &[u8], data: &[u8]) -> u16 {
        let mut sum: u32 = 0;

        // Header
        let mut i = 0;
        while i + 1 < hdr.len() {
            let word = u16::from_be_bytes([hdr[i], hdr[i + 1]]) as u32;
            sum += word;
            i += 2;
        }
        if i < hdr.len() {
            sum += (hdr[i] as u32) << 8;
        }

        // Data
        let mut i = 0;
        while i + 1 < data.len() {
            let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
            sum += word;
            i += 2;
        }
        if i < data.len() {
            sum += (data[i] as u32) << 8;
        }

        // Fold carry
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }

        !sum as u16
    }
}

/// Receive and process ICMP packet
///
/// # Arguments
/// - `skb`: SkBuff containing ICMP packet (after IP header has been pulled)
/// - `src_ip`: Source IP address
/// - `dest_ip`: Destination IP address
pub fn icmp_rcv(skb: &SkBuff, src_ip: u32, _dest_ip: u32) -> Result<(), ()> {
    // SAFETY: skb.data points to the ICMP payload within the skb buffer and
    // skb.len is the valid byte count; the skb is still owned by this caller.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < ICMP_HDR_LEN {
        return Err(());
    }

    let icmp_hdr = match IcmpHdr::from_bytes(data) {
        Some(hdr) => hdr,
        None => return Err(()),
    };

    let payload = &data[ICMP_HDR_LEN..];

    match icmp_hdr.type_ {
        icmp_type::ECHO_REQUEST => {
            icmp_echo_reply(src_ip, icmp_hdr, payload);
        }
        icmp_type::DEST_UNREACH | icmp_type::TIME_EXCEEDED => {
            // Pass to upper layer protocols (TCP)
            if payload.len() >= core::mem::size_of::<crate::net::ipv4::IpHdr>() + 8 {
                // The payload starts with original IP header + 8 bytes of transport header
                // SAFETY: payload length was checked to be >= IPHDR_LEN + 8 above,
                // and IpHdr is repr(C) with size IPHDR_LEN, so the cast is valid.
                let orig_ip_hdr = unsafe {
                    &*(payload.as_ptr() as *const crate::net::ipv4::IpHdr)
                };
                let orig_src_ip = u32::from_be(orig_ip_hdr.saddr);
                let orig_dst_ip = u32::from_be(orig_ip_hdr.daddr);
                let orig_proto = orig_ip_hdr.protocol;

                if orig_proto == 6 {
                    // TCP — find matching connection and report soft error
                    let transport_hdr = &payload[core::mem::size_of::<crate::net::ipv4::IpHdr>()..];
                    if transport_hdr.len() >= 8 {
                        let orig_src_port = u16::from_be_bytes([transport_hdr[0], transport_hdr[1]]);
                        let orig_dst_port = u16::from_be_bytes([transport_hdr[2], transport_hdr[3]]);
                        crate::net::tcp::tcp_v4_err(
                            icmp_hdr.type_,
                            icmp_hdr.code,
                            orig_src_ip,
                            orig_src_port,
                            orig_dst_ip,
                            orig_dst_port,
                        );
                    }
                }
            }
        }
        _ => {
            // Other ICMP types — silently ignore
        }
    }

    Ok(())
}

/// Send ICMP echo reply in response to echo request
fn icmp_echo_reply(src_ip: u32, req_hdr: &IcmpHdr, payload: &[u8]) {
    let mut skb = match crate::net::buffer::alloc_skb(1500) {
        Some(s) => s,
        None => return,
    };

    // Build ICMP echo reply header (type=0, code=0, same id/seq)
    let mut hdr = IcmpHdr {
        type_: icmp_type::ECHO_REPLY,
        code: 0,
        checksum: 0,
        id: req_hdr.id,
        seq: req_hdr.seq,
    };

    // SAFETY: &hdr is a valid stack-local reference; IcmpHdr is repr(C) so
    // reinterpreting its bytes as [u8] covers exactly size_of::<IcmpHdr>() bytes.
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(
            &hdr as *const IcmpHdr as *const u8,
            core::mem::size_of::<IcmpHdr>(),
        )
    };

    hdr.checksum = IcmpHdr::compute_checksum(hdr_bytes, payload);

    // Push header then data into skb
    if skb.skb_put_data(payload).is_err() {
        return;
    }

    let ptr = match skb.skb_push(ICMP_HDR_LEN as u32) {
        Some(p) => p,
        None => return,
    };

    // SAFETY: ptr was returned by skb_push with ICMP_HDR_LEN bytes of space,
    // and &hdr is a valid stack-local IcmpHdr of exactly that size.
    unsafe {
        core::ptr::copy_nonoverlapping(
            &hdr as *const IcmpHdr as *const u8,
            ptr as *mut u8,
            ICMP_HDR_LEN,
        );
    }

    let _ = crate::net::ipv4::ipv4_send(skb, src_ip, 1); // IPPROTO_ICMP = 1
}

/// Send ICMP destination unreachable message
///
/// # Arguments
/// - `orig_skb`: Original SkBuff that triggered the error
/// - `code`: ICMP code (e.g., icmp_code::PORT_UNREACH)
/// - `info`: Additional info (MTU for FRAG_NEEDED, 0 otherwise)
pub fn icmp_send_dest_unreach(orig_skb: &SkBuff, code: u8, _info: u32) {
    // SAFETY: orig_skb.data points to the packet data within the skb buffer and
    // orig_skb.len is the valid byte count; the skb is still owned by this caller.
    let data = unsafe { core::slice::from_raw_parts(orig_skb.data, orig_skb.len as usize) };

    // Build ICMP dest unreachable: type=3, code, checksum, unused(4 bytes) + original IP header + 8 bytes
    let mut hdr = IcmpHdr {
        type_: icmp_type::DEST_UNREACH,
        code,
        checksum: 0,
        id: 0,
        seq: 0,
    };

    // Include original IP header + first 8 bytes of payload
    let incl_len = core::cmp::min(data.len(), crate::net::ipv4::IPHDR_LEN + 8);
    let orig_data = &data[..incl_len];

    // SAFETY: &hdr is a valid stack-local reference; IcmpHdr is repr(C) so
    // reinterpreting its bytes as [u8] covers exactly size_of::<IcmpHdr>() bytes.
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(
            &hdr as *const IcmpHdr as *const u8,
            core::mem::size_of::<IcmpHdr>(),
        )
    };

    hdr.checksum = IcmpHdr::compute_checksum(hdr_bytes, orig_data);

    let mut skb = match crate::net::buffer::alloc_skb(1500) {
        Some(s) => s,
        None => return,
    };

    if skb.skb_put_data(orig_data).is_err() {
        return;
    }

    let ptr = match skb.skb_push(ICMP_HDR_LEN as u32) {
        Some(p) => p,
        None => return,
    };

    // SAFETY: ptr was returned by skb_push with ICMP_HDR_LEN bytes of space,
    // and &hdr is a valid stack-local IcmpHdr of exactly that size.
    unsafe {
        core::ptr::copy_nonoverlapping(
            &hdr as *const IcmpHdr as *const u8,
            ptr as *mut u8,
            ICMP_HDR_LEN,
        );
    }

    // Extract source IP from original IP header to use as destination
    if data.len() >= crate::net::ipv4::IPHDR_LEN {
        // SAFETY: data length was checked to be >= IPHDR_LEN above,
        // and IpHdr is repr(C) with size IPHDR_LEN, so the cast is valid.
        let orig_ip_hdr = unsafe {
            &*(data.as_ptr() as *const crate::net::ipv4::IpHdr)
        };
        let orig_src_ip = u32::from_be(orig_ip_hdr.saddr);
        let _ = crate::net::ipv4::ipv4_send(skb, orig_src_ip, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icmphdr_size() {
        assert_eq!(core::mem::size_of::<IcmpHdr>(), 8);
    }

    #[test]
    fn test_icmp_checksum() {
        let hdr = [0x08, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01]; // Echo request
        let csum = IcmpHdr::compute_checksum(&hdr, &[]);
        assert_ne!(csum, 0);
    }
}
