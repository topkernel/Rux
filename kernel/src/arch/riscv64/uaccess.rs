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


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

// Include optimized assembly implementation


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
// IRREDUCIBLE F-ROW (F-UACC-1): the exception-table copy bottoms —
// reference kernel arch/riscv/lib/uaccess.S (__copy_user,
// __clear_user, the lr/sc futex word); hand-written asm is the only
// shape that can carry fixup entries per instruction.
extern "C" {
    fn __copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __futex_cmpxchg_u32(uaddr: *mut u32, expected: u32, new: u32) -> u64;
    fn __clear_user(to: *mut u8, n: usize) -> usize;
}
/// Copy data from kernel to user space
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-18): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    crate::arch::arch_floor::riscv64::uaccess::copy_to_user(to, from, n)
}

/// Fault-in a user address range for writing (COW resolution / demand
/// mapping), best-effort: returns true if at least the first page got
/// resolved. Only handles resolvable cases; genuinely bad ranges still
/// return false so callers report EFAULT.
pub(crate) fn fault_in_write(start: *mut u8, len: usize) -> bool {
    use crate::mm::page::VirtAddr as PageVirtAddr;
    use crate::arch::mm::memory_layout::VirtAddr as ArchVirtAddr;

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
            crate::arch::mm::mm_ops::PageTableWalker::walk(root_ppn, page as u64)
        {
            if bits & crate::arch::mm::PageTableEntry::W != 0 {
                resolved_any = true;
                if page == last_page { break; }
                page += 0x1000;
                continue;
            }
            // Valid but read-only: COW? (COW software bit 8)
            if bits & (1 << 8) != 0 {
                match crate::arch::mm::mm_ops::handle_cow_fault(
                    addr_space,
                    root_ppn,
                    ArchVirtAddr::new(page as u64),
                ) {
                    // Retry = a sibling thread resolved the COW (or the
                    // mapping changed) between our check and the locked
                    // re-walk — the following write either succeeds or
                    // faults again into the right path. Count as progress.
                    crate::arch::mm::CowFaultResult::Resolved
                    | crate::arch::mm::CowFaultResult::Retry => {
                        resolved_any = true;
                    }
                    crate::arch::mm::CowFaultResult::OutOfMemory => {}
                }
            }
            // Non-COW read-only is a genuine protection fault — give up.
            if page == last_page { break; }
            page += 0x1000;
            continue;
        }

        // Not present: demand-map through the fault engine (anonymous
        // write fault / stack growth / file page).
        match crate::arch::mm::page_fault::handle_mm_fault(
            &addr_space,
            ArchVirtAddr::new(page as u64),
            crate::arch::mm::page_fault::FaultFlags::WRITE,
        ) {
            crate::arch::mm::page_fault::MmFaultResult::CowPending => {
                // handle_cow_fault resolves it
                match crate::arch::mm::mm_ops::handle_cow_fault(
                    addr_space,
                    root_ppn,
                    ArchVirtAddr::new(page as u64),
                ) {
                    crate::arch::mm::CowFaultResult::Resolved
                    | crate::arch::mm::CowFaultResult::Retry => {
                        resolved_any = true;
                    }
                    crate::arch::mm::CowFaultResult::OutOfMemory => {}
                }
            }
            crate::arch::mm::page_fault::MmFaultResult::Handled
            | crate::arch::mm::page_fault::MmFaultResult::AlreadyMapped => {
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
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-17): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    crate::arch::arch_floor::riscv64::uaccess::copy_from_user(to, from, n)
}
/// Zero user space memory
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-16): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn clear_user(to: *mut u8, n: usize) -> usize {
    crate::arch::arch_floor::riscv64::uaccess::clear_user(to, n)
}

// ============================================================================
// Convenience wrapper functions
// ============================================================================
/// Safe user space read wrapper
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-15): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_user<T: Copy>(from: *const T) -> Option<T> {
    crate::arch::arch_floor::riscv64::uaccess::get_user::<T>(from)
}
/// Safe user space write wrapper
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-14): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn put_user<T: Copy>(to: *mut T, value: T) -> bool {
    crate::arch::arch_floor::riscv64::uaccess::put_user::<T>(to, value)
}
/// Safely read a null-terminated string from user space
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-13): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn strncpy_from_user<'a>(from: *const u8, max_len: usize, buf: &'a mut [u8]) -> Result<&'a [u8], i64> {
    crate::arch::arch_floor::riscv64::uaccess::strncpy_from_user(from, max_len, buf)
}
/// Get length of null-terminated string in user space
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-12): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn strnlen_user(str: *const u8, maxlen: usize) -> usize {
    crate::arch::arch_floor::riscv64::uaccess::strnlen_user(str, maxlen)
}
/// Atomic compare-exchange on a user u32 word (PI futex protocol,
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-11): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn futex_cmpxchg_u32_user(uaddr: *mut u32, expected: u32, new: u32) -> Option<u32> {
    crate::arch::arch_floor::riscv64::uaccess::futex_cmpxchg_u32_user(uaddr, expected, new)
}
