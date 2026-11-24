//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! SIGNAL POLICY (the signal crate flip): the send-path queueing
//! core, the default-action table, the pid-ns init delivery filter, the
//! exec/fork handler-reset rules, the /proc SigIgn/SigCgt pair, and the
//! user-originated sender-stamping entries — moved VERBATIM out of
//! kernel/src/signal.rs. This file is NOT declared by the framework's
//! module tree (kernel/src/lib.rs never names it): the signal
//! service crate is its sole compilation home, under
//! #![forbid(unsafe_code)]. The framework reaches this code only
//! through kernel/src/signal_hooks.rs (OnceSlot rows, all cold paths).
//!
//! The MECHANISM stays framework-side: the pending set and its queue,
//! the pt_regs frame build/restore, the wake path, the pinned pid-hash
//! lookups, the schedule loop. This file calls DOWN into those through
//! the crate's root re-exports — direct cross-crate calls, no rows.

use crate::signal::{SigAction, SigActionKind, SigInfo, Signal, SignalStruct, SIGRTMIN, si_code};

// ============================================================================
// Send-path policy
// ============================================================================

/// User-originated signal (kill/tgkill/tkill): identical to send_signal
/// but the queued siginfo carries the SENDER's pid/uid (SI_USER), so
/// sigwaitinfo/sigtimedwait readers and SA_SIGINFO handlers see si_pid.
/// Kernel-internal senders (timers, OOM, tty) keep using send_signal —
/// their siginfo is SI_KERNEL with no sender.
pub fn send_signal_from_user(pid: u32, sig: i32) -> Result<(), i32> {
    if sig < 1 || sig > 64 {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }
    let (sender_pid, sender_uid) = match crate::sched::current() {
        Some(t) => (t.pid(), t.cred().uid),
        None => (0, 0),
    };
    // sc150 (Session Q, launcher lane): early-boot service threads are
    // SIGKILLed mid-pthread_create (hiview/sdc OS_IPC threads, code=-9,
    // identical tails every boot) and the resulting IPC-session deaths
    // leave SA 501 (AppMgrService) unregistered — the direct cause of
    // the app children parking. NAME THE SENDER of every user SIGKILL.
    if sig == 9 {
        static K9_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if K9_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 80 {
            let scomm = crate::sched::current()
                .map(|t| t.comm())
                .map(|c| core::str::from_utf8(c).unwrap_or(""))
                .unwrap_or("")
                .trim_end_matches('\0');
            crate::pr_debug!(
                "SIGKILL-SEND from pid={} comm='{}' -> target pid={}",
                sender_pid,
                scomm,
                pid
            );
        }
    }
    crate::signal::send_signal_with_info(pid, SigInfo::new(sig, si_code::SI_USER, sender_pid, sender_uid))
}

