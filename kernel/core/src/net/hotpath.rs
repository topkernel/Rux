//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The net subsystem's code (seed-core-design §2c.3): everything of the
//! subsystem that is neither `unsafe.rs` nor `types.rs`, in one file.
//!
//! Sections follow the order of the old module declarations; each banner
//! names the topic file it was (file_arc was file_arc.rs) and the file's
//! module doc follows it as the section's header comment. What lived
//! directly in mod.rs — the three socket-file Arc edges — is the first
//! section. Inside a section the items keep their original order and
//! text. Type definitions are in `types.rs`, the audited unsafe edges in
//! `unsafe.rs`.
//!
//! None of the old files carried a per-file `#![forbid(unsafe_code)]`
//! inner attribute, so none was dropped; the scattered unsafe of the
//! sections is collected in pass 3.
//!
//! The file's NAME overstates until the shrink stage has run: it holds
//! all of the subsystem's code under the hot-path name, not a verified
//! set of hot-path functions (§2c.3, user decision 2026-11-25).
//!
//! Two renames were forced by the merge: the mod.rs root items `file_arc`
//! and `take_file_arc` collide with the live twins of the same names in
//! file_arc.rs (the ones the net service crate re-exports at its root),
//! so the dead mod.rs copies take their file name as prefix
//! (`mod_file_arc`, `mod_take_file_arc`); `reclaim_file_arc` had no twin
//! and keeps its name. No caller changes: the mod.rs trio has none (each
//! is a never-used `pub(crate)` fn, dead-code warned at base).

use super::types::{EthProtocol, IpProtocol, PacketType, SkBuff};
use core::sync::atomic::AtomicU64;

use crate::fs::file::File;

// ═══════════════ net root ═════════════════════════════════════════
// (was mod.rs: its module declarations and its re-export of buffer's
//  items are gone — every path names types or hotpath now; the file's
//  module doc and its api note follow as this section's header comment)
// Network Subsystem

// The network hook table (NetHooks) is crate::api::net.

// ==================== Socket-file Arc edges (round-5) ====================

/// Recover a strong `Arc<T>` from a socket File's private_data slot —
/// the ONE audited Arc-recovery edge for the net family (INV-NETARC-1).
///
/// INV-NETARC-1 (provenance + liveness): the File's private_data holds
/// an `Arc::into_raw` pointer installed by the socket's own file-ops
/// installer, and the caller has ALREADY identity-checked the file
/// against that ops table (`*file.ops.get()` equality — the per-family
/// `*_socket_from_fd` checks). The File's lifetime keeps the Arc alive
/// (close() is the only drop path); increment-before-clone keeps the
/// returned Arc independent of the slot.
pub(crate) fn mod_file_arc<T>(file: &crate::fs::file::File) -> Option<alloc::sync::Arc<T>> {
    let ptr = file.private_data_ptr()? as *const T;
    // SAFETY: INV-NETARC-1 — the pointer is a live Arc::into_raw value
    // of this type (ops identity checked by the caller); the increment
    // balances the from_raw construction below.
    unsafe {
        alloc::sync::Arc::increment_strong_count(ptr);
        Some(alloc::sync::Arc::from_raw(ptr))
    }
}
/// Take the Arc OUT of a closing socket File's private_data slot and
/// HAND IT BACK (the close paths that must USE the socket before the
/// drop — flush held data, protocol close; INV-NETARC-1 + the caller's
/// ops identity check). The cell is emptied first, making a second
/// reclaim impossible.
pub(crate) fn reclaim_file_arc<T>(file: &crate::fs::file::File) -> Option<alloc::sync::Arc<T>> {
    let ptr = file.private_data_ptr()? as *const T;
    file.clear_private_data();
    // SAFETY: INV-NETARC-1 — reconstructing consumes the slot's single
    // reference; the emptied cell makes this exactly-once.
    unsafe { Some(alloc::sync::Arc::from_raw(ptr)) }
}
/// Take the Arc OUT of a closing socket File's private_data slot and
/// drop it (the close path — INV-NETARC-1 + the caller's ops identity
/// check; the slot is cleared under the file's own close serialization).
pub(crate) fn mod_take_file_arc<T>(file: &crate::fs::file::File) -> Option<()> {
    let ptr = file.private_data_ptr()? as *const T;
    // SAFETY: INV-NETARC-1 — reconstructing the stored Arc consumes the
    // slot's single reference (close is the last user of the file).
    unsafe {
        drop(alloc::sync::Arc::from_raw(ptr));
    }
    file.clear_private_data();
    Some(())
}

