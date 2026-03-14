//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/interrupts - Interrupt statistics
//!
//! Reference: Linux fs/proc/interrupts.c, arch/riscv/kernel/irq.c

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum number of CPUs
const MAX_CPUS: usize = 4;

/// Maximum number of PLIC IRQs
const MAX_IRQS: usize = 128;

// ============================================================================
// Per-CPU local interrupt counters (RISC-V internal, not via PLIC)
// ============================================================================

/// Timer interrupt counters per CPU
static TIMER_COUNT: [AtomicU64; MAX_CPUS] = [
    AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0),
];

/// Software interrupt (IPI) counters per CPU
static SOFT_COUNT: [AtomicU64; MAX_CPUS] = [
    AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0),
];

// ============================================================================
// PLIC external interrupt counters per CPU
// ============================================================================

/// External interrupt counters for CPU 0
static PLIC_COUNT_CPU0: [AtomicU64; MAX_IRQS] = init_counters();
/// External interrupt counters for CPU 1
static PLIC_COUNT_CPU1: [AtomicU64; MAX_IRQS] = init_counters();
/// External interrupt counters for CPU 2
static PLIC_COUNT_CPU2: [AtomicU64; MAX_IRQS] = init_counters();
/// External interrupt counters for CPU 3
static PLIC_COUNT_CPU3: [AtomicU64; MAX_IRQS] = init_counters();

/// Initialize counter array
const fn init_counters() -> [AtomicU64; MAX_IRQS] {
    [const { AtomicU64::new(0) }; MAX_IRQS]
}

/// Get PLIC counter array for CPU
fn get_plic_counters(cpu: usize) -> &'static [AtomicU64; MAX_IRQS] {
    match cpu {
        0 => &PLIC_COUNT_CPU0,
        1 => &PLIC_COUNT_CPU1,
        2 => &PLIC_COUNT_CPU2,
        3 => &PLIC_COUNT_CPU3,
        _ => &PLIC_COUNT_CPU0,
    }
}

// ============================================================================
// Counter increment functions (called from trap handler)
// ============================================================================

/// Increment timer interrupt counter
pub fn timer_inc(cpu: usize) {
    if cpu < MAX_CPUS {
        TIMER_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
    }
}

/// Increment software interrupt counter
pub fn soft_inc(cpu: usize) {
    if cpu < MAX_CPUS {
        SOFT_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
    }
}

/// Increment PLIC external interrupt counter
pub fn plic_inc(irq: usize, cpu: usize) {
    if irq < MAX_IRQS && cpu < MAX_CPUS {
        get_plic_counters(cpu)[irq].fetch_add(1, Ordering::Relaxed);
    }
}

// ============================================================================
// Counter read functions
// ============================================================================

/// Get timer interrupt count
pub fn timer_count(cpu: usize) -> u64 {
    if cpu < MAX_CPUS {
        TIMER_COUNT[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Get software interrupt count
pub fn soft_count(cpu: usize) -> u64 {
    if cpu < MAX_CPUS {
        SOFT_COUNT[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Get PLIC interrupt count
pub fn plic_count(irq: usize, cpu: usize) -> u64 {
    if irq < MAX_IRQS && cpu < MAX_CPUS {
        get_plic_counters(cpu)[irq].load(Ordering::Relaxed)
    } else {
        0
    }
}

// ============================================================================
// /proc/interrupts generation
// ============================================================================

/// IRQ descriptions for known PLIC interrupts
static IRQ_DESCS: [&str; 16] = [
    "",              // 0: reserved
    "virtio-mmio",   // 1: VirtIO MMIO
    "virtio-mmio",   // 2: VirtIO MMIO
    "virtio-mmio",   // 3: VirtIO MMIO
    "virtio-mmio",   // 4: VirtIO MMIO
    "virtio-mmio",   // 5: VirtIO MMIO
    "virtio-mmio",   // 6: VirtIO MMIO
    "virtio-mmio",   // 7: VirtIO MMIO
    "virtio-mmio",   // 8: VirtIO MMIO
    "",              // 9: reserved
    "uart",          // 10: UART (ns16550a)
    "",              // 11: reserved
    "",              // 12: reserved
    "",              // 13: reserved
    "",              // 14: reserved
    "",              // 15: reserved
];

/// Generate /proc/interrupts content
pub fn generate() -> Vec<u8> {
    let mut output = String::new();

    // Get number of online CPUs
    let num_cpus = crate::arch::riscv64::smp::num_started_cpus().min(MAX_CPUS);

    // Header: CPU0 CPU1 CPU2 CPU3 ...
    output.push_str("           ");
    for cpu in 0..num_cpus {
        output.push_str(&format!(" {:>10}", format!("CPU{}", cpu)));
    }
    output.push_str("\n");

    // RISC-V Timer interrupt (local, not via PLIC)
    output.push_str("TMR:      ");
    for cpu in 0..num_cpus {
        output.push_str(&format!(" {:>10}", timer_count(cpu)));
    }
    output.push_str("  RISC-V Timer\n");

    // RISC-V Software interrupt (for IPI, local)
    output.push_str("SWI:      ");
    for cpu in 0..num_cpus {
        output.push_str(&format!(" {:>10}", soft_count(cpu)));
    }
    output.push_str("  RISC-V Software IPI\n");

    // PLIC external interrupts - always show known IRQs
    // Show IRQs 1-15 (VirtIO MMIO, UART) and 32-47 (VirtIO PCI)
    let show_irqs: Vec<usize> = (1..=15).chain(32..=47).collect();

    for irq in show_irqs {
        // IRQ number
        output.push_str(&format!("{:>3}: ", irq));

        // Per-CPU counts
        for cpu in 0..num_cpus {
            output.push_str(&format!(" {:>10}", plic_count(irq, cpu)));
        }

        // Interrupt controller type
        output.push_str("  PLIC ");

        // IRQ type (edge/level) - PLIC supports level-triggered
        output.push_str("level ");

        // Device/driver name
        let name = if irq < IRQ_DESCS.len() && !IRQ_DESCS[irq].is_empty() {
            IRQ_DESCS[irq]
        } else if irq >= 32 && irq < 128 {
            "virtio-pci"
        } else {
            "unknown"
        };
        output.push_str(name);
        output.push_str("\n");
    }

    // ERR count (spurious interrupts)
    output.push_str("ERR:          0\n");

    output.into_bytes()
}
