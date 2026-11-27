//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Network Buffer (SkBuff)

// (INV-SKB-1: the sk_buff family — heads are allocator-owned blocks
// with headroom-tracked data pointers (alloc/reserve/put maintain the
// span invariants); data_slice_at rows are bounds-checked against the
// len tail; Send/Sync carried by the head's single-owner discipline.)
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

// Send/Sync commitments for SkBuff live in the audited unsafe.rs
// (net/unsafe.rs).

/// SkBuff owns its backing allocation; dropping a buffer that was not
/// explicitly freed releases it. Every error path that used to leak a
/// ~1.5KB skb now frees it automatically (review NET-C2 / M1).
impl Drop for SkBuff {
    fn drop(&mut self) {
        self.release();
    }
}

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
        // LL_MAX_HEADER: Ethernet(14) + IP(20) + TCP(20) = 54 bytes minimum
        // of push room; 128 covers options and future tunnels. The old 16
        // made every tcp_build_packet/ipv4_send skb_push fail outright
        // (review NET-C2).
        const NET_SKBUFF_HEADROOM: usize = 128;

        let headroom = NET_SKBUFF_HEADROOM;
        let data_size = if size == 0 {
            NET_SKBUFF_DATA_ALIGN
        } else {
            ((size as usize) + NET_SKBUFF_DATA_ALIGN - 1) / NET_SKBUFF_DATA_ALIGN * NET_SKBUFF_DATA_ALIGN
        };
        let alloc_size = headroom + data_size + NET_SKBUFF_DATA_ALIGN;

        let layout = alloc::alloc::Layout::from_size_align(alloc_size, NET_SKBUFF_DATA_ALIGN)
            .ok()?;

        // Alloc edge lives in the audited unsafe.rs (INV-NET-SKBUFF-ALLOC-1).
        let head = crate::net::edges::alloc_layout(layout);
        if head.is_null() {
            return None;
        }

        // Raw-core edges live in the audited unsafe.rs (INV-NET-SKBUFF-1).
        let data = crate::net::edges::ptr_add(head, headroom);
        let tail = data;
        let end = crate::net::edges::ptr_add(head, alloc_size);

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
    /// Releases allocated memory. Idempotent: nulling the pointers makes the
    /// Drop impl (and a later double free) a no-op.
    pub fn free(mut self) {
        self.release();
    }

    /// Release the backing allocation if still held.
    fn release(&mut self) {
        let size = (self.end as usize).wrapping_sub(self.head as usize);
        if self.head.is_null() {
            return;
        }
        if size == 0 || size > (isize::MAX as usize) {
            crate::pr_err!("skb free: corrupted buffer head={:p} end={:p} size={}",
                self.head, self.end, size);
            return;
        }
        // Free edge lives in the audited unsafe.rs (INV-NET-SKBUFF-ALLOC-1).
        crate::net::edges::dealloc_checked(self.head, size);
        self.head = core::ptr::null_mut();
        self.end = core::ptr::null_mut();
        self.data = core::ptr::null_mut();
        self.tail = core::ptr::null_mut();
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
        let new_tail = (self.tail as usize).checked_add(len as usize)?;
        if new_tail > self.end as usize {
            return None;
        }

        let ptr = self.tail;
        // Raw-core edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        self.tail = crate::net::edges::ptr_add(self.tail, len as usize);
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
        let new_data = (self.data as usize).checked_sub(len as usize)?;
        if new_data < self.head as usize {
            return None;
        }

        // Raw-core edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        self.data = crate::net::edges::ptr_sub(self.data, len as usize);
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

        // Raw-core edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        let new_data = crate::net::edges::ptr_add(self.data, len as usize);
        // Ensure data pointer does not pass tail (invariant: head <= data <= tail <= end)
        if new_data as usize > self.tail as usize {
            return None;
        }
        self.data = new_data;
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

        // Raw-core edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        self.tail = crate::net::edges::ptr_add(self.tail, len as usize);
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

        // Copy edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        crate::net::edges::copy_bytes(data.as_ptr(), ptr, data.len());

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

    /// Borrow `n` bytes of the data area as a shared slice (clamped to
    /// the buffer length). Packet buffers are single-owner by
    /// construction (`alloc_skb` hands out exclusive buffers), so the
    /// borrow cannot alias another live reference.
    pub fn data_slice(&self, n: u32) -> &[u8] {
        let n = n.min(self.len);
        // Raw-core edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        crate::net::edges::slice_parts(self.data, n as usize)
    }

    /// Borrow `n` bytes of the data area starting at `off` as a shared
    /// slice (clamped to the buffer end).
    pub fn data_slice_at(&self, off: u32, n: u32) -> &[u8] {
        if off >= self.len {
            return &[];
        }
        let n = n.min(self.len - off);
        // Raw-core edges live in the audited unsafe.rs (INV-NET-SKBUFF-1).
        crate::net::edges::slice_parts(
            crate::net::edges::ptr_add(self.data, off as usize),
            n as usize,
        )
    }

    /// Typed read view of the header at the data-area start: `T` must
    /// fit within `len` (returns None otherwise — a corrupt/truncated
    /// frame yields no view instead of an out-of-bounds read).
    pub fn hdr<T>(&self) -> Option<&T> {
        if (core::mem::size_of::<T>() as u32) > self.len {
            return None;
        }
        // Typed-view edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        Some(crate::net::edges::typed_ref(self.data as *const T))
    }

    /// Typed write view of the header at the data-area start: `T` must
    /// fit within `len` (None otherwise).
    pub fn hdr_mut<T>(&mut self) -> Option<&mut T> {
        if (core::mem::size_of::<T>() as u32) > self.len {
            return None;
        }
        // Typed-view edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        Some(crate::net::edges::typed_mut(self.data as *mut T))
    }

    /// Typed write view of the header plus the byte tail behind it, in
    /// one borrow (the header checksum-over-segment shape): returns
    /// None when `T` does not fit. The two borrows are disjoint ranges
    /// of the exclusively-owned buffer.
    pub fn hdr_and_tail<T>(&mut self) -> Option<(&mut T, &[u8])> {
        let hlen = core::mem::size_of::<T>() as u32;
        if hlen > self.len {
            return None;
        }
        // Disjoint header+tail view lives in the audited unsafe.rs
        // (INV-NET-SKBUFF-1).
        Some(crate::net::edges::hdr_tail_view(
            self.data,
            hlen as usize,
            (self.len - hlen) as usize,
        ))
    }

    /// Push `size_of::<T>()` bytes of headroom and hand the caller a
    /// typed mutable view of them (the header-BUILD edge — round-5
    /// consolidation: every protocol's `&mut *(ptr as *mut Hdr)` push
    /// site routes through here).
    ///
    /// INH-BUF-4: `T` is a repr(C) wire header; the view is exactly the
    /// pushed bytes; the buffer stays exclusively owned.
    pub fn push_hdr<T: Copy>(&mut self, build: impl FnOnce(&mut T)) -> Option<()> {
        let ptr = self.skb_push(core::mem::size_of::<T>() as u32)?;
        // Typed-view edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        let h: &mut T = crate::net::edges::typed_mut(ptr.cast::<T>());
        build(h);
        Some(())
    }

    /// Append `size_of::<T>()` bytes at the tail and hand a typed
    /// mutable view (the header-APPEND edge — protocols that build
    /// payload-first, header-last, e.g. ARP). INH-BUF-4 as push_hdr.
    pub fn put_hdr<T: Copy>(&mut self, build: impl FnOnce(&mut T)) -> Option<()> {
        let ptr = self.skb_put(core::mem::size_of::<T>() as u32)?;
        // Typed-view edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        let h: &mut T = crate::net::edges::typed_mut(ptr.cast::<T>());
        build(h);
        Some(())
    }

    /// Push raw bytes as new headroom (the byte-level twin of
    /// push_hdr_val — INH-BUF-4).
    pub fn push_bytes(&mut self, src: &[u8]) -> Option<()> {
        let ptr = self.skb_push(src.len() as u32)?;
        // Copy edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        crate::net::edges::copy_bytes(src.as_ptr(), ptr, src.len());
        Some(())
    }

    /// Push a ready-made header VALUE as new headroom (the build edge
    /// for callers that compute the header on the stack — INH-BUF-4).
    pub fn push_hdr_val<T: Copy>(&mut self, hdr: &T) -> Option<()> {
        let ptr = self.skb_push(core::mem::size_of::<T>() as u32)?;
        // Copy edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
        crate::net::edges::copy_bytes(
            hdr as *const T as *const u8,
            ptr,
            core::mem::size_of::<T>(),
        );
        Some(())
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

        // Raw-core + copy edges live in the audited unsafe.rs
        // (INV-NET-SKBUFF-1).
        crate::net::edges::copy_bytes(
            crate::net::edges::ptr_add(self.data, offset as usize),
            buf.as_mut_ptr(),
            copy_len as usize,
        );

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


// ==================== Net-family header-view edges (round-5) ====================

impl SkBuff {
    /// The whole data area as a shared slice (safe composition of
    /// `data_slice` at full length — round-5).
    pub fn data_all(&self) -> &[u8] {
        self.data_slice(self.len)
    }
}

/// View the head of `data` as a shared `&T` wire header — the ONE
/// audited parse edge shared by the protocol modules (replaces the
/// per-protocol `&*(data.as_ptr() as *const Hdr)` duplicates).
///
/// INH-BUF-3: `T: Copy` (a repr(C) wire header) and `data` has at
/// least `size_of::<T>()` bytes (checked; None otherwise). The borrow
/// is tied to `data` — no invented lifetimes.
pub fn hdr_view<T: Copy>(data: &[u8]) -> Option<&T> {
    if data.len() < core::mem::size_of::<T>() {
        return None;
    }
    // Typed-view edge lives in the audited unsafe.rs (INV-NET-SKBUFF-1).
    Some(crate::net::edges::typed_ref(data.as_ptr() as *const T))
}

/// The mutable twin of [`hdr_view`] (the build side over a caller-owned
/// byte region).
pub fn hdr_view_mut<T: Copy>(data: &mut [u8]) -> Option<&mut T> {
    if data.len() < core::mem::size_of::<T>() {
        return None;
    }
    // Typed-view edge as hdr_view, over the exclusive borrow
    // (INV-NET-SKBUFF-1).
    Some(crate::net::edges::typed_mut(data.as_mut_ptr() as *mut T))
}
