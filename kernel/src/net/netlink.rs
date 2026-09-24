//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AF_NETLINK sockets + minimal rtnetlink — P0-2 (network configuration
//! entry point: the `ip`/`ifconfig` path).
//!
//! Supported message set (family NETLINK_ROUTE):
//! - RTM_GETLINK (dump + single) / RTM_NEWLINK (flags up/down change)
//!   → ifinfomsg + IFLA_IFNAME / IFLA_ADDRESS / IFLA_MTU / IFLA_STATS
//! - RTM_GETADDR (dump + single) / RTM_NEWADDR / RTM_DELADDR
//!   → ifaddrmsg + IFA_ADDRESS / IFA_LOCAL / IFA_LABEL
//! - RTM_GETROUTE (dump) / RTM_NEWROUTE
//!   → rtmsg + RTA_DST / RTA_GATEWAY / RTA_OIF
//! - NLMSG_ERROR ACKs (NLM_F_ACK) and errors for unknown requests
//!
//! Model: sendmsg/sendto/write PARSES the request buffer(s) and executes
//! the commands immediately, queueing response messages on the socket's
//! receive queue; recvmsg/recvfrom/read returns exactly ONE netlink
//! message per call (Linux rtnetlink ABI). Dumps queue one message per
//! record plus a final NLMSG_DONE.
//!
//! The interface database presented here is fixed at lo (ifindex 1) and
//! eth0 (ifindex 2) — the canonical Linux indices — with flags/MTU/MAC
//! synced live from the underlying drivers/net devices, and addresses in
//! a small settable table (eth0 mirrors arp::get_local_ip()).

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::errno::constants;
use crate::fs::file::{File, FileFlags, FileOps};
use crate::net::socket::{SocketOptions, SOCK_CLOEXEC_FLAG, SOCK_NONBLOCK_FLAG, SOCK_TYPE_MASK};
use crate::process::wait::WaitQueueHead;
use crate::sync::spinlock::Spinlock;

// ============================================================================
// Constants
// ============================================================================

/// Address family
pub const AF_NETLINK: i32 = 16;

/// NETLINK_USERSOCK etc. — only ROUTE is implemented.
pub const NETLINK_ROUTE: i32 = 0;

/// Socket types accepted for netlink
pub const SOCK_RAW: i32 = 3;

/// sizeof(struct sockaddr_nl)
pub const SOCKADDR_NL_LEN: usize = 12;

// nlmsg types
const NLMSG_NOOP: u16 = 1;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;

// nlmsg flags
const NLM_F_REQUEST: u16 = 0x01;
const NLM_F_MULTI: u16 = 0x02;
const NLM_F_ACK: u16 = 0x04;
const NLM_F_DUMP: u16 = 0x300; // ROOT | MATCH
const NLM_F_CREATE: u16 = 0x400;

// rtnetlink message types
const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTM_GETLINK: u16 = 18;
const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
const RTM_GETADDR: u16 = 22;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;
const RTM_GETROUTE: u16 = 26;

// rtnetlink attributes
const IFLA_ADDRESS: u16 = 1;
const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_STATS: u16 = 7;

const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_LABEL: u16 = 3;

const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;

// Interface flags (ifr_flags / ifi_flags)
const IFF_UP: u32 = 0x1;
const IFF_BROADCAST: u32 = 0x2;
const IFF_LOOPBACK: u32 = 0x8;
const IFF_RUNNING: u32 = 0x40;
const IFF_MULTICAST: u32 = 0x1000;

// rt scope / protocol
const RT_SCOPE_UNIVERSE: u8 = 0;
const RT_SCOPE_LINK: u8 = 253;
const RT_TABLE_MAIN: u8 = 254;
const RTPROT_STATIC: u8 = 4;

// ============================================================================
// Interface database
// ============================================================================

/// One interface record (presented index space: lo=1, eth0=2).
struct IfaceEntry {
    index: u32,
    name: &'static str,
    /// (ip host byte order, prefix length)
    addrs: Vec<(u32, u8)>,
}

static IFACES: Spinlock<Vec<IfaceEntry>> = Spinlock::new(Vec::new());

/// Presented ifindex for the two devices.
const LO_INDEX: u32 = 1;
const ETH0_INDEX: u32 = 2;

/// Populate the table on first use (lo = 127.0.0.1/8, eth0 mirrors the
/// live local IP).
fn iface_init_locked() {
    let mut ifaces = IFACES.lock();
    if !ifaces.is_empty() {
        return;
    }
    ifaces.push(IfaceEntry {
        index: LO_INDEX,
        name: "lo",
        addrs: vec![(0x7F000001, 8)],
    });
    ifaces.push(IfaceEntry {
        index: ETH0_INDEX,
        name: "eth0",
        addrs: vec![(crate::net::arp::get_local_ip(), 24)],
    });
}

/// Count prefix bits of a netmask.
fn mask_to_prefix(mask: u32) -> u8 {
    mask.count_ones() as u8
}

