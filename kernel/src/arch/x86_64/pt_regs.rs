//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 PtRegs — Linux-compatible register frame.
//!
//! ## Layout (matches struct pt_regs in Linux asm/ptrace.h)
//!
//! ```text
//! Offset  Field       Description
//! ------  -----       -----------
//! 0x00    r15         Callee-saved
//! 0x08    r14         Callee-saved
//! 0x10    r13         Callee-saved
//! 0x18    r12         Callee-saved
//! 0x20    rbp         Frame pointer
//! 0x28    rbx         Callee-saved
//! 0x30    r11         Scratch / rflags after syscall
//! 0x38    r10         Syscall arg 4
//! 0x40    r9          Syscall arg 6
//! 0x48    r8          Syscall arg 5
//! 0x50    rax         Syscall number / return value
//! 0x58    rcx         Syscall arg 4 (C ABI) / return address after syscall
//! 0x60    rdx         Syscall arg 3
//! 0x68    rsi         Syscall arg 2
//! 0x70    rdi         Syscall arg 1
//! 0x78    orig_rax    Original rax (syscall rollback) / error code
//! 0x80    rip         Instruction pointer
//! 0x88    cs          Code segment
//! 0x90    rflags      Flags
//! 0x98    rsp         User stack pointer
//! 0xa0    ss          Stack segment
//! ```
//!
//! Total size: 0xa8 = 168 bytes.
//!
//! The trap entry stubs in trap.S push registers in exactly this order
//! on top of the hardware frame, so the struct overlays the stack frame
//! one-to-one. For entries from user mode the stub runs on the TSS
//! rsp0 stack, which is the task's kernel stack top — so the frame
//! lands precisely at `stack_top - sizeof(PtRegs)` (same contract as
//! riscv64, relied on by `current_task_pt_regs` and fork).

/// x86_64 register state structure
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PtRegs {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub orig_rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// PtRegs structure size
pub const PT_REGS_SIZE: usize = 0xa8;

// Static assertion: ensure PtRegs size is correct
const _: () = assert!(core::mem::size_of::<PtRegs>() == PT_REGS_SIZE);

impl PtRegs {
    /// Create new empty PtRegs
    pub const fn new() -> Self {
        Self {
            r15: 0, r14: 0, r13: 0, r12: 0, rbp: 0, rbx: 0,
            r11: 0, r10: 0, r9: 0, r8: 0, rax: 0, rcx: 0,
            rdx: 0, rsi: 0, rdi: 0, orig_rax: 0, rip: 0, cs: 0,
            rflags: 0, rsp: 0, ss: 0,
        }
    }

    /// Check if from user mode (CS RPL == 3)
    #[inline]
    pub fn user_mode(&self) -> bool {
        (self.cs & 3) == 3
    }

    /// Check if from kernel mode
    #[inline]
    pub fn kernel_mode(&self) -> bool {
        !self.user_mode()
    }

    /// Mark this frame as returning to user mode (CPL 3 segments).
    #[inline]
    pub fn mark_user_frame(&mut self) {
        self.cs = 0x33; // __USER_CS
        self.ss = 0x2b; // __USER_DS
    }

    /// Mark this frame as a kernel-mode frame (CPL 0 segments).
    #[inline]
    pub fn mark_kernel_frame(&mut self) {
        self.cs = 0x08; // __KERNEL_CS
        self.ss = 0x10; // __KERNEL_DS
    }

    /// Get syscall number
    #[inline]
    pub fn syscall_nr(&self) -> i64 {
        self.rax as i64
    }

    /// Get syscall arguments (x86_64 kernel ABI: rdi rsi rdx r10 r8 r9)
    #[inline]
    pub fn syscall_args(&self) -> [u64; 6] {
        [self.rdi, self.rsi, self.rdx, self.r10, self.r8, self.r9]
    }

    /// Set syscall return value
    #[inline]
    pub fn set_return_value(&mut self, val: i64) {
        self.rax = val as u64;
    }

    /// Set syscall error return
    #[inline]
    pub fn set_return_error(&mut self, error: i32, val: i64) {
        self.rax = if error != 0 { -error as i64 as u64 } else { val as u64 };
    }

    /// Rollback syscall (restore rax to its pre-syscall value)
    #[inline]
    pub fn syscall_rollback(&mut self) {
        self.rax = self.orig_rax;
    }

    /// Get instruction pointer (PC)
    #[inline]
    pub fn instruction_pointer(&self) -> u64 {
        self.rip
    }

    /// Set instruction pointer (PC)
    #[inline]
    pub fn set_instruction_pointer(&mut self, pc: u64) {
        self.rip = pc;
    }

    /// Get user stack pointer
    #[inline]
    pub fn user_stack_pointer(&self) -> u64 {
        self.rsp
    }

    /// Set user stack pointer
    #[inline]
    pub fn set_user_stack_pointer(&mut self, sp: u64) {
        self.rsp = sp;
    }

    /// Get frame pointer
    #[inline]
    pub fn frame_pointer(&self) -> u64 {
        self.rbp
    }

    /// Check if interrupts were disabled in the interrupted context
    #[inline]
    pub fn irqs_disabled(&self) -> bool {
        (self.rflags & (1 << 9)) == 0
    }
}

// ==================== Exception / interrupt causes ====================

/// Exception cause (x86 vector numbers; interrupt vectors >= 32 use the
/// same synthetic codes as the riscv64 side so generic code matches on
/// one enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    DivideError = 0,
    Debug = 1,
    Nmi = 2,
    Breakpoint = 3,
    Overflow = 4,
    BoundRangeExceeded = 5,
    IllegalInstruction = 6,
    DeviceNotAvailable = 7,
    DoubleFault = 8,
    InvalidTss = 10,
    SegmentNotPresent = 11,
    StackSegmentFault = 12,
    GeneralProtectionFault = 13,
    PageFault = 14,
    SpuriousInterrupt = 15,
    X87FloatingPointException = 16,
    AlignmentCheck = 17,
    ControlProtectionException = 21,
    UnknownException = 31,

    // Interrupts (synthetic, >= 64 — mirrors riscv64 encoding)
    SupervisorSoft = 64,
    SupervisorTimer = 68,
    SupervisorExternal = 72,
}

