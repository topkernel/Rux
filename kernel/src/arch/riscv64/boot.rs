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
    // mhartid is an M-mode CSR and traps when read from S-mode. The hart id
    // lives in tp during early boot and in task.ti_cpu once scheduling starts;
    // cpu_id() implements that protocol.
    super::cpu_id()
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
