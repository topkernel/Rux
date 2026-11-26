//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The DRIVER SERVICE SEAM (crate-flip rung 4): the framework→driver
//! call surface as one registration table, the NetHooks shape applied
//! to the driver tree. Every place the framework (or a sibling service
//! crate) calls INTO a driver leaf resolves through a row here; the
//! drivers service crate fills the table at composition time
//! (register_all, before rust_main_impl), and pre-registration builds
//! see the no-op EMPTY defaults — the same contract NetHooks gives the
//! net stack.
//!
//! Row census (measured before the flip): devfs char rows (watchdog,
//! access_tokenid), loopback init/device/send/poll/stats (probe, netdev
//! registry, the net stack's send/poll short-circuits, /proc/net), and
//! the fbdev init/ioctl/file-identity/user-mapped-mark surface (boot,
//! the ioctl+mmap dispatch, tests).
//!
//! Kernel layer policy (docs/architecture/kernel-layers.md):
//! this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::vec::Vec;
use crate::fs::{types::File, types::FileOps};
use crate::fs::types::DevNo;
use crate::drivers::net::space::{DeviceStats, NetDevice};
use crate::net::types::SkBuff;

/// One char-device row the owning driver leaf registers: the (devno,
/// ops) pair devfs registers at its historical populate point, plus the
/// /dev node name+mode when the driver owns one (`None` = registry-only;
/// e.g. access_tokenid's node comes from the ipc access-token store).
pub struct CharDeviceRow {
    pub devno: DevNo,
    pub ops: &'static FileOps,
    /// (node name, mode) — joined into devfs's char_nodes batch insert.
    pub node: Option<(&'static str, u32)>,
}

/// The framework→driver-leaf call surface. Every field is a row the
/// drivers crate implements; EMPTY is the pre-registration no-op
/// table (crate absent or not yet composed).
#[derive(Clone, Copy)]
pub struct DriverHooks {
    /// Char-device rows registered at devfs populate (the historical
    /// register_char_device sequence: after the framework's ashmem row).
    pub devfs_char_rows: fn() -> Vec<CharDeviceRow>,
    /// Bring up "lo" (true = registered). The caller keeps the sysfs
    /// uevent and boot-report sequencing.
    pub loopback_init: fn() -> bool,
    /// The registered loopback netdev (netdev registry lookups).
    pub get_loopback_device: fn() -> Option<&'static mut NetDevice>,
    /// Transmit on "lo" (the net stack's loopback short-circuits).
    pub loopback_send: fn(SkBuff) -> i32,
    /// Drain one backlog skb (the net stack's poll path).
    pub loopback_poll: fn() -> Option<SkBuff>,
    /// "lo" counters for /proc/net.
    pub loopback_stats: fn() -> DeviceStats,
    /// Register /dev/fb0 + start the presenter (boot, after the GPU
    /// framebuffer info exists).
    pub init_fbdev: fn() -> Result<(), ()>,
    /// /dev/fb0 file identity (ioctl + mmap dispatch).
    pub is_fb_file: fn(&File) -> bool,
    /// The FBIO_* dispatch for /dev/fb0 fds.
    pub fbdev_ioctl: fn(u32, usize) -> i64,
    /// The WDIOC_* dispatch for /dev/watchdog fds (None = not a
    /// watchdog file).
    pub watchdog_ioctl: fn(&File, u32, usize) -> Option<i64>,
    /// The TOKENID ioctl dispatch for /dev/access_token_id fds (None =
    /// not a tokenid file).
    pub tokenid_ioctl: fn(&File, u32, usize) -> Option<i64>,
    /// Mark the framebuffer user-mapped (kick the auto-flush presenter).
    pub mark_fb_user_mapped: fn(),
    /// Register the virtio probe family's initcall rows (the three
    /// DeviceInit entries: virtio-blk MMIO @Early, virtio-blk PCI
    /// @Device, virtio-net @Late). Dispatched from the boot core at
    /// the family's historical registration point — inside
    /// rust_main_tail, right before the Early level drains, i.e.
    /// AFTER arch::smp::init() (the initcall table takes a Spinlock,
    /// so this row must NOT be filled from the composition bin's
    /// pre-rust_main register_all, which runs before the per-CPU area
    /// exists on x86_64).
    pub register_probe_initcalls: fn(),
    /// PCI hotplug rescan (`echo 1 > /sys/bus/pci/rescan`): re-enumerate
    /// the ECAM space for virtio-blk functions not yet claimed. Returns
    /// the number of disks bound. Cold, user-triggered.
    pub pci_rescan_block_hotplug: fn() -> usize,
}

// The no-op defaults. Named fns (not closure coercions) so each row's
// no-behavior is greppable.
fn no_char_rows() -> Vec<CharDeviceRow> {
    Vec::new()
}
fn no_loopback_init() -> bool {
    false
}
fn no_loopback_device() -> Option<&'static mut NetDevice> {
    None
}
fn no_loopback_send(_skb: SkBuff) -> i32 {
    0
}
fn no_loopback_poll() -> Option<SkBuff> {
    None
}
fn no_loopback_stats() -> DeviceStats {
    DeviceStats::default()
}
fn no_init_fbdev() -> Result<(), ()> {
    Ok(())
}
fn no_fb_file(_file: &File) -> bool {
    false
}
fn no_fbdev_ioctl(_cmd: u32, _arg: usize) -> i64 {
    -(crate::errno::constants::ENOTTY as i64)
}
fn no_mark_fb_user_mapped() {}
fn no_register_probe_initcalls() {}
fn no_pci_rescan_block_hotplug() -> usize {
    0
}
fn no_watchdog_ioctl(_file: &File, _cmd: u32, _arg: usize) -> Option<i64> {
    None
}
fn no_tokenid_ioctl(_file: &File, _cmd: u32, _arg: usize) -> Option<i64> {
    None
}