/// Live flags/mtu/mac for a presented index. Defaults mirror the drivers'
/// initial state when the device is absent.
fn iface_hw(index: u32) -> (u32, u32, [u8; 6], u16) {
    match index {
        LO_INDEX => {
            let flags = crate::drivers::net::get_loopback_device()
                .map(|d| d.flags)
                .unwrap_or(IFF_UP | IFF_RUNNING | IFF_LOOPBACK);
            (flags, 65536, [0; 6], 772) // ARPHRD_LOOPBACK
        }
        ETH0_INDEX => {
            if let Some(dev) = crate::drivers::net::get_virtio_net_device_net() {
                let mut mac = [0u8; 6];
                mac.copy_from_slice(&dev.addr[..6]);
                let mtu = dev.mtu;
                let flags = dev.flags;
                (flags, mtu, mac, 1) // ARPHRD_ETHER
            } else {
                (
                    IFF_UP | IFF_RUNNING | IFF_BROADCAST,
                    1500,
                    [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
                    1,
                )
            }
        }
        _ => (0, 0, [0; 6], 1),
    }
}

/// Apply an IFF_UP change to the underlying device.
fn iface_set_up(index: u32, up: bool) {
    let dev = match index {
        LO_INDEX => crate::drivers::net::get_loopback_device(),
        ETH0_INDEX => crate::drivers::net::get_virtio_net_device_net(),
        _ => return,
    };
    if let Some(dev) = dev {
        if up {
            dev.up();
        } else {
            dev.down();
        }
    }
}

/// Find an interface by name (presented indices only).
fn iface_find(name: &str) -> Option<u32> {
    iface_init_locked();
    IFACES.lock().iter().find(|i| i.name == name).map(|i| i.index)
}

/// Get the primary address of an interface.
pub fn iface_get_addr(index: u32) -> Option<u32> {
    iface_init_locked();
    IFACES
        .lock()
        .iter()
        .find(|i| i.index == index)
        .and_then(|i| i.addrs.first().map(|a| a.0))
}

/// Set the address of an interface (replaces the primary). eth0 also
/// updates the live local IP used by the TX path.
pub fn iface_set_addr(index: u32, ip: u32, prefix: u8) -> bool {
    iface_init_locked();
    let mut ifaces = IFACES.lock();
    if let Some(entry) = ifaces.iter_mut().find(|i| i.index == index) {
        entry.addrs.clear();
        entry.addrs.push((ip, prefix));
        if index == ETH0_INDEX {
            crate::net::arp::set_local_ip(ip);
        }
        true
    } else {
        false
    }
}

// ============================================================================
// Netlink message builders
// ============================================================================

/// Append a netlink message (header + payload, padded to 4) to `out`.
fn nlmsg_push(out: &mut Vec<u8>, msg_type: u16, flags: u16, seq: u32, payload: &[u8]) {
    let total = 16 + payload.len();
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&msg_type.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // pid = kernel (0)
    out.extend_from_slice(payload);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

/// Append a rtattr (padded to 4) to a payload buffer.
fn rtattr_push(payload: &mut Vec<u8>, rta_type: u16, data: &[u8]) {
    let len = 4 + data.len();
    payload.extend_from_slice(&(len as u16).to_le_bytes());
    payload.extend_from_slice(&rta_type.to_le_bytes());
    payload.extend_from_slice(data);
    while payload.len() % 4 != 0 {
        payload.push(0);
    }
}

/// Build one RTM_NEWLINK response message for an interface.
fn build_link_msg(seq: u32, index: u32, name: &str, multi: bool) -> Vec<u8> {
    let (flags, mtu, mac, htype) = iface_hw(index);
    let mut ifinfo = Vec::new();
    ifinfo.push(0u8); // family = AF_UNSPEC
    ifinfo.push(0u8); // pad
    ifinfo.extend_from_slice(&htype.to_le_bytes());
    ifinfo.extend_from_slice(&(index as i32).to_le_bytes());
    ifinfo.extend_from_slice(&flags.to_le_bytes());
    ifinfo.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // change = ~0

    let mut payload = ifinfo;
    rtattr_push(&mut payload, IFLA_ADDRESS, &mac);
    let mut name_buf = Vec::with_capacity(name.len() + 1);
    name_buf.extend_from_slice(name.as_bytes());
    name_buf.push(0);
    rtattr_push(&mut payload, IFLA_IFNAME, &name_buf);
    rtattr_push(&mut payload, IFLA_MTU, &mtu.to_le_bytes());
    // IFLA_STATS: struct rtnl_link_stats (68 bytes is plenty for tools
    // that just walk the attribute list).
    rtattr_push(&mut payload, IFLA_STATS, &[0u8; 68]);

    let mut out = Vec::new();
    nlmsg_push(
        &mut out,
        RTM_NEWLINK,
        if multi { NLM_F_MULTI } else { 0 },
        seq,
        &payload,
    );
    out
}

/// Build one RTM_NEWADDR response message for an interface address.
fn build_addr_msg(seq: u32, index: u32, ip: u32, prefix: u8, name: &str, multi: bool) -> Vec<u8> {
    let mut ifaddr = Vec::new();
    ifaddr.push(2u8); // family = AF_INET
    ifaddr.push(prefix);
    ifaddr.push(0u8); // flags
    ifaddr.push(if index == LO_INDEX { 254u8 } else { RT_SCOPE_UNIVERSE }); // scope
    ifaddr.extend_from_slice(&index.to_le_bytes());

    let mut payload = ifaddr;
    let be = ip.to_be_bytes();
    rtattr_push(&mut payload, IFA_ADDRESS, &be);
    rtattr_push(&mut payload, IFA_LOCAL, &be);
    let mut label = Vec::with_capacity(name.len() + 1);
    label.extend_from_slice(name.as_bytes());
    label.push(0);
    rtattr_push(&mut payload, IFA_LABEL, &label);

    let mut out = Vec::new();
    nlmsg_push(
        &mut out,
        RTM_NEWADDR,
        if multi { NLM_F_MULTI } else { 0 },
        seq,
        &payload,
    );
    out
}

/// Build one RTM_NEWROUTE response message for a route entry.
fn build_route_msg(seq: u32, route: &crate::net::ipv4::route::RouteEntry, multi: bool) -> Vec<u8> {
    let mut rtm = Vec::new();
    rtm.push(2u8); // family = AF_INET
    rtm.push(mask_to_prefix(route.mask));
    rtm.push(0u8); // src_len
    rtm.push(0u8); // tos
    rtm.push(RT_TABLE_MAIN);
    rtm.push(RTPROT_STATIC);
    rtm.push(if route.gateway == 0 { RT_SCOPE_LINK } else { RT_SCOPE_UNIVERSE });
    rtm.push(1u8); // RTN_UNICAST
    rtm.extend_from_slice(&0u32.to_le_bytes()); // flags

    let mut payload = rtm;
    let dst = if route.mask == 0xFFFF_FFFF {
        route.dst.to_be_bytes()
    } else {
        (route.dst & route.mask).to_be_bytes()
    };
    rtattr_push(&mut payload, RTA_DST, &dst);
    if route.gateway != 0 {
        rtattr_push(&mut payload, RTA_GATEWAY, &route.gateway.to_be_bytes());
    }
    rtattr_push(&mut payload, RTA_OIF, &(route.oif as i32).to_le_bytes());

    let mut out = Vec::new();
    nlmsg_push(
        &mut out,
        RTM_NEWROUTE,
        if multi { NLM_F_MULTI } else { 0 },
        seq,
        &payload,
    );
    out
}

/// Build an NLMSG_ERROR (error = 0 means ACK).
fn build_error_msg(seq: u32, error: i32, orig_hdr: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&error.to_le_bytes());
    payload.extend_from_slice(orig_hdr);
    let mut out = Vec::new();
    nlmsg_push(&mut out, NLMSG_ERROR, 0, seq, &payload);
    out
}

/// Build the NLMSG_DONE terminator of a dump.
fn build_done_msg(seq: u32) -> Vec<u8> {
    let mut out = Vec::new();
    nlmsg_push(&mut out, NLMSG_DONE, NLM_F_MULTI, seq, &0i32.to_le_bytes());
    out
}

// ============================================================================
// Request attribute parsing
// ============================================================================

/// Walk the rtattr list of a request payload, invoking `f(type, data)` per
/// attribute. Payload must already be past the fixed sub-struct header.
fn for_each_rtattr(payload: &[u8], mut f: impl FnMut(u16, &[u8])) {
    let mut off = 0usize;
    while off + 4 <= payload.len() {
        let len = u16::from_le_bytes([payload[off], payload[off + 1]]) as usize;
        if len < 4 || off + len > payload.len() {
            return;
        }
        let rta_type = u16::from_le_bytes([payload[off + 2], payload[off + 3]]);
        f(rta_type, &payload[off + 4..off + len]);
        off += (len + 3) & !3;
    }
}

fn read_u32(data: &[u8]) -> Option<u32> {
    if data.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

// ============================================================================
// Netlink socket
// ============================================================================

/// AF_NETLINK socket.
pub struct NetlinkSocket {
    /// NETLINK_ROUTE
    pub protocol: i32,
    /// Assigned port id (the creating pid)
    pub portid: u32,
    /// Receive queue of complete response messages (one per recv).
    recv_queue: Spinlock<VecDeque<Vec<u8>>>,
    /// Closed flag (EOF for blocked receivers).
    closed: Spinlock<bool>,
    /// Wait queue for blocking recv.
    pub wait_queue: WaitQueueHead,
    /// Stored socket options (SOL_SOCKET subset).
    pub options: Spinlock<SocketOptions>,
}

// SAFETY: all mutable state is behind Spinlocks.
unsafe impl Sync for NetlinkSocket {}

impl NetlinkSocket {
    pub fn new(protocol: i32) -> Self {
        Self {
            protocol,
            portid: crate::sched::get_current_pid(),
            recv_queue: Spinlock::new(VecDeque::new()),
            closed: Spinlock::new(false),
            wait_queue: WaitQueueHead::new(),
            options: Spinlock::new(SocketOptions::new()),
        }
    }

    /// W3: SO_RCVTIMEO as an absolute jiffies deadline.
    pub fn rcvtimeo_deadline(&self) -> Option<u64> {
        let us = self.options.lock().rcvtimeo_us;
        if us == 0 {
            return None;
        }
        Some(crate::drivers::timer::get_jiffies() + (us / 10_000).max(1))
    }

    fn recv_ready(&self) -> bool {
        !self.recv_queue.lock().is_empty() || *self.closed.lock()
    }

    fn push_response(&self, msg: Vec<u8>) {
        self.recv_queue.lock().push_back(msg);
        self.wait_queue.wake_up_all();
    }
}

/// One blocking wait round (same discipline as unix.rs / socket.rs).
fn netlink_wait_round(
    sock: &Arc<NetlinkSocket>,
    deadline: Option<u64>,
) -> Result<(), i32> {
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return Err(-11),
    };
    sock.wait_queue.prepare_to_wait(current, false, true);

    if sock.recv_ready() {
        sock.wait_queue.finish_wait(current);
        crate::sched::dequeue_task(&*current);
        return Ok(());
    }
    if crate::signal::signal_pending() {
        sock.wait_queue.finish_wait(current);
        crate::sched::dequeue_task(&*current);
        return Err(-4); // EINTR
    }

    let timer_id = deadline
        .map(|dl| crate::timer::add_timer_wakeup(dl, crate::sched::get_current_pid()))
        .unwrap_or(0);
    if deadline.is_some() && timer_id == 0 {
        sock.wait_queue.finish_wait(current);
        crate::sched::dequeue_task(&*current);
        return Err(-12); // ENOMEM
    }

    crate::arch::riscv64::cpu::restore_irq(true);
    crate::sched::schedule();

    if timer_id != 0 {
        crate::timer::del_timer(timer_id);
    }
    sock.wait_queue.finish_wait(current);

    if crate::signal::signal_pending() {
        return Err(-4);
    }
    if let Some(dl) = deadline {
        if crate::drivers::timer::get_jiffies() >= dl {
            return Err(-11);
        }
    }
    Ok(())
}

// ============================================================================
// Request processing (send side)
// ============================================================================

/// Execute one parsed netlink request message and queue its responses.
fn netlink_exec(sock: &Arc<NetlinkSocket>, msg: &[u8]) {
    if msg.len() < 16 {
        return;
    }
    let msg_len = u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
    let msg_len = msg_len.min(msg.len());
    let msg_type = u16::from_le_bytes([msg[4], msg[5]]);
    let flags = u16::from_le_bytes([msg[6], msg[7]]);
    let seq = u32::from_le_bytes([msg[8], msg[9], msg[10], msg[11]]);
    let _pid = u32::from_le_bytes([msg[12], msg[13], msg[14], msg[15]]);
    let payload = &msg[16..msg_len];
    let orig_hdr = &msg[..16];

    let ack_requested = flags & NLM_F_ACK != 0;
    let _ = flags & NLM_F_REQUEST; // informational

    match msg_type {
        NLMSG_NOOP | NLMSG_ERROR | NLMSG_DONE => {}
        RTM_GETLINK => {
            iface_init_locked();
            let ifindex = if payload.len() >= 16 {
                // ifinfomsg.ifi_index at offset 4 (family, pad, type)
                read_u32(&payload[4..8]).unwrap_or(0)
            } else {
                0
            };
            if flags & NLM_F_DUMP != 0 {
                let entries: Vec<(u32, &'static str)> = IFACES
                    .lock()
                    .iter()
                    .map(|i| (i.index, i.name))
                    .collect();
                for (index, name) in entries {
                    sock.push_response(build_link_msg(seq, index, name, true));
                }
                sock.push_response(build_done_msg(seq));
            } else if ifindex != 0 {
                let found = IFACES
                    .lock()
                    .iter()
                    .find(|i| i.index == ifindex)
                    .map(|i| i.name);
                match found {
                    Some(name) => {
                        sock.push_response(build_link_msg(seq, ifindex, name, false));
                        if ack_requested {
                            sock.push_response(build_error_msg(seq, 0, orig_hdr));
                        }
                    }
                    None => sock.push_response(build_error_msg(seq, -19, orig_hdr)), // ENODEV
                }
            } else {
                sock.push_response(build_error_msg(seq, -22, orig_hdr)); // EINVAL
            }
        }
        RTM_NEWLINK => {
            // ifinfomsg: family, pad, type(u16), index(i32), flags(u32), change(u32)
            if payload.len() < 16 {
                sock.push_response(build_error_msg(seq, -22, orig_hdr));
                return;
            }
            let ifindex = read_u32(&payload[4..8]).unwrap_or(0);
            let ifi_flags = read_u32(&payload[8..12]).unwrap_or(0);
            let ifi_change = read_u32(&payload[12..16]).unwrap_or(0);
            iface_init_locked();
            let known = IFACES.lock().iter().any(|i| i.index == ifindex);
            if !known {
                // Creating new links is out of scope for this round.
                sock.push_response(build_error_msg(seq, -95, orig_hdr)); // EOPNOTSUPP
                return;
            }
            if ifi_change & IFF_UP != 0 {
                iface_set_up(ifindex, ifi_flags & IFF_UP != 0);
            }
            if ack_requested {
                sock.push_response(build_error_msg(seq, 0, orig_hdr));
            }
        }
        RTM_DELLINK => {
            sock.push_response(build_error_msg(seq, -95, orig_hdr)); // EOPNOTSUPP
        }
        RTM_GETADDR => {
            iface_init_locked();
            let ifindex = if payload.len() >= 8 {
                read_u32(&payload[4..8]).unwrap_or(0)
            } else {
                0
            };
            if flags & NLM_F_DUMP != 0 {
                let entries: Vec<(u32, &'static str, Vec<(u32, u8)>)> = IFACES
                    .lock()
                    .iter()
                    .map(|i| (i.index, i.name, i.addrs.clone()))
                    .collect();
                for (index, name, addrs) in entries {
                    for (ip, prefix) in addrs {
                        sock.push_response(build_addr_msg(seq, index, ip, prefix, name, true));
                    }
                }
                sock.push_response(build_done_msg(seq));
            } else if ifindex != 0 {
                let found = IFACES
                    .lock()
                    .iter()
                    .find(|i| i.index == ifindex)
                    .map(|i| (i.name, i.addrs.clone()));
                match found {
                    Some((name, addrs)) => {
                        for (ip, prefix) in addrs {
                            sock.push_response(build_addr_msg(seq, ifindex, ip, prefix, name, false));
                        }
                        if ack_requested {
                            sock.push_response(build_error_msg(seq, 0, orig_hdr));
                        }
                    }
                    None => sock.push_response(build_error_msg(seq, -19, orig_hdr)),
                }
            } else {
                sock.push_response(build_error_msg(seq, -22, orig_hdr));
            }
        }
        RTM_NEWADDR => {
            // ifaddrmsg: family, prefixlen, flags, scope, ifindex(u32)
            if payload.len() < 8 {
                sock.push_response(build_error_msg(seq, -22, orig_hdr));
                return;
            }
            let prefix = payload[1];
            let ifindex = read_u32(&payload[4..8]).unwrap_or(0);
            let mut local: Option<u32> = None;
            for_each_rtattr(&payload[8..], |t, d| {
                if (t == IFA_LOCAL || t == IFA_ADDRESS) && d.len() >= 4 {
                    let be = [d[0], d[1], d[2], d[3]];
                    if t == IFA_LOCAL || local.is_none() {
                        local = Some(u32::from_be_bytes(be));
                    }
                }
            });
            match local {
                Some(ip) => {
                    if iface_set_addr(ifindex, ip, prefix) {
                        if ack_requested {
                            sock.push_response(build_error_msg(seq, 0, orig_hdr));
                        }
                    } else {
                        sock.push_response(build_error_msg(seq, -19, orig_hdr));
                    }
                }
                None => sock.push_response(build_error_msg(seq, -22, orig_hdr)),
            }
        }
        RTM_DELADDR => {
            if payload.len() < 8 {
                sock.push_response(build_error_msg(seq, -22, orig_hdr));
                return;
            }
            let ifindex = read_u32(&payload[4..8]).unwrap_or(0);
            iface_init_locked();
            let mut ifaces = IFACES.lock();
            if let Some(entry) = ifaces.iter_mut().find(|i| i.index == ifindex) {
                entry.addrs.clear();
                drop(ifaces);
                if ack_requested {
                    sock.push_response(build_error_msg(seq, 0, orig_hdr));
                }
            } else {
                drop(ifaces);
                sock.push_response(build_error_msg(seq, -19, orig_hdr));
            }
        }
        RTM_GETROUTE => {
            let routes = crate::net::ipv4::route::route_dump();
            for route in &routes {
                sock.push_response(build_route_msg(seq, route, true));
            }
            sock.push_response(build_done_msg(seq));
        }
        RTM_NEWROUTE => {
            // rtmsg: family, dst_len, src_len, tos, table, proto, scope, type, flags(u32)
            if payload.len() < 12 {
                sock.push_response(build_error_msg(seq, -22, orig_hdr));
                return;
            }
            let dst_len = payload[1] as u32;
            let mut dst = 0u32;
            let mut gw = 0u32;
            let mut oif = 0u32;
            for_each_rtattr(&payload[12..], |t, d| {
                match t {
                    RTA_DST => {
                        if d.len() >= 4 {
                            dst = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
                        }
                    }
                    RTA_GATEWAY => {
                        if d.len() >= 4 {
                            gw = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
                        }
                    }
                    RTA_OIF => {
                        if let Some(v) = read_u32(d) {
                            oif = v;
                        }
                    }
                    _ => {}
                }
            });
            let mask = if dst_len == 0 {
                0
            } else if dst_len >= 32 {
                0xFFFF_FFFF
            } else {
                (!0u32) << (32 - dst_len)
            };
            if crate::net::ipv4::route::route_add(dst, mask, gw, oif, 1500).is_ok() {
                if ack_requested {
                    sock.push_response(build_error_msg(seq, 0, orig_hdr));
                }
            } else {
                sock.push_response(build_error_msg(seq, -12, orig_hdr)); // ENOMEM (table full)
            }
        }
        RTM_DELROUTE => {
            if payload.len() < 12 {
                sock.push_response(build_error_msg(seq, -22, orig_hdr));
                return;
            }
            let dst_len = payload[1] as u32;
            let mut dst = 0u32;
            for_each_rtattr(&payload[12..], |t, d| {
                if t == RTA_DST && d.len() >= 4 {
                    dst = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
                }
            });
            let mask = if dst_len >= 32 {
                0xFFFF_FFFF
            } else {
                (!0u32) << (32 - dst_len.min(31))
            };
            let mask = if dst_len == 0 { 0 } else { mask };
            if crate::net::ipv4::route::route_remove(dst, mask) {
                if ack_requested {
                    sock.push_response(build_error_msg(seq, 0, orig_hdr));
                }
            } else {
                sock.push_response(build_error_msg(seq, -19, orig_hdr)); // ENODEV (no such route → ESRCH really)
            }
        }
        _ => {
            sock.push_response(build_error_msg(seq, -95, orig_hdr)); // EOPNOTSUPP
        }
    }
}

/// sendmsg/sendto/write: parse a buffer of one or more netlink requests
/// and execute them (responses are queued for recv).
pub fn netlink_send(sock: &Arc<NetlinkSocket>, buf: &[u8]) -> Result<usize, i32> {
    let mut off = 0usize;
    while off + 16 <= buf.len() {
        let len = u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]) as usize;
        if len < 16 || off + len > buf.len() {
            return Err(-22); // EINVAL — malformed message
        }
        netlink_exec(sock, &buf[off..off + len]);
        off += (len + 3) & !3;
    }
    if off == 0 && !buf.is_empty() {
        return Err(-22); // truncated header
    }
    Ok(buf.len())
}

/// recvmsg/recvfrom/read: pop exactly one response message (blocking
/// per `nonblock`/`deadline`).
pub fn netlink_recv(
    sock: &Arc<NetlinkSocket>,
    buf: &mut [u8],
    nonblock: bool,
    deadline: Option<u64>,
) -> Result<usize, i32> {
    loop {
        if let Some(msg) = sock.recv_queue.lock().pop_front() {
            let n = msg.len().min(buf.len());
            buf[..n].copy_from_slice(&msg[..n]);
            return Ok(n);
        }
        if *sock.closed.lock() {
            return Ok(0); // EOF
        }
        if nonblock {
            return Err(-11); // EAGAIN
        }
        netlink_wait_round(sock, deadline)?;
    }
}

/// close(): mark EOF and wake blocked receivers.
pub fn netlink_close(sock: &Arc<NetlinkSocket>) {
    *sock.closed.lock() = true;
    sock.wait_queue.wake_up_all();
}

// ============================================================================
// File operations
// ============================================================================

/// Recover a strong Arc<NetlinkSocket> from a File's private_data.
/// SAFETY: the caller must have identity-checked the File against
/// NETLINK_OPS first.
unsafe fn netlink_of_file(file: &File) -> Option<Arc<NetlinkSocket>> {
    let ptr = (*file.private_data.get())?;
    let socket_ptr = ptr as *const NetlinkSocket;
    Arc::increment_strong_count(socket_ptr);
    Some(Arc::from_raw(socket_ptr))
}

fn netlink_file_read(file: &File, buf: &mut [u8]) -> isize {
    // SAFETY: ops identity was verified — created by netlink_socket_install.
    let socket = match unsafe { netlink_of_file(file) } {
        Some(s) => s,
        None => return -9,
    };
    let nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;
    let deadline = socket.rcvtimeo_deadline();
    match netlink_recv(&socket, buf, nonblock, deadline) {
        Ok(n) => n as isize,
        Err(e) => e as isize,
    }
}

fn netlink_file_write(file: &File, buf: &[u8]) -> isize {
    // SAFETY: ops identity was verified (see netlink_file_read).
    let socket = match unsafe { netlink_of_file(file) } {
        Some(s) => s,
        None => return -9,
    };
    match netlink_send(&socket, buf) {
        Ok(n) => n as isize,
        Err(e) => e as isize,
    }
}

fn netlink_file_close(file: &File) -> i32 {
    // SAFETY: private_data came from Arc::into_raw(Arc<NetlinkSocket>).
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };
    unsafe { *file.private_data.get() = None; }
    // SAFETY: reconstruct the leaked Arc so close runs and the ref drops.
    let socket = unsafe { Arc::from_raw(ptr as *const NetlinkSocket) };
    netlink_close(&socket);
    0
}

