//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The DRIVERS SERVICE CRATE (crate-flip rung 4): the leaf driver
//! files — watchdog, access_tokenid, the loopback netdev, the
//! simple-framebuffer/fbdev display leaves, and the virtio probe
//! orchestrator — under a crate-level unsafe-deny boundary. The
//! framework keeps the mechanism drivers (virtio transports + virtio
//! device families, intc, timer/clockevent machinery, rtc timekeeping,
//! pci, the netdev/block/GPU registries, DRM's GEM allocator, the
//! framebuffer pixel substrate) and the two seams: drivers/hooks.rs
//! (the DriverHooks table this crate fills) and drivers/initcalls.rs
//! (the leveled device-init table).
//!
//! Sole-copy state (the rung-1 recipe's final step): the service files
//! LIVE in this crate (src/drivers/); the framework keeps every
//! registry, transport and substrate those files consume.
//!
//! Why these files are leaves (the flip's measurement): every unsafe
//! construct in them is zero — their raw edges (MMIO windows, uaccess,
//! devno identity) already sit on audited framework edges — and their
//! framework-facing surface is exactly the DriverHooks row set. Files
//! whose unsafe IS the device protocol (virtio queues, intc register
//! walks) or whose consumers are framework machinery (rtc: the timer
//! tree; loop_dev: the mount/page-cache path) stay in the framework —
//! the rung's flip commit message carries the full movable-vs-stays
//! census with the numbers.
//!
//! Rung-4 tranche 2 (the probe orchestrator): virtio/probe.rs is pure
//! orchestration — it consumes the framework's virtio/pci/netdev
//! mechanisms downward and its only two upward callers are cold (the
//! boot core's initcall registration point and the /sys/bus/pci/rescan
//! sysfs attribute), both now DriverHooks rows. The other zero-unsafe
//! candidates measured and STAYED framework-side (see the tranche's
//! commit message for the numbers): virtio_pci.rs (the VirtIOPCI type
//! names 1 framework static array + signatures in 4 staying files; its
//! queue ops are on the block-I/O and fsync paths), pci/mod.rs and
//! offset.rs (consumed by those same staying files), hpet.rs and
//! plic.rs (hot timekeeping/IRQ-entry call sites), iostat.rs (block
//! completion accounting), input/{event,ps2}.rs and virtio_cmd.rs
//! (consumed by the staying framework input/gpu/drm trees).
#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the service files' crate::X paths resolve unchanged (the net
// shape).
pub use rux::{
    arch, cmdline, config, console, debug, errno, fs, interrupt, mm,
    net,  safety, sched, security, signal, sync, timer, trace,
};
// The process service tree (crate-flip rung: process): the merged
// view (moved policy + the framework-staying process modules) —
// crate::process::X paths keep resolving, one-way dependency.
pub use ::process::process;
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println, print_status};
// The ipc service tree (crate-flip rung: ipc): the merged view
// (service policy + framework binder) — access_tokenid consumes the
// access_token registry through it.
pub use ::ipc::ipc;
extern crate alloc;

/// The boot FDT scanners (root-cold lane): the moved cmdline walkers,
/// registered through the framework's BootScanHooks rows.
pub mod of_fdt;

// The driver tree, nested under crate::drivers (the historical paths).
pub mod drivers {
    // The framework's remaining driver tree (registries, transports,
    // mechanism drivers) — glob so `crate::drivers::hooks`,
    // `crate::drivers::timer`, `crate::drivers::net::space` etc.
    // resolve for the service files.
    pub use rux::drivers::{blkdev, drm, hooks, initcalls, input,
        intc, loop_dev, pci, rtc, timer};
    pub use rux::drivers::ashmem;

    // The service files (sole copy: src/drivers/, historical paths).
    pub mod watchdog;
    pub mod access_tokenid;

    // The virtio leaf (rung-4 tranche 2): the framework keeps the
    // virtio mechanism tree (the mod.rs core + queues + the virtio_pci
    // transport + the offset consts); the probe orchestrator joins
    // here (sole copy: src/drivers/virtio/probe.rs).
    pub mod virtio {
        pub use rux::drivers::virtio::*;
        pub mod probe;
    }

