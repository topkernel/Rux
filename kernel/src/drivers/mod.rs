//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 设备驱动模块

pub mod intc;
pub mod timer;
pub mod blkdev;
pub mod pci;
pub mod virtio;
pub mod net;

#[cfg(feature = "riscv64")]
pub mod gpu;

pub mod keyboard;
pub mod mouse;

// Re-export VirtIO probe module for backward compatibility
pub use virtio::probe;
