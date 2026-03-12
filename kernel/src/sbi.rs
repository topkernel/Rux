//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V SBI (Supervisor Binary Interface) call wrapper
//!
//! Uses sbi-rt crate's SBI 0.2 extension

use core::arch::asm;

/// SBI 0.2 TIMER extension's set_timer (recommended)
pub use sbi_rt::set_timer;

/// SBI Extension IDs
pub const SBI_EXT_IPI: usize = 0x735049;  // "IPI"

/// SBI IPI Extension Function IDs
pub const SBI_EXT_IPI_SEND_IPI: usize = 0;

/// SBI error codes
pub const SBI_SUCCESS: i64 = 0;
pub const SBI_ERR_FAILURE: i64 = -1;
pub const SBI_ERR_NOT_SUPPORTED: i64 = -2;
pub const SBI_ERR_INVALID_PARAM: i64 = -3;
pub const SBI_ERR_DENIED: i64 = -4;
pub const SBI_ERR_INVALID_ADDRESS: i64 = -5;

/// Send IPI to specified hart
///
/// # Arguments
/// * `hart_id` - Target hart ID
///
/// # Returns
/// * `bool` - true for success, false for failure
///
/// # Implementation
/// Uses SBI IPI Extension (EID #0x735049)
pub fn send_ipi(hart_id: usize) -> bool {
    unsafe {
        let sbi_ext_id: u64 = SBI_EXT_IPI as u64;
        let sbi_func_id: u64 = SBI_EXT_IPI_SEND_IPI as u64;
        let hart_mask: u64 = 1u64 << hart_id;

        let mut error: u64 = hart_mask;
        let mut value: u64 = 0u64;

        asm!(
            "ecall",
            in("a7") sbi_ext_id,
            in("a6") sbi_func_id,
            inout("a0") error,
            inout("a1") value,
            options(nomem)
        );

        // SBI spec: error = 0 means success
        if error as i64 != SBI_SUCCESS {
            // SBI call failed
            crate::println!("sbi: send_ipi to hart {} failed, error={} ({})",
                hart_id,
                error as i64,
                match error as i64 {
                    SBI_ERR_NOT_SUPPORTED => "NOT_SUPPORTED",
                    SBI_ERR_INVALID_PARAM => "INVALID_PARAM",
                    SBI_ERR_DENIED => "DENIED",
                    SBI_ERR_INVALID_ADDRESS => "INVALID_ADDRESS",
                    _ => "UNKNOWN"
                }
            );
            false
        } else {
            true
        }
    }
}
