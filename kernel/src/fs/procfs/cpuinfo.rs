//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/cpuinfo - CPU information
//!
//! D-4 kvtab_pad conversion (refactor lane): the per-CPU block is one
//! declared row list per arch; keys carry their own tab padding (the
//! byte alignment belongs to the key, matching Linux's riscv/x86
//! layouts). The x86 row list PRESERVES a pre-conversion quirk: the
//! `processor` row appears twice per block (once before vendor_id,
//! once after cache_alignment) — reported to the coordinator as a
//! separate candidate fix; a behavior-preserving conversion must not
//! change it here.

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
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

/// Declared key order of one /proc/cpuinfo block, x86_64. The trailing
/// duplicate `processor` row is the preserved pre-conversion shape
/// (see the module doc) until the coordinator decides the fix.
pub const CPUINFO_X86_FIELDS: &[&str] = &[
    "processor\t",
    "vendor_id\t",
    "cpu family\t",
    "model\t\t",
    "model name\t",
    "flags\t\t",
    "clflush size\t",
    "cache_alignment\t",
    "processor\t",
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
                // Preserved pre-conversion duplicate (module doc).
                ("processor\t", &cpu),
            ];
            content.push_str(&super::procfile::render_kvtab_pad(rows));
        }

        if cpu < num_cpus - 1 {
            content.push('\n');
        }
    }

    content.into_bytes()
}

/// Conversion pins for /proc/cpuinfo (D-4 kvtab_pad slice): every
/// declared key renders the `key<tabs>: value\n` shape (empty value →
/// `key<tabs>:\n`, no space), and a live render's per-block key
/// sequence equals the arch's const. Callable bodies for the in-kernel
/// unit-test suite.
#[cfg(any(test, feature = "unit-test"))]
pub mod cpuinfo_file_tests {
    #[cfg(feature = "riscv64")]
    const FIELDS: &[&str] = super::CPUINFO_RISCV_FIELDS;
    #[cfg(feature = "x86_64")]
    const FIELDS: &[&str] = super::CPUINFO_X86_FIELDS;

    pub fn shape_check() {
        for k in FIELDS {
            let got = crate::fs::procfs::procfile::render_kvtab_pad(&[(k, &0u64)]);
            assert_eq!(got, alloc::format!("{}: 0\n", k), "kvtab_pad shape drifted for {:?}", k);
        }
        // Empty-value row: colon then newline, NO space (the flags row).
        let got = crate::fs::procfs::procfile::render_kvtab_pad(&[("flags\t\t", &"")]);
        assert_eq!(got, "flags\t\t:\n", "kvtab_pad empty-value shape drifted");
    }

    pub fn order_check() {
        let bytes = super::generate();
        let text = alloc::string::String::from_utf8(bytes)
            .unwrap_or_else(|_| panic!("/proc/cpuinfo must be UTF-8"));
        // First block = first `num_cpus-independent` slice: the keys of
        // the first block, in order, must equal the const.
        let n = crate::arch::smp::num_started_cpus().max(1);
        let first_block_len = if n > 1 {
            text.find("\n\n").unwrap_or(text.len())
        } else {
            text.len()
        };
        let keys: alloc::vec::Vec<&str> = text[..first_block_len]
            .lines()
            .map(|l| l.split_once(": ").map(|(k, _)| k).unwrap_or(
                // the empty-value row splits at the bare colon
                l.strip_suffix(':').unwrap_or(l),
            ))
            .collect();
        assert_eq!(keys, FIELDS, "/proc/cpuinfo per-block key order drifted");
    }

    #[cfg(test)]
    #[test]
    fn cpuinfo_shape() {
        shape_check();
        order_check();
    }
}
