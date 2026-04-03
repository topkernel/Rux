//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Scheduler Implementation — Global RunQueue Design
//!
//! # Architecture
//!
//! One global run queue (GRQ) shared by all CPUs, with per-class sub-queues:
//!   - DL queue (BTreeMap, EDF)
//!   - RT queue (bitmap + per-priority lists)
//!   - CFS queue (BTreeMap, vruntime)
//!
//! Per-CPU state is minimal: just `current`, `idle`, `stop` task pointers.
//!
//! # Scheduling Flow
//!
//!   schedule() → lock GRQ → pick_next_task() → unlock GRQ → context_switch()
//!
//! No per-CPU load balancing or stealing is needed — the global queue is
//! inherently balanced.  CPUs pull tasks on demand; idle CPUs are woken via IPI.
//!
//! # Note
//!
//! Raw pointers are used to avoid borrow checker limitations, which is common
//! practice in OS kernel development.

use crate::errno;
use crate::process::task::{Task, TaskState, SchedPolicy, Pid};
use crate::arch;
use crate::println;
use crate::config::MAX_CPUS;
use alloc::boxed::Box;
use crate::process::pid::alloc_pid;
use core::arch::asm;
use crate::sync::spinlock::RawSpinlock;

use crate::config::MAX_TASKS;

// ==================== Global RunQueue ====================

/// Global run queue — one instance for the whole system.
///
/// Protected by a single `RawSpinlock`.  All enqueue / dequeue / pick_next
/// operations must hold this lock.  Timer-tick updates (`scheduler_tick`)
/// only touch the per-CPU `current` task and do NOT need the lock.
pub struct GlobalRunQueue {
    /// Protects dl_rq, rt_rq, cfs_rq, nr_running
    lock: RawSpinlock,
    /// Deadline queue (EDF sorted by deadline)
    pub dl_rq: crate::sched::deadline::DlRunQueue,
    /// Real-time queue (bitmap + per-priority lists)
    pub rt_rq: crate::sched::rt::RtRunQueue,
    /// CFS queue (BTreeMap sorted by vruntime)
    pub cfs_rq: crate::sched::fair::CfsRunQueue,
    /// Total runnable task count (read atomically for idle checks)
    pub nr_running: core::sync::atomic::AtomicUsize,
    /// Bitmap of idle CPUs: bit N = 1 means CPU N is idle
    idle_cpus: core::sync::atomic::AtomicU32,
}

unsafe impl Sync for GlobalRunQueue {}

