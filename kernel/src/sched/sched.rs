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
        // R11-4: single-slot overwrite lost notifications when two tasks
        // exited on the same CPU before a context switch (echo + cat in a
        // pipeline) — the parent's SIGCHLD vanished and mrsh's wait hung.
        // Fire any pending predecessor INLINE (we are in syscall context,
        // post-preempt-enable; send_signal + wake are safe here) before
        // taking the slot.
        let old = DEFERRED_EXIT_NOTIFY_PID[cpu].swap(parent_pid as i32, core::sync::atomic::Ordering::Relaxed);
        if old > 0 && old as u32 != parent_pid {
            process_deferred_exit_pid(old as u32);
        }
    }
}

/// Process the deferred exit notification (if any) for the current CPU.
///
/// Must be called AFTER `context_switch` so the exiting task is no longer
/// running on any CPU when we wake the parent.
/// Process deferred exit notification for a specific CPU.
/// Called with the CPU that the exiting task was running on (captured
/// before context_switch), because cpu_id() returns the new task's
/// CPU after the switch.
/// R11-4: deliver a specific pending notify inline (slot chaining).
fn process_deferred_exit_pid(parent_pid: u32) {
    use crate::signal::Signal;
    let _ = crate::signal::send_signal(parent_pid, Signal::SIGCHLD as i32);
    let parent = crate::process::pid_hash::pid_hash_lookup_pinned(parent_pid);
    if !parent.is_null() {
        let _woken = unsafe { (*parent).wait_chldexit.wake_up_all() };
        crate::process::task::Task::task_put(parent);
    }
}

fn process_deferred_exit_notify_cpu(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    let pid = DEFERRED_EXIT_NOTIFY_PID[cpu].load(core::sync::atomic::Ordering::Relaxed);
    if pid <= 0 {
        return;
    }
    // Clear the slot (consume the notification).
    DEFERRED_EXIT_NOTIFY_PID[cpu].store(0, core::sync::atomic::Ordering::Relaxed);

    let parent = crate::process::pid_hash::pid_hash_lookup_pinned(pid as u32);
    if !parent.is_null() {
        // SAFETY: parent was obtained from pid_hash_lookup and is a valid Task
        // pointer (PID hash table entries are not freed until release_task).
        unsafe {
            // Migrate a SLEEPING parent to the current CPU before waking it:
            // wake_up → select_task_rq reads ti_cpu, so steering it here
            // lets this CPU's idle task pick it up immediately.
            // NEVER touch ti_cpu of a task that is still running or already
            // queued on another CPU — cpu_id() reads tp→ti_cpu on that CPU,
            // and hijacking it corrupts the victim's per-CPU view
            // (review 2R.15).
            // R13-1 (root cause of S-A AND S-B): is_sleeping() is TRUE
            // during the whole prepare_to_wait -> schedule() window while
            // the parent is STILL EXECUTING on another CPU. Steering its
            // ti_cpu then poisons cpu_id() (a tp->ti_cpu FIELD read) for
            // the rest of its kernel path: the next __schedule resolves
            // prev from the WRONG per-CPU slot and switches context
            // against a different task — two tasks, one kernel stack;
            // zeroed stack locals (the NULL+0x30 CAS) and mixed trap
            // frames (the jump-to-user-address) both fall out. on_cpu()
            // is exactly "picked, context not yet saved": true inside
            // that window (skip), false once genuinely switched out
            // (safe to steer).
            if (*parent).state().is_sleeping() && !(*parent).on_cpu() {
                (*parent).set_ti_cpu(cpu as i32);
            }
        }
        crate::process::task::Task::task_put(parent);
    }

    use crate::signal::Signal;
    let _ = crate::signal::send_signal(pid as u32, Signal::SIGCHLD as i32);

    // R9-12: re-lookup after send_signal — the captured pointer crossed a
    // signal-delivery call during which the (zombie) parent could have been
    // reaped and freed on another CPU; operating on the fresh lookup (or
    // none) closes the narrow UAF.
    let parent_fresh = crate::process::pid_hash::pid_hash_lookup_pinned(pid as u32);
    if !parent_fresh.is_null() {
        unsafe {
            let _woken = (*parent_fresh).wait_chldexit.wake_up_all();
        }
        crate::process::task::Task::task_put(parent_fresh);
    }
}

/// Per-CPU idle task storage
static mut IDLE_TASK_STORAGES: [core::mem::MaybeUninit<Task>; MAX_CPUS] = [
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
    core::mem::MaybeUninit::uninit(),
];

/// Per-CPU idle task pointers, accessed by boot.S to set tp on secondary CPUs.
/// Written once by sched::init() on the boot CPU, read by secondary_start in
/// assembly before any C code runs.  0 means not yet initialized.
#[no_mangle]
static mut __secondary_idle_tasks: [usize; MAX_CPUS] = [0; MAX_CPUS];

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

    // tp was already set to the idle task pointer by boot.S (loaded from
    // __secondary_idle_tasks).  Just register with the per-CPU state.
    unsafe {
        let idle_ptr = IDLE_TASK_STORAGES[cpu_id].as_ptr() as *mut Task;

        // Verify tp matches the pre-created idle task
        let current_tp: usize;
        core::arch::asm!("mv {}, tp", out(reg) current_tp);
        debug_assert_eq!(current_tp, idle_ptr as usize,
            "init_secondary: tp mismatch! tp={:#x}, expected={:#x}", current_tp, idle_ptr as usize);

        core::arch::asm!("csrw sscratch, zero");

        let pcpu = cpu_state_mut(cpu_id);
        pcpu.idle = idle_ptr;
        pcpu.current = idle_ptr;
    }
}

