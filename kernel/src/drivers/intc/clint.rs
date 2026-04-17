//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V CLINT (Core-Local Interrupt Controller) driver
//!
//! **Note**: Modern RISC-V systems (OpenSBI v1.3+) do not allow S-mode direct access to CLINT registers
//! CLINT is configured as M-mode only, S-mode must use SBI calls to access timer and IPI functionality
//!
//! CLINT is responsible for handling:
//! - Software interrupts (MSIP) - used for inter-processor interrupts (IPI)
//! - Timer interrupts (MTIMECMP)
//! - Time register (MTIME)
//!
//! This implementation uses SBI calls instead of direct MMIO access

use core::sync::atomic::{AtomicU32, Ordering};
use crate::sbi;

// IPI count per hart — must match config::MAX_CPUS
const NUM_HARTS: usize = crate::config::MAX_CPUS;
static IPI_COUNT: [AtomicU32; NUM_HARTS] = {
    let arr: [AtomicU32; NUM_HARTS] = [
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
    ];
    arr
};

/// Initialize CLINT driver
///
/// Note: Modern systems do not need direct access to CLINT registers
/// SBI firmware manages CLINT, S-mode accesses through SBI calls
pub fn init() {
    // SBI system automatically manages CLINT
    // No S-mode software initialization needed
    // Clear IPI counters
    for hart in 0..NUM_HARTS {
        IPI_COUNT[hart].store(0, Ordering::Relaxed);
    }
}

/// Send IPI to specified hart
///
/// Uses SBI IPI Extension (EID #0x735049)
///
/// # Parameters
/// * `target_hart` - Target hart ID (0-3)
pub fn send_ipi(target_hart: usize) {
    if target_hart >= 4 {
        return;
    }

    // Send IPI via SBI (not direct CLINT MSIP register access)
    if sbi::send_ipi(target_hart) {
        // Update counter
        IPI_COUNT[target_hart].fetch_add(1, Ordering::Relaxed);
    }
}

/// Clear IPI for specified hart
///
/// Note: When using SBI, IPI clearing is handled automatically by SBI firmware
/// S-mode software does not need to manually clear MSIP register
///
/// # Parameters
/// * `hart` - Hart ID
pub fn clear_ipi(hart: usize) {
    if hart >= 4 {
        return;
    }

    // SBI system automatically clears IPI
    // In software interrupt handler, SBI automatically clears pending state
    // No S-mode software manual clearing needed

    // Optional: Clear counter (if needed)
    // IPI_COUNT[hart].store(0, Ordering::Relaxed);
}

/// Get number of IPIs sent to specified hart
///
/// # Parameters
/// * `hart` - Hart ID
///
/// # Returns
/// IPI count
pub fn get_ipi_count(hart: usize) -> u32 {
    if hart < 4 {
        IPI_COUNT[hart].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Read system time (time CSR)
///
/// Uses RISC-V `rdtime` instruction to read time
///
/// # Returns
/// Current time (cycles)
pub fn read_time() -> u64 {
    // SAFETY: rdtime is a read-only CSR available in S-mode on RISC-V.
    unsafe {
        let time: u64;
        core::arch::asm!(
            "rdtime {}",
            out(reg) time,
            options(nostack, readonly)
        );
        time
    }
}

/// Set timer compare value
///
/// Uses SBI TIMER Extension's set_timer function
///
/// # Parameters
/// * `hart` - Hart ID (note: set_timer is per-hart)
/// * `value` - Timer compare value (absolute time)
pub fn set_timecmp(_hart: usize, value: u64) {
    // Use SBI set_timer
    // Note: SBI's set_timer is per-hart, automatically applies to current hart
    // SAFETY: sbi_rt::set_timer is an SBI ecall with no special preconditions;
    // the `value` is an absolute timer deadline (u64), always valid.
    unsafe {
        sbi_rt::set_timer(value);
    }
}