/// Thread-directed user signal (tkill/tgkill — glibc raise() is tgkill):
/// identical to send_signal_from_user but the queued siginfo carries
/// si_code = SI_TKILL, exactly like Linux's do_send_specific(). Without
/// it a raise()d fatal signal (e.g. a SIGTERM suicide) died with no
/// sender information at all: the standard-signal bitmap-only path left
/// nothing for the delivery-time siginfo reconstruction, so the SIGDEATH
/// forensics printed si_pid=0/si_code=0 and the real sender stayed
/// invisible (gnome-session-binary SIGTERM hunt, fire-gate-merged-r1).
pub fn send_signal_from_user_tkill(tid: u32, sig: i32) -> Result<(), i32> {
    // gdm bring-up r9-r13: the mass SIGKILL storm bypassed every other
    // instrumented path; tkill/tgkill delivery was the silent one (the
    // legacy sys_tkill has NO tgid restriction — any process can
    // tkill(ANY_PID, SIGKILL)). Name caller + target for fatal signals.
    // gdmr29 (TEMPORARY widening to every signal): catch the SIGTRAP
    // sender that kills gdm-session-worker. Revert to 9/15 before merge.
    //
    // Reverted (print-audit lane, 2026-11, same revert as the KSIG twin
    // in send_signal_g): the widened `if true` put a ~40-char pr_err on
    // EVERY tkill/tgkill — pthread_kill, raise(), LTP thread tests —
    // 4000 budgeted lines ≈ 2.4 s of polled-16550 serial per boot (the
    // EXITGRP class, e2717687). The SIGTRAP hunt is closed; the 9/15
    // filter is restored. The next every-signal hunt should use a
    // dfx-switched tracepoint instead.
    if sig == Signal::SIGKILL as i32 || sig == Signal::SIGTERM as i32 {
        static TK_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        // OH boot lane 2026-11-12: dfx-gated (sigtrace). The OH churn
        // delivers kill/term per service restart and FFRT group
        // teardown — every one paid the emit lock + serial line for
        // this CLOSED hunt (100th's fresh record: 3.7k samples through
        // acquire_emit_lock on the signal/kill paths).
        if crate::trace::EVENT_SIGTRACE.armed()
            && TK_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 4000
        {
            let (cp, cc) = crate::sched::current()
                .map(|t| (t.pid(), alloc::string::String::from(t.comm_str())))
                .unwrap_or((0, alloc::string::String::from("kernel")));
            crate::pr_err!(
                "TKSIG{}: caller pid={} comm='{}' -> tid={}",
                sig,
                cp,
                cc,
                tid
            );
        }
    }
    // oh-svc abort-reason tap (2026-10-31): a self-raised SIGABRT (musl
    // abort()) prints NOTHING — the OH image's stderr is /dev/null and
    // hilogd may never see the dying process — so render_service's abort
    // (first composition frame, tgid 896) died with no reason anywhere.
    // On the FIRST sig-6 of each process, dump the raiser's user rip/rsp
    // plus its exec-VMA map so the serial names the aborting library and
    // the return chain into it (musl raise keeps the caller frames live
    // on the stack; scanning rsp upward for code-region values gives the
    // poor-man's backtrace). Budgeted: one dump per tgid, 8 per boot.
    if sig == 6 {
        static ABORT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        static LAST_TGID: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        // oh-svc T1 abort-reason tap (known-gap 4): the dump walks the
        // exec-VMA map and scans the user stack — gated behind
        // rux:ohhunt (retired dfx=ohhunt) so ordinary userspace aborts (glibc assertions in
        // LTP runs included) cost nothing.
        if crate::trace::EVENT_OHHUNT.armed()
            && ABORT_N.load(core::sync::atomic::Ordering::Relaxed) < 8
            && LAST_TGID.load(core::sync::atomic::Ordering::Relaxed) != tid
            && ABORT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8
        {
            LAST_TGID.store(tid, core::sync::atomic::Ordering::Relaxed);
            // dfx-migrate: the abort also records into the unified
            // framework (rux:ohhunt kind=SIGABRT — the event is also
            // the family's arm switch). a=the dying tid, b=the raiser.
            let raiser = crate::sched::current().map(|t| t.pid() as u64).unwrap_or(0);
            crate::trace::trace_ohhunt(3, tid, raiser, 0);
            crate::signal_edges::abort_reason_dump(tid);
        }
    }
    if sig < 1 || sig > 64 {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }
    let (sender_pid, sender_uid) = match crate::sched::current() {
        Some(t) => (t.pid(), t.cred().uid),
        None => (0, 0),
    };
    // sc151: tgkill/raise suicide path of the SIGKILL-SEND tracer (see
    // send_signal_from_user).
    if sig == 9 {
        static K9T_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if K9T_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 80 {
            let scomm = crate::sched::current()
                .map(|t| t.comm())
                .map(|c| core::str::from_utf8(c).unwrap_or(""))
                .unwrap_or("")
                .trim_end_matches('\0');
            crate::pr_debug!(
                "SIGKILL-TKILL from pid={} comm='{}' -> target tid={}",
                sender_pid,
                scomm,
                tid
            );
        }
    }
    crate::signal::send_signal_with_info_directed(tid, SigInfo::new(sig, si_code::SI_TKILL, sender_pid, sender_uid))
}

/// Force-unblock a SYNCHRONOUS signal in the current task (Linux
/// force_sig parity for hardware faults).
///
/// A user-mode SIGSEGV/SIGBUS delivered to a thread that blocks the
/// signal stays pending forever: do_signal's `first_unmasked` filter
/// skips it, execution returns to the faulting pc, and the fault repeats
/// — an unkillable kernel-visible livelock (observed: Xorg's
/// InputThread, which blocks every signal via pthread_sigmask,
/// refaulting 88k+ times at one address). POSIX leaves blocked
/// synchronous signals undefined; Linux's observable behavior is that
/// hardware-generated signals are delivered regardless of the mask, so
/// the kernel clears the bit for the faulting thread before returning.
pub fn force_unblock_current(sig: i32) {
    if !(1..=64).contains(&sig) {
        return;
    }
    if let Some(t) = crate::sched::current() {
        t.sigmask &= !(1u64 << (sig - 1));
    }
}

