//! Task thread-state helpers that followed fork.rs to the process
//! crate (seed-core-design 1a: cold free functions leave). Their only
//! callers are the fork path (this crate); thread_set_entry stayed
//! core-side (init.rs calls it by name at boot).

#![forbid(unsafe_code)]

use crate::process::task::Task;

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
