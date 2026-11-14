//! Reaper forensics — the OH launcher/systemui app-cycle death hunt
//! (wake-history TASK §110, the waitid leg of the reaper probe).
//!
//! The first leg (a32046b8d) printed reaps at the BLOCKING wait4 path
//! only: 17 reaps in 240 s, all clean EXITCODE helpers, and NO launcher
//! reap. The launcher/systemui app-cycle children of appspawn die with
//! childexit status=0xfffffffe (exit_group(-2): the raw user arg, which
//! the Linux ABI masks to WEXITSTATUS 254) and either die through a
//! waitid reap, a WNOHANG wait4 reap, or linger un-reaped as zombies
//! between dumps. This module completes the picture with four arms:
//!
//!   SPAWN   — a task naming itself com.* (PR_SET_NAME or exec basename):
//!             the appspawn-child incarnation clock starts here.
//!   ZEXIT   — every LEADER task entering ZOMBIE: pid, comm, decoded
//!             exit status, parent pid+comm, age since SPAWN, and the
//!             task's always-on last-syscall latch (what it was doing).
//!   REAP4/REAP4N/REAPW — every successful wait4 reap (blocking and the
//!             WNOHANG arm the first leg missed) and waitid reap
//!             (WNOWAIT peeks marked `-nw`): who reaped which child,
//!             status decoded. REAPW-POLL samples the no-event poll
//!             shape (caller + options) so poll loops are visible
//!             without flooding.
//!   ZLINGER — 1 Hz scan from the timer softirq: zombies un-reaped
//!             longer than ZLINGER_AFTER_SECS, with the parent's state
//!             and last-syscall latch (what the non-reaping parent is
//!             blocked on instead of reaping).
//!
//! Status decode reference (this kernel's encoding): sys_exit stores
//! `(status & 0xff) << 8`; a fatal signal stores `-signo`. wait4's
//! user wstatus uses the Linux wait-status layout (sig in bits 0-6,
//! core in bit 7, exit code in bits 8-15); waitid reports si_code
//! CLD_* (1 EXITED, 2 KILLED, 3 DUMPED, 5 STOPPED, 6 CONTINUED) and a
//! si_status that is the raw exit code (0-255) or term signal.
//!
//! Everything is OFF unless booted with `dfx=schedtrace` — the SAME
//! switch as the first-leg wait4 arm. The dfx switch table is frozen;
//! this is an extension of that existing gated probe, not a new switch.
//!
//! Locking: one Spinlock per map; sections are short copy in/out. The
//! timer-softirq scan uses the try-variant of the pid-hash walk (a
//! partial snapshot is fine — the next second retries).

use core::sync::atomic::Ordering;

/// Name-set (SPAWN) tracking capacity. Small: only com.*-prefixed names
/// are recorded (OH bundle names — appspawn children + a handful of
/// services), dozens per boot.
const NAME_MAP_LEN: usize = 32;

/// Zombie exit-time map capacity (pid → exit jiffies + code), for
/// ZLINGER age. Rotating overwrite: the ZEXIT prints carry the same
/// data durably; the map only has to survive the 10 s linger window.
const EXIT_MAP_LEN: usize = 96;

/// A zombie un-reaped longer than this is reported as lingering.
const ZLINGER_AFTER_SECS: u64 = 10;

/// Re-report a still-lingering zombie at this cadence (age growth).
const ZLINGER_REPEAT_SECS: u64 = 30;

/// Print budgets per arm (runaway pollers / exit storms must not eat
/// the serial; 240 s of app-cycle traffic sits far below all of them).
const MAX_SPAWN_PRINTS: u32 = 200;
const MAX_ZEXIT_PRINTS: u32 = 400;
const MAX_REAP4_PRINTS: u32 = 400;
const MAX_REAPW_PRINTS: u32 = 500;
const MAX_POLL_PRINTS: u32 = 12;
const MAX_ZLINGER_PRINTS: u32 = 96;

#[derive(Clone, Copy)]
struct NameRec {
    pid: u32,
    t_set: u64,
}

#[derive(Clone, Copy)]
struct ExitRec {
    pid: u32,
    t_exit: u64,
    code: i32,
    last_report: u64,
}

static NAME_MAP: crate::sync::spinlock::Spinlock<[NameRec; NAME_MAP_LEN]> =
    crate::sync::spinlock::Spinlock::new([NameRec { pid: 0, t_set: 0 }; NAME_MAP_LEN]);

static EXIT_MAP: crate::sync::spinlock::Spinlock<[ExitRec; EXIT_MAP_LEN]> =
    crate::sync::spinlock::Spinlock::new([ExitRec { pid: 0, t_exit: 0, code: 0, last_report: 0 }; EXIT_MAP_LEN]);

/// The gate for every arm here (dfx=schedtrace).
#[inline]
pub fn on() -> bool {
    crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SchedTrace)
}

/// jiffies → "123.4s" seconds string (one decimal).
fn secs(j: u64) -> alloc::string::String {
    let hz = crate::config::KERNEL_HZ as u64;
    let whole = j / hz;
    let tenths = (j % hz) * 10 / hz;
    alloc::format!("{}.{}s", whole, tenths)
}

/// Decode this kernel's internal exit_code: `>= 0` = `(status<<8)`,
/// `< 0` = death by signal `-code`.
fn decode_kernel_code(code: i32) -> (&'static str, i32) {
    if code < 0 {
        ("SIG", -code)
    } else {
        ("exit", (code >> 8) & 0xff)
    }
}

