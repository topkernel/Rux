//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! Scheduler Implementation
//!
//!
//! - Scheduling classes (sched_class): fair, rt, idle, deadline
//! - Run queues (rq): one rq per CPU
//! - Scheduling entities (sched_entity): fair scheduling unit
//! - Scheduling entry: schedule() -> __schedule() -> context_switch()
//!
//! Current implementation: Simple FIFO scheduler (extensible to CFS)
//!
//! Note: Raw pointers are used to avoid borrow checker limitations, which is common practice in OS kernel development

use crate::errno;
use crate::process::task::{Task, TaskState, SchedPolicy, Pid};
use crate::arch;
use crate::println;
use crate::fs::{FdTable, File, FileFlags, FileOps, CharDev};
use crate::config::{MAX_CPUS, DEFAULT_TIME_SLICE_MS, TIME_SLICE_TICKS};
use alloc::sync::Arc;
use alloc::boxed::Box;
use crate::process::pid::alloc_pid;
use core::arch::asm;
use crate::sync::spinlock::Spinlock;

// Use config value for max tasks
use crate::config::MAX_TASKS;

pub struct RunQueue {
    /// CFS run queue
    ///
    /// Red-black tree sorted by vruntime (BTreeMap implementation)
    pub cfs_rq: crate::sched::fair::CfsRunQueue,

    /// RT run queue
    ///
    /// Priority bitmap + per-priority lists for O(1) selection
    pub rt: crate::sched::rt::RtRunQueue,

    /// Deadline run queue
    ///
    /// Sorted by earliest deadline (EDF)
    pub dl: crate::sched::deadline::DlRunQueue,

    /// Run queue - using raw pointers (retained for legacy scheduling)
    pub tasks: [*mut Task; MAX_TASKS],

    /// Currently running task
    pub current: *mut Task,

    /// Task count
    pub nr_running: usize,

    /// Idle task
    pub idle: *mut Task,

    /// Stop task (for CPU hotplug/migration)
    pub stop: *mut Task,

    /// Whether to use CFS scheduler
    ///
    /// true: Use CFS scheduling
    /// false: Use simple Round Robin scheduling
    use_cfs: bool,
}

unsafe impl Send for RunQueue {}

static mut PER_CPU_RQ: [Option<Spinlock<RunQueue>>; MAX_CPUS] = [None, None, None, None];

static RQ_INIT_LOCK: Spinlock<[bool; MAX_CPUS]> = Spinlock::new([false; MAX_CPUS]);


static mut NEED_RESCHED: [core::sync::atomic::AtomicBool; MAX_CPUS] = [
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
];

#[inline]
pub fn need_resched() -> bool {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id >= MAX_CPUS {
            return false;
        }
        NEED_RESCHED[cpu_id].load(core::sync::atomic::Ordering::Acquire)
    }
}

/// Assembly-callable need_resched check (returns 0 or 1 in a0)
#[no_mangle]
pub extern "C" fn asm_need_resched() -> i64 {
    if need_resched() { 1 } else { 0 }
}

#[inline]
pub fn set_need_resched() {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id < MAX_CPUS {
            NEED_RESCHED[cpu_id].store(true, core::sync::atomic::Ordering::Release);
        }
    }
}

#[inline]
fn clear_need_resched() {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id < MAX_CPUS {
            NEED_RESCHED[cpu_id].store(false, core::sync::atomic::Ordering::Release);
        }
    }
}

