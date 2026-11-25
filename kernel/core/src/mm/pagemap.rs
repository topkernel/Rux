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

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

// Platform-specific AddressSpace re-export (canonical type: crate::mm)
pub use crate::mm::mm_struct::AddressSpace;

// Re-export common types
pub use crate::mm::page::{VirtAddr, PhysAddr, PAGE_SIZE};

// VMA related types
pub use crate::mm::vma::{Vma, VmaFlags, VmaManager, VmaType, VmaError};

// MapError, Perm and PageTableType are defined in mm/types.rs; the
// From<VmaError> conversion stays here. Re-exported so the
// `mm::pagemap::…` paths are unchanged.
pub use super::types::{MapError, PageTableType, Perm};

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

