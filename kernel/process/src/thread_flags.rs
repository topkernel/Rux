//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! thread_info-style flag values that MOVE (p23-shrink-process).
//! TIF_SIGPENDING / TIF_NEED_RESCHED stayed core-side: the pinned impl
//! Task floor reads them in-file. TIF_MEMDIE moves here next to its only
//! setter policy (the alloc-failure task-kill latch); the framework's
//! alloc-failure path reaches the latch through the ProcessHooks
//! memdie_latch row (mm/oom_kill.rs reads the constant directly).

#![forbid(unsafe_code)]
/// TIF_MEMDIE - exiting (out of memory)
pub const TIF_MEMDIE: u32 = 4;

/// Test-and-set TIF_MEMDIE on the current task (the framework's
/// alloc-failure kill_self latch): false when the task already carries
/// the flag (do_exit must not recurse into itself through the alloc
/// path it just failed), true when this call armed it.
pub fn memdie_latch() -> bool {
    let Some(task) = crate::sched::hotpath::current() else {
        return false;
    };
    if task.test_ti_flag(TIF_MEMDIE) {
        false
    } else {
        task.set_ti_flag(TIF_MEMDIE);
        true
    }
}
