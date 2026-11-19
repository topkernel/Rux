//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ARP cache and packet parsing invariant tests.
//!
//! Types copied from: kernel/src/net/arp.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/net/arp.rs
// ============================================================================

pub const ETH_ALEN: usize = 6;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArpEntry {
    pub ip: u32,
    pub mac: [u8; ETH_ALEN],
    pub last_updated: u64,
    pub valid: bool,
}

impl ArpEntry {
    pub fn new(ip: u32, mac: [u8; ETH_ALEN]) -> Self {
        Self {
            ip,
            mac,
            last_updated: 0,
            valid: true,
        }
    }

    pub fn is_expired(&self, timeout: u64) -> bool {
        false
    }
}

/// Verify-local ArpCache using Vec instead of fixed-size array.
pub struct ArpCache {
    entries: Vec<ArpEntry>,
}

const ARP_CACHE_SIZE: usize = 64;

impl ArpCache {
    pub fn new() -> Self {
        Self { entries: Vec::with_capacity(ARP_CACHE_SIZE) }
    }

    pub fn lookup(&self, ip: u32) -> Option<ArpEntry> {
        for entry in self.entries.iter() {
            if entry.valid && entry.ip == ip {
                return Some(*entry);
            }
        }
        None
    }

    pub fn update(&mut self, ip: u32, mac: [u8; ETH_ALEN]) {
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.ip == ip {
                entry.mac = mac;
                entry.last_updated = 0;
                return;
            }
        }
        if self.entries.len() < ARP_CACHE_SIZE {
            self.entries.push(ArpEntry::new(ip, mac));
        } else {
            self.entries[0] = ArpEntry::new(ip, mac);
        }
    }

    pub fn remove(&mut self, ip: u32) {
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.ip == ip {
                entry.valid = false;
                return;
            }
        }
    }

    pub fn clear(&mut self) {
        for entry in self.entries.iter_mut() {
            entry.valid = false;
        }
    }

    pub fn count(&self) -> usize {
        self.entries.iter().filter(|e| e.valid).count()
    }

    #[allow(dead_code)]
    pub fn is_full(&self) -> bool {
        self.entries.len() >= ARP_CACHE_SIZE
    }
}

/// ARP operation types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpOp {
    ARPOP_REQUEST = 1,
    ARPOP_REPLY = 2,
    ARPOP_RREQUEST = 3,
    ARPOP_RREPLY = 4,
}

/// ARP packet header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArpHdr {
    pub ar_hrd: u16,
    pub ar_pro: u16,
    pub ar_hln: u8,
    pub ar_pln: u8,
    pub ar_op: u16,
}

/// ARP packet (Ethernet + IPv4)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArpPacket {
    pub hdr: ArpHdr,
    pub ar_sha: [u8; ETH_ALEN],
    pub ar_sip: u32,
    pub ar_tha: [u8; ETH_ALEN],
    pub ar_tip: u32,
}

impl ArpPacket {
    pub const LEN: usize = core::mem::size_of::<ArpPacket>();

    pub fn from_bytes(data: &[u8]) -> Option<&ArpPacket> {
        if data.len() < Self::LEN {
            return None;
        }
        unsafe { Some(&*(data.as_ptr() as *const ArpPacket)) }
    }

    pub fn is_request(&self) -> bool {
        u16::from_be(self.hdr.ar_op) == ArpOp::ARPOP_REQUEST as u16
    }

    pub fn is_reply(&self) -> bool {
        u16::from_be(self.hdr.ar_op) == ArpOp::ARPOP_REPLY as u16
    }

    pub fn sender_mac(&self) -> [u8; ETH_ALEN] {
        self.ar_sha
    }

    pub fn sender_ip(&self) -> u32 {
        u32::from_be(self.ar_sip)
    }

    pub fn target_mac(&self) -> [u8; ETH_ALEN] {
        self.ar_tha
    }

