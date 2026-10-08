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
    alloc_asid, free_asid, asid_usage_count,
    flush_tlb_all, flush_tlb_asid, flush_tlb_page, flush_tlb_range, flush_tlb_kernel,
    build_satp, satp_to_asid, satp_to_ppn, read_satp, write_satp,
    AsidContext, print_asid_status,
};
