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

/// Device tree pointer (set by boot.S from x0)
extern "C" {
    static dtb_pointer: u64;
}

pub fn get_core_id() -> u64 {
    super::cpu_id()
}

/// Get device tree pointer (QEMU passes it in x0 at entry)
pub fn get_dtb_pointer() -> u64 {
    // SAFETY: dtb_pointer is a static extern set by boot.S before
    // rust_main runs; reading it is safe once the kernel has started.
    unsafe { dtb_pointer }
}