pub fn scheduler_tick() {
    // Touch softlockup timestamp
    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    crate::dfx::softlockup::touch(cpu_id);

    // Get current CPU's run queue
    let rq = match this_cpu_rq() {
        Some(r) => r,
        None => return,
    };

    let mut rq_inner = rq.lock();
    let current = rq_inner.current;

    if current.is_null() {
        return;
    }

    // If using CFS scheduler
    if rq_inner.use_cfs {
        // Get current time
        let now = crate::sched::fair::sched_clock();

        // Update current task's execution time
        rq_inner.cfs_rq.update_curr(now);

        unsafe {
            // Step 1: Get current task's scheduling info (immutable borrow)
            let (curr_vruntime, curr_weight) = {
                let task = &*current;
                let se = task.sched_entity();
                (se.get_vruntime(), se.load.weight)
            };

            // Calculate time slice
            let slice_ns = rq_inner.cfs_rq.sched_slice(&crate::sched::fair::SchedEntity {
                load: crate::sched::fair::LoadWeight::new(curr_weight),
                vruntime: core::sync::atomic::AtomicU64::new(curr_vruntime),
                sum_exec_runtime: core::sync::atomic::AtomicU64::new(0),
                exec_start: core::sync::atomic::AtomicU64::new(0),
                prev_sum_exec_runtime: core::sync::atomic::AtomicU64::new(0),
                on_rq: core::sync::atomic::AtomicBool::new(false),
                slice: core::sync::atomic::AtomicU64::new(0),
            });
            let slice_ticks = (slice_ns / 10_000_000) as u32;

            // Step 2: Update time slice and decrement (mutable borrow)
            let still_has_slice = {
                let task = &mut *current;
                task.set_time_slice(slice_ticks.max(1));
                task.tick_time_slice()
            };

            if !still_has_slice {
                // Time slice exhausted, set need reschedule flag
                drop(rq_inner);
                set_need_resched();
            } else {
                // Check if preemption is needed
                // If there's a task with smaller vruntime in queue, should preempt
                if let Some(next) = rq_inner.cfs_rq.peek_next() {
                    if !next.is_null() && next != current {
                        // Get next task's vruntime
                        let next_vruntime = {
                            let next_task = &*next;
                            let next_se = next_task.sched_entity();
                            next_se.get_vruntime()
                        };

                        // Check if preemption is needed
                        let wakeup_granularity = crate::sched::fair::SCHED_MIN_GRANULARITY_NS;
                        if curr_vruntime > next_vruntime {
                            let delta = curr_vruntime - next_vruntime;
                            if delta > wakeup_granularity {
                                drop(rq_inner);
                                set_need_resched();
                            }
                        }
                    }
                }
            }
        }
        return;
    }

    // Round Robin scheduler
    // Update time slice (using Task's public method)
    let task = unsafe { &mut *current };
    let still_has_slice = task.tick_time_slice();

    // Check if time slice is exhausted
    if !still_has_slice {
        // Time slice exhausted, reallocate time slice
        task.reset_time_slice();

        // Set need_resched flag to trigger rescheduling
        drop(rq_inner);  // Release lock before setting flag
        set_need_resched();
    }
}

pub fn resched_curr() {
    set_need_resched();
}

/// Remote trigger reschedule on specified CPU
///
/// When a task on a CPU needs to be scheduled,
/// send IPI to notify target CPU
///
///
/// # Arguments
/// * `cpu` - Target CPU ID
pub fn resched_cpu(cpu: usize) {
    unsafe {
        if cpu < MAX_CPUS {
            NEED_RESCHED[cpu].store(true, core::sync::atomic::Ordering::Release);
            // Send Reschedule IPI to target CPU if different from current
            let this_cpu = crate::arch::cpu_id() as usize;
            if this_cpu != cpu {
                #[cfg(feature = "riscv64")]
                crate::arch::ipi::send_reschedule_ipi(cpu);
            }
        }
    }
}


pub fn wake_up_process(task: *mut Task) -> bool {
    use crate::process::Task;
    Task::wake_up(task)
}

pub fn this_cpu_rq() -> Option<&'static Spinlock<RunQueue>> {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id >= MAX_CPUS {
            return None;
        }
        PER_CPU_RQ[cpu_id].as_ref()
    }
}

pub fn cpu_rq(cpu_id: usize) -> Option<&'static Spinlock<RunQueue>> {
    unsafe {
        if cpu_id >= MAX_CPUS {
            return None;
        }
        PER_CPU_RQ[cpu_id].as_ref()
    }
}

pub fn init_per_cpu_rq(cpu_id: usize) {
    if cpu_id >= MAX_CPUS {
        return;
    }

    let mut init_flags = RQ_INIT_LOCK.lock();
    if init_flags[cpu_id] {
        return;  // Already initialized
    }

    unsafe {
        let mut rt_rq = crate::sched::rt::RtRunQueue::new();
        rt_rq.init();

        let mut dl_rq = crate::sched::deadline::DlRunQueue::new();

        PER_CPU_RQ[cpu_id] = Some(Spinlock::new(RunQueue {
            cfs_rq: crate::sched::fair::CfsRunQueue::new(),
            rt: rt_rq,
            dl: dl_rq,
            tasks: [core::ptr::null_mut(); MAX_TASKS],
            current: core::ptr::null_mut(),
            nr_running: 0,
            idle: core::ptr::null_mut(),
            stop: core::ptr::null_mut(),
            use_cfs: true,
        }));

        init_flags[cpu_id] = true;
    }
}