impl GlobalRunQueue {
    /// Create a new GlobalRunQueue (NOT const — BTreeMap::new() is not const).
    fn new() -> Self {
        Self {
            lock: RawSpinlock::new(),
            dl_rq: crate::sched::deadline::DlRunQueue::new(),
            rt_rq: {
                let mut rt = crate::sched::rt::RtRunQueue::new();
                rt.init();
                rt
            },
            cfs_rq: crate::sched::fair::CfsRunQueue::new(),
            nr_running: core::sync::atomic::AtomicUsize::new(0),
            idle_cpus: core::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Lock the global RQ (disable interrupts + preempt + lock).
    #[inline]
    pub fn lock_irqsave(&self) -> GrqGuard<'_> {
        let flags = crate::arch::riscv64::cpu::save_and_disable_irq();
        crate::interrupt::preempt::preempt_count_add(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
        self.lock.lock();
        GrqGuard { grq: self as *const Self as *mut Self, flags, _marker: core::marker::PhantomData }
    }

    /// Lock without IRQ save (for non-interrupt contexts like init).
    #[inline]
    pub fn lock_plain(&self) -> GrqPlainGuard<'_> {
        crate::interrupt::preempt::preempt_count_add(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
        self.lock.lock();
        GrqPlainGuard { grq: self as *const Self as *mut Self, _marker: core::marker::PhantomData }
    }

    // ---- idle CPU bitmap ----

    /// Mark a CPU as idle.
    pub fn mark_idle(&self, cpu: usize) {
        if cpu < MAX_CPUS {
            self.idle_cpus.fetch_or(1u32 << cpu, core::sync::atomic::Ordering::Release);
        }
    }

    /// Mark a CPU as busy (no longer idle).  Returns true if it was idle.
    pub fn clear_idle(&self, cpu: usize) -> bool {
        if cpu < MAX_CPUS {
            let mask = 1u32 << cpu;
            let prev = self.idle_cpus.fetch_and(!mask, core::sync::atomic::Ordering::AcqRel);
            (prev & mask) != 0
        } else {
            false
        }
    }

    /// Find an idle CPU in the given affinity mask.
    pub fn find_idle_cpu(&self, affinity: u32) -> Option<usize> {
        let idle = self.idle_cpus.load(core::sync::atomic::Ordering::Acquire);
        let candidates = idle & affinity;
        if candidates == 0 {
            return None;
        }
        Some(candidates.trailing_zeros() as usize)
    }

    /// Total load across all classes (for informational purposes).
    pub fn rq_load(&self) -> usize {
        let cfs = self.cfs_rq.nr_running() as usize;
        let rt = self.rt_rq.nr_running() as usize;
        let dl = self.dl_rq.nr_running() as usize;
        cfs + rt + dl
    }
}

// ==================== GRQ Guards ====================

/// Guard for `lock_irqsave()` — unlock + preempt enable + IRQ restore on drop.
pub struct GrqGuard<'a> {
    grq: *mut GlobalRunQueue,
    flags: bool,
    _marker: core::marker::PhantomData<&'a GlobalRunQueue>,
}

impl<'a> GrqGuard<'a> {
    /// Release the spinlock but keep interrupts disabled.
    /// Returns the saved IRQ flags. Caller must call restore_irq() later.
    pub fn unlock_irqretain(self) -> bool {
        let flags = self.flags;
        unsafe { (*self.grq).lock.unlock() };
        crate::interrupt::preempt::preempt_count_sub(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
        core::mem::forget(self);
        flags
    }
}

impl core::ops::Deref for GrqGuard<'_> {
    type Target = GlobalRunQueue;
    fn deref(&self) -> &Self::Target {
        // Safety: we hold the lock, so access is safe.
        unsafe { &*self.grq }
    }
}

impl core::ops::DerefMut for GrqGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: we hold the lock, so exclusive access is guaranteed.
        unsafe { &mut *self.grq }
    }
}

impl Drop for GrqGuard<'_> {
    fn drop(&mut self) {
        unsafe { (*self.grq).lock.unlock() };
        crate::interrupt::preempt::preempt_count_sub(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
        crate::arch::riscv64::cpu::restore_irq(self.flags);
    }
}

/// Guard for plain `lock_plain()` (no IRQ save).
pub struct GrqPlainGuard<'a> {
    grq: *mut GlobalRunQueue,
    _marker: core::marker::PhantomData<&'a GlobalRunQueue>,
}

impl core::ops::Deref for GrqPlainGuard<'_> {
    type Target = GlobalRunQueue;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.grq }
    }
}

impl core::ops::DerefMut for GrqPlainGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.grq }
    }
}

impl Drop for GrqPlainGuard<'_> {
    fn drop(&mut self) {
        unsafe { (*self.grq).lock.unlock() };
        crate::interrupt::preempt::preempt_count_sub(
            crate::interrupt::preempt::PREEMPT_OFFSET,
        );
    }
}

// ==================== Per-CPU State ====================

/// Minimal per-CPU state — no queues, just task pointers.
pub struct PerCpuState {
    /// Currently running task
    pub current: *mut Task,
    /// Per-CPU idle task (PID 0)
    pub idle: *mut Task,
    /// Per-CPU stop task (for hotplug)
    pub stop: *mut Task,
}

impl PerCpuState {
    const fn new() -> Self {
        Self {
            current: core::ptr::null_mut(),
            idle: core::ptr::null_mut(),
            stop: core::ptr::null_mut(),
        }
    }
}

// ==================== Static Instances ====================

/// Global run queue — MaybeUninit because GlobalRunQueue::new() is not const.
static mut GRQ: core::mem::MaybeUninit<GlobalRunQueue> = core::mem::MaybeUninit::uninit();

