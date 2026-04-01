//! Linux-compatible preempt_count implementation
//!
//! Bit layout (matches Linux `thread_info.preempt_count`):
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
/// NMI mask (bit [20])
pub const NMI_MASK: i32 = 0x0010_0000;
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
#[inline]
pub fn preempt_count_add(val: i32) {
    if let Some(task) = crate::sched::current() {
        task.add_preempt_count(val);
    }
}

/// Subtract value from current task's preempt_count
#[inline]
pub fn preempt_count_sub(val: i32) {
    if let Some(task) = crate::sched::current() {
        task.sub_preempt_count(val);
    }
}

// ============================================================================
// IRQ entry/exit helpers
// ============================================================================

/// Enter IRQ context — increment hardirq count
///
/// Called at the beginning of hardware interrupt handling.
/// Equivalent to Linux's `irqentry_enter()` → `irq_enter()`.
#[inline]
pub fn irq_enter() {
    preempt_count_add(HARDIRQ_OFFSET);
}

/// Exit IRQ context — decrement hardirq count and process softirqs
///
/// Called at the end of hardware interrupt handling.
/// Equivalent to Linux's `irq_exit()` → `invoke_softirq()`.
#[inline]
pub fn irq_exit() {
    preempt_count_sub(HARDIRQ_OFFSET);

    // If we are no longer in hardirq context (outermost irq_exit),
    // check for and process pending softirqs.
    if !in_irq() {
        crate::interrupt::softirq::invoke_softirq();
    }
}
