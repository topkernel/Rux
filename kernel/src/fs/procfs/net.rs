//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/net/* — network subsystem introspection (P1)
//!
//! Files (formats aligned with Linux so ss/netstat-style parsers work):
//! - dev      interface RX/TX statistics (lo + eth0)
//! - tcp      TCP connection table (v4 sockets; IPv4 hex-address format)
//! - tcp6     TCP connection table (pure v6 sockets)
//! - udp      UDP bind table (v4 sockets)
//! - udp6     UDP bind table (pure v6 sockets)
//! - arp      ARP cache
//! - route    IPv4 routing table
//! - sockstat socket statistics summary

use alloc::string::String;
use alloc::vec::Vec;

/// Format a u32 IPv4 address (host byte order) as the %08X hex word Linux
/// prints in /proc/net/tcp et al: the network-order byte sequence read as
/// a little-endian integer (127.0.0.1 -> "0100007F").
fn hex_addr_v4(ip: u32) -> String {
    alloc::format!(
        "{:02X}{:02X}{:02X}{:02X}",
        ip & 0xFF,
        (ip >> 8) & 0xFF,
        (ip >> 16) & 0xFF,
        (ip >> 24) & 0xFF
    )
}

/// Format a port as Linux's 4-digit uppercase hex.
fn hex_port(port: u16) -> String {
    alloc::format!("{:04X}", port)
}

/// Format a 16-byte v6 address as the 32-char hex string Linux prints in
/// /proc/net/tcp6 — the address bytes in memory order, dotted every 4
/// hex chars (no colons in /proc/net; ss splits them).
fn hex_addr_v6(a: &[u8; 16]) -> String {
    let mut s = String::new();
    for i in 0..16 {
        s.push_str(&alloc::format!("{:02X}", a[i]));
        if i % 2 == 1 && i != 15 {
            s.push(':');
        }
    }
    s
}

// ============================================================================
// /proc/net/dev
// ============================================================================

/// Interface statistics row (name, stats)
fn dev_rows() -> Vec<(&'static str, crate::drivers::net::space::DeviceStats)> {
    let mut rows = Vec::new();
    // lo is always present (loopback_init runs at boot).
    rows.push(("lo", crate::drivers::net::loopback::loopback_stats()));
    if let Some(dev) = crate::drivers::net::virtio_net::get_device() {
        rows.push(("eth0", dev.get_stats()));
    }
    rows
}

/// Generate /proc/net/dev content
pub fn generate_dev() -> Vec<u8> {
    let mut out = String::new();
    out.push_str(
        "Inter-|   Receive                                                |  Transmit\n",
    );
    out.push_str(
        " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
    );

    for (name, s) in dev_rows() {
        out.push_str(&alloc::format!(
            "{:>6}: {:>7} {:>7} {:>4} {:>4} {:>4} {:>5} {:>10} {:>9} {:>8} {:>7} {:>4} {:>4} {:>4} {:>5} {:>7} {:>10}\n",
            name,
            s.rx_bytes,
            s.rx_packets,
            s.rx_errors,
            s.rx_dropped,
            0, // fifo
            0, // frame
            0, // compressed
            s.multicast,
            s.tx_bytes,
            s.tx_packets,
            s.tx_errors,
            s.tx_dropped,
            0, // fifo
            0, // colls
            0, // carrier
            0, // compressed
        ));
    }

    out.into_bytes()
}

// ============================================================================
// /proc/net/tcp, tcp6, udp, udp6
// ============================================================================

/// Shared table header (Linux format).
fn sock_table_header() -> &'static str {
    "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n"
}

/// One formatted connection row (shared by tcp/tcp6/udp/udp6). The ports
/// arrive pre-formatted (hex, 4 digits).
fn sock_row(sl: u32, local: &str, lport: u16, remote: &str, rport: u16, state: u8) -> String {
    alloc::format!(
        "{:4}: {}:{} {}:{} {:02X} {:08X} {:08X} {:02X} {:08X} {:8} {:5} {:8} {}\n",
        sl,
        local,
        hex_port(lport),
        remote,
        hex_port(rport),
        state,
        0u32, // tx_queue:hi
        0u32, // rx_queue:lo (written as part of the tx/rx pair)
        0,    // timer active
        0,    // tm->when
        0,    // retrnsmt
        0,    // uid
        0,    // timeout
        0,    // inode (socket inodes untracked in this stack)
    )
}

/// Generate /proc/net/tcp (v4 TCP slots)
pub fn generate_tcp() -> Vec<u8> {
    let mut out = String::from(sock_table_header());
    let mut sl = 0u32;
    for s in crate::net::tcp::tcp_dump() {
        if s.is_v6 {
            continue;
        }
        out.push_str(&sock_row(
            sl,
            &hex_addr_v4(s.local_ip),
            s.local_port,
            &hex_addr_v4(s.remote_ip),
            s.remote_port,
            s.linux_state,
        ));
        sl += 1;
    }
    out.into_bytes()
}

