//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The SkBuff free-helper pair (shrink stage, allocation-table MOVE-FREE):
//! `alloc_skb` / `kfree_skb` were the only non-impl free functions of the
//! framework's `net/hotpath.rs`; they move here, into the net service
//! crate, as crate-local definitions. The SkBuff type and its allocation
//! substrate (`SkBuff::alloc` / `release`, the audited INV-SKB-1 body)
//! stay framework-side (`rux::net::types` / `rux::net::unsafe`) — these
//! two are one-line wrappers over that pinned surface, and every former
//! consumer path re-exports them at this crate's root (lib.rs), the
//! reverse of the pre-move direction (the crate used to reach them
//! through the framework re-export).
//!
//! (INV-SKB-1: the sk_buff family — heads are allocator-owned blocks
//! with headroom-tracked data pointers (alloc/reserve/put maintain the
//! span invariants); data_slice_at rows are bounds-checked against the
//! len tail; Send/Sync carried by the head's single-owner discipline.)

#![forbid(unsafe_code)]

use rux::net::types::SkBuff;

/// Helper function to allocate SkBuff
///
/// # Arguments
/// - `size`: Data size
///
/// # Returns
/// The allocated SkBuff
pub fn alloc_skb(size: u32) -> Option<SkBuff> {
    SkBuff::alloc(size)
}

/// Helper function to free SkBuff
///
/// # Arguments
/// - `skb`: SkBuff to free
pub fn kfree_skb(skb: SkBuff) {
    skb.free();
}
