//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
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
//! Everything is OFF unless booted with `rux:schedtrace (retired debug=schedtrace)` — the SAME
//! switch as the first-leg wait4 arm. The debug switch table is frozen;
//! this is an extension of that existing gated probe, not a new switch.
//!
//! Locking: one Spinlock per map; sections are short copy in/out. The
//! timer-softirq scan uses the try-variant of the pid-hash walk (a
//! partial snapshot is fine — the next second retries).
//!
//! CRATE-FLIP RUNG debug: the SPAWN/ZEXIT/REAP4/REAPW/POLL print
//! arms moved to the debug service crate (kernel/debug/src/
//! reaper.rs) — every arm fires only through these same-named wrappers,
//! whose call sites (do_exit, exec, the wait4/waitid syscall bodies)
//! pre-gate on reaper::on(), so the row dispatch is armed-boot-only.
//! THIS file keeps the on() gate, the 1 Hz ZLINGER scan (timer
//! softirq) and the EXIT_MAP/NAME_MAP statics (pub peek + write
//! surface for the crate's arms).

#![forbid(unsafe_code)]
use core::sync::atomic::Ordering;

/// Name-set (SPAWN) tracking capacity. Small: only com.*-prefixed names
/// are recorded (OH bundle names — appspawn children + a handful of
/// services), dozens per boot. PUB: the debug arms index it.
pub const NAME_MAP_LEN: usize = 32;

/// Zombie exit-time map capacity (pid → exit jiffies + code), for
/// ZLINGER age. Rotating overwrite: the ZEXIT prints carry the same
/// data durably; the map only has to survive the 10 s linger window.
/// PUB: the debug arms index it.
pub const EXIT_MAP_LEN: usize = 96;

/// A zombie un-reaped longer than this is reported as lingering.
const ZLINGER_AFTER_SECS: u64 = 10;

/// Re-report a still-lingering zombie at this cadence (age growth).
const ZLINGER_REPEAT_SECS: u64 = 30;

/// Print budget for the ZLINGER arm (runaway exit storms must not eat
/// the serial).
const MAX_ZLINGER_PRINTS: u32 = 96;

/// PUB write/peek surface (crate-flip rung debug): the debug crate's
/// SPAWN/ZEXIT arms lock and write these maps; the ZLINGER scan below
/// reads EXIT_MAP under the same lock.
#[derive(Clone, Copy)]
pub struct NameRec {
    pub pid: u32,
    pub t_set: u64,
}

#[derive(Clone, Copy)]
pub struct ExitRec {
    pub pid: u32,
    pub t_exit: u64,
    pub code: i32,
    pub last_report: u64,
}

pub static NAME_MAP: crate::sync::spinlock::Spinlock<[NameRec; NAME_MAP_LEN]> =
    crate::sync::spinlock::Spinlock::new([NameRec { pid: 0, t_set: 0 }; NAME_MAP_LEN]);

pub static EXIT_MAP: crate::sync::spinlock::Spinlock<[ExitRec; EXIT_MAP_LEN]> =
    crate::sync::spinlock::Spinlock::new([ExitRec { pid: 0, t_exit: 0, code: 0, last_report: 0 }; EXIT_MAP_LEN]);

/// The gate for every arm here (rux:schedtrace (retired debug=schedtrace)).
#[inline]
pub fn on() -> bool {
    crate::trace::EVENT_SCHEDTRACE.armed()
}

/// jiffies → "123.4s" seconds string (one decimal). PUB: the debug
/// arms print with it.
pub fn secs(j: u64) -> alloc::string::String {
    let hz = crate::config::KERNEL_HZ as u64;
    let whole = j / hz;
    let tenths = (j % hz) * 10 / hz;
    alloc::format!("{}.{}s", whole, tenths)
}

/// Decode this kernel's internal exit_code: `>= 0` = `(status<<8)`,
/// `< 0` = death by signal `-code`. PUB (debug arms).
pub fn decode_kernel_code(code: i32) -> (&'static str, i32) {
    if code < 0 {
        ("SIG", -code)
    } else {
        ("exit", (code >> 8) & 0xff)
    }
}

/// Decode a wait4 wstatus (Linux wait-status layout). PUB (debug arms).
pub fn decode_wstatus(st: i32) -> (&'static str, i32) {
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

/// Decode a waitid si_code (CLD_*). PUB (debug arms).
pub fn decode_si_code(c: i32) -> &'static str {
    match c {
        1 => "CLD_EXITED",
        2 => "CLD_KILLED",
        3 => "CLD_DUMPED",
        5 => "CLD_STOPPED",
        6 => "CLD_CONTINUED",
        _ => "CLD-?",
    }
}

/// comm bytes → printable &str (NUL-trimmed, lossy). PUB (debug arms).
pub fn comm_str(raw: &[u8]) -> &str {
    let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    core::str::from_utf8(&raw[..len]).unwrap_or("?")
}

/// SPAWN arm (moved to the debug crate): same-named wrapper; the call
/// sites (exec basename / PR_SET_NAME) pre-gate on on().
pub fn note_nameset(pid: u32, comm: &[u8]) {
    crate::debug::debug_hooks::reaper_note_nameset(pid, comm)
}

/// ZEXIT arm (moved to the debug crate): same-named wrapper; called
/// from do_exit right after the ZOMBIE store + notify resolution (the
/// call site pre-gates on on()).
pub fn on_zombie(pid: u32, code: i32, parent_pid: u32, comm: &[u8; 16], last_nr: u32, last_a0: u64) {
    crate::debug::debug_hooks::reaper_on_zombie(pid, code, parent_pid, comm, last_nr, last_a0)
}

/// REAP arm: successful wait4 reap (moved to the debug crate):
/// same-named wrapper; `nohang` distinguishes the blocking do_wait path
/// (REAP4) from the WNOHANG do_wait_nonblock path (REAP4N).
pub fn on_reap_wait4(reaper: u32, child: u32, wstatus: i32, nohang: bool) {
    crate::debug::debug_hooks::reaper_on_reap_wait4(reaper, child, wstatus, nohang)
}

/// REAP arm: successful waitid event (moved to the debug crate):
/// same-named wrapper; `wnowait` marks a peek that left the child
/// reapable; `via_pidfd` marks the pidfd idtype arm.
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
    crate::debug::debug_hooks::reaper_on_reap_waitid(
        reaper,
        si_pid,
        si_code,
        si_status,
        idtype,
        id,
        options,
        wnowait,
        via_pidfd,
    )
}

/// Sampled no-event waitid print (moved to the debug crate):
/// same-named wrapper; proves the poll shape without flooding.
pub fn poll_note(reaper: u32, idtype: i32, id: i32, options: i32) {
    crate::debug::debug_hooks::reaper_poll_note(reaper, idtype, id, options)
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
    crate::process::pid_hash::pid_hash_for_each_task_try(|t| {
        if nfound >= found.len() {
            return;
        }
        let task = t;
        if task.state() == crate::process::task::TaskState::new(
            crate::process::task::TaskState::ZOMBIE,
        ) && task.pid() != 0
        {
            found[nfound] = (
                task.pid(),
                task.exit_code(),
                *task.comm(),
                task.ppid(),
                task.ext.last_syscall_nr.load(Ordering::Relaxed),
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
                    p.ext.last_syscall_nr.load(Ordering::Relaxed),
                    p.ext.last_syscall_a0.load(Ordering::Relaxed)
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
