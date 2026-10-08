//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 Local APIC driver (SMP): ACPI MADT discovery, xAPIC/x2APIC
//! register access, IPI send, and the per-CPU LAPIC timer.
//!
//! ## Mode selection
//!
//! x2APIC is used when CPUID leaf 1 ECX bit 21 reports it; the mode is
//! switched on every CPU by setting IA32_APIC_BASE[10] (EXTD) before any
//! LAPIC register access.  Without x2APIC the MMIO window at 0xFEE00000
//! (identity-mapped by `setup_device_mappings`) is used.
//!
//! ## External interrupts stay on the 8259 (virtual wire)
//!
//! The IOAPIC migration is a separate phase; after this driver enables
//! the local APIC, LVT LINT0 is programmed to ExtINT so the 8259 PIC
//! lines keep flowing through the local APIC.  Every EOI in the trap
//! path must therefore call [`eoi`] in addition to `pic_send_eoi` (a
//! local APIC left in-service wedges all further interrupt delivery).
//!
//! ## Timer
//!
//! Each CPU runs a periodic LAPIC timer at KERNEL_HZ (vector
//! `LOCAL_TIMER_VECTOR`).  The tick rate is calibrated once on the BSP
//! against PIT channel 2 (the speaker-counter one-shot window), which
//! also yields the TSC frequency for the timekeeping driver
//! (`drivers::timer`).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::arch::cpu::{inb, outb, rdmsr, wrmsr};

// ---------------------------------------------------------------------------
// Register map
// ---------------------------------------------------------------------------

/// IA32_APIC_BASE MSR
const MSR_IA32_APIC_BASE: u32 = 0x1B;
/// APIC global-enable bit (bit 11 of IA32_APIC_BASE)
const APIC_BASE_ENABLE: u64 = 1 << 11;
/// x2APIC mode bit (bit 10 of IA32_APIC_BASE)
const APIC_BASE_X2APIC: u64 = 1 << 10;

// xAPIC MMIO register offsets (from LAPIC_BASE)
const XAPIC_ID: u32 = 0x020;
const XAPIC_SVR: u32 = 0x0F0;
const XAPIC_EOI: u32 = 0x0B0;
const XAPIC_ICR_LO: u32 = 0x300;
const XAPIC_ICR_HI: u32 = 0x310;
const XAPIC_TIMER_LVT: u32 = 0x320;
const XAPIC_TIMER_INIT: u32 = 0x380;
const XAPIC_TIMER_CURR: u32 = 0x390;
const XAPIC_TIMER_DIV: u32 = 0x3E0;
const XAPIC_LVT_LINT0: u32 = 0x350;
const XAPIC_LVT_LINT1: u32 = 0x356;
const XAPIC_TPR: u32 = 0x080;

// x2APIC MSR indices
const X2APIC_ID: u32 = 0x802;
const X2APIC_TPR: u32 = 0x808;
const X2APIC_EOI: u32 = 0x80B;
const X2APIC_SVR: u32 = 0x80F;
const X2APIC_ICR: u32 = 0x830;
const X2APIC_TIMER_LVT: u32 = 0x832;
const X2APIC_TIMER_INIT: u32 = 0x838;
const X2APIC_TIMER_CURR: u32 = 0x839;
const X2APIC_TIMER_DIV: u32 = 0x83E;
const X2APIC_LVT_LINT0: u32 = 0x835;
const X2APIC_LVT_LINT1: u32 = 0x836;

/// Spurious-interrupt vector (SVR): must match the IDT catch-all.
const SPURIOUS_VECTOR: u32 = 0xFF;

/// LAPIC timer divide configuration: divide by 16 (encoding 0x3).
const TIMER_DIV_16: u64 = 0x3;
const TIMER_DIVISOR: u64 = 16;

/// LVT mask bit.
const LVT_MASKED: u32 = 1 << 16;
/// LVT timer mode bits [18:17]: 1 = periodic.
const LVT_TIMER_PERIODIC: u32 = 1 << 17;
/// LVT delivery mode ExtINT (bits [10:8] = 7) for LINT0 virtual wire.
const LVT_EXTINT: u32 = 0x7 << 8;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// x2APIC active (per-machine: all CPUs switch together).
static X2APIC: AtomicBool = AtomicBool::new(false);
/// Driver initialized on this CPU (guards MSR writes that would #GP
/// before x2APIC is enabled).
static READY: AtomicBool = AtomicBool::new(false);

/// LAPIC id per kernel CPU number (BSP renumbered to 0).
static CPU_LAPIC_IDS: [AtomicU32; crate::config::MAX_CPUS] =
    [const { AtomicU32::new(u32::MAX) }; crate::config::MAX_CPUS];
