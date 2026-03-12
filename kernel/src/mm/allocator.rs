//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Memory Allocator Module
//!
//! This module re-exports the Buddy System allocator's public interface.
//! The Buddy System is an efficient memory allocation algorithm that supports O(log n) allocation and deallocation.

pub use crate::mm::buddy_allocator::{BuddyAllocator, init_heap, HEAP_ALLOCATOR};

// The old BumpAllocator has been deprecated, but the type alias is kept to avoid breaking changes
#[deprecated(note = "Use BuddyAllocator instead")]
pub type BumpAllocator = BuddyAllocator;
