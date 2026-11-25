//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Process/thread exit and wait implementation
//!
//! - do_exit: termination with thread-group semantics
//!   - non-leader thread: ring detach + deferred release (no ZOMBIE —
//!     threads are never wait4()ed; Linux delay_put analogue)
//!   - leader (or single-threaded): force SIGKILL remaining members,
//!     wait for the group to drain, then the classic ZOMBIE flow
//! - do_exit_group / de_thread: group teardown entry points (syscall 94,
//!   execve, fatal default signals)
//! - release_task: Reap zombie child resources (incl. exited group members)
//! - do_wait / do_wait_nonblock / do_waitid: wait for child state change

#![forbid(unsafe_code)]
use crate::errno;
use crate::process::task::{Pid, Task, TaskState};

pub fn do_exit(exit_code: i32) -> ! {
    let cur: &mut Task = match crate::sched::hotpath::current() {
        Some(c) => c,
        None => {
            loop {
                // SAFETY: `wfi` is a plain hint instruction with no side effects.
                crate::arch::cpu::wfi();
            }
        }
    };

    // Leg-52 item 0 (fleet s19/s24-r30, stress-lane A/B verdict):
    // release the direct-reclaim gate if THIS task dies while still
    // holding it. The gate's CAS (mm/page_alloc.rs) has exits only at
    // the reclaim block's own returns; a task killed mid-block funnels
    // here without unwinding them, which used to leave IN_DIRECT_RECLAIM
    // stuck at 1 forever — every later allocation then took the
    // no-drain skip path (the fake-OOM family). One CAS on our own pid:
    // a gate already released or held by another task is untouched.
    // SAFETY: current is the raw pointer to the calling task, valid
    // because we are it and do_exit never returns.
    let dying_pid = cur.pid();
    crate::mm::page_alloc::reclaim_gate_exit_release(dying_pid);

    // ub-gfx (greeter-chain forensics): name the death of every gdm
    // ladder / accounts-daemon / Xorg process with its exit code, so the
    // serial alone answers "what killed the greeter". Budgeted 120.
    {
        static XIT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let n = XIT_N.load(core::sync::atomic::Ordering::Relaxed);
        if n < 120 {
            // SAFETY: current is valid (we are it); comm() reads the
            // task's own fixed comm buffer.
            let comm = cur.comm();
            let name: &[u8] = &comm[..comm.iter().position(|&b| b == 0).unwrap_or(comm.len())];
            let interesting = name.starts_with(b"gdm")
                || name.starts_with(b"Xorg")
                || name.starts_with(b"gnome")
                || name.starts_with(b"accounts")
                || name.starts_with(b"(s-")  // systemd's pre-exec helpers
                || name.starts_with(b"avahi-d");
            if interesting {
                if XIT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 120 {
                    crate::pr_info!(
                        "GFXEXIT #{} pid={} comm={:?} code={} ({:#x})",
                        XIT_N.load(core::sync::atomic::Ordering::Relaxed),
                        dying_pid,
                        core::str::from_utf8(name).unwrap_or("?"),
                        exit_code,
                        exit_code
                    );
                }
            }
        }
    }

    // R127 wake-drop hunt: the exit-leak detector. A nonzero wq_regs at
    // exit means this task is going away with wait-queue entries still
    // naming it — the park was never unwound (no remove/finish_wait
    // ran). Those entries go stale the moment the Task page is freed
    // and become the pointer-recycling theft / wrong-task-wake class
    // the R127 screen in wait.rs evicts. Gated by the hunt's tracepoint
    // (input:evdev_wake armed via trace_event=); raw-console, budgeted.
    {
        let regs = cur.wq_regs.load(core::sync::atomic::Ordering::Relaxed);
        if regs != 0 && crate::trace::EVENT_EVDEV_WAKE.armed() {
            use core::sync::atomic::{AtomicU32, Ordering as Rel};
            static WQL_N: AtomicU32 = AtomicU32::new(0);
            let n = WQL_N.fetch_add(1, Rel::Relaxed);
            if n < 64 {
                let comm = cur.comm_str();
                let line: alloc::string::String =
                    alloc::format!("WQLEAK[{}] pid={} regs={} comm='{}'\n", n, dying_pid, regs, comm);
                // Raw UART, one write per line: readable at any loglevel.
                // Routed through the taskdump funnel (obs-hygiene): the
                // line stages and flushes atomically instead of per-byte
                // uart_putc_raw, which the OH kmsg flood spliced mid-line
                // on SMP8.
                crate::debug::taskdump::taskdump_raw_line(line.as_bytes());
            }
        }
    }

    // perf EXIT sideband (hiperf process tracking; no-op unless a perf
    // event is armed).
    crate::perf::hook_exit(cur.tgid(), dying_pid);

    // huks-wedge lane (hw2): death-while-holding decode. The wedge's
    // recurring shape ends with a lock-holder thread that VANISHES from
    // every later taskdump while the frozen word keeps its locked value
    // (hw1w1: distributeddata tid 1068 acquired 0x7ff0005f7d94 at
    // 66.8 s and was gone by ~88 s; hw4w1: huks_service tid 477 took a
    // handled stack-boundary SEGV at 9.0 s and was gone by 34 s while
    // two musl __lock words sat at 0x2 forever). The EXIT CAUSE decides
    // the verdict — decode it per holder-class thread:
    //   exit_code < 0            -> death by signal (-exit_code); the
    //                              queued siginfo names the SENDER.
    //   group_exiting && code>=0 -> exit_group(n) (n = code>>8), the
    //                              leader or a fatal group kill.
    //   !leader && !group_exiting-> a single worker/thread exit
    //                              (SYS_exit, status code>>8) — the
    //                              "clean exit that skipped the unlock"
    //                              face, incl. post-SEGV handler unwind
    //                              (see the segv_age field).
    // Fires for: the fuxwatch acquire-latched owner, threads of the
    // park-latched tgid, threads of a tgid with a standing frozen word,
    // and any thread with a delivered-SIGSEGV history. FuxStuck-gated,
    // 96/boot.
    {
        use core::sync::atomic::Ordering::Relaxed as Relaxed2;
        static HWEXIT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if crate::trace::EVENT_FUXSTUCK.armed()
            && HWEXIT_N.fetch_add(1, Relaxed2) < 96
        {
            let owner_tid = crate::sync::futex::FUXWATCH_OWNER_TID.load(Relaxed2);
            let park_tgid = crate::sync::futex::FUXPARK_TGID.load(Relaxed2);
            let my_tgid = cur.tgid() as u32;
            let is_leader = dying_pid == my_tgid;
            let frozen = crate::api::sync::rwfrz_tgid_frozen(my_tgid);
            #[cfg(feature = "x86_64")]
            let segv = crate::arch::x86_64::trap::segv_hist_age(dying_pid);
            #[cfg(not(feature = "x86_64"))]
            let segv: Option<(u64, u64)> = None;
            // Edge B (114th): the txnhunt LIVE OWING arm — a thread (or
            // any thread of a tgid) the audit last saw owing a sync
            // binder reply. The frozen-word arm below is blind for the
            // first 60 s of a wedge; this arm catches the silent
            // deaths the 113rd could not see. Snapshot read is
            // lock-free (see binder::txn_owing_of).
            let (owing_n, owing_txn, owing_stid) =
                crate::ipc::binder::txn_owing_of(my_tgid, dying_pid);
            let hit = (owner_tid != 0 && dying_pid == owner_tid)
                || (park_tgid != 0 && my_tgid == park_tgid)
                || (!is_leader && frozen)
                || segv.is_some()
                || owing_n > 0;
            if hit {
                let comm_b = cur.comm();
                let comm = core::str::from_utf8(comm_b.as_slice())
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let lastsys = cur.ext.last_syscall_nr.load(Relaxed2);
                let pending = cur.pending.signal.load(Relaxed2);
                // Group-exit state: distinguishes exit_group/fatal-group
                // (do_exit_group publishes it) from a single thread exit.
                let (group_exiting, _group_code) = cur.signal_arc()
                    .map(|ss| {
                        (
                            ss.group_exiting.load(Relaxed2),
                            ss.group_exit_code.load(Relaxed2),
                        )
                    })
                    .unwrap_or((false, 0));
                let cause = if exit_code < 0 {
                    "signal"
                } else if group_exiting {
                    "exit_group"
                } else if !is_leader {
                    "thread-exit"
                } else {
                    "exit"
                };
                // Sender forensics for signal deaths (kill/tgkill carry
                // si_pid; kernel-originated faults print si_code).
                let (sender, sicode) = if exit_code < 0 {
                    cur.pending
                        .peek_info(-exit_code)
                        .map(|si| (si.si_pid, si.si_code))
                        .unwrap_or((0, 0))
                } else {
                    (0, 0)
                };
                crate::pr_warn!(
                    "HW-EXIT-OF-HOLDER tid={} comm='{}' tgid={} code={:#x} cause={} sig={} sender={} sicode={:#x} leader={} grpexit={} lastsys={} pend={:#x} segv_age_j={} segv_cr2lo={:#x} ownerlatch={} parklatch={} owing_n={} owing_txn={} owing_stid={}",
                    dying_pid,
                    comm,
                    my_tgid,
                    exit_code,
                    cause,
                    if exit_code < 0 { -exit_code } else { 0 },
                    sender,
                    sicode,
                    is_leader,
                    group_exiting,
                    lastsys as i64,
                    pending,
                    segv.map(|s| s.0).unwrap_or(0),
                    segv.map(|s| s.1).unwrap_or(0),
                    owner_tid,
                    park_tgid,
                    owing_n,
                    owing_txn,
                    owing_stid
                );
            }
        }
    }

    // mmi-endgame (116th): the UIEXIT comm arm — clean-death decode for
    // the UI-phase process family. The 115th's boots ended with a
    // system-wide death event at ~136-190 s (hw10w1: ueventd,
    // watchdog_servic, storage_daemon, hdf_devmgr, every *_host,
    // ui_service, render_service, allocator/composer_host,
    // uinput_inject, multimodalinput, bootanimation, then a second wave
    // ending with hilogd) with NO fault rows, NO SIGCHLD kmsg and no
    // reaping — every victim was PARKED (futex/BWR/poll/epoll), so they
    // were killed, not voluntarily exited — and the holder print's
    // owing/frozen/segv arms matched none of them (and its 96-row
    // window is consumed by ALL exits, so it goes blind mid-boot).
    // This arm fires for ANY thread exit whose comm — or whose tgid
    // LEADER's comm — names the UI-phase family, with its OWN budget,
    // and decodes the death: exit code + cause (signal: si_pid names
    // the SENDER; exit_group; thread-exit), the exit CALLER's user
    // return address (pt_regs at do_exit: the instruction after the
    // exit/exit_group syscall site in libc, or the interruption point
    // of a killed thread), the last-syscall latch with args, the parent
    // (pid+comm) that will or will not reap, and the txnhunt owing
    // snapshot. FuxStuck-gated like the holder block above.
    {
        use core::sync::atomic::Ordering::Relaxed as Rel;
        static UIEXIT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        // early30 lane (120th): + samgr, huks_service — the early-30s
        // intermittent death window's two observed faces (hw10w3 huks
        // SIGABRT 32-34 s; mew3 samgr SIGSEGV 35.26 s) had NO comm arm,
        // so the SIGABRT leader death printed nothing kernel-side (no
        // SEGV history, no owing latch). Same budget, same gating.
        const UIEXIT_COMMS: [&str; 20] = [
            "multimodalinput",
            "uinput_inject",
            "mmi_service",
            "mmi",
            "bootanimation",
            "render_service",
            "composer_host",
            "allocator_host",
            "ui_service",
            "foundation",
            "input_user_host",
            "inputmethod_ser",
            "hdf_devmgr",
            "storage_daemon",
            "ueventd",
            "watchdog_servic",
            "hilogd",
            "sensors",
            "samgr",
            "huks_service",
        ];
        if crate::trace::EVENT_FUXSTUCK.armed()
            && UIEXIT_N.load(Rel) < 96
        {
            let comm_matches = |raw: &[u8; 16]| -> bool {
                let name = core::str::from_utf8(raw).unwrap_or("").trim_end_matches('\0');
                UIEXIT_COMMS.iter().any(|&w| name == w)
            };
            // SAFETY: current is the running (exiting) task; the leader
            // pointer is fixed up by group_leader_ptr() and stays valid
            // until the group is reaped — later than this point.
            let my_comm = cur.comm();
            let leader = cur.group_leader_ptr();
            let leader_comm: &[u8; 16] = if leader.is_null() {
                my_comm
            } else {
                crate::process::r#unsafe::borrow_task(leader).comm()
            };
            let my_named = comm_matches(my_comm);
            if my_named || comm_matches(leader_comm) {
                // Churn filter (first matrix's lesson): the UI processes
                // shed dozens of FFRT worker threads during startup —
                // clean single-thread exit_group(0)-free exits that
                // burned the whole budget in the first 10 s. A thread
                // whose OWN comm is on the list (mmi_service-style
                // named threads) always prints; a match via the LEADER
                // comm alone prints only for non-benign deaths (signal,
                // group exit, leader death, nonzero code).
                let my_tgid_early = cur.tgid() as u32;
                let is_leader_early = dying_pid == my_tgid_early;
                let grp_early = cur
                    .signal_arc()
                    .map(|ss| ss.group_exiting.load(Rel))
                    .unwrap_or(false);
                let benign_churn =
                    !my_named && exit_code == 0 && !grp_early && !is_leader_early;
                if !benign_churn {
                let n = UIEXIT_N.fetch_add(1, Rel);
                if n < 96 {
                    let comm = core::str::from_utf8(my_comm)
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    let leader_name = core::str::from_utf8(leader_comm)
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    let my_tgid = cur.tgid() as u32;
                    let is_leader = dying_pid == my_tgid;
                    // Parent at death time (pid + comm for the reaping
                    // question). None/implausible = racing reaper or an
                    // already-freed parent pointer.
                    let (ppid, pcomm) = match cur.parent_ptr() {
                        Some(pp) if Task::is_plausible_task_ptr(pp) => {
                            let pp_ref = crate::process::r#unsafe::borrow_task(pp as *mut Task);
                            (
                                pp_ref.pid(),
                                alloc::string::String::from(
                                    core::str::from_utf8(pp_ref.comm())
                                        .unwrap_or("")
                                        .trim_end_matches('\0'),
                                ),
                            )
                        },
                        _ => (0, alloc::string::String::from("?")),
                    };
                    let lastsys = cur.ext.last_syscall_nr.load(Rel) as i64;
                    let la0 = cur.ext.last_syscall_a0.load(Rel);
                    let la1 = cur.ext.last_syscall_a1.load(Rel);
                    let (group_exiting, _group_code) = cur.signal_arc()
                        .map(|ss| {
                            (ss.group_exiting.load(Rel), ss.group_exit_code.load(Rel))
                        })
                        .unwrap_or((false, 0));
                    let cause = if exit_code < 0 {
                        "signal"
                    } else if group_exiting {
                        "exit_group"
                    } else if !is_leader {
                        "thread-exit"
                    } else {
                        "exit"
                    };
                    // Sender forensics for signal deaths (kill/tgkill
                    // carry si_pid; kernel-forced kills print si_code).
                    let (sender, sicode) = if exit_code < 0 {
                        cur.pending
                            .peek_info(-exit_code)
                            .map(|si| (si.si_pid, si.si_code))
                            .unwrap_or((0, 0))
                    } else {
                        (0, 0)
                    };
                    // The exit caller's user return address (see the
                    // RWFRZ-THREADEXIT precedent above for the cfg split).
                    let exrip = match cur.pt_regs_ref() {
                        Some(pr) => crate::process::r#unsafe::pt_user_pc(pr),
                        None => 0u64,
                    };
                    let (owing_n, owing_txn, owing_stid) =
                        crate::ipc::binder::txn_owing_of(my_tgid, dying_pid);
                    // wrl-class lane: owing_txn without owing_stid conflates
                    // the thread's OWN debt with the tgid-wide fallback —
                    // txn_owing_of returns the first txn owed by ANY thread
                    // of the tgid when this tid has no exact match (f2's
                    // tid-1523 exit printed owing txn4570, which its live
                    // sibling 975 owed; the neverword lane read that as
                    // "handler exits mid-transaction"). own=1 marks the
                    // exact-tid match; own=0 means a sibling owes it.
                    let owing_own = owing_stid != 0 && owing_stid == dying_pid;
                    // HOTPATH-PRINT: rux:fuxstuck (retired debug=fuxstuck)-gated UI-family exits.
                    crate::pr_warn!(
                        "UIEXIT[{}] tid={} comm='{}' tgid={} leader='{}' ppid={}('{}') code={:#x} cause={} sig={} sender={} sicode={:#x} grpexit={} leader={} lastsys={}({:#x},{:#x}) exrip={:#x} owing_n={} owing_txn={} owing_stid={} own={}",
                        n,
                        dying_pid,
                        comm,
                        my_tgid,
                        leader_name,
                        ppid,
                        pcomm,
                        exit_code,
                        cause,
                        if exit_code < 0 { -exit_code } else { 0 },
                        sender,
                        sicode,
                        group_exiting,
                        is_leader,
                        lastsys,
                        la0,
                        la1,
                        exrip,
                        owing_n,
                        owing_txn,
                        owing_stid,
                        if owing_own { 1 } else { 0 }
                    );
                }
                }
            }
        }
    }

    // sc57 (57th session): thread-exit forensics for the OH wedge —
    // an FFRT/binder worker that exits while holding a userspace
    // ffrt::mutex leaves every later locker parked forever with no
    // kernel-visible holder (the mutex has no owner field in the
    // image's libffrt). rux:txnhunt (retired debug=txnhunt) prints every NON-LEADER exit in
    // the registered samgr pid, bounded to 64/boot.
    {
        use core::sync::atomic::Ordering::Relaxed;
        static THEXIT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let tgid = cur.tgid();
        let is_leader = dying_pid == tgid;
        if !is_leader
            && crate::trace::EVENT_TXNHANG.armed()
            && tgid != 0
            && tgid == crate::ipc::binder::binder_sareg_samgr_pid()
            && THEXIT_N.fetch_add(1, Relaxed) < 64
        {
            let comm = core::str::from_utf8(cur.comm())
                .unwrap_or("")
                .trim_end_matches('\0');
            // HOTPATH-PRINT: rux:txnhunt (retired debug=txnhunt)-gated samgr thread exit line.
            crate::pr_warn!(
                "SAMGR-THREADEXIT tid={} comm='{}' code={}",
                dying_pid,
                comm,
                exit_code
            );
        }
        // rwlock-livelock: thread-exit forensics — the ub2404 wedge's
        // frozen word is GLib's GAsyncQueue mutex (pool-spawner parked
        // in g_cond_wait's relock at word=2) and the mutex holder
        // matches NO live task. If a thread EXITS while holding the
        // queue mutex (glibc normal mutexes are not robust; GLib's
        // spawn protocol has no ownership recovery), the wedge is a
        // userspace death-while-holding. Gate on the tgid of the FIRST
        // process that parks a waiter on a word the census later flags
        // (learned dynamically — comm-gating missed GLib's
        // "pool-udisksd" worker comms in boot J). NON-LEADER exits only
        // (thread exits; whole-process death is not this shape).
        {
            use core::sync::atomic::Ordering::Relaxed;
            static UDEXIT_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let watch_tgid = crate::api::sync::rwfz_exit_watch_tgid();
            let is_leader = dying_pid == tgid;
            if !is_leader
                && watch_tgid != 0
                && tgid == watch_tgid
                && UDEXIT_N.fetch_add(1, Relaxed) < 128
            {
                let comm_b = cur.comm();
                let comm = core::str::from_utf8(comm_b.as_slice())
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let lastsys = cur.ext.last_syscall_nr.load(Relaxed);
                let urip = match cur.pt_regs_ref() {
                    Some(pr) => crate::process::r#unsafe::pt_user_pc(pr),
                    None => 0u64,
                };
                // HOTPATH-PRINT: thread exits of the watched tgid.
                crate::pr_warn!(
                    "RWFRZ-THREADEXIT tid={} comm='{}' code={} lastsys={} unrip={:#x}",
                    dying_pid,
                    comm,
                    exit_code,
                    lastsys as i64,
                    urip
                );
                // rwlock-livelock: the dying thread's USER return chain —
                // the exit rip is always libc's exit syscall site and
                // cannot say WHERE the thread came from; the live frames
                // below it name the GLib path (worker-proxy retirement vs
                // a requester dying between g_async_queue_lock and
                // g_cond_wait — the death-while-holding window).
                if UDEXIT_N.load(Relaxed) <= 8 {
                    // SAFETY: current is the calling task; read-only walk
                    // of its own user stack (the frames are intact at
                    // do_exit time).
                    crate::process::r#unsafe::apppark_rets(cur);
                }
            }
        }
    }
    // 50th (crash-storm hunt): failed-spawn forensics — a process LEADER
    // that exits without ever having execve'd forked and died pre-exec
    // (the OH 337->588->589 chain: 588 exits 512/code 2, 589 exits 0,
    // zero execve attempts anywhere in their rings). The birth window
    // (first 64 syscalls of the task's life, latched at dispatch) names
    // the killing call. Threads are skipped (they never exec); prints
    // are bounded to 8 per boot — kernel-thread exits and subshell-style
    // fork-and-exit children burn at most the budget, never the serial.
    {
        use core::sync::atomic::Ordering::Relaxed;
        static NOSPWN_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let is_leader = cur.pid() == cur.tgid();
        let execd = cur.ext.did_exec.load(Relaxed);
        if is_leader && !execd && NOSPWN_N.fetch_add(1, Relaxed) < 8 {
            let comm = core::str::from_utf8(cur.comm())
                .unwrap_or("")
                .trim_end_matches('\0');
            let mut line = alloc::string::String::from("NOSPAWN-EXIT pid=");
            use core::fmt::Write;
            let _ = write!(
                line,
                "{} comm='{}' code={} birth:",
                dying_pid, comm, exit_code
            );
            for i in 0..crate::process::task::BIRTH_CALLS_LEN {
                let nr = cur.ext.birth_calls[i].load(Relaxed);
                if nr == 0 {
                    break; // ring not filled past here
                }
                let a0 = cur.ext.birth_a0[i].load(Relaxed);
                let _ = write!(line, " {}:{:#x}", nr & 0xffff, a0);
            }
            crate::pr_debug!("{}", line);
        }
    }

    // forkprof (cmdline forkprof=1): leader-arm phase timings. The probe
    // rig's children are single-threaded, so the leader arm is the exit
    // cost lmbench attributes to fork+exit.
    let fp = crate::debug::forkprof::on() && dying_pid == cur.tgid();
    let fp_t0 = if fp { crate::debug::forkprof::now() } else { 0 };
    if fp {
        crate::debug::forkprof::trace(crate::debug::forkprof::T_EXIT_ENTRY, dying_pid);
    }

    // The sched:sched_process_exit tracepoint, recorded at do_exit entry
    // (the reference's record point). group_dead: this exit takes the
    // thread group to its last live thread — nr_threads has not been
    // decremented yet, so 1 means this is it. Gated behind armed().
    if crate::trace::EVENT_SCHED_PROCESS_EXIT.armed() {
        // cur is the exiting task, pinned by being the running context.
        {
            let words = crate::trace::comm_to_words(cur.comm());
            crate::trace::record(
                &crate::trace::EVENT_SCHED_PROCESS_EXIT,
                [
                    words[0],
                    words[1],
                    cur.pid() as u64,
                    cur.prio() as u64,
                    (cur.nr_threads() <= 1) as u64,
                    0,
                    0,
                    0,
                    0,
                    0,
                ],
            );
        }
    }

    // current is the raw pointer to the calling task, guaranteed valid
    // since we are the currently executing task and will never return
    // from this function. One consolidated reference.
    // SAFETY: we are `current`; do_exit never returns.

    {
        // sc129 (Session P): the OH app children's FFRT worker threads are
        // CREATED (clone CLONE_THREAD succeeds, census pid logged) and then
        // exit instantly and CLEANLY (process survives) — the worker's own
        // fail-latch dies with it. Print the exiting THREAD's fail tail
        // here (leader exits excluded — those are whole-process exits with
        // their own forensics). Budget 12.
        //
        // Serial-clean (perf-fork lane, 2026-11): the whole block — two
        // ring-dump Strings plus a ~500-char pr_warn line — cost ~6 ms of
        // polled-16550 UART time per thread exit and ran for the first 80
        // thread exits of every boot; pthread create/exit loops (and any
        // thread-heavy bring-up) paid it directly. The dumps belong to
        // the syscall-trace hunt that added them: default off behind
        // rux:syscalltrace (retired debug=syscalltrace) (sc129/sc131/sc135 hunts re-arm with the flag).
        if !(cur.pid() == cur.tgid())
            && crate::trace::EVENT_SYSCALLTRACE.armed()
        {
            use core::fmt::Write;
            let mut s = alloc::string::String::new();
            {
                let cur8 =
                    cur.ext.last_fails_cur.load(core::sync::atomic::Ordering::Relaxed);
                for k in 0..8 {
                    let i = (cur8 + 8 - 1 - k) % 8; // newest first
                    let v = cur.ext.last_fails[i].load(core::sync::atomic::Ordering::Relaxed);
                    if v != 0 {
                        // sc135 (Session Q): a0/a1 name the failing call's
                        // leading args (prctl option et al).
                        let a0 = cur.ext.last_fails_a0[i]
                            .load(core::sync::atomic::Ordering::Relaxed);
                        let a1 = cur.ext.last_fails_a1[i]
                            .load(core::sync::atomic::Ordering::Relaxed);
                        let _ = write!(
                            s,
                            " nr{}=-{} a0={:#x} a1={:#x}",
                            v & 0xffff,
                            v >> 16,
                            a0,
                            a1
                        );
                    }
                }
            }
            // sc131 (Session P): the FFRT worker batch exits CLEANLY with
            // ZERO failing syscalls — the fails-only print was silent for
            // exactly the threads we hunt. Always print (threads only),
            // now with the ALL-calls ring (96 since sc135, newest first)
            // so the worker's whole short life is visible.
            let mut c = alloc::string::String::new();
            {
                const RL: usize = crate::process::task::LAST_CALLS_LEN;
                let cur12 =
                    cur.ext.last_calls_cur.load(core::sync::atomic::Ordering::Relaxed);
                for k in 0..RL {
                    let i = (cur12 + RL - 1 - k) % RL; // newest first
                    let v = cur.ext.last_calls[i].load(core::sync::atomic::Ordering::Relaxed);
                    if v != 0 {
                        let _ = write!(c, " {}", v);
                    }
                }
            }
            static NW: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if NW.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 80 {
                let nc = core::str::from_utf8(cur.comm())
                    .unwrap_or("")
                    .trim_end_matches('\0');
                // SAFETY: the leader is a live ring member (group not
                // yet torn down at this point of do_exit).
                let lc = core::str::from_utf8(
                    crate::process::r#unsafe::borrow_task(cur.group_leader_ptr()).comm(),
                )
                .unwrap_or("")
                .trim_end_matches('\0');
                crate::pr_warn!(
                    "thead-exit: tid {} comm='{}' leader='{}' code={} calls:{} fails:{}",
                    cur.pid(),
                    nc,
                    lc,
                    exit_code,
                    c,
                    s
                );
            }
        }
        // Linux discipline: PF_EXITING is do_exit's FIRST store, so every
        // other subsystem (the scheduler's R56/R57 harvest-steal machinery
        // in particular) can refuse to "rescue" a task whose kernel stack
        // is live in the exit path — re-enqueueing it hands that stack to
        // a second hart (the n1-organic double-run panic).
        cur.set_ti_flags(
            cur.ti_flags()
                | crate::process::task::task_flags::TaskFlags::PF_EXITING.bits(),
        );
        let current_pid = cur.pid();
        let leader = cur.group_leader_ptr();
        let is_leader = leader as *const Task == cur as *const Task;
        let parent_pid = if is_leader {
            cur.ppid()
        } else {
            // Threads share the leader's real parent; the leader was
            // resolved from the ring above and is still live.
            crate::process::r#unsafe::borrow_task(leader).ppid()
        };

        // ===== Journal handle unwind =====
        // A task killed mid-syscall (SIGKILL/SIGSEGV between
        // set_current_handle and clear_current_handle — LTP abort01 sends
        // SIGABRT into its children) dies owning a jbd2 handle: its
        // transaction's t_updates stays elevated forever, and EVERY later
        // jbd2_journal_stop spins out its 1,000,000-lap "wait for other
        // handles" loop (seconds under TCG — the capset01/03 soft-lockup
        // family; multiplied per write it became a whole-chunk wedge).
        // Unwind the reference now: the handle lives on this task's dying
        // stack and can never reach ext4_journal_stop itself. The
        // registered dirty buffers stay in the transaction for the next
        // stopper to commit; nothing else in the handle needs release.
        // The current-task handle owns this journalling slot; the
        // edges document their per-task exclusivity (INV-PROC-JOURNAL-1).
        {
            // The rung-5 seam row: the handle type is ext4-crate-owned;
            // the row body does the refcount drop + slot clear
            // (INV-PROC-JOURNAL-1 documented crate-side).
            crate::api::ext4::exit_journal_unwind();
        }

        // ===== Ext4 big lock force-release =====
        // Same leak family as the journal handle above: do_exit does not
        // unwind Rust guards, so a task killed inside an ext4 critical
        // section would strand EXT4_BIG_LOCK on a dead owner forever
        // (GNOME r72-fire2: every CPU then spun or slept against the
        // dead owner's pid — even a recycled one — while gs-b/Xorg
        // starved). The dying task can make no forward progress in its
        // section; drop the lock and wake all waiters so the filesystem
        // survives the kill.
        crate::fs::journal_seam::EXT4_BIG_LOCK.release_if_owner(current_pid);

        // ===== VFS mutation lock force-release =====
        // Same leak family: do_exit does not unwind Rust guards, so a
        // task killed inside a lookup+mutate window would strand
        // VFS_MUTATION_LOCK (a SleepMutex since the sync-realign lane)
        // on a dead owner forever.
        crate::fs::vfs::VFS_MUTATION_LOCK.release_if_owner(current_pid);

        // ===== Ptrace tracer death: auto-detach tracees =====
        // Linux forgets the tracing relationship when the tracer exits
        // (ptrace_exit → every tracee is detached and resumed). Without
        // this, a tracer that dies between PTRACE_ATTACH and DETACH (the
        // LTP runner 30s-kills a wedged test) leaves its tracee STOPPED
        // forever — when the tracee is init (ptrace11 attaches PID 1) the
        // whole guest wedges.
        {
            crate::process::pid_hash::pid_hash_for_each_task(|t| {
                if (*t).tracer_pid() != current_pid || (*t).pid() == current_pid {
                    return;
                }
                // SAFETY: task is alive for the duration of the hash walk
                // (same discipline as send_signal_to_pgid's in-walk use).
                t.set_tracer_pid(0);
                t.set_ptrace_options(0);
                t.clear_single_step();
                if t.state().contains(TaskState::STOPPED) {
                    crate::signal::signal_wake_up(t as *const Task as *mut Task);
                }
            });
        }

        // ===== Core dump (P1) =====
        // A negative exit code encodes death by signal; the core-dumping
        // signals get a core file written BEFORE any resource is torn
        // ===== Binder pool reaping =====
        // Threads killed by a signal never reach BINDER_THREAD_EXIT;
        // without this their pool entries live forever, the spawner's
        // threads.len() gate counts corpses and never replenishes, and
        // the process's clients stall (b92: samgr proc todo depth 13,
        // hdf_devhost waiters parked 162s). Cleanly-exiting IPC threads
        // also route through the same reaper via the ioctl.
        crate::ipc::binder::binder_task_died(
            current_pid,
            cur.tgid(),
            cur.ns_pid_local(),
        );

        // ===== Core dump (P1) =====
        // down. Runs in the dying task's context (its cwd/fdtable/mm are
        // still intact). This is the single funnel for both the signal
        // default-action path (handle_default_signal -> do_exit_group)
        // and the direct trap paths (SIGSEGV/SIGILL/... -> do_exit).
        if exit_code < 0 && crate::api::process::signal_makes_core(-exit_code) {
            // cur is the exiting task (do_coredump's contract; it never
            // runs twice for one task).
            crate::api::process::do_coredump(cur, -exit_code);
        }

        crate::pr_debug!("exit: pid={}, tgid={}, exit_code={}, ppid={} ({})",
            current_pid, cur.tgid(), exit_code, parent_pid,
            if is_leader { "leader" } else { "thread" });

        // DEBUG (dbus-death hunt): every death-by-signal gets a KERN_INFO
        // line on the console — the default loglevel hides pr_debug, and
        // the only external symptom of the dbus-daemon kill was
        // ECONNREFUSED with the socket file left behind. `kill -0`-style
        // exits (normal, code>=0) are not printed: the fork/exec storm
        // would bury the signal deaths in noise.
        //
        // RETIRED (probe-retire): the line was unconditional (18 lines
        // per ub2404 boot — every service that died of a signal). It now
        // lives behind the existing signal-family switch (rux:sigtrace (retired debug=sigtrace)).
        if exit_code < 0
            && crate::trace::EVENT_SIGTRACE.armed()
        {
            let comm_raw = cur.comm();
            let comm_len = comm_raw.iter().position(|&b| b == 0).unwrap_or(comm_raw.len());
            // Sender forensics: SI_USER deaths (kill/tgkill) carry the
            // sender's pid in the queued siginfo — print it so "who killed
            // X" is answerable from the console (kernel-originated signals
            // have no sender and print si_code instead).
            let sender = cur.pending
                .peek_info(-exit_code)
                .map(|si| (si.si_pid, si.si_code))
                .unwrap_or((0, 0));
            crate::pr_info!(
                "SIGDEATH: pid={} tgid={} comm={:?} sig={} ppid={}{} si_pid={} si_code={:#x}",
                current_pid,
                cur.tgid(),
                core::str::from_utf8(&comm_raw[..comm_len]),
                -exit_code,
                parent_pid,
                if is_leader { "" } else { " (thread)" },
                sender.0,
                sender.1,
            );
        }

        #[cfg(feature = "debug-futex-trace")]
        {
            use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
            taskdump_raw_line(b"FTX EXIT pid=");
            taskdump_dec(current_pid as u64);
            taskdump_raw_line(b" code=");
            taskdump_dec(exit_code as i64 as u64);
            taskdump_raw_line(b" threads=");
            taskdump_dec(cur.nr_threads() as u64);
            taskdump_raw_line(b"\n");
        }

        // Set exit code
        cur.set_exit_code(exit_code);

        // ===== CPU-time accounting fold =====
        // Linux semantics: a dying thread's user/system ticks fold into the
        // thread-group leader (so /proc/[pid]/stat, times() and
        // getrusage(RUSAGE_SELF) report group totals), and the leader's
        // accumulated totals fold into the PARENT's cutime/cstime (what
        // times()/getrusage(RUSAGE_CHILDREN) report after wait()). Lost
        // silently when the parent already exited (orphan), matching the
        // fact that nobody can wait for them anymore.
        {
            use core::sync::atomic::Ordering::Relaxed;
            let u = cur.utime_ticks.load(Relaxed);
            let s = cur.stime_ticks.load(Relaxed);
            // ru_maxrss propagation (Linux signal->cmaxrss): the child's
            // resident set at exit (captured below while the mm is still
            // alive) folds into the parent's RUSAGE_CHILDREN hiwater.
            let rss_kb = {
                let mm_rss = cur.address_space()
                    .map(|mm| mm.rss())
                    .unwrap_or(0);
                mm_rss * (crate::mm::page::PAGE_SIZE / 1024) as u64
            };
            cur.ext.exit_maxrss_kb.store(rss_kb, Relaxed);
            if is_leader {
                // RUSAGE_CHILDREN folds at REAP time, not exit time
                // (reference wait_task_zombie accumulates into the
                // parent's signal struct only when the zombie is
                // waited): a parent reading RUSAGE_CHILDREN while the
                // child is still a ZOMBIE must NOT see its contribution
                // (LTP getrusage03's pre_wait assertion). The fold lives
                // in release_task; exit only captures exit_maxrss_kb.
            } else {
                // The leader is alive (we are its member; the ring
                // decrement happens below).
                let leader_ref = crate::process::r#unsafe::borrow_task(leader);
                leader_ref.utime_ticks.fetch_add(u, Relaxed);
                leader_ref.stime_ticks.fetch_add(s, Relaxed);
            }
        }

        // Drop any in-progress signal frame: a handler that longjmp'd out
        // left sigframe set, which blocks all further delivery including
        // SIGKILL (review H15) — clear it so the dying task stays killable.
        cur.sigframe = None;
        cur.sigframe_addr = 0;

        // ===== Robust list + clear_child_tid: BEFORE the mm ref drops =====
        // Both write into user memory and futex-wake with THIS task's mm
        // as the key identity; once set_address_space(None) runs the key
        // is unobtainable and the user words unreachable.
        crate::api::process::exit_robust_list(cur);
        crate::api::process::exit_clear_child_tid(cur);

        // ===== cgroup v2 (U1b): leave the cgroup + settle memory =====
        // Drops cgroup membership (pids.current / cgroup.procs) and
        // uncharges the task's mmap(2) memory ledger — see
        // sched::cgroup::cgroup_exit for why the ledger, not total_vm,
        // is the uncharge unit.
        crate::sched::hotpath::cgroup_exit(cur);

        // ===== exit_mm: drop THIS task's mm reference =====
        // Linux exit_mm discipline:
        // - shm detach (nattch) runs inside MmStruct::drop now — exactly
        //   once per mm, when the LAST Arc reference goes away. The old
        //   pre-drop `Arc::strong_count == 1` check here was racy against
        //   walker-held clones (ptrace/OOM/rmap) and against concurrent
        //   sibling exits (double detach / missed detach).
        // - satp switch to the kernel root page table: EVERY dying task on
        //   a user page table, last or not. After clear_child_tid/robust
        //   list nothing touches user memory, and a dying group member
        //   holding the last reference FREES the page tables while this
        //   task may still be spinning in wait_for_thread_group_death on
        //   another CPU — running on a freed root is the ARCH-H4 fault.
        // - set_address_space(None) swaps the slot under the mm-slot lock
        //   and drops our Arc after it; if it is the last reference the
        //   teardown (grace wait + PT free + VMA release) runs in THIS
        //   context, after the satp switch. SysV shm detach (nattch) runs
        //   inside MmStruct::drop — exactly once per mm, which subsumes
        //   the old strong_count==1 pre-check here (LTP shmat01 family).
        if cur.address_space_arc().is_some() {
            crate::process::r#unsafe::switch_to_kernel_root_on_exit();
        }
        let fp_t_mmdrop = if fp { crate::debug::forkprof::now() } else { 0 };
        cur.set_address_space(None);
        cur.clear_active_mm();
        // (fork-frag lane) exit is the other natural table-churn point:
        // drain defer-freed leaf tables so the frames return before the
        // next fork's child-table allocs look for them (no-op on arches
        // without the protocol).
        crate::arch::mm::pt_pending_drain();
        if fp {
            crate::debug::forkprof::add(crate::debug::forkprof::P_EX_MMDROP, fp_t_mmdrop);
            crate::debug::forkprof::trace(crate::debug::forkprof::T_MMDROP_DONE, dying_pid);
        }

        // Wake vfork parent (if any) — child has dropped the shared address
        // space, so the parent can safely resume.  This must happen before
        // ZOMBIE state so that the parent sees the child is still valid.
        crate::process::task::vfork_wake_parent(cur);

        // ===== exit_files: Release file descriptor table reference =====
        cur.set_fdtable(None);

        // ===== Clean up futex waiters =====
        crate::sync::futex::futex_cleanup(cur);
        // PI futexes: release held locks (owner-died handoff to the top
        // waiter) and leave every PI wait queue.
        crate::sync::futex_pi::futex_pi_exit_cleanup(cur);

        // ===== Clean up POSIX MQ fd entries =====
        crate::api::ipc::mq_fds_cleanup(cur);

        // ===== Disarm interval timers (ITIMER_REAL/VIRTUAL/PROF) =====
        for i in 0..3 {
            let old_id = cur.itimer_ids[i].swap(0, core::sync::atomic::Ordering::AcqRel);
            if old_id != 0 {
                crate::timer::del_timer(old_id);
            }
        }

        // ===== Disarm POSIX timers =====
        {
            let mut timers = cur.posix_timers.lock();
            for pt in timers.drain(..) {
                if pt.kernel_timer_id != 0 {
                    crate::timer::del_timer(pt.kernel_timer_id);
                }
            }
        }

        // ===== Reverse SEM_UNDO adjustments =====
        crate::ipc::sysv_sem::sem_undo_exit(cur);

        // ===== Orphan reparenting (review PROC-P03) =====
        // Children of a dying task must be re-attached (a surviving
        // thread-group member first, else the subreaper/init (PID 1)),
        // otherwise their parent pointers dangle after our Task slot is
        // freed (ppid()/for_each_child UAF) and orphaned ZOMBIEs are
        // never reaped — the PID space leaks monotonically.
        // Applies to threads too: a thread may have fork()ed children.
        // Process-tree mutations take PROCESS_TREE_LOCK inside.
        let fp_t_rep = if fp { crate::debug::forkprof::now() } else { 0 };
        crate::api::process::reparent_children_to_init(cur);
        if fp {
            crate::debug::forkprof::add(crate::debug::forkprof::P_EX_REPARENT, fp_t_rep);
        }

        // ===== Thread-group split =====
        if !is_leader {
            // ---- Non-leader thread: deferred release, never a ZOMBIE ----
            // Our mm Arc reference is already dropped above; other members
            // keep the address space alive.
            cur.signal = None; // shared with the leader; drop our ref
            cur.set_fs(None); // CLONE_FS-shared; drop our ref

            // Park on the leader's dead list BEFORE leaving the ring.
            // Ordering matters: nr_threads hits 1 in thread_group_leave,
            // which unblocks a dying leader (and the parent's reap of it).
            // If the push came after the leave, the leader could be fully
            // released in that window and we would push onto freed memory.
            // Parking first is safe: free_dead_member spins on on_cpu,
            // which stays set until our final schedule below — so the
            // leader's release cannot pass our entry while we still touch
            // leader fields.
            // SAFETY: leader comes from group_leader_ptr() while we are
            // still linked in its ring — it cannot be mid-release (its
            // release_task waits for ring members to be DEAD first).
            if !leader.is_null() {
                // SAFETY: the leader is alive; the list is lock-guarded.
                crate::process::r#unsafe::borrow_task(leader).dead_threads.lock().push(cur as *mut Task);
            }

            // Leave the ring (decrements the leader's nr_threads) — AFTER
            // the mm drop and the dead-list park.
            let leader = Task::thread_group_leave(cur);

            // r16 (mechanism B, re-landed on the RELEASE-DEFER drain): the
            // sweep below can SLEEP — free_dead_member ->
            // synchronize_rcu_bounded_pollfirst/_sleepy arms a PID-keyed
            // one-shot timer wake for every 1-jiffy poll round (and the
            // deferred-release finisher at the sweep's tail parks the same
            // way). Freeing our pid BEFORE the sweep made each such wake
            // unresolvable: the expiry hit a freed (possibly reused) pid,
            // WAKE-PID-MISS dropped the token, and the waiter parked past
            // its own deadline — the bound is only re-checked on wake, so
            // the "bounded" wait became a permanent mid-exit sleep (the GB
            // worker wedges; the OH SIGKILL storms reproduce the class).
            // The hash remove + free now run AFTER the sweep: the tid
            // lingers only across the sweep, the task never returns to
            // userspace, and a tgkill/gettid racing this window observes a
            // PF_EXITING task — the reference's own pre-reap shape.
            // rcu.rs keeps an independent guard (RCU-SLEEPY-PID-FREE) for
            // any future pid-then-sleep order.

            // Opportunistically reclaim older dead members (frees stacks of
            // previously-joined threads; `current` is skipped — on_cpu).
            if !leader.is_null() {
                crate::api::process::sweep_dead_threads(leader);
            }

            // PID + hash bookkeeping: the tid disappears now (gettid/tgkill
            // on it → ESRCH, no zombie window).
            crate::process::pid_hash::pid_hash_remove(current_pid);
            crate::api::process::free_pid(current_pid);

            // R9-3 discipline: no preempt window between DEAD and the
            // final schedule.
            crate::interrupt::preempt::preempt_count_add(1);
            // Final state: DEAD (not ZOMBIE — wait4 never sees threads).
            cur.set_state(TaskState::new(TaskState::DEAD));
            crate::sched::hotpath::dequeue_task(cur);
            crate::interrupt::preempt::preempt_count_sub(1);

            // ===== do_task_dead: Final schedule, never returns =====
            // sc186/sc189 (session R eighth leg): the plain schedule()
            // could be repicked/refused (a racing wake/heal re-links the
            // exiting task; __schedule's same-task repick fast path then
            // returns without switching) — the wfi park loop below would
            // strand this CPU forever with the slot still naming the dead
            // task and its leaked preempt bits uncleared (PREEMPT-LEAK
            // pc=0x10000 rip=wfi+5, 10+ min, twice reproduced).
            // schedule_dead_never_returns() re-asserts DEAD + unlinks +
            // clears leaked preempt bits until a real context switch.
            crate::sched::hotpath::schedule_dead_never_returns();

            loop {
                // SAFETY: `wfi` is a plain hint instruction with no side effects.
                crate::arch::cpu::wfi();
            }
        }

        // ---- Leader (or single-threaded process): classic ZOMBIE flow ----
        // If group members are still alive (exit() from the leader, or
        // exit_group), kill them and wait for the drain before tearing
        // down the shared-mm-dependent state any further.
        if cur.nr_threads() > 1 {
            crate::api::process::kill_other_threads(cur);
            // NOT killable: an exiting leader cannot unwind, and the
            // shared-mm teardown below requires the drain (see the
            // wait_for_thread_group_death doc).
            crate::api::process::wait_for_thread_group_death(cur, false);
        }

        // ===== pid-namespace init exit: tear the namespace down =====
        // zap_pid_ns_processes parity: when a container's init dies, every
        // task left in that namespace — and in DESCENDANT namespaces (they
        // hold pids in it too) — receives a kernel-forced SIGKILL. Without
        // the cascade the survivors lingered under an unreachable pid-1
        // (LTP pidns05: the whole nested chain must die with the outer
        // init). Kernel-origin SIGKILL pierces the unkillable-init rule in
        // do_signal (the SEND_SIG_PRIV force exception), so nested
        // container inits die too.
        if cur.ns_pid.is_some() && cur.ns_local_pid == 1 {
            let dying_ns = cur.ns_pid.clone();
            // gdm bring-up r9/r10: a silent mass SIGKILL took out the
            // whole daemon set (dbus, upowerd, accounts, tailer, sink,
            // polkitd, gdm3) with ZERO prints at any loglevel and no
            // SIGKILL-SEND trace — this loop is the only silent kernel
            // SIGKILL fan-out. Name the namespace init and every victim
            // until this is root-caused.
            // SAFETY: current is the raw pointer to the calling task,
            // valid because we are it.
            let ccomm = alloc::string::String::from(cur.comm_str());
            let ns_addr = dying_ns
                .as_ref()
                .map(|d| alloc::sync::Arc::as_ref(d) as *const _ as usize)
                .unwrap_or(0);
            let mut victims: alloc::vec::Vec<(u32, alloc::string::String)> =
                alloc::vec::Vec::new();
            crate::process::pid_hash::pid_hash_for_each_task(|task| {
                let t = &*task;
                if t.pid() == current_pid {
                    return;
                }
                let hit = dying_ns.as_ref().map(|d| {
                    // Same namespace or any descendant: walk the target's
                    // parent chain looking for the dying namespace.
                    let mut ns = t.ns_pid.clone();
                    while let Some(n) = ns {
                        if alloc::sync::Arc::ptr_eq(&n, d) {
                            return true;
                        }
                        ns = n.parent.clone();
                    }
                    false
                }).unwrap_or(false);
                if hit {
                    victims.push((
                        t.pid(),
                        alloc::string::String::from(
                            t.comm_str(),
                        ),
                    ));
                }
            });
            crate::pr_err!(
                "PIDNS-KILL: ns-init pid={} comm='{}' ns=0x{:x} victims={}",
                current_pid,
                ccomm,
                ns_addr,
                victims.len()
            );
            for (vp, vc) in victims.iter().take(20) {
                crate::pr_err!("PIDNS-KILL victim pid={} comm='{}'", vp, vc);
            }
            for (vp, _) in victims.iter() {
                let _ = crate::signal::send_signal(
                    *vp,
                    crate::signal::Signal::SIGKILL as i32,
                );
            }
        }

        // ===== PID 1 exit: power the machine down =====
        // No orphan is ever reaped or respawned without init — the
        // shutdown(8) init-signal path lands here (init kills every user
        // process, including shutdown(8) itself, then exits). Runs the
        // reboot(2) cascade (never returns) unless one is already in
        // flight — that caller is waiting for exactly this exit.
        if current_pid == 1 {
            crate::api::process::init_exited_fallback_cascade();
        }

        // R9-3: close the preempt window between ZOMBIE and the deferred
        // notify — a timer IRQ landing here would schedule() us out with
        // state != RUNNING, and lines below (the notify arm) would never
        // run: the parent's only wake source lost.
        let fp_t_notify = if fp { crate::debug::forkprof::now() } else { 0 };
        crate::interrupt::preempt::preempt_count_add(1);
        // Set process state to Zombie
        cur.set_state(TaskState::new(TaskState::ZOMBIE));

        // Dequeue from global run queue
        crate::sched::hotpath::dequeue_task(cur);

        // pidfd exit notification: every pidfd poller targeting this
        // thread group must observe EPOLLIN now (the reference wakes the
        // pid's waitqueue here). The woken tasks re-check their own
        // target's state, so the global queue's spurious wakes are
        // harmless; this CPU holds preempt_count, so they run after the
        // terminal schedule below.
        crate::fs::pidfd::exit_notify_all_waiters();

        // ===== exit_notify: Defer parent notification =====
        // CRITICAL: Do NOT wake the parent here.  If the parent runs on
        // another CPU it can reap (free_task_slot) this task before
        // schedule() switches us away, causing a use-after-free that
        // corrupts ti_cpu (initialised to -1 in Task::new).
        // Instead, store the parent PID in a per-CPU deferred slot;
        // __schedule processes it AFTER the context switch, when this
        // task is no longer running on any CPU.
        //
        // 99th (orphan-during-exit parity, §87.8 hunt): the reference's
        // do_notify_parent resolves task->real_parent AT NOTIFY TIME.
        // `parent_pid` above was captured at do_exit ENTRY — if the real
        // parent died while this task ran its exit tail, the child was
        // reparented (reparent_children_to_init) to a subreaper/init
        // AFTER the capture but BEFORE our ZOMBIE store, and the
        // reparent's own zombie-notify arm (which fires only for
        // children ALREADY ZOMBIE at move time) never fired for us. The
        // packed pid would then name a dead task: the delivery's
        // pid_hash lookup fails, no SIGCHLD, no wait_chldexit wake — and
        // if the new parent (init) is parked in wait4, the zombie
        // strands with its waiter forever. Resolve the CURRENT parent
        // here, inside the same preempt-guarded section: None = a racing
        // reaper already claimed us (it is inside release_task; the
        // notify would be noise) — skip; Some(p) = notify p (whether or
        // not it differs from the captured pid — same pid is the
        // identity case).
        let notify_parent: u32 = match cur.parent_ptr() {
            Some(pp) if Task::is_plausible_task_ptr(pp) => {
                crate::process::r#unsafe::borrow_task(pp as *mut Task).pid()
            }
            _ => 0,
        };
        if notify_parent != 0 {
            crate::sched::hotpath::defer_exit_notify(notify_parent, current_pid);
        }
        crate::interrupt::preempt::preempt_count_sub(1);
        // 125th waitid leg: reaper forensics — record + print every
        // leader ZOMBIE (exit time, decoded status, parent, SPAWN age,
        // last-syscall latch) so the REAP4/REAPW arms can time the reap
        // and ZLINGER can catch the never-reaped case. rux:schedtrace (retired debug=schedtrace)-
        // gated (debug/reaper.rs). Non-leaders never reach ZOMBIE (the
        // deferred-release tail above), so this fires per process.
        if crate::debug::reaper::on() {
            crate::debug::reaper::on_zombie(
                current_pid,
                exit_code,
                notify_parent,
                cur.comm(),
                cur.ext.last_syscall_nr.load(core::sync::atomic::Ordering::Relaxed),
                cur.ext.last_syscall_a0.load(core::sync::atomic::Ordering::Relaxed),
            );
        }
        if fp {
            crate::debug::forkprof::add(crate::debug::forkprof::P_EX_NOTIFY, fp_t_notify);
            crate::debug::forkprof::add(crate::debug::forkprof::P_EX_TOTAL, fp_t0);
            crate::debug::forkprof::trace(crate::debug::forkprof::T_NOTIFY_DONE, dying_pid);
        }

        // ===== do_task_dead: Final schedule, never returns =====
        // sc186/sc189: see the non-leader tail above — the terminal
        // deschedule re-asserts ZOMBIE/DEAD + unlink + preempt-bit clear
        // until a real context switch happens (the plain schedule() could
        // be repicked/refused and strand this CPU in the wfi loop).
        crate::sched::hotpath::schedule_dead_never_returns();

        loop {
            // SAFETY: `wfi` is a plain hint instruction with no side effects.
            crate::arch::cpu::wfi();
        }
    }
}

