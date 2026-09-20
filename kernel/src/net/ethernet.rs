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

    /// Get protocol type. Unknown ethertypes map to None at the dispatch
    /// site — the old `unwrap_or(ETH_P_IP)` mis-parsed VLAN/IPv6 frames as
    /// IPv4.
    pub fn protocol(&self) -> EthProtocol {
        let proto = u16::from_be(self.h_proto);
        EthProtocol::from_u16(proto).unwrap_or(EthProtocol::ETH_P_8021Q)
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
        // Wire format is big-endian; the RX side reads with from_be, so the
        // TX side must convert (review NET-H1 — every outbound frame was
        // dropped by the peer with the raw little-endian value).
        eth_hdr.h_proto = proto.to_u16().to_be();
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
/// Adds Ethernet header and sends to network device. On an ARP miss the
/// packet is parked on the bounded per-IP pending queue (R35) and the
/// ARP request goes out immediately; the packet is flushed unicast when
/// the reply lands.
pub fn ethernet_send(mut skb: SkBuff) -> Result<(), ()> {
    let src_mac = match get_device_mac() {
        Some(mac) => mac,
        None => [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
    };

    // Parse the IPv4 destination once: the loopback short-circuit and the
    // ARP resolution below both need it.
    // Loopback short-circuit: 127.0.0.0/8 must not go through ARP (there
    // is no MAC to resolve; broadcasting loopback traffic worked only by
    // accident of ip_rcv not checking the destination address).
    let dest_ip = if (skb.len as usize) >= crate::net::ipv4::IPHDR_LEN {
        // SAFETY: skb.data and skb.len describe a valid byte range.
        let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };
        crate::net::ipv4::IpHdr::from_bytes(data)
            .filter(|hdr| hdr.version_ihl >> 4 == 4)
            .map(|hdr| u32::from_be(hdr.daddr))
    } else {
        None
    };

    if let Some(ip) = dest_ip {
        if ip >> 24 == 127 {
            eth_push_header(&mut skb, [0, 0, 0, 0, 0, 0], src_mac, EthProtocol::ETH_P_IP)?;
            let _ = crate::drivers::net::loopback::loopback_send(skb);
            return Ok(());
        }
    }

    // R35: on an ARP miss, park the packet on the bounded per-IP pending
    // queue and send the ARP request now — the packet follows the reply
    // as a proper unicast frame. This replaces the old broadcast fallback
    // that lost the first packet to every new destination and relied on
    // upper-layer retries to mask it.
    let dest_mac = match dest_ip {
        Some(ip) => match crate::net::arp::arp_lookup(ip) {
            Some(mac) => mac,
            None => return crate::net::arp::arp_pending_send(ip, skb),
        },
        // Not IPv4 (or unparseable): nothing to resolve — keep the legacy
        // broadcast behavior for these rare frames.
        None => ETH_BROADCAST,
    };

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
///
/// R34: returns the MAC the device actually reports (read from virtio-net
/// config space). The old hardcoded 52:54:00:12:34:56 made every outbound
/// frame carry a source MAC different from the device's — QEMU's virtio-net
/// RX filter (device MAC + broadcast only, no CTRL_VQ promisc support) then
/// dropped every slirp reply, because the peer answered the MAC it saw, not
/// the one the device owns.
fn get_device_mac() -> Option<[u8; 6]> {
    if let Some(device) = crate::drivers::net::virtio_net::get_device() {
        return Some(device.get_mac());
    }

    None
}

/// Send packet to network device (pub(crate): also used by the ARP
/// pending-queue flush in arp.rs)
pub(crate) fn transmit_to_device(skb: SkBuff) -> i32 {
    if let Some(device) = crate::drivers::net::virtio_net::get_device() {
        return device.xmit(skb);
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
pub fn ethernet_rcv(mut skb: SkBuff) -> Result<(), ()> {
    // Pull the Ethernet header OFF the skb before dispatch: ip_rcv and
    // arp_rcv expect to start at their own headers. The old code handed
    // them the frame with the 14-byte Ethernet header still attached, so
    // every inbound packet was parsed at the wrong offset and silently
    // dropped (review NET-C1).
    let eth_hdr = match eth_pull_header(&mut skb) {
        Some(hdr) => hdr,
        None => {
            skb.free();
            return Err(());
        }
    };

    let protocol = eth_hdr.protocol();

    match protocol {
        EthProtocol::ETH_P_IP => {
            crate::net::ipv4::ip_rcv(&mut skb)?;
        }
        EthProtocol::ETH_P_ARP => {
            let _ = crate::net::arp::arp_rcv(&skb, eth_hdr);
        }
        _ => {
            // Unknown ethertype (VLAN, IPv6, ...): drop instead of the old
            // unwrap_or(ETH_P_IP) fallback that mis-parsed them as IPv4.
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
    // R35: drop ARP-parked packets whose resolution timed out (bounded
    // per pass; frees happen outside the ARP_PENDING lock).
    crate::net::arp::arp_pending_gc();

    // Drain the loopback backlog FIRST: the virtio-net poll path has known
    // descriptor-handling defects (review DRIV NEW) and must not be able to
    // block loopback delivery.
    while let Some(skb) = crate::drivers::net::loopback::loopback_poll() {
        let _ = ethernet_rcv(skb);
    }

    if let Some(device) = crate::drivers::net::virtio_net::get_device() {
        while let Some(skb) = device.poll() {
            let _ = ethernet_rcv(skb);
        }
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
