//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Typed-safety primitives — the unsafe-slim round-3 foundation
//! (lane A, 2026-11-19).
//!
//! The round-1/2 histogram said the unsafe mass is concentrated in two
//! mechanical families: raw-pointer dereferences around long-lived
//! allocations (36% tree-wide) and per-call-site `unsafe { unsafe_fn() }`
//! wrappers (15%). This module is the shared answer: wrappers that carry
//! their safety proof IN THE TYPE, so construction is the one audited
//! unsafe edge and every later access is safe code.
//!
//! The reference discipline, mirroring the landed templates (UserPtr's
//! INV-UACCESS family in `arch/uaccess_safe.rs`, GenDisk's INV-BLKDEV-1,
//! the TaskRef/TaskBorrow handles in `process/task_ref.rs`):
//!
//! * [`ptr::KPtr`] — an owned kernel allocation (`INV-KPTR-*`).
//! * [`ptr::StaticRef`] — a shared reference to machine-lifetime storage
//!   (`INV-SREF-*`).
//! * [`mmio::MmioRegion`] — a bounds-checked volatile device-register
//!   window (`INV-MMIO-*`).
//! * [`call`] — the unsafe-fn-call wrapper discipline plus the
//!   `safe_call!` macro (`INV-SAFECALL-*`, `INV-SAFECAST-*`).
//!
//! Rules that keep this module honest:
//!
//! 1. Every unsafe block below is the SINGLE audited edge for its
//!    invariant, named `INV-XXX-N`, and Kani-checkable where marked
//!    (proofs live in `kernel/verify`).
//! 2. Primitives never grow domain logic; they only hold the pointer
//!    discipline. Device protocol stays in drivers.
//! 3. Escape hatches (`as_ptr`, `into_raw`, `ptr_at`) exist for the
//!    queue/DMA internals that still live on raw addresses; their docs
//!    name the obligation that transfers to the caller.

pub mod call;
pub mod dma;
pub mod mmio;
pub mod ptr;
