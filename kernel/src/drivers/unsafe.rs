//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The drivers subsystem's ONE audited unsafe file (mainline
//! minimal-first consolidation, 2026-11-27). The drivers had no
//! pre-existing edges file; every raw edge they inline now crosses one
//! of the audited families below, and every OTHER file in the
//! subsystem is safe (file-level forbid).
//!
//! INV registry:
//! - INV-DRV-ALLOC  raw allocator + Box reclaim + raw read/ref/mut
//!   views + slice relabels + pointer arithmetic (callers own the
//!   lifetime contracts: init-time/machine-lifetime slots, sole-ref
//!   boxes, bounds-checked windows).
//! - INV-DRV-MMIO   volatile device-register reads/writes (identity-
//!   mapped or aliased device pages set up by setup_device_mappings;
//!   volatile because the device writes it, INV-DMA-3).
//! - INV-DRV-CSR    CSR/fence asm (sie masking, riscv fences, the PIT
//!   8254 port sequence, the aarch64 timer arm, the gicv3/apic
//!   register-file sequences, the x2APIC MSR edge INV-DRV-MSR).
//! - INV-DRV-RAW    pointer-derived views whose exclusivity comes from
//!   the caller's protocol (BLK/GPU lock, guard lifetimes, wait-queue
//!   current-task handles).
//! - INV-DRV-COPY   copy_nonoverlapping into syscall-validated or
//!   device-owned buffers (with-offset twin for page arithmetic).
//! - INV-DRV-BOX    Box::from_raw reclaims of sole-ref private-data
//!   boxes stored by devfs_open / the drm export path.
//! - INV-DRV-SLICE  from_raw_parts relabels (u8 image of validated
//!   buffers).
//! - INV-DRV-TLB    flush_tlb_all (fresh user PTEs before first use).
//! - INV-DRV-SBI    the sbi_rt::set_timer ecall.
//! - INV-DRV-MAP    reserved firmware-table region mapping.
//! - INV-DRV-TASKLET the input bottom-half tasklet init.

use core::arch::asm;
#[cfg(feature = "x86_64")]
use core::sync::atomic::Ordering;

// ===========================================================================
// INV-DRV-ALLOC: raw memory edges
// ===========================================================================

/// Allocate a `layout`-sized raw block (null checked by the caller).
#[inline]
pub fn raw_alloc(layout: core::alloc::Layout) -> *mut u8 {
    // SAFETY: INV-DRV-ALLOC — non-zero layout by contract; the null
    // check is the caller's first use.
    unsafe { alloc::alloc::alloc(layout) }
}

/// Allocate a zeroed `layout`-sized raw block (null checked by caller).
#[inline]
pub fn raw_alloc_zeroed(layout: core::alloc::Layout) -> *mut u8 {
    // SAFETY: INV-DRV-ALLOC — non-zero layout by contract.
    unsafe { alloc::alloc::alloc_zeroed(layout) }
}

/// Free a block allocated with EXACTLY `layout` while it is logically
/// dead (never submitted to the device, or the object's own Drop).
#[inline]
pub fn raw_dealloc(ptr: *mut u8, layout: core::alloc::Layout) {
    // SAFETY: INV-DRV-ALLOC — ptr came from raw_alloc[_zeroed] with
    // this exact layout; exclusive ownership at the call site.
    unsafe { alloc::alloc::dealloc(ptr, layout) }
}

/// Reclaim the sole-ref `Box<T>` a raw private-data slot stores
/// (INV-DRV-BOX): devfs_open's Box<DevNo>, the drm export path's
/// Box<Arc<GemObject>>.
#[inline]
pub fn box_take<T>(p: *mut T) -> alloc::boxed::Box<T> {
    // SAFETY: INV-DRV-BOX — the box was created by Box::into_raw at
    // open/export time and this is the single reclaim site.
    unsafe { alloc::boxed::Box::from_raw(p) }
}

/// Read the `T` value a raw slot points at (INV-DRV-RAW): the
/// Box<DevNo> file-lifetime slots (read-only deref).
#[inline]
pub fn raw_read<T: Copy>(p: *const T) -> T {
    // SAFETY: INV-DRV-RAW — p names a live, initialized T (the
    // file-lifetime box); the read is a plain bitwise load.
    unsafe { core::ptr::read(p) }
}

