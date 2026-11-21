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
//! Kernel layer policy (docs/architecture/kernel-layers.md):
//! this module is Framework (arch/) — it holds the unsafe mechanics so
//! service files (`syscall/**` first) can carry
//! `#![forbid(unsafe_code)]` while still doing user I/O.

// (INV-UACCESS-SAFE-1: the capsule's own edges — each safe wrapper
// holds exactly one audited call into the per-arch exception-table
// primitive (INV-UACCESS-1); the UserPtr/UserSlice cell rows carry
// the address/len pair discipline.)
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

/// Zero-fill `n` bytes at the user address `dst_user`.
///
/// Returns the number of bytes NOT cleared (0 = complete success).
#[inline]
pub fn clear_user(dst_user: usize, n: usize) -> usize {
    // SAFETY: the primitive access_ok-checks and faults through the
    // exception table; any dst_user value is sound to pass.
    unsafe { uaccess::clear_user(dst_user as *mut u8, n) }
}

/// Length of the NUL-terminated user string at `src_user` (length
/// including the NUL, matching the primitive), or 0 on failure /
/// overlong string.
#[inline]
pub fn strnlen_user(src_user: usize, maxlen: usize) -> usize {
    // SAFETY: the primitive access_ok-checks and faults through the
    // exception table; any src_user value is sound to pass.
    unsafe { uaccess::strnlen_user(src_user as *const u8, maxlen) }
}

/// Atomic compare-exchange on a user u32 word (PI futex protocol).
/// Returns the observed old value, or `None` on a rejected/faulting
/// user address.
#[inline]
pub fn futex_cmpxchg_u32(uaddr: usize, expected: u32, new: u32) -> Option<u32> {
    // SAFETY: the primitive access_ok-checks the 4-byte user word and
    // reports faults as None; any uaddr value is sound to pass.
    unsafe { uaccess::futex_cmpxchg_u32_user(uaddr as *mut u32, expected, new) }
}

/// Copy one `T` in from user memory at `src_user` into `dst`.
///
/// Returns the number of bytes NOT copied (0 = complete success). On
/// failure the uncopied tail of `dst` is zero-filled by the primitive.
///
/// Contract (same as the arch `copy_from_user` the call sites used to
/// wrap): `T: Copy` and `T` carries no bit-pattern validity constraint
/// beyond its bytes (plain integer/struct UAPI types — not `bool`/
/// `char`/enums with niches); `dst` must point at an initialized `T`.
#[inline]
pub fn copy_in_t<T: Copy>(dst: &mut T, src_user: usize) -> usize {
    // SAFETY: the kernel side is a live `&mut T` (caller-initialized);
    // the user side is access_ok-checked and faults through the
    // exception table, so any src_user value is sound to pass.
    unsafe {
        uaccess::copy_from_user(
            dst as *mut T as *mut u8,
            src_user as *const u8,
            core::mem::size_of::<T>(),
        )
    }
}

/// Copy one `T` out from kernel memory to user address `dst_user`.
///
/// Returns the number of bytes NOT copied (0 = complete success).
#[inline]
pub fn copy_out_t<T: Copy>(dst_user: usize, src: &T) -> usize {
    // SAFETY: the kernel side is a shared `&T` (size_of::<T>() readable
    // bytes by the type system); the user side fault-safes inside.
    unsafe {
        uaccess::copy_to_user(
            dst_user as *mut u8,
            src as *const T as *const u8,
            core::mem::size_of::<T>(),
        )
    }
}

/// Typed handle to one `Copy` value living in user memory.
///
/// Constructing this is infallible and validates nothing; every
/// accessor routes through the exception-table primitives, so a bad
/// address reports failure instead of faulting the kernel. The handle
/// is `!Send`/`!Sync` (it aliases whatever user page happens to be
/// mapped at `addr` at use time — it must not outlive the syscall on
/// another CPU).
///
/// This is the typed-user-pointer primitive of the unsafe-slim
/// campaign: service code holds `UserPtr<T>` instead of `*const T` and
/// needs no `unsafe` at the use site.
pub struct UserPtr<T: Copy> {
    addr: usize,
    _marker: core::marker::PhantomData<*mut T>,
}

impl<T: Copy> UserPtr<T> {
    /// Handle to the `T` at user virtual address `addr`.
    #[inline]
    pub fn new(addr: usize) -> Self {
        UserPtr {
            addr,
            _marker: core::marker::PhantomData,
        }
    }

