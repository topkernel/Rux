//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! aarch64 pt_regs — EL0/EL1 exception frame layout.
//!
//! Mirrors Linux's `struct pt_regs` for arm64 (the layout glibc/musl and
//! ptrace users expect on this ABI): the 31 general-purpose x0..x30
//! registers, sp, pc, pstate, then the exception metadata.
//!
//! ## Layout
//!
//! ```text
//! Offset  Field       Description
//! ------  -----       -----------
//! 0x000   regs[31]    x0..x30 (x0 at index 0, lr/x30 at index 30)
//! 0x0f8   sp          Stack pointer at exception entry
//! 0x100   pc          Exception return address (ELR_ELx)
//! 0x108   pstate      Saved PSTATE (SPSR_ELx)
//! 0x110   esr         Exception syndrome (ESR_ELx)
//! 0x118   far         Fault address (FAR_ELx)
//! 0x120   orig_x0     Original x0 (for syscall rollback)
//! 0x128   syscallno   Syscall number (SVC — x8 at entry)
//! 0x130   frame_pad   Padding to a 16-byte-multiple frame size
//! ```
//!
//! Total size: 0x140 = 320 bytes.
//!
//! ## Why 320, not the 312 the fields need
//!
//! The entry path does `sub sp, sp, #S_FRAME_SIZE` on top of a
//! 16-aligned kernel stack (stack top or a mid-stack EL1 sp), and the
//! Rust handler runs with sp = pt_regs base. The AAPCS64 requires SP
//! 16-aligned at call boundaries (Rust prologues may spill q-registers
//! relative to sp), so S_FRAME_SIZE must be a multiple of 16. 312 is
//! 8 mod 16; the frame carries 8 spare bytes — kernel-private trailing
//! pad, exactly like Linux arm64's trailing stackframe words. The
//! ptrace-visible prefix (regs/sp/pc/pstate/orig_x0/syscallno) is
//! unchanged by the pad.

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

/// AArch64 processor state (PSTATE) — the SPSR_ELx snapshot.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Pstate(pub u64);

impl Pstate {
    /// EL0t (user, SP_EL0) — M[3:0] = 0b0000.
    pub const EL0T: u64 = 0b0000;
    /// EL1h (kernel, SP_EL1) — M[3:0] = 0b0101.
    pub const EL1H: u64 = 0b0101;
    /// EL1t (kernel, SP_EL0) — M[3:0] = 0b0100.
    pub const EL1T: u64 = 0b0100;
    /// Mode field mask.
    const M_MASK: u64 = 0b1111;

    #[inline]
    pub fn mode(&self) -> u64 {
        self.0 & Self::M_MASK
    }

    #[inline]
    pub fn is_el0(&self) -> bool {
        self.mode() == Self::EL0T
    }
}

// ==================== PSTATE bit definitions ====================

/// SPSR M[3:0] — exception level/mode at exception entry
pub const SPSR_M_MASK: u64 = 0b1111;
/// M = EL0t (0b0000) — user mode
pub const SPSR_M_EL0T: u64 = 0b0000;
/// M = EL1h (0b0101) — kernel using SP_EL1
pub const SPSR_M_EL1H: u64 = 0b0101;

/// SPSR bit 9 — Debug exception mask
pub const SPSR_D: u64 = 1 << 9;
/// SPSR bit 8 — SError mask
pub const SPSR_A: u64 = 1 << 8;
/// SPSR bit 7 — IRQ mask
pub const SPSR_I: u64 = 1 << 7;
/// SPSR bit 6 — FIQ mask
pub const SPSR_F: u64 = 1 << 6;
/// DAIF mask bits combined
pub const SPSR_DAIF: u64 = SPSR_D | SPSR_A | SPSR_I | SPSR_F;

/// PSTATE bits riscv64-status-equivalents (interface naming parity):
/// SR_SPP ≈ "was supervisor" (M != EL0t), SR_PIE ≈ "interrupts were on"
/// (I bit clear), SR_SIE ≈ current IRQ enable.
pub const SR_SPP: u64 = 0; // not a single bit on arm64 — use helpers below
pub const SR_PIE: u64 = 0; // see irqs_disabled(); kept for parity only
pub const SR_SIE: u64 = 0;

