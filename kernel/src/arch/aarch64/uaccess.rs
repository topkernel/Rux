//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! User space access functions (AArch64)
//!
//! Provides kernel↔user copy functionality with exception-table fault
//! safety — the same mechanism the riscv64 (uaccess.S) and x86_64
//! (global_asm!) backends use:
//!
//! - `access_ok` range-checks against the user VA window
//! - the copy cores are global_asm with `.ex_table` entries; a fault on
//!   the user access jumps to the fixup, which returns the number of
//!   uncopied bytes instead of faulting the kernel
//!
//! Note on PAN: cortex-a72 (ARMv8.0, what QEMU virt models) does not
//! implement PSTATE.PAN, so the kernel can dereference user VAs directly
//! whenever the current TTBR0 carries the user map — the same discipline
//! as riscv64 with SR_SUM. Fault safety comes from the exception table,
//! not from a hardware access gate.

use core::arch::global_asm;

global_asm!(
    r#"
    // extable insn, fixup — add an exception-table entry: when the
    // instruction at `insn` faults, the trap path rewinds PC to `fixup`.
    //
    // CRITICAL usage rule (matches the riscv64 twin's EXTABLE): the
    // label referenced by \insn MUST be placed immediately BEFORE the
    // faulting instruction, and the macro invoked immediately AFTER it:
    //
    //     .Lfault_site:
    //         <load/store that may fault>
    //         extable .Lfault_site, fixup
    //
    // A label defined after the macro binds to the NEXT instruction, and
    // the entry would cover the wrong instruction.
    .macro extable, insn, fixup
    .pushsection .ex_table, "a"
    .balign 8
    .quad \insn
    .quad \fixup
    .popsection
    .endm

    // x0 = user dst, x1 = kernel src, x2 = n; returns uncopied in x0.
    // On a fault the post-indexed store's writeback is architecturally
    // suppressed, so x2 still holds the not-yet-copied count. The
    // extable entry MUST label the user store (the strb), NOT the kernel
    // load: a fault's PC is the faulting instruction's own address, and
    // an entry on the ldrb would never match a user-address abort (the
    // store would oops instead of returning a short count).
    .align 4
    .global __aa64_copy_to_user
    .type __aa64_copy_to_user, %function
__aa64_copy_to_user:
    cbz x2, .Lctu_done
.Lctu_load:
    ldrb w4, [x1], #1
.Lctu_store:
    strb w4, [x0], #1
    extable .Lctu_store, .Lctu_fault
    subs x2, x2, #1
    b.ne .Lctu_load
.Lctu_done:
    mov x0, #0
    ret
.Lctu_fault:
    mov x0, x2
    ret

    // x0 = kernel dst, x1 = user src, x2 = n; returns uncopied in x0.
    .align 4
    .global __aa64_copy_from_user
    .type __aa64_copy_from_user, %function
__aa64_copy_from_user:
    cbz x2, .Lcfu_done
.Lcfu_load:
    ldrb w4, [x1], #1
    extable .Lcfu_load, .Lcfu_fault
    strb w4, [x0], #1
    subs x2, x2, #1
    b.ne .Lcfu_load
.Lcfu_done:
    mov x0, #0
    ret
.Lcfu_fault:
    mov x0, x2
    ret

    // x0 = user dst, x1 = n; returns uncleared in x0.
    .align 4
    .global __aa64_clear_user
    .type __aa64_clear_user, %function
__aa64_clear_user:
    mov x2, x1
    cbz x2, .Lclru_done
.Lclru_store:
    strb wzr, [x0], #1
    extable .Lclru_store, .Lclru_fault
    subs x2, x2, #1
    b.ne .Lclru_store
.Lclru_done:
    mov x0, #0
    ret
.Lclru_fault:
    mov x0, x2
    ret
"#
);

/// Linux MAX_RW_COUNT (INT_MAX & PAGE_MASK, 2 GiB minus one page)
pub const MAX_RW_COUNT: usize = 0x7FFF_F000;

/// User space access error type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAccessError {
    Success,
    InvalidSource,
    InvalidDestination,
    Unaligned,
    Unknown,
}

/// Check if user space address is valid
#[inline]
pub fn access_ok(addr: usize, size: usize) -> bool {
    use super::mm::user_addr::{USER_START, USER_END};

    if addr < USER_START {
        return false;
    }
    let end = match addr.checked_add(size) {
        Some(e) => e,
        None => return false,
    };
    end <= USER_END
}

// ============================================================================
// Assembly function declarations
// ============================================================================

extern "C" {
    fn __aa64_copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __aa64_copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __aa64_clear_user(to: *mut u8, n: usize) -> usize;
}

