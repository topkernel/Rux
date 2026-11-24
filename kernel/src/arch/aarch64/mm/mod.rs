//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 virtual memory management — module tree mirrors the arch mm
//! interface (same shape as the riscv64/x86_64 backends).

// Memory layout constants and address types
pub mod memory_layout;
pub use memory_layout::*;

// Page table structures (descriptor, PageTable, Satp/TTBR)
pub mod pagetable;
pub use pagetable::*;

// MMU initialization and page mapping
pub mod mmu_init;

pub mod unsafe_edges;
pub use unsafe_edges::*;
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

/// M1a seam: register this arch's IN-TREE mm policy rows (the
/// pre-carve provider). When the arch's policy carve lands in the
/// mm crate, the crate's `register_all` (called earlier by the
/// composition bin) wins the OnceSlot and these rows compile out —
/// flipped arches delete their rows here in the carve commit.
pub fn register_in_tree_mm_policy() {
    // M1a carve (aarch64): every policy row flipped to the mm crate
    // (the x86_64/riscv64 twins); the composition bin's registration
    // wins the OnceSlot before this runs.
}
