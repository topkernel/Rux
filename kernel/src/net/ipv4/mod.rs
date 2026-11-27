//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv4 Protocol


#![forbid(unsafe_code)]
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
    // R-NETTX-2 (2026-11-28, TCP-connect-hunt): tot_len counts the IP
    // header TOO (RFC 791 §3.1), unlike the v6 payload_len this
    // safe-edge rewrite was shaped after. The 5a9e1545d rewrite captured
    // skb.len BEFORE the push_hdr (transport bytes only) — every IPv4
    // packet declared 20 bytes short. The wire proof (netprobe SYN,
    // filter-dump pcap): IP tot_len=40 over a 40-byte TCP header —
    // tcpdump "bad hdr length 40 - too long, > 20", slirp dropped every
    // SYN, no SYN-ACK ever. Compute the wire total from the transport
    // length BEFORE the push (skb_push grows skb.len, but the closure
    // cannot read it).
    let transport_len = skb.len as usize;
    let total_len = transport_len + IPHDR_LEN; // header included, per RFC 791
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

    // R-NETTX-1 (2026-11-28, TCP-connect-hang hunt): the emit is the
    // CONTRACT of this function — every upper-layer sender (TcpTxBatch::
    // emit_all, BuiltUdp::emit, icmp, raw) hands the built skb here and
    // treats Ok as "on the wire". The 5a9e1545d unsafe-slim rewrite
    // dropped the tail that hands the packet to ip_output: the skb was
    // built, checksummed, and silently freed. Every IPv4 transmit died
    // at once — no TCP SYN ever left (both loopback and virtio-net
    // connects hung in SYN_SENT forever: lat_tcp/lat_connect/bw_tcp
    // TIMEOUT 2/2, netprobe connect(10.0.2.2:443) EINPROGRESS then
    // select=0 soerr=0, zero tcp-in segments), no UDP datagram, no ICMP
    // echo reply. Loopback UDP/TCP and netdev UDP share this one root —
    // the resolve rc=0 in the GB logs was served from the image's
    // /etc/hosts pin, which is why the dead TX went unnoticed there.
    //
    // W3: TX fragmentation — an IP packet above the Ethernet MTU
    // (loopback excepted: its MTU is effectively 64KB and loopback_send
    // frames are never wire-bound) is split into MTU-sized fragments at
    // 8-byte boundaries with MF/frag_off set. The old pre-5a9e1545d code
    // handed the oversized packet to the driver, which dropped it (UDP
    // >1472B sendto always failed with EIO).
    if (dest_ip >> 24) != 127 && skb.len as usize > crate::config::ETH_MTU as usize {
        return ip_fragment_output(skb);
    }

    ip_output(skb)
}

/// W3: split a fully-built IP packet (header + payload in `skb`) into
/// MTU-sized fragments and transmit each through the normal output path.
/// Consumes and frees `skb`. Restored with the R-NETTX-1 emit fix in
/// safe-edge form (data_all/hdr_mut; no raw slices): unlike the deleted
/// pre-5a9e1545d version, the WHOLE original header (options included)
/// is copied into every fragment — the old hdr_len clamp at 20 bytes
/// corrupted fragments of options-bearing datagrams.
fn ip_fragment_output(skb: SkBuff) -> Result<(), ()> {
    // The clamped safe view of the whole datagram (header + payload).
    let pkt = skb.data_all();
    if pkt.len() < IPHDR_LEN {
        return Err(());
    }
    // The header parse validated IHL >= 5 upstream (push_hdr built a
    // 0x45/option-less header here), but stay defensive: ihl bytes must
    // lie inside the packet for the header copy below.
    let ihl_bytes = ((pkt[0] & 0x0F) as usize) * 4;
    if ihl_bytes < IPHDR_LEN || ihl_bytes > pkt.len() {
        return Err(());
    }
    let (header, payload) = pkt.split_at(ihl_bytes);

    // Payload bytes per fragment, 8-byte aligned per RFC 791.
    let frag_payload = (crate::config::ETH_MTU as usize - ihl_bytes) & !7;
    if frag_payload == 0 {
        return Err(());
    }

    // All fragments share the original datagram's ID.
    let id = u16::from_be_bytes([header[4], header[5]]);

    let mut off = 0usize;
    while off < payload.len() {
        let end = core::cmp::min(off + frag_payload, payload.len());
        let last = end == payload.len();

        let flen = ihl_bytes + (end - off);
        let mut frag = match crate::net::buffer::alloc_skb(crate::config::ETH_MTU as u32) {
            Some(f) => f,
            None => {
                // Mid-datagram allocation failure: the already-sent
                // fragments will be discarded by the receiver on timeout.
                return Err(());
            }
        };
        if frag.skb_put_data(&header).is_err() || frag.skb_put_data(&payload[off..end]).is_err() {
            frag.free();
            return Err(());
        }
        // Patch the fragment's own header fields and re-checksum (the
        // copied header still carries the original tot_len/frag_off/check).
        if let Some(fh) = frag.hdr_mut::<IpHdr>() {
            fh.tot_len = (flen as u16).to_be();
            fh.id = id.to_be();
            let frag_bits = ((off / 8) as u16)
                | if last { 0 } else { ip_frag_flags::MF };
            fh.frag_off = frag_bits.to_be();
            fh.check = 0;
            let hdr_bytes = crate::safety::bytes::bytes_of(fh);
            fh.check = checksum::ip_checksum(&hdr_bytes[..IPHDR_LEN]).to_be();
        }

        // Each fragment is its own Ethernet frame (ARP resolution per dest
        // is cached after the first).
        let _ = ip_output(frag);

        off = end;
    }

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
