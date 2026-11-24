//! The wait family, carved out of process/exit.rs for the process
//! service crate (crate-flip rung: process). The exit MECHANISM
//! (do_exit/do_exit_group/reap/release) stays framework-side in
//! process/exit.rs — its callers are the arch traps, signal delivery
//! and the scheduler reap path (hot); this file is the wait4/waitid
//! policy the syscall layer reaches through the crate.

#![forbid(unsafe_code)]
use crate::errno;
use crate::process::task::{Pid, Task, TaskState};
use crate::process::exit::{release_task, ERESTARTSYS};
/// Per-arch wait-class entry syscall numbers (NATIVE numbering, same
/// decode contract as the r41v17 TIMEDSLEEP ticktrap's NS_SYS).
#[cfg(feature = "x86_64")]
const WAIT_SYS: [u64; 2] = [61, 247]; // wait4, waitid
#[cfg(feature = "riscv64")]
const WAIT_SYS: [u64; 2] = [260, 95]; // wait4, waitid
#[cfg(feature = "aarch64")]
const WAIT_SYS: [u64; 2] = [260, 95]; // wait4, waitid

/// Park-age tracking ring: (pid, first-seen-parked jiffy, last-reported
/// jiffy). A row lives while its pid is observed parked under a wait
/// syscall; one absent beat clears it (normal wait4 parks are
/// milliseconds and never survive two 1 Hz beats).
struct WlRow {
    pid: core::sync::atomic::AtomicU32,
    since: core::sync::atomic::AtomicU32,
    reported: core::sync::atomic::AtomicU32,
}
const WL_RING: usize = 8;
static WL_SEEN: [WlRow; WL_RING] = [const {
    WlRow {
        pid: core::sync::atomic::AtomicU32::new(0),
        since: core::sync::atomic::AtomicU32::new(0),
        reported: core::sync::atomic::AtomicU32::new(0),
    }
}; WL_RING];

/// Read a parked task's kernel-entry syscall frame (nr + first arg),
/// mirroring the TIMEDSLEEP ghost hunt's entry_sys contract. Returns
/// (0, 0) when the stack is not mapped/sane. `tk` is a live task
/// pinned by the caller's pid-hash walk; the frame read goes through
/// the typed `pt_regs_ref` view.
fn wl_entry_syscall(tk: &Task) -> (u64, u64) {
    if tk.kernel_stack_top_checked().is_none() {
        return (0, 0);
    }
    match tk.pt_regs_ref() {
        #[cfg(feature = "x86_64")]
        Some(regs) => (regs.orig_rax as u64, regs.rdi as u64),
        #[cfg(feature = "riscv64")]
        Some(regs) => (regs.a7 as u64, regs.a0 as u64),
        #[cfg(feature = "aarch64")]
        Some(regs) => (regs.regs[8] as u64, regs.regs[0] as u64),
        None => (0, 0),
    }
}

