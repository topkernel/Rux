//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Device driver module

pub mod intc;
pub mod timer;
pub mod blkdev;
pub mod pci;
pub mod virtio;
pub mod net;

#[cfg(feature = "riscv64")]
pub mod gpu;

pub mod input;

// Re-export VirtIO probe module for backward compatibility
pub use virtio::probe;
