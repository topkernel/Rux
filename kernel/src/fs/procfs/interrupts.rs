//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/interrupts - Interrupt statistics
//!
//! Reads per-IRQ per-CPU counters from the irq_desc framework.
//! Timer and software interrupt counters remain local (RISC-V internal,
//! not routed through PLIC/irq_desc).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum number of CPUs — must match config::MAX_CPUS
const MAX_CPUS: usize = crate::config::MAX_CPUS;

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

// ============================================================================
// PLIC counters now live in irq_desc.per_cpu_count
// These wrappers delegate to the IRQ framework.
// ============================================================================

/// Increment PLIC interrupt counter (delegates to irq framework)
pub fn plic_inc(irq: usize, cpu: usize) {
    crate::interrupt::irq_inc_count(irq as u32, cpu);
}

/// Get PLIC interrupt count (delegates to irq framework)
pub fn plic_count(irq: usize, cpu: usize) -> u64 {
    crate::interrupt::irq_get_count(irq as u32, cpu)
}

// ============================================================================
// /proc/interrupts generation
// ============================================================================

/// Generate /proc/interrupts content
pub fn generate() -> Vec<u8> {
    let mut output = String::new();

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

    // PLIC external interrupts - read from irq_desc
    let show_irqs: Vec<usize> = (1..=15).chain(32..=47).collect();

    for irq in show_irqs {
        // IRQ number
        output.push_str(&format!("{:>3}: ", irq));

        // Per-CPU counts from irq_desc
        for cpu in 0..num_cpus {
            let count = crate::interrupt::irq_get_count(irq as u32, cpu);
            output.push_str(&format!(" {:>10}", count));
        }

        // Interrupt controller type
        output.push_str("  PLIC ");

        // IRQ type (edge/level) - PLIC supports level-triggered
        output.push_str("level ");

        // Device/driver name from irq_desc
        let name = crate::interrupt::irq_get_name(irq as u32)
            .unwrap_or(if irq >= 32 && irq < 128 { "virtio-pci" } else { "unknown" });
        output.push_str(name);
        output.push_str("\n");
    }

    // ERR count (spurious interrupts)
    output.push_str("ERR:          0\n");

    output.into_bytes()
}
