//! The NET SERVICE CRATE (crate-flip rung 2): the network stack —
//! TCP/UDP/UNIX/ICMP/ARP/route/netlink + the ethernet layer — under a
//! crate-level unsafe-deny boundary. The framework keeps
//! net/net_hooks.rs (the hook table) and the drivers consume the
//! stack's packet-ingress surface downward.
//!
//! Sole-copy state (the rung-1 recipe's final step): the service files
//! LIVE in this crate (src/net/); the framework keeps net_hooks,
//! file_arc, net_edges and buffer.

// FORBID since the substrate extraction: the socket-table reference
// formation, Arc-refcount probes, validated derefs and POD header
// views moved to the framework's net/net_edges.rs (INV-NET-SOCKTAB-1,
// INV-NET-ARCREF-1, INV-NET-HDRVIEW-1); the file→Arc adoption lives in
// net/file_arc.rs and skb slicing in net/buffer.rs. The three
// historical `unsafe impl Sync` markers were VESTIGIAL (the structs
// are auto-Sync — verified by deletion + green check) and are gone.
#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the service files' crate::X paths resolve unchanged (the proven shape).
pub use rux::{
    arch, cmdline, config, console, dfx, drivers, errno, fs, interrupt, mm,
    process, safety, sched, security, signal, sync, timer, trace,
};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
pub use rux::syscall; // the framework's syscall seam (support items)
// Service-internal test lives CENTRALLY in kernel/src/tests/services/.
#[path = "../../src/tests/services/net_hdrs_test.rs"]
pub mod net_hdrs_test;

extern crate alloc;

// The net tree, nested under crate::net (the historical paths).
pub mod net {
    // the service files
    #[path = "arp.rs"] pub mod arp;
    // buffer.rs (the raw-memory substrate: allocator calls, pointer
    // arithmetic, slice construction — 22 unsafe constructs) stays
    // FRAMEWORK-side (the edges.rs pattern); the crate consumes it
    // via the re-export below.
    pub use rux::net::buffer;
    pub use rux::syscall; // the framework's syscall seam (support/edges)
    pub use rux::net::file_arc::file_arc;
    pub use rux::net::file_arc::take_file_arc as reclaim_file_arc;
    pub use rux::net::file_arc::take_file_arc;
    pub use rux::net::net_edges;
    #[path = "ethernet.rs"] pub mod ethernet;
    #[path = "icmp.rs"] pub mod icmp;
    #[path = "netlink.rs"] pub mod netlink;
    #[path = "raw.rs"] pub mod raw;
    #[path = "socket.rs"] pub mod socket;
    #[path = "tcp.rs"] pub mod tcp;
    #[path = "tcp_timer.rs"] pub mod tcp_timer;
    #[path = "udp.rs"] pub mod udp;
    #[path = "unix.rs"] pub mod unix;
    #[path = "ipv4/mod.rs"] pub mod ipv4;
    #[path = "ipv6/mod.rs"] pub mod ipv6;

    // Guest tests that moved with the crate flip (SERVICE-TEST SEAM:
    // rux-bin registers run_unit_tests below; the framework's counters
    // are shared, so the pins land in the same suite totals).
    #[cfg(feature = "unit-test")]

    // The framework's whole-module globs (the old shared mod.rs's shape).
    pub use crate::arch::*;
    pub use crate::config;
    pub use crate::console;
    pub use crate::dfx::*;
    pub use crate::drivers::*;
    pub use crate::errno;
    pub use crate::fs::*;
    pub use crate::interrupt;
    pub use crate::mm::*;
    pub use crate::process::*;
    pub use crate::safety::*;
    pub use crate::sched::*;
    pub use crate::security::*;
    pub use crate::signal::*;
    pub use crate::sync::*;
    pub use crate::timer::*;
    pub use crate::trace;
    pub use rux::net::net_hooks;
}

// The framework's whole-module globs (the service files' `use super::*` /
// crate:: paths).
pub mod net_globs {
    pub use crate::net::*;
}

/// The service-side hook implementations (the NetHooks table's data).
/// Every row names its net-side source; the framework getters (procfs,
/// futex diagnostics, syscost, timers, softirq, vfs stat, posix_mq,
/// the boot init seam) resolve through these.
mod hooks_impl {
    use rux::net::net_hooks::{TcpRow, UdpRow, ArpRow, RouteRow, DevRow, UnixDiag, TcpCensusRow};

    pub fn tcp_rows() -> alloc::vec::Vec<TcpRow> {
        crate::net::tcp::tcp_dump()
            .into_iter()
            .map(|s| TcpRow {
                is_v6: s.is_v6,
                local_ip: s.local_ip,
                local_ip6: s.local_ip6,
                local_port: s.local_port,
                remote_ip: s.remote_ip,
                remote_ip6: s.remote_ip6,
                remote_port: s.remote_port,
                linux_state: s.linux_state,
            })
            .collect()
    }

