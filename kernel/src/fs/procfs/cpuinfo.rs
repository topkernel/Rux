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
            // CPUID-driven x86_64 fields (the llvmpipe/JIT fix,
            // 2026-11-08): Mesa's gallium/llvmpipe derives the CPU
            // capabilities from THIS file's flags line (util_get_cpu_caps
            // parses /proc/cpuinfo on Linux — not CPUID). An empty flags
            // row made it pick a 32-bit-only JIT subtarget, and the first
            // 64-bit codegen request aborted the process: "LLVM ERROR:
            // 64-bit code requested on a subtarget that doesn't support
            // it!" — which killed gnome-shell (sig=6) on every boot after
            // its first frame, taking the session down with it
            // ("failed to register before timeout" → respawn storm →
            // the gnome-session-failed dialog). Emit the real feature
            // words from CPUID.
            let (leaf1a, _b, leaf1c, leaf1d) = crate::arch::x86_64::cpu::cpuid(1, 0);
            let (_7a, leaf7b, _c, _d) = crate::arch::x86_64::cpu::cpuid(7, 0);
            let family = ((leaf1a >> 8) & 0xf) + ((leaf1a >> 20) & 0xff);
            let model = ((leaf1a >> 4) & 0xf) | (((leaf1a >> 16) & 0xf) << 4);
            let stepping = leaf1a & 0xf;

            let mut flags = String::new();
            // leaf-1 EDX
            for (bit, name) in [
                (0u32, "fpu"),
                (1, "vme"),
                (2, "de"),
                (3, "pse"),
                (4, "tsc"),
                (5, "msr"),
                (6, "pae"),
                (7, "mce"),
                (8, "cx8"),
                (9, "apic"),
                (11, "sep"),
                (12, "mtrr"),
                (13, "pge"),
                (14, "mca"),
                (15, "cmov"),
                (16, "pat"),
                (17, "pse36"),
                (18, "pn"),
                (19, "clflush"),
                (21, "dts"),
                (22, "acpi"),
                (23, "mmx"),
                (24, "fxsr"),
                (25, "sse"),
                (26, "sse2"),
                (27, "ss"),
                (28, "ht"),
                (29, "tm"),
                (31, "pbe"),
            ] {
                if leaf1d & (1 << bit) != 0 {
                    flags.push_str(name);
                    flags.push(' ');
                }
            }
            // leaf-1 ECX
            for (bit, name) in [
                (0u32, "pni"),
                (1, "pclmulqdq"),
                (2, "dtes64"),
                (3, "monitor"),
                (5, "vmx"),
                (6, "smx"),
                (7, "est"),
                (8, "tm2"),
                (9, "ssse3"),
                (12, "fma"),
                (13, "cx16"),
                (14, "xtpr"),
                (17, "pdcm"),
                (19, "sse4_1"),
                (20, "sse4_2"),
                (21, "x2apic"),
                (22, "movbe"),
                (23, "popcnt"),
                (24, "tsc_deadline_timer"),
                (25, "aes"),
                (26, "xsave"),
                (27, "avx"),
                (28, "f16c"),
                (29, "rdrand"),
                (30, "hypervisor"),
            ] {
                if leaf1c & (1 << bit) != 0 {
                    flags.push_str(name);
                    flags.push(' ');
                }
            }
            // leaf-7 EBX (only the ones llvmpipe/gallium look for).
            for (bit, name) in [
                (3u32, "bmi1"),
                (5, "avx2"),
                (8, "bmi2"),
            ] {
                if leaf7b & (1 << bit) != 0 {
                    flags.push_str(name);
                    flags.push(' ');
                }
            }
            // Long mode: leaf 0x80000001 EDX bit 29 — THE flag Mesa's
            // probe needs to build a 64-bit-capable JIT subtarget.
            let (_ea, _eb, _ec, ext_d) = crate::arch::x86_64::cpu::cpuid(0x80000001, 0);
            if ext_d & (1 << 29) != 0 {
                flags.push_str("lm ");
            }
            // Trim the trailing space.
            if flags.ends_with(' ') {
                flags.pop();
            }

            let rows: &[(&'static str, &dyn core::fmt::Display)] = &[
                ("processor\t", &cpu),
                ("vendor_id\t", &"AuthenticAMD"),
                ("cpu family\t", &family),
                ("model\t\t", &model),
                ("model name\t", &"x86_64"),
                ("flags\t\t", &flags),
                ("clflush size\t", &"64"),
                ("cache_alignment\t", &"64"),
                ("stepping\t", &stepping),
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
    #[cfg(feature = "aarch64")]
    const FIELDS: &[&str] = super::CPUINFO_AARCH64_FIELDS;

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
