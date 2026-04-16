//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ARP Protocol

use crate::net::buffer::SkBuff;
use crate::net::ethernet::{ETH_ALEN, eth_is_broadcast_addr};
use crate::config::ARP_CACHE_SIZE;
use crate::drivers::timer::get_jiffies;
use crate::drivers::timer::HZ;
use crate::sync::spinlock::Spinlock;

/// ARP hardware types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpHrd {
    /// Ethernet
    ARPHRD_ETHER = 1,
    /// Loopback device
    ARPHRD_LOOPBACK = 772,
    /// None
    ARPHRD_VOID = 0xFFFF,
}

/// ARP protocol types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpPro {
    /// IPv4
    ARPPROTO_IP = 0x0800,
    /// IPv6
    ARPPROTO_IPV6 = 0x86DD,
}

/// ARP operation types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ArpOp {
    /// ARP request
    ARPOP_REQUEST = 1,
    /// ARP reply
    ARPOP_REPLY = 2,
    /// RARP request
    ARPOP_RREQUEST = 3,
    /// RARP reply
    ARPOP_RREPLY = 4,
}

/// ARP packet header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArpHdr {
    /// Hardware type (e.g., ARPHRD_ETHER = 1)
    pub ar_hrd: u16,
    /// Protocol type (e.g., ETH_P_IP = 0x0800)
    pub ar_pro: u16,
    /// Hardware address length (Ethernet = 6)
    pub ar_hln: u8,
    /// Protocol address length (IPv4 = 4)
    pub ar_pln: u8,
    /// Operation type (ARPOP_REQUEST/ARPOP_REPLY)
    pub ar_op: u16,
}

/// ARP packet (Ethernet + IPv4)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArpPacket {
    /// ARP header
    pub hdr: ArpHdr,
    /// Sender hardware address (MAC)
    pub ar_sha: [u8; ETH_ALEN],
    /// Sender protocol address (IP)
    pub ar_sip: u32,
    /// Target hardware address (MAC)
    pub ar_tha: [u8; ETH_ALEN],
    /// Target protocol address (IP)
    pub ar_tip: u32,
}

impl ArpPacket {
    /// Total ARP packet length (Ethernet + IPv4)
    pub const LEN: usize = core::mem::size_of::<ArpPacket>();

    /// Create ARP packet from byte slice
    pub fn from_bytes(data: &[u8]) -> Option<&'static Self> {
        if data.len() < Self::LEN {
            return None;
        }

        // SAFETY: data has at least ArpPacket::LEN bytes; lifetime is 'static
        // because it aliases skb data which lives until packet is freed.
        unsafe {
            Some(&*(data.as_ptr() as *const ArpPacket))
        }
    }

    /// Check if this is an ARP request
    pub fn is_request(&self) -> bool {
        u16::from_be(self.hdr.ar_op) == ArpOp::ARPOP_REQUEST as u16
    }

    /// Check if this is an ARP reply
    pub fn is_reply(&self) -> bool {
        u16::from_be(self.hdr.ar_op) == ArpOp::ARPOP_REPLY as u16
    }

    /// Get sender MAC address
    pub fn sender_mac(&self) -> [u8; ETH_ALEN] {
        self.ar_sha
    }

    /// Get sender IP address
    pub fn sender_ip(&self) -> u32 {
        u32::from_be(self.ar_sip)
    }

    /// Get target MAC address
    pub fn target_mac(&self) -> [u8; ETH_ALEN] {
        self.ar_tha
    }

    /// Get target IP address
    pub fn target_ip(&self) -> u32 {
        u32::from_be(self.ar_tip)
    }
}

/// ARP cache entry
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArpEntry {
    /// IP address
    pub ip: u32,
    /// MAC address
    pub mac: [u8; ETH_ALEN],
    /// Last update time
    pub last_updated: u64,
    /// Whether this entry is valid
    pub valid: bool,
}

