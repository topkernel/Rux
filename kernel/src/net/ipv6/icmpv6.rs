//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ICMPv6 (P1): Echo Request/Reply (ping6), Neighbor Discovery
//! (NS/NA — ARP's replacement for IPv6) and Router Solicitation.
//!
//! All messages carry the mandatory ICMPv6 checksum computed over the
//! IPv6 pseudo-header (see super::transport_checksum6).

use super::{
    is_local_addr, is_unspecified, multicast_mac, neigh_update, solicited_node_multicast,
    transport_checksum6, Ipv6Addr, IPV6_ADDR_ALL_NODES, IPV6_ADDR_ALL_ROUTERS, IPV6_HDR_LEN,
};
use crate::net::buffer::alloc_skb;

/// ICMPv6 header length
pub const ICMPV6_HDR_LEN: usize = 4;

/// ICMPv6 message types (RFC 4443 / RFC 4861)
pub mod icmpv6_type {
    /// Echo request (ping6)
    pub const ECHO_REQUEST: u8 = 128;
    /// Echo reply
    pub const ECHO_REPLY: u8 = 129;
    /// Multicast listener report (v1)
    pub const MLD_REPORT: u8 = 131;
    /// Router solicitation
    pub const ROUTER_SOLICITATION: u8 = 133;
    /// Router advertisement
    pub const ROUTER_ADVERTISEMENT: u8 = 134;
    /// Neighbor solicitation
    pub const NEIGHBOR_SOLICITATION: u8 = 135;
    /// Neighbor advertisement
    pub const NEIGHBOR_ADVERTISEMENT: u8 = 136;
}

/// NDP option types
pub mod ndp_option {
    /// Source link-layer address
    pub const SOURCE_LL_ADDR: u8 = 1;
    /// Target link-layer address
    pub const TARGET_LL_ADDR: u8 = 2;
}

/// ICMPv6 fixed header { type, code, checksum }
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Icmpv6Hdr {
    pub typ: u8,
    pub code: u8,
    pub check: u16,
}

/// Our device MAC (fallback default matches ethernet.rs convention)
fn our_mac() -> [u8; 6] {
    crate::drivers::net::virtio_net::get_device()
        .map(|d| d.get_mac())
        .unwrap_or([0x52, 0x54, 0x00, 0x12, 0x34, 0x56])
}

/// Build and transmit one ICMPv6 message: hdr fields with checksum left 0,
/// then `body` (options included), checksum fixed up big-endian.
fn icmpv6_send(
    typ: u8,
    code: u8,
    src: &Ipv6Addr,
    dst: &Ipv6Addr,
    body: &[u8],
) -> Result<(), ()> {
    if is_unspecified(src) {
        return Err(()); // no source address yet
    }

    let total = ICMPV6_HDR_LEN + body.len();
    let mut skb = match alloc_skb(total as u32) {
        Some(s) => s,
        None => return Err(()),
    };

    {
        let ptr = skb.skb_put(total as u32).ok_or(())?;
        // SAFETY: skb_put reserved exactly `total` bytes at ptr.
        unsafe {
            let hdr = &mut *(ptr as *mut Icmpv6Hdr);
            hdr.typ = typ;
            hdr.code = code;
            hdr.check = 0;
            core::ptr::copy_nonoverlapping(body.as_ptr(), ptr.add(ICMPV6_HDR_LEN), body.len());

            let seg = core::slice::from_raw_parts(ptr, total);
            hdr.check = transport_checksum6(src, dst, super::next_header::ICMPV6, seg).to_be();
        }
    }

    super::ipv6_send(skb, src, dst, super::next_header::ICMPV6)
}

/// Receive an ICMPv6 packet (base IPv6 header already pulled).
pub fn icmpv6_rcv(skb: &crate::net::buffer::SkBuff, src: &Ipv6Addr, dst: &Ipv6Addr) {
    if (skb.len as usize) < ICMPV6_HDR_LEN {
        return;
    }
    // SAFETY: length checked above; skb.data covers skb.len valid bytes.
    let seg = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };
    // SAFETY: seg has at least ICMPV6_HDR_LEN bytes.
    let hdr = unsafe { &*(seg.as_ptr() as *const Icmpv6Hdr) };

    // Checksum is mandatory in ICMPv6 — a mismatched message is dropped.
    if transport_checksum6(src, dst, super::next_header::ICMPV6, seg) != 0 {
        return;
    }

    let body = &seg[ICMPV6_HDR_LEN..];

    match hdr.typ {
        icmpv6_type::ECHO_REQUEST => {
            // Reply from the addressed local address (link-local when the
            // request came to a multicast address, per RFC 4443 §4.2).
            let reply_src = if is_local_addr(dst) {
                *dst
            } else {
                match super::get_link_local() {
                    Some(ll) => ll,
                    None => return,
                }
            };
            // Echo identifier+seq+data verbatim (the whole body).
            let _ = icmpv6_send(icmpv6_type::ECHO_REPLY, 0, &reply_src, src, body);
        }
        icmpv6_type::NEIGHBOR_SOLICITATION => {
            handle_ns(src, dst, body);
        }
        icmpv6_type::NEIGHBOR_ADVERTISEMENT => {
            handle_na(src, body);
        }
        icmpv6_type::ROUTER_SOLICITATION
        | icmpv6_type::MLD_REPORT => {
            // Host-only stack: nothing to answer.
        }
        icmpv6_type::ROUTER_ADVERTISEMENT => {
            // Could install a default route/gateway; minimal stack keeps
            // link-local only (a slirp network has no v6 routers anyway).
        }
        _ => {}
    }
}