// ═══════════════ file_arc ═════════════════════════════════════════
// (was file_arc.rs)
// The net stack's file→Arc identity helpers (framework side; crate-flip
// rung 2): the raw adoption edge lives framework-side because the crate
// is forbid-enforced. The type claim is the caller's ops-identity check.

/// Adopt the File's private-data Arc (INV-FS-FILEPRIV pattern: the
/// caller checked the ops-table identity first — the type proof).
pub fn file_arc<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
    // SAFETY: the caller's ops identity check proves the type; the Arc
    // was leaked by into_raw at creation and the File holds a ref.
    unsafe {
        let p = (*file.private_data.get())?;
        if p.is_null() { return None; }
        let arc = alloc::sync::Arc::from_raw(p as *const T);
        // clone then forget to keep the File's reference alive
        let cloned = alloc::sync::Arc::clone(&arc);
        core::mem::forget(arc);
        Some(cloned)
    }
}

/// Take the Arc (consuming the File's reference — the close-path twin).
pub fn take_file_arc<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
    // SAFETY: same identity contract; the take empties the cell (the
    // close-path's exactly-once reclaim).
    unsafe {
        let p = (*file.private_data.get()).take()?;
        Some(alloc::sync::Arc::from_raw(p as *const T))
    }
}

// ═══════════════ buffer ═════════════════════════════════════════
// (was buffer.rs)
// Network Buffer (SkBuff)

// (INV-SKB-1: the sk_buff family — heads are allocator-owned blocks
// with headroom-tracked data pointers (alloc/reserve/put maintain the
// span invariants); data_slice_at rows are bounds-checked against the
// len tail; Send/Sync carried by the head's single-owner discipline.)

// (INV-SKB-1: the sk_buff family — heads are allocator-owned blocks
// with headroom-tracked data pointers (alloc/reserve/put maintain the
// span invariants); data_slice_at rows are bounds-checked against the
// len tail; Send/Sync carried by the head's single-owner discipline.)


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

