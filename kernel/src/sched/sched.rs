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
use spin::Mutex;

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
    tasks: [*mut Task; MAX_TASKS],

    /// Currently running task
    pub current: *mut Task,

    /// Task count
    nr_running: usize,

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

static mut PER_CPU_RQ: [Option<Mutex<RunQueue>>; MAX_CPUS] = [None, None, None, None];

static RQ_INIT_LOCK: Mutex<[bool; MAX_CPUS]> = Mutex::new([false; MAX_CPUS]);


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
    // Send Reschedule IPI to target CPU
    #[cfg(feature = "riscv64")]
    crate::arch::ipi::send_reschedule_ipi(cpu);
}


pub fn wake_up_process(task: *mut Task) -> bool {
    use crate::process::Task;
    Task::wake_up(task)
}

pub fn this_cpu_rq() -> Option<&'static Mutex<RunQueue>> {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id >= MAX_CPUS {
            return None;
        }
        PER_CPU_RQ[cpu_id].as_ref()
    }
}

pub fn cpu_rq(cpu_id: usize) -> Option<&'static Mutex<RunQueue>> {
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

        PER_CPU_RQ[cpu_id] = Some(Mutex::new(RunQueue {
            cfs_rq: crate::sched::fair::CfsRunQueue::new(),
            rt: rt_rq,
            dl: dl_rq,
            tasks: [core::ptr::null_mut(); MAX_TASKS],
            current: core::ptr::null_mut(),
            nr_running: 0,
            idle: core::ptr::null_mut(),
            stop: core::ptr::null_mut(),
            use_cfs: false,  // Temporarily disable CFS for debugging timer interrupt
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

use crate::config::TASK_POOL_SIZE as CONFIG_TASK_POOL_SIZE;
const TASK_POOL_SIZE: usize = CONFIG_TASK_POOL_SIZE;

// Calculate actual size of Task struct to ensure each slot is large enough
// Task includes: CpuContext, AddressSpace, Option<Box<FdTable>>,
//                Option<Box<SignalStruct>>, ListHead, etc.
const TASK_SIZE: usize = core::mem::size_of::<Task>();

// Task struct alignment requirement
const TASK_ALIGN: usize = core::mem::align_of::<Task>();

// Calculate aligned slot size (rounded up to alignment boundary)
const TASK_SLOT_SIZE: usize = (TASK_SIZE + TASK_ALIGN - 1) / TASK_ALIGN * TASK_ALIGN;

// Task pool lock - protects TASK_POOL and TASK_BITMAP
static TASK_POOL_LOCK: Mutex<()> = Mutex::new(());

// Use aligned static array as task pool
// Use repr(align) to ensure array has correct alignment
#[repr(C, align(16))]
struct AlignedTaskPool {
    data: [u8; TASK_POOL_SIZE * TASK_SLOT_SIZE],
}

static mut TASK_POOL: AlignedTaskPool = AlignedTaskPool {
    data: [0; TASK_POOL_SIZE * TASK_SLOT_SIZE],
};

/// Bitmap to track which task slots are in use
/// Each bit represents one task slot (1 = in use, 0 = free)
static mut TASK_BITMAP: [u64; (TASK_POOL_SIZE + 63) / 64] = [0; (TASK_POOL_SIZE + 63) / 64];

/// Find first zero bit in a u64
fn find_first_zero_word(word: u64) -> Option<u32> {
    if word == !0 {
        return None;
    }
    Some(word.trailing_ones())
}

/// Allocate a slot from task pool using bitmap
///
/// Returns initialized Task pointer, caller is responsible for setting other Task fields
pub fn alloc_task_slot() -> Option<*mut Task> {
    let _lock = TASK_POOL_LOCK.lock();

    unsafe {
        // Scan bitmap for free slot
        for (word_idx, word) in TASK_BITMAP.iter().enumerate() {
            if let Some(bit_idx) = find_first_zero_word(*word) {
                let slot_idx = word_idx * 64 + bit_idx as usize;
                if slot_idx >= TASK_POOL_SIZE {
                    return None;
                }
                // Mark slot as used
                TASK_BITMAP[word_idx] |= 1u64 << bit_idx;

                let pool_slot_addr = TASK_POOL.data.as_ptr().add(slot_idx * TASK_SLOT_SIZE);
                let task_ptr: *mut Task = pool_slot_addr as *mut Task;

                // Allocate PID
                let pid = match alloc_pid() {
                    Some(p) => p,
                    None => {
                        // Rollback: clear the bit
                        TASK_BITMAP[word_idx] &= !(1u64 << bit_idx);
                        return None;
                    }
                };

                // Initialize Task
                Task::new_task_at(task_ptr, pid, SchedPolicy::Normal);

                return Some(task_ptr);
            }
        }

        // No free slots found
        None
    }
}

/// Calculate slot index from task pointer
fn task_ptr_to_slot_idx(task_ptr: *mut Task) -> Option<usize> {
    unsafe {
        let ptr_addr = task_ptr as usize;
        let pool_start = TASK_POOL.data.as_ptr() as usize;
        let pool_end = pool_start + TASK_POOL_SIZE * TASK_SLOT_SIZE;

        if ptr_addr < pool_start || ptr_addr >= pool_end {
            return None;
        }

        let offset = ptr_addr - pool_start;
        if offset % TASK_SLOT_SIZE != 0 {
            return None;
        }

        Some(offset / TASK_SLOT_SIZE)
    }
}

/// Free task pool slot
///
/// Marks the slot as available for reuse
pub fn free_task_slot(task_ptr: *mut Task) {
    let _lock = TASK_POOL_LOCK.lock();

    if let Some(slot_idx) = task_ptr_to_slot_idx(task_ptr) {
        unsafe {
            let word_idx = slot_idx / 64;
            let bit_idx = slot_idx % 64;
            TASK_BITMAP[word_idx] &= !(1u64 << bit_idx);
        }
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

        // Allocate kernel stack for idle task
        if let Some(stack_top) = (*idle_ptr).alloc_kernel_stack() {
            // Update context.sp to point to stack top
            (*idle_ptr).context_mut().sp = stack_top as u64;
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

unsafe fn __schedule() {
    // Clear need_resched flag
    clear_need_resched();

    // Get current CPU's run queue
    let rq = match this_cpu_rq() {
        Some(r) => r,
        None => return,
    };

    let mut rq_inner = rq.lock();

    // Get current task
    let prev = rq_inner.current;

    if prev.is_null() {
        return;
    }

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

    if next == prev {
        return;
    }

    // Context switch (needs to be done outside lock)
    drop(rq_inner);
    context_switch(&mut *prev, &mut *next);
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
            if (*task_ptr).state() == TaskState::new(TaskState::RUNNING) {
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
    let cpu_id = crate::arch::cpu_id() as u64 as usize;

    // Update current task
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

    // Switch to next's user page table
    if let Some(addr_space) = (*next).address_space() {
        let user_ppn = addr_space.root_ppn();
        let satp_value = (8u64 << 60) | user_ppn;  // Mode=8 (Sv39), ASID=0, PPN=user_ppn

        // Set user page table
        core::arch::asm!(
            "csrw satp, {0}",
            "sfence.vma",
            in(reg) satp_value,
            options(nostack, preserves_flags)
        );
    } else {
        // Task has no address space (e.g., idle task)
        // Switch to kernel page table to ensure kernel code can execute
        let kernel_ppn = crate::arch::mm::get_kernel_page_table_ppn();
        let satp_value = (8u64 << 60) | kernel_ppn;  // Mode=8 (Sv39), ASID=0, PPN=kernel_ppn

        core::arch::asm!(
            "csrw satp, {0}",
            "sfence.vma",
            in(reg) satp_value,
            options(nostack, preserves_flags)
        );
    }

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
        // Debug: Check L0[0x1f] for current process (child after fork)
        if let Some(current) = crate::sched::current() {
            let pid = (*current).pid();
            if pid > 1 {  // Only for child processes (not init)
                if let Some(aspace) = (*current).address_space() {
                    let root_ppn = aspace.pgd;
                    if root_ppn != 0 {
                        let root_addr = root_ppn << 12;
                        let root_table = root_addr as *const u64;
                        let pte2 = core::ptr::read(root_table);
                        if pte2 & 0x1 != 0 {
                            let ppn1 = pte2 >> 10;
                            let l1_addr = ppn1 << 12;
                            let l1_table = l1_addr as *const u64;
                            let pte1 = core::ptr::read(l1_table);
                            if pte1 & 0x1 != 0 {
                                let ppn0 = pte1 >> 10;
                                let l0_addr = ppn0 << 12;
                                let l0_table = l0_addr as *const u64;
                                let l0_pte_1f = core::ptr::read(l0_table.add(0x1f));
                                // Print full page table walk
                                crate::println!("schedule_tail: PID {} root={:#x} L2[0]={:#x} L1_addr={:#x} L1[0]={:#x} L0_addr={:#x} L0[0x1f]={:#x}",
                                    pid, root_addr, pte2, l1_addr, pte1, l0_addr, l0_pte_1f);
                            }
                        }
                    }
                }
            }
        }

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
    let pid = task.pid();
    if let Some(rq) = this_cpu_rq() {
        let mut rq_inner = rq.lock();
        if rq_inner.nr_running < MAX_TASKS {
            let task_ptr = task as *mut Task;

            // Set task state to RUNNING
            task.set_state(TaskState::new(TaskState::RUNNING));

            // Set task's CPU ID (ensure ti_cpu is properly initialized)
            let cpu_id = crate::arch::cpu_id() as i32;
            task.set_ti_cpu(cpu_id);

            // If using CFS, also add to CFS queue
            if rq_inner.use_cfs {
                rq_inner.cfs_rq.enqueue(task_ptr);
            }

            // Also add to traditional queue (compatibility)
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
    if let Some(rq) = this_cpu_rq() {
        let mut rq_inner = rq.lock();
        let task_ptr = task as *const Task as *mut Task;

        // If using CFS, remove from CFS queue
        if rq_inner.use_cfs {
            rq_inner.cfs_rq.dequeue(task_ptr);
        }

        // Remove from traditional queue
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

/// Get current task pointer (O(1) via tp register)
///
/// Uses RISC-V tp (thread pointer) register which holds the current
/// task_struct pointer. This avoids acquiring the runqueue lock.
pub fn current() -> Option<&'static mut Task> {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *mut Task;
    if tp.is_null() {
        None
    } else {
        unsafe { Some(&mut *tp) }
    }
}

/// Get current task PID (O(1) via tp register)
pub fn get_current_pid() -> u32 {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *const Task;
    if tp.is_null() {
        0
    } else {
        unsafe { (*tp).pid() }
    }
}

/// Get current task PPID (O(1) via tp register)
pub fn get_current_ppid() -> u32 {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *const Task;
    if tp.is_null() {
        0
    } else {
        unsafe { (*tp).ppid() }
    }
}

pub unsafe fn find_task_by_pid(pid: Pid) -> *mut Task {
    // Traverse all CPU run queues
    for cpu_id in 0..MAX_CPUS {
        if let Some(rq) = cpu_rq(cpu_id) {
            let rq_inner = rq.lock();
            for i in 0..rq_inner.nr_running {
                let task = rq_inner.tasks[i];
                if !task.is_null() && (*task).pid() == pid {
                    return task;
                }
            }
        }
    }
    core::ptr::null_mut()
}

pub fn get_current_fdtable() -> Option<&'static FdTable> {
    let rq_opt = this_cpu_rq();

    if rq_opt.is_none() {
        return None;
    }

    let rq = rq_opt.unwrap();
    let rq_inner = rq.lock();
    let current = rq_inner.current;

    if current.is_null() {
        return None;
    }

    unsafe { (*current).try_fdtable() }
}

pub fn init_std_fds() {
    use crate::fs::char_dev::{CharDev, CharDevType};

    if let Some(rq) = this_cpu_rq() {
        unsafe {
            let rq_inner = rq.lock();
            let current = rq_inner.current;

            if current.is_null() {
                return;
            }

            // Idle task has no fdtable
            // Note: FdTable has interior mutability, so &FdTable is sufficient
            let fdtable = match (*current).try_fdtable() {
                Some(ft) => ft,
                None => return,
            };

            // Create UART character device
            let uart_dev = CharDev::new(CharDevType::UartConsole, 0);

            // File operations function table
            static UART_OPS: FileOps = FileOps {
                read: Some(uart_file_read),
                write: Some(uart_file_write),
                lseek: None,
                close: None,
            };

            // Create stdin (fd=0)
            let stdin = Arc::new(File::new(FileFlags::new(FileFlags::O_RDONLY)));
            stdin.set_ops(&UART_OPS);
            stdin.set_private_data(&uart_dev as *const CharDev as *mut u8);

            // Create stdout (fd=1)
            let stdout = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
            stdout.set_ops(&UART_OPS);
            stdout.set_private_data(&uart_dev as *const CharDev as *mut u8);

            // Create stderr (fd=2)
            let stderr = Arc::new(File::new(FileFlags::new(FileFlags::O_WRONLY)));
            stderr.set_ops(&UART_OPS);
            stderr.set_private_data(&uart_dev as *const CharDev as *mut u8);

            // Install standard file descriptors
            let _ = fdtable.install_fd(0, stdin);
            let _ = fdtable.install_fd(1, stdout);
            let _ = fdtable.install_fd(2, stderr);
        }
    }
}

fn uart_file_read(file: &File, buf: &mut [u8]) -> isize {
    if let Some(priv_data) = unsafe { *file.private_data.get() } {
        let char_dev = unsafe { &*(priv_data as *const CharDev) };
        unsafe { return char_dev.read(buf.as_mut_ptr(), buf.len()) };
    }
    -9  // EBADF
}

fn uart_file_write(file: &File, buf: &[u8]) -> isize {
    if let Some(priv_data) = unsafe { *file.private_data.get() } {
        let char_dev = unsafe { &*(priv_data as *const CharDev) };
        unsafe { return char_dev.write(buf.as_ptr(), buf.len()) };
    }
    -9  // EBADF
}

// ============================================================================
// Signal Handling
// ============================================================================

pub fn send_signal(pid: Pid, sig: i32) -> Result<(), i32> {
    use crate::signal::Signal;

    // Check if signal number is valid
    if sig < 1 || sig > 64 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    unsafe {
        // Traverse all CPU run queues to find target process
        for cpu_id in 0..MAX_CPUS {
            if let Some(rq) = cpu_rq(cpu_id) {
                let rq_inner = rq.lock();

                for i in 0..MAX_TASKS {
                    let task_ptr = rq_inner.tasks[i];
                    if task_ptr.is_null() {
                        continue;
                    }

                    let task = &*task_ptr;

                    // Check if PID matches
                    if task.pid() != pid {
                        continue;
                    }

                    // SIGKILL and SIGSTOP cannot be ignored
                    if sig == Signal::SIGKILL as i32 || sig == Signal::SIGSTOP as i32 {
                        // Add directly to pending signals
                        task.pending.add(sig);
                        // Wake up sleeping process
                        drop(rq_inner);  // Release lock
                        use crate::signal;
                        signal::signal_wake_up(task_ptr);
                        return Ok(());
                    }

                    // Idle task has no signal handling
                    let signal_ref: &crate::signal::SignalStruct = match task.signal.as_ref() {
                        Some(s) => s,
                        None => {
                            // No signal structure, add directly to pending queue
                            task.pending.add(sig);
                            // Wake up sleeping process
                            drop(rq_inner);  // Release lock
                            use crate::signal;
                            signal::signal_wake_up(task_ptr);
                            return Ok(());
                        }
                    };

                    // Check if signal is masked
                    if signal_ref.is_masked(sig) {
                        return Err(errno::Errno::TryAgain.as_neg_i32());
                    }

                    // Check signal handling action
                    if let Some(action) = signal_ref.get_action(sig) {
                        match action.action() {
                            crate::signal::SigActionKind::Ignore => {
                                return Ok(());  // Ignore signal
                            }
                            crate::signal::SigActionKind::Default => {
                                // Default handling: add to pending queue
                                task.pending.add(sig);
                                // Wake up sleeping process
                                drop(rq_inner);  // Release lock
                                use crate::signal;
                                signal::signal_wake_up(task_ptr);
                                return Ok(());
                            }
                            crate::signal::SigActionKind::Handler => {
                                // User-defined handler: add to pending queue
                                task.pending.add(sig);
                                // Wake up sleeping process
                                drop(rq_inner);  // Release lock
                                use crate::signal;
                                signal::signal_wake_up(task_ptr);
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }

        // Process not found
        Err(errno::Errno::NoSuchProcess.as_neg_i32())
    }
}

pub fn send_signal_self(sig: i32) -> Result<(), i32> {
    let current_pid = get_current_pid();
    send_signal(current_pid, sig)
}

pub fn handle_pending_signals() {

    if let Some(rq) = this_cpu_rq() {
        unsafe {
            let rq_inner = rq.lock();
            let current = rq_inner.current;

            if current.is_null() {
                return;
            }

            // Get first pending signal
            while let Some(sig) = (*current).pending.first() {
                // Get signal handling action
                let signal_ref: &crate::signal::SignalStruct = match (*current).signal.as_ref() {
                    Some(s) => s,
                    None => {
                        // No signal structure, use default handling
                        // Remove signal and continue
                        (*current).pending.remove(sig);
                        continue;
                    }
                };

                let action = signal_ref.get_action(sig).unwrap();

                match action.action() {
                    crate::signal::SigActionKind::Ignore => {
                        // Ignore signal, just remove
                        (*current).pending.remove(sig);
                    }
                    crate::signal::SigActionKind::Default => {
                        // Default handling
                        match sig {
                            15 | 9 => {  // SIGTERM=15, SIGKILL=9
                                // Terminate process
                                (*current).pending.remove(sig);
                                // TODO: Implement process termination
                            }
                            19 => {  // SIGSTOP
                                // Stop process
                                (*current).set_state(TaskState::new(TaskState::STOPPED));
                                (*current).pending.remove(sig);
                            }
                            18 => {  // SIGCONT
                                // Continue process
                                (*current).set_state(TaskState::new(TaskState::RUNNING));
                                (*current).pending.remove(sig);
                            }
                            _ => {
                                // Other signals, remove
                                (*current).pending.remove(sig);
                            }
                        }
                    }
                    crate::signal::SigActionKind::Handler => {
                        // Call user handler
                        // TODO: Implement user-mode signal handler invocation
                        (*current).pending.remove(sig);
                    }
                }

                // If signal was handled, may need to reschedule
                if (*current).state() == TaskState::new(TaskState::STOPPED) {
                    drop(rq_inner);
                    // Release kernel big lock (must release before sleeping)
                    crate::sync::kernel_lock_release();
                    schedule();
                    // Re-acquire kernel big lock after waking up
                    crate::sync::kernel_lock_acquire();
                    break;
                }
            }
        }
    }
}

pub fn check_and_handle_signals() {
    handle_pending_signals();
}

// ============================================================================
// Process Exit and Wait
// ============================================================================

pub fn do_exit(exit_code: i32) -> ! {
    use crate::signal::Signal;

    if let Some(rq) = this_cpu_rq() {
        unsafe {
            let rq_inner = rq.lock();
            let current = rq_inner.current;

            if current.is_null() {
                // No current process, halt directly
                loop {
                    asm!("wfi", options(nomem, nostack));
                }
            }

            let current_pid = (*current).pid();
            let parent_pid = (*current).ppid();

            // Set exit code
            (*current).set_exit_code(exit_code);

            // Set process state to Zombie
            (*current).set_state(TaskState::new(TaskState::ZOMBIE));

            // Remove from run queue
            drop(rq_inner);  // Release lock before calling dequeue_task
            dequeue_task(&*current);

            // Send SIGCHLD signal to parent process and wake up parent
            if parent_pid != 0 {
                let _ = send_signal(parent_pid, Signal::SIGCHLD as i32);

                // Wake up parent process (if parent is blocked waiting in wait4)
                let parent = find_task_by_pid(parent_pid);
                if !parent.is_null() {
                    wake_up_process(parent);
                }
            }

            // Release kernel big lock (must release when process exits, otherwise other processes can't acquire lock)
            crate::sync::kernel_lock_release();

            // Scheduler selects next process to run
            schedule();

            // Never reached here
            loop {
                asm!("wfi", options(nomem, nostack));
            }
        }
    } else {
        // No run queue, halt directly
        loop {
            unsafe {
                asm!("wfi", options(nomem, nostack));
            }
        }
    }
}

pub fn do_wait(pid: i32, status_ptr: *mut i32) -> Result<Pid, i32> {
    unsafe {
        let current = if let Some(rq) = this_cpu_rq() {
            rq.lock().current
        } else {
            return Err(errno::Errno::NoChild.as_neg_i32());
        };

        if current.is_null() {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let current_pid = (*current).pid();

        // If current is idle task (PID 0), no real process is running
        if current_pid == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        // Loop waiting for child process to exit
        loop {
            let mut found_child = false;

            // Traverse all CPU run queues to find zombie child processes
            for cpu_id in 0..MAX_CPUS {
                if let Some(rq) = cpu_rq(cpu_id) {
                    let mut rq_inner = rq.lock();

                    for i in 0..MAX_TASKS {
                        let task_ptr = rq_inner.tasks[i];
                        if task_ptr.is_null() {
                            continue;
                        }

                        let task = &*task_ptr;
                        let task_ppid = task.ppid();

                        // Check if it's a child process
                        if task_ppid != current_pid {
                            continue;
                        }

                        found_child = true;

                        // Check if it's the specified PID (if specified)
                        if pid > 0 && task.pid() != pid as u32 {
                            continue;
                        }

                        // Check if it's in Zombie state
                        if task.state() == TaskState::new(TaskState::ZOMBIE) {
                            let child_pid = task.pid();
                            let exit_code = task.exit_code();

                            // Write exit status
                            if !status_ptr.is_null() {
                                *status_ptr = exit_code;
                            }

                            // Remove from run queue
                            rq_inner.tasks[i] = core::ptr::null_mut();
                            rq_inner.nr_running -= 1;

                            // Free zombie task's resources (address space, fd table, etc.)
                            // Safety: task_ptr is valid and points to a ZOMBIE task
                            unsafe {
                                (*task_ptr).clear_address_space();
                                (*task_ptr).clear_fdtable();
                                (*task_ptr).clear_signal_handlers();
                                (*task_ptr).free_kernel_stack();
                            }

                            // Free PID for reuse
                            crate::process::pid::free_pid(child_pid);

                            // Free task slot for reuse
                            free_task_slot(task_ptr);

                            return Ok(child_pid);
                        }
                    }
                }
            }

            // Has child processes but none have exited yet
            if found_child {
                // Use Task::sleep() to enter interruptible sleep state
                crate::process::Task::sleep(crate::process::task::TaskState::new(TaskState::INTERRUPTIBLE));

                // After waking up, check if signals have arrived
                use crate::signal;
                if signal::signal_pending() {
                    return Err(errno::Errno::InterruptedSystemCall.as_neg_i32());  // EINTR
                }
            } else {
                // No child processes
                return Err(errno::Errno::NoChild.as_neg_i32());
            }
        }
    }
}

pub fn do_wait_nonblock(pid: i32, status_ptr: *mut i32) -> Result<Pid, i32> {
    unsafe {
        let current = if let Some(rq) = this_cpu_rq() {
            rq.lock().current
        } else {
            // No runqueue, means uninitialized, return ECHILD directly
            return Err(errno::Errno::NoChild.as_neg_i32());
        };

        if current.is_null() {
            // current is null (possibly called from non-process context), return ECHILD
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let current_pid = (*current).pid();

        // If current is idle task (PID 0), no real process is running
        // Return ECHILD because idle task has no child processes
        if current_pid == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let mut found_child = false;

        // Traverse all CPU run queues to find zombie child processes
        for cpu_id in 0..MAX_CPUS {
            if let Some(rq) = cpu_rq(cpu_id) {
                let mut rq_inner = rq.lock();

                for i in 0..MAX_TASKS {
                    let task_ptr = rq_inner.tasks[i];
                    if task_ptr.is_null() {
                        continue;
                    }

                    let task = &*task_ptr;

                    // Check if it's a child process
                    if task.ppid() != current_pid {
                        continue;
                    }

                    found_child = true;

                    // Check if it's the specified PID (if specified)
                    if pid > 0 && task.pid() != pid as u32 {
                        continue;
                    }

                    // Check if it's in Zombie state
                    if task.state() == TaskState::new(TaskState::ZOMBIE) {
                        let child_pid = task.pid();
                        let exit_code = task.exit_code();

                        // Write exit status
                        if !status_ptr.is_null() {
                            *status_ptr = exit_code;
                        }

                        // Remove from run queue
                        rq_inner.tasks[i] = core::ptr::null_mut();
                        rq_inner.nr_running -= 1;

                        // Free zombie task's resources
                        (*task_ptr).clear_address_space();
                        (*task_ptr).clear_fdtable();
                        (*task_ptr).clear_signal_handlers();
                        (*task_ptr).free_kernel_stack();

                        // Free PID for reuse
                        crate::process::pid::free_pid(child_pid);

                        // Free task slot for reuse
                        free_task_slot(task_ptr);

                        return Ok(child_pid);
                    }
                }
            }
        }

        // Has child processes but none have exited yet
        if found_child {
            // Return EAGAIN (-11), sys_wait4 will convert it to 0
            Err(errno::Errno::TryAgain.as_neg_i32())
        } else {
            // No child processes
            // Return ECHILD (-10)
            Err(errno::Errno::NoChild.as_neg_i32())
        }
    }
}

// ============================================================================
// Load Balancing Mechanism
// ============================================================================

fn rq_load(rq: &RunQueue) -> usize {
    // Total load = legacy tasks + CFS tasks + RT tasks + DL tasks
    let cfs_load = if rq.use_cfs { rq.cfs_rq.nr_running() as usize } else { 0 };
    let rt_load = rq.rt.nr_running() as usize;
    let dl_load = rq.dl.nr_running() as usize;
    rq.nr_running + cfs_load + rt_load + dl_load
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
            return Some(task);
        }
    }

    // Try stealing from RT queue (only if not currently running)
    if !src_rq.rt.is_empty() {
        if let Some(task) = src_rq.rt.pick_next() {
            // Don't steal currently running task
            if task != src_rq.current {
                return Some(task);
            }
        }
    }

    // Try stealing from legacy queue
    // Search from tail (least recently run tasks)
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

        // Found migratable task
        // Remove from source queue
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
                    rq.cfs_rq.enqueue(task);
                } else {
                    // Fall back to legacy queue
                    if rq.nr_running >= MAX_TASKS {
                        return;
                    }
                    rq.tasks[rq.nr_running] = task;
                    rq.nr_running += 1;
                }
            }
            SchedPolicy::Idle => {
                // Idle tasks are never enqueued
            }
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
