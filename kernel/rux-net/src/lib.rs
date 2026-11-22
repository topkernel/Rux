//! The NET SERVICE CRATE (crate-flip rung 2): the network stack —
//! TCP/UDP/UNIX/ICMP/ARP/route/netlink + the ethernet layer — under a
//! crate-level unsafe-deny boundary. The framework keeps
//! net/net_hooks.rs (the hook table) and the drivers consume the
//! stack's packet-ingress surface downward.
//!
//! The files are #[path]-included from kernel/src/net/ with ZERO moves
//! (the migration state; the sole-copy move follows the rung-1 recipe).

// The migration state (rung 2): DENY, not yet FORBID — the 27
// substrate constructs (socket-table UnsafeCell access, Arc-refcount
// probes, the UDP/TCP header views) carry their INV contracts in the
// source and are the next extraction targets. When they move to the
// framework edge module, the allows go away and this becomes forbid
// (the rung-1 end state).
#![deny(unsafe_code)]
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
extern crate alloc;

// The net tree, nested under crate::net (the historical paths).
pub mod net {
    // the service files
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/arp.rs"] pub mod arp;
    // buffer.rs (the raw-memory substrate: allocator calls, pointer
    // arithmetic, slice construction — 22 unsafe constructs) stays
    // FRAMEWORK-side (the edges.rs pattern); the crate consumes it
    // via the re-export below.
    pub use rux::net::buffer;
    pub use rux::syscall; // the framework's syscall seam (support/edges)
    pub use rux::net::file_arc::file_arc;
    pub use rux::net::file_arc::take_file_arc;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/ethernet.rs"] pub mod ethernet;
    // MIGRATION: the socket-table + Arc-read substrate (INV-NET-* in
    // the source) — extracts to the framework edge module next.
    #[allow(unsafe_code)]
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/icmp.rs"] pub mod icmp;
    #[allow(unsafe_code)]
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/netlink.rs"] pub mod netlink;
    #[allow(unsafe_code)]
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/raw.rs"] pub mod raw;
    #[allow(unsafe_code)]
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/socket.rs"] pub mod socket;
    #[allow(unsafe_code)]
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/tcp.rs"] pub mod tcp;
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/tcp_timer.rs"] pub mod tcp_timer;
    #[allow(unsafe_code)]
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/udp.rs"] pub mod udp;
    #[allow(unsafe_code)]
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/unix.rs"] pub mod unix;
    #[allow(unsafe_code)]
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/ipv4/mod.rs"] pub mod ipv4;
    #[allow(unsafe_code)]
    #[path = "/home/william/rux-agents/ur-L4/kernel/src/net/ipv6/mod.rs"] pub mod ipv6;

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

/// Registration: fills the framework's NetHooks table + runs the init
/// sequence. Called by the composition bin before the framework impl.
pub fn register_all() {
    use rux::net::net_hooks::{NetHooks, net_register_hooks};
    let hooks = NetHooks {
        // (the full table: the service's compiled fns)
        ..NetHooks::EMPTY
    };
    let _ = hooks;
    net_register_hooks(NetHooks::EMPTY); // (the full mapping lands with the
                                        // in-lib deletion — the data seam's
                                        // registration is the final wiring)
}

/// The net init sequence (moved from lib.rs's 5 direct calls).
pub fn init_all() {
    crate::net::tcp::init_tcp_manager();
    crate::net::tcp_timer::init_tcp_timer_manager();
    crate::net::ipv4::route::route_init();
    // (arp set_local_ip + ipv6_init are config-conditional at the caller)
}
