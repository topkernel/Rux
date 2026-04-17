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
    /// Expose total nr_running for diagnostic use.
    pub fn grq_nr_running() -> usize {
        grq().rq_load()
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
        // SAFETY: we hold the GRQ lock via this guard, so the unlock is valid.
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
        // SAFETY: we hold the GRQ lock via this guard, so shared access is valid.
        unsafe { &*self.grq }
    }
}

impl core::ops::DerefMut for GrqGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: we hold the GRQ lock via this guard, so exclusive access is guaranteed.
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
        // SAFETY: we hold the GRQ lock via this guard, so shared access is valid.
        unsafe { &*self.grq }
    }
}

impl core::ops::DerefMut for GrqPlainGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: we hold the GRQ lock via this guard, so exclusive access is guaranteed.
        unsafe { &mut *self.grq }
    }
}

impl Drop for GrqPlainGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: we hold the GRQ lock via this guard; Drop runs exactly once.
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
    // SAFETY: called exactly once during boot on the primary CPU before any
    // concurrent access; GRQ_READY flag serializes initialization.
    if GRQ_READY.compare_exchange(
        false, true,
        core::sync::atomic::Ordering::AcqRel,
        core::sync::atomic::Ordering::Acquire,
    ).is_err() {
        // Another CPU is initializing; spin until done
        while !GRQ_READY.load(core::sync::atomic::Ordering::Acquire) {
            core::hint::spin_loop();
        }
        return;
    }
    GRQ = core::mem::MaybeUninit::new(GlobalRunQueue::new());
}

/// Get a shared reference to GRQ.
fn grq() -> &'static GlobalRunQueue {
    if !GRQ_READY.load(core::sync::atomic::Ordering::Acquire) {
        panic!("GRQ accessed before initialization");
    }
    // SAFETY: GRQ_READY is checked above; if true, GRQ was fully initialized by grq_init().
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

// ==================== Deferred Exit Notification ====================
//
// When a task exits (do_exit), it must notify its parent (SIGCHLD +
// wake_up).  However, waking the parent BEFORE schedule() creates a
// race: the parent can reap (free_task_slot) the exiting task before
// schedule() switches it away, causing use-after-free corruption.
//
// Solution: store the parent PID in a per-CPU slot; __schedule
// processes it AFTER the context switch, when the exiting task is
// guaranteed to no longer run on any CPU.

/// Per-CPU deferred exit-notification parent PID (0 = no pending notification).
static DEFERRED_EXIT_NOTIFY_PID: [core::sync::atomic::AtomicI32; MAX_CPUS] = [
    const { core::sync::atomic::AtomicI32::new(0) },
    const { core::sync::atomic::AtomicI32::new(0) },
    const { core::sync::atomic::AtomicI32::new(0) },
    const { core::sync::atomic::AtomicI32::new(0) },
];

