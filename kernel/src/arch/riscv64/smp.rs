//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V SMP (Symmetric Multi-Processing) support
//!
//! Multi-core boot and management framework

use crate::println;
use crate::config::MAX_CPUS;
use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

/// SMP boot stack size - from config
pub const STACK_SIZE: usize = crate::config::SMP_BOOT_STACK_SIZE;

pub const BOOT_HART_ID: usize = 0;

static ACTUAL_BOOT_HART: AtomicU32 = AtomicU32::new(u32::MAX);

static SMP_INIT_DONE: AtomicU32 = AtomicU32::new(0);

static CPU_STARTED: [AtomicU32; MAX_CPUS] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

fn mark_cpu_started(hart_id: usize) {
    if hart_id < MAX_CPUS {
        CPU_STARTED[hart_id].store(1, Ordering::Release);
    }
}

/// Get current CPU's hardware thread ID
///
/// Design:
/// - Early boot phase: tp = hart_id (small value)
/// - After scheduler runs: tp = task_struct pointer, hart_id stored in task_struct.ti_cpu
///
/// Determine current mode by checking tp value range:
/// - If tp < 0x1000, consider it as hart_id (early boot)
/// - Otherwise consider it as task_struct pointer
///
/// Note:
/// - Cannot use mhartid CSR (M-mode only, S-mode access triggers exception)
/// - Must ensure trap.S handles tp register correctly
#[inline]
pub fn cpu_id() -> usize {
    unsafe {
        let tp_value: u64;
        asm!("mv {}, tp", out(reg) tp_value, options(nomem, nostack, pure));

        // Check if tp is a small value (hart_id during early boot phase)
        // Valid task_struct pointers should be in kernel address space (>= 0x80000000)
        if tp_value < 0x1000 {
            // Early boot phase, tp directly stores hart_id
            tp_value as usize
        } else {
            // tp points to task_struct, get hart_id from ti_cpu field
            // ti_cpu offset in Task struct is 0x18 (24 bytes)
            let ti_cpu_offset = 0x18;
            let cpu_ptr = (tp_value as usize + ti_cpu_offset) as *const core::sync::atomic::AtomicI32;
            (*cpu_ptr).load(core::sync::atomic::Ordering::Relaxed) as usize
        }
    }
}

#[inline]
pub fn is_boot_hart() -> bool {
    let actual = ACTUAL_BOOT_HART.load(Ordering::Acquire) as usize;
    if actual != u32::MAX as usize {
        cpu_id() == actual
    } else {
        // If actual boot hart not set yet, fall back to checking if hart 0
        cpu_id() == BOOT_HART_ID
    }
}

#[no_mangle]
pub extern "C" fn secondary_cpu_start() -> ! {
    // Read hart ID from tp register (saved by boot.S)
    let hart_id: usize = cpu_id();

    // Mark CPU as started
    mark_cpu_started(hart_id);

    // Enter idle loop (WFI)
    loop {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}

pub fn init() -> bool {
    let my_hart = cpu_id();

    // Try to become boot core (using CAS operation)
    // Only the first CPU to reach here can successfully set ACTUAL_BOOT_HART
    let mut is_boot_cpu = false;
    if ACTUAL_BOOT_HART.compare_exchange(
        u32::MAX,
        my_hart as u32,
        Ordering::AcqRel,
        Ordering::Acquire
    ).is_ok() {
        is_boot_cpu = true;
    }

    if is_boot_cpu {
        // Mark primary core as started
        mark_cpu_started(my_hart);

        // Wake up other CPUs
        let mut started_count = 0;
        for hart_id in 0..MAX_CPUS {
            if hart_id != my_hart {
                // Secondary core start address: use kernel entry point _start (all CPUs start from _start)
                // external function _start from boot.S
                let start_addr: usize;
                unsafe {
                    asm!(
                        "la {}, _start",
                        out(reg) start_addr,
                        options(nomem, nostack)
                    );
                }

                // Call SBI hart_start
                let ret = sbi_rt::hart_start(hart_id, start_addr, 0);

                // SBI return value: ret.error == 0 means success
                if ret.error == 0 {
                    started_count += 1;
                }
            }
        }

        // Wake all secondary cores first, then set completion flag
        // Ensure secondary cores don't check SMP_INIT_DONE before being woken
        if started_count > 0 {
            // Slight delay to ensure all secondary cores have entered wait loop
            for _ in 0..100 {
                unsafe { asm!("nop", options(nomem, nostack)); }
            }
            // Now set initialization complete flag
            SMP_INIT_DONE.store(1, Ordering::Release);
        }

        is_boot_cpu
    } else {
        // Non-boot core: wait for initialization to complete
        while SMP_INIT_DONE.load(Ordering::Acquire) == 0 {
            unsafe {
                asm!("wfi", options(nomem, nostack));
            }
        }

        // Mark self as started
        mark_cpu_started(my_hart);

        false
    }
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

/// Per-CPU interrupt stacks
/// Each CPU has its own dedicated interrupt stack to avoid races
#[link_section = ".bss"]
#[used]
pub static mut PER_CPU_INTR_STACKS: [[u8; INTR_STACK_SIZE]; MAX_CPUS] = [[0; INTR_STACK_SIZE]; MAX_CPUS];

/// Initialize per-CPU interrupt stack base pointer for assembly code
/// This must be called early in boot before any traps occur
pub fn init_per_cpu_intr_stacks() {
    unsafe {
        // Set the assembly variable to point to our stack array
        extern "C" {
            static mut __per_cpu_intr_stacks_base: usize;
        }
        __per_cpu_intr_stacks_base = PER_CPU_INTR_STACKS.as_ptr() as usize;
    }
}

/// Get current CPU's interrupt stack top address
#[no_mangle]
#[inline(never)]
pub extern "C" fn get_per_cpu_intr_stack_top() -> usize {
    let cpu = cpu_id();
    if cpu < MAX_CPUS {
        unsafe {
            let stack_base = PER_CPU_INTR_STACKS[cpu].as_ptr() as usize;
            stack_base + INTR_STACK_SIZE
        }
    } else {
        // Fallback to CPU 0 stack if CPU ID is invalid
        unsafe {
            let stack_base = PER_CPU_INTR_STACKS[0].as_ptr() as usize;
            stack_base + INTR_STACK_SIZE
        }
    }
}