/// Neighbor Solicitation: 4 reserved bytes + 16-byte target + options.
/// We answer when the target is one of our addresses (RFC 4861 §7.2.4).
fn handle_ns(src: &Ipv6Addr, dst: &Ipv6Addr, body: &[u8]) {
    if body.len() < 20 {
        return;
    }
    let mut target = [0u8; 16];
    target.copy_from_slice(&body[4..20]);
    if !is_local_addr(&target) {
        return;
    }

    // Learn the solicitor's MAC from its source link-layer option (type 1).
    if let Some(mac) = parse_ll_option(body, ndp_option::SOURCE_LL_ADDR) {
        neigh_update(*src, mac);
    }

    // NA destination: the solicitor's unicast address when it had one,
    // otherwise the all-nodes multicast (RFC 4861 §7.2.4).
    let na_dst = if is_unspecified(src) {
        IPV6_ADDR_ALL_NODES
    } else {
        *src
    };
    let _ = dst; // NS was to our solicited-node multicast; unused otherwise

    // NA body (after the 4-byte ICMPv6 header): R|S|O flags + 3 reserved
    // bytes, 16-byte target, then the target link-layer option (2 + 6,
    // padded to its 8-byte unit).
    let mut na_body = [0u8; 28];
    na_body[0] = 0x40; // S(solicited)=1, R=0 (not a router), O=0
    na_body[4..20].copy_from_slice(&target);
    na_body[20] = ndp_option::TARGET_LL_ADDR;
    na_body[21] = 1; // length in 8-byte units
    na_body[22..28].copy_from_slice(&our_mac());

    let _ = icmpv6_send(
        icmpv6_type::NEIGHBOR_ADVERTISEMENT,
        0,
        &target,
        &na_dst,
        &na_body,
    );
}

/// Neighbor Advertisement: 4 reserved + flags + target + options.
/// Confirms the sender's link-layer address for its target address.
fn handle_na(src: &Ipv6Addr, body: &[u8]) {
    if body.len() < 20 {
        return;
    }
    let mut target = [0u8; 16];
    target.copy_from_slice(&body[4..20]);
    // The NA's target is the address being advertised = the sender's own
    // address; the target link-layer option carries its MAC.
    if let Some(mac) = parse_ll_option(body, ndp_option::TARGET_LL_ADDR) {
        neigh_update(target, mac);
    }
    let _ = src;
}

/// Extract a link-layer option's MAC from an NDP message body.
fn parse_ll_option(body: &[u8], want: u8) -> Option<[u8; 6]> {
    let mut i = 20usize; // skip the fixed 20-byte NS/NA prefix
    while i + 2 <= body.len() {
        let typ = body[i];
        let len = body[i + 1] as usize;
        if len == 0 || i + len * 8 > body.len() {
            return None;
        }
        if typ == want && len >= 1 && i + 8 <= body.len() {
            let mut mac = [0u8; 6];
            mac.copy_from_slice(&body[i + 2..i + 8]);
            return Some(mac);
        }
        i += len * 8;
    }
    None
}

/// Send a Neighbor Solicitation for `target` (drops the first packet to an
/// unresolved neighbor — upper-layer retransmits and the next attempt
/// finds the cache populated; mirrors the pre-R35 ARP behavior minus the
/// broadcast fallback).
pub fn send_ns(target: &Ipv6Addr) {
    let ll = match super::get_link_local() {
        Some(ll) => ll,
        None => return,
    };
    let sn = solicited_node_multicast(target);
    // Seed the multicast neighbor entry so the NS itself can be emitted.
    neigh_update(sn, multicast_mac(&sn));

    // NS body: 4 reserved + target + source-ll option.
    let mut body = [0u8; 28];
    body[4..20].copy_from_slice(target);
    body[20] = ndp_option::SOURCE_LL_ADDR;
    body[21] = 1;
    body[22..28].copy_from_slice(&our_mac());

    let _ = icmpv6_send(
        icmpv6_type::NEIGHBOR_SOLICITATION,
        0,
        &ll,
        &sn,
        &body,
    );
}

/// Send a Router Solicitation (RFC 4861 §6.1.1) — best-effort, ignored by
/// router-less networks like slirp.
pub fn send_rs() {
    let ll = match super::get_link_local() {
        Some(ll) => ll,
        None => return,
    };
    // RS body: 4 reserved bytes + source-ll option (2 + 6 = one 8-byte unit).
    let mut body = alloc::vec::Vec::new();
    body.resize(4, 0);
    body.push(ndp_option::SOURCE_LL_ADDR);
    body.push(1);
    body.extend_from_slice(&our_mac());

    neigh_update(IPV6_ADDR_ALL_ROUTERS, multicast_mac(&IPV6_ADDR_ALL_ROUTERS));
    let _ = icmpv6_send(
        icmpv6_type::ROUTER_SOLICITATION,
        0,
        &ll,
        &IPV6_ADDR_ALL_ROUTERS,
        &body,
    );
}

/// v4-compat helper used by tests / future callers
#[allow(dead_code)]
pub fn icmpv6_min_mtu() -> usize {
    1280 - IPV6_HDR_LEN
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::{ndp_option, parse_ll_option};

    #[test]
    fn test_option_parse() {
        let mut body = [0u8; 28];
        body[20] = ndp_option::TARGET_LL_ADDR;
        body[21] = 1;
        body[22..28].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(
            parse_ll_option(&body, ndp_option::TARGET_LL_ADDR),
            Some([1, 2, 3, 4, 5, 6])
        );
        assert_eq!(parse_ll_option(&body, ndp_option::SOURCE_LL_ADDR), None);
    }
}