// Each CPU needs its own idle task storage
static mut IDLE_TASK_STORAGES: [core::mem::MaybeUninit<Task>; MAX_CPUS] = [
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
];


/// Allocate a Task struct from the kernel heap.
///
/// Dynamically allocates via the global allocator (buddy/slab backend),
/// initializes the Task in-place, and registers it in the PID hash table.
pub fn alloc_task_slot() -> Option<*mut Task> {
    let layout = core::alloc::Layout::new::<Task>();
    let task_ptr = unsafe { alloc::alloc::alloc(layout) } as *mut Task;
    if task_ptr.is_null() {
        return None;
    }

    let pid = match alloc_pid() {
        Some(p) => p,
        None => {
            unsafe { alloc::alloc::dealloc(task_ptr as *mut u8, core::alloc::Layout::new::<Task>()); }
            return None;
        }
    };

    unsafe {
        Task::new_task_at(task_ptr, pid, SchedPolicy::Normal);
        crate::process::pid_hash::pid_hash_insert(task_ptr);
    }

    Some(task_ptr)
}

/// Free a Task struct back to the kernel heap.
pub fn free_task_slot(task_ptr: *mut Task) {
    if task_ptr.is_null() {
        return;
    }
    unsafe {
        alloc::alloc::dealloc(task_ptr as *mut u8, core::alloc::Layout::new::<Task>());
    }
}

pub fn init() {
    // Initialize current CPU's run queue
    let cpu_id = crate::arch::cpu_id() as u64 as usize;

    // Check if CPU ID is valid
    if cpu_id >= MAX_CPUS {
        println!("sched: init: invalid cpu_id {}", cpu_id);
        return;
    }

    init_per_cpu_rq(cpu_id);

    unsafe {
        // Use current CPU's dedicated idle task storage
        let idle_ptr = IDLE_TASK_STORAGES[cpu_id].as_mut_ptr();
        Task::new_idle_at(idle_ptr);

        // Register idle task in PID hash table
        crate::process::pid_hash::pid_hash_insert(idle_ptr);

        // Allocate kernel stack for idle task
        if let Some(stack_top) = (*idle_ptr).alloc_kernel_stack() {
            // Set thread.sp to point to kernel stack top
            // Idle task doesn't need pt_regs, just needs a valid stack
            (*idle_ptr).thread_mut().sp = stack_top as u64;
        } else {
            println!("sched: failed to allocate kernel stack for idle task");
        }

        // Set idle task's ti_cpu field
        // This allows cpu_id() to get hart_id from tp-pointed task_struct
        (*idle_ptr).set_ti_cpu(cpu_id as i32);

        // ===== Switch to tp/sscratch protocol =====
        //
        // Before this:
        //   - tp = hart_id (passed from OpenSBI)
        //   - sscratch = undefined
        //
        // After this:
        //   - tp = idle task pointer (current task_struct)
        //   - sscratch = 0 (indicates kernel mode)
        //
        // This allows trap.S to use sscratch swap to detect user/kernel

        // 1. Set sscratch = 0 (indicates currently in kernel mode)
        core::arch::asm!("csrw sscratch, zero");

        // 2. Switch tp to point to idle task
        //    Now tp points to current CPU's current task_struct
        core::arch::asm!("mv tp, {0}", in(reg) idle_ptr);

        // Set current CPU's run queue
        if let Some(rq) = this_cpu_rq() {
            let mut rq_inner = rq.lock();
            rq_inner.idle = idle_ptr;
            rq_inner.current = idle_ptr;
        }
    }
}

#[inline(never)]
pub fn schedule() {
    unsafe {
        __schedule();
    }
}

/// Assembly-callable schedule wrapper
#[no_mangle]
pub extern "C" fn asm_schedule() {
    schedule();
}

