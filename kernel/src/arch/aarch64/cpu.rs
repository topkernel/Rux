//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! CPU-related operations (AArch64)
//!
//! System-register primitives: DAIF interrupt control, generic timer
//! reads (CNTFRQ/CNTVCT), per-CPU identity (MPIDR/TPIDR), barriers.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

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
    if cpu < crate::arch::aarch64::MAX_CPUS {
        crate::sched::sched::cpu_state(cpu).current as u64
    } else {
        0
    }
}
/// Get current thread ID (TPIDR_EL1 — the aarch64 "tp")
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-14): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_thread_id() -> u64 {
    crate::arch::arch_floor::aarch64::cpu::get_thread_id()
}
/// Set current thread ID (TPIDR_EL1)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-13): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn set_thread_id(tid: u64) {
    crate::arch::arch_floor::aarch64::cpu::set_thread_id(tid)
}
/// Get counter frequency (CNTFRQ_EL0 — the generic timer frequency).
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-12): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_counter_freq() -> u64 {
    crate::arch::arch_floor::aarch64::cpu::get_counter_freq()
}
/// Read the virtual counter (CNTVCT_EL0) — the monotonic cycle counter
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-11): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn read_counter() -> u64 {
    crate::arch::arch_floor::aarch64::cpu::read_counter()
}

/// Read the wall-clock cycle counter (alias of read_counter on aarch64 —
/// the generic timer IS the wall clock source; no separate mtime)
#[inline]
pub fn read_time() -> u64 {
    read_counter()
}
/// Enable interrupts (clear D/A/I/F mask bits in DAIF)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-10): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn enable_irq() {
    crate::arch::arch_floor::aarch64::cpu::enable_irq()
}
/// Disable interrupts (set D/A/I/F mask bits in DAIF)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-9): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn disable_irq() {
    crate::arch::arch_floor::aarch64::cpu::disable_irq()
}
/// Wait for interrupt
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-8): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn wfi() {
    crate::arch::arch_floor::aarch64::cpu::wfi()
}
/// Wait for event (SEV companion)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-7): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn wfe() {
    crate::arch::arch_floor::aarch64::cpu::wfe()
}
/// Send event (wakes WFE waiters)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn sev() {
    crate::arch::arch_floor::aarch64::cpu::sev()
}
/// Instruction serialization barrier
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn isb() {
    crate::arch::arch_floor::aarch64::cpu::isb()
}
/// Data synchronization barrier (full system)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn dsb() {
    crate::arch::arch_floor::aarch64::cpu::dsb()
}
/// Data memory barrier
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn dmb() {
    crate::arch::arch_floor::aarch64::cpu::dmb()
}
/// Invalidate local TLB (all of EL1; full flush)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn local_flush_tlb_all() {
    crate::arch::arch_floor::aarch64::cpu::local_flush_tlb_all()
}
/// Get interrupt mask state (DAIF.I/F)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_interrupts_state() -> bool {
    crate::arch::arch_floor::aarch64::cpu::get_interrupts_state()
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
