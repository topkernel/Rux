//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 fault entry: do_page_fault + exception (fixup) table.
//!
//! The `__ex_table` section is emitted by the uaccess asm: entries
//! (fault_rip, fixup_rip); a kernel #PF landing inside a bracketed uaccess
//! window jumps to the fixup instead of panicking — this is how
//! copy_to_user returns EFAULT.
//!
//! Ported from arch/riscv64/mm/exception.rs: fault address comes from CR2
//! (not the trap frame), RIP replaces EPC, and the riscv-specific
//! NOVMA-EXEC forensic replay is reduced to a one-line witness (the x86
//! port has no uaccess probe plumbing yet).


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use super::page_fault::FaultFlags;
use crate::arch::pt_regs::PtRegs;
use crate::arch::mm::{VirtAddr, MmFaultResult, handle_cow_fault, handle_mm_fault};
use crate::println;

/// Exception table entry (16 bytes, sorted by fault_rip at link time ideally)
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
///
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-11): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn fixup_exception(addr: u64) -> Option<u64> {
    crate::arch::arch_floor::x86_64::mm::exception::fixup_exception(addr)
}

pub fn exception_table_empty() -> bool {
    // Linker symbols only; address-of is safe.
    &raw const __ex_table_start as usize == &raw const __ex_table_end as usize
}

pub fn exception_table_count() -> usize {
    // Linker symbols only; address arithmetic is safe.
    (&raw const __ex_table_end as usize - &raw const __ex_table_start as usize)
        / core::mem::size_of::<ExceptionTableEntry>()
}

/// Send signal to current process
///
/// # Arguments
/// - `sig`: Signal number
/// - `code`: Signal code (SI_XXX)
/// - `addr`: Address that triggered exception
/// - `rip`: Instruction address where exception occurred
/// - `regs`: PtRegs pointer, used to get the current task
pub(crate) fn send_signal(sig: i32, _code: i32, _addr: u64, _rip: u64, _regs: &PtRegs) {
    // Send signal using the real signal mechanism
    if let Some(current) = crate::sched::current() {
        let pid = current.pid();
        crate::signal::send_signal(pid, sig).ok();

        // force_sig parity for hardware faults (the riscv64/aarch64
        // twins and x86_64's own GP/trap arms call this; the #PF arms
        // here missed it): a fault-generated SIGSEGV/SIGBUS pended on a
        // thread that blocks the signal stays pending forever — do_signal's
        // first_unmasked filter skips it, execution returns to the
        // faulting rip, and the fault repeats: an unkillable livelock.
        // Convicted on the LTP mmap05->shmdt01/mmap1 pair (2026-11-21):
        // the child inherits a SIGSEGV-blocked mask, its first fault
        // lands here, and the task refaults at the same rip until the
        // runner's timeout (the user_segv_forensics dumps repeat, the
        // VMA disappears under its own racing teardown). Linux delivers
        // hardware-generated signals regardless of the mask.
        crate::signal::force_unblock_current(sig);

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
pub(crate) fn bad_area(regs: &mut PtRegs, _access_type: u32, fault_addr: VirtAddr) -> MmFaultResult {
    // User mode accessing invalid address
    if regs.user_mode() {
        send_signal(11, 1, fault_addr.bits(), regs.rip, regs);  // SIGSEGV, SEGV_MAPERR = 1
        return MmFaultResult::Segfault;
    }

    // Kernel mode accessing invalid address
    // Check exception table
    if let Some(fixup) = fixup_exception(regs.rip) {
        regs.rip = fixup;
        return MmFaultResult::Handled;
    }

    // Cannot fix, kernel panic
    MmFaultResult::KernelPanic
}

/// Page fault handling - no_context path
///
/// Called when valid process context cannot be obtained
pub(crate) fn no_context(regs: &mut PtRegs, _fault_addr: VirtAddr) -> MmFaultResult {
    // Check exception table
    if let Some(fixup) = fixup_exception(regs.rip) {
        regs.rip = fixup;
        return MmFaultResult::Handled;
    }

    // SMAP audit beacon (perf review 2026-10-31): a no_context fault with
    // CR4.SMAP on, AC clear and a user-range fault address is a RAW
    // user-pointer dereference. Name the instruction BEFORE the oops
    // machinery mangles the context — the audit loop is: run the tests,
    // read this line, convert the site to uaccess, repeat.
    {
        static SHOWN: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        if SHOWN.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
            crate::println!(
                "PF-NOCTX rip={:#x} addr={:#x} ac={} cr4smap={}",
                regs.rip,
                _fault_addr.bits(),
                (regs.rflags >> 18) & 1,
                crate::arch::cpu::smap_enabled() as u8
            );
        }
    }

    // Cannot handle
    MmFaultResult::KernelPanic
}
/// do_page_fault - page fault handling main function (#PF, vector 14).
///
/// Collected into `arch::arch_floor` (INV-ARCH-EXCEPTION-10): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn do_page_fault(regs: &mut PtRegs, access_type: u32) -> MmFaultResult {
    crate::arch::arch_floor::x86_64::mm::exception::do_page_fault(regs, access_type)
}
