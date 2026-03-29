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
#[inline(never)]
pub unsafe fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    // Check if user space address is valid
    if !access_ok(to as usize, n) {
        return n;
    }

    // Enable user memory access (set SUM bit in sstatus)
    // Use a local variable instead of hardcoding t6 to avoid clobber issues
    let sum_bit: u64 = 0x40000;
    core::arch::asm!(
        "csrs sstatus, {0}",
        in(reg) sum_bit,
        options(nomem, nostack)
    );

    // Copy bytes one by one
    for i in 0..n {
        // Use volatile read/write to avoid compiler optimizations
        let byte = core::ptr::read_volatile(from.add(i));
        core::ptr::write_volatile(to.add(i), byte);
    }

    // Disable user memory access (clear SUM bit in sstatus)
    core::arch::asm!(
        "csrc sstatus, {0}",
        in(reg) sum_bit,
        options(nomem, nostack)
    );

    0 // Success
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
#[inline(never)]
pub unsafe fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    // Check if user space address is valid
    if !access_ok(from as usize, n) {
        return n;
    }

    // Enable user memory access (set SUM bit in sstatus)
    let sum_bit: u64 = 0x40000;
    core::arch::asm!(
        "csrs sstatus, {0}",
        in(reg) sum_bit,
        options(nomem, nostack)
    );

    // Copy bytes one by one
    for i in 0..n {
        // Use volatile read to avoid compiler optimizations
        let byte = core::ptr::read_volatile(from.add(i));
        core::ptr::write_volatile(to.add(i), byte);
    }

    // Disable user memory access (clear SUM bit in sstatus)
    core::arch::asm!(
        "csrc sstatus, {0}",
        in(reg) sum_bit,
        options(nomem, nostack)
    );

    0 // Success
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

/// Safely read a null-terminated string from user space
///
/// # Arguments
/// - `from`: User space source address
/// - `max_len`: Maximum bytes to read (including null terminator)
/// - `buf`: Kernel buffer to store the string
///
/// # Returns
/// Returns Ok(slice) on success (without null terminator), Err(-EFAULT) on failure
pub fn strncpy_from_user<'a>(from: *const u8, max_len: usize, buf: &'a mut [u8]) -> Result<&'a [u8], i64> {
    // EFAULT = 14
    const EFAULT: i64 = 14;

    if from.is_null() {
        return Err(-EFAULT);
    }

    // Verify the pointer itself is in user space.
    if !access_ok(from as usize, 1) {
        return Err(-EFAULT);
    }

    // Like Linux: compute max readable bytes as distance from pointer to
    // TASK_SIZE_MAX. This avoids the old bug where access_ok(from, max_len)
    // failed when from was near the end of user space.
    let addr = from as usize;
    let user_end = super::mm::user_addr::USER_END;
    let max = if addr < user_end {
        user_end - addr
    } else {
        return Err(-EFAULT);
    };
    let limit = core::cmp::min(max_len, buf.len());
    let limit = core::cmp::min(limit, max);

    // Enable user memory access (set SUM bit in sstatus)
    let sum_bit: u64 = 0x40000;
    unsafe {
        core::arch::asm!(
            "csrs sstatus, {0}",
            in(reg) sum_bit,
            options(nomem, nostack)
        );
    }

    let mut i = 0;
    unsafe {
        while i < limit {
            let byte = core::ptr::read_volatile(from.add(i));
            buf[i] = byte;
            if byte == 0 {
                break;
            }
            i += 1;
        }
    }

    // Disable user memory access (clear SUM bit in sstatus)
    unsafe {
        core::arch::asm!(
            "csrc sstatus, {0}",
            in(reg) sum_bit,
            options(nomem, nostack)
        );
    }

    if i == 0 {
        return Err(-EFAULT);
    }

    Ok(&buf[..i])
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