/// Whether GRQ has been initialized
static GRQ_READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Initialize the global run queue (called once during boot).
unsafe fn grq_init() {
    if GRQ_READY.load(core::sync::atomic::Ordering::Acquire) {
        return;
    }
    GRQ = core::mem::MaybeUninit::new(GlobalRunQueue::new());
    GRQ_READY.store(true, core::sync::atomic::Ordering::Release);
}

/// Get a shared reference to GRQ.
fn grq() -> &'static GlobalRunQueue {
    debug_assert!(GRQ_READY.load(core::sync::atomic::Ordering::Acquire));
    unsafe { GRQ.assume_init_ref() }
}

/// Per-CPU state array (indexed by cpu_id)
static mut PER_CPU: [PerCpuState; MAX_CPUS] = [
    PerCpuState::new(),
    PerCpuState::new(),
    PerCpuState::new(),
    PerCpuState::new(),
];

/// Per-CPU RQ initialization flags
static RQ_INITIALIZED: [core::sync::atomic::AtomicBool; MAX_CPUS] = [
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
];

/// Per-CPU reschedule flags
static mut NEED_RESCHED: [core::sync::atomic::AtomicBool; MAX_CPUS] = [
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
];

/// Per-CPU idle task storage
static mut IDLE_TASK_STORAGES: [core::mem::MaybeUninit<Task>; MAX_CPUS] = [
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
];

// ==================== Reschedule Flags ====================

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

pub fn resched_curr() {
    set_need_resched();
}

/// Send reschedule IPI to target CPU
pub fn resched_cpu(cpu: usize) {
    unsafe {
        if cpu < MAX_CPUS {
            NEED_RESCHED[cpu].store(true, core::sync::atomic::Ordering::Release);
            let this_cpu = crate::arch::cpu_id() as usize;
            if this_cpu != cpu {
                #[cfg(feature = "riscv64")]
                crate::arch::ipi::send_reschedule_ipi(cpu);
            }
        }
    }
}

// ==================== Per-CPU Accessors ====================

/// Get per-CPU state for the current CPU.
#[inline]
fn this_cpu() -> &'static PerCpuState {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        &PER_CPU[cpu_id.min(MAX_CPUS - 1)]
    }
}

/// Get per-CPU state for the current CPU (mutable).
#[inline]
fn this_cpu_mut() -> &'static mut PerCpuState {
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        &mut PER_CPU[cpu_id.min(MAX_CPUS - 1)]
    }
}

/// Get per-CPU state for a specific CPU.
#[inline]
fn cpu_state(cpu_id: usize) -> &'static PerCpuState {
    unsafe { &PER_CPU[cpu_id.min(MAX_CPUS - 1)] }
}

/// Get per-CPU state for a specific CPU (mutable).
#[inline]
fn cpu_state_mut(cpu_id: usize) -> &'static mut PerCpuState {
    unsafe { &mut PER_CPU[cpu_id.min(MAX_CPUS - 1)] }
}

// ==================== Dummy RunQueue (compatibility) ====================

/// Dummy RunQueue for compatibility with SchedClass trait / procfs output.
pub struct RunQueue;

// ==================== Initialization ====================

pub fn init_per_cpu_rq(cpu_id: usize) {
    if cpu_id >= MAX_CPUS {
        return;
    }
    if RQ_INITIALIZED[cpu_id].load(core::sync::atomic::Ordering::Acquire) {
        return;
    }

    // Initialize the global RQ once
    unsafe { grq_init(); }

    RQ_INITIALIZED[cpu_id].store(true, core::sync::atomic::Ordering::Release);
}

pub fn init_secondary(cpu_id: usize) {
    if cpu_id >= MAX_CPUS || cpu_id == 0 {
        return;
    }

    init_per_cpu_rq(cpu_id);

    unsafe {
        let idle_ptr = IDLE_TASK_STORAGES[cpu_id].as_mut_ptr();
        Task::new_idle_at(idle_ptr);

        crate::process::pid_hash::pid_hash_insert(idle_ptr);

        if let Some(stack_top) = (*idle_ptr).alloc_kernel_stack() {
            (*idle_ptr).thread_mut().sp = stack_top as u64;
        }

        (*idle_ptr).set_ti_cpu(cpu_id as i32);

        core::arch::asm!("csrw sscratch, zero");
        core::arch::asm!("mv tp, {0}", in(reg) idle_ptr);

        let pcpu = cpu_state_mut(cpu_id);
        pcpu.idle = idle_ptr;
        pcpu.current = idle_ptr;
    }
}

