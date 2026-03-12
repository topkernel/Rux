//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit kernel boot process

// Include boot.S assembly code
core::arch::global_asm!(include_str!("boot.S"));

/// Device tree pointer (set by boot.S)
extern "C" {
    /// Device tree pointer (passed by OpenSBI via a1 register)
    static dtb_pointer: u64;
}

pub fn get_core_id() -> u64 {
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
    unsafe { dtb_pointer }
}
