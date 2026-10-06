//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 user-memory access.
//!
//! Phase 1 (this file): direct-access copies — the kernel's higher-half
//! mappings coexist with user space and SMAP/SMEP are not enabled, so a
//! plain Rust loop reaches user memory. Bounds are checked with
//! `access_ok` first. An unmapped/unfaultable user page still takes a
//! kernel #PF: X86-TODO(agent x86-trap) replaces the copy cores with
//! exception-table-bracketed asm (`__ex_table` windows) so faults land
//! in `exception::fixup_exception` and return short counts / None,
//! exactly like the riscv64 twin.

use super::mm::memory_layout::user_addr::USER_END;

pub const MAX_RW_COUNT: usize = 0x7FFF_F000;

/// Is the [addr, addr+size) window inside user VA space?
pub fn access_ok(addr: usize, size: usize) -> bool {
    addr.checked_add(size).map_or(false, |end| end <= USER_END)
}

/// Copy kernel → user. Returns the number of bytes NOT copied.
///
/// # Safety
/// `from` must be readable for `n` bytes; `to` is validated by access_ok.
pub unsafe fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if !access_ok(to as usize, n) {
        return n;
    }
    // SAFETY: caller guarantees `from`; `to` was just bounds-checked.
    unsafe {
        core::ptr::copy_nonoverlapping(from, to, n);
    }
    0
}

/// Copy user → kernel. Returns the number of bytes NOT copied.
///
/// # Safety
/// `to` must be writable for `n` bytes; `from` is validated by access_ok.
pub unsafe fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if !access_ok(from as usize, n) {
        return n;
    }
    // SAFETY: caller guarantees `to`; `from` was just bounds-checked.
    unsafe {
        core::ptr::copy_nonoverlapping(from, to, n);
    }
    0
}

/// Zero user memory. Returns the number of bytes NOT cleared.
///
/// # Safety
/// `to` is validated by access_ok only in this phase.
pub unsafe fn clear_user(to: *mut u8, n: usize) -> usize {
    if !access_ok(to as usize, n) {
        return n;
    }
    // SAFETY: bounds-checked above.
    unsafe {
        core::ptr::write_bytes(to, 0, n);
    }
    0
}

/// Read one value from user memory.
///
/// # Safety
/// `from` is validated by access_ok only in this phase.
pub unsafe fn get_user<T: Copy>(from: *const T) -> Option<T> {
    if !access_ok(from as usize, core::mem::size_of::<T>()) {
        return None;
    }
    // SAFETY: bounds-checked above.
    unsafe { Some(core::ptr::read_volatile(from)) }
}

/// Write one value to user memory.
///
/// # Safety
/// `to` is validated by access_ok only in this phase.
pub unsafe fn put_user<T: Copy>(to: *mut T, value: T) -> bool {
    if !access_ok(to as usize, core::mem::size_of::<T>()) {
        return false;
    }
    // SAFETY: bounds-checked above.
    unsafe {
        core::ptr::write_volatile(to, value);
    }
    true
}

/// Copy a NUL-terminated string from user memory into `buf`.
pub fn strncpy_from_user<'a>(from: *const u8, max_len: usize, buf: &'a mut [u8]) -> Result<&'a [u8], i64> {
    if from as usize == 0 || !access_ok(from as usize, 1) {
        return Err(-14); // EFAULT
    }
    let limit = max_len.min(buf.len());
    for i in 0..limit {
        // SAFETY: single byte inside the checked window.
        let b = unsafe { core::ptr::read_volatile(from.add(i)) };
        buf[i] = b;
        if b == 0 {
            return Ok(&buf[..i]);
        }
    }
    Err(-36) // ENAMETOOLONG
}

/// Length of a NUL-terminated user string, 0 on fault/too long.
pub unsafe fn strnlen_user(str: *const u8, maxlen: usize) -> usize {
    if str as usize == 0 || !access_ok(str as usize, 1) {
        return 0;
    }
    for i in 0..maxlen {
        // SAFETY: index within maxlen of a checked pointer.
        if unsafe { core::ptr::read_volatile(str.add(i)) } == 0 {
            return i;
        }
    }
    0
}
