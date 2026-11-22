//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Byte-view primitives for `Copy` kernel structs — the
//! `slice::from_raw_parts` family's primitives (unsafe-slim round 4).
//!
//! Before this module, every struct-to-byte reinterpretation in the
//! kernel hand-rolled one `unsafe { core::slice::from_raw_parts(p as
//! *const u8, n) }` (87 sites at the round-4 census: ext4 on-disk
//! structs, journal tags, dfx record headers, command-line blobs) plus
//! one `unsafe { core::str::from_utf8_unchecked }` per reader. Each
//! site re-proved the same three things: the pointer covers exactly
//! `size_of::<T>()` bytes, the buffer is alive, and the length is
//! right. This module compresses that to ONE audited edge per shape;
//! every call site becomes plain safe code.
//!
//! Invariants:
//!
//! * **INV-CBYTES-1 (byte image)** — a `T: Copy` value's byte image is
//!   `size_of::<T>()` bytes starting at the reference. Reads through
//!   the returned byte slice may observe padding bytes: for on-disk /
//!   wire-format structs (`repr(C)` images matched to an external
//!   format) that is the format's own definition; for pure-Rust types
//!   the caller must not give padding meaning. This is the same
//!   argument the hand-rolled sites made — here it is stated once.
//! * **INV-CBYTES-2 (bounds/alignment)** — the sub-object accessors
//!   (`from_bytes`, `read_at`, `write_at`) check length bounds and
//!   perform UNALIGNED accesses, so a packed disk image needs no
//!   alignment proof from the caller.
//! * **INV-CBYTES-3 (liveness)** — every accessor takes a reference
//!   with the access lifetime; no pointer escapes. `bytes_of_mut`
//!   requires `&mut T`, so writes cannot alias.

use core::mem::size_of;

/// View a shared `T: Copy` as its byte image (INV-CBYTES-1).
#[inline]
pub fn bytes_of<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: INV-CBYTES-1 — the reference is live, sized, and its
    // byte image is exactly size_of::<T>() bytes; the view re-labels
    // the same memory for reads.
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, size_of::<T>()) }
}

/// View an exclusive `T: Copy` as its mutable byte image
/// (INV-CBYTES-1/3).
#[inline]
pub fn bytes_of_mut<T: Copy>(v: &mut T) -> &mut [u8] {
    // SAFETY: INV-CBYTES-1/3 — same re-labeling as bytes_of, made
    // exclusive by &mut T.
    unsafe { core::slice::from_raw_parts_mut(v as *mut T as *mut u8, size_of::<T>()) }
}

/// Rebuild a `T: Copy` from the head of a byte slice (INV-CBYTES-2).
/// `None` when the slice is shorter than `size_of::<T>()`.
#[inline]
pub fn from_bytes<T: Copy>(bytes: &[u8]) -> Option<T> {
    if bytes.len() < size_of::<T>() {
        return None;
    }
    // SAFETY: INV-CBYTES-2 — the slice proves the bytes are live for
    // the call; read_unaligned because a byte image carries no
    // alignment promise.
    Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const T) })
}

/// Read a `T: Copy` at byte offset `off` in a buffer, unaligned
/// (INV-CBYTES-2). `None` when the read would run past the buffer.
#[inline]
pub fn read_at<T: Copy>(buf: &[u8], off: usize) -> Option<T> {
    if buf.len() < off.saturating_add(size_of::<T>()) {
        return None;
    }
    // SAFETY: INV-CBYTES-2 — bounds checked above, unaligned read.
    Some(unsafe { core::ptr::read_unaligned(buf.as_ptr().add(off) as *const T) })
}

/// Write a `T: Copy` at byte offset `off` in a buffer, unaligned
/// (INV-CBYTES-2/3). `None` when the write would run past the buffer.
#[inline]
pub fn write_at<T: Copy>(buf: &mut [u8], off: usize, val: T) -> Option<()> {
    if buf.len() < off.saturating_add(size_of::<T>()) {
        return None;
    }
    // SAFETY: INV-CBYTES-2/3 — bounds checked above; the buffer is
    // exclusively borrowed; unaligned write.
    unsafe { core::ptr::write_unaligned(buf.as_mut_ptr().add(off) as *mut T, val) };
    Some(())
}

