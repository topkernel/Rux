//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The BOOT FDT SCANNERS (root-cold lane, 2026-11-24): the flattened
//! device tree walkers that mine the bootloader DTB at boot —
//! /chosen/bootargs, the /memory nodes, the linux,initrd-start/end
//! pair, and the first /cpus/cpu@* riscv,isa string — physically
//! moved from kernel/core/src/cmdline.rs (the boot-only cold half of that
//! file; the Linux parallel is drivers/of/fdt.c's
//! early_init_dt_scan family). The framework keeps the storage, the
//! getters (read on trap/timer/sched paths by name) and the three
//! audited INV-FDT-1 read edges (cmdline::{fdt_u8, fdt_add,
//! fdt_u32}); this module reaches them as direct cross-crate calls
//! (crate -> core, inlinable) and is itself ZERO unsafe under the
//! crate forbid.
//!
//! The framework dispatches here through the four BootScanHooks rows
//! (kernel/core/src/boot_scan_hooks.rs) — every call site is boot-time
//! cold (cmdline::init, lib.rs's memblock/paging phases, vext's ISA
//! prime), so the row indirection is a few ns on paths doing µs–ms
//! of work. The one behavioral edit forced by the forbid: the
//! moved parse_memory_regions initializes its output array with a
//! plain repeat expression instead of `core::mem::zeroed()` — same
//! all-zero bytes, no unsafe.

use crate::cmdline::{fdt_add, fdt_u32, fdt_u8, MemoryRegion, MAX_MEMORY_REGIONS};
use alloc::string::String;
use alloc::vec;

const FDT_BEGIN_NODE: u32 = 0x1;
const FDT_END_NODE: u32 = 0x2;
const FDT_PROP: u32 = 0x3;
const FDT_END: u32 = 0x9;

/// Parse the kernel command line from the bootloader DTB (`/chosen`).
pub fn parse_bootargs(dtb_ptr: u64) -> Option<String> {
    let fdt = dtb_ptr as *const u8;

    // Big-endian u32 at a header offset (INV-FDT-1 reads).
    let read_u32 = |offset: usize| -> u32 { fdt_u32(fdt_add(fdt, offset as isize)) };

    // Read magic number
    let magic = read_u32(0);
    if magic != 0xd00dfeed {
        return None;
    }

    // Read header info
    // FDT header layout (offset -> meaning):
    // 0x00: magic
    // 0x04: totalsize
    // 0x08: off_dt_struct
    // 0x0C: off_dt_strings
    // 0x10: off_mem_rsvmap
    // 0x14: version
    // 0x18: last_comp_version
    // 0x1C: boot_cpuid_phys
    // 0x20: size_dt_strings
    // 0x24: size_dt_struct
    let _totalsize = read_u32(0x04) as usize;
    let off_dt_struct = read_u32(0x08) as usize;
    let off_dt_strings = read_u32(0x0C) as usize;
    let _off_mem_rsvmap = read_u32(0x10) as usize;
    let version = read_u32(0x14) as usize;
    let _last_comp_version = read_u32(0x18) as usize;
    let _boot_cpuid_phys = read_u32(0x1C) as usize;
    let size_dt_strings = read_u32(0x20) as usize;
    let size_dt_struct = read_u32(0x24) as usize;

    let mut ptr = fdt_add(fdt, off_dt_struct as isize);
    let end = fdt_add(fdt, (off_dt_struct + size_dt_struct) as isize);
    let strings = fdt_add(fdt, off_dt_strings as isize);

    // Big-endian u32 at a cursor (INV-FDT-1 reads).
    let read_u32_at = |p: *const u8| -> u32 { fdt_u32(p) };

    let mut depth = 0;
    let mut in_chosen = false;
    let _ = version;
    let _ = size_dt_strings;

    while ptr < end {
        let token = fdt_u32(ptr);
        ptr = fdt_add(ptr, 4);

        match token {
            FDT_BEGIN_NODE => {
                // Read node name
                let mut nodename = [0u8; 64];
                let mut i = 0;
                while fdt_u8(ptr) != 0 && i < 64 {
                    nodename[i] = fdt_u8(ptr);
                    ptr = fdt_add(ptr, 1);
                    i += 1;
                }
                ptr = fdt_add(ptr, 1);
                // Align to 4 bytes
                ptr = fdt_add(ptr, ((4 - ((ptr as usize) & 3)) & 3) as isize);

                let name = core::str::from_utf8(&nodename[..i]).ok()?;
                if name == "chosen" || name.starts_with("chosen@") {
                    in_chosen = true;
                }
                depth += 1;
            }
            FDT_END_NODE => {
                if in_chosen && depth == 1 {
                    in_chosen = false;
                }
                depth -= 1;
            }
            FDT_PROP => {
                let len = fdt_u32(ptr) as usize;
                let nameoff = fdt_u32(fdt_add(ptr, 4)) as usize;
                ptr = fdt_add(ptr, 8);

                // Read property name
                let mut name_ptr = fdt_add(strings, nameoff as isize);
                let mut prop_name = [0u8; 32];
                let mut i = 0;
                while fdt_u8(name_ptr) != 0 && i < 32 {
                    prop_name[i] = fdt_u8(name_ptr);
                    name_ptr = fdt_add(name_ptr, 1);
                    i += 1;
                }
                let name = core::str::from_utf8(&prop_name[..i]).ok().unwrap_or("???");

                if in_chosen && name == "bootargs" {
                    // Read bootargs string
                    let mut bootargs = vec![0u8; len];
                    for j in 0..len {
                        bootargs[j] = fdt_u8(fdt_add(ptr, j as isize));
                    }
                    if let Ok(bootargs_str) = core::str::from_utf8(&bootargs) {
                        // Remove trailing null character
                        let trimmed = bootargs_str.trim_end_matches('\0');
                        return Some(String::from(trimmed));
                    }
                }

                ptr = fdt_add(ptr, len as isize);
                // Align to 4 bytes
                ptr = fdt_add(ptr, ((4 - ((ptr as usize) & 3)) & 3) as isize);
            }
            FDT_END => {
                break;
            }
            _ => {
                // Unknown token, ignore
                break;
            }
        }
    }

    None
}