/// Decode a wait4 wstatus (Linux wait-status layout).
fn decode_wstatus(st: i32) -> (&'static str, i32) {
    let sig = st & 0x7f;
    if sig == 0 && st & 0xff00 != 0 {
        ("exit", (st >> 8) & 0xff)
    } else if sig == 0 {
        ("exit0", 0)
    } else if st & 0x80 != 0 {
        ("SIGcore", sig)
    } else {
        ("SIG", sig)
    }
}

/// Decode a waitid si_code (CLD_*).
fn decode_si_code(c: i32) -> &'static str {
    match c {
        1 => "CLD_EXITED",
        2 => "CLD_KILLED",
        3 => "CLD_DUMPED",
        5 => "CLD_STOPPED",
        6 => "CLD_CONTINUED",
        _ => "CLD_?",
    }
}

/// comm bytes → printable &str (NUL-trimmed, lossy).
fn comm_str(raw: &[u8]) -> &str {
    let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    core::str::from_utf8(&raw[..len]).unwrap_or("?")
}

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

    let now = crate::drivers::timer::get_jiffies();
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
        let ppid = crate::process::find_task_by_pid(pid)
            .map(|p| p.ppid())
            .unwrap_or(0);
        let pname = crate::process::find_task_by_pid(ppid)
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

    let now = crate::drivers::timer::get_jiffies();
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
    let pname = crate::process::find_task_by_pid(parent_pid)
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
    crate::process::find_task_by_pid(pid)
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
        secs(crate::drivers::timer::get_jiffies()),
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
        secs(crate::drivers::timer::get_jiffies()),
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
    let now = crate::drivers::timer::get_jiffies();
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

/// ZLINGER arm: 1 Hz zombie scan, called from the timer softirq under
/// the SchedTrace gate. Uses the try-variant pid-hash walk (a held
/// bucket lock means a partial snapshot — fine, next second retries).
pub fn zombie_scan(now: u64) {
    use core::sync::atomic::AtomicU32;

    static N: AtomicU32 = AtomicU32::new(0);

    // Collect under the bucket locks; print after (no printing while
    // holding a pid-hash bucket spinlock).
    let mut found: [(u32, i32, [u8; 16], u32, u32); 16] = [(0, 0, [0; 16], 0, 0); 16];
    let mut nfound = 0usize;
    crate::process::pid_hash::pid_hash_for_each_task_try(|t| unsafe {
        if nfound >= found.len() {
            return;
        }
        let task = &*t;
        if task.state() == crate::process::task::TaskState::new(
            crate::process::task::TaskState::ZOMBIE,
        ) && task.pid() != 0
        {
            found[nfound] = (
                task.pid(),
                task.exit_code(),
                *task.comm(),
                task.ppid(),
                task.last_syscall_nr.load(Ordering::Relaxed),
            );
            nfound += 1;
        }
    });

    if nfound == 0 {
        return;
    }

    let hz = crate::config::KERNEL_HZ as u64;
    for i in 0..nfound {
        let (pid, code, comm, ppid, _lastsys) = found[i];
        let (t_exit, last_report) = {
            let map = EXIT_MAP.lock();
            match map.iter().find(|r| r.pid == pid) {
                Some(r) => (r.t_exit, r.last_report),
                None => {
                    // Exited before the probe armed / map rotated away:
                    // lower-bound age from the scan's own first sight.
                    (0, 0)
                }
            }
        };
        let age = if t_exit != 0 {
            now.saturating_sub(t_exit)
        } else {
            0
        };
        let due = if t_exit != 0 {
            age >= ZLINGER_AFTER_SECS * hz
                && now.saturating_sub(last_report) >= ZLINGER_REPEAT_SECS * hz
        } else {
            // Unknown exit time: report at the repeat cadence only.
            now.saturating_sub(last_report) >= ZLINGER_REPEAT_SECS * hz
        };
        if !due || N.load(Ordering::Relaxed) >= MAX_ZLINGER_PRINTS {
            continue;
        }
        if N.fetch_add(1, Ordering::Relaxed) >= MAX_ZLINGER_PRINTS {
            break;
        }
        {
            let mut map = EXIT_MAP.lock();
            if let Some(slot) = map.iter_mut().find(|r| r.pid == pid) {
                slot.last_report = now;
            } else {
                // Unknown-exit zombie adopted into the map so the repeat
                // cadence applies to it too.
                let mut oldest = 0;
                for k in 1..EXIT_MAP_LEN {
                    if map[k].t_exit < map[oldest].t_exit {
                        oldest = k;
                    }
                }
                map[oldest] = ExitRec { pid, t_exit: 0, code, last_report: now };
            }
        }
        let (kind, val) = decode_kernel_code(code);
        // Name the non-reaping parent and what it is blocked on instead.
        let parent_desc = crate::process::find_task_by_pid(ppid)
            .map(|p| {
                alloc::format!(
                    "{}('{}' state={:#x} lastsys={} a0={:#x})",
                    ppid,
                    comm_str(p.comm()),
                    p.state().bits(),
                    p.last_syscall_nr.load(Ordering::Relaxed),
                    p.last_syscall_a0.load(Ordering::Relaxed)
                )
            })
            .unwrap_or_else(|| alloc::format!("{}(gone)", ppid));
        let linger = if age != 0 {
            secs(age)
        } else {
            alloc::string::String::from(">=scan-window")
        };
        crate::pr_warn!(
            "ZLINGER t={} pid={} comm='{}' parent={} {}={} linger={}",
            secs(now),
            pid,
            comm_str(&comm),
            parent_desc,
            kind,
            val,
            linger
        );
    }
}
