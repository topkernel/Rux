//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Cause parsing and CSR bit definition invariant tests.
//!
//! Types copied from: kernel/src/arch/riscv64/pt_regs.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/arch/riscv64/pt_regs.rs
// ============================================================================

pub const SR_SPP: u64 = 1 << 8;
pub const SR_PIE: u64 = 1 << 5;
pub const SR_SIE: u64 = 1 << 1;
pub const SR_SUM: u64 = 1 << 18;
pub const SR_UXL_32: u64 = 1 << 32;
pub const SR_UXL_64: u64 = 2 << 32;
pub const SR_FS_OFF: u64 = 0 << 13;
pub const SR_FS_INITIAL: u64 = 1 << 13;
pub const SR_FS_CLEAN: u64 = 2 << 13;
pub const SR_FS_DIRTY: u64 = 3 << 13;
pub const SR_FS: u64 = 3 << 13;
pub const SR_VS_OFF: u64 = 0 << 9;
pub const SR_VS_INITIAL: u64 = 1 << 9;
pub const SR_VS_CLEAN: u64 = 2 << 9;
pub const SR_VS_DIRTY: u64 = 3 << 9;

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

    pub fn is_exception(&self) -> bool {
        !self.is_interrupt()
    }

    pub fn is_page_fault(&self) -> bool {
        matches!(self,
            Cause::InstructionPageFault |
            Cause::LoadPageFault |
            Cause::StoreAmoPageFault)
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-CAUSE-1: ecall from user mode
    #[test]
    fn test_ecall_user(_v in 0u8..1u8) {
        let c = Cause::from_cause(8);
        prop_assert_eq!(c, Cause::EcallUser);
        prop_assert!(c.is_exception());
        prop_assert!(!c.is_interrupt());
        prop_assert!(!c.is_page_fault());
    }

    /// INV-CAUSE-2: supervisor timer interrupt
    #[test]
    fn test_supervisor_timer(_v in 0u8..1u8) {
        let c = Cause::from_cause(1u64 << 63 | 5);
        prop_assert_eq!(c, Cause::SupervisorTimer);
        prop_assert!(c.is_interrupt());
        prop_assert!(!c.is_page_fault());
    }

    /// INV-CAUSE-3: instruction page fault
    #[test]
    fn test_inst_page_fault(_v in 0u8..1u8) {
        let c = Cause::from_cause(12);
        prop_assert_eq!(c, Cause::InstructionPageFault);
        prop_assert!(c.is_page_fault());
        prop_assert!(c.is_exception());
    }

    /// INV-CAUSE-4: load page fault
    #[test]
    fn test_load_page_fault(_v in 0u8..1u8) {
        let c = Cause::from_cause(13);
        prop_assert_eq!(c, Cause::LoadPageFault);
        prop_assert!(c.is_page_fault());
    }

    /// INV-CAUSE-5: store page fault
    #[test]
    fn test_store_page_fault(_v in 0u8..1u8) {
        let c = Cause::from_cause(15);
        prop_assert_eq!(c, Cause::StoreAmoPageFault);
        prop_assert!(c.is_page_fault());
    }

    /// INV-CAUSE-6: unknown exception maps to IllegalInstruction
    #[test]
    fn test_unknown_exception(code in 100u64..1000u64) {
        let c = Cause::from_cause(code);
        prop_assert_eq!(c, Cause::IllegalInstruction);
    }

    /// INV-CAUSE-7: unknown interrupt maps to IllegalInstruction
    #[test]
    fn test_unknown_interrupt(code in 100u64..1000u64) {
        let c = Cause::from_cause(1u64 << 63 | code);
        prop_assert_eq!(c, Cause::IllegalInstruction);
    }

    /// INV-CAUSE-8: SR_SPP, SR_PIE, SR_SIE are distinct powers of 2
    #[test]
    fn test_csr_bits_distinct(_v in 0u8..1u8) {
        let bits = [SR_SPP, SR_PIE, SR_SIE, SR_SUM];
        let mut seen = 0u64;
        for b in &bits {
            prop_assert_eq!(*b & (*b - 1), 0, "CSR bit {} not power of 2", b);
            prop_assert_eq!(seen & b, 0, "CSR bit {} overlaps", b);
            seen |= b;
        }
    }

    /// INV-CAUSE-9: SR_FS values cover full range 0..3
    #[test]
    fn test_fs_values(_v in 0u8..1u8) {
        let fs_vals = [SR_FS_OFF, SR_FS_INITIAL, SR_FS_CLEAN, SR_FS_DIRTY];
        prop_assert_eq!(fs_vals[0] >> 13, 0);
        prop_assert_eq!(fs_vals[1] >> 13, 1);
        prop_assert_eq!(fs_vals[2] >> 13, 2);
        prop_assert_eq!(fs_vals[3] >> 13, 3);
    }

    /// INV-CAUSE-10: SR_FS == SR_FS_OFF | SR_FS_INITIAL | SR_FS_CLEAN | SR_FS_DIRTY
    #[test]
    fn test_fs_mask(_v in 0u8..1u8) {
        let all = SR_FS_OFF | SR_FS_INITIAL | SR_FS_CLEAN | SR_FS_DIRTY;
        prop_assert_eq!(all, SR_FS);
    }

    /// INV-CAUSE-11: interrupt bit (63) is the only difference
    #[test]
    fn test_interrupt_bit(
        code in 1u64..16u64,
    ) {
        let exc = Cause::from_cause(code);
        let irq = Cause::from_cause(1u64 << 63 | code);
        // Same code but different interrupt/exception status
        // Only true for recognized interrupt codes
        if code == 1 || code == 5 || code == 9 {
            prop_assert!(irq.is_interrupt());
            prop_assert!(!exc.is_interrupt());
        }
    }

    /// INV-CAUSE-12: SR_UXL_32 != SR_UXL_64
    #[test]
    fn test_uxl_distinct(_v in 0u8..1u8) {
        prop_assert_ne!(SR_UXL_32, SR_UXL_64);
        // UXL_32 = 1 << 32, UXL_64 = 2 << 32
        prop_assert_eq!(SR_UXL_32, 1u64 << 32);
        prop_assert_eq!(SR_UXL_64, 2u64 << 32);
    }

    /// INV-CAUSE-13: all page faults are exceptions not interrupts
    #[test]
    fn test_page_faults_not_interrupts(_v in 0u8..1u8) {
        for code in [12u64, 13u64, 15u64] {
            let c = Cause::from_cause(code);
            prop_assert!(c.is_page_fault());
            prop_assert!(!c.is_interrupt());
        }
    }
}
