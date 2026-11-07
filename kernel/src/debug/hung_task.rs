//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Hung Task Detector
//!
//! Detects tasks stuck in `TASK_UNINTERRUPTIBLE` state for too long.
//! A kernel thread (`khungtaskd`) periodically scans all tasks and reports
//! any that have been in D-state with unchanged context switch count
//! for longer than the threshold.
//!
//! debug-dlsleep upgrade: the report now (a) prints the task's real comm,
//! (b) symbolizes the parked stack INLINE via the runtime ksym table
//! (raw hex kept alongside — belt and braces), and (c) walks the
//! waiter→lock→owner chain: if the victim is parked on a SleepMutex
//! (Task::blocked_on, set only in the contended slow path — see
//! sync/mutex.rs and the zero-impact constraint in run/LANE.md), the
//! report names the mutex, follows its owner, prints the owner's
//! comm/pid/state (with its CPU when RUNNING — directing the cross-CPU
//! peek) and the owner's own parked stack, recursing up to depth 3.
//! ABBA cycles terminate with a "chain cycles at pid N" line.
//! `debug=hungpanic` escalates a report to a full panic (reference
//! hung_task_panic analog, default off) — the panic handler's last-gasp
//! bundle (debug/taskdump.rs) then photographs the whole system.
//!
//! All of the above is COLD report path only: khungtaskd's 60 s scan
//! cadence and the mutex acquire/release fast paths are untouched.

use core::sync::atomic::{AtomicBool, Ordering};
use crate::debug::taint;
use crate::debug::backtrace;
use crate::debug::backtrace::ConsoleWriter;
use core::fmt::Write;

/// Hung task timeout in seconds (default: 120s)
const HUNG_TASK_TIMEOUT_SECS: u64 = 120;

/// Check interval (timeout / 2)
const HUNG_TASK_CHECK_INTERVAL_SECS: u64 = HUNG_TASK_TIMEOUT_SECS / 2;

/// Whether khungtaskd is running
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Get nanosecond timestamp from RISC-V `rdtime`.
fn now_ns() -> u64 {
    let time: u64 = crate::arch::cpu::read_time();
    // timebase ticks → ns (×100 for the 10 MHz CLINT; the old ×10
    // multiplier read every window 10x too small — reviewround 8).
    time.saturating_mul(1_000_000_000) / crate::config::TIMER_CLOCK_FREQ_HZ as u64
}

/// Initialize the hung task detector.
///
/// Starts the `khungtaskd` kernel thread.
pub fn init() {
    if RUNNING.load(Ordering::Acquire) {
        return;
    }

    // Start khungtaskd kernel thread
    let result = crate::process::kthread::kthread_run(
        khungtaskd_fn,
        core::ptr::null_mut(),
        "khungtaskd",
    );

    if result.is_some() {
        RUNNING.store(true, Ordering::Release);
        } else {
        crate::pr_warn!("debug: failed to start khungtaskd");
    }

    // debug=abba: synthetic ABBA deadlock self-test — two kthreads
    // cross-acquire two static SleepMutexes, the orchestrator runs the
    // FULL report path on one victim once both are D-parked, asserts the
    // chain convicted both tasks + both mutexes + the cycle, then
    // unwinds the deadlock (release_if_owner, the do_exit analog).
    // Runs INLINE from the boot task (gated switch, zero default cost):
    // its yield_cpu() pumps drive the two kthreads to their park sites
    // deterministically before boot continues — a kthread orchestrator
    // would never be scheduled on a boot whose init fails to load (the
    // post-init halt() path stops scheduling).
    if crate::debug::switches::enabled(crate::debug::switches::DebugSwitch::AbbaTest) {
        abba_orchestrator_fn(core::ptr::null_mut());
    }
}

