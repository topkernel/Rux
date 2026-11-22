//! Framework-side net SUPPORT surface (crate-flip rung 2): the
//! lib-consumed net plumbing — ops-identity probes, the procfs stat
//! getters, the dfx diagnostics — reached through a HOOK TABLE (the
//! SyscallHooks pattern: nullable fns no-op until the service crate's
//! register; the composition bin calls it before the framework impl).

use crate::fs::File;
use alloc::string::String;
use alloc::vec::Vec;

/// The net hook set: every lib-side consumer of net-stack internals.
#[derive(Clone, Copy)]
pub struct NetHooks {
    /// /proc/net/tcp + tcp6: the TCP socket table rows (laddr:lport
    /// raddr:rport st tx:rx tr:tm->when uid inode) — procfs formats.
    pub tcp_rows: fn() -> Vec<TcpRow>,
    /// /proc/net/udp + udp6.
    pub udp_rows: fn() -> Vec<UdpRow>,
    /// /proc/net/unix: the AF_UNIX registry (procfs + the futex
    /// wedge-hunt diagnostics).
    pub unix_registry_snapshot: fn() -> Vec<String>,
    /// /proc/net/arp rows.
    pub arp_rows: fn() -> Vec<ArpRow>,
    /// /proc/net/route rows.
    pub route_rows: fn() -> Vec<RouteRow>,
    /// /proc/net/dev rows.
    pub dev_rows: fn() -> Vec<DevRow>,
    /// The futex fd-kind identity probes (the AF_UNIX hunt family).
    pub is_unix_socket_ops: fn(&'static crate::fs::FileOps) -> bool,
    pub is_inet_socket_ops: fn(&'static crate::fs::FileOps) -> bool,
    pub is_netlink_ops: fn(&'static crate::fs::FileOps) -> bool,
    pub is_raw_ops: fn(&'static crate::fs::FileOps) -> bool,
    /// posix_mq's netlink notify probe.
    pub netlink_socket_from_fd: fn(usize) -> Option<usize>,
    /// syscost's unix-socket tracing probe (fd-based).
    pub unix_socket_of_fd: fn(usize) -> Option<(alloc::string::String, bool)>,
    /// The futex fd-detail line for a unix socket (bound=... peer=...).
    pub unix_detail_of: fn(&File) -> Option<alloc::string::String>,
}

/// A /proc/net/tcp row (the socket-table getter's raw data; procfs
/// formats the header + lines).
pub struct TcpRow {
    pub is_v6: bool,
    pub local_ip: u32,
    pub local_ip6: [u8; 16],
    pub local_port: u16,
    pub remote_ip: u32,
    pub remote_ip6: [u8; 16],
    pub remote_port: u16,
    pub linux_state: u8,
}

pub struct UdpRow {
    pub is_v6: bool,
    pub local_ip: u32,
    pub local_ip6: [u8; 16],
    pub local_port: u16,
    pub remote_ip: u32,
    pub remote_ip6: [u8; 16],
    pub remote_port: u16,
}

pub struct ArpRow {
    pub ip: u32,
    pub mac: [u8; 6],
    pub dev: String,
}

pub struct RouteRow {
    pub oif: u32,
    pub dst: u32,
    pub gateway: u32,
    pub mask: u32,
    pub flags: u32,
    pub metric: u32,
    pub mtu: u32,
}

pub struct DevRow {
    pub name: String,
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
}

impl NetHooks {
    pub const EMPTY: NetHooks = NetHooks {
        tcp_rows: || Vec::new(),
        udp_rows: || Vec::new(),
        unix_registry_snapshot: || Vec::new(),
        arp_rows: || Vec::new(),
        route_rows: || Vec::new(),
        dev_rows: || Vec::new(),
        is_unix_socket_ops: |_| false,
        is_inet_socket_ops: |_| false,
        is_netlink_ops: |_| false,
        is_raw_ops: |_| false,
        netlink_socket_from_fd: |_| None,
        unix_socket_of_fd: |_| None,
        unix_detail_of: |_| None,
    };
}

pub static NET_HOOKS: crate::safety::slot::OnceSlot<NetHooks> =
    crate::safety::slot::OnceSlot::new();

/// Registration entry (called by the net service crate at boot).
pub fn net_register_hooks(hooks: NetHooks) {
    let _ = NET_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> NetHooks {
    *NET_HOOKS.get().unwrap_or(&NetHooks::EMPTY)
}

// The lib-side call surface.

#[inline]
pub fn tcp_rows() -> Vec<TcpRow> { (hooks().tcp_rows)() }
/// The dump-shaped twin procfs consumes (same hook, doc alias).
#[inline]
pub fn tcp_rows_dump() -> Vec<TcpRow> { (hooks().tcp_rows)() }
#[inline]
pub fn udp_rows() -> Vec<UdpRow> { (hooks().udp_rows)() }
#[inline]
pub fn udp_rows_dump() -> Vec<UdpRow> { (hooks().udp_rows)() }
#[inline]
pub fn unix_registry_snapshot() -> Vec<String> { (hooks().unix_registry_snapshot)() }
/// /proc/net/unix's formatted snapshot (the same hook): one entry per
/// line, pre-joined — procfs pushes each as a row.
#[inline]
pub fn unix_net_snapshot() -> Vec<String> { (hooks().unix_registry_snapshot)() }
#[inline]
pub fn arp_rows() -> Vec<ArpRow> { (hooks().arp_rows)() }
#[inline]
pub fn arp_rows_dump() -> Vec<ArpRow> { (hooks().arp_rows)() }
#[inline]
pub fn route_rows() -> Vec<RouteRow> { (hooks().route_rows)() }
#[inline]
pub fn route_rows_dump() -> Vec<RouteRow> { (hooks().route_rows)() }
#[inline]
pub fn dev_rows() -> Vec<DevRow> { (hooks().dev_rows)() }
#[inline]
pub fn is_unix_socket_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_unix_socket_ops)(ops) }
#[inline]
pub fn is_inet_socket_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_inet_socket_ops)(ops) }
#[inline]
pub fn is_netlink_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_netlink_ops)(ops) }
#[inline]
pub fn is_raw_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_raw_ops)(ops) }
#[inline]
pub fn netlink_socket_from_fd(fd: usize) -> Option<usize> { (hooks().netlink_socket_from_fd)(fd) }
#[inline]
pub fn unix_socket_of_fd(fd: usize) -> Option<(alloc::string::String, bool)> { (hooks().unix_socket_of_fd)(fd) }
#[inline]
pub fn unix_socket_of(file: &File) -> Option<alloc::string::String> { (hooks().unix_detail_of)(file) }