/// exit_group — terminate the WHOLE thread group (Linux do_group_exit).
///
/// Force SIGKILL every other member (leader included, whoever calls it),
/// then run the caller's do_exit. Called from:
/// - sys_exit_group (NR 94)
/// - fatal default signal actions (Linux do_group_exit semantics: a fatal
///   signal in ANY thread kills the process)
pub fn do_exit_group(exit_code: i32) -> ! {
    let cur = match crate::sched::hotpath::current() {
        Some(c) => c,
        None => do_exit(exit_code),
    };
    // gdm bring-up r9-r14: every daemon storm-death funnels through here
    // (fatal signals do_group_exit, plus explicit exit_group). Name the
    // caller, the code, and the pending-signal set that drove it — the
    // storm's first domino and the TERM->st=9 status overwrite both
    // decode from this one print.
    //
    // Serial-clean discipline (perf-fork lane, 2026-11): pr_err printed
    // on EVERY exit_group — one ~60-char line through the polled 16550
    // costs ~650 us under KVM, which was ~85% of the whole fork+exit
    // round trip (lmbench lat_proc,fork sat ~20x over the reference
    // column on this print alone; forkprof/ftrace attribution, ledger
    // perf-fork round 1). The storm hunts are done; the line moves to
    // pr_debug (hidden by the default loglevel, raisable for the next
    // hunt) so the exit path stays off the console.
    {
        // SAFETY: current is the raw pointer to the calling task.
        let ccomm = cur.comm_str();
        let pend = cur.pending.signal.load(core::sync::atomic::Ordering::Relaxed);
        crate::pr_debug!(
            "EXITGRP: pid={} comm='{}' code={} pending=0x{:x}",
            cur.pid(),
            ccomm,
            exit_code,
            pend
        );
    }
    // Publish the group exit status BEFORE stopping the siblings: the
    // SIGKILL below makes each member's fatal-signal path re-enter
    // do_exit_group with THIS code, so the whole group — the leader
    // included — reports the clean status instead of "killed by
    // signal 9" (reference signal_group_exit; LTP mq_notify03 exit(0)
    // from the mq callback thread).
    // SAFETY: current is the running task (matched above), non-null.
    let signal_arc = cur.signal_arc();
    if let Some(ss) = signal_arc {
        ss.group_exit_code
            .store(exit_code, core::sync::atomic::Ordering::Release);
        ss.group_exiting
            .store(true, core::sync::atomic::Ordering::Release);
    }
    crate::api::process::kill_other_threads(cur);
    do_exit(exit_code)
}

