//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network header pins — ported from the dead #[cfg(test)] blocks in
//! net/ethernet.rs and net/ipv4/mod.rs per the test-layout policy (those
//! host-side blocks never compiled in this no_std crate; these assertions
//! never actually ran until now). Originally kernel/src/tests/net_hdrs.rs;
//! moved with the rung-2 crate flip (the test follows the files it pins)
//! and runs in-guest through the SERVICE-TEST SEAM (rux-bin registers
//! rux_net::run_unit_tests; the framework's pass/fail counters are shared,
//! so the pins land in the same suite totals).

use crate::net::ethernet::{
    eth_is_broadcast_addr, eth_is_multicast_addr, eth_is_valid_unicast_addr, EthHdr,
};
use crate::net::ipv4::IpHdr;

pub fn test_net_hdrs() {
    rux::tests::test_group_start("net_hdrs");

    // Ethernet header ABI size.
    rux::test_assert_eq!(core::mem::size_of::<EthHdr>(), 14, "EthHdr == 14 bytes");

    // Address class pins.
    let broadcast: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
    rux::test_assert!(eth_is_broadcast_addr(&broadcast), "ff:ff:ff:ff:ff:ff is broadcast");
    rux::test_assert!(eth_is_multicast_addr(&broadcast), "broadcast is also multicast");

    let multicast: [u8; 6] = [0x01, 0x00, 0x5E, 0x00, 0x00, 0x01];
    rux::test_assert!(eth_is_multicast_addr(&multicast), "01:00:5e:... is multicast");
    rux::test_assert!(!eth_is_broadcast_addr(&multicast), "multicast is not broadcast");

    let unicast: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    rux::test_assert!(!eth_is_multicast_addr(&unicast), "52:54:00:... is unicast");
    rux::test_assert!(!eth_is_broadcast_addr(&unicast), "unicast is not broadcast");
    rux::test_assert!(eth_is_valid_unicast_addr(&unicast), "valid unicast");

    // IPv4 header ABI size and version/IHL nibbles.
    rux::test_assert_eq!(core::mem::size_of::<IpHdr>(), 20, "IpHdr == 20 bytes");
    let mut hdr = IpHdr::default();
    hdr.version_ihl = 0x45;
    rux::test_assert_eq!(hdr.version_ihl >> 4, 4, "version nibble");
    rux::test_assert_eq!(hdr.version_ihl & 0x0F, 5, "IHL nibble (5 words = 20 bytes)");
}
