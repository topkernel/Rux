//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 exception handling — exception table + page fault routing
//! (stage-2 port of the riscv64 twin's do_page_fault policy).
//!
//! fixup_exception is arch-neutral (the .ex_table section format and the
//! linker symbols match the other backends byte-for-byte). do_page_fault
//! routes: kernel uaccess demand-fill BEFORE the fixup (twin R82), the
//! fixup itself, the kernel stack-overflow panic, and the user-mode arm
//! (handle_mm_fault + CowPending resolution via handle_cow_fault).


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use super::memory_layout::VirtAddr;
use super::mm_ops::handle_cow_fault;
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
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn fixup_exception(addr: u64) -> Option<u64> {
    crate::arch::arch_floor::aarch64::mm::exception::fixup_exception(addr)
}
/// Check if the exception table is empty
///
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
#[allow(dead_code)]
pub fn exception_table_empty() -> bool {
    crate::arch::arch_floor::aarch64::mm::exception::exception_table_empty()
}
/// Get the exception table entry count
///
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
#[allow(dead_code)]
pub fn exception_table_count() -> usize {
    crate::arch::arch_floor::aarch64::mm::exception::exception_table_count()
}
/// Send a signal to the current process with si_code provenance (twin
///
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn send_signal(sig: i32, code: i32, _addr: u64, _access_type: u32) {
    crate::arch::arch_floor::aarch64::mm::exception::send_signal(sig, code, _addr, _access_type)
}

/// Bad-area path: user gets SIGSEGV (MAPERR); kernel tries the exception
/// table, then panics (twin contract)
pub(crate) fn bad_area(regs: &mut PtRegs, access_type: u32, fault_addr: VirtAddr) -> MmFaultResult {
    if regs.user_mode() {
        send_signal(11, 1, fault_addr.bits(), access_type); // SIGSEGV, SEGV_MAPERR
        return MmFaultResult::Segfault;
    }
    if let Some(fixup) = fixup_exception(regs.pc) {
        regs.pc = fixup;
        return MmFaultResult::Fixed;
    }
    MmFaultResult::KernelPanic
}

/// No-context path (early boot / kernel thread)
pub(crate) fn no_context(regs: &mut PtRegs, fault_addr: VirtAddr) -> MmFaultResult {
    if let Some(fixup) = fixup_exception(regs.pc) {
        regs.pc = fixup;
        return MmFaultResult::Fixed;
    }
    crate::pr_err!(
        "aarch64 page fault in no-address-space context: addr={:#x} pc={:#x}",
        fault_addr.bits(),
        regs.pc
    );
    MmFaultResult::KernelPanic
}
/// do_page_fault — page fault handling entry (called from the trap path)
///
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn do_page_fault(regs: &mut PtRegs, access_type: u32) -> MmFaultResult {
    crate::arch::arch_floor::aarch64::mm::exception::do_page_fault(regs, access_type)
}
