//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! GICv3 interrupt controller driver for the QEMU `virt` machine
//! (`-machine gic-version=3`).
//!
//! Implements the IrqChip/IrqDomainOps contract the riscv64 PLIC driver
//! established: one linear domain with hwirq == GIC INTID (identity),
//! chip ops for mask/unmask/eoi, and a claim/EOI pair the arch trap
//! path consumes.
//!
//! Address map (QEMU virt, DTB `/intc` node is authoritative — reg =
//! GICD 0x0800_0000/64K + GICR 0x080A_0000, redistributor frames at
//! 2*64KB stride):
//!   - Distributor (GICD_*): SPI configuration, routing
//!   - Redistributor (GICR_*): per-CPU frame pair (RD_base + SGI_base)
//!   - CPU interface: ICC_* system registers only (no MMIO window)
//!
//! INTID allocation:
//!   0..15   SGIs (INTID 0 carries the IPI bitmap, see arch/aarch64/ipi.rs)
//!   16..31  PPIs (INTID 27 = PPI 11 = EL1 virtual timer)
//!   32..    SPIs (SPI n → INTID 32+n; PL011 = SPI 1, PCI INTx = SPI 3-6)
//!
//! All GIC regions are low MMIO below RAM base and are accessed through
//! the kernel-half MMIO alias (`mmio_alias`) — never raw phys VAs.

use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::interrupt::{
    IrqChip, IrqData, IrqDomainOps, IrqDomain,
    irq_domain_create_linear, irq_create_mapping,
};

// ==================== Base addresses ====================

/// Distributor base (kernel-half alias of phys 0x0800_0000)
const GICD_BASE: usize =
    crate::arch::mm::memory_layout::mmio_alias(crate::arch::mm::memory_layout::GICD_BASE)
        as usize;

/// Redistributor frames base (kernel-half alias of phys 0x080A_0000)
const GICR_FRAMES: usize =
    crate::arch::mm::memory_layout::mmio_alias(crate::arch::mm::memory_layout::GICR_BASE)
        as usize;

/// Redistributor frame stride: RD_base (64KB) + SGI_base (64KB) per CPU.
const GICR_STRIDE: usize = 0x20000;

/// Maximum redistributor frames probed (stop at GICR_TYPER.Last; the
/// QEMU virt region sized by the DTB covers well beyond MAX_CPUS=4).
const GICR_MAX_FRAMES: usize = 16;

// ==================== INTID constants ====================

/// EL1 virtual timer PPI (PPI 11): CNTV_CVAL/CNTV_CTL pair the timer
/// driver programs. The DTB timer node lists PPIs 13/14/11/10 (secure
/// phys / EL1 phys / virtual / hyp); the driver arms the VIRTUAL timer
/// (always accessible at EL1, no CNTHCTL_EL2 gating), so its PPI 11 /
/// INTID 27 is the one wired here.
pub const TIMER_PPI_INTID: u32 = 27;

/// SGI used for the IPI bitmap multiplex (arch/aarch64/ipi.rs twin of
/// the riscv64 SSIP path).
pub const IPI_SGI_INTID: u32 = 0;

/// Spurious INTID returned by ICC_IAR1_EL1 when no Group-1 interrupt
/// is pending.
const INTID_SPURIOUS: u32 = 1023;

// ==================== GICD register offsets ====================

mod gicd {
    pub const CTLR: usize = 0x0000;       // DS=bit6 ARE=RAO G1=bit1 G0=bit0
    pub const TYPER: usize = 0x0004;      // ITLinesNumber bits[4:0]
    // GICD_IGROUPR(n) sits at 0x0080 + 4n (GICv3 map; 0x020-0x07c is
    // reserved/RAZ-WI). The port originally wrote 0x0020, where every
    // Group-1 assignment silently vanished: SPIs stayed in reset Group 0
    // while the (correctly-addressed) enable/priority/routing writes
    // landed — PL011 RX (INTID 33) sat pending+enabled at the
    // distributor but was never forwarded, because Group-0 distribution
    // is disabled on this machine.
    pub const IGROUPR: usize = 0x0080;    // +4*intid/32 (Group select)
    pub const ISENABLER: usize = 0x0100;  // +4*(intid/32)
    pub const ICENABLER: usize = 0x0180;  // +4*(intid/32)
    pub const IPRIORITYR: usize = 0x0400; // +intid (byte per INTID)
    pub const IROUTER: usize = 0x6000;    // +8*(intid-32), 64-bit affinity
}

