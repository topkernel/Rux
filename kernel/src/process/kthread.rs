//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kernel thread subsystem
//!
//! Provides `kernel_thread()` and simplified `kthread` API for creating
//! kernel-mode threads. Used by ksoftirqd and other kernel services.

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use crate::sync::spinlock::Spinlock;

use crate::process::task::{self, Task};
use crate::sched;

// ============================================================================
// Kthread info storage
// ============================================================================

/// Per-thread kthread state, stored in a static map keyed by PID
struct KthreadInfo {
    /// Whether kthread_stop() has been called
    should_stop: AtomicBool,
    /// Return value from the thread function
    result: AtomicI32,
    /// Thread name (shown as "Name:"/comm in /proc like Linux kthreads)
    name: &'static str,
}

/// Static map: PID → KthreadInfo
static KTHREAD_MAP: Spinlock<BTreeMap<u32, KthreadInfo>> = Spinlock::new(BTreeMap::new());

// ============================================================================
// Kernel thread creation
// ============================================================================

/// Create a kernel thread.
///
/// Allocates a new task, sets it up as a kernel thread with
/// `ret_from_fork_kernel_asm` as entry point, and enqueues it.
///
/// # Arguments
/// - `fn_ptr`: Thread function (`extern "C" fn(*mut c_void) -> i32`)
/// - `arg`: Argument passed to the thread function
/// - `flags`: Creation flags (reserved, pass 0)
/// - `name`: Human-readable name (stored in KthreadInfo for debugging)
///
/// # Returns
/// Reference to the new Task, or None on failure.
///
/// # Safety
/// Must be called from process context (after scheduler init).
pub fn kernel_thread(
    fn_ptr: extern "C" fn(*mut core::ffi::c_void) -> i32,
    arg: *mut core::ffi::c_void,
    _flags: u32,
    _name: &'static str,
) -> Option<&'static mut Task> {
    // 1. Allocate a task slot (includes kernel stack allocation)
    let task_ptr = sched::alloc_task_slot()?;
    // SAFETY: alloc_task_slot returns a valid, properly aligned pointer to a
    // zeroed Task struct with an associated kernel stack.
    let task = unsafe { &mut *task_ptr };

    // 2. Mark as kernel thread
    task.set_ti_flag(task::task_flags::TaskFlags::PF_KTHREAD.bits());

    // 3. Compute pt_regs address before mutable borrow (pt_regs takes &self)
    let pt_regs_ptr = task.pt_regs();
    let pid = task.pid();

    // 4. Zero out pt_regs (clean slate for ret_from_exception)
    // SAFETY: pt_regs_ptr points to the saved-register area at the top of the
    // kernel stack allocated by alloc_task_slot; size matches PtRegs layout.
    unsafe {
        core::ptr::write_bytes(
            pt_regs_ptr, 0u8,
            core::mem::size_of::<crate::arch::pt_regs::PtRegs>(),
        );
    }

    // 4b. Mark the frame supervisor-mode so the return path stays in
    //     kernel mode (kernel threads never enter user mode)
    // SAFETY: pt_regs_ptr was just zeroed above and points to valid memory
    // on the kernel stack.
    unsafe {
        (*pt_regs_ptr).mark_kernel_frame();
    }

    // 5. Set up thread context for ret_from_fork_kernel_asm
    //    - thread.ra = entry point
    //    - thread.sp = pt_regs at stack top
    //    - thread.s[0] = fn_ptr (restored to s0, read by asm)
    //    - thread.s[1] = arg    (restored to s1, read by asm)
    extern "C" {
        /// riscv64 kernel-thread trampoline (reads fn/arg from s0/s1).
        #[cfg(feature = "riscv64")]
        fn ret_from_fork_kernel_asm();
        /// x86_64 trap entry (restores the pt_regs at thread.sp).
        #[cfg(feature = "x86_64")]
        fn ret_from_fork();
    }
    {
        let thread = task.thread_mut();
        #[cfg(feature = "riscv64")]
        {
            thread.ra = ret_from_fork_kernel_asm as u64;
            thread.sp = pt_regs_ptr as u64;
            thread.s[0] = fn_ptr as u64;
            thread.s[1] = arg as u64;
        }
        #[cfg(feature = "x86_64")]
        {
            // fn/arg stashed in the switch-restored r12/r13 slots until
            // the x86_64 kthread trampoline contract is pinned.
            thread.callee.ret_addr = ret_from_fork as u64;
            thread.sp = pt_regs_ptr as u64;
            thread.callee.r12 = fn_ptr as u64;
            thread.callee.r13 = arg as u64;
        }
    }

    // 6. Task state: stays TASK_NEW (written by new_task_at) until the
    //    class insert flips it to RUNNING atomically with the linkage
    //    (R52 — the old explicit set_state(RUNNING) here re-opened the
    //    half-built-task-is-wakeable window between this line and the
    //    enqueue below).

    // 7. Store KthreadInfo
    {
        // comm mirrors the kthread name (Linux kthreads carry their name in
        // task_struct->comm) — the DFX task dumps and /proc then agree with
        // the KTHREAD_MAP name instead of printing empty comm= lines.
        let mut name_buf = [0u8; 16];
        let len = core::cmp::min(_name.len(), 15);
        name_buf[..len].copy_from_slice(&_name.as_bytes()[..len]);
        task.set_comm(&name_buf);
        let mut map = KTHREAD_MAP.lock();
        map.insert(pid, KthreadInfo {
            should_stop: AtomicBool::new(false),
            result: AtomicI32::new(0),
            name: _name,
        });
    }

    // 8. Enqueue the task (makes it visible to scheduler)
    //    enqueue_task consumes the mutable reference, so we re-borrow via raw pointer.
    //    R52: CHECK the insert — a refused enqueue must unwind the kthread
    //    (hash/pid/slot + the KTHREAD_MAP entry), never leave a
    //    constructed-but-unschedulable task behind (fork.rs has the twin).
    if !sched::enqueue_task(task) {
        crate::pr_warn!(
            "kthread '{}': enqueue refused for pid={} — unwinding (R52 tripwire)",
            _name, pid
        );
        {
            let mut map = KTHREAD_MAP.lock();
            map.remove(&pid);
        }
        // SAFETY: task_ptr was returned by alloc_task_slot and never
        // enqueued; unwind mirrors the fork.rs failure paths (hash first,
        // then the stack — R7-B3/R33-pre discipline).
        unsafe {
            crate::process::pid_hash::pid_hash_remove(pid);
            (*task_ptr).free_kernel_stack();
            crate::process::pid::free_pid(pid);
            crate::sched::free_task_slot(task_ptr);
        }
        return None;
    }


    // SAFETY: task_ptr still points to the valid Task allocated above;
    // enqueue_task consumed the mutable borrow but the allocation persists.
    Some(unsafe { &mut *task_ptr })
}

