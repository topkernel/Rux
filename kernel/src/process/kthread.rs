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

use crate::process::task::{self, Task, TaskState};
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
            core::mem::size_of::<crate::arch::riscv64::pt_regs::PtRegs>(),
        );
    }

    // 4b. Set sstatus.SPP = 1 so ret_from_exception returns to S-mode
    //     (kernel threads must return to supervisor mode, not user mode)
    // SAFETY: pt_regs_ptr was just zeroed above and points to valid memory
    // on the kernel stack; SR_SPP is a constant with only the SPP bit set.
    unsafe {
        (*pt_regs_ptr).status = crate::arch::riscv64::pt_regs::SR_SPP;
    }

    // 5. Set up thread context for ret_from_fork_kernel_asm
    //    - thread.ra = entry point
    //    - thread.sp = pt_regs at stack top
    //    - thread.s[0] = fn_ptr (restored to s0, read by asm)
    //    - thread.s[1] = arg    (restored to s1, read by asm)
    extern "C" {
        fn ret_from_fork_kernel_asm();
    }
    {
        let thread = task.thread_mut();
        thread.ra = ret_from_fork_kernel_asm as u64;
        thread.sp = pt_regs_ptr as u64;
        thread.s[0] = fn_ptr as u64;
        thread.s[1] = arg as u64;
    }

    // 6. Set task state to RUNNING (already set by alloc_task_slot, but be explicit)
    task.set_state(TaskState::new(TaskState::RUNNING));

    // 7. Store KthreadInfo
    {
        let mut map = KTHREAD_MAP.lock();
        map.insert(pid, KthreadInfo {
            should_stop: AtomicBool::new(false),
            result: AtomicI32::new(0),
        });
    }

    // 8. Enqueue the task (makes it visible to scheduler)
    //    enqueue_task consumes the mutable reference, so we re-borrow via raw pointer.
    sched::enqueue_task(task);

    crate::pr_info!("kthread: created kernel thread '{}' pid={}", _name, pid);

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

    // In a full implementation we'd wait for the thread to exit here.
    // For now, just return 0 since we can't easily block under BKL.
    // The caller can use wait_chldexit or similar mechanism.
    crate::pr_info!("kthread: stop requested for pid={}", pid);

    0
}

/// Bind a kernel thread to a specific CPU.
///
/// Must be called before the thread is first scheduled (i.e., right after
/// `kernel_thread()` returns, before it runs).
pub fn kthread_bind(task: &mut Task, cpu: usize) {
    let mask = 1u32 << cpu;
    task.set_cpus_allowed(mask);
    task.set_ti_cpu(cpu as i32);
}
