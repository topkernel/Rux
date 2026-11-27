//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

/// CPU-related operations (RISC-V 64-bit)
use core::arch::asm;

/// Get current core ID (hart ID)
#[inline]
pub fn get_core_id() -> u64 {
    // mhartid is an M-mode CSR and traps when read from S-mode; delegate to
    // the tp-based implementation instead.
    super::cpu_id()
}

/// Read ANOTHER hart's tp task pointer. riscv64 keeps tp in a register
/// only, so cross-hart reads use the scheduler's per-CPU current mirror
/// (cpu_state(c).current — written at pick under the GRQ; mirrors tp at
/// every completed switch). Diagnostic-only.
#[inline]
pub fn get_thread_id_of(cpu: usize) -> u64 {
    if cpu < crate::config::MAX_CPUS {
        crate::sched::sched::cpu_state(cpu).current as u64
    } else {
        0
    }
}
/// Get current thread ID
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-26): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_thread_id() -> u64 {
    crate::arch::arch_floor::riscv64::cpu::get_thread_id()
}
/// Set current thread ID
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-25): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn set_thread_id(tid: u64) {
    crate::arch::arch_floor::riscv64::cpu::set_thread_id(tid)
}

/// Get counter frequency (RISC-V uses time CSR)
#[inline]
pub fn get_counter_freq() -> u64 {
    // QEMU virt platform default frequency: 10 MHz
    10_000_000
}
/// Read counter (time CSR)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-24): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn read_counter() -> u64 {
    crate::arch::arch_floor::riscv64::cpu::read_counter()
}
/// Enable interrupts
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-23): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn enable_irq() {
    crate::arch::arch_floor::riscv64::cpu::enable_irq()
}
/// Disable interrupts
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-22): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn disable_irq() {
    crate::arch::arch_floor::riscv64::cpu::disable_irq()
}
/// Wait for interrupt
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-21): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn wfi() {
    crate::arch::arch_floor::riscv64::cpu::wfi()
}
/// Read the wall-clock cycle counter (mtime as seen through rdtime).
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-20): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn read_time() -> u64 {
    crate::arch::arch_floor::riscv64::cpu::read_time()
}
/// Instruction serialization barrier
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-19): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn isb() {
    crate::arch::arch_floor::riscv64::cpu::isb()
}
/// Data synchronization barrier
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-18): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn dsb() {
    crate::arch::arch_floor::riscv64::cpu::dsb()
}
/// Data memory barrier
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-17): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn dmb() {
    crate::arch::arch_floor::riscv64::cpu::dmb()
}
/// Get interrupt mask state
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-16): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_interrupts_state() -> bool {
    crate::arch::arch_floor::riscv64::cpu::get_interrupts_state()
}
/// Save interrupt state and disable interrupts
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-15): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn save_and_disable_irq() -> bool {
    crate::arch::arch_floor::riscv64::cpu::save_and_disable_irq()
}

/// Restore interrupt state
#[inline]
pub fn restore_irq(state: bool) {
    if state {
        enable_irq();
    }
}
