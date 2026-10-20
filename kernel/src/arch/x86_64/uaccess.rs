//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 user-memory access.
//!
//! Copy cores are `rep movsb`/`rep stosb` loops bracketed with
//! `__ex_table` entries: the kernel's higher-half mappings coexist with
//! user space and SMAP/SMEP are not enabled, so plain string ops reach
//! user memory, and a #PF on an unmapped/unfaultable user page lands in
//! the fixup (see arch/x86_64/mm/exception.rs `fixup_exception`) instead
//! of the KernelPanic path — the copy then returns a short count, exactly
//! like the riscv64 twin's SUM-bracketed copies.
//!
//! Restartability is the load-bearing property: the architecture
//! guarantees that an exception in the middle of a REP string leaves
//! RCX = remaining iterations and RSI/RDI past the completed ones, so
//! the fixup only has to move RCX into the return register.
//!
//! EVERY accessor — scalars included — routes through the extable copy
//! cores. A bare `read_volatile`/`write_volatile` on an access_ok-gated
//! user pointer still panics the kernel when the page is unmapped
//! (LTP x86-r2 N1: 37 EFAULT-family tests hit `pagefault ... (no fixup)`
//! with rip inside core::ptr::read_volatile). The riscv64 twin has the
//! same shape: get_user/put_user = size-of-T copies, string walkers =
//! per-byte get_user.

use super::mm::memory_layout::user_addr::USER_END;

pub const MAX_RW_COUNT: usize = 0x7FFF_F000;

// asm copy cores (AT&T syntax — global_asm defaults to Intel on x86).
core::arch::global_asm!(
    r#"
.section .text.uaccess
.align 8

/* __x64_copy_to_user: rdi = user dst, rsi = kernel src, rdx = n.
 * Returns rax = bytes NOT copied. */
.global __x64_copy_to_user
.hidden __x64_copy_to_user
__x64_copy_to_user:
    movq %rdx, %rcx
    testq %rcx, %rcx
    jz 2f
    cld
1:  rep movsb
    .pushsection __ex_table, "a"
    .balign 8
    .quad 1b
    .quad 3f
    .popsection
2:  movq %rcx, %rax          /* rcx == 0 on completion */
    ret
3:  movq %rcx, %rax          /* fault mid-copy: rcx = remaining */
    ret

/* __x64_copy_from_user: rdi = kernel dst, rsi = user src, rdx = n.
 * Returns rax = bytes NOT copied. */
.global __x64_copy_from_user
.hidden __x64_copy_from_user
__x64_copy_from_user:
    movq %rdx, %rcx
    testq %rcx, %rcx
    jz 2f
    cld
1:  rep movsb
    .pushsection __ex_table, "a"
    .balign 8
    .quad 1b
    .quad 3f
    .popsection
2:  movq %rcx, %rax
    ret
3:  movq %rcx, %rax
    ret

/* __x64_clear_user: rdi = user dst, rsi = n.
 * Returns rax = bytes NOT cleared. */
.global __x64_clear_user
.hidden __x64_clear_user
__x64_clear_user:
    movq %rsi, %rcx
    testq %rcx, %rcx
    jz 2f
    xorl %eax, %eax
    cld
1:  rep stosb
    .pushsection __ex_table, "a"
    .balign 8
    .quad 1b
    .quad 3f
    .popsection
2:  xorl %eax, %eax
    ret
3:  movq %rcx, %rax
    ret
"#,
    options(att_syntax)
);

extern "C" {
    fn __x64_copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __x64_copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __x64_clear_user(to: *mut u8, n: usize) -> usize;
}

/// Is the [addr, addr+size) window inside user VA space?
pub fn access_ok(addr: usize, size: usize) -> bool {
    addr.checked_add(size).map_or(false, |end| end <= USER_END)
}

