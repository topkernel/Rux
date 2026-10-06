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

/// Multiboot info as provided by QEMU (multiboot1, 32-bit fields).
#[repr(C)]
struct MultibootInfo {
    flags: u32,
    mem_lower: u32,
    mem_upper: u32,
    boot_device: u32,
    cmdline: u32,
    mods_count: u32,
    mods_addr: u32,
    syms0: u32,
    syms1: u32,
    syms2: u32,
    mmap_length: u32,
    mmap_addr: u32,
    drives_length: u32,
    drives_addr: u32,
    config_table: u32,
    boot_loader_name: u32,
}

/// One e820-style entry in the multiboot memory map.
#[repr(C)]
struct MbMmapEntry {
    size: u32,
    addr_low: u32,
    addr_high: u32,
    len_low: u32,
    len_high: u32,
    mtype: u32,
}

/// A memory region as handed to the generic memblock layer.
#[derive(Clone, Copy)]
pub struct BootMemoryRegion {
    pub start: u64,
    pub end: u64,
    pub usable: bool,
}

/// Cap the early-copied memory map (multiboot reports at most a handful).
const MAX_MMAP_ENTRIES: usize = 64;
/// Cap the early-copied command line (matches cmdline.rs buffer).
const CMDLINE_MAX: usize = 1024;

static mut BOOT_MMAP: [BootMemoryRegion; MAX_MMAP_ENTRIES] =
    [BootMemoryRegion { start: 0, end: 0, usable: false }; MAX_MMAP_ENTRIES];
static mut BOOT_MMAP_COUNT: usize = 0;
static mut BOOT_CMDLINE: [u8; CMDLINE_MAX] = [0; CMDLINE_MAX];
static mut BOOT_MAGIC: u32 = 0;

extern "C" {
    static boot_mb_magic: u32;
    static boot_mb_info: u32;
}

/// Get the multiboot magic (0x2BADB002 when booted by a multiboot loader).
pub fn get_boot_magic() -> u32 {
    // SAFETY: written by boot.S before any Rust code runs.
    unsafe { core::ptr::read_volatile(&raw const boot_mb_magic) }
}

/// Placeholder for the riscv64 FDT interface — x86 has no device tree.
/// Callers use it to locate the boot device tree; return 0 (none).
pub fn get_dtb_pointer() -> u64 {
    0
}

/// Copy the memory map and command line out of the multiboot blob.
///
/// MUST run while the bootstrap identity mapping is still active
/// (before `mm::init()` switches CR3).
pub fn early_boot_init() {
    let mbi: &MultibootInfo = unsafe {
        let ptr = core::ptr::read_volatile(&raw const boot_mb_info) as usize as *const MultibootInfo;
        if ptr as usize == 0 {
            return;
        }
        &*ptr
    };

    // SAFETY: BSS scratch, single-threaded early boot context.
    unsafe {
        BOOT_MAGIC = get_boot_magic();

        // ---- Memory map (mmap format: flags bit 6) ----
        if mbi.flags & (1 << 6) != 0 && mbi.mmap_addr != 0 && mbi.mmap_length != 0 {
            let mut off = 0u32;
            let mut idx = 0usize;
            while off < mbi.mmap_length && idx < MAX_MMAP_ENTRIES {
                let entry = &*((mbi.mmap_addr + off) as *const MbMmapEntry);
                let size = if entry.size != 0 { entry.size } else { 20 };
                let start = (entry.addr_low as u64) | ((entry.addr_high as u64) << 32);
                let len = (entry.len_low as u64) | ((entry.len_high as u64) << 32);
                BOOT_MMAP[idx] = BootMemoryRegion {
                    start,
                    end: start + len,
                    usable: entry.mtype == 1,
                };
                idx += 1;
                off += size + 4;
            }
            BOOT_MMAP_COUNT = idx;
        } else {
            // Fall back to mem_lower/mem_upper (flags bit 0).
            let upper_end = (1u64 << 20) + mbi.mem_upper as u64 * 1024;
            BOOT_MMAP[0] = BootMemoryRegion { start: 0, end: upper_end, usable: true };
            BOOT_MMAP_COUNT = 1;
        }

        // ---- Command line (flags bit 2) ----
        if mbi.flags & (1 << 2) != 0 && mbi.cmdline != 0 {
            let src = mbi.cmdline as *const u8;
            let mut i = 0usize;
            while i < CMDLINE_MAX - 1 {
                let b = core::ptr::read_volatile(src.add(i));
                if b == 0 {
                    break;
                }
                BOOT_CMDLINE[i] = b;
                i += 1;
            }
        }
    }
}

/// The early-copied memory map (valid for the whole boot).
pub fn boot_memory_regions() -> &'static [BootMemoryRegion] {
    // SAFETY: written once in early_boot_init, read-only afterwards.
    unsafe {
        let count = core::ptr::read_volatile(&raw const BOOT_MMAP_COUNT);
        &BOOT_MMAP[..count]
    }
}

/// The early-copied kernel command line, as a &str prefix (NUL-terminated
/// buffer); empty string when the loader passed none.
pub fn boot_cmdline() -> &'static str {
    // SAFETY: written once in early_boot_init, read-only afterwards.
    let bytes: &[u8] = unsafe { &BOOT_CMDLINE };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..end]).unwrap_or("")
}

// ---------------------------------------------------------------------------
// 64-bit high-half entry — first instructions of the kernel image
// ---------------------------------------------------------------------------

core::arch::global_asm!(
    r#"
.section .text.boot64, "ax"
.align 4096
.global __x86_64_start
.type __x86_64_start, @function
__x86_64_start:
    /* We arrive here from boot.S with:
     *   - long mode + higher-half mapping active
     *   - rsp on the low boot stack (still identity-mapped)
     *   - selectors already loaded (0x08/0x10) */
    xor rbp, rbp
    /* Keep the low boot stack for now; mm::init switches to the real
     * boot task stack when the scheduler comes up. */
    call {early_boot_init}
    call {rust_main}
1:  hlt
    jmp 1b
.size __x86_64_start, . - __x86_64_start
"#,
    early_boot_init = sym early_boot_init,
    rust_main = sym crate::rust_main,
);