pub fn init() {
    let cpu_id = crate::arch::cpu_id() as u64 as usize;

    if cpu_id >= MAX_CPUS {
        println!("sched: init: invalid cpu_id {}", cpu_id);
        return;
    }

    init_per_cpu_rq(cpu_id);

    unsafe {
        let idle_ptr = IDLE_TASK_STORAGES[cpu_id].as_mut_ptr();
        Task::new_idle_at(idle_ptr);

        crate::process::pid_hash::pid_hash_insert(idle_ptr);

        if let Some(stack_top) = (*idle_ptr).alloc_kernel_stack() {
            (*idle_ptr).thread_mut().sp = stack_top as u64;
        } else {
            println!("sched: failed to allocate kernel stack for idle task");
        }

        (*idle_ptr).set_ti_cpu(cpu_id as i32);

        core::arch::asm!("csrw sscratch, zero");
        core::arch::asm!("mv tp, {0}", in(reg) idle_ptr);

        let pcpu = cpu_state_mut(cpu_id);
        pcpu.idle = idle_ptr;
        pcpu.current = idle_ptr;
    }
}

// ==================== Task Allocation ====================

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

pub fn free_task_slot(task_ptr: *mut Task) {
    if task_ptr.is_null() {
        return;
    }
    unsafe {
        alloc::alloc::dealloc(task_ptr as *mut u8, core::alloc::Layout::new::<Task>());
    }
}

// ==================== Core Scheduling ====================

pub fn wake_up_process(task: *mut Task) -> bool {
    use crate::process::Task;
    Task::wake_up(task)
}

#[inline(never)]
pub fn schedule() {
    unsafe {
        __schedule();
    }
}

#[no_mangle]
pub extern "C" fn asm_schedule() {
    schedule();
}

unsafe fn __schedule() {
    clear_need_resched();

    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    let prev = this_cpu().current;

    if prev.is_null() {
        return;
    }

    let prev_pid = (*prev).pid();

    // Lock the global RQ
    let mut grq_guard = grq().lock_irqsave();

    crate::pr_debug!("sched: __schedule cpu={} prev={} nr_running={}",
        cpu_id, prev_pid, grq_guard.nr_running.load(core::sync::atomic::Ordering::Relaxed));

    // Update CFS runtime for current task (if it's a CFS task)
    let prev_policy = (*prev).policy();
    if prev_policy == SchedPolicy::Normal
        || prev_policy == SchedPolicy::Batch
        || prev_policy == SchedPolicy::Idle
    {
        let now = crate::sched::fair::sched_clock();
        grq_guard.cfs_rq.update_curr(now);
    }

    // Re-enqueue prev if still runnable and not idle
    if (*prev).state() == TaskState::new(TaskState::RUNNING) && prev_pid != 0 {
        enqueue_task_locked(&mut *grq_guard, prev);
    }

    // Pick next task
    let next = pick_next_task(&mut *grq_guard, cpu_id);

    if !next.is_null() && next != prev {
        crate::pr_debug!("sched: pick_next cpu={} {} -> {}",
            cpu_id, prev_pid, (*next).pid());
    }

    // Update per-CPU current under lock
    this_cpu_mut().current = next;

    // Clear idle bit since we're about to run something
    grq().clear_idle(cpu_id);

    // Release lock but keep IRQs disabled for context_switch
    let flags = grq_guard.unlock_irqretain();

    if next == prev {
        crate::arch::riscv64::cpu::restore_irq(flags);
        return;
    }

    if !next.is_null() {
        context_switch(&mut *prev, &mut *next);
    }

    // context_switch returns here in prev's context (when scheduled back).
    crate::arch::riscv64::cpu::restore_irq(flags);
}

