//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Procfs renderer pins — moved here per the test-layout policy from the
//! in-module `cpuinfo_file_tests` / `status_file_tests` blocks inside
//! fs/procfs/cpuinfo.rs and fs/procfs/pid.rs (both carried dead
//! #[cfg(test)] harnesses that never compiled in this no_std crate).
//!
//! Conversion pins for the D-4 kvtab/kvtab_pad slices: the key sequence,
//! the `key<tabs>: value` / `Key:\t<value>` byte shapes, and the
//! hex-mask/kB/quadruple value formats are pinned against live renders.

use alloc::string::String;
use alloc::vec::Vec;

// ========================================================================
// /proc/cpuinfo (per-arch key table)
// ========================================================================

#[cfg(feature = "riscv64")]
const CPUINFO_FIELDS: &[&str] = crate::fs::procfs::cpuinfo::CPUINFO_RISCV_FIELDS;
#[cfg(feature = "x86_64")]
const CPUINFO_FIELDS: &[&str] = crate::fs::procfs::cpuinfo::CPUINFO_X86_FIELDS;
#[cfg(feature = "aarch64")]
const CPUINFO_FIELDS: &[&str] = crate::fs::procfs::cpuinfo::CPUINFO_AARCH64_FIELDS;

fn cpuinfo_shape_check() {
    for k in CPUINFO_FIELDS {
        let got = crate::fs::procfs::procfile::render_kvtab_pad(&[(k, &0u64)]);
        assert_eq!(got, alloc::format!("{}: 0\n", k), "kvtab_pad shape drifted for {:?}", k);
    }
    // Empty-value row: colon then newline, NO space (the flags row).
    let got = crate::fs::procfs::procfile::render_kvtab_pad(&[("flags\t\t", &"")]);
    assert_eq!(got, "flags\t\t:\n", "kvtab_pad empty-value shape drifted");
}

fn cpuinfo_order_check() {
    let bytes = crate::fs::procfs::cpuinfo::generate();
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|_| panic!("/proc/cpuinfo must be UTF-8"));
    // First block = the per-cpu-independent slice: the keys of the first
    // block, in order, must equal the arch const.
    let n = crate::arch::smp::num_started_cpus().max(1);
    let first_block_len = if n > 1 {
        text.find("\n\n").unwrap_or(text.len())
    } else {
        text.len()
    };
    let keys: Vec<&str> = text[..first_block_len]
        .lines()
        .map(|l| l.split_once(": ").map(|(k, _)| k).unwrap_or(
            // the empty-value row splits at the bare colon
            l.strip_suffix(':').unwrap_or(l),
        ))
        .collect();
    assert_eq!(keys, CPUINFO_FIELDS, "/proc/cpuinfo per-block key order drifted");
}

// ========================================================================
// /proc/[pid]/status
// ========================================================================

/// Mechanical floor: every declared key renders `Key:\t0\n`-shaped
/// (colon, ONE tab, value, newline; empty value → trailing tab).
fn status_shape_check() {
    for k in crate::fs::procfs::pid::STATUS_FIELDS {
        let got = crate::fs::procfs::procfile::render_kvtab(&[(k, &0u64)]);
        assert_eq!(got, alloc::format!("{}:\t0\n", k), "kvtab shape drifted for {}", k);
    }
    // The empty-value row keeps its trailing tab (status's Groups).
    let got = crate::fs::procfs::procfile::render_kvtab(&[("Groups", &"")]);
    assert_eq!(got, "Groups:\t\n", "kvtab empty-value shape drifted");
}

/// Live pin: the current task's status file carries exactly
/// STATUS_FIELDS, in order, one row each.
fn status_order_check() {
    let pid = crate::process::current_pid() as u64;
    let bytes = crate::fs::procfs::pid::generate_status(pid);
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|_| panic!("/proc/[pid]/status must be UTF-8"));
    let keys: Vec<&str> = text
        .lines()
        .map(|l| l.split_once(":\t").map(|(k, _)| k).unwrap_or(l))
        .collect();
    assert_eq!(
        keys,
        crate::fs::procfs::pid::STATUS_FIELDS,
        "/proc/[pid]/status key order drifted"
    );
    // Value-format pins: the hex masks are 16 lower-hex digits, the
    // Vm rows carry the kB suffix, Uid/Gid are four tab-separated fields.
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("SigBlk:\t") {
            assert_eq!(v.len(), 16, "SigBlk mask width drifted: {:?}", v);
            assert!(v.bytes().all(|b| b.is_ascii_hexdigit()), "SigBlk not hex: {:?}", v);
        }
        if let Some(v) = line.strip_prefix("VmRSS:\t") {
            assert!(v.ends_with(" kB"), "VmRSS unit suffix drifted: {:?}", v);
        }
        if let Some(v) = line.strip_prefix("Uid:\t") {
            assert_eq!(v.split('\t').count(), 4, "Uid quadruple drifted: {:?}", v);
        }
    }
}

pub fn test_procfs() {
    crate::tests::test_group_start("procfs");
    cpuinfo_shape_check();
    cpuinfo_order_check();
    status_shape_check();
    status_order_check();
    crate::tests::test_pass("procfs-pins");
}
