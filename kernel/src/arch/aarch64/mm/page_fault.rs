//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 page fault handling — surface + stage-2 stubs.
//!
//! handle_mm_fault and the COW/demand machinery are the stage-2 port
//! (the 4-level PageTableWalker, VMA-driven demand fill, COW protocol —
//! the riscv64 twin's invariants INV-COW-1..5 apply verbatim). The
//! signatures match so the generic tree typechecks today; the bodies are
//! loud `todo!`s so any stage-1-era hit is diagnosable.

use super::memory_layout::VirtAddr;
use crate::mm::AddressSpace;

// ==================== Fault Flags ====================

/// Fault access type flags (interface parity with the other backends)
pub struct FaultFlags;

impl FaultFlags {
    /// Read fault
    pub const READ: u32 = 0x01;
    /// Write fault
    pub const WRITE: u32 = 0x02;
    /// Execute fault (instruction fetch)
    pub const EXEC: u32 = 0x04;
    /// User mode access
    pub const USER: u32 = 0x08;
    /// Kernel mode access
    pub const KERNEL: u32 = 0x10;
}

// ==================== Fault Result ====================

/// Page fault handling result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmFaultResult {
    /// Handled successfully, can retry instruction
    Handled,
    /// Address not in any VMA (segmentation fault)
    Segfault,
    /// Permission denied (protection fault)
    PermissionDenied,
    /// Out of memory
    OutOfMemory,
    /// Bus error — file-backed fault past EOF (SIGBUS to the task)
    BusError,
    /// Already mapped (no handling needed)
    AlreadyMapped,
    /// COW pending (handled by handle_cow_fault)
    CowPending,
    /// Kernel exception fixed (via exception table)
    Fixed,
    /// Unfixable kernel exception
    KernelPanic,
}

// ==================== Fault Handler ====================

/// Main page fault handler (VMA-driven demand fill / COW / stack growth)
///
/// STAGE 2: port of the riscv64 handle_mm_fault onto the aarch64
/// 4-level walker (the walker itself lands in mm_ops).
pub fn handle_mm_fault(
    _addr_space: &AddressSpace,
    _fault_addr: VirtAddr,
    _flags: u32,
) -> MmFaultResult {
    todo!("aarch64 stage-2: handle_mm_fault (VMA demand fill + COW)")
}

/// Walk the user page tables and return the physical page mapped at
/// vaddr (diagnostics; riscv64 twin)
pub fn get_user_phys(_root_ppn: u64, _vaddr: u64) -> Option<u64> {
    todo!("aarch64 stage-2: get_user_phys (4-level user-table walk)")
}
