//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network Buffer (SkBuff)

use core::sync::atomic::AtomicU64;

/// Packet types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// Packet sent to this host
    Host = 0,
    /// Packet sent to another host
    Otherhost = 1,
    /// Broadcast packet
    Broadcast = 2,
    /// Multicast packet
    Multicast = 3,
}

/// Ethernet protocol types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum EthProtocol {
    /// IPv4
    ETH_P_IP = 0x0800,
    /// ARP
    ETH_P_ARP = 0x0806,
    /// IPv6
    ETH_P_IPV6 = 0x86DD,
    /// 802.1Q VLAN
    ETH_P_8021Q = 0x8100,
}

impl EthProtocol {
    /// Convert from u16
    pub fn from_u16(val: u16) -> Option<Self> {
        match val {
            0x0800 => Some(EthProtocol::ETH_P_IP),
            0x0806 => Some(EthProtocol::ETH_P_ARP),
            0x86DD => Some(EthProtocol::ETH_P_IPV6),
            0x8100 => Some(EthProtocol::ETH_P_8021Q),
            _ => None,
        }
    }

    /// Convert to u16
    pub fn to_u16(self) -> u16 {
        self as u16
    }
}

/// IP protocol types
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum IpProtocol {
    /// IP
    IPPROTO_IP = 0,
    /// ICMP
    IPPROTO_ICMP = 1,
    /// TCP
    IPPROTO_TCP = 6,
    /// UDP
    IPPROTO_UDP = 17,
    /// IPv6
    IPPROTO_IPV6 = 41,
}

impl IpProtocol {
    /// Convert from u8
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(IpProtocol::IPPROTO_IP),
            1 => Some(IpProtocol::IPPROTO_ICMP),
            6 => Some(IpProtocol::IPPROTO_TCP),
            17 => Some(IpProtocol::IPPROTO_UDP),
            41 => Some(IpProtocol::IPPROTO_IPV6),
            _ => None,
        }
    }

    /// Convert to u8
    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

/// Network buffer (SkBuff)
///
/// # Memory Layout
/// ```text
/// |<- head                 ->|<- data       ->|<- tail ->|<- end ->|
/// |  (headroom)             |  (actual data) | (tailroom) |
/// ```
#[repr(C)]
pub struct SkBuff {
    /// Protocol type (ETH_P_IP, ETH_P_ARP, etc.)
    pub protocol: u16,
    /// Packet length
    pub len: u32,
    /// Data pointer (points to current protocol layer data start)
    pub data: *mut u8,
    /// Tail pointer (points to data end)
    pub tail: *mut u8,
    /// Buffer end pointer
    pub end: *mut u8,
    /// Buffer start pointer
    pub head: *mut u8,
    /// Packet type
    pub pkt_type: PacketType,
    /// Timestamp
    pub tstamp: u64,
    /// MAC address length (for Ethernet)
    pub mac_len: u8,
    /// MAC header pointer
    pub mac_header: *mut u8,
    /// Network layer header pointer
    pub network_header: *mut u8,
    /// Transport layer header pointer
    pub transport_header: *mut u8,
}

unsafe impl Send for SkBuff {}

/// SkBuff global allocator ID
static SKBUFF_ALLOCATOR_ID: AtomicU64 = AtomicU64::new(0);

impl SkBuff {
    /// Allocate a new SkBuff
    ///
    /// # Arguments
    /// - `size`: Data size (in bytes)
    ///
    /// # Returns
    /// The allocated SkBuff, or None if allocation fails
    ///
    /// # Notes
    /// - Buffer size allocated is `size + 2 * NET_SKBUFF_DATA_ALIGN` (for headroom and tailroom)
    /// - data and tail initially point to position after headroom
    /// - headroom is for adding protocol headers (MAC, IP, TCP, etc.)
    pub fn alloc(size: u32) -> Option<Self> {
        const NET_SKBUFF_DATA_ALIGN: usize = 16;

        let headroom = NET_SKBUFF_DATA_ALIGN;
        let data_size = if size == 0 {
            NET_SKBUFF_DATA_ALIGN
        } else {
            ((size as usize) + NET_SKBUFF_DATA_ALIGN - 1) / NET_SKBUFF_DATA_ALIGN * NET_SKBUFF_DATA_ALIGN
        };
        let alloc_size = headroom + data_size + NET_SKBUFF_DATA_ALIGN;

        let layout = alloc::alloc::Layout::from_size_align(alloc_size, NET_SKBUFF_DATA_ALIGN)
            .ok()?;

        let head = unsafe { alloc::alloc::alloc(layout) };
        if head.is_null() {
            return None;
        }

        let data = unsafe { head.add(headroom) };
        let tail = data;
        let end = unsafe { head.add(alloc_size) };

        Some(SkBuff {
            protocol: 0,
            len: 0,
            data,
            tail,
            end,
            head,
            pkt_type: PacketType::Host,
            tstamp: 0,
            mac_len: 0,
            mac_header: core::ptr::null_mut(),
            network_header: core::ptr::null_mut(),
            transport_header: core::ptr::null_mut(),
        })
    }

    /// Free SkBuff
    ///
    /// # Notes
    /// Releases allocated memory
    pub fn free(self) {
        unsafe {
            let layout = alloc::alloc::Layout::from_size_align(
                (self.end as usize) - (self.head as usize),
                16,
            ).unwrap();
            alloc::alloc::dealloc(self.head, layout);
        }
    }

    /// Add data at tail
    ///
    /// # Arguments
    /// - `len`: Length of data to add
    ///
    /// # Returns
    /// Pointer to the added position, or None if insufficient space
    ///
    /// # Notes
    /// - Moves tail pointer forward
    /// - Increases len
    pub fn skb_put(&mut self, len: u32) -> Option<*mut u8> {
        if self.tail as usize + len as usize > self.end as usize {
            return None;
        }

        let ptr = self.tail;
        self.tail = unsafe { self.tail.add(len as usize) };
        self.len += len;
        Some(ptr)
    }

