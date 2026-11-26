//! Framework-side net UNSAFE EDGES (crate-flip rung 2): the raw-memory
//! substrate the service files must not carry now that net is
//! forbid-enforced — static-mut table reference formation, the
//! Arc-refcount forensics probe, the validated socket deref, and the
//! POD header views. (The file→Arc adoption family lives in
//! net/file_arc.rs and skb slicing in net/buffer.rs — both already
//! framework-side.)
//!
//! Families (one INV row each, registered in the safety ledger):
//! - INV-NET-SOCKTAB-1 — `static_table_ref` / `static_table_mut`
//! - INV-NET-ARCREF-1 — `arc_refcount_probe` / `audited_deref`
//! - INV-NET-HDRVIEW-1 — `pod_view` / `pod_view_mut`: alignment is
//!   CHECKED here (the old inline casts never checked it); length is
//!   the caller's packet-validation contract.

/// INV-NET-SOCKTAB-1: `p` comes from `addr_of_mut!` on one of the
/// stack's `static mut` socket/route tables; the caller holds the
/// documented serialization (the per-table lock or the softirq/syscall
/// discipline the site already carries), so no two live `&mut` aliases
/// of the same table exist concurrently. The reference lives as long
/// as the table: `'static`.
pub fn static_table_ref<T>(p: *mut T) -> &'static T {
    // SAFETY: INV-NET-SOCKTAB-1 above (caller-serialized global table).
    unsafe { &*p }
}

/// The mutable twin of `static_table_ref` (same INV row: the caller's
/// serialization is what makes a second alias impossible).
pub fn static_table_mut<T>(p: *mut T) -> &'static mut T {
    // SAFETY: INV-NET-SOCKTAB-1 above (caller-serialized global table).
    unsafe { &mut *p }
}

/// INV-NET-ARCREF-1a: diagnostic refcount reads for the Arc allocation
/// whose DATA pointer is `data_ptr` (Arc layout `[strong: u64, weak:
/// u64, data...]`; `Arc::into_raw` hands out `&data`, so the counts
/// live at `data_ptr - 16` / `data_ptr - 8`). Volatile reads,
/// forensics only — the values are heuristics, never control of
/// memory safety.
pub fn arc_refcount_probe(data_ptr: usize) -> (u64, u64) {
    // SAFETY: INV-NET-ARCREF-1a above (read-only volatile u64 pair in
    // the same allocation as the validated data pointer).
    unsafe {
        let strong = core::ptr::read_volatile((data_ptr - 16) as *const u64);
        let weak = core::ptr::read_volatile((data_ptr - 8) as *const u64);
        (strong, weak)
    }
}

/// INV-NET-ARCREF-1b: `p` names a live Arc-backed allocation whose
/// strong reference outlives the borrow (the File's private_data cell
/// or the socket table's slot holds it) — the caller validated the
/// provenance (heap-range + ops-identity) before calling.
pub fn audited_deref<T>(p: *const T) -> &'static T {
    // SAFETY: INV-NET-ARCREF-1b above (validated live Arc-backed ptr).
    unsafe { &*p }
}

/// INV-NET-CONTAINEROF-1: `member_addr` names a field of a live
/// `T` (the wait-queue registries store FIELD pointers); `offset` is
/// the caller's `offset_of!` for that same field, so
/// `member_addr - offset` addresses the whole object. Diagnostic-only
/// sites (trace-armed prints).
pub fn container_of<T>(member_addr: usize, offset: usize) -> &'static T {
    // SAFETY: INV-NET-CONTAINEROF-1 above (field ptr + matching offset).
    unsafe { &*((member_addr - offset) as *const T) }
}

/// INV-NET-HDRVIEW-1 (the read arm): interpret `data` as a POD header
/// `T`. Length AND alignment are checked — a short or misaligned
/// buffer returns `None` (the packet is dropped; the old inline casts
/// checked only length, so an unaligned skb buffer was silent UB).
pub fn pod_view<'a, T>(data: &'a [u8]) -> Option<&'a T> {
    if data.len() < core::mem::size_of::<T>() {
        return None;
    }
    let p = data.as_ptr();
    if (p as usize) & (core::mem::align_of::<T>() - 1) != 0 {
        return None;
    }
    // SAFETY: INV-NET-HDRVIEW-1 — length and alignment checked above;
    // T is a POD wire header (no interior references).
    Some(unsafe { &*(p as *const T) })
}