/// wait4-park census — the §87.8 verdict instrument. Anomaly-gated,
/// self-rated to 1 Hz, pid-deduped 30 s, max 2 lines/beat: zero serial
/// on healthy boots (a normal fork+wait park is milliseconds and never
/// survives two beats).
///
/// A task INTERRUPTIBLE-parked under a wait4/waitid entry frame for
/// >= 2 s gets ONE verdict line naming every hop of the child-exit wake
/// chain, so a capture decodes the loss site in a single read:
///
///   ch=[pid:st ...] nz=N ns=M  children census (ZOMBIE=0x20 bit). A
///                             parked waiter WITH nz>0 = the condition
///                             is true and the wake never landed.
///   q=(entries,woken)         the waiter's wait_chldexit queue; an
///                             entry whose woken flag is set while the
///                             task sleeps = the one-shot token was
///                             consumed without a running task (the
///                             W1/sc100 sticky-token family).
///   pend=0x.. blk=0x..        pending/blocked signal masks (bit 16 =
///                             SIGCHLD; pend set + still parked = the
///                             signal wake was eaten or never came).
///   slots=cN:parent/child     nonempty DEFERRED_EXIT_NOTIFY slots; one
///                             naming this waiter = the child's exit
///                             notify was stored and never delivered.
pub fn wait4_park_audit() {
    use core::sync::atomic::Ordering as O;
    use crate::process::task::TaskState;

    // 1 Hz self-gate (one relaxed load per tick when quiet).
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::timer::get_jiffies();
    let last = LAST.load(O::Relaxed);
    if now.wrapping_sub(last) < 100 {
        return;
    }
    if LAST
        .compare_exchange(last, now, O::AcqRel, O::Acquire)
        .is_err()
    {
        return;
    }

    // Pass 1: find wait-class sleepers, refresh the age ring.
    // (pid, ptr, age_jiffies, since_row_index)
    let mut waiters: [(u32, *mut Task, u64); 2] = [(0, core::ptr::null_mut(), 0); 2];
    let mut nwait = 0usize;
    let mut observed = [false; WL_RING];
    crate::sched::for_each_task_try(|t| {
        if nwait == 2 {
            return;
        }
        // Diagnostic reads of a task pinned by the try-walk (typed).
        let (sleeping, pid) = (t.state().is_sleeping(), t.pid());
        if !sleeping || pid == 0 {
            return;
        }
        let (nr, _a0) = wl_entry_syscall(t);
        if !WAIT_SYS.contains(&nr) {
            return;
        }
        // Track park age in the ring.
        let mut row = WL_RING;
        for (i, r) in WL_SEEN.iter().enumerate() {
            if r.pid.load(O::Acquire) == pid {
                row = i;
                observed[i] = true;
                break;
            }
        }
        if row == WL_RING {
            for (i, r) in WL_SEEN.iter().enumerate() {
                if r.pid.load(O::Acquire) == 0 {
                    r.pid.store(pid, O::Release);
                    r.since.store(now as u32, O::Release);
                    r.reported.store(0, O::Release);
                    row = i;
                    observed[i] = true;
                    break;
                }
            }
        }
        if row == WL_RING {
            return; // ring full of other parked waiters
        }
        let since = WL_SEEN[row].since.load(O::Acquire) as u64;
        let age = now.saturating_sub(since);
        if age >= 200 {
            waiters[nwait] = (pid, t as *const Task as *mut Task, age);
            nwait += 1;
        }
    });
    // Clear rows whose pid was not observed parked this beat.
    for (i, r) in WL_SEEN.iter().enumerate() {
        if !observed[i] && r.pid.load(O::Acquire) != 0 {
            r.pid.store(0, O::Release);
            r.since.store(0, O::Release);
        }
    }
    if nwait == 0 {
        return;
    }

    // Pass 2 (under the process-tree try-lock so parent links are
    // stable): classify every task whose parent is one of the waiters,
    // and collect the waiter-side verdict fields. Skips the beat when
    // the tree lock is contended (the next 1 Hz beat retries).
    let tree = match crate::process::task::PROCESS_TREE_LOCK.try_lock() {
        Some(g) => g,
        None => return,
    };
    // per-waiter accumulators
    let mut nz = [0u32; 2];
    let mut nch = [0u32; 2];
    let mut shown: [alloc::vec::Vec<(u32, u32)>; 2] =
        [alloc::vec::Vec::new(), alloc::vec::Vec::new()];
    let mut qz = [(0usize, 0usize); 2];
    let mut found = [false; 2];
    let mut pend = [0u64; 2];
    let mut blk = [0u64; 2];
    let mut st_bits = [0u32; 2];
    let mut nr_arg = [0u64; 2];
    crate::sched::for_each_task_try(|t| {
        // Reads under the tree lock (release_task's tree detach also
        // holds it), each task pinned by the try-walk.
        {
            for w in 0..nwait {
                if t.pid() == waiters[w].0 {
                    found[w] = true;
                    st_bits[w] = t.state().bits();
                    let (nr, a0) = wl_entry_syscall(t);
                    nr_arg[w] = (a0 << 32) | nr;
                    pend[w] = t.pending.get_all();
                    blk[w] = t.sigmask;
                    if let Some((total, woken)) = t.wait_chldexit.vw_queue_census_try() {
                        qz[w] = (total, woken);
                    }
                    continue;
                }
                if let Some(pp) = t.parent_ptr() {
                    if Task::is_plausible_task_ptr(pp)
                        && crate::process::unsafe_edges::borrow_task(pp as *mut Task).pid()
                            == waiters[w].0
                    {
                        nch[w] += 1;
                        let st = t.state().bits();
                        if st & TaskState::ZOMBIE != 0 {
                            nz[w] += 1;
                        }
                        if shown[w].len() < 6 {
                            shown[w].push((t.pid(), st));
                        }
                    }
                }
            }
        }
    });
    drop(tree);

    // Print the verdict: only when the anomaly is real — a reapable
    // zombie under a parked waiter (the loss, any age) or a long park
    // (>= 10 s; the child-never-exited face). 30 s re-report dedup,
    // 2-line budget.
    for w in 0..nwait {
        if !found[w] {
            continue;
        }
        let idx = WL_SEEN.iter().position(|r| r.pid.load(O::Acquire) == waiters[w].0);
        let rep = idx.map(|i| WL_SEEN[i].reported.load(O::Acquire) as u64).unwrap_or(0);
        let anomaly = nz[w] > 0 || waiters[w].2 >= 1000;
        if !anomaly || (rep != 0 && now.saturating_sub(rep) < 3000) {
            continue;
        }
        if let Some(i) = idx {
            WL_SEEN[i].reported.store(now as u32, O::Release);
        }
        let mut slots = [0i64; crate::config::MAX_CPUS];
        crate::sched::deferred_exit_notify_slots(&mut slots);
        let mut line: alloc::string::String = alloc::format!(
            "WAIT4-PARK pid={} age_s={} sys={} a0={:#x} st={:#x} ch=[",
            waiters[w].0,
            waiters[w].2 / 100,
            nr_arg[w] & 0xFFFF_FFFF,
            nr_arg[w] >> 32,
            st_bits[w],
        );
        for (k, (cp, cst)) in shown[w].iter().enumerate() {
            if k > 0 {
                line.push(' ');
            }
            use core::fmt::Write;
            let _ = write!(line, "{}:{:#x}", cp, cst);
        }
        use core::fmt::Write;
        let _ = write!(
            line,
            "] nch={} nz={} q=({},{}) pend={:#x} blk={:#x} slots=",
            nch[w], nz[w], qz[w].0, qz[w].1, pend[w], blk[w]
        );
        let mut any_slot = false;
        for (c, s) in slots.iter().enumerate() {
            if *s != 0 {
                if any_slot {
                    line.push(' ');
                }
                any_slot = true;
                let parent = ((*s as u64) >> 32) as u32;
                let child = (*s as u64 & 0xFFFF_FFFF) as u32;
                let _ = write!(line, "c{}:{}/{}", c, parent, child);
            }
        }
        if !any_slot {
            line.push('-');
        }
        crate::pr_warn!("{}", line);
    }
}