/// Parse memory regions from device tree
///
/// This function parses /memory nodes from the device tree to discover
/// available physical memory regions. This follows the approach of
/// using device tree for memory discovery.
///
/// # Arguments
/// - `dtb_ptr`: Device tree flattened data pointer (virtual address)
///
/// # Returns
/// - `[MemoryRegion; MAX_MEMORY_REGIONS]`: the discovered memory
///   regions (stack-allocated, no heap needed; the tail beyond the
///   discovered count is all-zero)
///
/// # Device Tree Format
/// Memory nodes have the format:
/// ```dts
/// memory@80000000 {
///     device_type = "memory";
///     reg = <0x00 0x80000000 0x00 0x8000000>;  // <address-high address-low size-high size-low>
/// };
/// ```
pub fn parse_memory_regions(dtb_ptr: u64) -> [MemoryRegion; MAX_MEMORY_REGIONS] {
    // All-zero start state (the pre-move body used mem::zeroed; a
    // plain repeat of the literal is the same bytes with no unsafe).
    let mut regions: [MemoryRegion; MAX_MEMORY_REGIONS] =
        [MemoryRegion { base: 0, size: 0 }; MAX_MEMORY_REGIONS];
    let mut count = 0usize;
    let fdt = dtb_ptr as *const u8;

    // Big-endian u32 at a header offset (INV-FDT-1 reads).
    let read_u32 = |offset: usize| -> u32 { fdt_u32(fdt_add(fdt, offset as isize)) };

    // Read magic number
    let magic = read_u32(0);
    if magic != 0xd00dfeed {
        return regions;
    }

    // Read header info
    let _totalsize = read_u32(0x04) as usize;
    let off_dt_struct = read_u32(0x08) as usize;
    let off_dt_strings = read_u32(0x0C) as usize;
    let size_dt_strings = read_u32(0x20) as usize;
    let size_dt_struct = read_u32(0x24) as usize;
    let _ = size_dt_strings;

    let mut ptr = fdt_add(fdt, off_dt_struct as isize);
    let end = fdt_add(fdt, (off_dt_struct + size_dt_struct) as isize);
    let strings = fdt_add(fdt, off_dt_strings as isize);

    // Big-endian u32 at a cursor (INV-FDT-1 reads).
    let read_u32_at = |p: *const u8| -> u32 { fdt_u32(p) };

    let mut depth = 0;
    let mut in_memory = false;
    let mut has_device_type_memory = false;
    // QEMU's aarch64 DTB emits `reg` BEFORE `device_type` inside
    // /memory@... — buffer the reg cells and commit at node exit so the
    // parse no longer depends on property order.
    let mut pending_reg: Option<(u64, u64)> = None;

    while ptr < end {
        let token = fdt_u32(ptr);
        ptr = fdt_add(ptr, 4);

        match token {
            FDT_BEGIN_NODE => {
                // Read node name
                let mut nodename = [0u8; 64];
                let mut i = 0;
                while fdt_u8(ptr) != 0 && i < 64 {
                    nodename[i] = fdt_u8(ptr);
                    ptr = fdt_add(ptr, 1);
                    i += 1;
                }
                ptr = fdt_add(ptr, 1);
                // Align to 4 bytes
                ptr = fdt_add(ptr, ((4 - ((ptr as usize) & 3)) & 3) as isize);

                let name = core::str::from_utf8(&nodename[..i]).unwrap_or("");
                // Memory nodes are named "memory" or "memory@<address>"
                in_memory = name == "memory" || name.starts_with("memory@");
                has_device_type_memory = false;
                depth += 1;
            }
            FDT_END_NODE => {
                if in_memory && has_device_type_memory {
                    if let Some((base, size)) = pending_reg.take() {
                        if size > 0 && count < MAX_MEMORY_REGIONS {
                            regions[count] = MemoryRegion {
                                base: base as usize,
                                size: size as usize,
                            };
                            count += 1;
                        }
                    }
                }
                in_memory = false;
                has_device_type_memory = false;
                pending_reg = None;
                depth -= 1;
            }
            FDT_PROP => {
                let len = fdt_u32(ptr) as usize;
                let nameoff = fdt_u32(fdt_add(ptr, 4)) as usize;
                ptr = fdt_add(ptr, 8);

                // Read property name
                let mut name_ptr = fdt_add(strings, nameoff as isize);
                let mut prop_name = [0u8; 32];
                let mut i = 0;
                while fdt_u8(name_ptr) != 0 && i < 32 {
                    prop_name[i] = fdt_u8(name_ptr);
                    name_ptr = fdt_add(name_ptr, 1);
                    i += 1;
                }
                let name = core::str::from_utf8(&prop_name[..i]).unwrap_or("???");

                // Check for device_type = "memory"
                if in_memory && name == "device_type" {
                    let mut value = [0u8; 16];
                    for j in 0..len.min(16) {
                        value[j] = fdt_u8(fdt_add(ptr, j as isize));
                    }
                    if let Ok(value_str) = core::str::from_utf8(&value[..len.min(16)]) {
                        has_device_type_memory = value_str.trim_end_matches('\0') == "memory";
                    }
                }

                // Parse "reg" property for memory regions
                // Format: <address-high address-low size-high size-low> (for 64-bit addresses)
                // or <address size> (for 32-bit addresses)
                if in_memory && name == "reg" && len >= 16 {
                    // Read 64-bit address and size (each is 2 cells of 32 bits)
                    let addr_high = read_u32_at(ptr) as u64;
                    let addr_low = read_u32_at(fdt_add(ptr, 4)) as u64;
                    let size_high = read_u32_at(fdt_add(ptr, 8)) as u64;
                    let size_low = read_u32_at(fdt_add(ptr, 12)) as u64;

                    let base = (addr_high << 32) | addr_low;
                    let size = (size_high << 32) | size_low;

                    if size > 0 {
                        pending_reg = Some((base, size));
                    }
                }

                ptr = fdt_add(ptr, len as isize);
                // Align to 4 bytes
                ptr = fdt_add(ptr, ((4 - ((ptr as usize) & 3)) & 3) as isize);
            }
            FDT_END => {
                break;
            }
            _ => {
                // Unknown token, ignore
                break;
            }
        }
    }

    regions
}