/// View a whole `[T: Copy]` slice as its byte image (INV-CBYTES-1 slice
/// arm: a slice of `T` is `len()` elements at `size_of::<T>()` stride,
/// so its byte image is exactly `len() * size_of::<T>()` contiguous
/// bytes starting at the slice — the same argument the hand-rolled
/// `from_raw_parts(v.as_ptr() as *const u8, n * size_of::<T>())` sites
/// made per site; uaccess batch copies over `[PollFd]`/`[T]` buffers).
#[inline]
pub fn bytes_of_slice<T: Copy>(v: &[T]) -> &[u8] {
    let n = v.len().checked_mul(size_of::<T>()).expect("slice byte length overflow");
    // SAFETY: INV-CBYTES-1 — the slice is live for the borrow and its
    // byte image is exactly n bytes; the view re-labels the same memory.
    unsafe { core::slice::from_raw_parts(v.as_ptr() as *const u8, n) }
}

/// View a whole `[T: Copy]` slice as its mutable byte image
/// (INV-CBYTES-1/3 slice arm; the exclusive twin of `bytes_of_slice`).
#[inline]
pub fn bytes_of_slice_mut<T: Copy>(v: &mut [T]) -> &mut [u8] {
    let n = v.len().checked_mul(size_of::<T>()).expect("slice byte length overflow");
    // SAFETY: INV-CBYTES-1/3 — same re-labeling as bytes_of_slice, made
    // exclusive by &mut [T].
    unsafe { core::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut u8, n) }
}

/// Byte view of a PINNED page-cache frame span.
///
/// INV-CBYTES-4 (pinned-frame window): `*frame` is a kernel direct-map
/// alias of a whole `PAGE_SIZE` page-cache frame that the caller holds a
/// pin on, and `[off, off + len)` lies inside that one page. The pin is
/// taken by `page_cache::get()` / `get_range_into()` /
/// `get_range_with_entry()` (each calls `pin_locked()` on the page and
/// transfers the pin to the caller) and released through the matching
/// `put()` / `put_range*()` on EVERY path including error exits. The pin keeps the frame allocated and the
/// cache keeps its bytes stable while held, so the view is a plain
/// re-labeling of live, readable memory. The returned slice borrows the
/// caller's own handle to the pointer (the `bio::bh_data(&bh)` shape), so
/// it cannot outlive the caller's pin-holding scope. This is the
/// byte-level twin of the GUP pinned-alias copies (`mm::edges::copy_kva*`).
#[inline]
pub fn pinned_frame_bytes(frame: &*const u8, off: usize, len: usize) -> &[u8] {
    debug_assert!(off.checked_add(len).is_some_and(|end| end <= crate::mm::PAGE_SIZE));
    // SAFETY: INV-CBYTES-4 — off+len is inside the pinned PAGE_SIZE
    // frame; the caller's pin holds the frame for this borrow.
    unsafe { core::slice::from_raw_parts((*frame).add(off), len) }
}

/// Mutable twin of [`pinned_frame_bytes`] (INV-CBYTES-4, exclusive
/// arm): the caller holds the pin AND is the writer of this span — the
/// page-cache write paths copy user/kernel bytes into the pinned frame
/// under the description's `io_lock`, so no other accessor aliases the
/// span while this view lives.
#[inline]
pub fn pinned_frame_bytes_mut(frame: &*mut u8, off: usize, len: usize) -> &mut [u8] {
    debug_assert!(off.checked_add(len).is_some_and(|end| end <= crate::mm::PAGE_SIZE));
    // SAFETY: INV-CBYTES-4 exclusive arm — inside the pinned frame; the
    // io_lock-holding writer is the only accessor of the span.
    unsafe { core::slice::from_raw_parts_mut((*frame).add(off), len) }
}

/// Volatile store of a `Copy` value through an exclusive field reference.
///
/// INV-CBYTES-5 (advisory volatile field store): `place` is a live
/// `&mut` to a plain-old-data field the caller owns for the store, so
/// validity, alignment and exclusivity are all carried by the reference
/// itself — the only thing `unsafe` adds is the VOLATILE qualifier.
/// Use it for advisory fields that concurrent lock-free READERS sample
/// (timestamps, sizes, counters): the volatile store keeps the write
/// from being folded away or reordered past the caller's publication
/// point, and readers are expected to tolerate a torn or stale value by
/// the field's own contract. This is the reference-taking twin of the
/// raw-pointer diagnostic stores in the process/sync edges.
#[inline]
pub fn store_volatile<T: Copy>(place: &mut T, value: T) {
    // SAFETY: INV-CBYTES-5 — a plain volatile write through a live
    // exclusive reference; T: Copy so there is no drop glue.
    unsafe { core::ptr::write_volatile(place, value) }
}