/// Number of discovered enabled CPUs (>= 1 after init).
static DISCOVERED_CPUS: AtomicU32 = AtomicU32::new(1);

/// Calibrated LAPIC timer input frequency (Hz, before the divider).
pub static LAPIC_TIMER_FREQ: AtomicU64 = AtomicU64::new(0);
/// Calibrated TSC frequency (Hz).
pub static TSC_FREQ: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Register access
// ---------------------------------------------------------------------------

/// Read a LAPIC register in whichever mode is active.
#[inline]
fn reg_read(reg_xapic: u32, msr_x2apic: u32) -> u32 {
    if X2APIC.load(Ordering::Acquire) {
        // SAFETY: x2APIC MSR read; READY gates first use.
        unsafe { rdmsr(msr_x2apic) as u32 }
    } else {
        let va = crate::arch::mm::memory_layout::LAPIC_BASE as usize + reg_xapic as usize;
        // SAFETY: MMIO read of the local APIC window (identity-mapped at
        // setup_device_mappings time).
        unsafe { core::ptr::read_volatile(va as *const u32) }
    }
}

/// Write a LAPIC register in whichever mode is active.
#[inline]
fn reg_write(reg_xapic: u32, msr_x2apic: u32, value: u32) {
    if X2APIC.load(Ordering::Acquire) {
        // SAFETY: x2APIC MSR write; READY gates first use.
        unsafe { wrmsr(msr_x2apic, value as u64) };
    } else {
        let va = crate::arch::mm::memory_layout::LAPIC_BASE as usize + reg_xapic as usize;
        // SAFETY: MMIO write to the local APIC window.
        unsafe { core::ptr::write_volatile(va as *mut u32, value) };
    }
}

/// Acknowledge the highest-priority in-service interrupt on this CPU's
/// local APIC.  No-op until the driver is initialized (an EOI MSR write
/// with x2APIC disabled would #GP).
#[inline]
pub fn eoi() {
    if READY.load(Ordering::Acquire) {
        reg_write(XAPIC_EOI, X2APIC_EOI, 0);
    }
}

/// Is the driver initialized (LAPIC enabled on this CPU)?
#[inline]
pub fn ready() -> bool {
    READY.load(Ordering::Acquire)
}

/// Number of CPUs the MADT listed as enabled (BSP included).
#[inline]
pub fn cpu_count() -> usize {
    DISCOVERED_CPUS.load(Ordering::Acquire) as usize
}

/// LAPIC id of a kernel CPU number.
#[inline]
pub fn lapic_id(cpu: usize) -> u32 {
    if cpu < crate::config::MAX_CPUS {
        CPU_LAPIC_IDS[cpu].load(Ordering::Acquire)
    } else {
        u32::MAX
    }
}

// ---------------------------------------------------------------------------
// IPI
// ---------------------------------------------------------------------------

// ICR delivery commands (fixed physical mode, assert level, edge).
const ICR_FIXED: u32 = 0x0000;
const ICR_INIT: u32 = 0x0500;
const ICR_STARTUP: u32 = 0x0600;
const ICR_LEVEL_ASSERT: u32 = 1 << 14;

/// Send a fixed-mode IPI with `vector` to the CPU whose LAPIC id is
/// `dest`.  Physical destination mode.
pub fn send_ipi(dest: u32, vector: u8) {
    let cmd = ICR_FIXED | (vector as u32) | ICR_LEVEL_ASSERT;
    send_icr(dest, cmd);
}

/// Send an INIT-deassert... not needed on anything modern; kept for the
/// classic bring-up sequence shape.  Sends INIT assert.
pub fn send_init(dest: u32) {
    send_icr(dest, ICR_INIT | ICR_LEVEL_ASSERT);
}

/// Send a SIPI: `real_mode_page` is the physical 4K page (vector number)
/// the target starts executing (CS = page, IP = 0).
pub fn send_sipi(dest: u32, real_mode_page: u8) {
    send_icr(dest, ICR_STARTUP | ICR_LEVEL_ASSERT | real_mode_page as u32);
}

