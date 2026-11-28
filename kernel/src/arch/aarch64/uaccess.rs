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


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::arch::global_asm;


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

// IRREDUCIBLE F-ROW (F-UACC-3): the exception-table copy bottoms —
// reference kernel arch/arm64/lib/copy_from_user.S; same fixup-table
// class as the other arches.
extern "C" {
    fn __aa64_copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __aa64_copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __aa64_clear_user(to: *mut u8, n: usize) -> usize;
    fn __aa64_futex_cmpxchg_u32(uaddr: *mut u32, expected: u32, new: u32) -> u64;
}
/// Atomic compare-exchange on a user u32 word (PI futex protocol,
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-9): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn futex_cmpxchg_u32_user(uaddr: *mut u32, expected: u32, new: u32) -> Option<u32> {
    crate::arch::arch_floor::aarch64::uaccess::futex_cmpxchg_u32_user(uaddr, expected, new)
}
/// Copy data from kernel to user space. Returns number of uncopied bytes.
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-8): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    crate::arch::arch_floor::aarch64::uaccess::copy_to_user(to, from, n)
}
/// Copy user → kernel. Returns the number of bytes NOT copied.
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-7): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    crate::arch::arch_floor::aarch64::uaccess::copy_from_user(to, from, n)
}
/// Zero user memory. Returns the number of bytes NOT cleared.
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn clear_user(to: *mut u8, n: usize) -> usize {
    crate::arch::arch_floor::aarch64::uaccess::clear_user(to, n)
}
/// Write a single value to user space (typed). Returns success.
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn put_user<T: Copy>(to: *mut T, value: T) -> bool {


                                                   let size = core::mem::size_of::<T>();
                                                   if !access_ok(to as usize, size) {
                                                       return false;
                                                   }
                                                   // SAFETY: exception-table copy behind access_ok.
                                                   let uncopied = { copy_to_user(to as *mut u8, &value as *const T as *const u8, size) };
                                                   uncopied == 0

}
/// Read a single value from user space (typed). Returns None on fault.
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_user<T: Copy>(from: *const T) -> Option<T> {
    crate::arch::arch_floor::aarch64::uaccess::get_user::<T>(from)
}
/// Copy a NUL-terminated string from user space (bounded by max_len),
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn strncpy_from_user<'a>( from: *const u8, max_len: usize, buf: &'a mut [u8], ) -> Result<&'a [u8], i64> {
    crate::arch::arch_floor::aarch64::uaccess::strncpy_from_user(from, max_len, buf)
}
/// strlen for a user string, bounded by maxlen (twin signature).
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn strnlen_user(str: *const u8, maxlen: usize) -> usize {
    crate::arch::arch_floor::aarch64::uaccess::strnlen_user(str, maxlen)
}

// ============================================================================
// Boot-time fault-containment self-check (dfx-aa64-uaccess-selftest)
// ============================================================================
/// Poissoned-pointer probe for the whole fault-containment chain:
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
#[cfg(feature = "dfx-aa64-uaccess-selftest")]
pub 
fn selftest() {


              // printk's console emit does not reach PL011 on this board yet (the
              // boot table goes through console::putchar directly) — same here.
              fn emit(line: &str) {
                  // SAFETY: putchar is the polled console byte writer (the same
                  // path print_status uses); callable in any context.

                      for &b in line.as_bytes() {
                          crate::console::putchar(b);
                      }
                      crate::console::putchar(b'\n');

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
              let uncopied = { copy_from_user(dst.as_mut_ptr(), PROBE as *const u8, N) };
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
              let uncopied = { copy_to_user(PROBE, src.as_ptr(), N) };
              check(
                  uncopied == N,
                  "copy_to_user EFAULT",
                  &alloc::format!("uncopied={}/{}", uncopied, N),
              );

              // 4. clear_user on the poisoned pointer: uncleared count.
              // SAFETY: the user destination deliberately faults into the fixup.
              let uncleared = { clear_user(PROBE, N) };
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
              let slen = { strnlen_user(PROBE as *const u8, 16) };
              check(slen == 0, "strnlen_user EFAULT", &alloc::format!("len={}", slen));

              // 7. get_user/put_user sized variants: None / false.
              // SAFETY: deliberately faulting user accesses into the fixup.
              let g = { get_user(PROBE as *const u64) };
              check(g.is_none(), "get_user<u64> EFAULT", "expected None");
              // SAFETY: deliberately faulting user write into the fixup.
              let p = { put_user(PROBE as *mut u64, 1u64) };
              check(!p, "put_user<u64> EFAULT", "expected false");

              emit(&alloc::format!(
                  "aa64-uaccess-selftest: {} ({}/{})",
                  if pass == total { "ALL PASS" } else { "FAILURES PRESENT" },
                  pass,
                  total
              ));

}