    /// The user virtual address this handle points at.
    #[inline]
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// access_ok pre-check for `size_of::<T>()` at this address. An
    /// Ok from here does not pin the page (it can be unmapped before
    /// the actual copy); the copy accessor is still fault-safe.
    #[inline]
    pub fn is_valid(&self) -> bool {
        uaccess::access_ok(self.addr, core::mem::size_of::<T>())
    }

    /// Read the value; `None` on a rejected range or user page fault.
    #[inline]
    pub fn read(&self) -> Option<T> {
        // SAFETY: T is Copy and the primitive validates/fault-safes
        // the user address itself.
        unsafe { uaccess::get_user(self.addr as *const T) }
    }

    /// Write `value`; false on a rejected range or user page fault.
    #[inline]
    pub fn write(&self, value: T) -> bool {
        // SAFETY: T is Copy and the primitive validates/fault-safes
        // the user address itself.
        unsafe { uaccess::put_user(self.addr as *mut T, value) }
    }

    /// Byte offset handle (e.g. a field inside a user struct header).
    #[inline]
    pub fn add(&self, offset: usize) -> Self {
        UserPtr {
            addr: self.addr.wrapping_add(offset),
            _marker: core::marker::PhantomData,
        }
    }
}

/// Typed handle to a byte range in user memory.
///
/// Same contract as [`UserPtr`]: construction validates nothing, every
/// accessor is fault-safe through the exception table, handle is
/// `!Send`/`!Sync`.
pub struct UserSlice {
    addr: usize,
    len: usize,
    _marker: core::marker::PhantomData<*mut u8>,
}

impl UserSlice {
    /// Handle to `len` bytes at user virtual address `addr`.
    #[inline]
    pub fn new(addr: usize, len: usize) -> Self {
        UserSlice {
            addr,
            len,
            _marker: core::marker::PhantomData,
        }
    }

    /// Handle to the user bytes backing `buf`'s address range — for
    /// iovec-style flattening where a kernel walk produced the range.
    #[inline]
    pub fn from_parts(addr: usize, len: usize) -> Self {
        Self::new(addr, len)
    }

    /// The user virtual address of the first byte.
    #[inline]
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// The length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// access_ok pre-check for the whole range (non-pinning, same
    /// caveat as `UserPtr::is_valid`).
    #[inline]
    pub fn is_valid(&self) -> bool {
        uaccess::access_ok(self.addr, self.len)
    }

    /// Copy the range into `dst` (kernel memory). Returns the number
    /// of bytes NOT copied (0 = complete). On failure the uncopied
    /// tail of `dst` is zero-filled by the primitive.
    #[inline]
    pub fn copy_in(&self, dst: &mut [u8]) -> usize {
        // SAFETY: dst is a mutable slice (kernel side proven by the
        // type system); the user side fault-safes inside.
        unsafe {
            uaccess::copy_from_user(dst.as_mut_ptr(), self.addr as *const u8, dst.len())
        }
    }

    /// Copy `src` (kernel memory) out to the range. Returns the number
    /// of bytes NOT copied (0 = complete).
    #[inline]
    pub fn copy_out(&self, src: &[u8]) -> usize {
        // SAFETY: src is a shared slice (kernel side proven by the
        // type system); the user side fault-safes inside.
        unsafe { uaccess::copy_to_user(self.addr as *mut u8, src.as_ptr(), src.len()) }
    }

    /// Zero-fill the range. Returns the number of bytes NOT cleared.
    #[inline]
    pub fn clear(&self) -> usize {
        // SAFETY: fault-safe primitive; any address is sound to pass.
        unsafe { uaccess::clear_user(self.addr as *mut u8, self.len) }
    }

    /// Read one `Copy` value at byte `offset` inside the range.
    /// `None` when `offset` leaves no room for `T` or the access
    /// fails.
    #[inline]
    pub fn read_at<T: Copy>(&self, offset: usize) -> Option<T> {
        if offset.checked_add(core::mem::size_of::<T>())? > self.len {
            return None;
        }
        // SAFETY: T is Copy; bounds-checked against the handle's own
        // length; the user side fault-safes inside the primitive.
        unsafe { uaccess::get_user(self.addr.wrapping_add(offset) as *const T) }
    }

    /// Write one `Copy` value at byte `offset` inside the range;
    /// false when `offset` leaves no room for `T` or the access fails.
    #[inline]
    pub fn write_at<T: Copy>(&self, offset: usize, value: T) -> bool {
        match offset.checked_add(core::mem::size_of::<T>()) {
            Some(end) if end <= self.len => {}
            _ => return false,
        }
        // SAFETY: T is Copy; bounds-checked against the handle's own
        // length; the user side fault-safes inside the primitive.
        unsafe { uaccess::put_user(self.addr.wrapping_add(offset) as *mut T, value) }
    }
}
