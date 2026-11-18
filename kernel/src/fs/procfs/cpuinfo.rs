//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/cpuinfo - CPU information
//!
//! D-4 kvtab_pad conversion (refactor lane): the per-CPU block is one
//! declared row list per arch; keys carry their own tab padding (the
//! byte alignment belongs to the key, matching Linux's riscv/x86
//! layouts).

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::vec::Vec;

/// Declared key order of one /proc/cpuinfo block, riscv64 (Linux's
/// riscv layout; the M-mode CSRs read as 0 in S-mode — see generate).
pub const CPUINFO_RISCV_FIELDS: &[&str] = &[
    "processor\t",
    "hart\t\t",
    "isa\t\t",
    "hart isa\t",
    "mmu\t\t",
    "mvendorid\t",
    "marchid\t\t",
    "mimpid\t\t",
];

/// Declared key order of one /proc/cpuinfo block, x86_64. Linux emits
/// `processor` exactly once per block, at the top (arch/x86/kernel/cpu/
/// proc.c show_cpuinfo); the pre-conversion code duplicated it after
/// cache_alignment, which the D-4 conversion preserved byte-for-byte —
/// removed here.
pub const CPUINFO_X86_FIELDS: &[&str] = &[
    "processor\t",
    "vendor_id\t",
    "cpu family\t",
    "model\t\t",
    "model name\t",
    "flags\t\t",
    "clflush size\t",
    "cache_alignment\t",
];

/// Declared key order of one /proc/cpuinfo block, aarch64 (the
/// reference kernel's arm64 layout — keys carry their own tab padding;
/// `CPU architecture` is long enough that it gets none).
pub const CPUINFO_AARCH64_FIELDS: &[&str] = &[
    "processor\t",
    "BogoMIPS\t",
    "Features\t",
    "CPU implementer\t",
    "CPU architecture",
    "CPU variant\t",
    "CPU part\t",
    "CPU revision\t",
];

/// Generate /proc/cpuinfo content
pub fn generate() -> Vec<u8> {
    use crate::arch::smp::num_started_cpus;

    let mut content = String::new();
    let num_cpus = num_started_cpus();

    for cpu in 0..num_cpus {
        // In S-mode, we cannot read mvendorid, marchid, mimpid, misa
        // directly. These are M-mode CSRs and accessing them causes an
        // illegal instruction. Use static information or SBI calls
        // instead.
        #[cfg(feature = "riscv64")]
        {
            let rows: &[(&'static str, &dyn core::fmt::Display)] = &[
                ("processor\t", &cpu),
                ("hart\t\t", &cpu),
                // ISA string — matches the format expected by musl and procps
                ("isa\t\t", &"rv64imafdc"),
                ("hart isa\t", &"rv64imafdc"),
                ("mmu\t\t", &"sv39"),
                ("mvendorid\t", &"0x0"),
                ("marchid\t\t", &"0x0"),
                ("mimpid\t\t", &"0x0"),
            ];
            content.push_str(&super::procfile::render_kvtab_pad(rows));
        }

        #[cfg(feature = "x86_64")]
        {
            // Generic x86_64 fields (vendor/family/model detection is a
            // later CPUID-driven upgrade).
            let rows: &[(&'static str, &dyn core::fmt::Display)] = &[
                ("processor\t", &cpu),
                ("vendor_id\t", &"unknown"),
                ("cpu family\t", &"0"),
                ("model\t\t", &"0"),
                ("model name\t", &"x86_64"),
                ("flags\t\t", &""), // no flags: `flags\t\t:\n` byte shape
                ("clflush size\t", &"64"),
                ("cache_alignment\t", &"64"),
            ];
            content.push_str(&super::procfile::render_kvtab_pad(rows));
        }

        #[cfg(feature = "aarch64")]
        {
            // Static cortex-a72 values (MIDR_EL1-driven detection is a
            // later upgrade), same honesty class as the x86_64 branch.
            let rows: &[(&'static str, &dyn core::fmt::Display)] = &[
                ("processor\t", &cpu),
                ("BogoMIPS\t", &"0.00"),
                ("Features\t", &""), // no features: `Features\t:\n` byte shape
                ("CPU implementer\t", &"0x41"),
                ("CPU architecture", &"8"),
                ("CPU variant\t", &"0x0"),
                ("CPU part\t", &"0xd08"),
                ("CPU revision\t", &"0"),
            ];
            content.push_str(&super::procfile::render_kvtab_pad(rows));
        }

        if cpu < num_cpus - 1 {
            content.push('\n');
        }
    }

    content.into_bytes()
}

