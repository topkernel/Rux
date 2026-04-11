//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V Sv39 Virtual Memory Management
//!
//! Module structure:
//! - memory_layout: Constants, address types, kernel mapping
//! - pagetable: PageTableEntry, PageTable, Satp
//! - mmu_init: MMU initialization, page mapping functions
//! - mm_ops: MmStruct extension methods, user space operations, COW
//! - page_fault: handle_mm_fault(), FaultFlags
//! - fault: do_page_fault(), exception table, signal handling
//! - fixmap: Early device mappings
//! - asid: Address space ID management

// Memory layout constants and address types
pub mod memory_layout;
pub use memory_layout::*;

// Page table structures (PTE, PageTable, Satp)
pub mod pagetable;
pub use pagetable::*;

// MMU initialization and page mapping
pub mod mmu_init;
pub use mmu_init::*;

// Memory management operations (MmStruct extensions, COW)
pub mod mm_ops;
pub use mm_ops::*;

// Page fault handling (handle_mm_fault)
pub mod page_fault;
pub use page_fault::*;

// Exception handling (do_page_fault, exception table, signals)
pub mod exception;
pub use exception::{do_page_fault, fixup_exception};

// Fixmap for early device mappings
pub mod fixmap;
pub use fixmap::*;

// ASID management
pub mod asid;
pub use asid::{
    ASID_BITS, MAX_ASID, ASID_KERNEL, ASID_RESERVED, ASID_FIRST,
    alloc_asid, free_asid, asid_usage_count,
    flush_tlb_all, flush_tlb_asid, flush_tlb_page, flush_tlb_range, flush_tlb_kernel,
    build_satp, satp_to_asid, satp_to_ppn, read_satp, write_satp,
    AsidContext, print_asid_status,
};