/// Raw ICR write, waiting for the delivery-status bit to clear first.
fn send_icr(dest: u32, cmd: u32) {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    if X2APIC.load(Ordering::Acquire) {
        // Delivery status bit is reserved/RAZ in x2APIC: the write is
        // posted; back-to-back MSR writes serialize sufficiently.
        let icr = ((dest as u64) << 32) | cmd as u64;
        // SAFETY: x2APIC ICR write.
        unsafe { wrmsr(X2APIC_ICR, icr) };
    } else {
        // Wait for Delivery Status (bit 12) to clear.
        while reg_read(XAPIC_ICR_LO, X2APIC_ICR) & (1 << 12) != 0 {
            core::hint::spin_loop();
        }
        reg_write(XAPIC_ICR_HI, 0, (dest & 0xFF) << 24);
        reg_write(XAPIC_ICR_LO, 0, cmd);
    }
}

// ---------------------------------------------------------------------------
// Timer
// ---------------------------------------------------------------------------

/// Timer vector owned by this driver (registered in the IDT by the arch
/// trap layer; must stay in sync with trap.rs's `vec::LOCAL_TIMER`).
pub const LOCAL_TIMER_VECTOR: u8 = 0xE0;

/// Arm the periodic LAPIC timer on this CPU at KERNEL_HZ.  Idempotent.
///
/// Returns true when the timer is actually armed. A false return (no
/// calibrated frequency / zero tick count) means this CPU has no LAPIC
/// tick — the caller must keep another tick source alive (PIT IRQ0) or
/// the system boots completely tick-less (X86-CLK: sleeps then hang
/// forever because nothing raises the timer softirq).
pub fn timer_start() -> bool {
    if !READY.load(Ordering::Acquire) {
        return false;
    }
    let freq = LAPIC_TIMER_FREQ.load(Ordering::Acquire);
    if freq == 0 {
        return false; // calibration never ran: keep the timer masked
    }
    let ticks = freq / TIMER_DIVISOR / (crate::config::KERNEL_HZ as u64);
    if ticks == 0 {
        return false;
    }
    reg_write(XAPIC_TIMER_DIV, X2APIC_TIMER_DIV, TIMER_DIV_16 as u32);
    reg_write(XAPIC_TIMER_INIT, X2APIC_TIMER_INIT, ticks as u32);
    let lvt = (LOCAL_TIMER_VECTOR as u32) | LVT_TIMER_PERIODIC;
    reg_write(XAPIC_TIMER_LVT, X2APIC_TIMER_LVT, lvt);
    true
}

/// Mask the LAPIC timer on this CPU.
pub fn timer_mask() {
    if READY.load(Ordering::Acquire) {
        let lvt = (LOCAL_TIMER_VECTOR as u32) | LVT_TIMER_PERIODIC | LVT_MASKED;
        reg_write(XAPIC_TIMER_LVT, X2APIC_TIMER_LVT, lvt);
    }
}

// ---------------------------------------------------------------------------
// Delay (post-calibration)
// ---------------------------------------------------------------------------