unsafe fn __schedule() {
    // Clear need_resched flag
    clear_need_resched();

    // Get current CPU's run queue
    let rq = match this_cpu_rq() {
        Some(r) => r,
        None => {
            return;
        }
    };

    let mut rq_inner = rq.lock();

    // Get current task
    let prev = rq_inner.current;

    if prev.is_null() {
        return;
    }

    let prev_pid = (*prev).pid();
    let prev_state = (*prev).state().bits();
    let nr_running = rq_inner.nr_running;

    crate::pr_debug!("sched: __schedule, prev={} (state={}, nr_running={})",
        prev_pid, prev_state, nr_running);

    // Update current task's execution time (CFS)
    if rq_inner.use_cfs {
        let now = crate::sched::fair::sched_clock();
        rq_inner.cfs_rq.update_curr(now);
    }

    // If only idle task exists (nr_running == 0), try load balancing
    if rq_inner.nr_running == 0 {
        drop(rq_inner);
        load_balance();

        let rq = match this_cpu_rq() {
            Some(r) => r,
            None => return,
        };
        rq_inner = rq.lock();

        // Even if nr_running == 0, continue to switch to idle task
        // Don't return early, otherwise sret after page fault handling will return to wrong context
    }

    // If current task is still in running state, re-add to CFS queue
    // (if using CFS and current task was previously in queue)
    // Note: idle task (pid=0) should not be added to queue
    if rq_inner.use_cfs && !prev.is_null() {
        let prev_task = &*prev;
        let prev_pid = prev_task.pid();
        let is_running = prev_task.state() == TaskState::new(TaskState::RUNNING);
        if is_running && prev_pid != 0 {
            // Re-add to CFS queue
            rq_inner.cfs_rq.enqueue(prev);
        }
    }

    // Pick next task
    let next = pick_next_task(&mut *rq_inner);

    if !next.is_null() {
        let next_pid = (*next).pid();
        let next_state = (*next).state().bits();

        if next != prev {
            crate::pr_debug!("sched: pick_next, {} -> {} (next_state={})",
                prev_pid, next_pid, next_state);
        }
    }

    if next == prev {
        return;
    }

    // Context switch (needs to be done outside lock)
    drop(rq_inner);

    // Disable interrupts for context switch
    unsafe {
        core::arch::asm!(
            "csrci sstatus, 2",
            options(nomem, nostack)
        );

        context_switch(&mut *prev, &mut *next);

        // Enable interrupts
        core::arch::asm!(
            "csrsi sstatus, 2",
            options(nomem, nostack)
        );
    }
}

unsafe fn pick_next_task(rq: &mut RunQueue) -> *mut Task {
    // Iterate through scheduling classes in priority order
    // stop > deadline > rt > fair > idle

    let rq_ptr = rq as *mut RunQueue;

    // Check stop task first
    if !rq.stop.is_null() {
        return rq.stop;
    }

    // Check deadline tasks
    if !rq.dl.is_empty() {
        if let Some(task) = rq.dl.pick_next() {
            return task;
        }
    }

    // Check RT tasks
    if !rq.rt.is_empty() {
        if let Some(task) = rq.rt.pick_next() {
            return task;
        }
    }

    // Check CFS tasks
    if rq.use_cfs {
        return pick_next_task_cfs(rq);
    }

    // Fall back to Round Robin scheduler
    pick_next_task_rr(rq)
}

/// CFS scheduler: Pick next task
///
/// Select task with smallest vruntime
unsafe fn pick_next_task_cfs(rq: &mut RunQueue) -> *mut Task {
    // Update current task's runtime
    let now = crate::sched::fair::sched_clock();
    rq.cfs_rq.update_curr(now);

    // Try to select next runnable task from CFS queue
    let mut loop_count = 0;
    loop {
        loop_count += 1;
        if loop_count > 100 {
            return rq.idle;
        }

        // Select next task from CFS queue
        let next = match rq.cfs_rq.pick_next() {
            Some(n) => n,
            None => {
                // CFS queue is empty, check current task
                let current = rq.current;
                if !current.is_null() && (*current).state() == TaskState::new(TaskState::RUNNING) {
                    return current;
                }

                // No runnable task, return idle task
                return rq.idle;
            }
        };

        // Check task state, only return RUNNING state tasks
        let task_state = (*next).state();
        if task_state == TaskState::new(TaskState::RUNNING) {
            // Set as current task
            rq.cfs_rq.set_curr(next);

            // Calculate and set time slice
            let task = &mut *next;
            let se = task.sched_entity();
            let slice_ns = rq.cfs_rq.sched_slice(se);
            let slice_ms = crate::sched::fair::sched_slice_to_ms(slice_ns);
            task.set_time_slice(slice_ms.max(1) as u32);  // At least 1ms

            return next;
        }

        // Task is not in RUNNING state (could be ZOMBIE, STOPPED, etc.)
        // Don't re-enqueue, continue to select next task
    }
}

