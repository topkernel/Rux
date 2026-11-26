//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The reaper PRINT arms (moved from kernel/core/src/debug/reaper.rs,
//! crate-flip rung debug): SPAWN/ZEXIT/REAP4/REAPW/POLL lines. The
//! on() gate, the ZLINGER 1 Hz scan and the EXIT_MAP/NAME_MAP statics
//! stay framework-side (timer + do_exit/wait paths); every arm here is
//! called through the framework's same-named wrappers, whose call sites
//! pre-gate on reaper::on() — the arms fire only on
//! EVENT_SCHEDTRACE-armed boots. Bodies moved verbatim.

use core::sync::atomic::Ordering;

use crate::debug::{
    hotpath::comm_str, hotpath::decode_kernel_code, hotpath::decode_si_code, hotpath::decode_wstatus, hotpath::secs, types::ExitRec, types::NameRec,
    hotpath::EXIT_MAP, hotpath::EXIT_MAP_LEN, hotpath::NAME_MAP, hotpath::NAME_MAP_LEN,
};

/// Print budgets per arm (runaway pollers / exit storms must not eat
/// the serial; 240 s of app-cycle traffic sits far below all of them).
const MAX_SPAWN_PRINTS: u32 = 200;
const MAX_ZEXIT_PRINTS: u32 = 400;
const MAX_REAP4_PRINTS: u32 = 400;
const MAX_REAPW_PRINTS: u32 = 500;
const MAX_POLL_PRINTS: u32 = 12;

/// SPAWN arm: a task named itself (PR_SET_NAME or exec basename). Only
/// OH-ish names are tracked — the observed first-boot shapes are the
/// 15-byte comm truncations "m.ohos.systemui" / "m.ohos.launcher" (the
/// runtime passes a 17-byte "com.ohos.<name>" whose comm-15 window
/// starts two bytes in) plus "com."-prefixed variants; a bare contains
/// match on "ohos" covers both without following every daemon rename.
pub fn note_nameset(pid: u32, comm: &[u8]) {
    use core::sync::atomic::AtomicU32;

    let name = comm_str(comm);
    if !(name.contains("ohos") || name.starts_with("com.")) {
        return;
    }

    let now = crate::timer::get_jiffies();
    {
        let mut map = NAME_MAP.lock();
        if let Some(slot) = map.iter_mut().find(|r| r.pid == pid) {
            slot.t_set = now;
        } else if let Some(slot) = map.iter_mut().find(|r| r.pid == 0) {
            slot.pid = pid;
            slot.t_set = now;
        } else {
            // Full: steal the oldest slot (forensic map, lossy is fine).
            let mut oldest = 0;
            for i in 1..NAME_MAP_LEN {
                if map[i].t_set < map[oldest].t_set {
                    oldest = i;
                }
            }
            map[oldest] = NameRec { pid, t_set: now };
        }
    }

    static N: AtomicU32 = AtomicU32::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < MAX_SPAWN_PRINTS {
        // Parent pid at nameset time (appspawn for app children).
        let ppid = crate::process::hotpath::find_task_by_pid(pid)
            .map(|p| p.ppid())
            .unwrap_or(0);
        let pname = crate::process::hotpath::find_task_by_pid(ppid)
            .map(|p| comm_str(p.comm()))
            .unwrap_or("?");
        crate::pr_warn!(
            "SPAWN t={} pid={} comm='{}' parent={}('{}')",
            secs(now),
            pid,
            name,
            ppid,
            pname
        );
    }
}

/// SPAWN lookup: nameset jiffies for a pid (age at exit).
fn nameset_of(pid: u32) -> Option<u64> {
    let map = NAME_MAP.lock();
    map.iter().find(|r| r.pid == pid).map(|r| r.t_set)
}

