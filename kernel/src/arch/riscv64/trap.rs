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
use crate::fs::procfs::interrupts;

// Include trap.S assembly code
core::arch::global_asm!(include_str!("trap.S"));

/// Get pt_regs for current task
///
/// Linux-style: pt_regs is always at (kernel_stack_top - sizeof(pt_regs))
/// This is more reliable than using ti_kernel_sp, which can get stale
/// when a task is preempted in kernel mode.
pub fn current_task_pt_regs() -> Option<&'static mut PtRegs> {
    use crate::sched::current;
    use crate::process::task::Task;

    unsafe {
        let task = current()?;

        // Get kernel stack top (Linux: task_stack_page(tsk) + THREAD_SIZE)
        let stack_top = (*task).get_kernel_stack()?;
        let stack_top_addr = stack_top as u64;

        // pt_regs is at stack_top - sizeof(PtRegs)
        // Linux: (struct pt_regs *)(task_stack_page(tsk) + THREAD_SIZE - sizeof(struct pt_regs))
        let pt_regs_ptr = (stack_top_addr - PT_REGS_SIZE as u64) as *mut PtRegs;

        Some(&mut *pt_regs_ptr)
    }
}

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

        // Initialize sscratch to 0 for kernel mode
        // When kernel is running, sscratch should be 0 so that on trap:
        //   csrrw tp, sscratch, tp  ->  tp becomes 0
        //   beqz tp, .Lfrom_kernel  ->  taken, correct path
        // When switching to user mode, sscratch will be set to current task
        asm!(
            "csrw sscratch, zero",
            options(nomem, nostack)
        )
    }
}

pub fn init_syscall() {
    // RISC-V uses ecall instruction, dispatched in exception handler
}

