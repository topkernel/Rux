//! Framework-side net SUPPORT surface (crate-flip rung 2): the
//! lib-consumed net plumbing — ops-identity probes, the procfs stat
//! getters, the debug diagnostics — reached through a HOOK TABLE (the
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
    /// syscost's unix-socket tracing probe.
    pub unix_socket_of: fn(&File) -> Option<(bool, String)>,
    /// posix_mq's notify socket probe.
    pub mq_notify_sock: fn(&File) -> Option<i32>,
    // ---- rung-2 flip rows (the sole-copy deletion's reverse surface) ----
    /// futex park dump: (bound, peer) names of the unix socket on `fd`.
    pub unix_fd_bound_peer: fn(fd: usize) -> Option<(String, String)>,
    /// futex KILLDUMP: the unix socket file's diagnostic snapshot.
    pub unix_file_diag: fn(&File) -> Option<UnixDiag>,
    /// syscost recv_describe's unix arm: the formatted
    /// "unix:kind bound=[..] peer=[..]" line.
    pub unix_recv_describe: fn(fd: usize) -> Option<String>,
    /// hung_task's listener census (prints net-side, like the park
    /// audit).
    pub unix_listener_snapshot: fn(),
    /// taskdump's SLEEPSNAP unix park audit (prints net-side).
    pub unix_park_audit: fn(),
    /// The timers' "any TCP socket alive" probe (idle-balance guard).
    pub tcp_table_nonempty: fn() -> bool,
    /// memwatch's TCPCEN rows (locking is the hook impl's own).
    pub tcp_census_rows: fn() -> Vec<TcpCensusRow>,
    /// virtio-net RX bottom half (the packet pump).
    pub ethernet_poll: fn(),
    /// The TCP timer softirq handler (registered at softirq::init;
    /// takes the softirq vector like every open_softirq handler).
    pub tcp_timer_softirq: fn(usize),
    /// sysfs uevent fanout (kobject_uevent's netlink broadcast).
    pub uevent_broadcast: fn(&[u8]),
    /// vfs fstat's socket arm: stat every socket family as S_IFSOCK
    /// (sd-bus's sd_is_socket negotiation depends on it).
    pub socket_file_stat: fn(&File, &mut crate::fs::Stat) -> Option<i32>,
    /// posix_mq SIGEV_THREAD delivery: push the 32-byte musl marker
    /// message onto the helper's netlink socket file.
    pub netlink_push_kernel_message: fn(&File, [u8; 32]),
    /// The init seam (rust_main_impl's net bring-up, in boot order).
    pub init_tcp: fn(),
    pub init_tcp_timer: fn(),
    pub init_route: fn(),
    pub set_local_ip: fn(u32),
    pub ipv6_init: fn(),
}

/// futex KILLDUMP's unix-socket snapshot (all fields pre-formatted or
/// numeric net-side; the framework only prints).
pub struct UnixDiag {
    pub kind: String,
    pub bound: String,
    pub peer: String,
    pub queued_bytes: u64,
    pub truesize: u64,
    pub sndbuf: u64,
    pub qlen: u64,
}

/// memwatch's TCPCEN row (one TCP table slot, net-side locked walk).
pub struct TcpCensusRow {
    pub fd: u32,
    pub state: u8,
    pub rx_len: u64,
    pub rcv_wnd: u64,
    pub local_port: u16,
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
        unix_socket_of: |_| None,
        mq_notify_sock: |_| None,
        unix_fd_bound_peer: |_| None,
        unix_file_diag: |_| None,
        unix_recv_describe: |_| None,
        unix_listener_snapshot: || {},
        unix_park_audit: || {},
        tcp_table_nonempty: || false,
        tcp_census_rows: || Vec::new(),
        ethernet_poll: || {},
        tcp_timer_softirq: |_| {},
        uevent_broadcast: |_| {},
        socket_file_stat: |_, _| None,
        netlink_push_kernel_message: |_, _| {},
        init_tcp: || {},
        init_tcp_timer: || {},
        init_route: || {},
        set_local_ip: |_| {},
        ipv6_init: || {},
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
pub fn unix_socket_of(file: &File) -> Option<(bool, String)> { (hooks().unix_socket_of)(file) }
#[inline]
pub fn mq_notify_sock(file: &File) -> Option<i32> { (hooks().mq_notify_sock)(file) }
// The rung-2 flip rows.
#[inline]
pub fn unix_fd_bound_peer(fd: usize) -> Option<(String, String)> { (hooks().unix_fd_bound_peer)(fd) }
#[inline]
pub fn unix_file_diag(file: &File) -> Option<UnixDiag> { (hooks().unix_file_diag)(file) }
#[inline]
pub fn unix_recv_describe(fd: usize) -> Option<String> { (hooks().unix_recv_describe)(fd) }
#[inline]
pub fn unix_listener_snapshot() { (hooks().unix_listener_snapshot)() }
#[inline]
pub fn unix_park_audit() { (hooks().unix_park_audit)() }
#[inline]
pub fn tcp_table_nonempty() -> bool { (hooks().tcp_table_nonempty)() }
#[inline]
pub fn tcp_census_rows() -> Vec<TcpCensusRow> { (hooks().tcp_census_rows)() }
#[inline]
pub fn ethernet_poll() { (hooks().ethernet_poll)() }
#[inline]
pub fn tcp_timer_softirq(vec: usize) { (hooks().tcp_timer_softirq)(vec) }
#[inline]
pub fn uevent_broadcast(msg: &[u8]) { (hooks().uevent_broadcast)(msg) }
#[inline]
pub fn socket_file_stat(file: &File, stat: &mut crate::fs::Stat) -> Option<i32> { (hooks().socket_file_stat)(file, stat) }
#[inline]
pub fn netlink_push_kernel_message(file: &File, msg: [u8; 32]) { (hooks().netlink_push_kernel_message)(file, msg) }

#[inline]
pub fn init_tcp() { (hooks().init_tcp)() }
#[inline]
pub fn init_tcp_timer() { (hooks().init_tcp_timer)() }
#[inline]
pub fn init_route() { (hooks().init_route)() }
#[inline]
pub fn set_local_ip(ip: u32) { (hooks().set_local_ip)(ip) }
#[inline]
pub fn ipv6_init() { (hooks().ipv6_init)() }