/// ZEXIT arm: a LEADER task entered ZOMBIE. Called from do_exit right
/// after the ZOMBIE store + notify resolution — the task is still
/// linked and its fields readable. `code` is this kernel's internal
/// exit_code; `last_nr`/`last_a0` are the task's always-on pre-translate
/// last-syscall latch (what it was in when it died).
pub fn on_zombie(pid: u32, code: i32, parent_pid: u32, comm: &[u8; 16], last_nr: u32, last_a0: u64) {
    use core::sync::atomic::AtomicU32;

    let now = crate::timer::get_jiffies();
    {
        let mut map = EXIT_MAP.lock();
        if let Some(slot) = map.iter_mut().find(|r| r.pid == pid) {
            slot.t_exit = now;
            slot.code = code;
            slot.last_report = 0;
        } else if let Some(slot) = map.iter_mut().find(|r| r.pid == 0) {
            slot.pid = pid;
            slot.t_exit = now;
            slot.code = code;
            slot.last_report = 0;
        } else {
            let mut oldest = 0;
            for i in 1..EXIT_MAP_LEN {
                if map[i].t_exit < map[oldest].t_exit {
                    oldest = i;
                }
            }
            map[oldest] = ExitRec { pid, t_exit: now, code, last_report: 0 };
        }
    }

    static N: AtomicU32 = AtomicU32::new(0);
    if N.fetch_add(1, Ordering::Relaxed) >= MAX_ZEXIT_PRINTS {
        return;
    }
    let (kind, val) = decode_kernel_code(code);
    let age = match nameset_of(pid) {
        Some(t0) => alloc::format!(" age={}", secs(now.saturating_sub(t0))),
        None => alloc::string::String::new(),
    };
    let pname = crate::process::hotpath::find_task_by_pid(parent_pid)
        .map(|p| comm_str(p.comm()))
        .unwrap_or("?");
    crate::pr_warn!(
        "ZEXIT t={} pid={} comm='{}' parent={}('{}') {}={}{} lastsys={} a0={:#x}",
        secs(now),
        pid,
        comm_str(comm),
        parent_pid,
        pname,
        kind,
        val,
        age,
        last_nr,
        last_a0
    );
}

/// The reaper's comm (lookup — the reaper is the CALLING task, alive).
fn comm_of(pid: u32) -> alloc::string::String {
    crate::process::hotpath::find_task_by_pid(pid)
        .map(|p| alloc::string::String::from(comm_str(p.comm())))
        .unwrap_or_else(|| alloc::string::String::from("?"))
}

/// REAP arm: successful wait4 reap. `nohang` distinguishes the blocking
/// do_wait path (REAP4) from the WNOHANG do_wait_nonblock path (REAP4N)
/// the first leg did not cover.
pub fn on_reap_wait4(reaper: u32, child: u32, wstatus: i32, nohang: bool) {
    use core::sync::atomic::AtomicU32;

    static N: AtomicU32 = AtomicU32::new(0);
    if N.fetch_add(1, Ordering::Relaxed) >= MAX_REAP4_PRINTS {
        return;
    }
    let (kind, val) = decode_wstatus(wstatus);
    crate::pr_warn!(
        "REAP4{} t={} reaper={}('{}') child={} wstatus={:#x} {}={}",
        if nohang { "N" } else { "" },
        secs(crate::timer::get_jiffies()),
        reaper,
        comm_of(reaper),
        child,
        wstatus as u32 as i32,
        kind,
        val
    );
}

/// REAP arm: successful waitid event. `wnowait` marks a peek that left
/// the child reapable; `via_pidfd` marks the pidfd idtype arm.
pub fn on_reap_waitid(
    reaper: u32,
    si_pid: u32,
    si_code: i32,
    si_status: i32,
    idtype: i32,
    id: i32,
    options: i32,
    wnowait: bool,
    via_pidfd: bool,
) {
    use core::sync::atomic::AtomicU32;

    static N: AtomicU32 = AtomicU32::new(0);
    if N.fetch_add(1, Ordering::Relaxed) >= MAX_REAPW_PRINTS {
        return;
    }
    crate::pr_warn!(
        "REAPW{} t={} reaper={}('{}') idtype={} id={} opts={:#x} si_pid={} {} status={}{}",
        if wnowait { "-nw" } else { "" },
        secs(crate::timer::get_jiffies()),
        reaper,
        comm_of(reaper),
        if via_pidfd { -1 } else { idtype },
        id,
        options,
        si_pid,
        decode_si_code(si_code),
        si_status,
        if via_pidfd { " via=pidfd" } else { "" }
    );
}

/// Sampled no-event waitid print: proves the poll shape (who polls,
/// with what options) without flooding — rate-limited, hard-capped.
pub fn poll_note(reaper: u32, idtype: i32, id: i32, options: i32) {
    use core::sync::atomic::{AtomicU32, AtomicU64};

    static N: AtomicU32 = AtomicU32::new(0);
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = crate::timer::get_jiffies();
    let last = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 5 * crate::config::KERNEL_HZ as u64 {
        return;
    }
    if LAST
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    if N.fetch_add(1, Ordering::Relaxed) >= MAX_POLL_PRINTS {
        return;
    }
    crate::pr_warn!(
        "REAPW-POLL t={} reaper={}('{}') idtype={} id={} opts={:#x} (no event)",
        secs(now),
        reaper,
        comm_of(reaper),
        idtype,
        id,
        options
    );
}
