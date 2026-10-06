//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 bring-up stubs for the MmStruct arch-method family.
//!
//! The riscv64 backend implements `mmap`/`munmap`/`fork`/... as methods on
//! the generic `MmStruct` (arch/riscv64/mm/mm_ops.rs). The pinned x86_64
//! skeleton does not carry that impl block yet (X86-TODO agent x86-mm);
//! these stubs hold the compile together until the real port lands.
//!
//! DELETE THIS MODULE when arch/x86_64/mm/mm_ops.rs grows the real
//! `impl crate::mm::MmStruct` methods — duplicate definitions are a
//! compile error by design, so the removal point is unambiguous.

use super::mm_struct::MmStruct;
use super::pagemap::{MapError, Perm};
use super::page::VirtAddr as PageVirtAddr;
use super::vma::{VmaFlags, VmaType};

impl MmStruct {
    /// mmap system call implementation
    pub fn mmap(
        &self,
        _addr: PageVirtAddr,
        _size: usize,
        _flags: VmaFlags,
        _vma_type: VmaType,
        _perm: Perm,
        _map_flags: u32,
    ) -> Result<PageVirtAddr, MapError> {
        Err(MapError::OutOfMemory) // x86-mm bring-up stub
    }

    pub fn munmap(&self, _addr: PageVirtAddr, _size: usize) -> Result<(), MapError> {
        Ok(()) // x86-mm bring-up stub
    }

    pub fn zap_page_range(&self, _start: PageVirtAddr, _size: usize) -> Result<(), MapError> {
        Ok(()) // x86-mm bring-up stub
    }

    pub fn find_free_area(&self, _size: usize) -> Result<PageVirtAddr, MapError> {
        Err(MapError::OutOfMemory) // x86-mm bring-up stub
    }

    /// Copy address space for fork
    pub fn fork(&self) -> Result<MmStruct, MapError> {
        Err(MapError::OutOfMemory) // x86-mm bring-up stub
    }
}