impl Cause {
    /// Parse from an x86 vector number (0..255)
    pub fn from_cause(vector: u64) -> Self {
        match vector {
            0 => Cause::DivideError,
            1 => Cause::Debug,
            2 => Cause::Nmi,
            3 => Cause::Breakpoint,
            4 => Cause::Overflow,
            5 => Cause::BoundRangeExceeded,
            6 => Cause::IllegalInstruction,
            7 => Cause::DeviceNotAvailable,
            8 => Cause::DoubleFault,
            10 => Cause::InvalidTss,
            11 => Cause::SegmentNotPresent,
            12 => Cause::StackSegmentFault,
            13 => Cause::GeneralProtectionFault,
            14 => Cause::PageFault,
            16 => Cause::X87FloatingPointException,
            17 => Cause::AlignmentCheck,
            21 => Cause::ControlProtectionException,
            // IRQ vectors (32..): map timer/softirq/external classes
            64 => Cause::SupervisorSoft,
            68 => Cause::SupervisorTimer,
            72 => Cause::SupervisorExternal,
            _ => {
                if vector >= 32 && vector < 256 {
                    // All external IRQs classify as external interrupts
                    Cause::SupervisorExternal
                } else {
                    Cause::UnknownException
                }
            }
        }
    }

    /// Is interrupt
    pub fn is_interrupt(&self) -> bool {
        matches!(self, Cause::SupervisorSoft | Cause::SupervisorTimer | Cause::SupervisorExternal)
    }

    /// Is exception
    pub fn is_exception(&self) -> bool {
        !self.is_interrupt()
    }

    /// Is page fault (x86 distinguishes read/write/exec via the error
    /// code, not the vector; generic callers pass FaultFlags directly)
    pub fn is_page_fault(&self) -> bool {
        matches!(self, Cause::PageFault)
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

    pub const R15: usize = core::mem::offset_of!(PtRegs, r15);
    pub const R14: usize = core::mem::offset_of!(PtRegs, r14);
    pub const R13: usize = core::mem::offset_of!(PtRegs, r13);
    pub const R12: usize = core::mem::offset_of!(PtRegs, r12);
    pub const RBP: usize = core::mem::offset_of!(PtRegs, rbp);
    pub const RBX: usize = core::mem::offset_of!(PtRegs, rbx);
    pub const R11: usize = core::mem::offset_of!(PtRegs, r11);
    pub const R10: usize = core::mem::offset_of!(PtRegs, r10);
    pub const R9: usize = core::mem::offset_of!(PtRegs, r9);
    pub const R8: usize = core::mem::offset_of!(PtRegs, r8);
    pub const RAX: usize = core::mem::offset_of!(PtRegs, rax);
    pub const RCX: usize = core::mem::offset_of!(PtRegs, rcx);
    pub const RDX: usize = core::mem::offset_of!(PtRegs, rdx);
    pub const RSI: usize = core::mem::offset_of!(PtRegs, rsi);
    pub const RDI: usize = core::mem::offset_of!(PtRegs, rdi);
    pub const ORIG_RAX: usize = core::mem::offset_of!(PtRegs, orig_rax);
    pub const RIP: usize = core::mem::offset_of!(PtRegs, rip);
    pub const CS: usize = core::mem::offset_of!(PtRegs, cs);
    pub const RFLAGS: usize = core::mem::offset_of!(PtRegs, rflags);
    pub const RSP: usize = core::mem::offset_of!(PtRegs, rsp);
    pub const SS: usize = core::mem::offset_of!(PtRegs, ss);
}

/// Export offset constants for assembly use
#[allow(dead_code)]
pub use offsets::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pt_regs_size() {
        assert_eq!(core::mem::size_of::<PtRegs>(), 168);
    }

    #[test]
    fn test_offsets() {
        assert_eq!(offsets::R15, 0x00);
        assert_eq!(offsets::RDI, 0x70);
        assert_eq!(offsets::ORIG_RAX, 0x78);
        assert_eq!(offsets::RIP, 0x80);
        assert_eq!(offsets::SS, 0xa0);
    }
}
