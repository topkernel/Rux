//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The drivers' audited unsafe edges (framework zone).
//!
//! Pass 3 (seed-core §2c.5) collects every inline unsafe region of
//! drivers/hotpath.rs — including the inline timer/intc modules — into
//! this file, one audited contract per edge. hotpath.rs keeps the
//! policy/flow code and calls these edges; every wrapper carries the
//! contract its census row cited (or an explicit GAP note where no
//! harness exists yet). Arch-specific edges keep the `#[cfg(feature)]`
//! attribute their hotpath caller sites sit behind.
//!
//! Contract registry (mirrors docs/development/unsafe-verification.md):
//! - INV-MMIO-1         — device/register windows are identity-mapped or
//!   MSR-backed before first use; the volatile access shape is fixed.
//! - F5                 — arch timer/CSR/SBI programming floor (per-arch
//!   edges, cfg-gated exactly like their hotpath callers).
//! - INV-BLKDEV-1/2/3   — gendisk registry stability, io_buf pool layout,
//!   driver-private slot discipline.
//! - INV-SAFECALL-1     — the R17-C io_buf carve + pending completion
//!   protocol edges.
//! - F7                 — the PCI pending-table access discipline (the
//!   caller holds the disk's BLK lock; no second acquisition).
//! - INV-DMA-2/3        — vring slot disjointness / device-completed
//!   memory reads.
//! - INV-KPTR-3, INV-SLOT-1, INV-SREF-1, INV-CHARDEV-PRIV-1, INV-KTASK-1,
//!   INV-FS-FILEPRIV-1, F9 — per-area contracts cited at each edge.

use crate::fs::types::{BufferHead, DevNo};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::ptr::read_volatile;