    pub fn target_ip(&self) -> u32 {
        u32::from_be(self.ar_tip)
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    #[test]
    fn test_empty_lookup(ip in 0u32..0xFFFFFFFFu32) {
        let cache = ArpCache::new();
        prop_assert!(cache.lookup(ip).is_none());
    }

    #[test]
    fn test_update_lookup(
        ip in 1u32..0xFFFFFFFEu32,
        b0 in 0u8..255u8, b1 in 0u8..255u8, b2 in 0u8..255u8,
        b3 in 0u8..255u8, b4 in 0u8..255u8, b5 in 0u8..255u8,
    ) {
        let mut cache = ArpCache::new();
        let mac = [b0, b1, b2, b3, b4, b5];
        cache.update(ip, mac);
        let entry = cache.lookup(ip).unwrap();
        prop_assert_eq!(entry.ip, ip);
        prop_assert_eq!(entry.mac, mac);
        prop_assert!(entry.valid);
    }

    #[test]
    fn test_update_existing(
        ip in 1u32..0xFFFFFFFEu32,
        b0 in 0u8..255u8, b1 in 0u8..255u8, b2 in 0u8..255u8,
        b3 in 0u8..255u8, b4 in 0u8..255u8, b5 in 0u8..255u8,
    ) {
        let mut cache = ArpCache::new();
        let mac1 = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let mac2 = [b0, b1, b2, b3, b4, b5];
        cache.update(ip, mac1);
        prop_assert_eq!(cache.count(), 1);
        cache.update(ip, mac2);
        prop_assert_eq!(cache.count(), 1);
        let entry = cache.lookup(ip).unwrap();
        prop_assert_eq!(entry.mac, mac2);
    }

    #[test]
    fn test_lookup_miss(
        ip1 in 0x01000000u32..0xFE000000u32,
        ip2 in 0x01000000u32..0xFE000000u32,
    ) {
        let mut cache = ArpCache::new();
        let mac = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        cache.update(ip1, mac);
        if ip1 != ip2 {
            prop_assert!(cache.lookup(ip2).is_none());
        }
    }

    #[test]
    fn test_remove(ip in 1u32..0xFFFFFFFEu32) {
        let mut cache = ArpCache::new();
        let mac = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01];
        cache.update(ip, mac);
        prop_assert_eq!(cache.count(), 1);
        cache.remove(ip);
        prop_assert_eq!(cache.count(), 0);
        prop_assert!(cache.lookup(ip).is_none());
    }

    #[test]
    fn test_remove_nonexistent(
        ip1 in 1u32..0xFFFFFFFEu32,
        ip2 in 1u32..0xFFFFFFFEu32,
    ) {
        let mut cache = ArpCache::new();
        let mac = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        cache.update(ip1, mac);
        let count_before = cache.count();
        if ip1 != ip2 {
            cache.remove(ip2);
            prop_assert_eq!(cache.count(), count_before);
            prop_assert!(cache.lookup(ip1).is_some());
        }
    }

    #[test]
    fn test_clear(
        ips in proptest::collection::vec(1u32..0xFFFFFFFEu32, 1..20),
    ) {
        let mut cache = ArpCache::new();
        let mac = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        for ip in &ips { cache.update(*ip, mac); }
        prop_assert!(cache.count() > 0);
        cache.clear();
        prop_assert_eq!(cache.count(), 0);
    }

    #[test]
    fn test_interleaved_update_remove(
        ops in proptest::collection::vec(proptest::bool::ANY, 0..50),
        seed in 0u32..0x10000u32,
    ) {
        let mut cache = ArpCache::new();
        let mut active: Vec<(u32, [u8; ETH_ALEN])> = Vec::new();
        for (i, do_add) in ops.iter().enumerate() {
            let ip = seed.wrapping_add(i as u32 * 3 + 1);
            let mac = [i as u8, (i >> 1) as u8, (i >> 2) as u8, (i >> 3) as u8, (i >> 4) as u8, (i >> 5) as u8];
            if *do_add {
                cache.update(ip, mac);
                if let Some(entry) = active.iter_mut().find(|(e_ip, _)| *e_ip == ip) {
                    entry.1 = mac;
                } else { active.push((ip, mac)); }
            } else if let Some(idx) = active.iter().position(|(e_ip, _)| *e_ip == ip) {
                active.remove(idx);
                cache.remove(ip);
            }
        }
        let expected_count: usize = active.iter()
            .filter(|(ip, mac)| {
                if let Some(entry) = cache.lookup(*ip) { entry.mac == *mac && entry.valid }
                else { false }
            }).count();
        prop_assert_eq!(expected_count, active.len());
    }

