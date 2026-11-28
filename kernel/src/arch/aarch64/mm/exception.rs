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
pub(crate) 
fn send_signal(sig: i32, code: i32, _addr: u64, _access_type: u32) {


                                                                   if let Some(current) = crate::sched::current() {
                                                                       let pid = current.pid();
                                                                       crate::signal::send_signal_with_info_directed(
                                                                           pid,
                                                                           crate::signal::SigInfo::new(sig, code, 0, 0),
                                                                       )
                                                                       .ok();
                                                                       // SAFETY: current() returned a live task pointer.
                                                                       { crate::signal::signal_wake_up(current as *mut _) };
                                                                   }

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
pub 
fn do_page_fault(regs: &mut PtRegs, access_type: u32) -> MmFaultResult {


                                                                       // /proc/vmstat pgfault + the per-task minflt counter (user mode only)
                                                                       crate::mm::vmstat::note_fault();
                                                                       if regs.user_mode() {
                                                                           if let Some(task) = crate::sched::current() {
                                                                               // SAFETY: current() returned a live task pointer; per-task
                                                                               // atomic counter only.
                                                                               {
                                                                                   (*task).min_flt.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                                                                               }
                                                                           }
                                                                       }

                                                                       let fault_addr = VirtAddr::new(regs.far);

                                                                       crate::pr_debug!(
                                                                           "do_page_fault: addr={:#x}, pc={:#x}, type={:#x}, mode={}",
                                                                           fault_addr.bits(),
                                                                           regs.pc,
                                                                           access_type,
                                                                           if regs.kernel_mode() { "kernel" } else { "user" }
                                                                       );

                                                                       let current = match crate::sched::current() {
                                                                           Some(t) => t,
                                                                           None => return no_context(regs, fault_addr),
                                                                       };
                                                                       let addr_space = match current.address_space() {
                                                                           Some(aspace) => aspace,
                                                                           None => return no_context(regs, fault_addr),
                                                                       };

                                                                       // Interrupt context: the mm fault path allocates and takes the PTE
                                                                       // section — it must not run from an IRQ handler (twin guard, the
                                                                       // x86_64/riscv64 arms; the riscv64 preempt-underflow healing engine
                                                                       // has no aarch64 counterpart, so this is the plain no_context form).
                                                                       // A fixup entry still unwinds a uaccess copy cleanly.
                                                                       if crate::interrupt::preempt::in_interrupt() {
                                                                           return no_context(regs, fault_addr);
                                                                       }

                                                                       // Kernel-mode fault on a user address
                                                                       if regs.kernel_mode() {
                                                                           // Demand-fill BEFORE fixup (twin R82): a uaccess fault on a
                                                                           // VMA-covered but not-yet-present/COW page must fault the page
                                                                           // in, not report EFAULT. EXEC faults are excluded (fetches never
                                                                           // come from uaccess windows).
                                                                           let kernel_data_fill = fault_addr.bits()
                                                                               < crate::arch::mm::user_addr::USER_END as u64
                                                                               && (access_type & FaultFlags::EXEC) == 0;
                                                                           if kernel_data_fill {
                                                                               let covered = addr_space
                                                                                   .vma_read()
                                                                                   .find(crate::mm::page::VirtAddr::new(fault_addr.as_usize()))
                                                                                   .is_some();
                                                                               // 84th (twin of the x86_64 installs-SIGABRT fix): an
                                                                               // UNCOVERED kernel uaccess fault must still reach
                                                                               // handle_mm_fault so its VMA-miss arm grows a GROWSDOWN
                                                                               // stack (Linux expand_downwards for kernel-originated
                                                                               // faults) — out-params in never-touched below-floor stack
                                                                               // pages otherwise EFAULT. Non-stack uncovered addresses
                                                                               // still fixup (find_stack_vma None -> Segfault -> `_`).
                                                                               if covered || addr_space
                                                                                   .vma_read()
                                                                                   .find_stack_vma(crate::mm::page::VirtAddr::new(fault_addr.as_usize()))
                                                                                   .is_some()
                                                                               {
                                                                                   let result = handle_mm_fault(
                                                                                       &addr_space,
                                                                                       fault_addr,
                                                                                       access_type | FaultFlags::USER,
                                                                                   );
                                                                                   match result {
                                                                                       MmFaultResult::Handled | MmFaultResult::Fixed => {
                                                                                           return MmFaultResult::Handled;
                                                                                       }
                                                                                       MmFaultResult::CowPending => {
                                                                                           // Kernel WRITE uaccess onto a COW page: resolve
                                                                                           // like the user-mode path (retry counts as
                                                                                           // progress).
                                                                                           // SAFETY: faulting mapping of the current task.
                                                                                           {
                                                                                               match handle_cow_fault(
                                                                                                   addr_space,
                                                                                                   addr_space.root_ppn(),
                                                                                                   fault_addr,
                                                                                               ) {
                                                                                                   crate::arch::aarch64::mm::mm_ops::CowFaultResult::Resolved
                                                                                                   | crate::arch::aarch64::mm::mm_ops::CowFaultResult::Retry => {
                                                                                                       return MmFaultResult::Handled;
                                                                                                   }
                                                                                                   crate::arch::aarch64::mm::mm_ops::CowFaultResult::OutOfMemory => {}
                                                                                               }
                                                                                           }
                                                                                       }
                                                                                       // Unresolvable: fall through to the fixup so the copy
                                                                                       // reports a short copy.
                                                                                       _ => {}
                                                                                   }
                                                                               }
                                                                           }

                                                                           // Exception-table fixup (uaccess windows — twin R78 discipline)
                                                                           if let Some(fixup) = fixup_exception(regs.pc) {
                                                                               regs.pc = fixup;
                                                                               return MmFaultResult::Fixed;
                                                                           }

                                                                           // Kernel stack overflow
                                                                           let fault_addr_usize = fault_addr.bits() as usize;
                                                                           // SAFETY: current task accessors.
                                                                           {
                                                                               if current.is_in_kernel_stack(fault_addr_usize)
                                                                                   || current.is_stack_overflow(regs.sp as usize)
                                                                               {
                                                                                   panic!(
                                                                                       "Kernel stack overflow in task {} (fault_addr={:#x}, sp={:#x})",
                                                                                       current.pid(),
                                                                                       fault_addr_usize,
                                                                                       regs.sp
                                                                                   );
                                                                               }
                                                                           }

                                                                           // Kernel accessed an invalid address (kernel bug)
                                                                           return MmFaultResult::KernelPanic;
                                                                       }

                                                                       // User-mode fault: TTBR0 VAs must be canonical user addresses —
                                                                       // bits 63:47 all clear. The hardware faults before the walk, but the
                                                                       // kernel's own walkers MASK the index to the low 48 bits: a page
                                                                       // mapped at the truncated position would make every fault resolve
                                                                       // as "mapped, perms OK" and retry forever (twin non-canonical-loop
                                                                       // note).
                                                                       if (fault_addr.bits() >> 47) != 0 {
                                                                           return bad_area(regs, access_type, fault_addr);
                                                                       }

                                                                       let result = handle_mm_fault(&addr_space, fault_addr, access_type | FaultFlags::USER);

                                                                       match result {
                                                                           MmFaultResult::Handled => MmFaultResult::Handled,
                                                                           MmFaultResult::CowPending => {
                                                                               // COW page: resolve through the arch walker
                                                                               // SAFETY: faulting mapping of the current task.
                                                                               {
                                                                                   match handle_cow_fault(&addr_space, addr_space.root_ppn(), fault_addr) {
                                                                                       crate::arch::aarch64::mm::mm_ops::CowFaultResult::Resolved => MmFaultResult::Handled,
                                                                                       // PTE changed between the lock-free COW check and
                                                                                       // the locked re-walk — re-execute (twin fake-OOM
                                                                                       // layer-2 note); the next entry sees the new state.
                                                                                       crate::arch::aarch64::mm::mm_ops::CowFaultResult::Retry => MmFaultResult::Handled,
                                                                                       crate::arch::aarch64::mm::mm_ops::CowFaultResult::OutOfMemory => {
                                                                                           send_signal(9, 0, fault_addr.bits(), access_type); // SIGKILL
                                                                                           MmFaultResult::OutOfMemory
                                                                                       }
                                                                                   }
                                                                               }
                                                                           }
                                                                           MmFaultResult::AlreadyMapped => {
                                                                               // Mapped but the permission check failed
                                                                               send_signal(11, 2, fault_addr.bits(), access_type); // SEGV_ACCERR
                                                                               MmFaultResult::PermissionDenied
                                                                           }
                                                                           MmFaultResult::Segfault => bad_area(regs, access_type, fault_addr),
                                                                           MmFaultResult::PermissionDenied => {
                                                                               send_signal(11, 2, fault_addr.bits(), access_type); // SEGV_ACCERR
                                                                               MmFaultResult::PermissionDenied
                                                                           }
                                                                           MmFaultResult::OutOfMemory => {
                                                                               send_signal(9, 0, fault_addr.bits(), access_type); // SIGKILL
                                                                               MmFaultResult::OutOfMemory
                                                                           }
                                                                           MmFaultResult::BusError => {
                                                                               send_signal(7, 2, fault_addr.bits(), access_type); // SIGBUS, BUS_ADRERR
                                                                               MmFaultResult::BusError
                                                                           }
                                                                           MmFaultResult::Fixed | MmFaultResult::KernelPanic => {
                                                                               bad_area(regs, access_type, fault_addr)
                                                                           }
                                                                       }

}