pub fn enable_timer_interrupt() {
    // Step 1: Enable STIE (bit 5 in sie) using atomic bit set
    unsafe {
        asm!(
            "li t0, 0x20",      // STIE bit (bit 5)
            "csrs sie, t0",     // Atomic bit set
            options(nomem, nostack)
        );
    }

    // Step 2: Set the timer trigger
    crate::drivers::timer::set_next_trigger();

    // Step 3: Enable global interrupts (sstatus.SIE = 1) if not already enabled
    unsafe {
        asm!(
            "csrsi sstatus, 2", // Set SIE bit (bit 1)
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
        // Enable external interrupt (SEIE bit) - use csrs to preserve other bits
        asm!(
            "li t0, 512",          // SEIE bit (2^9)
            "csrs sie, t0",        // Set SEIE bit without clearing other bits
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

        crate::pr_debug!("trap: cause={:?}, epc={:#x}, sp={:#x}, tp={:#x}, mode={}",
            cause, regs_ref.epc, regs_ref.sp, regs_ref.tp,
            if regs_ref.user_mode() { "user" } else { "kernel" });

        match cause {
            // Timer interrupt
            Cause::SupervisorTimer => {
                crate::interrupt::preempt::irq_enter();
                handle_timer_interrupt(regs_ref);
                crate::interrupt::preempt::irq_exit();
            }

            // Software interrupt (IPI)
            Cause::SupervisorSoft => {
                crate::interrupt::preempt::irq_enter();
                handle_software_interrupt(regs_ref);
                crate::interrupt::preempt::irq_exit();
            }

            // External interrupt
            Cause::SupervisorExternal => {
                crate::interrupt::preempt::irq_enter();
                handle_external_interrupt(regs_ref);
                crate::interrupt::preempt::irq_exit();
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
fn handle_timer_interrupt(_regs: &mut PtRegs) {
    crate::pr_debug!("trap: timer interrupt on cpu {}", crate::arch::cpu_id());

    // Increment interrupt counter for /proc/interrupts
    let cpu = crate::arch::cpu_id() as usize;
    interrupts::timer_inc(cpu);

    // Clear the timer interrupt pending bit by setting a new stimecmp value
    // With sstc extension, we can clear STIP by writing to stimecmp
    unsafe {
        core::arch::asm!(
            "csrw stimecmp, {0}",
            in(reg) 0xFFFFFFFFFFFFFFFFu64,
            options(nomem, nostack)
        );
    }

    // 1. Update jiffies
    crate::drivers::timer::timer_interrupt_handler();

    // 2. Set next timer interrupt
    crate::drivers::timer::set_next_trigger();

    // 3. TODO: Call scheduler tick
    // crate::sched::scheduler_tick();

    // 4. TODO: Check if reschedule needed
    // if crate::sched::need_resched() && !crate::sync::is_locked() {
    //     crate::sched::schedule();
    // }
}

/// Handle software interrupt (IPI)
fn handle_software_interrupt(_regs: &mut PtRegs) {
    let hart_id = crate::arch::riscv64::smp::cpu_id();

    // Increment software interrupt counter for /proc/interrupts
    interrupts::soft_inc(hart_id as usize);

    // Clear software interrupt
    unsafe {
        core::arch::asm!("csrc sip, 0x2", options(nomem, nostack));
    }

    // Handle IPI
    crate::arch::ipi::handle_software_ipi(hart_id as usize);
}

/// Handle external interrupt
///
/// Claims the highest-priority pending IRQ from PLIC and dispatches
/// through the IRQ framework. EOI (PLIC complete) is done by the
/// flow handler via irq_chip.irq_eoi.
fn handle_external_interrupt(_regs: &mut PtRegs) {
    let hart_id = crate::arch::riscv64::smp::cpu_id() as usize;

    if let Some(hwirq) = crate::drivers::intc::plic::claim(hart_id) {
        if let Some(domain) = crate::interrupt::get_default_domain() {
            crate::interrupt::generic_handle_domain_irq(domain, hwirq as u32);
        } else {
            // No domain registered — fall back to direct complete
            crate::drivers::intc::plic::complete(hart_id, hwirq);
        }
    }
}

/// Handle system call
fn handle_syscall(regs: &mut PtRegs) {
    let orig_epc = regs.epc;
    let syscall_num = regs.a7;  // syscall number is in a7, not orig_a0!

    crate::pr_debug!("trap: ecall from user, nr={}, epc={:#x}", syscall_num, orig_epc);

    // Default return value is -ENOSYS
    regs.a0 = crate::errno::constants::ENOSYS as u64;

    // Skip ecall instruction
    // RISC-V has both 32-bit ecall and 16-bit c.ecall instructions
    // Check if the instruction is compressed (lowest 2 bits != 11)
    let instr_size = if orig_epc % 4 == 0 {
        4 // 32-bit instruction
    } else {
        // Read the instruction to check if it's compressed
        let instr16: u16;
        unsafe {
            let ptr = orig_epc as *const u16;
            instr16 = core::ptr::read_volatile(ptr);
        }
        if (instr16 & 0x3) != 0x3 {
            2 // 16-bit compressed instruction
        } else {
            4 // 32-bit instruction
        }
    };
    regs.epc = orig_epc + instr_size;

    // Call syscall handler
    crate::syscall::syscall_handler(regs);
}

/// Handle illegal instruction
///
/// Linux-compatible: check for FPU first-use before terminating.
/// When sstatus.FS = OFF, any FP instruction causes IllegalInstruction.
/// We detect this case and enable FPU lazily (set FS = INITIAL),
/// then retry the instruction.
fn handle_illegal_instruction(regs: &mut PtRegs) {
    let epc = regs.epc;

    // Read the instruction to determine size
    let instr16: u16;
    unsafe {
        let ptr16 = epc as *const u16;
        instr16 = core::ptr::read_unaligned(ptr16);
    }

    // Check if this is a compressed (16-bit) instruction
    let is_compressed = (instr16 & 0x3) != 0x3;
    let instr_size = if is_compressed { 2 } else { 4 };

    // Check if FPU is disabled (FS = OFF) and this might be an FP instruction
    const SR_FS: u64 = 0x3 << 13;
    const SR_FS_INITIAL: u64 = 0x1 << 13;
    let fs = regs.status & SR_FS;
    if fs == 0 {
        // FPU is off - check if this is an FP instruction
        let is_fp = if is_compressed {
            // Compressed FP instructions on RV64 with D extension:
            // Quadrant 0 (bits[1:0]=00): C.FLD (funct3=001), C.FSD (funct3=101)
            // Quadrant 2 (bits[1:0]=10): C.FLDSP (funct3=001), C.FSDSP (funct3=101)
            // So for any compressed inst: funct3 (bits[15:13]) = 001 or 101 means FP
            let funct3 = (instr16 >> 13) & 0x7;
            funct3 == 1 || funct3 == 5
        } else {
            // 32-bit FP instructions:
            // Load/Store: opcode[6:0] = 0000111 (FLW/FLD) or 0100111 (FSW/FSD)
            // FP compute: opcode[6:0] = 0000101 (FMADD etc) or 0001001 (FMSUB etc)
            //             or 0001101 (FNMSUB etc) or 0001110 (FNMADD etc) or 1010011 (FP ops)
            let instr32: u32 = if instr_size == 4 {
                unsafe {
                    let ptr32 = epc as *const u32;
                    core::ptr::read_unaligned(ptr32)
                }
            } else {
                instr16 as u32
            };
            let opcode = instr32 & 0x7F;
            opcode == 0x07 || opcode == 0x27 ||   // FLW/FLD, FSW/FSD
            opcode == 0x05 || opcode == 0x09 ||   // FMADD, FMSUB
            opcode == 0x0D || opcode == 0x0E ||   // FNMSUB, FNMADD
            opcode == 0x53                         // FP ops (FADD, FSUB, FMUL, FDIV, etc.)
        };

        if is_fp {
            // Enable FPU by setting FS = INITIAL in pt_regs
            // The instruction will be retried automatically
            regs.status = (regs.status & !SR_FS) | SR_FS_INITIAL;
            return;
        }
    }

    // Not an FP instruction or FPU already enabled - terminate the process
    crate::pr_debug!("trap: illegal instruction at epc={:#x}, mode={}",
        epc, if regs.user_mode() { "user" } else { "kernel" });

    if let Some(current) = crate::sched::current() {
        current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
        crate::sync::kernel_lock_release();
        crate::sched::schedule();
    }

    regs.epc += instr_size;
}

/// Handle breakpoint
fn handle_breakpoint(regs: &mut PtRegs) {
    if regs.user_mode() {
        // Send SIGTRAP or terminate process
        if let Some(current) = crate::sched::current() {
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
/// Delegate to mm::exception::do_page_fault for complete handling
fn handle_page_fault(regs: &mut PtRegs, access_type: u32) {
    use crate::arch::riscv64::mm::exception::{do_page_fault, MmFaultResult};

    let fault_addr = regs.badaddr;

    crate::pr_debug!("trap: page fault addr={:#x}, epc={:#x}, type={}, mode={}",
        fault_addr, regs.epc, access_type,
        if regs.kernel_mode() { "kernel" } else { "user" });

    let result = do_page_fault(regs, access_type);

    match result {
        MmFaultResult::Handled | MmFaultResult::Fixed => {
            // Page handled, re-execute instruction
        }
        MmFaultResult::Segfault => {
            crate::pr_err!("pagefault: Segfault at {:#x}, epc={:#x}, mode={}",
                fault_addr, regs.epc, if regs.kernel_mode() { "kernel" } else { "user" });
            // Terminate user process
            if regs.user_mode() {
                if let Some(current) = crate::sched::current() {
                    current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
                }
            }
            crate::sync::kernel_lock_release();
            crate::sched::schedule();
        }
        MmFaultResult::PermissionDenied => {
            crate::pr_err!("pagefault: Permission denied at {:#x}", fault_addr);
            // Terminate user process
            if regs.user_mode() {
                if let Some(current) = crate::sched::current() {
                    current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
                }
            }
            crate::sync::kernel_lock_release();
            crate::sched::schedule();
        }
        MmFaultResult::OutOfMemory => {
            crate::pr_err!("pagefault: Out of memory at {:#x}", fault_addr);
            // Terminate user process
            if regs.user_mode() {
                if let Some(current) = crate::sched::current() {
                    current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
                }
            }
            crate::sync::kernel_lock_release();
            crate::sched::schedule();
        }
        MmFaultResult::KernelPanic => {
            crate::pr_emerg!("trap: Kernel panic - page fault at {:#x}", fault_addr);
            crate::pr_emerg!("  epc={:#x}, sp={:#x}", regs.epc, regs.sp);
            crate::pr_emerg!("  ra={:#x}, s0={:#x}", regs.ra, regs.s0);
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
    crate::pr_debug!("trap: unknown exception {:?}, epc={:#x}, badaddr={:#x}",
        cause, regs.epc, regs.badaddr);
    crate::pr_err!("trap: Unknown exception: {:?}, epc={:#x}, badaddr={:#x}",
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

/// Debug function to print clone regs (called from assembly)
#[no_mangle]
pub extern "C" fn debug_print_clone_regs(_s1: u64, _sp: u64, _a7: u64, _epc: u64) {
    // Debug output disabled
}

/// Debug function called before schedule() in trap.S
/// Arguments: a0 = sp, a1 = tp, a2 = ti_kernel_sp
#[no_mangle]
pub extern "C" fn debug_before_schedule(_sp: u64, _tp: u64, _ti_kernel_sp: u64) {
    // Debug disabled
}

/// Debug function called after schedule() returns in trap.S
/// Arguments: a0 = sp, a1 = tp, a2 = ti_kernel_sp
#[no_mangle]
pub extern "C" fn debug_after_schedule(_sp: u64, _tp: u64, _ti_kernel_sp: u64) {
    // Debug disabled
}

/// Debug function to verify sepc was written correctly
/// Arguments: a0 = actual sepc value, a1 = expected value
#[no_mangle]
pub extern "C" fn debug_sepc_verify(_actual: u64, _expected: u64) {
    // Debug disabled
}

/// Debug function called at trap entry
/// Arguments: a0 = pt_regs location, a1 = cause, a2 = tp
#[no_mangle]
pub extern "C" fn debug_trap_entry(_regs_ptr: u64, _cause: u64, _tp: u64) {
    // Debug disabled
}

/// Debug function to print trap exit info (called from assembly)
#[no_mangle]
pub extern "C" fn debug_trap_exit(_sp: u64, _tp: u64) {
    // Debug disabled
}

// ============================================================================
// ret_from_fork functions (Linux-style)
// ============================================================================

/// ret_from_fork_user - Called when a forked child returns to user mode
///
/// This is called from assembly ret_from_fork_user_asm after schedule_tail.
/// The child process will return to user space via ret_from_exception.
///
/// Reference: Linux arch/riscv/kernel/process.c:219-222
///
/// # Arguments
/// - `regs`: Pointer to the child's pt_regs (already set up by copy_thread)
#[no_mangle]
pub extern "C" fn ret_from_fork_user(_regs: *mut PtRegs) {
    // Called from assembly after schedule_tail
    // Child process returns to user mode via ret_from_exception
}

/// ret_from_fork_kernel - Called when a kernel thread starts execution
///
/// This is called from assembly ret_from_fork_kernel_asm after schedule_tail.
/// Kernel threads call their function and then exit.
///
/// Reference: Linux arch/riscv/kernel/process.c:212-217
///
/// # Arguments
/// - `fn_arg`: Argument to pass to the kernel thread function
/// - `fn_ptr`: Kernel thread function pointer
/// - `regs`: Pointer to pt_regs (for returning to user mode after thread exits)
#[no_mangle]
pub extern "C" fn ret_from_fork_kernel(fn_arg: *mut core::ffi::c_void,
                                       fn_ptr: extern "C" fn(*mut core::ffi::c_void) -> i32,
                                       _regs: *mut PtRegs) {
    // Call the kernel thread function
    let _ret = fn_ptr(fn_arg);

    // Kernel thread has finished, call do_exit
    crate::process::exit::do_exit(_ret);
}