/// wait4 pid-selector semantics (Linux wait_tasktype):
/// - pid > 0 : that exact child
/// - pid == 0: any child in the CALLER's process group
/// - pid == -1: any child
/// - pid < -1: any child whose pgid == -pid
#[inline]
fn wait_pid_matches(child: &Task, pid: i32, caller_pgid: u32) -> bool {
    if pid > 0 {
        child.pid() == pid as u32
    } else if pid == 0 {
        child.pgid() == caller_pgid
    } else if pid == -1 {
        true
    } else {
        child.pgid() == (-pid) as u32
    }
}

/// Blocking wait for child process state change
///
/// # Arguments
/// * `pid` - PID to wait for (-1 = any child, >0 = specific PID,
///           0 = caller's process group, <-1 = process group -pid)
/// * `status_ptr` - User pointer to store exit status
/// * `options` - Wait options (WUNTRACED, etc.)
///
/// # Returns
/// * `Ok(child_pid)` - Child PID that changed state
/// * `Err(-ECHILD)` - No children
/// * `Err(-EINTR)` - Interrupted by signal
/// * `Err(-EFAULT)` - status_ptr unwritable
pub fn do_wait(pid: i32, status_ptr: *mut i32, options: i32) -> Result<Pid, i32> {

    // The sched:sched_process_wait tracepoint — the reference records
    // it at do_wait entry with the WAITER's comm/prio and the REQUESTED
    // pid argument (pid_nr of the wait_opts pid; -1 = any child).
    if crate::trace::EVENT_SCHED_PROCESS_WAIT.armed() {
        if let Some(cur) = crate::sched::current() {
            // cur is the calling task, pinned by being the running context.
            {
                let words = crate::trace::comm_to_words(cur.comm());
                crate::trace::record(
                    &crate::trace::EVENT_SCHED_PROCESS_WAIT,
                    [
                        words[0],
                        words[1],
                        pid as u64,
                        cur.prio() as u64,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                    ],
                );
            }
        }
    }

    // current is the calling task, valid throughout the wait loop (the
    // task sleeps via schedule() in INTERRUPTIBLE state so it won't be
    // freed). One consolidated reference; raw uses re-derive the pointer.
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return Err(errno::Errno::NoChild.as_neg_i32()),
    };
    let current = cur as *mut Task;

    let current_pid = cur.pid();

    // If current is idle task (PID 0), no real process is running
    if current_pid == 0 {
        return Err(errno::Errno::NoChild.as_neg_i32());
    }

    let caller_pgid = cur.pgid();

        loop {
            let mut found_child = false;
            let mut zombie_child: Option<*mut Task> = None;
            let mut stopped_child: Option<*mut Task> = None;

            const WUNTRACED: i32 = 0x00000002;

            // Iterate over the children of EVERY thread-group member
            // (Linux do_wait walks while_each_thread — any thread can
            // wait for any member's child; glib's gmain worker thread
            // reaps children the main thread spawned). Threads are never
            // on these lists — only fork()/clone-without-CLONE_THREAD
            // products are.
            cur.for_each_group_child(|child_ptr| {
                // SAFETY: the walker hands out children linked under the
                // tree lock (valid for the closure body).
                let child = crate::process::unsafe_edges::borrow_task(child_ptr);

                if !wait_pid_matches(child, pid, caller_pgid) {
                    return;
                }

                found_child = true;

                // Check if it's in Zombie state
                if child.state() == TaskState::new(TaskState::ZOMBIE) {
                    // E8-REPAR: CLAIM the zombie here, inside the same
                    // tree-lock critical section that selected it
                    // (unlink + parent=None). Two group members woken by
                    // the same SIGCHLD (glib reaper thread + main) could
                    // both select this zombie and double-release_task it
                    // — the second release would lock/unlink/free through
                    // the already-freed Task. Losers simply fail the
                    // claim and re-scan. Claim only the first winner:
                    // every claimed zombie must be reaped by this call or
                    // it becomes unreapable (no list links it anymore).
                    if zombie_child.is_none() && Task::claim_child_locked(child_ptr) {
                        zombie_child = Some(child_ptr);
                    }
                } else if child.state() == TaskState::new(TaskState::STOPPED)
                    && !child.stop_reported.load(core::sync::atomic::Ordering::Acquire)
                    && (options & WUNTRACED != 0 || child.tracer_pid() == current_pid)
                    && stopped_child.is_none()
                {
                    // Linux wait_task_stopped(): "Traditionally we see
                    // ptrace'd stopped children regardless of options" —
                    // WUNTRACED gates job-control stops only; a stop of a
                    // task WE trace is always reported (LTP ptrace01/05
                    // use plain waitpid on a TRACEME'd child). First
                    // match wins, like the zombie selection above.
                    stopped_child = Some(child_ptr);
                }
            });

            // If found zombie child, reap it
            if let Some(child_ptr) = zombie_child {
                // SAFETY: claimed under the tree lock (claim_child_locked)
                // — this caller owns the reap.
                let child = crate::process::unsafe_edges::borrow_task(child_ptr);
                let child_pid = child.pid();
                let raw_exit = child.exit_code();

                // Encode exit status per waitpid ABI:
                // - Normal exit: exit_code IS the wait status already —
                //   sys_exit/sys_exit_group store `(code & 0xff) << 8`
                //   (Linux pre-shifts at the syscall boundary), so the
                //   value is passed through with no second shift.
                // - Killed by signal: status = |signal_number|      (WIFSIGNALED, WTERMSIG)
                //   plus WCOREDUMP (0x80) when a core file was written.
                let status: i32 = if raw_exit >= 0 {
                    (raw_exit as u32 & 0xFFFF) as i32
                } else {
                    let mut s = (-(raw_exit as i32) as u32 & 0x7F) as i32;
                    if child.core_dumped() {
                        s |= 0x80; // WCOREDUMP
                    }
                    s
                };

                // Write exit status through the typed exception-table path.
                if !status_ptr.is_null() {
                    let _uncopied = crate::arch::uaccess_safe::copy_out_t(status_ptr as usize, &status);
                }

                // Release task resources (kernel stack, Arc refs, PID)
                // Note: remove_child is done inside release_task()
                let fp_t_reap =
                    if crate::debug::forkprof::on() { crate::debug::forkprof::now() } else { 0 };
                release_task(child_ptr);
                if fp_t_reap != 0 {
                    crate::debug::forkprof::add(crate::debug::forkprof::P_WT_REAP, fp_t_reap);
                    crate::debug::forkprof::trace(crate::debug::forkprof::T_REAP_DONE, child_pid);
                }

                return Ok(child_pid);
            }

            // If found stopped child (WUNTRACED), report it
            if let Some(child_ptr) = stopped_child {
                // SAFETY: selected under the tree lock; still linked.
                let child = crate::process::unsafe_edges::borrow_task(child_ptr);
                let child_pid = child.pid();
                let stop_sig = child.stop_signal();

                // Encode stopped status: (stop_signal << 8) | 0x7F
                let status: i32 = (((stop_sig as u32) << 8) | 0x7F) as i32;

                // Write status safely using copy_to_user
                if !status_ptr.is_null() {
                    let _uncopied =
                        crate::arch::uaccess_safe::copy_out_t(status_ptr as usize, &status);
                }

                // Mark this stop event as reported (only report once)
                child.stop_reported.store(true, core::sync::atomic::Ordering::Release);

                // Note: stopped child is NOT reaped, it stays in children list
                return Ok(child_pid);
            }

            // Linux do_wait() also walks the caller's TRACEE list
            // (ptrace_do_wait): a task attached with PTRACE_ATTACH that is
            // not a real child (LTP ptrace11 attaches init and
            // waitpid(1,...)s) is still waitable for its stops. A tracee
            // that has not stopped yet also keeps the wait blocking (it
            // plays the role of a child for ECHILD purposes).
            let mut found_tracee = false;
            {
                let mut tracee_stop: Option<*mut Task> = None;
                crate::process::pid_hash::pid_hash_for_each_task(|t| {
                    if tracee_stop.is_some() {
                        return;
                    }
                    if (*t).tracer_pid() != current_pid || (*t).pid() == current_pid {
                        return;
                    }
                    if !wait_pid_matches(&*t, pid, caller_pgid) {
                        return;
                    }
                    found_tracee = true;
                    if t.state() == TaskState::new(TaskState::STOPPED)
                        && !t.stop_reported.load(core::sync::atomic::Ordering::Acquire)
                    {
                        tracee_stop = Some(t as *const Task as *mut Task);
                    }
                });
                if let Some(t) = tracee_stop {
                    // SAFETY: `t` was found by the pid-hash walk under the
                    // same scan discipline (a tracee of ours, pinned by
                    // the hash's RCU section).
                    let ts = crate::process::unsafe_edges::borrow_task(t);
                    let child_pid = ts.pid();
                    let stop_sig = ts.stop_signal();
                    let status: i32 = (((stop_sig as u32) << 8) | 0x7F) as i32;
                    if !status_ptr.is_null() {
                        let _uncopied =
                            crate::arch::uaccess_safe::copy_out_t(status_ptr as usize, &status);
                    }
                    ts.stop_reported.store(true, core::sync::atomic::Ordering::Release);
                    return Ok(child_pid);
                }
            }

            // No zombie or stopped child found
            if found_child || found_tracee {
                // Atomically add to waitqueue AND set INTERRUPTIBLE.
                // Prevents lost-wakeup race where child exits between
                // add() and set_state(), marking the entry woken but
                // wake_up_process skips the actual wake (task still RUNNING).
                cur.wait_chldexit.prepare_to_wait(current, false, true);

                // Recheck after prepare_to_wait (waker may have fired
                // between the initial scan and prepare_to_wait — a wake of
                // a still-RUNNING waiter is refused, so the state change
                // can only be caught here). If a child is already zombie or
                // freshly STOPPED (ptrace stop / group stop landing in that
                // window), finish_wait restores RUNNING state and we loop
                // to report it — no schedule() needed. Without the stopped
                // recheck, a stop that raced the arm was only visible via
                // the (discarded-for-SIG_IGN-SIGCHLD) signal path and the
                // wait slept forever (LTP ptrace05 wedge family).
                {
                    let mut need_rescan = false;
                    cur.for_each_group_child(|child_ptr| {
                        if need_rescan {
                            return;
                        }
                        // SAFETY: tree-locked child from the walker.
                        let c = crate::process::unsafe_edges::borrow_task(child_ptr);
                        if !wait_pid_matches(c, pid, caller_pgid) {
                            return;
                        }
                        let st = c.state();
                        if st == TaskState::new(TaskState::ZOMBIE) {
                            need_rescan = true;
                        } else if st == TaskState::new(TaskState::STOPPED)
                            && !c.stop_reported.load(core::sync::atomic::Ordering::Acquire)
                            && (options & WUNTRACED != 0 || c.tracer_pid() == current_pid)
                        {
                            need_rescan = true;
                        }
                    });
                    if !need_rescan {
                        crate::process::pid_hash::pid_hash_for_each_task(|t| {
                            if need_rescan {
                                return;
                            }
                            if (*t).tracer_pid() != current_pid || (*t).pid() == current_pid {
                                return;
                            }
                            if !wait_pid_matches(&*t, pid, caller_pgid) {
                                return;
                            }
                            if (*t).state() == TaskState::new(TaskState::STOPPED)
                                && !(*t).stop_reported.load(core::sync::atomic::Ordering::Acquire)
                            {
                                need_rescan = true;
                            }
                        });
                    }
                    if need_rescan {
                        cur.wait_chldexit.finish_wait(current);
                        // The child's wake_up() may have raced our
                        // prepare_to_wait and enqueued us; we are going to
                        // keep running on this CPU — take ourselves back
                        // off the queue (review NEW-C2).
                        crate::sched::dequeue_if_enqueued(cur);
                        continue; // re-enter loop to report/reap
                    }
                }

                // Check for pending signals before sleeping
                use crate::signal;
                if signal::signal_pending() {
                    cur.wait_chldexit.finish_wait(current);
                    crate::sched::dequeue_if_enqueued(cur);
                    // ERESTARTSYS, not EINTR: the signal-delivery path
                    // restarts the wait when the handler is SA_RESTART
                    // (Linux semantics — glib's SIGCHLD handler is).
                    return Err(ERESTARTSYS);
                }

                // Enable interrupts before schedule(). We're in syscall context
                // (SIE=0). Without this, timer IRQ can't fire and the task
                // can never be rescheduled.
                crate::arch::cpu::restore_irq(true);

                // Schedule other processes
                // forkprof: this schedule() returns only after the child
                // ran + exited + woke us — it IS the fork->exit handoff.
                let fp_t_ws =
                    if crate::debug::forkprof::on() { crate::debug::forkprof::now() } else { 0 };
                if fp_t_ws != 0 {
                    crate::debug::forkprof::trace(crate::debug::forkprof::T_SCHED_SLEEP, current_pid);
                }
                crate::sched::schedule();
                if fp_t_ws != 0 {
                    crate::debug::forkprof::add(crate::debug::forkprof::P_WT_SCHED, fp_t_ws);
                    crate::debug::forkprof::trace(crate::debug::forkprof::T_SCHED_WAKE, current_pid);
                }

                // After wakeup: state is RUNNING (set by wake_up_process).
                // Clean up waitqueue entry before next iteration.
                cur.wait_chldexit.finish_wait(current);
            } else {
                // No child processes at all
                return Err(errno::Errno::NoChild.as_neg_i32());
            }
        }
}

