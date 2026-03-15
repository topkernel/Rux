//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V PtRegs structure
//!
//! ## Layout
//!
//! ```text
//! Offset  Field       Description
//! ------  -----       -----------
//! 0x00    epc         Program counter (sepc CSR)
//! 0x08    ra          Return address (x1)
//! 0x10    sp          Stack pointer (x2)
//! 0x18    gp          Global pointer (x3)
//! 0x20    tp          Thread pointer (x4)
//! 0x28    t0          Temporary register (x5)
//! 0x30    t1          Temporary register (x6)
//! 0x38    t2          Temporary register (x7)
//! 0x40    s0/fp       Saved register/Frame pointer (x8)
//! 0x48    s1          Saved register (x9)
//! 0x50    a0          Argument/Return value (x10)
//! 0x58    a1          Argument (x11)
//! 0x60    a2          Argument (x12)
//! 0x68    a3          Argument (x13)
//! 0x70    a4          Argument (x14)
//! 0x78    a5          Argument (x15)
//! 0x80    a6          Argument (x16)
//! 0x88    a7          Argument/Syscall number (x17)
//! 0x90    s2          Saved register (x18)
//! 0x98    s3          Saved register (x19)
//! 0xa0    s4          Saved register (x20)
//! 0xa8    s5          Saved register (x21)
//! 0xb0    s6          Saved register (x22)
//! 0xb8    s7          Saved register (x23)
//! 0xc0    s8          Saved register (x24)
//! 0xc8    s9          Saved register (x25)
//! 0xd0    s10         Saved register (x26)
//! 0xd8    s11         Saved register (x27)
//! 0xe0    t3          Temporary register (x28)
//! 0xe8    t4          Temporary register (x29)
//! 0xf0    t5          Temporary register (x30)
//! 0xf8    t6          Temporary register (x31)
//! 0x100   status      sstatus CSR
//! 0x108   badaddr     stval CSR
//! 0x110   cause       scause CSR
//! 0x118   orig_a0     Original a0 (for syscall rollback)
//! ```
//!
//! Total size: 0x120 = 288 bytes

use core::arch::asm;

/// RISC-V register state structure
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct PtRegs {
    // Program counter
    pub epc: u64,      // 0x00 - sepc CSR

    // General purpose registers
    pub ra: u64,       // 0x08 - x1
    pub sp: u64,       // 0x10 - x2
    pub gp: u64,       // 0x18 - x3
    pub tp: u64,       // 0x20 - x4
    pub t0: u64,       // 0x28 - x5
    pub t1: u64,       // 0x30 - x6
    pub t2: u64,       // 0x38 - x7
    pub s0: u64,       // 0x40 - x8 (also fp)
    pub s1: u64,       // 0x48 - x9
    pub a0: u64,       // 0x50 - x10
    pub a1: u64,       // 0x58 - x11
    pub a2: u64,       // 0x60 - x12
    pub a3: u64,       // 0x68 - x13
    pub a4: u64,       // 0x70 - x14
    pub a5: u64,       // 0x78 - x15
    pub a6: u64,       // 0x80 - x16
    pub a7: u64,       // 0x88 - x17
    pub s2: u64,       // 0x90 - x18
    pub s3: u64,       // 0x98 - x19
    pub s4: u64,       // 0xa0 - x20
    pub s5: u64,       // 0xa8 - x21
    pub s6: u64,       // 0xb0 - x22
    pub s7: u64,       // 0xb8 - x23
    pub s8: u64,       // 0xc0 - x24
    pub s9: u64,       // 0xc8 - x25
    pub s10: u64,      // 0xd0 - x26
    pub s11: u64,      // 0xd8 - x27
    pub t3: u64,       // 0xe0 - x28
    pub t4: u64,       // 0xe8 - x29
    pub t5: u64,       // 0xf0 - x30
    pub t6: u64,       // 0xf8 - x31

    // CSR registers
    pub status: u64,   // 0x100 - sstatus CSR
    pub badaddr: u64,  // 0x108 - stval CSR
    pub cause: u64,    // 0x110 - scause CSR

    // Syscall support
    pub orig_a0: u64,  // 0x118 - Original a0, for syscall rollback
}