/// Copy kernel → user. Returns the number of bytes NOT copied.
///
/// A write into a COW-downgraded user page (e.g. a register-held local on
/// the parent's stack after fork — the parent's first write to that page
/// may be THIS kernel copy) faults under CR0.WP, resolves through the
/// trap path, and the `rep movsb` re-executes — see exception.rs. The
/// fault_in_write retry below is the second belt (Linux
/// fault_in_pages_writeable + copy retry parity, ported from the riscv64
/// twin): if the extable fixup already consumed the fault once (short
/// copy) and the range is still resolvable, resolve it here and retry
/// once instead of propagating EFAULT.
///
/// # Safety
/// `from` must be readable for `n` bytes; `to` is validated by access_ok.
pub unsafe fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if !access_ok(to as usize, n) {
        return n;
    }
    // SAFETY: exception-table copy — a fault on `to` jumps to the fixup
    // and returns the remaining count instead of faulting the kernel.
    let uncopied = unsafe { __x64_copy_to_user(to, from, n) };
    if uncopied == 0 {
        return 0;
    }
    // SAFETY: best-effort COW/demand resolution on the user range.
    if unsafe { fault_in_write(to, n) } {
        // SAFETY: retry the copy after resolution.
        return unsafe { __x64_copy_to_user(to, from, n) };
    }
    uncopied
}

/// Copy user → kernel. Returns the number of bytes NOT copied.
///
/// # Safety
/// `to` must be writable for `n` bytes; `from` is validated by access_ok.
pub unsafe fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if !access_ok(from as usize, n) {
        return n;
    }
    // SAFETY: exception-table copy (see copy_to_user).
    unsafe { __x64_copy_from_user(to, from, n) }
}

/// Zero user memory. Returns the number of bytes NOT cleared.
///
/// Same COW discipline as copy_to_user (see fault_in_write).
///
/// # Safety
/// `to` is validated by access_ok; a fault lands in the exception table.
pub unsafe fn clear_user(to: *mut u8, n: usize) -> usize {
    if !access_ok(to as usize, n) {
        return n;
    }
    // SAFETY: exception-table clear (see copy_to_user).
    let uncleared = unsafe { __x64_clear_user(to, n) };
    if uncleared == 0 {
        return 0;
    }
    // SAFETY: best-effort COW/demand resolution on the user range.
    if unsafe { fault_in_write(to, n) } {
        // SAFETY: retry the clear after resolution.
        return unsafe { __x64_clear_user(to, n) };
    }
    uncleared
}

