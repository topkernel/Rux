//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The ROOT single files' one audited unsafe collection (mainline
//! minimal-first architecture, 2026-11-27). The crate-root single
//! files — lib.rs, init.rs, cmdline.rs, console.rs, cov.rs, initrd.rs,
//! list.rs, main.rs, errno.rs, and (final round) printk.rs, sbi.rs,
//! signal.rs — belong to no subsystem directory, so their unsafe
//! families collect here under this one audited file (the gate
//! whitelists exactly one `unsafe.rs` per directory plus this root
//! file). The file began as boot_edges.rs (the boot-provenance
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

// ---------------------------------------------------------------------------
// INV-COV-CAPTURE-1: the minicov profile-capture edge (moved from cov.rs,
// the cov-dump feature's one unsafe).
// ---------------------------------------------------------------------------

/// Capture the whole-program coverage profile into `buf`.
///
/// INV-COV-CAPTURE-1 (cov-dump builds only): `capture_coverage` walks
/// the `__llvm_prf_*` sections rustc emitted for THIS image; it is not
/// thread-safe, which is fine — the suite hook runs it once on the test
/// CPU after all other test work has completed. `buf` is plain heap
/// memory; the runtime writes bytes through the CoverageWriter impl and
/// never retains references.
#[cfg(feature = "cov-dump")]
pub fn capture_coverage(buf: &mut alloc::vec::Vec<u8>) -> Result<(), i32> {
    // SAFETY: INV-COV-CAPTURE-1 — single-CPU post-suite capture into a
    // plain heap buffer.
    unsafe { minicov::capture_coverage(buf).map_err(|_| -5) }
}

// ---------------------------------------------------------------------------
// INV-INITRD-1: the unpacked initrd image slice (moved from initrd.rs,
// the initrd lane's one unsafe).
// ---------------------------------------------------------------------------

/// The initrd image as a byte slice over the reserved RAM range.
///
/// INV-INITRD-1: [virt, virt+len) is the reserved initrd RAM the FDT
/// parse validated (end > start, inside RAM), read through the
/// permanent linear mapping (phys_to_virt of the reserved range); the
/// range is owned by the initrd for the whole boot and never written
/// through this slice.
pub fn initrd_raw_slice(virt: usize, len: usize) -> &'static [u8] {
    // SAFETY: INV-INITRD-1 — reserved, validated, machine-lifetime RAM.
    unsafe { core::slice::from_raw_parts(virt as *const u8, len) }
}

// ---------------------------------------------------------------------------
// INV-LIST-RAW-1: the intrusive-list raw primitives (moved from list.rs
// by the root-file unsafe collection, 2026-11-27; list.rs keeps the
// safe API surface — add/add_tail/del/entry/for_each/first_entry — and
// delegates here). Contracts are list.rs's INV-LIST-1 unchanged.
// ---------------------------------------------------------------------------

impl crate::list::ListHead {
    /// Raw `next` read off a list position.
    ///
    /// INV-LIST-RAW-1: `head` is an initialized ListHead — a live list
    /// position (sentinel or linked node); the links were written under
    /// the list-owning lock and are read under the same discipline.
    #[inline]
    pub fn raw_next(head: *const crate::list::ListHead) -> *mut crate::list::ListHead {
        // SAFETY: INV-LIST-RAW-1 — initialized, live list position.
        unsafe { (*head).next }
    }

    /// Raw `prev` read off a list position.
    ///
    /// INV-LIST-RAW-1: same contract as [`ListHead::raw_next`].
    #[inline]
    pub fn raw_prev(head: *const crate::list::ListHead) -> *mut crate::list::ListHead {
        // SAFETY: INV-LIST-RAW-1 — initialized, live list position.
        unsafe { (*head).prev }
    }

    /// Link `self` between `prev` and `next` (the one insertion
    /// primitive — add and add_tail both resolve to it).
    ///
    /// INV-LIST-RAW-1: `self` is a freshly constructed (unlinked) node;
    /// `prev`/`next` are adjacent live positions of the list the
    /// caller's lock owns; the four writes are the standard Linux
    /// __list_add bracket.
    #[inline]
    pub fn raw_link_between(&mut self, prev: *mut crate::list::ListHead, next: *mut crate::list::ListHead) {
        self.next = next;
        self.prev = prev;
        // SAFETY: INV-LIST-RAW-1 — live neighbors of the owning lock's
        // list; writes stay inside their link fields.
        unsafe {
            (*prev).next = self;
            (*next).prev = self;
        }
    }