/// Non-blocking wait for child process
///
/// # Arguments
/// * `pid` - PID to wait for (-1 = any, >0 = specific, 0 = caller's pgid,
///           <-1 = pgid == -pid)
/// * `status_ptr` - User pointer to store exit status
/// * `options` - WUNTRACED (report stopped children too)
///
/// # Returns
/// * `Ok(child_pid)` - Child PID that has exited / was reported stopped
/// * `Err(-ECHILD)` - No children
/// * `Err(-EAGAIN)` - Children exist but none reported (sys_wait4 → 0)
pub fn do_wait_nonblock(pid: i32, status_ptr: *mut i32, options: i32) -> Result<Pid, i32> {
    const WUNTRACED: i32 = 0x00000002;

    // current is the calling task, valid throughout this call; one
    // consolidated reference (raw uses re-derive the pointer).
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return Err(errno::Errno::NoChild.as_neg_i32()),
    };

    let current_pid = cur.pid();

    // If current is idle task (PID 0), no real process is running
    if current_pid == 0 {
        return Err(errno::Errno::NoChild.as_neg_i32());
    }

    let caller_pgid = cur.pgid();

        // W4SPIN entry heartbeat (69th): if a single wait4 call hangs
        // inside the group-child walk (tree-lock spin / corrupt list),
        // the reap-side print below never fires while pid 1 burns CPU
        // in this syscall — this heartbeat separates "stuck in the
        // walk" (heartbeat silent) from "reaping an endless stream"
        // (heartbeat + reap prints every second).
        if current_pid == 1
            && crate::trace::EVENT_PID1SPIN.armed()
        {
            use core::sync::atomic::{AtomicU64, Ordering as O2};
            static LAST_EJ: AtomicU64 = AtomicU64::new(0);
            let j = crate::timer::get_jiffies() as u64;
            let last = LAST_EJ.load(O2::Relaxed);
            if j.saturating_sub(last) >= 100
                && LAST_EJ
                    .compare_exchange(last, j, O2::AcqRel, O2::Relaxed)
                    .is_ok()
            {
                crate::pr_warn!("W4SPIN entry pid=1 wait_pid={} opts={:#x}", pid, options);
            }
        }

        let mut found_child = false;
        let mut zombie_ptr: Option<*mut Task> = None;
        let mut stopped_ptr: Option<*mut Task> = None;

        // Scan the children of every thread-group member to find a
        // zombie / WUNTRACED stop — do NOT modify the list during
        // iteration. (Group-wide: glib's gmain worker thread polls
        // waitpid(WNOHANG) for children the main thread spawned.)
        cur.for_each_group_child(|child_ptr| {
            // SAFETY: selected/claimed under the tree lock; still linked.
            let child = crate::process::unsafe_edges::borrow_task(child_ptr);

            if !wait_pid_matches(child, pid, caller_pgid) {
                return;
            }

            found_child = true;

            // E8-REPAR discipline (same as do_wait): CLAIM the zombie
            // here, inside the walk's tree-lock critical section. Two
            // group members polling WNOHANG concurrently (glib reaper
            // thread + main) could both select the same zombie and
            // double-release_task it — the second release would
            // lock/unlink/free through the already-freed Task. A failed
            // claim means another waiter is reaping it; keep scanning.
            if child.state() == TaskState::new(TaskState::ZOMBIE)
                && zombie_ptr.is_none()
                && Task::claim_child_locked(child_ptr)
            {
                zombie_ptr = Some(child_ptr);
            } else if child.state() == TaskState::new(TaskState::STOPPED)
                && !child.stop_reported.load(core::sync::atomic::Ordering::Acquire)
                && (options & WUNTRACED != 0 || child.tracer_pid() == current_pid)
                && stopped_ptr.is_none()
            {
                // ptrace'd stops are reported regardless of WUNTRACED
                // (reference wait_task_stopped); see do_wait for the rationale.
                stopped_ptr = Some(child_ptr);
            }
        });

        if let Some(child_ptr) = zombie_ptr {
            // SAFETY: selected/claimed under the tree lock; still linked.
            let child = crate::process::unsafe_edges::borrow_task(child_ptr);
            let child_pid = child.pid();
            // W4SPIN (69th, rux:pid1spin (retired debug=pid1spin)): init's SIGCHLD handler loops
            // `while (waitpid(-1, &st, WNOHANG) > 0);` — a positive return
            // stream that never ends wedges pid 1 at 100% CPU and kills
            // the param service (the 105-family root). Once per second,
            // name the reaped child AND dump init's remaining children
            // census: the same pid repeating (claim/release bug) vs an
            // endless zombie stream (exit-path bug) is a one-look call.
            if current_pid == 1
                && crate::trace::EVENT_PID1SPIN.armed()
            {
                use core::sync::atomic::{AtomicU64, Ordering as O2};
                static LAST_J: AtomicU64 = AtomicU64::new(0);
                static CALLS: AtomicU64 = AtomicU64::new(0);
                let calls = CALLS.fetch_add(1, O2::Relaxed);
                let j = crate::timer::get_jiffies() as u64;
                let last = LAST_J.load(O2::Relaxed);
                if j.saturating_sub(last) >= 100
                    && LAST_J
                        .compare_exchange(last, j, O2::AcqRel, O2::Relaxed)
                        .is_ok()
                {
                    let mut line: alloc::string::String = alloc::format!(
                        "W4SPIN reaped pid={} tgid={} calls={} children:",
                        child_pid,
                        child.tgid(),
                        calls
                    );
                    // Read-only census walk of the caller's own group
                    // children under the tree lock (the walk in this
                    // function just ran under the same lock).
                    {
                        cur.for_each_group_child(|cp| {
                            let c = crate::process::unsafe_edges::borrow_task(cp);
                            let st = c.state().bits();
                            if line.len() < 700 {
                                line.push_str(&alloc::format!(
                                    " [{}:{}:{:#x}{}]",
                                    c.pid(),
                                    c.tgid(),
                                    st,
                                    if c.pid() == c.tgid() { "" } else { "!thr" }
                                ));
                            }
                        });
                    }
                    crate::pr_warn!("{}", line);
                }
            }
            let raw_exit = child.exit_code();

            let status: i32 = if raw_exit >= 0 {
                // Pre-shifted at sys_exit (Linux ABI) — pass through.
                (raw_exit as u32 & 0xFFFF) as i32
            } else {
                let mut s = (-(raw_exit as i32) as u32 & 0x7F) as i32;
                if child.core_dumped() {
                    s |= 0x80; // WCOREDUMP
                }
                s
            };

            if !status_ptr.is_null() {
                if crate::arch::uaccess_safe::copy_out_t(status_ptr as usize, &status) != 0 {
                    // EFAULT: the caller passed an unwritable status
                    // pointer. The zombie is already CLAIMED (unlinked),
                    // so it cannot be left for a later wait — release it
                    // here and still report the fault (divergence from
                    // the reference's unreaped-EFAULT, forced by the
                    // claim's unlinked-implies-reaped invariant).
                    release_task(child_ptr);
                    return Err(crate::errno::Errno::BadAddress.as_neg_i32());
                }
            }

            release_task(child_ptr);
            return Ok(child_pid);
        }

        if let Some(child_ptr) = stopped_ptr {
            // SAFETY: selected/claimed under the tree lock; still linked.
            let child = crate::process::unsafe_edges::borrow_task(child_ptr);
            let child_pid = child.pid();
            let stop_sig = child.stop_signal();
            let status: i32 = (((stop_sig as u32) << 8) | 0x7F) as i32;
            if !status_ptr.is_null() {
                if crate::arch::uaccess_safe::copy_out_t(status_ptr as usize, &status) != 0 {
                    return Err(crate::errno::Errno::BadAddress.as_neg_i32());
                }
            }
            child.stop_reported.store(true, core::sync::atomic::Ordering::Release);
            return Ok(child_pid);
        }

        if found_child {
            Err(errno::Errno::TryAgain.as_neg_i32())
        } else {
            Err(errno::Errno::NoChild.as_neg_i32())
        }
}