/// Busy-wait `ms` milliseconds using the calibrated TSC.  Only valid
/// after `init()` (BSP) has calibrated.
pub fn delay_ms(ms: u64) {
    let freq = TSC_FREQ.load(Ordering::Acquire);
    if freq == 0 {
        // Uncalibrated fallback: a fixed generous spin.
        for _ in 0..ms.saturating_mul(100_000) {
            core::hint::spin_loop();
        }
        return;
    }
    let start = crate::arch::cpu::read_counter();
    let target = freq / 1000 * ms;
    while crate::arch::cpu::read_counter().wrapping_sub(start) < target {
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------------
// Calibration (PIT channel 2 one-shot window)
// ---------------------------------------------------------------------------

const PIT_CH2_DATA: u16 = 0x42;
const PIT_MODE_CMD: u16 = 0x43;
const PIT_CH2_GATE_PORT: u16 = 0x61;
/// PIT input gate frequency (fixed 1193182 Hz crystal).
const PIT_INPUT_FREQ: u64 = 1_193_182;
/// Calibration window in milliseconds (longer = tighter calibration;
/// boot-time cost under TCG is acceptable up to ~50ms).
const CAL_MS: u64 = 50;

/// Run one PIT channel-2 one-shot window and return the elapsed TSC and
/// LAPIC-timer counts inside it.
fn calibration_window() -> (u64, u64) {
    // SAFETY: fixed 8254 ports; IF is off during early bring-up.
    unsafe {
        // Gate off + speaker off, then re-program for a clean reload.
        let p61 = inb(PIT_CH2_GATE_PORT);
        outb(PIT_CH2_GATE_PORT, p61 & !0x03);
        // Mode 0 (interrupt on terminal count), lo/hi bytes.
        outb(PIT_MODE_CMD, 0xB0);
        let count = (PIT_INPUT_FREQ / 1000 * CAL_MS) as u16;
        outb(PIT_CH2_DATA, (count & 0xFF) as u8);
        outb(PIT_CH2_DATA, (count >> 8) as u8);

        let tsc0 = crate::arch::cpu::read_counter();
        let ccr0 = reg_read(XAPIC_TIMER_CURR, X2APIC_TIMER_CURR) as u64;

        // Gate on, speaker output disconnected.  OUT2 (port 0x61 bit 5)
        // goes high when the counter reaches zero.
        outb(PIT_CH2_GATE_PORT, (p61 & !0x02) | 0x01);
        while inb(PIT_CH2_GATE_PORT) & 0x20 == 0 {
            core::hint::spin_loop();
        }

        let tsc1 = crate::arch::cpu::read_counter();
        let ccr1 = reg_read(XAPIC_TIMER_CURR, X2APIC_TIMER_CURR) as u64;
        (tsc1.wrapping_sub(tsc0), ccr0.wrapping_sub(ccr1))
    }
}

/// Calibrate the TSC and the LAPIC timer input against PIT ch2.  Must
/// run with interrupts disabled, after the LAPIC is enabled and the
/// timer LVT is masked.
fn calibrate() {
    // Free-run the timer (huge initial count) while masked.
    reg_write(XAPIC_TIMER_DIV, X2APIC_TIMER_DIV, TIMER_DIV_16 as u32);
    let lvt_masked = (LOCAL_TIMER_VECTOR as u32) | LVT_MASKED;
    reg_write(XAPIC_TIMER_LVT, X2APIC_TIMER_LVT, lvt_masked);
    reg_write(XAPIC_TIMER_INIT, X2APIC_TIMER_INIT, u32::MAX);

    // Two windows: a transiently stuck CCR/OUT2 (observed under TCG when
    // the register interface is half-broken) must not leave the system
    // uncalibrated — the second window re-arms the free-running counter.
    let mut tsc_delta = 0;
    let mut lapic_delta = 0;
    for _ in 0..2 {
        reg_write(XAPIC_TIMER_DIV, X2APIC_TIMER_DIV, TIMER_DIV_16 as u32);
        reg_write(XAPIC_TIMER_LVT, X2APIC_TIMER_LVT, lvt_masked);
        reg_write(XAPIC_TIMER_INIT, X2APIC_TIMER_INIT, u32::MAX);
        let (t, l) = calibration_window();
        if t > 0 {
            tsc_delta = t;
        }
        if l > 0 {
            lapic_delta = l;
        }
        if tsc_delta > 0 && lapic_delta > 0 {
            break;
        }
    }
    let secs = CAL_MS as f64 / 1000.0;

    if lapic_delta > 0 && tsc_delta > 0 {
        let lapic = (lapic_delta as f64 / secs) as u64;
        let tsc = (tsc_delta as f64 / secs) as u64;
        // Sanity clamps: a 25MHz..10GHz window keeps garbage calibrations
        // (stuck counter, broken OUT2) from arming a lunatic timer.
        if (25_000_000..=10_000_000_000).contains(&lapic) {
            LAPIC_TIMER_FREQ.store(lapic, Ordering::Release);
        }
        if (25_000_000..=10_000_000_000).contains(&tsc) {
            TSC_FREQ.store(tsc, Ordering::Release);
        }
    }
    // Leave the timer masked; timer_start() arms it per CPU.
    crate::drivers::timer::lapic_time_calibrated(
        LAPIC_TIMER_FREQ.load(Ordering::Acquire),
        TSC_FREQ.load(Ordering::Acquire),
    );

    // Boot diagnostic (Linux prints the same numbers): which rates the
    // timekeeping chain actually calibrated to. A failed TSC calibration
    // here means CLOCK_MONOTONIC stays on the coarse PIT clock; a failed
    // LAPIC calibration means the periodic tick falls back to PIT IRQ0.
    let tsc = TSC_FREQ.load(Ordering::Acquire);
    let lapic = LAPIC_TIMER_FREQ.load(Ordering::Acquire);
    crate::print_status(
        "timer",
        &alloc::format!(
            "TSC {} MHz, LAPIC {} MHz ({} APIC)",
            tsc / 1_000_000,
            lapic / 1_000_000,
            if X2APIC.load(Ordering::Acquire) { "x2" } else { "MMIO" }
        ),
        tsc != 0 && lapic != 0,
    );
}

// ---------------------------------------------------------------------------
// ACPI MADT discovery
// ---------------------------------------------------------------------------

#[repr(C, packed)]
struct Rsdp {
    signature: [u8; 8], // "RSD PTR "
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_addr: u32,
    length: u32,
    xsdt_addr: u64,
}

#[repr(C, packed)]
struct SdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    _oem: [u8; 6],
    _oem_table_id: [u8; 8],
    _oem_rev: u32,
    _creator_id: u32,
    _creator_rev: u32,
}

/// Validate the checksum over the first `len` bytes at `vaddr`.
fn checksum_ok(vaddr: usize, len: usize) -> bool {
    let mut sum: u8 = 0;
    for i in 0..len {
        // SAFETY: caller passes a mapped, readable table region.
        sum = sum.wrapping_add(unsafe { core::ptr::read_volatile((vaddr + i) as *const u8) });
    }
    sum == 0
}

/// Volatile 32-bit read from a linear-mapped physical address.
fn read_phys_u32(phys: usize) -> u32 {
    let va = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new(phys as u64),
    );
    // SAFETY: linear-mapped RAM window.
    unsafe { core::ptr::read_volatile(va.as_usize() as *const u32) }
}