    /// Unlink `self` from its two neighbors and mark it deleted
    /// (self-linked, the debugging convention).
    ///
    /// INV-LIST-RAW-1: `self` is currently linked; its neighbors are
    /// live nodes of the list the caller's lock owns.
    #[inline]
    pub fn raw_unlink(&mut self) {
        let next = self.next;
        let prev = self.prev;
        // SAFETY: INV-LIST-RAW-1 — live neighbors; the splice keeps the
        // list consistent (list_del shape).
        unsafe {
            (*next).prev = prev;
            (*prev).next = next;
        }
        // Mark as deleted (point to self, for debugging)
        self.next = self as *mut _;
        self.prev = self as *mut _;
    }
}

// ===========================================================================
// PRINTK (moved from printk.rs by the root-file unsafe collection,
// 2026-11-27: the lock-free ring slot edges and the global logger
// install). printk.rs keeps the ring struct, the slot protocol prose
// and every caller; only the raw accesses move cross here.
// ===========================================================================

/// The lock-free printk ring's raw slot-read edge.
///
/// INV-PRINTK-RING-1: `RING_RECORDS` is a fixed-size static array of
/// ring slots; `idx` arrives bounds-reduced (seq % RING_BUFFER_CAPACITY)
/// from the caller, so the subscript cannot go out of range. The
/// aliasing discipline is the documented slot protocol: a reader may
/// only trust the record's fields between an Acquire load of `pub_seq`
/// that matched its expected sequence and the reads it performs after
/// that load — the standard lock-free ring shape (the one documented
/// tear window is a writer stalling a full ring inside one preemption
/// window; readers clamp `text_len`, so a tear garbles, never faults).
#[inline]
pub(crate) fn ring_slot_get(ring: &crate::printk::RecordsArray, idx: usize) -> &'static crate::printk::LogRecord {
    // SAFETY: INV-PRINTK-RING-1 — in-bounds slot under the slot protocol.
    unsafe { &(*ring.0.get())[idx] }
}

/// The lock-free printk ring's raw slot-write edge (writer side).
///
/// INV-PRINTK-RING-1 (writer arm): the writer owns the slot between the
/// invalidation store and the publication store of its reserved
/// sequence — distinct sequences map to distinct slots, so at most one
/// writer fills a slot at a time (modulo the documented stall window).
#[inline]
pub(crate) fn ring_slot_get_mut(ring: &crate::printk::RecordsArray, idx: usize) -> &'static mut crate::printk::LogRecord {
    // SAFETY: INV-PRINTK-RING-1 (writer arm) — owned slot between
    // invalidate and publish.
    unsafe { &mut (*ring.0.get())[idx] }
}

/// `Sync` for the ring's `UnsafeCell` holder.
///
/// INV-PRINTK-RING-1 (Sync arm): sound because every access to the
/// cells goes through [`ring_slot_get`] / [`ring_slot_get_mut`], whose
/// slot protocol is the only authorized access shape.
unsafe impl Sync for crate::printk::RecordsArray {}

/// Install the global `log` facade logger (printk's init_logger).
///
/// INV-PRINTK-LOGGER-1: `log::set_logger` may succeed exactly once (the
/// facade enforces it internally); the only caller is
/// `printk::init_logger`, run once during early boot before any task
/// exists, so there is no second install to race.
pub fn set_global_logger(logger: &'static dyn log::Log) {
    // SAFETY: INV-PRINTK-LOGGER-1 — single early-boot install.
    unsafe {
        log::set_logger(logger).ok();
    }
}

// ===========================================================================
// SBI (moved from sbi.rs by the root-file unsafe collection, 2026-11-27:
// the two supervisor-mode ecalls — SRST system reset and IPI send —
// share one ecall edge).
// ===========================================================================

/// One SBI 0.2 ecall: returns (error, value) in the a0/a1 convention.
///
/// INV-SBI-ECALL-1: a supervisor-mode `ecall` into the machine-mode SBI
/// runtime, arguments per the SBI calling convention (a7 = extension
/// id, a6 = function id, a0/a1 arguments in and error/value out). The
/// call is synchronous — control returns to the instruction after the
/// ecall on the same hart — and `nomem` is sound because the SBI
/// contract passes everything in registers. Reachable at any point the
/// kernel runs (the SBI firmware is the platform, always present).
#[cfg(feature = "riscv64")]
pub fn sbi_ecall(eid: u64, fid: u64, a0: u64, a1: u64) -> (u64, u64) {
    let mut error: u64 = a0;
    let mut value: u64 = a1;
    // SAFETY: INV-SBI-ECALL-1 — register-only ecall into the SBI runtime.
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            inout("a0") error,
            inout("a1") value,
            options(nomem)
        );
    }
    (error, value)
}

