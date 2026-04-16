//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Platform-agnostic Address Space Interface
//!
//! This module re-exports platform-specific AddressSpace implementations:
//! - RISC-V: arch/riscv64/mm.rs
//!
//! High-level VMA operations (brk, mmap, munmap) are provided in platform implementations

// Platform-specific AddressSpace re-export
pub use crate::arch::riscv64::mm::AddressSpace;

// Re-export common types
pub use crate::mm::page::{VirtAddr, PhysAddr, PAGE_SIZE};

// VMA related types
pub use crate::mm::vma::{Vma, VmaFlags, VmaManager, VmaType, VmaError};

// Map error types (public interface)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// Already mapped
    AlreadyMapped,
    /// Not mapped
    NotMapped,
    /// Out of memory
    OutOfMemory,
    /// Invalid parameter
    Invalid,
}

// Implement From<VmaError> to support ? operator
impl From<VmaError> for MapError {
    fn from(err: VmaError) -> Self {
        match err {
            VmaError::Overlap => MapError::AlreadyMapped,
            VmaError::NoSpace => MapError::OutOfMemory,
            VmaError::NotFound => MapError::NotMapped,
            VmaError::Invalid => MapError::Invalid,
        }
    }
}

// Page permissions (public interface)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Perm {
    /// No access
    None = 0,
    /// Read only
    Read = 1,
    /// Read and write
    ReadWrite = 2,
    /// Read, write and execute
    ReadWriteExec = 3,
    /// Read and execute (R=1, W=0, X=1)
    ReadExec = 4,
    /// Execute only (R=0, W=0, X=1) — valid on Sv39
    Exec = 5,
}

// Page table types (public interface)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageTableType {
    /// Kernel page table
    Kernel = 0,
    /// User page table
    User = 1,
}
