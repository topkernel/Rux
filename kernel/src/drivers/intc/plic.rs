//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V PLIC (Platform-Level Interrupt Controller) driver

use core::arch::asm;
use crate::println;

// PLIC base address - QEMU virt platform uses 0x0c000000
// NOTE: Must use plain hex digits (0x0c000000) not (0x0c00_0000) to avoid
// the compiler dropping the leading zero!
const PLIC_BASE: usize = 201326592;  // 0x0c000000 in decimal

mod offset {
    // Priority registers (4 bytes per interrupt)
    pub const PRIORITY: usize = 0x0000;

    // Pending interrupt registers (4 bytes per read)
    pub const PENDING: usize = 0x1000;

    // Enable registers (one group per hart)
    pub const ENABLE: usize = 0x2000;

    // Threshold register (one per hart)
    // Located at context offset 0x0000
    pub const THRESHOLD: usize = 0x0000;

    // Claim register (one per hart)
    // Complete register (one per hart)
    // Located at context offset 0x0004
    pub const CLAIM_COMPLETE: usize = 0x0004;
}

pub const MAX_INTERRUPTS: usize = 128;

const CONTEXT_SIZE: usize = 0x1000;

pub const PLIC_PRIORITY_BASE: u32 = 1;
pub const PLIC_PRIORITY_MIN: u32 = 0;
pub const PLIC_PRIORITY_MAX: u32 = 7;

pub struct Plic {
    base: usize,
    num_harts: usize,
}

impl Plic {
    /// Create new PLIC instance
    pub const fn new(base: usize, num_harts: usize) -> Self {
        Self {
            base,
            num_harts,
        }
    }

    /// Initialize PLIC
    ///
    /// Disable all interrupts, set threshold
    pub fn init(&self) {
        // Disable all interrupts (set priority to 0, meaning disabled)
        for irq in 1..MAX_INTERRUPTS {
            self.set_priority(irq, 0);
        }

        // Set threshold for each hart (only respond to interrupts with priority > threshold)
        for hart in 0..self.num_harts {
            self.set_threshold(hart, 0);
        }

        // Disable interrupts for all harts
        for hart in 0..self.num_harts {
            for irq_in_word in 0..(MAX_INTERRUPTS / 32) {
                self.disable_interrupts(hart, irq_in_word);
            }
        }
    }