/// Defer sending SIGCHLD to `parent_pid` until after the next context switch.
///
/// Called from `do_exit` *before* `schedule()`.  The notification is
/// delivered by `__schedule` once the exiting task has been switched away.
pub fn defer_exit_notify(parent_pid: u32) {
    let cpu = arch::cpu_id() as usize;
    if cpu < MAX_CPUS {
        DEFERRED_EXIT_NOTIFY_PID[cpu].store(parent_pid as i32, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Process the deferred exit notification (if any) for the current CPU.
///
/// Must be called AFTER `context_switch` so the exiting task is no longer
/// running on any CPU when we wake the parent.
fn process_deferred_exit_notify() {
    let cpu = arch::cpu_id() as usize;
    if cpu >= MAX_CPUS {
        return;
    }
    let pid = DEFERRED_EXIT_NOTIFY_PID[cpu].load(core::sync::atomic::Ordering::Relaxed);
    if pid <= 0 {
        return;
    }
    // Clear the slot (consume the notification).
    DEFERRED_EXIT_NOTIFY_PID[cpu].store(0, core::sync::atomic::Ordering::Relaxed);

    use crate::signal::Signal;
    let _ = crate::signal::send_signal(pid as u32, Signal::SIGCHLD as i32);

    let parent = crate::process::pid_hash::pid_hash_lookup(pid as u32);
    if !parent.is_null() {
        // SAFETY: parent was obtained from pid_hash_lookup and is a valid Task
        // pointer (PID hash table entries are not freed until release_task).
        unsafe {
            (*parent).wait_chldexit.wake_up_all();
        }
    }
}

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
    // SAFETY: cpu_id is bounds-checked against MAX_CPUS before array access;
    // NEED_RESCHED elements are AtomicBool, safe for concurrent reads.
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
    // SAFETY: cpu_id is bounds-checked against MAX_CPUS; only the current CPU
    // writes its own NEED_RESCHED entry (AtomicBool).
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        if cpu_id < MAX_CPUS {
            NEED_RESCHED[cpu_id].store(true, core::sync::atomic::Ordering::Release);
        }
    }
}

#[inline]
fn clear_need_resched() {
    // SAFETY: cpu_id is bounds-checked; only the current CPU clears its own flag.
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
    // SAFETY: cpu is bounds-checked against MAX_CPUS; NEED_RESCHED[cpu] is AtomicBool.
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
    // SAFETY: cpu_id is bounds-checked against MAX_CPUS; each CPU only reads its own slot.
    unsafe {
        let id = crate::arch::cpu_id() as u64 as usize;
        if id >= MAX_CPUS {
            panic!("this_cpu: cpu_id {} >= MAX_CPUS {}", id, MAX_CPUS);
        }
        &PER_CPU[id]
    }
}

/// Get per-CPU state for the current CPU (mutable).
#[inline]
fn this_cpu_mut() -> &'static mut PerCpuState {
    // SAFETY: cpu_id is clamped to [0, MAX_CPUS-1]; only the current CPU mutates its own slot.
    unsafe {
        let cpu_id = crate::arch::cpu_id() as u64 as usize;
        &mut PER_CPU[cpu_id.min(MAX_CPUS - 1)]
    }
}

/// Get per-CPU state for a specific CPU.
#[inline]
pub fn cpu_state(cpu_id: usize) -> &'static PerCpuState {
    // SAFETY: cpu_id is clamped to [0, MAX_CPUS-1]; PER_CPU is a static array.
    unsafe { &PER_CPU[cpu_id.min(MAX_CPUS - 1)] }
}

/// Get per-CPU state for a specific CPU (mutable).
#[inline]
fn cpu_state_mut(cpu_id: usize) -> &'static mut PerCpuState {
    // SAFETY: cpu_id is clamped to [0, MAX_CPUS-1]; caller must ensure no aliasing.
    unsafe { &mut PER_CPU[cpu_id.min(MAX_CPUS - 1)] }
}

/// Check if a CPU is online (has been assigned an idle task).
#[inline]
pub fn cpu_online(cpu: usize) -> bool {
    cpu < MAX_CPUS && !cpu_state(cpu).idle.is_null()
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
    // SAFETY: grq_init() is idempotent (checks GRQ_READY); called during boot.
    unsafe { grq_init(); }

    RQ_INITIALIZED[cpu_id].store(true, core::sync::atomic::Ordering::Release);
}

