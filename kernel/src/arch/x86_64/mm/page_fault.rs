//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 page-fault outcome types + handle_mm_fault entry.
//!
//! The enums are the arch-interface contract (generic code matches on
//! them); the handler body is X86-TODO(agent x86-mm): decode via
//! exception.rs, drive the generic VMA/anon/COW logic, install PTEs
//! through PageTableWalker.

use super::memory_layout::*;
use crate::mm::AddressSpace;

/// Fault access flags (u32 bitset, matching generic callers)
pub struct FaultFlags;
impl FaultFlags {
    pub const READ: u32 = 1 << 0;
    pub const WRITE: u32 = 1 << 1;
    pub const EXEC: u32 = 1 << 2;
    pub const USER: u32 = 1 << 3;
    pub const KERNEL: u32 = 1 << 4;
}

/// Outcome of a handled fault
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmFaultResult {
    /// Fault fully handled — retry the instruction
    Handled,
    /// Anonymous page materialized — retry
    Fixed,
    /// COW resolved — retry
    CowPending,
    /// Mapping already exists — retry
    AlreadyMapped,
    Segfault,
    PermissionDenied,
    OutOfMemory,
    BusError,
    KernelPanic,
}

/// Fault a user address in. X86-TODO(agent x86-mm)
pub fn handle_mm_fault(
    addr_space: &AddressSpace,
    fault_addr: VirtAddr,
    flags: u32,
) -> MmFaultResult {
    let _ = (addr_space, fault_addr, flags);
    MmFaultResult::Segfault
}

/// Physical address behind a user virtual address, if mapped.
pub fn get_user_phys(root_ppn: u64, vaddr: u64) -> Option<u64> {
    // SAFETY: caller guarantees root_ppn is a live user root.
    unsafe { PageTableWalker::walk(root_ppn, vaddr).map(|(ppn, _)| ppn << 12) }
}

use super::mm_ops::PageTableWalker;
