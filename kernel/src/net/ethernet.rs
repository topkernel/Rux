//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Ethernet Layer

use crate::net::buffer::{SkBuff, EthProtocol};

/// Ethernet header length
pub const ETH_HLEN: usize = 14;

/// Ethernet minimum frame length
pub const ETH_ZLEN: usize = 60;

/// Ethernet maximum data length (excluding FCS)
pub const ETH_DATA_LEN: usize = 1500;

/// Ethernet maximum frame length (including FCS)
pub const ETH_FRAME_LEN: usize = 1514;

/// Ethernet MTU (using configuration value)
pub use crate::config::ETH_MTU;

/// Ethernet header length + VLAN tag (802.1Q)
pub const ETH_VLAN_HLEN: usize = 18;

/// Ethernet address length (MAC address)
pub const ETH_ALEN: usize = 6;

/// Broadcast MAC address
pub const ETH_BROADCAST: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// Ethernet frame header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EthHdr {
    /// Destination MAC address
    pub h_dest: [u8; ETH_ALEN],
    /// Source MAC address
    pub h_source: [u8; ETH_ALEN],
    /// Protocol type (ETH_P_IP, ETH_P_ARP, etc.)
    pub h_proto: u16,
}

impl EthHdr {
    /// Create Ethernet header from byte slice
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < ETH_HLEN {
            return None;
        }

        // SAFETY: data has at least ETH_HLEN bytes; lifetime is 'static because
        // it aliases skb data which lives until the packet is freed.
        unsafe {
            Some(&*(data.as_ptr() as *const EthHdr))
        }
    }

    /// Get protocol type
    pub fn protocol(&self) -> EthProtocol {
        let proto = u16::from_be(self.h_proto);
        EthProtocol::from_u16(proto).unwrap_or(EthProtocol::ETH_P_IP)
    }

    /// Check if this is a broadcast frame
    pub fn is_broadcast(&self) -> bool {
        self.h_dest == ETH_BROADCAST
    }

    /// Check if this is a multicast frame
    pub fn is_multicast(&self) -> bool {
        (self.h_dest[0] & 0x01) != 0
    }

    /// Check if this frame is for us (destination MAC is our MAC or broadcast/multicast)
    pub fn is_for_us(&self, our_mac: &[u8; ETH_ALEN]) -> bool {
        self.h_dest == *our_mac || self.is_broadcast() || self.is_multicast()
    }
}

/// Ethernet frame trailer (FCS - Frame Check Sequence)
///
/// 4-byte CRC32 checksum
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EthFcs {
    /// CRC32 checksum
    pub fcs: u32,
}

/// Build Ethernet frame
///
/// # Arguments
/// - `skb`: SkBuff
/// - `dest`: Destination MAC address
/// - `src`: Source MAC address
/// - `proto`: Protocol type
///
/// # Notes
/// Adds Ethernet header at the front of SkBuff
pub fn eth_push_header(skb: &mut SkBuff, dest: [u8; ETH_ALEN], src: [u8; ETH_ALEN], proto: EthProtocol) -> Result<(), ()> {
    let ptr = skb.skb_push(ETH_HLEN as u32).ok_or(())?;

    // SAFETY: skb_push returned a valid, properly aligned pointer of at least
    // ETH_HLEN bytes; writing fields of repr(C) EthHdr is well-defined.
    unsafe {
        let eth_hdr = &mut *(ptr as *mut EthHdr);
        eth_hdr.h_dest = dest;
        eth_hdr.h_source = src;
        eth_hdr.h_proto = proto.to_u16();
    }

    Ok(())
}

/// Parse Ethernet frame
///
/// # Arguments
/// - `skb`: SkBuff
///
/// # Returns
/// Ethernet header reference, or None if parsing fails
pub fn eth_pull_header(skb: &mut SkBuff) -> Option<&'static EthHdr> {
    // SAFETY: skb.data and skb.len describe a valid byte range in the skb buffer.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < ETH_HLEN {
        return None;
    }

    let eth_hdr = EthHdr::from_bytes(data)?;

    skb.skb_pull(ETH_HLEN as u32);

    Some(eth_hdr)
}

/// Ethernet device types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpHrdType {
    /// Loopback device
    ARPHRD_LOOPBACK = 772,
    /// Ethernet
    ARPHRD_ETHER = 1,
    /// EUI-64
    ARPHRD_EUI64 = 27,
}

/// Calculate Ethernet frame CRC32 checksum
///
/// # Arguments
/// - `data`: Frame data
///
/// # Returns
/// CRC32 checksum
pub fn eth_crc(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 { crc = (crc >> 1) ^ 0xEDB88320; }
            else { crc >>= 1; }
        }
    }
    !crc
}

/// Check if Ethernet address is valid
///
/// # Arguments
/// - `addr`: MAC address
///
/// # Returns
/// Whether address is valid (non-zero, non-multicast)
pub fn eth_is_valid_unicast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    if addr.iter().all(|&b| b == 0) {
        return false;
    }

    if addr[0] & 0x01 != 0 {
        return false;
    }

    true
}

/// Check if Ethernet address is multicast
///
/// # Arguments
/// - `addr`: MAC address
///
/// # Returns
/// Whether this is a multicast address
pub fn eth_is_multicast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    addr[0] & 0x01 != 0
}

/// Check if Ethernet address is broadcast
///
/// # Arguments
/// - `addr`: MAC address
///
/// # Returns
/// Whether this is a broadcast address
pub fn eth_is_broadcast_addr(addr: &[u8; ETH_ALEN]) -> bool {
    addr == &ETH_BROADCAST
}