/// waitid options (match Linux wait.h)
const WNOHANG: i32     = 0x00000001;
const WUNTRACED: i32   = 0x00000002;
const WSTOPPED: i32    = WUNTRACED;
const WEXITED: i32     = 0x00000004;
const WCONTINUED: i32  = 0x00000008;
const WNOWAIT: i32     = 0x01000000;

/// waitid idtype
const P_ALL: i32  = 0;
const P_PID: i32  = 1;
const P_PGID: i32 = 2;

/// CLD_* si_code values for siginfo_t
const CLD_EXITED: i32    = 1;
const CLD_KILLED: i32    = 2;
const CLD_DUMPED: i32    = 3;
const CLD_STOPPED: i32   = 5;
const CLD_CONTINUED: i32 = 6;

/// Write waitid siginfo_t fields to user memory.
///
/// Only writes the fields used by waitid (si_signo, si_errno, si_code,
/// si_pid, si_uid, si_status), leaving the rest of the 128-byte
/// siginfo_t untouched.
// SAFETY: Caller must ensure infop points to a valid, writable user buffer of at
// least 128 bytes. copy_to_user handles user-space access safely.
fn write_siginfo(
    infop: *mut u8,
    si_code: i32,
    si_pid: u32,
    si_uid: u32,
    si_status: i32,
) {
    // Every field goes through the typed exception-table family.
    use crate::arch::uaccess_safe as ua;
    let base = infop as usize;

    let _ = ua::write_user::<i32>(base, 17); // SIGCHLD
    let _ = ua::write_user::<i32>(base + 4, 0);
    let _ = ua::write_user::<i32>(base + 8, si_code);
    // offset 12: padding
    let _ = ua::write_user::<u32>(base + 16, si_pid);
    let _ = ua::write_user::<u32>(base + 20, si_uid);
    let _ = ua::write_user::<i32>(base + 24, si_status);
}