/// khungtaskd main loop.
///
/// Wakes every `HUNG_TASK_CHECK_INTERVAL_SECS` via a one-shot kernel timer
/// and scans all tasks (pid-hash traversal — never the pid-slot array).
extern "C" fn khungtaskd_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::task::TaskState;

    // Check interval in jiffies (timeout/2, at least one tick).
    let interval_jiffies = (HUNG_TASK_CHECK_INTERVAL_SECS
        * crate::config::KERNEL_HZ as u64)
        .max(1);

    loop {
        // Check for stop request
        if crate::process::kthread::kthread_should_stop() {
            break;
        }

        // --- Periodic sleep with a real waker (reviewround 8: the old loop
        // set INTERRUPTIBLE and scheduled with NO waker armed — khungtaskd
        // slept until some unrelated wake happened to land on it, so hung
        // tasks were detected minutes late or never). Mirror the
        // nanosleep_impl state-first discipline: arm a one-shot timer for
        // our pid, mark INTERRUPTIBLE, re-check, then sleep.
        let current = match crate::sched::current() {
            Some(t) => t as *mut crate::process::task::Task,
            None => break,
        };
        // SAFETY: current is this kthread's own task pointer.
        let my_pid = unsafe { (*current).pid() };
        let target = crate::drivers::timer::get_jiffies() + interval_jiffies;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);

        if timer_id != 0 {
            // SAFETY: current is the running task's pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
            }
            // Re-check AFTER marking sleeping: the one-shot timer may have
            // fired between arming and set_state — either its wake captured
            // the INTERRUPTIBLE state, or the deadline is already visible.
            if crate::drivers::timer::get_jiffies() >= target {
                // SAFETY: current is the running task's pointer.
                unsafe {
                    (*current).set_state(TaskState::new(TaskState::RUNNING));
                    // NEW-C2: a racing wake may have enqueued us while we
                    // are still executing — take ourselves back off.
                    crate::sched::dequeue_task(&*current);
                }
                crate::timer::del_timer(timer_id);
            } else {
                // Enable interrupts so the tick can reach us, then sleep.
                crate::arch::cpu::restore_irq(true);
                crate::sched::schedule();
                // Spurious wake before the deadline: drop the timer and
                // loop (re-armed next iteration).
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            // Timer table full — stay runnable, yield one scheduling round
            // and retry the arm.
            crate::sched::schedule();
        }

        // Scan all tasks
        check_tasks();

        // Binder stall scan: threads parked in a blocking binder read are
        // S-state (invisible above); they mark an IPC chain that stopped
        // answering — the common OH bring-up stall shape.
        crate::ipc::binder::stall_report();

        // debug=sleepsnap: name the syscall every other S-state thread is
        // parked in (appspawn-server wedge hunt, b11) — self-gated.
        crate::ipc::binder::sleep_snapshot();

        // debug=socksnap: AF_UNIX listener queue depths + every epoll
        // interest set (appspawn-server wedge hunt, b15) — self-gated.
        crate::net::unix::unix_listener_snapshot();
        crate::syscall::misc::epoll_interest_snapshot();
    }

    RUNNING.store(false, Ordering::Release);
    0
}