pub fn init() {
    let boot_cpu = crate::arch::cpu_id() as u64 as usize;

    if boot_cpu >= MAX_CPUS {
        println!("sched: init: invalid cpu_id {}", boot_cpu);
        return;
    }

    // Pre-create idle tasks for ALL CPUs (Linux model).
    // This must happen before start_secondaries() so that boot.S can
    // load tp from __secondary_idle_tasks before any C code runs on
    // secondary harts, allowing safe early interrupt enable.
    for cpu in 0..MAX_CPUS {
        init_per_cpu_rq(cpu);

        // SAFETY: cpu is bounds-checked (loop 0..MAX_CPUS);
        // IDLE_TASK_STORAGES[cpu] is a per-CPU MaybeUninit<Task>
        // only written once during init.
        unsafe {
            let idle_ptr = IDLE_TASK_STORAGES[cpu].as_mut_ptr();
            Task::new_idle_at(idle_ptr);

            crate::process::pid_hash::pid_hash_insert(idle_ptr);

            if let Some(stack_top) = (*idle_ptr).alloc_kernel_stack() {
                (*idle_ptr).thread_mut().sp = stack_top as u64;
            } else {
                println!("sched: failed to allocate kernel stack for idle task cpu {}", cpu);
            }

            (*idle_ptr).set_ti_cpu(cpu as i32);

            // Store pointer for assembly secondary_start
            __secondary_idle_tasks[cpu] = idle_ptr as usize;

            if cpu == boot_cpu {
                core::arch::asm!("csrw sscratch, zero");
                core::arch::asm!("mv tp, {0}", in(reg) idle_ptr);

                let pcpu = cpu_state_mut(cpu);
                pcpu.idle = idle_ptr;
                pcpu.current = idle_ptr;
            }
        }
    }
}

// ==================== Task Allocation ====================

/// R17-B: live-Task-page ownership bitmap — any heap free covering a page
/// that carries a live Task (marked at alloc, unmarked at free) is refused
/// and reported. Proved the free side clean in round 17; kept as the
/// permanent tripwire for the alloc-side handoff hunt.
static TASK_PAGE_OWNED: [core::sync::atomic::AtomicU64; 128] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 128];

#[inline]
fn task_page_mark(ptr: *mut u8) {
    let page = (ptr as usize) >> 12;
    let w = &TASK_PAGE_OWNED[(page >> 6) & 127];
    w.fetch_or(1u64 << (page & 63), core::sync::atomic::Ordering::AcqRel);
}
#[inline]
fn task_page_unmark(ptr: *mut u8) {
    let page = (ptr as usize) >> 12;
    let w = &TASK_PAGE_OWNED[(page >> 6) & 127];
    w.fetch_and(!(1u64 << (page & 63)), core::sync::atomic::Ordering::AcqRel);
}
/// Set while alloc_task_slot itself is allocating, so the heap's
/// alloc-side probe (R18-1) does not flag the legitimate first handoff.
pub static IN_TASK_ALLOC: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// True if the page carrying `ptr` is marked as a live Task page.
#[inline]
pub fn task_page_is_owned(ptr: *const u8) -> bool {
    let page = (ptr as usize) >> 12;
    let w = &TASK_PAGE_OWNED[(page >> 6) & 127];
    w.load(core::sync::atomic::Ordering::Acquire) & (1u64 << (page & 63)) != 0
}

pub fn alloc_task_slot() -> Option<*mut Task> {
    let layout = core::alloc::Layout::new::<Task>();
    IN_TASK_ALLOC.store(true, core::sync::atomic::Ordering::Release);
    // SAFETY: Layout is non-zero (Task is sized); null check follows immediately.
    let task_ptr = unsafe { alloc::alloc::alloc(layout) } as *mut Task;
    IN_TASK_ALLOC.store(false, core::sync::atomic::Ordering::Release);
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
        if !Task::new_task_at(task_ptr, pid, SchedPolicy::Normal) {
            // R10-9: stack allocation failed — discard the slot cleanly.
            crate::process::pid::free_pid(pid);
            unsafe { alloc::alloc::dealloc(task_ptr as *mut u8, core::alloc::Layout::new::<Task>()); }
            return None;
        }
        crate::process::pid_hash::pid_hash_insert(task_ptr);
    }

    Some(task_ptr)
}

/// R12-4: recently-freed Task slots (pid at free time) for post-mortems.
pub static FREED_TASK_RING: [core::sync::atomic::AtomicU32; 64] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 64];
static FREED_RING_HEAD: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// R12-4: poison marker written into a freed Task's state/pid words.
pub const TASK_POISON: u32 = 0xDEAD_BEEF;

