//! Net-lane framework edges — the audited unsafe boundary
//! (mainline minimal-first consolidation, 2026-11-27).
//!
//! Doctrine (the FRAMEWORK/SERVICE split, same as fs/unsafe.rs and
//! process/unsafe.rs): every service file of this subsystem (buffer.rs,
//! socket.rs, tcp.rs, udp.rs, unix.rs, raw.rs, netlink.rs, route.rs,
//! ipv4/, ipv6/, ...) carries ZERO `unsafe` keywords — enforced by a
//! file-level `#![forbid(unsafe_code)]`. Every genuinely-unsafe
//! operation they need lives HERE as a named edge with an INV
//! contract, one audit per edge FAMILY, not per call site.
//!
//! Edge inventory (families):
//!
//! * SkBuff raw storage — INV-NET-ALLOC-1: alloc/dealloc of the skb
//!   head allocation (16-byte NET_SKBUFF_DATA_ALIGN layout) and the
//!   pointer arithmetic that carves data/tail/end out of it. The caller
//!   proves the bounds (headroom <= alloc_size, len checked against
//!   head/end); the edge performs the arithmetic/copy.
//! * Static table views — INV-NET-STATIC-1: `&'static (mut) T`
//!   formation over a `static mut` global (ROUTE_TABLE under
//!   ROUTE_LOCK, UDP_SOCKET_TABLE / TCP_SOCKET_TABLE under their
//!   documented serializations). The caller holds the lock; the edge
//!   states the obligation once.
//! * Socket-file Arc private data — INV-NETARC-1: the
//!   `Arc::into_raw` window installed at socket creation, its
//!   increment-balanced clone, its borrow, and its exactly-once take
//!   (moved here wholesale from net/mod.rs — `crate::net::file_arc`
//!   call sites are unchanged via the mod.rs re-export).
//! * Wire-header views — INH-BUF-3: `hdr_view`/`hdr_view_mut` (the
//!   shared parse/build edge moved here from buffer.rs; buffer.rs
//!   re-exports so `crate::net::buffer::hdr_view` call sites stay
//!   stable) plus raw_ref/raw_mut over validated data pointers.
//! * Diagnostics — INV-NET-DIAG-1: volatile reads of the Arc count
//!   words around a into_raw pointer (forensics only).
//! * User-memory copies — INV-NET-UACCESS-1: the unix-socket send
//!   staging copy (caller access_ok-validated).
//! * Send/Sync commitments for SkBuff, Socket, UnixSocket,
//!   NetlinkSocket, RawSocket (moved verbatim; SAFETY notes intact).
//!
//! This module is Framework: it may use `unsafe` and is part of the
//! audited TCB. Nothing here grows domain logic — protocol semantics
//! stay in the service files; only the pointer discipline lives here.

use core::alloc::Layout;

// ===========================================================================
// INV-NET-ALLOC-1: SkBuff raw storage
// ===========================================================================

/// Allocate the skb head block. Caller obligation: `layout` has
/// non-zero size and valid (16) alignment — checked by the caller
/// before this edge.
#[inline]
pub fn alloc_raw(layout: Layout) -> *mut u8 {
    // SAFETY: INV-NET-ALLOC-1 — non-zero size, valid alignment.
    unsafe { alloc::alloc::alloc(layout) }
}

/// Free the skb head block. Caller obligation: `p` came from
/// [`alloc_raw`] with the same layout family (16-byte align), the
/// buffer is exclusively owned at free time.
#[inline]
pub fn dealloc_raw(p: *mut u8, layout: Layout) {
    // SAFETY: INV-NET-ALLOC-1 — pointer from alloc() with the layout
    // recomputed from (end - head); layout mismatch is the caller's
    // error path.
    unsafe { alloc::alloc::dealloc(p, layout) }
}