mod gicd_ctlr_bits {
    pub const RWP: u32 = 1 << 31;         // Read-Write Pending
    // GICD_CTLR group enables on this machine (QEMU virt GICv3 with
    // security unsupported: GICD_CTLR.DS is RAO, so the security-disabled
    // view applies — bit 0 = EnableGrp0, bit 1 = EnableGrp1(NS), and the
    // ARE bits are RAO/WI, affinity routing is permanently on).
    //
    // The port originally wrote bit 0 believing it was the Group-1
    // enable, leaving Group-1 forwarding off while every INTID was
    // configured Group 1 (IGROUPR all-ones). The delivery scan then
    // masked every pending interrupt out (its group mask degenerated
    // to zero): ICC_IAR1_EL1 read 0x3FF forever, no IRQ exception was
    // ever taken, and the architectural timer's PPI sat pending in the
    // redistributor (GICR_ISPENDINGR0 bit 27 latched) without ever
    // reaching the CPU interface. Group-1 forwarding is bit 1.
    pub const ENABLE_G1: u32 = 1 << 1;    // Group-1 (NS) forwarding
    pub const ENABLE_G0: u32 = 1 << 0;    // Group-0 forwarding (unused)
}

// ==================== GICR register offsets ====================
//
// Two sub-frames per CPU: RD_base (frame + 0x0000) and SGI_base
// (frame + 0x10000, INTID 0-31 configuration).

mod gicr {
    // RD_base
    pub const CTLR: usize = 0x0000;       // RWP = bit 3
    pub const TYPER: usize = 0x0008;      // 64-bit: aff [63:32], Last bit 4
    pub const WAKER: usize = 0x0014;      // ProcessorSleep bit1 ChildrenAsleep bit2

    // SGI_base
    pub const SGI_IGROUPR0: usize = 0x10080; // Group for INTID 0-31
    pub const SGI_IPRIORITYR: usize = 0x10400; // +4*intid
    pub const SGI_ISENABLER0: usize = 0x10100;
    pub const SGI_ICENABLER0: usize = 0x10180;
}

mod gicr_ctlr_bits {
    pub const RWP: u32 = 1 << 3;
}

mod gicr_waker_bits {
    pub const PROCESSOR_SLEEP: u32 = 1 << 1;
    pub const CHILDREN_ASLEEP: u32 = 1 << 2;
}

// ==================== Priorities ====================
//
// GICv3 priority field: 5 implemented bits ([7:3], 32 levels), smaller
// value = higher priority; PMR runs at 0xFF so everything is deliverable.
// Mirrors the PLIC twin's layering: interactive sources outrank bulk.

const PRIO_TIMER: u32 = 0x40;  // tick + scheduler heartbeat
const PRIO_UART: u32 = 0x60;   // console (SPI 1 → INTID 33)
const PRIO_DEFAULT: u32 = 0xA0;

fn default_priority_for(intid: u32) -> u32 {
    match intid {
        TIMER_PPI_INTID => PRIO_TIMER,
        33 => PRIO_UART,             // PL011 UART SPI 1
        _ => PRIO_DEFAULT,
    }
}

// ==================== MMIO accessors ====================

#[inline]
unsafe fn reg_read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline]
unsafe fn reg_write32(addr: usize, v: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, v) }
}

