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

/// Linux MAX_RW_COUNT (INT_MAX & PAGE_MASK, 2 GiB minus one page): the
/// per-syscall upper bound for single read/write style transfers, so a
/// huge user count can never overflow internal length arithmetic.
/// Applied at the syscall layer via `crate::syscall::io::clamp_rw_count`.
pub const MAX_RW_COUNT: usize = 0x7FFF_F000;

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

    // Delegate to assembly implementation which has exception table entries
    // for fault-safe user memory access.
    let uncopied = __copy_to_user(to, from, n);
    if uncopied == 0 {
        return 0;
    }

    // The copy faulted. A write to a COW-downgraded user page (e.g. a
    // register-held local on the parent's stack after fork — the parent's
    // first write to that page may be THIS kernel copy) is resolvable:
    // fault the range in for write and retry once, like Linux's
    // fault_in_pages_writeable + copy retry (NEW2 root cause).
    if fault_in_write(to, n) {
        let uncopied2 = __copy_to_user(to, from, n);
        return uncopied2;
    }
    uncopied
}

/// Fault-in a user address range for writing (COW resolution / demand
/// mapping), best-effort: returns true if at least the first page got
/// resolved. Only handles resolvable cases; genuinely bad ranges still
/// return false so callers report EFAULT.
unsafe fn fault_in_write(start: *mut u8, len: usize) -> bool {
    use crate::mm::page::VirtAddr as PageVirtAddr;
    use crate::arch::riscv64::mm::memory_layout::VirtAddr as ArchVirtAddr;

    let task = match crate::sched::current() {
        Some(t) => t,
        None => return false,
    };
    let addr_space = match (*task).address_space() {
        Some(a) => a,
        None => return false,
    };
    let root_ppn = addr_space.root_ppn();

    let first_page = (start as usize) & !0xFFF;
    let last_page = ((start as usize) + len.saturating_sub(1)) & !0xFFF;
    let mut resolved_any = false;
    let mut page = first_page;
    loop {
        // Already writable?
        if let Some((_ppn, bits)) =
            crate::arch::riscv64::mm::mm_ops::PageTableWalker::walk(root_ppn, page as u64)
        {
            if bits & crate::arch::riscv64::mm::PageTableEntry::W != 0 {
                resolved_any = true;
                if page == last_page { break; }
                page += 0x1000;
                continue;
            }
            // Valid but read-only: COW? (COW software bit 8)
            if bits & (1 << 8) != 0 {
                if crate::arch::riscv64::mm::mm_ops::handle_cow_fault(
                    root_ppn,
                    ArchVirtAddr::new(page as u64),
                )
                .is_some()
                {
                    resolved_any = true;
                }
            }
            // Non-COW read-only is a genuine protection fault — give up.
            if page == last_page { break; }
            page += 0x1000;
            continue;
        }

        // Not present: demand-map through the fault engine (anonymous
        // write fault / stack growth / file page).
        match crate::arch::riscv64::mm::page_fault::handle_mm_fault(
            &addr_space,
            ArchVirtAddr::new(page as u64),
            crate::arch::riscv64::mm::page_fault::FaultFlags::WRITE,
        ) {
            crate::arch::riscv64::mm::page_fault::MmFaultResult::CowPending => {
                // handle_cow_fault resolves it
                if crate::arch::riscv64::mm::mm_ops::handle_cow_fault(
                    root_ppn,
                    ArchVirtAddr::new(page as u64),
                )
                .is_some()
                {
                    resolved_any = true;
                }
            }
            crate::arch::riscv64::mm::page_fault::MmFaultResult::Handled
            | crate::arch::riscv64::mm::page_fault::MmFaultResult::AlreadyMapped => {
                resolved_any = true;
            }
            _ => {}
        }
        if page == last_page { break; }
        page += 0x1000;
    }
    resolved_any
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
/// On failure the uncopied tail of `to` is zero-filled, matching Linux's
/// copy_from_user (prevents kernel-heap information disclosure when the
/// caller proceeds with a short copy).
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
        // Zero the whole kernel buffer: callers (e.g. msgsnd, semtimedop)
        // historically consumed the buffer even on failure, and leaving
        // previous kernel heap contents in it is an information leak.
        core::ptr::write_bytes(to, 0, n);
        return n;
    }

    // Delegate to assembly implementation which has exception table entries
    // for fault-safe user memory access.
    let uncopied = __copy_from_user(to, from, n);
    if uncopied > 0 {
        // Zero-fill the uncopied tail (Linux semantics, review SEC): the
        // kernel buffer must never keep stale contents past the point the
        // user copy reached.
        let copied = n - uncopied;
        core::ptr::write_bytes(to.add(copied), 0, uncopied);
    }
    uncopied
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
/// Uses `get_user` (backed by the assembly exception-table implementation)
/// for each byte, so a page fault in user memory returns an error instead of
/// panicking the kernel.
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

    // Compute max readable bytes as distance from pointer to
    // USER_END. This avoids the old bug where access_ok(from, max_len)
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

    let mut i = 0;
    while i < limit {
        // get_user goes through copy_from_user → __copy_from_user (assembly)
        // which has exception table entries, so page faults are handled safely.
        // Note: a NUL at index 0 (empty string) is a SUCCESS returning an
        // empty slice — the old `i == 0 -> EFAULT` rejected it (review BUG).
        match unsafe { get_user(from.add(i)) } {
            Some(byte) => {
                buf[i] = byte;
                if byte == 0 {
                    break;
                }
                i += 1;
            }
            None => return Err(-EFAULT),
        }
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