/// Scan all tasks for hung tasks (pid-hash traversal; bookkeeping lives on
/// each Task — the old pid-indexed side table skipped every task whose pid
/// was >= MAX_TASKS, which on this system means essentially all of them,
/// reviewround 8).
///
/// debug-dlsleep: victims are COLLECTED under the bucket lock (pinned via
/// the task refcount, an atomic add — mirroring
/// pid_hash_lookup_pinned's body) and reported AFTER the iteration. The
/// upgraded report walks the owner chain, symbolizes stacks and may panic
/// (`debug=hungpanic`) — none of that belongs under a pid-hash bucket
/// spinlock.
fn check_tasks() {
    let now = now_ns();
    let timeout_ns = HUNG_TASK_TIMEOUT_SECS * 1_000_000_000;

    const MAX_VICTIMS: usize = 8;
    let mut victims: [(*mut crate::process::task::Task, u64); MAX_VICTIMS] =
        [(core::ptr::null_mut(), 0); MAX_VICTIMS];
    let mut nvictims = 0usize;
    let mut overflow = 0usize;

    crate::sched::for_each_task(|task| {
        // SAFETY: callback runs under the pid-hash bucket lock; reads and
        // atomic stores on the task's own hung_task_since field only, plus
        // the refcount pin when a victim is latched (the Task cannot be
        // freed under us: it is in the hash we hold, and the pin holds it
        // after we drop the lock).
        unsafe {
            let task_ref = &*task;
            let state = task_ref.state();
            let pid = task_ref.pid();

            // Only check uninterruptible tasks
            if !state.contains(crate::process::task::TaskState::UNINTERRUPTIBLE) {
                // Not in D-state: reset tracking
                task_ref.hung_task_since.store(0, Ordering::Release);
                return;
            }

            // Skip the idle task (pid 0).
            if pid == 0 {
                return;
            }

            let last_time = task_ref.hung_task_since.load(Ordering::Acquire);

            if last_time == 0 {
                // First time seeing this task in D-state
                task_ref.hung_task_since.store(now, Ordering::Release);
            } else {
                let elapsed = now.saturating_sub(last_time);
                if elapsed > timeout_ns {
                    if nvictims < MAX_VICTIMS {
                        // Pin: keep the Task allocated past the bucket lock.
                        task_ref.task_refcnt.fetch_add(1, Ordering::AcqRel);
                        victims[nvictims] = (task, elapsed / 1_000_000_000);
                        nvictims += 1;
                    } else {
                        overflow += 1;
                    }
                    // Reset to avoid repeated reports
                    task_ref.hung_task_since.store(now, Ordering::Release);
                }
            }
        }
    });

    for i in 0..nvictims {
        let (t, elapsed) = victims[i];
        report_hung_task(t, elapsed);
        // SAFETY: pinned above with fetch_add; this drops our reference.
        unsafe { crate::process::task::Task::task_put(t); }
    }
    if overflow > 0 {
        crate::pr_warn!(
            "hung_task: {} additional D-state victims skipped (cap {})",
            overflow,
            MAX_VICTIMS
        );
    }
}

// ---------------------------------------------------------------------------
// Report-time helpers (COLD paths — khungtaskd report / self-test only)
// ---------------------------------------------------------------------------

/// Kernel-text window for the parked-stack return-address filter. The two
/// arch linker scripts spell the bounds differently (x86_64:
/// __stext/__etext, riscv64/aarch64: _stext/_etext).
fn kernel_text_window() -> (u64, u64) {
    #[cfg(feature = "x86_64")]
    extern "C" {
        static __stext: u8;
        static __etext: u8;
    }
    #[cfg(not(feature = "x86_64"))]
    extern "C" {
        static _stext: u8;
        static _etext: u8;
    }
    #[cfg(feature = "x86_64")]
    let lo = core::ptr::addr_of!(__stext) as u64;
    #[cfg(feature = "x86_64")]
    let hi = core::ptr::addr_of!(__etext) as u64;
    #[cfg(not(feature = "x86_64"))]
    let lo = core::ptr::addr_of!(_stext) as u64;
    #[cfg(not(feature = "x86_64"))]
    let hi = core::ptr::addr_of!(_etext) as u64;
    (lo, hi)
}

/// Print-safe comm writer: emits the comm bytes up to the first NUL,
/// replacing non-printables with '.' (a comm holds arbitrary bytes —
/// prctl PR_SET_NAME copies raw).
fn write_comm(w: &mut ConsoleWriter, t: &crate::process::task::Task) {
    let comm = t.comm();
    let name = comm.split(|&b| b == 0).next().unwrap_or(b"?");
    let _ = w.write_str("\"");
    for &b in name.iter().take(16) {
        if b >= 0x20 && b < 0x7f {
            let _ = w.write_char(b as char);
        } else {
            let _ = w.write_char('.');
        }
    }
    let _ = w.write_str("\"");
}

