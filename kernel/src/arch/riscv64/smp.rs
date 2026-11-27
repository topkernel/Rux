//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V SMP (Symmetric Multi-Processing) support
//!
//! Boot flow:
//! 1. OpenSBI picks one hart as boot hart and starts it in S-mode
//! 2. Boot hart runs _start → rust_main → full kernel init
//! 3. After scheduler init, boot hart calls start_secondaries()
//! 4. start_secondaries() uses SBI HSM hart_start() for each secondary hart
//! 5. Secondary harts enter secondary_start (boot.S), set up MMU, jump to
//!    secondary_cpu_entry()


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use crate::println;
use crate::config::MAX_CPUS;
use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};


/// SMP boot stack size - from config
pub const STACK_SIZE: usize = crate::config::SMP_BOOT_STACK_SIZE;

pub(crate) static CPU_STARTED: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];

/// Actual boot hart ID, saved by init(). QEMU/OpenSBI may pick any hart.
pub(crate) static BOOT_HART_ID: AtomicU32 = AtomicU32::new(u32::MAX);

/// Get the boot hart ID (the hart that ran rust_main).
pub fn boot_hart_id() -> usize {
    BOOT_HART_ID.load(Ordering::Acquire) as usize
}

pub(crate) fn mark_cpu_started(hart_id: usize) {
    if hart_id < MAX_CPUS {
        CPU_STARTED[hart_id].store(1, Ordering::Release);
    }
}

/// Whether hart `cpu` has entered secondary_cpu_entry (or is the boot hart).
///
/// Callers must use this instead of indexing 0..MAX_CPUS blindly: QEMU may be
/// started with fewer harts than the compile-time MAX_CPUS (-smp 2 with
/// MAX_CPUS=4). Hart_start for a nonexistent hart fails with SBI_INVALID_PARAM
/// and the CPU never comes online.
pub fn cpu_started(cpu: usize) -> bool {
    cpu < MAX_CPUS && CPU_STARTED[cpu].load(Ordering::Acquire) == 1
}
/// Get current CPU's hardware thread ID
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-19): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn cpu_id() -> usize {
    crate::arch::arch_floor::riscv64::smp::cpu_id()
}

#[inline]
pub fn is_boot_hart() -> bool {
    let boot = BOOT_HART_ID.load(Ordering::Acquire);
    if boot == u32::MAX {
        // Very early: not yet initialized, fall back to checking CPU 0
        cpu_id() == 0
    } else {
        cpu_id() == boot as usize
    }
}

/// SMP init — called from rust_main by the boot hart.
/// On QEMU virt, OpenSBI only starts one hart into S-mode.
/// Returns true (always the boot hart).
pub fn init() -> bool {
    let my_hart = cpu_id();
    BOOT_HART_ID.store(my_hart as u32, Ordering::Release);
    mark_cpu_started(my_hart);
    true
}

extern "C" {
    /// Assembly entry point in boot.S for secondary CPUs.
    fn secondary_start();
}

/// Per-CPU boot stacks for secondary CPUs.
#[repr(align(16))]
pub(crate) struct BootStack(pub(crate) [u8; STACK_SIZE]);

pub(crate) static SECONDARY_BOOT_STACKS: [BootStack; MAX_CPUS] =
    [const { BootStack([0u8; STACK_SIZE]) }; MAX_CPUS];

/// VA_OFFSET for converting virtual addresses to physical.
pub(crate) const VA_OFFSET: usize = 0xffffffff80000000usize - 0x80200000usize;


/// Flag set by boot CPU after ALL initialization is complete.
/// Secondary CPUs spin-wait on this flag before doing anything.
pub(crate) static BOOT_COMPLETE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Called by boot CPU just before entering cpu_idle_loop.
/// Signals secondary CPUs that they may now participate in scheduling.
pub fn signal_boot_complete() {
    BOOT_COMPLETE.store(true, core::sync::atomic::Ordering::Release);
}

/// Called by secondary CPUs to check if boot is complete.
pub fn is_boot_complete() -> bool {
    BOOT_COMPLETE.load(core::sync::atomic::Ordering::Acquire)
}
/// Start secondary CPUs via SBI HSM hart_start.
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-17): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn start_secondaries() {
    crate::arch::arch_floor::riscv64::smp::start_secondaries()
}


pub fn num_started_cpus() -> usize {
    let mut count = 0;
    for i in 0..MAX_CPUS {
        if CPU_STARTED[i].load(Ordering::Acquire) == 1 {
            count += 1;
        }
    }
    count
}

// ==================== Per-CPU Interrupt Stack ====================

/// Per-CPU interrupt stack size (16KB each)
pub const INTR_STACK_SIZE: usize = 16384;

/// Aligned stack wrapper to ensure 16-byte alignment
#[repr(C, align(16))]
pub(crate) struct AlignedStack(pub(crate) [u8; INTR_STACK_SIZE]);

impl AlignedStack {
    /// Get a pointer to the underlying stack buffer
    pub const fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr()
    }
}

/// Initialize per-CPU interrupt stack base pointer for assembly code
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-14): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn init_per_cpu_intr_stacks() {
    crate::arch::arch_floor::riscv64::smp::init_per_cpu_intr_stacks()
}

// Collected-unsafe re-exports: the unsafe floor for these names lives in
// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.
// INV-ARCH note: get_per_cpu_intr_stack_top moved to arch::arch_floor::riscv64::smp (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::smp::get_per_cpu_intr_stack_top;
pub(crate) use crate::arch::arch_floor::riscv64::smp::PER_CPU_INTR_STACKS;
// INV-ARCH note: secondary_cpu_entry moved to arch::arch_floor::riscv64::smp (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::smp::secondary_cpu_entry;
pub(crate) use crate::arch::arch_floor::riscv64::smp::BOOT_HART_SATP;
