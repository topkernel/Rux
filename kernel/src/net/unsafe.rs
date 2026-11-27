//! The net subsystem's ONE audited unsafe file (mainline minimal-first
//! consolidation, 2026-11-27): every raw edge the skb core, the Arc
//! private-data slots, the protocol tables and the diagnostics still
//! need lives here behind one audited item per family; every other net
//! file is safe code over them.
//!
//! Edge families:
//! - the SkBuff exclusive-ownership raw core (INV-NET-SKBUFF-1) and its
//!   allocation edges (INV-NET-SKBUFF-ALLOC-1)
//! - the socket Arc private-data slots (INV-NETARC-1, previously
//!   documented on net/mod.rs's file_arc/take_file_arc)
//! - the protocol tables' static-mut storage (INV-NET-ROUTETAB-1 /
//!   INV-NET-UDPTAB-1 / INV-NET-TCPTAB-1)
//! - the socket forensics reads (INV-NET-SOCKDIAG-1)

use crate::net::buffer::SkBuff;

// ------------------------------------------------------------------
// Send/Sync commitments (moved verbatim with their SAFETY notes)
// ------------------------------------------------------------------

// SAFETY: SkBuff packet buffers are single-owner by construction
// (`alloc_skb` hands out exclusive buffers); its raw pointers point
// into one heap allocation only the owner touches.
unsafe impl Send for SkBuff {}
unsafe impl Sync for SkBuff {}

// SAFETY: all mutable state is behind Spinlocks / atomics.
unsafe impl Sync for crate::net::netlink::NetlinkSocket {}

// SAFETY: all mutable state is behind Spinlocks.
unsafe impl Sync for crate::net::raw::RawSocket {}

// SAFETY: all mutable state is behind Spinlocks / atomics.
unsafe impl Sync for crate::net::socket::Socket {}

// SAFETY: all mutable state is behind Spinlocks.
unsafe impl Sync for crate::net::unix::UnixSocket {}

// ------------------------------------------------------------------
// SkBuff raw core (INV-NET-SKBUFF-1): every accessor's CALLER has
// bounds-checked the offset against head..end above the unsafe line;
// buffers are single-owner by construction, so no aliasing borrow
// exists. The primitives below are the only places raw arithmetic
// happens.
// ------------------------------------------------------------------

/// Allocate a region with a checked layout (INV-NET-SKBUFF-ALLOC-1:
/// the caller built a non-zero-size, 16-aligned layout).
pub(crate) fn alloc_layout(layout: alloc::alloc::Layout) -> *mut u8 {
    // SAFETY: INV-NET-SKBUFF-ALLOC-1 above.
    unsafe { alloc::alloc::alloc(layout) }
}

/// Free a region (INV-NET-SKBUFF-ALLOC-1: head came from alloc_layout
/// with this exact size; a bad layout is a kernel bug — report, leak,
/// never free blind).
pub(crate) fn dealloc_checked(head: *mut u8, size: usize) {
    // SAFETY: INV-NET-SKBUFF-ALLOC-1 — head/size from the matching alloc.
    unsafe {
        if let Ok(layout) = alloc::alloc::Layout::from_size_align(size, 16) {
            alloc::alloc::dealloc(head, layout);
        } else {
            crate::pr_err!("skb free: invalid layout size={}", size);
        }
    }
}

/// In-bounds forward offset (caller checked head <= p+n <= end).
pub(crate) fn ptr_add(p: *mut u8, n: usize) -> *mut u8 {
    // SAFETY: INV-NET-SKBUFF-1 — caller bounds-checked.
    unsafe { p.add(n) }
}

/// In-bounds backward offset (caller checked head <= p-n).
pub(crate) fn ptr_sub(p: *mut u8, n: usize) -> *mut u8 {
    // SAFETY: INV-NET-SKBUFF-1 — caller bounds-checked.
    unsafe { p.sub(n) }
}

/// Shared byte view of `[p, p+n)` (caller clamped n to the buffer).
pub(crate) fn slice_parts<'a>(p: *const u8, n: usize) -> &'a [u8] {
    // SAFETY: INV-NET-SKBUFF-1 — caller-clamped range of the exclusive
    // allocation; the 'static shape matches the packet-lifetime
    // convention the header views document.
    unsafe { core::slice::from_raw_parts(p, n) }
}

/// Shared typed view of a wire header (caller checked size_of::<T>
/// against the buffer length).
pub(crate) fn typed_ref<'a, T>(p: *const T) -> &'a T {
    // SAFETY: INV-NET-SKBUFF-1 — size checked; plain repr(C) wire
    // header; packet-lifetime borrow per the hdr() contract.
    unsafe { &*p }
}

