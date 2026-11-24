//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Device driver module


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
pub mod initcalls;
pub mod hooks;
pub mod drm;

// The VirtIO probe orchestrator moved to the drivers service crate
// (rung-4 tranche 2, sole copy); the boot core reaches it through
// hooks::register_probe_initcalls and sysfs through
// hooks::pci_rescan_block_hotplug.