/// Create and immediately wake a kernel thread.
///
/// Convenience wrapper around `kernel_thread()`.
#[inline]
pub fn kthread_run(
    fn_ptr: extern "C" fn(*mut core::ffi::c_void) -> i32,
    arg: *mut core::ffi::c_void,
    name: &'static str,
) -> Option<&'static mut Task> {
    kernel_thread(fn_ptr, arg, 0, name)
}

// ============================================================================
// Kthread control
// ============================================================================

/// Check if the current kernel thread should stop.
///
/// Call this in your kernel thread's main loop.
/// Returns `true` if `kthread_stop()` has been called.
pub fn kthread_should_stop() -> bool {
    let pid = crate::process::current_pid();
    let map = KTHREAD_MAP.lock();
    match map.get(&pid) {
        Some(info) => info.should_stop.load(Ordering::Acquire),
        None => false,
    }
}

/// A kernel thread's name (procfs comm: /proc/[pid]/status "Name:",
/// /proc/[pid]/comm and field 2 of /proc/[pid]/stat).
pub fn kthread_name(pid: u32) -> Option<&'static str> {
    let map = KTHREAD_MAP.lock();
    map.get(&pid).map(|info| info.name)
}

/// Signal a kernel thread to stop and wait for it to exit.
///
/// Sets the should_stop flag and wakes the thread.
/// Returns the thread's exit code.
///
/// # Note
/// In the current BKL environment, this must be called carefully
/// to avoid deadlock. The caller should ensure BKL is released
/// before calling if the target thread might hold it.
pub fn kthread_stop(task: &mut Task) -> i32 {
    let pid = task.pid();

    // Set should_stop flag
    {
        let map = KTHREAD_MAP.lock();
        if let Some(info) = map.get(&pid) {
            info.should_stop.store(true, Ordering::Release);
        }
    }

    // Wake the thread if sleeping
    let task_ptr: *mut Task = task as *mut Task;
    Task::wake_up(task_ptr);

    crate::pr_info!("kthread: stop requested for pid={}", pid);

    // Wait for the thread to reach a terminal state (ZOMBIE or DEAD).
    // Yield CPU each iteration to avoid busy-spinning.
    while !task.state().is_dead() {
        crate::sched::yield_cpu();
    }

    // Clean up KthreadInfo
    {
        let mut map = KTHREAD_MAP.lock();
        map.remove(&pid);
    }

    task.exit_code()
}

/// Bind a kernel thread to a specific CPU.
///
/// Must be called before the thread is first scheduled (i.e., right after
/// `kernel_thread()` returns, before it runs).
pub fn kthread_bind(task: &mut Task, cpu: usize) {
    let mask = if cpu < 32 { 1u32 << cpu } else { 0u32 };
    task.set_cpus_allowed(mask);
    task.set_ti_cpu(cpu as i32);
}