/// Round Robin scheduler: Pick next task (retained as backup)
unsafe fn pick_next_task_rr(rq: &mut RunQueue) -> *mut Task {
    let current = rq.current;

    // Simple linear search
    for i in 0..MAX_TASKS {
        let task_ptr = rq.tasks[i];

        if !task_ptr.is_null() && task_ptr != current {
            let state = (*task_ptr).state();
            if state == TaskState::new(TaskState::RUNNING) {
                return task_ptr;
            }
        }
    }

    // No other runnable task found, check if current task is runnable
    if !current.is_null() && (*current).state() == TaskState::new(TaskState::RUNNING) {
        return current;
    }

    // No runnable task, return idle task
    rq.idle
}

unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // Get current CPU ID
    let cpu_id = crate::arch::cpu_id() as u64 as usize;    // Update current task
    if let Some(rq) = this_cpu_rq() {
        let mut rq_inner = rq.lock();
        rq_inner.current = next;
    }

    // Set next's ti_cpu field
    (*next).set_ti_cpu(cpu_id as i32);

    // Clear fork child flag (execute only once)
    // fork child's context.ra is already set to ret_from_fork
    // Standard cpu_switch_to will restore ra, then ret instruction jumps to ret_from_fork
    if (*next).is_fork_child() {
        (*next).clear_fork_child();
    }

    // NOTE: Page table switch is handled by arch::context::context_switch
    // Do NOT switch page tables here - it would make UART inaccessible

    // Unified context switch path
    // __switch_to only saves/restores callee-saved registers
    // All processes return to user mode through trap return mechanism
    //
    // fork/execve child process:
    // - context.ra = ret_from_fork
    // - context.sp = pt_regs_ptr
    // - cpu_switch_to restores ra and sp
    // - ret instruction jumps to ret_from_fork
    // - ret_from_fork restores all registers from pt_regs and returns to user mode
    //
    // Preempted process:
    // - context saves complete callee-saved registers
    // - After cpu_switch_to restores, returns to where schedule() was called
    drop(&mut *next);
    crate::arch::context::context_switch(prev, next);

    // After cpu_switch_to restores, returns to where schedule() was called
}

/// schedule_tail - Called when fork child is first scheduled
///
/// This function is called when a new task is first scheduled to execute, used to:
/// 1. Complete cleanup after task switch
/// 2. Handle set_child_tid (if set)
/// 3. Calculate pending signals
///
/// # Arguments
/// * `prev` - Previous task (parent process)
///
/// # Note
/// In RISC-V, this function is called by ret_from_fork,
/// at which point the kernel big lock has been acquired (done in assembly).
#[no_mangle]
pub extern "C" fn schedule_tail(prev: *mut Task) {
    unsafe {
        if !prev.is_null() {
            // Complete previous task's switch cleanup
            // finish_task_switch(prev)
            // Rux: Since using kernel big lock, cleanup is relatively simple

            // If previous task state is ZOMBIE, may need to wake up parent process
            // (This part is already handled in do_exit)

            // Release previous task's reference count (if any)
            // TODO: Implement put_task_struct(prev)
        }

        // Handle set_child_tid
        // If user set CLONE_CHILD_SETTID via clone,
        // need to write child process's PID to user memory
        // Rux temporarily skips this since full clone support is still in development

        // Calculate pending signals
        // calculate_sigpending()
        // Rux: Check signals before returning to user mode
    }
}