/// True when the first /cpus/cpu@* node's "riscv,isa" property lists
/// the V extension. The ISA string looks like "rv64imafdcvh" (with
/// optional _multi_letter names after): the base prefix "rv" + width
/// digits is skipped, then any single-letter extension 'v' counts.
/// Used to prime the V-extension capability cache at boot (vext.rs).
pub fn parse_cpu_isa_has_v(dtb_ptr: u64) -> bool {
    let fdt = dtb_ptr as *const u8;

    if fdt_u32(fdt) != 0xd00dfeed {
        return false;
    }
    let off_dt_struct = fdt_u32(fdt_add(fdt, 0x08)) as usize;
    let off_dt_strings = fdt_u32(fdt_add(fdt, 0x0C)) as usize;
    let size_dt_struct = fdt_u32(fdt_add(fdt, 0x24)) as usize;

    let mut ptr = fdt_add(fdt, off_dt_struct as isize);
    let end = fdt_add(fdt, (off_dt_struct + size_dt_struct) as isize);
    let strings = fdt_add(fdt, off_dt_strings as isize);

    let mut in_cpu = false;
    while ptr < end {
        let token = fdt_u32(ptr);
        ptr = fdt_add(ptr, 4);
        match token {
            FDT_BEGIN_NODE => {
                let mut name = [0u8; 32];
                let mut i = 0;
                while fdt_u8(ptr) != 0 && i < 32 {
                    name[i] = fdt_u8(ptr);
                    ptr = fdt_add(ptr, 1);
                    i += 1;
                }
                while fdt_u8(ptr) != 0 {
                    ptr = fdt_add(ptr, 1);
                }
                ptr = fdt_add(ptr, 1); // NUL
                // Align to 4
                let pad = (4 - (ptr as usize & 3)) & 3;
                ptr = fdt_add(ptr, pad as isize);
                let n = &name[..i];
                in_cpu = n.starts_with(b"cpu@") || n == b"cpu";
            }
            FDT_PROP => {
                let len = fdt_u32(ptr) as usize;
                let nameoff = fdt_u32(fdt_add(ptr, 4)) as usize;
                ptr = fdt_add(ptr, 8);
                let val = ptr;
                // Property block is 4-aligned already (len padded).
                let mut name = [0u8; 32];
                let mut i = 0;
                while i < 32 {
                    let c = fdt_u8(fdt_add(strings, (nameoff + i) as isize));
                    if c == 0 {
                        break;
                    }
                    name[i] = c;
                    i += 1;
                }
                let pname = &name[..i];
                if in_cpu && (pname == b"riscv,isa" || pname == b"isa") && len > 0 {
                    // Parse "rv{32,64}<letters>[_name...]"
                    let mut j = 0usize;
                    let v = val as *const u8;
                    // skip base "rv"
                    if j + 1 < len && fdt_u8(fdt_add(v, j as isize)) == b'r' && fdt_u8(fdt_add(v, (j + 1) as isize)) == b'v' {
                        j += 2;
                        // skip width digits
                        while j < len && fdt_u8(fdt_add(v, j as isize)).is_ascii_digit() {
                            j += 1;
                        }
                        // single-letter extensions until separator
                        while j < len {
                            let c = fdt_u8(fdt_add(v, j as isize));
                            if c == 0 || c == b'_' {
                                break;
                            }
                            if c == b'v' {
                                return true;
                            }
                            j += 1;
                        }
                    }
                }
                let pad = (4 - (len & 3)) & 3;
                ptr = fdt_add(ptr, (len + pad) as isize);
            }
            FDT_END_NODE => {
                in_cpu = false;
            }
            _ => {}
        }
    }
    false
}

