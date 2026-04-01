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

use crate::arch::riscv64::pt_regs::PtRegs;
use crate::arch::riscv64::mm::{VirtAddr, FaultFlags, AddressSpace, handle_cow_fault, handle_mm_fault};
use crate::println;
use crate::process::task::TaskState;
use crate::mm::vma::VmaFlags;

/// Page fault handling result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmFaultResult {
    /// Handled successfully, can retry instruction
    Handled,
    /// Address not in any VMA (segmentation fault)
    Segfault,
    /// Insufficient permissions (protection fault)
    PermissionDenied,
    /// Out of memory
    OutOfMemory,
    /// Already mapped (no handling needed)
    AlreadyMapped,
    /// COW in progress (handled by handle_cow_fault)
    CowPending,
    /// Kernel exception fixed (via exception table)
    Fixed,
    /// Unfixable kernel exception
    KernelPanic,
}

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
/// Uses linear search to find matching instruction address in exception table.
/// If found, returns fixup address; otherwise returns None.
///
/// # Arguments
/// - `addr`: Instruction address where exception occurred (usually EPC value)
///
/// # Returns
/// - `Some(fixup_addr)`: Found fixup address
/// - `None`: No matching entry found
///
/// # Performance
/// Linear search O(n), but exception table is usually small (tens to hundreds of entries),
/// performance impact is acceptable. Can use binary search for optimization (requires sorted table).
pub fn fixup_exception(addr: u64) -> Option<u64> {
    unsafe {
        let start = &__ex_table_start as *const ExceptionTableEntry;
        let end = &__ex_table_end as *const ExceptionTableEntry;

        // Calculate number of entries in table
        let count = (end as usize - start as usize) / core::mem::size_of::<ExceptionTableEntry>();

        // Linear search
        for i in 0..count {
            let entry = &*start.add(i);
            if entry.insn == addr {
                return Some(entry.fixup);
            }
        }
    }

    None
}

/// Check if exception table is empty
#[allow(dead_code)]
pub fn exception_table_empty() -> bool {
    unsafe {
        let start = &__ex_table_start as *const ExceptionTableEntry;
        let end = &__ex_table_end as *const ExceptionTableEntry;
        start == end
    }
}

/// Get exception table entry count
#[allow(dead_code)]
pub fn exception_table_count() -> usize {
    unsafe {
        let start = &__ex_table_start as *const ExceptionTableEntry;
        let end = &__ex_table_end as *const ExceptionTableEntry;
        (end as usize - start as usize) / core::mem::size_of::<ExceptionTableEntry>()
    }
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
fn send_signal(sig: i32, _code: i32, _addr: u64, _epc: u64, _access_type: u32, _regs: &crate::arch::riscv64::pt_regs::PtRegs) {
    // Send signal using real signal mechanism
    if let Some(current) = crate::sched::current() {
        let pid = current.pid();
        // Call signal module's send_signal function
        crate::signal::send_signal(pid, sig);

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
fn bad_area(regs: &mut PtRegs, access_type: u32, fault_addr: VirtAddr) -> MmFaultResult {
    // User mode accessing invalid address
    if regs.user_mode() {
        // Send SIGSEGV
        let sig = if access_type & FaultFlags::WRITE != 0 {
            11  // SIGSEGV
        } else if access_type & FaultFlags::EXEC != 0 {
            11  // SIGSEGV
        } else {
            11  // SIGSEGV
        };

        send_signal(sig, 1, fault_addr.bits(), regs.epc, access_type, regs);  // SEGV_MAPERR = 1
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
fn no_context(_regs: &mut PtRegs, _fault_addr: VirtAddr) -> MmFaultResult {
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
/// # Arguments
/// - `regs`: Trap frame/register state
/// - `access_type`: Access type (FaultFlags)
///
/// # Returns
/// Handling result
pub fn do_page_fault(regs: &mut PtRegs, access_type: u32) -> MmFaultResult {
    let fault_addr = VirtAddr::new(regs.badaddr);

    crate::pr_debug!("do_page_fault: addr={:#x}, epc={:#x}, type={:#x}, mode={}",
        fault_addr.bits(), regs.epc, access_type,
        if regs.kernel_mode() { "kernel" } else { "user" });

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
        // If fault address is near current task's kernel stack, it's likely a stack overflow
        let fault_addr_usize = fault_addr.bits() as usize;

        if current.is_in_kernel_stack(fault_addr_usize) || current.is_stack_overflow(regs.sp as usize) {
            panic!("Kernel stack overflow in task {} (fault_addr={:#x}, sp={:#x})",
                current.pid(), fault_addr_usize, regs.sp);
        }

        // Check exception table (copy_to_user/copy_from_user etc.)
        if let Some(fixup) = fixup_exception(regs.epc) {
            regs.epc = fixup;
            return MmFaultResult::Fixed;
        }

        // Kernel accessed invalid address (possibly a bug)
        return MmFaultResult::KernelPanic;
    }

    // User mode page fault handling

    // 1. Call handle_mm_fault to handle
    let result = handle_mm_fault(&addr_space, fault_addr, access_type | FaultFlags::USER);

    match result {
        crate::arch::riscv64::mm::MmFaultResult::Handled => {
            // Page mapped, can re-execute instruction
            return MmFaultResult::Handled;
        }
        crate::arch::riscv64::mm::MmFaultResult::CowPending => {
            // COW page, try copy-on-write
            match unsafe { handle_cow_fault(addr_space.root_ppn(), fault_addr) } {
                Some(()) => {
                    return MmFaultResult::Handled;
                }
                None => {
                    // COW failed, possibly out of memory
                    return MmFaultResult::OutOfMemory;
                }
            }
        }
        crate::arch::riscv64::mm::MmFaultResult::AlreadyMapped => {
            // Mapped but permission issue
            // Possibly writing to read-only page etc.
            send_signal(11, 2, fault_addr.bits(), regs.epc, access_type, regs);  // SIGSEGV, SEGV_ACCERR = 2
            return MmFaultResult::PermissionDenied;
        }
        crate::arch::riscv64::mm::MmFaultResult::Segfault => {
            // Address not in any VMA
            return bad_area(regs, access_type, fault_addr);
        }
        crate::arch::riscv64::mm::MmFaultResult::PermissionDenied => {
            // Insufficient permissions
            send_signal(11, 2, fault_addr.bits(), regs.epc, access_type, regs);  // SIGSEGV, SEGV_ACCERR = 2
            return MmFaultResult::PermissionDenied;
        }
        crate::arch::riscv64::mm::MmFaultResult::OutOfMemory => {
            // Out of memory, send SIGKILL
            send_signal(9, 0, fault_addr.bits(), regs.epc, access_type, regs);  // SIGKILL
            return MmFaultResult::OutOfMemory;
        }
    }
}
