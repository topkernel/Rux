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
pub unsafe fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    // Check if user space address is valid
    if !access_ok(to as usize, n) {
        return n;
    }

    // Use exception-handled copy
    // If access fails, return uncopied bytes
    let mut remaining = n;
    let mut dst = to as usize;
    let mut src = from as usize;

    while remaining > 0 {
        // Try to copy one byte
        let result = copy_one_byte_to_user(dst, src);

        match result {
            Ok(_) => {
                dst += 1;
                src += 1;
                remaining -= 1;
            }
            Err(_) => {
                // Copy failed, return remaining bytes
                break;
            }
        }
    }

    remaining
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
pub unsafe fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    // Check if user space address is valid
    if !access_ok(from as usize, n) {
        return n;
    }

    // Use exception-handled copy
    let mut remaining = n;
    let mut dst = to as usize;
    let mut src = from as usize;

    while remaining > 0 {
        // Try to copy one byte
        let result = copy_one_byte_from_user(dst, src);

        match result {
            Ok(_) => {
                dst += 1;
                src += 1;
                remaining -= 1;
            }
            Err(_) => {
                // Copy failed, return remaining bytes
                break;
            }
        }
    }

    remaining
}

/// Copy single byte to user space (with exception handling)
///
/// # Returns
/// - Ok(()): Copy successful
/// - Err(()): Copy failed (invalid user address)
///
/// # Note
/// This is a simplified implementation, actual exception table mechanism requires assembly support
#[inline(always)]
unsafe fn copy_one_byte_to_user(to: usize, from: usize) -> Result<(), ()> {
    // Simplified implementation: direct copy
    // Actual exception table version needs to add exception table entries in assembly
    let src_ptr = from as *const u8;
    let dst_ptr = to as *mut u8;

    // Check user address validity
    if !access_ok(to, 1) {
        return Err(());
    }

    *dst_ptr = *src_ptr;
    Ok(())
}

/// Copy single byte from user space (with exception handling)
///
/// # Returns
/// - Ok(()): Copy successful
/// - Err(()): Copy failed (invalid user address)
///
/// # Note
/// This is a simplified implementation, actual exception table mechanism requires assembly support
#[inline(always)]
unsafe fn copy_one_byte_from_user(to: usize, from: usize) -> Result<(), ()> {
    // Simplified implementation: direct copy
    let src_ptr = from as *const u8;
    let dst_ptr = to as *mut u8;

    // Check user address validity
    if !access_ok(from, 1) {
        return Err(());
    }

    *dst_ptr = *src_ptr;
    Ok(())
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
pub unsafe fn clear_user(to: *mut u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    if !access_ok(to as usize, n) {
        return n;
    }

    let mut remaining = n;
    let mut dst = to as usize;

    while remaining > 0 {
        let result = clear_one_byte_user(dst);

        match result {
            Ok(_) => {
                dst += 1;
                remaining -= 1;
            }
            Err(_) => {
                break;
            }
        }
    }

    remaining
}

/// Zero single byte in user space
///
/// # Note
/// This is a simplified implementation, actual exception table mechanism requires assembly support
#[inline(always)]
unsafe fn clear_one_byte_user(to: usize) -> Result<(), ()> {
    let dst_ptr = to as *mut u8;

    if !access_ok(to, 1) {
        return Err(());
    }

    *dst_ptr = 0;
    Ok(())
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