/// Signal-sending core with optional siginfo (rt_sigqueueinfo family and
/// kernel-generated signals like SIGCHLD that carry a payload).
/// `si: Option<SigInfo>` — queued (RT: appended, standard: coalesced) so
/// SA_SIGINFO handlers and sigtimedwait readers see the real fields.
///
/// `group` is the reference's `type != PIDTYPE_PID` word for the
/// signal:signal_generate tracepoint: 1 = directed at the thread group
/// (kill/SIGCHLD/sigqueue family), 0 = directed at this one thread
/// (tkill/tgsigqueueinfo/force_sig fault paths/SIGPIPE).
///
/// The mask check uses the TARGET TASK's per-thread sigmask (Linux
/// wants_signal): the shared SignalStruct's mask field is not per-thread,
/// and threads routinely run with different blocked sets.
///
/// `task` is borrowed from the caller's PINNED handle (task_refcnt held
/// by the caller) — liveness is the pin's, not this fn's problem.
pub fn send_signal_core(
    task: &crate::process::task::Task,
    sig: i32,
    si: Option<SigInfo>,
    group: i32,
) -> Result<(), i32> {
    use crate::signal::Signal;

    // The signal:signal_generate tracepoint — the reference fires it
    // once per __send_signal at the outcome (its `ret:` label) with the
    // result code: 0 delivered, 1 ignored, 2 already pending (standard
    // signals coalesce in the bitmap), 4 overflow-fail. Emission is a
    // closure over the outcome so every return path records with the
    // same field set; armed() gates the whole thing to two relaxed
    // loads when off. `already_std` (the reference's legacy_queue) is
    // read BEFORE the first queue mutation and reused by every later
    // outcome.
    let si_errno = si.as_ref().map(|i| i.si_errno).unwrap_or(0);
    let si_code = si.as_ref().map(|i| i.si_code).unwrap_or(0);
    let emit = |result: i32| {
        if crate::trace::EVENT_SIGNAL_GENERATE.armed() {
            let words = crate::trace::comm_to_words(&*task.comm());
            crate::trace::record(
                &crate::trace::EVENT_SIGNAL_GENERATE,
                [
                    sig as u64,
                    si_errno as u64,
                    si_code as u64,
                    words[0],
                    words[1],
                    task.pid() as u64,
                    group as u64,
                    result as u64,
                    0,
                    0,
                ],
            );
        }
    };
    // legacy_queue(): a STANDARD signal already pending coalesces — the
    // second send does not queue a new instance (result 2 below). RT
    // signals always queue, so they stay "delivered".
    let already_std = sig >= 1
        && sig < SIGRTMIN
        && (task.pending.get_all() >> (sig - 1)) & 1 == 1;

    // Namespace view of the sender: the receiver must read si_pid as the
    // sender's pid INSIDE ITS OWN pid namespace (0 when the sender has no
    // pid there — a kill from an ancestor namespace, LTP pidns16/pidns30).
    // Kernel info-carrying sends stamp the GLOBAL pid; translate once
    // here so every queue below stores the receiver-local value
    // (reference __send_signal copies through task_pid_nr_ns).
    let si = si.map(|mut info| {
        // Only pid-bearing layouts translate: the kill/rt classes
        // (si_code <= 0: SI_USER/SI_TKILL/SI_QUEUE/SI_MESGQ...) and
        // SIGCHLD (CLD_* codes are positive but the layout still carries
        // the child pid at 16). POLL_IN/POLL_OUT SIGIO payloads reuse
        // that offset for si_band and must pass through untouched.
        let pid_layout =
            info.si_code <= 0 || info.si_signo == Signal::SIGCHLD as i32;
        if pid_layout && info.si_pid > 0 {
            info.si_pid = crate::process::ns::translate_sender_pid_ns(
                info.si_pid as u32,
                task.ns_pid.as_ref(),
            ) as i32;
        }
        info
    });

    // sc93 launcher gate: a cross-process SIGKILL to a wedged launcher
    // means the AMS lifecycle timeout fired AND the userspace DFX dumper
    // already failed on this process (every LIFECYCLE_TIMEOUT freeze
    // artifact reports "failed to fully dump due to timeout"). The
    // kernel dumps the wedged threads itself before they die — park
    // futex word + value, rip/rsp, file-backed frames. Runs before
    // anything is queued or the target is woken, so the park state is
    // still intact.
    if sig == Signal::SIGKILL as i32 {
        let t_comm = task.comm();
        let t_len = t_comm.iter().position(|&b| b == 0).unwrap_or(t_comm.len());
        if let Ok(t_str) = core::str::from_utf8(&t_comm[..t_len]) {
            if t_str.contains("launcher") {
                crate::sync::sync_diag_hooks::kill_dump_tgid(task.tgid() as u32);
            }
        }
    }

    // KILLTRACE (sigterm hunt): name the sender of fatal signals.
    // Coverage: every SIGTERM (rare in practice), cross-group SIGKILLs, and
    // SIGSEGV/SIGKILL to watched comms (gnome-session*/ls — the session
    // chain forensics targets). Same-group SIGKILLs are skipped — that is
    // exit_group's kill_other_threads teardown noise, not an external kill.
    // The origin tag decodes as: 0=SI_USER(kill), -6=SI_TKILL(tgkill/raise),
    // 1/2=SEGV_*(hardware fault), 0x80=SI_KERNEL; no tag = info-less
    // kernel-internal sender (timer/mq/pdeathsig paths). Rate-limited so a
    // runaway killer cannot flood the console.
    //
    // RETIRED (probe-retire): the trace was unconditional (it fired on
    // every systemd SIGTERM/SIGHUP of a service unit). It now lives
    // behind the existing signal-family switch (rux:sigtrace (retired dfx=sigtrace)) — the same
    // gate the other signal forensics in this file use — so an ordinary
    // boot is silent and a signal hunt can still arm it.
    if crate::trace::EVENT_SIGTRACE.armed()
        && (sig == Signal::SIGKILL as i32
            || sig == Signal::SIGTERM as i32
            || sig == Signal::SIGSEGV as i32
            || sig == Signal::SIGHUP as i32)
    {
        static KILLTRACE_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if let Some(cur) = crate::sched::current() {
            {
                let same_group = cur.tgid() == task.tgid();
                let s_comm = (*cur).comm();
                let s_len = s_comm
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(s_comm.len());
                let t_comm = task.comm();
                let t_len = t_comm
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(t_comm.len());
                let t_str = core::str::from_utf8(&t_comm[..t_len]).unwrap_or("?");
                let watched = t_str.starts_with("gnome-session") || t_str.starts_with("ls");
                let group_teardown = same_group && sig == Signal::SIGKILL as i32;
                let interesting = sig == Signal::SIGTERM as i32
                    || sig == Signal::SIGHUP as i32
                    || (sig == Signal::SIGKILL as i32 && !same_group)
                    || watched;
                if interesting
                    && !group_teardown
                    && KILLTRACE_N.fetch_add(
                        1, core::sync::atomic::Ordering::Relaxed) < 300
                {
                    match si {
                        Some(i) => crate::pr_info!(
                            "KILLTRACE: pid={}({}) -> pid={}({}) tgid={} ppid={} sig={} origin={:#x}",
                            cur.pid(),
                            core::str::from_utf8(&s_comm[..s_len])
                                .unwrap_or("?"),
                            task.pid(),
                            t_str,
                            task.tgid(),
                            task.ppid(),
                            sig,
                            i.si_code,
                        ),
                        None => crate::pr_info!(
                            "KILLTRACE: pid={}({}) -> pid={}({}) tgid={} ppid={} sig={} origin=none",
                            cur.pid(),
                            core::str::from_utf8(&s_comm[..s_len])
                                .unwrap_or("?"),
                            task.pid(),
                            t_str,
                            task.tgid(),
                            task.ppid(),
                            sig,
                        ),
                    }
                }
            }
        }
    }

    // Linux semantics for the global init ("only signals it has a handler
    // for are DELIVERED", signal(7)) are enforced at DELIVERY time in
    // do_signal — never here at send time. The old send-time discard
    // dropped BLOCKED signals before they could pend, which broke the
    // pid-1 reaper pattern: OH init (and systemd) block SIGCHLD and read
    // it through a signalfd, so no handler is ever installed. With the
    // send-time filter every SIGCHLD to init vanished at the exit-notify
    // path — init never reaped, dead services stayed zombies, and the
    // critical-service restart (foundation hosts ams/bms/wms) never ran
    // (OH boots died at bootanimation.finished with foundation SIGKILLed
    // and unreaped). Linux's sig_ignored() explicitly exempts blocked
    // signals ("Blocked signals are never ignored"); the SIGNAL_UNKILLABLE
    // check lives in get_signal() — after dequeue, on the delivery path.
    // do_signal reproduces that: for pid 1, a dequeued signal without an
    // installed handler is discarded instead of executed (SIGKILL/SIGSTOP
    // included — you cannot install handlers for those), so the LTP
    // kill10 broadcast-kill protection is preserved, while pending
    // blocked signals stay observable via signalfd/sigwaitinfo as on
    // Linux. Kernel-forced kills of a crashing init bypass do_signal (the
    // trap path calls do_exit directly), so a genuinely broken init still
    // dies.

    // SIGKILL and SIGSTOP cannot be ignored
    if sig == Signal::SIGKILL as i32 || sig == Signal::SIGSTOP as i32 {
        // (KILLTRACE logging happens at the top of this function — it
        // covers SIGKILL/SIGTERM/SIGSEGV with sender attribution.)
        if let Some(info) = si {
            task.pending.add_info(info);
        } else {
            task.pending.add(sig);
        }
        crate::signal::signal_wake_up(task as *const crate::process::task::Task as *mut crate::process::task::Task);
        // P1 signalfd hook: wake signalfd readers of this task.
        crate::fs::event_notify::signalfd_notify_task(task as *const crate::process::task::Task as *mut crate::process::task::Task);
        emit(if already_std { 2 } else { 0 });
        return Ok(());
    }

    // Idle task has no signal handling
    let signal_ref: &SignalStruct = match task.signal.as_ref() {
        Some(s) => s,
        None => {
            task.pending.add(sig);
            crate::signal::signal_wake_up(task as *const crate::process::task::Task as *mut crate::process::task::Task);
            // P1 signalfd hook: wake signalfd readers of this task.
            crate::fs::event_notify::signalfd_notify_task(task as *const crate::process::task::Task as *mut crate::process::task::Task);
            emit(if already_std { 2 } else { 0 });
            return Ok(());
        }
    };

    // SFDDBG (launcher bring-up): trace every signal TO pid 1 through the
    // send path — disposition, block state, pending-after. OH init blocks
    // SIGCHLD and reads it via signalfd; any silent drop here wedges the
    // whole service-restart machinery. Rate-limited. Serial-cleanliness:
    // opt-in via rux:sfddbg (retired dfx=sfddbg) (every child exit reaches here on ordinary
    // boots too).
    if task.pid() == 1
        && crate::trace::EVENT_SFDDBG.armed()
    {
        static SFDDBG_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if SFDDBG_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 200 {
            let disp = signal_ref
                .get_action(sig)
                .map(|a| match a.action() {
                    SigActionKind::Default => "DFL",
                    SigActionKind::Ignore => "IGN",
                    SigActionKind::Handler => "HND",
                })
                .unwrap_or("none");
            let blocked = task.sigmask & (1u64 << (sig - 1)) != 0;
            crate::pr_info!(
                "SFDDBG: send->pid1 sig={} disp={} blocked={} pending={:#x}",
                sig,
                disp,
                blocked,
                task.pending.get_all()
            );
        }
    }

    // Linux sig_ignored() ordering: BLOCKED signals are NEVER ignored —
    // check the block mask BEFORE the SIG_IGN discard. A task that blocks
    // a signal and consumes it via signalfd/sigwaitinfo (systemd's and OH
    // init's reaper: block SIGCHLD, read it from a signalfd, SIG_IGN
    // disposition untouched) must see it pend; the old order ran the
    // Ignore-drop first and the signal vanished before the fd could ever
    // observe it (OH boots: init never reaped a single child; every dead
    // service stayed zombie and the critical-service restart never ran).
    //
    // Linux prepare_signal(): a SIG_IGN disposition (and not blocked)
    // discards the signal BEFORE it is marked pending. The old
    // add-then-remove left a transient pending bit that
    // wait_event_interruptible polls could observe — the waiter then
    // returned -ERESTARTSYS for a signal that vanished before delivery,
    // leaking the raw -512 sentinel to userspace as a bogus errno.
    //
    // sig_task_ignored() exception: a TRACED task never ignores a signal
    // (Linux's ptrace check exempts only SIGKILL/SIGSTOP, which bypass
    // this path anyway) — the tracer must see every delivery as a
    // signal-delivery-stop, even one whose disposition is SIG_IGN (LTP
    // ptrace01/ptrace05: a TRACEME'd child SIG_IGN'ing a signal, or the
    // default-ignored SIGCHLD, still has to stop when the signal arrives).
    let sig_blocked = sig >= 1
        && sig <= 64
        && task.sigmask & (1u64 << (sig - 1)) != 0;
    if !sig_blocked {
        if let Some(action) = signal_ref.get_action(sig) {
            if action.action() == SigActionKind::Ignore && task.tracer_pid() == 0 {
                emit(1);
                return Ok(());
            }
        }
    }

    // Add signal to pending set BEFORE checking mask.
    // Masked signals stay pending and will be delivered when unmasked.
    //
    // Linux __send_signal(): real-time signals with a user-class si_code
    // (SI_TKILL/SI_QUEUE/... — si_code < 0) allocate a sigqueue that is
    // charged against RLIMIT_SIGPENDING; when the limit is exhausted the
    // send fails EAGAIN (kill()-originated SI_USER is exempt — kill is
    // not allowed to fail EAGAIN). Standard signals coalesce in the
    // bitmap and never take this path.
    if let Some(info) = si {
        if info.si_signo >= SIGRTMIN && info.si_code < 0 {
            // irqsave: the rlimits spinlock is shared with IRQ context
            // (scheduler_tick reads RLIMIT_CPU of current via
            // Task::rlimit's lock_irqsave). A plain-lock holder
            // interrupted by its own CPU's tick self-deadlocks the tick
            // at IF=0 — the both-CPU-freeze DEADLOCK-spin class (the
            // spinner prints at Task::rlimit+0x89; verified byte-exact
            // on the OH A/B pollwake roll pw-5 and cg-1's storm).
            let limit = task.rlimits.lock_irqsave()[crate::process::task::rlimit_res::SIGPENDING].0;
            if limit != u64::MAX && task.pending.rt_queued_count() as u64 >= limit {
                emit(4);
                return Err(crate::errno::Errno::TryAgain.as_neg_i32());
            }
        }
        task.pending.add_info(info);
    } else {
        task.pending.add(sig);
    }

    // Check if the signal is blocked FOR THIS THREAD — still pending,
    // just not delivered now.
    if sig >= 1 && sig <= 64 {
        let bit = 1u64 << (sig - 1);
        if task.sigmask & bit != 0 {
            // A blocked signal that just became pending must still WAKE
            // interruptible sleepers (Linux wakes sigtimedwait/sigwait
            // pollers; si block state only delays handler delivery).
            // Without this, a task that entered rt_sigtimedwait and THEN
            // received the (blocked) signal it waits for slept forever —
            // the LTP sigwait family hang (sigtimedwait01/sigwait01/
            // sigwaitinfo01/rt_sigtimedwait01). Waking is always safe:
            // every sleeper re-checks its own condition and re-sleeps if
            // unmet.
            crate::signal::signal_wake_up(task as *const crate::process::task::Task as *mut crate::process::task::Task);
            // P1 signalfd hook: a BLOCKED signal is exactly the signalfd
            // use case (the app blocks the mask and reads via the fd) —
            // no handler path will run, so this wake is the only thing
            // that unblocks a read(sfd) on this signal.
            crate::fs::event_notify::signalfd_notify_task(task as *const crate::process::task::Task as *mut crate::process::task::Task);
            emit(if already_std { 2 } else { 0 });
            return Ok(());
        }
    }

    // Default or Handler disposition — wake the target so an
    // interruptible sleeper reaches the delivery point.
    crate::signal::signal_wake_up(task as *const crate::process::task::Task as *mut crate::process::task::Task);
    // P1 signalfd hook: wake signalfd readers of this task (in-mask
    // signals are typically blocked, so they stay pending and are only
    // observable through the fd — this wake is what unblocks read(sfd)).
    crate::fs::event_notify::signalfd_notify_task(task as *const crate::process::task::Task as *mut crate::process::task::Task);
    emit(if already_std { 2 } else { 0 });
    Ok(())
}