fn netlink_file_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    // SAFETY: ops identity was verified (see netlink_file_read).
    let socket = match unsafe { netlink_of_file(file) } {
        Some(s) => s,
        None => return POLLERR,
    };
    let mut ready = 0u16;
    if events & POLLIN != 0 && socket.recv_ready() {
        ready |= POLLIN | POLLRDNORM;
    }
    if events & POLLOUT != 0 {
        ready |= POLLOUT | POLLWRNORM; // requests execute immediately
    }
    ready
}

/// AF_NETLINK socket file operations.
pub static NETLINK_OPS: FileOps = FileOps {
    read: Some(netlink_file_read),
    write: Some(netlink_file_write),
    lseek: None,
    close: Some(netlink_file_close),
    poll: Some(netlink_file_poll),
};

// ============================================================================
// Socket creation / fd plumbing
// ============================================================================

fn netlink_socket_install(
    socket: &Arc<NetlinkSocket>,
    nonblock: bool,
    cloexec: bool,
) -> Result<usize, i32> {
    let flags = FileFlags::O_RDWR | if nonblock { FileFlags::O_NONBLOCK } else { 0 };
    let file = Arc::new(File::new(FileFlags::new(flags)));
    file.set_ops(&NETLINK_OPS);
    file.set_private_data(Arc::into_raw(socket.clone()) as *mut u8);

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(t) => t,
        None => {
            unwind_netlink_file(&file);
            return Err(-9);
        }
    };
    let fd = match fdtable.alloc_fd() {
        Some(f) => f,
        None => {
            unwind_netlink_file(&file);
            return Err(-24);
        }
    };
    if fdtable.install_fd(fd, file.clone()).is_err() {
        unwind_netlink_file(&file);
        return Err(-24);
    }
    if cloexec {
        fdtable.set_fd_cloexec(fd, true);
    }
    Ok(fd)
}

