//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Whole-procfs renderer coverage (tests/procfs_all.rs).
//!
//! The coverage boot's worst-files list is dominated by /proc renderers
//! that sat at 0%: uptime, version, loadavg, vmstat, diskstats,
//! softirqs, swaps, locks, kallsyms, cmdline, configgz, timer_list,
//! self_proc, the sysctl files, and every /proc/net table — each is a
//! pure `generate() -> Vec<u8>` over kernel state, so one kernel-side
//! call per file exercises its full render path. This file drives them
//! all, asserts UTF-8 output (every one of these files must be
//! text-parseable by userland), and pins the few stable shapes the
//! ABI promises (meminfo's MemTotal row, uptime's two-column line).

use alloc::string::String;

fn utf8_ok(bytes: &[u8]) -> bool {
    core::str::from_utf8(bytes).is_ok()
}

pub fn test_procfs_all() {
    crate::tests::test_group_start("procfs: all renderers");

    // ---- /proc/uptime ---------------------------------------------------
    crate::fs::procfs::uptime::account_idle_cycles(0, 1234);
    let idle = crate::fs::procfs::uptime::total_idle_cycles();
    crate::test_assert!(idle >= 1234, "proc/uptime: idle accounting");
    let out = crate::fs::procfs::uptime::generate();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/uptime: render");
    crate::test_assert!(
        String::from_utf8_lossy(&out).split_whitespace().count() >= 2,
        "proc/uptime: two-column shape"
    );

    // ---- /proc/version --------------------------------------------------
    let out = crate::fs::procfs::version::generate();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/version: render");
    crate::test_assert!(
        !crate::fs::procfs::version::get_version_string().is_empty(),
        "proc/version: version string"
    );
    crate::test_assert!(
        !crate::fs::procfs::version::get_release_string().is_empty(),
        "proc/version: release string"
    );

    // ---- /proc/loadavg --------------------------------------------------
    crate::fs::procfs::loadavg::update_load_avg();
    let out = crate::fs::procfs::loadavg::generate();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/loadavg: render");

    // ---- /proc/vmstat + /proc/buddyinfo ---------------------------------
    let out = crate::fs::procfs::vmstat::generate();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/vmstat: render");
    let out = crate::fs::procfs::vmstat::generate_buddyinfo();
    crate::test_assert!(utf8_ok(&out), "proc/buddyinfo: render");

    // ---- /proc/diskstats -------------------------------------------------
    let out = crate::fs::procfs::diskstats::generate();
    crate::test_assert!(utf8_ok(&out), "proc/diskstats: render");

    // ---- /proc/softirqs + /proc/interrupts -------------------------------
    let out = crate::fs::procfs::softirqs::generate();
    crate::test_assert!(utf8_ok(&out), "proc/softirqs: render");
    crate::fs::procfs::interrupts::timer_inc(0);
    crate::fs::procfs::interrupts::soft_inc(0);
    let out = crate::fs::procfs::interrupts::generate();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/interrupts: render");

    // ---- /proc/swaps -----------------------------------------------------
    let out = crate::fs::procfs::swaps::generate();
    crate::test_assert!(utf8_ok(&out), "proc/swaps: render");

    // ---- /proc/locks -----------------------------------------------------
    let out = crate::fs::procfs::locks::generate();
    crate::test_assert!(utf8_ok(&out), "proc/locks: generate");
    let s = crate::fs::procfs::locks::render();
    crate::test_assert!(utf8_ok(s.as_bytes()), "proc/locks: render");

    // ---- /proc/kallsyms (may legitimately be empty when the symbol
    //      table is compiled off; the render path itself must run) --------
    let out = crate::fs::procfs::kallsyms::generate();
    crate::test_assert!(utf8_ok(&out), "proc/kallsyms: render");

    // ---- /proc/cmdline ---------------------------------------------------
    let out = crate::fs::procfs::cmdline::generate();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/cmdline: render");
    crate::test_assert!(
        !crate::fs::procfs::cmdline::get_cmdline().is_empty(),
        "proc/cmdline: get_cmdline"
    );

    // ---- /proc/config.gz (uncompressed text payload) ---------------------
    let out = crate::fs::procfs::configgz::generate();
    crate::test_assert!(utf8_ok(&out), "proc/configgz: render");

    // ---- /proc/timer_list ------------------------------------------------
    let out = crate::fs::procfs::timer_list::generate();
    crate::test_assert!(utf8_ok(&out), "proc/timer_list: render");

    // ---- /proc/meminfo ---------------------------------------------------
    let out = crate::fs::procfs::meminfo::generate();
    let text = String::from_utf8_lossy(&out);
    crate::test_assert!(
        !out.is_empty() && text.contains("MemTotal"),
        "proc/meminfo: MemTotal row"
    );

    // ---- /proc/self/... (symlink targets + cwd-link generate) -----------
    let out = crate::fs::procfs::self_proc::get_self_link();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/self: link target");
    let out = crate::fs::procfs::self_proc::get_thread_self_link();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/thread-self: link target");
    let out = crate::fs::procfs::self_proc::generate();
    crate::test_assert!(utf8_ok(&out), "proc/self: generate");

    // ---- /proc/schedstat --------------------------------------------------
    crate::fs::procfs::schedstat::note_yield(0);
    crate::fs::procfs::schedstat::note_sched_run(0);
    let out = crate::fs::procfs::schedstat::generate();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/schedstat: render");

    // ---- /proc/stat (aggregates; note_switch needs a &Task, driven by
    //      the real scheduler path instead) --------------------------------
    crate::fs::procfs::stat::charge_tick(false);
    crate::fs::procfs::stat::charge_tick(true);
    let _ = crate::fs::procfs::stat::total_context_switches();
    let _ = crate::fs::procfs::stat::total_forks();
    let out = crate::fs::procfs::stat::generate();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/stat: render");

    // ---- /proc/mounts + /proc/filesystems + /proc/self/mountinfo ---------
    let out = crate::fs::procfs::mounts::generate();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/mounts: render");
    let out = crate::fs::procfs::mounts::generate_filesystems();
    crate::test_assert!(!out.is_empty() && utf8_ok(&out), "proc/filesystems: render");
    let out = crate::fs::procfs::mounts::generate_mountinfo();
    crate::test_assert!(utf8_ok(&out), "proc/mountinfo: render");

    // ---- /proc/sys/kernel/hostname + domainname --------------------------
    let out = crate::fs::procfs::sysctl::generate_hostname();
    crate::test_assert!(utf8_ok(&out), "sysctl: hostname");
    let out = crate::fs::procfs::sysctl::generate_domainname();
    crate::test_assert!(utf8_ok(&out), "sysctl: domainname");

    // ---- /proc/sysrq-trigger (write side; 'h' = help, non-destructive) ---
    let n = crate::fs::procfs::sysrq::write_trigger(b"h\n");
    crate::test_assert!(n == 2, "sysrq: help trigger accepted");
    let n = crate::fs::procfs::sysrq::write_trigger(b"\n");
    crate::test_assert!(n == 1, "sysrq: empty write is a no-op");

    // ---- /proc/net tables -------------------------------------------------
    let out = crate::fs::procfs::net::generate_dev();
    crate::test_assert!(utf8_ok(&out), "proc/net/dev: render");
    let out = crate::fs::procfs::net::generate_tcp();
    crate::test_assert!(utf8_ok(&out), "proc/net/tcp: render");
    let out = crate::fs::procfs::net::generate_tcp6();
    crate::test_assert!(utf8_ok(&out), "proc/net/tcp6: render");
    let out = crate::fs::procfs::net::generate_udp();
    crate::test_assert!(utf8_ok(&out), "proc/net/udp: render");
    let out = crate::fs::procfs::net::generate_udp6();
    crate::test_assert!(utf8_ok(&out), "proc/net/udp6: render");
    let out = crate::fs::procfs::net::generate_arp();
    crate::test_assert!(utf8_ok(&out), "proc/net/arp: render");
    let out = crate::fs::procfs::net::generate_route();
    crate::test_assert!(utf8_ok(&out), "proc/net/route: render");
    let out = crate::fs::procfs::net::generate_fib_trie();
    crate::test_assert!(utf8_ok(&out), "proc/net/fib_trie: render");
    let out = crate::fs::procfs::net::generate_sockstat();
    crate::test_assert!(utf8_ok(&out), "proc/net/sockstat: render");
    let out = crate::fs::procfs::net::generate_unix();
    crate::test_assert!(utf8_ok(&out), "proc/net/unix: render");
}
