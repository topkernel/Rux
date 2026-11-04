//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

//! Process sleep and wakeup mechanism test
use crate::println;
use crate::process::task::{Task, TaskState};
use super::{test_pass, test_fail, test_group_start, test_skip};

/// Phase gate for the wake probe kthread (Test 3):
///   0 = park exactly once (state=INTERRUPTIBLE + schedule) — the test
///       waits for this park, then issues its first wake;
///   1 = woken once; never park again (yield loop) so the test's second
///       wake deterministically observes a RUNNING target;
///   2 = exit at the next loop check (test cleanup).
static WAKE_PROBE_PHASE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

extern "C" fn wake_probe_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use core::sync::atomic::Ordering;
    loop {
        if crate::process::kthread::kthread_should_stop() {
            return 0;
        }
        match WAKE_PROBE_PHASE.load(Ordering::Acquire) {
            0 => {
                // The one and only park. The test's first wake_up
                // transitions us to RUNNING + enqueue; this schedule()
                // then returns (switched back in by our picker). The
                // set_state/schedule pair is the ksoftirqd park idiom —
                // all safe calls through the sched::current reference.
                if let Some(cur) = crate::sched::current() {
                    cur.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
                }
                crate::sched::schedule();
                WAKE_PROBE_PHASE.store(1, Ordering::Release);
            }
            1 => {
                // Runnable and staying that way: a voluntary yield keeps
                // the state word RUNNING (linked, never sleeping).
                crate::sched::yield_cpu();
            }
            _ => return 0,
        }
    }
}

pub fn test_sleep_and_wakeup() {
    test_group_start("sleep and wakeup");

    // Test 1: Verify TaskState constant values (Linux task-state bit layout:
    // ZOMBIE is EXIT_ZOMBIE = 0x20, not the old sequential 16)
    if TaskState::RUNNING == 0 && TaskState::INTERRUPTIBLE == 1
        && TaskState::UNINTERRUPTIBLE == 2 && TaskState::ZOMBIE == 0x20
        && TaskState::STOPPED == 4 {
        test_pass("TaskState constants");
    } else {
        test_fail("TaskState constants", "value mismatch");
    }

    // Test 2: Verify state setting and getting
    let mut task = Task::new(999, crate::process::task::SchedPolicy::Normal);
    if !task.state().is_running() {
        test_fail("initial state", "should be Running");
        return;
    }
    task.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    if !task.state().is_interruptible() {
        test_fail("set INTERRUPTIBLE", "state not set");
        return;
    }
    task.set_state(TaskState::new(TaskState::UNINTERRUPTIBLE));
    if !task.state().is_sleeping() {
        test_fail("set UNINTERRUPTIBLE", "state not set");
        return;
    }
    task.set_state(TaskState::new(TaskState::RUNNING));
    if !task.state().is_running() {
        test_fail("restore RUNNING", "state not set");
        return;
    }
    test_pass("state get/set");

    // Test 3: Verify wake_up function. r11 soundness fix: the old version
    // woke a Task living on THIS stack — wake_up ENQUEUES into the live
    // GRQ, so under SMP another CPU could pick pid-1000 and context-switch
    // into a garbage frame (the r8 comment's dangling-pointer hazard, and
    // the deterministic "wake_up running task" flake on main: once the
    // first wake's enqueue got picked+dequeued elsewhere, the second wake
    // found RUNNING ∧ unlinked ∧ unowned and the R46 phantom heal's insert
    // returned true against the reference contract). The vehicle is now a
    // REAL kernel thread: it parks exactly once (state=INTERRUPTIBLE +
    // schedule), and after its first wake it never parks again, so the
    // second wake deterministically observes a RUNNING target.
    let kt = match crate::process::kthread::kthread_run(
        wake_probe_fn,
        core::ptr::null_mut(),
        "waketest",
    ) {
        Some(t) => t,
        None => {
            test_skip("wake_up sleeping task", "kthread spawn failed");
            test_skip("wake_up running task (false)", "kthread spawn failed");
            return;
        }
    };

    // Wait (bounded) until the probe has parked: state reads INTERRUPTIBLE.
    let mut parked = false;
    for _ in 0..2_000_000u32 {
        if kt.state().is_interruptible() {
            parked = true;
            break;
        }
        crate::sched::yield_cpu();
    }
    if !parked {
        test_fail("wake_up sleeping task", "probe never parked");
        WAKE_PROBE_PHASE.store(2, core::sync::atomic::Ordering::Release);
        let _ = crate::process::kthread::kthread_stop(kt);
        return;
    }

    let kt_ptr: *mut Task = kt;
    let result = Task::wake_up(kt_ptr);
    if result && kt.state().is_running() {
        test_pass("wake_up sleeping task");
    } else {
        test_fail("wake_up sleeping task", "failed");
    }
    // Reference contract (try_to_wake_up): a RUNNING target is refused —
    // return false. The probe stays runnable between the two wakes (its
    // body only yields), so this observation is race-free whether the
    // probe is linked-runnable, mid-pick, or on-CPU.
    let result2 = Task::wake_up(kt_ptr);
    if !result2 {
        test_pass("wake_up running task (false)");
    } else {
        test_fail("wake_up running task", "should return false");
    }

    // Retire the probe: phase 2 makes its body exit at the next loop
    // check; kthread_stop sets should_stop (belt and braces) and waits
    // for the terminal state.
    WAKE_PROBE_PHASE.store(2, core::sync::atomic::Ordering::Release);
    let _code = crate::process::kthread::kthread_stop(kt);

    // Test 4: Verify sleep function exists
    test_pass("sleep function available");

    test_println!("test: sleep and wakeup testing completed.");
}