fn unwind_netlink_file(file: &Arc<File>) {
    // SAFETY: the raw pointer came from Arc::into_raw above; sole owner.
    let ptr = unsafe { *file.private_data.get() };
    if let Some(ptr) = ptr {
        unsafe { drop(Arc::from_raw(ptr as *const NetlinkSocket)); }
    }
}

/// socket(AF_NETLINK, type, protocol) — entry from sys_socket_create.
pub fn netlink_socket_create(type_: i32, protocol: i32) -> Result<usize, i32> {
    const KNOWN_TYPE_FLAGS: i32 = SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG;
    if type_ & !(SOCK_TYPE_MASK | KNOWN_TYPE_FLAGS) != 0 {
        return Err(-22); // EINVAL
    }
    match type_ & SOCK_TYPE_MASK {
        SOCK_RAW | crate::net::socket::SOCK_DGRAM => {}
        _ => return Err(-94), // ESOCKTNOSUPPORT
    }
    if protocol != NETLINK_ROUTE {
        return Err(-93); // EPROTONOSUPPORT
    }
    let nonblock = (type_ & SOCK_NONBLOCK_FLAG) != 0;
    let cloexec = (type_ & SOCK_CLOEXEC_FLAG) != 0;
    let socket = Arc::new(NetlinkSocket::new(protocol));
    netlink_socket_install(&socket, nonblock, cloexec)
}

