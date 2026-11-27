//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 boot — QEMU virt direct-boot handoff.
//!
//! boot.S is compiled separately in build.rs and linked as the first
//! object so `_start` sits at the kernel's physical load address
//! (0x4008_0000, RAM base + text_offset). QEMU `-kernel` with an ELF
//! image loads the segments at their LMAs and enters at `e_entry` with
//! x0 = device-tree pointer, at EL2 (boot.S drops to EL1 before the MMU
//! comes on).


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

/// Device tree pointer (boot.S publishes the embedded QEMU-virt DTB
/// blob's address; QEMU's ELF -kernel entry zeroes x0, so there is no
/// firmware pointer to save — see docs/architecture/aarch64.md)
extern "C" {
    static dtb_pointer: u64;
}

pub fn get_core_id() -> u64 {
    super::cpu_id()
}
/// Get device tree pointer (the embedded blob's linked address)
///
/// Collected into `arch::arch_floor` (INV-ARCH-BOOT-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_dtb_pointer() -> u64 {
    crate::arch::arch_floor::aarch64::boot::get_dtb_pointer()
}