/// Volatile 64-bit read from a linear-mapped physical address.
fn read_phys_u64(phys: usize) -> u64 {
    let va = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new(phys as u64),
    );
    // SAFETY: linear-mapped RAM window.
    unsafe { core::ptr::read_volatile(va.as_usize() as *const u64) }
}

/// Map the ACPI table scan windows into the linear map.  The firmware
/// table area (0xE0000..0x100000) and the EBDA are e820-reserved, so
/// `setup_linear_mapping` (usable regions only) skips them — map them
/// here with the same flags the linear map uses.
pub(crate) fn map_acpi_windows() {
    // Legacy EBDA area (segment read from 0x40E, up to 0x9FC00+1K).
    ensure_phys_mapped(0x9_F000, 0x1_000);
    // Firmware ACPI table area.
    ensure_phys_mapped(0xE_0000, 0x2_0000);
}

/// Last physical page mapped by `ensure_phys_mapped` (skips redundant
/// remaps of the same window — the RSDP scan never calls this).
static LAST_MAPPED_START: AtomicU64 = AtomicU64::new(u64::MAX);
static LAST_MAPPED_END: AtomicU64 = AtomicU64::new(0);

/// Map `[phys, phys+len)` into the linear map (firmware tables live in
/// e820-reserved regions the linear map skipped).  Re-mapping an
/// already-mapped PTE rewrites the same value (idempotent).
fn ensure_phys_mapped(phys: usize, len: usize) {
    const PAGE: usize = 4096;
    let start = phys & !(PAGE - 1);
    let end = (phys + len.max(1) + PAGE - 1) & !(PAGE - 1);
    if end <= start {
        return;
    }
    // Skip when the requested range sits inside the last mapped window.
    let (ls, le) = (
        LAST_MAPPED_START.load(Ordering::Acquire),
        LAST_MAPPED_END.load(Ordering::Acquire),
    );
    if ls != u64::MAX && start >= ls as usize && end <= le as usize {
        return;
    }
    let flags = crate::arch::mm::pagetable::PageTableEntry::P
        | crate::arch::mm::pagetable::PageTableEntry::RW
        | crate::arch::mm::pagetable::PageTableEntry::ACCESSED
        | crate::arch::mm::pagetable::PageTableEntry::DIRTY;
    let page_offset = crate::arch::mm::memory_layout::PAGE_OFFSET as u64;
    // SAFETY: kernel-only linear-map VAs for firmware table regions;
    // the regions are reserved (never handed to the page allocator).
    unsafe {
        crate::arch::mm::mmu_init::map_kernel_region(
            page_offset + start as u64,
            start as u64,
            (end - start) as u64,
            flags,
        );
    }
    LAST_MAPPED_START.store(start as u64, Ordering::Release);
    LAST_MAPPED_END.store(end as u64, Ordering::Release);
}

/// Map a page around `phys` and read the u32 at `phys` (safe for ACPI
/// header fields before the table length is known).
fn read_table_u32(phys: usize) -> u32 {
    ensure_phys_mapped(phys & !0xFFF, 8);
    read_phys_u32(phys)
}