/// Move every child of `dying` to a new parent (Linux
/// forget_original_parent): the nearest PR_SET_CHILD_SUBREAPER ancestor if
/// one exists, else init (PID 1). Zombie children get a SIGCHLD to the new
/// parent so the shell's reaper loop can collect them; live children
/// simply change parent. Any child with a parent-death signal
/// (PR_SET_PDEATHSIG) registered receives it now. Runs before `dying`
/// transitions to ZOMBIE and under the process-tree lock, so no child can
/// observe a dangling parent.
///
/// `dying` is the currently running task; child pointers come from its
/// children list and are protected by PROCESS_TREE_LOCK.
pub const ERESTARTSYS: i32 = -512;

pub fn wake_group_chldexit(task: &Task) {
    use crate::process::task::PROCESS_TREE_LOCK;

    let _lock = PROCESS_TREE_LOCK.lock();
    let start = task as *const Task;
    let mut member = task as *const Task;
    let mut guard = 0u32;
    loop {
        let _ = crate::process::r#unsafe::borrow_task(member as *mut Task)
            .wait_chldexit
            .wake_up_all();
        member = crate::process::r#unsafe::borrow_task(member as *mut Task)
            .next_thread_ptr() as *const Task;
        if member == start || member.is_null() {
            break;
        }
        guard += 1;
        if guard > 65536 {
            break;
        }
    }
}

// ============================================================================
// wait4-park census (99th, §87.8 wait4/SIGCHLD wakeup-loss hunt)
// ============================================================================