/// Parse the initrd location from /chosen (QEMU `-initrd` sets these).
///
/// Returns `(start, end)` physical addresses from `linux,initrd-start` /
/// `linux,initrd-end`. Both properties are 1 or 2 big-endian u32 cells
/// (address-cells dependent); QEMU's generated DTB uses single u32 cells
/// on 32-bit RAM layouts and u64 (2 cells) otherwise — accept both.
///
/// Like `parse_memory_regions`, this is called on the EARLY identity
/// mapping, so `dtb_ptr` is a physical address here.
///
/// OH Phase 1: the OpenHarmony ramdisk (`-initrd ramdisk.img`, cmdline
/// `root=/dev/ram0`) lands through this path.
pub fn parse_initrd_region(dtb_ptr: u64) -> Option<(usize, usize)> {
    let (start, end) = parse_chosen_u64_pair(dtb_ptr, "linux,initrd-start", "linux,initrd-end")?;
    if end > start && start != 0 {
        Some((start as usize, end as usize))
    } else {
        None
    }
}

/// Read a pair of /chosen integer properties (used for the initrd range).
fn parse_chosen_u64_pair(
    dtb_ptr: u64,
    name_a: &str,
    name_b: &str,
) -> Option<(u64, u64)> {
    let a = parse_chosen_prop(dtb_ptr, name_a)?;
    let b = parse_chosen_prop(dtb_ptr, name_b)?;
    Some((a, b))
}