/// PtRegs structure size
pub const PT_REGS_SIZE: usize = 0x120; // 288 bytes

// Static assertion: ensure PtRegs size is correct
const _: () = assert!(core::mem::size_of::<PtRegs>() == PT_REGS_SIZE);

impl PtRegs {
    /// Create new empty PtRegs
    pub const fn new() -> Self {
        Self {
            epc: 0, ra: 0, sp: 0, gp: 0, tp: 0,
            t0: 0, t1: 0, t2: 0, s0: 0, s1: 0,
            a0: 0, a1: 0, a2: 0, a3: 0, a4: 0,
            a5: 0, a6: 0, a7: 0, s2: 0, s3: 0,
            s4: 0, s5: 0, s6: 0, s7: 0, s8: 0,
            s9: 0, s10: 0, s11: 0, t3: 0, t4: 0,
            t5: 0, t6: 0, status: 0, badaddr: 0,
            cause: 0, orig_a0: 0,
        }
    }

    /// Check if from user mode
    ///
    /// Determined by checking sstatus.SPP bit
    /// SPP=0 means from user mode, SPP=1 means from kernel mode
    #[inline]
    pub fn user_mode(&self) -> bool {
        (self.status & SR_SPP) == 0
    }

    /// Check if from kernel mode
    #[inline]
    pub fn kernel_mode(&self) -> bool {
        !self.user_mode()
    }

    /// Get syscall number
    #[inline]
    pub fn syscall_nr(&self) -> i64 {
        self.a7 as i64
    }

    /// Get syscall arguments
    ///
    /// Returns array of 6 arguments
    #[inline]
    pub fn syscall_args(&self) -> [u64; 6] {
        [
            self.orig_a0,  // Use orig_a0 as first argument
            self.a1,
            self.a2,
            self.a3,
            self.a4,
            self.a5,
        ]
    }

    /// Set syscall return value
    #[inline]
    pub fn set_return_value(&mut self, val: i64) {
        self.a0 = val as u64;
    }

    /// Set syscall error return
    ///
    /// If error is non-zero, return -error; otherwise return val
    #[inline]
    pub fn set_return_error(&mut self, error: i32, val: i64) {
        self.a0 = if error != 0 { -error as i64 as u64 } else { val as u64 };
    }

    /// Rollback syscall
    ///
    /// Restore a0 to original value
    #[inline]
    pub fn syscall_rollback(&mut self) {
        self.a0 = self.orig_a0;
    }

    /// Get instruction pointer (PC)
    #[inline]
    pub fn instruction_pointer(&self) -> u64 {
        self.epc
    }

    /// Set instruction pointer (PC)
    #[inline]
    pub fn set_instruction_pointer(&mut self, pc: u64) {
        self.epc = pc;
    }

    /// Get user stack pointer
    #[inline]
    pub fn user_stack_pointer(&self) -> u64 {
        self.sp
    }

    /// Set user stack pointer
    #[inline]
    pub fn set_user_stack_pointer(&mut self, sp: u64) {
        self.sp = sp;
    }

    /// Get frame pointer
    #[inline]
    pub fn frame_pointer(&self) -> u64 {
        self.s0  // s0 is also fp
    }

    /// Check if interrupts are disabled
    #[inline]
    pub fn irqs_disabled(&self) -> bool {
        (self.status & SR_PIE) == 0
    }
}

// ==================== CSR bit definitions ====================

/// SPP (Supervisor Previous Privilege) - bit 8
/// Indicates privilege level before entering trap
/// 0 = User mode, 1 = Supervisor mode
pub const SR_SPP: u64 = 1 << 8;

/// SPIE (Supervisor Previous Interrupt Enable) - bit 5
/// Indicates if interrupts were enabled before entering trap
pub const SR_PIE: u64 = 1 << 5;