/// Send a signal with a user-supplied si_code (rt_sigqueueinfo family).
///
/// Queues a full SigInfo for real-time signals (sigwaitinfo consumers)
/// and falls back to the plain bitmap for standard signals.
pub fn send_signal_info(pid: u32, sig: i32, si_code: i32) -> Result<(), i32> {
    if sig < 1 || sig > 64 {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }
    let my_pid = crate::process::current_pid();
    let my_uid = match crate::sched::current() {
        Some(c) => (*c).cred().uid,
        None => 0,
    };
    let info = SigInfo::new(sig, si_code, my_pid, my_uid);
    crate::signal::send_signal_with_info(pid, info)
}

/// Send a signal to all processes in a given process group.
///
/// Used by the TTY ISIG handler to deliver SIGINT/SIGQUIT/SIGTSTP
/// to the foreground process group.
pub fn send_signal_to_pgid(pgid: u32, sig: i32) {
    // Broadcast over the PID hash so sleeping tasks receive it too; each
    // target still goes through the kill permission check (kernel-originated
    // signals from the TTY layer pass because init runs as root).
    crate::process::pid_hash::pid_hash_for_each_task(|task| {
        // The walk hands &Task under the bucket lock — plain safe reads.
        if task.pgid() == pgid && crate::security::can_send_signal(task.cred()) {
            let _ = crate::signal::send_signal(task.pid(), sig);
        }
    });
}