/// Resolve a process fd to its NetlinkSocket (identity-checked).
pub fn netlink_socket_from_fd(fd: usize) -> Option<Arc<NetlinkSocket>> {
    let fdtable = crate::sched::get_current_fdtable()?;
    let file = fdtable.get_file(fd)?;
    {
        let ops = unsafe { *file.ops.get() };
        match ops {
            Some(ops) if core::ptr::eq(ops, &NETLINK_OPS) => {}
            _ => return None,
        }
    }
    // SAFETY: ops identity confirmed; private_data is a leaked Arc.
    unsafe { netlink_of_file(&file) }
}

/// (NetlinkSocket, file O_NONBLOCK) for a fd.
pub fn netlink_file_of(fd: usize) -> Option<(Arc<NetlinkSocket>, bool)> {
    let fdtable = crate::sched::get_current_fdtable()?;
    let file = fdtable.get_file(fd)?;
    {
        let ops = unsafe { *file.ops.get() };
        match ops {
            Some(ops) if core::ptr::eq(ops, &NETLINK_OPS) => {}
            _ => return None,
        }
    }
    let nonblock = (file.flags().bits() & FileFlags::O_NONBLOCK) != 0;
    // SAFETY: ops identity confirmed above.
    let socket = unsafe { netlink_of_file(&file) }?;
    Some((socket, nonblock))
}

