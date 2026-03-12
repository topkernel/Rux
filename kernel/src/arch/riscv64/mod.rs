//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit architecture support
//!
//! Supports RISC-V 64-bit (RV64GC) architecture

pub mod boot;
pub mod pt_regs;
pub mod trap;
pub mod context;
pub mod cpu;
// syscall module has moved to kernel/src/syscall/
pub mod mm;
pub mod smp;
pub mod ipi;
pub mod process;
pub mod thread;
pub mod uaccess;

use crate::println;
use core::arch::asm;



pub fn arch_init() {
    init();
}

pub fn init() {
    println!("arch: Initializing RISC-V architecture...");

    // Set up exception vector table
    trap::init();

    // Disable interrupts
    unsafe {
        // RISC-V: Clear mstatus.MIE (Machine Interrupt Enable)
        let mut mstatus: u64;
        asm!("csrrw {}, mstatus, zero", out(reg) mstatus);
        mstatus &= !(1 << 3); // Clear MIE
        asm!("csrw mstatus, {}", in(reg) mstatus);

        println!("arch: Interrupts disabled in machine mode");
    }

    // Print CPU info
    print_cpu_info();

    println!("arch: Architecture initialization [DONE]");
}

fn print_cpu_info() {
    unsafe {
        // Read mhartid (hardware thread ID)
        let mhartid: u64;
        asm!("csrrw {}, mhartid, zero", out(reg) mhartid);

        // Read mimpid (machine implementation ID)
        let mimpid: u64;
        asm!("csrrw {}, mimpid, zero", out(reg) mimpid);

        // Read marchid (architecture ID)
        let marchid: u64;
        asm!("csrrw {}, marchid, zero", out(reg) marchid);

        println!("arch: mhartid (HART ID) = {}", mhartid);
        println!("arch: mimpid (Impl ID) = {:#x}", mimpid);
        println!("arch: marchid (Arch ID) = {:#x}", marchid);
    }
}

pub fn enable_interrupts() {
    unsafe {
        // Set mstatus.MIE (Machine Interrupt Enable)
        let mut mstatus: u64;
        asm!("csrrw {}, mstatus, zero", out(reg) mstatus);
        mstatus |= 1 << 3; // Set MIE
        asm!("csrw mstatus, {}", in(reg) mstatus);

        println!("arch: Machine-mode interrupts enabled");
    }
}

/// Get current CPU (hart) ID
///
/// Design:
/// - Early boot phase: tp = hart_id (small value)
/// - After scheduler runs: tp = task_struct pointer, hart_id stored in task_struct.ti_cpu
///
/// Determine current mode by checking tp value range:
/// - If tp < 0x1000, consider it as hart_id (early boot)
/// - Otherwise consider it as task_struct pointer
///
/// In S-mode, we cannot access mhartid CSR (only accessible from M-mode).
pub fn cpu_id() -> u64 {
    unsafe {
        let tp_value: u64;
        asm!("mv {}, tp", out(reg) tp_value, options(nomem, nostack, pure));

        // Check if tp is a small value (hart_id during early boot phase)
        // Valid task_struct pointers should be in kernel address space (>= 0x80000000)
        if tp_value < 0x1000 {
            // Early boot phase, tp directly stores hart_id
            tp_value
        } else {
            // tp points to task_struct, get hart_id from ti_cpu field
            // ti_cpu offset in Task struct is 0x18 (24 bytes)
            let ti_cpu_offset = 0x18;
            let cpu_ptr = (tp_value as usize + ti_cpu_offset) as *const core::sync::atomic::AtomicI32;
            (*cpu_ptr).load(core::sync::atomic::Ordering::Relaxed) as u64
        }
    }
}