pub fn enqueue_task(task: &'static mut Task) {
    let cpu_id = task.ti_cpu() as usize;

    // If ti_cpu is unassigned (-1), assign to this CPU.
    // Otherwise use the task's assigned CPU so cross-CPU wakeups
    // (e.g., child exiting on CPU 1 waking parent on CPU 0) work correctly.
    let target_cpu = if cpu_id >= MAX_CPUS {
        let this_cpu = crate::arch::cpu_id() as i32;
        task.set_ti_cpu(this_cpu);
        this_cpu as usize
    } else {
        cpu_id
    };

    if let Some(rq) = cpu_rq(target_cpu) {
        let mut rq_inner = rq.lock();

        if rq_inner.nr_running < MAX_TASKS {
            let task_ptr = task as *mut Task;

            // Set task state to RUNNING
            task.set_state(TaskState::new(TaskState::RUNNING));

            // Dispatch to per-class queue based on scheduling policy
            let policy = task.policy();
            match policy {
                SchedPolicy::Fifo | SchedPolicy::Rr => {
                    rq_inner.rt.enqueue(task_ptr, false);
                }
                SchedPolicy::Deadline => {
                    rq_inner.dl.enqueue(task_ptr);
                }
                SchedPolicy::Normal | SchedPolicy::Batch => {
                    if rq_inner.use_cfs {
                        rq_inner.cfs_rq.enqueue(task_ptr);
                    }
                    // Fall through to legacy queue
                    for i in 0..MAX_TASKS {
                        if rq_inner.tasks[i].is_null() {
                            rq_inner.tasks[i] = task_ptr;
                            rq_inner.nr_running += 1;
                            return;
                        }
                    }
                    return;
                }
                SchedPolicy::Idle => {
                    return;
                }
            }

            // Also add to legacy queue for RT/DL tasks
            for i in 0..MAX_TASKS {
                if rq_inner.tasks[i].is_null() {
                    rq_inner.tasks[i] = task_ptr;
                    rq_inner.nr_running += 1;
                    return;
                }
            }
        }
    }
}

pub fn dequeue_task(task: &Task) {
    let cpu_id = task.ti_cpu() as usize;
    if cpu_id >= MAX_CPUS {
        return;
    }

    if let Some(rq) = cpu_rq(cpu_id) {
        let mut rq_inner = rq.lock();
        let task_ptr = task as *const Task as *mut Task;

        // Remove from per-class queue based on scheduling policy
        let policy = task.policy();
        match policy {
            SchedPolicy::Fifo | SchedPolicy::Rr => {
                rq_inner.rt.dequeue(task_ptr);
            }
            SchedPolicy::Deadline => {
                rq_inner.dl.dequeue(task_ptr);
            }
            SchedPolicy::Normal | SchedPolicy::Batch => {
                if rq_inner.use_cfs {
                    rq_inner.cfs_rq.dequeue(task_ptr);
                }
            }
            SchedPolicy::Idle => {}
        }

        // Remove from legacy queue
        for i in 0..MAX_TASKS {
            if rq_inner.tasks[i] == task_ptr {
                rq_inner.tasks[i] = core::ptr::null_mut();
                rq_inner.nr_running -= 1;
                return;
            }
        }
    }
}

pub fn yield_cpu() {
    // Release kernel big lock (must release before yielding CPU)
    crate::sync::kernel_lock_release();
    schedule();
    // Re-acquire kernel big lock after waking up
    crate::sync::kernel_lock_acquire();
}

/// Iterate over all tasks in the system, calling `f` for each non-null task.
pub fn for_each_task<F>(f: F)
where
    F: Fn(*mut Task),
{
    for cpu in 0..MAX_CPUS {
        if let Some(rq_lock) = cpu_rq(cpu) {
            let rq = rq_lock.lock();
            for i in 0..MAX_TASKS {
                let task_ptr = rq.tasks[i];
                if !task_ptr.is_null() {
                    f(task_ptr);
                }
            }
        }
    }
}

/// Get current task pointer (O(1) via tp register)
///
/// Uses RISC-V tp (thread pointer) register which holds the current
/// task_struct pointer. This avoids acquiring the runqueue lock.
///
/// Note: During early boot, tp contains the hart ID (0-3), not a task pointer.
/// We check for valid kernel address range to distinguish between hart IDs
/// and actual task pointers.
pub fn current() -> Option<&'static mut Task> {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *mut Task;
    // Check for null or small values (hart IDs 0-3 during early boot)
    // Valid task pointers must be in kernel address space (>= 0x80000000)
    if tp.is_null() || (tp as usize) < 0x80000000 {
        None
    } else {
        unsafe { Some(&mut *tp) }
    }
}

