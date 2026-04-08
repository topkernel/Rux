//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for network device flags and ArpHrdType constants.
//! Copied from: kernel/src/drivers/net/space.rs

use proptest::prelude::*;

pub const IFNAMSIZ: usize = 16;
pub const MAX_ADDR_LEN: usize = 32;

// Copied ArpHrdType
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpHrdType {
    ARPHRD_LOOPBACK = 772,
    ARPHRD_ETHER = 1,
    ARPHRD_VOID = 0xFFFF,
}

// Copied dev_flags module
pub mod dev_flags {
    pub const IFF_UP: u32 = 0x1;
    pub const IFF_BROADCAST: u32 = 0x2;
    pub const IFF_LOOPBACK: u32 = 0x8;
    pub const IFF_RUNNING: u32 = 0x40;
    pub const IFF_MULTICAST: u32 = 0x1000;
}

// Copied DeviceStats
#[derive(Debug, Default, Clone, Copy)]
pub struct DeviceStats {
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
    pub multicast: u64,
}

// Simplified NetDevice flag operations (no unsafe, no function pointers)
pub struct NetDeviceFlags {
    pub flags: u32,
}

impl NetDeviceFlags {
    pub fn new() -> Self {
        Self { flags: 0 }
    }

    pub fn up(&mut self) {
        self.flags |= dev_flags::IFF_UP | dev_flags::IFF_RUNNING;
    }

    pub fn down(&mut self) {
        self.flags &= !(dev_flags::IFF_UP | dev_flags::IFF_RUNNING);
    }

    pub fn is_up(&self) -> bool {
        (self.flags & dev_flags::IFF_UP) != 0
    }

    pub fn is_running(&self) -> bool {
        (self.flags & dev_flags::IFF_RUNNING) != 0
    }
}

proptest! {
    #[test]
    fn test_dev_flags_distinct(_v in 0u8..1u8) {
        let flags = [
            dev_flags::IFF_UP,
            dev_flags::IFF_BROADCAST,
            dev_flags::IFF_LOOPBACK,
            dev_flags::IFF_RUNNING,
            dev_flags::IFF_MULTICAST,
        ];
        for i in 0..flags.len() {
            for j in (i+1)..flags.len() {
                assert_eq!(flags[i] & flags[j], 0,
                    "IFF flags {} and {} overlap", i, j);
            }
        }
    }

    #[test]
    fn test_dev_flags_powers_of_two(_v in 0u8..1u8) {
        let flags = [
            dev_flags::IFF_UP,
            dev_flags::IFF_BROADCAST,
            dev_flags::IFF_LOOPBACK,
            dev_flags::IFF_RUNNING,
            dev_flags::IFF_MULTICAST,
        ];
        for &f in &flags {
            assert!(f > 0 && (f & (f - 1)) == 0,
                "IFF flag {:#x} not power of two", f);
        }
    }

    #[test]
    fn test_up_sets_both_flags(_v in 0u8..1u8) {
        let mut dev = NetDeviceFlags::new();
        dev.up();
        assert!(dev.is_up());
        assert!(dev.is_running());
    }

    #[test]
    fn test_down_clears_both_flags(_v in 0u8..1u8) {
        let mut dev = NetDeviceFlags::new();
        dev.up();
        dev.down();
        assert!(!dev.is_up());
        assert!(!dev.is_running());
    }

    #[test]
    fn test_up_down_up(flags in 0u32..0x2000u32) {
        let mut dev = NetDeviceFlags::new();
        dev.flags = flags;
        dev.up();
        assert!(dev.is_up());
        dev.down();
        assert!(!dev.is_up());
        dev.up();
        assert!(dev.is_up());
    }

    #[test]
    fn test_down_preserves_other_flags(initial_flags in 0u32..0x2000u32) {
        let mut dev = NetDeviceFlags::new();
        dev.flags = initial_flags;
        let other = initial_flags & !(dev_flags::IFF_UP | dev_flags::IFF_RUNNING);
        dev.up();
        dev.down();
        // Other flags preserved
        assert_eq!(dev.flags & !(dev_flags::IFF_UP | dev_flags::IFF_RUNNING), other);
    }

    #[test]
    fn test_arp_hrd_type_distinct(_v in 0u8..1u8) {
        let types = [
            ArpHrdType::ARPHRD_LOOPBACK as i32,
            ArpHrdType::ARPHRD_ETHER as i32,
            ArpHrdType::ARPHRD_VOID as i32,
        ];
        for i in 0..types.len() {
            for j in (i+1)..types.len() {
                assert_ne!(types[i], types[j]);
            }
        }
    }

    #[test]
    fn test_arp_hrd_type_values(_v in 0u8..1u8) {
        assert_eq!(ArpHrdType::ARPHRD_LOOPBACK as i32, 772);
        assert_eq!(ArpHrdType::ARPHRD_ETHER as i32, 1);
        assert_eq!(ArpHrdType::ARPHRD_VOID as i32, 0xFFFF);
    }

    #[test]
    fn test_device_stats_default(_v in 0u8..1u8) {
        let stats = DeviceStats::default();
        assert_eq!(stats.rx_packets, 0);
        assert_eq!(stats.tx_packets, 0);
        assert_eq!(stats.rx_errors, 0);
        assert_eq!(stats.tx_errors, 0);
    }

    #[test]
    fn test_device_stats_copy(stats_rx in 0u64..10000u64, stats_tx in 0u64..10000u64) {
        let mut stats = DeviceStats::default();
        stats.rx_packets = stats_rx;
        stats.tx_packets = stats_tx;
        let copy = stats;
        assert_eq!(copy.rx_packets, stats_rx);
        assert_eq!(copy.tx_packets, stats_tx);
    }

    #[test]
    fn test_iff_up_value(_v in 0u8..1u8) {
        assert_eq!(dev_flags::IFF_UP, 0x1);
        assert_eq!(dev_flags::IFF_BROADCAST, 0x2);
        assert_eq!(dev_flags::IFF_LOOPBACK, 0x8);
        assert_eq!(dev_flags::IFF_RUNNING, 0x40);
        assert_eq!(dev_flags::IFF_MULTICAST, 0x1000);
    }
}
