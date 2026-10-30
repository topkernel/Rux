//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 exception handling — exception table + page fault routing.
//!
//! fixup_exception is arch-neutral (the .ex_table section format and the
//! linker symbols match the other backends byte-for-byte). do_page_fault
//! decodes the aarch64 fault registers (ESR/FAR): a kernel-mode abort at
//! a uaccess fault site rewinds regs.pc to the fixup landing pad
//! (MmFaultResult::Fixed → the trap return re-executes there and the
//! copy reports a short count / -EFAULT instead of an oops); user-mode
//! faults route into handle_mm_fault once the mm lane lands its stage-2
//! body (VMA demand fill / COW / kill policy).

use super::memory_layout::VirtAddr;
use super::page_fault::{handle_mm_fault, FaultFlags};
// Re-exported so the trap path can `use crate::arch::mm::exception::MmFaultResult`.
pub use super::page_fault::MmFaultResult;
use crate::arch::pt_regs::PtRegs;

/// Exception table entry (16 bytes: faulting PC + fixup PC)
#[repr(C)]
pub struct ExceptionTableEntry {
    /// Instruction address where an exception may occur (PC value)
    pub insn: u64,
    /// Jump target after fixup
    pub fixup: u64,
}

// Exception table boundary symbols (defined by the linker script)
extern "C" {
    static __ex_table_start: ExceptionTableEntry;
    static __ex_table_end: ExceptionTableEntry;
}

/// Find the fixup address for a faulting PC
///
/// Linear search O(n) — the table holds a handful of entries (the three
/// uaccess copy cores), matching the riscv64/x86_64 twins.
pub fn fixup_exception(addr: u64) -> Option<u64> {
    // SAFETY: the section bounds are linker-provided and valid; entries
    // are immutable after link.
    unsafe {
        let start = &__ex_table_start as *const ExceptionTableEntry;
        let end = &__ex_table_end as *const ExceptionTableEntry;
        let count = (end as usize - start as usize) / core::mem::size_of::<ExceptionTableEntry>();
        for i in 0..count {
            let entry = &*start.add(i);
            if entry.insn == addr {
                return Some(entry.fixup);
            }
        }
    }
    None
}

/// Check if the exception table is empty
#[allow(dead_code)]
pub fn exception_table_empty() -> bool {
    // SAFETY: linker-provided bounds.
    unsafe { &__ex_table_start as *const _ == &__ex_table_end as *const _ }
}

/// Get the exception table entry count
#[allow(dead_code)]
pub fn exception_table_count() -> usize {
    // SAFETY: linker-provided bounds.
    unsafe {
        (&__ex_table_end as *const _ as usize - &__ex_table_start as *const _ as usize)
            / core::mem::size_of::<ExceptionTableEntry>()
    }
}

/// do_page_fault — page fault handling entry (called from the trap path)
///
/// STAGE 2: the full policy (VMA lookup, signal delivery, kernel-panic
/// classification, the no_context report) lands with the trap entry
/// path. The stage-1 body routes kernel-mode faults through the
/// exception table fixup (uaccess safety) and treats everything else as
/// a loud, diagnosable fault.
pub fn do_page_fault(regs: &mut PtRegs, access_type: u32) -> MmFaultResult {
    crate::mm::vmstat::note_fault();
    if regs.user_mode() {
        if let Some(task) = crate::sched::current() {
            // SAFETY: current() returned a live task pointer; per-task
            // atomic counter only.
            unsafe {
                (*task).min_flt.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    // On aarch64 the fault address is FAR_EL1 (saved in pt_regs.far by
    // the trap entry)
    let fault_addr = VirtAddr::new(regs.far);

    crate::pr_debug!(
        "do_page_fault: addr={:#x}, pc={:#x}, type={:#x}, mode={}",
        fault_addr.bits(),
        regs.pc,
        access_type,
        if regs.kernel_mode() { "kernel" } else { "user" }
    );

    // Kernel-mode fault: try the exception table first (uaccess copies).
    // A fault inside a uaccess fault-site returns to the fixup landing
    // pad, which reports a short count — the caller surfaces -EFAULT.
    // Identical to the riscv64 twin's kernel-mode arm (regs.epc = fixup).
    if regs.kernel_mode() {
        if let Some(fixup) = fixup_exception(regs.pc) {
            regs.pc = fixup;
            return MmFaultResult::Fixed;
        }
    }

    // User-mode fault: route into the mm fault machinery
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return no_context(regs, fault_addr),
    };
    let addr_space = match current.address_space() {
        Some(aspace) => aspace,
        None => return no_context(regs, fault_addr),
    };

    handle_mm_fault(&addr_space, fault_addr, access_type)
}

/// No-address-space fault handling (early boot / kernel thread)
fn no_context(regs: &mut PtRegs, addr: VirtAddr) -> MmFaultResult {
    // Check the exception table before declaring the fault fatal
    // (riscv64 twin's no_context: a uaccess fault in a kernel thread —
    // no address space — still must return through the fixup).
    if let Some(fixup) = fixup_exception(regs.pc) {
        regs.pc = fixup;
        return MmFaultResult::Fixed;
    }
    crate::pr_err!(
        "aarch64 page fault in no-address-space context: addr={:#x} pc={:#x}",
        addr.bits(),
        regs.pc
    );
    MmFaultResult::KernelPanic
}