/// Get current task PID (O(1) via tp register)
pub fn get_current_pid() -> u32 {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *const Task;
    // Check for null or small values (hart IDs 0-3 during early boot)
    if tp.is_null() || (tp as usize) < 0x80000000 {
        0
    } else {
        unsafe { (*tp).pid() }
    }
}

/// Get current task PPID (O(1) via tp register)
pub fn get_current_ppid() -> u32 {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *const Task;
    // Check for null or small values (hart IDs 0-3 during early boot)
    if tp.is_null() || (tp as usize) < 0x80000000 {
        0
    } else {
        unsafe { (*tp).ppid() }
    }
}

pub unsafe fn find_task_by_pid(pid: Pid) -> *mut Task {
    crate::process::pid_hash::pid_hash_lookup(pid)
}

// ============================================================================
// Load Balancing Mechanism
// ============================================================================

pub(crate) fn rq_load(rq: &RunQueue) -> usize {
    // Use per-class counters only. The legacy rq.nr_running is kept for
    // compatibility but is NOT summed here — enqueue_task() increments both
    // rq.nr_running and per-class counters, so summing both would double-count.
    let cfs_load = if rq.use_cfs { rq.cfs_rq.nr_running() as usize } else { 0 };
    let rt_load = rq.rt.nr_running() as usize;
    let dl_load = rq.dl.nr_running() as usize;
    cfs_load + rt_load + dl_load
}

fn find_busiest_cpu(this_cpu: usize) -> Option<usize> {
    let this_rq = cpu_rq(this_cpu)?;
    let this_load = rq_load(&*this_rq.lock());

    let mut busiest_cpu = None;
    let mut max_load = this_load;

    // Load imbalance threshold (migrate only if difference is at least LOAD_IMBALANCE_THRESH tasks)
    use crate::config::LOAD_IMBALANCE_THRESH;

    for cpu in 0..MAX_CPUS {
        if cpu == this_cpu {
            continue;  // Skip current CPU
        }

        if let Some(rq) = cpu_rq(cpu) {
            let load = rq_load(&*rq.lock());

            // Only migrate when other CPU load is significantly higher
            if load > max_load + LOAD_IMBALANCE_THRESH {
                max_load = load;
                busiest_cpu = Some(cpu);
            }
        }
    }

    busiest_cpu
}

fn steal_task(src_rq: &mut RunQueue) -> Option<*mut Task> {
    // Try stealing from CFS queue first (fair tasks are most migratable)
    if src_rq.use_cfs {
        if let Some(task) = src_rq.cfs_rq.pick_next() {
            // CFS pick_next already removed from per-class queue.
            // Now remove from legacy queue to keep counters consistent.
            remove_from_legacy_queue(src_rq, task);
            return Some(task);
        }
    }

    // Try stealing from RT queue (only if not currently running)
    if !src_rq.rt.is_empty() {
        if let Some(task) = src_rq.rt.pick_next() {
            // Don't steal currently running task
            if task != src_rq.current {
                remove_from_legacy_queue(src_rq, task);
                return Some(task);
            }
        }
    }

    // Try stealing from DL queue
    if !src_rq.dl.is_empty() {
        if let Some(task) = src_rq.dl.pick_next() {
            if task != src_rq.current {
                remove_from_legacy_queue(src_rq, task);
                return Some(task);
            }
        }
    }

    // Try stealing from legacy queue (fallback for tasks not in per-class queues)
    for i in (0..src_rq.nr_running).rev() {
        let task = src_rq.tasks[i];

        if task.is_null() {
            continue;
        }

        let task_ref = unsafe { &*task };

        // Don't steal idle task (PID 0)
        if task_ref.pid() == 0 {
            continue;
        }

        // Don't steal currently running task
        if task == src_rq.current {
            continue;
        }

        // Found migratable task — remove from legacy queue
        src_rq.tasks[i] = core::ptr::null_mut();
        src_rq.nr_running -= 1;

        // Move remaining tasks to fill gap
        for j in i..src_rq.nr_running {
            src_rq.tasks[j] = src_rq.tasks[j + 1];
        }
        src_rq.tasks[src_rq.nr_running] = core::ptr::null_mut();

        return Some(task);
    }

    None
}