// ===========================================================================
// SIGNAL (moved from signal.rs by the root-file unsafe collection,
// 2026-11-27: the UAPI handler type, the queue's Send/Sync declaration,
// the trap-frame edge, the POD frame template, the file-dentry slot
// read, and the trap.S entry symbol). The 15 sites fell into these
// five families plus six blocks whose callees were already safe fns
// (TaskBorrow::from_raw, ptrace::ptrace_stop, Task::address_space_arc)
// — those wrappers were simply dropped, not moved.
// ===========================================================================

/// Signal handler function type (the rt_sigaction sa_handler UAPI).
///
/// INV-SIG-HANDLER-1: the `unsafe` lives in the TYPE — calling through
/// it invokes user code, which only the signal-return trampoline does
/// (it erets to user mode; the kernel never calls a handler
/// directly). Kernel-side, a SigHandler is inert bits: constructed
/// from the user's sigaction words, copied to the user frame, never
/// dereferenced by kernel code.
pub type SigHandler = unsafe extern "C" fn(i32);

/// `Send`/`Sync` for the per-task signal queue.
///
/// INV-SIG-QUEUE-1: SigQueue wraps `Spinlock<VecDeque<SigInfo>>`; every
/// access is serialized by the spinlock, so sharing `&SigQueue` across
/// cores (Sync) and moving ownership across CPUs (Send — queues live
/// in tasks, tasks migrate) are sound.
unsafe impl Send for crate::signal::SigQueue {}
unsafe impl Sync for crate::signal::SigQueue {}

/// Convert the trap path's raw frame pointer into the delivery
/// reference (the one frame edge of do_signal and of the syscall
/// restart fixup).
///
/// INV-SIG-REGS-1: `regs` comes from the arch trap entry and is the
/// running task's live frame for the whole of the delivery call —
/// non-null by trap-path construction, owned by the CPU executing the
/// conversion, never accessed concurrently. Only do_signal /
/// restart_syscall_no_handler (the trap-exit path) call this.
pub fn live_pt_regs(regs: *mut crate::arch::pt_regs::PtRegs) -> &'static mut crate::arch::pt_regs::PtRegs {
    // SAFETY: INV-SIG-REGS-1 — the trap path's live, exclusive frame.
    unsafe { &mut *regs }
}

/// A zeroed SignalFrame template (the restore-path input buffer).
///
/// INV-SIG-FRAME-POD-1: SignalFrame is a repr(C) byte-image (ints,
/// byte arrays and POD SigInfo words only) — all-zero bytes are a valid
/// instance, so `zeroed` cannot produce an invalid value. The template
/// is immediately overwritten: copy_in from user memory (the handler's
/// edits win — M-04) or the kernel backup replaces it wholesale.
pub fn zeroed_signal_frame() -> crate::signal::SignalFrame {
    // SAFETY: INV-SIG-FRAME-POD-1 — POD byte-image, all-zero valid.
    unsafe { core::mem::zeroed() }
}

/// Read the File dentry slot (the diagnostic name-resolution edge).
///
/// INV-SIG-FILEDENTRY-1: the File's dentry slot is `UnsafeCell` because
/// it is installed post-construction (`set_dentry`) during open; after
/// installation it is never cleared nor re-pointed, so a reader sees
/// either None or a stable Arc. The caller reads it under the vma_read
/// lock of the abort-dump walker (a short read, no yield, the File is
/// kept alive by the vma's own Arc for the read's duration).
pub fn file_dentry_slot(file: &crate::fs::file::File) -> &'static Option<alloc::sync::Arc<crate::fs::dentry::Dentry>> {
    // SAFETY: INV-SIG-FILEDENTRY-1 — stable-after-install slot, short
    // read under the vma_read lock.
    unsafe { &*file.dentry.get() }
}

// ---------------------------------------------------------------------------
// Assembly-crossing entry symbol (moved from signal.rs by the root-file
// unsafe collection, 2026-11-27 — same rule as sched/unsafe.rs: a symbol
// exported to assembly IS a raw edge, and #![forbid(unsafe_code)] cannot
// coexist with #[no_mangle] in a file, so the export shim lives here and
// the body stays in signal.rs under the same Rust path).
// ---------------------------------------------------------------------------

/// Trap-exit signal delivery entry (trap.S calls this by symbol on
/// every return to user space).
///
/// INV-SIG-ENTRY-1: the shim only forwards the trap path's frame
/// pointer to [`crate::signal::check_and_deliver_signals`], which owns
/// the null check and the INV-SIG-REGS-1 frame edge.
#[no_mangle]
pub extern "C" fn check_and_deliver_signals(regs: *mut crate::arch::pt_regs::PtRegs) {
    crate::signal::check_and_deliver_signals(regs)
}