/// Up to `max_frames` kernel-text words from a parked task's stack,
/// scanning UP from its saved sp (R76: the caller frames of the parked
/// chain live at HIGHER addresses — stacks grow down). Every parked task
/// entered the park through `__schedule`/`context_switch`; the words
/// between sp and the `__schedule` frame are that machinery, so the scan
/// ANCHORS at the first `__schedule`/`context_switch` frame and shows
/// the frames ABOVE it — the park site itself (the mutex wait fn, the io
/// wait fn) and its callers, which is what convicts. Fallback when no
/// schedule frame is found (fresh park, odd frame layout): show from the
/// first text word. The window (512 words max) is bounded by the task's
/// kernel-stack top so the read can never leave the task's own stack
/// pages. Each frame prints RAW HEX plus the INLINE ksyms symbolization
/// (name+off/size) — belt and braces: the raw word still symbolizes
/// offline if the runtime table is unusable.
///
/// Returns the first symbolized name shown (the wchan analogue for
/// non-mutex parks) or None.
fn dump_parked_stack(
    w: &mut ConsoleWriter,
    t: &crate::process::task::Task,
    label: &str,
    max_frames: usize,
) -> Option<&'static str> {
    let thsp = t.thread().sp;
    if !((thsp >> 48) == 0xffff && (thsp & 7) == 0 && thsp > 0x1000) {
        return None;
    }
    let _ = write!(w, "      {} sp={:#x}", label, thsp);
    let (lo, hi) = kernel_text_window();
    let base = thsp as *const u64;
    // Bound the upward walk at the task's kernel-stack top (pt_regs lives
    // there; beyond it is not this task's memory). checked: a chain owner
    // may be exiting concurrently — a freed stack reports None and the
    // window stays conservative (sc261 lesson: never read a foreign
    // task's stack without owning proof).
    let mut words = 512usize;
    if let Some(top) = t.kernel_stack_top_checked() {
        let top = top as u64;
        if top > thsp && (top - thsp) / 8 < words as u64 {
            words = ((top - thsp) / 8) as usize;
        }
    }
    // Pass 1: locate the schedule-machinery anchor.
    let is_text = |v: u64| v >= lo && v < hi && (v & 1) == 0;
    let is_machinery = |name: &str| name.contains("__schedule") || name.contains("context_switch");
    let mut anchor = 0usize;
    for k in 0..words {
        // SAFETY: diagnostic read within the task's own kernel stack
        // pages (the Task exists and pins them); bounded by the top above.
        let v = unsafe { core::ptr::read_volatile(base.add(k)) };
        if is_text(v) {
            if let Some((name, _, _)) = crate::debug::ksyms::lookup(v) {
                if is_machinery(name) {
                    anchor = k + 1;
                }
            }
        }
        if k > 48 {
            // The machinery cluster lives in the first few frames; beyond
            // 48 words a machinery hit is stale data from an older, deeper
            // call on this stack — stop extending the anchor.
            break;
        }
    }
    // Pass 2: show frames from the anchor upward, skipping the residual
    // machinery frames (the anchor is after the FIRST match; the cluster
    // may span a couple more words).
    let mut shown = 0;
    let mut first_sym: Option<&'static str> = None;
    let mut k = anchor;
    while k < words {
        // SAFETY: as pass 1.
        let v = unsafe { core::ptr::read_volatile(base.add(k)) };
        if is_text(v) {
            if let Some((name, _, _)) = crate::debug::ksyms::lookup(v) {
                if is_machinery(name) {
                    k += 1;
                    continue;
                }
            }
            let _ = write!(w, " {:#x}", v);
            if let Some((name, addr, size)) = crate::debug::ksyms::lookup(v) {
                let _ = write!(w, "[{}+0x{:x}/0x{:x}]", name, v - addr, size);
                if first_sym.is_none() {
                    first_sym = Some(name);
                }
            }
            shown += 1;
            if shown >= max_frames {
                break;
            }
        }
        k += 1;
    }
    let _ = w.write_str("\n");
    first_sym
}