/// Remove a task from the legacy `tasks[]` array and decrement `nr_running`.
/// Used by steal_task() after removing from a per-class queue to keep counters consistent.
fn remove_from_legacy_queue(rq: &mut RunQueue, task: *mut crate::process::Task) {
    for i in 0..rq.nr_running {
        if rq.tasks[i] == task {
            rq.tasks[i] = core::ptr::null_mut();
            rq.nr_running -= 1;
            // Compact the array
            for j in i..rq.nr_running {
                rq.tasks[j] = rq.tasks[j + 1];
            }
            rq.tasks[rq.nr_running] = core::ptr::null_mut();
            return;
        }
    }
}

pub fn load_balance() {
    unsafe {
        let this_cpu = crate::arch::cpu_id() as u64 as usize;

        // Get current CPU's run queue
        let this_rq = match this_cpu_rq() {
            Some(r) => r,
            None => return,
        };

        let this_rq_inner = this_rq.lock();
        let this_load = rq_load(&*this_rq_inner);

        // Only load balance when current CPU is idle or very free
        // Threshold: current load <= 1 (only idle task or only one user task)
        if this_load > 1 {
            return;  // Current CPU has enough tasks, no need for load balancing
        }

        drop(this_rq_inner);  // Release lock to avoid deadlock

        // Find busiest CPU
        if let Some(busiest_cpu) = find_busiest_cpu(this_cpu) {
            if let Some(busiest_rq) = cpu_rq(busiest_cpu) {
                let mut busiest_rq_inner = busiest_rq.lock();

                // Steal task from busy CPU
                if let Some(task) = steal_task(&mut *busiest_rq_inner) {
                    // Get task info
                    let _task_pid = (*task).pid();

                    // Release busy CPU's lock
                    drop(busiest_rq_inner);

                    // Re-acquire current CPU's lock
                    let mut this_rq_inner = this_rq.lock();

                    // Add task to current CPU's run queue
                    enqueue_task_locked(&mut *this_rq_inner, task);

                    // Update task's CPU affinity (optional)
                    // (*task).set_cpu(this_cpu);
                }
            }
        }
    }
}

fn enqueue_task_locked(rq: &mut RunQueue, task: *mut Task) {
    if task.is_null() {
        return;
    }

    unsafe {
        let task_ref = &*task;
        let policy = task_ref.policy();

        // Enqueue based on scheduling class
        match policy {
            SchedPolicy::Fifo | SchedPolicy::Rr => {
                rq.rt.enqueue(task, false);
            }
            SchedPolicy::Deadline => {
                rq.dl.enqueue(task);
            }
            SchedPolicy::Normal | SchedPolicy::Batch => {
                if rq.use_cfs {
                    rq.cfs_rq.enqueue_migrate(task);
                } else {
                    // Fall back to legacy queue only
                    if rq.nr_running >= MAX_TASKS {
                        return;
                    }
                    rq.tasks[rq.nr_running] = task;
                    rq.nr_running += 1;
                    return;
                }
            }
            SchedPolicy::Idle => {
                // Idle tasks are never enqueued
                return;
            }
        }

        // Also add to legacy queue for all per-class enqueued tasks
        if rq.nr_running < MAX_TASKS {
            rq.tasks[rq.nr_running] = task;
            rq.nr_running += 1;
        }
    }
}

// ============================================================================
// CPU Idle Loop
// ============================================================================

/// CPU idle loop
///
/// Called when CPU has no tasks to run
/// Will try load balancing, and enter WFI sleep if no tasks
pub fn cpu_idle_loop() -> ! {
    use crate::arch;

    loop {
        // 1. Try to schedule tasks
        unsafe {
            schedule();
        }

        // 2. Check if only idle task exists
        if let Some(rq) = this_cpu_rq() {
            let rq_inner = rq.lock();
            let current = rq_inner.current;
            let nr_running = rq_inner.nr_running;
            drop(rq_inner);

            // If only idle task (nr_running == 1 and current is idle)
            // or no tasks at all (nr_running == 0, shouldn't happen)
            if nr_running == 1 && !current.is_null() {
                unsafe {
                    let pid = (*current).pid();
                    if pid == 0 {
                        // Only idle task, try load balancing
                        drop(rq);
                        load_balance();

                        // Reschedule after load balancing
                        schedule();
                    }
                }
            }
        }

        // 3. Enter WFI sleep, wait for interrupt to wake up
        // Interrupt will set need_resched flag, thus breaking out of WFI
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}
