//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv4 routing table longest-prefix match invariant tests.
//!
//! Types copied from: kernel/src/net/ipv4/route.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/net/ipv4/route.rs
// ============================================================================

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RouteEntry {
    pub dst: u32,
    pub mask: u32,
    pub gateway: u32,
    pub oif: u32,
    pub mtu: u32,
    pub flags: RouteFlags,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteFlags(pub u32);

impl RouteFlags {
    pub const RTF_UP: u32 = 0x0001;
    pub const RTF_GATEWAY: u32 = 0x0002;
    pub const RTF_HOST: u32 = 0x0004;
    pub const RTF_REINSTATE: u32 = 0x0008;
    pub const RTF_DYNAMIC: u32 = 0x0010;
    pub const RTF_MODIFIED: u32 = 0x0020;
    pub const RTF_MALICED: u32 = 0x0040;
    pub const RTF_FWD: u32 = 0x0080;
    pub const RTF_LOCAL: u32 = 0x0100;
    pub const RTF_BROADCAST: u32 = 0x0200;
    pub const RTF_NETWORK: u32 = 0x0400;
}

impl RouteEntry {
    pub fn new(dst: u32, mask: u32, gateway: u32, oif: u32, mtu: u32) -> Self {
        Self {
            dst,
            mask,
            gateway,
            oif,
            mtu,
            flags: RouteFlags(0),
        }
    }

    pub fn is_gateway(&self) -> bool {
        (self.flags.0 & RouteFlags::RTF_GATEWAY) != 0
    }

    pub fn is_host(&self) -> bool {
        (self.flags.0 & RouteFlags::RTF_HOST) != 0
    }

    pub fn is_network(&self) -> bool {
        (self.flags.0 & RouteFlags::RTF_NETWORK) != 0
    }

    pub fn matches(&self, addr: u32) -> bool {
        (addr & self.mask) == (self.dst & self.mask)
    }
}

/// Verify-local RouteTable using Vec instead of fixed-size array.
pub struct RouteTable {
    entries: Vec<Option<RouteEntry>>,
}

impl RouteTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::with_capacity(64),
        }
    }

    pub fn lookup(&self, dst: u32) -> Option<RouteEntry> {
        let mut best_match: Option<RouteEntry> = None;
        let mut best_mask = 0u32;

        for entry in self.entries.iter() {
            if let Some(route) = entry {
                if route.matches(dst) && route.mask >= best_mask {
                    best_match = Some(*route);
                    best_mask = route.mask;
                }
            }
        }

        best_match
    }

    pub fn add(&mut self, route: RouteEntry) -> Result<(), ()> {
        if self.entries.len() >= 64 {
            return Err(());
        }
        self.entries.push(Some(route));
        Ok(())
    }

    pub fn remove(&mut self, dst: u32, mask: u32) -> bool {
        for i in 0..self.entries.len() {
            if let Some(route) = self.entries[i] {
                if route.dst == dst && route.mask == mask {
                    for j in i..self.entries.len() - 1 {
                        self.entries[j] = self.entries[j + 1];
                    }
                    self.entries.pop();
                    return true;
                }
            }
        }
        false
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }
}