/// Generate /proc/net/tcp6 (pure v6 TCP slots)
pub fn generate_tcp6() -> Vec<u8> {
    let mut out = String::from(sock_table_header());
    let mut sl = 0u32;
    for s in crate::net::tcp::tcp_dump() {
        if !s.is_v6 {
            continue;
        }
        out.push_str(&sock_row(
            sl,
            &hex_addr_v6(&s.local_ip6),
            s.local_port,
            &hex_addr_v6(&s.remote_ip6),
            s.remote_port,
            s.linux_state,
        ));
        sl += 1;
    }
    out.into_bytes()
}

/// Generate /proc/net/udp (v4 UDP slots). Linux reports UDP slots with
/// state 07 (CLOSED).
pub fn generate_udp() -> Vec<u8> {
    let mut out = String::from(sock_table_header());
    let mut sl = 0u32;
    for s in crate::net::udp::udp_dump() {
        if s.is_v6 {
            continue;
        }
        out.push_str(&sock_row(
            sl,
            &hex_addr_v4(s.local_ip),
            s.local_port,
            &hex_addr_v4(s.remote_ip),
            s.remote_port,
            7, // UDP: CLOSED per Linux
        ));
        sl += 1;
    }
    out.into_bytes()
}

/// Generate /proc/net/udp6 (pure v6 UDP slots)
pub fn generate_udp6() -> Vec<u8> {
    let mut out = String::from(sock_table_header());
    let mut sl = 0u32;
    for s in crate::net::udp::udp_dump() {
        if !s.is_v6 {
            continue;
        }
        out.push_str(&sock_row(
            sl,
            &hex_addr_v6(&s.local_ip6),
            s.local_port,
            &hex_addr_v6(&s.remote_ip6),
            s.remote_port,
            7, // UDP: CLOSED per Linux
        ));
        sl += 1;
    }
    out.into_bytes()
}

// ============================================================================
// /proc/net/arp
// ============================================================================

/// Generate /proc/net/arp content
pub fn generate_arp() -> Vec<u8> {
    let mut out =
        String::from("IP address       HW type     Flags       HW address            Mask     Device\n");
    for e in crate::net::arp::arp_dump() {
        out.push_str(&alloc::format!(
            "{:>15}  {:<10} 0x{:x}     {}  *        eth0\n",
            format_v4(e.ip),
            "0x1", // ARPHRD_ETHER
            2,     // ATF_COM (complete entry)
            format_mac(&e.mac),
        ));
    }
    out.into_bytes()
}

/// MAC -> "aa:bb:cc:dd:ee:ff"
fn format_mac(mac: &[u8; 6]) -> String {
    alloc::format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// u32 (host order) -> dotted quad
fn format_v4(ip: u32) -> String {
    alloc::format!(
        "{}.{}.{}.{}",
        (ip >> 24) & 0xFF,
        (ip >> 16) & 0xFF,
        (ip >> 8) & 0xFF,
        ip & 0xFF
    )
}

// ============================================================================
// /proc/net/route
// ============================================================================

/// Generate /proc/net/route content (IPv4)
pub fn generate_route() -> Vec<u8> {
    let mut out = String::from(
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n",
    );
    for r in crate::net::ipv4::route::route_dump() {
        let iface = if r.oif == 1 { "lo" } else { "eth0" };
        out.push_str(&alloc::format!(
            "{}\t{}\t{}\t{:04X}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            iface,
            hex_addr_v4(r.dst),
            hex_addr_v4(r.gateway),
            r.flags.0,
            0, // refcnt
            0, // use
            0, // metric
            hex_addr_v4(r.mask),
            r.mtu,
            0, // window
            0, // irtt
        ));
    }
    out.into_bytes()
}

// ============================================================================
// /proc/net/sockstat
// ============================================================================

/// Generate /proc/net/sockstat content
pub fn generate_sockstat() -> Vec<u8> {
    let tcp = crate::net::tcp::tcp_dump();
    let udp = crate::net::udp::udp_dump();
    let tcp6 = tcp.iter().filter(|s| s.is_v6).count();
    let udp6 = udp.iter().filter(|s| s.is_v6).count();
    let tw = tcp
        .iter()
        .filter(|s| s.linux_state == 6) // TIME_WAIT
        .count();

    let mut out = String::new();
    out.push_str(&alloc::format!(
        "sockets: used {}\n",
        tcp.len() + udp.len()
    ));
    out.push_str(&alloc::format!(
        "TCP: inuse {} orphan 0 tw {} alloc {} mem 0\n",
        tcp.len() - tcp6,
        tw,
        tcp.len()
    ));
    out.push_str(&alloc::format!(
        "UDP: inuse {} mem 0\n",
        udp.len() - udp6
    ));
    out.push_str("UDPLITE: inuse 0 mem 0\n");
    out.push_str("RAW: inuse 0\n");
    out.push_str("FRAG: inuse 0 memory 0\n");
    out.push_str(&alloc::format!("IPv6: inuse {}\n", tcp6 + udp6));

    out.into_bytes()
}
