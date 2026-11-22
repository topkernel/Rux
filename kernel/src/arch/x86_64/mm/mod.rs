//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 virtual memory management — module tree mirrors the arch mm
//! interface (same shape as the riscv64 backend).

pub mod memory_layout;
pub use memory_layout::*;

pub mod pagetable;
pub use pagetable::*;

pub mod mmu_init;
pub use mmu_init::*;

pub mod unsafe_edges;
pub use unsafe_edges::*;

pub mod mm_ops;
pub use mm_ops::*;

pub mod page_fault;
pub use page_fault::*;

pub mod exception;
pub use exception::{do_page_fault, fixup_exception};

pub mod fixmap;
pub use fixmap::*;

pub mod asid;
pub use asid::{
    ASID_BITS, MAX_ASID, ASID_KERNEL, ASID_RESERVED, ASID_FIRST,
    alloc_asid, free_asid, asid_usage_count, pcid_enabled, enable_pcid,
    flush_tlb_all, flush_tlb_asid, flush_tlb_page, flush_tlb_page_asid, flush_tlb_range,
    flush_tlb_kernel, switch_root,
    build_satp, satp_to_asid, satp_to_ppn, read_satp, write_satp,
    AsidContext, print_asid_status,
};

/// M1a seam: register this arch's IN-TREE mm policy rows (the
/// pre-carve provider). When the arch's policy carve lands in the
/// rux-mm crate, the crate's `register_all` (called earlier by the
/// composition bin) wins the OnceSlot and these rows compile out —
/// flipped arches delete their rows here in the carve commit.
pub fn register_in_tree_mm_policy() {
    // M1a carve (x86_64): every policy row flipped to the rux-mm crate;
    // the composition bin registered its rows before rust_main_impl
    // (OnceSlot first-wins), so nothing remains to register here.
}