/// SIE (Supervisor Interrupt Enable) - bit 1
/// Global interrupt enable
pub const SR_SIE: u64 = 1 << 1;

/// SUM (Supervisor User Memory Access) - bit 18
/// Allow S-mode to access user memory
pub const SR_SUM: u64 = 1 << 18;

/// UXL (User XLEN) - bits 33:32
/// User mode width: 1 = 32-bit, 2 = 64-bit
pub const SR_UXL_32: u64 = 1 << 32;
pub const SR_UXL_64: u64 = 2 << 32;

/// FS (Floating-point Status) - bits 14:13
pub const SR_FS_OFF: u64 = 0 << 13;
pub const SR_FS_INITIAL: u64 = 1 << 13;
pub const SR_FS_CLEAN: u64 = 2 << 13;
pub const SR_FS_DIRTY: u64 = 3 << 13;
/// FS field mask (bits 13-14)
pub const SR_FS: u64 = 3 << 13;

/// VS (Vector Status) - bits 10:9
pub const SR_VS_OFF: u64 = 0 << 9;
pub const SR_VS_INITIAL: u64 = 1 << 9;
pub const SR_VS_CLEAN: u64 = 2 << 9;
pub const SR_VS_DIRTY: u64 = 3 << 9;

// ==================== Exception cause codes ====================

/// Exception cause (scause)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    // Exceptions (scause MSB is 0)
    /// Instruction address misaligned
    InstructionAddressMisaligned = 0,
    /// Instruction access fault
    InstructionAccessFault = 1,
    /// Illegal instruction
    IllegalInstruction = 2,
    /// Breakpoint
    Breakpoint = 3,
    /// Load address misaligned
    LoadAddressMisaligned = 4,
    /// Load access fault
    LoadAccessFault = 5,
    /// Store/AMO address misaligned
    StoreAmoAddressMisaligned = 6,
    /// Store/AMO access fault
    StoreAmoAccessFault = 7,
    /// User mode ecall
    EcallUser = 8,
    /// Supervisor mode ecall
    EcallSupervisor = 9,
    /// Machine mode ecall
    EcallMachine = 11,
    /// Instruction page fault
    InstructionPageFault = 12,
    /// Load page fault
    LoadPageFault = 13,
    /// Store/AMO page fault
    StoreAmoPageFault = 15,

    // Interrupts (scause MSB is 1) - use codes 64+ to distinguish from exceptions
    /// Software interrupt (code 1 with interrupt bit)
    SupervisorSoft = 64,
    /// Timer interrupt (code 5 with interrupt bit)
    SupervisorTimer = 68,
    /// External interrupt (code 9 with interrupt bit)
    SupervisorExternal = 72,
}

impl Cause {
    /// Parse from scause value
    pub fn from_cause(cause: u64) -> Self {
        // Check if interrupt bit (MSB) is set
        if cause & (1u64 << 63) != 0 {
            // It's an interrupt - extract the code
            let code = cause & !(1u64 << 63);
            match code {
                1 => Cause::SupervisorSoft,
                5 => Cause::SupervisorTimer,
                9 => Cause::SupervisorExternal,
                _ => Cause::IllegalInstruction, // Unknown interrupt
            }
        } else {
            // It's an exception
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
                _ => Cause::IllegalInstruction, // Default
            }
        }
    }

    /// Is interrupt
    pub fn is_interrupt(&self) -> bool {
        matches!(self,
            Cause::SupervisorSoft |
            Cause::SupervisorTimer |
            Cause::SupervisorExternal)
    }

    /// Is exception
    pub fn is_exception(&self) -> bool {
        !self.is_interrupt()
    }

    /// Is page fault
    pub fn is_page_fault(&self) -> bool {
        matches!(self,
            Cause::InstructionPageFault |
            Cause::LoadPageFault |
            Cause::StoreAmoPageFault)
    }
}

