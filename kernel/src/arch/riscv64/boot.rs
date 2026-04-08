//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit kernel boot process

// boot.S is compiled separately in build.rs and linked as the first object
// to ensure _start is at the kernel's load address.

/// Device tree pointer (set by boot.S)
extern "C" {
    /// Device tree pointer (passed by OpenSBI via a1 register)
    static dtb_pointer: u64;
}

pub fn get_core_id() -> u64 {
    // SAFETY: mhartid is a standard RISC-V CSR that returns the hardware thread ID.
    // Only valid during early M-mode boot (before entering S-mode).
    unsafe {
        let hart_id: u64;
        core::arch::asm!("csrrw {}, mhartid, zero", out(reg) hart_id);
        hart_id
    }
}

/// Get device tree pointer
///
/// When OpenSBI jumps to the kernel, the a1 register contains the device tree pointer.
/// If no device tree, a1 is 0.
pub fn get_dtb_pointer() -> u64 {
    // SAFETY: dtb_pointer is a static extern set by boot.S before rust_main runs.
    // Reading it is safe once the kernel has started.
    unsafe { dtb_pointer }
}