/// Write a sockaddr_nl { AF_NETLINK, pid=0(kernel), groups=0 } to user.
/// SAFETY: addr_ptr/addrlen_ptr access_ok-validated for 12/4 bytes.
pub unsafe fn put_sockaddr_nl(addr_ptr: *mut u8, addrlen_ptr: *mut u32) {
    put_sockaddr_nl_bound(addr_ptr, addrlen_ptr, 0);
}

/// Write a sockaddr_nl { AF_NETLINK, pid=portid, groups=0 } to user
/// (getsockname on a bound netlink socket).
/// SAFETY: addr_ptr/addrlen_ptr access_ok-validated for 12/4 bytes.
pub unsafe fn put_sockaddr_nl_bound(addr_ptr: *mut u8, addrlen_ptr: *mut u32, portid: u32) {
    use crate::arch::riscv64::uaccess::{copy_to_user, put_user};
    let mut buf = [0u8; SOCKADDR_NL_LEN];
    buf[0] = AF_NETLINK as u8;
    buf[4..8].copy_from_slice(&portid.to_le_bytes());
    let _ = copy_to_user(addr_ptr, buf.as_ptr(), SOCKADDR_NL_LEN);
    let _ = put_user(addrlen_ptr, SOCKADDR_NL_LEN as u32);
}

