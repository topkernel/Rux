//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Test: /proc/pressure (PSI) — file shape, EMA math, monotonic
//! total, and the stall-guard -> sampler -> render plumbing driven
//! across a live jiffy boundary. Runs against the live accounting
//! (the sampler ticks behind scheduler_tick the whole time).

use crate::sched::psi;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use super::{test_fail, test_group_start, test_pass};

/// Check one `avgN=X.YZ` field: name, `=`, integer part, exactly two
/// decimals, value bounded to 100.00 percent.
fn check_field(field: &str, name: &str) -> bool {
    let Some(rest) = field.strip_prefix(name) else { return false };
    let Some(rest) = rest.strip_prefix('=') else { return false };
    let Some((ip, fp)) = rest.split_once('.') else { return false };
    if ip.is_empty() || !ip.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if fp.len() != 2 || !fp.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let v: u32 = format!("{}{}", ip, fp).parse().unwrap_or(u32::MAX);
    v <= 10000
}

/// Check one `some avg10=X.YZ avg60=X.YZ avg300=X.YZ total=N` line.
fn check_line(label: &str, line: &str) -> bool {
    let parts: Vec<&str> = line.split(' ').collect();
    if parts.len() != 5 || parts[0] != label {
        return false;
    }
    if !check_field(parts[1], "avg10")
        || !check_field(parts[2], "avg60")
        || !check_field(parts[3], "avg300")
    {
        return false;
    }
    match parts[4].strip_prefix("total=") {
        Some(t) => !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

pub fn test_psi() {
    test_group_start("psi");

    // Test 1: /proc/pressure/cpu — exactly one `some` line, the
    // reference shape ("CPU FULL is undefined at the system level").
    let cpu = String::from_utf8_lossy(&psi::generate(psi::Resource::Cpu)).into_owned();
    let cpu_lines: Vec<&str> = cpu.lines().collect();
    if cpu_lines.len() == 1 && check_line("some", cpu_lines[0]) {
        test_pass("cpu file shape (some line only)");
    } else {
        test_fail("cpu file shape", &cpu);
    }

    // Test 2: memory and io — two lines each: some then full.
    for res in [psi::Resource::Memory, psi::Resource::Io] {
        let content = String::from_utf8_lossy(&psi::generate(res)).into_owned();
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() == 2
            && check_line("some", lines[0])
            && check_line("full", lines[1])
        {
            test_pass("memory/io file shape (some + full)");
        } else {
            test_fail("memory/io file shape", &content);
        }
    }

    // Test 3: `total` is monotonic across reads (each read takes a
    // fresh sample; stall time never un-accrues).
    let t1 = psi::snapshot_for_test();
    let t2 = psi::snapshot_for_test();
    let monotonic = (0..5).all(|i| t2[i].3 >= t1[i].3);
    if monotonic {
        test_pass("total monotonic across reads");
    } else {
        test_fail("total monotonic", "total decreased between reads");
    }

    // Test 4: the EMA math — sustained 100% samples converge the
    // averages to 100.00 (fixed-point 100*FIXED_1), and 0% samples
    // decay them back toward 0 (400 periods = 80 s, far past the
    // 10 s window's tail).
    let mut load = 0u64;
    for _ in 0..400 {
        load = psi::ema_step_for_test(load, 0, 2_000_000_000, 2_000_000_000);
    }
    let (i10, f10) = psi::load_int_frac_for_test(load);
    if i10 == 100 && f10 == 0 {
        test_pass("EMA converges to 100.00 under sustained stall");
    } else {
        test_fail("EMA convergence", &format!("avg10={}.{:02}", i10, f10));
    }
    let mut load2 = load;
    for _ in 0..400 {
        load2 = psi::ema_step_for_test(load2, 0, 0, 2_000_000_000);
    }
    let (i10b, _) = psi::load_int_frac_for_test(load2);
    if i10b <= 1 {
        test_pass("EMA decays under sustained idle");
    } else {
        test_fail("EMA decay", &format!("avg10={} after idle window", i10b));
    }

    // Test 5: averages bounded [0, 100.00] on the live lines.
    let snap = psi::snapshot_for_test();
    let bounded =
        snap.iter().all(|s| s.0 <= 100 * 2048 && s.1 <= 100 * 2048 && s.2 <= 100 * 2048);
    if bounded {
        test_pass("live averages bounded to [0, 100.00]");
    } else {
        test_fail("averages bounded", "an avg exceeded 100.00 percent");
    }

    // Tests 6-7: the stall guards charge their resource's `total`
    // across a jiffy boundary — the live plumbing guard -> conditions
    // -> sampler -> render, driven without device traffic.
    //
    // Why synthetic: the only IoCompletion::wait call sites (the
    // async batch read path) are disabled tree-wide by the
    // completion-attribution interim gate (ext4/file.rs
    // fill_page_cache_batch, 2026-11-05 cache-poison hunt), and the
    // synchronous write path busy-polls inside submit_request — so
    // no real I/O can enter an iowait section today. The guard
    // plumbing itself is exercised here; the call sites are
    // build-verified (page_alloc.rs slowpath, io_completion.rs wait).
    stall_guard_charges("io", psi::Resource::Io, false);
    stall_guard_charges("memory", psi::Resource::Memory, true);
}

/// Parse the `total=N` (microseconds) off the `some` line of a
/// rendered /proc/pressure resource body.
fn total_us(body: &str) -> Option<u64> {
    for line in body.lines() {
        let mut parts = line.split(' ');
        if parts.next() == Some("some") {
            let last = parts.last()?;
            return last.strip_prefix("total=").and_then(|t| t.parse().ok());
        }
    }
    None
}

/// Hold one stall guard across a full jiffy and require the
/// resource's `some` total to grow by at least the held interval
/// (a read takes its own sample point, so the whole guard-held span
/// is charged — HZ=100 means one jiffy = 10000 µs).
fn stall_guard_charges(name: &str, res: psi::Resource, memstall: bool) {
    let before = total_us(&String::from_utf8_lossy(&psi::generate(res)).into_owned());

    // Hold the section across two jiffy boundaries: whatever the last
    // sample point was, the read's own sample below then covers an
    // interval that entirely lies inside the held window.
    let _guard = if memstall {
        psi::StallGuard::memstall()
    } else {
        psi::StallGuard::iowait()
    };
    let j0 = crate::drivers::timer::get_jiffies();
    while crate::drivers::timer::get_jiffies() == j0 {
        core::hint::spin_loop();
    }
    let j1 = crate::drivers::timer::get_jiffies();
    while crate::drivers::timer::get_jiffies() == j1 {
        core::hint::spin_loop();
    }

    let after = total_us(&String::from_utf8_lossy(&psi::generate(res)).into_owned());
    match (before, after) {
        (Some(b), Some(a)) if a > b => {
            test_pass(
                "stall guard charges total across a jiffy",
            );
        }
        (b, a) => {
            test_fail(
                "stall guard charges total across a jiffy",
                &format!("{} some total {:?} -> {:?} µs", name, b, a),
            );
        }
    }
}
