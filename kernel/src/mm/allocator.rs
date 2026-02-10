//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 内存分配器模块
//!
//! 本模块重新导出 Buddy System 分配器的公共接口。
//! Buddy System 是一种高效的内存分配算法，支持 O(log n) 的分配和释放。

pub use crate::mm::buddy_allocator::{BuddyAllocator, init_heap, HEAP_ALLOCATOR};

// 旧的 BumpAllocator 已被废弃，但保留类型定义以避免破坏性更改
#[deprecated(note = "Use BuddyAllocator instead")]
pub type BumpAllocator = BuddyAllocator;
