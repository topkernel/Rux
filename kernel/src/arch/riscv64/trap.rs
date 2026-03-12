//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V exception handling
//!
//! Handle various exceptions and interrupts

use core::arch::asm;
use crate::process::task::TaskState;
use riscv::register::sie;

// Include trap.S assembly code
core::arch::global_asm!(include_str!("trap.S"));

// Re-export PtRegs and related constants
pub use super::pt_regs::{PtRegs, Cause, PT_REGS_SIZE};
pub use super::pt_regs::{SR_SPP, SR_PIE, SR_SIE, SR_SUM};

/// Current CPU's PtRegs pointer (used for fork)
static CURRENT_PT_REGS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Get current PtRegs pointer
/// Used for fork to copy parent's trap state
pub fn current_pt_regs() -> *const PtRegs {
    CURRENT_PT_REGS.load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs
}

/// Initialize trap handling
pub fn init() {
    unsafe {
        // Set stvec to point to trap_entry
        extern "C" {
            fn trap_entry();
        }

        let stvec_value = trap_entry as *const () as u64;
        asm!(
            "csrw stvec, {}",
            in(reg) stvec_value,
            options(nostack)
        );

        // Initialize sscratch to hart_id + 1
        let hart_id: u64;
        asm!(
            "mv {}, tp",
            out(reg) hart_id,
            options(nomem, nostack, pure)
        );
        let sscratch_value = hart_id + 1;

        asm!(
            "csrw sscratch, {}",
            in(reg) sscratch_value,
            options(nomem, nostack)
        )
    }
}

pub fn init_syscall() {
    // RISC-V uses ecall instruction, dispatched in exception handler
}

pub fn enable_timer_interrupt() {
    unsafe {
        asm!(
            "li t0, 32",           // STIE bit (2^5)
            "csrw sie, t0",
            options(nomem, nostack)
        );

        // Set SIE and SUM bits
        asm!(
            "csrsi sstatus, 2",      // SIE = 0x2
            "li t0, 262144",         // SUM = 0x40000
            "csrs sstatus, t0",
            options(nomem, nostack)
        );
    }
}

pub fn disable_timer_interrupt() {
    unsafe {
        sie::clear_stimer();
    }
}

pub fn enable_external_interrupt() {
    unsafe {
        asm!(
            "li t0, 512",          // SEIE bit (2^9)
            "csrw sie, t0",
            options(nomem, nostack)
        );

        asm!(
            "csrsi sstatus, 2",
            "li t0, 262144",
            "csrs sstatus, t0",
            options(nomem, nostack)
        );
    }
}

