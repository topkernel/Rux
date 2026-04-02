//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Loopback network device

use crate::drivers::net::space::{NetDevice, NetDeviceOps, DeviceStats, ArpHrdType, dev_flags};
use crate::net::buffer::SkBuff;
use crate::sync::spinlock::Spinlock;

/// Loopback device statistics (protected by Mutex)
static LO_STATS: Spinlock<DeviceStats> = Spinlock::new(DeviceStats {
    rx_packets: 0,
    tx_packets: 0,
    rx_bytes: 0,
    tx_bytes: 0,
    rx_errors: 0,
    tx_errors: 0,
    rx_dropped: 0,
    tx_dropped: 0,
    multicast: 0,
});

/// Loopback device lock
static LO_DEVICE_LOCK: Spinlock<()> = Spinlock::new(());

/// Loopback device (protected by lock)
static mut LO_DEVICE: Option<NetDevice> = None;

/// Loopback device transmit function
///
/// # Parameters
/// - `skb`: Packet to transmit
///
/// # Returns
/// Always returns 0 (success)
///
/// # Notes
/// The loopback device is special because:
/// - Transmitted packets are immediately received
/// - No hardware is involved
fn loopback_xmit(skb: SkBuff) -> i32 {
    // Update statistics (need lock protection)
    {
        let mut stats = LO_STATS.lock();
        stats.tx_packets += 1;
        stats.tx_bytes += skb.len as u64;
        stats.rx_packets += 1;
        stats.rx_bytes += skb.len as u64;
    }

    // TODO: Pass packet to network protocol stack
    // Current simplified implementation: directly free packet
    // Full implementation should call netif_rx(skb)

    // Free packet
    skb.free();

    0
}

/// Loopback device statistics function
fn loopback_get_stats() -> DeviceStats {
    let stats = LO_STATS.lock();
    DeviceStats {
        rx_packets: stats.rx_packets,
        tx_packets: stats.tx_packets,
        rx_bytes: stats.rx_bytes,
        tx_bytes: stats.tx_bytes,
        rx_errors: stats.rx_errors,
        tx_errors: stats.tx_errors,
        rx_dropped: stats.rx_dropped,
        tx_dropped: stats.tx_dropped,
        multicast: stats.multicast,
    }
}

/// Loopback device operation interface
static LOOPBACK_OPS: NetDeviceOps = NetDeviceOps {
    xmit: loopback_xmit,
    init: None,
    uninit: None,
    get_stats: Some(loopback_get_stats),
};

/// Initialize loopback device
///
/// # Returns
/// Device pointer on success, None on failure
pub fn loopback_init() -> Option<&'static mut NetDevice> {
    let _lock = LO_DEVICE_LOCK.lock();
    unsafe {
        // Check if already initialized
        if LO_DEVICE.is_some() {
            return LO_DEVICE.as_mut();
        }

        // Create loopback device
        let mut device = NetDevice {
            name: [0u8; 16],
            ifindex: 0,
            mtu: 65536,  // Loopback device has larger MTU
            type_: ArpHrdType::ARPHRD_LOOPBACK,
            addr: [0u8; 32],
            addr_len: 0,
            netdev_ops: &LOOPBACK_OPS,
            priv_: core::ptr::null_mut(),
            stats: DeviceStats::default(),
            flags: dev_flags::IFF_UP | dev_flags::IFF_RUNNING | dev_flags::IFF_LOOPBACK,
            rx_queue_len: 0,
        };

        // Set device name
        let name = b"lo\0";
        device.name[..name.len()].copy_from_slice(name);

        // Set address (loopback device has no MAC address)
        device.addr_len = 0;

        // Store device
        LO_DEVICE = Some(device);

        LO_DEVICE.as_mut()
    }
}

/// Get loopback device
///
/// # Returns
/// Loopback device pointer, or None if not initialized
pub fn get_loopback_device() -> Option<&'static mut NetDevice> {
    unsafe { LO_DEVICE.as_mut() }
}

/// Reset loopback device statistics
///
/// # Notes
/// Used in test environments to reset statistics before tests
pub fn loopback_reset_stats() {
    let mut stats = LO_STATS.lock();
    *stats = DeviceStats::default();
}

/// Send packet to loopback device
///
/// # Parameters
/// - `skb`: Packet to send
///
/// # Returns
/// 0 on success, negative error code on failure
pub fn loopback_send(skb: SkBuff) -> i32 {
    loopback_xmit(skb)
}

/// Poll loopback device for received packets
///
/// # Returns
/// Some(skb) if packet available, otherwise None
///
/// # Notes
/// Loopback device has no real receive queue
/// This function currently returns None because loopback transmit handles packets directly
pub fn loopback_poll() -> Option<SkBuff> {
    // Loopback send and receive are synchronous
    // Transmitted packets are handled in loopback_xmit
    // So no packets need to be returned here
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loopback_init() {
        let device = loopback_init();
        assert!(device.is_some());

        let device = device.unwrap();
        assert_eq!(device.get_name(), "lo");
        assert_eq!(device.mtu, 65536);
        assert!(device.is_up());
        assert!(device.is_running());
    }

    #[test]
    fn test_loopback_xmit() {
        // Initialize loopback device
        loopback_init();

        // Create test packet
        let skb = SkBuff::alloc(100).unwrap();

        // Send packet
        let result = loopback_send(skb);
        assert_eq!(result, 0);

        // Check statistics
        let stats = unsafe { LO_STATS };
        assert_eq!(stats.tx_packets, 1);
        assert_eq!(stats.rx_packets, 1);
    }
}