    pub fn udp_rows() -> alloc::vec::Vec<UdpRow> {
        crate::net::udp::udp_dump()
            .into_iter()
            .map(|s| UdpRow {
                is_v6: s.is_v6,
                local_ip: s.local_ip,
                local_ip6: s.local_ip6,
                local_port: s.local_port,
                remote_ip: s.remote_ip,
                remote_ip6: s.remote_ip6,
                remote_port: s.remote_port,
            })
            .collect()
    }

    pub fn arp_rows() -> alloc::vec::Vec<ArpRow> {
        crate::net::arp::arp_dump()
            .into_iter()
            .filter(|e| e.valid)
            .map(|e| ArpRow { ip: e.ip, mac: e.mac, dev: alloc::string::String::from("eth0") })
            .collect()
    }

    pub fn route_rows() -> alloc::vec::Vec<RouteRow> {
        crate::net::ipv4::route::route_dump()
            .into_iter()
            .map(|r| RouteRow {
                oif: r.oif,
                dst: r.dst,
                gateway: r.gateway,
                mask: r.mask,
                flags: r.flags.0,
                metric: 0,
                mtu: r.mtu,
            })
            .collect()
    }

    pub fn dev_rows() -> alloc::vec::Vec<DevRow> {
        let mut rows = alloc::vec::Vec::new();
        let lo = rux::drivers::hooks::loopback_stats();
        rows.push(DevRow {
            name: alloc::string::String::from("lo"),
            rx_bytes: lo.rx_bytes,
            rx_packets: lo.rx_packets,
            tx_bytes: lo.tx_bytes,
            tx_packets: lo.tx_packets,
        });
        if let Some(dev) = rux::drivers::net::virtio_net::get_device() {
            let s = dev.get_stats();
            rows.push(DevRow {
                name: alloc::string::String::from("eth0"),
                rx_bytes: s.rx_bytes,
                rx_packets: s.rx_packets,
                tx_bytes: s.tx_bytes,
                tx_packets: s.tx_packets,
            });
        }
        rows
    }

    pub fn unix_registry_snapshot() -> alloc::vec::Vec<alloc::string::String> {
        crate::net::unix::registry_snapshot()
    }

    pub fn is_unix_socket_ops(ops: &'static rux::fs::FileOps) -> bool {
        core::ptr::eq(ops, &crate::net::unix::UNIX_SOCKET_OPS)
    }

    pub fn is_inet_socket_ops(ops: &'static rux::fs::FileOps) -> bool {
        core::ptr::eq(ops, &crate::net::socket::SOCKET_OPS)
    }

    pub fn is_netlink_ops(ops: &'static rux::fs::FileOps) -> bool {
        core::ptr::eq(ops, &crate::net::netlink::NETLINK_OPS)
    }

    pub fn is_raw_ops(ops: &'static rux::fs::FileOps) -> bool {
        core::ptr::eq(ops, &crate::net::raw::RAW_OPS)
    }

    pub fn unix_socket_of(_file: &rux::fs::file::File) -> Option<(bool, alloc::string::String)> {
        None // (reserved row; no framework caller after the rung-2 flip)
    }

    pub fn mq_notify_sock(_file: &rux::fs::file::File) -> Option<i32> {
        None // (reserved row; posix_mq consumes is_netlink_ops directly)
    }

    pub fn unix_fd_bound_peer(fd: usize) -> Option<(alloc::string::String, alloc::string::String)> {
        let (sock, _) = crate::net::unix::unix_file_of(fd)?;
        let bound = sock
            .bound_name
            .lock()
            .clone()
            .unwrap_or_else(|| alloc::string::String::from("unbound"));
        let peer = crate::net::unix::unix_peer_bound_name(&sock)
            .unwrap_or_else(|| alloc::string::String::from("-"));
        Some((bound, peer))
    }

    pub fn unix_file_diag(file: &rux::fs::file::File) -> Option<UnixDiag> {
        let us = crate::net::unix::unix_of_file_diag(file)?;
        let kind = alloc::format!("{:?}", us.kind);
        let bound = us.bound_name.lock().clone().unwrap_or_default();
        let peer = us.dgram_peer.lock().clone().unwrap_or_default();
        let (qb, qt, cap, ql) = us.diag_snapshot();
        Some(UnixDiag {
            kind,
            bound,
            peer,
            queued_bytes: qb as u64,
            truesize: qt as u64,
            sndbuf: cap as u64,
            qlen: ql as u64,
        })
    }