impl ArpEntry {
    /// Create a new ARP cache entry
    pub fn new(ip: u32, mac: [u8; ETH_ALEN]) -> Self {
        Self {
            ip,
            mac,
            last_updated: get_jiffies(),
            valid: true,
        }
    }

    /// Check if this entry has expired
    ///
    /// # Arguments
    /// - `timeout`: Timeout duration (in seconds)
    ///
    /// # Returns
    /// Whether the entry has expired
    pub fn is_expired(&self, timeout: u64) -> bool {
        let timeout_jiffies = timeout * HZ;
        get_jiffies().saturating_sub(self.last_updated) > timeout_jiffies
    }
}

/// ARP cache
struct ArpCache {
    entries: [ArpEntry; ARP_CACHE_SIZE],
    count: usize,
}

impl ArpCache {
    const fn new() -> Self {
        const EMPTY_ENTRY: ArpEntry = ArpEntry {
            ip: 0,
            mac: [0; ETH_ALEN],
            last_updated: 0,
            valid: false,
        };

        Self {
            entries: [EMPTY_ENTRY; ARP_CACHE_SIZE],
            count: 0,
        }
    }

    /// Look up ARP cache entry
    fn lookup(&self, ip: u32) -> Option<ArpEntry> {
        for entry in self.entries.iter() {
            if entry.valid && entry.ip == ip {
                return Some(*entry);
            }
        }
        None
    }

    /// Default ARP entry timeout in seconds
    const ARP_TIMEOUT_SECS: u64 = 300; // 5 minutes

    /// Add or update ARP cache entry
    fn update(&mut self, ip: u32, mac: [u8; ETH_ALEN]) {
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.ip == ip {
                entry.mac = mac;
                entry.last_updated = get_jiffies();
                return;
            }
        }

        if self.count < ARP_CACHE_SIZE {
            self.entries[self.count] = ArpEntry::new(ip, mac);
            self.count += 1;
        } else {
            // Cache full — prefer replacing an expired entry
            let mut evict_idx = 0;
            let mut found_expired = false;
            for (i, entry) in self.entries.iter().enumerate() {
                if entry.valid && entry.is_expired(Self::ARP_TIMEOUT_SECS) {
                    evict_idx = i;
                    found_expired = true;
                    break;
                }
            }
            // Fallback: replace the least-recently-used valid entry
            if !found_expired {
                let mut min_time = u64::MAX;
                for (i, entry) in self.entries.iter().enumerate() {
                    if entry.valid && entry.last_updated < min_time {
                        min_time = entry.last_updated;
                        evict_idx = i;
                    }
                }
            }
            self.entries[evict_idx] = ArpEntry::new(ip, mac);
        }
    }

    /// Remove ARP cache entry
    fn remove(&mut self, ip: u32) {
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.ip == ip {
                entry.valid = false;
                self.count -= 1;
                return;
            }
        }
    }

    /// Clear ARP cache
    fn clear(&mut self) {
        self.count = 0;
        for entry in self.entries.iter_mut() {
            entry.valid = false;
        }
    }
}

/// Global ARP cache (Spinlock-protected for concurrent access from
/// syscall context and softirq/timer context).
static ARP_CACHE: Spinlock<ArpCache> = Spinlock::new(ArpCache::new());

/// Look up ARP cache
pub fn arp_lookup(ip: u32) -> Option<[u8; ETH_ALEN]> {
    let cache = ARP_CACHE.lock();
    cache.lookup(ip).map(|entry| entry.mac)
}

/// Update ARP cache
pub fn arp_update(ip: u32, mac: [u8; ETH_ALEN]) {
    ARP_CACHE.lock().update(ip, mac);
}

/// Remove ARP cache entry
pub fn arp_remove(ip: u32) {
    ARP_CACHE.lock().remove(ip);
}

/// Clear ARP cache
pub fn arp_clear() {
    ARP_CACHE.lock().clear();
}

