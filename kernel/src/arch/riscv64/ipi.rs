//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V IPI (Inter-Processor Interrupt) support
//!
//! - smp_cross_call() - Send cross-CPU call
//! - handle_IPI() - Handle IPI
//!
//! IPI types:
//! - RESCHEDULE: Notify target CPU to reschedule (when new task or load balancing)
//! - STOP: Stop target CPU
//!
//! Use RISC-V software interrupt (SSIP) and SBI IPI Extension (EID #0x735049)

use crate::sbi;
use crate::println;

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum IpiType {
    /// Reschedule
    Reschedule = 0,
    /// Stop CPU
    Stop = 1,
}

/// Send Reschedule IPI to specified CPU
///
/// When a CPU has a new task or needs load balancing,
/// send this IPI to notify the target CPU to reschedule.
///
///
/// # Arguments
/// * `target_cpu` - Target CPU ID
pub fn send_reschedule_ipi(target_cpu: usize) {
    if target_cpu >= 4 {
        return;
    }

    // Don't send to self
    let current_cpu = crate::arch::cpu_id() as usize;
    if target_cpu == current_cpu {
        return;
    }

    // Send IPI via SBI
    let _ = sbi::send_ipi(target_cpu);
}

/// Handle software interrupt IPI
///
/// Called when software interrupt is received.
/// Notifies scheduler to reschedule.
///
///
/// # Arguments
/// * `hart` - Current hart ID
pub fn handle_software_ipi(hart: usize) {
    // Handle IPI - trigger scheduler
    // When another CPU sends Reschedule IPI, it means scheduling is needed
    // e.g.: woken up high priority task, load balancing needed, etc.

    #[cfg(feature = "riscv64")]
    {
        // Set need reschedule flag
        crate::sched::set_need_resched();

        // Schedule immediately
        crate::sched::schedule();
    }

    // println!("ipi: Hart {} received reschedule IPI", hart);
}

/// Handle PLIC IPI (IRQ handler registered via request_irq)
///
/// Called by the IRQ framework for IRQs 11-13.
fn ipi_irq_handler(irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    let hart = crate::arch::cpu_id() as usize;
    match irq {
        11 => {
            handle_software_ipi(hart);
        }
        12 | 13 => {
            loop {
                unsafe {
                    core::arch::asm!("wfi", options(nomem, nostack));
                }
            }
        }
        _ => {}
    }
    crate::interrupt::IrqReturn::Handled
}

/// Register IPI handlers via the IRQ framework.
/// Called during init after the PLIC domain is created.
pub fn register_irq_handlers() {
    for irq in 11..14u32 {
        crate::interrupt::request_irq(
            irq,
            ipi_irq_handler,
            crate::interrupt::IRQF_SHARED,
            "IPI",
            0,
        ).ok();
    }
}

/// Legacy IPI handler (kept for compatibility with direct calls)
pub fn handle_ipi(irq: usize, hart: usize) {
    match irq {
        11 => {
            handle_software_ipi(hart);
        }
        12 | 13 => {
            loop {
                unsafe {
                    core::arch::asm!("wfi", options(nomem, nostack));
                }
            }
        }
        _ => {}
    }
}

/// Initialize IPI support
///
/// Enable software interrupt (SSIP)
pub fn init() {
    // Enable software interrupt
    unsafe {
        // Set SSIE bit (bit 1) in sie register
        core::arch::asm!(
            "csrsi sie, 2",  // Set bit 1 (SSIE = 0x2)
            options(nomem, nostack)
        );
    }

    // Register IPI handlers via IRQ framework
    register_irq_handlers();
}
