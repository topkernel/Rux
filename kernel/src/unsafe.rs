//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The ROOT single files' one audited unsafe collection (mainline
//! minimal-first architecture, 2026-11-27). The crate-root single
//! files — lib.rs, init.rs, cmdline.rs, console.rs, cov.rs, initrd.rs,
//! list.rs, main.rs, errno.rs — belong to no subsystem directory, so
//! their unsafe families collect here under this one audited file (the
//! gate whitelists exactly one `unsafe.rs` per directory plus this
//! root file). The file began as boot_edges.rs (the boot-provenance
//! framework edges, industrial-reduction lane L2) and was git-mv'd to
//! the per-directory `unsafe.rs` naming; the module keeps its
//! historical `boot_edges` identifier so every call site stays stable.
//!
//! Every family carries one INV contract each, in the shape of
//! `kernel/src/safety/`.

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


// ===========================================================================
// The machine-reset repertoire (moved from syscall/edges.rs, crate-flip
// step 3 2026-11-21: arch-shaped code — the 8042/triple-fault/SBI reset
// sequences belong with the boot provenance edges, not the syscall
// service). Contracts unchanged (INV-SYSC-4); callers are the process
// reboot cascade (process/reboot.rs) and the panic path.
// ===========================================================================

#[cfg(feature = "x86_64")]
pub fn x86_panic_reset() {
    // SAFETY: INV-SYSC-4 — bare port I/O on the reset port and a
    // deliberate faulting instruction; neither returns.
    unsafe {
        // Drain the 8042 input buffer first (writing the command
        // while it is full is dropped on real hardware).
        for _ in 0..100_000 {
            if crate::arch::cpu::inb(0x64) & 0x02 == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        crate::arch::cpu::outb(0x64, 0xFE);
        // Give the pulse time to land.
        for _ in 0..10_000_000 {
            core::hint::spin_loop();
        }
        // Triple fault: load a null/limit-0 IDT and int3 — the
        // vector lookup must fail, the double fault must fail the
        // same way, and the CPU (QEMU included) resets.
        static NULL_IDT: [u64; 2] = [0; 2];
        core::arch::asm!(
            "lidt [{}]",
            in(reg) &NULL_IDT,
            options(nomem, nostack)
        );
        core::arch::asm!("int3", options(nomem, nostack));
    }
}

#[cfg(feature = "x86_64")]
pub fn x86_reboot_pulse() {
    // SAFETY: INV-SYSC-4 — the two reset-port writes; reached only
    // behind the CAP_SYS_BOOT gate in sys_reboot.
    unsafe {
        crate::arch::cpu::outb(0x64, 0xFE);
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
        crate::arch::cpu::outb(0xcf9, 0x0E);
    }
}

#[cfg(feature = "riscv64")]
pub fn sbi_shutdown_ecall() {
    // SAFETY: INV-SYSC-4 — privileged SBI ecall; reached only after the
    // CAP_SYS_BOOT gate in sys_reboot (or the panic fallback).
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") 0x8u64,
            out("a0") _,
            out("a1") _,
            options(nomem)
        );
    }
}
