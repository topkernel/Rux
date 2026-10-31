//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Safe typed user-memory entries — the syscall capsule boundary's view
//! of uaccess.
//!
//! The per-backend `arch::uaccess` primitives are `unsafe fn`s over raw
//! pointers because the kernel-side pointer validity is a caller
//! obligation. This module re-exports that mechanics behind slice- and
//! address-typed SAFE functions: the kernel side is proven by the type
//! system (`&[u8]` / `&mut [u8]`), the user side is just a `usize`
//! address that may fault — every entry routes through access_ok plus
//! the exception-table copies, so ANY address is sound to pass and a bad
//! one simply reports uncopied bytes.
//!
//! Framekernel layer policy (docs/architecture/framekernel-layers.md):
//! this module is Framework (arch/) — it holds the unsafe mechanics so
//! service files (`syscall/**` first) can carry
//! `#![forbid(unsafe_code)]` while still doing user I/O.

use super::uaccess;

/// Copy `src` (kernel memory) out to the user address `dst_user`.
///
/// Returns the number of bytes NOT copied (0 = complete success). A
/// rejected or faulting user range returns a non-zero count; the kernel
/// source is only ever read.
#[inline]
pub fn copy_out(dst_user: usize, src: &[u8]) -> usize {
    // SAFETY: `src` is a shared slice, so src.len() readable bytes at
    // src.as_ptr() are guaranteed by the type system. The user side is
    // access_ok-checked and faults through the exception table inside
    // the primitive; any dst_user value is sound to pass.
    unsafe { uaccess::copy_to_user(dst_user as *mut u8, src.as_ptr(), src.len()) }
}

/// Copy bytes in from the user address `src_user` into `dst`.
///
/// Returns the number of bytes NOT copied (0 = complete success). On
/// failure the uncopied tail of `dst` is zero-filled by the primitive
/// (no stale kernel bytes survive a short copy).
#[inline]
pub fn copy_in(dst: &mut [u8], src_user: usize) -> usize {
    // SAFETY: `dst` is a mutable slice, so dst.len() writable bytes are
    // guaranteed by the type system. The user side is access_ok-checked
    // and faults through the exception table inside the primitive; any
    // src_user value is sound to pass.
    unsafe { uaccess::copy_from_user(dst.as_mut_ptr(), src_user as *const u8, dst.len()) }
}

/// Read one `Copy` value from user memory at `src_user`.
///
/// Returns `None` when the access fails (rejected range or fault); the
/// primitive access_ok-checks and faults through the exception table, so
/// any address is sound to pass.
#[inline]
pub fn read_user<T: Copy>(src_user: usize) -> Option<T> {
    // SAFETY: T is Copy (no drop glue for a partially-read value) and
    // the primitive validates/fault-safes the user address itself.
    unsafe { uaccess::get_user(src_user as *const T) }
}

/// Write one `Copy` value to user memory at `dst_user`.
///
/// Returns false when the access fails (rejected range or fault).
#[inline]
pub fn write_user<T: Copy>(dst_user: usize, value: T) -> bool {
    // SAFETY: T is Copy and the primitive validates/fault-safes the
    // user address itself.
    unsafe { uaccess::put_user(dst_user as *mut T, value) }
}