/// Exclusive typed view (caller checked size and serializes writers).
pub(crate) fn typed_mut<'a, T>(p: *mut T) -> &'a mut T {
    // SAFETY: INV-NET-SKBUFF-1 — size checked; exclusive ownership;
    // single-threaded build paths per the hdr_mut() contract.
    unsafe { &mut *p }
}

/// Header view + the byte tail behind it in one borrow (disjoint
/// ranges of the exclusive buffer).
pub(crate) fn hdr_tail_view<'a, T>(data: *mut u8, hlen: usize, tail_len: usize) -> (&'a mut T, &'a [u8]) {
    // SAFETY: INV-NET-SKBUFF-1 — exclusive ownership; [0,hlen) and
    // [hlen, hlen+tail_len) are disjoint by construction.
    unsafe {
        let hdr = &mut *(data as *mut T);
        let tail = core::slice::from_raw_parts(data.add(hlen), tail_len);
        (hdr, tail)
    }
}

/// Byte copy between validated kernel ranges.
pub(crate) fn copy_bytes(src: *const u8, dst: *mut u8, n: usize) {
    // SAFETY: INV-NET-SKBUFF-1 — both ranges validated by the caller
    // (skb_put/push reservation, bounds-checked offsets).
    unsafe { core::ptr::copy_nonoverlapping(src, dst, n) };
}

// ------------------------------------------------------------------
// Socket Arc private-data slots (INV-NETARC-1)
// ------------------------------------------------------------------

/// Reconstruct an owned `Arc<T>` from its into_raw pointer (the socket
/// close path; INV-NETARC-1 — the slot's single reference, the caller
/// cleared the cell first).
pub(crate) fn arc_increment_strong<T>(ptr: *const T) {
    // SAFETY: INV-NETARC-1 — the pointer is a live Arc::into_raw value
    // of this type; the increment balances the from_raw construction
    // the caller performs right after.
    unsafe { alloc::sync::Arc::increment_strong_count(ptr) }
}

pub(crate) fn arc_from_raw<T>(ptr: *const T) -> alloc::sync::Arc<T> {
    // SAFETY: INV-NETARC-1 above (into_raw round-trip, single owner).
    unsafe { alloc::sync::Arc::from_raw(ptr) }
}

/// Shared view of the Arc-stashed socket behind a validated pointer
/// (the r-p0 socket_borrow contract: the file's strong reference keeps
/// it alive for the borrow).
pub(crate) fn socket_ref(ptr: *const crate::net::socket::Socket) -> &'static crate::net::socket::Socket {
    // SAFETY: INV-NETARC-1 — validated live Arc<Socket>, strong ref
    // held by private_data for the file's lifetime.
    unsafe { &*(ptr as *const crate::net::socket::Socket) }
}

/// Container_of edge for the wait-queue diagnostics: recover the
/// owning UnixSocket from its embedded wait_queue pointer (INV-NET-
/// SOCKDIAG-1 — the queue pointer always names a live socket's field
/// in the wake path that reports).
pub(crate) fn unix_sock_from_waitq(
    queue: &crate::process::wait::WaitQueueHead,
) -> &'static crate::net::unix::UnixSocket {
    // SAFETY: INV-NET-SOCKDIAG-1 — container_of over an embedded field
    // of a live socket (offset_of is the compile-time layout fact).
    unsafe {
        let addr = (queue as *const _ as usize)
            - core::mem::offset_of!(crate::net::unix::UnixSocket, wait_queue);
        &*(addr as *const crate::net::unix::UnixSocket)
    }
}

/// Diagnostic read of the task's own syscall entry frame (INV-NET-
/// SOCKDIAG-1: the frame lies directly below the task's checked kernel
/// stack top; the read is diagnostic-only).
pub(crate) fn task_syscall_frame(top: usize) -> (u64, u64) {
    #[cfg(feature = "x86_64")]
    {
        // SAFETY: INV-NET-SOCKDIAG-1 — diagnostic read of this task's
        // own entry frame.
        unsafe {
            let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
                as *const crate::arch::x86_64::pt_regs::PtRegs;
            ((*regs).orig_rax, (*regs).rdi)
        }
    }
    #[cfg(feature = "riscv64")]
    {
        // SAFETY: INV-NET-SOCKDIAG-1 — same contract, riscv64.
        unsafe {
            let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
                as *const crate::arch::riscv64::pt_regs::PtRegs;
            ((*regs).a7, (*regs).a0)
        }
    }
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    {
        (0u64, 0u64)
    }
}