/// INV-NET-HDRVIEW-1 (the write arm): the TX push twin. `p` covers at
/// least `size_of::<T>()` writable bytes (the `skb_push` contract the
/// caller checked via its `Ok` return) and is alignment-checked here;
/// a misaligned push pointer returns `None` (packet dropped, never
/// UB).
pub fn pod_view_mut<T>(p: *mut u8) -> Option<&'static mut T> {
    if (p as usize) & (core::mem::align_of::<T>() - 1) != 0 {
        return None;
    }
    // SAFETY: INV-NET-HDRVIEW-1 — alignment checked above, length is
    // the skb_push contract; T is a POD wire header.
    Some(unsafe { &mut *(p as *mut T) })
}

// ==========================================================================
use super::types::{PacketType, SkBuff};
use crate::fs::types::File;
// pass 3 (seed-core §2c.5): net's collected unsafe — the inline
// blocks of the subsystem's mechanism files, one contract each.
// ==========================================================================

// ------------ net-family header-view edges (INV-SKB-1) ------------

/// View the head of `data` as a shared `&T` wire header — the ONE
/// audited parse edge shared by the protocol modules (replaces the
/// per-protocol `&*(data.as_ptr() as *const Hdr)` duplicates).
///
/// INH-BUF-3: `T: Copy` (a repr(C) wire header) and `data` has at
/// least `size_of::<T>()` bytes (checked; None otherwise). The borrow
/// is tied to `data` — no invented lifetimes.
/// INH-BUF-3: length checked (None otherwise); T is a plain repr(C) wire header; the borrow ties to data — the ONE audited parse edge shared by the protocol modules.
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
/// INH-BUF-3: length checked (None otherwise); the exclusive borrow backs the write view (hdr_view's mutable twin).
pub fn hdr_view_mut<T: Copy>(data: &mut [u8]) -> Option<&mut T> {
    if data.len() < core::mem::size_of::<T>() {
        return None;
    }
    // SAFETY: as hdr_view, over the exclusive borrow.
    Some(unsafe { &mut *(data.as_mut_ptr() as *mut T) })
}

// ------------ SkBuff lifecycle, pointer arithmetic and header views (INV-SKB-1) ------------

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
    /// INV-SKB-1: the head block is allocator-owned at alignment 16; the headroom/end offsets stay inside the allocation by construction (headroom + data + tailroom = alloc_size). Carries the ord-1/ord-2 offset adds of the same body.
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
}
impl SkBuff {
    /// Release the backing allocation if still held.
    /// INV-SKB-1: dealloc of the still-held head with the size recomputed from end - head; the corrupted-size guard routes to the error path instead of the allocator. Widened pub(crate): the Drop impl and free() in hotpath.rs call it across the module line.
    pub(crate) fn release(&mut self) {
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
}
impl SkBuff {
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
    /// INV-SKB-1: the tail advance is bounds-checked against end (checked_add + compare) before the add commits.
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
}
impl SkBuff {
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
    /// INV-SKB-1: the data retreat is bounds-checked against head (checked_sub + compare) before the sub commits.
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
}
impl SkBuff {
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
    /// INV-SKB-1: the data advance is len-checked, then tail-checked, before the add commits (head <= data <= tail <= end kept).
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
}
impl SkBuff {
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
    /// INV-SKB-1: the tail advance is bounds-checked against end before the add commits.
    pub fn skb_reserve(&mut self, len: u32) -> Option<*mut u8> {
        if self.tail as usize + len as usize > self.end as usize {
            return None;
        }

        // SAFETY: tail + len was bounds-checked against self.end above.
        self.tail = unsafe { self.tail.add(len as usize) };
        self.data = self.tail;
        Some(self.data)
    }
}
impl SkBuff {
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
    /// INV-SKB-1: skb_put reserved exactly len bytes at ptr; the copy writes only that span.
    pub fn skb_put_data(&mut self, data: &[u8]) -> Result<(), ()> {
        let len = data.len() as u32;
        let ptr = self.skb_put(len).ok_or(())?;

        // SAFETY: skb_put reserved exactly len bytes at ptr; data.len() == len.
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        }