/// Read one /chosen integer property (1 or 2 big-endian u32 cells).
fn parse_chosen_prop(dtb_ptr: u64, name: &str) -> Option<u64> {
    let fdt = dtb_ptr as *const u8;

    let read_u32 = |offset: usize| -> u32 { fdt_u32(fdt_add(fdt, offset as isize)) };

    if read_u32(0) != 0xd00dfeed {
        return None;
    }
    let off_dt_struct = read_u32(0x08) as usize;
    let off_dt_strings = read_u32(0x0C) as usize;
    let size_dt_struct = read_u32(0x24) as usize;

    let mut ptr = fdt_add(fdt, off_dt_struct as isize);
    let end = fdt_add(fdt, (off_dt_struct + size_dt_struct) as isize);
    let strings = fdt_add(fdt, off_dt_strings as isize);

    let read_u32_at = |p: *const u8| -> u32 { fdt_u32(p) };

    let mut depth = 0;
    let mut in_chosen = false;

    while ptr < end {
        let token = fdt_u32(ptr);
        ptr = fdt_add(ptr, 4);

        match token {
            FDT_BEGIN_NODE => {
                let mut nodename = [0u8; 64];
                let mut i = 0;
                while fdt_u8(ptr) != 0 && i < 64 {
                    nodename[i] = fdt_u8(ptr);
                    ptr = fdt_add(ptr, 1);
                    i += 1;
                }
                ptr = fdt_add(ptr, 1);
                ptr = fdt_add(ptr, ((4 - ((ptr as usize) & 3)) & 3) as isize);

                let node_name = core::str::from_utf8(&nodename[..i]).ok()?;
                if node_name == "chosen" || node_name.starts_with("chosen@") {
                    in_chosen = true;
                }
                depth += 1;
            }
            FDT_END_NODE => {
                if in_chosen && depth == 1 {
                    in_chosen = false;
                }
                depth -= 1;
            }
            FDT_PROP => {
                let len = fdt_u32(ptr) as usize;
                let nameoff = fdt_u32(fdt_add(ptr, 4)) as usize;
                ptr = fdt_add(ptr, 8);

                let mut name_ptr = fdt_add(strings, nameoff as isize);
                let mut prop_name = [0u8; 32];
                let mut i = 0;
                while fdt_u8(name_ptr) != 0 && i < 32 {
                    prop_name[i] = fdt_u8(name_ptr);
                    name_ptr = fdt_add(name_ptr, 1);
                    i += 1;
                }
                let pname = core::str::from_utf8(&prop_name[..i]).ok()?;

                let mut result = None;
                if in_chosen && pname == name {
                    result = match len {
                        4 => Some(read_u32_at(ptr) as u64),
                        8 => Some(
                            ((read_u32_at(ptr) as u64) << 32)
                                | read_u32_at(fdt_add(ptr, 4)) as u64,
                        ),
                        _ => None,
                    };
                }

                ptr = fdt_add(ptr, len as isize);
                ptr = fdt_add(ptr, ((4 - ((ptr as usize) & 3)) & 3) as isize);

                if let Some(v) = result {
                    return Some(v);
                }
            }
            FDT_END => break,
            _ => break,
        }
    }
    None
}