unsafe impl Send for SkBuff {}
unsafe impl Sync for SkBuff {}

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

        // SAFETY: layout has non-zero size and valid alignment (16), checked above.
        let head = unsafe { alloc::alloc::alloc(layout) };
        if head.is_null() {
            return None;
        }

        // SAFETY: head is non-null and headroom <= alloc_size, so the offset stays in-bounds.
        let data = unsafe { head.add(headroom) };
        let tail = data;
        // SAFETY: head is non-null and alloc_size equals the allocation size.
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
        // SAFETY: self.head was allocated by alloc() with alignment 16 and size computed
        // from (end - head). Layout mismatch falls back to error path.
        unsafe {
            if let Ok(layout) = alloc::alloc::Layout::from_size_align(size, 16) {
                alloc::alloc::dealloc(self.head, layout);
            } else {
                crate::pr_err!("skb free: invalid layout size={}", size);
            }
        }
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
        // SAFETY: new_tail was bounds-checked against self.end above.
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
        let new_data = (self.data as usize).checked_sub(len as usize)?;
        if new_data < self.head as usize {
            return None;
        }

        // SAFETY: new_data was bounds-checked against self.head above.
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

        // SAFETY: self.data is within head..end and len <= self.len, so offset stays in-bounds.
        let new_data = unsafe { self.data.add(len as usize) };
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

        // SAFETY: tail + len was bounds-checked against self.end above.
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

        // SAFETY: skb_put reserved exactly len bytes at ptr; data.len() == len.
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

    /// Borrow `n` bytes of the data area as a shared slice (clamped to
    /// the buffer length). Packet buffers are single-owner by
    /// construction (`alloc_skb` hands out exclusive buffers), so the
    /// borrow cannot alias another live reference.
    pub fn data_slice(&self, n: u32) -> &[u8] {
        let n = n.min(self.len);
        // SAFETY: data..data+n is inside the allocation (len is the
        // allocated length); exclusive ownership per the type doc.
        unsafe { core::slice::from_raw_parts(self.data, n as usize) }
    }

    /// Borrow `n` bytes of the data area starting at `off` as a shared
    /// slice (clamped to the buffer end).
    pub fn data_slice_at(&self, off: u32, n: u32) -> &[u8] {
        if off >= self.len {
            return &[];
        }
        let n = n.min(self.len - off);
        // SAFETY: same exclusive-ownership contract as data_slice; the
        // range was clamped to the buffer end.
        unsafe { core::slice::from_raw_parts(unsafe { self.data.add(off as usize) }, n as usize) }
    }

    /// Typed read view of the header at the data-area start: `T` must
    /// fit within `len` (returns None otherwise — a corrupt/truncated
    /// frame yields no view instead of an out-of-bounds read).
    pub fn hdr<T>(&self) -> Option<&T> {
        if (core::mem::size_of::<T>() as u32) > self.len {
            return None;
        }
        // SAFETY: T fits (checked above); the buffer is exclusively
        // owned; T is a plain wire header (Copy, no padding contracts
        // beyond repr(C)).
        Some(unsafe { &*(self.data as *const T) })
    }

    /// Typed write view of the header at the data-area start: `T` must
    /// fit within `len` (None otherwise).
    pub fn hdr_mut<T>(&mut self) -> Option<&mut T> {
        if (core::mem::size_of::<T>() as u32) > self.len {
            return None;
        }
        // SAFETY: T fits (checked above); exclusive ownership; the
        // caller serializes against readers (packet build paths are
        // single-threaded per buffer).
        Some(unsafe { &mut *(self.data as *mut T) })
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
        // SAFETY: exclusive ownership; the header reference covers
        // [0, hlen) and the tail slice [hlen, len) — disjoint by
        // construction.
        Some(unsafe {
            let p = self.data;
            let hdr = &mut *(p as *mut T);
            let tail = core::slice::from_raw_parts(
                p.add(hlen as usize),
                (self.len - hlen) as usize,
            );
            (hdr, tail)
        })
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
        // SAFETY: skb_push returned headroom of at least size_of::<T>()
        // bytes inside this exclusively-owned buffer.
        let h: &mut T = unsafe { &mut *ptr.cast::<T>() };
        build(h);
        Some(())
    }

    /// Append `size_of::<T>()` bytes at the tail and hand a typed
    /// mutable view (the header-APPEND edge — protocols that build
    /// payload-first, header-last, e.g. ARP). INH-BUF-4 as push_hdr.
    pub fn put_hdr<T: Copy>(&mut self, build: impl FnOnce(&mut T)) -> Option<()> {
        let ptr = self.skb_put(core::mem::size_of::<T>() as u32)?;
        // SAFETY: skb_put returned tail space of at least size_of::<T>()
        // bytes inside this exclusively-owned buffer.
        let h: &mut T = unsafe { &mut *ptr.cast::<T>() };
        build(h);
        Some(())
    }

    /// Push raw bytes as new headroom (the byte-level twin of
    /// push_hdr_val — INH-BUF-4).
    pub fn push_bytes(&mut self, src: &[u8]) -> Option<()> {
        let ptr = self.skb_push(src.len() as u32)?;
        // SAFETY: skb_push returned headroom of src.len() bytes inside
        // this exclusively-owned buffer.
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), ptr, src.len()) };
        Some(())
    }

    /// Push a ready-made header VALUE as new headroom (the build edge
    /// for callers that compute the header on the stack — INH-BUF-4).
    pub fn push_hdr_val<T: Copy>(&mut self, hdr: &T) -> Option<()> {
        let ptr = self.skb_push(core::mem::size_of::<T>() as u32)?;
        // SAFETY: skb_push returned headroom of at least size_of::<T>()
        // bytes inside this exclusively-owned buffer.
        unsafe {
            core::ptr::copy_nonoverlapping(
                hdr as *const T as *const u8,
                ptr,
                core::mem::size_of::<T>(),
            );
        }
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

        // SAFETY: offset and copy_len are bounds-checked; src and buf are valid pointers.
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
    // SAFETY: length checked above; T is a plain wire header; the
    // reference borrows from `data` with its own lifetime.
    Some(unsafe { &*(data.as_ptr() as *const T) })
}

/// The mutable twin of [`hdr_view`] (the build side over a caller-owned
/// byte region).
pub fn hdr_view_mut<T: Copy>(data: &mut [u8]) -> Option<&mut T> {
    if data.len() < core::mem::size_of::<T>() {
        return None;
    }
    // SAFETY: as hdr_view, over the exclusive borrow.
    Some(unsafe { &mut *(data.as_mut_ptr() as *mut T) })
}