/// AArch64 register frame (pt_regs)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PtRegs {
    /// x0..x30 (31 registers)
    pub regs: [u64; 31],
    /// stack pointer at exception entry
    pub sp: u64,
    /// exception return address (ELR_ELx)
    pub pc: u64,
    /// saved PSTATE (SPSR_ELx)
    pub pstate: u64,
    /// exception syndrome (ESR_ELx)
    pub esr: u64,
    /// fault address (FAR_ELx)
    pub far: u64,
    /// original x0 (for syscall rollback — Linux arm64 `orig_x0`)
    pub orig_x0: u64,
    /// syscall number (x8 at SVC entry — Linux arm64 `syscallno`)
    pub syscallno: u64,
    /// trailing pad: brings the frame to a 16-byte multiple so the
    /// entry `sub sp, sp, #S_FRAME_SIZE` keeps SP AAPCS64-aligned
    /// (kernel-private — not part of the ptrace-visible prefix)
    pub frame_pad: [u64; 2],
}

impl Default for PtRegs {
    fn default() -> Self {
        Self::new()
    }
}

/// PtRegs structure size (16-byte multiple — see the module doc)
pub const PT_REGS_SIZE: usize = 0x140; // 320 bytes

// Static assertion: ensure PtRegs size is correct
const _: () = assert!(core::mem::size_of::<PtRegs>() == PT_REGS_SIZE);

impl PtRegs {
    /// Create new empty PtRegs
    pub const fn new() -> Self {
        Self {
            regs: [0; 31],
            sp: 0,
            pc: 0,
            pstate: 0,
            esr: 0,
            far: 0,
            orig_x0: 0,
            syscallno: 0,
            frame_pad: [0; 2],
        }
    }

    /// Check if from user mode (PSTATE.M == EL0t)
    #[inline]
    pub fn user_mode(&self) -> bool {
        (self.pstate & SPSR_M_MASK) == SPSR_M_EL0T
    }

    /// Check if from kernel mode
    #[inline]
    pub fn kernel_mode(&self) -> bool {
        !self.user_mode()
    }

    /// Get syscall number (x8 at SVC entry, cached in syscallno)
    #[inline]
    pub fn syscall_nr(&self) -> i64 {
        self.syscallno as i64
    }

    /// Get syscall arguments (arm64 ABI: x0..x5)
    #[inline]
    pub fn syscall_args(&self) -> [u64; 6] {
        [
            self.orig_x0, // rollback-safe first argument (riscv64 parity)
            self.regs[1],
            self.regs[2],
            self.regs[3],
            self.regs[4],
            self.regs[5],
        ]
    }

    /// Set syscall return value (x0)
    #[inline]
    pub fn set_return_value(&mut self, val: i64) {
        self.regs[0] = val as u64;
    }

    /// Set syscall error return
    #[inline]
    pub fn set_return_error(&mut self, error: i32, val: i64) {
        self.regs[0] = if error != 0 { -error as i64 as u64 } else { val as u64 };
    }

    /// Rollback syscall: restore x0 to its original value
    #[inline]
    pub fn syscall_rollback(&mut self) {
        self.regs[0] = self.orig_x0;
    }

    /// Get instruction pointer (PC)
    #[inline]
    pub fn instruction_pointer(&self) -> u64 {
        self.pc
    }

    /// Set instruction pointer (PC)
    #[inline]
    pub fn set_instruction_pointer(&mut self, pc: u64) {
        self.pc = pc;
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

    /// Get frame pointer (x29 — the aarch64 fp)
    #[inline]
    pub fn frame_pointer(&self) -> u64 {
        self.regs[29]
    }

    /// Check if interrupts were disabled at exception entry
    /// (SPSR.I set = IRQ masked = "irqs disabled")
    #[inline]
    pub fn irqs_disabled(&self) -> bool {
        (self.pstate & SPSR_I) != 0
    }

    /// Mark this frame as returning to user mode (M = EL0t). Interface
    /// parity with the x86_64/riscv64 backends.
    #[inline]
    pub fn mark_user_frame(&mut self) {
        self.pstate = (self.pstate & !SPSR_M_MASK) | SPSR_M_EL0T;
    }

    /// Mark this frame as an EL1h (kernel) frame so eret stays in EL1.
    #[inline]
    pub fn mark_kernel_frame(&mut self) {
        self.pstate = (self.pstate & !SPSR_M_MASK) | SPSR_M_EL1H;
    }
}

// ==================== Exception syndrome (ESR) ====================

/// ESR_ELx EC (exception class) field
pub const ESR_EC_SHIFT: u64 = 26;
pub const ESR_EC_MASK: u64 = 0x3F << ESR_EC_SHIFT;
/// ESR ISS field (lower 25 bits)
pub const ESR_ISS_MASK: u64 = (1 << 25) - 1;
/// ESR IL (instruction length) bit
pub const ESR_IL: u64 = 1 << 25;

/// Exception cause — decoded from ESR_ELx (riscv64 Cause interface parity)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    // Exceptions
    /// Unknown exception class
    Unknown = 0,
    /// trapped WFx (wfe/wfi trap)
    TrappedWfx = 1,
    /// SVC instruction from AArch64 (EL0)
    EcallUser = 2,
    /// SVC/HVC from higher EL
    EcallSupervisor = 3,
    /// Instruction abort (lower EL)
    InstructionAbort = 4,
    /// Instruction abort (current EL)
    InstructionAbortCur = 5,
    /// Data abort (lower EL) — page faults, alignment, permission
    DataAbort = 6,
    /// Data abort (current EL)
    DataAbortCur = 7,
    /// Illegal execution state
    IllegalState = 9,
    /// Illegal instruction
    IllegalInstruction = 14,
    /// SError interrupt (asynchronous)
    SError = 17,