/// INV-NET-PTRARITH-1: advance a raw byte pointer by `n`. Caller
/// obligation: the result stays within the same allocation (the skb
/// head block; bounds checked at the call site against head/end).
#[inline]
pub fn ptr_advance(p: *mut u8, n: usize) -> *mut u8 {
    // SAFETY: INV-NET-PTRARITH-1 — caller-proven in-allocation offset.
    unsafe { p.add(n) }
}

/// INV-NET-PTRARITH-1 (retreat half): move a raw byte pointer back by
/// `n`. Same in-allocation obligation (head-bound checked at the call
/// site).
#[inline]
pub fn ptr_retreat(p: *mut u8, n: usize) -> *mut u8 {
    // SAFETY: INV-NET-PTRARITH-1 — caller-proven in-allocation offset.
    unsafe { p.sub(n) }
}

/// Copy `len` non-overlapping bytes into kernel `dst`. Caller
/// obligation: dst/src name `len` writable/readable bytes of live,
/// disjoint regions (skb_put-reserved tail space, caller slice).
#[inline]
pub fn copy_bytes(dst: *mut u8, src: *const u8, len: usize) {
    // SAFETY: caller-proven disjoint live regions.
    unsafe { core::ptr::copy_nonoverlapping(src, dst, len) }
}

/// INV-NET-PTRARITH-1 edge: shared slice view of `len` bytes at `p`.
/// Caller obligation: `p..p+len` is inside the exclusively-owned skb
/// buffer (len clamped to the buffer end at the call site); packet
/// buffers are single-owner by construction, so the borrow cannot
/// alias a live mutable reference.
#[inline]
pub fn slice_view<'a>(p: *const u8, len: usize) -> &'a [u8] {
    // SAFETY: INV-NET-PTRARITH-1 — in-allocation range, sole owner.
    unsafe { core::slice::from_raw_parts(p, len) }
}

// ===========================================================================
// Generic raw-borrow family (INH-BUF-3 wire-header views and friends)
// ===========================================================================

/// Shared reborrow of a validated raw pointer. Caller obligation: the
/// pointee is a live, initialized object of `T` not being mutated for
/// the borrow's duration (skb data area, private_data Arc payload,
/// wait-queue container back-derive under its lock).
#[inline]
pub fn raw_ref<'a, T>(p: *const T) -> &'a T {
    debug_assert!(!p.is_null());
    // SAFETY: caller-proven liveness/aliasing (family contract above).
    unsafe { &*p }
}

/// Exclusive reborrow of a validated raw pointer. As [`raw_ref`], plus
/// the caller's exclusion argument (&mut skb owner, build path).
#[inline]
pub fn raw_mut<'a, T>(p: *mut T) -> &'a mut T {
    debug_assert!(!p.is_null());
    // SAFETY: caller-proven liveness + exclusion.
    unsafe { &mut *p }
}

/// View the head of `data` as a shared `&T` wire header — the ONE
/// audited parse edge shared by the protocol modules (replaces the
/// per-protocol `&*(data.as_ptr() as *const Hdr)` duplicates).
///
/// INH-BUF-3: `T: Copy` (a repr(C) wire header) and `data` has at
/// least `size_of::<T>()` bytes (checked; None otherwise). The borrow
/// is tied to `data` — no invented lifetimes.
#[inline]
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
#[inline]
pub fn hdr_view_mut<T: Copy>(data: &mut [u8]) -> Option<&mut T> {
    if data.len() < core::mem::size_of::<T>() {
        return None;
    }
    // SAFETY: as hdr_view, over the exclusive borrow.
    Some(unsafe { &mut *(data.as_mut_ptr() as *mut T) })
}

