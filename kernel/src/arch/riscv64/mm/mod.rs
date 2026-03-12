//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V Sv39 virtual memory management
//!
//! RISC-V Sv39 paging specification:
//! - 3-level page table (512 PTE/level)
//! - 39-bit virtual address (512GB)
//! - 4KB page size
//! - Page table entry: 10-bit PPN + 10-bit flags

// Basic memory management (content from original mm.rs)
mod base;
pub use base::*;

// Page fault handling
pub mod fault;
pub use fault::{do_page_fault, MmFaultResult as FaultResult, fixup_exception};
