//! aarch64 pt_regs — EL0/EL1 exception frame layout.
//!
//! Mirrors Linux's `struct pt_regs` for arm64 (the layout glibc/musl and
//! ptrace users expect on this ABI): the 31 general-purpose x0..x30
//! registers, sp, pc, pstate, then the exception metadata.

use core::sync::atomic::{AtomicU64, Ordering};

/// AArch64 processor state (PSTATE) — the SPSR_ELx snapshot.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Pstate(pub u64);

impl Pstate {
    /// EL0 (user) — M[3:0] = 0b0000.
    pub const EL0: u64 = 0b0000;
    /// EL1h (kernel, SPsel) — M[3:0] = 0b0101.
    pub const EL1H: u64 = 0b0101;
    /// Mode field mask.
    const M_MASK: u64 = 0b1111;

    pub fn mode(&self) -> u64 {
        self.0 & Self::M_MASK
    }
}

/// AArch64 register frame (pt_regs).
#[repr(C)]
#[derive(Debug)]
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
}

// Register-index constants used across the kernel (same names as the
// riscv64/x86_64 backends expose for syscall argument access).
pub const PT_A0: usize = 0;
pub const PT_A1: usize = 1;
pub const PT_A2: usize = 2;
pub const PT_A3: usize = 3;
pub const PT_A4: usize = 4;
pub const PT_A5: usize = 5;
pub const PT_RA: usize = 30; // x30 = LR
pub const PT_SP: usize = 31; // index into the flat frame view
pub const PT_PC: usize = 32;
pub const PT_PSTATE: usize = 33;

impl PtRegs {
    pub const fn new() -> Self {
        Self {
            regs: [0; 31],
            sp: 0,
            pc: 0,
            pstate: Pstate::EL0,
            esr: 0,
            far: 0,
        }
    }

    /// True if the frame was taken from EL0 (user mode).
    pub fn user_mode(&self) -> bool {
        Pstate(self.pstate).mode() == Pstate::EL0
    }

    pub fn kernel_mode(&self) -> bool {
        !self.user_mode()
    }

    /// Syscall argument i (x0..x7).
    pub fn arg(&self, i: usize) -> u64 {
        self.regs[i]
    }

    /// Set syscall return value (x0).
    pub fn set_ret(&mut self, v: u64) {
        self.regs[0] = v;
    }

    /// Syscall number (x8 on the arm64 ABI).
    pub fn syscall_nr(&self) -> u64 {
        self.regs[8]
    }
}

impl Default for PtRegs {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Interface parity items the generic code expects from a backend's pt_regs
// module (see arch/riscv64/pt_regs.rs and arch/x86_64/pt_regs.rs).
// ---------------------------------------------------------------------------

/// Saved-current-frame pointer for the running CPU.
pub static CURRENT_PT_REGS: AtomicU64 = AtomicU64::new(0);

/// Exception table entry (aarch64 uses the same 2×u64 shape).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ExceptionTableEntry {
    pub insn: u64,
    pub fixup: u64,
}

/// pt_regs size (layout-compat assert target).
pub const PT_REGS_SIZE: usize = core::mem::size_of::<PtRegs>();

/// Registers of the current frame (unsaved variants used by dump paths).
pub const PT_REGS_COUNT: usize = 34;

// Bring-up TODO list for this module (each is required before the backend
// can enter userspace):
//   - vector entry assembly (vector.S) saving/restoring this frame
//   - syscall dispatch (SVC #0, ESR EC=0x15) reading x8/x0..x5
//   - signal frame build/restore on this layout