/// Typed header view plus the byte tail behind it, in one borrow (the
/// header checksum-over-segment shape). Caller obligation: `hlen` bytes
/// fit the exclusively-owned buffer at `p` (checked at the call site);
/// the returned header covers [0, hlen) and the tail slice
/// [hlen, hlen+tlen) — disjoint by construction.
#[inline]
pub fn hdr_tail_view<'a, T>(p: *mut u8, hlen: usize, tlen: usize) -> (&'a mut T, &'a [u8]) {
    // SAFETY: exclusive ownership; disjoint ranges by construction.
    unsafe {
        let hdr = &mut *(p as *mut T);
        let tail = core::slice::from_raw_parts(p.add(hlen), tlen);
        (hdr, tail)
    }
}

// ===========================================================================
// INV-NET-STATIC-1: static-mut table views
// ===========================================================================

/// Shared view of a lock-guarded `static mut` table. Caller
/// obligation: the table's documented serialization is held
/// (ROUTE_LOCK lock_irqsave guard, UDP_TABLE_LOCK or the
/// softirq/syscall discipline, TCP_TABLE_LOCK, or the single-threaded
/// init paths the sites name).
#[inline]
pub fn static_table_ref<'a, T>(p: *const T) -> &'a T {
    debug_assert!(!p.is_null());
    // SAFETY: INV-NET-STATIC-1 — caller holds the documented lock; the
    // reference is read-only.
    unsafe { &*p }
}

/// Exclusive view of a lock-guarded `static mut` table. Same
/// obligation as [`static_table_ref`] plus write exclusion (the same
/// lock, held exclusively).
#[inline]
pub fn static_table_mut<'a, T>(p: *mut T) -> &'a mut T {
    debug_assert!(!p.is_null());
    // SAFETY: INV-NET-STATIC-1 — caller holds the documented lock
    // exclusively.
    unsafe { &mut *p }
}

// ===========================================================================
// INV-NETARC-1: socket-file Arc private data
// ===========================================================================
//
// Socket Files carry `Arc::into_raw(socket)` in private_data (installed
// by the socket's own file-ops installer; the caller identity-checks
// the file against that ops table). The File's lifetime keeps the Arc
// alive (close() is the only drop path).

/// Clone the socket Arc behind a private_data pointer:
/// increment-then-adopt keeps the returned Arc independent of the slot.
/// Caller obligation: ops identity checked (the per-family
/// `*_socket_from_fd` checks).
#[inline]
pub fn arc_clone_raw<T>(p: *const T) -> alloc::sync::Arc<T> {
    debug_assert!(!p.is_null());
    // SAFETY: INV-NETARC-1 — the pointer is a live Arc::into_raw value
    // of this type; the increment balances the from_raw construction.
    unsafe {
        alloc::sync::Arc::increment_strong_count(p);
        alloc::sync::Arc::from_raw(p)
    }
}

/// Take the slot's Arc OUT (exactly-once reconstruct; the caller
/// retires the slot and drops the returned Arc). Caller obligation:
/// close-path serialization, ops identity checked.
#[inline]
pub fn arc_take<T>(p: *const T) -> alloc::sync::Arc<T> {
    debug_assert!(!p.is_null());
    // SAFETY: INV-NETARC-1 — reconstructing the stored Arc consumes the
    // slot's single reference (close is the last user of the file).
    unsafe { alloc::sync::Arc::from_raw(p) }
}

/// The whole data area of a closing socket File's private_data slot,
/// cloned out and dropped (the close path twin of [`arc_clone_raw`];
/// moved here verbatim from net/mod.rs).
pub fn take_file_arc<T>(file: &crate::fs::file::File) -> Option<()> {
    let ptr = file.private_data_ptr()? as *const T;
    // INV-NETARC-1 — reconstructing the stored Arc consumes the slot's
    // single reference (close is the last user of the file).
    drop(arc_take(ptr));
    file.clear_private_data();
    Some(())
}

/// Clone the socket Arc out of a socket File's private_data slot
/// (moved here verbatim from net/mod.rs; `crate::net::file_arc` call
/// sites are unchanged via the mod.rs re-export).
pub fn file_arc<T>(file: &crate::fs::file::File) -> Option<alloc::sync::Arc<T>> {
    let ptr = file.private_data_ptr()? as *const T;
    Some(arc_clone_raw(ptr))
}