/// Pick the next task to run on this CPU.
///
/// Checks in strict priority order: stop → DL → RT → CFS → idle.
/// Respects CPU affinity (cpus_allowed).
unsafe fn pick_next_task(grq: &mut GlobalRunQueue, cpu_id: usize) -> *mut Task {
    let pcpu = cpu_state(cpu_id);

    // 1. Stop task (per-CPU, highest priority)
    if !pcpu.stop.is_null() {
        return pcpu.stop;
    }

    // 2. Deadline — pick earliest-deadline task that can run on this CPU
    if !grq.dl_rq.is_empty() {
        if let Some(task) = grq.dl_rq.pick_next_cpu(cpu_id) {
            return task;
        }
    }

    // 3. RT — pick highest-priority task that can run on this CPU
    if !grq.rt_rq.is_empty() {
        if let Some(task) = grq.rt_rq.pick_next_cpu(cpu_id) {
            return task;
        }
    }

    // 4. CFS — pick min-vruntime task that can run on this CPU
    if !grq.cfs_rq.is_empty() {
        if let Some(task) = grq.cfs_rq.pick_next_cpu(cpu_id) {
            grq.cfs_rq.set_curr(task);
            let se = (*task).sched_entity();
            let slice_ns = grq.cfs_rq.sched_slice(se);
            let slice_ms = crate::sched::fair::sched_slice_to_ms(slice_ns);
            (*task).set_time_slice(slice_ms.max(1) as u32);
            return task;
        }
    }

    // 5. Nothing runnable → idle task
    pcpu.idle
}

/// Enqueue a task into the global RQ (called with GRQ lock held).
unsafe fn enqueue_task_locked(grq: &mut GlobalRunQueue, task: *mut Task) {
    if task.is_null() {
        return;
    }

    let policy = (*task).policy();

    // Set task state to RUNNING
    (*task).set_state(TaskState::new(TaskState::RUNNING));

    match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            grq.rt_rq.enqueue(task, false);
        }
        SchedPolicy::Deadline => {
            let now = crate::sched::fair::sched_clock();
            (*task).dl_entity().update_deadline(now);
            (*task).dl_entity().replenish_runtime();
            grq.dl_rq.enqueue(task);
        }
        SchedPolicy::Normal | SchedPolicy::Batch => {
            grq.cfs_rq.enqueue(task);
        }
        SchedPolicy::Idle => {
            // SCHED_IDLE uses CFS with low weight
            let se = (*task).sched_entity_mut();
            se.load.weight = 3;
            se.load.inv_weight = 0;
            grq.cfs_rq.enqueue(task);
        }
    }

    grq.nr_running.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Enqueue a task and try to wake an idle CPU.
pub fn enqueue_task(task: &'static mut Task) {
    let task_ptr = task as *mut Task;
    let cpus_allowed = task.cpus_allowed();
    let this_cpu = crate::arch::cpu_id() as usize;

    // Assign CPU if unassigned
    if task.ti_cpu() as usize >= MAX_CPUS {
        task.set_ti_cpu(this_cpu as i32);
    }

    // Lock GRQ and enqueue
    let mut grq_guard = grq().lock_irqsave();
    unsafe {
        enqueue_task_locked(&mut *grq_guard, task_ptr);
    }

    // Check for cross-CPU preemption (RT/DL)
    let policy = task.policy();
    if policy == SchedPolicy::Fifo || policy == SchedPolicy::Rr {
        check_rt_preempt(task_ptr, cpus_allowed);
    } else if policy == SchedPolicy::Deadline {
        check_dl_preempt(task_ptr, cpus_allowed);
    }

    drop(grq_guard);

    // Try to wake an idle CPU
    if let Some(idle_cpu) = grq().find_idle_cpu(cpus_allowed) {
        grq().clear_idle(idle_cpu);
        resched_cpu(idle_cpu);
    } else if cpus_allowed & (1u32 << this_cpu) == 0 {
        // Task can't run on this CPU — IPI a CPU that can
        let target = cpus_allowed.trailing_zeros() as usize;
        if target < MAX_CPUS && target != this_cpu {
            resched_cpu(target);
        }
    }
}

