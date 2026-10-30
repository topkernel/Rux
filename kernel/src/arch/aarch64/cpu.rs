//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! CPU-related operations (AArch64)
//!
//! System-register primitives: DAIF interrupt control, generic timer
//! reads (CNTFRQ/CNTVCT), per-CPU identity (MPIDR/TPIDR), barriers.

/// Get current core ID
#[inline]
pub fn get_core_id() -> u64 {
    super::cpu_id()
}

/// Read ANOTHER cpu's current task pointer. aarch64 keeps the current
/// task in the scheduler's per-CPU mirror (same as the riscv64 backend's
/// cross-hart protocol). Diagnostic-only.
#[inline]
pub fn get_thread_id_of(cpu: usize) -> u64 {
    if cpu < crate::config::MAX_CPUS {
        crate::sched::sched::cpu_state(cpu).current as u64
    } else {
        0
    }
}

/// Get current thread ID (TPIDR_EL1 — the aarch64 "tp")
#[inline]
pub fn get_thread_id() -> u64 {
    // SAFETY: reading TPIDR_EL1 is a pure system-register read.
    let tp: u64;
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el1", out(reg) tp, options(nomem, nostack, pure));
    }
    tp
}

/// Set current thread ID (TPIDR_EL1)
#[inline]
pub fn set_thread_id(tid: u64) {
    // SAFETY: writing TPIDR_EL1 stores the kernel thread pointer; the
    /// value is only ever read back by get_thread_id()/assembly.
    unsafe {
        core::arch::asm!("msr tpidr_el1, {}", in(reg) tid, options(nomem, nostack));
    }
}

/// Get counter frequency (CNTFRQ_EL0 — the generic timer frequency).
/// QEMU virt defaults to 64 MHz.
#[inline]
pub fn get_counter_freq() -> u64 {
    // SAFETY: CNTFRQ_EL0 is a read-only register; reading is always safe.
    let freq: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack, pure));
    }
    freq
}

/// Read the virtual counter (CNTVCT_EL0) — the monotonic cycle counter
#[inline]
pub fn read_counter() -> u64 {
    // SAFETY: CNTVCT_EL0 is a read-only monotonic counter.
    let val: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) val, options(nomem, nostack, pure));
    }
    val
}

/// Read the wall-clock cycle counter (alias of read_counter on aarch64 —
/// the generic timer IS the wall clock source; no separate mtime)
#[inline]
pub fn read_time() -> u64 {
    read_counter()
}

/// Enable interrupts (clear D/A/I/F mask bits in DAIF)
#[inline]
#[track_caller]
pub fn enable_irq() {
    crate::dfx::syscost::irq_note_on();
    // SAFETY: msr daifclr atomically clears the DAIF mask bits; safe at
    /// any time.
    unsafe {
        core::arch::asm!("msr daifclr, #0xf", options(nomem, nostack, preserves_flags));
    }
}

/// Disable interrupts (set D/A/I/F mask bits in DAIF)
#[inline]
#[track_caller]
pub fn disable_irq() {
    crate::dfx::syscost::irq_note_off(Some(core::panic::Location::caller()));
    // SAFETY: msr daifset atomically sets the DAIF mask bits; safe at any
    /// time.
    unsafe {
        core::arch::asm!("msr daifset, #0xf", options(nomem, nostack, preserves_flags));
    }
}

/// Wait for interrupt
#[inline]
pub fn wfi() {
    // SAFETY: wfi halts the core until the next interrupt; no side effects
    // beyond waiting. nomem intentionally omitted (ordering matters
    // against memory operations that set up the wake condition).
    unsafe {
        core::arch::asm!("wfi", options(nostack, preserves_flags));
    }
}

/// Wait for event (SEV companion)
#[inline]
pub fn wfe() {
    // SAFETY: wfe waits for an event; harmless hint instruction.
    unsafe {
        core::arch::asm!("wfe", options(nostack, preserves_flags));
    }
}

/// Send event (wakes WFE waiters)
#[inline]
pub fn sev() {
    // SAFETY: sev is a hint broadcast; no side effects on this core.
    unsafe {
        core::arch::asm!("sev", options(nomem, nostack, preserves_flags));
    }
}

/// Instruction serialization barrier
#[inline]
pub fn isb() {
    // SAFETY: isb serializes instruction fetch; no harmful side effects.
    unsafe {
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
}

/// Data synchronization barrier (full system)
#[inline]
pub fn dsb() {
    // SAFETY: dsb sy is a completion barrier; no harmful side effects.
    unsafe {
        core::arch::asm!("dsb sy", options(nomem, nostack, preserves_flags));
    }
}

/// Data memory barrier
#[inline]
pub fn dmb() {
    // SAFETY: dmb is an ordering barrier; no harmful side effects.
    unsafe {
        core::arch::asm!("dmb ish", options(nomem, nostack, preserves_flags));
    }
}

/// Invalidate local TLB (all of EL1; full flush)
#[inline]
pub fn local_flush_tlb_all() {
    // SAFETY: tlbi vmalle1 invalidates all TLB entries for this EL;
    // callers must have repaired the affected translations first.
    unsafe {
        core::arch::asm!(
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            options(nostack, preserves_flags)
        );
    }
}

/// Get interrupt mask state (DAIF.I/F)
#[inline]
pub fn get_interrupts_state() -> bool {
    let daif: u64;
    // SAFETY: DAIF is a read-only view register at EL1.
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, pure));
    }
    // IRQ mask bit is bit 7 (I)
    (daif & (1 << 7)) == 0
}

/// Save interrupt state and disable interrupts.
/// Returns the previous state (true = interrupts were enabled).
#[inline]
pub fn save_and_disable_irq() -> bool {
    let was = get_interrupts_state();
    disable_irq();
    was
}

/// Restore interrupt state
#[inline]
pub fn restore_irq(state: bool) {
    if state {
        enable_irq();
    }
}