pub fn free_task_slot(task_ptr: *mut Task) {
    if task_ptr.is_null() {
        return;
    }
        // R12-4: poison + record before the free — the next "zeroed/garbage
        // linked child" or wild-pointer wake then shows this exact marker
        // instead of anonymous zeros, proving (or ruling out) the
        // freed-while-referenced family at first sight.
        unsafe {
            let slot = task_ptr as *mut u8;
            let h = FREED_RING_HEAD.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % 64;
            FREED_TASK_RING[h].store((*task_ptr).pid(), core::sync::atomic::Ordering::Relaxed);
            // R47: poison state and pid at their COMPILE-TIME offsets
            // (task_offsets::TASK_STATE/TASK_PID; currently 0x50/0x54). The
            // previous hardcoded `slot.add(0x48)` u64 write predated the
            // journal_handle/ti_on_cpu/task_refcnt insertion before `state`
            // (the same +8 drift trap.rs R20 fixed for its own walk) and
            // landed entirely inside ti_a2 — so every poison reader
            // (Task::wake_up R15-6, enqueue_task_locked R15-6,
            // ensure_linked_locked R41, trap.rs panic walk) checked fields
            // that were never poisoned. Freed Task pages were
            // indistinguishable from live ones, and the stale wakes those
            // guards exist to drop proceeded to set_state(RUNNING == 0)
            // into freed-then-reused pages — the positional 4-byte zero at
            // the state word behind the phantom-RUNNING / recurrence-at-
            // same-address signature (rounds 39-47 form-B). Two separate
            // u32 writes (not one u64) so a future field inserted between
            // state and pid cannot silently miss pid again.
            core::ptr::write_volatile(
                slot.add(crate::process::task::TASK_STATE) as *mut u32,
                TASK_POISON,
            );
            core::ptr::write_volatile(
                slot.add(crate::process::task::TASK_PID) as *mut u32,
                TASK_POISON,
            );
            alloc::alloc::dealloc(slot, core::alloc::Layout::new::<Task>());
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
    if prev_pid == 0 {
        let nr = GlobalRunQueue::grq_nr_running();
        if nr == 0 {
            return;
        }
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
        // R20-6: cfs_rq.curr is a SINGLE global slot while several CPUs can
        // run CFS tasks concurrently — the old unconditional update_curr()
        // charged the slot's task once per running CPU (double charge) and
        // never charged the other running tasks at all (frozen vruntime →
        // permanent leftmost hog). Only the slot owner charges through
        // update_curr; everyone else charges its own prev directly.
        if grq_guard.cfs_rq.get_curr() == prev {
            grq_guard.cfs_rq.update_curr(now);
        } else {
            let se = (*prev).sched_entity();
            let delta = se.update_exec_runtime(now);
            if delta > 0 {
                se.update_vruntime(delta);
            }
        }
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
                // R8-1: unconditional — see dequeue_task() fix.
                if grq_guard.cfs_rq.get_curr() == prev {
                    grq_guard.cfs_rq.set_curr(core::ptr::null_mut());
                }
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
        // R41: prev MUST leave this section linked on a class queue (or be
        // verifiably already linked) — a refused insert here is the B-form
        // phantom (RUNNING, unlinked, never picked; see requeue_prev_locked).
        // The old plain enqueue_task_locked call trusted the class on_rq
        // guard; a stale-true flag dropped prev permanently.
        requeue_prev_locked(&mut *grq_guard, prev);
    }

    // Pick next task (R8-1b: prev is passed so the switching CPU may
    // re-pick itself through the next == prev fast path).
    let next = pick_next_task(&mut *grq_guard, cpu_id, prev);

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

    // Note: do NOT capture the CPU before context_switch. __schedule's frame
    // after context_switch belongs to the task being switched IN (coroutine
    // semantics), so a pre-captured cpu_id would be the CPU where THAT task
    // was switched out previously — not this CPU. After the switch, tp
    // points at `next` whose ti_cpu was just set to the hardware CPU by
    // context_switch, so a fresh cpu_id() read is the exiting task's CPU.

    // SAFETY: Runnable tasks cannot be freed while still on a CPU or runqueue.
    // IRQs remain disabled on this CPU, preventing concurrent scheduling.
    if !next.is_null() {
        context_switch(&mut *prev, &mut *next);
    }

    // After context_switch, the NEW task is running.  The exiting task
    // (prev) is no longer on any CPU, so it is now safe to notify its
    // parent (SIGCHLD + wake_up).  The deferred notification was stored
    // in a per-CPU slot by do_exit → defer_exit_notify() on THIS CPU.
    process_deferred_exit_notify_cpu(crate::arch::cpu_id() as usize);

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
unsafe fn pick_next_task(grq: &mut GlobalRunQueue, cpu_id: usize, prev: *mut Task) -> *mut Task {
    let pcpu = cpu_state(cpu_id);

    // 1. Stop task (per-CPU, highest priority)
    // TODO: Stop task support - need has_work() check when implemented
    // if !pcpu.stop.is_null() {
    //     return pcpu.stop;
    // }

    // 2. Deadline — pick earliest-deadline task that can run on this CPU
    if !grq.dl_rq.is_empty() {
        if let Some(task) = grq.dl_rq.pick_next_cpu(cpu_id, prev) {
            mark_picked_on_cpu(task);
            // R32-DL1: reset the CBS execution clock at pick (mirror of the
            // CFS R20-6 fix). exec_start otherwise still holds the
            // timestamp of the task's PREVIOUS switch-out (or 0 before its
            // first run), so the first scheduler_tick/__schedule charge
            // after a wake billed the entire sleep duration (or the whole
            // uptime) into the runtime budget and throttled the task
            // instantly.
            (*task).dl_entity().exec_start.store(
                crate::sched::fair::sched_clock(),
                core::sync::atomic::Ordering::Release,
            );
            // R7-B5: pick removes the task from the queue — pair the count.
            grq.nr_running.fetch_update(
                core::sync::atomic::Ordering::SeqCst,
                core::sync::atomic::Ordering::SeqCst,
                |v| v.checked_sub(1),
            );
            return task;
        }
    }

    // 3. RT — pick highest-priority task that can run on this CPU
    if !grq.rt_rq.is_empty() {
        if let Some(task) = grq.rt_rq.pick_next_cpu(cpu_id, prev) {
            mark_picked_on_cpu(task);
            grq.nr_running.fetch_update(
                core::sync::atomic::Ordering::SeqCst,
                core::sync::atomic::Ordering::SeqCst,
                |v| v.checked_sub(1),
            );
            return task;
        }
    }

    // 4. CFS — pick min-vruntime task that can run on this CPU
    if !grq.cfs_rq.is_empty() {
        if let Some(task) = grq.cfs_rq.pick_next_cpu(cpu_id, prev) {
            mark_picked_on_cpu(task);
            grq.nr_running.fetch_update(
                core::sync::atomic::Ordering::SeqCst,
                core::sync::atomic::Ordering::SeqCst,
                |v| v.checked_sub(1),
            );
            grq.cfs_rq.set_curr(task);
            // R20-6: reset the execution clock at pick. Nothing else resets
            // exec_start, so after a sleep it still holds the timestamp of
            // the task's PREVIOUS run — the first update_curr/direct charge
            // after wakeup billed the entire sleep duration into vruntime.
            (*task).sched_entity().set_exec_start(crate::sched::fair::sched_clock());
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

/// Mark a picked task on-CPU (R8-1b). Called under the GRQ lock; __switch_to
/// clears it after the task's outgoing context is saved. Other CPUs' picks
/// skip on-CPU tasks, so a RUNNING-but-unsaved task can never be picked
/// twice — the NEW2 root cause. The idle task is never marked (it is not
/// on the class queues; clearing a never-set flag is a harmless no-op).
#[inline]
unsafe fn mark_picked_on_cpu(task: *mut Task) {
    if !task.is_null() {
        (*task).set_on_cpu(true);
    }
}

/// R41 (stale on_rq defense, shared): after a REFUSED enqueue_task_locked,
/// find out whether the refusal hid a stale on_rq=true on an UNLINKED task
/// and heal it. Returns true when the task is linked when this returns —
/// either it verifiably already was (legitimate refusal: RR tick rotation,
/// a racing wake, change_task_policy's re-enqueue — a forced insert there
/// would double-link), or the heal cleared the stale flag and re-inserted
/// through the normal path.
///
/// The R41 audit found every on_rq write point in the tree link/unlink-
/// paired under the GRQ lock (fair.rs enqueue/dequeue/pick*, rt.rs
/// enqueue/dequeue, deadline.rs enqueue/dequeue/pick*), so no CURRENT-code
/// path manufactures a stale on_rq=true — it can only arrive from a pre-R39
/// legacy state, an out-of-scheduler memory-corruption engine (this
/// kernel's documented smash families), or a future regression. This
/// defense makes both wedge morphologies unconstructible regardless of
/// provenance: the B-form (prev preempted while phantom-flagged → RUNNING,
/// unlinked, never picked; round-41 capture: pid 359/360 of
/// `echo PP | cat`, 4 CPUs wfi-idle, stacks frozen in
/// schedule → __schedule → context_switch) via requeue_prev_locked, and
/// the A-form twin (wake of a phantom-flagged sleeper refused → the
/// wait-queue's one-shot wake token silently consumed) via
/// wake_up_enqueue. The tripwire print turns every incident into a loud,
/// self-healing event that pins the manufacturer for the follow-up hunt.
///
/// Called with the GRQ lock held, only on the refused path.
unsafe fn ensure_linked_locked(grq: &mut GlobalRunQueue, task: *mut Task) -> bool {
    // Re-run the guards that legitimately refuse (freed-page poison,
    // exiting task) BEFORE any field read below — a freed page cannot be
    // healed, and the policy/entity reads must not touch it.
    if (*task).pid() == TASK_POISON || (*task).state().is_dead() {
        return false;
    }

    // Check ACTUAL linkage — never trust the flag that just lied.
    let policy = (*task).policy();
    let linked = match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => grq.rt_rq.is_linked(task),
        SchedPolicy::Deadline => grq.dl_rq.is_linked(task),
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            grq.cfs_rq.is_linked(task)
        }
    };
    if linked {
        return true; // legitimate refusal: already queued exactly once
    }

    // Stale on_rq=true on an unlinked task — the wedge precursor. Report
    // (SBI direct write: we hold the GRQ lock — R34 discipline), clear the
    // stale flag, and re-insert through the normal path.
    {
        const MSG: &[u8] = b"R41-STALE-ONRQ healed pid=0x";
        for &b in MSG {
            sbi_rt::legacy::console_putchar(b as usize);
        }
        let v = (*task).pid() as u64;
        let mut sh = 64;
        while sh > 0 {
            sh -= 4;
            let nb = ((v >> sh) & 0xF) as u8;
            sbi_rt::legacy::console_putchar(
                (if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }) as usize,
            );
        }
        sbi_rt::legacy::console_putchar(b'\n' as usize);
    }
    match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            (*task).rt_entity().set_on_rq(false);
        }
        SchedPolicy::Deadline => {
            (*task).dl_entity().on_rq.store(
                false,
                core::sync::atomic::Ordering::Release,
            );
        }
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            (*task).sched_entity().set_on_rq(false);
        }
    }
    // With the flag cleared the class insert is admitted, and its success
    // pairs set_state(RUNNING) + grq.nr_running with the actual linkage
    // (R7-B5 discipline).
    enqueue_task_locked(grq, task)
}