    /// Set interrupt priority
    fn set_priority(&self, irq: usize, priority: u32) {
        let addr = self.base + offset::PRIORITY + irq * 4;
        unsafe {
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") priority,
                options(nostack)
            );
        }
    }

    /// Set hart's interrupt threshold
    ///
    /// Only interrupts with priority > threshold will be delivered to hart
    fn set_threshold(&self, hart: usize, threshold: u32) {
        let addr = self.base + offset::THRESHOLD + hart * CONTEXT_SIZE;
        unsafe {
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") threshold,
                options(nostack)
            );
        }
    }

    /// Enable interrupt for specified hart
    pub fn enable_interrupt(&self, hart: usize, irq: usize) {
        // First set interrupt priority (must be > 0 to trigger)
        self.set_priority(irq, PLIC_PRIORITY_BASE);

        // Then set corresponding bit in ENABLE register
        let word = irq / 32;
        let bit = irq % 32;
        let addr = self.base + offset::ENABLE + hart * CONTEXT_SIZE + word * 4;

        unsafe {
            let value: u32;
            asm!(
                "lw {}, 0({})",
                out(reg) value,
                in(reg) addr,
                options(nostack)
            );

            // Set corresponding bit
            let new_value = value | (1 << bit);

            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") new_value,
                options(nostack)
            );
        }
    }

    /// Disable interrupts for specified hart (disable all interrupts in a 32-bit word)
    fn disable_interrupts(&self, hart: usize, word: usize) {
        let addr = self.base + offset::ENABLE + hart * CONTEXT_SIZE + word * 4;
        unsafe {
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") 0u32,
                options(nostack)
            );
        }
    }

    /// Claim interrupt
    ///
    /// Returns highest priority pending interrupt ID
    pub fn claim(&self, hart: usize) -> Option<usize> {
        let addr = self.base + offset::CLAIM_COMPLETE + hart * CONTEXT_SIZE + 0x4;

        unsafe {
            let irq: u32;
            asm!(
                "lw {}, 0({})",
                out(reg) irq,
                in(reg) addr,
                options(nostack)
            );

            if irq == 0 {
                None
            } else {
                Some(irq as usize)
            }
        }
    }

    /// Complete interrupt
    ///
    /// Notify PLIC that interrupt handling is complete
    pub fn complete(&self, hart: usize, irq: usize) {
        let addr = self.base + offset::CLAIM_COMPLETE + hart * CONTEXT_SIZE + 0x4;

        unsafe {
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") irq as u32,
                options(nostack)
            );
        }
    }

    /// Read pending interrupt status
    pub fn read_pending(&self) -> u32 {
        let addr = self.base + offset::PENDING;

        unsafe {
            let pending: u32;
            asm!(
                "lw {}, 0({})",
                out(reg) pending,
                in(reg) addr,
                options(nostack)
            );

            pending
        }
    }

    /// Trigger software interrupt (IPI)
    ///
    /// Note: Standard PLIC does not support software-triggered interrupts
    /// This function writes directly to PENDING register to simulate interrupt
    /// Only works in emulation environments like QEMU virt
    pub fn trigger_ipi(&self, irq: usize) {
        if irq >= 32 {
            // PENDING register is 32-bit, only supports IRQ 0-31
            return;
        }

        let addr = self.base + offset::PENDING;

        unsafe {
            // Read current PENDING status
            let pending: u32;
            asm!(
                "lw {}, 0({})",
                out(reg) pending,
                in(reg) addr,
                options(nostack)
            );

            // Set corresponding bit
            let new_pending = pending | (1 << irq);

            // Write back to PENDING register
            asm!(
                "sw t1, 0(a0)",
                in("a0") addr,
                in("t1") new_pending,
                options(nostack)
            );
        }
    }
}

static PLIC: Plic = Plic::new(PLIC_BASE, 4);

pub fn init() {
    PLIC.init();

    // Enable key interrupts
    // RISC-V virt platform interrupt mapping (QEMU):
    // - IRQ 1-8: VirtIO devices (8 VirtIO slots)
    // - IRQ 10: UART (ns16550a)
    // - IRQ 11-13: IPI (software interrupts, for inter-core communication)
    let boot_hart = crate::arch::riscv64::smp::cpu_id();

    // Enable VirtIO device interrupts for boot hart
    // IRQ 1 is first VirtIO device (usually VirtIO-Blk)
    PLIC.enable_interrupt(boot_hart, 1);
    // Also enable IRQ for other VirtIO slots (in case there are multiple VirtIO devices)
    // IRQ 2-8 correspond to VirtIO slots 1-7
    for virtio_irq in 2..9 {  // 2 to 8 (inclusive)
        PLIC.enable_interrupt(boot_hart, virtio_irq);
    }

    // Enable UART interrupt for boot hart (QEMU RISC-V virt: IRQ 10)
    PLIC.enable_interrupt(boot_hart, 10);

    // Enable IPI interrupts (for inter-core communication)
    for hart in 0..4 {
        for ipi_irq in 11..14 {  // 11-13: IPI
            PLIC.enable_interrupt(hart, ipi_irq);
        }
    }
}

pub fn claim(hart: usize) -> Option<usize> {
    PLIC.claim(hart)
}

pub fn complete(hart: usize, irq: usize) {
    PLIC.complete(hart, irq)
}

pub fn enable_interrupt(hart: usize, irq: usize) {
    PLIC.enable_interrupt(hart, irq);
}

pub fn read_pending() -> u32 {
    PLIC.read_pending()
}

pub fn trigger_ipi(irq: usize) {
    PLIC.trigger_ipi(irq)
}
