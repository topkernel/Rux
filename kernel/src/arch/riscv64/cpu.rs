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
    // SAFETY: mhartid is a read-only machine CSR; reading it via csrrw is always safe.
    unsafe {
        core::arch::asm!("csrrw {}, mhartid, zero", out(reg) hart_id, options(nomem, nostack, pure));
    }
    hart_id
}

/// Get current thread ID
#[inline]
pub fn get_thread_id() -> u64 {
    // RISC-V uses tp register (x4) to store thread pointer
    // SAFETY: reading tp is a simple register move, no memory or side effects.
    let tp: u64;
    unsafe {
        core::arch::asm!("mv {}, tp", out(reg) tp, options(nomem, nostack, pure));
    }
    tp
}

/// Set current thread ID
#[inline]
pub fn set_thread_id(tid: u64) {
    // SAFETY: writing tp (x4) is a simple register move; the value is used as a thread pointer.
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
    // SAFETY: time is a read-only supervisor CSR; reading it is always safe.
    unsafe {
        core::arch::asm!("csrrw {}, time, zero", out(reg) time, options(nomem, nostack, pure));
    }
    time
}

/// Enable interrupts
#[inline]
pub fn enable_irq() {
    // SAFETY: csrsi atomically sets the SIE bit in sstatus; safe at any time.
    unsafe {
        asm!("csrsi sstatus, 2", options(nomem, nostack));
    }
}

/// Disable interrupts
#[inline]
pub fn disable_irq() {
    // SAFETY: csrci atomically clears the SIE bit in sstatus; safe at any time.
    unsafe {
        asm!("csrci sstatus, 2", options(nomem, nostack));
    }
}

/// Wait for interrupt
#[inline]
pub fn wfi() {
    // SAFETY: wfi halts the hart until the next interrupt; no side effects beyond waiting.
    // Note: nomem is intentionally omitted — WFI has side effects (it wakes on interrupts)
    // and must not be reordered past memory operations that set up the wake condition.
    unsafe {
        core::arch::asm!("wfi", options(nostack));
    }
}

/// Instruction serialization barrier
#[inline]
pub fn isb() {
    // SAFETY: fence.i is a local instruction cache barrier; it has no harmful side effects.
    unsafe {
        core::arch::asm!("fence.i", options(nomem, nostack));
    }
}

/// Data synchronization barrier
#[inline]
pub fn dsb() {
    // SAFETY: fence is a memory ordering barrier; no harmful side effects.
    unsafe {
        core::arch::asm!("fence", options(nomem, nostack));
    }
}

/// Data memory barrier
#[inline]
pub fn dmb() {
    // SAFETY: fence is a memory ordering barrier; no harmful side effects.
    unsafe {
        core::arch::asm!("fence", options(nomem, nostack));
    }
}

/// Get interrupt mask state
#[inline]
pub fn get_interrupts_state() -> bool {
    let sstatus: u64;
    // SAFETY: sstatus is a supervisor CSR; reading it is always safe.
    unsafe {
        asm!("csrr {}, sstatus", out(reg) sstatus, options(nomem, nostack, pure));
    }
    // sstatus.SIE bit (bit 1)
    (sstatus & (1 << 1)) != 0
}

/// Save interrupt state and disable interrupts
#[inline]
pub fn save_and_disable_irq() -> bool {
    // SAFETY: csrrci atomically reads sstatus and clears the SIE bit in one
    // instruction — no TOCTOU race. The returned value has the pre-clear SIE state.
    unsafe {
        let state: u64;
        asm!("csrrci {0}, sstatus, 2", out(reg) state, options(nomem, nostack));
        (state & 2) != 0
    }
}

/// Restore interrupt state
#[inline]
pub fn restore_irq(state: bool) {
    if state {
        enable_irq();
    }
}