// ============================================================================
// Delivery-path policy
// ============================================================================

/// The default-action table (the policy half of the old framework-side
/// handle_default_signal match): which CLASS of default behavior a
/// signal number gets when no handler is installed. The executor —
/// state changes, CLD_STOPPED/CLD_CONTINUED parent notification,
/// do_exit_group — stays framework-side (it holds the audited
/// TaskBorrow edges) and matches on the returned class.
pub fn default_action_class(sig: i32) -> crate::signal_hooks::DefaultActionClass {
    use crate::signal_hooks::DefaultActionClass;
    match sig {
        // Ignore by default: SIGCHLD(17), SIGURG(23), SIGWINCH(28);
        // unknown signal numbers default-ignore too.
        17 | 23 | 28 => DefaultActionClass::Ignore,
        // SIGCONT(18): resume a job-control stop AND record the
        // continuation event (reference prepare_signal/do_notify_parent_
        // cldstop(CLDCONTINUED)): a parent's waitid/waitpid with
        // WCONTINUED must see CLD_CONTINUED exactly once per resume.
        18 => DefaultActionClass::Continue,
        // Stop process: SIGSTOP(19), SIGTSTP(20), SIGTTIN(21), SIGTTOU(22)
        19 | 20 | 21 | 22 => DefaultActionClass::GroupStop,
        // Terminate process (core dump or direct termination):
        // SIGHUP..SIGSTKFLT (1-16), SIGXCPU/SIGXFSZ/SIGVTALRM/SIGPROF
        // (24-27), SIGIO/SIGPWR/SIGSYS (29-31).
        1..=16 | 24..=27 | 29..=31 => DefaultActionClass::Terminate,
        _ => DefaultActionClass::Ignore,
    }
}

