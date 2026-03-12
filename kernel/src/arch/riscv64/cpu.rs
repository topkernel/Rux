//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

/// CPU-related operations (RISC-V 64-bit)
use core::arch::asm;

/// Get current core ID (hart ID)
#[inline]
pub fn get_core_id() -> u64 {
    let hart_id: u64;
    unsafe {
        core::arch::asm!("csrrw {}, mhartid, zero", out(reg) hart_id, options(nomem, nostack, pure));
    }
    hart_id
}

/// Get current thread ID
#[inline]
pub fn get_thread_id() -> u64 {
    // RISC-V uses tp register (x4) to store thread pointer
    let tp: u64;
    unsafe {
        core::arch::asm!("mv {}, tp", out(reg) tp, options(nomem, nostack, pure));
    }
    tp
}

/// Set current thread ID
#[inline]
pub fn set_thread_id(tid: u64) {
    unsafe {
        core::arch::asm!("mv tp, {}", in(reg) tid, options(nomem, nostack));
    }
}

/// Get counter frequency (RISC-V uses time CSR)
#[inline]
pub fn get_counter_freq() -> u64 {
    // QEMU virt platform default frequency: 10 MHz
    10_000_000
}

/// Read counter (time CSR)
#[inline]
pub fn read_counter() -> u64 {
    let time: u64;
    unsafe {
        core::arch::asm!("csrrw {}, time, zero", out(reg) time, options(nomem, nostack, pure));
    }
    time
}

/// Enable interrupts
#[inline]
pub fn enable_irq() {
    unsafe {
        // Set sstatus.SIE (Supervisor Interrupt Enable) bit
        let mut sstatus: u64;
        asm!("csrrs {}, sstatus, zero", out(reg) sstatus);
        sstatus |= 1 << 1; // SIE bit
        asm!("csrw sstatus, {}", in(reg) sstatus);
    }
}

/// Disable interrupts
#[inline]
pub fn disable_irq() {
    unsafe {
        // Clear sstatus.SIE (Supervisor Interrupt Enable) bit
        let mut sstatus: u64;
        asm!("csrrs {}, sstatus, zero", out(reg) sstatus);
        sstatus &= !(1 << 1); // SIE bit
        asm!("csrw sstatus, {}", in(reg) sstatus);
    }
}

/// Wait for interrupt
#[inline]
pub fn wfi() {
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack));
    }
}

/// Instruction serialization barrier
#[inline]
pub fn isb() {
    unsafe {
        core::arch::asm!("fence.i", options(nomem, nostack));
    }
}

/// Data synchronization barrier
#[inline]
pub fn dsb() {
    unsafe {
        core::arch::asm!("fence", options(nomem, nostack));
    }
}

/// Data memory barrier
#[inline]
pub fn dmb() {
    unsafe {
        core::arch::asm!("fence", options(nomem, nostack));
    }
}

/// Get interrupt mask state
#[inline]
pub fn get_interrupts_state() -> bool {
    let sstatus: u64;
    unsafe {
        asm!("csrrs {}, sstatus, zero", out(reg) sstatus, options(nomem, nostack, pure));
    }
    // sstatus.SIE bit (bit 1)
    (sstatus & (1 << 1)) != 0
}

/// Save interrupt state and disable interrupts
#[inline]
pub fn save_and_disable_irq() -> bool {
    let state = get_interrupts_state();
    disable_irq();
    state
}

/// Restore interrupt state
#[inline]
pub fn restore_irq(state: bool) {
    if state {
        enable_irq();
    }
}
