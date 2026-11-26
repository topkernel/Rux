//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network device base class

use crate::net::buffer::SkBuff;
use crate::sync::types::Spinlock;

// IFNAMSIZ — the definition moved to drivers/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use crate::drivers::types::{IFNAMSIZ};

// MAX_ADDR_LEN — the definition moved to drivers/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use crate::drivers::types::{MAX_ADDR_LEN};

// ArpHrdType — the definition moved to drivers/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use crate::drivers::types::{ArpHrdType};

// NetDeviceOps — the definition moved to drivers/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use crate::drivers::types::{NetDeviceOps};

// DeviceStats — the definition moved to drivers/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use crate::drivers::types::{DeviceStats};

// NetDevice — the definition moved to drivers/types.rs; every impl and
// function that uses it stays here. Re-exported so the paths are unchanged.
pub use crate::drivers::types::{NetDevice};

unsafe impl Send for NetDevice {}
// SAFETY (round-5): NetDevice's cross-thread state (stats, flags) is
// mutated under the NetRx softirq / syscall discipline each op
// documents; the priv_ raw slot is driver-owned and set before
// registration. Sync was implicit in the old `static mut` usage.
unsafe impl Sync for NetDevice {}

/// Device status flags
pub mod dev_flags {
    /// Interface is up
    pub const IFF_UP: u32 = 0x1;
    /// Interface is broadcast
    pub const IFF_BROADCAST: u32 = 0x2;
    /// Interface is loopback
    pub const IFF_LOOPBACK: u32 = 0x8;
    /// Interface is running
    pub const IFF_RUNNING: u32 = 0x40;
    /// Interface has multicast enabled
    pub const IFF_MULTICAST: u32 = 0x1000;
}

/// Network device registry
///
/// Simplified implementation: uses counter to track device count (protected by Mutex)
static DEV_COUNT: Spinlock<usize> = Spinlock::new(0);

impl NetDevice {
    /// Set hardware address
    ///
    /// # Parameters
    /// - `addr`: Hardware address
    /// - `len`: Address length
    pub fn set_address(&mut self, addr: &[u8], len: u8) {
        self.addr_len = len;
        self.addr[..len as usize].copy_from_slice(&addr[..len as usize]);
    }

    /// Get device name
    pub fn get_name(&self) -> &str {
        // Checked UTF-8: interface names are userspace-writable (ioctl
        // SIOCSIFNAME) arbitrary bytes — the KOT-KILL comm panic class
        // (lb266) applies to every unchecked name print.
        let len = self.name.iter().position(|&c| c == 0).unwrap_or(IFNAMSIZ);
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }

    /// Transmit packet
    ///
    /// # Parameters
    /// - `skb`: Packet to transmit
    ///
    /// # Returns
    /// 0 on success, negative error code on failure
    pub fn xmit(&mut self, skb: SkBuff) -> i32 {
        (self.netdev_ops.xmit)(skb)
    }

    /// Get statistics
    pub fn get_stats(&self) -> DeviceStats {
        if let Some(get_stats_fn) = self.netdev_ops.get_stats {
            get_stats_fn()
        } else {
            self.stats
        }
    }

    /// Bring up device
    pub fn up(&mut self) {
        self.flags |= dev_flags::IFF_UP | dev_flags::IFF_RUNNING;
    }

    /// Shut down device
    pub fn down(&mut self) {
        self.flags &= !(dev_flags::IFF_UP | dev_flags::IFF_RUNNING);
    }

    /// Check if device is up
    pub fn is_up(&self) -> bool {
        (self.flags & dev_flags::IFF_UP) != 0
    }

    /// Check if device is running
    pub fn is_running(&self) -> bool {
        (self.flags & dev_flags::IFF_RUNNING) != 0
    }
}

/// Register network device
///
/// # Parameters
/// - `device`: Device to register
///
/// # Returns
/// Assigned device index on success, negative error code on failure
///
/// # Notes
/// - Adds device to global device list
/// - Assigns device index
pub fn register_netdevice(device: &'static mut NetDevice) -> i32 {
    let mut count = DEV_COUNT.lock();
    // Assign device index
    device.ifindex = *count as u32;

    // Increment count
    *count += 1;

    device.ifindex as i32
}

/// Unregister network device
///
/// # Parameters
/// - `device`: Device to unregister
pub fn unregister_netdevice(device: &mut NetDevice) {
    // Remove from global list
    // Simplified implementation: just mark device
    device.flags &= !dev_flags::IFF_UP;
}

/// Find network device by index
///
/// # Parameters
/// - `ifindex`: Device index
///
/// # Returns
/// Found device, or None if not found
pub fn get_netdevice_by_index(ifindex: u32) -> Option<&'static mut NetDevice> {
    // Simplified implementation: currently only supports finding loopback device
    // Full implementation needs to maintain device list
    if ifindex == 0 {
        crate::api::drivers::get_loopback_device()
    } else {
        None
    }
}

/// Find network device by name
///
/// # Parameters
/// - `name`: Device name
///
/// # Returns
/// Found device, or None if not found
pub fn get_netdevice_by_name(name: &str) -> Option<&'static mut NetDevice> {
    // Simplified implementation: currently only supports finding loopback device
    // Full implementation needs to maintain device list
    if name == "lo" {
        crate::api::drivers::get_loopback_device()
    } else {
        None
    }
}

/// Get total network device count
pub fn get_netdevice_count() -> usize {
    *DEV_COUNT.lock()
}
