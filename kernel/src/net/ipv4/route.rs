//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! IPv4 Routing Table

use crate::net::buffer::SkBuff;
use crate::config::ROUTE_TABLE_SIZE;

/// Routing table entry
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RouteEntry {
    /// Destination network address
    pub dst: u32,
    /// Network mask
    pub mask: u32,
    /// Gateway address
    pub gateway: u32,
    /// Output device index
    pub oif: u32,
    /// MTU
    pub mtu: u32,
    /// Flags
    pub flags: RouteFlags,
}

/// Routing flags
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteFlags(pub u32);

impl RouteFlags {
    /// Route is up
    pub const RTF_UP: u32 = 0x0001;
    /// Gateway route
    pub const RTF_GATEWAY: u32 = 0x0002;
    /// Host route
    pub const RTF_HOST: u32 = 0x0004;
    /// Reinstate route after reboot
    pub const RTF_REINSTATE: u32 = 0x0008;
    /// Dynamically installed route
    pub const RTF_DYNAMIC: u32 = 0x0010;
    /// Modified route
    pub const RTF_MODIFIED: u32 = 0x0020;
    /// Malicious redirect
    pub const RTF_MALICED: u32 = 0x0040;
    /// Forwarding
    pub const RTF_FWD: u32 = 0x0080;
    /// Local address
    pub const RTF_LOCAL: u32 = 0x0100;
    /// Broadcast route
    pub const RTF_BROADCAST: u32 = 0x0200;
    /// Network address
    pub const RTF_NETWORK: u32 = 0x0400;
}

impl RouteEntry {
    /// Create a new routing entry
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

    /// Check if this is a gateway route
    pub fn is_gateway(&self) -> bool {
        (self.flags.0 & RouteFlags::RTF_GATEWAY) != 0
    }

    /// Check if this is a host route
    pub fn is_host(&self) -> bool {
        (self.flags.0 & RouteFlags::RTF_HOST) != 0
    }

    /// Check if this is a network route
    pub fn is_network(&self) -> bool {
        (self.flags.0 & RouteFlags::RTF_NETWORK) != 0
    }

    /// Check if address matches this route
    pub fn matches(&self, addr: u32) -> bool {
        (addr & self.mask) == (self.dst & self.mask)
    }
}

/// Routing table
struct RouteTable {
    entries: [Option<RouteEntry>; ROUTE_TABLE_SIZE],
    count: usize,
}

impl RouteTable {
    const fn new() -> Self {
        const NONE: Option<RouteEntry> = None;
        Self {
            entries: [NONE; ROUTE_TABLE_SIZE],
            count: 0,
        }
    }

    /// Look up route
    fn lookup(&self, dst: u32) -> Option<RouteEntry> {
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

    /// Add route
    fn add(&mut self, route: RouteEntry) -> Result<(), ()> {
        if self.count >= ROUTE_TABLE_SIZE {
            return Err(());
        }

        self.entries[self.count] = Some(route);
        self.count += 1;
        Ok(())
    }

    /// Remove route
    fn remove(&mut self, dst: u32, mask: u32) -> bool {
        for i in 0..self.count {
            if let Some(route) = self.entries[i] {
                if route.dst == dst && route.mask == mask {
                    for j in i..self.count - 1 {
                        self.entries[j] = self.entries[j + 1];
                    }
                    self.entries[self.count - 1] = None;
                    self.count -= 1;
                    return true;
                }
            }
        }
        false
    }

    /// Clear routing table
    fn clear(&mut self) {
        self.count = 0;
        for entry in self.entries.iter_mut() {
            *entry = None;
        }
    }
}

/// Global routing table
static mut ROUTE_TABLE: RouteTable = RouteTable::new();

/// Look up route
///
/// # Arguments
/// - `dst`: Destination IP address (host byte order)
///
/// # Returns
/// Route entry if found, None otherwise
pub fn route_lookup(dst: u32) -> Option<RouteEntry> {
    unsafe { ROUTE_TABLE.lookup(dst) }
}

/// Add route
///
/// # Arguments
/// - `dst`: Destination network address (host byte order)
/// - `mask`: Network mask (host byte order)
/// - `gateway`: Gateway address (host byte order)
/// - `oif`: Output device index
/// - `mtu`: MTU
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn route_add(dst: u32, mask: u32, gateway: u32, oif: u32, mtu: u32) -> Result<(), ()> {
    let route = RouteEntry::new(dst, mask, gateway, oif, mtu);
    unsafe { ROUTE_TABLE.add(route) }
}

/// Remove route
///
/// # Arguments
/// - `dst`: Destination network address (host byte order)
/// - `mask`: Network mask (host byte order)
///
/// # Returns
/// Whether removal was successful
pub fn route_remove(dst: u32, mask: u32) -> bool {
    unsafe { ROUTE_TABLE.remove(dst, mask) }
}

/// Clear routing table
pub fn route_clear() {
    unsafe { ROUTE_TABLE.clear() }
}

/// Initialize default routes
///
/// Adds local loopback route and directly connected route
pub fn route_init() {
    let _ = route_add(
        0x7F000000,
        0xFF000000,
        0,
        0,
        16436,
    );

    let _ = route_add(
        0xC0A80100,
        0xFFFFFF00,
        0,
        1,
        1500,
    );
}

/// Send packet based on route
///
/// # Arguments
/// - `skb`: SkBuff
/// - `dst`: Destination IP address
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn route_output(skb: SkBuff, dst: u32) -> Result<(), ()> {
    let _route = route_lookup(dst).ok_or(())?;

    skb.free();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_entry_match() {
        let route = RouteEntry::new(
            0xC0A80100,
            0xFFFFFF00,
            0,
            1,
            1500,
        );

        assert!(route.matches(0xC0A80101));
        assert!(route.matches(0xC0A801FF));
        assert!(!route.matches(0xC0A80201));
    }

    #[test]
    fn test_route_lookup() {
        unsafe {
            ROUTE_TABLE.clear();
        }

        let _ = route_add(
            0xC0A80100,
            0xFFFFFF00,
            0,
            1,
            1500,
        );

        let route = route_lookup(0xC0A80101);
        assert!(route.is_some());
    }
}
