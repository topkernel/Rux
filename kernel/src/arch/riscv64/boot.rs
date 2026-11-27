//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit kernel boot process


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

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
/// Collected into `arch::arch_floor` (INV-ARCH-BOOT-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_dtb_pointer() -> u64 {
    crate::arch::arch_floor::riscv64::boot::get_dtb_pointer()
}