#[allow(dead_code)]
/// Generate a valid CIDR mask (contiguous 1-bits from MSB).
fn make_cidr_mask(prefix_len: u8) -> u32 {
    if prefix_len == 0 {
        return 0;
    }
    if prefix_len >= 32 {
        return 0xFFFFFFFF;
    }
    !((1u32 << (32 - prefix_len)) - 1)
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-ROUTE-1: matches correctly applies mask to both addr and dst
    #[test]
    fn test_matches_masking(
        dst in 0u32..0xFFFFFFFEu32,
        prefix_len in 0u8..33u8,
        host_bits in 0u32..0x10000u32,
    ) {
        let mask = make_cidr_mask(prefix_len);
        let route = RouteEntry::new(dst & mask, mask, 0, 1, 1500);

        // Same subnet must match
        let addr_in_subnet = (dst & mask) | (host_bits & !mask);
        prop_assert!(route.matches(addr_in_subnet));

        // Different subnet must not match (if mask != 0)
        if mask != 0 {
            let flipped = (dst & mask) ^ (mask & 0x80000000);
            let addr_other = flipped | (host_bits & !mask);
            prop_assert!(!route.matches(addr_other));
        }
    }

    /// INV-ROUTE-2: matches with /32 matches only exact dst
    #[test]
    fn test_matches_host_route(host in 1u32..0xFFFFFFFEu32) {
        let route = RouteEntry::new(host, 0xFFFFFFFF, 0, 1, 1500);
        prop_assert!(route.matches(host));
        prop_assert!(!route.matches(host ^ 1));
        prop_assert!(!route.matches(host.wrapping_add(1)));
        prop_assert!(!route.matches(host.wrapping_sub(1)));
    }

    /// INV-ROUTE-3: matches with /0 matches everything
    #[test]
    fn test_matches_default_route(addr in 0u32..0xFFFFFFFFu32) {
        let route = RouteEntry::new(0, 0, 0x0100007F, 1, 1500);
        prop_assert!(route.matches(addr));
    }

    /// INV-ROUTE-4: longest-prefix match selects most specific route
    #[test]
    fn test_lookup_longest_prefix(
        base in 0u32..0xF0000000u32,
        prefix_short in 8u8..16u8,
        prefix_long in 17u8..24u8,
    ) {
        let mask_short = make_cidr_mask(prefix_short);
        let mask_long = make_cidr_mask(prefix_long);
        // Ensure base is aligned to long prefix
        let network = base & mask_long;
        let addr = network | (mask_long + 1).min(0xFF); // host part

        let mut table = RouteTable::new();

        // More specific route
        let specific = RouteEntry::new(network, mask_long, 0, 2, 9000);
        let _ = table.add(specific);

        // Less specific route
        let general = RouteEntry::new(network & mask_short, mask_short, 0x01000001, 1, 1500);
        let _ = table.add(general);

        let found = table.lookup(addr).unwrap();
        prop_assert_eq!(found.mask, mask_long, "should pick longer prefix");
        prop_assert_eq!(found.oif, 2);
    }

    /// INV-ROUTE-5: add + lookup preserves count and correctness
    #[test]
    fn test_add_lookup_count(
        routes in proptest::collection::vec(
            proptest::bool::ANY.prop_flat_map(|is_host| {
                let prefix = if is_host { 32u8 } else { 24u8 };
                (0u32..0xF0000000u32, proptest::strategy::Just(prefix)).prop_map(|(net, p)| {
                    (net & make_cidr_mask(p), make_cidr_mask(p))
                })
            }),
            0..30
        ),
        probe in 0u32..0xFFFFFFFFu32,
    ) {
        let mut table = RouteTable::new();
        for (dst, mask) in &routes {
            let _ = table.add(RouteEntry::new(*dst, *mask, 0, 1, 1500));
        }

        let found = table.lookup(probe);
        if let Some(entry) = found {
            // Verify the returned entry actually matches
            prop_assert!(entry.matches(probe));
            // Verify it's the longest prefix among all matching routes
            let mut best_mask = 0u32;
            for (dst, mask) in &routes {
                let probe_masked = probe & mask;
                let dst_masked = dst & mask;
                if probe_masked == dst_masked && *mask >= best_mask {
                    best_mask = *mask;
                }
            }
            prop_assert_eq!(entry.mask, best_mask);
        }
    }

    /// INV-ROUTE-6: remove removes correct route
    #[test]
    fn test_remove_correct_route(
        routes in proptest::collection::vec(
            (0u32..0xF0000000u32, 8u8..25u8).prop_map(|(net, p)| {
                let mask = make_cidr_mask(p);
                (net & mask, mask)
            }),
            1..20
        ),
        remove_idx in 0usize..20usize,
    ) {
        let mut table = RouteTable::new();
        for &(dst, mask) in &routes {
            let _ = table.add(RouteEntry::new(dst, mask, 0, 1, 1500));
        }

        let remove_idx = remove_idx % routes.len();
        let (rm_dst, rm_mask) = routes[remove_idx];

        let count_before = table.count();
        prop_assert!(table.remove(rm_dst, rm_mask));
        prop_assert_eq!(table.count(), count_before - 1);

        // If no duplicate routes existed, lookup should not find exact match.
        // With duplicates, lookup may still find a matching route (correct behavior).
        let exact_count = routes.iter().filter(|&&(d, m)| d == rm_dst && m == rm_mask).count();
        if exact_count == 1 {
            // Was the only copy — lookup should not return the same (dst, mask)
            let found = table.lookup(rm_dst);
            if let Some(entry) = found {
                prop_assert!(entry.mask != rm_mask || entry.dst != rm_dst);
            }
        }
    }

    /// INV-ROUTE-7: remove non-existent returns false
    #[test]
    fn test_remove_nonexistent(
        routes in proptest::collection::vec(
            (0u32..0xF0000000u32, 8u8..25u8).prop_map(|(net, p)| {
                let mask = make_cidr_mask(p);
                (net & mask, mask)
            }),
            0..10
        ),
    ) {
        let mut table = RouteTable::new();
        for &(dst, mask) in &routes {
            let _ = table.add(RouteEntry::new(dst, mask, 0, 1, 1500));
        }

        prop_assert!(!table.remove(0xDEADBEEF, 0xFFFFFFFF));
    }

    /// INV-ROUTE-8: flag checks work correctly
    #[test]
    fn test_flags(
        flags_val in 0u32..0x1000u32,
    ) {
        let mut entry = RouteEntry::new(0, 0, 0, 0, 0);
        entry.flags = RouteFlags(flags_val);

        prop_assert_eq!(entry.is_gateway(), (flags_val & RouteFlags::RTF_GATEWAY) != 0);
        prop_assert_eq!(entry.is_host(), (flags_val & RouteFlags::RTF_HOST) != 0);
        prop_assert_eq!(entry.is_network(), (flags_val & RouteFlags::RTF_NETWORK) != 0);
    }

    /// INV-ROUTE-9: empty table lookup returns None
    #[test]
    fn test_empty_lookup(addr in 0u32..0xFFFFFFFFu32) {
        let table = RouteTable::new();
        prop_assert!(table.lookup(addr).is_none());
    }

    /// INV-ROUTE-10: after removing all routes, table is empty
    #[test]
    fn test_remove_all(
        routes in proptest::collection::vec(
            (0u32..0xF0000000u32, 8u8..25u8).prop_map(|(net, p)| {
                let mask = make_cidr_mask(p);
                (net & mask, mask)
            }),
            1..20
        ),
    ) {
        let mut table = RouteTable::new();
        for &(dst, mask) in &routes {
            let _ = table.add(RouteEntry::new(dst, mask, 0, 1, 1500));
        }
        prop_assert!(table.count() > 0);

        for &(dst, mask) in &routes {
            table.remove(dst, mask);
        }
        prop_assert_eq!(table.count(), 0);
    }

    /// INV-ROUTE-11: interleaved add/remove maintains correctness
    #[test]
    fn test_interleaved_add_remove(
        ops in proptest::collection::vec(
            proptest::bool::ANY,
            0..50
        ),
        seed in 0u32..0x10000u32,
    ) {
        let mut table = RouteTable::new();
        let mut added: Vec<(u32, u32)> = Vec::new();

        for (i, do_add) in ops.iter().enumerate() {
            if *do_add {
                let prefix = ((seed + i as u32) % 25) as u8;
                let mask = make_cidr_mask(prefix);
                let net = ((seed.wrapping_mul(i as u32 + 1)) & 0xF0000000) & mask;
                let _ = table.add(RouteEntry::new(net, mask, 0, 1, 1500));
                added.push((net, mask));
            } else if let Some((dst, mask)) = added.pop() {
                table.remove(dst, mask);
            }
        }

        // Verify every remaining entry is still in the table
        prop_assert_eq!(table.count(), added.len());
        for &(dst, _mask) in &added {
            let found = table.lookup(dst);
            prop_assert!(found.is_some(), "entry {} not found after interleaved ops", dst);
        }
    }
}
