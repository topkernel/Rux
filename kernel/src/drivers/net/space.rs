//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network device base class

use crate::net::buffer::SkBuff;
use crate::sync::spinlock::Spinlock;

/// Maximum device name length
pub const IFNAMSIZ: usize = 16;

/// Maximum hardware address length
pub const MAX_ADDR_LEN: usize = 32;

/// ARP hardware types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpHrdType {
    /// Loopback device
    ARPHRD_LOOPBACK = 772,
    /// Ethernet
    ARPHRD_ETHER = 1,
    /// None
    ARPHRD_VOID = 0xFFFF,
}

/// Network device operation interface
#[repr(C)]
pub struct NetDeviceOps {
    /// Transmit packet
    ///
    /// # Parameters
    /// - `skb`: Packet to transmit
    ///
    /// # Returns
    /// 0 on success, negative error code on failure
    pub xmit: fn(skb: SkBuff) -> i32,

    /// Device initialization (optional)
    pub init: Option<fn() -> i32>,

    /// Device teardown (optional)
    pub uninit: Option<fn() -> i32>,

    /// Get statistics (optional)
    pub get_stats: Option<fn() -> DeviceStats>,
}

/// Network device statistics
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct DeviceStats {
    /// Received packets
    pub rx_packets: u64,
    /// Transmitted packets
    pub tx_packets: u64,
    /// Received bytes
    pub rx_bytes: u64,
    /// Transmitted bytes
    pub tx_bytes: u64,
    /// Receive errors
    pub rx_errors: u64,
    /// Transmit errors
    pub tx_errors: u64,
    /// Receive dropped
    pub rx_dropped: u64,
    /// Transmit dropped
    pub tx_dropped: u64,
    /// Multicast received packets
    pub multicast: u64,
}

/// Network device
///
/// # Notes
/// - All network devices must implement this structure
/// - Uses function pointers for polymorphic calls
#[repr(C)]
pub struct NetDevice {
    /// Device name (e.g., "lo", "eth0")
    pub name: [u8; IFNAMSIZ],
    /// Device index
    pub ifindex: u32,
    /// MTU (maximum transmission unit)
    pub mtu: u32,
    /// Hardware type (ARPHRD_ETHER, ARPHRD_LOOPBACK, etc.)
    pub type_: ArpHrdType,
    /// Hardware address (MAC address)
    pub addr: [u8; MAX_ADDR_LEN],
    /// Hardware address length
    pub addr_len: u8,
    /// Device operation interface
    pub netdev_ops: &'static NetDeviceOps,
    /// Private data
    pub priv_: *mut u8,
    /// Statistics
    pub stats: DeviceStats,
    /// Device status
    pub flags: u32,
    /// Receive queue length
    pub rx_queue_len: u32,
}

unsafe impl Send for NetDevice {}

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
        unsafe {
            let len = self.name.iter().position(|&c| c == 0).unwrap_or(IFNAMSIZ);
            core::str::from_utf8_unchecked(&self.name[..len])
        }
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
        crate::drivers::net::get_loopback_device()
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
        crate::drivers::net::get_loopback_device()
    } else {
        None
    }
}

/// Get total network device count
pub fn get_netdevice_count() -> usize {
    *DEV_COUNT.lock()
}
