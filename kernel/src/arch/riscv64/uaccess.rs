//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! User space access functions
//!
//! Provides safe kernel-to-user space data copy functionality.
//! Uses exception table mechanism to handle page faults during user space access.
//!
//! # Main Functions
//! - `copy_to_user`: Copy data from kernel to user space
//! - `copy_from_user`: Copy data from user space to kernel
//! - `clear_user`: Zero user space memory
//!
//! # Exception Table Mechanism
//! These functions use exception tables to safely handle invalid user addresses.
//! If access fails, the function returns the number of uncopied bytes (instead of crashing).
//!
//! # Implementation Details
//! Based on Linux kernel (arch/riscv/lib/uaccess.S):
//! - Uses SR_SUM bit to enable user memory access from kernel mode
//! - Word-aligned copy (8 bytes) for better performance
//! - Unrolled loop (8 words per iteration) for bulk copy
//! - Exception table for safe access handling

// Include optimized assembly implementation
core::arch::global_asm!(include_str!("uaccess.S"));

/// User space access error type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAccessError {
    /// Access successful
    Success,
    /// Invalid source address
    InvalidSource,
    /// Invalid destination address
    InvalidDestination,
    /// Address unaligned
    Unaligned,
    /// Unknown error
    Unknown,
}

/// Check if user space address is valid
///
/// # Arguments
/// - `addr`: User space address
/// - `size`: Access size
///
/// # Returns
/// Returns true if address is within user space range
#[inline]
pub fn access_ok(addr: usize, size: usize) -> bool {
    use super::mm::user_addr::{USER_START, USER_END};

    // Check address range
    if addr < USER_START {
        return false;
    }

    // Check overflow
    let end = match addr.checked_add(size) {
        Some(e) => e,
        None => return false,
    };

    end <= USER_END
}

// ============================================================================
// Assembly function declarations
// ============================================================================

/// Copy data from kernel to user space (assembly implementation)
///
/// # Arguments
/// - `to`: User space destination address
/// - `from`: Kernel source address
/// - `n`: Number of bytes to copy
///
/// # Returns
/// Returns number of uncopied bytes. 0 means complete success.
///
/// # Safety
/// - `from` must point to valid kernel memory
/// - `to` must be a valid user space address
extern "C" {
    fn __copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __clear_user(to: *mut u8, n: usize) -> usize;
}

/// Copy data from kernel to user space
///
/// # Arguments
/// - `to`: User space destination address
/// - `from`: Kernel source address
/// - `n`: Number of bytes to copy
///
/// # Returns
/// Returns number of uncopied bytes. 0 means complete success.
///
/// # Safety
/// - `from` must point to valid kernel memory
/// - `to` must be a valid user space address (if invalid, returns n)
///
/// # Performance
/// Uses word-aligned copy (8 bytes at a time) for better performance.
/// For large copies, uses unrolled loop (64 bytes per iteration).
pub unsafe fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    // Check if user space address is valid
    if !access_ok(to as usize, n) {
        return n;
    }

    // Call optimized assembly implementation
    __copy_to_user(to, from, n)
}

/// Copy data from user space to kernel
///
/// # Arguments
/// - `to`: Kernel destination address
/// - `from`: User space source address
/// - `n`: Number of bytes to copy
///
/// # Returns
/// Returns number of uncopied bytes. 0 means complete success.
///
/// # Safety
/// - `to` must point to valid kernel memory
/// - `from` must be a valid user space address (if invalid, returns n)
///
/// # Performance
/// Uses word-aligned copy (8 bytes at a time) for better performance.
/// For large copies, uses unrolled loop (64 bytes per iteration).
pub unsafe fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    // Check if user space address is valid
    if !access_ok(from as usize, n) {
        return n;
    }

    // Call optimized assembly implementation
    __copy_from_user(to, from, n)
}

/// Zero user space memory
///
/// # Arguments
/// - `to`: User space start address
/// - `n`: Number of bytes to zero
///
/// # Returns
/// Returns number of unzeroed bytes
///
/// # Safety
/// `to` must be a valid user space address
///
/// # Performance
/// Uses word-aligned store (8 bytes at a time) for better performance.
pub unsafe fn clear_user(to: *mut u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    if !access_ok(to as usize, n) {
        return n;
    }

    // Call optimized assembly implementation
    __clear_user(to, n)
}

// ============================================================================
// Convenience wrapper functions
// ============================================================================

/// Safe user space read wrapper
///
/// # Arguments
/// - `from`: User space source address
///
/// # Returns
/// Returns read value on success, None on failure
#[inline]
pub unsafe fn get_user<T: Copy>(from: *const T) -> Option<T> {
    let size = core::mem::size_of::<T>();

    if !access_ok(from as usize, size) {
        return None;
    }

    let mut value: core::mem::MaybeUninit<T> = core::mem::MaybeUninit::uninit();

    let uncopied = copy_from_user(
        value.as_mut_ptr() as *mut u8,
        from as *const u8,
        size,
    );

    if uncopied == 0 {
        Some(value.assume_init())
    } else {
        None
    }
}

/// Safe user space write wrapper
///
/// # Arguments
/// - `to`: User space destination address
/// - `value`: Value to write
///
/// # Returns
/// Returns true on success, false on failure
#[inline]
pub unsafe fn put_user<T: Copy>(to: *mut T, value: T) -> bool {
    let size = core::mem::size_of::<T>();

    if !access_ok(to as usize, size) {
        return false;
    }

    let uncopied = copy_to_user(
        to as *mut u8,
        &value as *const T as *const u8,
        size,
    );

    uncopied == 0
}

/// Copy null-terminated string from user space
///
/// # Arguments
/// - `dst`: Kernel destination buffer
/// - `src`: User space source address
/// - `maxlen`: Maximum length to copy (including null terminator)
///
/// # Returns
/// Returns the length of the string (excluding null) on success,
/// or -EFAULT on failure
pub unsafe fn strncpy_from_user(dst: *mut u8, src: *const u8, maxlen: usize) -> isize {
    if maxlen == 0 {
        return 0;
    }

    if !access_ok(src as usize, 1) {
        return -14; // -EFAULT
    }

    let mut i = 0;
    while i < maxlen {
        let byte = match get_user(src.add(i) as *const u8) {
            Some(b) => b,
            None => return -14, // -EFAULT
        };

        *dst.add(i) = byte;

        if byte == 0 {
            return i as isize;
        }

        i += 1;
    }

    // Make sure string is null-terminated
    if maxlen > 0 {
        *dst.add(maxlen - 1) = 0;
    }

    maxlen as isize
}

/// Get length of null-terminated string in user space
///
/// # Arguments
/// - `str`: User space string address
/// - `maxlen`: Maximum length to check
///
/// # Returns
/// Returns the length of the string (excluding null) plus 1 on success,
/// or 0 on failure (including if string is longer than maxlen)
pub unsafe fn strnlen_user(str: *const u8, maxlen: usize) -> usize {
    if maxlen == 0 {
        return 0;
    }

    if !access_ok(str as usize, 1) {
        return 0;
    }

    let mut len = 0;
    while len < maxlen {
        match get_user(str.add(len) as *const u8) {
            Some(0) => return len + 1, // Include null terminator
            Some(_) => len += 1,
            None => return 0,
        }
    }

    0 // String too long
}