/// The pid-namespace init delivery filter (Linux get_signal()'s
/// SIGNAL_UNKILLABLE rule): should this dequeued signal to a pid-ns init
/// be DISCARDED instead of executed? Only signals the init installed a
/// handler for are delivered (signal(7)); kernel-forced SIGKILL and
/// ancestor-namespace user kills pierce the rule; the group-stop family
/// still stops a container init (kselftest memfd's nested-sysctl legs
/// park their helper children exactly this way).
///
/// `code`/`pid` are the queued siginfo's si_code/si_pid (0 when the
/// queue holds no payload — a bare bitmap bit); `handled` is whether
/// the disposition is a user handler. Called only after the framework's
/// is_unkillable_init predicate matched.
pub fn init_delivery_filter(sig: i32, handled: bool, code: i32, pid: i32) -> bool {
    // Only kernel-forced SIGKILL and user SIGKILLs from OUTSIDE
    // the receiver's namespace pierce the rule (reference
    // __send_signal's from_ancestor_ns: same-namespace user kills
    // of a pid-namespace init are discarded — LTP pidns04's
    // kill(1, SIGKILL) — while an ancestor-namespace user kill
    // DOES take the init down — LTP pidns05's parent kill). The
    // queue-time si_pid translation already encodes the sender's
    // visibility: a cross-namespace sender's pid reads 0.
    let same_ns_user_kill =
        (code == si_code::SI_USER || code == si_code::SI_TKILL) && pid != 0;
    let forced_kill = sig == Signal::SIGKILL as i32 && !same_ns_user_kill;
    // SIGNAL_UNKILLABLE protects only FATAL default actions
    // (terminate/core). The group-stop family STOPS a pid-ns
    // init and SIGCONT resumes it (reference get_signal():
    // sig_kernel_stop -> do_group_stop with no UNKILLABLE
    // consult; SIGCONT acts at send time in prepare_signal) —
    // a container init honors SIGSTOP/SIGCONT even with no
    // handler installed.
    let stop_or_cont = matches!(sig, 18 | 19 | 20 | 21 | 22);
    !handled && !forced_kill && !stop_or_cont
}