    // Interrupts — offset by 64 like the riscv64 encoding
    /// IRQ interrupt
    Irq = 64,
    /// FIQ interrupt
    Fiq = 65,
}

impl Cause {
    /// Parse from ESR_ELx value
    pub fn from_cause(esr: u64) -> Self {
        // Asynchronous vectors (trap.S) store pseudo-cause constants in
        // the frame's esr slot — ESR_EL1 is UNDEFINED on the IRQ/FIQ
        // vectors, and the SError vectors use the same small-integer
        // encoding. Map those FIRST: the ec decode below would read
        // bit 26+ and land on Unknown (64>>26 == 0). This is the same
        // encoding the riscv64 backend uses for its scause interrupts.
        match esr {
            64 => return Cause::Irq,
            65 => return Cause::Fiq,
            17 => return Cause::SError,
            _ => {}
        }
        let ec = (esr & ESR_EC_MASK) >> ESR_EC_SHIFT;
        match ec {
            0x00 => Cause::Unknown,
            0x01 => Cause::TrappedWfx,
            0x15 => Cause::EcallUser,      // SVC (AArch64)
            0x11 => Cause::EcallUser,      // SVC (AArch32)
            0x12 => Cause::EcallSupervisor, // HVC (PSCI conduit)
            0x20 => Cause::InstructionAbort,
            0x21 => Cause::InstructionAbortCur,
            0x24 => Cause::DataAbort,
            0x25 => Cause::DataAbortCur,
            0x09 => Cause::IllegalState,
            0x0e => Cause::IllegalInstruction,
            0x2f => Cause::SError,
            _ => Cause::Unknown,
        }
    }

    /// Is interrupt (IRQ/FIQ/SError)
    pub fn is_interrupt(&self) -> bool {
        matches!(self, Cause::Irq | Cause::Fiq | Cause::SError)
    }

    /// Is exception
    pub fn is_exception(&self) -> bool {
        !self.is_interrupt()
    }

    /// Is page fault (instruction/data abort that translates into a
    /// translation/permission/access-flag fault — the DFSC/IFSC check)
    pub fn is_page_fault(&self) -> bool {
        match self {
            Cause::InstructionAbort | Cause::InstructionAbortCur => true,
            Cause::DataAbort | Cause::DataAbortCur => true,
            _ => false,
        }
    }
}

// ==================== Helper functions ====================

/// Check if currently in interrupt context
#[inline]
pub fn in_interrupt() -> bool {
    crate::interrupt::preempt::in_interrupt()
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

    pub const X0: usize = core::mem::offset_of!(PtRegs, regs);
    pub const SP: usize = core::mem::offset_of!(PtRegs, sp);
    pub const PC: usize = core::mem::offset_of!(PtRegs, pc);
    pub const PSTATE: usize = core::mem::offset_of!(PtRegs, pstate);
    pub const ESR: usize = core::mem::offset_of!(PtRegs, esr);
    pub const FAR: usize = core::mem::offset_of!(PtRegs, far);
    pub const ORIG_X0: usize = core::mem::offset_of!(PtRegs, orig_x0);
    pub const SYSCALLNO: usize = core::mem::offset_of!(PtRegs, syscallno);
}

/// Export offset constants for assembly use
#[allow(dead_code)]
pub use offsets::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pt_regs_size() {
        assert_eq!(core::mem::size_of::<PtRegs>(), 320);
        // The entry asm relies on a 16-aligned frame size.
        assert_eq!(PT_REGS_SIZE % 16, 0);
    }

    #[test]
    fn test_offsets() {
        assert_eq!(offsets::X0, 0x00);
        assert_eq!(offsets::SP, 0x0F8);
        assert_eq!(offsets::PC, 0x100);
        assert_eq!(offsets::PSTATE, 0x108);
        assert_eq!(offsets::ORIG_X0, 0x120);
        assert_eq!(offsets::SYSCALLNO, 0x128);
    }
}
