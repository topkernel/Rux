//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! preempt_count implementation
//!
//! Bit layout:
//!   bits [0:7]   PREEMPT_MASK   — preemption disable count
//!   bits [8:15]  SOFTIRQ_MASK   — softirq nesting count
//!   bits [16:19] HARDIRQ_MASK   — hard IRQ nesting count
//!   bit  [20]    NMI_MASK       — NMI count
//!   bit  [26]    PREEMPT_ACTIVE — actively preempting

// ============================================================================
// Bit masks
// ============================================================================

/// Preemption disable mask (bits [0:7])
pub const PREEMPT_MASK: i32 = 0x0000_00FF;
/// Softirq mask (bits [8:15])
pub const SOFTIRQ_MASK: i32 = 0x0000_FF00;
/// Hard IRQ mask (bits [16:19])
pub const HARDIRQ_MASK: i32 = 0x000F_0000;
/// NMI mask (bits [20:23], matching Linux's 4-bit NMI field)
pub const NMI_MASK: i32 = 0x00F0_0000;
/// Preempt active flag (bit [26])
pub const PREEMPT_ACTIVE: i32 = 0x0400_0000;

// ============================================================================
// Offsets (add/sub these to preempt_count)
// ============================================================================

/// Preemption disable offset
pub const PREEMPT_OFFSET: i32 = 1;
/// Softirq offset
pub const SOFTIRQ_OFFSET: i32 = 1 << 8;
/// Hard IRQ offset
pub const HARDIRQ_OFFSET: i32 = 1 << 16;
/// NMI offset
pub const NMI_OFFSET: i32 = 1 << 20;

// ============================================================================
// Context query functions
// ============================================================================

/// Per-CPU snapshot of the interrupted task's own preempt_count, taken at
/// timer-trap ENTRY — before irq_enter() books the tick's own
/// HARDIRQ_OFFSET on the current task (fleet s2 session-6 discriminator).
/// The PREEMPT-LEAK probe prints it (pctick=) so an observed pc=0x10001
/// splits into its two producers: pctick=0x1 is a leaked PREEMPT bit
/// (the 0x10000 is the tick's own in-flight offset), while
/// pctick>=0x10001 is a stranded hardirq bit carried from an earlier
/// bracket. Written only from the timer trap path on the owning CPU;
/// read from the tick probe on the same CPU.
pub static TICK_ENTRY_PC: [core::sync::atomic::AtomicU32;
    crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU32::new(0) };
        crate::config::MAX_CPUS];

/// Per-CPU snapshot of the interrupted instruction pointer at timer-trap
/// entry (leg-33B incumbent attribution). For a preempt-off incumbent
/// monopolizing its CPU, the strand census names only pid/pc; the last
/// tick's interrupted RIP names the kernel function it was spinning in
/// (symbolize offline with addr2line). Written on the owning CPU from
/// the same sites as TICK_ENTRY_PC; read from the wakewatch strand
/// print under dfx=leakhunt.
pub static TICK_ENTRY_RIP: [core::sync::atomic::AtomicU64;
    crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) };
        crate::config::MAX_CPUS];

/// Read current task's preempt_count
#[inline]
pub fn preempt_count() -> i32 {
    match crate::sched::current() {
        Some(task) => task.preempt_count(),
        None => 0,
    }
}

/// Check if in any interrupt context (hardirq + softirq + NMI)
#[inline]
pub fn in_interrupt() -> bool {
    (preempt_count() & (HARDIRQ_MASK | SOFTIRQ_MASK | NMI_MASK)) != 0
}

/// Check if in hard IRQ context
#[inline]
pub fn in_irq() -> bool {
    (preempt_count() & HARDIRQ_MASK) != 0
}

/// Check if in softirq context
#[inline]
pub fn in_softirq() -> bool {
    (preempt_count() & SOFTIRQ_MASK) != 0
}

/// Check if in process (task) context
#[inline]
pub fn in_task() -> bool {
    !in_interrupt()
}

/// Check if preemption is allowed
#[inline]
pub fn preemptible() -> bool {
    let pc = preempt_count();
    pc == 0
}

// ============================================================================
// Manipulation functions
// ============================================================================

/// Add value to current task's preempt_count
///
/// #[track_caller] + the leakorigin hook (dfx=leakhunt): when val is
/// PREEMPT_OFFSET, record the acquire site in the per-task shadow (last
/// acquire wins). The disabled-path cost is one register compare plus
/// one relaxed load. At a leak, the shadow names the most recent guard
/// acquire — outer leaks (settled count 1) name the outer site, inner
/// leaks (an inner scope abandoned between its acquire and drop) name
/// the inner site; `cnt_after` in the report separates the two.
#[inline]
#[track_caller]
pub fn preempt_count_add(val: i32) {
    if let Some(task) = crate::sched::current() {
        if val == PREEMPT_OFFSET
            && crate::dfx::leakorigin::ENABLED.load(core::sync::atomic::Ordering::Relaxed)
        {
            let c0 = task.preempt_count();
            crate::dfx::leakorigin::note_acquire(
                task,
                core::panic::Location::caller(),
                c0.wrapping_add(1),
            );
        }
        task.add_preempt_count(val);
    }
}