/// Build ARP request packet
///
/// # Arguments
/// - `skb`: SkBuff
/// - `sender_mac`: Sender MAC address
/// - `sender_ip`: Sender IP address (network byte order)
/// - `target_ip`: Target IP address (network byte order)
pub fn arp_build_request(
    skb: &mut SkBuff,
    sender_mac: [u8; ETH_ALEN],
    sender_ip: u32,
    target_ip: u32,
) -> Result<(), ()> {
    let ptr = skb.skb_put(ArpPacket::LEN as u32).ok_or(())?;

    // SAFETY: skb_put returned a valid pointer of at least ArpPacket::LEN bytes;
    // writing fields of repr(C) ArpPacket is well-defined.
    unsafe {
        let arp_pkt = &mut *(ptr as *mut ArpPacket);

        arp_pkt.hdr.ar_hrd = (ArpHrd::ARPHRD_ETHER as u16).to_be();
        arp_pkt.hdr.ar_pro = (ArpPro::ARPPROTO_IP as u16).to_be();
        arp_pkt.hdr.ar_hln = ETH_ALEN as u8;
        arp_pkt.hdr.ar_pln = 4;
        arp_pkt.hdr.ar_op = (ArpOp::ARPOP_REQUEST as u16).to_be();

        arp_pkt.ar_sha = sender_mac;
        arp_pkt.ar_sip = sender_ip;

        arp_pkt.ar_tha = [0; ETH_ALEN];
        arp_pkt.ar_tip = target_ip;
    }

    Ok(())
}

/// Build ARP reply packet
///
/// # Arguments
/// - `skb`: SkBuff
/// - `sender_mac`: Sender MAC address
/// - `sender_ip`: Sender IP address (network byte order)
/// - `target_mac`: Target MAC address
/// - `target_ip`: Target IP address (network byte order)
pub fn arp_build_reply(
    skb: &mut SkBuff,
    sender_mac: [u8; ETH_ALEN],
    sender_ip: u32,
    target_mac: [u8; ETH_ALEN],
    target_ip: u32,
) -> Result<(), ()> {
    let ptr = skb.skb_put(ArpPacket::LEN as u32).ok_or(())?;

    // SAFETY: skb_put returned a valid pointer of at least ArpPacket::LEN bytes;
    // writing fields of repr(C) ArpPacket is well-defined.
    unsafe {
        let arp_pkt = &mut *(ptr as *mut ArpPacket);

        arp_pkt.hdr.ar_hrd = (ArpHrd::ARPHRD_ETHER as u16).to_be();
        arp_pkt.hdr.ar_pro = (ArpPro::ARPPROTO_IP as u16).to_be();
        arp_pkt.hdr.ar_hln = ETH_ALEN as u8;
        arp_pkt.hdr.ar_pln = 4;
        arp_pkt.hdr.ar_op = (ArpOp::ARPOP_REPLY as u16).to_be();

        arp_pkt.ar_sha = sender_mac;
        arp_pkt.ar_sip = sender_ip;

        arp_pkt.ar_tha = target_mac;
        arp_pkt.ar_tip = target_ip;
    }

    Ok(())
}

/// Receive and process ARP packet
///
/// # Arguments
/// - `skb`: SkBuff (containing ARP packet)
/// - `eth_hdr`: Ethernet header
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn arp_rcv(skb: &SkBuff, eth_hdr: &crate::net::ethernet::EthHdr) -> Result<(), ()> {
    // SAFETY: skb.data and skb.len describe a valid byte range in the skb buffer.
    let data = unsafe { core::slice::from_raw_parts(skb.data, skb.len as usize) };

    let arp_pkt = ArpPacket::from_bytes(data).ok_or(())?;

    if u16::from_be(arp_pkt.hdr.ar_hrd) != (ArpHrd::ARPHRD_ETHER as u16) {
        return Ok(());
    }

    if u16::from_be(arp_pkt.hdr.ar_pro) != (ArpPro::ARPPROTO_IP as u16) {
        return Ok(());
    }

    let sender_ip = arp_pkt.sender_ip();
    let sender_mac = arp_pkt.sender_mac();
    arp_update(sender_ip, sender_mac);

    if arp_pkt.is_request() {
        let target_ip = arp_pkt.target_ip();

        if is_local_ip(target_ip) {
            let local_mac = get_local_mac();
            let local_ip = get_local_ip();

            let _ = send_arp_reply(
                local_mac,
                local_ip,
                sender_mac,
                sender_ip,
            );
        }
    }

    Ok(())
}

