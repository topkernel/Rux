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


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use super::mm::memory_layout::user_addr::USER_END;

pub const MAX_RW_COUNT: usize = 0x7FFF_F000;

// asm copy cores (AT&T syntax — global_asm defaults to Intel on x86).
//
// AC bracketing (perf-filerd lane, bw_file_rd 50x-turned-1.5x anatomy):
// the cores used to set/clear EFLAGS.AC with pushfq + orq/andq on (%rsp)
// + popfq — a read-modify-write of the full flags word through memory,
// ~100+ cycles per bracket pair, around EVERY page-sized copy. With 16
// per-page copies per 64 KiB read(2) that bracket tax alone was a third
// of the copy phase. Two properties make the fast form safe:
//   * CR4.SMAP is the ONLY feature that faults user accesses at CPL0
//     with AC clear — when SMAP_ENABLED is 0 the bracket is dead weight
//     and is skipped entirely (STAC/CLAC would also #UD on a CPU without
//     the SMAP CPUID bit, e.g. QEMU's default TCG model).
//   * When SMAP is on, the SMAP CPUID bit (leaf 7 EBX[20]) implies the
//     STAC/CLAC instructions exist — 2-cycle flag writes, no memory, no
//     serialization — so the bracket becomes stac/clac.
// The fixup label ALSO clears AC (a fault mid-copy lands there with AC
// still set); with SMAP off both clac sites degrade to plain ret, same
// as the old pushfq/andq/popfq pair did.


// IRREDUCIBLE F-ROW (F-UACC-2): the exception-table copy bottoms —
// reference kernel arch/x86/lib/copy_user_64.S (rep movsb with .fixup
// sections); hand-written asm is the only shape with fixup entries.
extern "C" {
    fn __x64_copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __x64_copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize;
    fn __x64_clear_user(to: *mut u8, n: usize) -> usize;
    fn __x64_futex_cmpxchg_u32(uaddr: *mut u32, expected: u32, new: u32) -> u64;
}
/// Atomic compare-exchange on a user u32 word (PI futex protocol,
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-27): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn futex_cmpxchg_u32_user(uaddr: *mut u32, expected: u32, new: u32) -> Option<u32> {
    crate::arch::arch_floor::x86_64::uaccess::futex_cmpxchg_u32_user(uaddr, expected, new)
}

/// Is the [addr, addr+size) window inside user VA space?
pub fn access_ok(addr: usize, size: usize) -> bool {
    addr.checked_add(size).map_or(false, |end| end <= USER_END)
}
/// Copy kernel → user. Returns the number of bytes NOT copied.
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-26): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn copy_to_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    crate::arch::arch_floor::x86_64::uaccess::copy_to_user(to, from, n)
}
/// Copy user → kernel. Returns the number of bytes NOT copied.
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-25): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn copy_from_user(to: *mut u8, from: *const u8, n: usize) -> usize {
    crate::arch::arch_floor::x86_64::uaccess::copy_from_user(to, from, n)
}
/// Zero user memory. Returns the number of bytes NOT cleared.
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-24): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn clear_user(to: *mut u8, n: usize) -> usize {
    crate::arch::arch_floor::x86_64::uaccess::clear_user(to, n)
}

/// Fault-in a user address range for writing (COW resolution / demand
/// mapping), best-effort: returns true if at least the first page got
/// resolved. Only handles resolvable cases; genuinely bad ranges still
/// return false so callers report EFAULT. Ported from the riscv64 twin
/// (Linux fault_in_pages_writeable parity) — with CR0.WP set, a
/// supervisor uaccess write to a COW/read-only page faults instead of
/// silently piercing the protection (F1b), so the copy layer needs an
/// explicit resolve-and-retry for the pages the trap path could not fix.
pub(crate) fn fault_in_write(start: *mut u8, len: usize) -> bool {
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
                match crate::arch::mm::mm_ops::handle_cow_fault(
                    addr_space,
                    root_ppn,
                    crate::arch::mm::VirtAddr::new(page as u64),
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
            crate::arch::mm::VirtAddr::new(page as u64),
            crate::arch::mm::page_fault::FaultFlags::WRITE,
        ) {
            crate::arch::mm::MmFaultResult::CowPending => {
                // handle_cow_fault resolves it
                // Leg-15 pace: tick after return (pte_lock released).
                let cow_j0 = crate::drivers::timer::get_jiffies();
                let cow_res = crate::arch::mm::mm_ops::handle_cow_fault(
                    addr_space,
                    root_ppn,
                    crate::arch::mm::VirtAddr::new(page as u64),
                );
                crate::dfx::schb::note_cow(cow_j0);
                match cow_res {
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
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-23): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_user<T: Copy>(from: *const T) -> Option<T> {
    crate::arch::arch_floor::x86_64::uaccess::get_user::<T>(from)
}
/// Write one value to user memory.
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-22): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn put_user<T: Copy>(to: *mut T, value: T) -> bool {
    crate::arch::arch_floor::x86_64::uaccess::put_user::<T>(to, value)
}
/// Safely read a null-terminated string from user space
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-21): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn strncpy_from_user<'a>( from: *const u8, max_len: usize, buf: &'a mut [u8], ) -> Result<&'a [u8], i64> {
    crate::arch::arch_floor::x86_64::uaccess::strncpy_from_user(from, max_len, buf)
}
/// Get length of null-terminated string in user space
///
/// Collected into `arch::arch_floor` (INV-ARCH-UACCESS-20): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn strnlen_user(str: *const u8, maxlen: usize) -> usize {
    crate::arch::arch_floor::x86_64::uaccess::strnlen_user(str, maxlen)
}
