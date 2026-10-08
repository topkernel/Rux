//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Goldfish RTC driver — boot-time wall clock source.
//!
//! QEMU's `virt` machine instantiates a `goldfish-rtc` device at
//! 0x101000 unconditionally (hw/riscv/virt.c: `sysbus_create_simple
//! ("goldfish-rtc", ...)`; visible in the DTB as "google,goldfish-rtc").
//! No `-device` flag is needed. The device exposes the host wall clock
//! (as of VM start) plus guest uptime as a 64-bit nanosecond counter,
//! split across two 32-bit little-endian MMIO registers.
//!
//! Why not fw_cfg `FW_CFG_RTC_TIME` (selector 0x15): the entry does not
//! exist in QEMU v10.2.2 — `hw/nvram/fw_cfg.c`, `include/hw/nvram/
//! fw_cfg.h`, the standard-headers copy and `docs/specs/fw_cfg.rst`
//! contain no RTC entry at all, and the RISC-V `virt` machine's
//! `create_fw_cfg()` registers only `FW_CFG_NB_CPUS` (+ the common
//! set). Selecting an unregistered key yields a zero-length entry, so
//! every read returns 0 (verified empirically: the selector accepts
//! writes and the signature item reads "QEMU", but 0x15 reads as 0).
//! The goldfish RTC is the virt machine's RTC.
//!
//! Register map (32-bit LE, all accesses must be exactly 4 bytes):
//!   0x00 TIME_LOW    read: bits  0-31 of the ns counter
//!   0x04 TIME_HIGH   read: bits 32-63 of the ns counter
//!   0x08+ alarm/IRQ registers — unused here
//!
//! Model: REALTIME = monotonic (CLINT `time` CSR) + WALL_EPOCH_OFFSET_SECS
//! (syscall/time.rs). `rtc_init_wall_clock()` reads the device once at
//! boot and derives the offset; settimeofday/clock_settime re-derive it.

/// MMIO base of the goldfish RTC on the QEMU virt platform.
pub const GOLDFISH_RTC_BASE: u64 = 0x101_000;

/// Kernel-half alias of the device page used for actual register reads
/// (see memory_layout::mmio_alias — the identity window is replaceable
/// by user MAP_FIXED mappings in their own address space).
pub const GOLDFISH_RTC_VA: u64 = crate::arch::mm::memory_layout::mmio_alias(0x101_000);

/// Size of the mapped device region (one page — the register file is
/// 0x20 bytes but the DTB reg spec is the full page).
pub const GOLDFISH_RTC_SIZE: u64 = 0x1000;

const REG_TIME_LOW: u64 = 0x00;
const REG_TIME_HIGH: u64 = 0x04;

/// Read one 32-bit device register.
///
/// # Safety (internal)
/// The page at GOLDFISH_RTC_BASE is identity-mapped with device flags by
/// `setup_device_mappings()`; this helper is only reachable after that.
#[inline]
fn read_reg(offset: u64) -> u32 {
    unsafe { core::ptr::read_volatile((GOLDFISH_RTC_VA + offset) as *const u32) }
}

/// Read the RTC's nanosecond counter (host wall time at VM start + guest
/// uptime, in ns since the Unix epoch).
///
/// The counter is latched one 32-bit half at a time, so a naive
/// LOW/HIGH pair can tear across a low-word wrap (every ~4.3 s of ns).
/// Linux's goldfish-rtc driver re-reads TIME_HIGH around the LOW read and
/// retries until it is stable — same protocol here.
pub fn read_ns() -> u64 {
    loop {
        let hi1 = read_reg(REG_TIME_HIGH) as u64;
        let lo = read_reg(REG_TIME_LOW) as u64;
        let hi2 = read_reg(REG_TIME_HIGH) as u64;
        if hi1 == hi2 {
            return (hi1 << 32) | lo;
        }
    }
}

/// Sanity floor for a plausible RTC reading in NANOseconds: 2001-09-09
/// (1e9 s = 1e18 ns) — filters out a zero/garbage read without rejecting
/// any realistic host date. (The device is read as ns, not s!)
const MIN_PLAUSIBLE_RTC_NS: u64 = 1_000_000_000_000_000_000;

/// Read the RTC once at boot and derive the wall-clock epoch offset.
///
/// The offset is rounded to the nearest second so the composite
/// (monotonic + offset) REALTIME stays within half a second of the RTC
/// value; sub-second accuracy would need a fractional offset in the
/// model (vDSO data page carries whole seconds only).
///
/// Must run after `setup_device_mappings()` (device page mapped) and
/// before `vdso_init()` (which snapshots the offset into the vDSO data
/// page). If the device does not report a plausible time the offset
/// stays 0 and the system runs on monotonic boot time.
pub fn rtc_init_wall_clock() {
    let rtc_ns = read_ns();
    if rtc_ns < MIN_PLAUSIBLE_RTC_NS {
        return;
    }

    // Monotonic clock in ns at (almost) the same instant: 10 MHz CLINT
    // `time` CSR = 100 ns per cycle exactly.
    let mono_ns = crate::arch::cpu::read_time().saturating_mul(100);

    // wall = mono + offset  ⇒  offset = rtc - mono (round to nearest).
    let offset_secs = rtc_ns.saturating_sub(mono_ns).saturating_add(500_000_000) / 1_000_000_000;
    crate::syscall::time::set_wall_epoch_offset_secs(offset_secs);
}

/// Current wall-clock seconds since the Unix epoch.
///
/// Single canonical timestamp source for filesystems (ext4 mtime/ctime,
/// tmpfs, rootfs) and UTIME_NOW: monotonic seconds + the wall offset.
/// Once the RTC has been read (or settimeofday ran) this is real UTC.
///
/// Coarse, tick-grid-snapped (see drivers/timer::coarse_ns_since_boot):
/// this MUST agree with what clock_gettime(CLOCK_REALTIME_COARSE)
/// returns — LTP utime01 brackets a utime(NULL) stamp between two
/// CLOCK_REALTIME_COARSE samples, and the old live-rdtime conversion
/// here ran ahead of the coarse reads whenever tick-handler updates
/// lagged the timebase.
pub fn wall_secs() -> u64 {
    (crate::drivers::timer::coarse_ns_since_boot()
        + crate::syscall::time::wall_epoch_offset_ns())
        / 1_000_000_000
}
