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
fn handle_timer_interrupt(_regs: &mut PtRegs) {
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
fn handle_external_interrupt(_regs: &mut PtRegs) {
    let hart_id = crate::arch::riscv64::smp::cpu_id();

    if let Some(irq) = crate::drivers::intc::plic::claim(hart_id as usize) {
        // Increment PLIC interrupt counter for /proc/interrupts
        interrupts::plic_inc(irq as usize, hart_id as usize);

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
    // Debug: Check if child's L1 table is shared with kernel
    if let Some(current) = crate::sched::current() {
        let pid = (*current).pid();
        if pid == 2 {
            if let Some(aspace) = (*current).address_space() {
                let root_ppn = aspace.pgd;
                unsafe {
                    let root_addr = root_ppn << 12;
                    let root_table = root_addr as *const u64;

                    // Check L2[2] - should point to kernel's L1 table
                    let pte2_2 = core::ptr::read(root_table.add(2));
                    if pte2_2 & 0x1 != 0 {
                        let ppn1_shared = pte2_2 >> 10;
                        let l1_shared = (ppn1_shared << 12) as *const u64;

                        // Check L1[0x3f] - should point to kernel's L0 table for identity mapping
                        let pte1_3f = core::ptr::read(l1_shared.add(0x3f));
                        if pte1_3f & 0x1 != 0 {
                            let ppn0_shared = pte1_3f >> 10;
                            let l0_shared = (ppn0_shared << 12) as *const u64;

                            // Check L0[0xec] - maps virtual 0x87eec000
                            let pte0_ec = core::ptr::read(l0_shared.add(0xec));

                            let syscall_num = regs.a7;
                            if syscall_num == 96 {  // Only print once for first syscall
                                crate::println!("CHILD PAGE TABLE: root_ppn={:#x}", root_ppn);
                                crate::println!("  L2[2]={:#x} -> L1 at {:#x}", pte2_2, ppn1_shared << 12);
                                crate::println!("  L1[0x3f]={:#x} -> L0 at {:#x}", pte1_3f, ppn0_shared << 12);
                                crate::println!("  L0[0xec]={:#x} (maps vaddr 0x87eec000)", pte0_ec);
                                crate::println!("  Child's own L0 table is at phys {:#x}", 0x87eec000u64);

                                // The bug: if L0[0xec] maps 0x87eec000 to 0x87eec000,
                                // then writing to vaddr 0x87eec000 will corrupt child's L0 table!
                                if (pte0_ec >> 10) << 12 == 0x87eec000u64 && pte0_ec & 0x1 != 0 {
                                    crate::println!("!!! BUG CONFIRMED: Identity mapping overlaps with child's L0 table!");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let orig_epc = regs.epc;

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

    // Debug: Check L0[0x1f] AFTER syscall to detect user-mode corruption
    if let Some(current) = crate::sched::current() {
        let pid = (*current).pid();
        if pid > 1 {
            if let Some(aspace) = (*current).address_space() {
                let root_ppn = aspace.pgd;
                unsafe {
                    let root_addr = root_ppn << 12;
                    let root_table = root_addr as *const u64;
                    let pte2 = core::ptr::read(root_table);
                    if pte2 & 0x1 != 0 {
                        let ppn1 = pte2 >> 10;
                        let l1_addr = ppn1 << 12;
                        let l1_table = l1_addr as *const u64;
                        let pte1 = core::ptr::read(l1_table);
                        if pte1 & 0x1 != 0 {
                            let ppn0 = pte1 >> 10;
                            let l0_addr = ppn0 << 12;
                            let l0_table = l0_addr as *const u64;
                            let l0_pte_1f = core::ptr::read(l0_table.add(0x1f));
                            let syscall_num = regs.a7;
                            if l0_pte_1f != 0x21fbf9db {
                                crate::println!("!!! CORRUPTION AFTER syscall: PID {} syscall={} ret={} L0[0x1f]={:#x}",
                                    pid, syscall_num, regs.a0, l0_pte_1f);

                                // Check if any L0 entry maps to the L0 table's physical address
                                // This would be a circular mapping bug
                                let l0_ppn = l0_addr >> 12;
                                for i in 0..512 {
                                    let pte = core::ptr::read(l0_table.add(i));
                                    if pte & 0x1 != 0 { // Valid entry
                                        let entry_ppn = pte >> 10;
                                        if entry_ppn == l0_ppn {
                                            crate::println!("!!! BUG: L0[{:#x}] maps to L0 table itself! PTE={:#x}",
                                                i, pte);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Handle illegal instruction
fn handle_illegal_instruction(regs: &mut PtRegs) {
    let epc = regs.epc;
    let sstatus = regs.status;
    let badaddr = regs.badaddr;

    crate::println!("illegal_instr: epc={:#x} (aligned: {}), sstatus={:#x}, badaddr={:#x}",
        epc, epc % 4, sstatus, badaddr);

    // Read the instruction - handle both 16-bit and 32-bit instructions
    // First read 16 bits to check if it's a compressed instruction
    // Use read_unaligned because epc may not be properly aligned
    let instr16: u16;
    let instr32: u32;
    unsafe {
        let ptr16 = epc as *const u16;
        instr16 = core::ptr::read_unaligned(ptr16);
        // Also read 32 bits for comparison (may span cache lines)
        let ptr32 = epc as *const u32;
        instr32 = core::ptr::read_unaligned(ptr32);
    }

    crate::println!("illegal_instr: instr16={:#06x}, instr32={:#010x}", instr16, instr32);

    // Check if this is a compressed (16-bit) instruction
    // Compressed instructions have the lowest 2 bits not equal to 11
    let is_compressed = (instr16 & 0x3) != 0x3;
    crate::println!("illegal_instr: is_compressed={}", is_compressed);

    // Check if sstatus.SUM bit is set (allows S-mode to access user memory)
    let sum_set = (sstatus & SR_SUM) != 0;
    crate::println!("illegal_instr: sstatus.SUM={}", sum_set);

    // Check current satp (page table)
    let satp: u64;
    unsafe {
        asm!("csrr {0}, satp", out(reg) satp, options(nomem, nostack));
    }
    crate::println!("illegal_instr: satp={:#x}", satp);

    // Check sie and sip
    let sie: u64;
    let sip: u64;
    unsafe {
        asm!("csrr {0}, sie", out(reg) sie, options(nomem, nostack));
        asm!("csrr {0}, sip", out(reg) sip, options(nomem, nostack));
    }
    crate::println!("illegal_instr: sie={:#x}, sip={:#x}", sie, sip);

    // Terminate the process
    if let Some(current) = crate::sched::current() {
        crate::println!("illegal_instr: terminating PID {}", current.pid());
        current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
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

    // Debug: check mode before calling do_page_fault
    crate::println!("handle_page_fault: badaddr={:#x} status={:#x} kernel_mode={}",
        regs.badaddr, regs.status, regs.kernel_mode());

    let fault_addr = regs.badaddr;
    let result = do_page_fault(regs, access_type);

    match result {
        MmFaultResult::Handled | MmFaultResult::Fixed => {
            // Page handled, re-execute instruction
        }
        MmFaultResult::Segfault => {
            // Debug: print tp register and sscratch
            let tp: u64;
            let sscratch: u64;
            unsafe {
                core::arch::asm!("mv {}, tp", out(reg) tp, options(nomem, nostack, pure));
                core::arch::asm!("csrr {}, sscratch", out(reg) sscratch, options(nomem, nostack, pure));
            }
            crate::println!("pagefault: Segfault at {:#x}, epc={:#x}, mode={}",
                fault_addr, regs.epc, if regs.kernel_mode() { "kernel" } else { "user" });
            crate::println!("pagefault: tp={:#x}, sscratch={:#x}", tp, sscratch);
            // Debug: print user's sp, tp, ra from PtRegs
            crate::println!("pagefault: PtRegs.sp={:#x}, PtRegs.tp={:#x}, PtRegs.ra={:#x}",
                regs.sp, regs.tp, regs.ra);
            crate::println!("pagefault: PtRegs.gp={:#x}, PtRegs.status={:#x}",
                regs.gp, regs.status);
            // Print all argument registers
            crate::println!("pagefault: a0={:#x}, a1={:#x}, a2={:#x}, a3={:#x}",
                regs.a0, regs.a1, regs.a2, regs.a3);
            crate::println!("pagefault: a4={:#x}, a5={:#x}, a6={:#x}, a7={:#x}",
                regs.a4, regs.a5, regs.a6, regs.a7);
            // Print temp registers
            crate::println!("pagefault: t0={:#x}, t1={:#x}, t2={:#x}, t3={:#x}",
                regs.t0, regs.t1, regs.t2, regs.t3);
            crate::println!("pagefault: t4={:#x}, t5={:#x}, t6={:#x}",
                regs.t4, regs.t5, regs.t6);
            // Print saved registers
            crate::println!("pagefault: s0={:#x}, s1={:#x}, s2={:#x}, s3={:#x}",
                regs.s0, regs.s1, regs.s2, regs.s3);
            // Terminate user process
            if regs.user_mode() {
                if let Some(current) = crate::sched::current() {
                    crate::println!("pagefault: current()={:#x}, PID {}", current as *const _ as u64, current.pid());
                    current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
                }
            }
            crate::sync::kernel_lock_release();
            crate::sched::schedule();
        }
        MmFaultResult::PermissionDenied => {
            crate::println!("pagefault: Permission denied at {:#x}", fault_addr);
            // Terminate user process
            if regs.user_mode() {
                if let Some(current) = crate::sched::current() {
                    crate::println!("pagefault: terminating PID {}", current.pid());
                    current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
                }
            }
            crate::sync::kernel_lock_release();
            crate::sched::schedule();
        }
        MmFaultResult::OutOfMemory => {
            crate::println!("pagefault: Out of memory at {:#x}", fault_addr);
            // Terminate user process
            if regs.user_mode() {
                if let Some(current) = crate::sched::current() {
                    crate::println!("pagefault: terminating PID {}", current.pid());
                    current.set_state(crate::process::task::TaskState::new(TaskState::ZOMBIE));
                }
            }
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
