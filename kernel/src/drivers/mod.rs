//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 设备驱动模块

pub mod intc;
pub mod timer;
pub mod blkdev;
pub mod virtio;
pub mod net;

// Re-export VirtIO probe module for backward compatibility
pub use virtio::probe;