/// Check if a newly-enqueued RT task should preempt a running task on another CPU.
fn check_rt_preempt(task: *mut Task, cpus_allowed: u32) {
    unsafe {
        let task_prio = (*task).rt_priority();
        for cpu in 0..MAX_CPUS {
            if (cpus_allowed & (1u32 << cpu)) == 0 {
                continue;
            }
            let running = cpu_state(cpu).current;
            if running.is_null() || running == cpu_state(cpu).idle {
                continue;
            }
            let r_policy = (*running).policy();
            if r_policy == SchedPolicy::Normal
                || r_policy == SchedPolicy::Batch
                || r_policy == SchedPolicy::Idle
            {
                resched_cpu(cpu);
                return;
            }
            if (r_policy == SchedPolicy::Fifo || r_policy == SchedPolicy::Rr)
                && (*running).rt_priority() > task_prio
            {
                resched_cpu(cpu);
                return;
            }
        }
    }
}

/// Check if a newly-enqueued DL task should preempt a running task on another CPU.
fn check_dl_preempt(task: *mut Task, cpus_allowed: u32) {
    unsafe {
        let task_dl = (*task).dl_entity().deadline.load(core::sync::atomic::Ordering::Acquire);
        for cpu in 0..MAX_CPUS {
            if (cpus_allowed & (1u32 << cpu)) == 0 {
                continue;
            }
            let running = cpu_state(cpu).current;
            if running.is_null() || running == cpu_state(cpu).idle {
                continue;
            }
            let r_policy = (*running).policy();
            if r_policy != SchedPolicy::Deadline {
                resched_cpu(cpu);
                return;
            }
            let r_dl = (*running).dl_entity().deadline.load(core::sync::atomic::Ordering::Acquire);
            if task_dl < r_dl {
                resched_cpu(cpu);
                return;
            }
        }
    }
}

/// Dequeue a task from the global RQ.
pub fn dequeue_task(task: &Task) {
    let task_ptr = task as *const Task as *mut Task;
    let policy = task.policy();

    let mut grq_guard = grq().lock_irqsave();

    match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            grq_guard.rt_rq.dequeue(task_ptr);
        }
        SchedPolicy::Deadline => {
            grq_guard.dl_rq.dequeue(task_ptr);
        }
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            grq_guard.cfs_rq.dequeue(task_ptr);
        }
    }

    grq_guard.nr_running.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
}

// ==================== Scheduler Tick ====================

pub fn scheduler_tick() {
    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    crate::dfx::softlockup::touch(cpu_id);

    let current = this_cpu().current;
    if current.is_null() {
        return;
    }

    unsafe {
        let policy = (*current).policy();

        match policy {
            SchedPolicy::Normal | SchedPolicy::Batch => {
                let now = crate::sched::fair::sched_clock();

                // Update vruntime under GRQ lock
                {
                    let mut grq_guard = grq().lock_irqsave();
                    grq_guard.cfs_rq.update_curr(now);
                }

                let curr_vruntime = {
                    let se = (*current).sched_entity();
                    se.get_vruntime()
                };

                // Check if preemption is needed
                let grq_guard = grq().lock_irqsave();
                if let Some(next) = grq_guard.cfs_rq.peek_next() {
                    if !next.is_null() && next != current {
                        let next_vruntime = {
                            let next_se = (*next).sched_entity();
                            next_se.get_vruntime()
                        };
                        if curr_vruntime > next_vruntime {
                            let delta = curr_vruntime - next_vruntime;
                            if delta > crate::sched::fair::SCHED_MIN_GRANULARITY_NS {
                                drop(grq_guard);
                                set_need_resched();
                            }
                        }
                    }
                }
            }
            SchedPolicy::Rr => {
                let rt_entity = (*current).rt_entity();
                let remaining = rt_entity.dec_time_slice();
                if remaining == 0 {
                    rt_entity.reset_time_slice();
                    let mut grq_guard = grq().lock_irqsave();
                    grq_guard.rt_rq.enqueue(current, false);
                    drop(grq_guard);
                    set_need_resched();
                }
            }
            SchedPolicy::Fifo => {
                // FIFO: no time slice management
            }
            SchedPolicy::Deadline => {
                let now = crate::sched::fair::sched_clock();
                let dl_entity = (*current).dl_entity();
                let delta = now - dl_entity.exec_start.load(core::sync::atomic::Ordering::Relaxed);
                dl_entity.exec_start.store(now, core::sync::atomic::Ordering::Release);
                if !dl_entity.consume_runtime(delta) {
                    set_need_resched();
                }
            }
            SchedPolicy::Idle => {
                // SCHED_IDLE: treated like fair
            }
        }
    }
}