// ============================================================================
// Interface management ioctls (SIOCGIFCONF family)
// ============================================================================

/// SIOCGIFCONF (0x8912)
const SIOCGIFCONF: u32 = 0x8912;
/// SIOCGIFFLAGS (0x8913)
const SIOCGIFFLAGS: u32 = 0x8913;
/// SIOCSIFFLAGS (0x8914)
const SIOCSIFFLAGS: u32 = 0x8914;
/// SIOCGIFADDR (0x8915)
const SIOCGIFADDR: u32 = 0x8915;
/// SIOCSIFADDR (0x8916)
const SIOCSIFADDR: u32 = 0x8916;
/// SIOCGIFMTU (0x8921)
const SIOCGIFMTU: u32 = 0x8921;
/// SIOCGIFHWADDR (0x8927)
const SIOCGIFHWADDR: u32 = 0x8927;
/// SIOCGIFINDEX (0x8933)
const SIOCGIFINDEX: u32 = 0x8933;

/// sizeof(struct ifreq) on 64-bit Linux/musl (name[16] + 24-byte union).
const IFREQ_SIZE: usize = 40;

/// Is this request one of the interface-management ioctls we handle?
pub fn is_if_ioctl(request: u32) -> bool {
    matches!(
        request,
        SIOCGIFCONF
            | SIOCGIFFLAGS
            | SIOCSIFFLAGS
            | SIOCGIFADDR
            | SIOCSIFADDR
            | SIOCGIFMTU
            | SIOCGIFHWADDR
            | SIOCGIFINDEX
    )
}

