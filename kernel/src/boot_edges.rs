//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Boot-provenance framework edges (industrial-reduction lane L2).
//!
//! `main.rs`'s remaining unsafe was the boot-provenance family the
//! unsafe-slim round 4 documented as held: extern image symbols, the
//! kernel-map's `va_pa_offset` static, the boot memory-region table,
//! zero-page boot-parameter reads, and two panic-path diagnostic
//! walkers. This module is their consolidated home — one INV contract
//! each, in the shape of `kernel/src/safety/`. `main.rs` keeps exactly
//! one unsafe construct: the x86_64 boot stack-switch `asm!` (a
//! context-entry primitive, the Round-D floor).

/// Physical end of the kernel image (the `_end` linker symbol).
///
/// INV-BOOT-SYM-1: only the symbol's ADDRESS is read — the linker
/// provides the value; no memory at it is touched.
#[cfg(feature = "riscv64")]
pub fn image_end_va() -> usize {
    extern "C" {
        static _end: u8;
    }
    // SAFETY: INV-BOOT-SYM-1 — address-of a linker symbol.
    unsafe { core::ptr::addr_of!(_end) as usize }
}

/// Publish the kernel map's va→pa offset (single early-boot write).
///
/// INV-BOOT-SETUP-1: `rust_main` runs this exactly once, before any
/// CPU or task exists — the write races nothing.
pub fn set_va_pa_offset() {
    // SAFETY: INV-BOOT-SETUP-1 — single-threaded early boot.
    unsafe {
        crate::arch::mm::memory_layout::KERNEL_MAP.va_pa_offset = crate::arch::mm::VA_PA_OFFSET;
    }
}

/// The boot loader's initrd hint from the legacy zero-page (x86_64).
///
/// INV-BOOT-ZEROPAGE-1: the zero page (0x10000) is plain RAM written by
/// the ROM/bootloader before kernel entry; volatile reads, no writes.
#[cfg(feature = "x86_64")]
pub fn zeropage_initrd_hint() -> Option<(usize, usize)> {
    let bp = 0x10000usize as *const u8;
    // SAFETY: INV-BOOT-ZEROPAGE-1 — volatile u32 loads from the boot
    // parameter block.
    let image = unsafe { u32::from(core::ptr::read_volatile(bp.add(0x218) as *const u32)) } as usize;
    let size = unsafe { u32::from(core::ptr::read_volatile(bp.add(0x21C) as *const u32)) } as usize;
    if image != 0 && size != 0 {
        Some((image, size))
    } else {
        None
    }
}

// ---- boot memory-region table ----------------------------------------
// A `static mut` array filled once from the arch boot map before any
// reference escapes; the single-threaded early-boot discipline.

use crate::cmdline::MemoryRegion;

static mut REGIONS: [MemoryRegion; 64] = [MemoryRegion { base: 0, size: 0 }; 64];

/// Fill the boot region table from the arch map; returns the live slice.
///
/// INV-BOOT-REGIONS-1: called once during single-threaded early boot;
/// the array is written before any reference to it escapes; later
/// readers use [`regions`].
#[cfg(feature = "x86_64")]
pub fn regions_from_arch_boot() -> &'static [MemoryRegion] {
    let mut n = 0usize;
    for r in crate::arch::boot::boot_memory_regions() {
        if !r.usable || n >= 64 {
            continue;
        }
        // SAFETY: INV-BOOT-REGIONS-1 — single-threaded early boot; the
        // slot is written before any reference escapes.
        unsafe {
            REGIONS[n] = MemoryRegion {
                base: r.start as usize,
                size: (r.end - r.start) as usize,
            };
        }
        n += 1;
    }
    // SAFETY: INV-BOOT-REGIONS-1 — the first n entries were just
    // initialized and are never mutated again.
    unsafe { &REGIONS[..n] }
}

// ---- panic/boot diagnostic walkers ------------------------------------

/// The panic-path hex formatter over a static scratch buffer.
///
/// INV-BOOT-PANICBUF-1: the single-panicking-CPU path — the UART line
/// is emitted before any other CPU can reach this code (they are
/// stopped in the panic handler shortly after).
pub fn panic_hex(v: u64) -> &'static str {
    static mut BUF: [u8; 19] = [b'0'; 19];
    // SAFETY: INV-BOOT-PANICBUF-1 — single-panicking-CPU discipline.
    unsafe {
        BUF[0] = b'0';
        BUF[1] = b'x';
        for i in 0..16 {
            let nib = (v >> ((15 - i) * 4)) & 0xF;
            BUF[2 + i] = if nib < 10 { b'0' + nib as u8 } else { b'a' + (nib - 10) as u8 };
        }
        core::str::from_utf8_unchecked(&BUF)
    }
}

/// Lenient raw frame-pointer chain dump (16 ra slots, offline
/// symbolization).
///
/// INV-BOOT-RAWFRAMES-1: diagnostic read-only walk of the current
/// kernel stack; every slot is screened by `plausible_fp` (readable
/// kernel memory) before the volatile load; a stale slot terminates
/// the walk instead of faulting.
pub fn rawframes_line() {
    use crate::dfx::taskdump::taskdump_raw_line;
    // SAFETY: INV-BOOT-RAWFRAMES-1 — screened, volatile, kernel-stack
    // reads only.
    unsafe {
        let mut fp = crate::dfx::backtrace::current_frame_pointer();
        for _ in 0..16 {
            // plausible_fp: readable-kernel-memory only — a stale slot
            // holding a canonical-but-unmapped value (guard region) must
            // terminate the walk, not fault it.
            if !(fp >= 0x8000_0000_0000_0000
                && fp & 7 == 0
                && crate::dfx::memwatch::plausible_fp(fp))
            {
                break;
            }
            let ra = core::ptr::read_volatile((fp - 8) as *const u64);
            let next = core::ptr::read_volatile((fp - 0x10) as *const u64);
            taskdump_raw_line(b" ");
            let mut buf = [0u8; 16];
            for i in 0..16 {
                let nib = (ra >> ((15 - i) * 4)) & 0xF;
                buf[i] = if nib < 10 { b'0' + nib as u8 } else { b'a' + (nib - 10) as u8 };
            }
            taskdump_raw_line(&buf);
            if !(next & 7 == 0
                && next > fp
                && next < 0xffff_ffff_ffff_f000
                && crate::dfx::memwatch::plausible_fp(next))
            {
                break;
            }
            fp = next;
        }
    }
}
