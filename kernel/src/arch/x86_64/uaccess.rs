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
//! `get_user`/`put_user` (scalar accesses) remain direct loads/stores:
//! they are access_ok-gated single-word touches whose fault window is
//! one instruction; converting them is a later micro-hardening step.

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
/// # Safety
/// `from` must be readable for `n` bytes; `to` is validated by access_ok.
pub unsafe fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if !access_ok(to as usize, n) {
        return n;
    }
    // SAFETY: exception-table copy — a fault on `to` jumps to the fixup
    // and returns the remaining count instead of faulting the kernel.
    unsafe { __x64_copy_to_user(to, from, n) }
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
/// # Safety
/// `to` is validated by access_ok; a fault lands in the exception table.
pub unsafe fn clear_user(to: *mut u8, n: usize) -> usize {
    if !access_ok(to as usize, n) {
        return n;
    }
    // SAFETY: exception-table clear (see copy_to_user).
    unsafe { __x64_clear_user(to, n) }
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
    if from as usize == 0 || !access_ok(from as usize, max_len.min(buf.len())) {
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
    if str as usize == 0 || !access_ok(str as usize, maxlen) {
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
