//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V page fault handling
//!
//! Processing flow:
//! 1. Distinguish kernel/user mode
//! 2. Check interrupt context
//! 3. Find VMA
//! 4. Verify permissions
//! 5. Handle COW
//! 6. Handle anonymous pages
//! 7. Send signal or OOM
//!
//! # Exception Table Mechanism
//!
//! Exception tables are used to safely handle exceptions that may occur
//! when the kernel accesses user space.
//! Typical use cases:
//! - `copy_to_user()`: Copy data from kernel to user space
//! - `copy_from_user()`: Copy data from user space to kernel
//! - `get_user()`: Read single value from user space
//! - `put_user()`: Write single value to user space
//!
//! When these operations access invalid user addresses, a page fault is triggered.
//! The exception table records each access instruction that may fail and its fixup handler.
//! If a page fault occurs on these instructions, the kernel jumps to the fixup handler
//! instead of crashing.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use crate::arch::pt_regs::PtRegs;
use crate::arch::mm::{VirtAddr, FaultFlags, AddressSpace, handle_cow_fault, handle_mm_fault};

// Re-export MmFaultResult from page_fault (canonical definition)
pub use super::page_fault::MmFaultResult;
use crate::println;
use crate::process::task::TaskState;
use crate::mm::vma::VmaFlags;

/// Exception table entry
///
/// Used for exception fixup when kernel accesses user space.
/// When the kernel has an exception at the specified address, jump to fixup address to continue.
///
/// # Memory layout
/// Each entry occupies 16 bytes (2 × 8 byte addresses)
#[repr(C)]
pub struct ExceptionTableEntry {
    /// Instruction address where exception may occur (PC value)
    pub insn: u64,
    /// Jump address after fixup (position to continue after handling exception)
    pub fixup: u64,
}

/// Exception table boundary symbols (defined by linker script)
extern "C" {
    /// Exception table start address
    static __ex_table_start: ExceptionTableEntry;
    /// Exception table end address
    static __ex_table_end: ExceptionTableEntry;
}
/// Find fixup address in exception table
///
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-9): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn fixup_exception(addr: u64) -> Option<u64> {
    crate::arch::arch_floor::riscv64::mm::exception::fixup_exception(addr)
}
/// Check if exception table is empty
///
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-8): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
#[allow(dead_code)]
pub fn exception_table_empty() -> bool {
    crate::arch::arch_floor::riscv64::mm::exception::exception_table_empty()
}
/// Get exception table entry count
///
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-7): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
#[allow(dead_code)]
pub fn exception_table_count() -> usize {
    crate::arch::arch_floor::riscv64::mm::exception::exception_table_count()
}

/// Send signal to current process
///
/// # Arguments
/// - `sig`: Signal number
/// - `code`: Signal code (SI_XXX)
/// - `addr`: Address that triggered exception
/// - `epc`: Instruction address where exception occurred
/// - `access_type`: Access type
/// - `regs`: PtRegs pointer, used to get user mode tp
pub(crate) fn send_signal(sig: i32, code: i32, _addr: u64, _epc: u64, _access_type: u32, _regs: &crate::arch::pt_regs::PtRegs) {
    // Send signal using real signal mechanism. The queued siginfo carries
    // the si_code the caller classified (SEGV_MAPERR/SEGV_ACCERR): a fatal
    // hardware-fault death then prints a real origin in SIGDEATH/KILLTRACE
    // instead of the indistinguishable 0/0 of the info-less bitmap path.
    if let Some(current) = crate::sched::current() {
        let pid = current.pid();
        crate::signal::send_signal_with_info_directed(
            pid,
            crate::signal::SigInfo::new(sig, code, 0, 0),
        )
        .ok();

        // Wake up process to handle signal (if it's sleeping)
        crate::signal::signal_wake_up(current as *mut _);
    }
}

/// Check if in interrupt context
#[inline]
pub(crate) fn in_interrupt() -> bool {
    crate::interrupt::preempt::in_interrupt()
}

/// Page fault handling - bad_area path
///
/// Called when address is not in a valid VMA
pub(crate) fn bad_area(regs: &mut PtRegs, access_type: u32, fault_addr: VirtAddr) -> MmFaultResult {
    // User mode accessing invalid address
    if regs.user_mode() {
        send_signal(11, 1, fault_addr.bits(), regs.epc, access_type, regs);  // SIGSEGV, SEGV_MAPERR = 1
        return MmFaultResult::Segfault;
    }

    // Kernel mode accessing invalid address
    // Check exception table
    if let Some(fixup) = fixup_exception(regs.epc) {
        regs.epc = fixup;
        return MmFaultResult::Fixed;
    }

    // Cannot fix, kernel panic
    MmFaultResult::KernelPanic
}

/// Page fault handling - no_context path
///
/// Called when valid process context cannot be obtained
pub(crate) fn no_context(_regs: &mut PtRegs, _fault_addr: VirtAddr) -> MmFaultResult {
    // Check exception table
    if let Some(fixup) = fixup_exception(_regs.epc) {
        _regs.epc = fixup;
        return MmFaultResult::Fixed;
    }

    // Cannot handle
    MmFaultResult::KernelPanic
}
/// do_page_fault - Page fault handling main function
///
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn do_page_fault(regs: &mut PtRegs, access_type: u32) -> MmFaultResult {
    crate::arch::arch_floor::riscv64::mm::exception::do_page_fault(regs, access_type)
}
