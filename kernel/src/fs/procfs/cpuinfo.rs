//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/cpuinfo - CPU information

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

/// Generate /proc/cpuinfo content
pub fn generate() -> Vec<u8> {
    use crate::arch::smp::num_started_cpus;

    let mut content = String::new();
    let num_cpus = num_started_cpus();

    for cpu in 0..num_cpus {
        // In S-mode, we cannot read mvendorid, marchid, mimpid, misa directly.
        // These are M-mode CSRs and accessing them causes illegal instruction.
        // Use static information or SBI calls instead.

        content.push_str(&format!("processor\t: {}\n", cpu));

        // In S-mode, we cannot read mvendorid, marchid, mimpid directly.
        // These are M-mode CSRs and accessing them causes illegal instruction.
        // Use static information or SBI calls instead.
        #[cfg(feature = "riscv64")]
        {
            content.push_str(&format!("hart\t\t: {}\n", cpu));
            // ISA string — matches the format expected by musl and procps
            content.push_str("isa\t\t: rv64imafdc\n");
            content.push_str(&format!("hart isa\t: rv64imafdc\n"));
            content.push_str("mmu\t\t: sv39\n");
            // mvendorid, marchid, mimpid require M-mode or SBI call
            // For now, show as unavailable
            content.push_str("mvendorid\t: 0x0\n");
            content.push_str("marchid\t\t: 0x0\n");
            content.push_str("mimpid\t\t: 0x0\n");
        }

        #[cfg(feature = "x86_64")]
        {
            // Generic x86_64 fields (vendor/family/model detection is a
            // later CPUID-driven upgrade).
            content.push_str("vendor_id\t: unknown\n");
            content.push_str("cpu family\t: 0\n");
            content.push_str("model\t\t: 0\n");
            content.push_str("model name\t: x86_64\n");
            content.push_str("flags\t\t:\n");
            content.push_str("clflush size\t: 64\n");
            content.push_str("cache_alignment\t: 64\n");
            content.push_str(&format!("processor\t: {}\n", cpu));
        }

        if cpu < num_cpus - 1 {
            content.push('\n');
        }
    }

    content.into_bytes()
}