/// Check if IP is local IP
fn is_local_ip(ip: u32) -> bool {
    let local_ip = get_local_ip();
    ip == local_ip
}

/// Get local IP address
fn get_local_ip() -> u32 {
    0xC0A80164
}

/// Get local MAC address
fn get_local_mac() -> [u8; ETH_ALEN] {
    if let Some(device) = crate::drivers::net::virtio_net::get_device() {
        return device.get_mac();
    }
    [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]
}

/// Send ARP reply
///
/// # Arguments
/// - `sender_mac`: Sender (local) MAC address
/// - `sender_ip`: Sender (local) IP address
/// - `target_mac`: Target MAC address
/// - `target_ip`: Target IP address
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
fn send_arp_reply(
    sender_mac: [u8; ETH_ALEN],
    sender_ip: u32,
    target_mac: [u8; ETH_ALEN],
    target_ip: u32,
) -> Result<(), ()> {
    let mut skb = crate::net::buffer::alloc_skb(128).ok_or(())?;

    arp_build_reply(&mut skb, sender_mac, sender_ip.to_be(), target_mac, target_ip.to_be())?;

    crate::net::ethernet::eth_push_header(
        &mut skb,
        target_mac,
        sender_mac,
        crate::net::buffer::EthProtocol::ETH_P_ARP,
    )?;

    transmit_arp_packet(skb);

    Ok(())
}

/// Send ARP request
///
/// # Arguments
/// - `target_ip`: Target IP address (host byte order)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn send_arp_request(target_ip: u32) -> Result<(), ()> {
    let sender_mac = get_local_mac();
    let sender_ip = get_local_ip();

    let mut skb = crate::net::buffer::alloc_skb(128).ok_or(())?;

    arp_build_request(&mut skb, sender_mac, sender_ip.to_be(), target_ip.to_be())?;

    let broadcast_mac = crate::net::ethernet::ETH_BROADCAST;
    crate::net::ethernet::eth_push_header(
        &mut skb,
        broadcast_mac,
        sender_mac,
        crate::net::buffer::EthProtocol::ETH_P_ARP,
    )?;

    transmit_arp_packet(skb);

    Ok(())
}

/// Transmit ARP packet
fn transmit_arp_packet(skb: SkBuff) {
    if let Some(device) = crate::drivers::net::virtio_net::get_device() {
        device.xmit(skb);
        return;
    }

    crate::drivers::net::loopback::loopback_send(skb);
}

/// Resolve IP address to MAC address
///
/// First looks up the ARP cache, sends ARP request if not found
///
/// # Arguments
/// - `ip`: Target IP address (host byte order)
///
/// # Returns
/// MAC address if found in cache, None otherwise (sends ARP request)
pub fn resolve_ip(ip: u32) -> Option<[u8; ETH_ALEN]> {
    if let Some(mac) = arp_lookup(ip.to_be()) {
        return Some(mac);
    }

    let _ = send_arp_request(ip);

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arp_packet_size() {
        assert_eq!(core::mem::size_of::<ArpPacket>(), 28);
    }

    #[test]
    fn test_arp_cache_lookup() {
        let ip = 0xC0A80101;
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

        unsafe {
            // SAFETY: test context; ARP_CACHE is a global static.
            ARP_CACHE.update(ip, mac);
        }

        let result = arp_lookup(ip);
        assert_eq!(result, Some(mac));
    }
}
