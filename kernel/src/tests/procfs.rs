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

// ========================================================================
// kv / cols shape floors (formerly macro-emitted *_shape mods inside
// the procfs sources — the procfile! macro no longer emits test mods)
// ========================================================================

/// Mechanical drift floor for the kv form: each declared key renders as
/// exactly `"<key> <digits>\n"`.
fn kv_shape_check(name: &str, fields: &[&'static str]) {
    for k in fields {
        let got = crate::fs::procfs::procfile::render_kv(&[(k, &0u64)]);
        assert_eq!(got, alloc::format!("{} 0\n", k), "kv shape drifted for {} in {}", k, name);
    }
    // The whole file renders one row per declared key, in order.
    let rows: Vec<(&'static str, &dyn core::fmt::Display)> =
        fields.iter().map(|k| (*k, &0u64 as &dyn core::fmt::Display)).collect();
    let rendered = crate::fs::procfs::procfile::render_kv(&rows);
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines.len(), fields.len(), "{} row count drifted", name);
    for (line, key) in lines.iter().zip(fields.iter()) {
        assert_eq!(line, &alloc::format!("{} 0", key), "{} row shape drifted", name);
    }
}

/// Mechanical drift floor for the cols form: `label` + ` <value>` per
/// declared column + `\n` — single spaces, no trailing space.
fn cols_shape_check(name: &str, label: &str, columns: &[&'static str]) {
    let vals: Vec<&dyn core::fmt::Display> =
        columns.iter().map(|_| &0u64 as &dyn core::fmt::Display).collect();
    let got = crate::fs::procfs::procfile::render_cols(label, &vals);
    let mut expect = alloc::string::String::from(label);
    for _ in columns {
        expect.push_str(" 0");
    }
    expect.push('\n');
    assert_eq!(got, expect, "cols shape drifted for {}", name);
}

// ========================================================================
// Moved bodies (formerly vmstat/meminfo/stat/diskstats _file_tests
// mods inside the procfs sources)
// ========================================================================

fn vmstat_snapshot_check() {
    use crate::fs::procfs::vmstat::VMSTAT_FIELDS;
    let zero: u64 = 0;
    let rows: Vec<(&'static str, &dyn core::fmt::Display)> =
        VMSTAT_FIELDS.iter().map(|k| (*k, &zero as &dyn core::fmt::Display)).collect();
    let got = crate::fs::procfs::procfile::render_kv(&rows);
    let lines: Vec<&str> = got.lines().collect();
    assert_eq!(lines.first(), Some(&"nr_free_pages 0"));
    assert_eq!(lines.get(1), Some(&"nr_zone_inactive_anon 0"));
    assert_eq!(
        lines.iter().position(|l| *l == "pgalloc_dma32 0"),
        Some(43),
        "pgalloc_dma32 moved — column order is Linux-observable"
    );
    assert_eq!(lines.last(), Some(&"pswpout 0"));
    assert_eq!(lines.len(), VMSTAT_FIELDS.len());
    assert!(VMSTAT_FIELDS.len() == 60, "field count changed: {} — update this snapshot deliberately", VMSTAT_FIELDS.len());
}

fn meminfo_keys_shape_check() {
    use alloc::format;
    use alloc::vec::Vec;
    use crate::fs::procfs::meminfo::{meminfo, MEMINFO_FIELDS};
    let out = meminfo();
    let text = core::str::from_utf8(&out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let expect: Vec<&str> = MEMINFO_FIELDS
        .iter()
        .map(|k| k.strip_suffix('!').unwrap_or(k))
        .collect();
    assert_eq!(lines.len(), expect.len(), "line count drifted");
    for (line, key) in lines.iter().zip(expect.iter()) {
        let head = format!("{}:", key);
        assert!(
            line.starts_with(&head),
            "line {:?} does not start with declared key {:?}",
            line,
            key
        );
        let unitless = MEMINFO_FIELDS
            .iter()
            .any(|k| k.strip_suffix('!') == Some(*key));
        if unitless {
            assert!(!line.ends_with(" kB"), "unitless row gained kB: {:?}", line);
        } else {
            assert!(line.ends_with(" kB"), "row lost its kB unit: {:?}", line);
        }
        // The value column: at least one space after the padded
        // key field, before the value (width-16 key + >=1 space).
        let tail = &line[head.len()..];
        assert!(tail.starts_with(' '), "no gap after key field: {:?}", line);
    }
}

/// Byte order of /proc/stat's structural line prefixes.
const STAT_LINE_PREFIXES: &[&str] = &[
    "cpu ", "cpu0", "\nintr ", "\nctxt ", "\nbtime ", "\nprocesses ",
    "\nprocs_running ", "\nprocs_blocked ", "\nsoftirq",
];

fn stat_line_order_check() {
    use crate::fs::procfs::stat::generate;
    let bytes = generate();
    let text = alloc::string::String::from_utf8(bytes).expect("/proc/stat must be UTF-8");
    let mut last = 0usize;
    for p in STAT_LINE_PREFIXES {
        let at = text.find(p).unwrap_or_else(|| {
            panic!("/proc/stat line order: missing prefix {:?} after byte {}", p, last)
        });
        assert!(at >= last, "/proc/stat prefix {:?} out of order ({} < {})", p, at, last);
        last = at;
    }
    for line in text.lines() {
        if line.starts_with("cpu") {
            assert_eq!(line.trim_end(), line, "trailing space on cpu row");
        }
    }
}

fn diskstats_snapshot_check() {
    use crate::fs::procfs::diskstats::{diskstats_row, DISKSTAT_COLUMNS};
    let got = diskstats_row(
        "254 0 vda",
        14, 0, 34, 2,      // reads family
        7, 0, 19, 3,       // writes family
        1, 12, 12,         // in-flight, io time, weighted
        0, 0, 0, 0,        // discards
        0, 0,              // flush
    );
    assert_eq!(
        got,
        "254 0 vda 14 0 34 2 7 0 19 3 1 12 12 0 0 0 0 0 0\n",
        "/proc/diskstats row bytes drifted from Linux's 20-field layout"
    );
    assert_eq!(DISKSTAT_COLUMNS.len(), 17, "declared column count drifted");
    let tokens = got.trim_end_matches('\n')
        .split(' ').filter(|t| !t.is_empty()).count();
    assert_eq!(tokens, 3 + DISKSTAT_COLUMNS.len(), "/proc/diskstats field count drifted");
}

pub fn test_procfs() {
    crate::tests::test_group_start("procfs");

    // /proc/cpuinfo + /proc/[pid]/status pins (ported earlier).
    cpuinfo_shape_check();
    cpuinfo_order_check();
    status_shape_check();
    status_order_check();
    crate::tests::test_pass("procfs-pins");

    // kv-form shape floors (formerly the macro-emitted vmstat_shape /
    // meminfo_shape mods).
    kv_shape_check("vmstat", crate::fs::procfs::vmstat::VMSTAT_FIELDS);
    kv_shape_check("meminfo", crate::fs::procfs::meminfo::MEMINFO_FIELDS);
    vmstat_snapshot_check();
    meminfo_keys_shape_check();
    crate::tests::test_pass("procfs-vmstat-meminfo");

    // cols-form shape floors (formerly stat_row_shape /
    // schedstat_row_shape / diskstats_row_shape).
    cols_shape_check("stat", "cpu", crate::fs::procfs::stat::STAT_CPU_COLUMNS);
    cols_shape_check("schedstat", "cpu0", crate::fs::procfs::schedstat::SCHEDSTAT_CPU_COLUMNS);
    cols_shape_check("diskstats", "254 0 vda", crate::fs::procfs::diskstats::DISKSTAT_COLUMNS);
    stat_line_order_check();
    diskstats_snapshot_check();
    crate::tests::test_pass("procfs-stat-diskstats");
}