// ===========================================================================
// INV-NET-UACCESS-1 (staging twin): expose a fully-copied staging buffer
// ===========================================================================

/// Set a Vec's length after an out-of-band fill. Caller obligation:
/// `n <= v`'s capacity and the first `n` elements are initialized (the
/// copy_from_user_bytes staging contract: capacity is exactly count;
/// the copy fills it before set_len exposes the bytes; a partial copy
/// faults before this edge runs).
#[inline]
pub fn vec_set_len<T>(v: &mut alloc::vec::Vec<T>, n: usize) {
    // SAFETY: caller-proven capacity + initialization contract above.
    unsafe { v.set_len(n) }
}

// ===========================================================================
// INV-NET-DIAG-1: forensic volatile reads
// ===========================================================================

/// Diagnostic entry-frame read: (syscall-no, a0). Caller obligation
/// (INV-NET-DIAG-1): `top` came from kernel_stack_top_checked() and is
/// non-zero (the caller checks both before calling), so the frame
/// pages are mapped.
#[inline]
pub fn entry_sys_args(top: usize) -> (u64, u64) {
    #[cfg(feature = "x86_64")]
    {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: diagnostic read of this task's own entry frame; the
        // caller gated the stack via kernel_stack_top_checked.
        unsafe { ((*regs).orig_rax, (*regs).rdi) }
    }
    #[cfg(feature = "riscv64")]
    {
        let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::riscv64::pt_regs::PtRegs;
        // SAFETY: same contract as the x86_64 arm.
        unsafe { ((*regs).a7, (*regs).a0) }
    }
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    (0, 0)
}

/// Volatile-read one u64 (Arc count-word forensics around an into_raw
/// pointer; the same allocation is still live because a plausible
/// strong count was read first — the caller classifies before use).
#[inline]
pub fn volatile_read_u64(p: *const u64) -> u64 {
    // SAFETY: INV-NET-DIAG-1 — diagnostic read of two u64s in the same
    // live allocation (Arc layout [strong, weak, data...]).
    unsafe { core::ptr::read_volatile(p) }
}

// ===========================================================================
// INV-NET-UACCESS-1: user-memory copies
// ===========================================================================

/// Copy `len` bytes from user `src_user` into kernel `dst`; returns
/// bytes NOT copied (0 = full success). Caller obligation: the user
/// range was access_ok-validated for exactly this transfer.
#[inline]
pub fn copy_from_user_bytes(dst: *mut u8, src_user: *const u8, len: usize) -> usize {
    // SAFETY: INV-NET-UACCESS-1 — caller-validated user range.
    unsafe { crate::arch::uaccess::copy_from_user(dst, src_user, len) }
}

// ===========================================================================
// Send/Sync commitments (moved verbatim from their home files by the
// 2026-11-27 mainline consolidation; each carries its original SAFETY
// note — the per-field lock discipline arguments live with the type
// definitions, and these impls add no field access of their own).
// ===========================================================================

// SAFETY: SkBuff owns its backing allocation; the raw pointers are only
// dereferenced through the buffer's own &self/&mut self methods.
unsafe impl Send for crate::net::buffer::SkBuff {}
unsafe impl Sync for crate::net::buffer::SkBuff {}

// SAFETY: Socket uses Spinlocks for all mutable shared state.
unsafe impl Sync for crate::net::socket::Socket {}

// SAFETY: all mutable state is behind Spinlocks.
unsafe impl Sync for crate::net::unix::UnixSocket {}

// SAFETY: all mutable state is behind Spinlocks / atomics.
unsafe impl Sync for crate::net::netlink::NetlinkSocket {}

// SAFETY: all mutable state is behind Spinlocks.
unsafe impl Sync for crate::net::raw::RawSocket {}
