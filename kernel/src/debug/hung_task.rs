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

#![forbid(unsafe_code)]
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::debug::taint;
use crate::debug::backtrace;

/// Hung task timeout in seconds (default: 120s) — the live value behind
/// /proc/sys/kernel/hung_task_timeout_secs (0 disables the detector, like
/// the reference).
static HUNG_TASK_TIMEOUT_SECS: AtomicU64 = AtomicU64::new(120);

/// Panic instead of reporting when a hung task is detected (default: off)
/// — /proc/sys/kernel/hung_task_panic.
static HUNG_TASK_PANIC: AtomicBool = AtomicBool::new(false);

/// Live timeout (s) — sysctl read face.
pub fn timeout_secs() -> u64 {
    HUNG_TASK_TIMEOUT_SECS.load(Ordering::Relaxed)
}

/// Set the timeout (s); 0 disables detection. Called by the sysctl write.
pub fn set_timeout_secs(v: u64) {
    HUNG_TASK_TIMEOUT_SECS.store(v, Ordering::Relaxed);
}

pub fn panic_on_hung_task() -> bool {
    HUNG_TASK_PANIC.load(Ordering::Relaxed)
}

pub fn set_panic_on_hung_task(v: bool) {
    HUNG_TASK_PANIC.store(v, Ordering::Relaxed);
}

/// Whether khungtaskd is running
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Get nanosecond timestamp from RISC-V `rdtime`.
fn now_ns() -> u64 {
    let time: u64 = crate::arch::cpu::read_time();
    // timebase ticks → ns (×100 for the 10 MHz CLINT; the old ×10
    // multiplier read every window 10x too small — review批次8).
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
        crate::pr_warn!("dfx: failed to start khungtaskd");
    }
}