/// R41 (B-form phantom — prev re-enqueue): requeue `prev` for its upcoming
/// switch-out. `prev` is this CPU's current with state==RUNNING; once the
/// context_switch below stores its context it is on NO cpu, so it MUST be
/// linked on a class queue when this returns. A plain enqueue_task_locked
/// call trusted the class on_rq guard — a stale-true flag dropped prev
/// permanently (RUNNING-but-unlinked, never picked again). On refusal the
/// shared ensure_linked_locked verifies actual linkage and heals only the
/// genuine divergence, so legitimately-linked prevs (RR rotation, racing
/// wake, policy-change re-enqueue) are never double-linked.
///
/// Called with the GRQ lock held.
unsafe fn requeue_prev_locked(grq: &mut GlobalRunQueue, prev: *mut Task) {
    if enqueue_task_locked(grq, prev) {
        return; // normal path: fresh insert linked prev (state already RUNNING)
    }
    // Refused — verify and heal (no-op skip when prev is already linked).
    ensure_linked_locked(grq, prev);
}

/// Enqueue a task into the global RQ (called with GRQ lock held).
///
/// R39: returns whether the task was actually inserted by this call. False
/// covers the freed-task poison, the exiting-task guard, AND the class
/// enqueues' own on_rq double-enqueue guard (rt/fair/dl all return false
/// when it skips — R7-B5 depends on that to keep grq.nr_running balanced),
/// which is unreachable for every legitimate caller state. On false the
/// task's state is left untouched (SLEEPING stays wakeable; a prev that was
/// already RUNNING stays RUNNING — it is on_cpu, so the divergence is the
/// pre-existing stale-on_rq form, not manufactured here).
unsafe fn enqueue_task_locked(grq: &mut GlobalRunQueue, task: *mut Task) -> bool {
    if task.is_null() {
        return false;
    }

    // R15-6 (S-R resurrection guard): refuse to enqueue a FREED Task.
    // free_task_slot poisons state/pid with 0xDEADBEEF, but the poison
    // still satisfies is_sleeping() (bit0 of 0xEF is set), so a stale
    // pid-hash reader (synchronize_rcu is a no-op: RCU_GEN never
    // advances — only rcu_softirq_handler bumps it and nothing raises
    // the Rcu vector) can pass the freed page's on_rq==false guard and
    // insert the dead pointer into tasks_timeline. The next pick then
    // context-switches onto the reused page and __switch_to's
    // `ld ra, thread_ra` + `ret` transfers control to whatever
    // pointer-shaped bytes now occupy that offset — the S-R signature
    // (kernel illegal instruction at a page-aligned linear-map heap
    // address). Detect the poison, report, and drop the enqueue.
    {
        // free_task_slot poisons state AND pid (two u32 writes at their
        // compile-time offsets — task_offsets::TASK_STATE/TASK_PID; R47 fixed
        // the stale hardcoded +0x48 that landed in ti_a2); a real PID can
        // never equal 0xDEADBEEF, so the pid check alone is an unambiguous
        // freed-page detector.
        let pid = (*task).pid();
        if pid == TASK_POISON {
            // R34: SBI direct write — this probe fires while the GRQ lock is
            // held; console::putchar would take the UART lock under GRQ
            // (same discipline as RawSpinlock::deadlock_warn and the buddy
            // double-free tripwire).
            const MSG: &[u8] = b"ENQ-POISONED-TASK dropped pid=0x";
            for &b in MSG { sbi_rt::legacy::console_putchar(b as usize); }
            let mut sh = 64;
            let v = pid as u64;
            while sh > 0 {
                sh -= 4;
                let nb = ((v >> sh) & 0xF) as u8;
                sbi_rt::legacy::console_putchar((if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }) as usize);
            }
            sbi_rt::legacy::console_putchar(b'\n' as usize);
            return false;
        }
    }

    // R33 (A-family wedge — dead-task resurrection): refuse an EXITING task
    // BEFORE the unconditional set_state(RUNNING) below. do_exit sets ZOMBIE
    // before its own dequeue; a wake_up whose UNLOCKED is_sleeping() filter
    // read predated the target's exit would otherwise flip ZOMBIE→RUNNING
    // and re-queue the dying task AFTER its dequeue. The reaping parent's
    // release_task could then free the kernel stack while another CPU picks
    // the zombie and __switch_to loads thread.sp (freed stack) into sp —
    // the A-type wedge (trap sequence storing to a heap address that no
    // longer maps the stack). ZOMBIE/DEAD are terminal here: nothing
    // legitimately enqueues an exiting task. (0xDEADBEEF poison also trips
    // is_dead() — bit 0x20 set — as a second line behind the pid check.)
    {
        let st = (*task).state();
        if st.is_dead() {
            // R34: SBI direct write — under the GRQ lock (was console::putchar,
            // which nests the UART lock inside GRQ).
            const MSG: &[u8] = b"ENQ-DEAD-TASK dropped\n";
            for &b in MSG { sbi_rt::legacy::console_putchar(b as usize); }
            return false;
        }
    }

    // R39 (B-form hardening): write RUNNING only when the task is actually
    // going to be linked. The old unconditional set_state(RUNNING) ran
    // BEFORE the class insert; every class insert can silently refuse via
    // its on_rq double-enqueue guard. For a task whose on_rq flag is
    // (erroneously or stale-ly) true while it is NOT linked in the tree,
    // the old order produced EXACTLY the captured B-form morphology —
    // state=RUNNING, task on no queue and no CPU, never scheduled again
    // (icount round-38 snapshot: pid 344 mrsh, 4 CPUs wfi-idle, runqueues
    // empty). With the write moved after the insert decision:
    //   - every legitimate path is unchanged — all callers reach here with
    //     state==RUNNING whenever the guard legitimately skips (a linked
    //     task is always RUNNING: __schedule re-enqueues only prev_running,
    //     the RR tick rotation gates on RUNNING, fork/kthread tasks are
    //     freshly inserted, change_task_policy dequeues before re-enqueue);
    //   - a refused insert now leaves the task SLEEPING (honest, form-A,
    //     still wakeable) instead of a phantom RUNNING that nothing will
    //     ever schedule.
    let policy = (*task).policy();

    // R7-B5: count only actual insertions. The class enqueues all carry a
    // double-enqueue guard (on_rq); incrementing nr_running unconditionally
    // while the guard skipped inflated the counter on every redundant
    // wake — the idle fast path in __schedule then never triggered again.
    let inserted = match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            grq.rt_rq.enqueue(task, false)
        }
        SchedPolicy::Deadline => {
            let now = crate::sched::fair::sched_clock();
            (*task).dl_entity().update_deadline(now);
            (*task).dl_entity().replenish_runtime();
            grq.dl_rq.enqueue(task)
        }
        SchedPolicy::Normal | SchedPolicy::Batch => {
            grq.cfs_rq.enqueue(task)
        }
        SchedPolicy::Idle => {
            // SCHED_IDLE uses CFS with low weight
            let se = (*task).sched_entity_mut();
            se.load = crate::sched::fair::LoadWeight::new(crate::sched::fair::WEIGHT_IDLEPRIO);
            grq.cfs_rq.enqueue(task)
        }
    };

    if inserted {
        // Linked (or verifiably already linked) — now make the state match
        // the queue membership (R39 ordering, see above).
        (*task).set_state(TaskState::new(TaskState::RUNNING));
        grq.nr_running.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    inserted
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
    // (fresh fork/init/kthread task: insert always succeeds; result unused)

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

/// R33 (A-family wedge — dead-task resurrection): atomically, under the GRQ
/// lock, re-verify that `task` is still in a wakeable state (sleeping or
/// stopped), transition it to RUNNING and enqueue it.
///
/// `Task::wake_up` filters on an UNLOCKED state read. Between that read and
/// the old unlocked set_state(RUNNING)+enqueue_task, the target could be
/// woken by another CPU, run to completion, pass do_exit (set ZOMBIE +
/// dequeue + final schedule) and be reaped by its parent — release_task
/// frees the kernel stack BEFORE the pinned task_put, so the stale wake
/// would then resurrect a freed-stack task onto the runqueue. Holding the
/// GRQ lock across re-check + transition + enqueue closes the window:
///   - a task still INTERRUPTIBLE/STOPPED cannot exit while we hold the
///     lock (to reach do_exit it must first be woken and scheduled, and
///     both the wake transition and the pick happen under this lock);
///   - a task that already exited reads ZOMBIE/DEAD here and is refused
///     (enqueue_task_locked's dead-guard double-checks before its
///     set_state(RUNNING)).
/// Returns true if the task was transitioned and enqueued.
pub fn wake_up_enqueue(task: *mut Task) -> bool {
    if task.is_null() {
        return false;
    }
    let mut grq_guard = grq().lock_irqsave();
    // SAFETY: task is non-null and validated by the caller (Task::wake_up's
    // poison check plus an unlocked wakeable-state filter, or a pinned
    // pid-hash lookup); the GRQ lock is held across the state transition so
    // the re-check and the enqueue are one atomic step.
    unsafe {
        let st = (*task).state();
        if !(st.is_sleeping() || st.contains(TaskState::STOPPED)) {
            // RUNNING (woken elsewhere / never slept), ZOMBIE, or DEAD —
            // the wake is stale or redundant; drop it. (Freed-page poison
            // keeps bit0 set so it passes is_sleeping(); it is caught one
            // step later by the pid poison check — and the dead-guard —
            // inside enqueue_task_locked.)
            //
            // R46 (B-form phantom — the refused-wake mirror of R41): RUNNING
            // is legitimate ONLY while the task is on a CPU (on_cpu, set at
            // pick, cleared by __switch_to once the outgoing context is
            // saved) or linked on a class queue. __schedule's discipline
            // makes every context switch-out leave prev either
            // (RUNNING ∧ linked) or (¬RUNNING ∧ unlinked), and every RUNNING
            // write in the tree is paired with an insert or with
            // on-CPU-ness — so RUNNING ∧ !on_cpu ∧ unlinked is NOT
            // producible by any interleaving of the scheduler itself. It
            // arrives from outside (a zero/write into the state word by the
            // documented Task-struct smash families — note RUNNING == 0 —
            // or a future regression), and because every upstream wake
            // filter refuses a RUNNING target, nothing would ever schedule
            // the task again: no queue holds it, no wake reaches it
            // (round-46 capture: pid 335, state=RUNNING, on_rq=0, 4 CPUs
            // wfi-idle, stack frozen in trap_exit -> asm_need_resched ->
            // schedule -> __schedule -> context_switch). Verify ACTUAL
            // linkage (never trust the flag), report, and fall through to
            // the insert below, which links it; a co-existing stale
            // on_rq=true is healed by the R41 branch this falls into, so
            // both mirror phantoms now heal at the same point.
            let phantom = st == TaskState::new(TaskState::RUNNING)
                && !(*task).on_cpu()
                && !match (*task).policy() {
                    SchedPolicy::Fifo | SchedPolicy::Rr => grq_guard.rt_rq.is_linked(task),
                    SchedPolicy::Deadline => grq_guard.dl_rq.is_linked(task),
                    SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                        grq_guard.cfs_rq.is_linked(task)
                    }
                };
            if !phantom {
                return false;
            }
            // R34: SBI direct write — we hold the GRQ lock here.
            {
                const MSG: &[u8] = b"R46-RUNNING-PHANTOM healed pid=0x";
                for &b in MSG {
                    sbi_rt::legacy::console_putchar(b as usize);
                }
                let v = (*task).pid() as u64;
                let mut sh = 64;
                while sh > 0 {
                    sh -= 4;
                    let nb = ((v >> sh) & 0xF) as u8;
                    sbi_rt::legacy::console_putchar(
                        (if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }) as usize,
                    );
                }
                sbi_rt::legacy::console_putchar(b'\n' as usize);
            }
            // Fall through: enqueue_task_locked links the task (its
            // set_state(RUNNING) is a no-op re-write; nr_running pairs with
            // the new linkage per R7-B5).
        }
        // R39: propagate the insert result. A refused insert (only possible
        // via a guard divergence for a sleeping/STOPPED target) leaves the
        // task SLEEPING — honest and still wakeable — and now also REPORTED
        // to the caller as a failed wake instead of a silent success that
        // consumed the waker's one-shot token while linking nothing.
        //
        // R41 (A-form twin): "still wakeable" is only true if a LATER wake
        // can succeed — but the refusing guard was the task's own stuck
        // on_rq=true, which nothing else clears for an unlinked task, so
        // without the heal below every future wake would also be refused
        // (the sleeper wedges permanently while the waker's token was
        // consumed). On refusal, verify actual linkage and heal the stale
        // flag — the same defense requeue_prev_locked applies to prev.
        //
        // R42 (lost wakeup in the sleeper's link-drop window): a `true`
        // from ensure_linked_locked means the task is LINKED — but this
        // function's contract is "transitioned AND enqueued", and there is
        // a fully legitimate, frequently raced producer of exactly this
        // refusal: the target is between its prepare_to_wait/set_state
        // (SLEEPING written under the WAITQUEUE/futex-bucket lock, NOT the
        // GRQ lock) and its own schedule() — still linked on its class
        // queue (the link is dropped only by its __schedule, under this
        // same GRQ lock). The waker's one-shot token (wait-queue entry
        // marked woken, futex waiter unlinked from the chain, semaphore/
        // pipe token consumed) is spent BEFORE Task::wake_up returns, and
        // wait-queue/futex wakers ignore the result — so leaving the state
        // SLEEPING here lets the sleeper's __schedule dequeue it with no
        // future wake available: a permanent SMP hang (the silent form-A
        // residual in every gate since R39; the R41 tripwire never fires
        // because the task is genuinely linked — this is NOT the stale
        // flag the heal below targets). Flip the state, exactly as the
        // pre-R39 unconditional set_state did for this window: the
        // sleeper's schedule() then sees prev_running, stays linked, and
        // its wait loop re-checks the condition the waker just made true
        // (a spurious wake simply loops back to sleep). For the heal
        // branch ensure_linked_locked's re-insert already set RUNNING;
        // for the poison/dead refusal it returned false and we pass that
        // through untouched.
        if enqueue_task_locked(&mut *grq_guard, task) {
            true
        } else if ensure_linked_locked(&mut *grq_guard, task) {
            (*task).set_state(TaskState::new(TaskState::RUNNING));
            true
        } else {
            false
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
                && task_prio > (*running).rt_priority()
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

    let mut grq_guard = grq().lock_irqsave();

    // R39: read the policy UNDER the GRQ lock. The old read raced
    // change_task_policy (which swaps policy + migrates the class queues
    // under this same lock): a policy captured before the lock could
    // address the WRONG class queue — dequeuing nothing from the new class
    // while believing the compensation succeeded, or worse, no-op'ing the
    // exit-path dequeue of a task that had just been re-linked elsewhere.
    let policy = task.policy();
    let actually_dequeued = match policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            // R31-5: propagate the real dequeue result — the hardcoded
            // true decremented nr_running on every no-op (every RT/DL
            // exit + wait-recheck), leaking the idle fast path.
            grq_guard.rt_rq.dequeue(task_ptr)
        }
        SchedPolicy::Deadline => {
            grq_guard.dl_rq.dequeue(task_ptr)
        }
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            let dequeued = grq_guard.cfs_rq.dequeue(task_ptr);
            // R8-1: clear curr UNCONDITIONALLY when it matches — the R7-2
            // version required `dequeued`, but a RUNNING task was already
            // picked off the tree (dequeue returns false), so the exit path
            // never fired and update_curr kept writing into the FREED Task
            // on every idle tick (the corruption engine behind NEW2's
            // wandering damage).
            if grq_guard.cfs_rq.get_curr() == task_ptr {
                grq_guard.cfs_rq.set_curr(core::ptr::null_mut());
            }
            dequeued
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

/// Atomically change a task's scheduling policy (and RT priority) with the
/// proper run-queue migration (review PROC-P07): dequeue from the OLD class
/// queue, update the fields, enqueue into the NEW class queue — all under
/// the GRQ lock. Without the migration a task could sit on two class queues
/// at once (or on none), letting two CPUs pick it simultaneously or
/// stranding it off-queue forever.
pub fn change_task_policy(task: *mut Task, new_policy: crate::process::task::SchedPolicy, rt_prio: u32) {
    use crate::process::task::SchedPolicy;

    let is_current = match current() {
        Some(c) => c as *const Task == task as *const Task,
        None => false,
    };

    let mut grq_guard = grq().lock_irqsave();
    let old_policy = unsafe { (*task).policy() };

    // Whether the task is actually linked on a class run queue. The RUNNING
    // state is NOT sufficient: a task picked by another CPU is dequeued at
    // pick time — re-enqueueing it lets two CPUs run it simultaneously
    // (regression round 5, HIGH).
    let linked = match old_policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => {
            unsafe { (*task).rt_entity().is_on_rq() }
        }
        SchedPolicy::Deadline => {
            unsafe { (*task).dl_entity().is_on_rq() }
        }
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            unsafe { (*task).sched_entity().is_on_rq() }
        }
    };

    if linked {
        // R31-5 discipline: propagate the class dequeue result (RT/DL now
        // return accurate bools; the old `; true` hardcodes drifted
        // nr_running whenever the guarded dequeue was a no-op).
        let dequeued = match old_policy {
            SchedPolicy::Fifo | SchedPolicy::Rr => {
                grq_guard.rt_rq.dequeue(task)
            }
            SchedPolicy::Deadline => {
                grq_guard.dl_rq.dequeue(task)
            }
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
                grq_guard.cfs_rq.dequeue(task)
            }
        };
        if dequeued {
            grq_guard.nr_running.fetch_update(
                core::sync::atomic::Ordering::SeqCst,
                core::sync::atomic::Ordering::SeqCst,
                |v| v.checked_sub(1),
            );
        }
    }

    // SAFETY: exclusive access under the GRQ lock.
    unsafe {
        (*task).set_rt_priority(rt_prio);
        (*task).set_policy(new_policy);
    }

    // Re-enqueue into the new class queue under the same lock.
    if linked {
        // SAFETY: GRQ lock is held via grq_guard; enqueue_task_locked
        // expects the lock to be held.
        unsafe {
            enqueue_task_locked(&mut *grq_guard, task);
        }
        // Cross-CPU preemption checks mirror enqueue_task.
        let policy = unsafe { (*task).policy() };
        if policy == SchedPolicy::Fifo || policy == SchedPolicy::Rr {
            check_rt_preempt(task, unsafe { (*task).cpus_allowed() });
        } else if policy == SchedPolicy::Deadline {
            check_dl_preempt(task, unsafe { (*task).cpus_allowed() });
        }
    }
}

