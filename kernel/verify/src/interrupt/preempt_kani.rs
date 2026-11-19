//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for preempt counter bitfield masks.
//!
//! Types copied from: kernel/src/interrupt/preempt.rs

#![cfg(kani)]

pub const PREEMPT_MASK: i32 = 0x0000_00FF;
pub const SOFTIRQ_MASK: i32 = 0x0000_FF00;
pub const HARDIRQ_MASK: i32 = 0x000F_0000;
pub const NMI_MASK: i32 = 0x0010_0000;
pub const PREEMPT_ACTIVE: i32 = 0x0400_0000;

pub const PREEMPT_OFFSET: i32 = 1;
pub const SOFTIRQ_OFFSET: i32 = 1 << 8;
pub const HARDIRQ_OFFSET: i32 = 1 << 16;
pub const NMI_OFFSET: i32 = 1 << 20;

pub fn in_interrupt(pc: i32) -> bool { (pc & (HARDIRQ_MASK | SOFTIRQ_MASK | NMI_MASK)) != 0 }
pub fn in_irq(pc: i32) -> bool { (pc & HARDIRQ_MASK) != 0 }
pub fn in_softirq(pc: i32) -> bool { (pc & SOFTIRQ_MASK) != 0 }
pub fn in_nmi(pc: i32) -> bool { (pc & NMI_MASK) != 0 }
pub fn in_task(pc: i32) -> bool { !in_interrupt(pc) }
pub fn preemptible(pc: i32) -> bool { pc == 0 }

/// INV-PREEMPT-K1: masks are non-overlapping.
#[kani::proof]
fn verify_masks_non_overlapping() {
    let masks = [PREEMPT_MASK, SOFTIRQ_MASK, HARDIRQ_MASK, NMI_MASK];
    for i in 0..masks.len() {
        for j in (i + 1)..masks.len() {
            assert_eq!(masks[i] & masks[j], 0);
        }
    }
}

/// INV-PREEMPT-K2: PREEMPT_ACTIVE doesn't overlap with any mask.
#[kani::proof]
fn verify_preempt_active_no_overlap() {
    assert_eq!(PREEMPT_ACTIVE & PREEMPT_MASK, 0);
    assert_eq!(PREEMPT_ACTIVE & SOFTIRQ_MASK, 0);
    assert_eq!(PREEMPT_ACTIVE & HARDIRQ_MASK, 0);
    assert_eq!(PREEMPT_ACTIVE & NMI_MASK, 0);
}

/// INV-PREEMPT-K3: in_task == !in_interrupt.
#[kani::proof]
fn verify_in_task_complement() {
    let pc: i32 = kani::any();
    kani::assume(pc >= 0 && pc < 0x0500_0000);
    assert_eq!(in_task(pc), !in_interrupt(pc));
}

/// INV-PREEMPT-K4: in_interrupt == in_irq || in_softirq || in_nmi.
#[kani::proof]
fn verify_interrupt_decomposition() {
    let pc: i32 = kani::any();
    kani::assume(pc >= 0 && pc < 0x0500_0000);
    let by_parts = in_irq(pc) || in_softirq(pc) || in_nmi(pc);
    assert_eq!(in_interrupt(pc), by_parts);
}

/// INV-PREEMPT-K5: preemptible only when pc == 0.
#[kani::proof]
fn verify_preemptible_only_zero() {
    let pc: i32 = kani::any();
    kani::assume(pc >= 1 && pc < 0x0500_0000);
    assert!(!preemptible(pc));
    assert!(preemptible(0));
}

/// INV-PREEMPT-K6: irq enter/exit symmetry.
#[kani::proof]
fn verify_irq_enter_exit() {
    let pc: i32 = kani::any();
    kani::assume(pc >= 0 && pc < HARDIRQ_MASK);
    let after_enter = pc + HARDIRQ_OFFSET;
    assert!(in_irq(after_enter));
    let after_exit = after_enter - HARDIRQ_OFFSET;
    assert_eq!(after_exit, pc);
}

/// INV-PREEMPT-K7: mask coverage.
#[kani::proof]
fn verify_mask_coverage() {
    let all = PREEMPT_MASK | SOFTIRQ_MASK | HARDIRQ_MASK | NMI_MASK | PREEMPT_ACTIVE;
    assert_eq!(all, 0x041F_FFFF);
}