// ==================== Helper functions ====================

/// Check if currently in interrupt context
///
/// Currently returns false, preemption count needs to be implemented later
#[inline]
pub fn in_interrupt() -> bool {
    // TODO: Implement preemption count check
    false
}

/// Check if currently in process context
#[inline]
pub fn in_task() -> bool {
    !in_interrupt()
}

// ==================== Offset constants (for assembly use) ====================

/// Field offsets in PtRegs
#[allow(dead_code)]
mod offsets {
    use super::*;

    pub const EPC: usize = core::mem::offset_of!(PtRegs, epc);
    pub const RA: usize = core::mem::offset_of!(PtRegs, ra);
    pub const SP: usize = core::mem::offset_of!(PtRegs, sp);
    pub const GP: usize = core::mem::offset_of!(PtRegs, gp);
    pub const TP: usize = core::mem::offset_of!(PtRegs, tp);
    pub const T0: usize = core::mem::offset_of!(PtRegs, t0);
    pub const T1: usize = core::mem::offset_of!(PtRegs, t1);
    pub const T2: usize = core::mem::offset_of!(PtRegs, t2);
    pub const S0: usize = core::mem::offset_of!(PtRegs, s0);
    pub const S1: usize = core::mem::offset_of!(PtRegs, s1);
    pub const A0: usize = core::mem::offset_of!(PtRegs, a0);
    pub const A1: usize = core::mem::offset_of!(PtRegs, a1);
    pub const A2: usize = core::mem::offset_of!(PtRegs, a2);
    pub const A3: usize = core::mem::offset_of!(PtRegs, a3);
    pub const A4: usize = core::mem::offset_of!(PtRegs, a4);
    pub const A5: usize = core::mem::offset_of!(PtRegs, a5);
    pub const A6: usize = core::mem::offset_of!(PtRegs, a6);
    pub const A7: usize = core::mem::offset_of!(PtRegs, a7);
    pub const S2: usize = core::mem::offset_of!(PtRegs, s2);
    pub const S3: usize = core::mem::offset_of!(PtRegs, s3);
    pub const S4: usize = core::mem::offset_of!(PtRegs, s4);
    pub const S5: usize = core::mem::offset_of!(PtRegs, s5);
    pub const S6: usize = core::mem::offset_of!(PtRegs, s6);
    pub const S7: usize = core::mem::offset_of!(PtRegs, s7);
    pub const S8: usize = core::mem::offset_of!(PtRegs, s8);
    pub const S9: usize = core::mem::offset_of!(PtRegs, s9);
    pub const S10: usize = core::mem::offset_of!(PtRegs, s10);
    pub const S11: usize = core::mem::offset_of!(PtRegs, s11);
    pub const T3: usize = core::mem::offset_of!(PtRegs, t3);
    pub const T4: usize = core::mem::offset_of!(PtRegs, t4);
    pub const T5: usize = core::mem::offset_of!(PtRegs, t5);
    pub const T6: usize = core::mem::offset_of!(PtRegs, t6);
    pub const STATUS: usize = core::mem::offset_of!(PtRegs, status);
    pub const BADADDR: usize = core::mem::offset_of!(PtRegs, badaddr);
    pub const CAUSE: usize = core::mem::offset_of!(PtRegs, cause);
    pub const ORIG_A0: usize = core::mem::offset_of!(PtRegs, orig_a0);
}

/// Export offset constants for assembly use
#[allow(dead_code)]
pub use offsets::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pt_regs_size() {
        assert_eq!(core::mem::size_of::<PtRegs>(), 288);
    }

    #[test]
    fn test_offsets() {
        assert_eq!(offsets::EPC, 0x00);
        assert_eq!(offsets::RA, 0x08);
        assert_eq!(offsets::SP, 0x10);
        assert_eq!(offsets::A0, 0x50);
        assert_eq!(offsets::STATUS, 0x100);
        assert_eq!(offsets::ORIG_A0, 0x118);
    }
}
