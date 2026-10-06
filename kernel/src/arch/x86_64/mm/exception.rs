//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 fault entry: do_page_fault + exception (fixup) table.
//!
//! The `__ex_table` section is emitted by the uaccess asm
//! (X86-TODO(agent x86-trap)): entries (fault_rip, fixup_rip); a kernel
//! #PF landing inside a bracketed uaccess window jumps to the fixup
//! instead of panicking — this is how copy_to_user returns EFAULT.

use super::page_fault::{FaultFlags, MmFaultResult};
use crate::arch::pt_regs::PtRegs;

/// Exception-table entry (16 bytes, sorted by fault_rip at link time ideally)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExceptionTableEntry {
    pub fault_rip: u64,
    pub fixup_rip: u64,
}

extern "C" {
    static __ex_table_start: ExceptionTableEntry;
    static __ex_table_end: ExceptionTableEntry;
}

/// If `addr` is inside a uaccess exception window, return the fixup target.
pub fn fixup_exception(addr: u64) -> Option<u64> {
    // SAFETY: linker symbols bound the table; only addresses are read.
    unsafe {
        let start = &raw const __ex_table_start as *const ExceptionTableEntry;
        let end = &raw const __ex_table_end as *const ExceptionTableEntry;
        let mut p = start;
        while p < end {
            let e = &*p;
            if e.fault_rip == addr {
                return Some(e.fixup_rip);
            }
            p = p.add(1);
        }
    }
    None
}

pub fn exception_table_empty() -> bool {
    // SAFETY: linker symbols only.
    unsafe { &raw const __ex_table_start as usize == &raw const __ex_table_end as usize }
}

pub fn exception_table_count() -> usize {
    // SAFETY: linker symbols only.
    unsafe {
        (&raw const __ex_table_end as usize - &raw const __ex_table_start as usize)
            / core::mem::size_of::<ExceptionTableEntry>()
    }
}

/// The #PF handler entry, called from trap.rs with the error code decoded.
///
/// `access_type` is a FaultFlags bitset. On x86 the error code gives:
/// bit0 P (0 = not-present, 1 = protection), bit1 W/R, bit2 U/S, bit4 I/D.
pub fn do_page_fault(regs: &mut PtRegs, access_type: u32) -> MmFaultResult {
    let cr2 = crate::arch::cpu::read_cr2();

    // Kernel-mode fault inside a uaccess window → fixup (returns EFAULT path)
    if regs.kernel_mode() && !exception_table_empty() {
        if let Some(fixup) = fixup_exception(regs.rip) {
            regs.rip = fixup;
            return MmFaultResult::Handled;
        }
    }

    // X86-TODO(agent x86-mm): user path — resolve via AddressSpace +
    // handle_mm_fault, deliver SIGSEGV/SIGBUS on failure (mirror the
    // riscv64 twin in arch/riscv64/mm/exception.rs).
    let _ = (access_type, cr2);
    MmFaultResult::Segfault
}