/// Subtract value from current task's preempt_count
///
/// #[track_caller]: on underflow the report names the guard's drop site
/// (file:line) instead of this generic helper — the leak hunt's missing
/// fingerprint (which lock family double-drops).
#[inline]
#[track_caller]
pub fn preempt_count_sub(val: i32) {
    let loc = core::panic::Location::caller();
    if let Some(task) = crate::sched::current() {
        task.sub_preempt_count_at(val, loc);
    }
}

/// `preempt_count_sub` with an explicit captured drop-site location —
/// used by #[track_caller] inline helpers (spinlock guards) so the
/// underflow report names the GUARD's line, not the helper's.
#[inline]
pub fn preempt_count_sub_at(
    val: i32,
    loc: &'static core::panic::Location<'static>,
) {
    if let Some(task) = crate::sched::current() {
        task.sub_preempt_count_at(val, loc);
    }
}

/// As `preempt_count_sub_at`, plus the guard's lock instance address for
/// the underflow report (leak-hunt round 3e: name WHICH lock).
#[inline]
pub fn preempt_count_sub2_at(
    val: i32,
    loc: &'static core::panic::Location<'static>,
    site: usize,
) {
    if let Some(task) = crate::sched::current() {
        task.sub_preempt_count2_at(val, loc, site);
    }
}

/// R74: reset a corrupted (negative) preempt_count to zero. A negative
/// count is never legal — it is the fingerprint of an unpaired decrement
/// (double-executed guard drop during a double-run). Called only from
/// the fault-path corruption self-heal, with a loud print alongside.
#[inline]
pub fn preempt_count_reset() {
    if let Some(task) = crate::sched::current() {
        task.ti_preempt_count_zero();
    }
}

// ============================================================================
// IRQ entry/exit helpers
// ============================================================================

/// Enter IRQ context — increment hardirq count
///
/// Called at the beginning of hardware interrupt handling.
/// IRQ entry path.
#[inline]
pub fn irq_enter() {
    preempt_count_add(HARDIRQ_OFFSET);
    crate::dfx::syscost::zone_hardirq_enter(); // syscost probe (no-op when off)
}

/// Exit IRQ context — decrement hardirq count and process softirqs
///
/// Called at the end of hardware interrupt handling.
/// IRQ exit path with softirq invocation.
#[inline]
pub fn irq_exit() {
    preempt_count_sub(HARDIRQ_OFFSET);

    // If we are no longer in hardirq context (outermost irq_exit),
    // check for and process pending softirqs.
    //
    // Skip softirq invocation when there is no current task (secondary
    // CPUs in the early WFI loop have tp=hart_id, not a task_struct).
    // Without a current task, preempt_count is always 0, so in_irq()
    // returns false even during hardware interrupt handling.  Running
    // do_softirq_own_stack() in this state would switch to the same
    // interrupt stack that already holds PtRegs, corrupting the trap
    // frame and crashing the CPU.
    // R14-6 (HIGH-1): also require !in_softirq() — a task holding a
    // lock_bh guard (SOFTIRQ_OFFSET set) that takes this IRQ must NOT run
    // softirq handlers inline at exit: the handler can spin on the very
    // lock the interrupted holder owns (lock_bh was a lie before). Deferring
    // to ksoftirqd is always safe.
    //
    // Dispatch black hole fix (UART perf gate round 2, 2026-10-20): the
    // timer IRQ raises its softirqs via raise_softirq_irqoff (no
    // ksoftirqd wake — it expects THIS path to run them). When the skip
    // fires, nothing else ever dispatches them until some LATER IRQ
    // happens to exit outside a bh section — a task looping through
    // lock_bh sections with sub-tick iterations starved its CPU's
    // Hrtimer softirq for seconds (TIMER-SOFTIRQ-STALL tick_age=0ms
    // shape: ticks arriving, dispatch skipped). Linux drains at
    // local_bh_enable(); until that exists here, waking ksoftirqd at the
    // skip point closes the hole: the pending bits stay set, and the
    // woken kthread drains them as soon as the scheduler picks it.
    if crate::sched::current().is_some() {
        if !in_irq() && !in_softirq() {
            crate::interrupt::softirq::invoke_softirq();
        } else if !in_irq() && in_softirq() {
            crate::interrupt::ksoftirqd::wakeup_ksoftirqd();
        }
    }
    crate::dfx::syscost::zone_hardirq_exit(); // syscost probe (iret frame boundary; no-op when off)
}

// ============================================================================
// NMI entry/exit helpers
// ============================================================================

/// Enter NMI context — increment NMI count
///
/// Unlike `irq_enter()`, NMI entry is lock-free and never invokes softirqs.
/// Must be paired with `nmi_exit()`.
#[inline]
pub fn nmi_enter() {
    preempt_count_add(NMI_OFFSET);
}

/// Exit NMI context — decrement NMI count
///
/// Does NOT invoke softirqs (unlike `irq_exit`) because NMI context
/// must return as quickly as possible.
#[inline]
pub fn nmi_exit() {
    preempt_count_sub(NMI_OFFSET);
}

/// Check if currently in NMI context
#[inline]
pub fn in_nmi() -> bool {
    (preempt_count() & NMI_MASK) != 0
}

/// Full NMI entry path
#[inline]
pub fn irqentry_nmi_enter() {
    nmi_enter();
}

/// Full NMI exit path
#[inline]
pub fn irqentry_nmi_exit() {
    nmi_exit();
}