    // The net leaf: the framework keeps net/{space,virtio_net} (the
    // netdev registry + the virtio protocol driver); loopback joins
    // here.
    pub mod net {
        pub use rux::drivers::net::*;
        pub mod loopback;
    }

    // The gpu leaves: the framework keeps gpu/{framebuffer (the pixel
    // substrate), virtio_gpu, virtio_cmd, the device registry}; the
    // simple-framebuffer probe and the fbdev chardev join here.
    pub mod gpu {
        pub use rux::drivers::gpu::*;
        pub mod fb_simple;
        pub mod fbdev;
    }

    // The framework's whole-module globs (the service files'
    // `use super::*` / crate:: paths).
    pub use crate::arch::*;
    pub use crate::config;
    pub use crate::console;
    pub use crate::debug::*;
    pub use crate::errno;
    pub use crate::fs::*;
    pub use crate::interrupt;
    pub use crate::mm::*;
    pub use crate::process::*;
    pub use crate::safety::*;
    pub use crate::sched::*;
    pub use crate::security::*;
    pub use crate::signal::*;
    pub use crate::sync::*;
    pub use crate::timer::*;
    pub use crate::trace;
}

/// The service-side hook rows (the DriverHooks table's data). Called by
/// the composition bin (bin) before `rust_main_impl`: the framework
/// getters (devfs populate, the netdev registry, /proc/net, the ioctl
/// and mmap dispatch, the fbdev boot init) resolve through these.
pub fn register_all() {
    use rux::drivers::hooks::CharDeviceRow;
    rux::drivers::hooks::register_driver_hooks(rux::drivers::hooks::DriverHooks {
        devfs_char_rows: || {
            alloc::vec![
                CharDeviceRow {
                    devno: drivers::access_tokenid::DEV_ACCESS_TOKENID,
                    ops: &drivers::access_tokenid::TOKENID_OPS,
                    // No /dev node: the ipc access-token registry owns
                    // the node (see devfs populate's char_nodes note).
                    node: None,
                },
                CharDeviceRow {
                    devno: drivers::watchdog::DEV_WATCHDOG,
                    ops: &drivers::watchdog::WATCHDOG_OPS,
                    node: Some(("watchdog", 0o666)),
                },
            ]
        },
        loopback_init: || drivers::net::loopback::loopback_init().is_some(),
        get_loopback_device: drivers::net::loopback::get_loopback_device,
        loopback_send: drivers::net::loopback::loopback_send,
        loopback_poll: drivers::net::loopback::loopback_poll,
        loopback_stats: drivers::net::loopback::loopback_stats,
        init_fbdev: drivers::gpu::fbdev::init_fbdev,
        is_fb_file: drivers::gpu::fbdev::is_fb_file,
        fbdev_ioctl: drivers::gpu::fbdev::fbdev_ioctl,
        watchdog_ioctl: drivers::watchdog::watchdog_file_ioctl,
        tokenid_ioctl: drivers::access_tokenid::tokenid_file_ioctl,
        mark_fb_user_mapped: drivers::gpu::fbdev::mark_fb_user_mapped,
        // The virtio probe family's initcall registration (rung-4
        // tranche 2). NOT called here: the row is dispatched by the
        // boot core at the family's historical registration point
        // (inside rust_main_tail, after arch::smp::init), because the
        // row body takes the initcall table's Spinlock — a register_all
        // call site (pre-rust_main_impl, pre-per-CPU on x86_64) would
        // fault. The rescan row is filled here and dispatched from the
        // sysfs rescan attribute (cold, user-triggered).
        register_probe_initcalls: drivers::virtio::probe::register_probe_initcalls,
        pci_rescan_block_hotplug: drivers::virtio::probe::pci_rescan_block_hotplug,
    });

    // The boot FDT scanners (root-cold lane): the moved cmdline walkers.
    // Registered here like every other row — publication stores plain
    // values only; the framework's boot-core dispatch sites are all cold.
    rux::boot_scan_hooks::register_boot_scan_hooks(rux::boot_scan_hooks::BootScanHooks {
        parse_bootargs: of_fdt::parse_bootargs,
        parse_memory_regions: of_fdt::parse_memory_regions,
        parse_initrd_region: of_fdt::parse_initrd_region,
        parse_cpu_isa_has_v: of_fdt::parse_cpu_isa_has_v,
    });
}