/// Blocking waitid: wait for child process state change
///
/// # Arguments
/// * `idtype` - P_ALL (0), P_PID (1), or P_PGID (2)
/// * `id` - PID or PGID to wait for (ignored if P_ALL)
/// * `infop` - User pointer to siginfo_t
/// * `options` - WNOHANG | WEXITED | WSTOPPED | WCONTINUED | WNOWAIT
///
/// # Returns
/// * `Ok(true)` - Event found (child info written to infop)
/// * `Ok(false)` - WNOHANG and no event yet: return 0 to userspace and
///   leave infop UNTOUCHED (Linux/POSIX behavior — the old -EAGAIN broke
///   every WNOHANG poll loop).
/// * `Err(-ECHILD)` - No children
/// * `Err(-EINTR)` - Interrupted by signal
pub fn do_waitid(
    idtype: i32,
    id: i32,
    infop: *mut u8,
    options: i32,
) -> Result<bool, i32> {

    // Must specify at least one of WEXITED/WSTOPPED/WCONTINUED
    if options & (WEXITED | WSTOPPED | WCONTINUED) == 0 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // current is the calling task, valid throughout the wait loop; one
    // consolidated reference (raw uses re-derive the pointer).
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return Err(errno::Errno::NoChild.as_neg_i32()),
    };
    let current = cur as *mut Task;

    if cur.pid() == 0 {
        return Err(errno::Errno::NoChild.as_neg_i32());
    }

        loop {
            let mut found_child = false;
            let mut result_child: Option<*mut Task> = None;
            let mut result_code: i32 = 0;
            let mut result_kind: i32 = 0; // 0=zombie, 1=stopped, 2=continued

            // Group-wide scan (same as do_wait): any thread of the
            // process may waitid for any member's child.
            cur.for_each_group_child(|child_ptr| {
                // SAFETY: the walker hands out tree-locked children.
                let child = crate::process::unsafe_edges::borrow_task(child_ptr);

                // idtype filter
                if idtype == P_PID && child.pid() != id as u32 {
                    return;
                }
                if idtype == P_PGID && child.pgid() != id as u32 {
                    return;
                }
                // P_ALL: match any child

                found_child = true;

                // First event wins (the reference's wait_consider_task
                // stops the walk at the first child that yields an
                // event) — the is_none() guards keep a later match from
                // overwriting an earlier selection, which would strand
                // a CLAIMED zombie below.
                // Check for zombie (exited) — only if WEXITED
                if options & WEXITED != 0
                    && child.state() == TaskState::new(TaskState::ZOMBIE)
                {
                    // E8-REPAR discipline (see do_wait): CLAIM the
                    // zombie inside the walk's tree-lock critical
                    // section when this call will reap it — two group
                    // members blocked in waitid could otherwise both
                    // select it and double-release_task (free the Task
                    // twice). WNOWAIT never reaps, so it must NOT claim
                    // (leaves the zombie reapable by a later wait).
                    if result_child.is_none()
                        && (options & WNOWAIT != 0
                            || Task::claim_child_locked(child_ptr))
                    {
                        result_child = Some(child_ptr);
                        let raw_exit = child.exit_code();
                        let mut was_killed = false;
                        if raw_exit >= 0 {
                            // si_status is the raw 0-255 exit code
                            // (WEXITSTATUS semantics); sys_exit
                            // pre-shifted it into bits 8-15, shift back
                            // down here.
                            result_code = ((raw_exit as u32) >> 8) as i32;
                        } else {
                            result_code = (-raw_exit) as i32;
                            was_killed = true;
                        }
                        result_kind = if was_killed { 3 } else { 0 };
                    }
                }
                // Check for stopped — only if WSTOPPED. Unified with
                // wait4's one-shot semantics: stop_reported, not a
                // separate stop_signal()!=0 test (the two mechanisms
                // racing double-reported one stop event — review 3.3).
                else if options & WSTOPPED != 0
                    && result_child.is_none()
                    && child.state() == TaskState::new(TaskState::STOPPED)
                {
                    if !child.stop_reported.load(core::sync::atomic::Ordering::Acquire) {
                        result_child = Some(child_ptr);
                        result_code = child.stop_signal();
                        result_kind = 1; // stopped
                    }
                }
                // Check for continued — only if WCONTINUED (reference
                // wait_task_continued). SIGCONT's default action marks
                // the resume on the child: stop_signal sentinel
                // -SIGCONT with stop_reported re-armed. Report once.
                else if options & WCONTINUED != 0 && result_child.is_none() {
                    if !child.stop_reported.load(core::sync::atomic::Ordering::Acquire)
                        && child.stop_signal() == -(crate::signal::Signal::SIGCONT as i32)
                    {
                        result_child = Some(child_ptr);
                        result_code = crate::signal::Signal::SIGCONT as i32;
                        result_kind = 2; // continued
                    }
                }
            });

            if let Some(child_ptr) = result_child {
                // SAFETY: selected (and zombie cases claimed) under the
                // tree lock; this caller owns the report.
                let child = crate::process::unsafe_edges::borrow_task(child_ptr);
                let child_pid = child.pid();
                let child_uid = child.cred().uid;

                // Encode si_code and si_status
                let (si_code, si_status) = if result_kind == 1 {
                    // Stopped
                    (CLD_STOPPED, result_code)
                } else if result_kind == 2 {
                    // Continued: si_status is SIGCONT (reference
                    // do_notify_parent_cldstop's CLD_CONTINUED payload)
                    (CLD_CONTINUED, result_code)
                } else if result_kind == 3 {
                    // Killed by signal (CLD_DUMPED when a core was written)
                    if child.core_dumped() {
                        (CLD_DUMPED, result_code)
                    } else {
                        (CLD_KILLED, result_code)
                    }
                } else {
                    // Normal exit
                    (CLD_EXITED, result_code)
                };

                // Write siginfo to user. si_pid is reported in the
                // CALLER's pid namespace (U1c parity with wait4's
                // report_wait_pid); the child is still hash-resident at
                // this point — release_task runs after the write.
                let report_pid = crate::process::ns::wait_vpid(child_pid);
                write_siginfo(infop, si_code, report_pid, child_uid, si_status);

                // Reap zombie unless WNOWAIT
                if (result_kind == 0 || result_kind == 3) && options & WNOWAIT == 0 {
                    release_task(child_ptr);
                }

                // Mark the stop/continue event reported (one-shot,
                // wait4-consistent)
                if (result_kind == 1 || result_kind == 2) && options & WNOWAIT == 0 {
                    child.stop_reported.store(true, core::sync::atomic::Ordering::Release);
                }

                return Ok(true);
            }

            // No matching child found
            if found_child {
                // Children exist but none in target state
                if options & WNOHANG != 0 {
                    return Ok(false);
                }

                // Atomically add to waitqueue AND set INTERRUPTIBLE.
                cur.wait_chldexit.prepare_to_wait(current, false, true);

                // R9-10: re-check for a zombie AFTER registration (do_wait
                // has this; the child's deferred notify may have fired
                // while we were still RUNNING, consuming the only wake).
                // R20-4: the recheck MUST apply the same idtype/id filter
                // as the main scan — without it, a zombie sibling that the
                // filter excludes (e.g. waitid(P_PID, X) while Y is zombie)
                // keeps setting found_zombie, and the loop takes the
                // `continue` path forever without ever sleeping: a
                // kernel-space busy hang.
                {
                    let mut found_zombie = false;
                    cur.for_each_group_child(|child_ptr| {
                        // SAFETY: tree-locked child from the walker.
                        let c = crate::process::unsafe_edges::borrow_task(child_ptr);
                        if idtype == P_PID && c.pid() != id as u32 {
                            return;
                        }
                        if idtype == P_PGID && c.pgid() != id as u32 {
                            return;
                        }
                        if c.state() == TaskState::new(TaskState::ZOMBIE) {
                            found_zombie = true;
                        }
                    });
                    if found_zombie {
                        cur.wait_chldexit.finish_wait(current);
                        crate::sched::dequeue_if_enqueued(cur);
                        continue;
                    }
                }

                if crate::signal::signal_pending() {
                    cur.wait_chldexit.finish_wait(current);
                    // R8-5 (NEW-C2): undo the signal's concurrent enqueue.
                    crate::sched::dequeue_task(cur);
                    // ERESTARTSYS, not EINTR: the signal-delivery path
                    // restarts the wait when the handler is SA_RESTART.
                    return Err(ERESTARTSYS);
                }

                // Enable interrupts before schedule() — we're in syscall
                // context (SIE=0). Without this, timer IRQ can't fire and
                // the task can never be rescheduled.
                crate::arch::cpu::restore_irq(true);

                crate::sched::schedule();

                // After wakeup: state is RUNNING (set by wake_up_process).
                cur.wait_chldexit.finish_wait(current);
            } else {
                return Err(errno::Errno::NoChild.as_neg_i32());
            }
        }
}
