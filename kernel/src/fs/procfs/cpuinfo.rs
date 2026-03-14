//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/cpuinfo - CPU information
//!
//! Reference: Linux arch/riscv/kernel/cpu.c

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// Generate /proc/cpuinfo content
///
/// Displays CPU information in the standard Linux format for RISC-V.
pub fn generate() -> Vec<u8> {
    use crate::arch::riscv64::smp::num_started_cpus;

    let mut content = String::new();
    let num_cpus = num_started_cpus();

    for cpu in 0..num_cpus {
        // In S-mode, we cannot read mvendorid, marchid, mimpid, misa directly.
        // These are M-mode CSRs and accessing them causes illegal instruction.
        // Use static information or SBI calls instead.

        content.push_str(&format!("processor\t: {}\n", cpu));
        content.push_str(&format!("hart\t\t: {}\n", cpu));
        // Use detected ISA string from boot or static
        content.push_str("isa\t\t: rv64imafdc\n");
        content.push_str("mmu\t\t: sv39\n");
        // mvendorid, marchid, mimpid require M-mode or SBI call
        // For now, show as unavailable
        content.push_str("mvendorid\t: 0x0\n");
        content.push_str("marchid\t\t: 0x0\n");
        content.push_str("mimpid\t\t: 0x0\n");

        if cpu < num_cpus - 1 {
            content.push('\n');
        }
    }

    content.into_bytes()
}