/// Fault-in a user address range for writing (COW resolution / demand
/// mapping), best-effort: returns true if at least the first page got
/// resolved. Only handles resolvable cases; genuinely bad ranges still
/// return false so callers report EFAULT. Ported from the riscv64 twin
/// (Linux fault_in_pages_writeable parity) — with CR0.WP set, a
/// supervisor uaccess write to a COW/read-only page faults instead of
/// silently piercing the protection (F1b), so the copy layer needs an
/// explicit resolve-and-retry for the pages the trap path could not fix.
unsafe fn fault_in_write(start: *mut u8, len: usize) -> bool {
    use crate::arch::mm::PageTableEntry;
    use crate::arch::mm::mm_ops::PageTableWalker;

    let task = match crate::sched::current() {
        Some(t) => t,
        None => return false,
    };

    let addr_space = match task.address_space() {
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
        if let Some((_ppn, bits)) = PageTableWalker::walk(root_ppn, page as u64) {
            if bits & PageTableEntry::W != 0 {
                resolved_any = true;
                if page == last_page { break; }
                page += 0x1000;
                continue;
            }
            // Valid but read-only: COW? (x86_64 COW software bit 9)
            if bits & (1 << 9) != 0 {
                match unsafe {
                    crate::arch::mm::mm_ops::handle_cow_fault(
                        root_ppn,
                        crate::arch::mm::VirtAddr::new(page as u64),
                    )
                } {
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
            crate::arch::mm::VirtAddr::new(page as u64),
            crate::arch::mm::page_fault::FaultFlags::WRITE,
        ) {
            crate::arch::mm::MmFaultResult::CowPending => {
                // handle_cow_fault resolves it
                match unsafe {
                    crate::arch::mm::mm_ops::handle_cow_fault(
                        root_ppn,
                        crate::arch::mm::VirtAddr::new(page as u64),
                    )
                } {
                    crate::arch::mm::CowFaultResult::Resolved
                    | crate::arch::mm::CowFaultResult::Retry => {
                        resolved_any = true;
                    }
                    crate::arch::mm::CowFaultResult::OutOfMemory => {}
                }
            }
            crate::arch::mm::MmFaultResult::Handled
            | crate::arch::mm::MmFaultResult::AlreadyMapped => {
                resolved_any = true;
            }
            _ => {}
        }
        if page == last_page { break; }
        page += 0x1000;
    }
    resolved_any
}

/// Read one value from user memory.
///
/// Routes through the exception-table copy core: a fault on the user
/// page returns a short copy → None (EFAULT), never a kernel panic.
///
/// # Safety
/// `from` is validated by access_ok only in this phase.
pub unsafe fn get_user<T: Copy>(from: *const T) -> Option<T> {
    let size = core::mem::size_of::<T>();

    if !access_ok(from as usize, size) {
        return None;
    }

    let mut value: core::mem::MaybeUninit<T> = core::mem::MaybeUninit::uninit();

    // SAFETY: destination is a size-of-T kernel stack slot; the source
    // read faults into the extable fixup which returns a short count.
    let uncopied = unsafe {
        __x64_copy_from_user(value.as_mut_ptr() as *mut u8, from as *const u8, size)
    };

    if uncopied == 0 {
        // SAFETY: `size` bytes were copied into the slot.
        Some(unsafe { value.assume_init() })
    } else {
        None
    }
}

/// Write one value to user memory.
///
/// Routes through the exception-table copy core (see get_user).
///
/// # Safety
/// `to` is validated by access_ok only in this phase.
pub unsafe fn put_user<T: Copy>(to: *mut T, value: T) -> bool {
    let size = core::mem::size_of::<T>();

    if !access_ok(to as usize, size) {
        return false;
    }

    // SAFETY: source is a size-of-T kernel stack slot; the destination
    // write faults into the extable fixup which returns a short count.
    let uncopied =
        unsafe { __x64_copy_to_user(to as *mut u8, &value as *const T as *const u8, size) };

    uncopied == 0
}

/// Safely read a null-terminated string from user space
///
/// Uses `get_user` (backed by the exception-table copy core) for each
/// byte, so a page fault in user memory returns an error instead of
/// panicking the kernel. Ported verbatim from the riscv64 twin.
///
/// # Arguments
/// - `from`: User space source address
/// - `max_len`: Maximum bytes to read (including null terminator)
/// - `buf`: Kernel buffer to store the string
///
/// # Returns
/// Returns Ok(slice) on success (without null terminator), Err(-EFAULT) on failure
pub fn strncpy_from_user<'a>(
    from: *const u8,
    max_len: usize,
    buf: &'a mut [u8],
) -> Result<&'a [u8], i64> {
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
    let max = if addr < USER_END { USER_END - addr } else { return Err(-EFAULT) };
    let limit = core::cmp::min(max_len, buf.len());
    let limit = core::cmp::min(limit, max);

    let mut i = 0;
    while i < limit {
        // get_user goes through __x64_copy_from_user (assembly) which has
        // exception table entries, so page faults are handled safely.
        // Note: a NUL at index 0 (empty string) is a SUCCESS returning an
        // empty slice — rejecting it breaks empty-argument execs.
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
/// or 0 on failure (including if string is longer than maxlen).
pub unsafe fn strnlen_user(str: *const u8, maxlen: usize) -> usize {
    if maxlen == 0 {
        return 0;
    }

    if !access_ok(str as usize, 1) {
        return 0;
    }

    let mut len = 0;
    while len < maxlen {
        match unsafe { get_user(str.add(len)) } {
            Some(0) => return len + 1, // Include null terminator
            Some(_) => len += 1,
            None => return 0,
        }
    }

    0 // String too long
}
