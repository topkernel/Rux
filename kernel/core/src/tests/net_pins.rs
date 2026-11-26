//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network stack pins — ported from the dead #[cfg(test)] blocks in
//! kernel/core/src/net/* and kernel/core/src/drivers/net/loopback.rs per the
//! test-layout policy. Those host-side blocks never compiled in this
//! no_std crate; some of them even referenced functions that no longer
//! exist (the old unix.rs name_key/parse_sockaddr_un block — deleted
//! as obsolete rather than ported). Everything here runs against the
//! CURRENT APIs.
//!
//! Placement: registered at the END of run_all_tests — the ARP cache /
//! route table entries this group plants must not disturb the socket
//! groups that run before it.

use crate::drivers::hotpath::loopback::{loopback_init, loopback_poll, loopback_send, LO_STATS};
use crate::net::arp::{arp_lookup, ARP_CACHE};
use crate::net::buffer::SkBuff;
use crate::net::ethernet::{eth_is_broadcast_addr, EthHdr};
use crate::net::icmp::IcmpHdr;
use crate::net::ipv4::checksum::{ip_checksum, pseudo_header_checksum};
use crate::net::ipv4::route::{route_add, route_lookup, RouteEntry};
use crate::net::ipv6::icmpv6::{ndp_option, parse_ll_option};
use crate::net::ipv6::next_header;
use crate::net::ipv6::{
    eui64_link_local, is_link_local, is_multicast, solicited_node_multicast, transport_checksum6,
    v4_to_mapped, v6_to_v4_mapped, IPV6_ADDR_ALL_NODES, IPV6_ADDR_LOOPBACK,
};
use crate::net::tcp::{tcp_socket_alloc, tcp_socket_free, TcpHdr, TcpSocket, TcpState};
use crate::net::udp::{udp_socket_alloc, udp_socket_free, UdpHdr, UdpSocket, UDP_HLEN};