/// Find the RSDP in the EBDA / legacy BIOS ROM area.  Returns its
/// physical address.
fn find_rsdp() -> Option<usize> {
    // EBDA base is a real-mode segment at 0x40E.
    let ebda_seg = read_phys_u32(0x40E) & 0xFFFF;
    let candidates: [usize; 2] = [(ebda_seg as usize) << 4, 0xE_0000];
    let sizes: [usize; 2] = [1024, 0x2_0000];

    for (base, size) in candidates.iter().zip(sizes.iter()) {
        let mut a = *base & !0xF;
        while a + 36 <= base + size {
            let va = crate::arch::mm::memory_layout::phys_to_virt(
                crate::arch::mm::memory_layout::PhysAddr::new(a as u64),
            )
            .as_usize();
            // SAFETY: mapped candidate region.
            let sig = unsafe { core::ptr::read_volatile(va as *const [u8; 8]) };
            if &sig == b"RSD PTR " {
                let len = {
                    // Revision 0 (ACPI 1.0) checksums only the first 20
                    // bytes; later revisions use the Length field.
                    let rev = unsafe { core::ptr::read_volatile((va + 15) as *const u8) };
                    if rev >= 2 {
                        let l = unsafe { core::ptr::read_volatile((va + 20) as *const u32) };
                        (l as usize).min(64)
                    } else {
                        20
                    }
                };
                if checksum_ok(va, len) {
                    return Some(a);
                }
            }
            a += 16;
        }
    }
    None
}

/// Walk RSDT/XSDT entries and return the physical address of the table
/// with the given signature.
fn find_table(rsdp_phys: usize, want: &[u8; 4]) -> Option<usize> {
    let rsdp_va = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new(rsdp_phys as u64),
    )
    .as_usize();
    // SAFETY: validated RSDP.
    let rsdp = unsafe { &*(rsdp_va as *const Rsdp) };

    let (entries_phys, entry_size, count): (usize, usize, usize) = if rsdp.revision >= 2 {
        // SAFETY: packed field read.
        let xsdt = unsafe { core::ptr::read_volatile(&raw const rsdp.xsdt_addr) } as usize;
        if xsdt == 0 {
            return None;
        }
        let hdr_len = read_table_u32(xsdt + 4).min(0x2_0000) as usize;
        ensure_phys_mapped(xsdt, hdr_len.max(64));
        let xsdt_va = crate::arch::mm::memory_layout::phys_to_virt(
            crate::arch::mm::memory_layout::PhysAddr::new(xsdt as u64),
        )
        .as_usize();
        if !checksum_ok(xsdt_va, hdr_len) {
            return None;
        }
        (xsdt + core::mem::size_of::<SdtHeader>(), 8, (hdr_len - 36) / 8)
    } else {
        // SAFETY: packed field read.
        let rsdt = unsafe { core::ptr::read_volatile(&raw const rsdp.rsdt_addr) } as usize;
        if rsdt == 0 {
            return None;
        }
        let hdr_len = read_table_u32(rsdt + 4).min(0x2_0000) as usize;
        ensure_phys_mapped(rsdt, hdr_len.max(64));
        let rsdt_va = crate::arch::mm::memory_layout::phys_to_virt(
            crate::arch::mm::memory_layout::PhysAddr::new(rsdt as u64),
        )
        .as_usize();
        if !checksum_ok(rsdt_va, hdr_len) {
            return None;
        }
        (rsdt + core::mem::size_of::<SdtHeader>(), 4, (hdr_len - 36) / 4)
    };

    for i in 0..count {
        let tbl = if entry_size == 8 {
            read_phys_u64(entries_phys + i * 8) as usize
        } else {
            read_phys_u32(entries_phys + i * 4) as usize
        };
        if tbl == 0 {
            continue;
        }
        // Map the header page before touching it (entry tables live in
        // reserved firmware regions).
        ensure_phys_mapped(tbl, 64);
        let tbl_va = crate::arch::mm::memory_layout::phys_to_virt(
            crate::arch::mm::memory_layout::PhysAddr::new(tbl as u64),
        )
        .as_usize();
        // SAFETY: header-sized mapped read.
        let sig = unsafe { core::ptr::read_volatile(tbl_va as *const [u8; 4]) };
        if &sig == want {
            let len = read_table_u32(tbl + 4) as usize;
            ensure_phys_mapped(tbl, len.max(64));
            return Some(tbl);
        }
    }
    None
}

/// Find the HPET MMIO base from the ACPI HPET table (used by the timer
/// driver's HPET clocksource; the fixed q35 base 0xFED00000 is the
/// caller's fallback).  Returns None when no table exists or the base
/// address is not a system-memory address.
pub fn find_hpet_base() -> Option<usize> {
    // The RSDP scan touches the e820-reserved firmware windows, which
    // the linear map skips — map them first (idempotent).
    map_acpi_windows();
    let tbl = find_rsdp().and_then(|rsdp| find_table(rsdp, b"HPET"))?;
    // ACPI HPET table: 36-byte header, 8-byte event timer block ID,
    // then a Generic Address Structure whose u64 address sits at
    // offset 48 (space id at 44; 0 = system memory).
    let space = read_phys_u32(tbl + 44) as u8;
    if space != 0 {
        return None;
    }
    let lo = read_phys_u32(tbl + 48) as usize;
    let hi = read_phys_u32(tbl + 52) as usize;
    let addr = lo | (hi << 32);
    if addr == 0 {
        None
    } else {
        Some(addr)
    }
}