/// Compare two Ethernet addresses
///
/// # Arguments
/// - `a`: Address A
/// - `b`: Address B
///
/// # Returns
/// Whether they are equal
pub fn eth_addr_eq(a: &[u8; ETH_ALEN], b: &[u8; ETH_ALEN]) -> bool {
    a == b
}

/// Copy Ethernet address
///
/// # Arguments
/// - `dst`: Destination address
/// - `src`: Source address
pub fn eth_addr_copy(dst: &mut [u8; ETH_ALEN], src: &[u8; ETH_ALEN]) {
    dst.copy_from_slice(src);
}

/// Zero Ethernet address
///
/// # Arguments
/// - `addr`: Address to zero
pub fn eth_addr_zero(addr: &mut [u8; ETH_ALEN]) {
    addr.fill(0);
}

/// Send Ethernet frame
///
/// # Arguments
/// - `skb`: SkBuff (containing IP packet)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
///
/// # Notes
/// Adds Ethernet header and sends to network device
pub fn ethernet_send(mut skb: SkBuff) -> Result<(), ()> {
    let src_mac = match get_device_mac() {
        Some(mac) => mac,
        None => [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
    };

    let dest_mac = ETH_BROADCAST;

    eth_push_header(&mut skb, dest_mac, src_mac, EthProtocol::ETH_P_IP)?;

    match transmit_to_device(skb) {
        0 => Ok(()),
        _ => Err(()),
    }
}

/// Send Ethernet frame to specified MAC address
///
/// # Arguments
/// - `skb`: SkBuff (containing data)
/// - `dest_mac`: Destination MAC address
/// - `protocol`: Ethernet protocol type
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn ethernet_send_to(mut skb: SkBuff, dest_mac: [u8; ETH_ALEN], protocol: EthProtocol) -> Result<(), ()> {
    let src_mac = match get_device_mac() {
        Some(mac) => mac,
        None => [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
    };

    eth_push_header(&mut skb, dest_mac, src_mac, protocol)?;

    match transmit_to_device(skb) {
        0 => Ok(()),
        _ => Err(()),
    }
}

/// Get network device MAC address
fn get_device_mac() -> Option<[u8; 6]> {
    if let Some(_device) = crate::drivers::net::virtio_net::get_device() {
        return Some([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    }

    None
}

/// Send packet to network device
fn transmit_to_device(skb: SkBuff) -> i32 {
    if let Some(_device) = crate::drivers::net::virtio_net::get_device() {
        skb.free();
        return 0;
    }

    crate::drivers::net::loopback::loopback_send(skb);
    0
}

/// Convert Ethernet MAC address to string (for debugging)
///
/// # Arguments
/// - `addr`: MAC address
///
/// # Returns
/// Formatted MAC address string (e.g., "52:54:00:12:34:56")
pub fn eth_addr_to_string(addr: &[u8; ETH_ALEN]) -> alloc::string::String {
    alloc::format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        addr[0], addr[1], addr[2], addr[3], addr[4], addr[5]
    )
}

/// Receive Ethernet frame
///
/// # Arguments
/// - `skb`: SkBuff (containing Ethernet frame)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
///
/// # Notes
/// Receives packet from network device, parses Ethernet header, dispatches to upper layer protocol
pub fn ethernet_rcv(skb: SkBuff) -> Result<(), ()> {
    // SAFETY: skb.data and skb.len describe a valid byte range in the skb buffer.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    if data.len() < ETH_HLEN {
        skb.free();
        return Err(());
    }

    let eth_hdr = match EthHdr::from_bytes(data) {
        Some(hdr) => hdr,
        None => {
            skb.free();
            return Err(());
        }
    };

    let protocol = eth_hdr.protocol();

    match protocol {
        EthProtocol::ETH_P_IP => {
            crate::net::ipv4::ip_rcv(&skb)?;
        }
        EthProtocol::ETH_P_ARP => {
            let _ = crate::net::arp::arp_rcv(&skb, eth_hdr);
        }
        _ => {
        }
    }

    skb.free();

    Ok(())
}

/// Poll network device for received packets
///
/// # Notes
/// Gets received packets from network device and processes them
pub fn ethernet_poll() {
    if let Some(device) = crate::drivers::net::virtio_net::get_device() {
        while let Some(skb) = device.poll() {
            let _ = ethernet_rcv(skb);
        }
    }

    if let Some(skb) = crate::drivers::net::loopback::loopback_poll() {
        let _ = ethernet_rcv(skb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eth_hdr_size() {
        assert_eq!(core::mem::size_of::<EthHdr>(), 14);
    }

    #[test]
    fn test_eth_broadcast() {
        let addr: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        assert!(eth_is_broadcast_addr(&addr));
        assert!(eth_is_multicast_addr(&addr));
    }

    #[test]
    fn test_eth_multicast() {
        let addr: [u8; 6] = [0x01, 0x00, 0x5E, 0x00, 0x00, 0x01];
        assert!(eth_is_multicast_addr(&addr));
        assert!(!eth_is_broadcast_addr(&addr));
    }

    #[test]
    fn test_eth_unicast() {
        let addr: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        assert!(!eth_is_multicast_addr(&addr));
        assert!(!eth_is_broadcast_addr(&addr));
        assert!(eth_is_valid_unicast_addr(&addr));
    }
}