/// Fixed-capacity chain summary — the report path must not allocate
/// (usable under memory pressure and from the ABBA self-test's verdict).
#[derive(Clone, Copy)]
pub struct ChainSummary {
    /// Mutex edges followed (0..=3).
    pub hops: usize,
    /// pids[0] = victim, then each owner followed.
    pub pids: [u32; 4],
    /// Mutex name per hop.
    pub mutex_names: [&'static str; 3],
    /// Mutex address per hop.
    pub mutex_addrs: [u64; 3],
    /// Set when the chain closed on itself (ABBA): the pid it cycles at.
    pub cycle_at: Option<u32>,
    /// Set when an owner pid resolved to no live task.
    pub owner_missing: Option<u32>,
}

impl ChainSummary {
    fn empty(victim_pid: u32) -> Self {
        Self {
            hops: 0,
            pids: [victim_pid, 0, 0, 0],
            mutex_names: ["", "", ""],
            mutex_addrs: [0; 3],
            cycle_at: None,
            owner_missing: None,
        }
    }
}

/// Walk the waiter→lock→owner chain from a D-state victim and print each
/// hop. The victim→mutex edge comes from `Task::blocked_on` (written only
/// in the SleepMutex contended slow path); the mutex→owner edge reads the
/// mutex's EXISTING owner field at report time via a TRY-lock (a wedged
/// state-spinlock holder must not deadlock the detector). Owners are
/// pinned through the pid hash (pid_hash_lookup_pinned + task_put).
///
/// Depth-capped at 3 hops; a revisit of any pid in the chain prints
/// "chain cycles at pid N" (the ABBA verdict) and stops.
fn walk_owner_chain(w: &mut ConsoleWriter, victim: *mut crate::process::task::Task) -> ChainSummary {
    use crate::process::task::Task;

    let mut sum = ChainSummary::empty(unsafe { (*victim).pid() });
    let mut seen_n = 1usize;
    let mut cur: *mut Task = victim;
    let mut pinned: [*mut Task; 3] = [core::ptr::null_mut(); 3];
    let mut pinned_n = 0usize;

    while sum.hops < 3 {
        // SAFETY: cur is the victim (caller-pinned) or one of our own
        // pinned lookups; blocked_on holds 0 or a &'static SleepMutex
        // address, so the deref below cannot dangle.
        let mptr = unsafe { (*cur).blocked_on.load(Ordering::Acquire) };
        if mptr == 0 {
            // Not parked on a SleepMutex (io wait, down() semaphore, ...):
            // the symbolized parked stack above already names the wait fn.
            break;
        }
        let m = mptr as *const crate::sync::mutex::SleepMutex;
        let hop = sum.hops;
        // SAFETY: m is a &'static SleepMutex address (blocked_on's only
        // legal values); diag_snapshot try-locks, never blocks.
        let (name, (locked, owner, depth, waiters)) = unsafe { ((*m).diag_name(), (*m).diag_snapshot()) };
        let _ = locked;
        let cur_pid = unsafe { (*cur).pid() };
        let _ = write!(
            w,
            "      CHAIN[{}] pid={} waits on {} @ {:#x} (owner={} depth={} waiters={})\n",
            hop, cur_pid, name, mptr, owner, depth, waiters
        );
        sum.mutex_names[hop] = name;
        sum.mutex_addrs[hop] = mptr;
        sum.hops += 1;

        if owner == 0 {
            let _ = write!(w, "      CHAIN[{}] lock FREE with waiters — stranded handoff (dropped wake / stuck count)\n", hop);
            break;
        }
        if owner == u32::MAX {
            let _ = write!(w, "      CHAIN[{}] mutex state lock busy — owner unknown at snapshot\n", hop);
            break;
        }
        // Cycle check against everyone followed so far (victim included).
        let mut cycles = false;
        for i in 0..seen_n {
            if sum.pids[i] == owner {
                cycles = true;
                break;
            }
        }
        if cycles {
            let _ = write!(
                w,
                "      CHAIN cycles at pid {} — ABBA deadlock (holder chain closes on itself)\n",
                owner
            );
            sum.cycle_at = Some(owner);
            break;
        }

        let ot = crate::process::pid_hash::pid_hash_lookup_pinned(owner);
        if ot.is_null() {
            let _ = write!(w, "      CHAIN[{}] owner pid {} not in pid-hash (exiting/leaked?)\n", hop, owner);
            sum.owner_missing = Some(owner);
            break;
        }
        pinned[pinned_n] = ot;
        pinned_n += 1;
        // SAFETY: pinned lookup — Task stays allocated until task_put.
        let o: &Task = unsafe { &*ot };
        let state = o.state();
        let on_cpu = o.on_cpu();
        let _ = write!(w, "      CHAIN[{}] owner ", hop);
        write_comm(w, o);
        let _ = write!(
            w,
            ":{} state={:#x} on_cpu={} cpu={} claim={}\n",
            owner,
            state.bits(),
            on_cpu as u8,
            o.ti_cpu(),
            o.running_on_cpu.load(Ordering::Acquire),
        );
        if state.contains(crate::process::task::TaskState::RUNNING) || on_cpu {
            let _ = write!(
                w,
                "      CHAIN[{}] owner RUNNING/on-CPU (cpu {}) — its stack is LIVE; peek cross-CPU (taskdump/sysrq)\n",
                hop,
                o.ti_cpu()
            );
        } else {
            // Owner parked too: its own parked stack, symbolized.
            dump_parked_stack(w, o, "CHAIN-OWN-STK", 12);
        }
        sum.pids[seen_n] = owner;
        seen_n += 1;
        cur = ot;
    }

    for i in 0..pinned_n {
        // SAFETY: our own pinned lookups; drops the references we took.
        unsafe { Task::task_put(pinned[i]); };
    }
    sum
}

/// Report a hung task. Returns the chain summary (used by the debug=abba
/// self-test verdict; callers that only print ignore it).
fn report_hung_task(task: *mut crate::process::task::Task, elapsed_secs: u64) -> ChainSummary {
    let mut w = ConsoleWriter::new();
    let task_ref = unsafe { &*task };

    // Reference shape: "INFO: task <comm>:<pid> blocked for more than N
    // seconds." (comm is real — Task::comm has existed all along; the old
    // "No comm name yet" placeholder printed pid twice).
    let _ = write!(w, "INFO: task ");
    write_comm(&mut w, task_ref);
    let _ = write!(w, ":{} blocked for more than {} seconds\n", task_ref.pid(), elapsed_secs);

    // R76 (fast-pace wedge hunt): the old report printed only CURRENT's
    // backtrace — the khungtaskd scanner itself — which says nothing about
    // where the D-state task is parked or which wake it lost. Dump the
    // victim's scheduler-side context (ownership word, cpu, queue flag,
    // vfork linkage) and the symbolized kernel-text words from its parked
    // stack (see dump_parked_stack for the machinery anchor + cap).
    {
        use crate::process::task::Task;
        let t: &Task = task_ref;
        let claim = t.running_on_cpu.load(Ordering::Acquire);
        let vfork_ppid = t.vfork_parent_ptr().map(|p| unsafe { (*p).pid() }).unwrap_or(0);
        let _ = write!(
            w,
            "      HUNG-CTX pid={} state={:#x} oncpu={} claim={} pk={} ti_cpu={} onrq={} vfork_ppid={}\n",
            t.pid(),
            t.state().bits(),
            t.on_cpu() as u8,
            claim,
            t.pick_state(),
            t.ti_cpu(),
            t.sched_entity().is_on_rq() as u8,
            vfork_ppid,
        );
        // debug-dlsleep: the parked stack, symbolized INLINE (name+off
        // beside every raw word) — the log alone now names the wait site.
        let first_sym = dump_parked_stack(&mut w, t, "HUNG-STK", 16);

        // The waiter→lock→owner chain. Non-mutex parks (io wait etc.)
        // fall straight through: the first symbolized frame above is the
        // wchan line — it names the wait function.
        let sum = walk_owner_chain(&mut w, task);
        if sum.hops == 0 {
            match first_sym {
                Some(sym) => {
                    let _ = write!(w, "      wchan: {} (not parked on a SleepMutex)\n", sym);
                }
                None => {
                    let _ = write!(w, "      wchan: ? (no kernel-text frames on the parked stack)\n");
                }
            }
        }

        let taint_str = taint::taint_string_arr();
        let taint_display = unsafe { core::str::from_utf8_unchecked(&taint_str) };
        let _ = write!(w, "      Tainted: {}\n", taint_display);

        // Stack trace (khungtaskd's own — kept for log-shape continuity).
        drop(w);
        backtrace::dump_stack();

        // Taint kernel
        taint::add_taint(taint::TaintFlags::DIE);

        // debug=hungpanic (reference hung_task_panic analog, default
        // off): escalate the report to a full panic so the boot stops
        // with the report + the panic last-gasp bundle photographing the
        // whole system. Formatting is numeric only — the panic path
        // itself stays allocation-free.
        if crate::debug::switches::enabled(crate::debug::switches::DebugSwitch::HungPanic) {
            panic!(
                "hung_task: pid {} blocked for more than {} seconds",
                t.pid(),
                elapsed_secs
            );
        }
        sum
    }
}

// ---------------------------------------------------------------------------
// debug=abba: synthetic ABBA deadlock self-test
// ---------------------------------------------------------------------------

/// The two crossed mutexes. Static (SleepMutex::new is const), so the
/// blocked_on values and every report-time deref are 'static-safe.
static ABBA_A: crate::sync::mutex::SleepMutex =
    crate::sync::mutex::SleepMutex::new("DLSELP-ABBA-A", false);
static ABBA_B: crate::sync::mutex::SleepMutex =
    crate::sync::mutex::SleepMutex::new("DLSELP-ABBA-B", false);

static ABBA_A_HELD: AtomicBool = AtomicBool::new(false);
static ABBA_B_HELD: AtomicBool = AtomicBool::new(false);

/// T1: take A, signal, wait for T2's signal, then block on B (held by T2,
/// which is blocking on A) — parked forever in D-state.
extern "C" fn abba_t1_fn(_arg: *mut core::ffi::c_void) -> i32 {
    let _ga = ABBA_A.lock();
    ABBA_A_HELD.store(true, Ordering::Release);
    while !ABBA_B_HELD.load(Ordering::Acquire) {
        crate::sched::yield_cpu();
    }
    let _gb = ABBA_B.lock();
    0
}

/// T2: the mirror image — take B, signal, wait for T1's signal, block on A.
extern "C" fn abba_t2_fn(_arg: *mut core::ffi::c_void) -> i32 {
    let _gb = ABBA_B.lock();
    ABBA_B_HELD.store(true, Ordering::Release);
    while !ABBA_A_HELD.load(Ordering::Acquire) {
        crate::sched::yield_cpu();
    }
    let _ga = ABBA_A.lock();
    0
}

/// Is `pid` currently parked in D-state on a mutex? (pinned lookup — the
/// Task cannot be freed under the read).
fn abba_parked_on(pid: u32) -> bool {
    let t = crate::process::pid_hash::pid_hash_lookup_pinned(pid);
    if t.is_null() {
        return false;
    }
    // SAFETY: pinned lookup; reads stable diagnostic fields.
    let parked = unsafe {
        (*t).state().contains(crate::process::task::TaskState::UNINTERRUPTIBLE)
            && (*t).blocked_on.load(Ordering::Acquire) != 0
    };
    // SAFETY: drops the pin we took.
    unsafe { crate::process::task::Task::task_put(t); }
    parked
}

extern "C" fn abba_orchestrator_fn(_arg: *mut core::ffi::c_void) -> i32 {
    let t1 = match crate::process::kthread::kthread_run(abba_t1_fn, core::ptr::null_mut(), "abba-t1") {
        Some(t) => t,
        None => {
            crate::pr_err!("DLSELP-ABBA: FAIL to spawn t1");
            return 1;
        }
    };
    let t2 = match crate::process::kthread::kthread_run(abba_t2_fn, core::ptr::null_mut(), "abba-t2") {
        Some(t) => t,
        None => {
            crate::pr_err!("DLSELP-ABBA: FAIL to spawn t2");
            return 1;
        }
    };
    let t1_pid = t1.pid();
    let t2_pid = t2.pid();

    // Wait for the deadlock to establish: both cross-blocked in D-state.
    let deadline = crate::drivers::timer::get_jiffies()
        + 10 * crate::config::KERNEL_HZ as u64;
    loop {
        if abba_parked_on(t1_pid) && abba_parked_on(t2_pid) {
            break;
        }
        if crate::drivers::timer::get_jiffies() >= deadline {
            crate::pr_err!(
                "DLSELP-ABBA: FAIL deadlock never established (t1={} t2={})",
                abba_parked_on(t1_pid) as u8,
                abba_parked_on(t2_pid) as u8
            );
            return 1;
        }
        crate::sched::yield_cpu();
    }

    // The money shot: the FULL report path on victim t1. The chain must
    // name both mutexes, follow to t2, and cycle back at t1.
    let victim = crate::process::pid_hash::pid_hash_lookup_pinned(t1_pid);
    if victim.is_null() {
        crate::pr_err!("DLSELP-ABBA: FAIL victim vanished before report");
        return 1;
    }
    crate::pr_info!("DLSELP-ABBA: deadlock established, reporting victim pid {}", t1_pid);
    let sum = report_hung_task(victim, 9999);
    // Drops the orchestrator's pin on the victim (task_put is safe).
    crate::process::task::Task::task_put(victim);

    // Verdict from the summary alone (the log above carries the human
    // version): 2 hops, victim waits B then owner waits A, cycle at t1.
    let ok = sum.hops == 2
        && sum.cycle_at == Some(t1_pid)
        && sum.mutex_names[0] == ABBA_B.diag_name()
        && sum.mutex_names[1] == ABBA_A.diag_name()
        && sum.pids[1] == t2_pid;
    if ok {
        crate::pr_info!(
            "DLSELP-ABBA: PASS hops=2 chain {}:{:#x} -> {} owner pid {} -> cycle-at pid {}",
            ABBA_B.diag_name(), &ABBA_B as *const _ as u64, ABBA_A.diag_name(), t2_pid, t1_pid
        );
    } else {
        crate::pr_err!(
            "DLSELP-ABBA: FAIL verdict hops={} cycle_at={:?} names=[{},{},{:?}] pids={:?}",
            sum.hops,
            sum.cycle_at,
            sum.mutex_names[0],
            sum.mutex_names[1],
            sum.owner_missing,
            sum.pids
        );
    }

    // Unwind: force-release both sides (release_if_owner — the do_exit
    // analog). One side alone would resolve the chain; both is robust.
    ABBA_A.release_if_owner(t1_pid);
    ABBA_B.release_if_owner(t2_pid);

    // Confirm the system unwound: both threads leave D and exit.
    let deadline = crate::drivers::timer::get_jiffies()
        + 10 * crate::config::KERNEL_HZ as u64;
    loop {
        if !abba_parked_on(t1_pid) && !abba_parked_on(t2_pid) {
            break;
        }
        if crate::drivers::timer::get_jiffies() >= deadline {
            crate::pr_err!("DLSELP-ABBA: FAIL threads did not unwind after force-release");
            return 1;
        }
        crate::sched::yield_cpu();
    }
    if ok {
        crate::pr_info!("DLSELP-ABBA: DONE (deadlock released, threads unwound)");
    }
    if ok { 0 } else { 1 }
}