// ==================== Context Switch ====================

unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    let cpu_id = crate::arch::cpu_id() as u64 as usize;

    (*next).set_ti_cpu(cpu_id as i32);

    if (*next).is_fork_child() {
        (*next).clear_fork_child();
    }

    drop(&mut *next);
    crate::arch::context::context_switch(prev, next);
}

#[no_mangle]
pub extern "C" fn schedule_tail(prev: *mut Task) {
    unsafe {
        if !prev.is_null() {
            // Rux: cleanup after task switch
        }
    }
}

// ==================== Utility Functions ====================

pub fn yield_cpu() {
    crate::sync::kernel_lock_release();
    schedule();
    crate::sync::kernel_lock_acquire();
}

/// Iterate over all tasks via PID hash table.
pub fn for_each_task<F>(f: F)
where
    F: Fn(*mut Task),
{
    // Iterate all running CPUs + global RQ tasks
    unsafe {
        for cpu in 0..MAX_CPUS {
            let pcpu = cpu_state(cpu);
            if !pcpu.current.is_null() {
                f(pcpu.current);
            }
            if !pcpu.idle.is_null() && pcpu.idle != pcpu.current {
                f(pcpu.idle);
            }
        }
    }
}

pub fn current() -> Option<&'static mut Task> {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *mut Task;
    if tp.is_null() || (tp as usize) < 0x80000000 {
        None
    } else {
        unsafe { Some(&mut *tp) }
    }
}

pub fn get_current_pid() -> u32 {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *const Task;
    if tp.is_null() || (tp as usize) < 0x80000000 {
        0
    } else {
        unsafe { (*tp).pid() }
    }
}

pub fn get_current_ppid() -> u32 {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *const Task;
    if tp.is_null() || (tp as usize) < 0x80000000 {
        0
    } else {
        unsafe { (*tp).ppid() }
    }
}

pub unsafe fn find_task_by_pid(pid: Pid) -> *mut Task {
    crate::process::pid_hash::pid_hash_lookup(pid)
}

/// Load balance — no-op with global RQ (inherently balanced).
pub fn load_balance() {
    // No-op: global queue is always balanced
}

/// Compatibility stubs — no longer meaningful with global RQ
pub fn this_cpu_rq() -> Option<&'static crate::sync::spinlock::Spinlock<RunQueue>> {
    None
}

pub fn cpu_rq(_cpu_id: usize) -> Option<&'static crate::sync::spinlock::Spinlock<RunQueue>> {
    None
}

// ==================== CPU Idle Loop ====================

pub fn cpu_idle_loop() -> ! {
    use crate::arch;

    if !crate::arch::riscv64::smp::is_boot_hart() {
        crate::arch::riscv64::trap::enable_timer_interrupt();
    }

    let cpu_id = crate::arch::cpu_id() as u64 as usize;

    loop {
        // 1. Try to pick a task from the global RQ
        unsafe {
            schedule();
        }

        // 2. Check if we're still running idle
        let is_idle = {
            let pcpu = this_cpu();
            let curr = pcpu.current;
            !curr.is_null() && unsafe { (*curr).pid() == 0 }
        };

        if is_idle {
            // Mark this CPU as idle
            grq().mark_idle(cpu_id);

            // 3. Enter WFI — wait for interrupt (IPI will wake us)
            unsafe {
                asm!("wfi", options(nomem, nostack));
            }

            // Woken up — clear idle and loop back to schedule()
            grq().clear_idle(cpu_id);
        }
    }
}