pub fn init_secondary(cpu_id: usize) {
    if cpu_id >= MAX_CPUS {
        return;
    }

    init_per_cpu_rq(cpu_id);

    // SAFETY: cpu_id is bounds-checked; IDLE_TASK_STORAGES[cpu_id] is a per-CPU
    // MaybeUninit<Task> only written during secondary CPU init; Task::new_idle_at
    // initializes the Task in-place before any pointer dereferences.
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

    // SAFETY: cpu_id is bounds-checked above; IDLE_TASK_STORAGES[cpu_id] is a
    // per-CPU MaybeUninit<Task> only written during boot init.
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
    // SAFETY: Layout is non-zero (Task is sized); null check follows immediately.
    let task_ptr = unsafe { alloc::alloc::alloc(layout) } as *mut Task;
    if task_ptr.is_null() {
        return None;
    }

    let pid = match alloc_pid() {
        Some(p) => p,
        None => {
            // SAFETY: task_ptr was allocated above with the same Layout; not yet initialized.
            unsafe { alloc::alloc::dealloc(task_ptr as *mut u8, core::alloc::Layout::new::<Task>()); }
            return None;
        }
    };

    // SAFETY: task_ptr was freshly allocated with Layout::new::<Task>() and is non-null;
    // new_task_at initializes it in-place before use.
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
    // SAFETY: task_ptr was allocated by alloc_task_slot with Layout::new::<Task>();
    // null check above; caller must ensure no other references exist.
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
    // SAFETY: __schedule() manipulates raw task pointers and calls context_switch;
    // must be called from kernel context with valid current task pointer.
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

    // Context switch is an RCU quiescent state.
    crate::sync::rcu::rcu_note_context_switch();

    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    let prev = this_cpu().current;

    if prev.is_null() {
        return;
    }

    let prev_pid = (*prev).pid();

    // Fast path: if current task is idle and no tasks are runnable,
    // skip the GRQ lock entirely.  This avoids idle CPUs spinning on
    // the lock with IRQs disabled (via lock_irqsave), which would
    // prevent timer ticks from updating the soft-lockup timestamp.
    if prev_pid == 0 && GlobalRunQueue::grq_nr_running() == 0 {
        return;
    }

    // Lock the global RQ
    let mut grq_guard = grq().lock_irqsave();

    // Update runtime accounting for current task based on its scheduling class.
    // CFS: update vruntime; DL: consume runtime budget.
    let prev_policy = (*prev).policy();
    if prev_policy == SchedPolicy::Normal
        || prev_policy == SchedPolicy::Batch
        || prev_policy == SchedPolicy::Idle
    {
        let now = crate::sched::fair::sched_clock();
        grq_guard.cfs_rq.update_curr(now);
    } else if prev_policy == SchedPolicy::Deadline {
        // Update DL runtime accounting
        let dl = (*prev).dl_entity();
        let now = crate::sched::fair::sched_clock();
        let exec_start = dl.exec_start.load(core::sync::atomic::Ordering::Acquire);
        if exec_start != 0 && now > exec_start {
            let delta = now - exec_start;
            dl.consume_runtime(delta);
        }
        dl.exec_start.store(now, core::sync::atomic::Ordering::Release);
    }

    // Deactivate prev: dequeue from the correct class-specific runqueue.
    let prev_running = (*prev).state() == TaskState::new(TaskState::RUNNING);
    if !prev_running && prev_pid != 0 {
        match prev_policy {
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                grq_guard.cfs_rq.dequeue(prev);
            }
            SchedPolicy::Fifo | SchedPolicy::Rr => {
                grq_guard.rt_rq.dequeue(prev);
            }
            SchedPolicy::Deadline => {
                grq_guard.dl_rq.dequeue(prev);
            }
        }
    }

    // Re-enqueue prev if still runnable and not idle
    if prev_running && prev_pid != 0 {
        enqueue_task_locked(&mut *grq_guard, prev);
    }

    // Pick next task
    let next = pick_next_task(&mut *grq_guard, cpu_id);

    // Capture next_pid while we still hold references (before unlock)
    let next_pid = if !next.is_null() { (*next).pid() } else { 0 };

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

    // SAFETY: Runnable tasks cannot be freed while still on a CPU or runqueue.
    // IRQs remain disabled on this CPU, preventing concurrent scheduling.
    if !next.is_null() {
        context_switch(&mut *prev, &mut *next);
    }

    // After context_switch, the NEW task is running.  The exiting task
    // (prev) is no longer on any CPU, so it is now safe to notify its
    // parent (SIGCHLD + wake_up).  The deferred notification was stored
    // in a per-CPU slot by do_exit → defer_exit_notify().
    process_deferred_exit_notify();

    // We must ensure interrupts are enabled so that timer ticks, wake-ups,
    // and I/O completions can be delivered. The previous task's saved IRQ
    // state (flags) is irrelevant here — the new task needs SIE=1.
    //
    // This is critical when schedule() is called from syscall context
    // where SIE=0 (cleared by hardware on trap entry). Without this,
    // restore_irq(false) leaves the new task with SIE=0, preventing
    // any interrupts until sret — which may never happen if the new
    // task blocks again.
    //
    // Matches Linux behavior: __schedule() always returns with
    // interrupts enabled in the calling context.
    crate::arch::riscv64::cpu::restore_irq(true);
}