        Ok(())
    }
}
impl SkBuff {
    /// Borrow `n` bytes of the data area as a shared slice (clamped to
    /// the buffer length). Packet buffers are single-owner by
    /// construction (`alloc_skb` hands out exclusive buffers), so the
    /// borrow cannot alias another live reference.
    /// INV-SKB-1: the slice is clamped to len, inside the exclusively-owned allocation.
    pub fn data_slice(&self, n: u32) -> &[u8] {
        let n = n.min(self.len);
        // SAFETY: data..data+n is inside the allocation (len is the
        // allocated length); exclusive ownership per the type doc.
        unsafe { core::slice::from_raw_parts(self.data, n as usize) }
    }
}
impl SkBuff {
    /// Borrow `n` bytes of the data area starting at `off` as a shared
    /// slice (clamped to the buffer end).
    /// INV-SKB-1: off/n clamped to the buffer end; the offset add and the slice construction stay inside the exclusively-owned allocation. Carries the nested ord-1 offset add of the same block.
    pub fn data_slice_at(&self, off: u32, n: u32) -> &[u8] {
        if off >= self.len {
            return &[];
        }
        let n = n.min(self.len - off);
        // SAFETY: same exclusive-ownership contract as data_slice; the
        // range was clamped to the buffer end.
        unsafe { core::slice::from_raw_parts(unsafe { self.data.add(off as usize) }, n as usize) }
    }
}
impl SkBuff {
    /// Typed read view of the header at the data-area start: `T` must
    /// fit within `len` (returns None otherwise — a corrupt/truncated
    /// frame yields no view instead of an out-of-bounds read).
    /// INV-SKB-1: T fits within len (checked, None otherwise); the view covers the start of the exclusively-owned data area.
    pub fn hdr<T>(&self) -> Option<&T> {
        if (core::mem::size_of::<T>() as u32) > self.len {
            return None;
        }
        // SAFETY: T fits (checked above); the buffer is exclusively
        // owned; T is a plain wire header (Copy, no padding contracts
        // beyond repr(C)).
        Some(unsafe { &*(self.data as *const T) })
    }
}
impl SkBuff {
    /// Typed write view of the header at the data-area start: `T` must
    /// fit within `len` (None otherwise).
    /// INV-SKB-1: T fits within len (checked); exclusive ownership, the caller serializes against readers (packet build paths are single-threaded per buffer).
    pub fn hdr_mut<T>(&mut self) -> Option<&mut T> {
        if (core::mem::size_of::<T>() as u32) > self.len {
            return None;
        }
        // SAFETY: T fits (checked above); exclusive ownership; the
        // caller serializes against readers (packet build paths are
        // single-threaded per buffer).
        Some(unsafe { &mut *(self.data as *mut T) })
    }
}
impl SkBuff {
    /// Typed write view of the header plus the byte tail behind it, in
    /// one borrow (the header checksum-over-segment shape): returns
    /// None when `T` does not fit. The two borrows are disjoint ranges
    /// of the exclusively-owned buffer.
    /// INV-SKB-1: T fits within len (checked); the header reference covers [0, hlen) and the tail slice [hlen, len) — disjoint ranges of the exclusively-owned buffer.
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
}
impl SkBuff {
    /// Push `size_of::<T>()` bytes of headroom and hand the caller a
    /// typed mutable view of them (the header-BUILD edge — round-5
    /// consolidation: every protocol's `&mut *(ptr as *mut Hdr)` push
    /// site routes through here).
    ///
    /// INH-BUF-4: `T` is a repr(C) wire header; the view is exactly the
    /// pushed bytes; the buffer stays exclusively owned.
    /// INH-BUF-4: skb_push returned headroom of at least size_of::<T>() bytes inside this exclusively-owned buffer; the view covers exactly the pushed bytes.
    pub fn push_hdr<T: Copy>(&mut self, build: impl FnOnce(&mut T)) -> Option<()> {
        let ptr = self.skb_push(core::mem::size_of::<T>() as u32)?;
        // SAFETY: skb_push returned headroom of at least size_of::<T>()
        // bytes inside this exclusively-owned buffer.
        let h: &mut T = unsafe { &mut *ptr.cast::<T>() };
        build(h);
        Some(())
    }
}
impl SkBuff {
    /// Append `size_of::<T>()` bytes at the tail and hand a typed
    /// mutable view (the header-APPEND edge — protocols that build
    /// payload-first, header-last, e.g. ARP). INH-BUF-4 as push_hdr.
    /// INH-BUF-4: skb_put returned tail space of at least size_of::<T>() bytes inside this exclusively-owned buffer; the view covers exactly the appended bytes.
    pub fn put_hdr<T: Copy>(&mut self, build: impl FnOnce(&mut T)) -> Option<()> {
        let ptr = self.skb_put(core::mem::size_of::<T>() as u32)?;
        // SAFETY: skb_put returned tail space of at least size_of::<T>()
        // bytes inside this exclusively-owned buffer.
        let h: &mut T = unsafe { &mut *ptr.cast::<T>() };
        build(h);
        Some(())
    }
}
impl SkBuff {
    /// Push raw bytes as new headroom (the byte-level twin of
    /// push_hdr_val — INH-BUF-4).
    /// INH-BUF-4: skb_push returned headroom of src.len() bytes; the copy writes only that span.
    pub fn push_bytes(&mut self, src: &[u8]) -> Option<()> {
        let ptr = self.skb_push(src.len() as u32)?;
        // SAFETY: skb_push returned headroom of src.len() bytes inside
        // this exclusively-owned buffer.
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), ptr, src.len()) };
        Some(())
    }
}
impl SkBuff {
    /// Push a ready-made header VALUE as new headroom (the build edge
    /// for callers that compute the header on the stack — INH-BUF-4).
    /// INH-BUF-4: skb_push returned headroom of at least size_of::<T>() bytes; the copy writes exactly the header value.
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
}
impl SkBuff {
    /// Copy SkBuff data
    ///
    /// # Arguments
    /// - `buf`: Destination buffer
    /// - `offset`: Offset
    /// - `len`: Copy length
    ///
    /// # Returns
    /// Actual number of bytes copied
    /// INV-SKB-1: offset and copy_len are bounds-checked against len; the copy reads inside the allocation and writes the caller's buf.
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

// ------------ SkBuff Send/Sync proofs (INV-SKB-1) ------------

/// INV-SKB-1 Send: the head block is allocator-owned and single-owner by construction (alloc_skb hands out exclusive buffers); Send is the single-owner proof.
unsafe impl Send for SkBuff {}
/// INV-SKB-1 Sync: the same single-owner discipline — a live packet buffer is never shared between contexts; Sync is that proof.
unsafe impl Sync for SkBuff {}

// ------------ socket-file Arc edges (INV-NETARC-1) ------------

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
/// INV-NETARC-1: the caller's ops-identity check proves the Arc type; the increment balances the from_raw below, the File's close() is the only drop path.
pub(crate) fn mod_file_arc<T>(file: &crate::fs::types::File) -> Option<alloc::sync::Arc<T>> {
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
/// INV-NETARC-1: reconstructing consumes the slot's single reference; the emptied cell (clear_private_data before the from_raw) makes this exactly-once.
pub(crate) fn reclaim_file_arc<T>(file: &crate::fs::types::File) -> Option<alloc::sync::Arc<T>> {
    let ptr = file.private_data_ptr()? as *const T;
    file.clear_private_data();
    // SAFETY: INV-NETARC-1 — reconstructing consumes the slot's single
    // reference; the emptied cell makes this exactly-once.
    unsafe { Some(alloc::sync::Arc::from_raw(ptr)) }
}
/// Take the Arc OUT of a closing socket File's private_data slot and
/// drop it (the close path — INV-NETARC-1 + the caller's ops identity
/// check; the slot is cleared under the file's own close serialization).
/// INV-NETARC-1: reconstructing the stored Arc consumes the slot's single reference; close is the last user of the file.
pub(crate) fn mod_take_file_arc<T>(file: &crate::fs::types::File) -> Option<()> {
    let ptr = file.private_data_ptr()? as *const T;
    // SAFETY: INV-NETARC-1 — reconstructing the stored Arc consumes the
    // slot's single reference (close is the last user of the file).
    unsafe {
        drop(alloc::sync::Arc::from_raw(ptr));
    }
    file.clear_private_data();
    Some(())
}
/// Adopt the File's private-data Arc (INV-FS-FILEPRIV pattern: the
/// caller checked the ops-table identity first — the type proof).
/// INV-NETARC-1: the caller's ops-identity check proves the type; the clone-then-forget keeps the File's own reference alive (INV-FS-FILEPRIV pattern).
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
/// INV-NETARC-1: same identity contract as file_arc; the take empties the cell (the close-path's exactly-once reclaim).
pub fn take_file_arc<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
    // SAFETY: same identity contract; the take empties the cell (the
    // close-path's exactly-once reclaim).
    unsafe {
        let p = (*file.private_data.get()).take()?;
        Some(alloc::sync::Arc::from_raw(p as *const T))
    }
}
