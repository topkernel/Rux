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

// ---------------------------------------------------------------------------
// INV-BOOT-STACKSWITCH-1: the x86_64 boot stack switch (lib.rs's one
// unsafe construct — a context-entry primitive, the Round-D floor;
// moved here by the root-file unsafe collection, 2026-11-27).
// ---------------------------------------------------------------------------

/// Switch to the heap-backed boot kernel stack and continue the boot.
///
/// INV-BOOT-STACKSWITCH-1: runs exactly once, on the BSP, from
/// `rust_main` — before any other CPU or task exists, so nothing can
/// observe the mid-switch `rsp`. The stack is the caller's leaked 4 MiB
/// box (`core::mem::forget` right before the call) — machine lifetime,
/// no lifetime to uphold. The continuation is
/// `crate::boot_stack_continuation` (`-> !`), matching the `noreturn`
/// asm option: control never comes back to this frame.
#[cfg(feature = "x86_64")]
pub fn x86_boot_stack_switch(top: usize) -> ! {
    // SAFETY: INV-BOOT-STACKSWITCH-1 — one-shot BSP context entry onto
    // the machine-lifetime stack; the continuation never returns.
    unsafe {
        core::arch::asm!(
            "mov rsp, {0}",
            "jmp {1}",
            in(reg) top,
            sym crate::boot_stack_continuation,
            options(noreturn)
        );
    }
}

// ---------------------------------------------------------------------------
// INV-INIT-TASKSTORAGE-1: the boot init task's static storage (moved from
// init.rs by the root-file unsafe collection, 2026-11-27). The ONE Task
// that lives outside the kernel linear map — E8-REPAR names its address
// to the pointer screen.
// ---------------------------------------------------------------------------

/// Static storage of the init (PID 1) task — `MaybeUninit` to avoid
/// auto-initialization issues; built in place by
/// `init::create_and_start_init_process` through [`init_task_mut`].
pub static mut INIT_TASK_STORAGE: core::mem::MaybeUninit<crate::process::task::Task> =
    core::mem::MaybeUninit::uninit();

/// Address of the boot init task's static storage (E8-REPAR:
/// `Task::is_plausible_task_ptr` accepts exactly this address in
/// addition to the linear-map window, so the pointer screen needs no
/// unmapped image-range holes).
///
/// INV-INIT-TASKSTORAGE-1 (address arm): only the symbol's ADDRESS is
/// read — the storage may be uninitialized at call time; nothing is
/// loaded through the pointer.
pub fn init_task_addr() -> usize {
    // SAFETY: INV-INIT-TASKSTORAGE-1 (address arm) — address-of a
    // static; no memory at it is touched.
    unsafe { core::ptr::addr_of!(INIT_TASK_STORAGE) as usize }
}

/// The init task's storage as a raw pointer — the construction edge
/// (`Task::new_task_at` builds PID 1 in place).
///
/// INV-INIT-TASKSTORAGE-1 (pointer arm): only the pointer is taken; the
/// storage may be uninitialized until `new_task_at` runs on it.
pub fn init_task_ptr() -> *mut crate::process::task::Task {
    // SAFETY: INV-INIT-TASKSTORAGE-1 (pointer arm) — pointer-of a
    // static; no memory is touched through it here.
    unsafe { INIT_TASK_STORAGE.as_mut_ptr() }
}

/// The init task as a `&'static mut` — the single deref edge for the
/// boot-time setup of PID 1 (parent/sid/pgid, fdtable, signal/fs
/// structs, the execve loader borrow, the enqueue).
///
/// INV-INIT-TASKSTORAGE-1 (deref arm): called only on the single-thread
/// early-boot path (`init::create_and_start_init_process`), AFTER
/// `Task::new_task_at` has constructed the task in place and before it
/// is enqueued — no other CPU or task exists yet, so the writes race
/// nothing; every later reader reaches the task through the
/// scheduler/pid-hash handles the enqueue publishes.
#[allow(static_mut_refs)]
pub fn init_task_mut() -> &'static mut crate::process::task::Task {
    // SAFETY: INV-INIT-TASKSTORAGE-1 (deref arm) — post-construction,
    // single-threaded early boot, pre-publication.
    unsafe { INIT_TASK_STORAGE.assume_init_mut() }
}

// ---------------------------------------------------------------------------
// INV-FDT-1: the boot-time FDT byte-access family (moved from cmdline.rs
// by the root-file unsafe collection, 2026-11-27; also used by
// arch/aarch64/smp.rs through the cmdline re-export).
// ---------------------------------------------------------------------------

/// The ONE audited read edge for boot-time FDT byte accesses
/// (INV-FDT-1): boot.S maps the bootloader-provided DTB blob at its
/// physical address for the machine's lifetime; every parse fn anchors
/// the blob with the 0xd00dfeed magic check before walking, and the
/// walkers keep their cursors inside [fdt, fdt+totalsize).
#[inline]
pub(crate) fn fdt_u8(p: *const u8) -> u8 {
    // SAFETY: INV-FDT-1 — a 1-byte read inside the mapped DTB blob.
    unsafe { p.read() }
}