/// khungtaskd main loop.
///
/// Wakes every `HUNG_TASK_CHECK_INTERVAL_SECS` via a one-shot kernel timer
/// and scans all tasks (pid-hash traversal — never the pid-slot array).
extern "C" fn khungtaskd_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::task::TaskState;

    loop {
        // Check for stop request
        if crate::process::kthread::kthread_should_stop() {
            break;
        }

        // Check interval in jiffies — timeout/2 (Linux check_hung_task
        // cadence), re-read every round so a live sysctl write of
        // hung_task_timeout_secs takes effect without restarting the
        // thread. Saturating: a huge sysctl value must not wrap.
        let interval_jiffies = (timeout_secs() / 2)
            .saturating_mul(crate::config::KERNEL_HZ as u64)
            .max(1);

        // --- Periodic sleep with a real waker (review批次8: the old loop
        // set INTERRUPTIBLE and scheduled with NO waker armed — khungtaskd
        // slept until some unrelated wake happened to land on it, so hung
        // tasks were detected minutes late or never). Mirror the
        // nanosleep_impl state-first discipline: arm a one-shot timer for
        // our pid, mark INTERRUPTIBLE, re-check, then sleep.
        let my_pid = match crate::sched::current() {
            Some(t) => t.pid(),
            None => break,
        };
        let target = crate::drivers::timer::get_jiffies() + interval_jiffies;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);

        if timer_id != 0 {
            // Re-derived running-task borrows (never held across
            // schedule() below — the discipline that replaces the raw
            // pointer this loop carried).
            crate::sched::current().map(|t| t.set_state(TaskState::new(TaskState::INTERRUPTIBLE)));
            // Re-check AFTER marking sleeping: the one-shot timer may have
            // fired between arming and set_state — either its wake captured
            // the INTERRUPTIBLE state, or the deadline is already visible.
            if crate::drivers::timer::get_jiffies() >= target {
                if let Some(t) = crate::sched::current() {
                    t.set_state(TaskState::new(TaskState::RUNNING));
                    // NEW-C2: a racing wake may have enqueued us while we
                    // are still executing — take ourselves back off.
                    crate::sched::dequeue_task(t);
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

        // rux:sleepsnap (retired dfx=sleepsnap): name the syscall every other S-state thread is
        // parked in (appspawn-server wedge hunt, b11) — self-gated.
        crate::ipc::binder::sleep_snapshot();

        // rux:socksnap (retired dfx=socksnap): AF_UNIX listener queue depths + every epoll
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
/// review批次8).
fn check_tasks() {
    // timeout_secs == 0 disables the detector (reference semantic of
    // /proc/sys/kernel/hung_task_timeout_secs = 0).
    let timeout = timeout_secs();
    if timeout == 0 {
        return;
    }
    let now = now_ns();
    let timeout_ns = timeout.saturating_mul(1_000_000_000);

    crate::sched::for_each_task(|task| {
        // The walk hands &Task under the bucket lock; reads and atomic
        // stores on the task's own hung_task_since field only.
        {
            let task_ref = task;
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
                    // Hung task detected!
                    report_hung_task(task, elapsed / 1_000_000_000);

                    // kernel.hung_task_panic=1: die instead of waiting for
                    // more reports (reference check_hung_task panic path).
                    if panic_on_hung_task() {
                        panic!(
                            "hung_task: task {} blocked for more than {} seconds",
                            task_ref.pid(),
                            elapsed / 1_000_000_000
                        );
                    }

                    // Reset to avoid repeated reports
                    task_ref.hung_task_since.store(now, Ordering::Release);
                }
            }
        }
    });
}

/// Report a hung task.
///
/// Emitted through the printk ring (KERN_ERR), NOT the raw-uart
/// ConsoleWriter: the old report bypassed the ring entirely, so `dmesg`
/// never saw the splat and it was lost after reboot (2026-11-28 drill b3).
/// KERN_ERR lands in the ring at any console_loglevel and matches the
/// reference's hung-task class. The raw-uart path stays reserved for
/// true-panic contexts (trap/panic printer, oops).
fn report_hung_task(task_ref: &crate::process::task::Task, elapsed_secs: u64) {
    // rwlock-livelock: comm was a pid placeholder — the ub2404 wedge left
    // an anonymous D-state task (312) whose identity decided the whole
    // anatomy. comm() exists (the W2 refusal print uses it).
    let comm_b = task_ref.comm();
    let comm_end = comm_b.iter().position(|&b| b == 0).unwrap_or(16);
    let comm = core::str::from_utf8(&comm_b[..comm_end]).unwrap_or("?");
    crate::pr_err!(
        "INFO: task {}:{} blocked for more than {} seconds",
        task_ref.pid(),
        comm,
        elapsed_secs
    );

    // R76 (fast-pace wedge hunt): the old report printed only CURRENT's
    // backtrace — the khungtaskd scanner itself — which says nothing about
    // where the D-state task is parked or which wake it lost. Dump the
    // victim's scheduler-side context (ownership word, cpu, queue flag,
    // vfork linkage) and up to 8 kernel-text words from its parked stack;
    // symbolized offline these name the wait site directly.
    {
        use crate::process::task::Task;
        let t: &Task = task_ref;
        let claim = t.running_on_cpu.load(Ordering::Acquire);
        // INV-FOR-1 adopt edge: the vfork parent tree handle under the
        // caller's softirq context — TaskBorrow's short-read contract.
        let vfork_ppid = t
            .vfork_parent_ptr()
            .and_then(|p| crate::debug::forensics_edges::task_borrow(p as *mut crate::process::task::Task))
            .map(|p| p.pid())
            .unwrap_or(0);
        // Built into a String: the HUNG-CTX record carries the optional
        // user-entry frame tail (unrip/unrsys) after the fixed fields, and
        // printk takes one format pass per record.
        let mut line = alloc::string::String::new();
        {
            use core::fmt::Write as _;
            let _ = write!(
                line,
                "      HUNG-CTX pid={} state={:#x} oncpu={} claim={} pk={} ti_cpu={} onrq={} vfork_ppid={} lastsys={} a0={:#x}",
                t.pid(),
                t.state().bits(),
                t.on_cpu() as u8,
                claim,
                t.pick_state(),
                t.ti_cpu(),
                t.sched_entity().is_on_rq() as u8,
                vfork_ppid,
                t.last_syscall_nr.load(Ordering::Relaxed),
                t.last_syscall_a0.load(Ordering::Relaxed),
            );
            // rwlock-livelock: the parked task's user entry frame — syscall
            // nr + user rip decide "kernel thread" (no user frame) vs
            // "userspace task blocked in syscall X at library Y". The old
            // print left task 312 anonymous; lastsys/a0 stayed 0.
            if let Some(pr) = t.pt_regs_ref() {
                #[cfg(feature = "x86_64")]
                let (urip, _usp, unr, _ua0, _ua1, _ua2) =
                    (pr.rip, pr.rsp, pr.orig_rax, pr.rdi, pr.rsi, pr.rdx);
                #[cfg(feature = "riscv64")]
                let (urip, _usp, unr, _ua0, _ua1, _ua2) =
                    (pr.epc, pr.sp, pr.a7, pr.orig_a0, pr.a1, pr.a2);
                #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
                let (urip, unr) = (0u64, 0u64);
                let _ = write!(line, " unrip={:#x} unrsys={}", urip, unr as i64);
            }
        }
        crate::pr_err!("{}", line);
        let thsp = t.thread().sp;
        if (thsp >> 48) == 0xffff && (thsp & 7) == 0 && thsp > 0x1000 {
            // R76: scan UPWARD from the parked sp through the
            // bounds-checked window (INV-WINDOW-*); the Task pins the
            // stack pages.
            let win = t.kernel_stack_top_checked().and_then(|top| {
                crate::safety::frame::KernelStackWindow::new(
                    top - crate::config::KERNEL_STACK_SIZE,
                    top,
                )
            });
            let mut shown = 0;
            // R76: scan UPWARD from the parked sp — the caller frames of
            // the parked chain live at HIGHER addresses (stacks grow down);
            // the words below sp are the context-switch machinery, which is
            // byte-identical for every parked task and names nothing.
            // Window = [text start, text end): the old hardcoded cap
            // 0xffffffff80200000 sat entirely BELOW the x86_64 kernel text
            // (0xffffffff80200000..0xffffffff80690000), so no victim frame
            // ever matched and every report showed only the scanner's own
            // backtrace. The two arch linker scripts spell the bounds
            // differently (x86_64: __stext/__etext, riscv64: _stext/_etext).
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
            let mut line = alloc::format!("      HUNG-STK sp={:#x}", thsp);
            {
                use core::fmt::Write as _;
                for k in 0..200 {
                    let v = match win.and_then(|w2| w2.read_u64(thsp as usize + k * 8)) {
                        Some(v) => v,
                        None => break,
                    };
                    if v >= lo && v < hi && (v & 1) == 0 {
                        // Symbolize inline (2026-11-28 drill lane): the raw
                        // hex forced offline addr2line correlation before the
                        // wait site could be named; the ksym table rides in
                        // .rodata either way.
                        let _ = write!(line, " {:#x}({})", v, crate::debug::ksyms::format_symbol(v));
                        shown += 1;
                        if shown >= 8 {
                            break;
                        }
                    }
                }
            }
            crate::pr_err!("{}", line);
        }
    }

    let taint_str = taint::taint_string_arr();
    let taint_display = core::str::from_utf8(&taint_str).unwrap_or("?");
    crate::pr_err!("      Tainted: {}", taint_display);

    // Stack trace — ring variant (raw-uart dump_stack() is panic-only).
    backtrace::dump_stack_ring();

    // Taint kernel
    taint::add_taint(taint::TaintFlags::DIE);
}
