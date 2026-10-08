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
fn send_signal(sig: i32, _code: i32, _addr: u64, _rip: u64, _regs: &PtRegs) {
    // Send signal using the real signal mechanism
    if let Some(current) = crate::sched::current() {
        let pid = current.pid();
        crate::signal::send_signal(pid, sig).ok();

        // Wake up process to handle signal (if it's sleeping)
        crate::signal::signal_wake_up(current as *mut _);
    }
}

/// Check if in interrupt context
#[inline]
fn in_interrupt() -> bool {
    crate::interrupt::preempt::in_interrupt()
}

/// Page fault handling - bad_area path
///
/// Called when address is not in a valid VMA
fn bad_area(regs: &mut PtRegs, _access_type: u32, fault_addr: VirtAddr) -> MmFaultResult {
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
fn no_context(regs: &mut PtRegs, _fault_addr: VirtAddr) -> MmFaultResult {
    // Check exception table
    if let Some(fixup) = fixup_exception(regs.rip) {
        regs.rip = fixup;
        return MmFaultResult::Handled;
    }

    // Cannot handle
    MmFaultResult::KernelPanic
}

/// do_page_fault - page fault handling main function (#PF, vector 14).
///
/// # Arguments
/// - `regs`: Trap frame/register state
/// - `access_type`: Access type (FaultFlags bitset decoded from the error
///   code by the trap entry: bit0 P (0 = not-present, 1 = protection),
///   bit1 W/R, bit2 U/S, bit4 I/D)
///
/// # Returns
/// Handling result
pub fn do_page_fault(regs: &mut PtRegs, access_type: u32) -> MmFaultResult {
    let cr2 = crate::arch::cpu::read_cr2();
    let fault_addr = VirtAddr::new(cr2);

    crate::pr_debug!(
        "do_page_fault: addr={:#x}, rip={:#x}, type={:#x}, mode={}",
        fault_addr.bits(), regs.rip, access_type,
        if regs.kernel_mode() { "kernel" } else { "user" }
    );

    // X3 fix: do NOT fixup uaccess-window kernel faults here. The early
    // check below intercepted EVERY kernel #PF inside copy_to/from_user
    // and routed it to the fixup (EFAULT) BEFORE the kernel_data_fill
    // branch lower down could demand-fill a VMA-covered user page — so
    // the FIRST kernel touch of any not-yet-faulted anonymous page
    // (malloc'd sigaltstack, fresh stdio buffers) reported EFAULT
    // instead of faulting the page in (sigaltstack01: sigframe
    // copy_to_user onto the never-touched altstack → forced SIGSEGV).
    // The fixup still applies at the tail of the kernel path, after
    // demand-fill had its chance.

    // Get current process's address space
    let current = match crate::sched::current() {
        Some(t) => t,
        None => {
            // No current process, might be early boot stage
            return no_context(regs, fault_addr);
        }
    };

    let addr_space = match current.address_space() {
        Some(aspace) => aspace,
        None => {
            // Kernel thread has no address space
            return no_context(regs, fault_addr);
        }
    };

    // Check if in interrupt context
    if in_interrupt() {
        // Cannot sleep in interrupt context
        return no_context(regs, fault_addr);
    }

    // Kernel mode access
    if regs.kernel_mode() {
        // Check for kernel stack overflow
        // If fault address is near current task's kernel stack, it's likely
        // a stack overflow
        let fault_addr_usize = fault_addr.bits() as usize;

        if current.is_in_kernel_stack(fault_addr_usize) || current.is_stack_overflow(regs.rsp as usize) {
            panic!(
                "Kernel stack overflow in task {} (fault_addr={:#x}, rsp={:#x})",
                current.pid(), fault_addr_usize, regs.rsp
            );
        }

        // Kernel-mode access to a USER address whose VMA covers it: demand-
        // fill the page and RETRY the instruction (Linux semantics — the
        // kernel's copy_from_user on a not-yet-faulted user page must fault
        // the page in, not EFAULT through the exception table — that
        // starved every stdio buffer allocation in dynamically linked
        // binaries on riscv64).
        //
        // EXEC faults are EXCLUDED: a ring-0 instruction fetch is never
        // satisfied by a user page (SMEP blocks U=1 fetches at CPL0 even
        // when mapped), so "fixing" the VMA and retrying just refaults
        // forever — a jump through a garbage function pointer in kernel
        // context then wedges the CPU in an unbounded fault/retry loop.
        // Route kernel EXEC faults straight to the exception table /
        // KernelPanic so the bug is loud and the CPU halts instead of
        // looping.
        let kernel_data_fill = fault_addr.bits()
            < crate::arch::mm::user_addr::USER_END as u64
            && (access_type & FaultFlags::EXEC) == 0;
        if kernel_data_fill {
            let covered = addr_space
                .vma_read()
                .find(crate::mm::page::VirtAddr::new(fault_addr.as_usize()))
                .is_some();
            if covered {
                let result = handle_mm_fault(
                    &addr_space,
                    fault_addr,
                    access_type | FaultFlags::USER,
                );
                if matches!(result, MmFaultResult::Handled)
                    || matches!(result, MmFaultResult::Fixed)
                {
                    // Re-execute the faulting kernel load/store.
                    return MmFaultResult::Handled;
                }
                // Unresolvable (bad perms etc.): fall through to the
                // exception-table fixup so the copy reports a short copy.
            }
        }

        // Check exception table (copy_to_user/copy_from_user etc.)
        if let Some(fixup) = fixup_exception(regs.rip) {
            regs.rip = fixup;
            return MmFaultResult::Handled;
        }

        // Kernel accessed invalid address (possibly a bug)
        println!(
            "pagefault: kernel fault addr={:#x} rip={:#x} pid={} (no fixup)",
            fault_addr.bits(),
            regs.rip,
            crate::sched::get_current_pid()
        );
        return MmFaultResult::KernelPanic;
    }

    // User mode page fault handling

    // Non-canonical or kernel-half address: hardware faults BEFORE any
    // page-table walk, but the kernel's own walkers MASK the index to 48
    // bits — a page mapped at the truncated position would make every
    // fault resolve as "already mapped, perms OK", retrying the same
    // non-canonical fetch forever (the riscv64 trap-loop lesson). User
    // VAs must have bits 63:47 all clear.
    if (fault_addr.bits() >> 47) != 0 {
        return bad_area(regs, access_type, fault_addr);
    }

    // 1. Call handle_mm_fault to handle
    let result = handle_mm_fault(&addr_space, fault_addr, access_type | FaultFlags::USER);

    match result {
        MmFaultResult::Handled | MmFaultResult::Fixed => {
            // Page mapped, can re-execute instruction
            MmFaultResult::Handled
        }
        MmFaultResult::CowPending => {
            // COW page, try copy-on-write
            match unsafe { handle_cow_fault(addr_space.root_ppn(), fault_addr) } {
                crate::arch::mm::CowFaultResult::Resolved => {
                    MmFaultResult::Handled
                }
                crate::arch::mm::CowFaultResult::Retry => {
                    // The PTE changed between handle_mm_fault's lock-free
                    // is_cow_page() check and the locked re-walk — a
                    // sibling thread sharing this mm broke the COW first.
                    // Re-execute the instruction; if the store still
                    // faults, the next entry sees the new PTE state and
                    // takes the proper path (never a kill for the race
                    // loser — the riscv64 "fake OOM layer 2" lesson).
                    MmFaultResult::Handled
                }
                crate::arch::mm::CowFaultResult::OutOfMemory => {
                    // Allocating the private copy genuinely failed
                    MmFaultResult::OutOfMemory
                }
            }
        }
        MmFaultResult::AlreadyMapped => {
            // Mapped but permission issue
            // Possibly writing to read-only page etc.
            send_signal(11, 2, fault_addr.bits(), regs.rip, regs);  // SIGSEGV, SEGV_ACCERR = 2
            MmFaultResult::PermissionDenied
        }
        MmFaultResult::Segfault => {
            // Address not in any VMA — one-line witness (the riscv64 twin
            // replays rings/uaccess here; x86 bring-up has no such
            // plumbing yet).
            if access_type & FaultFlags::EXEC != 0 {
                let cr3 = crate::arch::cpu::read_cr3();
                let exe = crate::sched::current()
                    .map(|t| t.get_exe_path())
                    .unwrap_or(&[]);
                let exe_str = core::str::from_utf8(exe).unwrap_or("?");
                crate::pr_err!(
                    "NOVMA-EXEC: addr={:#x} pid={} exe={} pgd={:#x} cr3_ppn={:#x}{} rip={:#x}",
                    fault_addr.bits(),
                    crate::sched::get_current_pid(),
                    exe_str,
                    addr_space.pgd() as u64,
                    cr3 >> 12,
                    if (cr3 >> 12) == addr_space.pgd() as u64 { "" } else { " MISMATCH" },
                    regs.rip
                );
            }
            bad_area(regs, access_type, fault_addr)
        }
        MmFaultResult::PermissionDenied => {
            // Insufficient permissions
            send_signal(11, 2, fault_addr.bits(), regs.rip, regs);  // SIGSEGV, SEGV_ACCERR = 2
            MmFaultResult::PermissionDenied
        }
        MmFaultResult::OutOfMemory => {
            // Out of memory, send SIGKILL
            send_signal(9, 0, fault_addr.bits(), regs.rip, regs);  // SIGKILL
            MmFaultResult::OutOfMemory
        }
        MmFaultResult::BusError => {
            // File-backed fault past EOF: SIGBUS with si_code BUS_ADRERR (2)
            send_signal(7, 2, fault_addr.bits(), regs.rip, regs);
            MmFaultResult::BusError
        }
        // KernelPanic is not produced by handle_mm_fault
        MmFaultResult::KernelPanic => {
            bad_area(regs, access_type, fault_addr)
        }
    }
}