/// Parse the MADT: fill `CPU_LAPIC_IDS` with enabled CPU ids, BSP first.
fn parse_madt(madt_phys: usize) -> usize {
    let va = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new(madt_phys as u64),
    )
    .as_usize();
    let hdr_len = read_table_u32(madt_phys + 4).min(0x1_0000) as usize;
    ensure_phys_mapped(madt_phys, hdr_len.max(64));
    if !checksum_ok(va, hdr_len) {
        return 0;
    }
    // SAFETY: mapped MADT.
    let hdr = unsafe { &*(va as *const SdtHeader) };
    if &hdr.signature != b"APIC" {
        return 0;
    }

    let bsp_id = this_lapic_id_raw();
    let mut ids = [0u32; crate::config::MAX_CPUS];
    let mut n = 0usize;

    let body = va + core::mem::size_of::<SdtHeader>() + 8; // skip lapic_addr+flags
    let mut off = body;
    let end = va + hdr_len;
    while off + 2 <= end && n < crate::config::MAX_CPUS {
        // SAFETY: entry header read inside the validated table.
        let (etype, elen) = unsafe {
            (
                core::ptr::read_volatile(off as *const u8),
                core::ptr::read_volatile((off + 1) as *const u8),
            )
        };
        if elen < 2 || off + elen as usize > end {
            break;
        }
        match etype {
            0 => {
                // Processor Local APIC: [u8 acpi_id][u8 apic_id][u32 flags]
                if elen >= 8 {
                    // SAFETY: fixed offsets inside the entry.
                    let (apic_id, flags) = unsafe {
                        (
                            core::ptr::read_volatile((off + 3) as *const u8) as u32,
                            core::ptr::read_volatile((off + 4) as *const u32),
                        )
                    };
                    // bit 0 = enabled (ACPI < 5.1); bit 1 = online-capable.
                    if flags & 0x3 != 0 {
                        ids[n] = apic_id;
                        n += 1;
                    }
                }
            }
            9 => {
                // Local x2APIC: [u16 rsvd][u32 uid][u32 flags][u32 acpi_id]
                if elen >= 16 {
                    // SAFETY: fixed offsets inside the entry.
                    let (uid, flags) = unsafe {
                        (
                            core::ptr::read_volatile((off + 4) as *const u32),
                            core::ptr::read_volatile((off + 8) as *const u32),
                        )
                    };
                    if flags & 0x3 != 0 {
                        ids[n] = uid;
                        n += 1;
                    }
                }
            }
            _ => {}
        }
        off += elen as usize;
    }

    // Renumber: BSP (this CPU) becomes kernel CPU 0, the rest follow in
    // MADT order.
    let mut k = 0usize;
    CPU_LAPIC_IDS[k].store(bsp_id, Ordering::Release);
    crate::arch::smp::PER_CPU[k].lapic_id.store(bsp_id, Ordering::Release);
    k += 1;
    for &id in ids.iter().take(n) {
        if id != bsp_id && k < crate::config::MAX_CPUS {
            CPU_LAPIC_IDS[k].store(id, Ordering::Release);
            k += 1;
        }
    }
    k
}

/// Read the local APIC id without the READY guard (used during MADT
/// parsing while the APIC is globally enabled but the driver is not yet
/// marked ready).
fn this_lapic_id_raw() -> u32 {
    if X2APIC.load(Ordering::Acquire) {
        // SAFETY: x2APIC enabled by init_x2apic_or_mmio before this runs.
        unsafe { rdmsr(X2APIC_ID) as u32 }
    } else {
        reg_read(XAPIC_ID, X2APIC_ID) >> 24
    }
}