impl DriverHooks {
    /// The pre-registration table: every row a no-op (or ENOTTY for the
    /// ioctl row, the not-a-tty answer). A framework build without the
    /// drivers crate composed boots without the leaf devices — the same
    /// contract NetHooks::EMPTY gives a net-less framework build.
    pub const EMPTY: DriverHooks = DriverHooks {
        devfs_char_rows: no_char_rows,
        loopback_init: no_loopback_init,
        get_loopback_device: no_loopback_device,
        loopback_send: no_loopback_send,
        loopback_poll: no_loopback_poll,
        loopback_stats: no_loopback_stats,
        init_fbdev: no_init_fbdev,
        is_fb_file: no_fb_file,
        fbdev_ioctl: no_fbdev_ioctl,
        watchdog_ioctl: no_watchdog_ioctl,
        tokenid_ioctl: no_tokenid_ioctl,
        mark_fb_user_mapped: no_mark_fb_user_mapped,
        register_probe_initcalls: no_register_probe_initcalls,
        pci_rescan_block_hotplug: no_pci_rescan_block_hotplug,
    };
}

pub static DRIVER_HOOKS: crate::safety::slot::OnceSlot<DriverHooks> =
    crate::safety::slot::OnceSlot::new();

/// Registration entry (called by the drivers service crate at boot;
/// first publish wins — a second registration is a no-op).
pub fn register_driver_hooks(hooks: DriverHooks) {
    let _ = DRIVER_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> DriverHooks {
    *DRIVER_HOOKS.get().unwrap_or(&DriverHooks::EMPTY)
}

// The framework-side call surface (one getter per row).

/// Char-device rows for devfs populate (registration order = the
/// historical sequence).
#[inline]
pub fn devfs_char_rows() -> Vec<CharDeviceRow> {
    (hooks().devfs_char_rows)()
}
/// Bring up "lo".
#[inline]
pub fn loopback_init() -> bool {
    (hooks().loopback_init)()
}
/// The registered loopback netdev.
#[inline]
pub fn get_loopback_device() -> Option<&'static mut NetDevice> {
    (hooks().get_loopback_device)()
}
/// Transmit on "lo".
#[inline]
pub fn loopback_send(skb: SkBuff) -> i32 {
    (hooks().loopback_send)(skb)
}
/// Drain one loopback backlog skb.
#[inline]
pub fn loopback_poll() -> Option<SkBuff> {
    (hooks().loopback_poll)()
}
/// "lo" counters.
#[inline]
pub fn loopback_stats() -> DeviceStats {
    (hooks().loopback_stats)()
}
/// Register /dev/fb0 + start the presenter.
#[inline]
pub fn init_fbdev() -> Result<(), ()> {
    (hooks().init_fbdev)()
}
/// /dev/fb0 file identity.
#[inline]
pub fn is_fb_file(file: &File) -> bool {
    (hooks().is_fb_file)(file)
}
/// The FBIO_* dispatch.
#[inline]
pub fn fbdev_ioctl(cmd: u32, arg: usize) -> i64 {
    (hooks().fbdev_ioctl)(cmd, arg)
}
/// Mark the framebuffer user-mapped.
#[inline]
pub fn mark_fb_user_mapped() {
    (hooks().mark_fb_user_mapped)()
}
/// The WDIOC_* dispatch (None = not a watchdog file).
#[inline]
pub fn watchdog_ioctl(file: &File, cmd: u32, arg: usize) -> Option<i64> {
    (hooks().watchdog_ioctl)(file, cmd, arg)
}
/// The TOKENID ioctl dispatch (None = not a tokenid file).
#[inline]
pub fn tokenid_ioctl(file: &File, cmd: u32, arg: usize) -> Option<i64> {
    (hooks().tokenid_ioctl)(file, cmd, arg)
}
/// Register the virtio probe family's initcall rows (boot core's
/// historical registration point, before the Early drain).
#[inline]
pub fn register_probe_initcalls() {
    (hooks().register_probe_initcalls)()
}
/// PCI hotplug rescan (`echo 1 > /sys/bus/pci/rescan`).
#[inline]
pub fn pci_rescan_block_hotplug() -> usize {
    (hooks().pci_rescan_block_hotplug)()
}
