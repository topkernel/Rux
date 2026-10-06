//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 SMP: per-CPU state, secondary bring-up (SIPI later).
//!
//! Per-CPU data follows the cache-line-partitioned discipline
//! (docs/development/rust-kernel-best-practices.md §1 + survey A1):
//! CPU-private fields and cross-CPU fields never share a line, and the
//! CPU publishes its own id inside the struct.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[repr(C)]
#[repr(align(64))]
pub struct PerCpu {
    // --- CPU-private line ---
    /// Current task pointer (0 = idle/none)
    pub current_task: AtomicU64,
    /// Kernel stack top for TSS.rsp0 (private)
    pub kernel_stack_top: AtomicU64,
    _pad_private: [u64; 6],

    // --- Cross-CPU line ---
    /// This CPU's number, published once at startup (0 = offline)
    pub cpu_online: AtomicU32,
    /// Started-secondary wait flag (SIPI handshake, X86-TODO)
    pub started: AtomicU32,
    _pad_shared: [u64; 6],
}

impl PerCpu {
    const fn new() -> Self {
        PerCpu {
            current_task: AtomicU64::new(0),
            kernel_stack_top: AtomicU64::new(0),
            _pad_private: [0; 6],
            cpu_online: AtomicU32::new(0),
            started: AtomicU32::new(0),
            _pad_shared: [0; 6],
        }
    }
}

/// The boot CPU uses slot 0; SIPI secondaries take 1..MAX_CPUS
pub static PER_CPU: [PerCpu; crate::config::MAX_CPUS] =
    [const { PerCpu::new() }; crate::config::MAX_CPUS];

/// Initialize the boot CPU's slot
pub fn init() {
    PER_CPU[0].cpu_online.store(1, Ordering::Release);
    PER_CPU[0].started.store(1, Ordering::Release);
}

/// (interface parity — per-CPU interrupt stacks are IST/TSS on x86)
pub fn init_per_cpu_intr_stacks() {}

/// Number of CPUs online (1 until SIPI lands)
pub fn num_started_cpus() -> usize {
    PER_CPU.iter().filter(|c| c.started.load(Ordering::Acquire) == 1).count()
}

pub fn cpu_started(cpu: usize) -> bool {
    PER_CPU[cpu].started.load(Ordering::Acquire) == 1
}

/// Is this the boot CPU? (x86: always CPU 0 in the BSP)
pub fn is_boot_hart() -> bool {
    true
}

/// Bring up secondaries. X86-TODO(agent x86-trap, later phase): SIPI via
/// LAPIC delivery to a low-memory trampoline with a real mode stub.
pub fn start_secondaries() {}

/// Current CPU number (interface parity with riscv64::smp::cpu_id)
pub fn cpu_id() -> u64 {
    // X86-TODO(SMP): with GS-based per-CPU base this reads the real CPU id;
    // single-CPU bring-up is always 0.
    0
}

// ---- current-task slot ----

pub fn current_task_ptr() -> u64 {
    PER_CPU[crate::arch::cpu_id() as usize].current_task.load(Ordering::Acquire)
}

pub fn set_current_task_ptr(task: u64) {
    PER_CPU[crate::arch::cpu_id() as usize].current_task.store(task, Ordering::Release);
}