// ============================================================================
// Disposition-table policy (SignalStruct handler rules)
// ============================================================================

/// Reset signal handlers to SIG_DFL on execve (POSIX requirement)
/// — the policy body of SignalStruct::flush_handlers.
///
/// POSIX: "Signals set to the default action shall be set to the default
/// for the new process image. Signals set to be caught by the calling
/// process shall be set to the default action. Signals set to SIG_IGN
/// shall be set to SIG_IGN."
///
/// If `force_default` is true, even SIG_IGN handlers are reset (used by
/// some privileged exec paths).
pub fn flush_handlers(ss: &SignalStruct, force_default: bool) {
    ss.with_actions_mut(|actions| {
        for i in 0..64 {
            let sig = (i + 1) as i32;
            // SIGKILL and SIGSTOP cannot be caught/ignored, skip
            if sig == Signal::SIGKILL as i32 || sig == Signal::SIGSTOP as i32 {
                continue;
            }
            if !force_default && actions[i].action() == SigActionKind::Ignore {
                continue;  // Preserve SIG_IGN across exec
            }
            actions[i] = SigAction::new();  // Reset to SIG_DFL
        }
    });
}

/// CLONE_CLEAR_SIGHAND (kernel flush_signal_handlers(t, 0), called
/// from copy_sighand): reset every signal disposition that is not
/// SIG_IGN to SIG_DFL, clearing flags and mask. Used by glibc's
/// posix_spawn (via clone3) to give the spawn helper a clean signal
/// state without a helper syscall — the flag rides along CLONE_VM
/// where the child cannot run sigaction code before exec. SIG_IGN
/// dispositions SURVIVE (reference passes force_default=0: "Reset
/// all signal handler not set to SIG_IGN to SIG_DFL"); SIGCHLD is
/// NOT special-cased.
pub fn reset_all_handlers_default(ss: &SignalStruct) {
    ss.with_actions_mut(|actions| {
        for a in actions.iter_mut() {
            let keep_ign = a.action() == SigActionKind::Ignore;
            *a = if keep_ign { SigAction::ignore() } else { SigAction::new() };
        }
    });
}

/// Compute the /proc/[pid]/status SigIgn and SigCgt mask pair in one
/// lock hold: SigIgn has a bit per signal disposed to SIG_IGN, SigCgt
/// a bit per signal with a user handler installed.
pub fn ign_cgt_masks(ss: &SignalStruct) -> (u64, u64) {
    ss.with_actions_mut(|actions| {
        let mut ign = 0u64;
        let mut cgt = 0u64;
        for (i, a) in actions.iter().enumerate() {
            match a.action() {
                SigActionKind::Ignore => ign |= 1u64 << i,
                SigActionKind::Handler => cgt |= 1u64 << i,
                SigActionKind::Default => {}
            }
        }
        (ign, cgt)
    })
}