/// Cursor advance inside the mapped DTB (INV-FDT-1): a safe wrapper so
/// the walkers never hand-roll pointer arithmetic.
#[inline]
pub(crate) fn fdt_add(p: *const u8, n: isize) -> *const u8 {
    // SAFETY: INV-FDT-1 — arithmetic within the mapped DTB blob; the
    // walkers bound their loops by the header-derived `end`.
    unsafe { p.offset(n) }
}

/// Big-endian u32 read at a cursor inside the mapped DTB (INV-FDT-1).
#[inline]
pub(crate) fn fdt_u32(p: *const u8) -> u32 {
    // SAFETY: INV-FDT-1 — a 4-byte read inside the mapped DTB blob.
    unsafe { u32::from_be_bytes(core::ptr::read_unaligned(p as *const [u8; 4])) }
}

// ---------------------------------------------------------------------------
// INV-CMDLINE-STORAGE-1: the boot cmdline string's leaked storage (the
// Box::leak slice the boot parse publishes through CMDLINE_PTR/LEN).
// ---------------------------------------------------------------------------

/// Materialize the stored boot cmdline as a `&'static str`.
///
/// INV-CMDLINE-STORAGE-1: ptr/len were set together by the boot parse
/// (`Box::leak` of the bootargs string) — machine lifetime, never
/// freed, `len` exactly the string's byte length; the bytes are UTF-8
/// by construction (the parser slices the same string it stores).
pub fn cmdline_str(ptr: *const u8, len: usize) -> Option<&'static str> {
    // SAFETY: INV-CMDLINE-STORAGE-1 — machine-lifetime leaked slice,
    // exact len, UTF-8 bytes.
    unsafe {
        let slice = core::slice::from_raw_parts(ptr, len);
        core::str::from_utf8(slice).ok()
    }
}

// ---------------------------------------------------------------------------
// INV-CONSOLE-UART-1: the console's UART hardware byte edges (moved from
// console.rs by the root-file unsafe collection, 2026-11-27). The
// register FILE and the stall/probe logic stay in console.rs — only the
// raw per-arch byte moves cross here.
// ---------------------------------------------------------------------------

/// One byte to the ns16550a data register (riscv64 MMIO base).
///
/// INV-CONSOLE-UART-1: `base` is the mapped UART register base
/// (get_uart_base()); the data-register store is the 8250 TX protocol.
#[cfg(feature = "riscv64")]
pub fn uart_dr_store_riscv(base: usize, b: u8) {
    // SAFETY: INV-CONSOLE-UART-1 — MMIO store to the UART data register.
    unsafe {
        core::arch::asm!(
            "sb t1, 0(a0)",
            in("a0") base,
            in("t1") b,
            options(nostack, nomem)
        );
    }
}

/// One byte to the PL011 data register (aarch64 fixmap/identity VA).
///
/// INV-CONSOLE-UART-1: `addr` is uart_virt_addr() — the PL011 fixmap VA
/// once the early map is installed (mmu_init::init →
/// fixmap::init_uart_fixmap), the identity physical address before that
/// (boot.S idmap still live); both are valid data-register addresses.
#[cfg(feature = "aarch64")]
pub fn pl011_dr_store(addr: usize, b: u8) {
    // SAFETY: INV-CONSOLE-UART-1 — DR store through the fixmap/identity VA.
    unsafe {
        core::arch::asm!(
            "str w1, [x0]",
            in("x0") addr,
            in("w1") b as u32,
            options(nostack, nomem)
        );
    }
}

/// PL011 Flag Register read (`addr` is the FR offset — see the data_ready
/// caller's RX-empty bit test).
///
/// INV-CONSOLE-UART-1: pure MMIO flag read through the fixmap/identity
/// VA (same address contract as [`pl011_dr_store`]).
#[cfg(feature = "aarch64")]
pub fn pl011_fr_read(addr: usize) -> u32 {
    let fr: u32;
    // SAFETY: INV-CONSOLE-UART-1 — volatile FR read.
    unsafe {
        core::arch::asm!(
            "ldr {fr}, [{addr}]",
            fr = out(reg) fr,
            addr = in(reg) addr,
            options(nostack, readonly)
        );
    }
    fr
}

/// The COM1 transmit burst: string-out the chunk to the 8250 data port.
///
/// INV-CONSOLE-UART-1 (x86_64 arm): COM1 THR is the standard 8250 data
/// port; the string IO writes exactly chunk.len() bytes from chunk. DF
/// is cleared per the ABI before `rep` (belt: `cld`).
#[cfg(feature = "x86_64")]
pub fn com1_rep_outsb(port: u16, chunk: &[u8]) {
    // SAFETY: INV-CONSOLE-UART-1 — bounded rep-outsb to the data port.
    unsafe {
        core::arch::asm!(
            "cld",
            "rep outsb",
            in("dx") port,
            in("rcx") chunk.len() as u64,
            in("rsi") chunk.as_ptr(),
            lateout("rcx") _,
            lateout("rsi") _,
            options(nostack)
        );
    }
}