    /// Add data at head
    ///
    /// # Arguments
    /// - `len`: Length of data to add
    ///
    /// # Returns
    /// Pointer to the added position, or None if insufficient space
    ///
    /// # Notes
    /// - Moves data pointer backward
    /// - Increases len
    pub fn skb_push(&mut self, len: u32) -> Option<*mut u8> {
        if (self.data as usize) < (self.head as usize + len as usize) {
            return None;
        }

        self.data = unsafe { self.data.sub(len as usize) };
        self.len += len;
        Some(self.data)
    }

    /// Remove data from head
    ///
    /// # Arguments
    /// - `len`: Length of data to remove
    ///
    /// # Returns
    /// Data pointer after removal
    ///
    /// # Notes
    /// - Moves data pointer forward
    /// - Decreases len
    pub fn skb_pull(&mut self, len: u32) -> Option<*mut u8> {
        if len > self.len {
            return None;
        }

        self.data = unsafe { self.data.add(len as usize) };
        self.len -= len;
        Some(self.data)
    }

    /// Reserve space at tail
    ///
    /// # Arguments
    /// - `len`: Length of space to reserve
    ///
    /// # Returns
    /// Pointer to reserved position, or None if insufficient space
    ///
    /// # Notes
    /// - Moves tail pointer forward, but does not increase len
    pub fn skb_reserve(&mut self, len: u32) -> Option<*mut u8> {
        if self.tail as usize + len as usize > self.end as usize {
            return None;
        }

        self.tail = unsafe { self.tail.add(len as usize) };
        self.data = self.tail;
        Some(self.data)
    }

    /// Write data to tail position
    ///
    /// # Arguments
    /// - `data`: Data to write
    ///
    /// # Returns
    /// Ok(()) on success, Err(()) on failure
    ///
    /// # Notes
    /// - First calls skb_put to get space
    /// - Then copies data to that space
    pub fn skb_put_data(&mut self, data: &[u8]) -> Result<(), ()> {
        let len = data.len() as u32;
        let ptr = self.skb_put(len).ok_or(())?;

        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        }

        Ok(())
    }

    /// Set MAC header
    ///
    /// # Arguments
    /// - `len`: MAC header length
    pub fn set_mac_header(&mut self, len: u8) {
        self.mac_header = self.data;
        self.mac_len = len;
    }

    /// Set network layer header
    ///
    /// # Notes
    /// Current data pointer position is the network layer header
    pub fn set_network_header(&mut self) {
        self.network_header = self.data;
    }

    /// Set transport layer header
    ///
    /// # Notes
    /// Current data pointer position is the transport layer header
    pub fn set_transport_header(&mut self) {
        self.transport_header = self.data;
    }

    /// Get MAC header
    pub fn get_mac_header(&self) -> *const u8 {
        self.mac_header
    }

    /// Get network layer header
    pub fn get_network_header(&self) -> *const u8 {
        self.network_header
    }

    /// Get transport layer header
    pub fn get_transport_header(&self) -> *const u8 {
        self.transport_header
    }

    /// Get data pointer
    pub fn data(&self) -> *const u8 {
        self.data
    }

    /// Get mutable data pointer
    pub fn data_mut(&mut self) -> *mut u8 {
        self.data
    }

    /// Get data length
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Copy SkBuff data
    ///
    /// # Arguments
    /// - `buf`: Destination buffer
    /// - `offset`: Offset
    /// - `len`: Copy length
    ///
    /// # Returns
    /// Actual number of bytes copied
    pub fn skb_copy_bits(&self, offset: u32, buf: &mut [u8], len: u32) -> u32 {
        if offset > self.len {
            return 0;
        }

        let copy_len = core::cmp::min(len, self.len - offset);
        if copy_len == 0 {
            return 0;
        }

        unsafe {
            let src = self.data.add(offset as usize);
            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), copy_len as usize);
        }

        copy_len
    }
}

/// Helper function to allocate SkBuff
///
/// # Arguments
/// - `size`: Data size
///
/// # Returns
/// The allocated SkBuff
pub fn alloc_skb(size: u32) -> Option<SkBuff> {
    SkBuff::alloc(size)
}

/// Helper function to free SkBuff
///
/// # Arguments
/// - `skb`: SkBuff to free
pub fn kfree_skb(skb: SkBuff) {
    skb.free();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skb_alloc() {
        let skb = SkBuff::alloc(1500);
        assert!(skb.is_some());
        let skb = skb.unwrap();
        assert_eq!(skb.len(), 0);
        assert!(skb.is_empty());
    }

    #[test]
    fn test_skb_put() {
        let mut skb = SkBuff::alloc(1500).unwrap();
        let data = b"Hello, World!";

        assert!(skb.skb_put_data(data).is_ok());
        assert_eq!(skb.len(), data.len() as u32);
        assert!(!skb.is_empty());
    }

    #[test]
    fn test_skb_push() {
        let mut skb = SkBuff::alloc(1500).unwrap();

        skb.skb_put_data(b"World!").unwrap();

        let ptr = skb.skb_push(7).unwrap();
        unsafe {
            core::ptr::copy_nonoverlapping(b"Hello, ".as_ptr(), ptr, 7);
        }

        assert_eq!(skb.len(), 13);
    }

    #[test]
    fn test_skb_pull() {
        let mut skb = SkBuff::alloc(1500).unwrap();
        skb.skb_put_data(b"Hello, World!").unwrap();

        skb.skb_pull(7);
        assert_eq!(skb.len(), 6);
    }
}