/// Vec set_len after a validated copy (INV-NET-SKBUFF-ALLOC-1
/// lineage: capacity is exactly `n`; the copy filled it before this
/// exposes the bytes).
pub(crate) fn vec_set_len(buf: &mut alloc::vec::Vec<u8>, n: usize) {
    // SAFETY: capacity >= n (allocated above); the copy filled n bytes.
    unsafe { buf.set_len(n) }
}

/// Forensic read of the Arc head counts at `raw - 16 / - 8` (the
/// SOCK-UNCLASS-PTR adjudication; diagnostic only, same allocation).
pub(crate) fn arc_counts(raw: usize) -> (u64, u64) {
    // SAFETY: INV-NET-SOCKDIAG-1 — diagnostic reads of two u64s in the
    // same allocation the validated pointer names.
    unsafe {
        (
            core::ptr::read_volatile((raw - 16) as *const u64),
            core::ptr::read_volatile((raw - 8) as *const u64),
        )
    }
}

// ------------------------------------------------------------------
// Protocol tables (static-mut under their documented serialization)
// ------------------------------------------------------------------

/// INV-NET-ROUTETAB-1: ROUTE_TABLE is accessed only under ROUTE_LOCK
/// (the caller's guard) — the edges below run inside that contract.
pub(crate) fn route_table_lookup(dst: u32) -> Option<crate::net::ipv4::route::RouteEntry> {
    // SAFETY: INV-NET-ROUTETAB-1 — lock held by the caller.
    unsafe { crate::net::ipv4::route::ROUTE_TABLE.lookup(dst) }
}

pub(crate) fn route_table_add(route: crate::net::ipv4::route::RouteEntry) -> Result<(), ()> {
    // SAFETY: INV-NET-ROUTETAB-1 — lock held by the caller.
    unsafe { crate::net::ipv4::route::ROUTE_TABLE.add(route) }
}

pub(crate) fn route_table_remove(dst: u32, mask: u32) -> bool {
    // SAFETY: INV-NET-ROUTETAB-1 — lock held by the caller.
    unsafe { crate::net::ipv4::route::ROUTE_TABLE.remove(dst, mask) }
}

pub(crate) fn route_table_clear() {
    // SAFETY: INV-NET-ROUTETAB-1 — lock held by the caller.
    unsafe { crate::net::ipv4::route::ROUTE_TABLE.clear() }
}

pub(crate) fn route_table_entries() -> alloc::vec::Vec<crate::net::ipv4::route::RouteEntry> {
    // SAFETY: INV-NET-ROUTETAB-1 — lock held by the caller.
    unsafe {
        crate::net::ipv4::route::ROUTE_TABLE.entries[..crate::net::ipv4::route::ROUTE_TABLE.count]
            .iter()
            .flatten()
            .cloned()
            .collect()
    }
}

/// INV-NET-UDPTAB-1: UDP_SOCKET_TABLE accessors run under the caller's
/// documented serialization (UDP_TABLE_LOCK or the softirq/syscall
/// discipline the scan sites document).
pub(crate) fn udp_table() -> &'static mut crate::net::udp::UdpSocketTable {
    // SAFETY: INV-NET-UDPTAB-1 — caller-serialized; pure reference
    // formation, no re-entrancy.
    unsafe { &mut crate::net::udp::UDP_SOCKET_TABLE }
}

pub(crate) fn udp_table_shared() -> &'static crate::net::udp::UdpSocketTable {
    // SAFETY: INV-NET-UDPTAB-1 — read-only view, caller-serialized.
    unsafe { &crate::net::udp::UDP_SOCKET_TABLE }
}

/// INV-NET-TCPTAB-1: TCP_SOCKET_TABLE mutations run under
/// TCP_TABLE_LOCK (taken by the callers); the reference formation here
/// is the same documented contract.
pub(crate) fn tcp_table() -> &'static mut crate::net::tcp::TcpSocketTable {
    // SAFETY: INV-NET-TCPTAB-1 — caller-serialized.
    unsafe { &mut crate::net::tcp::TCP_SOCKET_TABLE }
}

pub(crate) fn tcp_table_shared() -> &'static crate::net::tcp::TcpSocketTable {
    // SAFETY: INV-NET-TCPTAB-1 — read-only view, caller-serialized.
    unsafe { &crate::net::tcp::TCP_SOCKET_TABLE }
}
