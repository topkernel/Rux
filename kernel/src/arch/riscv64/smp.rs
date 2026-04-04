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

use crate::println;
use crate::config::MAX_CPUS;
use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

/// SMP boot stack size - from config
pub const STACK_SIZE: usize = crate::config::SMP_BOOT_STACK_SIZE;

static CPU_STARTED: [AtomicU32; MAX_CPUS] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

/// Actual boot hart ID, saved by init(). QEMU/OpenSBI may pick any hart.
static BOOT_HART_ID: AtomicU32 = AtomicU32::new(u32::MAX);

/// Get the boot hart ID (the hart that ran rust_main).
pub fn boot_hart_id() -> usize {
    BOOT_HART_ID.load(Ordering::Acquire) as usize
}

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
struct BootStack([u8; STACK_SIZE]);

static SECONDARY_BOOT_STACKS: [BootStack; MAX_CPUS] = [
    BootStack([0u8; STACK_SIZE]),
    BootStack([0u8; STACK_SIZE]),
    BootStack([0u8; STACK_SIZE]),
    BootStack([0u8; STACK_SIZE]),
];

/// VA_OFFSET for converting virtual addresses to physical.
const VA_OFFSET: usize = 0xffffffff80000000usize - 0x80200000usize;

/// Saved satp from boot hart (permanent page table for secondaries to use)
#[no_mangle]
static BOOT_HART_SATP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Flag set by boot CPU after ALL initialization is complete.
/// Secondary CPUs spin-wait on this flag before doing anything.
static BOOT_COMPLETE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

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
/// Must be called AFTER console + scheduler initialization.
pub fn start_secondaries() {
    // Save current satp (permanent page table) for secondaries
    let current_satp: u64;
    unsafe {
        core::arch::asm!("csrr {}, satp", out(reg) current_satp, options(nomem, nostack));
    }
    BOOT_HART_SATP.store(current_satp, Ordering::Release);

    let start_addr = unsafe { secondary_start as usize - VA_OFFSET };
    let my_hart = cpu_id();
    println!("smp: boot hart={}, start_addr={:#x}, starting secondaries...", my_hart, start_addr);

    for hart in 0..MAX_CPUS {
        if hart == my_hart { continue; }

        // Allocate stack for this hart (virtual address, convert to PA)
        let base = SECONDARY_BOOT_STACKS[hart].0.as_ptr() as usize;
        let stack_top_pa = (base + STACK_SIZE) - VA_OFFSET;

        let ret = sbi_rt::hart_start(hart, start_addr, stack_top_pa);
        if ret.error != 0 {
            println!("smp: hart {} start failed (error={})", hart, ret.error);
        } else {
            println!("smp: hart {} started", hart);
        }
    }

    // Wait for secondaries to come online
    for _ in 0..50_000_000 {
        if num_started_cpus() == MAX_CPUS {
            break;
        }
        core::hint::spin_loop();
    }

    let cpu_count = num_started_cpus();
    if cpu_count > 1 {
        println!("smp: {} CPUs online", cpu_count);
    }
}

/// Rust entry point for secondary CPUs.
/// Called from assembly `secondary_start` in boot.S after MMU is enabled.
/// a0 = hartid
#[no_mangle]
pub extern "C" fn secondary_cpu_entry(hart_id: usize) -> ! {
    // Mark this CPU as started so the boot CPU's start_secondaries()
    // spin-wait can detect us and proceed.
    mark_cpu_started(hart_id);

    // Set up trap handling early so WFI below can actually wake on timer.
    // Without this, sie=0 and sstatus.SIE=0, so WFI never resumes in QEMU.
    crate::arch::riscv64::trap::init();

    crate::arch::riscv64::trap::enable_timer_interrupt();

    // Spin until boot CPU has finished ALL single-CPU initialization
    // (devfs mknod, evdev, init ELF loading, etc.).
    // Timer interrupts are now enabled, so WFI will wake periodically.
    while !is_boot_complete() {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
    }

    // Boot CPU has finished init — safe to call kmalloc now.
    crate::sched::init_secondary(hart_id);

    // Enable external interrupts (sie.SEIE) for this hart
    crate::arch::riscv64::trap::enable_external_interrupt();

    crate::pr_info!("sched: cpu {} online", hart_id);

    // Enter scheduler idle loop (timer interrupts enabled inside the loop)
    crate::sched::cpu_idle_loop();
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
struct AlignedStack([u8; INTR_STACK_SIZE]);

impl AlignedStack {
    /// Get a pointer to the underlying stack buffer
    pub const fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr()
    }
}

/// Per-CPU interrupt stacks
/// Each CPU has its own dedicated interrupt stack to avoid races
/// Uses AlignedStack wrapper to ensure 16-byte alignment for proper PtRegs access
#[link_section = ".bss"]
#[used]
pub static mut PER_CPU_INTR_STACKS: [AlignedStack; MAX_CPUS] = [const { AlignedStack([0u8; INTR_STACK_SIZE]) }; MAX_CPUS];

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