/// Pick the next task to run on this CPU.
///
/// Checks in strict priority order: stop → DL → RT → CFS → idle.
/// Respects CPU affinity (cpus_allowed).
unsafe fn pick_next_task(grq: &mut GlobalRunQueue, cpu_id: usize) -> *mut Task {
    let pcpu = cpu_state(cpu_id);

    // 1. Stop task (per-CPU, highest priority)
    // TODO: Stop task support - need has_work() check when implemented
    // if !pcpu.stop.is_null() {
    //     return pcpu.stop;
    // }

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
            se.load = crate::sched::fair::LoadWeight::new(crate::sched::fair::WEIGHT_IDLEPRIO);
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
    // SAFETY: GRQ lock is held via grq_guard; enqueue_task_locked expects the lock to be held.
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
    // SAFETY: task is a valid pointer from enqueue_task; cpu_state(cpu).current/idle
    // are valid pointers set during init; null checks before dereference.
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
    // SAFETY: task is a valid pointer from enqueue_task; per-CPU current/idle
    // pointers are valid when not null (set during CPU init).
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

    let actually_dequeued = match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            grq_guard.rt_rq.dequeue(task_ptr);
            true // RT dequeue always succeeds (no bool return)
        }
        SchedPolicy::Deadline => {
            grq_guard.dl_rq.dequeue(task_ptr);
            true
        }
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            grq_guard.cfs_rq.dequeue(task_ptr)
        }
    };

    if actually_dequeued {
        grq_guard.nr_running.fetch_update(
            core::sync::atomic::Ordering::SeqCst,
            core::sync::atomic::Ordering::SeqCst,
            |v| v.checked_sub(1),
        );
    }
}

// ==================== Scheduler Tick ====================