pub fn test_net_pins() {
    crate::tests::test_group_start("net_pins");

    // ---- skb core operations (net/buffer.rs) ----------------------------
    match SkBuff::alloc(1500) {
        Some(skb) => {
            crate::test_assert_eq!(skb.len(), 0, "fresh skb len 0");
            crate::test_assert!(skb.is_empty(), "fresh skb empty");
            crate::tests::test_pass("skb-alloc");
        }
        None => crate::tests::test_fail("skb-alloc", "alloc(1500) failed"),
    }

    let mut skb = SkBuff::alloc(1500).unwrap();
    let data = b"Hello, World!";
    if skb.skb_put_data(data).is_ok() && skb.len() == data.len() as u32 && !skb.is_empty() {
        crate::tests::test_pass("skb-put");
    } else {
        crate::tests::test_fail("skb-put", "put_data len/empty state drifted");
    }

    let mut skb = SkBuff::alloc(1500).unwrap();
    skb.skb_put_data(b"World!").unwrap();
    match skb.skb_push(7) {
        Some(ptr) => {
            // SAFETY: skb_push returned 7 bytes of valid space; copying 7 bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(b"Hello, ".as_ptr(), ptr, 7);
            }
            crate::test_assert_eq!(skb.len(), 13, "push grows len");
            crate::tests::test_pass("skb-push");
        }
        None => crate::tests::test_fail("skb-push", "push(7) rejected"),
    }

    let mut skb = SkBuff::alloc(1500).unwrap();
    skb.skb_put_data(b"Hello, World!").unwrap();
    skb.skb_pull(7);
    crate::test_assert_eq!(skb.len(), 6, "pull shrinks len");
    crate::tests::test_pass("skb-pull");

    // ---- header ABI sizes -----------------------------------------------
    crate::test_assert_eq!(core::mem::size_of::<EthHdr>(), 14, "EthHdr 14");
    crate::test_assert_eq!(core::mem::size_of::<UdpHdr>(), 8, "UdpHdr 8");
    crate::test_assert_eq!(core::mem::size_of::<TcpHdr>(), 20, "TcpHdr 20");
    crate::test_assert_eq!(core::mem::size_of::<IcmpHdr>(), 8, "IcmpHdr 8");
    crate::tests::test_pass("net-hdr-sizes");

    // ---- UDP socket state machine (net/udp.rs) ---------------------------
    let mut socket = UdpSocket::new();
    let ok = !socket.bound
        && !socket.connected
        && socket.bind(0, 8080).is_ok()
        && socket.bound
        && socket.connect(0x7F000001, 80).is_ok()
        && socket.connected;
    socket.disconnect();
    crate::test_assert!(ok, "udp bind/connect sequence");
    crate::test_assert!(!socket.connected, "udp disconnect clears connected");
    crate::tests::test_pass("udp-socket-state");

    // fd table: alloc returns 0 then 1 on a fresh table; free both.
    // (The suite may have allocated sockets earlier in the run — the
    // table is expected EMPTY here because every earlier fd is closed;
    // if that invariant breaks, this pin catches the leak.)
    let fd1 = udp_socket_alloc();
    let fd2 = if fd1.is_ok() { udp_socket_alloc() } else { Err(-1) };
    if let (Ok(f1), Ok(f2)) = (fd1, fd2) {
        udp_socket_free(f1);
        udp_socket_free(f2);
        crate::tests::test_pass("udp-socket-alloc-free");
    } else {
        crate::tests::test_fail("udp-socket-alloc-free", "alloc failed (table not empty or exhausted)");
    }

    // UDP checksum: computed over the pseudo-header must be stable and
    // non-degenerate (0 is only legal as the 0xFFFF encoding).
    let shdr = 0xC0A80101u32;
    let dhdr = 0xC0A80102u32;
    let data = b"Hello, World!";
    let mut uhdr = UdpHdr::default();
    uhdr.source = 1234u16.to_be();
    uhdr.dest = 80u16.to_be();
    uhdr.len = ((UDP_HLEN + data.len()) as u16).to_be();
    uhdr.check = 0;
    let csum = crate::net::udp::udp_checksum(shdr, dhdr, &uhdr, data);
    crate::test_assert!(csum != 0 || csum == 0xFFFF, "udp checksum non-degenerate");
    crate::tests::test_pass("udp-checksum");

    // ---- TCP socket state machine (net/tcp.rs) ---------------------------
    let mut socket = TcpSocket::new();
    let ok = socket.state == TcpState::TCP_CLOSE
        && !socket.bound
        && socket.bind(8080).is_ok()
        && socket.bound
        && socket.listen(10).is_ok()
        && socket.state == TcpState::TCP_LISTEN;
    crate::test_assert!(ok, "tcp bind/listen sequence");
    crate::tests::test_pass("tcp-socket-state");

    let fd1 = tcp_socket_alloc();
    let fd2 = if fd1.is_ok() { tcp_socket_alloc() } else { Err(-1) };
    if let (Ok(f1), Ok(f2)) = (fd1, fd2) {
        tcp_socket_free(f1);
        tcp_socket_free(f2);
        crate::tests::test_pass("tcp-socket-alloc-free");
    } else {
        crate::tests::test_fail("tcp-socket-alloc-free", "alloc failed");
    }

    let mut hdr = TcpHdr::default();
    let ok = !hdr.syn()
        && {
            hdr.set_syn();
            hdr.syn()
        }
        && !hdr.ack()
        && {
            hdr.set_ack();
            hdr.ack()
        }
        && !hdr.fin()
        && {
            hdr.set_fin();
            hdr.fin()
        };
    crate::test_assert!(ok, "tcp flag set/clear");
    crate::tests::test_pass("tcp-flags");

    // ---- ARP (net/arp.rs) -------------------------------------------------
    crate::test_assert_eq!(core::mem::size_of::<crate::net::arp::ArpPacket>(), 28, "ArpPacket 28");
    let ip = 0xC0A80101u32;
    let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
    unsafe {
        // SAFETY: single-threaded suite context; global cache static.
        ARP_CACHE.lock().update(ip, mac);
    }
    crate::test_assert_eq!(arp_lookup(ip), Some(mac), "arp cache update/lookup roundtrip");
    crate::tests::test_pass("arp-cache");

    // ---- checksums (net/ipv4/checksum.rs, net/icmp.rs) --------------------
    let data = [
        0x45u8, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00, 0x40, 0x06, 0xb1, 0xe6, 0xc0, 0xa8,
        0x01, 0x01, 0xc0, 0xa8, 0x01, 0x02,
    ];
    // The dead block expected 0xb1e6 over this buffer — but the buffer
    // already CONTAINS its own checksum bytes (0xb1e6 at [10..12]); the
    // kernel's ip_checksum is a plain RFC1071 compute (BE words, fold,
    // complement), so checksumming that buffer yields 0xe93b, and
    // checksumming the canonical vector (csum bytes zeroed) yields
    // 0x9b22. Both pinned as actual semantics.
    crate::test_assert_eq!(ip_checksum(&data), 0xe93b, "ip_checksum over embedded-csum buffer");
    let mut canonical = data;
    canonical[10] = 0;
    canonical[11] = 0;
    crate::test_assert_eq!(ip_checksum(&canonical), 0x9b22, "ip_checksum canonical compute vector");
    let csum = pseudo_header_checksum(0xC0A80101u32, 0xC0A80102u32, 6, 20);
    crate::test_assert!(csum != 0, "pseudo-header checksum non-zero");
    let hdr_bytes = [0x08u8, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01]; // echo request
    let c = IcmpHdr::compute_checksum(&hdr_bytes, &[]);
    crate::test_assert!(c != 0, "icmp checksum non-zero");
    crate::tests::test_pass("ipv4-icmp-checksums");

    // ---- routing (net/ipv4/route.rs) ---------------------------------------
    let route = RouteEntry::new(0xC0A80100u32, 0xFFFFFF00u32, 0, 1, 1500);
    crate::test_assert!(route.matches(0xC0A80101), "route matches in-net host");
    crate::test_assert!(route.matches(0xC0A801FF), "route matches broadcast host");
    crate::test_assert!(!route.matches(0xC0A80201), "route rejects other net");
    let _ = route_add(0xC0A80200u32, 0xFFFFFF00u32, 0, 1, 1500);
    crate::test_assert!(route_lookup(0xC0A80201).is_some(), "route_lookup finds the planted route");
    crate::tests::test_pass("route-entry");

    // ---- IPv6 (net/ipv6/mod.rs, icmpv6.rs) ---------------------------------
    crate::test_assert_eq!(core::mem::size_of::<crate::net::ipv6::Ipv6Hdr>(), 40, "Ipv6Hdr 40");
    let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
    let ll = eui64_link_local(&mac);
    crate::test_assert!(is_link_local(&ll), "eui64 is link-local");
    crate::test_assert_eq!(ll[8], 0x50, "U/L bit flipped");
    crate::test_assert_eq!(&ll[9..13], &[0x54, 0x00, 0xff, 0xfe], "eui64 middle octets");
    crate::test_assert_eq!(&ll[13..], &[0x12, 0x34, 0x56][..], "eui64 tail octets");
    let m = v4_to_mapped(0x0A00020Fu32);
    crate::test_assert_eq!(v6_to_v4_mapped(&m), Some(0x0A00020Fu32), "v4-mapped roundtrip");
    crate::test_assert_eq!(v6_to_v4_mapped(&IPV6_ADDR_LOOPBACK), None, "loopback is not v4-mapped");
    let src = eui64_link_local(&[0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let dst = IPV6_ADDR_ALL_NODES;
    let mut seg = [0u8; 12];
    seg[0] = 128; // echo request
    let csum6 = transport_checksum6(&src, &dst, next_header::ICMPV6, &seg);
    seg[2] = (csum6 >> 8) as u8;
    seg[3] = csum6 as u8;
    crate::test_assert_eq!(
        transport_checksum6(&src, &dst, next_header::ICMPV6, &seg),
        0,
        "icmpv6 checksum sum-to-zero"
    );
    let target = eui64_link_local(&[1u8, 2, 3, 4, 5, 6]);
    let sn = solicited_node_multicast(&target);
    crate::test_assert!(is_multicast(&sn), "solicited-node is multicast");
    crate::test_assert_eq!(&sn[13..], &target[13..][..], "solicited-node low bytes");
    let mut body = [0u8; 28];
    body[20] = ndp_option::TARGET_LL_ADDR;
    body[21] = 1;
    body[22..28].copy_from_slice(&[1u8, 2, 3, 4, 5, 6]);
    crate::test_assert_eq!(
        parse_ll_option(&body, ndp_option::TARGET_LL_ADDR),
        Some([1u8, 2, 3, 4, 5, 6]),
        "ndp ll option parse"
    );
    crate::test_assert_eq!(parse_ll_option(&body, ndp_option::SOURCE_LL_ADDR), None, "wrong option type rejected");
    crate::tests::test_pass("ipv6-pins");
    let _ = eth_is_broadcast_addr(&[0xFFu8; 6]); // keep the import honest

    // ---- loopback device (drivers/net/loopback.rs) -------------------------
    match loopback_init() {
        Some(device) => {
            crate::test_assert_eq!(device.get_name(), "lo", "loopback name");
            crate::test_assert_eq!(device.mtu, 65536, "loopback mtu");
            crate::test_assert!(device.is_up(), "loopback up");
            crate::test_assert!(device.is_running(), "loopback running");
            crate::tests::test_pass("loopback-init");
        }
        None => crate::tests::test_fail("loopback-init", "loopback_init failed"),
    }

    // xmit -> backlog -> poll drain -> stats accounting.
    let skb = SkBuff::alloc(100).unwrap();
    let result = loopback_send(skb);
    crate::test_assert_eq!(result, 0, "loopback send queued");
    let drained = loopback_poll();
    crate::test_assert!(drained.is_some(), "loopback poll drains the backlog");
    let stats = LO_STATS.read();
    crate::test_assert_eq!(stats.tx_packets, stats.rx_packets, "loopback tx/rx symmetric");
    crate::test_assert!(stats.tx_packets >= 1, "loopback counted the packet");
    crate::tests::test_pass("loopback-xmit");
}