    pub fn unix_recv_describe(fd: usize) -> Option<alloc::string::String> {
        let (usock, nonblock) = crate::net::unix::unix_file_of(fd)?;
        let kind = match usock.kind {
            crate::net::unix::UnixKind::Stream => "stream",
            _ => "dgram",
        };
        let bound = usock.bound_name.lock().clone().unwrap_or_default();
        let peer_name = match usock.peer_cred() {
            Some(c) => alloc::format!("pid {}", c.pid),
            None => usock
                .dgram_peer
                .lock()
                .clone()
                .unwrap_or_else(|| alloc::string::String::from("-")),
        };
        Some(alloc::format!(
            "unix:{} bound=[{}] peer=[{}]{}",
            kind,
            bound,
            peer_name,
            if nonblock { " O_NONBLOCK" } else { "" }
        ))
    }

    pub fn unix_listener_snapshot() {
        crate::net::unix::unix_listener_snapshot()
    }

    pub fn unix_park_audit() {
        crate::net::unix::unix_park_audit()
    }

    pub fn tcp_table_nonempty() -> bool {
        crate::net::tcp::get_tcp_socket_table().count() > 0
    }

    pub fn tcp_census_rows() -> alloc::vec::Vec<TcpCensusRow> {
        crate::net::tcp::tcp_census_dump()
    }

    pub fn ethernet_poll() {
        crate::net::ethernet::ethernet_poll();
    }

    pub fn socket_file_stat(
        file: &rux::fs::file::File,
        stat: &mut rux::fs::Stat,
    ) -> Option<i32> {
        crate::net::socket::socket_file_stat(file, stat)
    }

    pub fn uevent_broadcast(msg: &[u8]) {
        crate::net::netlink::uevent_broadcast(msg);
    }

    pub fn netlink_push_kernel_message(file: &rux::fs::file::File, msg: [u8; 32]) {
        if let Some(sock) = crate::net::file_arc::<crate::net::netlink::NetlinkSocket>(file) {
            let mut v = alloc::vec::Vec::new();
            v.extend_from_slice(&msg);
            sock.push_kernel_message(v);
        }
    }
}

/// Registration: fills the framework's NetHooks table (the crate-flip
/// rung-2 data seam). Called by the composition bin before the
/// framework impl.
pub fn register_all() {
    use rux::net::net_hooks::{NetHooks, net_register_hooks};
    net_register_hooks(NetHooks {
        tcp_rows: hooks_impl::tcp_rows,
        udp_rows: hooks_impl::udp_rows,
        unix_registry_snapshot: hooks_impl::unix_registry_snapshot,
        arp_rows: hooks_impl::arp_rows,
        route_rows: hooks_impl::route_rows,
        dev_rows: hooks_impl::dev_rows,
        is_unix_socket_ops: hooks_impl::is_unix_socket_ops,
        is_inet_socket_ops: hooks_impl::is_inet_socket_ops,
        is_netlink_ops: hooks_impl::is_netlink_ops,
        is_raw_ops: hooks_impl::is_raw_ops,
        unix_socket_of: hooks_impl::unix_socket_of,
        mq_notify_sock: hooks_impl::mq_notify_sock,
        unix_fd_bound_peer: hooks_impl::unix_fd_bound_peer,
        unix_file_diag: hooks_impl::unix_file_diag,
        unix_recv_describe: hooks_impl::unix_recv_describe,
        unix_listener_snapshot: hooks_impl::unix_listener_snapshot,
        unix_park_audit: hooks_impl::unix_park_audit,
        tcp_table_nonempty: hooks_impl::tcp_table_nonempty,
        tcp_census_rows: hooks_impl::tcp_census_rows,
        ethernet_poll: hooks_impl::ethernet_poll,
        tcp_timer_softirq: crate::net::tcp_timer::timer_softirq_handler,
        uevent_broadcast: hooks_impl::uevent_broadcast,
        socket_file_stat: hooks_impl::socket_file_stat,
        netlink_push_kernel_message: hooks_impl::netlink_push_kernel_message,
        init_tcp: crate::net::tcp::init_tcp_manager,
        init_tcp_timer: crate::net::tcp_timer::init_tcp_timer_manager,
        init_route: crate::net::ipv4::route::route_init,
        set_local_ip: crate::net::arp::set_local_ip,
        ipv6_init: crate::net::ipv6::ipv6_init,
    });
}

/// The net init sequence (moved from lib.rs's 5 direct calls).
pub fn init_all() {
    crate::net::tcp::init_tcp_manager();
    crate::net::tcp_timer::init_tcp_timer_manager();
    crate::net::ipv4::route::route_init();
    // (arp set_local_ip + ipv6_init are config-conditional at the caller)
}

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (rux-bin registers it at
/// boot). Reporting goes through the framework's counters, so pass/fail
/// lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    crate::net_hdrs_test::test_net_hdrs();
}