/// Trap handler
///
/// Called by trap.S with PtRegs pointer
#[no_mangle]
pub extern "C" fn trap_handler(regs: *mut PtRegs) {
    unsafe {
        // Save current PtRegs pointer (used for fork)
        CURRENT_PT_REGS.store(regs as u64, core::sync::atomic::Ordering::Relaxed);

        let regs_ref = &mut *regs;
        let cause = Cause::from_cause(regs_ref.cause);

        match cause {
            // Timer interrupt
            Cause::SupervisorTimer => {
                handle_timer_interrupt(regs_ref);
            }

            // Software interrupt (IPI)
            Cause::SupervisorSoft => {
                handle_software_interrupt(regs_ref);
            }

            // External interrupt
            Cause::SupervisorExternal => {
                handle_external_interrupt(regs_ref);
            }

            // User mode system call
            Cause::EcallUser => {
                handle_syscall(regs_ref);
            }

            // Illegal instruction
            Cause::IllegalInstruction => {
                if regs_ref.user_mode() {
                    handle_illegal_instruction(regs_ref);
                } else {
                    crate::println!("trap: Illegal instruction in kernel at epc={:#x}",
                        regs_ref.epc);
                    regs_ref.epc += 4;
                }
            }

            // Breakpoint
            Cause::Breakpoint => {
                handle_breakpoint(regs_ref);
            }

            // Instruction page fault
            Cause::InstructionPageFault => {
                handle_page_fault(regs_ref, crate::arch::riscv64::mm::FaultFlags::EXEC);
            }

            // Load page fault
            Cause::LoadPageFault => {
                handle_page_fault(regs_ref, crate::arch::riscv64::mm::FaultFlags::READ);
            }

            // Store page fault
            Cause::StoreAmoPageFault => {
                handle_page_fault(regs_ref, crate::arch::riscv64::mm::FaultFlags::WRITE);
            }

            // Other exceptions
            _ => {
                handle_unknown_exception(regs_ref, cause);
            }
        }

        // Clear current PtRegs pointer
        CURRENT_PT_REGS.store(0, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Handle timer interrupt
fn handle_timer_interrupt(regs: &mut PtRegs) {
    // Check if holding kernel big lock
    let is_locked = crate::sync::is_locked();

    // 1. Update jiffies
    crate::drivers::timer::timer_interrupt_handler();

    // 2. Scheduler tick
    crate::sched::scheduler_tick();

    // 3. Set next timer interrupt
    crate::drivers::timer::set_next_trigger();

    // 4. If reschedule needed and not holding kernel big lock
    // Cannot schedule when holding kernel big lock, otherwise lock state will be corrupted
    if crate::sched::need_resched() && !is_locked {
        // Save current state and schedule
        // Note: scheduling will modify regs, new process state will be restored on return
        crate::sched::schedule();
    }
}

/// Handle software interrupt (IPI)
fn handle_software_interrupt(_regs: &mut PtRegs) {
    let hart_id = crate::arch::riscv64::smp::cpu_id();

    // Clear software interrupt
    unsafe {
        core::arch::asm!("csrc sip, 0x2", options(nomem, nostack));
    }

    // Handle IPI
    crate::arch::ipi::handle_software_ipi(hart_id as usize);
}

/// Handle external interrupt
fn handle_external_interrupt(_regs: &mut PtRegs) {
    let hart_id = crate::arch::riscv64::smp::cpu_id();

    if let Some(irq) = crate::drivers::intc::plic::claim(hart_id as usize) {
        match irq {
            1..=8 => {
                // VirtIO MMIO device interrupt
                // First handle VirtIO-Blk
                crate::drivers::virtio::interrupt_handler();
                // Then handle VirtIO-Net
                crate::drivers::net::virtio_net::interrupt_handler();
            }
            32..=127 => {
                // VirtIO PCI device interrupt
                crate::drivers::virtio::interrupt_handler_pci(irq as usize);
            }
            10 => {
                // UART interrupt
            }
            11..=13 => {
                // IPI interrupt
                crate::arch::ipi::handle_ipi(irq, hart_id as usize);
            }
            _ => {
                // Unknown interrupt
            }
        }

        crate::drivers::intc::plic::complete(hart_id as usize, irq);
    }
}

/// Handle system call
fn handle_syscall(regs: &mut PtRegs) {
    // Save orig_a0 (already done in trap.S, just ensure here)
    // regs.orig_a0 already set in assembly

    let _syscall_num = regs.a7;
    let _orig_a0 = regs.a0;

    // Default return value is -ENOSYS
    regs.a0 = crate::errno::constants::ENOSYS as u64;

    // Skip ecall instruction
    regs.epc += 4;

    // Call syscall handler (using new syscall module)
    crate::syscall::syscall_handler(regs);
}

/// Handle illegal instruction
fn handle_illegal_instruction(regs: &mut PtRegs) {
    // Send SIGILL or terminate process
    if let Some(current) = crate::sched::current() {
        crate::println!("trap: Illegal instruction at epc={:#x}, terminating PID {}",
            regs.epc, current.pid());
        current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
        // Release kernel big lock before scheduling
        crate::sync::kernel_lock_release();
        crate::sched::schedule();
    }

    regs.epc += 4;
}

/// Handle breakpoint
fn handle_breakpoint(regs: &mut PtRegs) {
    if regs.user_mode() {
        // Send SIGTRAP or terminate process
        if let Some(current) = crate::sched::current() {
            crate::println!("trap: Breakpoint at epc={:#x}, terminating PID {}",
                regs.epc, current.pid());
            current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
            // Release kernel big lock before scheduling
            crate::sync::kernel_lock_release();
            crate::sched::schedule();
        }
    }

    regs.epc += 4;
}

/// Handle page fault
///
/// Delegate to mm::fault::do_page_fault for complete handling
fn handle_page_fault(regs: &mut PtRegs, access_type: u32) {
    use crate::arch::riscv64::mm::fault::{do_page_fault, MmFaultResult};

    let fault_addr = regs.badaddr;
    let result = do_page_fault(regs, access_type);

    match result {
        MmFaultResult::Handled | MmFaultResult::Fixed => {
            // Page handled, re-execute instruction
        }
        MmFaultResult::Segfault => {
            crate::println!("pagefault: Segfault at {:#x}, epc={:#x}, mode={}",
                fault_addr, regs.epc, if regs.kernel_mode() { "kernel" } else { "user" });
            crate::sync::kernel_lock_release();
            crate::sched::schedule();
        }
        MmFaultResult::PermissionDenied => {
            crate::println!("pagefault: Permission denied at {:#x}", fault_addr);
            crate::sync::kernel_lock_release();
            crate::sched::schedule();
        }
        MmFaultResult::OutOfMemory => {
            crate::println!("pagefault: Out of memory at {:#x}", fault_addr);
            crate::sync::kernel_lock_release();
            crate::sched::schedule();
        }
        MmFaultResult::KernelPanic => {
            crate::println!("trap: Kernel panic - page fault at {:#x}", fault_addr);
            #[cfg(debug_assertions)]
            loop {
                unsafe { core::arch::asm!("wfi") };
            }
        }
        _ => {}
    }
}

/// Handle unknown exception
fn handle_unknown_exception(regs: &mut PtRegs, cause: Cause) {
    crate::println!("trap: Unknown exception: {:?}, epc={:#x}, badaddr={:#x}",
        cause, regs.epc, regs.badaddr);

    if regs.user_mode() {
        // Terminate user process
        if let Some(current) = crate::sched::current() {
            current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
            // Release kernel big lock before scheduling
            crate::sync::kernel_lock_release();
            crate::sched::schedule();
        }
    }

    // Skip instruction
    regs.epc += 4;
}

// ============================================================================
// Compatibility: Keep old TrapFrame type alias
// ============================================================================

/// Old TrapFrame type alias (compatibility)
pub type TrapFrame = PtRegs;

/// Old ExceptionCause type alias (compatibility)
pub type ExceptionCause = Cause;

/// Get current TrapFrame pointer (compatibility)
#[deprecated(note = "Use current_pt_regs instead")]
pub fn current_trap_frame() -> *const TrapFrame {
    current_pt_regs()
}
