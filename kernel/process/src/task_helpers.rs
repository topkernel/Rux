//! Task thread-state helpers that followed fork.rs to the process
//! crate (seed-core-design 1a: cold free functions leave). Their only
//! callers are the fork path (this crate); thread_set_entry stayed
//! core-side (init.rs calls it by name at boot).

#![forbid(unsafe_code)]

use rux::process::types::Task;

/// user-side copies live in its pt_regs).
pub fn thread_clear_callee_saved(thread: &mut crate::arch::thread::ThreadStruct) {
    #[cfg(feature = "riscv64")]
    {
        thread.s.fill(0);
    }
    #[cfg(feature = "x86_64")]
    {
        thread.callee = Default::default();
    }
}

/// thread.tp_value and re-installed at switch-in).
pub fn set_user_tls(task: &mut Task, tls: u64) {
    #[cfg(feature = "riscv64")]
    {
        // The SAFE frame view (pt_regs_mut rides the audited edge).
        if let Some(regs) = task.pt_regs_mut() {
            regs.tp = tls;
        }
    }
    #[cfg(feature = "x86_64")]
    {
        task.thread_mut().fs_base = tls;
    }
    #[cfg(feature = "aarch64")]
    {
        task.thread_mut().tp_value = tls;
    }
}

// ==================== arch thread bridges (p23-shrink-process) ====================
// Moved here from core/src/process/hotpath.rs (task_helpers.rs was this
// bridge's planned home when the crate flip ran). The framework's boot
// task creator (init.rs) and the console TIOCSCTTY-family pgid reader
// reach them through the ProcessHooks rows; the fork path (this crate)
// calls them directly.

/// Zero the kernel-side callee-saved registers of a fresh task (the

/// Seed a fresh task's context-switch state: the first switch-in
/// "returns" into `entry` with `sp` as the kernel stack pointer.
pub fn thread_set_entry(
    thread: &mut crate::arch::thread::ThreadStruct,
    entry: u64,
    sp: u64,
) {
    thread.sp = sp;
    #[cfg(feature = "riscv64")]
    {
        thread.ra = entry;
    }
    #[cfg(feature = "aarch64")]
    {
        // Same shape as riscv64: __switch_to returns into thread.ra with
        // sp = thread.sp. The fork child's ra = ret_from_fork, whose
        // first switch-in runs the user-exit loop into the seeded
        // pt_regs frame (child erets to EL0 with x0 = 0).
        thread.ra = entry;
    }
    #[cfg(feature = "x86_64")]
    {
        thread.callee.ret_addr = entry;
    }
}

/// Get current process group ID
pub fn current_pgid() -> u32 {
    // SAFETY: sched::current() returns a valid pointer to the currently running task.
    crate::sched::hotpath::current().map_or(0, |t| rux::process::r#unsafe::task_pgid_raw(t))
}