/// Shared view of a raw object (INV-DRV-RAW): GenDisk slots
/// (INV-BLKDEV-1/3 machine-lifetime boxes), the GPU device snapshot,
/// buffer-head data windows, the IoCompletion wait handoff.
#[inline]
pub fn raw_ref<'a, T: ?Sized>(p: *const T) -> &'a T {
    // SAFETY: INV-DRV-RAW — the caller's protocol proves the object is
    // live and un-mutated for the borrow's scope.
    unsafe { &*p }
}

/// Mutable view of a raw object (INV-DRV-RAW): virtqueue descriptor
/// slots (INV-DMA-2 disjoint slots + device lock), buffer-head data.
#[inline]
pub fn raw_mut<'a, T: ?Sized>(p: *mut T) -> &'a mut T {
    // SAFETY: INV-DRV-RAW — the caller's protocol (device lock, guard
    // lifetime) proves exclusive access for the borrow's scope.
    unsafe { &mut *p }
}

/// Byte-slice relabel of a validated buffer (INV-DRV-SLICE): the RX
/// buffer's linear map, the u32 image of a user slice.
#[inline]
pub fn slice_u8<'a>(p: *const u8, len: usize) -> &'a [u8] {
    // SAFETY: INV-DRV-SLICE — p..p+len is the caller-validated window.
    unsafe { core::slice::from_raw_parts(p, len) }
}

/// copy_nonoverlapping into a device-owned or user-validated block
/// (INV-DRV-COPY).
#[inline]
pub fn copy_bytes(src: *const u8, dst: *mut u8, n: usize) {
    // SAFETY: INV-DRV-COPY — dst..dst+n is in bounds (the carved
    // staging block / the caller's typed response buffer); src reads n
    // initialized bytes.
    unsafe { core::ptr::copy_nonoverlapping(src, dst, n) }
}

/// The with-offset twin (INV-DRV-COPY): page-window sources into
/// syscall buffer slices (ashmem's pool-page reads).
#[inline]
pub fn copy_bytes_offset(src: *const u8, src_off: usize, dst: *mut u8, dst_off: usize, n: usize) {
    // SAFETY: INV-DRV-COPY — src+src_off and dst+dst_off are the
    // validated windows; n bytes never overlap (kernel src, user dst).
    unsafe {
        core::ptr::copy_nonoverlapping(src.add(src_off), dst.add(dst_off), n);
    }
}

/// Raw pointer offset (INV-DRV-ALLOC arithmetic twin): the carve
/// edge's response half.
#[inline]
pub fn ptr_add<T>(p: *mut T, off: usize) -> *mut T {
    // SAFETY: INV-DRV-ALLOC — in bounds by the caller's layout math.
    unsafe { p.add(off) }
}

/// u32 element offset into a GEM backing store (INV-DRV-RAW: the
/// sample readers' clamped windows).
#[inline]
pub fn u32_offset(p: *const u32, idx: usize) -> *const u32 {
    // SAFETY: INV-DRV-RAW — idx < words (caller-clamped).
    unsafe { p.add(idx) }
}

// ===========================================================================
// INV-DRV-MMIO: volatile device-register edges
// ===========================================================================

#[inline]
pub fn volatile_read_u8(p: *const u8) -> u8 {
    // SAFETY: INV-DRV-MMIO — a live response byte / device page.
    unsafe { core::ptr::read_volatile(p) }
}

#[inline]
pub fn volatile_read_u16(p: *const u16) -> u16 {
    // SAFETY: INV-DRV-MMIO — a live used-ring field.
    unsafe { core::ptr::read_volatile(p) }
}

#[inline]
pub fn volatile_read_u32(p: *const u32) -> u32 {
    // SAFETY: INV-DRV-MMIO — a live used-ring entry field / APIC
    // window / RTC register.
    unsafe { core::ptr::read_volatile(p) }
}

/// Typed volatile read (INV-DRV-MMIO): the virtio-input event slots
/// (device-written DMA memory, INV-DMA-3).
#[inline]
pub fn volatile_read_t<T: Copy>(p: *const T) -> T {
    // SAFETY: INV-DRV-MMIO — a live registered DMA buffer slot.
    unsafe { core::ptr::read_volatile(p) }
}

/// u32 element read at `idx` (INV-DRV-MMIO): the GEM backing-store
/// sample readers.
#[inline]
pub fn volatile_read_u32_at(p: *const u32, idx: usize) -> u32 {
    // SAFETY: INV-DRV-MMIO — idx is caller-clamped into the object.
    unsafe { p.add(idx).read_volatile() }
}