#[inline]
unsafe fn reg_read64(addr: usize) -> u64 {
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

#[inline]
unsafe fn reg_write64(addr: usize, v: u64) {
    unsafe { core::ptr::write_volatile(addr as *mut u64, v) }
}

/// Wait for a RWP-style busy bit to clear (bounded; the emulated GIC
/// clears it in single-digit accesses).
fn wait_not_set(addr: usize, bit: u32) {
    for _ in 0..1_000_000 {
        // SAFETY: diagnostic poll of a GIC register previously written.
        if unsafe { reg_read32(addr) } & bit == 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

// ==================== ICC_* system registers ====================

/// Read ICC_IAR1_EL1 — acknowledge the highest-priority pending
/// Group-1 interrupt (priority drop). Returns the INTID.
#[inline]
pub fn iar1_read() -> u32 {
    let v: u64;
    // SAFETY: ICC_IAR1_EL1 read is the GIC acknowledge; side effect is
    // the priority drop for the returned INTID.
    unsafe { asm!("mrs {}, icc_iar1_el1", out(reg) v, options(nomem, nostack)) };
    v as u32 & 0xFF_FFFF
}

/// Write ICC_EOIR1_EL1 — end of interrupt (priority drop completion +
/// deactivation; ICC_CTLR_EL1.EOImode stays 0 = single-write EOI).
#[inline]
pub fn eoir1_write(intid: u32) {
    // SAFETY: EOIR1 write deactivates the previously acknowledged INTID.
    unsafe {
        asm!("msr icc_eoir1_el1, {}",
             in(reg) intid as u64, options(nomem, nostack))
    };
}

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
pub fn send_sgi(target_aff0: u8, intid: u32) {
    let sgi = (((intid as u64) & 0xF) << 24) | (1u64 << (target_aff0 & 0xF));
    // SAFETY: SGI1R write raises the SGI on the target core(s).
    unsafe {
        asm!("dsb st",
             "msr icc_sgi1r_el1, {}",
             in(reg) sgi, options(nomem, nostack))
    };
}

// ==================== Distributor ====================

/// Number of INTIDs the distributor covers ((TYPER.ITLinesNumber+1)*32)
static DIST_INTID_LIMIT: AtomicU32 = AtomicU32::new(0);

/// Domain size: the irq_desc array is PLIC_MAX_INTERRUPTS (128) wide —
/// the GIC domain maps INTIDs [0, 128) identity; higher SPIs stay
/// unmapped (QEMU virt's UART/PCI INTx INTIDs all sit below 64).
const DOMAIN_SIZE: u32 = crate::config::PLIC_MAX_INTERRUPTS as u32;

fn dist_init() {
    let typer = unsafe { reg_read32(GICD_BASE + gicd::TYPER) };
    let lines = ((typer & 0x1F) as u32 + 1) * 32;
    DIST_INTID_LIMIT.store(lines, Ordering::Release);

    // 1. Disable the distributor while reconfiguring.
    unsafe { reg_write32(GICD_BASE + gicd::CTLR, 0) };
    wait_not_set(GICD_BASE + gicd::CTLR, gicd_ctlr_bits::RWP);

    // 2. Configure every SPI: priority + routing per INTID, then
    //    full-word Group/disable per 32-INTID bank (IGROUPR has no set
    //    alias; every SPI in [32, limit) is Group 1 and starts
    //    disabled). Level-triggered reset default of GICD_ICFGR matches
    //    every SPI this machine wires (UART, PCI INTx — all level).
    //    IPRIORITYR packs FOUR INTIDs per 32-bit word — build whole
    //    words; a naive per-INTID full-word write zeroes the three
    //    neighbor byte lanes (caught by readback during bring-up).
    let limit = lines.min(DOMAIN_SIZE);
    let boot_mpidr = crate::arch::smp::index_to_mpidr(0);
    for word_id in (32..limit).step_by(4) {
        let mut word: u32 = 0;
        for lane in 0..4u32 {
            let id = word_id + lane;
            if id < limit {
                word |= (default_priority_for(id) & 0xFF) << (8 * lane);
            }
        }
        // IPRIORITYR is BYTE-indexed by INTID (four INTIDs per word):
        // the word covering INTIDs [word_id, word_id+4) sits at byte
        // offset word_id itself. The original +4*word_id spread wrote
        // each word 4x too far out (INTID 32's priorities landed in
        // INTID 128's slots), leaving every SPI at its reset priority.
        unsafe { reg_write32(GICD_BASE + gicd::IPRIORITYR + word_id as usize, word) };
    }
    for intid in 32..limit {
        unsafe {
            reg_write64(
                GICD_BASE + gicd::IROUTER + 8 * (intid as usize - 32),
                boot_mpidr,
            );
        }
    }
    for n in 1..(limit as usize + 31) / 32 {
        unsafe {
            reg_write32(GICD_BASE + gicd::IGROUPR + 4 * n, 0xFFFF_FFFF);
            reg_write32(GICD_BASE + gicd::ICENABLER + 4 * n, 0xFFFF_FFFF);
        }
    }

    // 3. Enable: Group-1 forwarding (bit 1 — see gicd_ctlr_bits; ARE is
    //    RAO/WI on this machine and needs no write).
    unsafe {
        reg_write32(GICD_BASE + gicd::CTLR, gicd_ctlr_bits::ENABLE_G1)
    };
    wait_not_set(GICD_BASE + gicd::CTLR, gicd_ctlr_bits::RWP);
}

// ==================== Redistributor + CPU interface (per CPU) ====================

/// Cached RD_base (alias VA) per cpu index; 0 = not yet located.
static RD_BASE_PER_CPU: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Locate this CPU's RD_base frame by matching GICR_TYPER.AffinityValue
/// against MPIDR. Frames walk 2*64KB apart until the Last marker.
fn find_my_rd_base() -> Option<usize> {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        let cached = RD_BASE_PER_CPU[cpu].load(Ordering::Acquire);
        if cached != 0 {
            return Some(cached as usize);
        }
    }

    let mpidr = crate::arch::smp::read_mpidr();
    let my_aff = (mpidr & 0xFF)
        | ((mpidr >> 8) & 0xFF) << 8
        | ((mpidr >> 16) & 0xFF) << 16
        | ((mpidr >> 32) & 0xFF) << 24;

    let mut frame = GICR_FRAMES;
    for _ in 0..GICR_MAX_FRAMES {
        let typer = unsafe { reg_read64(frame + gicr::TYPER) };
        let aff = (typer >> 32) as u64 & 0xFFFF_FFFF;
        if aff == my_aff {
            if cpu < crate::config::MAX_CPUS {
                RD_BASE_PER_CPU[cpu].store(frame as u64, Ordering::Release);
            }
            return Some(frame);
        }
        if typer & (1 << 4) != 0 {
            // GICR_TYPER.Last — no more frames in the region.
            break;
        }
        frame += GICR_STRIDE;
    }
    None
}

/// Configure this CPU's redistributor SGI/PPI bank: all INTID 0-31
/// Group 1, priorities set, everything but the IPI SGI + timer PPI
/// left disabled (enabled on demand by enable_ppi / request_irq).
fn redist_configure_local(rd: usize) {
    // Wake the frame if it slept (PSCI-off CPUs can re-enter sleep;
    // QEMU clears ProcessorSleep itself, the poll is for real HW).
    let waker = rd + gicr::WAKER;
    if unsafe { reg_read32(waker) } & gicr_waker_bits::PROCESSOR_SLEEP != 0 {
        unsafe { reg_write32(waker, 0) };
        for _ in 0..1_000_000 {
            if unsafe { reg_read32(waker) } & gicr_waker_bits::CHILDREN_ASLEEP == 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }

    // Group 1 for the whole SGI/PPI bank; keep SGIs + timer PPI enabled.
    // (IPRIORITYR packs four INTIDs per word — accumulate, see dist_init.)
    unsafe {
        reg_write32(rd + gicr::SGI_IGROUPR0, 0xFFFF_FFFF);
        reg_write32(rd + gicr::SGI_ICENABLER0, 0xFFFF_FFFF);
        for word_id in (0..32u32).step_by(4) {
            let mut word: u32 = 0;
            for lane in 0..4u32 {
                word |= (default_priority_for(word_id + lane) & 0xFF) << (8 * lane);
            }
            reg_write32(rd + gicr::SGI_IPRIORITYR + 4 * (word_id / 4) as usize, word);
        }
        let mut enable0: u32 = 0;
        for sgi in 0..16u32 {
            enable0 |= 1 << sgi; // SGIs always live (IPI)
        }
        enable0 |= 1 << TIMER_PPI_INTID; // arch timer
        reg_write32(rd + gicr::SGI_ISENABLER0, enable0);
    }
}

/// Enable the CPU interface system registers (ICC_*): SRE, PMR, group-1
/// enable. Called per CPU from the boot path (trap::enable_external_interrupt
/// / secondary_cpu_entry). Idempotent.
pub fn enable_cpu_interface() {
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
        if let Some(rd) = find_my_rd_base() {
            redist_configure_local(rd);
        }

        // 3. Unmask priorities and forward Group 1 to EL1.
        asm!("msr icc_pmr_el1, {}", in(reg) 0xFFu64, options(nomem, nostack));
        asm!("msr icc_igrpen1_el1, {}", in(reg) 1u64, options(nomem, nostack));
        asm!("isb", options(nomem, nostack));
    }
}

/// Enable a per-CPU interrupt (PPI/SGI INTID < 32) on THIS cpu's
/// redistributor frame. Idempotent.
pub fn enable_ppi(intid: u32) {
    if intid >= 32 {
        return;
    }
    if let Some(rd) = find_my_rd_base() {
        // SAFETY: set-alias write to this CPU's own frame.
        unsafe { reg_write32(rd + gicr::SGI_ISENABLER0, 1 << intid) };
    }
}

// ==================== IrqChip ops ====================

fn gic_mask(data: &IrqData) {
    let intid = data.hwirq;
    if intid < 32 {
        if let Some(rd) = find_my_rd_base() {
            // SAFETY: clear-alias write, own frame.
            unsafe { reg_write32(rd + gicr::SGI_ICENABLER0, 1 << intid) };
        }
    } else {
        // SAFETY: clear-alias write to the distributor.
        unsafe {
            reg_write32(
                GICD_BASE + gicd::ICENABLER + 4 * (intid / 32) as usize,
                1 << (intid % 32),
            )
        };
    }
}

fn gic_unmask(data: &IrqData) {
    let intid = data.hwirq;
    if intid < 32 {
        enable_ppi(intid);
    } else {
        // SAFETY: set-alias write to the distributor.
        unsafe {
            reg_write32(
                GICD_BASE + gicd::ISENABLER + 4 * (intid / 32) as usize,
                1 << (intid % 32),
            )
        };
    }
}

fn gic_eoi(data: &IrqData) {
    eoir1_write(data.hwirq);
}

static GIC_CHIP: IrqChip = IrqChip {
    name: "arm-gic-v3",
    irq_mask: Some(gic_mask),
    irq_unmask: Some(gic_unmask),
    irq_ack: None,
    irq_eoi: Some(gic_eoi),
    irq_set_type: None,
    irq_set_affinity: None,
};

fn gic_irq_map(_domain: &IrqDomain, _virq: u32, _hwirq: u32) -> i32 {
    0
}

fn gic_irq_unmap(_domain: &IrqDomain, _virq: u32) {}

static GIC_DOMAIN_OPS: IrqDomainOps = IrqDomainOps {
    map: Some(gic_irq_map),
    unmap: Some(gic_irq_unmap),
};

// ==================== Claim / EOI (arch trap path) ====================

/// Acknowledge the highest-priority pending Group-1 interrupt on this
/// CPU. None = spurious (ICC_IAR1 1023 or special IDs).
#[inline]
pub fn claim() -> Option<u32> {
    let intid = iar1_read();
    if intid >= INTID_SPURIOUS - 3 {
        // 1020-1023: spurious / special-purpose IDs — nothing to run.
        // (EOI for 1020-1022 is ICC_EOIR1 with the same value; with all
        // interrupts in Group 1 these cannot occur on this machine.)
        if intid == INTID_SPURIOUS {
            return None;
        }
        eoir1_write(intid);
        return None;
    }
    Some(intid)
}

/// End-of-interrupt for an INTID claimed via `claim` outside the IRQ
/// framework flow (timer PPI / IPI SGI special-cases, fallback EOI).
#[inline]
pub fn eoi(intid: u32) {
    eoir1_write(intid);
}

// ==================== Init ====================

pub fn init() {
    // 1. Distributor: configure + enable (single-CPU context, before
    //    start_secondaries).
    dist_init();

    // 2. Boot cpu's redistributor + CPU interface; secondaries repeat
    //    via secondary_cpu_entry → trap::enable_external_interrupt.
    enable_cpu_interface();

    // 3. IRQ domain: linear identity over INTID [0, DOMAIN_SIZE),
    //    pre-mapped like the PLIC twin.
    let domain = irq_domain_create_linear(
        &GIC_DOMAIN_OPS,
        DOMAIN_SIZE as usize,
        GICD_BASE,
        Some(&GIC_CHIP),
    );
    let limit = DIST_INTID_LIMIT.load(Ordering::Acquire).min(DOMAIN_SIZE);
    for intid in 32..limit {
        irq_create_mapping(domain, intid);
    }

    let lines = DIST_INTID_LIMIT.load(Ordering::Acquire);
    crate::print_status(
        "intc",
        &alloc::format!(
            "GICv3 @ {:#x} (dist {:#x}): {} INTIDs, timer PPI {} (INTID {}), IPI SGI {}",
            GICD_BASE,
            crate::arch::mm::memory_layout::GICR_BASE,
            lines,
            TIMER_PPI_INTID - 16,
            TIMER_PPI_INTID,
            IPI_SGI_INTID
        ),
        true,
    );
    crate::print_status("intc", "IRQ domain + chip registered", true);

}