/// Remove `task` from the global run queue if a racing wake_up() enqueued it
/// while it was transiently marked sleeping — the "prepare-to-wait recheck
/// decided not to sleep" path calls this before continuing to run, so no
/// other CPU can pick a task that is already executing (review NEW-C2).
///
/// CFS-only and idempotent: cfs_rq.dequeue() checks the entity's own on_rq
/// flag and is a safe no-op for a task that is not linked. RT/DL are
/// excluded because their RR rotation legitimately keeps the current task
/// linked, so "on queue" cannot be distinguished from "running" there.
pub fn dequeue_if_enqueued(task: &Task) {
    let task_ptr = task as *const Task as *mut Task;
    let mut grq_guard = grq().lock_irqsave();
    let dequeued = match task.policy() {
        SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle => {
            grq_guard.cfs_rq.dequeue(task_ptr)
        }
        _ => false,
    };
    if dequeued {
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

    // R18-3b: full context line for correlation.
    // R18-3: per-tick canary check — DIAGNOSED (r18): pid 305's call
    // chain legitimately reaches the stack's deepest 8 bytes; frames spill
    // into the adjacent heap page (the children-list zeroing engine).
    // First-hit-only reporting; the fix is stack-size/depth work (r19).
    if let Some(t) = crate::sched::current() {
        unsafe {
            let bottom = (*t).kernel_stack_bottom();
            if bottom != 0 {
                let v = core::ptr::read_volatile(bottom as *const u64);
                static REPORTED: core::sync::atomic::AtomicBool =
                    core::sync::atomic::AtomicBool::new(false);
                if v != 0xCAFE_F00D_DEAD_BEEF
                    && v != 0
                    && !REPORTED.swap(true, core::sync::atomic::Ordering::AcqRel)
                {
                    use crate::console::putchar;
                    const MSG: &[u8] = b"TICK-CANARY pid=";
                    for &b in MSG { putchar(b); }
                    let pid = (*t).pid();
                    let mut vv = pid as usize;
                    if vv == 0 { putchar(b'0'); }
                    let mut dd = [0u8; 10]; let mut kk = 0;
                    while vv > 0 { dd[kk] = b'0' + (vv % 10) as u8; kk += 1; vv /= 10; }
                    while kk > 0 { kk -= 1; putchar(dd[kk]); }
                    const M2: &[u8] = b" v=0x";
                    for &b in M2 { putchar(b); }
                    let mut sh = 64;
                    while sh > 0 { sh -= 4; let nb = ((v >> sh) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                    putchar(b'\n');
                    // R18-6: consistency — bottom vs live kernel_stack.
                    const M0: &[u8] = b" bottom=0x";
                    for &b in M0 { putchar(b); }
                    {
                        let b2 = bottom;
                        let mut sh0 = 64;
                        while sh0 > 0 { sh0 -= 4; let nb = ((b2 >> sh0) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                    }
                    const M6: &[u8] = b" ks=0x";
                    for &b in M6 { putchar(b); }
                    {
                        let ks = match unsafe { (*t).get_kernel_stack() } { Some(p) => p as usize, None => 0 };
                        let mut sh = 64;
                        while sh > 0 { sh -= 4; let nb = ((ks >> sh) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                    }
                    putchar(b'\n');
                    // R18-5: the interrupted epc — if the smash is fresh,
                    // the victim was executing in the deep chain RIGHT NOW.
                    const M5: &[u8] = b" epc=0x";
                    for &b in M5 { putchar(b); }
                    {
                        use crate::arch::riscv64::trap::current_pt_regs;
                        use crate::arch::riscv64::pt_regs::PtRegs;
                        let pr = current_pt_regs() as *const PtRegs;
                        if !pr.is_null() {
                            let e = unsafe { (*pr).epc };
                            let mut sh = 64;
                            while sh > 0 { sh -= 4; let nb = ((e >> sh) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                        }
                    }
                    putchar(b'\n');
                    // R18-4: frame count + deepest symbols at smash time.
                    let mut frames = 0u32;
                    crate::dfx::backtrace::walk_stack_trace(&mut |pc, _fp| {
                        frames += 1;
                        if frames <= 6 {
                            const M3: &[u8] = b" f=0x";
                            for &b in M3 { putchar(b); }
                            let mut sh = 64;
                            while sh > 0 { sh -= 4; let nb = ((pc >> sh) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                        }
                    });
                    const M4: &[u8] = b" frames=";
                    for &b in M4 { putchar(b); }
                    let mut vv = frames;
                    if vv == 0 { putchar(b'0'); }
                    let mut dd = [0u8; 6]; let mut kk = 0;
                    while vv > 0 { dd[kk] = b'0' + (vv % 10) as u8; kk += 1; vv /= 10; }
                    while kk > 0 { kk -= 1; putchar(dd[kk]); }
                    putchar(b'\n');
                    // repair so we report once
                    core::ptr::write_volatile(bottom as *mut u64, 0xCAFE_F00D_DEAD_BEEF);
                }
            }
        }
    }

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
                    // R20-6: see __schedule — only the CPU whose task owns
                    // the single cfs_rq.curr slot charges through it; every
                    // CPU charges its own current task exactly once.
                    if grq_guard.cfs_rq.get_curr() == current {
                        grq_guard.cfs_rq.update_curr(now);
                    } else {
                        let se = (*current).sched_entity();
                        let delta = se.update_exec_runtime(now);
                        if delta > 0 {
                            se.update_vruntime(delta);
                        }
                    }

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
                    // R20-1: only rotate a task that is still RUNNING. The
                    // tick IRQ can land inside two windows where the old
                    // unconditional set_state(RUNNING)+enqueue corrupted
                    // state:
                    //  (a) prepare_to_wait has set INTERRUPTIBLE but
                    //      schedule() has not run yet — the rotation would
                    //      resurrect the sleeper onto the runqueue;
                    //  (b) do_exit has set ZOMBIE (its preempt_disable does
                    //      NOT block the timer IRQ, only the IRQ-exit
                    //      preemption point) — the rotation would re-queue
                    //      an exiting task, letting another CPU pick and
                    //      resume a zombie.
                    // current is paused on THIS CPU inside the IRQ, so the
                    // state read is race-free.
                    if (*current).state() == TaskState::new(TaskState::RUNNING) {
                        let mut grq_guard = grq().lock_irqsave();
                        // R39: propagate the insert result and pair the
                        // grq.nr_running count. The rotation links a RUNNING
                        // task that __schedule will then NOT re-count (its
                        // on_rq guard skips the insert, so the fetch_add
                        // there never fires) — every RR rotation leaked one
                        // count from the atomic, drift that any future
                        // reader of grq.nr_running would inherit.
                        if grq_guard.rt_rq.enqueue(current, false) {
                            grq_guard.nr_running.fetch_add(
                                1,
                                core::sync::atomic::Ordering::Relaxed,
                            );
                        }
                        set_need_resched(); // Set before dropping lock to prevent lost wake-up
                        drop(grq_guard);
                    }
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
    // Called after context_switch in the new task's context (ret_from_fork).
    // R9-2 (HIGH): a newborn never returns into __schedule's tail, so the
    // per-CPU deferred exit notify stored by the task we replaced would be
    // skipped — and the next exit on this CPU overwrites the slot, losing
    // the parent's SIGCHLD forever (silent wait hang in fork+fast-exit
    // pipelines). Process it here; the slot clear makes this idempotent
    // with the __schedule tail.
    process_deferred_exit_notify_cpu(crate::arch::cpu_id() as usize);
}

// ==================== Utility Functions ====================

/// R25-6: renice a possibly-queued task under the GRQ lock with
/// load_weight rebalance (bare set_nice made enqueue/dequeue weights
/// mismatch and wrap the counter).
pub unsafe fn sched_renice_locked(task: *mut Task, niceval: i32) {
    // R31-3: actually take the GRQ lock, and gate the load_weight
    // rebalance on the task being IN THE TREE (picked tasks are off it —
    // the unconditional adjust permanently shifted the counter for the
    // common running/sleeping renice target).
    let _g = grq().lock_irqsave();
    let old_weight = (*task).sched_entity().load.weight;
    let was_on_rq = (*task).sched_entity().is_on_rq();
    (*task).set_nice(niceval);
    let new_weight = (*task).sched_entity().load.weight;
    if was_on_rq && old_weight != new_weight {
        let g = grq();
        g.cfs_rq.load_weight.fetch_add(new_weight, core::sync::atomic::Ordering::AcqRel);
        g.cfs_rq.load_weight.fetch_sub(old_weight, core::sync::atomic::Ordering::AcqRel);
    }
}

/// R25-6: load_weight rebalance for an in-tree reweight (GRQ-locked caller).
pub fn grq_cfs_adjust(old_w: u64, new_w: u64) {
    // Operate on the GRQ's CFS class — the caller holds the GRQ lock.
    // (Access via the same unsafe pattern the file uses for GRQ.)
    unsafe {
        let g = grq() as *const GlobalRunQueue as *mut GlobalRunQueue;
        (*g).cfs_rq.load_weight.fetch_add(new_w, core::sync::atomic::Ordering::AcqRel);
        (*g).cfs_rq.load_weight.fetch_sub(old_w, core::sync::atomic::Ordering::AcqRel);
    }
}

pub fn yield_cpu() {
    schedule();
}

/// Iterate over all tasks.
///
/// R20-3: iterate the PID hash table. The previous implementation only
/// walked the per-CPU current+idle pointers, making every caller that
/// needs "all tasks" blind to sleeping/queued tasks:
///   - mm/rmap.rs try_to_unmap: stale PTEs in sleeping tasks survived
///     swap-out/migration (data-corruption family);
///   - mm/compact.rs migration: sleeping tasks were skipped entirely;
///   - dfx/hung_task.rs: D-state tasks are by definition not running on
///     any CPU, so the detector never saw its exact target;
///   - sysinfo procs count: undercount.
/// kill(-1) in syscall/process.rs already documented this gap and switched
/// to pid_hash_for_each_task; these callers went through this wrapper.
///
/// `f` runs under the per-bucket lock: a task linked into a bucket cannot
/// be removed (removal needs the same lock) and therefore cannot be freed
/// while `f` runs. `f` must not sleep or take a PID-hash bucket lock.
pub fn for_each_task<F>(f: F)
where
    F: Fn(*mut Task),
{
    crate::process::pid_hash::pid_hash_for_each_task(|t| f(t));
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

            // R20-2: a task may have been enqueued between schedule()'s
            // nr_running==0 fast-path read and mark_idle — enqueue_task's
            // find_idle_cpu missed us (the idle bit was not set yet), so no
            // resched IPI was sent and WFI would sleep until the next timer
            // tick (up to 10ms of needless latency). Re-check now: the
            // mark_idle above makes any enqueue after this point visible to
            // find_idle_cpu, so the check-then-WFI sequence is closed.
            if GlobalRunQueue::grq_nr_running() > 0 {
                grq().clear_idle(cpu_id);
                continue;
            }

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
