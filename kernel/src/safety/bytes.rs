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
