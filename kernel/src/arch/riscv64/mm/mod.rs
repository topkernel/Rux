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

// ASID management
pub mod asid;
pub use asid::{
    ASID_BITS, MAX_ASID, ASID_KERNEL, ASID_RESERVED, ASID_FIRST,
    alloc_asid, free_asid, asid_usage_count,
    flush_tlb_all, flush_tlb_asid, flush_tlb_page, flush_tlb_range, flush_tlb_kernel,
    build_satp, satp_to_asid, satp_to_ppn, read_satp, write_satp,
    AsidContext, print_asid_status,
};

// Page fault handling
pub mod fault;
pub use fault::{do_page_fault, MmFaultResult as FaultResult, fixup_exception};
