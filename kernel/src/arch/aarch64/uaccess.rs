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
/// NOTE: unlike the riscv64 twin, no fault_in_write retry runs here. The
/// resolvable-fault coverage the twin gets from that retry is provided
/// by the R82 demand-fill arm in mm/exception.rs::do_page_fault: a
/// kernel-mode uaccess fault on a VMA-covered user address (not-present
/// or COW) is resolved by the fault handler itself before the
/// exception-table fixup ever sees it, so the instruction retries and
/// the copy proceeds. Only unresolvable faults (no VMA, no mapping
/// possible) reach the fixup and report a short count — the correct
/// answer for those.
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
/// On failure the uncopied tail of `to` is zero-filled, matching the
/// riscv64/x86_64 twins (prevents kernel-heap information disclosure
/// when the caller proceeds with a short copy).
///
/// # Safety
/// `to` must be writable for `n` bytes; `from` is validated by access_ok.
#[inline(never)]
pub unsafe fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    if !access_ok(from as usize, n) {
        // Zero the whole kernel buffer: callers (e.g. msgsnd, semtimedop)
        // historically consumed the buffer even on failure, and leaving
        // previous kernel heap contents in it is an information leak
        // (riscv64 twin behavior).
        core::ptr::write_bytes(to, 0, n);
        return n;
    }
    // SAFETY: exception-table copy (see copy_to_user).
    let uncopied = unsafe { __aa64_copy_from_user(to, from, n) };
    if uncopied > 0 {
        // Zero-fill the uncopied tail: the kernel buffer must never keep
        // stale contents past the point the user copy reached.
        let copied = n - uncopied;
        // SAFETY: `copied` bytes were verifiably written; the rest is `to`'s.
        unsafe { core::ptr::write_bytes(to.add(copied), 0, uncopied) };
    }
    uncopied
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
/// into `buf`. Returns the borrowed slice on success (without the NUL),
/// Err(-EFAULT) on fault.
///
/// Port of the riscv64 interface: byte-wise copy through the
/// exception-table core with early NUL stop; the readable span is
/// clamped to the distance from `from` to USER_END so a pointer near the
/// top of the user window does not fail a whole-range access_ok.
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
    if !access_ok(from as usize, 1) {
        return Err(-EFAULT);
    }

    // Max readable bytes as the distance to USER_END (twin behavior).
    let addr = from as usize;
    let user_end = super::mm::user_addr::USER_END;
    let max = if addr < user_end { user_end - addr } else { return Err(-EFAULT) };
    let limit = max_len.min(buf.len()).min(max);

    let mut i = 0;
    while i < limit {
        // A NUL at index 0 (empty string) is a SUCCESS returning an
        // empty slice (twin behavior).
        // SAFETY: single-byte user read through the exception-table copy.
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

/// strlen for a user string, bounded by maxlen (twin signature).
///
/// Returns the length INCLUDING the NUL terminator, or 0 on fault /
/// no NUL within maxlen (the riscv64/x86_64 convention: 0 is the
/// failure-or-too-long sentinel — callers treat 0 as EFAULT).
pub unsafe fn strnlen_user(str: *const u8, maxlen: usize) -> usize {
    if maxlen == 0 {
        return 0;
    }
    if !access_ok(str as usize, 1) {
        return 0;
    }

    let mut len = 0;
    while len < maxlen {
        // SAFETY: bounded single-byte reads through the exception-table
        // copy (get_user → copy_from_user → __aa64_copy_from_user).
        match unsafe { get_user(str.add(len)) } {
            Some(0) => return len + 1, // include the NUL
            Some(_) => len += 1,
            None => return 0,
        }
    }
    0 // no NUL within maxlen
}

// ============================================================================
// Boot-time fault-containment self-check (dfx-aa64-uaccess-selftest)
// ============================================================================

/// Poissoned-pointer probe for the whole fault-containment chain:
/// access_ok range rules → asm fault site → data abort (EC 0x25) →
/// exception-table fixup → short count / -EFAULT, with NO oops.
///
/// Runs at dfx init (before init exec — userspace exec is blocked on the
/// mm walker anyway) on a kernel context where TTBR0 still carries
/// boot.S's early identity map ([0, 2GB) device + RAM blocks — it is
/// torn down only after SMP start, well after this probe), so the probe
/// address must sit INSIDE the user window but OUTSIDE every mapping:
/// 4 GB — below TASK_SIZE (256GB), above the early map, an invalid L1
/// entry either way. access_ok must ACCEPT it (the exception table, not
/// the range check, contains the fault).
#[cfg(feature = "dfx-aa64-uaccess-selftest")]
pub fn selftest() {
    // printk's console emit does not reach PL011 on this board yet (the
    // boot table goes through console::putchar directly) — same here.
    fn emit(line: &str) {
        // SAFETY: putchar is the polled console byte writer (the same
        // path print_status uses); callable in any context.
        unsafe {
            for &b in line.as_bytes() {
                crate::console::putchar(b);
            }
            crate::console::putchar(b'\n');
        }
    }

    const PROBE: *mut u8 = 0x1_0000_0000 as *mut u8; // 4GB: user window, unmapped
    const N: usize = 64;

    let mut pass = 0usize;
    let mut total = 0usize;
    let mut check = |ok: bool, what: &str, detail: &str| {
        total += 1;
        if ok {
            pass += 1;
        }
        emit(&alloc::format!(
            "aa64-uaccess-selftest: {} {} ({})",
            if ok { "PASS" } else { "FAIL" },
            what,
            detail
        ));
    };

    // 1. Range rules: the probe address is INSIDE the user window (the
    //    fault must come from the exception table, not access_ok), and
    //    kernel-half / overflow addresses are rejected outright.
    check(
        access_ok(PROBE as usize, N),
        "access_ok accepts user-window probe",
        "0x100000000+64 in [0, TASK_SIZE)",
    );
    check(
        !access_ok(0xffff_ffff_8000_0000usize, 8),
        "access_ok rejects kernel half",
        "0xffffffff80000000",
    );
    check(
        !access_ok(0xffff_ffff_ffff_fff0usize, 32),
        "access_ok rejects overflow",
        "addr+size wraps",
    );

    // 2. copy_from_user on the poisoned pointer: short count + zeroed
    //    kernel buffer (twin info-leak rule).
    let mut dst = [0xA5u8; N];
    // SAFETY: dst is a valid stack buffer; the user source deliberately
    // faults and must land in the fixup (returns N, zero-fills dst).
    let uncopied = unsafe { copy_from_user(dst.as_mut_ptr(), PROBE as *const u8, N) };
    check(
        uncopied == N && dst.iter().all(|&b| b == 0),
        "copy_from_user EFAULT",
        &alloc::format!("uncopied={}/{}, dst zeroed={}", uncopied, N,
            dst.iter().all(|&b| b == 0)),
    );

    // 3. copy_to_user to the poisoned pointer: short count.
    let src = [0x5Au8; N];
    // SAFETY: src is a valid stack buffer; the user destination
    // deliberately faults and must land in the fixup.
    let uncopied = unsafe { copy_to_user(PROBE, src.as_ptr(), N) };
    check(
        uncopied == N,
        "copy_to_user EFAULT",
        &alloc::format!("uncopied={}/{}", uncopied, N),
    );

    // 4. clear_user on the poisoned pointer: uncleared count.
    // SAFETY: the user destination deliberately faults into the fixup.
    let uncleared = unsafe { clear_user(PROBE, N) };
    check(
        uncleared == N,
        "clear_user EFAULT",
        &alloc::format!("uncleared={}/{}", uncleared, N),
    );

    // 5. strncpy_from_user: Err(-EFAULT).
    let mut sbuf = [0u8; 16];
    match strncpy_from_user(PROBE as *const u8, 16, &mut sbuf) {
        Err(e) => check(e == -14, "strncpy_from_user EFAULT", &alloc::format!("errno={}", e)),
        Ok(_) => check(false, "strncpy_from_user EFAULT", "returned Ok on poisoned ptr"),
    }

    // 6. strnlen_user: 0 sentinel.
    // SAFETY: deliberately faulting user read into the fixup.
    let slen = unsafe { strnlen_user(PROBE as *const u8, 16) };
    check(slen == 0, "strnlen_user EFAULT", &alloc::format!("len={}", slen));

    // 7. get_user/put_user sized variants: None / false.
    // SAFETY: deliberately faulting user accesses into the fixup.
    let g = unsafe { get_user(PROBE as *const u64) };
    check(g.is_none(), "get_user<u64> EFAULT", "expected None");
    // SAFETY: deliberately faulting user write into the fixup.
    let p = unsafe { put_user(PROBE as *mut u64, 1u64) };
    check(!p, "put_user<u64> EFAULT", "expected false");

    emit(&alloc::format!(
        "aa64-uaccess-selftest: {} ({}/{})",
        if pass == total { "ALL PASS" } else { "FAILURES PRESENT" },
        pass,
        total
    ));
}