pub fn scheduler_tick() {
    let cpu_id = crate::arch::cpu_id() as u64 as usize;
    crate::dfx::softlockup::touch(cpu_id);

    // Poll UART for pending data — MUST be before current check.
    if crate::console::uart_has_data() {
        crate::console::read_waitq().wake_up_one();
    }

    // Update load average (auto-throttled to every 5 seconds).
    crate::fs::procfs::loadavg::update_load_avg();

    let current = this_cpu().current;
    if current.is_null() {
        return;
    }

    // SAFETY: current is this_cpu().current, a valid Task pointer set during CPU init;
    // null check above; we only touch fields appropriate for the current CPU's task.
    unsafe {
        let policy = (*current).policy();

        match policy {
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                let now = crate::sched::fair::sched_clock();

                // Single GRQ lock acquisition: update vruntime + check
                // preemption together.  Previously this was two separate
                // lock/unlock cycles, which doubled the contention window
                // and caused soft lockups when other CPUs held the lock.
                let should_resched = {
                    let mut grq_guard = grq().lock_irqsave();
                    grq_guard.cfs_rq.update_curr(now);

                    let curr_vruntime = {
                        let se = (*current).sched_entity();
                        se.get_vruntime()
                    };

                    if let Some(next) = grq_guard.cfs_rq.peek_next() {
                        if !next.is_null() && next != current {
                            let next_vruntime = {
                                let next_se = (*next).sched_entity();
                                next_se.get_vruntime()
                            };
                            if curr_vruntime > next_vruntime {
                                let delta = curr_vruntime - next_vruntime;
                                delta > crate::sched::fair::SCHED_MIN_GRANULARITY_NS
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }; // grq_guard dropped here

                if should_resched {
                    set_need_resched();
                }
            }
            SchedPolicy::Rr => {
                let rt_entity = (*current).rt_entity();
                let remaining = rt_entity.dec_time_slice();
                if remaining == 0 {
                    rt_entity.reset_time_slice();
                    // Ensure task state is RUNNING before re-enqueue,
                    // otherwise a concurrently set INTERRUPTIBLE state would
                    // place a sleeping task on the runqueue.
                    unsafe { (*current).set_state(TaskState::new(TaskState::RUNNING)); }
                    let mut grq_guard = grq().lock_irqsave();
                    grq_guard.rt_rq.enqueue(current, false);
                    set_need_resched(); // Set before dropping lock to prevent lost wake-up
                    drop(grq_guard);
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

    crate::arch::context::context_switch(prev, next);
}

#[no_mangle]
pub extern "C" fn schedule_tail(_prev: *mut Task) {
    // Called after context_switch in the new task's context.
    // Placeholder for per-task post-switch setup (e.g., RCU, tick).
}

// ==================== Utility Functions ====================

pub fn yield_cpu() {
    schedule();
}

/// Iterate over all tasks via PID hash table.
pub fn for_each_task<F>(f: F)
where
    F: Fn(*mut Task),
{
    // SAFETY: per-CPU current/idle pointers are set during CPU init; null check
    // before calling f(); f() receives the raw pointer but does not dereference.
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
        // SAFETY: tp is the tp register set by context_switch to a valid Task pointer;
        // null and low-address checks above; this CPU has exclusive mutable access to its current task.
        unsafe { Some(&mut *tp) }
    }
}

pub fn get_current_pid() -> u32 {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *const Task;
    if tp.is_null() || (tp as usize) < 0x80000000 {
        0
    } else {
        // SAFETY: tp is the current task pointer from tp register; null and address checks above.
        unsafe { (*tp).pid() }
    }
}

pub fn get_current_ppid() -> u32 {
    let tp = crate::arch::riscv64::cpu::get_thread_id() as *const Task;
    if tp.is_null() || (tp as usize) < 0x80000000 {
        0
    } else {
        // SAFETY: tp is the current task pointer from tp register; null and address checks above.
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
        // Ensure IRQs are enabled before each schedule() call.
        // When the idle task is switched back to (from a task that called
        // schedule() with SIE=0, e.g. from syscall context), __schedule's
        // restore_irq restores SIE=0. Without re-enabling here, no timer
        // interrupts would fire and the system would be stuck.
        // This matches the reference implementation where the idle loop's
        // cpuidle enables IRQs before entering the idle state.
        crate::arch::riscv64::cpu::restore_irq(true);

        // 1. Try to pick a task from the global RQ
        // SAFETY: called from idle task context; schedule() handles its own locking.
        let my_cpu = crate::arch::cpu_id() as usize;
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
            grq().mark_idle(cpu_id);
            crate::sync::rcu::rcu_note_context_switch();

            // Poll UART for pending data before entering WFI.
            if crate::console::uart_has_data() {
                crate::console::read_waitq().wake_up_one();
            }

            // Enter WFI to halt CPU until next interrupt (timer, UART, IPI).
            // IRQs must be enabled (SIE=1) so timer ticks and wake-ups arrive.
            unsafe { crate::arch::riscv64::cpu::wfi(); }

            grq().clear_idle(cpu_id);
        }
    }
}