/// Copy data from kernel to user space. Returns number of uncopied bytes.
///
/// # Safety
/// - `from` must point to valid kernel memory
/// - `to` is validated by access_ok; a fault lands in the exception table
#[inline(never)]
pub unsafe fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    if !access_ok(to as usize, n) {
        return n;
    }
    // SAFETY: exception-table copy — a fault on `to` jumps to the fixup
    // and returns a short count instead of faulting the kernel.
    unsafe { __aa64_copy_to_user(to, from, n) }
}

/// Copy user → kernel. Returns the number of bytes NOT copied.
///
/// # Safety
/// `to` must be writable for `n` bytes; `from` is validated by access_ok.
#[inline(never)]
pub unsafe fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    if !access_ok(from as usize, n) {
        return n;
    }
    // SAFETY: exception-table copy (see copy_to_user).
    unsafe { __aa64_copy_from_user(to, from, n) }
}

/// Zero user memory. Returns the number of bytes NOT cleared.
///
/// # Safety
/// `to` is validated by access_ok; a fault lands in the exception table.
#[inline(never)]
pub unsafe fn clear_user(to: *mut u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    if !access_ok(to as usize, n) {
        return n;
    }
    // SAFETY: exception-table clear (see copy_to_user).
    unsafe { __aa64_clear_user(to, n) }
}

/// Write a single value to user space (typed). Returns success.
///
/// # Safety
/// `to` must be aligned and validated by access_ok
#[inline]
pub unsafe fn put_user<T: Copy>(to: *mut T, value: T) -> bool {
    let size = core::mem::size_of::<T>();
    if !access_ok(to as usize, size) {
        return false;
    }
    // SAFETY: exception-table copy behind access_ok.
    let uncopied = unsafe { copy_to_user(to as *mut u8, &value as *const T as *const u8, size) };
    uncopied == 0
}

/// Read a single value from user space (typed). Returns None on fault.
///
/// # Safety
/// `from` must be aligned and validated by access_ok
#[inline]
pub unsafe fn get_user<T: Copy>(from: *const T) -> Option<T> {
    let size = core::mem::size_of::<T>();
    if !access_ok(from as usize, size) {
        return None;
    }
    let mut out = core::mem::MaybeUninit::<T>::zeroed();
    // SAFETY: exception-table copy behind access_ok.
    let uncopied =
        unsafe { copy_from_user(out.as_mut_ptr() as *mut u8, from as *const u8, size) };
    if uncopied == 0 {
        // SAFETY: fully initialized by the copy above
        Some(unsafe { out.assume_init() })
    } else {
        None
    }
}

/// Copy a NUL-terminated string from user space (bounded by max_len),
/// into `buf`. Returns the borrowed slice on success.
///
/// Port of the riscv64 interface: byte-wise copy with early NUL stop;
/// faults land in the exception table and surface as -EFAULT.
pub fn strncpy_from_user<'a>(
    from: *const u8,
    max_len: usize,
    buf: &'a mut [u8],
) -> Result<&'a [u8], i64> {
    if max_len == 0 || buf.is_empty() {
        return Ok(&[]);
    }
    if !access_ok(from as usize, 1) {
        return Err(-14);
    }
    let cap = max_len.min(buf.len());
    for i in 0..cap {
        let mut byte: u8 = 0;
        // SAFETY: single-byte user read; the address range was validated
        // per byte and faults land in the exception table (short count).
        let uncopied = unsafe { copy_from_user(&mut byte as *mut u8 as *mut u8, unsafe { from.add(i) }, 1) };
        if uncopied != 0 {
            return Err(-14);
        }
        buf[i] = byte;
        if byte == 0 {
            return Ok(&buf[..i]); // exclude the NUL
        }
    }
    Ok(&buf[..cap])
}

/// strlen for a user string, bounded by max_len (fault-safe)
pub fn strnlen_user(from: *const u8, max_len: usize) -> Result<usize, i64> {
    if max_len == 0 {
        return Ok(0);
    }
    if !access_ok(from as usize, 1) {
        return Err(-14);
    }
    let mut probe: [u8; 1] = [0; 1];
    for i in 0..max_len {
        // SAFETY: bounded single-byte fault-safe reads
        let uncopied = unsafe { copy_from_user(probe.as_mut_ptr(), unsafe { from.add(i) }, 1) };
        if uncopied != 0 {
            return Err(-14);
        }
        if probe[0] == 0 {
            return Ok(i + 1); // including the NUL, strnlen_user convention
        }
    }
    Ok(max_len)
}