    #[test]
    fn test_cache_capacity(_v in 0u8..1u8) {
        let mut cache = ArpCache::new();
        let base_mac = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        for i in 0..ARP_CACHE_SIZE {
            let mut mac = base_mac;
            mac[5] = i as u8;
            cache.update(i as u32, mac);
        }
        prop_assert_eq!(cache.count(), ARP_CACHE_SIZE);

        // Entry 0 is evicted; the new entry overwrites index 0
        let evict_mac = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        cache.update(ARP_CACHE_SIZE as u32, evict_mac);

        // Old entry at ip=0 is gone (overwritten)
        prop_assert!(cache.lookup(0).is_none());
        // New entry is findable
        let entry = cache.lookup(ARP_CACHE_SIZE as u32).unwrap();
        prop_assert_eq!(entry.mac, evict_mac);
        // Count stays the same
        prop_assert_eq!(cache.count(), ARP_CACHE_SIZE);
    }

    #[test]
    fn test_from_bytes_short(len in 0usize..64usize) {
        let data = vec![0u8; len];
        if len < ArpPacket::LEN {
            prop_assert!(ArpPacket::from_bytes(&data).is_none());
        }
    }

    #[test]
    fn test_packet_op_detection(op_raw in 0u16..5u16) {
        let mut pkt: ArpPacket = unsafe { core::mem::zeroed() };
        pkt.hdr.ar_op = op_raw.to_be();
        let bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(&pkt as *const ArpPacket as *const u8, ArpPacket::LEN)
        };
        let parsed = ArpPacket::from_bytes(bytes).unwrap();
        prop_assert_eq!(parsed.is_request(), op_raw == 1);
        prop_assert_eq!(parsed.is_reply(), op_raw == 2);
    }

    #[test]
    fn test_packet_ip_extraction(
        sip_host in 0u32..0xFFFFFFFFu32,
        tip_host in 0u32..0xFFFFFFFFu32,
    ) {
        let mut pkt: ArpPacket = unsafe { core::mem::zeroed() };
        pkt.ar_sip = sip_host.to_be();
        pkt.ar_tip = tip_host.to_be();
        let bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(&pkt as *const ArpPacket as *const u8, ArpPacket::LEN)
        };
        let parsed = ArpPacket::from_bytes(bytes).unwrap();
        prop_assert_eq!(parsed.sender_ip(), sip_host);
        prop_assert_eq!(parsed.target_ip(), tip_host);
    }

    #[test]
    fn test_packet_mac_extraction(
        mac_vals in proptest::collection::vec(0u8..255u8, 12),
    ) {
        let mut pkt: ArpPacket = unsafe { core::mem::zeroed() };
        pkt.ar_sha.copy_from_slice(&mac_vals[0..6]);
        pkt.ar_tha.copy_from_slice(&mac_vals[6..12]);
        let bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(&pkt as *const ArpPacket as *const u8, ArpPacket::LEN)
        };
        let parsed = ArpPacket::from_bytes(bytes).unwrap();
        prop_assert_eq!(parsed.sender_mac(), <[u8; ETH_ALEN]>::try_from(&mac_vals[0..6]).unwrap());
        prop_assert_eq!(parsed.target_mac(), <[u8; ETH_ALEN]>::try_from(&mac_vals[6..12]).unwrap());
    }

    #[test]
    fn test_packet_size(_v in 0u8..1u8) {
        prop_assert_eq!(ArpPacket::LEN, 32);
    }
}