#[inline]
pub fn mmio_write16(p: *mut u16, v: u16) {
    // SAFETY: INV-DRV-MMIO — the virtio-pci notify capability window.
    unsafe { core::ptr::write_volatile(p, v) }
}

#[inline]
pub fn mmio_write32(p: *mut u32, v: u32) {
    // SAFETY: INV-DRV-MMIO — the virtio-mmio QueueNotify register.
    unsafe { core::ptr::write_volatile(p, v) }
}

// ===========================================================================
// INV-DRV-CSR: register-file / fence asm edges
// ===========================================================================

/// The generic call-site return address (riscv `ra`; x86 [rbp+8] with
/// frame pointers forced on). Deadlock-diagnostics consumers only.
#[inline]
pub fn return_address() -> usize {
    let ra: usize;
    #[cfg(feature = "riscv64")]
    // SAFETY: pure register read, no memory access or side effects.
    unsafe {
        asm!("mv {}, ra", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(feature = "x86_64")]
    // SAFETY: frame pointers are forced on, so [rbp+8] is the return
    // address of this inlined function's caller.
    unsafe {
        asm!("mov {}, qword ptr [rbp + 8]", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    {
        ra = 0;
    }
    ra
}

/// MMIO write fence (riscv `fence w, o`): all previous writes are
/// visible before the notify store reaches the device.
#[inline]
pub fn fence_w_o() {
    #[cfg(feature = "riscv64")]
    // SAFETY: bare fence instruction, no operands.
    unsafe {
        asm!("fence w, o");
    }
}

/// The WRITE-side device fence (riscv `fence iorw, iorw`, Linux
/// __io_aw): the MMIO store must reach the device before any
/// subsequent access by this hart.
#[inline]
pub fn fence_iorw() {
    #[cfg(feature = "riscv64")]
    // SAFETY: bare fence instruction, no operands.
    unsafe {
        asm!("fence iorw, iorw");
    }
}

/// Full memory fence (riscv `fence`, the GPU cache-flush twin).
#[inline]
pub fn fence_full() {
    #[cfg(feature = "riscv64")]
    // SAFETY: bare fence instruction, no operands.
    unsafe {
        asm!("fence");
    }
    #[cfg(not(feature = "riscv64"))]
    {
        // strongly-ordered: the volatile MMIO write is the fence
    }
}

/// Read sie and clear SEIE (bit 9) via a register-based csrc (valid in
/// S-mode; the mask register is a scratch local).
#[cfg(feature = "riscv64")]
#[inline]
pub fn riscv_mask_seie() -> u64 {
    let sie: u64;
    // SAFETY: INV-DRV-CSR — S-mode sie access per the SBI contract.
    unsafe {
        asm!(
            "csrr {sie}, sie",
            "li {mask}, {seie}",
            "csrc sie, {mask}",
            sie = out(reg) sie,
            mask = out(reg) _,
            seie = const 1 << 9,
        );
    }
    sie
}

/// Restore the sie CSR saved by riscv_mask_seie.
#[cfg(feature = "riscv64")]
#[inline]
pub fn riscv_sie_restore(sie: u64) {
    // SAFETY: INV-DRV-CSR — S-mode sie access per the SBI contract.
    unsafe {
        asm!("csrw sie, {sie}", sie = in(reg) sie);
    }
}

#[cfg(feature = "x86_64")]
/// x2APIC/xAPIC MSR edge (INV-DRV-MSR): IA32_APIC_BASE, ICR, SVR.
#[inline]
pub fn msr_read(msr: u32) -> u64 {
    // SAFETY: INV-DRV-MSR — the apic driver gates first use on READY.
    unsafe { crate::arch::cpu::rdmsr(msr) }
}

#[cfg(feature = "x86_64")]
#[inline]
pub fn msr_write(msr: u32, v: u64) {
    // SAFETY: INV-DRV-MSR — the apic driver owns the programming order.
    unsafe { crate::arch::cpu::wrmsr(msr, v) }
}

/// The 8254 PIT channel-0 programming sequence (ports 0x40/0x43 are
/// the fixed data and mode/command registers).
#[cfg(feature = "x86_64")]
pub fn pit_program_ch0(divisor: u16) {
    use crate::arch::cpu::io_wait;
    use crate::drivers::timer::x86_64::{PIT_CH0_DATA, PIT_CH0_MODE3_BIN, PIT_MODE_CMD};
    // SAFETY: INV-DRV-CSR — the standard 8254 channel-0 square-wave
    // programming sequence.
    unsafe {
        crate::arch::cpu::outb(PIT_MODE_CMD, PIT_CH0_MODE3_BIN);
        io_wait();
        crate::arch::cpu::outb(PIT_CH0_DATA, (divisor & 0xff) as u8);
        io_wait();
        crate::arch::cpu::outb(PIT_CH0_DATA, (divisor >> 8) as u8);
    }
}

// ===========================================================================
// INV-DRV-TLB / INV-DRV-SBI / INV-DRV-MAP / INV-DRV-TASKLET
// ===========================================================================

/// Global TLB flush (fresh user PTEs need a flush before first use on
/// any hart).
#[inline]
pub fn flush_tlb_all() {
    // SAFETY: INV-DRV-TLB — arch shootdown; correctness contract is the
    // caller's (fresh PTEs not yet used).
    unsafe { crate::arch::mm::asid::flush_tlb_all() }
}

#[cfg(feature = "riscv64")]
/// SBI set_timer ecall (per-hart; `value` is an absolute deadline).
#[inline]
pub fn sbi_set_timer(value: u64) {
    // SAFETY: INV-DRV-SBI — the ecall has no special preconditions.
    unsafe { sbi_rt::set_timer(value); }
}

/// Map a reserved firmware-table region into the kernel linear map.
pub fn map_reserved_region(virt: u64, phys: u64, size: u64, flags: u64) {
    // SAFETY: INV-DRV-MAP — kernel-only linear-map VAs; the regions are
    // reserved (never handed to the page allocator).
    unsafe {
        crate::arch::mm::mmu_init::map_kernel_region(virt, phys, size, flags);
    }
}

/// Initialize the input bottom-half tasklet (boot-time, single-threaded
/// kernel-init context).
pub fn input_bh_tasklet_init(func: fn(*mut crate::interrupt::tasklet::TaskletStruct)) {
    // SAFETY: INV-DRV-TASKLET — called once during kernel init before
    // any device IRQ is registered.
    unsafe {
        crate::drivers::input::INPUT_BH_TASKLET.init(func);
    }
}

// ===========================================================================
// Moved whole from their home files (mainline consolidation,
// 2026-11-27): the io_buf pool family, the I/O-completion delivery
// family, and the arch device-programming sequences. Each carries its
// original contract comments.
// ===========================================================================


// ============================================================================
// SUBMIT-2X: pooled io_bufs (per-CPU 64-byte free lists)
//
// Every virtio-blk chain allocates one 64-byte header+resp block (the
// R17-C combined allocation) at submit and frees it at completion. At
// depth-1 4 KiB direct-I/O rates that is two global-allocator lock
// round trips per I/O (alloc on the submitting CPU, dealloc on the
// completion CPU) plus the allocator's size-class search — measured as
// a named residual on the dio-conc open list ("64 B io_buf alloc/free
// per request"). The pool below keeps one free list per CPU behind an
// uncontended IRQ-save spinlock (the walker frees in hardirq on the
// COMPLETION CPU, so plain per-CPU data is not enough); overflow
// drains to the heap, underflow refills from it.
//
// Lifetime shape is unchanged from the heap free this replaces: a
// buffer enters the pool only after its chain's used-ring entry was
// consumed by the walker (device done DMA-ing the response byte) or
// after a submit error path that built no chain. The timeout/abandon
// path keeps its deliberate leak (R21-N2) by simply not returning the
// buffer. Buffers parked in the pool carry the intrusive next pointer
// in their first 8 bytes; alloc re-initializes the header before any
// chain sees it.
// ============================================================================

/// Size/align of the pooled io_buf (the R17-C header+resp block).
const IO_BUF_LAYOUT: alloc::alloc::Layout = unsafe {
    // SAFETY: 64 with align 16 is a valid layout (const-stable unwrap on
    // an infallible constructor).
    alloc::alloc::Layout::from_size_align_unchecked(64, 16)
};

pub(crate) fn io_buf_alloc() -> *mut u8 {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::drivers::virtio::IO_BUF_POOLS.len() {
        let mut pl = crate::drivers::virtio::IO_BUF_POOLS[cpu].lock_irqsave();
        if pl.head != 0 {
            // SAFETY: parked buffers were allocated with IO_BUF_LAYOUT;
            // the first 8 bytes hold the intrusive next pointer.
            let p = pl.head as *mut u8;
            pl.head = unsafe { *(p as *mut usize) };
            pl.count -= 1;
            return p;
        }
    }
    // SAFETY: IO_BUF_LAYOUT is a non-zero-size valid layout.
    unsafe { alloc::alloc::alloc(IO_BUF_LAYOUT) }
}
pub(crate) fn io_buf_free(p: *mut u8) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::drivers::virtio::IO_BUF_POOLS.len() {
        let mut pl = crate::drivers::virtio::IO_BUF_POOLS[cpu].lock_irqsave();
        if pl.count < crate::drivers::virtio::IO_BUF_POOL_CAP {
            // SAFETY: p is a live 64-byte, 16-aligned block; parking the
            // next pointer in its first 8 bytes is safe because every
            // consumer re-initializes the header before use.
            unsafe { *(p as *mut usize) = pl.head };
            pl.head = p as usize;
            pl.count += 1;
            return;
        }
    }
    // SAFETY: p was allocated with IO_BUF_LAYOUT.
    unsafe { alloc::alloc::dealloc(p, IO_BUF_LAYOUT) };
}
pub(crate) fn io_buf_release(ptr: *mut u8, layout: alloc::alloc::Layout) {
    if ptr.is_null() {
        return; // tombstone shape: nothing was ever allocated
    }
    if layout.size() == IO_BUF_LAYOUT.size() && layout.align() == IO_BUF_LAYOUT.align() {
        io_buf_free(ptr);
    } else {
        // SAFETY: dealloc with the exact layout the submitter allocated
        // with (non-pool shape).
        unsafe { alloc::alloc::dealloc(ptr, layout) };
    }
}
pub(crate) fn begin_delivery(completion: *mut crate::fs::io_completion::IoCompletion) {
    // SAFETY: the count protocol keeps the IoCompletion alive; callers
    // hold the pending-table lock that serializes the handshake.
    unsafe { (*completion).begin_delivery(); }
}
pub(crate) fn deliver_completion(
    completion: *mut crate::fs::io_completion::IoCompletion,
    status: i32,
    token: usize,
) {
    // SAFETY: see doc — count protocol + retire order.
    unsafe {
        (*completion).complete_with_token(status, token);
        (*completion).end_delivery();
    }
}
/// Capture the caller's return address (the virtio acquire site) for
/// holder forensics — same technique as spinlock.rs's
/// caller_return_address().
#[inline]
pub(crate) fn pci_blk_acquire_site_raw() -> usize {
    let ra: usize;
    #[cfg(feature = "riscv64")]
    // SAFETY: pure register read, no memory access or side effects.
    unsafe {
        core::arch::asm!("mv {}, ra", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(feature = "x86_64")]
    // SAFETY: frame pointers are forced on, so [rbp+8] is this inlined
    // helper's caller's return address.
    unsafe {
        core::arch::asm!("mov {}, qword ptr [rbp + 8]", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    {
        ra = 0;
    }
    ra
}
// ---------------------------------------------------------------------------
// Register access
// ---------------------------------------------------------------------------

#[cfg(feature = "x86_64")]
/// Read a LAPIC register in whichever mode is active.
#[inline]
pub fn apic_reg_read(reg_xapic: u32, msr_x2apic: u32) -> u32 {
    if crate::drivers::intc::apic::X2APIC.load(Ordering::Acquire) {
        // SAFETY: x2APIC MSR read; READY gates first use.
        unsafe { crate::arch::cpu::rdmsr(msr_x2apic) as u32 }
    } else {
        // Linux fixmap discipline: the LAPIC window is accessed through the
        // kernel-half device alias (shared into every mm), never through a
        // low identity VA — trap-context EOIs run under user CR3s too.
        let va = crate::arch::mm::memory_layout::device_alias(
            crate::arch::mm::memory_layout::LAPIC_BASE as u64,
        ) as usize + reg_xapic as usize;
        // SAFETY: MMIO read of the local APIC window (kernel-half alias,
        // mapped once at setup_device_mappings time).
        unsafe { core::ptr::read_volatile(va as *const u32) }
    }
}


#[cfg(feature = "x86_64")]
/// Write a LAPIC register in whichever mode is active.
#[inline]
pub fn apic_reg_write(reg_xapic: u32, msr_x2apic: u32, value: u32) {
    if crate::drivers::intc::apic::X2APIC.load(Ordering::Acquire) {
        // SAFETY: x2APIC MSR write; READY gates first use.
        unsafe { crate::arch::cpu::wrmsr(msr_x2apic, value as u64) };
    } else {
        let va = crate::arch::mm::memory_layout::device_alias(
            crate::arch::mm::memory_layout::LAPIC_BASE as u64,
        ) as usize + reg_xapic as usize;
        // SAFETY: MMIO write to the local APIC window (kernel-half alias).
        unsafe { core::ptr::write_volatile(va as *mut u32, value) };
    }
}

    // IRREDUCIBLE F-ROW (F-HVCALL-X1): the KVM hypercall ABI
    // (vmmcall/vmcall with the RBX-shuttle push/mov/pop bracket) —
    // reference kernel arch/x86/kernel/kvm.c __send_ipi_mask's apic_hc_raw()
    // twin; a register protocol with no memory operands has no safe
    // shape. The bitmap is a single-bit cluster with min=dest
    // (bitmap_low=1, bitmap_high=0).
    #[cfg(feature = "x86_64")]
pub fn hc(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, vmmcall: bool) -> i64 {
        let ret: i64;
        // SAFETY: INV-DRV-CSR — register-only hypercall protocol, no
        // memory operands (see the contract note above).
        // The KVM ABI passes arg1 in RBX, which LLVM reserves — shuttle
        // it through any free GPR with an explicit push/mov/pop bracket
        // (stack-neutral; that is why nostack is not claimed).
        if vmmcall {
            unsafe {
            core::arch::asm!(
                "push rbx",
                "mov rbx, {b}",
                "vmmcall",
                "pop rbx",
                b = in(reg) a1,
                in("rax") nr, in("rcx") a2, in("rdx") a3, in("rsi") a4,
                lateout("rax") ret,
            );
            }
        } else {
            unsafe {
            core::arch::asm!(
                "push rbx",
                "mov rbx, {b}",
                "vmcall",
                "pop rbx",
                b = in(reg) a1,
                in("rax") nr, in("rcx") a2, in("rdx") a3, in("rsi") a4,
                lateout("rax") ret,
            );
            }
        }
        ret
    }
#[cfg(feature = "x86_64")]
#[inline(always)]
pub fn tick_arm_ra_raw() -> u64 {
    #[cfg(feature = "x86_64")]
    {
        let ra: u64;
        // SAFETY: pure register-relative read, no side effects.
        unsafe {
            core::arch::asm!(
                "mov {}, qword ptr [rbp + 8]",
                out(reg) ra,
                options(nomem, nostack, preserves_flags)
            );
        }
        ra
    }
    #[cfg(not(feature = "x86_64"))]
    {
        0
    }
}
// ==== recovered from kernel/src/drivers/intc/gicv3.rs ====
// ==================== ICC_* system registers ====================

#[cfg(feature = "aarch64")]
/// Read ICC_IAR1_EL1 — acknowledge the highest-priority pending
/// Group-1 interrupt (priority drop). Returns the INTID.
#[inline]
pub fn gicv3_iar1_read() -> u32 {
    let v: u64;
    // SAFETY: ICC_IAR1_EL1 read is the GIC acknowledge; side effect is
    // the priority drop for the returned INTID.
    unsafe { asm!("mrs {}, icc_iar1_el1", out(reg) v, options(nomem, nostack)) };
    v as u32 & 0xFF_FFFF
}


#[cfg(feature = "aarch64")]
/// Write ICC_EOIR1_EL1 — end of interrupt (priority drop completion +
/// deactivation; ICC_CTLR_EL1.EOImode stays 0 = single-write EOI).
#[inline]
pub fn gicv3_eoir1_write(intid: u32) {
    // SAFETY: EOIR1 write deactivates the previously acknowledged INTID.
    unsafe {
        asm!("msr icc_eoir1_el1, {}",
             in(reg) intid as u64, options(nomem, nostack))
    };
}


#[cfg(feature = "aarch64")]
/// Generate an SGI (ICC_SGI1R_EL1) to the target list.
///
/// GICv3 field layout (unlike the GICv2 GICD_SGIR encoding this driver
/// originally used): INTID [27:24], TargetList [15:0], Aff1 [23:16],
/// IRM [40], Aff2 [39:32], Aff3 [55:48]. QEMU virt keeps every cpu in
/// one cluster (Aff1/2/3 = 0), so a TargetList bit for Aff0 selects the
/// cpu. The old (1<<aff0)<<8 | intid encoding put the target bit at
/// [15:8]+n and the INTID at [3:0]; the emulated GIC decoded a
/// TargetList of 0x100 (no cpu has Aff0=8) and silently dropped the SGI
/// — the IPI facility never fired.
#[inline]
pub fn gicv3_send_sgi(target_aff0: u8, intid: u32) {
    let sgi = (((intid as u64) & 0xF) << 24) | (1u64 << (target_aff0 & 0xF));
    // SAFETY: SGI1R write raises the SGI on the target core(s).
    unsafe {
        asm!("dsb st",
             "msr icc_sgi1r_el1, {}",
             in(reg) sgi, options(nomem, nostack))
    };
}
#[cfg(feature = "aarch64")]
/// Enable the CPU interface system registers (ICC_*): SRE, PMR, group-1
/// enable. Called per CPU from the boot path (trap::enable_external_interrupt
/// / secondary_cpu_entry). Idempotent.
// IRREDUCIBLE F-ROW (F-GIC-A1): ICC_* system-register programming —
// reference kernel drivers/irqchip/irq-gic-v3.c gic_cpu_init; the
// GICv3 CPU-interface enable sequence is register-file access with no
// safe shape (one audited block per sequence).
pub fn gicv3_enable_cpu_interface() {
    // SAFETY: ICC_* system-register programming below is the standard
    // GICv3 CPU-interface enable sequence for Group 1 at EL1.
    unsafe {
        // 1. System-register access (QEMU fixes SRE=1; set defensively).
        let sre: u64;
        asm!("mrs {}, icc_sre_el1", out(reg) sre, options(nomem, nostack));
        if sre & 1 == 0 {
            asm!("msr icc_sre_el1, {}", in(reg) 1u64, options(nomem, nostack));
            asm!("isb", options(nomem, nostack));
        }

        // 2. Per-CPU redistributor: locate + configure SGI/PPI bank.
        if let Some(rd) = crate::drivers::intc::gicv3::find_my_rd_base() {
            crate::drivers::intc::gicv3::redist_configure_local(rd);
        }

        // 3. Unmask priorities and forward Group 1 to EL1.
        asm!("msr icc_pmr_el1, {}", in(reg) 0xFFu64, options(nomem, nostack));
        asm!("msr icc_igrpen1_el1, {}", in(reg) 1u64, options(nomem, nostack));
        asm!("isb", options(nomem, nostack));
    }
}
#[cfg(feature = "riscv64")]
// ==== recovered from kernel/src/drivers/intc/clint.rs ====
/// Read system time (time CSR)
///
/// Uses RISC-V `rdtime` instruction to read time
///
/// # Returns
/// Current time (cycles)
pub fn clint_read_time() -> u64 {
    // SAFETY: rdtime is a read-only CSR available in S-mode on RISC-V.
    unsafe {
        let time: u64;
        core::arch::asm!(
            "rdtime {}",
            out(reg) time,
            options(nostack, readonly)
        );
        time
    }
}
#[cfg(feature = "riscv64")]
// ==== recovered from kernel/src/drivers/timer/riscv64.rs ====
/// Set timer using stimecmp CSR (sstc extension)
///
/// This is more direct than SBI and avoids potential issues
#[inline]
pub fn riscv_stimecmp_set(deadline: u64) {
    unsafe {
        asm!(
            "csrw stimecmp, {0}",
            in(reg) deadline,
            options(nomem, nostack)
        );
    }
}
#[cfg(feature = "aarch64")]
// ==== recovered from kernel/src/drivers/timer/aarch64.rs ====
/// Arm the EL1 virtual timer to an absolute CNTVCT deadline
#[inline]
pub fn aarch64_timer_arm(deadline: u64) {
    // SAFETY: writing the virtual-timer compare/enable registers arms the
    // per-cpu timer; the only side effect is the future PPI interrupt.
    unsafe {
        core::arch::asm!(
            "msr cntv_cval_el0, {d}",
            "msr cntv_ctl_el0, {e}",
            d = in(reg) deadline,
            e = in(reg) 1u64, // enable, unmasked
            options(nomem, nostack)
        );
    }
}
// ==== recovered from kernel/src/drivers/rtc.rs ====
/// Read one 32-bit device register.
///
/// # Safety (internal)
/// The page at GOLDFISH_RTC_BASE is identity-mapped with device flags by
/// `setup_device_mappings()`; this helper is only reachable after that.
#[inline]
pub fn rtc_read_reg(offset: u64) -> u32 {
    let va = crate::drivers::rtc::GOLDFISH_RTC_VA.load(core::sync::atomic::Ordering::Relaxed);
    unsafe { core::ptr::read_volatile((va + offset) as *const u32) }
}

/// Split one pooled 64-byte io_buf into its typed halves and
/// initialize them (INV-SAFECALL-1: header at offset 0, response at
/// +48 — the R17-C combined block; THE one audited carve edge for the
/// pool shape; `io_buf` must come from `io_buf_alloc`).
pub(crate) fn io_buf_init(
    io_buf: *mut u8,
    type_: u32,
    sector: u64,
) -> (*mut crate::drivers::virtio::queue::VirtIOBlkReqHeader, *mut crate::drivers::virtio::queue::VirtIOBlkResp) {
    // SAFETY: io_buf is non-null (callers check), 64 bytes, 16-aligned;
    // +48 leaves 16 bytes for the response — both in bounds. The
    // initializations follow immediately.
    unsafe {
        let header_ptr = io_buf as *mut crate::drivers::virtio::queue::VirtIOBlkReqHeader;
        let resp_ptr = io_buf.add(48) as *mut crate::drivers::virtio::queue::VirtIOBlkResp;
        *header_ptr = crate::drivers::virtio::queue::VirtIOBlkReqHeader {
            type_,
            reserved: 0,
            sector,
        };
        (*resp_ptr).status = 0xFF;
        (header_ptr, resp_ptr)
    }
}

// ===========================================================================
// Send/Sync commitments (moved verbatim from their home files by the
// 2026-11-27 mainline consolidation; each carries its original SAFETY
// note where it had one).
// ===========================================================================

// ==== recovered from kernel/src/drivers/blkdev/mod.rs ====
unsafe impl Send for crate::drivers::blkdev::GenDisk {}
unsafe impl Sync for crate::drivers::blkdev::GenDisk {}
unsafe impl Send for crate::drivers::blkdev::BlockDeviceManager {}
unsafe impl Sync for crate::drivers::blkdev::BlockDeviceManager {}
// ==== recovered from kernel/src/drivers/drm/mod.rs ====
// SAFETY: the object's mutable state is a single AtomicU32; kva/phys are
// stable for the object's lifetime.
unsafe impl Send for crate::drivers::drm::GemObject {}
unsafe impl Sync for crate::drivers::drm::GemObject {}
// ==== recovered from kernel/src/drivers/gpu/framebuffer.rs ====
unsafe impl Send for crate::drivers::gpu::framebuffer::FrameBuffer {}
unsafe impl Sync for crate::drivers::gpu::framebuffer::FrameBuffer {}
// ==== recovered from kernel/src/drivers/gpu/virtio_gpu.rs ====
unsafe impl Send for crate::drivers::gpu::virtio_gpu::VirtioGpuDevice {}
unsafe impl Sync for crate::drivers::gpu::virtio_gpu::VirtioGpuDevice {}
// ==== recovered from kernel/src/drivers/input/virtio_input.rs ====
unsafe impl Send for crate::drivers::input::virtio_input::VirtioInputDevice {}
unsafe impl Sync for crate::drivers::input::virtio_input::VirtioInputDevice {}
// ==== recovered from kernel/src/drivers/net/space.rs ====
unsafe impl Send for crate::drivers::net::space::NetDevice {}
// SAFETY (round-5): NetDevice's cross-thread state (stats, flags) is
// mutated under the NetRx softirq / syscall discipline each op
// documents; the priv_ raw slot is driver-owned and set before
// registration. Sync was implicit in the old `static mut` usage.
unsafe impl Sync for crate::drivers::net::space::NetDevice {}
// ==== recovered from kernel/src/drivers/net/virtio_net.rs ====
// SAFETY: All shared state is protected by Spinlocks (irqsave where needed),
// ensuring no data races across threads/CPUs.
unsafe impl Send for crate::drivers::net::virtio_net::VirtIONetDevice {}
// ==== recovered from kernel/src/drivers/virtio/mod.rs ====
// SAFETY: VirtIOBlkDevice is only accessed from kernel context; internal Spinlocks
// serialize all mutable access to shared fields.
unsafe impl Send for crate::drivers::virtio::VirtIOBlkDevice {}
// SAFETY: All shared state is protected by Spinlocks (irqsafe where needed),
// ensuring no data races across threads/CPUs.
unsafe impl Sync for crate::drivers::virtio::VirtIOBlkDevice {}
// SAFETY: PendingIo is stored in a Spinlock-protected table and only accessed
// from IRQ/softirq context; raw pointers within are not shared across threads.
unsafe impl Send for crate::drivers::virtio::PendingIo {}
// ==== recovered from kernel/src/drivers/virtio/queue.rs ====
unsafe impl Send for crate::drivers::virtio::queue::VirtQueue {}
unsafe impl Sync for crate::drivers::virtio::queue::VirtQueue {}