/// This CPU's LAPIC id.
pub fn this_lapic_id() -> u32 {
    this_lapic_id_raw()
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Enable x2APIC when the CPU reports it; returns the new mode.
///
/// X86-CLK fix: the feature test reads CPUID leaf 1 ECX bit 21 (the
/// actual x2APIC bit). The old inline-asm probe left EAX at 0, so it
/// tested bit 21 of the LEAF-0 ECX — a third of the VENDOR STRING
/// ("ntel" for GenuineIntel has bit 21 set, "cAMD" for AuthenticAMD
/// does not). Every Intel-vendor CPU was therefore switched to x2APIC
/// MSR mode whether or not x2APIC was real, and on QEMU TCG models
/// where the MSR interface is not backed (Skylake-Client) every
/// register access returned 0: dead LAPIC timer, failed calibration,
/// and (because the boot path then masked the PIT fallback) a
/// completely tick-less system — nanosleep hung forever.
///
/// Even a correctly advertised x2APIC is verified before use: after
/// setting EXTD, the SVR MSR must read back nonzero (its reset value is
/// 0xFF). A dead interface falls back to MMIO xAPIC, which works on
/// every model seen so far.
fn init_x2apic_or_mmio() -> bool {
    // CPUID leaf 1, ECX bit 21 = x2APIC.
    let has_x2 = crate::arch::cpu::cpuid(1, 0).2 & (1 << 21) != 0;

    // SAFETY: IA32_APIC_BASE programming of this CPU.
    unsafe {
        let base = rdmsr(MSR_IA32_APIC_BASE);
        let mut new = base | APIC_BASE_ENABLE;
        if has_x2 {
            new |= APIC_BASE_X2APIC;
        }
        wrmsr(MSR_IA32_APIC_BASE, new);
    }

    if has_x2 {
        // Verify the MSR interface actually responds before committing
        // to it: any functioning APIC reads SVR nonzero (reset 0xFF);
        // an unbacked range reads 0.
        // SAFETY: x2APIC MSR read gated on the EXTD write above.
        let svr = unsafe { rdmsr(X2APIC_SVR) };
        if svr == 0 {
            // Fall back to MMIO xAPIC: clear EXTD, keep global enable.
            // SAFETY: IA32_APIC_BASE programming of this CPU.
            unsafe {
                let base = rdmsr(MSR_IA32_APIC_BASE);
                wrmsr(MSR_IA32_APIC_BASE, base & !APIC_BASE_X2APIC | APIC_BASE_ENABLE);
            }
            return false;
        }
    }
    has_x2
}

/// BSP bring-up: enable the local APIC, discover CPUs from the ACPI
/// MADT, calibrate the timer, and program the LVTs.  Called from
/// `drivers::intc::init()` with interrupts disabled, after the linear
/// map and device mappings exist.
pub fn init() {
    let x2 = init_x2apic_or_mmio();
    X2APIC.store(x2, Ordering::Release);
    READY.store(true, Ordering::Release);

    // Spurious vector + enable; TPR = 0 (accept everything).
    reg_write(XAPIC_SVR, X2APIC_SVR, SPURIOUS_VECTOR | 0x100);
    reg_write(XAPIC_TPR, X2APIC_TPR, 0);

    // LINT0 = ExtINT (virtual wire keeps the 8259 flowing), LINT1 masked.
    reg_write(XAPIC_LVT_LINT0, X2APIC_LVT_LINT0, LVT_EXTINT);
    reg_write(XAPIC_LVT_LINT1, X2APIC_LVT_LINT1, LVT_MASKED);

    // Discover CPUs (MADT); a missing table degrades to single-CPU.
    map_acpi_windows();
    let found = find_rsdp()
        .and_then(|rsdp| find_table(rsdp, b"APIC"))
        .map(|madt| parse_madt(madt))
        .unwrap_or(1);
    DISCOVERED_CPUS.store(found.max(1) as u32, Ordering::Release);

    // One-time calibration (TSC + LAPIC timer vs PIT ch2).
    calibrate();
}

/// Per-CPU bring-up on an AP: enable the local APIC in the same mode the
/// BSP chose, program the LVTs, and arm the timer (the CPU still runs
/// with IF clear until the caller enables interrupts).
pub fn init_secondary() {
    // SAFETY: IA32_APIC_BASE programming of this CPU.
    unsafe {
        let base = rdmsr(MSR_IA32_APIC_BASE)
            | APIC_BASE_ENABLE
            | if X2APIC.load(Ordering::Acquire) { APIC_BASE_X2APIC } else { 0 };
        wrmsr(MSR_IA32_APIC_BASE, base);
    }
    READY.store(true, Ordering::Release);

    reg_write(XAPIC_SVR, X2APIC_SVR, SPURIOUS_VECTOR | 0x100);
    reg_write(XAPIC_TPR, X2APIC_TPR, 0);
    reg_write(XAPIC_LVT_LINT0, X2APIC_LVT_LINT0, LVT_EXTINT);
    reg_write(XAPIC_LVT_LINT1, X2APIC_LVT_LINT1, LVT_MASKED);

    timer_start();
}
