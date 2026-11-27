//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 boot handoff: multiboot1 info parsing.
//!
//! `boot.S` (32-bit stub) jumps to `__x86_64_start` here — the first
//! instruction of the kernel image (.text.boot64) — which lands in the
//! higher-half map and calls `rust_main()`.
//!
//! The multiboot info blob lives in low memory under the bootstrap
//! identity mapping, which dies once `mm::init()` installs the real
//! page tables. `early_boot_init()` therefore COPIES everything we
//! need (memory map, cmdline, bootloader name) into kernel BSS before
//! any of that happens.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

/// Multiboot info as provided by QEMU (multiboot1, 32-bit fields).
#[repr(C)]
pub(crate) struct MultibootInfo {
    pub(crate) flags: u32,
    pub(crate) mem_lower: u32,
    pub(crate) mem_upper: u32,
    pub(crate) boot_device: u32,
    pub(crate) cmdline: u32,
    pub(crate) mods_count: u32,
    pub(crate) mods_addr: u32,
    pub(crate) syms0: u32,
    pub(crate) syms1: u32,
    pub(crate) syms2: u32,
    pub(crate) mmap_length: u32,
    pub(crate) mmap_addr: u32,
    pub(crate) drives_length: u32,
    pub(crate) drives_addr: u32,
    pub(crate) config_table: u32,
    pub(crate) boot_loader_name: u32,
}

/// One e820-style entry in the multiboot memory map.
#[repr(C)]
pub(crate) struct MbMmapEntry {
    pub(crate) size: u32,
    pub(crate) addr_low: u32,
    pub(crate) addr_high: u32,
    pub(crate) len_low: u32,
    pub(crate) len_high: u32,
    pub(crate) mtype: u32,
}

/// A memory region as handed to the generic memblock layer.
#[derive(Clone, Copy)]
pub struct BootMemoryRegion {
    pub start: u64,
    pub end: u64,
    pub usable: bool,
}

/// Cap the early-copied memory map (multiboot reports at most a handful).
pub(crate) const MAX_MMAP_ENTRIES: usize = 64;
/// Cap the early-copied command line (matches cmdline.rs buffer).
pub(crate) const CMDLINE_MAX: usize = 1024;

pub(crate) static mut BOOT_MMAP: [BootMemoryRegion; MAX_MMAP_ENTRIES] =
    [BootMemoryRegion { start: 0, end: 0, usable: false }; MAX_MMAP_ENTRIES];
pub(crate) static mut BOOT_MMAP_COUNT: usize = 0;
pub(crate) static mut BOOT_CMDLINE: [u8; CMDLINE_MAX] = [0; CMDLINE_MAX];
pub(crate) static mut BOOT_MAGIC: u32 = 0;

extern "C" {
    static boot_mb_magic: u32;
    static boot_mb_info: u32;
}
/// Get the multiboot magic (0x2BADB002 when booted by a multiboot loader).
///
/// Collected into `arch::arch_floor` (INV-ARCH-BOOT-7): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_boot_magic() -> u32 {
    crate::arch::arch_floor::x86_64::boot::get_boot_magic()
}

/// Placeholder for the riscv64 FDT interface — x86 has no device tree.
/// Callers use it to locate the boot device tree; return 0 (none).
pub fn get_dtb_pointer() -> u64 {
    0
}
/// Copy the memory map and command line out of the multiboot blob.
///
/// Collected into `arch::arch_floor` (INV-ARCH-BOOT-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn early_boot_init() {
    crate::arch::arch_floor::x86_64::boot::early_boot_init()
}
/// The early-copied memory map (valid for the whole boot).
///
/// Collected into `arch::arch_floor` (INV-ARCH-BOOT-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn boot_memory_regions() -> &'static [BootMemoryRegion] {
    crate::arch::arch_floor::x86_64::boot::boot_memory_regions()
}
/// The early-copied kernel command line, as a &str prefix (NUL-terminated
///
/// Collected into `arch::arch_floor` (INV-ARCH-BOOT-4): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn boot_cmdline() -> &'static str {
    crate::arch::arch_floor::x86_64::boot::boot_cmdline()
}

// ---------------------------------------------------------------------------
// 64-bit high-half entry — first instructions of the kernel image
// ---------------------------------------------------------------------------

