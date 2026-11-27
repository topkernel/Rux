//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Device driver module


// The subsystem's ONE audited unsafe file (mainline minimal-first
// consolidation, 2026-11-27): every raw edge the drivers inline crosses
// one audited family here (INV-DRV-*); every other file is safe with a
// file-level forbid.
#[path = "unsafe.rs"]
pub mod edges;

pub mod intc;
pub mod timer;
pub mod rtc;
pub mod blkdev;
pub mod pci;
pub mod virtio;
pub mod net;

// virtio-gpu is arch-generic since the PCI transport (virtio_pci.rs) keeps
// firmware-assigned BARs on x86_64 and self-assigns them on riscv64; the
// ECAM walker (pci::find_ecam_devices) is arch-generic too.
pub mod gpu;

pub mod input;
pub mod loop_dev;
pub mod ashmem;
pub mod access_tokenid;
pub mod watchdog;
pub mod drm;

// Re-export VirtIO probe module for backward compatibility
pub use virtio::probe;