/// Interface-management ioctl dispatcher. `arg` is the user pointer to an
/// ifreq (or an ifconf for SIOCGIFCONF). Returns 0 or a negative errno.
pub fn net_if_ioctl(request: u32, arg: usize) -> i64 {
    use crate::arch::riscv64::uaccess::{access_ok, copy_from_user, copy_to_user, get_user, put_user};

    if arg == 0 {
        return -(constants::EINVAL as i64);
    }

    if request == SIOCGIFCONF {
        // struct ifconf { int len; pad; struct ifreq *ifc_req; }
        if !access_ok(arg, 16) {
            return -(constants::EFAULT as i64);
        }
        // SAFETY: access_ok(16) validated the range; exception-table copy.
        let len = unsafe { get_user::<i32>(arg as *const i32).unwrap_or(0) };
        // struct ifconf on 64-bit: { int len; pad(4); struct ifreq *req; }
        let req_ptr = unsafe { get_user::<usize>((arg + 8) as *const usize).unwrap_or(0) };
        if len < 0 {
            return -(constants::EINVAL as i64);
        }
        if len == 0 || req_ptr == 0 {
            // len 0: report required size.
            // SAFETY: arg validated above.
            unsafe {
                let _ = put_user(arg as *mut i32, 2 * IFREQ_SIZE as i32);
            }
            return 0;
        }
        let count = (len as usize / IFREQ_SIZE).min(2);
        if !access_ok(req_ptr, count * IFREQ_SIZE) {
            return -(constants::EFAULT as i64);
        }
        iface_init_locked();
        let entries: Vec<(u32, &'static str)> = IFACES
            .lock()
            .iter()
            .map(|i| (i.index, i.name))
            .collect();
        let mut used = 0usize;
        let mut kbuf = alloc::vec![0u8; count * IFREQ_SIZE];
        for (i, (index, name)) in entries.iter().enumerate().take(count) {
            let base = i * IFREQ_SIZE;
            kbuf[base..base + name.len()].copy_from_slice(name.as_bytes());
            // ifr_addr sockaddr_in at offset 16: family=AF_INET, port 0, addr BE
            let ip = iface_get_addr(*index).unwrap_or(0);
            kbuf[base + 16] = 2;
            kbuf[base + 17] = 0;
            let be = ip.to_be_bytes();
            kbuf[base + 20..base + 24].copy_from_slice(&be);
            used += IFREQ_SIZE;
        }
        // SAFETY: req_ptr validated with access_ok.
        if unsafe { copy_to_user(req_ptr as *mut u8, kbuf.as_ptr(), used) } != 0 {
            return -(constants::EFAULT as i64);
        }
        // SAFETY: arg validated above.
        unsafe {
            let _ = put_user(arg as *mut i32, used as i32);
        }
        return 0;
    }

    // Everything else operates on a struct ifreq.
    if !access_ok(arg, IFREQ_SIZE) {
        return -(constants::EFAULT as i64);
    }
    let mut ifreq = [0u8; IFREQ_SIZE];
    // SAFETY: access_ok(IFREQ_SIZE) validated; exception-table copy.
    if unsafe { copy_from_user(ifreq.as_mut_ptr(), arg as *const u8, IFREQ_SIZE) } != 0 {
        return -(constants::EFAULT as i64);
    }
    let name_len = ifreq[..16].iter().position(|&c| c == 0).unwrap_or(16);
    let name = match core::str::from_utf8(&ifreq[..name_len]) {
        Ok(n) => n,
        Err(_) => return -(constants::EINVAL as i64),
    };
    let index = match iface_find(name) {
        Some(i) => i,
        None => return -(constants::ENODEV as i64),
    };

    let put_ifreq = |ifreq: &[u8; IFREQ_SIZE]| -> i64 {
        // SAFETY: arg validated with access_ok(IFREQ_SIZE).
        if unsafe { copy_to_user(arg as *mut u8, ifreq.as_ptr(), IFREQ_SIZE) } != 0 {
            return -(constants::EFAULT as i64);
        }
        0
    };

    match request {
        SIOCGIFADDR => {
            let ip = iface_get_addr(index).unwrap_or(0);
            ifreq[16] = 2; // AF_INET
            let be = ip.to_be_bytes();
            ifreq[20..24].copy_from_slice(&be);
            put_ifreq(&ifreq)
        }
        SIOCSIFADDR => {
            // sockaddr_in at offset 16: family(2), port(2), addr(4) at +4
            if ifreq[16] != 2 {
                return -(constants::EINVAL as i64);
            }
            let ip = u32::from_be_bytes([
                ifreq[20], ifreq[21], ifreq[22], ifreq[23],
            ]);
            if !iface_set_addr(index, ip, 24) {
                return -(constants::ENODEV as i64);
            }
            0
        }
        SIOCGIFFLAGS => {
            let (flags, _, _, _) = iface_hw(index);
            let f = (flags & 0xFFFF) as u16;
            ifreq[16..18].copy_from_slice(&f.to_le_bytes());
            put_ifreq(&ifreq)
        }
        SIOCSIFFLAGS => {
            let f = u16::from_le_bytes([ifreq[16], ifreq[17]]) as u32;
            iface_set_up(index, f & IFF_UP != 0);
            0
        }
        SIOCGIFMTU => {
            let (_, mtu, _, _) = iface_hw(index);
            ifreq[16..20].copy_from_slice(&mtu.to_le_bytes());
            put_ifreq(&ifreq)
        }
        SIOCGIFHWADDR => {
            let (_, _, mac, htype) = iface_hw(index);
            ifreq[16..18].copy_from_slice(&htype.to_le_bytes()); // sa_family = ARPHRD_*
            ifreq[18..24].copy_from_slice(&mac);
            put_ifreq(&ifreq)
        }
        SIOCGIFINDEX => {
            ifreq[16..20].copy_from_slice(&index.to_le_bytes());
            put_ifreq(&ifreq)
        }
        _ => -(constants::ENOTTY as i64),
    }
}
