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

/// SBI SRST (System Reset) extension: EID 0x53525354 ("SRST")
pub const SBI_EXT_SRST: usize = 0x53525354;

/// SBI SRST function IDs
pub const SBI_SRST_FUNCTION_RESET: usize = 0;

/// SBI SRST reset types (FID 0 / a0)
pub mod srst_type {
    /// Shutdown the system
    pub const SHUTDOWN: usize = 0;
    /// Cold reboot
    pub const COLD_REBOOT: usize = 1;
    /// Warm reboot
    pub const WARM_REBOOT: usize = 2;
}

/// SBI SRST reset reasons (a1)
pub mod srst_reason {
    /// No reason
    pub const NONE: usize = 0;
    /// System failure
    pub const FAILURE: usize = 1;
}

/// SBI error codes
pub const SBI_SUCCESS: i64 = 0;
pub const SBI_ERR_FAILURE: i64 = -1;
pub const SBI_ERR_NOT_SUPPORTED: i64 = -2;
pub const SBI_ERR_INVALID_PARAM: i64 = -3;
pub const SBI_ERR_DENIED: i64 = -4;
pub const SBI_ERR_INVALID_ADDRESS: i64 = -5;

/// Request a system reset through the SBI SRST extension (SBI 0.3+).
///
/// # Arguments
/// * `reset_type` - srst_type::SHUTDOWN / COLD_REBOOT / WARM_REBOOT
/// * `reason` - srst_reason::NONE / FAILURE
///
/// # Returns
/// * `true` when the request was accepted (the system is going down —
///   this call does not return on a compliant SBI implementation)
pub fn sbi_system_reset(reset_type: usize, reason: usize) -> bool {
    unsafe {
        let mut error: u64 = reset_type as u64;
        let mut value: u64 = reason as u64;

        asm!(
            "ecall",
            in("a7") SBI_EXT_SRST as u64,
            in("a6") SBI_SRST_FUNCTION_RESET as u64,
            inout("a0") error,
            inout("a1") value,
            options(nomem)
        );

        if error as i64 != SBI_SUCCESS {
            crate::println!(
                "sbi: system reset failed, error={} (SRST not supported?)",
                error as i64
            );
            false
        } else {
            true
        }
    }
}

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
