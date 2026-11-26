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
//! framebuffer pixel substrate) and the two seams: api/api/drivers.rs
//! (the DriverHooks table this crate fills) and drivers/initcalls.rs
//! (the leveled device-init table).
//!
//! Sole-copy state (the rung-1 recipe's final step): the service files
//! LIVE in this crate (src/), flat under its root; the framework keeps every
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
// shape). `net` is NOT in this list: this crate's own net module (the
// loopback leaf) owns the name; the framework's network stack is
// `rux::net`.
pub use rux::{
    arch, cmdline, config, console, debug, errno, fs, interrupt, mm,
    safety, sched, security, signal, sync, timer, trace,
};
// The process service tree (crate-flip rung: process): the merged
// view (moved policy + the framework-staying process modules) —
// crate::process::X paths keep resolving, one-way dependency.
pub use ::process;
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println, print_status};
// The ipc service tree (crate-flip rung: ipc): the merged view
// (service policy + framework binder) — access_tokenid consumes the
// access_token registry through it.
pub use ::ipc;
extern crate alloc;

/// The boot FDT scanners (root-cold lane): the moved cmdline walkers,
/// registered through the framework's BootScanHooks rows.
pub mod of_fdt;

// The framework's remaining driver tree (registries, transports,
// mechanism drivers) — re-exported so the service files reach them as
// names of this root (`hooks`, `pci`, `initcalls`, ...).
// (rux::drivers::hotpath::timer is NOT among them: the crate root's `timer` is
// the framework's timekeeping module, which the service files use; the
// driver one is named by its full path.)
pub use rux::drivers::{types, hotpath, hotpath::intc};

// The service files (sole copy), flat at the crate root
// (seed-core-design 2c.2).
pub mod watchdog;
pub mod access_tokenid;

// The virtio leaf (rung-4 tranche 2): the framework keeps the
// virtio mechanism tree (the mod.rs core + queues + the virtio_pci
// transport + the offset consts); the probe orchestrator joins
// here (sole copy: src/virtio/probe.rs).
pub mod virtio {
    // virtio/{mod.rs,queue.rs,offset.rs,virtio_pci.rs} are flattened into
    // hotpath: the old module paths (queue::, offset::, virtio_pci::) are
    // hotpath item names now; the device types come from types.
    pub use rux::drivers::hotpath::*;
    pub use rux::drivers::types::*;
    pub mod probe;
}

// The net leaf: the framework keeps net/{space,virtio_net} (the
// netdev registry + the virtio protocol driver); loopback joins
// here. This module owns the crate root's `net` name.
pub mod net {
    // net/space and net/virtio_net are flattened into hotpath: their items
    // and the module names' old paths live as hotpath items now.
    pub use rux::drivers::{
        hotpath::{
            dev_flags,
            register_netdevice,
            unregister_netdevice,
            get_netdevice_by_index,
            get_netdevice_by_name,
            get_netdevice_count,
            virtio_net_init,
            get_virtio_net_device,
            get_virtio_net_device_net,
            virtio_net_get_device,
            virtio_net_enable_device_interrupt,
            init_pci,
        },
        types::{
            NetDevice,
            NetDeviceOps,
            DeviceStats,
            ArpHrdType,
        },
    };
    pub mod loopback;
}

// The gpu leaves: the framework keeps gpu/{framebuffer (the pixel
// substrate), virtio_gpu, virtio_cmd, the device registry}; the
// simple-framebuffer probe and the fbdev chardev join here.
pub mod gpu {
    // gpu/{framebuffer,virtio_cmd,virtio_gpu} are flattened into hotpath.
    pub use rux::drivers::{
        hotpath::{
            probe_virtio_gpu,
            set_framebuffer_info,
            get_framebuffer_info,
            set_gpu_device,
            flush_framebuffer,
            with_gpu_device,
        },
        types::{
            FrameBuffer,
            FrameBufferInfo,
            VirtioGpuDevice,
        },
    };
    pub mod fb_simple;
    pub mod fbdev;
}

/// The service-side hook rows (the DriverHooks table's data). Called by
/// the composition bin (bin) before `rust_main_impl`: the framework
/// getters (devfs populate, the netdev registry, /proc/net, the ioctl
/// and mmap dispatch, the fbdev boot init) resolve through these.
pub fn register_all() {
    use rux::api::drivers::CharDeviceRow;
    rux::api::drivers::register_driver_hooks(rux::api::drivers::DriverHooks {
        devfs_char_rows: || {
            alloc::vec![
                CharDeviceRow {
                    devno: access_tokenid::DEV_ACCESS_TOKENID,
                    ops: &access_tokenid::TOKENID_OPS,
                    // No /dev node: the ipc access-token registry owns
                    // the node (see devfs populate's char_nodes note).
                    node: None,
                },
                CharDeviceRow {
                    devno: watchdog::DEV_WATCHDOG,
                    ops: &watchdog::WATCHDOG_OPS,
                    node: Some(("watchdog", 0o666)),
                },
            ]
        },
        loopback_init: || net::loopback::loopback_init().is_some(),
        get_loopback_device: net::loopback::get_loopback_device,
        loopback_send: net::loopback::loopback_send,
        loopback_poll: net::loopback::loopback_poll,
        loopback_stats: net::loopback::loopback_stats,
        init_fbdev: gpu::fbdev::init_fbdev,
        is_fb_file: gpu::fbdev::is_fb_file,
        fbdev_ioctl: gpu::fbdev::fbdev_ioctl,
        watchdog_ioctl: watchdog::watchdog_file_ioctl,
        tokenid_ioctl: access_tokenid::tokenid_file_ioctl,
        mark_fb_user_mapped: gpu::fbdev::mark_fb_user_mapped,
        // The virtio probe family's initcall registration (rung-4
        // tranche 2). NOT called here: the row is dispatched by the
        // boot core at the family's historical registration point
        // (inside rust_main_tail, after arch::smp::init), because the
        // row body takes the initcall table's Spinlock — a register_all
        // call site (pre-rust_main_impl, pre-per-CPU on x86_64) would
        // fault. The rescan row is filled here and dispatched from the
        // sysfs rescan attribute (cold, user-triggered).
        register_probe_initcalls: virtio::probe::register_probe_initcalls,
        pci_rescan_block_hotplug: virtio::probe::pci_rescan_block_hotplug,
    });

    // The boot FDT scanners (root-cold lane): the moved cmdline walkers.
    // Registered here like every other row — publication stores plain
    // values only; the framework's boot-core dispatch sites are all cold.
    rux::api::boot::register_boot_scan_hooks(rux::api::boot::BootScanHooks {
        parse_bootargs: of_fdt::parse_bootargs,
        parse_memory_regions: of_fdt::parse_memory_regions,
        parse_initrd_region: of_fdt::parse_initrd_region,
        parse_cpu_isa_has_v: of_fdt::parse_cpu_isa_has_v,
    });
}
