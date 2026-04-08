//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for RISC-V Cause parsing and CSR bit definitions.
//!
//! Types copied from: kernel/src/arch/riscv64/pt_regs.rs

#![cfg(kani)]

pub const SR_SPP: u64 = 1 << 8;
pub const SR_PIE: u64 = 1 << 5;
pub const SR_SIE: u64 = 1 << 1;
pub const SR_SUM: u64 = 1 << 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    InstructionAddressMisaligned = 0,
    InstructionAccessFault = 1,
    IllegalInstruction = 2,
    Breakpoint = 3,
    LoadAddressMisaligned = 4,
    LoadAccessFault = 5,
    StoreAmoAddressMisaligned = 6,
    StoreAmoAccessFault = 7,
    EcallUser = 8,
    EcallSupervisor = 9,
    EcallMachine = 11,
    InstructionPageFault = 12,
    LoadPageFault = 13,
    StoreAmoPageFault = 15,
    SupervisorSoft = 64,
    SupervisorTimer = 68,
    SupervisorExternal = 72,
}

impl Cause {
    pub fn from_cause(cause: u64) -> Self {
        if cause & (1u64 << 63) != 0 {
            let code = cause & !(1u64 << 63);
            match code {
                1 => Cause::SupervisorSoft,
                5 => Cause::SupervisorTimer,
                9 => Cause::SupervisorExternal,
                _ => Cause::IllegalInstruction,
            }
        } else {
            match cause {
                0 => Cause::InstructionAddressMisaligned,
                1 => Cause::InstructionAccessFault,
                2 => Cause::IllegalInstruction,
                3 => Cause::Breakpoint,
                4 => Cause::LoadAddressMisaligned,
                5 => Cause::LoadAccessFault,
                6 => Cause::StoreAmoAddressMisaligned,
                7 => Cause::StoreAmoAccessFault,
                8 => Cause::EcallUser,
                9 => Cause::EcallSupervisor,
                11 => Cause::EcallMachine,
                12 => Cause::InstructionPageFault,
                13 => Cause::LoadPageFault,
                15 => Cause::StoreAmoPageFault,
                _ => Cause::IllegalInstruction,
            }
        }
    }

    pub fn is_interrupt(&self) -> bool {
        matches!(self,
            Cause::SupervisorSoft |
            Cause::SupervisorTimer |
            Cause::SupervisorExternal)
    }

    pub fn is_page_fault(&self) -> bool {
        matches!(self,
            Cause::InstructionPageFault |
            Cause::LoadPageFault |
            Cause::StoreAmoPageFault)
    }
}

/// INV-CAUSE-K1: ecall from user mode is an exception, not interrupt/page-fault.
#[kani::proof]
fn verify_ecall_user() {
    let c = Cause::from_cause(8);
    assert_eq!(c, Cause::EcallUser);
    assert!(!c.is_interrupt());
    assert!(!c.is_page_fault());
}

/// INV-CAUSE-K2: supervisor timer interrupt is an interrupt, not page-fault.
#[kani::proof]
fn verify_supervisor_timer() {
    let c = Cause::from_cause(1u64 << 63 | 5);
    assert_eq!(c, Cause::SupervisorTimer);
    assert!(c.is_interrupt());
    assert!(!c.is_page_fault());
}

/// INV-CAUSE-K3: all page faults are exceptions, not interrupts.
#[kani::proof]
fn verify_page_faults_not_interrupts() {
    let c1 = Cause::from_cause(12);
    assert!(c1.is_page_fault());
    assert!(!c1.is_interrupt());

    let c2 = Cause::from_cause(13);
    assert!(c2.is_page_fault());
    assert!(!c2.is_interrupt());

    let c3 = Cause::from_cause(15);
    assert!(c3.is_page_fault());
    assert!(!c3.is_interrupt());
}

/// INV-CAUSE-K4: unknown exception/interrupt maps to IllegalInstruction.
#[kani::proof]
fn verify_unknown_maps_to_illegal() {
    let code: u64 = kani::any();
    kani::assume(code > 15 && code < 1000);
    assert_eq!(Cause::from_cause(code), Cause::IllegalInstruction);

    let irq = 1u64 << 63 | code;
    // Only codes 1, 5, 9 are recognized interrupts
    if code != 1 && code != 5 && code != 9 {
        assert_eq!(Cause::from_cause(irq), Cause::IllegalInstruction);
    }
}

/// INV-CAUSE-K5: CSR bits SR_SPP, SR_PIE, SR_SIE, SR_SUM are distinct powers of 2.
#[kani::proof]
fn verify_csr_bits_distinct() {
    let bits = [SR_SPP, SR_PIE, SR_SIE, SR_SUM];
    for i in 0..bits.len() {
        for j in (i + 1)..bits.len() {
            assert_eq!(bits[i] & bits[j], 0);
        }
        assert!(bits[i] > 0 && (bits[i] & (bits[i] - 1)) == 0);
    }
}
