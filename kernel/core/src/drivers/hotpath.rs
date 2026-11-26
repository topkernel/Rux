//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The drivers subsystem's code (seed-core-design §2c.3): everything of the
//! subsystem that is neither `types.rs` nor an `unsafe.rs`, in one file.
//! (The module doc of the old drivers/mod.rs: Device driver module.)
//!
//! Sections follow the order of the old module declarations; each banner
//! names the old file it was and the file's module doc follows it as the
//! section's header comment. Inside a section the items keep their original
//! order. Type definitions are in `types.rs`. The `timer/` and `intc/`
//! directories are ONE inline module each: their per-architecture child
//! modules are chosen by `#[cfg]` on the module declaration, so nothing
//! inside them changed. The unsafe that is still scattered over the
//! sections is collected in pass 3 — the per-file `#![forbid(unsafe_code)]`
//! inner attributes the old files carried come back with it.
//!
//! The file's NAME overstates until the shrink stage has run: it holds all
//! of the subsystem's code under the hot-path name, not a verified set of
//! hot-path functions (§2c.3, user decision 2026-11-25).

use super::types::{ASHMEM_NAME_LEN, AreaInner, ArpHrdType, AshmemArea, AshmemPages, AshmemPin, AvailRing, BARType, BITS_LEN, BIT_TYPES, BlockDeviceManager, BlockDeviceOps, CmdResourceAttachBacking, CmdResourceCreate2d, CmdResourceFlush, CmdSetScanout, CmdTransferToHost2d, Desc, DeviceInit, DeviceStats, DiskStat, DisplayPmode, DmaSeg, DrmModeInfo, DrmState, Entry, Eth0Phase, EvdevDevice, FbObject, FrameBuffer, FrameBufferInfo, GemObject, GenDisk, GpuCtrlHeader, HuntLine, IFNAMSIZ, InitLevel, InputAbsinfo, InputEvent, InputId, IoBufFreelist, IttStage, LI_OFF_DEVICE, LI_OFF_ENCRYPT_TYPE, LI_OFF_FLAGS, LI_OFF_INIT, LI_OFF_INODE, LI_OFF_KEY_SIZE, LI_OFF_NAME, LI_OFF_NUMBER, LI_OFF_OFFSET, LI_OFF_RDEVICE, LOOP_INFO64_SIZE, LOOP_INFO_SIZE, LO_NAME_SIZE, LoopDev, LoopStatus, MemEntry, NetDevice, NetDeviceOps, NotifyWidth, PCIBAR, PCIConfig, PS2Keyboard, PS2Mouse, PciBlkGuard, PendingIo, PropDesc, Rect, ReqCmd, Request, RespDisplayInfo, RespNoData, UnpinRange, UsedElem, UsedRing, VirtIOBlkDevice, VirtIOBlkReqHeader, VirtIOBlkResp, VirtIOCapType, VirtIONetDevice, VirtIONetHdr, VirtIOPCI, VirtQueue, VirtioGpuDevice, VirtioInputDevice, VirtioInputEvent, VirtioLockNest, Vring};

extern crate alloc;

use alloc::string::ToString; // String moved to types.rs with DiskStat
use alloc::boxed::{Box};
use alloc::{format, vec};
use alloc::vec::{Vec};
use crate::sync::types::{Spinlock, SpinlockIrqGuard};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicUsize, Ordering, fence};
use alloc::collections::btree_map::{BTreeMap};
use crate::net::buffer::{SkBuff};
use core::ptr::{read_volatile, write_volatile};
use alloc::alloc::{Layout, alloc, alloc_zeroed, dealloc};
use alloc::sync::{Arc};
use alloc::collections::vec_deque::{VecDeque};
use alloc::string::{String};
use crate::fs::file::{File, FileOps};
use crate::fs::dev_t::{DEV_EVDEV_KEYBOARD, DEV_EVDEV_POINTER, DevNo, LOOP_MAJOR, MISC_MAJOR};
use crate::arch::mm::{PageTableEntry, map_user_page};
use crate::arch::mm::memory_layout::{PhysAddr as MmPhysAddr, VirtAddr as MmVirtAddr};
use crate::mm::hotpath::{PAGE_MASK, free_pages, get_zeroed_page};
use crate::mm::types::{GfpFlags, PAGE_SIZE, VmaFlags, VmaType};
// ════════════════ intc ════════════════════════════════
// (was the directory intc/: one inline module — its child
// modules are chosen by #[cfg] on the module declaration, so the
// files keep their text unchanged inside the braces)
// Interrupt controller driver
//
// Supports GICv3 (ARM64), PLIC (RISC-V64), and CLINT (RISC-V64)
pub mod intc {
#[cfg(feature = "aarch64")]

pub mod gicv3 {
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
//
// Round-5: the raw volatile words route through safety::mmio::MmioRegion
// (INV-MMIO-1: the GIC distributor/redistributor register files are
// mapped at the fixed aliases this module computes — see the module
// doc; every access below is a single word inside that mapping).

/// Access one u32 register by alias address (INV-MMIO-1/2/3).
#[inline]
fn reg_read32(addr: usize) -> u32 {
    crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0)
}

#[inline]
fn reg_write32(addr: usize, v: u32) {
    crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, v)
}

#[inline]
fn reg_read64(addr: usize) -> u64 {
    crate::safety::mmio::MmioRegion::new(addr, 8).read_u64(0)
}

#[inline]
fn reg_write64(addr: usize, v: u64) {
    crate::safety::mmio::MmioRegion::new(addr, 8).write_u64(0, v)
}

/// Wait for a RWP-style busy bit to clear (bounded; the emulated GIC
/// clears it in single-digit accesses).
fn wait_not_set(addr: usize, bit: u32) {
    for _ in 0..1_000_000 {
        // SAFETY: diagnostic poll of a GIC register previously written.
        if  reg_read32(addr)  & bit == 0 {
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
    let typer =  reg_read32(GICD_BASE + gicd::TYPER) ;
    let lines = ((typer & 0x1F) as u32 + 1) * 32;
    DIST_INTID_LIMIT.store(lines, Ordering::Release);

    // 1. Disable the distributor while reconfiguring.
     reg_write32(GICD_BASE + gicd::CTLR, 0) ;
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
         reg_write32(GICD_BASE + gicd::IPRIORITYR + word_id as usize, word) ;
    }
    for intid in 32..limit {
        
            reg_write64(
                GICD_BASE + gicd::IROUTER + 8 * (intid as usize - 32),
                boot_mpidr,
            );
        
    }
    for n in 1..(limit as usize + 31) / 32 {
        
            reg_write32(GICD_BASE + gicd::IGROUPR + 4 * n, 0xFFFF_FFFF);
            reg_write32(GICD_BASE + gicd::ICENABLER + 4 * n, 0xFFFF_FFFF);
        
    }

    // 3. Enable: Group-1 forwarding (bit 1 — see gicd_ctlr_bits; ARE is
    //    RAO/WI on this machine and needs no write).
    
        reg_write32(GICD_BASE + gicd::CTLR, gicd_ctlr_bits::ENABLE_G1)
    ;
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
        let typer =  reg_read64(frame + gicr::TYPER) ;
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
    if  reg_read32(waker)  & gicr_waker_bits::PROCESSOR_SLEEP != 0 {
         reg_write32(waker, 0) ;
        for _ in 0..1_000_000 {
            if  reg_read32(waker)  & gicr_waker_bits::CHILDREN_ASLEEP == 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }

    // Group 1 for the whole SGI/PPI bank; keep SGIs + timer PPI enabled.
    // (IPRIORITYR packs four INTIDs per word — accumulate, see dist_init.)
    
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

/// Enable the CPU interface system registers (ICC_*): SRE, PMR, group-1
/// enable. Called per CPU from the boot path (trap::enable_external_interrupt
/// / secondary_cpu_entry). Idempotent.
// IRREDUCIBLE F-ROW (F-GIC-A1): ICC_* system-register programming —
// reference kernel drivers/irqchip/irq-gic-v3.c gic_cpu_init; the
// GICv3 CPU-interface enable sequence is register-file access with no
// safe shape (one audited block per sequence).
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
         reg_write32(rd + gicr::SGI_ISENABLER0, 1 << intid) ;
    }
}

// ==================== IrqChip ops ====================

fn gic_mask(data: &IrqData) {
    let intid = data.hwirq;
    if intid < 32 {
        if let Some(rd) = find_my_rd_base() {
            // SAFETY: clear-alias write, own frame.
             reg_write32(rd + gicr::SGI_ICENABLER0, 1 << intid) ;
        }
    } else {
        // SAFETY: clear-alias write to the distributor.
        
            reg_write32(
                GICD_BASE + gicd::ICENABLER + 4 * (intid / 32) as usize,
                1 << (intid % 32),
            )
        ;
    }
}

fn gic_unmask(data: &IrqData) {
    let intid = data.hwirq;
    if intid < 32 {
        enable_ppi(intid);
    } else {
        // SAFETY: set-alias write to the distributor.
        
            reg_write32(
                GICD_BASE + gicd::ISENABLER + 4 * (intid / 32) as usize,
                1 << (intid % 32),
            )
        ;
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

}

#[cfg(feature = "riscv64")]

pub mod plic {
use core::arch::asm;
use crate::println;
use crate::interrupt::{
    IrqChip, IrqData, IrqDomainOps, IrqDomain,
    irq_domain_create_linear, irq_create_mapping,
};

// PLIC base address - QEMU virt platform uses 0x0c000000. Accessed via
// the kernel-half MMIO alias (mmio_alias): the identity low window is
// cloned into every user root where a MAP_FIXED user mapping (gjs cage
// at 0x0c000000) may replace it — the alias is user-unreachable.
const PLIC_BASE: usize =
    crate::arch::mm::memory_layout::mmio_alias(201326592) as usize; // alias of 0x0c000000

mod offset {
    pub const PRIORITY: usize = 0x000000;
    pub const PENDING: usize = 0x001000;
    pub const ENABLE_BASE: usize = 0x002000;
    pub const ENABLE_SIZE: usize = 0x80;        // per context
    pub const CONTEXT_BASE: usize = 0x200000;
    pub const CONTEXT_SIZE: usize = 0x1000;      // per context
    pub const CONTEXT_THRESHOLD: usize = 0x00;
    pub const CONTEXT_CLAIM: usize = 0x04;
}

/// Maximum number of interrupts - from config
pub const MAX_INTERRUPTS: usize = crate::config::PLIC_MAX_INTERRUPTS;

pub const PLIC_PRIORITY_BASE: u32 = 1;
pub const PLIC_PRIORITY_MIN: u32 = 0;
pub const PLIC_PRIORITY_MAX: u32 = 7;

/// S-mode context ID for a given hart (2 * hart + 1)
fn s_mode_ctx(hart: usize) -> usize {
    2 * hart + 1
}

/// Basic priority layering (review LINUX-DIFF: every IRQ used to get
/// priority 1, so the PLIC could never prefer the console over bulk device
/// work). QEMU virt IRQ map: 1..=8 are the virtio-mmio slots, 10 is the
/// UART console. Console/interactive sources outrank block/net throughput
/// sources; everything else stays at the base priority.
fn default_priority_for(irq: usize) -> u32 {
    match irq {
        10 => 5,            // UART console — keep interactive latency low
        1..=8 => 3,         // virtio-mmio (blk/net/input/gpu)
        _ => PLIC_PRIORITY_BASE,
    }
}

// ==================== PLIC hardware operations ====================

pub struct Plic {
    base: usize,
    num_harts: usize,
}

impl Plic {
    pub const fn new(base: usize, num_harts: usize) -> Self {
        Self { base, num_harts }
    }

    /// Initialize PLIC hardware: disable all IRQs, set thresholds to 0.
    pub fn init(&self) {
        // Disable all interrupts (priority = 0)
        for irq in 1..MAX_INTERRUPTS {
            self.set_priority(irq, 0);
        }

        // Set threshold = 0 for all S-mode contexts and clear enables
        for hart in 0..self.num_harts {
            let ctx = s_mode_ctx(hart);

            // Clear all enable bits for this S-mode context
            for word in 0..((MAX_INTERRUPTS + 31) / 32) {
                let addr = self.base + offset::ENABLE_BASE + ctx * offset::ENABLE_SIZE + word * 4;
                // INV-MMIO-1: mapped PLIC enable register for this
                // S-mode context (MmioRegion owns the volatile word).
                crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, 0);
            }

            // Set threshold = 0 for this S-mode context
            self.set_threshold(hart, 0);
        }
    }

    fn set_priority(&self, irq: usize, priority: u32) {
        let addr = self.base + offset::PRIORITY + irq * 4;
        // INV-MMIO-1: mapped PLIC priority register (PLIC_BASE + 4*irq).
        crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, priority);
    }

    fn set_threshold(&self, hart: usize, threshold: u32) {
        let ctx = s_mode_ctx(hart);
        let addr = self.base + offset::CONTEXT_BASE + ctx * offset::CONTEXT_SIZE + offset::CONTEXT_THRESHOLD;
        // INV-MMIO-1: mapped PLIC threshold register for this context.
        crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, threshold);
    }

    /// Enable an interrupt for a given hart.
    ///
    /// NOTE: The read-modify-write on the enable register is not atomic.
    /// Safe under current single-hart-per-context setup (each hart has its
    /// own enable register set). For SMP where multiple harts share a
    /// context, this needs AMO (`amoadd.w`) or a spinlock.
    pub fn enable_interrupt(&self, hart: usize, irq: usize) {
        let _rmw = ENABLE_RMW_LOCK.lock();
        self.set_priority(irq, default_priority_for(irq));
        let ctx = s_mode_ctx(hart);
        let word = irq / 32;
        let bit = irq % 32;
        let addr = self.base + offset::ENABLE_BASE + ctx * offset::ENABLE_SIZE + word * 4;
        // INV-MMIO-1: mapped PLIC enable register for this S-mode
        // context. Each hart has its own enable context, so no
        // cross-hart race in the current configuration.
        let reg = crate::safety::mmio::MmioRegion::new(addr, 4);
        let value = reg.read_u32(0);
        reg.write_u32(0, value | (1 << bit));
    }

    fn disable_interrupt(&self, hart: usize, irq: usize) {
        let _rmw = ENABLE_RMW_LOCK.lock();
        let ctx = s_mode_ctx(hart);
        let word = irq / 32;
        let bit = irq % 32;
        let addr = self.base + offset::ENABLE_BASE + ctx * offset::ENABLE_SIZE + word * 4;
        // INV-MMIO-1: mapped PLIC enable register; RMW clears one bit.
        let reg = crate::safety::mmio::MmioRegion::new(addr, 4);
        let value = reg.read_u32(0);
        reg.write_u32(0, value & !(1 << bit));
    }

    pub fn claim(&self, hart: usize) -> Option<usize> {
        let ctx = s_mode_ctx(hart);
        let addr = self.base + offset::CONTEXT_BASE + ctx * offset::CONTEXT_SIZE + offset::CONTEXT_CLAIM;
        // INV-MMIO-1: mapped PLIC claim/complete register for this context.
        let irq = crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0);
        if irq == 0 { None } else { Some(irq as usize) }
    }

    pub fn complete(&self, hart: usize, irq: usize) {
        let ctx = s_mode_ctx(hart);
        let addr = self.base + offset::CONTEXT_BASE + ctx * offset::CONTEXT_SIZE + offset::CONTEXT_CLAIM;
        // INV-MMIO-1: writing irq completes the claim.
        crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, irq as u32);
    }

    /// Read one word of the PLIC pending array.
    ///
    /// The pending array at offset::PENDING has one 32-bit word per 32
    /// IRQs (MAX_INTERRUPTS/32 words total). The old `read_pending` only
    /// ever read word 0, so IRQs >= 32 were invisible to any diagnostics
    /// (review BUG: read_pending 全字).
    pub fn read_pending_word(&self, word: usize) -> u32 {
        let words = (MAX_INTERRUPTS + 31) / 32;
        if word >= words {
            return 0;
        }
        let addr = self.base + offset::PENDING + word * 4;
        // INV-MMIO-1: mapped PLIC pending register (read-only).
        crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0)
    }

    /// Read the FIRST pending word (IRQs 0..31). See read_pending_word for
    /// the full array.
    pub fn read_pending(&self) -> u32 {
        self.read_pending_word(0)
    }

    pub fn trigger_ipi(&self, irq: usize) {
        if irq >= 32 { return; }
        let addr = self.base + offset::PENDING;
        // INV-MMIO-1: mapped PLIC pending register; RMW sets a pending bit.
        let pending = crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0);
        crate::safety::mmio::MmioRegion::new(addr, 4).write_u32(0, pending | (1 << irq));
    }
}

/// R14-15 (MED-11): PLIC enable-word read-modify-writes from different
/// CPUs lose updates (one IRQ silently left enabled/disabled). All RMW
/// on enable words goes through this lock.
static ENABLE_RMW_LOCK: crate::sync::types::Spinlock<()> =
    crate::sync::types::Spinlock::new(());

static PLIC: Plic = Plic::new(PLIC_BASE, crate::config::MAX_CPUS);

// ==================== IrqChip implementation ====================

fn plic_mask(data: &IrqData) {
    for hart in 0..crate::config::MAX_CPUS {
        PLIC.disable_interrupt(hart, data.hwirq as usize);
    }
}

fn plic_unmask(data: &IrqData) {
    // Multi-core enable (review LINUX-DIFF): the old code enabled the IRQ
    // on the boot hart ONLY, serially funneling every external interrupt
    // onto hart 0 and starving the other CPUs. PLIC claim semantics make
    // multi-hart enable safe: when the IRQ fires, every enabled hart traps
    // and claims, the hardware hands the IRQ to exactly ONE claimer and
    // returns 0 to the rest (the old comment's "all claim the same IRQ and
    // deadlock on the action lock" scenario cannot happen) — racing claims
    // are precisely how interrupt load spreads across CPUs.
    // Enabling an offline hart's context is harmless (it never claims).
    for hart in 0..crate::config::MAX_CPUS {
        PLIC.enable_interrupt(hart, data.hwirq as usize);
    }
}

fn plic_eoi(data: &IrqData) {
    let hart = crate::arch::smp::cpu_id() as usize;
    PLIC.complete(hart, data.hwirq as usize);
}

/// Post-SMP-bringup delivery restore (the VW-EXT4LOCK wedge root cause).
///
/// OpenSBI's per-hart warm init reprograms every secondary's S-mode PLIC
/// context as that hart wakes (threshold raised to max, enable bits
/// cleared), clobbering the kernel's early-init state on every context
/// except the boot hart's — the boot hart never passes through warm init,
/// so its context alone keeps the enables the kernel's request_irq
/// unmask had set on ALL contexts. Whether external IRQs deliver at all
/// after SMP online then depends on the boot hart's own context: with
/// num_harts hardcoded to 4, a boot hart of 4..7 kept its firmware
/// residue threshold=7 — ZERO external interrupts delivered anywhere
/// (virtio-blk completions AND UART RX). Every async block I/O then
/// waited to its 10 s deadline (the only async harvest fallback), the
/// shell held EXT4_BIG_LOCK across those waits, fdreapd D-parked behind
/// it, and the fed boot script never reached the shell (UART RX IRQ
/// dead): the VW-EXT4LOCK fdreapd-stall hang face, ~4/8 of boots on
/// -smp 8 cells (never on -smp 2: harts 0/1 are always init-covered).
///
/// Reference parity: Linux's sifive-plic driver re-establishes per-CPU
/// enable state through its cpu-online notifier
/// (irq_sifive_plic_cpu_starting). We do the same in one shot: when the
/// bringup window closes, zero every S-mode context's threshold and
/// re-apply the unmask of every registered, enabled IRQ. Idempotent;
/// runs once on the boot hart before userspace starts.
pub fn after_smp_online_restore() {
    for hart in 0..crate::config::MAX_CPUS {
        PLIC.set_threshold(hart, 0);
    }
    let rearmed = crate::interrupt::irqdesc::remask_registered_irqs();
    crate::pr_info!(
        "intc: post-SMP PLIC restore: thresholds=0 on {} S-mode contexts, {} IRQs re-enabled",
        crate::config::MAX_CPUS,
        rearmed
    );
}

/// PLIC irq_chip (function-pointer-table pattern)
static PLIC_CHIP: IrqChip = IrqChip {
    name: "riscv-plic",
    irq_mask: Some(plic_mask),
    irq_unmask: Some(plic_unmask),
    irq_ack: None,
    irq_eoi: Some(plic_eoi),
    irq_set_type: None,
    irq_set_affinity: None,
};

// ==================== IrqDomainOps implementation ====================

fn plic_irq_map(_domain: &IrqDomain, _virq: u32, _hwirq: u32) -> i32 {
    // chip is already set by irq_create_mapping
    0
}

fn plic_irq_unmap(_domain: &IrqDomain, _virq: u32) {
    // Nothing to do
}

static PLIC_DOMAIN_OPS: IrqDomainOps = IrqDomainOps {
    map: Some(plic_irq_map),
    unmap: Some(plic_irq_unmap),
};

// ==================== Public API ====================

/// Initialize PLIC and create the IRQ domain.
pub fn init() {
    // 1. Initialize PLIC hardware
    PLIC.init();

    // 2. Create the PLIC IRQ domain (linear, 1:1 mapping)
    let domain = irq_domain_create_linear(
        &PLIC_DOMAIN_OPS,
        MAX_INTERRUPTS,
        PLIC_BASE,
        Some(&PLIC_CHIP),
    );

    // 3. Pre-map all PLIC IRQs (1:1 identity mapping)
    for hwirq in 1..MAX_INTERRUPTS {
        irq_create_mapping(domain, hwirq as u32);
    }

    // 4. Enable IPI interrupts for all harts (S-mode contexts)
    for hart in 0..crate::config::MAX_CPUS {
        for ipi_irq in 11..14 {
            PLIC.enable_interrupt(hart, ipi_irq);
        }
    }
}

pub fn claim(hart: usize) -> Option<usize> {
    PLIC.claim(hart)
}

pub fn complete(hart: usize, irq: usize) {
    PLIC.complete(hart, irq)
}

pub fn enable_interrupt(hart: usize, irq: usize) {
    PLIC.enable_interrupt(hart, irq)
}

pub fn read_pending() -> u32 {
    PLIC.read_pending()
}

pub fn trigger_ipi(irq: usize) {
    PLIC.trigger_ipi(irq)
}

/// VW forensic snapshot: print the PLIC delivery state (VW-PLIC) — the
/// pending array, each S-mode context's enable words, and its threshold.
/// Read-only MMIO, safe to call from watchdog/deadline contexts. Answers
/// the "delivery vs assertion" question for a dead device IRQ: a pending
/// bit that stays set with zero handler entries names a delivery break
/// (enable/threshold); a clear pending bit means the device never
/// asserted (driver/device side).
pub fn vw_plic_dump() {
    let words = (MAX_INTERRUPTS + 31) / 32;
    // Pending array
    let mut pend = [0u32; 16];
    let n = words.min(pend.len());
    for w in 0..n {
        pend[w] = PLIC.read_pending_word(w);
    }
    crate::pr_err!(
        "VW-PLIC pending={:08x}{:08x} harts(maxcpus)={}",
        (pend[1] & 0xfffffffe) | ((pend[0] >> 31) & 1), // IRQs 32..62 (33=blk,32..47=gpex) compacted
        pend[0] & 0x7fffffff,
        crate::config::MAX_CPUS
    );
    // Enable words + threshold per S-mode context
    for hart in 0..crate::config::MAX_CPUS {
        let ctx = s_mode_ctx(hart);
        let addr = self_addr_enable(ctx, 0);
        let addr1 = self_addr_enable(ctx, 1);
        // INV-MMIO-1: mapped PLIC enable register reads (diagnostic).
        let en0 = crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0);
        let en1 = crate::safety::mmio::MmioRegion::new(addr1, 4).read_u32(0);
        let th = read_threshold(ctx);
        crate::pr_err!(
            "VW-PLIC ctx={} (hart {}) enable[0]={:08x} enable[1]={:08x} threshold={}",
            ctx,
            hart,
            en0,
            en1,
            th
        );
    }
}

fn self_addr_enable(ctx: usize, word: usize) -> usize {
    PLIC_BASE + offset::ENABLE_BASE + ctx * offset::ENABLE_SIZE + word * 4
}

fn read_threshold(ctx: usize) -> u32 {
    let addr = PLIC_BASE
        + offset::CONTEXT_BASE
        + ctx * offset::CONTEXT_SIZE
        + offset::CONTEXT_THRESHOLD;
    // INV-MMIO-1: mapped PLIC threshold register read (diagnostic).
    crate::safety::mmio::MmioRegion::new(addr, 4).read_u32(0)
}

}

#[cfg(feature = "riscv64")]

pub mod clint {
use core::sync::atomic::{AtomicU32, Ordering};
use crate::sbi;

// IPI count per hart — must match config::MAX_CPUS
const NUM_HARTS: usize = crate::config::MAX_CPUS;
// Review CONS: the literal 4-element initializer (and the hardcoded `>= 4`
// bounds checks below) silently truncated/broke if MAX_CPUS ever changed;
// a replication expression keeps the array sized by the single source of
// truth (and would fail to compile with a fixed literal list).
static IPI_COUNT: [AtomicU32; NUM_HARTS] = [const { AtomicU32::new(0) }; NUM_HARTS];

/// Initialize CLINT driver
///
/// Note: Modern systems do not need direct access to CLINT registers
/// SBI firmware manages CLINT, S-mode accesses through SBI calls
pub fn init() {
    // SBI system automatically manages CLINT
    // No S-mode software initialization needed
    // Clear IPI counters
    for hart in 0..NUM_HARTS {
        IPI_COUNT[hart].store(0, Ordering::Relaxed);
    }
}

/// Send IPI to specified hart
///
/// Uses SBI IPI Extension (EID #0x735049)
///
/// # Parameters
/// * `target_hart` - Target hart ID (0-3)
pub fn send_ipi(target_hart: usize) {
    if target_hart >= NUM_HARTS {
        return;
    }

    // Send IPI via SBI (not direct CLINT MSIP register access)
    if sbi::send_ipi(target_hart) {
        // Update counter
        IPI_COUNT[target_hart].fetch_add(1, Ordering::Relaxed);
    }
}

/// Clear IPI for specified hart
///
/// Note: When using SBI, IPI clearing is handled automatically by SBI firmware
/// S-mode software does not need to manually clear MSIP register
///
/// # Parameters
/// * `hart` - Hart ID
pub fn clear_ipi(hart: usize) {
    if hart >= NUM_HARTS {
        return;
    }

    // SBI system automatically clears IPI
    // In software interrupt handler, SBI automatically clears pending state
    // No S-mode software manual clearing needed

    // Optional: Clear counter (if needed)
    // IPI_COUNT[hart].store(0, Ordering::Relaxed);
}

/// Get number of IPIs sent to specified hart
///
/// # Parameters
/// * `hart` - Hart ID
///
/// # Returns
/// IPI count
pub fn get_ipi_count(hart: usize) -> u32 {
    if hart < NUM_HARTS {
        IPI_COUNT[hart].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Read system time (time CSR)
///
/// Uses RISC-V `rdtime` instruction to read time
///
/// # Returns
/// Current time (cycles)
pub fn read_time() -> u64 {
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

/// Set timer compare value
///
/// Uses SBI TIMER Extension's set_timer function
///
/// # Parameters
/// * `hart` - Hart ID (note: set_timer is per-hart)
/// * `value` - Timer compare value (absolute time)
pub fn set_timecmp(_hart: usize, value: u64) {
    // Use SBI set_timer
    // Note: SBI's set_timer is per-hart, automatically applies to current hart
    // SAFETY: sbi_rt::set_timer is an SBI ecall with no special preconditions;
    // the `value` is an absolute timer deadline (u64), always valid.
    unsafe {
        sbi_rt::set_timer(value);
    }
}

}

#[cfg(feature = "x86_64")]

pub mod apic {
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

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
// xAPIC ISR/IRR bits 255:224 (SP-5 r5 trap: vector 0xE0 lives there)
const XAPIC_ISR7: u32 = 0x170;
const XAPIC_IRR7: u32 = 0x270;

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
// x2APIC ISR/IRR bits 255:224 (SP-5 r5 trap: vector 0xE0 lives there)
const X2APIC_ISR7: u32 = 0x817;
const X2APIC_IRR7: u32 = 0x827;

/// Spurious-interrupt vector (SVR): must match the IDT catch-all.
const SPURIOUS_VECTOR: u32 = 0xFF;

/// LAPIC timer divide configuration: divide by 16 (encoding 0x3).
const TIMER_DIV_16: u64 = 0x3;

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

/// RT-R4: per-CPU "the timer LVT/divide are already programmed for
/// one-shot at our vector" — lets timer_arm_oneshot skip the two mode
/// WRMSRs (VM exits) on re-arms, keeping the `now`-read-to-TMICT gap at
/// one exit (see timer_arm_oneshot). Cleared whenever another op
/// reprograms the LVT (periodic start, mask).
static ONESHOT_LVT_LIVE: [core::sync::atomic::AtomicBool; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicBool::new(false) }; crate::config::MAX_CPUS];

/// Invalidate the per-CPU one-shot-LVT cache (called by the ops that
/// reprogram the LVT away from plain one-shot).
fn ONESHOT_LVT_LIVE_INVALIDATE() {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        ONESHOT_LVT_LIVE[cpu].store(false, Ordering::Release);
    }
}
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

/// Write a LAPIC register in whichever mode is active.
#[inline]
fn reg_write(reg_xapic: u32, msr_x2apic: u32, value: u32) {
    if X2APIC.load(Ordering::Acquire) {
        // SAFETY: x2APIC MSR write; READY gates first use.
        unsafe { wrmsr(msr_x2apic, value as u64) };
    } else {
        let va = crate::arch::mm::memory_layout::device_alias(
            crate::arch::mm::memory_layout::LAPIC_BASE as u64,
        ) as usize + reg_xapic as usize;
        // SAFETY: MMIO write to the local APIC window (kernel-half alias).
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
///
/// RT-2X item 4: emits EXACTLY the reference's x2APIC ICR shape for a
/// FIXED unicast — `((dest) << 32) | vector`, no trigger-mode bits
/// (bit 14 is meaningless for FIXED edge delivery; the reference never
/// sets it). When the hypervisor advertises KVM_FEATURE_PV_SEND_IPI
/// the FIXED path takes the paravirt hypercall instead — the same
/// `kvm_send_ipi_mask` replacement the reference kernel logs at boot
/// ("kvm-guest: APIC: send_IPI_mask() replaced") — measured to be the
/// reference's ~10x-cheaper IPI send+delivery leg on this AMD/KVM host
/// (rtbench rt_wakesys / rt_preempt, run/RT-2X.md).
pub fn send_ipi(dest: u32, vector: u8) {
    // RT-2X: the PV hypercall path is opt-in via `ipi=pv` — measured
    // NOT faster than the raw ICR write on this host's KVM (both ride
    // the same exit; rt_wakesys 15.0 vs 13.1 us, run/RT-2X.md), but the
    // knob stays for hosts where PV delivery differs and for A/B.
    // Memoized: one cmdline resolution ever, then a byte load per IPI.
    match KVM_PV_IPI_ARMED.load(Ordering::Acquire) {
        0 => {
            let on = KVM_PV_IPI.load(Ordering::Acquire)
                && crate::cmdline::get_param("ipi").as_deref() == Some("pv");
            KVM_PV_IPI_ARMED.store(if on { 1 } else { 2 }, Ordering::Release);
            if on && kvm_pv_send_ipi(dest, vector) {
                return;
            }
        }
        1 => {
            if kvm_pv_send_ipi(dest, vector) {
                return;
            }
        }
        _ => {}
    }
    let cmd = vector as u32;
    send_icr(dest, cmd);
}

// ---------------------------------------------------------------------------
// KVM paravirt IPI (KVM_FEATURE_PV_SEND_IPI, the reference's
// kvm_send_ipi_mask — RT-2X item 4)
// ---------------------------------------------------------------------------

/// Hypercall number (uapi linux/kvm_para.h; stable since introduction —
/// the host KVM interprets this, and 10 has been KVM_HC_SEND_IPI since
/// the 4.16 merge).
const KVM_HC_SEND_IPI: u64 = 10;
/// CPUID leaf carrying KVM_FEATURE_* bits.
const KVM_CPUID_FEATURES: u32 = 0x4000_0001;
/// Bit 11: PV_SEND_IPI.
const KVM_FEATURE_PV_SEND_IPI: u32 = 1 << 11;

/// PV_SEND_IPI negotiated with the hypervisor (set once at init).
static KVM_PV_IPI: AtomicBool = AtomicBool::new(false);
/// Runtime USE of the PV path (0 = undecided, 1 = on, 2 = off) — the
/// `ipi=pv` cmdline knob, memoized on first send (a per-IPI cmdline
/// scan would cost more than the IPI).
static KVM_PV_IPI_ARMED: AtomicU8 = AtomicU8::new(0);
/// Use VMMCALL (SVM) vs VMCALL (VMX) — chosen by the physical CPU
/// vendor at init (KVM intercepts the one its hardware defines).
static KVM_HC_VMMCALL: AtomicBool = AtomicBool::new(false);

/// Probe CPUID for the KVM signature + PV_SEND_IPI feature bit. Called
/// once from `init()` (BSP context, before any IPI is needed).
pub fn kvm_para_init() {
    // Leaf 0x40000000: max leaf + hypervisor signature. KVM's is
    // "KVMKVMKVM" in ebx/ecx/edx.
    let (max_leaf, b, c, d) = crate::arch::x86_64::cpu::cpuid(0x4000_0000, 0);
    if max_leaf < KVM_CPUID_FEATURES || b != 0x4b4d564b || c != 0x564b4d56 || d != 0x4d {
        return; // not KVM (or ancient): keep the bare ICR path
    }
    // KVM_FEATURE_* bits live in EAX of the features leaf.
    let (feat, _, _, _) = crate::arch::x86_64::cpu::cpuid(KVM_CPUID_FEATURES, 0);
    if feat & KVM_FEATURE_PV_SEND_IPI == 0 {
        return;
    }
    // Hypercall instruction: SVM guests must VMMCALL, VMX guests VMCALL.
    // CPUID leaf 0 vendor is the EBX:EDX:ECX convention — measured on
    // this rig: ebx="Auth"(0x68747541), edx="enti"(0x69746e65),
    // ecx="cAMD"(0x444d4163).
    let (_, vb, vc, vd) = crate::arch::x86_64::cpu::cpuid(0, 0);
    let amd = vb == 0x6874_7541 && vd == 0x6974_6e65 && vc == 0x444d_4163;
    KVM_HC_VMMCALL.store(amd, Ordering::Release);
    KVM_PV_IPI.store(true, Ordering::Release);
}

/// One FIXED unicast through the PV hypercall. Returns false (caller
/// falls back to the ICR write) when not negotiated or the hypervisor
/// refuses. The register convention is the KVM pv hypercall ABI:
/// rax=nr, rbx=a1, rcx=a2, rdx=a3, rsi=a4, return rax (0 on success,
/// negative -KVM_E* on refusal).
fn kvm_pv_send_ipi(dest: u32, vector: u8) -> bool {
    if !KVM_PV_IPI.load(Ordering::Acquire) {
        return false;
    }
    // IRREDUCIBLE F-ROW (F-HVCALL-X1): the KVM hypercall ABI
    // (vmmcall/vmcall with the RBX-shuttle push/mov/pop bracket) —
    // reference kernel arch/x86/kernel/kvm.c __send_ipi_mask's hc()
    // twin; a register protocol with no memory operands has no safe
    // shape. The bitmap is a single-bit cluster with min=dest
    // (bitmap_low=1, bitmap_high=0).
    unsafe fn hc(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, vmmcall: bool) -> i64 {
        let ret: i64;
        // The KVM ABI passes arg1 in RBX, which LLVM reserves — shuttle
        // it through any free GPR with an explicit push/mov/pop bracket
        // (stack-neutral; that is why nostack is not claimed).
        if vmmcall {
            core::arch::asm!(
                "push rbx",
                "mov rbx, {b}",
                "vmmcall",
                "pop rbx",
                b = in(reg) a1,
                in("rax") nr, in("rcx") a2, in("rdx") a3, in("rsi") a4,
                lateout("rax") ret,
            );
        } else {
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
        ret
    }
    let vmmcall = KVM_HC_VMMCALL.load(Ordering::Acquire);
    // SAFETY: register-only protocol, see hc.
    let ret = unsafe { hc(KVM_HC_SEND_IPI, 1, 0, dest as u64, vector as u64, vmmcall) };
    ret == 0
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
    // WT4 (vsync-starvation hunt, 2026-10-19): the old expression divided
    // by TIMER_DIVISOR (16) AGAIN — but LAPIC_TIMER_FREQ is the CALIBRATED
    // CCR DECREMENT RATE, already measured with the divide configuration
    // armed (and in x2APIC mode, where this kernel runs, the divide
    // register is ignored entirely). The double division programmed the
    // initial count 16x too small: the periodic tick ran at ~1550 Hz per
    // CPU instead of HZ=100, and every tick raised Timer+Hrtimer softirqs
    // — wakewatch measured 3100 softirq passes/s, si_busy growing from
    // 12% to >90% of both CPUs as the timer wheel and TCP lists grew,
    // until task scheduling starved completely (picks frozen at ~60 s:
    // the compose-content 41.5 s dead stop / 0.5-0.7 s usleep blowouts
    // and the appspawn sleeps-through-socket-events shape).
    let ticks = freq / (crate::config::KERNEL_HZ as u64);
    if ticks == 0 {
        return false;
    }
    reg_write(XAPIC_TIMER_DIV, X2APIC_TIMER_DIV, TIMER_DIV_16 as u32);
    reg_write(XAPIC_TIMER_INIT, X2APIC_TIMER_INIT, ticks as u32);
    let lvt = (LOCAL_TIMER_VECTOR as u32) | LVT_TIMER_PERIODIC;
    reg_write(XAPIC_TIMER_LVT, X2APIC_TIMER_LVT, lvt);
    // RT-R4: the LVT now carries PERIODIC — the one-shot fast arm's
    // cached "LVT already one-shot" state is stale.
    ONESHOT_LVT_LIVE_INVALIDATE();
    timer_event_record(
        crate::arch::cpu_id() as usize,
        TIMER_OP_PERIODIC,
        0,
        ticks as u32,
    );
    true
}

/// Mask the LAPIC timer on this CPU.
pub fn timer_mask() {
    if READY.load(Ordering::Acquire) {
        let lvt = (LOCAL_TIMER_VECTOR as u32) | LVT_TIMER_PERIODIC | LVT_MASKED;
        reg_write(XAPIC_TIMER_LVT, X2APIC_TIMER_LVT, lvt);
        ONESHOT_LVT_LIVE_INVALIDATE();
        timer_event_record(crate::arch::cpu_id() as usize, TIMER_OP_MASK, 0, 0);
    }
}

/// Arm the LAPIC timer on this CPU in ONE-SHOT mode to fire at an
/// absolute nominal-domain deadline (TIMER_CLOCK_FREQ_HZ ticks).
///
/// Perf lane pd (2026-10-31): the periodic-only timer made every
/// high-resolution deadline (nanosleep/clock_nanosleep hres entries)
/// wait for the next 10 ms grid tick — on SMP the sleeper woke at the
/// FIRST of either CPU's ticks (pbench p7_sleep1ms med 1103 µs vs the
/// reference kernel's 16.6 µs, 66x). The riscv64 twin arms
/// min(next grid tick, earliest hres) via SBI set_timer; this is the
/// x86_64 mirror: `set_next_trigger()` computes that deadline and this
/// function programs it. After the switch, EVERY LOCAL_TIMER IRQ must
/// re-arm (`set_next_trigger()` from the tick handler) or this CPU's
/// tick stops entirely — a one-shot LAPIC fires once and stays quiet.
///
/// The deadline is in the read_time() (nominal 10 MHz) domain; the
/// calibrated CCR decrement rate converts it to an initial count. A
/// deadline at or before "now" clamps to the minimum count (fires
/// immediately; the Hrtimer softirq sweep then delivers the expiry).
/// Returns true when armed (APIC ready + calibration ran).
pub fn timer_arm_oneshot(deadline_nominal: u64) -> bool {
    if !READY.load(Ordering::Acquire) {
        return false;
    }
    let freq = LAPIC_TIMER_FREQ.load(Ordering::Acquire);
    if freq == 0 {
        return false; // calibration never ran: caller keeps periodic
    }
    // RT-R4 (R2 residual, fire side): program the MODE registers first,
    // then read `now` as late as possible — the TMICT write STARTS the
    // countdown, so every VM exit between the `now` read and the TMICT
    // write is un-announced dead time the fire pays back as lateness
    // (each x2APIC WRMSR is a VM exit, µs-class under load). The old
    // order (now → DIV write → LVT write → TMICT) started every countdown
    // ~2 exits late on EVERY arm; with the redundant DIV/LVT writes
    // skipped on re-arm (ONESHOT_LVT_LIVE below) the gap is one exit.
    // Divide register: ignored in x2APIC, calibrated-in for xAPIC — write
    // only on the first arm (or after a periodic/mask re-program).
    let cpu = crate::arch::cpu_id() as usize;
    let live = cpu < crate::config::MAX_CPUS
        && ONESHOT_LVT_LIVE[cpu].swap(true, Ordering::AcqRel);
    if !live {
        reg_write(XAPIC_TIMER_DIV, X2APIC_TIMER_DIV, TIMER_DIV_16 as u32);
        let lvt = LOCAL_TIMER_VECTOR as u32; // mode bits [18:17] = 0: one-shot
        reg_write(XAPIC_TIMER_LVT, X2APIC_TIMER_LVT, lvt);
    }
    let now = crate::drivers::hotpath::timer::read_time();
    let delta = deadline_nominal.saturating_sub(now).max(1);
    // counts = delta_seconds x freq, in u128 (delta up to a grid tick is
    // tiny, but a far-future hres deadline must not wrap).
    let counts = ((delta as u128) * (freq as u128)
        / crate::config::TIMER_CLOCK_FREQ_HZ as u128) as u64;
    let counts = counts.clamp(1, u32::MAX as u64) as u32;
    // Writing TMICT (re)starts the count.
    reg_write(XAPIC_TIMER_INIT, X2APIC_TIMER_INIT, counts);
    // SP-5 r5 arm trail: who programmed this CPU's timer last, with what.
    // r6: the recording (scalar fields + freeze ring) is unified in
    // timer_event_record.
    timer_event_record(
        crate::arch::cpu_id() as usize,
        TIMER_OP_ONESHOT,
        deadline_nominal,
        counts,
    );
    true
}

// ---------------------------------------------------------------------------
// SP-5 round 6 (LTP r24 kill10 handoff): freeze-face arm/service ring
// ---------------------------------------------------------------------------
// The both-CPU interrupt-freeze face leaves NOTHING executing in-guest —
// every print-based instrument (ticktrap included) is dead at the exact
// moment it is needed. This ring keeps the last TICK_ARM_RING_SLOTS
// timer events per CPU in plain memory; init() publishes its physical
// address so the HOST can dump it through the QEMU monitor after a
// freeze (HMP `xp/<n>x <phys>` — no guest involvement; NMI proved it
// does not resume the machine, k10-rux-nmi1). Ops 1-3 come from the
// programming sites below, TIMER_OP_SERVICE from the LOCAL_TIMER IRQ
// entry: the service->arm->service heartbeat is legible in the dump —
// a ring ending on an ARM entry with no following SERVICE = the timer
// was programmed but never fired (delivery loss); ending on a SERVICE
// with no re-arm = the nobody-re-arms face (a) of the r5 trap header.
pub const TIMER_OP_SERVICE: u32 = 4;
pub const TICK_ARM_RING_SLOTS: usize = 16;

/// One timer programming/service event (atomics-per-field, the
/// page_desc TEXT_EVT_RING pattern — no locks on the tick path).
pub struct TickArmEvent {
    /// TIMER_ARM_SEQ value at this event (monotone per CPU).
    pub seq: AtomicU64,
    /// read_time() at the event.
    pub tb: AtomicU64,
    /// Return address one frame above the recording site (frame
    /// pointers are forced on) — symbolize with addr2line.
    pub ra: AtomicU64,
    /// One-shot deadline (nominal ticks); 0 for service/mask/periodic.
    pub deadline: AtomicU64,
    /// Initial count written to TMICT.
    pub counts: AtomicU32,
    /// TIMER_OP_*.
    pub op: AtomicU32,
}
const fn tick_arm_new() -> TickArmEvent {
    TickArmEvent {
        seq: AtomicU64::new(0),
        tb: AtomicU64::new(0),
        ra: AtomicU64::new(0),
        deadline: AtomicU64::new(0),
        counts: AtomicU32::new(0),
        op: AtomicU32::new(0),
    }
}
const TICK_ARM_NEW: TickArmEvent = tick_arm_new();
const TICK_ARM_ROW: [TickArmEvent; TICK_ARM_RING_SLOTS] =
    [TICK_ARM_NEW; TICK_ARM_RING_SLOTS];
pub static TICK_ARM_RING: [[TickArmEvent; TICK_ARM_RING_SLOTS]; crate::config::MAX_CPUS] =
    [TICK_ARM_ROW; crate::config::MAX_CPUS];
pub static TICK_ARM_IDX: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

#[inline(always)]
fn tick_arm_ra() -> u64 {
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

/// Record one timer event: refresh the scalar last-* fields the TICKTRAP
/// line reads AND append to the freeze ring (single SEQ bump per event).
pub fn timer_event_record(cpu: usize, op: u32, deadline: u64, counts: u32) {
    if cpu >= crate::config::MAX_CPUS {
        return;
    }
    let seq = TIMER_ARM_SEQ[cpu].fetch_add(1, Ordering::Relaxed) + 1;
    let tb = crate::drivers::hotpath::timer::read_time();
    TIMER_ARM_OP[cpu].store(op, Ordering::Relaxed);
    // rt-r4: a SERVICE stamp (IRQ entry heartbeat) does not program the
    // timer — writing its 0 deadline would erase the arm the CURRENT fire
    // should be judged against (last_arm_deadline, the CYC-FIRE-LATE
    // armdl discriminator). Only real programming ops refresh the scalar;
    // the ring entry below still records the raw event.
    if op != TIMER_OP_SERVICE {
        TIMER_ARM_DEADLINE[cpu].store(deadline, Ordering::Relaxed);
    }
    TIMER_ARM_COUNTS[cpu].store(counts, Ordering::Relaxed);
    TIMER_ARM_TB[cpu].store(tb, Ordering::Relaxed);
    let idx =
        TICK_ARM_IDX[cpu].fetch_add(1, Ordering::Relaxed) as usize % TICK_ARM_RING_SLOTS;
    let e = &TICK_ARM_RING[cpu][idx];
    e.seq.store(seq, Ordering::Relaxed);
    e.tb.store(tb, Ordering::Relaxed);
    e.ra.store(tick_arm_ra(), Ordering::Relaxed);
    e.deadline.store(deadline, Ordering::Relaxed);
    e.counts.store(counts, Ordering::Relaxed);
    e.op.store(op, Ordering::Relaxed);
}

/// Physical address of the ring for the boot-time host-side forensics
/// line (kernel image VMA-LMA constant, linker.ld KERNEL_VMA).
pub fn tick_arm_ring_phys() -> usize {
    (&TICK_ARM_RING as *const _ as usize).wrapping_sub(0xffff_ffff_8000_0000)
}

// ---------------------------------------------------------------------------
// SP-5 round 5: frozen-tick trap (timers/interrupts guarded domain)
// ---------------------------------------------------------------------------
//
// During the memory-exhaustion ramp the per-CPU timer-IRQ counters freeze on
// BOTH CPUs while the timebase keeps advancing (round-4 clock boot: tb
// 6.17M -> 7.05M, irq0/irq1 stuck at 46/50). The four candidate causes:
//   (a) LAPIC timer programmed off / one-shot exhausted with nobody re-arming
//   (b) IF=0 stuck on the CPU (interrupts enabled flag never restored)
//   (c) IRR bit stuck (APIC accepted the IRQ but delivery is blocked:
//       TPR/SVR priority gate or a pending higher-priority ISR bit)
//   (d) halt wakeup lost (CPU halted, one-shot exhausted, nothing re-armed)
// The live register snapshot discriminates all four in one read.

/// Timer-programming op tags for the arm trail.
pub const TIMER_OP_ONESHOT: u32 = 1;
pub const TIMER_OP_MASK: u32 = 2;
pub const TIMER_OP_PERIODIC: u32 = 3;

/// Per-CPU count of LAPIC timer programming events (arm/mask/start).
pub static TIMER_ARM_SEQ: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
/// Last op kind (TIMER_OP_*).
pub static TIMER_ARM_OP: [AtomicU32; crate::config::MAX_CPUS] =
    [const { AtomicU32::new(0) }; crate::config::MAX_CPUS];
/// Last one-shot deadline (nominal ticks) programmed.
pub static TIMER_ARM_DEADLINE: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];
/// Last initial count written to TMICT.
pub static TIMER_ARM_COUNTS: [AtomicU32; crate::config::MAX_CPUS] =
    [const { AtomicU32::new(0) }; crate::config::MAX_CPUS];
/// Timebase value at the last programming event.
pub static TIMER_ARM_TB: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// rt-r4 (cycdbg fire-late discriminator): the deadline this CPU's LAPIC
/// was last armed at (0 = never armed). Read at a late fire to separate
/// "one-shot armed at a LATER deadline" (kernel coarse arm) from "armed
/// at the deadline, KVM delivered late" (host term).
pub fn last_arm_deadline(cpu: usize) -> u64 {
    if cpu >= crate::config::MAX_CPUS {
        return 0;
    }
    TIMER_ARM_DEADLINE[cpu].load(Ordering::Relaxed)
}

/// Live LAPIC timer + delivery state of THIS CPU, for the frozen-tick trap.
#[derive(Clone, Copy, Debug)]
pub struct LapicTimerSnapshot {
    /// Timer LVT entry (vector | mode<<17 | mask<<16).
    pub lvt: u32,
    /// Timer initial count register (reads back the last TMICT write).
    pub tmict: u32,
    /// Timer CURRENT count register (live; 0 = counter exhausted).
    pub tmcct: u32,
    /// Timer divide configuration.
    pub tdcr: u32,
    /// Spurious-vector register (bit 8 = APIC software enable).
    pub svr: u32,
    /// Task-priority register (blocks vectors below its class).
    pub tpr: u32,
    /// IRR bits 255:224 (vector 0xE0 = bit 0): IRQ accepted, not serviced.
    pub irr7: u32,
    /// ISR bits 255:224: IRQ in service.
    pub isr7: u32,
}

impl LapicTimerSnapshot {
    /// IRR pending for the local-timer vector.
    #[inline]
    pub fn irr_timer(&self) -> bool {
        self.irr7 & (1 << (LOCAL_TIMER_VECTOR & 31)) != 0
    }
    /// In-service for the local-timer vector.
    #[inline]
    pub fn isr_timer(&self) -> bool {
        self.isr7 & (1 << (LOCAL_TIMER_VECTOR & 31)) != 0
    }
    /// LVT mask bit.
    #[inline]
    pub fn lvt_masked(&self) -> bool {
        self.lvt & LVT_MASKED != 0
    }
    /// LVT mode bits [18:17]: 0 = one-shot, 1 = periodic.
    #[inline]
    pub fn lvt_periodic(&self) -> bool {
        self.lvt & LVT_TIMER_PERIODIC != 0
    }
}

/// Read THIS CPU's live LAPIC timer state. `None` when the APIC driver is
/// not up (pre-init context — no trap to arm there).
pub fn lapic_timer_snapshot() -> Option<LapicTimerSnapshot> {
    if !READY.load(Ordering::Acquire) {
        return None;
    }
    Some(LapicTimerSnapshot {
        lvt: reg_read(XAPIC_TIMER_LVT, X2APIC_TIMER_LVT),
        tmict: reg_read(XAPIC_TIMER_INIT, X2APIC_TIMER_INIT),
        tmcct: reg_read(XAPIC_TIMER_CURR, X2APIC_TIMER_CURR),
        tdcr: reg_read(XAPIC_TIMER_DIV, X2APIC_TIMER_DIV),
        svr: reg_read(XAPIC_SVR, X2APIC_SVR),
        tpr: reg_read(XAPIC_TPR, X2APIC_TPR),
        irr7: reg_read(XAPIC_IRR7, X2APIC_IRR7),
        isr7: reg_read(XAPIC_ISR7, X2APIC_ISR7),
    })
}

/// Adjudicate a snapshot into the four-way discrimination. Returns a short
/// reason string for the trap line (used by the regression test tooling).
pub fn tick_trap_adjudicate(s: &LapicTimerSnapshot) -> &'static str {
    if s.lvt_masked() {
        return "lvt-masked";
    }
    if s.svr & 0x100 == 0 {
        return "apic-disabled";
    }
    // TPR blocks the vector when its class exceeds the timer's class
    // (vector 0xE0 = class 0xE).
    if (s.tpr >> 4) > ((LOCAL_TIMER_VECTOR >> 4) as u32) {
        return "tpr-blocked";
    }
    if s.isr_timer() {
        return "in-service";
    }
    if s.irr_timer() {
        // Accepted but never dispatched: delivery blocked while the CPU
        // runs (IF=0 stuck is the leading producer — the peer IPI probe
        // in the same trap separates CPU-stuck from APIC-side).
        return "irr-stuck";
    }
    if s.tmcct == 0 && s.tmict != 0 {
        // Counter exhausted and no IRR bit: the one-shot fired (or was
        // never re-armed) and the interrupt is GONE — re-arm path dead
        // or the count was consumed by another LVT mode flip.
        return "oneshot-exhausted";
    }
    if s.tmcct != 0 {
        // Armed and counting: when it reaches zero the IRQ should fire —
        // a stale far-future count means the arm math was wrong.
        return "counting";
    }
    "unknown"
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
    // Fixed 8254 ports; IF is off during early bring-up (the port-IO
    // primitives are safe fns — INV-XE-PIO-1; the protocol ORDER is the
    // reference's i8254 dance).
    {
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
    crate::drivers::hotpath::timer::lapic_time_calibrated(
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
            "TSC {} MHz (raw {} Hz), LAPIC {} MHz (raw {} Hz) ({} APIC)",
            tsc / 1_000_000,
            tsc,
            lapic / 1_000_000,
            lapic,
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

/// Validate the checksum over the first `len` bytes at `vaddr`
/// (INV-MMIO-1: the linear-map table windows this module established).
fn checksum_ok(vaddr: usize, len: usize) -> bool {
    let region = crate::safety::mmio::MmioRegion::new(vaddr, len);
    let mut sum: u8 = 0;
    for i in 0..len {
        sum = sum.wrapping_add(region.read_u8(i));
    }
    sum == 0
}

/// Volatile 16-bit read from a linear-mapped physical address.
///
/// For 2-byte-aligned firmware fields (the EBDA segment word at 0x40E):
/// a u32 read there would trip INV-MMIO natural alignment — f2913fd03's
/// detector-as-assertion exposed exactly that latent violation (debug
/// x86_64 boots panicked in find_rsdp before this split).
fn read_phys_u16(phys: usize) -> u16 {
    let va = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new(phys as u64),
    );
    crate::safety::mmio::MmioRegion::new(va.as_usize(), 2).read_u16(0)
}

/// Volatile 32-bit read from a linear-mapped physical address.
fn read_phys_u32(phys: usize) -> u32 {
    let va = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new(phys as u64),
    );
    crate::safety::mmio::MmioRegion::new(va.as_usize(), 4).read_u32(0)
}

/// Volatile 64-bit read from a linear-mapped physical address.
fn read_phys_u64(phys: usize) -> u64 {
    let va = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new(phys as u64),
    );
    crate::safety::mmio::MmioRegion::new(va.as_usize(), 8).read_u64(0)
}

/// Byte-composed 64-bit read for PACKED firmware-table fields at
/// non-8-aligned physical addresses (XSDT entries at table+36).
fn read_phys_packed_u64(phys: usize) -> u64 {
    let va = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new(phys as u64),
    );
    crate::safety::mmio::MmioRegion::new(va.as_usize(), 8).read_packed_u64(0)
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
/// header fields before the table length is known). PACKED: ACPI table
/// bases are not guaranteed naturally aligned — QEMU's legacy blob
/// points rev-0 RSDPs at an RSDT on an odd address (observed
/// 0x7ffe23bd: +4 lands 1-mod-4 and trips INV-MMIO in debug builds).
fn read_table_u32(phys: usize) -> u32 {
    ensure_phys_mapped(phys & !0xFFF, 8);
    read_phys_packed_u32(phys)
}

/// Byte-composed 32-bit read for PACKED firmware-table fields at
/// non-naturally-aligned physical addresses (see MmioRegion::read_packed_u32).
fn read_phys_packed_u32(phys: usize) -> u32 {
    let va = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new(phys as u64),
    );
    crate::safety::mmio::MmioRegion::new(va.as_usize(), 4).read_packed_u32(0)
}

/// Find the RSDP in the EBDA / legacy BIOS ROM area.  Returns its
/// physical address.
fn find_rsdp() -> Option<usize> {
    // EBDA base is a real-mode segment word (u16) at 0x40E — read it with
    // the width it has (2-byte aligned); a u32 read at 0x40E violates
    // INV-MMIO natural alignment and panics debug builds (f2913fd03).
    let ebda_seg = read_phys_u16(0x40E) as usize;
    let candidates: [usize; 2] = [(ebda_seg as usize) << 4, 0xE_0000];
    let sizes: [usize; 2] = [1024, 0x2_0000];

    for (base, size) in candidates.iter().zip(sizes.iter()) {
        let mut a = *base & !0xF;
        while a + 36 <= base + size {
            let va = crate::arch::mm::memory_layout::phys_to_virt(
                crate::arch::mm::memory_layout::PhysAddr::new(a as u64),
            )
            .as_usize();
            // Mapped candidate region (INV-MMIO-1).
            let region = crate::safety::mmio::MmioRegion::new(va, 36);
            let mut sig = [0u8; 8];
            for (i, b) in sig.iter_mut().enumerate() {
                *b = region.read_u8(i);
            }
            if &sig == b"RSD PTR " {
                let len = {
                    // Revision 0 (ACPI 1.0) checksums only the first 20
                    // bytes; later revisions use the Length field.
                    let rev = region.read_u8(15);
                    if rev >= 2 {
                        let l = region.read_u32(20);
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
    // Validated RSDP (INV-MMIO-1 window over the checked structure).
    let rsdp_region = crate::safety::mmio::MmioRegion::new(rsdp_va, core::mem::size_of::<Rsdp>());
    let rsdp_rev = rsdp_region.read_u8(15);

    let (entries_phys, entry_size, count): (usize, usize, usize) = if rsdp_rev >= 2 {
        // Packed field read through the region.
        let xsdt = rsdp_region.read_u64(24) as usize;
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
        let rsdt = rsdp_region.read_u32(16) as usize;
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
            // XSDT entries start at +36 (SdtHeader): 4-mod-8 — packed
            // field, byte-composed read (a natural u64 here would trip
            // INV-MMIO on every 8-aligned XSDT).
            read_phys_packed_u64(entries_phys + i * 8) as usize
        } else {
            // RSDT entries — packed (table bases not aligned, see
            // read_table_u32).
            read_phys_packed_u32(entries_phys + i * 4) as usize
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
        // Header-sized mapped read (INV-MMIO-1).
        let region = crate::safety::mmio::MmioRegion::new(tbl_va, 4);
        let mut sig = [0u8; 4];
        for (i, b) in sig.iter_mut().enumerate() {
            *b = region.read_u8(i);
        }
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
    // HPET GAS fields at table+44/48/52 — packed reads (unaligned table
    // bases; the u64 address is read as two u32 halves because the
    // field itself sits at 4-mod-8 when the table base is odd).
    let space = read_phys_packed_u32(tbl + 44) as u8;
    if space != 0 {
        return None;
    }
    let lo = read_phys_packed_u32(tbl + 48) as usize;
    let hi = read_phys_packed_u32(tbl + 52) as usize;
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
    // Mapped MADT (INV-MMIO-1 window over the checksummed table).
    let madt = crate::safety::mmio::MmioRegion::new(va, hdr_len);
    let mut apic_sig = [0u8; 4];
    for (i, b) in apic_sig.iter_mut().enumerate() {
        *b = madt.read_u8(i);
    }
    if &apic_sig != b"APIC" {
        return 0;
    }

    let bsp_id = this_lapic_id_raw();
    let mut ids = [0u32; crate::config::MAX_CPUS];
    let mut n = 0usize;

    let body = va + core::mem::size_of::<SdtHeader>() + 8; // skip lapic_addr+flags
    let mut off = body;
    let end = va + hdr_len;
    while off + 2 <= end && n < crate::config::MAX_CPUS {
        // Entry header read inside the validated table.
        let (etype, elen) = (madt.read_u8(off - va), madt.read_u8(off + 1 - va));
        if elen < 2 || off + elen as usize > end {
            break;
        }
        match etype {
            0 => {
                // Processor Local APIC: [u8 acpi_id][u8 apic_id][u32 flags]
                if elen >= 8 {
                    let (apic_id, flags) = (
                        madt.read_u8(off + 3 - va) as u32,
                        // Packed field: entry offsets walk variable-length
                        // predecessors (10-byte ISOs et al.), so off+4 is
                        // not naturally aligned — byte-composed read.
                        madt.read_packed_u32(off + 4 - va),
                    );
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
                    let (uid, flags) = (
                        // Packed fields — see the type-0 arm.
                        madt.read_packed_u32(off + 4 - va),
                        madt.read_packed_u32(off + 8 - va),
                    );
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
    // CPUID leaf 1, ECX bit 21 = x2APIC. The cmdline override
    // `apic=xapic` forces MMIO xAPIC mode even when x2APIC exists
    // (RT-2X probe knob: hypervisor IPI-virtualization classes — AMD
    // AVIC virtualizes xAPIC-window writes, x2APIC MSR writes always
    // exit; see run/RT-2X.md).
    let has_x2 = crate::arch::cpu::cpuid(1, 0).2 & (1 << 21) != 0
        && crate::cmdline::get_param("apic").as_deref() != Some("xapic");

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
    // RT-2X item 4: negotiate the KVM paravirt IPI before any FIXED
    // IPI can be sent (secondary bring-up SIPIs are not FIXED and stay
    // on the raw ICR path by construction — they predate READY anyway).
    kvm_para_init();
    if KVM_PV_IPI.load(Ordering::Acquire) {
        crate::pr_info!(
            "apic: KVM PV_SEND_IPI negotiated ({} hypercall)",
            if KVM_HC_VMMCALL.load(Ordering::Acquire) { "vmmcall" } else { "vmcall" }
        );
    }
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

    // SP-5 r6: publish the freeze-face ring for host-side forensics
    // (one line; the QEMU monitor `xp` dump needs the address BEFORE
    // any freeze can silence the console).
    crate::println!(
        "TICKARM-RING va={:#x} phys={:#x} slots={} stride={}",
        &TICK_ARM_RING as *const _ as usize,
        tick_arm_ring_phys(),
        TICK_ARM_RING_SLOTS,
        core::mem::size_of::<TickArmEvent>()
    );
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

}

// Export corresponding interrupt controller based on platform
#[cfg(feature = "aarch64")]
pub use gicv3::*;


#[cfg(feature = "aarch64")]
pub fn init() {
    gicv3::init();
}

#[cfg(feature = "riscv64")]
pub fn init() {
    plic::init();
    clint::init();
}

// x86_64: the 8259 PIC is programmed by the arch trap bring-up; the
// local APIC (IPIs, per-CPU timer, SMP) comes up here, with the PIC
// kept alive in virtual-wire mode through LVT LINT0.
#[cfg(feature = "x86_64")]
pub fn init() {
    apic::init();
}

}

// ════════════════ timer ═══════════════════════════════
// (was the directory timer/: one inline module — its child
// modules are chosen by #[cfg] on the module declaration, so the
// files keep their text unchanged inside the braces)
// Timer driver
pub mod timer {
#[cfg(feature = "riscv64")]

pub mod riscv64 {
use riscv::register::time;
use crate::sbi;
use core::sync::atomic::{AtomicU64, Ordering};
use core::arch::asm;

/// Timer frequency (QEMU virt platform) - from config
pub const CLOCK_FREQ: u64 = crate::config::TIMER_CLOCK_FREQ_HZ;

/// System clock frequency (HZ) - from config
///
/// Triggers HZ clock interrupts per second
pub const HZ: u64 = crate::config::KERNEL_HZ as u64;

/// Time slice length (10 milliseconds)
///
/// Each time slice is 10ms, used for preemptive scheduling
const TIME_SLICE_TICKS: u64 = CLOCK_FREQ / HZ;  // 10ms

/// jiffies - global clock counter
///
///
/// Used for:
/// - Time measurement
/// - Timeout management
/// - Scheduling statistics
/// - Performance analysis
///
/// Type: AtomicU64 (supports multi-core concurrent access)
// (JIFFIES/JIFFIES_BASE hoisted to crate::timer — rung4 prep move 1)

/// jiffies related functions

/// Get current jiffies value
///
///
/// # Returns
/// - Current jiffies value (clock interrupt count since system boot)
#[inline]
pub fn get_jiffies() -> u64 {
    crate::timer::get_jiffies()
}

/// Test-only deterministic jiffy advance (unit-test harness): the tests
/// run with the timer interrupt disabled and — since R87 keeps secondary
/// harts interrupt-less until boot_complete — nothing increments JIFFIES
/// during the suite. Tests that need "time has passed" semantics (e.g.
/// the tcp-karn RTT-sampling control) bump the counter explicitly
/// instead of relying on incidental ticks.
#[cfg(feature = "unit-test")]
pub fn test_advance_jiffies(n: u64) {
    crate::timer::test_advance_jiffies(n)
}

/// Floor-map an absolute timebase value onto the jiffy grid — the
/// read side of increment_jiffies' `floor((time - base) / period)`
/// derivation (0 before the grid origin is established). The timer
/// wheel uses this to derive an hres entry's first-check jiffy from
/// its precise time-CSR deadline, so routing never trusts
/// caller-supplied jiffy bookkeeping.
#[inline]
pub fn jiffy_floor_of_time(t: u64) -> u64 {
    crate::timer::jiffy_floor_of_time(t, TIME_SLICE_TICKS)
}

/// Coarse time in nanoseconds since boot, snapped to the NOMINAL tick
/// grid and computed from the LIVE timebase — never from the stored
/// JIFFIES counter, whose tick-handler updates can lag the timebase by
/// seconds under heavy load / TCG. This is the kernel's
/// ktime_get_coarse_real_ts64 equivalent: every filesystem timestamp
/// stamp AND every CLOCK_REALTIME_COARSE / CLOCK_MONOTONIC_COARSE read
/// must route through the same source, or a timestamp bracketed by two
/// coarse reads can fall outside it (LTP utime01: utime(NULL) stamped
/// the inode with live seconds while tst_get_fs_timestamp()'s
/// CLOCK_REALTIME_COARSE lagged 3s behind — st_mtime > post_time).
/// Grid-snapping (not raw live time) keeps the "1/HZ granularity"
/// coarse-clock contract while bounding the error to one tick.
#[inline]
pub fn coarse_ns_since_boot() -> u64 {
    read_time() / TIME_SLICE_TICKS * (1_000_000_000 / HZ)
}

/// Advance jiffies from the TIMEBASE, not from IRQ counts.
///
/// Review批次7 (TIMING 高): every hart's timer IRQ incremented the single
/// global counter — with 4 CPUs, jiffies ran 4x fast (every jiffies-based
/// timeout fired at 1/4 of its nominal duration). Deriving jiffies from
/// `floor((time - base) / ticks_per_period)` instead makes the update
/// idempotent and multi-hart safe, and also keeps jiffies glued to the
/// NOMINAL tick grid (immune to accumulated re-arm latency).
#[inline]
fn increment_jiffies() {
    crate::timer::jiffies_update_from_timebase(read_time(), TIME_SLICE_TICKS);
}

/// Convert jiffies to milliseconds
///
#[inline]
pub const fn jiffies_to_msecs(jiffies: u64) -> u64 {
    jiffies * 1000 / HZ
}

/// Convert milliseconds to jiffies
///
#[inline]
pub const fn msecs_to_jiffies(msecs: u64) -> u64 {
    msecs * HZ / 1000
}

/// Read current time (time CSR)
#[inline]
pub fn read_time() -> u64 {
    time::read() as u64
}

/// High-resolution nanoseconds since boot, straight from the time CSR
/// (no jiffy quantization). Used by poll/select-style waits whose
/// timeout contract must not expire early: jiffy floors turn a 1ms
/// pselect into an instant return (LTP nanosleep01/select02/poll02
/// "woken up early" family). u128 intermediate — a u64 multiply would
/// overflow after ~30 minutes of uptime.
#[inline]
pub fn monotonic_ns_since_boot() -> u64 {
    ((read_time() as u128) * 1_000_000_000u128 / (CLOCK_FREQ as u128)) as u64
}

/// Set timer using stimecmp CSR (sstc extension)
///
/// This is more direct than SBI and avoids potential issues
#[inline]
fn set_timer_stimecmp(deadline: u64) {
    unsafe {
        asm!(
            "csrw stimecmp, {0}",
            in(reg) deadline,
            options(nomem, nostack)
        );
    }
}

/// Set timer using SBI call (fallback)
#[inline]
fn set_timer_sbi(deadline: u64) {
    sbi::set_timer(deadline);
}

/// Set timer - use SBI set_timer for reliable cross-hart behavior.
///
/// Direct stimecmp writes from S-mode can race with OpenSBI's M-mode
/// timer management on QEMU virt.  SBI set_timer goes through M-mode
/// which ensures the correct per-hart timer is armed.
pub fn set_timer(deadline: u64) {
    set_timer_sbi(deadline);
}

/// Set next timer interrupt, aligned to the NOMINAL tick grid — but never
/// later than the earliest outstanding HIGH-RESOLUTION deadline (precise
/// sleeps; LTP clock_nanosleep02 forbids early wakeups, so the hart timer
/// must be able to fire BETWEEN grid ticks).
///
/// Review批次8: arming at `now + period` accumulates every handler's
/// latency into the period (permanent drift — sleeps lengthen under
/// load). Rounding up to the next grid multiple keeps the interrupt
/// cadence glued to nominal 1/HZ boundaries.
pub fn set_next_trigger() {
    let current = read_time();
    let mut deadline = (current / TIME_SLICE_TICKS + 1) * TIME_SLICE_TICKS;
    let hres = crate::timer::hres_next_deadline();
    if hres < deadline {
        deadline = hres.max(current + 1);
    }
    set_timer(deadline);
}

/// Re-arm THIS hart's timer to min(next grid tick, earliest hres deadline).
/// Called by a task that just registered a high-resolution sleep so the
/// already-armed (later) grid tick does not delay its precise wake.
pub fn rearm_for_hres() {
    set_next_trigger();
}

/// Clock interrupt handler
///
///
/// # Functions
/// 1. Update jiffies counter
/// 2. Update system runtime statistics
/// 3. Trigger scheduler tick
/// 4. Process timer callbacks
///
/// # When called
/// Called by trap_handler on every clock interrupt
///
/// # Notes
/// - Called in interrupt context, cannot sleep
/// - Must complete quickly to avoid affecting system performance
pub fn timer_interrupt_handler() {
    // 1. Update jiffies counter
    increment_jiffies();

    // 1.5 P2 vDSO: refresh the shared time-data page under its seqlock
    // so user-space __vdso_clock_gettime readers keep a recent base.
    crate::mm::hotpath::vdso_data_tick();

    // 2. Raise Timer softirq for TCP timer processing
    //    (retransmission, delayed ACK, etc. — deferred to bottom half).
    //    sc184 (lane 2, perf-audit F1): gated on the table's high-water
    //    count — the handler early-returns when it is 0, so the raise
    //    was pure irq-exit dispatch overhead (x86_64 twin has details).
    if crate::api::net::tcp_table_nonempty() {
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Timer as usize,
        );
    }

    // 2.4 RT-2X item 4 (paired-test queue): expire due high-resolution
    //     timers at the TOP of the IRQ — the reference's
    //     hrtimer_interrupt shape (the x86_64 twin carries the measured
    //     story; run/RT-2X.md). The raise below stays for timers armed
    //     during this delivery.
    crate::timer::expire_hres_inline();

    // 2.5 Raise Hrtimer softirq for software timer processing.
    //     sc184 (lane 2): gated until a timer has been armed or an hres
    //     deadline is due (see TIMER_EVER_ARMED in timer.rs).
    if crate::timer::timer_softirq_work_pending() {
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Hrtimer as usize,
        );
    }

    // 3. TODO: Update process runtime statistics
    //    - Current process utime/stime
    //    - CPU statistics

    // 4. TODO: Process software timers
    //    - Check expired timers
    //    - Call timer callback functions

    // 5. TODO: Trigger scheduler tick
    //    - Update current process runtime
    //    - Check if scheduling is needed
    //    - Set need_resched flag

    // Note: Scheduling is handled by schedule() call in trap.rs
}

}
#[cfg(feature = "riscv64")]
pub use riscv64::*;

#[cfg(feature = "x86_64")]

pub mod x86_64 {
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use crate::arch::cpu::{inb, io_wait, outb};

/// PIT input gate frequency (fixed 8254 crystal, Hz)
pub const PIT_INPUT_FREQ: u64 = 1_193_182;

/// Nominal clock domain of `read_time()` — matches
/// `crate::config::TIMER_CLOCK_FREQ_HZ` so generic cycle arithmetic
/// keeps its units.
pub const CLOCK_FREQ: u64 = crate::config::TIMER_CLOCK_FREQ_HZ;

/// System clock frequency (HZ) — ticks per second
pub const HZ: u64 = crate::config::KERNEL_HZ as u64;

/// Nominal clock ticks per jiffy (10 MHz / HZ)
const TIME_SLICE_TICKS: u64 = CLOCK_FREQ / HZ;

/// PIT I/O ports (channel 0 data, mode/command register)
const PIT_CH0_DATA: u16 = 0x40;
const PIT_MODE_CMD: u16 = 0x43;

/// Mode/command byte: channel 0, lobyte/hibyte access, mode 3 (square wave),
/// binary count.
const PIT_CH0_MODE3_BIN: u8 = 0x36;

/// Fractional shift used by the TSC -> nominal-domain mapping (pub for
/// hpet::calibrate_tsc — keep in sync with the promotion math).
///
/// sc110-114 ROOT CAUSE NOTE: this was 12 for the whole sc-series, where
/// mult = ceil(CLOCK_FREQ << 12 / tsc_hz) = ceil(9.5434) = 10 — a 10.5%
/// resolution grid — so the mapping ran 10/9.5434 = 4.796% FAST against
/// the HPET on every boot (sc114: "TSC 4292 MHz" -> 4,292.4 MHz measured,
/// ceil still 10). The "one-time ~25 s TSC-vs-HPET step" that parked the
/// timekeeper behind `tscclk` (and inspired the per-CPU vCPU-offset skew
/// theory) was exactly this: 4.796% x uptime crosses the resync's 25 s
/// hard-rebase threshold at uptime 522.17 s — deterministic, and the
/// sc113/sc114 rebases both fired at 522.17 s with lead 250.41M ticks to
/// within 0.6 ms, matching the rounding error to 4 significant digits.
/// (The per-CPU corrections measured ~-21.9 ms on the AP = the same
/// 4.796% x 0.456 s AP-bringup instant, not vCPU skew.) At shift 32 the
/// mult grid is ~100 ppm and round-to-nearest halves the residual, so
/// the 1 Hz bounded resync absorbs what is left.
pub const TSC_MULSHIFT: u32 = 32;

/// TSC timekeeping state lives in the vDSO DATA PAGE (mm/vdso.rs
/// x86_track) since the unified-track re-arm: ONE anchor
/// {cycle_last, base_mono_nom, mult} serves both the kernel's
/// read_time() and the user-side vDSO interpolation, bit-identically.
/// The only timer-local state left:
///
/// - TSC_ACTIVE: the page anchor is published (promote ran);
/// - TSC_FROM_HPET: the anchor was HPET-calibrated (gates the 1 Hz
///   resync);
/// - TSC_LAST: the kernel-side monotonic ratchet (see read_time).
static TSC_ACTIVE: AtomicU64 = AtomicU64::new(0);

/// True when the active TSC timekeeper was calibrated against the HPET
/// (sc109: the OH launcher child burned its whole 3 s appspawn watchdog
/// in clock_gettime — no vDSO, so every call paid the syscall AND an
/// HPET MMIO read, which under KVM is a VM exit. KVM's TSC is invariant
/// and vCPU-synchronized, so after calibration RDTSC serves the nominal
/// clock with no exits; a 1 Hz resync against the HPET bounds drift).
static TSC_FROM_HPET: AtomicU64 = AtomicU64::new(0);

/// Monotonic ratchet: the largest value ever returned by read_time().
/// Plain unsynchronized max-store — a lost race only leaves the ratchet
/// one store behind, never lets it decrease.
///
/// On the unified track this NEVER engages on a healthy boot (the page
/// base only moves forward; the vCPU TSC never goes backward): it is
/// anomaly insurance for the >25 s rebase pull-back and any future
/// writer bug — the syscall clock freezes briefly instead of stepping
/// back. NOTE the vDSO track has no ratchet (userspace cannot see
/// TSC_LAST); that is exactly what makes the two tracks bit-identical,
/// and why the base must never move backward outside the rebase path.
static TSC_LAST: AtomicU64 = AtomicU64::new(0);

/// Lane instrumentation (vdso-clock): times read_time() fell off the TSC
/// track to the HPET fallback while the track was active.
static HPET_FALLBACK: AtomicU64 = AtomicU64::new(0);

/// Consecutive track-read failures in read_time() (reset on success;
/// ≥ TRACK_FAIL_ESCAPE trips the HPET escape hatch).
static TRACK_FAIL: AtomicU64 = AtomicU64::new(0);
const TRACK_FAIL_ESCAPE: u64 = 10;

/// Jiffies stamp of the last escape WARNING print (rate limiter: first
/// episode prints immediately, then at most one line per 60 s — an
/// SMP8 boot under a persistent reader-failure storm logged 200+ lines
/// otherwise, burying the serial console for every other consumer).
static ESCAPE_PRINT_JIFFIES: AtomicU64 = AtomicU64::new(0);

// (jiffies statics + get_jiffies + conversions + the tick-grid
// derivation HOISTED to crate::timer — the rung4 prep move; this driver
// keeps read_time (the TSC/HPET clocksource) and re-exports the
// framework items for its historical callers.)

pub use crate::timer::{
    get_jiffies, jiffies_to_msecs, msecs_to_jiffies,
};

/// Floor-map onto the jiffy grid (the framework derivation; the x86
/// period is TSC-nominal TIME_SLICE_TICKS).
#[inline]
pub fn jiffy_floor_of_time(t: u64) -> u64 {
    crate::timer::jiffy_floor_of_time(t, TIME_SLICE_TICKS)
}

/// Coarse time in nanoseconds since boot, snapped to the nominal tick
/// grid (same contract as the riscv64 twin).
#[inline]
pub fn coarse_ns_since_boot() -> u64 {
    read_time() / TIME_SLICE_TICKS * (1_000_000_000 / HZ)
}

/// Test-only jiffy advance (framework static).
#[cfg(feature = "unit-test")]
pub use crate::timer::test_advance_jiffies;

/// High-resolution nanoseconds since boot (see the riscv64 twin).
#[inline]
pub fn monotonic_ns_since_boot() -> u64 {
    ((read_time() as u128) * 1_000_000_000u128 / (CLOCK_FREQ as u128)) as u64
}

/// Read current time in the nominal CLOCK_FREQ clock domain.
///
/// TSC-derived first (fast: RDTSC, no VM exit) once the page anchor is
/// published — the UNIFIED TRACK (mm/vdso.rs x86_track): the same page
/// words, the same seq protocol, the same integer arithmetic the vDSO
/// functions run in userspace, so a syscall read and a vDSO read at the
/// same instant are bit-identical (LTP clock_gettime04's strict
/// adjacent-pair requirement). HPET-derived (one MMIO read per call)
/// only in the pre-promotion window; jiffies-derived before either.
///
/// VISIBLE-CLOCK DISCIPLINE (vdso-clock hunt, 2026-11-18): the syscall
/// clock and the vDSO track are ONE clock. Two structural violations of
/// that were the release-profile CLOCK_MONOTONIC poison:
///  1. the TSC_LAST ratchet was a load-then-conditional-store — two
///     racing readers could ratchet it DOWN, after which the visible
///     syscall clock stepped backward by the skew between their reads
///     (measured 25 µs; served to cyclictest as (uint64)(-25µs) ≈
///     1.8e19, which its double average prints as Avg≈1e15 with a
///     NEGATIVE Max). Now an atomic fetch_max: it never decreases.
///  2. when the track read failed (128 seq retries exhausted — a
///     reader descheduled mid-retry under a publication storm), the
///     fallback returned a RAW HPET value tens of µs off the track the
///     vDSO was serving: clock_nanosleep armed deadlines on one clock
///     while glibc read `now` from the other — the mixed-track skew is
///     the negative-latency class. Now the failure path FREEZES at the
///     ratchet (the track is forward-only; the freeze lasts only until
///     the next successful read, typically well under a tick) and only
///     a PERSISTENT failure (page wedged ≥ TRACK_FAIL_ESCAPE consecutive
///     reads) escapes to the HPET through the same forward-only
///     ratchet, loudly.
#[inline]
pub fn read_time() -> u64 {
    if TSC_ACTIVE.load(Ordering::Acquire) != 0 {
        if let Some(mut v) =
            crate::mm::hotpath::x86_track::tsc_track_now_nominal()
        {
            // Monotonic ratchet (TSC_LAST doc): never engages on a
            // healthy unified track; freezes the clock through the
            // rebase pull-back instead of stepping back.
            TRACK_FAIL.store(0, Ordering::Relaxed);
            let prev = TSC_LAST.fetch_max(v, Ordering::Relaxed);
            if v < prev {
                v = prev;
            }
            return v;
        }
        // Track busy/invalid: freeze the visible clock at the ratchet
        // rather than mixing in a second clocksource (see the doc
        // above). The freeze is bounded by the writer's section (µs)
        // and the track only moves forward, so nothing strands.
        let n = TRACK_FAIL.fetch_add(1, Ordering::Relaxed);
        let last = TSC_LAST.load(Ordering::Relaxed);
        if n < TRACK_FAIL_ESCAPE && last != 0 {
            return last;
        }
        // Rate-limited anomaly report: the escape itself repeats (each
        // fresh 10-fail streak re-escapes), but the print does not —
        // an SMP8 24.04 boot logged 200+ lines interleaved with every
        // other console consumer. First episode prints at once; then at
        // most one line per 60 s while the condition persists.
        if n == TRACK_FAIL_ESCAPE {
            let now_j = get_jiffies();
            let prev_j = ESCAPE_PRINT_JIFFIES.load(Ordering::Relaxed);
            if prev_j == 0 || now_j.saturating_sub(prev_j) >= HZ as u64 * 60 {
                if ESCAPE_PRINT_JIFFIES
                    .compare_exchange(prev_j, now_j, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    crate::pr_warn!(
                        "tsc-clock: track unreadable {} reads — escaping to HPET (anchor wedged or TSC smashed; last={})",
                        n,
                        last
                    );
                }
            }
        }
        let hpet = crate::drivers::hotpath::timer::hpet::read_time_nominal();
        let prev = TSC_LAST.fetch_max(hpet, Ordering::Relaxed);
        if hpet < prev {
            return prev;
        }
        return hpet;
    }
    if crate::drivers::hotpath::timer::hpet::ready() {
        crate::drivers::hotpath::timer::hpet::read_time_nominal()
    } else {
        get_jiffies() * TIME_SLICE_TICKS
    }
}

/// Promote the calibrated TSC to timekeeper (hpet::calibrate_tsc /
/// lapic_time_calibrated).
///
/// `mult` maps TSC deltas to the nominal domain (mult-then-shift),
/// `base_tsc` is the TSC stamp captured with `nominal_at_base`, and the
/// continuity base makes track(base_tsc) == nominal_at_base so the
/// switch from the previous clock cannot run time backward. The anchor
/// is published to the vDSO data page (the single timekeeping state);
/// from this instant read_time() and the vDSO fast path compute the
/// same value from the same words.
pub fn promote_tsc_timekeeper(mult: u64, base_tsc: u64, nominal_at_base: u64) {
    crate::mm::hotpath::x86_track::publish_tsc_anchor(mult, base_tsc, nominal_at_base);
    TSC_ACTIVE.store(1, Ordering::Release);
    TSC_FROM_HPET.store(1, Ordering::Release);
}

/// 1 Hz drift discipline for the HPET-calibrated unified track (timer
/// tick calls this; see TSC_FROM_HPET). The correction is a SINGLE
/// FORWARD STEP of the page anchor's base (tsc_anchor_step_forward):
/// forward moves are safe for the strict adjacent-pair tests (every
/// reader — kernel or vDSO — jumps forward together, since both read
/// the same word), while any per-CPU correction would re-introduce the
/// cross-CPU spread the unified track exists to eliminate. A track that
/// fell behind (mapping drift, KVM TSC resync) is stepped forward,
/// clamped to 10 ms per resync so a poisoned HPET read cannot jump the
/// clock; a lead >250 ms is REPORTED (post shift-32 this should be
/// silent for ~a day — any print is a new divergence worth reading),
/// and a lead >25 s is pulled back as a last resort (a real vCPU TSC
/// smash; the one backward base move — see tsc_anchor_step_back).
pub fn resync_tsc_against_hpet() {
    if TSC_FROM_HPET.load(Ordering::Acquire) == 0 {
        return;
    }
    let Some((cl, base, mult)) = crate::mm::hotpath::x86_track::tsc_anchor_peek() else {
        return;
    };
    let raw = crate::mm::hotpath::x86_track::read_tsc_ordered();
    let tsc_nom = base
        .wrapping_add(crate::mm::hotpath::x86_track::tsc_delta_nominal(
            raw.wrapping_sub(cl),
            mult,
        ));
    let hpet_nom = crate::drivers::hotpath::timer::hpet::read_time_nominal();
    if hpet_nom > tsc_nom {
        let behind = hpet_nom - tsc_nom;
        // 10 ms nominal-domain clamp (CLOCK_FREQ=10 MHz → 100_000 ticks).
        let step = behind.min(100_000);
        if behind > 100_000 {
            resync_report(b"tsc-clock: fell behind ", behind);
        }
        crate::mm::hotpath::x86_track::tsc_anchor_step_forward(step);
    } else if tsc_nom > hpet_nom && tsc_nom - hpet_nom > 2_500_000 {
        // 250 ms..25 s lead: REPORT ONLY. Proof-of-fix marker for the
        // shift-32 correction — at ~3 ppm residual mapping error this
        // needs ~23 h of uptime to fire, so any print here means a NEW
        // divergence (or an HPET read stall), worth reading.
        resync_report(b"tsc-clock: lead ", tsc_nom - hpet_nom);
    } else if tsc_nom - hpet_nom > 250_000_000 {
        // >25 s lead: either a vCPU TSC-offset jump (skew) or a smashed
        // mapping. Pull the anchor base back into sync and print the
        // raw pair for offline skew-vs-smash triage. The TSC_LAST
        // ratchet freezes the syscall track through the pull; the vDSO
        // track steps back (a broken-by-definition state — but better
        // than a clock 25 s off, and loud in the log).
        let lead = tsc_nom - hpet_nom;
        resync_report(b"tsc-clock: rebase, lead ", lead);
        crate::pr_warn!(
            "tsc-clock: rebase raw_tsc={:#x} tsc_nom={:#x} hpet_nom={:#x} cl={:#x} base={:#x} mult={:#x}",
            raw,
            tsc_nom,
            hpet_nom,
            cl,
            base,
            mult
        );
        crate::mm::hotpath::x86_track::tsc_anchor_step_back(lead);
    }
}

/// One-shot per-CPU divergence CHECK (AP bringup, IRQs off, before the
/// AP's first tick). With the unified track there is no per-CPU state
/// to calibrate — the page anchor serves every CPU identically, which
/// is only correct when the vCPU TSC offsets are synchronized (KVM
/// invariant TSC; the sc110-114 "skews" were the shift-12 mult bug).
/// Each AP therefore MEASURES its own (HPET − track) divergence and
/// reports anything beyond 1 ms: the tripwire for a host that really
/// arms per-vCPU TSC offsets (hotplug, post-migration), where the
/// global 1 Hz resync would oscillate by the skew. The residual µs
/// noise (HPET MMIO exit latency) is absorbed by the resync clamp.
pub fn calibrate_cpu_tsc_offset(cpu: usize) {
    if TSC_FROM_HPET.load(Ordering::Acquire) == 0 {
        return; // track not HPET-calibrated — nothing to check against
    }
    if cpu == 0 || cpu >= crate::config::MAX_CPUS {
        return;
    }
    let Some((cl, base, mult)) = crate::mm::hotpath::x86_track::tsc_anchor_peek() else {
        return;
    };
    // Three bracketed samples, take the extreme |hpet - track|: the
    // HPET MMIO read can stall (sc113 measured ~21 ms of read skew
    // right after AP startup), and a stall INFLATES the apparent
    // divergence — report the minimum-magnitude sample to keep the
    // tripwire honest.
    let mut best = i64::MAX;
    for _ in 0..3 {
        let hpet_nom = crate::drivers::hotpath::timer::hpet::read_time_nominal();
        let t_after = crate::mm::hotpath::x86_track::read_tsc_ordered();
        let track_after = base.wrapping_add(
            crate::mm::hotpath::x86_track::tsc_delta_nominal(
                t_after.wrapping_sub(cl),
                mult,
            ),
        );
        let d = hpet_nom as i64 - track_after as i64;
        if d.abs() < best.abs() {
            best = d;
        }
    }
    if best != i64::MAX && best.abs() > 10_000 {
        crate::pr_info!(
            "tsc-clock: cpu {} diverges {:+} us from the HPET track (no per-CPU correction; invariant-TSC assumption check)",
            cpu,
            best / 10
        );
    }
}

/// Rate-limited one-line report for resync events (max 8 per boot).
fn resync_report(msg: &[u8], v: u64) {
    static N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    if N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) >= 8 {
        return;
    }
    let mut line = alloc::string::String::new();
    use core::fmt::Write;
    let _ = write!(line, "{}{v}", core::str::from_utf8(msg).unwrap_or(""));
    crate::pr_warn!("{}", line);
}

/// True when the calibrated TSC is the active timekeeper (no HPET).
/// Exposed for diagnostics (debug monowatch names the clocksource).
pub fn tsc_active() -> bool {
    TSC_ACTIVE.load(Ordering::Acquire) != 0
}

/// Called by the intc/APIC driver after calibration: promote the TSC to
/// timekeeper.  `tsc_freq == 0` (failed calibration) keeps the PIT phase.
/// No-op when the HPET clocksource is active — the TSC then stays a
/// delay-only counter (see `hpet.rs`). Publishes the page anchor (the
/// unified track) but does NOT arm the 1 Hz HPET resync (no HPET to
/// check against — TSC_FROM_HPET stays 0).
pub fn lapic_time_calibrated(_lapic_freq: u64, tsc_freq: u64) {
    if tsc_freq == 0 {
        return;
    }
    if crate::drivers::hotpath::timer::hpet::ready() {
        return;
    }
    // nominal = (tsc >> S) * round(CLOCK_FREQ << S / tsc_freq) — nearest
    // rounding, NOT ceil: at a coarse shift a ceiled mult runs the whole
    // mapping fast by up to one mult quantum (sc110-114: 4.796% fast at
    // shift 12; see the TSC_MULSHIFT note).
    let mult = ((CLOCK_FREQ << TSC_MULSHIFT) + tsc_freq / 2) / tsc_freq;
    let now = crate::arch::cpu::read_counter();
    let continuity = get_jiffies() * TIME_SLICE_TICKS;
    crate::mm::hotpath::x86_track::publish_tsc_anchor(mult, now, continuity);
    TSC_ACTIVE.store(1, Ordering::Release);
}

/// Program PIT channel 0 for KERNEL_HZ square-wave ticks.
///
/// Called from the x86_64 timer-enable path when the LAPIC tick could
/// not arm (IRQ0 fallback); idempotent.
pub fn init() {
    let divisor = (PIT_INPUT_FREQ / HZ) as u16;
    // SAFETY: ports 0x40/0x43 are the fixed 8254 channel-0 data and
    // mode/command registers; this is the standard programming sequence.
    unsafe {
        outb(PIT_MODE_CMD, PIT_CH0_MODE3_BIN);
        io_wait();
        outb(PIT_CH0_DATA, (divisor & 0xff) as u8);
        io_wait();
        outb(PIT_CH0_DATA, (divisor >> 8) as u8);
    }
}

/// Arm the LAPIC timer in one-shot mode at the absolute nominal-domain
/// `deadline` (perf lane pd — see apic::timer_arm_oneshot). No-op on the
/// PIT fallback path (channel 0 free-runs at a fixed HZ cadence).
pub fn set_timer(deadline: u64) {
    crate::drivers::hotpath::intc::apic::timer_arm_oneshot(deadline);
}

/// Set the next timer interrupt: the NEXT NOMINAL GRID TICK, but never
/// later than the earliest outstanding HIGH-RESOLUTION deadline (the
/// riscv64 twin's discipline — precise sleeps must be able to fire
/// BETWEEN grid ticks). Arming is skipped while no precise timebase is
/// active: read_time() is then jiffies-derived and an one-shot armed
/// from the coarse clock would mistime; the periodic LAPIC/PIT tick
/// keeps the grid in that window (grid-only hres granularity, the
/// pre-fix behavior, only on calibration-failed boots).
///
/// NOTE (NOHZ): the adaptive-tick machinery lives in
/// crate::timer::nohz (a function-for-function mirror of the
/// reference's kernel/time/tick-sched.c) and re-arms through
/// tick_nohz_irq_exit at the IRQ exits. This function keeps ONLY the
/// pre-nohz grid-glue discipline for the callers outside the tick
/// re-evaluation path (the PIT-fallback tail and the early
/// hres-sleep re-arm; the latter now routes through tick_nohz as well).
pub fn set_next_trigger() {
    if !timebase_precise() {
        return;
    }
    // Grid-glued (review批次8 discipline): round UP to the next grid
    // multiple so handler latency never accumulates into the cadence.
    let current = read_time();
    let mut deadline = (current / TIME_SLICE_TICKS + 1) * TIME_SLICE_TICKS;
    let hres = crate::timer::hres_next_deadline();
    if hres < deadline {
        // Never arm in the past (a WRMSR count of 0 disarms; clamp to
        // just-after-now so an already-expired hres deadline fires at
        // once and the softirq sweep delivers it).
        deadline = hres.max(current + 1);
    }
    set_timer(deadline);
}

/// Re-arm THIS CPU's timer to min(next grid tick, earliest hres
/// deadline). Called by a task that just registered a high-resolution
/// sleep so the already-armed (later) deadline does not delay its
/// precise wake (perf lane pd: this used to be a no-op on x86_64 —
/// every hres sleep waited for the next 10 ms grid tick).
///
/// NOHZ: the sleeper's CPU re-evaluates through the tick_nohz
/// machinery — tick_nohz_next_event arms at the new wheel deadline
/// (the reference's timer-enqueue path re-programs the local
/// clockevent the same way); the retain case keeps the grid glue.
pub fn rearm_for_hres() {
    // Non-full-dynticks CPUs keep the plain grid-glue arm (the
    // reference's timer-enqueue re-program applies to nohz cpus only).
    let cpu = crate::arch::cpu_id() as usize;
    if !crate::timer::nohz::tick_nohz_full_cpu(cpu) {
        set_next_trigger();
        return;
    }
    crate::timer::nohz::tick_nohz_rearm_local();
}

/// True when a precise (non-jiffies) timebase is active — the tick
/// handler derives jiffies from it instead of counting IRQs.
#[inline]
fn timebase_precise() -> bool {
    crate::drivers::hotpath::timer::hpet::ready() || TSC_ACTIVE.load(Ordering::Acquire) != 0
}

/// Timebase value just before the HPET takes over (continuity offset
/// for the switch; the HPET counter starts near 0 at its enable).
pub fn pre_hpet_time() -> u64 {
    read_time()
}

/// Advance jiffies from the TIMEBASE, not from IRQ counts (riscv64
/// review批次7 discipline): with N CPUs each running a periodic LAPIC
/// timer, per-IRQ increments ran jiffies N times fast.  Deriving
/// `floor((now - base) / period)` makes the update idempotent and
/// multi-CPU safe.  Returns how many jiffies THIS call advanced (RT-2X:
/// 0 marks an hres-only fire — a deadline between grid ticks — which
/// the trap handler turns into the light no-tick-duty exit).
#[inline]
fn increment_jiffies_from_timebase() -> u64 {
    crate::timer::jiffies_update_from_timebase(read_time(), TIME_SLICE_TICKS)
}

/// Clock interrupt handler (called from the x86_64 trap entry).
/// Returns the number of jiffies this fire advanced (RT-2X): 0 marks an
/// hres-only fire (a precise deadline between grid ticks) — the trap
/// handler takes the light exit with no scheduler-tick duty for those.
pub fn timer_interrupt_handler() -> u64 {
    // Wakewatch: per-CPU timer-IRQ arrival count (storm detector — the
    // heartbeat prints the per-second delta) + last-arrival stamp, so a
    // TIMER-SOFTIRQ-STALL report can tell "this CPU's timer IRQs stopped
    // arriving too" (long IRQ-off window: putc/uart_write/console lock)
    // from "ticks kept arriving but softirq dispatch was skipped"
    // (in_softirq deferral / stuck SOFTIRQ_IN_PROGRESS).
    {
        let cpu = crate::arch::cpu_id() as usize;
        if cpu < crate::config::MAX_CPUS {
            crate::timer::WW_TICK_IRQ[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            crate::timer::ww_note_tick_arrival(cpu);
        }
    }
    let advanced = if timebase_precise() {
        // LAPIC-timer phase: idempotent, multi-CPU-safe jiffies.
        increment_jiffies_from_timebase()
    } else {
        // PIT phase: the free-running channel 0 makes every IRQ exactly
        // one tick (single-CPU boot fallback).
        crate::timer::jiffies_bump()
    };

    // RT-2X item 4 (paired-test queue): expire due high-resolution
    // timers HERE, at the top of the IRQ, before the tick body — the
    // reference's hrtimer_interrupt shape. Precise sleeps then wake
    // ahead of the per-tick accounting / scheduler_tick / scribble /
    // softlockup work (measured 30-70 us of body on the debug kernel)
    // instead of behind it; the irq-exit softirq dispatch below finds
    // the wheel already drained (dedupe fast-out). The raise stays:
    // timers armed DURING the delivery (re-arm actions) still get their
    // dispatch in the same IRQ.
    crate::timer::expire_hres_inline();

    // Tick-cached wall second for advisory atime stamps (oc-rc read
    // path) — idempotent relaxed store, once per tick per CPU.
    crate::drivers::hotpath::tick_refresh_cached_secs();

    // vDSO time page refresh (shared with the riscv64 twin)
    crate::mm::hotpath::vdso_data_tick();

    // Timer softirq for TCP timers (retransmission, delayed ACK).
    // sc184 (lane 2, perf-audit F1): the raise was unconditional —
    // 199 raises/s per CPU for a usually-empty TCP table (pa3), each
    // costing an irq-exit softirq dispatch pass at IF=0. The handler
    // (tcp_timer_tick) already early-returns when the table's high-water
    // count is 0, so gating the raise on the same count is
    // behavior-identical minus the wasted dispatch. count() is a
    // high-water mark: once any TCP socket has existed the raise
    // resumes unconditionally (and the handler then has real work).
    if crate::api::net::tcp_table_nonempty() {
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Timer as usize,
        );
    }

    // Hrtimer softirq for software timers. Same F1 gate: skip the raise
    // while no timer has ever been armed AND no high-resolution deadline
    // is due (timer::timer_softirq_work_pending is lock-free: a monotonic
    // ever-armed atomic plus the HRES_NEXT deadline word). Before the
    // first timer this skips the vector entirely; afterwards it never
    // gates (the handler sweeps real timers).
    if crate::timer::timer_softirq_work_pending() {
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Hrtimer as usize,
        );
    }
    advanced
}

}
#[cfg(feature = "x86_64")]
pub use x86_64::*;

#[cfg(feature = "x86_64")]

pub mod hpet {
use core::sync::atomic::{AtomicU64, Ordering};

use crate::config::TIMER_CLOCK_FREQ_HZ;

/// Fixed q35 HPET base (fallback when no ACPI HPET table is found).
const HPET_FIXED_BASE: usize = 0xFED0_0000;

// Register offsets within the HPET block.
const HPET_ID: usize = 0x000; // capabilities (low) / PERIOD fs (high)
const HPET_CFG: usize = 0x010; // general configuration
const HPET_CNTR_LO: usize = 0x0F0; // main counter, low 32 bits
const HPET_CNTR_HI: usize = 0x0F4; // main counter, high 32 bits

/// CAP bits: COUNT_SIZE_CAP (bit 15) — 64-bit main counter supported.
const ID_COUNT_SIZE_CAP: u32 = 1 << 15;
/// CFG bit 0: ENABLE_CNF — main counter runs.
const CFG_ENABLE: u32 = 1;

/// MMIO base (identity-mapped VA) once initialized; 0 = not present.
static BASE: AtomicU64 = AtomicU64::new(0);
/// wildwrite-hunt guard cluster: the QUIET-after-init timekeeper
/// statics live in a page-exclusive 2-page region (page 0 = canary
/// fill, page 1 = the members) so rux:wildwatch (retired debug=wildwatch) can clear RW on their
/// PTEs after init — a wild store then page-faults at the writer's RIP
/// instead of silently landing (b77/wt1 smashed BASE/BASE_LOCKED here;
/// read-side healing survives, attribution did not exist). The hot
/// per-read statics (BASE ready flag, LAST_GOOD) stay OUTSIDE the
/// cluster: they are legitimately written on every clock read.
#[repr(C, align(4096))]
struct WildGuardCluster {
    /// Pattern-filled by wildwatch::arm, verified from the timer cadence
    /// (catches PTE-invisible, DMA-class writers). No legitimate access.
    canary: [u64; 512],
    base_locked: AtomicU64,
    base_locked_inv: AtomicU64,
    div: AtomicU64,
    div_inv: AtomicU64,
    boot_offset: AtomicU64,
    boot_offset_inv: AtomicU64,
    _pad: [u64; 512 - 6],
}
static GUARD: WildGuardCluster = WildGuardCluster {
    canary: [0; 512],
    base_locked: AtomicU64::new(0),
    base_locked_inv: AtomicU64::new(u64::MAX),
    div: AtomicU64::new(0),
    div_inv: AtomicU64::new(u64::MAX),
    boot_offset: AtomicU64::new(0),
    boot_offset_inv: AtomicU64::new(u64::MAX),
    _pad: [0; 512 - 6],
};

/// The guard cluster's VA range (for wildwatch arming/registration).
pub fn wild_guard_range() -> (usize, usize) {
    let s = &GUARD as *const WildGuardCluster as usize;
    (s, core::mem::size_of::<WildGuardCluster>())
}

/// The value BASE held at successful init. The HPET is a FIXED MMIO
/// block — this value can never legitimately change after init. b77
/// capture: a wild kernel write changed BASE 0xffff8900fed00000 →
/// 0xffff89004ed00000 mid-boot (one byte of the physical address); every
/// subsequent clock read dereferenced an unmapped physmap address and
/// the whole boot graph froze (all tasks crawling, serial silent).
/// wt1: BASE and BASE_LOCKED taken together in one smash — see the
/// GUARD cluster doc above (wildwrite-hunt attribution trap).
#[allow(non_snake_case)]
fn BASE_LOCKED() -> &'static AtomicU64 {
    &GUARD.base_locked
}
/// Redundant encoding of BASE_LOCKED (bitwise complement) — see GUARD.
#[allow(non_snake_case)]
fn BASE_LOCKED_INV() -> &'static AtomicU64 {
    &GUARD.base_locked_inv
}
/// HPET ticks per nominal-domain tick (CLOCK_FREQ units). QEMU: 100 MHz
/// HPET / 10 MHz nominal = 10. 0 until `init` validated the period.
#[allow(non_snake_case)]
fn DIV() -> &'static AtomicU64 {
    &GUARD.div
}
/// Redundant encoding of DIV (bitwise complement). A zeroed DIV would
/// panic the divide in read_time_nominal; validation heals it back (or
/// falls back to the q35 constant 10).
#[allow(non_snake_case)]
fn DIV_INV() -> &'static AtomicU64 {
    &GUARD.div_inv
}
/// Value added to the HPET-derived reading so switching the timebase
/// onto it cannot move the clock backward (same discipline as the TSC
/// switch in x86_64.rs).
#[allow(non_snake_case)]
fn BOOT_OFFSET() -> &'static AtomicU64 {
    &GUARD.boot_offset
}
/// Redundant encoding of BOOT_OFFSET (bitwise complement).
#[allow(non_snake_case)]
fn BOOT_OFFSET_INV() -> &'static AtomicU64 {
    &GUARD.boot_offset_inv
}
/// Per-CPU last accepted reading (monotonic clamp — the clocksource
/// watchdog analogue). A garbage read that lands below this CPU's last
/// good time is re-read once and then FROZEN at the last good value
/// (a brief stall beats a minutes-long jump; the timer wheel's hres
/// deadlines are absolute, so a backward jump strands every sleeper
/// until the clock re-reaches the pre-jump value — the dead-stop
/// morphology). Per-CPU slots avoid a shared-line write on the hottest
/// path in the system.
static LAST_GOOD: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Backward steps up to this size are clamped away as jitter (50 ms in
/// the nominal 10 MHz tick domain).
const SMALL_BACKWARD_CLAMP_TICKS: u64 =
    50 * crate::config::TIMER_CLOCK_FREQ_HZ / 1000;

/// Direct-console heal report, rate-gated to one line per 25 jiffies
/// (callers may hold the TIMERS lock — R34 discipline: putchar_no_lock
/// only, never printk from the clock read path).
fn heal_report(prefix: &[u8], bad: u64, good: u64) {
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::drivers::hotpath::timer::get_jiffies();
    let last = LAST.load(Ordering::Relaxed);
    if now.wrapping_sub(last) < 25 {
        return;
    }
    if LAST
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    for &b in prefix {
        crate::console::putchar_no_lock(b);
    }
    let mut v = bad;
    for _ in 0..16 {
        let nb = (v >> 60) as u8;
        crate::console::putchar_no_lock(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
        v <<= 4;
    }
    crate::console::putchar_no_lock(b'-');
    crate::console::putchar_no_lock(b'>');
    let mut v = good;
    for _ in 0..16 {
        let nb = (v >> 60) as u8;
        crate::console::putchar_no_lock(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
        v <<= 4;
    }
    crate::console::putchar_no_lock(b'\n');
}

/// Validate the locked base against its complement copy. Returns the
/// alias VA to read, healing the pair (or falling back to the fixed q35
/// alias) when a wild write has touched either half.
fn validated_read_base() -> usize {
    let locked = BASE_LOCKED().load(Ordering::Acquire);
    let inv = BASE_LOCKED_INV().load(Ordering::Acquire);
    if locked != 0 && locked == !inv {
        return locked as usize;
    }
    // Corrupt pair: a wild write hit BASE_LOCKED and/or its complement.
    // Fall back to the fixed q35 alias — the correct block on this
    // platform (the ACPI table reports the same 0xFED00000) — and heal
    // the pair so later reads take the fast path again. If the locked
    // value merely lost its complement (still plausible), keep it.
    let fallback = {
        use crate::arch::mm::memory_layout::device_alias;
        device_alias(HPET_FIXED_BASE as u64) as usize
    };
    let good = if locked != 0 && (locked as usize) & 0xFFF == 0 {
        // Address still looks structurally valid: re-affirm it.
        locked
    } else {
        fallback as u64
    };
    heal_report(b"hpet: clock base healed ", locked, good);
    // wildwatch: the guard cluster is RO while the tripwire is armed.
    // Under RO the pair cannot have been scribbled through any
    // PTE-visible path, so this branch means a PTE-invisible writer
    // (see WILDWATCH-CANARY) — re-writing the pair would fault on the
    // RO page and masquerade as the wild writer. Skip the stores; the
    // fast path re-validates every read and the fallback above already
    // returns a good address.
    if !guard_armed() {
        BASE_LOCKED().store(good, Ordering::Release);
        BASE_LOCKED_INV().store(!good, Ordering::Release);
    }
    BASE.store(good, Ordering::Release);
    good as usize
}

/// True while rux:wildwatch (retired debug=wildwatch) holds the guard cluster read-only.
fn guard_armed() -> bool {
    #[cfg(feature = "x86_64")]
    {
        crate::debug::wildwatch::is_armed(&GUARD.base_locked as *const AtomicU64 as usize)
    }
    #[cfg(not(feature = "x86_64"))]
    {
        false
    }
}

/// True once the HPET is the active timekeeper.
#[inline]
pub fn ready() -> bool {
    BASE.load(Ordering::Acquire) != 0
}

/// 32-bit MMIO read at `base + off`.
///
/// SAFETY (caller): `base` must be an initialized, mapped HPET block.
#[inline]
fn r32(base: usize, off: usize) -> u32 {
    // INV-MMIO-1: the HPET register file is mapped at the fixed alias
    // this module computes (see init); single u32 inside that mapping.
    crate::safety::mmio::MmioRegion::new(base + off, 4).read_u32(0)
}

/// 32-bit MMIO write at `base + off`.
///
/// SAFETY (caller): `base` must be an initialized, mapped HPET block.
#[inline]
fn w32(base: usize, off: usize, v: u32) {
    crate::safety::mmio::MmioRegion::new(base + off, 4).write_u32(0, v)
}

/// Read the 64-bit main counter (two 32-bit halves).  The HPET-spec
/// discipline compares the HIGH halves around the low-half read: at
/// 100 MHz the low 32 bits advance between any two MMIO reads (so
/// comparing low halves would loop forever), while the high half
/// changes once every 2^32 / 1e8 s ~= 42.9 s.
///
/// The counter is 64-bit on QEMU (COUNT_SIZE_CAP) and at 100 MHz wraps
/// after ~5.8 kyr, so u64 arithmetic needs no wrap handling.
#[inline]
pub fn read_counter() -> u64 {
    // The read address is DERIVED from the validated locked pair, never
    // from BASE alone (wt1 capture: BASE and the old BASE-vs-LOCKED
    // comparison were both defeated by one smash of the adjacent
    // statics). BASE stays a pure ready flag here.
    let base = validated_read_base();
    // Re-arm the ready flag if a wild write zeroed it (ready()==false
    // would degrade read_time() to the coarse jiffies fallback).
    if BASE.load(Ordering::Acquire) as usize != base {
        BASE.store(base as u64, Ordering::Release);
    }
    loop {
        // SAFETY: validated_read_base returned a validated, mapped block.
        let hi1 =  r32(base, HPET_CNTR_HI)  as u64;
        let lo =  r32(base, HPET_CNTR_LO)  as u64;
        let hi2 =  r32(base, HPET_CNTR_HI)  as u64;
        if hi1 == hi2 {
            return (hi1 << 32) | lo;
        }
    }
}

/// Current time in the nominal CLOCK_FREQ domain (HPET-based), hardened
/// against the wild-write engine:
///  1. the counter address is derived from the validated locked pair
///     (fallback: fixed q35 alias);
///  2. DIV is validated (a zeroed DIV must panic-proof fall back);
///  3. BOOT_OFFSET is validated against its complement copy;
///  4. the result is clamped monotonic per CPU — a garbage read that
///     lands below this CPU's last accepted value is re-read once and
///     then FROZEN (never jump the clock backward: hres deadlines are
///     absolute, a backward jump strands every sleeper).
#[inline]
pub fn read_time_nominal() -> u64 {
    let div = {
        let d = DIV().load(Ordering::Acquire);
        let dinv = DIV_INV().load(Ordering::Acquire);
        if d != 0 && d == !dinv {
            d
        } else {
            // Scribbled (or zeroed): heal with the q35 constant — the
            // platform's only validated divisor (init rejects hardware
            // whose period is not an integer multiple of the nominal
            // domain, and this kernel targets QEMU q35/virt).
            const QEMU_DIV: u64 = 10; // 100 MHz HPET / 10 MHz nominal
            heal_report(b"hpet: clock div healed ", d, QEMU_DIV);
            if !guard_armed() {
                // (see validated_read_base: no stores onto the RO guard)
                DIV().store(QEMU_DIV, Ordering::Release);
                DIV_INV().store(!QEMU_DIV, Ordering::Release);
            }
            QEMU_DIV
        }
    };
    let offset = {
        let o = BOOT_OFFSET().load(Ordering::Acquire);
        let oinv = BOOT_OFFSET_INV().load(Ordering::Acquire);
        if o == !oinv {
            o
        } else {
            // Complement lost: the plain value is the better guess (a
            // smashed value usually reads as a pointer, not the small
            // early-boot offset; the monotonic clamp below catches any
            // residual poison anyway). Heal the pair.
            heal_report(b"hpet: clock offset healed ", oinv, o);
            if !guard_armed() {
                BOOT_OFFSET_INV().store(!o, Ordering::Release);
            }
            o
        }
    };
    let raw = read_counter() / div + offset;

    // Per-CPU monotonic guard (clocksource watchdog analogue). The read
    // is already address/DIV/offset validated; what remains is read
    // jitter and transient residual poison:
    //  - a SMALL backward step (<= 50 ms) is clamped to the last good
    //    value (tearing/rounding artifacts must never move the clock
    //    back — hres deadlines are absolute);
    //  - a LARGE backward step is a CORRECTION (e.g. a forward-poisoned
    //    value was accepted a moment before validation healed the
    //    offset) and is HONORED — freezing at a poisoned-high value
    //    would stall the clock forever. Logged so the event is visible.
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return raw;
    }
    let last = LAST_GOOD[cpu].load(Ordering::Acquire);
    if raw >= last.saturating_sub(SMALL_BACKWARD_CLAMP_TICKS) {
        let accept = raw.max(last);
        LAST_GOOD[cpu].store(accept, Ordering::Release);
        return accept;
    }
    heal_report(b"hpet: clock corrected backward ", last, raw);
    LAST_GOOD[cpu].store(raw, Ordering::Release);
    raw
}

/// Discover, enable, and verify the HPET.  Idempotent; returns true when
/// the HPET became (or already was) the active timekeeper.
///
/// Runs with interrupts disabled on the BSP before any secondary CPU is
/// online (called from main.rs right after the device mappings exist),
/// so the BASE/DIV/BOOT_OFFSET publication order needs no locking.
pub fn init() -> bool {
    if ready() {
        return true;
    }
    // Debugging gate (debug discipline): "nohpet" keeps the calibrated-TSC
    // timebase so clocksource behavior can be A/B compared.
    if crate::cmdline::get_param("nohpet").is_some() {
        return false;
    }

    // ACPI HPET table: header (36 bytes) + timer block ID (8) + generic
    // address structure (12, address u64 at +8) + counter/flags.  Fall
    // back to the fixed q35 base when the table is missing.
    let base = crate::drivers::hotpath::intc::apic::find_hpet_base().unwrap_or(HPET_FIXED_BASE);
    if base == 0 || base & 0xFFF != 0 {
        return false;
    }
    // Runtime counter reads happen from timer-interrupt context under
    // USER CR3s — route them through the kernel-half device alias (Linux
    // fixmap discipline), never through a low identity VA. The alias
    // window [0xb0000000, 4GB) covers q35's fixed HPET base; an ACPI
    // table outside it gets one explicit alias page.
    let alias_va = {
        use crate::arch::mm::memory_layout::{
            device_alias, DEVICE_ALIAS_PHYS_BASE,
        };
        let in_window = (base as u64) >= DEVICE_ALIAS_PHYS_BASE as u64
            && (base as u64) < 0x1_0000_0000;
        if !in_window {
            let flags = crate::arch::mm::pagetable::PageTableEntry::P
                | crate::arch::mm::pagetable::PageTableEntry::RW
                | crate::arch::mm::pagetable::PageTableEntry::ACCESSED
                | crate::arch::mm::pagetable::PageTableEntry::DIRTY
                | crate::arch::mm::pagetable::PageTableEntry::IO;
            // Mapping one uncached alias page for the HPET block in the
            // kernel half (reserved firmware region, never RAM) — the
            // map walker is a safe fn since round 1.
            {
                let va = 0xffff_8a00_0000_0000u64 + (base as u64 & 0xF_FFFF_F000);
                crate::arch::mm::mmu_init::map_kernel_region(
                    va,
                    base as u64 & !0xFFFu64,
                    0x1000,
                    flags,
                );
                va
            }
        } else {
            device_alias(base as u64)
        }
    };
    let base = alias_va as usize;

    // Validate the block: vendor ID nonzero, 64-bit counter capability,
    // period within the HPET spec bound (<= 100 ns) and an integer
    // multiple of the nominal 10 MHz domain.
    // SAFETY: base validated above.
    let id =  r32(base, HPET_ID) ;
    if id >> 16 == 0 || id & ID_COUNT_SIZE_CAP == 0 {
        return false;
    }
    let period_fs =  r32(base, HPET_ID + 4)  as u64;
    // 1 fs .. 100 ns; zero or huge periods mean no HPET / broken model.
    if period_fs == 0 || period_fs > 100_000_000 {
        return false;
    }
    let hpet_hz = 1_000_000_000_000_000 / period_fs;
    if hpet_hz < TIMER_CLOCK_FREQ_HZ || hpet_hz % TIMER_CLOCK_FREQ_HZ != 0 {
        // Nominal-domain conversion would not be exact (real HW with a
        // non-decade period): keep the calibrated-TSC timebase instead.
        return false;
    }
    let div = hpet_hz / TIMER_CLOCK_FREQ_HZ;

    // Enable the main counter (LEGACY stays clear: the PIT must keep
    // ticking for the IRQ0 fallback path and the TSC/LAPIC calibration).
    // SAFETY: base validated above.
    let cfg =  r32(base, HPET_CFG) ;
     w32(base, HPET_CFG, cfg | CFG_ENABLE) ;

    // Verify it actually counts: at >= 10 MHz the counter must advance
    // during a short bounded spin.  Retry with a longer spin before
    // giving up (first MMIO reads after device bring-up can be slow).
    let mut ok = false;
    for spin in [20_000u64, 2_000_000] {
        // SAFETY: base validated above.
        let c0 =  read_counter_raw(base) ;
        for _ in 0..spin {
            core::hint::spin_loop();
        }
        // SAFETY: base validated above.
        let c1 =  read_counter_raw(base) ;
        if c1 > c0 {
            ok = true;
            break;
        }
    }
    if !ok {
        // Disable again — do not leave a half-initialized counter.
        // SAFETY: base validated above.
         w32(base, HPET_CFG, cfg) ;
        return false;
    }

    // Publish: offset first (the pre-HPET clock value at the switch
    // instant), then the base that flips read_time() onto the HPET.
    // Interrupts are off and APs are parked, so this is race-free.
    // The complement copies arm the read-path validation (wt1: the
    // wild-write engine took BASE and BASE_LOCKED together — adjacent
    // statics — and poisoned every clock read; the pair + fixed-alias
    // fallback make the read path derive a good address regardless).
    let continuity = crate::drivers::hotpath::timer::x86_64::pre_hpet_time();
    DIV().store(div, Ordering::Release);
    DIV_INV().store(!div, Ordering::Release);
    BOOT_OFFSET().store(continuity, Ordering::Release);
    BOOT_OFFSET_INV().store(!continuity, Ordering::Release);
    BASE.store(base as u64, Ordering::Release);
    BASE_LOCKED().store(base as u64, Ordering::Release);
    BASE_LOCKED_INV().store(!(base as u64), Ordering::Release);
    true
}

/// Calibrate the TSC against the initialized HPET and promote the TSC to
/// timekeeper (sc109: the OH launcher has no vDSO clock_gettime — every
/// call paid a syscall AND an HPET MMIO read, a VM exit under KVM, and
/// the launcher burned its whole 3 s appspawn watchdog in that path).
///
/// KVM's TSC is invariant and vCPU-synchronized; both clocks derive from
/// QEMU's virtual clock, so the only error is measurement skew (~us)
/// across a 20 ms window (≪ 1 ppm). A 1 Hz resync
/// (x86_64::resync_tsc_against_hpet) bounds residual drift.
pub fn calibrate_tsc() -> bool {
    use crate::drivers::hotpath::timer::x86_64::{promote_tsc_timekeeper, CLOCK_FREQ, TSC_MULSHIFT};
    if !ready() {
        return false;
    }
    // Validated DIV/offset (same discipline as read_time_nominal).
    let (div, offset) = {
        let d = DIV().load(Ordering::Acquire);
        let dinv = DIV_INV().load(Ordering::Acquire);
        let o = BOOT_OFFSET().load(Ordering::Acquire);
        if d == 0 || d != !dinv {
            return false; // unhealed clock — keep the MMIO path
        }
        (d, o)
    };
    // 20 ms calibration window at 100 MHz HPET counts.
    const WINDOW: u64 = 2_000_000;
    let h0 = read_counter();
    let t0 = crate::arch::cpu::read_counter();
    let mut h1 = read_counter();
    while h1.saturating_sub(h0) < WINDOW {
        h1 = read_counter();
    }
    let t1 = crate::arch::cpu::read_counter();
    let dh = h1.saturating_sub(h0);
    let dt = t1.saturating_sub(t0);
    let nom_delta = dh / div;
    if nom_delta < WINDOW / div / 2 || dt == 0 {
        return false;
    }
    let tsc_hz = (dt as u128) * (CLOCK_FREQ as u128) / (nom_delta as u128);
    if tsc_hz == 0 || tsc_hz > 16_000_000_000u128 {
        return false;
    }
    // Nearest rounding, NOT ceil (sc110-114: a ceiled mult at shift 12
    // ran the mapping 4.796% fast — see the TSC_MULSHIFT note in
    // x86_64.rs). u128 math: CLOCK_FREQ << 32 = 4.3e16 fits, but the
    // u128 path keeps the tsc_hz divide exact.
    let mult = ((((CLOCK_FREQ as u128) << TSC_MULSHIFT) + (tsc_hz as u128) / 2)
        / (tsc_hz as u128)) as u64;
    let nominal_at_t1 = h1 / div + offset;
    promote_tsc_timekeeper(mult, t1, nominal_at_t1);
    crate::pr_info!(
        "hpet: TSC timekeeper promoted (tsc {} Hz, mult {}, 20ms window)",
        tsc_hz,
        mult
    );
    true
}

/// One 64-bit counter read against an explicit base (init-time verify).
/// Same high-half discipline as [`read_counter`]; at most one re-read
/// when the 429 s high-half boundary lands mid-read.
///
/// SAFETY (caller): `base` must be a mapped HPET block.
fn read_counter_raw(base: usize) -> u64 {
    
        let hi1 = r32(base, HPET_CNTR_HI) as u64;
        let lo = r32(base, HPET_CNTR_LO) as u64;
        let hi2 = r32(base, HPET_CNTR_HI) as u64;
        if hi1 == hi2 {
            (hi1 << 32) | lo
        } else {
            let hi = r32(base, HPET_CNTR_HI) as u64;
            let lo = r32(base, HPET_CNTR_LO) as u64;
            (hi << 32) | lo
        }
    
}

}

#[cfg(feature = "aarch64")]

pub mod aarch64 {
use core::sync::atomic::{AtomicU64, Ordering};

/// System clock frequency (HZ) - from config
pub const HZ: u64 = crate::config::KERNEL_HZ as u64;

/// Cached CNTFRQ_EL0 (read once at first use; 64 MHz class on QEMU virt)
static CLOCK_FREQ_CACHED: AtomicU64 = AtomicU64::new(0);

/// Nominal ticks per period — computed from the LIVE frequency
#[inline]
fn ticks_per_period() -> u64 {
    clock_freq() / HZ
}

/// Timer counter frequency (runtime from CNTFRQ_EL0)
#[inline]
pub fn clock_freq() -> u64 {
    let f = CLOCK_FREQ_CACHED.load(Ordering::Acquire);
    if f != 0 {
        return f;
    }
    let live = crate::arch::cpu::get_counter_freq();
    CLOCK_FREQ_CACHED
        .compare_exchange(0, live, Ordering::AcqRel, Ordering::Acquire)
        .ok();
    live
}

/// Compile-time nominal frequency (config parity; runtime CNTFRQ is
/// authoritative — QEMU virt aarch64 does NOT run at the 10 MHz the
/// riscv64 CLINT does)
pub const CLOCK_FREQ: u64 = crate::config::TIMER_CLOCK_FREQ_HZ;

// (JIFFIES/JIFFIES_BASE hoisted to crate::timer — rung4 prep move 1)

/// Get current jiffies value (framework static)
#[inline]
pub fn get_jiffies() -> u64 {
    crate::timer::get_jiffies()
}

/// Test-only deterministic jiffy advance (unit-test harness): the tests
/// run with the timer interrupt disabled and — since R87 keeps secondary
/// harts interrupt-less until boot_complete — nothing increments JIFFIES
/// during the suite. Tests that need "time has passed" semantics (e.g.
/// the tcp-karn RTT-sampling control) bump the counter explicitly
/// instead of relying on incidental ticks.
#[cfg(feature = "unit-test")]
pub fn test_advance_jiffies(n: u64) {
    crate::timer::test_advance_jiffies(n)
}

/// Floor-map an absolute timebase value onto the jiffy grid — the
/// read side of increment_jiffies' `floor((now - base) / period)`
/// derivation (0 before the grid origin is established). The timer
/// wheel uses this to derive an hres entry's first-check jiffy from
/// its precise counter deadline, so routing never trusts
/// caller-supplied jiffy bookkeeping.
#[inline]
pub fn jiffy_floor_of_time(t: u64) -> u64 {
    crate::timer::jiffy_floor_of_time(t, ticks_per_period())
}

/// Coarse time in nanoseconds since boot, snapped to the NOMINAL tick
/// grid and computed from the LIVE timebase (riscv64 twin has the LTP
/// utime01 history; same contract here)
#[inline]
pub fn coarse_ns_since_boot() -> u64 {
    let period_ns = 1_000_000_000 / HZ;
    read_time() / ticks_per_period() * period_ns
}

/// Advance jiffies from the TIMEBASE (idempotent, multi-cpu safe)
#[inline]
fn increment_jiffies() {
    crate::timer::jiffies_update_from_timebase(read_time(), ticks_per_period());
}

/// Convert jiffies to milliseconds
#[inline]
pub const fn jiffies_to_msecs(jiffies: u64) -> u64 {
    jiffies * 1000 / HZ
}

/// Convert milliseconds to jiffies
#[inline]
pub const fn msecs_to_jiffies(msecs: u64) -> u64 {
    msecs * HZ / 1000
}

/// Read current time (CNTVCT_EL0 virtual counter)
#[inline]
pub fn read_time() -> u64 {
    crate::arch::cpu::read_counter()
}

/// High-resolution nanoseconds since boot (u128 intermediate — see the
/// riscv64 twin's overflow note)
#[inline]
pub fn monotonic_ns_since_boot() -> u64 {
    let freq = clock_freq() as u128;
    ((read_time() as u128) * 1_000_000_000u128 / freq) as u64
}

/// Arm the EL1 virtual timer to an absolute CNTVCT deadline
#[inline]
pub fn set_timer(deadline: u64) {
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

/// Set next timer interrupt, aligned to the NOMINAL tick grid but never
/// later than the earliest high-resolution deadline (riscv64 twin logic)
pub fn set_next_trigger() {
    let current = read_time();
    let tpp = ticks_per_period();
    let mut deadline = (current / tpp + 1) * tpp;
    let hres = crate::timer::hres_next_deadline();
    if hres < deadline {
        deadline = hres.max(current + 1);
    }
    set_timer(deadline);
}

/// Re-arm this cpu's timer to min(next grid tick, earliest hres deadline)
pub fn rearm_for_hres() {
    set_next_trigger();
}

/// Clock interrupt handler (called from the EL1 IRQ trap path)
pub fn timer_interrupt_handler() {
    // 1. Update jiffies counter
    increment_jiffies();

    // 1.5 vDSO time-data page refresh
    crate::mm::hotpath::vdso_data_tick();

    // 2. Raise Timer softirq for TCP timer processing (gated on table
    //    occupancy — see the riscv64 twin / sc184)
    if crate::api::net::tcp_table_nonempty() {
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Timer as usize,
        );
    }

    // 2.4 RT-2X item 4 (paired-test queue): expire due high-resolution
    //     timers at the TOP of the IRQ — the reference's
    //     hrtimer_interrupt shape (the x86_64 twin carries the measured
    //     story; run/RT-2X.md).
    crate::timer::expire_hres_inline();

    // 2.5 Raise Hrtimer softirq when work is pending
    if crate::timer::timer_softirq_work_pending() {
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Hrtimer as usize,
        );
    }
}

}
#[cfg(feature = "aarch64")]
pub use aarch64::*;

}

// ════════════════ rtc ═════════════════════════════════
// (was rtc.rs)
// Goldfish RTC driver — boot-time wall clock source.
//
// QEMU's `virt` machine instantiates a `goldfish-rtc` device at
// 0x101000 unconditionally (hw/riscv/virt.c: `sysbus_create_simple
// ("goldfish-rtc", ...)`; visible in the DTB as "google,goldfish-rtc").
// No `-device` flag is needed. The device exposes the host wall clock
// (as of VM start) plus guest uptime as a 64-bit nanosecond counter,
// split across two 32-bit little-endian MMIO registers.
//
// Why not fw_cfg `FW_CFG_RTC_TIME` (selector 0x15): the entry does not
// exist in QEMU v10.2.2 — `hw/nvram/fw_cfg.c`, `include/hw/nvram/
// fw_cfg.h`, the standard-headers copy and `docs/specs/fw_cfg.rst`
// contain no RTC entry at all, and the RISC-V `virt` machine's
// `create_fw_cfg()` registers only `FW_CFG_NB_CPUS` (+ the common
// set). Selecting an unregistered key yields a zero-length entry, so
// every read returns 0 (verified empirically: the selector accepts
// writes and the signature item reads "QEMU", but 0x15 reads as 0).
// The goldfish RTC is the virt machine's RTC.
//
// Register map (32-bit LE, all accesses must be exactly 4 bytes):
//   0x00 TIME_LOW    read: bits  0-31 of the ns counter
//   0x04 TIME_HIGH   read: bits 32-63 of the ns counter
//   0x08+ alarm/IRQ registers — unused here
//
// Model: REALTIME = monotonic (CLINT `time` CSR) + WALL_EPOCH_OFFSET_SECS
// (syscall/time.rs). `rtc_init_wall_clock()` reads the device once at
// boot and derives the offset; settimeofday/clock_settime re-derive it.
/// MMIO base of the goldfish RTC on the QEMU virt platform.
pub const GOLDFISH_RTC_BASE: u64 = 0x101_000;

/// Kernel-half alias of the device page used for actual register reads.
///
/// Identity-window history (perf review 2026-10-31): this used to be
/// `mmio_alias(0x101_000)` — on x86_64 that is the identity passthrough,
/// and the read only worked through the bootstrap identity link in
/// PML4[0] (dropped at boot_stack_continuation). The device page is now
/// ioremapped into the kernel-half dynamic alias arena at init — same
/// treatment the virtio BARs get — and GOLDFISH_RTC_VA carries the
/// resolved kernel VA (still valid under every user CR3).
pub static GOLDFISH_RTC_VA: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0x101_000);

/// Size of the mapped device region (one page — the register file is
/// 0x20 bytes but the DTB reg spec is the full page).
pub const GOLDFISH_RTC_SIZE: u64 = 0x1000;

const REG_TIME_LOW: u64 = 0x00;
const REG_TIME_HIGH: u64 = 0x04;

/// Read one 32-bit device register.
///
/// # Safety (internal)
/// The page at GOLDFISH_RTC_BASE is identity-mapped with device flags by
/// `setup_device_mappings()`; this helper is only reachable after that.
#[inline]
fn read_reg(offset: u64) -> u32 {
    let va = GOLDFISH_RTC_VA.load(core::sync::atomic::Ordering::Relaxed);
    unsafe { core::ptr::read_volatile((va + offset) as *const u32) }
}

/// Resolve the device page into the kernel-half dynamic alias arena.
/// Called once from rtc init before the first register read; the atomic
/// VA makes the read side lock-free afterwards.
#[cfg(feature = "x86_64")]
pub fn rtc_init_alias() {
    if let Some(va) =
        crate::arch::x86_64::mm::mmu_init::ioremap_bar_kernel(GOLDFISH_RTC_BASE, GOLDFISH_RTC_SIZE)
    {
        GOLDFISH_RTC_VA.store(va, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Read the RTC's nanosecond counter (host wall time at VM start + guest
/// uptime, in ns since the Unix epoch).
///
/// The counter is latched one 32-bit half at a time, so a naive
/// LOW/HIGH pair can tear across a low-word wrap (every ~4.3 s of ns).
/// Linux's goldfish-rtc driver re-reads TIME_HIGH around the LOW read and
/// retries until it is stable — same protocol here.
pub fn read_ns() -> u64 {
    loop {
        let hi1 = read_reg(REG_TIME_HIGH) as u64;
        let lo = read_reg(REG_TIME_LOW) as u64;
        let hi2 = read_reg(REG_TIME_HIGH) as u64;
        if hi1 == hi2 {
            return (hi1 << 32) | lo;
        }
    }
}

/// Sanity floor for a plausible RTC reading in NANOseconds: 2001-09-09
/// (1e9 s = 1e18 ns) — filters out a zero/garbage read without rejecting
/// any realistic host date. (The device is read as ns, not s!)
const MIN_PLAUSIBLE_RTC_NS: u64 = 1_000_000_000_000_000_000;

/// Read the RTC once at boot and derive the wall-clock epoch offset.
///
/// The offset is rounded to the nearest second so the composite
/// (monotonic + offset) REALTIME stays within half a second of the RTC
/// value; sub-second accuracy would need a fractional offset in the
/// model (vDSO data page carries whole seconds only).
///
/// Must run after `setup_device_mappings()` (device page mapped) and
/// before `vdso_init()` (which snapshots the offset into the vDSO data
/// page). If the device does not report a plausible time the offset
/// stays 0 and the system runs on monotonic boot time.
pub fn rtc_init_wall_clock() {
    let rtc_ns = read_ns();
    if rtc_ns < MIN_PLAUSIBLE_RTC_NS {
        return;
    }

    // Monotonic clock in ns at (almost) the same instant: 10 MHz CLINT
    // `time` CSR = 100 ns per cycle exactly.
    let mono_ns = crate::arch::cpu::read_time().saturating_mul(100);

    // wall = mono + offset  ⇒  offset = rtc - mono (round to nearest).
    let offset_secs = rtc_ns.saturating_sub(mono_ns).saturating_add(500_000_000) / 1_000_000_000;
    crate::timer::set_wall_epoch_offset_secs(offset_secs);
}

/// Current wall-clock seconds since the Unix epoch.
///
/// Single canonical timestamp source for filesystems (ext4 mtime/ctime,
/// tmpfs, rootfs) and UTIME_NOW: monotonic seconds + the wall offset.
/// Once the RTC has been read (or settimeofday ran) this is real UTC.
///
/// Coarse, tick-grid-snapped (see drivers/timer::coarse_ns_since_boot):
/// this MUST agree with what clock_gettime(CLOCK_REALTIME_COARSE)
/// returns — LTP utime01 brackets a utime(NULL) stamp between two
/// CLOCK_REALTIME_COARSE samples, and the old live-rdtime conversion
/// here ran ahead of the coarse reads whenever tick-handler updates
/// lagged the timebase.
pub fn wall_secs() -> u64 {
    (crate::drivers::hotpath::timer::coarse_ns_since_boot()
        + crate::timer::wall_epoch_offset_ns())
        / 1_000_000_000
}

/// Tick-cached wall second for ADVISORY stamps only (oc-rc read-path
/// relatime gate): one relaxed load instead of a TSC read + divide per
/// read(2). Refreshed from `wall_secs()` by the timer tick; stale by at
/// most one tick. Never a substitute for `wall_secs()` where LTP
/// brackets a stamp against CLOCK_REALTIME_COARSE (utime/utimensat keep
/// the exact form). 0 before the first refresh — callers treat 0 as
/// "not cached yet" and fall back.
static CACHED_WALL_SECS: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Per-tick refresh (timer ISR, idempotent, multi-CPU safe).
pub fn tick_refresh_cached_secs() {
    let s = wall_secs() as u32;
    CACHED_WALL_SECS.store(s, core::sync::atomic::Ordering::Relaxed);
}

/// Cached advisory wall second (0 = not yet refreshed).
#[inline]
pub fn cached_wall_secs() -> u32 {
    CACHED_WALL_SECS.load(core::sync::atomic::Ordering::Relaxed)
}

// ════════════════ blkdev ══════════════════════════════
// (was blkdev/mod.rs)
// Block device driver layer
//
// Core concepts:
// - `struct gendisk`: Block device representation
// - `struct block_device`: Block device instance
// - `struct request_queue`: Request queue
// - `struct bio`: I/O descriptor
unsafe impl Send for GenDisk {}
unsafe impl Sync for GenDisk {}

impl GenDisk {
    /// Create new block device
    pub fn new(
        name: &'static str,
        major: u32,
        minors: u32,
        block_size: u32,
        ops: Option<&'static BlockDeviceOps>,
    ) -> Self {
        Self {
            name,
            major,
            first_minor: 0,
            minors,
            capacity: AtomicU64::new(0),
            block_size,
            ops,
            private_data: None,
            request_fn: None,
            async_read_fn: None,
            async_write_fn: None,
            async_read_sg_fn: None,
            async_write_sg_fn: None,
        }
    }

    /// Set capacity
    pub fn set_capacity(&self, sectors: u64) {
        self.capacity.store(sectors, Ordering::Release);
    }

    /// Get capacity
    pub fn get_capacity(&self) -> u64 {
        self.capacity.load(Ordering::Acquire)
    }

    /// Set private data
    pub fn set_private_data(&mut self, data: *mut u8) {
        self.private_data = Some(data);
    }

    /// Resolve the disk's private data as a shared `&T` — the ONE
    /// audited edge for the driver-private slot (INV-BLKDEV-3).
    ///
    /// INV-BLKDEV-3 (private slot): `private_data`, when set at device
    /// init before registration, points to a `T` that lives at least as
    /// long as the registered disk (the machine's lifetime) and is not
    /// mutated through this shared view while the disk is registered.
    pub fn private<T>(&self) -> Option<&T> {
        // SAFETY: INV-BLKDEV-3 — drivers store an init-time, machine-
        // lifetime object in the slot; the shared borrow here never
        // outlives the disk's registration.
        self.private_data.map(|p| unsafe { &*(p as *const T) })
    }

    /// Set request handler function
    pub fn set_request_fn(&mut self, f: fn(&mut Request)) {
        self.request_fn = Some(f);
    }

    /// Set async read function (for async I/O submission without blocking).
    /// Registering `f` asserts the INV-BLKDEV-2 contract on its behalf.
    pub fn set_async_read_fn(
        &mut self,
        f: fn(&GenDisk, u64, &mut [u8], &crate::fs::io_completion::IoCompletion) -> i32,
    ) {
        self.async_read_fn = Some(f);
    }

    /// Set async write function (for async I/O submission without blocking)
    pub fn set_async_write_fn(
        &mut self,
        f: fn(&GenDisk, u64, &[u8], &crate::fs::io_completion::IoCompletion) -> i32,
    ) {
        self.async_write_fn = Some(f);
    }

    /// Set async SG read function (fused multi-segment requests)
    pub fn set_async_read_sg_fn(
        &mut self,
        f: fn(&GenDisk, u64, &[DmaSeg], &crate::fs::io_completion::IoCompletion) -> i32,
    ) {
        self.async_read_sg_fn = Some(f);
    }

    /// Set async SG write function (fused multi-segment requests)
    pub fn set_async_write_sg_fn(
        &mut self,
        f: fn(&GenDisk, u64, &[DmaSeg], &crate::fs::io_completion::IoCompletion) -> i32,
    ) {
        self.async_write_sg_fn = Some(f);
    }

    /// True when the device accepts fused SG READ requests.
    pub fn has_async_read_sg(&self) -> bool {
        self.async_read_sg_fn.is_some()
    }

    /// True when the device accepts fused SG WRITE requests.
    pub fn has_async_write_sg(&self) -> bool {
        self.async_write_sg_fn.is_some()
    }
}


/// Kick every registered disk whose transport may hold quietly-submitted
/// chains (rung4 prep move 2): the block layer's drain/recovery paths
/// call this INSTEAD of naming any driver — each registered disk's
/// ops.kick (if present) doorbells its own queue. Disks whose transport
/// notifies inline (ops.kick == None) are skipped.
pub fn kick_all_pending_disks() {
    // The registry walk (the manager's own iteration contract —
    // INV-BLKDEV-1's stable Box addressing).
    let disks = BLOCK_MANAGER.disks.lock();
    for d in disks.iter() {
        if let Some(gd) = d {
            if let Some(kick) = gd.ops.and_then(|ops| ops.kick) {
                kick(gd);
            }
        }
    }
}

unsafe impl Send for BlockDeviceManager {}
unsafe impl Sync for BlockDeviceManager {}

impl BlockDeviceManager {
    const fn new() -> Self {
        Self {
            disks: Spinlock::new(Vec::new()),
            major_next: AtomicU32::new(1),
        }
    }

    /// Register block device
    pub fn register_disk(&self, disk: Box<GenDisk>) -> Result<(), &'static str> {
        let mut disks = self.disks.lock();

        // Check if device number is already in use
        for d in disks.iter() {
            if let Some(ref gd) = d {
                if gd.major == disk.major {
                    return Err("Major number already in use");
                }
            }
        }

        disks.push(Some(disk));
        Ok(())
    }

    /// Find block device.
    ///
    /// INV-BLKDEV-1 (unsafe-slim round 2): the returned `&'static GenDisk`
    /// is sound because a registered disk is boxed ONCE into the manager's
    /// Vec and never removed (no unregister path exists), and a Box's heap
    /// allocation is address-stable across Vec growth. Every raw-pointer
    /// hand-out this used to require is gone: callers hold a typed shared
    /// reference whose lifetime is the machine's.
    pub fn get_disk(&self, major: u32) -> Option<&'static GenDisk> {
        let disks = self.disks.lock();

        for d in disks.iter() {
            if let Some(ref gd) = d {
                if gd.major == major {
                    // SAFETY: INV-BLKDEV-1 above — the Box outlives the
                    // lock and the machine; the allocation is stable.
                    return Some(unsafe { &*(gd.as_ref() as *const GenDisk) });
                }
            }
        }

        None
    }

    /// Submit I/O request
    pub fn submit_request(&self, disk: &GenDisk, req: &mut Request) -> i32 {
        if let Some(request_fn) = disk.request_fn {
            // The registration edge (INV-BLKDEV-2) carries the contract;
            // the call is plain Rust now.
            request_fn(req);
            0  // Success
        } else {
            -6  // ENXIO
        }
    }
}


pub fn blkdev_read_async(
    disk: &'static GenDisk,
    sector: u64,
    buf: &mut [u8],
    completion: &crate::fs::io_completion::IoCompletion,
) -> Result<(), i32> {
    {
        if let Some(async_fn) = disk.async_read_fn {
            // /proc/diskstats: the async path does not go through
            // submit_request, so it is counted here.
            let major = disk.major;
            let minor = disk.first_minor;
            let sectors = (buf.len() / 512) as u64;
            let t0 = crate::drivers::hotpath::timer::read_time();
            // block:block_rq_issue (async read): the completion lands
            // later in the device's IRQ walker, which carries the
            // geometry the submit stores in the pending entry.
            if crate::trace::EVENT_BLOCK_RQ_ISSUE.armed() {
                // SAFETY: current task's comm is read for the record only.
                let comm = crate::sched::hotpath::current().map(|cur| (*cur).comm());
                if let Some(comm) = comm {
                    crate::trace::trace_block_rq_issue(
                        (major << 20) | minor,
                        sector,
                        sectors as u32,
                        buf.len() as u32,
                        0,
                        "R",
                        comm,
                    );
                }
            }
            // The registration edge (INV-BLKDEV-2) carries the contract;
            // the call is plain Rust now.
            let ret = async_fn(disk, sector, buf, completion);
            let elapsed_ns = crate::drivers::hotpath::timer::read_time().saturating_sub(t0) * 100
                / crate::config::TIMER_CLOCK_FREQ_HZ;
            note_io(major, minor, false, sectors, elapsed_ns);
            if ret < 0 {
                Err(ret)
            } else {
                Ok(())
            }
        } else {
            Err(-6)  // ENXIO — device doesn't support async I/O
        }
    }
}

static BLOCK_MANAGER: BlockDeviceManager = BlockDeviceManager::new();

/// Submit an async block write. Returns immediately; completion is signaled
/// later via interrupt.
///
/// The WRITE counterpart of [`blkdev_read_async`] (the plugged-write
/// primitive's device entry): `buf` is the DMA SOURCE, so it must remain
/// valid AND UNMODIFIED until `completion` fires — unlike a read, where only
/// freeing the DMA target was forbidden. Callers that cannot promise buffer
/// stability across the submission window must submit from a private copy
/// (bio's WriteBatch does exactly that).
///
/// # Arguments
/// * `disk` - Block device
/// * `sector` - Starting sector (512-byte units)
/// * `buf` - Data buffer (stable until completion)
/// * `completion` - IoCompletion to signal on completion
///
/// # Returns
/// Ok(()) if submitted successfully, Err on failure (the caller falls back
/// to a synchronous write — correctness is never lost, only the batching).
pub fn blkdev_write_async(
    disk: &'static GenDisk,
    sector: u64,
    buf: &[u8],
    completion: &crate::fs::io_completion::IoCompletion,
) -> Result<(), i32> {
    {
        if let Some(async_fn) = disk.async_write_fn {
            // /proc/diskstats: the async path does not go through
            // submit_request, so it is counted here (is_write = true).
            let major = disk.major;
            let minor = disk.first_minor;
            let sectors = (buf.len() / 512) as u64;
            let t0 = crate::drivers::hotpath::timer::read_time();
            // block:block_rq_issue (async write): completion lands in the
            // device's IRQ walker, which carries the geometry this submit
            // stores in the pending entry.
            if crate::trace::EVENT_BLOCK_RQ_ISSUE.armed() {
                // SAFETY: current task's comm is read for the record only.
                let comm = crate::sched::hotpath::current().map(|cur| (*cur).comm());
                if let Some(comm) = comm {
                    crate::trace::trace_block_rq_issue(
                        (major << 20) | minor,
                        sector,
                        sectors as u32,
                        buf.len() as u32,
                        0,
                        "W",
                        comm,
                    );
                }
            }
            // The registration edge (INV-BLKDEV-2) carries the contract;
            // the call is plain Rust now.
            let ret = async_fn(disk, sector, buf, completion);
            let elapsed_ns = crate::drivers::hotpath::timer::read_time().saturating_sub(t0) * 100
                / crate::config::TIMER_CLOCK_FREQ_HZ;
            note_io(major, minor, true, sectors, elapsed_ns);
            if ret < 0 {
                Err(ret)
            } else {
                Ok(())
            }
        } else {
            Err(-6)  // ENXIO — device doesn't support async writes
        }
    }
}

/// Submit a fused async scatter-gather READ: ONE device request covering
/// the segments' combined sector range starting at `sector`. This is the
/// merge endpoint the batch layers call after fusing device-adjacent
/// spans — one block_rq_issue per merged request, exactly the reference's
/// request shape.
///
/// The segments must cover one CONTIGUOUS device range (each seg is
/// `len` bytes; the request is `sector..sector + total_len/512`). Each
/// segment's buffer must remain a valid DMA target until the completion
/// fires.
///
/// Returns Ok(()) if submitted, Err on failure (caller falls back to
/// per-segment submissions — correctness is never lost, only the fusion).
pub fn blkdev_read_async_sg(
    disk: &'static GenDisk,
    sector: u64,
    segs: &[DmaSeg],
    completion: &crate::fs::io_completion::IoCompletion,
) -> Result<(), i32> {
    {
        if let Some(async_fn) = disk.async_read_sg_fn {
            let major = disk.major;
            let minor = disk.first_minor;
            let total: u64 = segs.iter().map(|s| (s.len / 512) as u64).sum();
            let t0 = crate::drivers::hotpath::timer::read_time();
            if crate::trace::EVENT_BLOCK_RQ_ISSUE.armed() {
                // SAFETY: current task's comm is read for the record only.
                let comm = crate::sched::hotpath::current().map(|cur| (*cur).comm());
                if let Some(comm) = comm {
                    crate::trace::trace_block_rq_issue(
                        (major << 20) | minor,
                        sector,
                        total as u32,
                        (total * 512) as u32,
                        0,
                        "R",
                        comm,
                    );
                }
            }
            // The registration edge (INV-BLKDEV-2) carries the contract.
            let ret = async_fn(disk, sector, segs, completion);
            let elapsed_ns = crate::drivers::hotpath::timer::read_time().saturating_sub(t0) * 100
                / crate::config::TIMER_CLOCK_FREQ_HZ;
            note_io(major, minor, false, total, elapsed_ns);
            if ret < 0 {
                Err(ret)
            } else {
                Ok(())
            }
        } else {
            Err(-6) // ENXIO — no SG support; caller submits per segment
        }
    }
}

/// Submit a fused async scatter-gather WRITE — the merged form of
/// [`blkdev_write_async`]. Every segment is a DMA SOURCE under the same
/// stability contract (valid and unmodified until the completion fires).
///
/// Returns Ok(()) if submitted, Err on failure (caller falls back to
/// per-segment submissions).
pub fn blkdev_write_async_sg(
    disk: &'static GenDisk,
    sector: u64,
    segs: &[DmaSeg],
    completion: &crate::fs::io_completion::IoCompletion,
) -> Result<(), i32> {
    {
        if let Some(async_fn) = disk.async_write_sg_fn {
            let major = disk.major;
            let minor = disk.first_minor;
            let total: u64 = segs.iter().map(|s| (s.len / 512) as u64).sum();
            let t0 = crate::drivers::hotpath::timer::read_time();
            if crate::trace::EVENT_BLOCK_RQ_ISSUE.armed() {
                // SAFETY: current task's comm is read for the record only.
                let comm = crate::sched::hotpath::current().map(|cur| (*cur).comm());
                if let Some(comm) = comm {
                    crate::trace::trace_block_rq_issue(
                        (major << 20) | minor,
                        sector,
                        total as u32,
                        (total * 512) as u32,
                        0,
                        "W",
                        comm,
                    );
                }
            }
            // The registration edge (INV-BLKDEV-2) carries the contract.
            let ret = async_fn(disk, sector, segs, completion);
            let elapsed_ns = crate::drivers::hotpath::timer::read_time().saturating_sub(t0) * 100
                / crate::config::TIMER_CLOCK_FREQ_HZ;
            note_io(major, minor, true, total, elapsed_ns);
            if ret < 0 {
                Err(ret)
            } else {
                Ok(())
            }
        } else {
            Err(-6) // ENXIO — no SG support; caller submits per segment
        }
    }
}

pub fn register_disk(disk: Box<GenDisk>) -> Result<(), &'static str> {
    // /proc/diskstats lists every registered device.
    register_device(disk.major, disk.first_minor, disk.name);
    BLOCK_MANAGER.register_disk(disk)
}

pub fn get_disk(major: u32) -> Option<&'static GenDisk> {
    BLOCK_MANAGER.get_disk(major)
}

pub fn submit_request(disk: &GenDisk, req: &mut Request) -> i32 {
    // /proc/diskstats: count the request and the time it took. The
    // request carries the transfer size, so the sector count is exact.
    let major = disk.major;
    let minor = disk.first_minor;
    let is_write = matches!(req.cmd_type, ReqCmd::Write);
    let sectors = (req.buffer.len() / 512) as u64;
    let t0 = crate::drivers::hotpath::timer::read_time();
    note_start(major, minor);
    // block:block_rq_issue / block_rq_complete — the synchronous
    // request path: the registered request_fn completes the transfer
    // before it returns, so issue is recorded at entry and complete
    // right after the device status is known. Both record calls carry
    // their armed() check inside; the comm copy is marshalled only
    // behind the issue event's own check.
    if crate::trace::EVENT_BLOCK_RQ_ISSUE.armed() {
        // SAFETY: current task's comm is read for the record only.
        let comm = crate::sched::hotpath::current().map(|cur| cur.comm());
        if let Some(comm) = comm {
            crate::trace::trace_block_rq_issue(
                (major << 20) | minor,
                req.sector,
                sectors as u32,
                req.buffer.len() as u32,
                0,
                req_rwbs(&req.cmd_type),
                comm,
            );
        }
    }
    let ret = BLOCK_MANAGER.submit_request(disk, req);
    if crate::trace::EVENT_BLOCK_RQ_COMPLETE.armed() {
        // The device status (R8-M2: submit_request returns 0 whenever a
        // request_fn is registered — the real completion status is the
        // request's error word).
        let error = req.error.load(core::sync::atomic::Ordering::Acquire);
        crate::trace::trace_block_rq_complete(
            (major << 20) | minor,
            req.sector,
            sectors as u32,
            error,
            0,
            req_rwbs(&req.cmd_type),
        );
    }
    note_end(major, minor);
    let elapsed_ns = crate::drivers::hotpath::timer::read_time().saturating_sub(t0) * 100
        / crate::config::TIMER_CLOCK_FREQ_HZ;
    note_io(major, minor, is_write, sectors, elapsed_ns);
    ret
}

/// The rwbs column of the block events: the transfer direction only
/// (the reference's sync/meta bits have no counterpart here — Rux
/// merges no requests and passes no write hints).
fn req_rwbs(cmd: &ReqCmd) -> &'static str {
    match cmd {
        ReqCmd::Read => "R",
        ReqCmd::Write => "W",
        ReqCmd::Flush => "F",
    }
}

pub fn blkdev_read(disk: &'static GenDisk, sector: u64, buf: &mut [u8]) -> Result<usize, i32> {
    let mut req = Request {
        cmd_type: ReqCmd::Read,
        sector,
        buffer: vec![0u8; buf.len()],
        device: disk,
        error: core::sync::atomic::AtomicI32::new(0),
    };

    let ret = BLOCK_MANAGER.submit_request(disk, &mut req);
    if ret < 0 {
        return Err(ret);
    }
    // R8-M2: device-side errors reach the caller (was swallowed).
    let dev_err = req.error.load(core::sync::atomic::Ordering::Acquire);
    if dev_err != 0 {
        return Err(dev_err);
    }

    // Copy data
    buf.copy_from_slice(&req.buffer);
    Ok(buf.len())
}

pub fn blkdev_write(disk: &'static GenDisk, sector: u64, buf: &[u8]) -> Result<usize, i32> {
    let mut req = Request {
        cmd_type: ReqCmd::Write,
        sector,
        buffer: buf.to_vec(),
        device: disk,
        error: core::sync::atomic::AtomicI32::new(0),
    };

    let ret = BLOCK_MANAGER.submit_request(disk, &mut req);
    if ret < 0 {
        return Err(ret);
    }
    // R9-4: device-side errors reach the caller (read path had this
    // since R8-M2a; without it a failed write was marked clean by
    // BufferHead::sync and swap-out silently discarded page data).
    let dev_err = req.error.load(core::sync::atomic::Ordering::Acquire);
    if dev_err != 0 {
        return Err(dev_err);
    }

    Ok(buf.len())
}

// ════════════════ blkdev::iostat ══════════════════════
// (was blkdev/iostat.rs)
// Block device I/O statistics — the counters behind Linux's
// `/proc/diskstats` (Linux keeps them in the block layer,
// `block/genhd.c`; so does Rux).
//
// Linux's /proc/diskstats prints one line per device:
//
// ```text
//    7       0 loop0 14 0 34 2 0 0 0 0 0 1 2 0 0 0 0 0 0
// ```
//
// The columns after the device name are: reads completed, reads merged,
// sectors read, milliseconds spent reading, writes completed, writes
// merged, sectors written, milliseconds spent writing, I/Os in progress,
// milliseconds spent doing I/O, weighted milliseconds spent doing I/O,
// discards completed, discards merged, sectors discarded, milliseconds
// spent discarding, flush requests and milliseconds spent flushing.
//
// Rux has no request merging, no discards and no separate flush path,
// so those columns report zero; the read/write/sector/time columns are
// real.
static STATS: Spinlock<Vec<Entry>> = Spinlock::new(Vec::new());

/// Register a device so it shows up in /proc/diskstats.
pub fn register_device(major: u32, minor: u32, name: &str) {
    let mut stats = STATS.lock();
    if stats.iter().any(|e| e.major == major && e.minor == minor) {
        return;
    }
    stats.push(Entry {
        major,
        minor,
        name: name.to_string(),
        reads: AtomicU64::new(0),
        sectors_read: AtomicU64::new(0),
        ms_reading: AtomicU64::new(0),
        writes: AtomicU64::new(0),
        sectors_written: AtomicU64::new(0),
        ms_writing: AtomicU64::new(0),
        io_in_progress: AtomicU64::new(0),
        ms_doing_io: AtomicU64::new(0),
    });
}

/// Count one completed I/O.
///
/// `sectors` is the transfer size in 512-byte sectors and `elapsed_ns`
/// the time it took.
pub fn note_io(major: u32, minor: u32, is_write: bool, sectors: u64, elapsed_ns: u64) {
    let stats = STATS.lock();
    let Some(entry) = stats
        .iter()
        .find(|e| e.major == major && e.minor == minor)
    else {
        return;
    };
    let ms = elapsed_ns / 1_000_000;
    if is_write {
        entry.writes.fetch_add(1, Ordering::Relaxed);
        entry.sectors_written.fetch_add(sectors, Ordering::Relaxed);
        entry.ms_writing.fetch_add(ms, Ordering::Relaxed);
    } else {
        entry.reads.fetch_add(1, Ordering::Relaxed);
        entry.sectors_read.fetch_add(sectors, Ordering::Relaxed);
        entry.ms_reading.fetch_add(ms, Ordering::Relaxed);
    }
    entry.ms_doing_io.fetch_add(ms, Ordering::Relaxed);
}

/// One I/O in flight (Linux's "I/Os in progress" column).
pub fn note_start(major: u32, minor: u32) {
    let stats = STATS.lock();
    if let Some(entry) = stats.iter().find(|e| e.major == major && e.minor == minor) {
        entry.io_in_progress.fetch_add(1, Ordering::Relaxed);
    }
}

/// The I/O finished.
pub fn note_end(major: u32, minor: u32) {
    let stats = STATS.lock();
    if let Some(entry) = stats.iter().find(|e| e.major == major && e.minor == minor) {
        entry.io_in_progress.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A snapshot of every device's counters.
pub fn snapshot() -> Vec<DiskStat> {
    let stats = STATS.lock();
    stats
        .iter()
        .map(|e| DiskStat {
            major: e.major,
            minor: e.minor,
            name: e.name.clone(),
            reads: e.reads.load(Ordering::Relaxed),
            sectors_read: e.sectors_read.load(Ordering::Relaxed),
            ms_reading: e.ms_reading.load(Ordering::Relaxed),
            writes: e.writes.load(Ordering::Relaxed),
            sectors_written: e.sectors_written.load(Ordering::Relaxed),
            ms_writing: e.ms_writing.load(Ordering::Relaxed),
            io_in_progress: e.io_in_progress.load(Ordering::Relaxed),
            ms_doing_io: e.ms_doing_io.load(Ordering::Relaxed),
        })
        .collect()
}

// ════════════════ pci ═════════════════════════════════
// (was pci/mod.rs)
// PCI configuration space access layer
//
// Implements PCI configuration space access and device enumeration
/// PCI configuration space register offsets
pub mod offset {
    pub const VENDOR_ID: u8 = 0x00;
    pub const DEVICE_ID: u8 = 0x02;
    pub const COMMAND: u8 = 0x04;
    pub const STATUS: u8 = 0x06;
    pub const REVISION: u8 = 0x08;
    pub const PROG_IF: u8 = 0x09;
    pub const SUBCLASS: u8 = 0x0A;
    pub const CLASS: u8 = 0x0B;
    pub const CACHE_LINE_SIZE: u8 = 0x0C;
    pub const LATENCY_TIMER: u8 = 0x0D;
    pub const HEADER_TYPE: u8 = 0x0E;
    pub const BIST: u8 = 0x0F;
    pub const BAR0: u8 = 0x10;
    pub const BAR1: u8 = 0x14;
    pub const BAR2: u8 = 0x18;
    pub const BAR3: u8 = 0x1C;
    pub const BAR4: u8 = 0x20;
    pub const BAR5: u8 = 0x24;
    pub const CARDBUS_CIS_PTR: u8 = 0x28;
    pub const SUBSYSTEM_VENDOR_ID: u8 = 0x2C;
    pub const SUBSYSTEM_ID: u8 = 0x2E;
    pub const EXP_ROM_BASE_ADDR: u8 = 0x30;
    pub const CAPABILITIES_PTR: u8 = 0x34;
    pub const RESERVED0: u8 = 0x35;
    pub const RESERVED1: u8 = 0x38;
    pub const INT_LINE: u8 = 0x3C;
    pub const INT_PIN: u8 = 0x3D;
    pub const MIN_GNT: u8 = 0x3E;
    pub const MAX_LAT: u8 = 0x3F;
}

/// PCI command register bits
pub mod command {
    pub const IO_SPACE: u16 = 0x0001;
    pub const MEMORY_SPACE: u16 = 0x0002;
    pub const BUS_MASTER: u16 = 0x0004;
    pub const SPECIAL_CYCLES: u16 = 0x0008;
    pub const MEM_WR_INV: u16 = 0x0010;
    pub const VGA_PALETTE: u16 = 0x0020;
    pub const PARITY_ERR_RESP: u16 = 0x0040;
    pub const SERR_ENABLE: u16 = 0x0080;
    pub const FAST_BACK_TO_BACK: u16 = 0x0100;
    pub const INT_DISABLE: u16 = 0x0400;
}

/// PCI status register bits
pub mod pci_status {
    pub const CAPABILITIES_LIST: u16 = 0x0010;
    pub const IRQ_STATUS: u16 = 0x0008;
    pub const CAPABILITIES_LIST_66MHZ: u16 = 0x0200;
    pub const FAST_BACK_TO_BACK: u16 = 0x0080;
    pub const DEVSEL_TIMING: u16 = 0x0600;
}


impl PCIBAR {
    pub const fn empty() -> Self {
        Self {
            base_addr: 0,
            size: 0,
            bar_type: BARType::None,
            prefetchable: false,
            is_64bit: false,
        }
    }

    /// Check if BAR is valid (non-empty)
    pub fn is_valid(&self) -> bool {
        self.bar_type != BARType::None && self.size > 0
    }
}


/// Translate an ECAM window address to the VA the kernel dereferences.
/// riscv64: the identity ECAM window (0x30000000+) is cloned into every
/// user root where a MAP_FIXED user mapping may replace it — all config
/// access goes through the kernel-half MMIO alias instead. x86_64: the
/// firmware's ECAM window already lives in the kernel half (kept verbatim).
#[cfg(feature = "riscv64")]
pub fn ecam_va(ecam_addr: u64) -> u64 {
    crate::arch::mm::memory_layout::mmio_alias(ecam_addr)
}
#[cfg(feature = "x86_64")]
pub fn ecam_va(ecam_addr: u64) -> u64 {
    ecam_addr
}
/// aarch64: same alias discipline as riscv64 (the low ECAM window must
/// never be dereferenced from a context whose root a user MAP_FIXED can
/// replace; the permanent kernel-half alias carries all config access).
#[cfg(feature = "aarch64")]
pub fn ecam_va(ecam_addr: u64) -> u64 {
    crate::arch::mm::memory_layout::mmio_alias(ecam_addr)
}

impl PCIConfig {
    /// Create new PCI configuration space access
    pub const fn new(base_addr: u64) -> Self {
        Self { base_addr }
    }

    /// Create a config accessor for an ECAM identity address (translates
    /// through ecam_va on riscv64; see ecam_va for why).
    pub fn new_at_ecam(ecam_addr: u64) -> Self {
        Self { base_addr: ecam_va(ecam_addr) }
    }

    /// The device's configuration-space window (INV-MMIO-1: the ECAM /
    /// port window this accessor was built from maps the function's
    /// 4 KiB config page; offsets below are u8-in-range).
    fn cfg(&self) -> crate::safety::mmio::MmioRegion {
        crate::safety::mmio::MmioRegion::new(self.base_addr as usize, 0x1000)
    }

    /// Read 32-bit configuration space register
    pub fn read_config_dword(&self, offset: u8) -> u32 {
        self.cfg().read_u32(offset as usize)
    }

    /// Write 32-bit configuration space register
    pub fn write_config_dword(&self, offset: u8, value: u32) {
        self.cfg().write_u32(offset as usize, value)
    }

    /// Read 16-bit configuration space register
    pub fn read_config_word(&self, offset: u8) -> u16 {
        self.cfg().read_u16(offset as usize)
    }

    /// Write 16-bit configuration space register
    pub fn write_config_word(&self, offset: u8, value: u16) {
        self.cfg().write_u16(offset as usize, value)
    }

    /// Read 8-bit configuration space register
    pub fn read_config_byte(&self, offset: u8) -> u8 {
        self.cfg().read_u8(offset as usize)
    }

    /// Read BAR
    pub fn read_bar(&self, bar_index: u8) -> PCIBAR {
        if bar_index > 5 {
            return PCIBAR::empty();
        }

        let bar_offset = offset::BAR0 + (bar_index * 4);
        let bar_low = self.read_config_dword(bar_offset);

        // Determine BAR type
        if bar_low & 0x00000001 == 0x00000001 {
            // I/O mapped BAR
            PCIBAR {
                base_addr: (bar_low & 0xFFFFFFFC) as u64,
                size: 0,
                bar_type: BARType::IOMapped,
                prefetchable: false,
                is_64bit: false,
            }
        } else {
            // Memory mapped BAR
            // Check if 64-bit BAR (bit 2-1 = 10)
            let is_64bit = (bar_low & 0x00000006) == 0x00000004;

            let base_addr = if is_64bit {
                // 64-bit BAR: read next BAR register as high 32 bits
                if bar_index + 1 <= 5 {
                    let bar_high = self.read_config_dword(bar_offset + 4);
                    ((bar_high as u64) << 32) | ((bar_low & 0xFFFFFFF0) as u64)
                } else {
                    // Should not happen: 64-bit BAR must have paired high 32-bit register
                    (bar_low & 0xFFFFFFF0) as u64
                }
            } else {
                // 32-bit BAR
                (bar_low & 0xFFFFFFF0) as u64
            };

            PCIBAR {
                base_addr,
                size: 0,
                bar_type: BARType::MemoryMapped,
                prefetchable: (bar_low & 0x00000008) != 0,
                is_64bit,
            }
        }
    }

    /// Probe BAR size
    ///
    /// Determine BAR size by writing all 1s then reading back
    pub fn probe_bar_size(&self, bar_index: u8) -> u64 {
        if bar_index > 5 {
            return 0;
        }

        let bar_offset = offset::BAR0 + (bar_index * 4);
        let original_low = self.read_config_dword(bar_offset);

        // Determine BAR type
        let is_io = (original_low & 0x01) != 0;
        let is_64bit = !is_io && ((original_low & 0x06) == 0x04);

        // Save original value
        let original_high = if is_64bit && bar_index + 1 <= 5 {
            self.read_config_dword(bar_offset + 4)
        } else {
            0
        };

        // Write all 1s to probe size
        self.write_config_dword(bar_offset, 0xFFFFFFFF);

        let size_low = self.read_config_dword(bar_offset);

        // Restore original value
        self.write_config_dword(bar_offset, original_low);

        let size = if is_io {
            // I/O BAR: size is inverted & ~0x03
            ((!(size_low & 0xFFFFFFFC)) + 1) as u64
        } else if is_64bit {
            // 64-bit Memory BAR
            // Write all 1s to high bits
            if bar_index + 1 <= 5 {
                self.write_config_dword(bar_offset + 4, 0xFFFFFFFF);
                let size_high = self.read_config_dword(bar_offset + 4);
                self.write_config_dword(bar_offset + 4, original_high);

                let low = (!(size_low & 0xFFFFFFF0)) as u64;
                let high = (!size_high) as u64;
                ((high << 32) | low) + 1
            } else {
                0
            }
        } else {
            // 32-bit Memory BAR
            ((!(size_low & 0xFFFFFFF0)) + 1) as u64
        };

        // If size is 0 or result overflowed, return 0
        if size == 0 || size > 0x8000000000 {
            0
        } else {
            size
        }
    }

    /// Allocate and set BAR address
    ///
    /// # Parameters
    /// - `bar_index`: BAR index (0-5)
    /// - `base_addr`: Base address to set
    ///
    /// # Returns
    /// Successfully set BAR info on success, error on failure
    pub fn assign_bar(&self, bar_index: u8, base_addr: u64) -> Result<PCIBAR, &'static str> {
        if bar_index > 5 {
            return Err("Invalid BAR index");
        }

        let bar_offset = offset::BAR0 + (bar_index * 4);

        // Read original value to determine BAR type
        let original_low = self.read_config_dword(bar_offset);
        let is_io = (original_low & 0x01) != 0;
        let is_64bit = !is_io && ((original_low & 0x06) == 0x04);

        // Alignment check: address must satisfy BAR alignment requirement
        let size = self.probe_bar_size(bar_index);
        if size == 0 {
            return Err("Invalid BAR size");
        }

        if base_addr % size != 0 {
            return Err("BAR address not properly aligned");
        }

        if is_64bit {
            // 64-bit BAR: write low 32 bits and high 32 bits
            let low = (base_addr & 0xFFFFFFFF) as u32;
            let high = (base_addr >> 32) as u32;

            // Preserve type bits in low bits
            let low_with_flags = low | (original_low & 0x0F);

            self.write_config_dword(bar_offset, low_with_flags);
            if bar_index + 1 <= 5 {
                self.write_config_dword(bar_offset + 4, high);
            }

            Ok(PCIBAR {
                base_addr,
                size,
                bar_type: BARType::MemoryMapped,
                prefetchable: (original_low & 0x08) != 0,
                is_64bit: true,
            })
        } else if is_io {
            // I/O BAR
            let addr = (base_addr & 0xFFFFFFFC) as u32;
            self.write_config_dword(bar_offset, addr | 0x01);

            Ok(PCIBAR {
                base_addr: addr as u64,
                size,
                bar_type: BARType::IOMapped,
                prefetchable: false,
                is_64bit: false,
            })
        } else {
            // 32-bit Memory BAR
            let addr = (base_addr & 0xFFFFFFF0) as u32;
            let prefetchable = (original_low & 0x08) != 0;
            self.write_config_dword(bar_offset, addr | (if prefetchable { 0x08 } else { 0x00 }) | 0x00);

            Ok(PCIBAR {
                base_addr: addr as u64,
                size,
                bar_type: BARType::MemoryMapped,
                prefetchable,
                is_64bit: false,
            })
        }
    }

    /// Get vendor ID
    pub fn vendor_id(&self) -> u16 {
        self.read_config_word(offset::VENDOR_ID)
    }

    /// Get device ID
    pub fn device_id(&self) -> u16 {
        self.read_config_word(offset::DEVICE_ID)
    }

    /// Get class code
    pub fn class_code(&self) -> u8 {
        self.read_config_byte(offset::CLASS)
    }

    /// Get subclass code
    pub fn subclass(&self) -> u8 {
        self.read_config_byte(offset::SUBCLASS)
    }

    /// Get programming interface
    pub fn prog_if(&self) -> u8 {
        self.read_config_byte(offset::PROG_IF)
    }

    /// Get revision ID
    pub fn revision_id(&self) -> u8 {
        self.read_config_byte(offset::REVISION)
    }

    /// Get interrupt pin
    pub fn interrupt_pin(&self) -> u8 {
        self.read_config_byte(offset::INT_PIN)
    }

    /// Get interrupt line
    pub fn interrupt_line(&self) -> u8 {
        self.read_config_byte(offset::INT_LINE)
    }

    /// Set command register (16-bit write to avoid overwriting STATUS)
    pub fn set_command(&self, cmd: u16) {
        self.write_config_word(offset::COMMAND, cmd);
    }

    /// Get command register
    pub fn command(&self) -> u16 {
        self.read_config_word(offset::COMMAND)
    }

    /// Enable bus mastering
    pub fn enable_bus_master(&self) {
        let current = self.command();
        self.set_command(current | command::BUS_MASTER | command::MEMORY_SPACE);
    }
}

/// Known vendor IDs
pub mod vendor {
    pub const RED_HAT: u16 = 0x1AF4;
}

/// VirtIO device IDs (PCI)
///
/// Legacy/transitional devices answer with the 0x1000-series IDs.
/// Modern (non-transitional, virtio 1.0) devices presented by QEMU
/// answer `0x1040 + virtio device type`: net=1 -> 0x1041, blk=2 ->
/// 0x1042, console=3, balloon=5, gpu=16 -> 0x1050, input=18 -> 0x1052.
/// This is the mapping the reference kernel's virtio_pci driver uses
/// (device_id = pci DID - 0x1040 for DID >= 0x1040), and it is what
/// real device models expose: `virtio-blk-pci,disable-legacy=on` on
/// the virt machine answers PCI 1af4:1042 (verified via the QEMU
/// monitor `info pci`). The virtio spec appendix lists different IDs
/// (0x1040 net / 0x1041 blk), but no real device presents those —
/// setting the constants to the spec table makes every
/// disable-legacy=on boot lose its disk (observed on main@82059759:
/// the PCI blk probe found zero devices, root fell back, no init
/// loadable). GPEX (aarch64) devices are transitional (0x1000/0x1001)
/// and are unaffected by the modern entries.
pub mod virtio_device {
    pub const VIRTIO_NET: u16 = 0x1000;
    pub const VIRTIO_NET_MODERN: u16 = 0x1041;
    pub const VIRTIO_BLK: u16 = 0x1001;
    pub const VIRTIO_BLK_MODERN: u16 = 0x1042;
    pub const VIRTIO_GPU: u16 = 0x1050;
    pub const VIRTIO_INPUT: u16 = 0x1052;
}

/// RISC-V PCIe ECAM base address
#[cfg(feature = "riscv64")]
pub const RISCV_PCIE_ECAM_BASE: u64 = 0x30000000;

/// PCIe ECAM configuration space size
pub const PCIE_ECAM_SIZE: u64 = 0x1000;

/// Scan the PCIe ECAM space and return the ECAM address of every function
/// whose vendor matches `vendor` and whose device id is in `device_ids`.
///
/// ECAM layout: addr = ECAM | bus<<20 | dev<<15 | fn<<12, so the per-slot
/// stride is 0x8000 and each slot carries 8 functions. The driver-local
/// scans previously walked the space with a bare 0x1000 stride — they read
/// the FUNCTION bits as device numbers, hiding every slot >= 4 (review
/// BUG "PCI 探测步长 0x1000 错"). All probe paths now share this helper.
///
/// Buses 0..8 are covered (bounded by the kernel's 8MB ECAM mapping):
/// boot devices live on bus 0, while devices hot-added behind a
/// pcie-root-port (U4 rescan) land on bus 1+. Absent functions read
/// vendor 0xFFFF and are skipped.

/// The arch ECAM base the walker uses (riscv64 virt: 0x30000000; x86_64
/// q35 MMCONFIG: 0xb0000000). Exposed for BDF derivations from a
/// function's config-space address (see drm::init_drm).
pub fn ecam_base() -> u64 {
    crate::arch::mm::memory_layout::PCIE_ECAM_BASE
}

pub fn find_ecam_devices(vendor: u16, device_ids: &[u16]) -> alloc::vec::Vec<u64> {
    {
        // Arch ECAM base (riscv64 virt: 0x30000000; x86_64 q35 MMCONFIG:
        // 0xb0000000). Both map config space at VA == ECAM phys base, so the
        // walker is identical after the constant swap.
        const ECAM_BASE: u64 = crate::arch::mm::memory_layout::PCIE_ECAM_BASE;
        const MAX_BUSES: u64 = 8;
        const MAX_SLOTS: u64 = 32;
        const FUNCTIONS_PER_SLOT: u64 = 8;

        let mut found = alloc::vec::Vec::new();
        for bus in 0..MAX_BUSES {
            for slot in 0..MAX_SLOTS {
                for func in 0..FUNCTIONS_PER_SLOT {
                    let ecam_addr = ECAM_BASE
                        + (bus << 20)
                        + (slot * 0x8000)
                        + (func * PCIE_ECAM_SIZE);
                    let config = PCIConfig::new_at_ecam(ecam_addr);

                    let vid = config.vendor_id();
                    if vid != vendor {
                        continue;
                    }
                    let did = config.device_id();
                    if device_ids.contains(&did) {
                        found.push(ecam_addr);
                    }
                }
            }
        }
        found
    }
}

/// Enumerate VirtIO devices on PCI bus
///
/// # Returns
/// Number of VirtIO devices found
pub fn enumerate_virtio_devices() -> usize {
    #[cfg(feature = "riscv64")]
    {
        let mut device_count = 0;

        // RISC-V: Scan PCIe ECAM space. ECAM layout is
        //   addr = ECAM | bus << 20 | dev << 15 | fn << 12
        // so the stride must be 0x8000 (per device) and all 8 functions
        // of each slot must be probed — the old 0x1000 stride scanned the
        // FUNCTION bits as device numbers, hiding every device at slot >= 4
        // and reporting slots 8x too large (review DRIV NEW-1).
        const MAX_SLOTS: u8 = 32;
        const FUNCTIONS_PER_SLOT: u64 = 8;

        for slot in 0..MAX_SLOTS {
            for func in 0..FUNCTIONS_PER_SLOT {
                let ecam_addr = RISCV_PCIE_ECAM_BASE
                    + (slot as u64 * 0x8000)
                    + (func * PCIE_ECAM_SIZE);
                let config = PCIConfig::new_at_ecam(ecam_addr);

                let vendor_id = config.vendor_id();
                let device_id = config.device_id();

                // Check if device exists
                if vendor_id == 0xFFFF {
                    continue;
                }

                // Check if VirtIO device (Red Hat)
                if vendor_id == vendor::RED_HAT {
                    // Identify VirtIO device type
                    match device_id {
                        virtio_device::VIRTIO_BLK
                        | virtio_device::VIRTIO_BLK_MODERN
                        | virtio_device::VIRTIO_NET
                        | virtio_device::VIRTIO_NET_MODERN
                        | virtio_device::VIRTIO_GPU
                        | virtio_device::VIRTIO_INPUT => {
                            device_count += 1;
                        }
                        _ => {}
                    }
                }
            }
        }

        device_count
    }

    #[cfg(not(feature = "riscv64"))]
    {
        0
    }
}

// ════════════════ virtio ══════════════════════════════
// (was virtio/mod.rs)
// VirtIO block device driver
// SAFETY: VirtIOBlkDevice is only accessed from kernel context; internal Spinlocks
// serialize all mutable access to shared fields.
unsafe impl Send for VirtIOBlkDevice {}
// SAFETY: All shared state is protected by Spinlocks (irqsafe where needed),
// ensuring no data races across threads/CPUs.
unsafe impl Sync for VirtIOBlkDevice {}

impl VirtIOBlkDevice {
    /// Create new VirtIO block device
    pub fn new(base_addr: u64) -> Self {
        Self {
            base_addr,
            disk: GenDisk::new("virtblk", 0, 1, 512, None as Option<&BlockDeviceOps>),
            capacity: 0,
            block_size: 512,
            initialized: Spinlock::new(false),
            virtqueue: Spinlock::new(None),
            queue_size: 0,
        }
    }

    /// Initialize device
    pub fn init(&mut self) -> Result<(), &'static str> {
        // VirtIO MMIO register offsets
        const MAGIC_VALUE_OFFSET: u64 = 0x000;
        const VERSION_OFFSET: u64 = 0x004;
        const DEVICE_ID_OFFSET: u64 = 0x008;
        const STATUS_OFFSET: u64 = 0x070;
        const GUEST_PAGE_SIZE_OFFSET: u64 = 0x028;
        const DEVICE_FEATURES_OFFSET: u64 = 0x010;
        const DRIVER_FEATURES_OFFSET: u64 = 0x020;
        const QUEUE_SEL_OFFSET: u64 = 0x030;
        const QUEUE_NUM_MAX_OFFSET: u64 = 0x034;
        const QUEUE_NUM_OFFSET: u64 = 0x038;

        // INV-MMIO-1..3 (unsafe-slim round 3): the device's register
        // window as a bounds-checked MmioRegion — every access below
        // is a safe volatile op (offsets are protocol constants; the
        // 0x200 window covers the config area the capacity read uses).
        let mmio = crate::safety::mmio::MmioRegion::new(self.base_addr as usize, 0x200);
        let read_reg = |off: u64| mmio.read_u32(off as usize);
        let write_reg = |off: u64, val: u32| mmio.write_u32(off as usize, val);
        {
            let _ = &read_reg; let _ = &write_reg;
            // 1. Verify magic number
            let magic = read_reg(MAGIC_VALUE_OFFSET);
            if magic != 0x74726976 {
                return Err("Invalid VirtIO magic value");
            }

            // 2. Verify version (only support Modern VirtIO 1.0+)
            let version = read_reg(VERSION_OFFSET);
            if version != 2 {
                return Err("Unsupported VirtIO version: only Modern VirtIO 1.0+ (version 2) is supported, Legacy VirtIO is not supported");
            }

            // 3. Verify device ID
            let device_id = read_reg(DEVICE_ID_OFFSET);
            if device_id != 2 {
                return Err("Not a VirtIO block device");
            }

            // (Register discipline: see the MmioRegion binding above —
            // offsets follow the VirtIO MMIO device spec.)
            // 4. State machine: Reset device
            write_reg(STATUS_OFFSET, 0x00);

            // 5. State machine: ACKNOWLEDGE (0x01)
            write_reg(STATUS_OFFSET, 0x01);
            let status = read_reg(STATUS_OFFSET);
            if status & 0x01 == 0 {
                return Err("Device rejected ACKNOWLEDGE status");
            }

            // 6. State machine: DRIVER (0x02)
            write_reg(STATUS_OFFSET, 0x01 | 0x02);
            let status = read_reg(STATUS_OFFSET);
            if status & 0x02 == 0 {
                return Err("Device rejected DRIVER status");
            }

            // Check if device needs reset (NEEDS_RESET bit)
            if status & 0x40 != 0 {
                write_reg(STATUS_OFFSET, 0x00);
                write_reg(STATUS_OFFSET, 0x01 | 0x02);
            }

            // 7. Feature negotiation (R35, modeled on the R34 virtio_net fix):
            // a modern (v2) MMIO device MUST have VIRTIO_F_VERSION_1 accepted
            // by the driver. The old code wrote DRIVER_FEATURES=0 through the
            // reset-default selector (word 0) and never touched
            // DRIVER_FEATURES_SEL — word 1 stayed 0, so VERSION_1 was never
            // acked and QEMU marked the device FAILED once FEATURES_OK got
            // set. Nothing beyond VERSION_1 is implemented (RO/FLUSH/
            // BLK_SIZE/... all deliberately not accepted).
            const DEVICE_FEATURES_SEL_OFFSET: u64 = 0x014;
            const DRIVER_FEATURES_SEL_OFFSET: u64 = 0x024;

            // VIRTIO_F_VERSION_1 = bit 32 = word 1, bit 0
            const F_VERSION_1: u32 = 1 << 0;

            // Read word 1 and require VERSION_1
            write_reg(DEVICE_FEATURES_SEL_OFFSET, 1);
            let feats_hi = read_reg(DEVICE_FEATURES_OFFSET);
            if feats_hi & F_VERSION_1 == 0 {
                return Err("Device does not offer VIRTIO_F_VERSION_1");
            }
            write_reg(DEVICE_FEATURES_SEL_OFFSET, 0);
            let _feats_lo = read_reg(DEVICE_FEATURES_OFFSET);

            // Write back: word 0 = 0 (nothing implemented), word 1 = VERSION_1
            write_reg(DRIVER_FEATURES_SEL_OFFSET, 0);
            write_reg(DRIVER_FEATURES_OFFSET, 0);
            write_reg(DRIVER_FEATURES_SEL_OFFSET, 1);
            write_reg(DRIVER_FEATURES_OFFSET, F_VERSION_1);

            // 9.5. Set FEATURES_OK bit and verify the device accepted the
            // negotiated set (a modern device clears the bit on rejection).
            write_reg(STATUS_OFFSET, 0x01 | 0x02 | 0x08);
            let status = read_reg(STATUS_OFFSET);
            if status & 0x08 == 0 {
                return Err("Device rejected negotiated features (features_ok cleared)");
            }

            // ========== VirtQueue setup ==========

            // 10. Select queue 0
            write_reg(QUEUE_SEL_OFFSET, 0);

            // 11. Read max queue size
            let max_queue_size = read_reg(QUEUE_NUM_MAX_OFFSET);

            if max_queue_size == 0 {
                return Err("VirtIO device has zero queue size");
            }

            self.queue_size = if max_queue_size < 8 { 4 } else { 8 };

            // 12. Set queue count
            write_reg(QUEUE_NUM_OFFSET, self.queue_size as u32);

            // 13. Create VirtQueue (allocate vring memory)
            // R35 (W32): virtio-mmio drops any register access whose size
            // != 4 ("wrong size access" in QEMU) — VirtQueue::new's W16
            // default made every MMIO blk notify a silent no-op, so I/O
            // never started. Use with_notify_width like the R34 net driver.
            let virtqueue = match VirtQueue::with_notify_width(
                self.queue_size,
                0,  // queue_index: block device only uses queue 0
                self.base_addr + 0x50,  // queue_notify
                self.base_addr + 0x60,  // interrupt_status
                self.base_addr + 0x64,  // interrupt_ack
                NotifyWidth::W32,
            ) {
                Some(vq) => vq,
                None => return Err("Failed to allocate VirtQueue"),
            };

            let desc_addr = virtqueue.get_desc_addr();
            let avail_addr = virtqueue.get_avail_addr();
            let used_addr = virtqueue.get_used_addr();
            // 14. Modern virtio-mmio (v2) split queue-address registers (the
            // same layout QEMU virt implements): QueueDesc 0x80/0x84,
            // QueueAvail 0x90/0x94, QueueUsed 0xa0/0xa4, QueueReady 0x44.
            // The previous code reused the PCI common-cfg offsets
            // (0x20-0x34, enable 0x1c): the address writes landed on
            // GuestFeaturesSel/GuestFeatures/QueueSel and QueueReady was
            // never actually set — the device never used the rings this
            // driver submits on (queue setup silently no-op'd).
            const QUEUE_DESC_LO_OFFSET: u64 = 0x80;
            const QUEUE_DESC_HI_OFFSET: u64 = 0x84;
            const QUEUE_DRIVER_LO_OFFSET: u64 = 0x90;
            const QUEUE_DRIVER_HI_OFFSET: u64 = 0x94;
            const QUEUE_DEVICE_LO_OFFSET: u64 = 0xA0;
            const QUEUE_DEVICE_HI_OFFSET: u64 = 0xA4;
            const QUEUE_READY_OFFSET: u64 = 0x44;

            // Convert virtual addresses to physical addresses
            let desc_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(desc_addr)
            ).0;
            let avail_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(avail_addr)
            ).0;
            let used_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(used_addr)
            ).0;

            // Write descriptor table address (low 32 bits)
            write_reg(QUEUE_DESC_LO_OFFSET, (desc_phys_addr & 0xFFFFFFFF) as u32);
            // Write descriptor table address (high 32 bits)
            write_reg(QUEUE_DESC_HI_OFFSET, (desc_phys_addr >> 32) as u32);

            // Write available ring address (low 32 bits)
            write_reg(QUEUE_DRIVER_LO_OFFSET, (avail_phys_addr & 0xFFFFFFFF) as u32);
            // Write available ring address (high 32 bits)
            write_reg(QUEUE_DRIVER_HI_OFFSET, (avail_phys_addr >> 32) as u32);

            // Write used ring address (low 32 bits)
            write_reg(QUEUE_DEVICE_LO_OFFSET, (used_phys_addr & 0xFFFFFFFF) as u32);
            // Write used ring address (high 32 bits)
            write_reg(QUEUE_DEVICE_HI_OFFSET, (used_phys_addr >> 32) as u32);

            // Set queue ready bit
            write_reg(QUEUE_READY_OFFSET, 1);

            // 15. Read device capacity (INV-MMIO-3: volatile u64 config read)
            self.capacity = mmio.read_u64(0x100);

            // 16. Update block device info
            self.disk.set_capacity(self.capacity as u64);
            self.disk.set_request_fn(Self::handle_request);
            self.disk.set_async_read_fn(Self::async_read_fn);
            self.disk.set_async_write_fn(Self::async_write_fn);
            *self.virtqueue.lock() = Some(virtqueue);

            // 17. State machine: DRIVER_OK (0x04)
            write_reg(STATUS_OFFSET, 0x01 | 0x02 | 0x08 | 0x04);

            // Memory barrier
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);

            // Mark as initialized
            *self.initialized.lock() = true;

            Ok(())
        }
    }

    /// Get capacity
    pub fn get_capacity(&self) -> u64 {
        self.capacity
    }

    /// Handle I/O request (registered as `request_fn`; INV-BLKDEV-2/3 —
    /// the block layer calls this only for THIS device, whose
    /// private_data is the init-time `&'static VirtIOBlkDevice`).
    fn handle_request(req: &mut Request) {
        let device = match req.device.private::<VirtIOBlkDevice>() {
            Some(d) => d,
            None => {
                req.error.store(-5, core::sync::atomic::Ordering::Release);
                return;
            }
        };

        // Execute operation based on command type
        let result = match req.cmd_type {
            crate::drivers::types::ReqCmd::Read => {
                // Read block
                device.read_block(req.sector, &mut req.buffer)
            }
            crate::drivers::types::ReqCmd::Write => {
                // Write block
                device.write_block(req.sector, &req.buffer)
            }
            crate::drivers::types::ReqCmd::Flush => {
                // Flush operation (return success for now)
                Ok(())
            }
        };

        // R8-M2: record the device status on the Request itself —
        // submit_request returns 0 whenever a request_fn exists, so the
        // caller never saw these errors (blkdev_read copied a zero buffer
        // and returned success, which bio then cached as BH_Uptodate).
        match result {
            Ok(()) => req.error.store(0, core::sync::atomic::Ordering::Release),
            Err(err) => {
                crate::pr_err!("virtio-blk: I/O error: {}", err);
                req.error.store(err, core::sync::atomic::Ordering::Release);
            }
        }
    }

    /// Read block
    pub fn read_block(&self, sector: u64, buf: &mut [u8]) -> Result<(), i32> {
        if !*self.initialized.lock_irqsave() {
            return Err(-5);  // EIO
        }

        // Phase 1: Set up and submit request (under queue lock)
        let (used_ring_ptr, prev_used, submitted_desc_id, queue_sz, header, resp) = {
            // Get VirtQueue (irqsafe: IRQ handler also takes this lock)
            let mut queue_guard = self.virtqueue.lock_irqsave();
            let _nest = VirtioLockNest::new();
            let queue = match queue_guard.as_mut() {
                Some(q) => q,
                None => return Err(-5),
            };


            // Construct VirtIO block request header
            let req_header = VirtIOBlkReqHeader {
                type_: req_type::VIRTIO_BLK_T_IN,
                reserved: 0,
                sector,
            };

            // INV-KPTR-1: the ONE audited allocation per buffer; error
            // paths below need no manual dealloc (Drop owns it now).
            let header = match crate::safety::ptr::KPtr::new(req_header) {
                Some(h) => h,
                None => return Err(-12),  // ENOMEM
            };
            let resp = match crate::safety::ptr::KPtr::new(VirtIOBlkResp {
                status: 0xFF,  // Initialize to invalid state
            }) {
                Some(r) => r,
                None => return Err(-12),  // ENOMEM
            };
            let header_ptr = header.as_ptr();
            let resp_ptr = resp.as_ptr();

            // VirtIO descriptor flags
            const VIRTQ_DESC_F_NEXT: u16 = 1;
            const VIRTQ_DESC_F_WRITE: u16 = 2;

            // Convert virtual addresses to physical addresses (VirtIO devices need physical addresses for DMA)
            let header_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(header_ptr as u64)
            ).0;
            let data_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(buf.as_ptr() as u64)
            ).0;
            let resp_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(resp_ptr as u64)
            ).0;

            // Allocate three descriptors — R22-6: dealloc header/resp on
            // failure (write_block already had this; read_block leaked).
            // (alloc failures drop header/resp via KPtr::drop — the
            // manual dealloc sites are gone.)
            let header_desc_idx = queue.alloc_desc().ok_or(-5)?;
            let data_desc_idx = queue.alloc_desc().ok_or(-5)?;
            let resp_desc_idx = queue.alloc_desc().ok_or(-5)?;

            // Set request header descriptor (read-only, device reads) - use physical address
            queue.set_desc(
                header_desc_idx,
                header_phys_addr,
                core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
                VIRTQ_DESC_F_NEXT,
                data_desc_idx,
            );

            // Set data buffer descriptor (write-only, device writes) - use physical address
            queue.set_desc(
                data_desc_idx,
                data_phys_addr,
                buf.len() as u32,
                VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,  // WRITE + NEXT
                resp_desc_idx,
            );

            // Set response descriptor (write-only, device writes) - use physical address
            queue.set_desc(
                resp_desc_idx,
                resp_phys_addr,
                core::mem::size_of::<VirtIOBlkResp>() as u32,
                VIRTQ_DESC_F_WRITE,  // Device writes status byte
                0,
            );

            // Snapshot actual used ring index for per-desc matching
            let prev = queue.get_used();
            // This chain's submission ordinal (read BEFORE the increment
            // below names it), for the sync-chain tombstone.
            let ordinal = get_mmio_expected_used_idx();

            // Submit to available ring
            queue.submit(header_desc_idx);

            // Notify device
            queue.notify();

            // Keep global counter in sync for async pending slot tracking
            increment_mmio_expected_used_idx();

            // Sync-chain tombstone: the device-truthed walker discards
            // this chain's used-ring entry by head_desc instead of
            // counting it unmatched (or worse — under the old positional
            // walk — firing a live async pending with it).
            mmio_publish_sync_tombstone(ordinal, header_desc_idx);

            let used_ptr = queue.used_ring_ptr();
            let q_size = queue.queue_size;

            (used_ptr, prev, header_desc_idx as u32, q_size, header, resp)
        };
        // queue_guard dropped here — VirtQueue spinlock released

        // Phase 2: Wait for THIS descriptor's completion (interrupt-driven)
        let completed = VirtQueue::wait_for_desc_completion(
            used_ring_ptr,
            &VIRTIO_BLK_WAIT_QUEUE,
            prev_used,
            submitted_desc_id,
            queue_sz,
        );

        // Phase 3: Check response
        if !completed {
            // R8-M2 (NEW2 mechanism 2): the descriptors are STILL SUBMITTED
            // — freeing header/resp here (and the caller later dropping its
            // data buffer) let the device DMA 4KB into freed, reallocated
            // memory: the wandering corruption behind fdtable/BufferHead
            // damage. Drain the used ring for THIS descriptor with a long
            // bounded spin; if the device truly never completes, LEAK the
            // header/resp bytes (integrity over a leak) and return EIO.
            // SAFETY: used_ring_ptr is the MMIO-mapped used ring captured
            // under the queue lock in Phase 1; read_volatile is the I/O
            // access pattern the completion path already uses.
            let mut late = false;
            // Same ring-scan discipline as wait_for_desc_completion,
            // through the typed volatile readers (INV-SAFECALL).
            for _ in 0..50_000_000u64 {
                core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
                let used_idx = used_ring_idx(used_ring_ptr);
                let mut scan = prev_used;
                while scan != used_idx {
                    let slot = scan as usize % queue_sz as usize;
                    let entry_id = used_ring_elem_id(used_ring_ptr, slot);
                    if entry_id == submitted_desc_id {
                        late = true;
                        break;
                    }
                    scan = scan.wrapping_add(1);
                }
                if late {
                    break;
                }
                core::hint::spin_loop();
            }
            if !late {
                // R21-N2 unchanged: LEAK header/resp on a true timeout
                // (the device may still DMA into them) — KPtr must NOT
                // drop here.
                core::mem::forget(header);
                core::mem::forget(resp);
                return Err(-5); // EIO — header/resp deliberately leaked
            }
            // Completed late — fall through to the normal status handling.
        }

        // INV-KPTR-5: the device wrote the status byte; volatile read.
        // (header/resp dealloc is KPtr::drop at scope end.)
        let status = resp.read_volatile().status;
        if status == queue_status::VIRTIO_BLK_S_OK {
            Ok(())
        } else {
            Err(-5)  // EIO
        }
    }

    /// Write block
    pub fn write_block(&self, sector: u64, buf: &[u8]) -> Result<(), i32> {
        if !*self.initialized.lock_irqsave() {
            return Err(-5);  // EIO
        }

        // Phase 1: Set up and submit request (under queue lock)
        let (used_ring_ptr, prev_used, submitted_desc_id, queue_sz, header, resp) = {
            // Get VirtQueue (irqsafe: IRQ handler also takes this lock)
            let mut queue_guard = self.virtqueue.lock_irqsave();
            let _nest = VirtioLockNest::new();
            let queue = queue_guard.as_mut().ok_or(-5)?;


            // Construct VirtIO block request header
            let req_header = VirtIOBlkReqHeader {
                type_: req_type::VIRTIO_BLK_T_OUT,
                reserved: 0,
                sector,
            };

            // INV-KPTR-1: the ONE audited allocation per buffer; error
            // paths below need no manual dealloc (Drop owns it now).
            let header = match crate::safety::ptr::KPtr::new(req_header) {
                Some(h) => h,
                None => return Err(-12),  // ENOMEM
            };
            let resp = match crate::safety::ptr::KPtr::new(VirtIOBlkResp {
                status: 0xFF,  // Initialize to invalid state
            }) {
                Some(r) => r,
                None => return Err(-12),  // ENOMEM
            };
            let header_ptr = header.as_ptr();
            let resp_ptr = resp.as_ptr();

            // VirtIO descriptor flags
            const VIRTQ_DESC_F_NEXT: u16 = 1;
            const VIRTQ_DESC_F_WRITE: u16 = 2;

            // Convert virtual addresses to physical addresses (VirtIO devices need physical addresses for DMA)
            let header_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(header_ptr as u64)
            ).0;
            let data_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(buf.as_ptr() as u64)
            ).0;
            let resp_phys_addr = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(resp_ptr as u64)
            ).0;

            // Allocate three descriptors
            let header_desc_idx = queue.alloc_desc().ok_or(-5)?;
            let data_desc_idx = queue.alloc_desc().ok_or(-5)?;
            let resp_desc_idx = queue.alloc_desc().ok_or(-5)?;

            // Set request header descriptor (read-only, device reads) - use physical address
            queue.set_desc(
                header_desc_idx,
                header_phys_addr,
                core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
                VIRTQ_DESC_F_NEXT,
                data_desc_idx,
            );

            // Set data buffer descriptor (read-only, device reads) - use physical address
            queue.set_desc(
                data_desc_idx,
                data_phys_addr,
                buf.len() as u32,
                VIRTQ_DESC_F_NEXT,
                resp_desc_idx,
            );

            // Set response descriptor (write-only, device writes) - use physical address
            queue.set_desc(
                resp_desc_idx,
                resp_phys_addr,
                core::mem::size_of::<VirtIOBlkResp>() as u32,
                VIRTQ_DESC_F_WRITE,
                0,
            );

            // Snapshot actual used ring index for per-desc matching
            let prev = queue.get_used();
            // This chain's submission ordinal (read BEFORE the increment
            // below names it), for the sync-chain tombstone.
            let ordinal = get_mmio_expected_used_idx();

            // Submit to available ring
            queue.submit(header_desc_idx);

            // Notify device
            queue.notify();

            // Keep global counter in sync for async pending slot tracking
            increment_mmio_expected_used_idx();

            // Sync-chain tombstone: the device-truthed walker discards
            // this chain's used-ring entry by head_desc instead of
            // counting it unmatched (or worse — under the old positional
            // walk — firing a live async pending with it).
            mmio_publish_sync_tombstone(ordinal, header_desc_idx);

            let used_ptr = queue.used_ring_ptr();
            let q_size = queue.queue_size;

            (used_ptr, prev, header_desc_idx as u32, q_size, header, resp)
        };
        // queue_guard dropped here — VirtQueue spinlock released

        // Phase 2: Wait for THIS descriptor's completion (interrupt-driven)
        let completed = VirtQueue::wait_for_desc_completion(
            used_ring_ptr,
            &VIRTIO_BLK_WAIT_QUEUE,
            prev_used,
            submitted_desc_id,
            queue_sz,
        );

        // Phase 3: Check response
        if !completed {
            // R8-M2 (NEW2 mechanism 2): the descriptors are STILL SUBMITTED
            // — freeing header/resp here (and the caller later dropping its
            // data buffer) let the device DMA 4KB into freed, reallocated
            // memory: the wandering corruption behind fdtable/BufferHead
            // damage. Drain the used ring for THIS descriptor with a long
            // bounded spin; if the device truly never completes, LEAK the
            // header/resp bytes (integrity over a leak) and return EIO.
            // SAFETY: used_ring_ptr is the MMIO-mapped used ring captured
            // under the queue lock in Phase 1; read_volatile is the I/O
            // access pattern the completion path already uses.
            let mut late = false;
            // Same ring-scan discipline as wait_for_desc_completion,
            // through the typed volatile readers (INV-SAFECALL).
            for _ in 0..50_000_000u64 {
                core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
                let used_idx = used_ring_idx(used_ring_ptr);
                let mut scan = prev_used;
                while scan != used_idx {
                    let slot = scan as usize % queue_sz as usize;
                    let entry_id = used_ring_elem_id(used_ring_ptr, slot);
                    if entry_id == submitted_desc_id {
                        late = true;
                        break;
                    }
                    scan = scan.wrapping_add(1);
                }
                if late {
                    break;
                }
                core::hint::spin_loop();
            }
            if !late {
                // R21-N2 unchanged: LEAK header/resp on a true timeout
                // (the device may still DMA into them) — KPtr must NOT
                // drop here.
                core::mem::forget(header);
                core::mem::forget(resp);
                return Err(-5); // EIO — header/resp deliberately leaked
            }
            // Completed late — fall through to the normal status handling.
        }

        // INV-KPTR-5: the device wrote the status byte; volatile read.
        // (header/resp dealloc is KPtr::drop at scope end.)
        let status = resp.read_volatile().status;
        if status == queue_status::VIRTIO_BLK_S_OK {
            Ok(())
        } else {
            Err(-5)  // EIO
        }
    }
}

/// Per-disk kick (rung4 prep move 2): resolves the disk's PCI slot from
/// its private data and doorbells that queue if quietly-submitted
/// chains are pending. Registered in VIRTIO_BLK_OPS.kick.
fn virtio_blk_kick_op(disk: &super::types::GenDisk) {
    // The registered private data is the slot index as a raw pointer
    // (set_private_data(slot as *mut u8) — INV-BLKDEV-2/3).
    let slot = disk.private_data.map(|p| p as usize).unwrap_or(usize::MAX);
    pci_blk_kick(slot);
}

/// VirtIO block device operations
static VIRTIO_BLK_OPS: BlockDeviceOps = BlockDeviceOps {
    open: None,
    release: None,
    getgeo: None,
    kick: Some(virtio_blk_kick_op),
};

// Async I/O methods (added in a separate impl block)
impl VirtIOBlkDevice {
    // ========================================================================
    // Async I/O submission (fire-and-forget, completion via interrupt)
    // ========================================================================

    /// Submit an async read request. Does NOT wait for completion.
    ///
    /// The caller must ensure `buf` remains valid until `completion.complete()` is called
    /// (from interrupt context). The completion is stored in the pending-I/O table
    /// and signaled by the interrupt handler.
    ///
    /// # Returns
    /// Ok(()) on successful submission, Err(i32) on failure.
    fn submit_read_async(
        &self,
        sector: u64,
        buf: &mut [u8],
        completion: &crate::fs::io_completion::IoCompletion,
    ) -> Result<(), i32> {
        if !*self.initialized.lock_irqsave() {
            return Err(-5);  // EIO
        }


        let mut queue_guard = self.virtqueue.lock_irqsave();
        let _nest = VirtioLockNest::new();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return Err(-5),
        };

        // INV-KPTR-1: one audited allocation per buffer; every error
        // path below drops them automatically (no manual deallocs).
        let header = match crate::safety::ptr::KPtr::new(VirtIOBlkReqHeader {
            type_: req_type::VIRTIO_BLK_T_IN,
            reserved: 0,
            sector,
        }) {
            Some(h) => h,
            None => return Err(-12),
        };
        let resp = match crate::safety::ptr::KPtr::new(VirtIOBlkResp { status: 0xFF }) {
            Some(r) => r,
            None => return Err(-12),
        };
        let header_ptr = header.as_ptr() as *mut u8;
        let resp_ptr = resp.as_ptr() as *mut u8;

        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        let header_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(header_ptr as u64),
        ).0;
        let data_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(buf.as_ptr() as u64),
        ).0;
        let resp_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(resp_ptr as u64),
        ).0;

        // (alloc failures drop header/resp via KPtr::drop.)
        let header_desc_idx = queue.alloc_desc().ok_or(-5)?;
        let data_desc_idx = queue.alloc_desc().ok_or(-5)?;
        let resp_desc_idx = queue.alloc_desc().ok_or(-5)?;

        queue.set_desc(header_desc_idx, header_phys,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT, data_desc_idx);
        queue.set_desc(data_desc_idx, data_phys, buf.len() as u32,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT, resp_desc_idx);
        queue.set_desc(resp_desc_idx, resp_phys,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            VIRTQ_DESC_F_WRITE, 0); // Device writes the status byte (DRIV-H4)

        let prev = get_mmio_expected_used_idx();
        let slot = prev as usize % MAX_PENDING_IO;
        // Occupancy check BEFORE submit: an entry still sitting at this
        // ordinal is a TOMBSTONE (timed-out chain whose used-ring entry the
        // walker has not consumed — the MMIO queue is single-in-flight, so
        // ordinals and completion order coincide). Overwriting it would
        // silently drop the tombstone's reservation and let the walker fire
        // this new pending with the OLD chain's used-ring entry. Refuse
        // instead; the caller falls back and the tombstone retires shortly.
        {
            let table = VIRTIO_MMIO_PENDING.lock_irqsave();
            if table[slot].is_some() {
                return Err(-5);
            }
        }
        queue.submit(header_desc_idx);
        queue.notify();
        increment_mmio_expected_used_idx();

        // Store in pending table
        // INV-KPTR-4: ownership transfers to the pending table (the
        // walker frees or pools header_ptr exactly as before).
        core::mem::forget(header);
        core::mem::forget(resp);
        let pending = PendingIo {
            completion: completion as *const _ as *mut _,
            resp_ptr,
            resp_layout: alloc::alloc::Layout::new::<VirtIOBlkResp>(),
            header_ptr,
            header_layout: alloc::alloc::Layout::new::<VirtIOBlkReqHeader>(),
            // Device-truth keys for the walker below (same discipline as
            // the PCI path): the chain's head descriptor id — what the
            // used ring's UsedElem.id names when the chain completes —
            // plus the submission ordinal. The walk used to rely purely on
            // the ordinal position ("single in-flight chain, so completion
            // order is submission order"), but any chain that consumed an
            // ordinal WITHOUT producing a used-ring entry (a dropped
            // header-less chain, an abandoned-but-unconsumed slot) skews
            // every later positional take by one — firing each pending one
            // chain EARLY with a later chain's OK status while its own DMA
            // never landed: the zero-serve family. Match by head_desc.
            head_desc: header_desc_idx as u32,
            ordinal: prev,
            timed_out: false,
            // Zero-serve guard: the walker stamps this into the delivered
            // completion; bread_wait verifies it against its own buffer.
            data_ptr: buf.as_ptr() as usize,
            is_write: false,
            // block:block_rq_complete geometry for the walker below.
            trace_dev: ((self.disk.major << 20) | self.disk.first_minor) as u32,
            trace_sector: sector,
            trace_nr_sector: (buf.len() / 512) as u32,
            n_descs: 3,
        };
        VIRTIO_MMIO_PENDING.lock_irqsave()[slot] = Some(pending);

        Ok(())
    }

    /// Submit an async WRITE request. Does NOT wait for completion — the
    /// mirror of [`Self::submit_read_async`] for the OUT direction (the
    /// plugged-write primitive). Two direction-specific differences, both
    /// load-bearing:
    ///
    /// 1. The data descriptor carries NO VIRTQ_DESC_F_WRITE: the device
    ///    READS the buffer. `buf` is therefore the DMA SOURCE and must stay
    ///    valid AND UNMODIFIED until the completion fires (the write-batch
    ///    contract; freeing or mutating it mid-flight corrupts the disk).
    /// 2. The pending's is_write bit makes the walker's trace record
    ///    report "W" (block:block_rq_complete truth).
    ///
    /// Everything else — pending-table slot discipline, head_desc
    /// device-truth matching, the zero-serve data_ptr token — is the read
    /// path's, unchanged.
    fn submit_write_async(
        &self,
        sector: u64,
        buf: &[u8],
        completion: &crate::fs::io_completion::IoCompletion,
    ) -> Result<(), i32> {
        if !*self.initialized.lock_irqsave() {
            return Err(-5);  // EIO
        }


        let mut queue_guard = self.virtqueue.lock_irqsave();
        let _nest = VirtioLockNest::new();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return Err(-5),
        };

        // INV-KPTR-1: one audited allocation per buffer; error paths
        // drop automatically.
        let header = match crate::safety::ptr::KPtr::new(VirtIOBlkReqHeader {
            type_: req_type::VIRTIO_BLK_T_OUT,
            reserved: 0,
            sector,
        }) {
            Some(h) => h,
            None => return Err(-12),
        };
        let resp = match crate::safety::ptr::KPtr::new(VirtIOBlkResp { status: 0xFF }) {
            Some(r) => r,
            None => return Err(-12),
        };
        let header_ptr = header.as_ptr() as *mut u8;
        let resp_ptr = resp.as_ptr() as *mut u8;

        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        let header_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(header_ptr as u64),
        ).0;
        let data_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(buf.as_ptr() as u64),
        ).0;
        let resp_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(resp_ptr as u64),
        ).0;

        // (alloc failures drop header/resp via KPtr::drop.)
        let header_desc_idx = queue.alloc_desc().ok_or(-5)?;
        let data_desc_idx = queue.alloc_desc().ok_or(-5)?;
        let resp_desc_idx = queue.alloc_desc().ok_or(-5)?;

        queue.set_desc(header_desc_idx, header_phys,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT, data_desc_idx);
        // WRITE direction: the device READS the data — no F_WRITE here.
        queue.set_desc(data_desc_idx, data_phys, buf.len() as u32,
            VIRTQ_DESC_F_NEXT, resp_desc_idx);
        queue.set_desc(resp_desc_idx, resp_phys,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            VIRTQ_DESC_F_WRITE, 0); // Device writes the status byte (DRIV-H4)

        let prev = get_mmio_expected_used_idx();
        let slot = prev as usize % MAX_PENDING_IO;
        // Occupancy check BEFORE submit (the tombstone-reservation rule of
        // submit_read_async — overwriting a live slot would let the walker
        // fire this pending with the OLD chain's used-ring entry).
        {
            let table = VIRTIO_MMIO_PENDING.lock_irqsave();
            if table[slot].is_some() {
                return Err(-5);
            }
        }
        queue.submit(header_desc_idx);
        queue.notify();
        increment_mmio_expected_used_idx();

        // Store in pending table
        // INV-KPTR-4: ownership transfers to the pending table (the
        // walker frees or pools header_ptr exactly as before).
        core::mem::forget(header);
        core::mem::forget(resp);
        let pending = PendingIo {
            completion: completion as *const _ as *mut _,
            resp_ptr,
            resp_layout: alloc::alloc::Layout::new::<VirtIOBlkResp>(),
            header_ptr,
            header_layout: alloc::alloc::Layout::new::<VirtIOBlkReqHeader>(),
            // Device-truth key for the walker (same discipline as the read
            // path): match the used-ring entry by the chain's head id.
            head_desc: header_desc_idx as u32,
            ordinal: prev,
            timed_out: false,
            // Zero-serve guard token (attribution proof for the waiter).
            data_ptr: buf.as_ptr() as usize,
            is_write: true,
            // block:block_rq_complete geometry for the walker below.
            trace_dev: ((self.disk.major << 20) | self.disk.first_minor) as u32,
            trace_sector: sector,
            trace_nr_sector: (buf.len() / 512) as u32,
            n_descs: 3,
        };
        VIRTIO_MMIO_PENDING.lock_irqsave()[slot] = Some(pending);

        Ok(())
    }

    /// Static wrapper for async read — the registered `async_read_fn`
    /// (INV-BLKDEV-2/3: called only for THIS device, whose private_data
    /// is the init-time `&'static VirtIOBlkDevice`; `buf` stays a valid
    /// DMA target until the completion fires — the caller's contract).
    ///
    /// Resolves the owning device through GenDisk.private_data (set to the
    /// VirtIOBlkDevice address in `init()`). The previous body cast the
    /// GEN disk pointer itself (`disk as *const VirtIOBlkDevice`) — a
    /// field pointer miscast into its container; every standard boot uses
    /// virtio-blk-pci so the path was dead, but any MMIO-disk boot that
    /// reached it would have read device fields out of bounds.
    fn async_read_fn(
        disk: &crate::drivers::types::GenDisk,
        sector: u64,
        buf: &mut [u8],
        completion: &crate::fs::io_completion::IoCompletion,
    ) -> i32 {
        let Some(device) = disk.private::<VirtIOBlkDevice>() else { return -5 };
        match device.submit_read_async(sector, buf, completion) {
            Ok(()) => 0,
            Err(e) => e,
        }
    }

    /// Static wrapper for async write — the registered `async_write_fn`
    /// (INV-BLKDEV-2/3). `buf` is the DMA source and must stay stable
    /// until the completion fires (see `submit_write_async`) — the
    /// caller's registration contract.
    fn async_write_fn(
        disk: &crate::drivers::types::GenDisk,
        sector: u64,
        buf: &[u8],
        completion: &crate::fs::io_completion::IoCompletion,
    ) -> i32 {
        let Some(device) = disk.private::<VirtIOBlkDevice>() else { return -5 };
        match device.submit_write_async(sector, buf, completion) {
            Ok(()) => 0,
            Err(e) => e,
        }
    }
}

/// Global VirtIO block device (MMIO)
static VIRTIO_BLK: crate::safety::slot::OnceSlot<VirtIOBlkDevice> =
    crate::safety::slot::OnceSlot::new();

/// Maximum number of PCI virtio-blk disks (OH boots six: updater/system/
/// vendor/sys_prod/chip_prod/userdata as vda..vdf; two spare letters).
pub const MAX_PCI_BLK_DISKS: usize = 8;

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
/// Per-CPU pool depth cap. Deep enough to ride out bursty windows
/// (a 64-request read window submits 64 chains from one CPU) without
// unbounded retention; excess frees go to the heap.
const IO_BUF_POOL_CAP: usize = 256;


static IO_BUF_POOLS: [Spinlock<IoBufFreelist>; crate::config::MAX_CPUS] =
    [const {
        Spinlock::new(IoBufFreelist {
            head: 0,
            count: 0,
        })
    }; crate::config::MAX_CPUS];

/// Allocate one 64-byte io_buf: per-CPU pool first, heap on underflow.
/// Returns null on heap OOM (callers already handle that shape).
pub(crate) fn io_buf_alloc() -> *mut u8 {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < IO_BUF_POOLS.len() {
        let mut pl = IO_BUF_POOLS[cpu].lock_irqsave();
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

/// Return one 64-byte io_buf: per-CPU pool while under the cap, heap
/// beyond it. `p` must have come from `io_buf_alloc` (or an identical
/// 64/16 heap allocation) and must be dead from the device's point of
/// view (see the lifetime note above).
pub(crate) fn io_buf_free(p: *mut u8) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < IO_BUF_POOLS.len() {
        let mut pl = IO_BUF_POOLS[cpu].lock_irqsave();
        if pl.count < IO_BUF_POOL_CAP {
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

/// Split one pooled 64-byte io_buf into its typed halves and
/// initialize them (INV-SAFECALL-1: header at offset 0, response at
/// +48 — the R17-C combined block; THE one audited carve edge for the
/// pool shape; `io_buf` must come from `io_buf_alloc`).
pub(crate) fn io_buf_init(
    io_buf: *mut u8,
    type_: u32,
    sector: u64,
) -> (*mut VirtIOBlkReqHeader, *mut VirtIOBlkResp) {
    // SAFETY: io_buf is non-null (callers check), 64 bytes, 16-aligned;
    // +48 leaves 16 bytes for the response — both in bounds. The
    // initializations follow immediately.
    unsafe {
        let header_ptr = io_buf as *mut VirtIOBlkReqHeader;
        let resp_ptr = io_buf.add(48) as *mut VirtIOBlkResp;
        *header_ptr = VirtIOBlkReqHeader {
            type_,
            reserved: 0,
            sector,
        };
        (*resp_ptr).status = 0xFF;
        (header_ptr, resp_ptr)
    }
}

/// Release a pending entry's io block: the per-CPU pool when the layout
/// is the pooled R17-C shape, the heap otherwise (INV-SAFECALL-1: the
/// ONE audited free edge for the pending-walker family; `ptr`/`layout`
/// come from a PendingIo the walker just retired).
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

/// Deliver a pending entry's completion (INV-SAFECALL-1: the ONE
/// audited deref edge; the delivering-protocol count taken under the
/// table lock keeps the IoCompletion (and its waiter's stack) alive
/// across the call).
/// Mark a pending's completion as under delivery (the walker-vs-abandon
/// handshake; same audited-deref discipline as [`deliver_completion`]).
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

/// PCI virtio-blk devices (one per slot; slot 0 is the boot disk when the
/// boot probe found PCI disks). Each slot owns its complete I/O engine:
/// device, vring, BLK lock, sync wait queue, expected-used counter, pending
/// table and unked counter — the singletons the pre-1b kernel had, per disk.
static PCI_BLK_DEVICES: [crate::safety::slot::OnceSlot<super::types::VirtIOPCI>; MAX_PCI_BLK_DISKS] =
    [const { crate::safety::slot::OnceSlot::new() }; MAX_PCI_BLK_DISKS];

/// Configured VirtQueues, one per PCI virtio-blk slot (mutable access
/// requires the slot's BLK lock — see `get_pci_device_queue_mut_at`).
static PCI_BLK_QUEUES: [crate::safety::slot::OnceSlot<VirtQueue>; MAX_PCI_BLK_DISKS] =
    [const { crate::safety::slot::OnceSlot::new() }; MAX_PCI_BLK_DISKS];

/// Per-slot BLK lock: serializes all I/O operations on ONE disk (submit +
/// pending-store under the lock, completion collection under the same
/// lock). Disks never share a lock, and no path holds two disk locks
/// nested (slot-loops take them strictly sequentially).
pub(crate) static PCI_BLK_LOCKS: [Spinlock<()>; MAX_PCI_BLK_DISKS] =
    [const { Spinlock::new(()) }; MAX_PCI_BLK_DISKS];

// ============================================================================
// PCI_BLK_LOCKS holder forensics (the OH b125 spin-wedge hunt)
//
// b125 (KVM q35 SMP2, storage-heavy boot phase) wedged permanently with
// cpu1 spinning on PCI_BLK_LOCKS[0] ("DEADLOCK: spinlock stuck cpu=1
// lock=0xffffffff80ba5a94") and NOTHING else on the serial log — no
// second stuck CPU, no VW reports, no panic. The holder was therefore
// neither spinning on another lock (it would have printed its own
// DEADLOCK line) nor progressing; every BLK critical section in the tree
// is supposed to be short RAM work, but the sync submit paths used to do
// two potentially-slow things UNDER the irqsave lock: (a) the 64-byte
// io_buf heap alloc/dealloc (nested heap-lock contention inside an
// IRQ-off window) and (b) `notify()` — an MMIO write that VM-exits to
// QEMU under KVM, where six virtio-blk disks make the exit path subject
// to BQL/iothread convoys. Both are now moved OUTSIDE the lock (see
// read_block_once / write_block_once / flush_block_once /
// pci_submit_data_async), Linux-parity style.
//
// To make any recurrence self-describing, every acquire goes through
// `pci_blk_lock()` below, which records holder cpu/pid/acquire-site/
// acquire-jiffies. A contended acquire that outlives a bounded try-spin
// prints the CURRENT holder via the console's lock-free putchar (never
// printk — the holder may be wedged inside printk's own lock), and the
// per-jiffy timer-softirq watchdog (vw_lag_watchdog) prints any holder
// older than 5 s.
// ============================================================================

/// Holder cpu + 1 (0 = free / mid-handoff), per slot.
static PCI_BLK_HOLDER_CPU: [core::sync::atomic::AtomicUsize; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; MAX_PCI_BLK_DISKS];
/// Holder pid (0 = unknown / early boot).
static PCI_BLK_HOLDER_PID: [core::sync::atomic::AtomicU32; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MAX_PCI_BLK_DISKS];
/// Holder's acquire call site (return address into read/write/flush/
/// submit/walker). 0 when unavailable.
static PCI_BLK_HOLDER_RA: [core::sync::atomic::AtomicUsize; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; MAX_PCI_BLK_DISKS];
/// Jiffies when the current holder acquired (0 = free). Saturating reads
/// are fine: this is diagnostic state, never control flow.
static PCI_BLK_HOLDER_AT: [core::sync::atomic::AtomicU64; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_PCI_BLK_DISKS];
/// Context tag of the holder: bit 0 = in hardirq, bit 1 = in softirq.
static PCI_BLK_HOLDER_CTX: [core::sync::atomic::AtomicU32; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MAX_PCI_BLK_DISKS];
/// Last jiffy each slot printed a contended-acquire forensics line
/// (rate limit for the lock-free printer).
static PCI_BLK_SPIN_PRINT_AT: [core::sync::atomic::AtomicU64; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_PCI_BLK_DISKS];

/// Capture the caller's return address (the virtio acquire site) for
/// holder forensics — same technique as spinlock.rs's
/// caller_return_address().
#[inline]
fn pci_blk_acquire_site() -> usize {
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

/// Lock-free console byte (the holder may be wedged holding printk's
/// lock — same discipline as RawSpinlock::deadlock_warn).
#[inline]
fn wedge_put(b: u8) {
    // SAFETY: putchar_no_lock is a byte-wise polling UART write usable
    // from any context, including with interrupts disabled.
    unsafe { crate::console::putchar_no_lock(b) }
}

/// Lock-free hex printer (usize, no leading zeros beyond the top nibble
/// set — keeps forensics lines short).
fn wedge_hex(mut v: usize) {
    let mut started = false;
    let mut shift = (core::mem::size_of::<usize>() * 4) as i32;
    while shift > 0 {
        shift -= 4;
        let nibble = (v >> shift as usize) & 0xF;
        if nibble != 0 || started || shift == 0 {
            let c = if nibble < 10 {
                b'0' + nibble as u8
            } else {
                b'a' + (nibble - 10) as u8
            };
            wedge_put(c);
            started = true;
        }
    }
}

/// Lock-free decimal printer (u64).
fn wedge_dec(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        wedge_put(b'0');
        return;
    }
    while v > 0 && n < buf.len() {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        wedge_put(buf[n]);
    }
}

/// Print one contended-acquire forensics line through the lock-free
/// console: who holds `slot`, since when, from which site — plus the
/// waiter's own site. Rate-limited per slot (>= 2 s apart).
fn pci_blk_contended_forensics(slot: usize, waiter_site: usize) {
    let now = crate::drivers::hotpath::timer::get_jiffies();
    let last = PCI_BLK_SPIN_PRINT_AT[slot].load(core::sync::atomic::Ordering::Acquire);
    if last != 0 && now.saturating_sub(last) < 2 {
        return;
    }
    PCI_BLK_SPIN_PRINT_AT[slot].store(now, core::sync::atomic::Ordering::Release);

    for &b in b"PCI-BLK-CONTEND slot=" {
        wedge_put(b);
    }
    wedge_dec(slot as u64);
    for &b in b" holder_cpu=" {
        wedge_put(b);
    }
    let hcpu = PCI_BLK_HOLDER_CPU[slot].load(core::sync::atomic::Ordering::Acquire);
    if hcpu == 0 {
        wedge_put(b'?');
    } else {
        wedge_dec((hcpu - 1) as u64);
    }
    for &b in b" pid=" {
        wedge_put(b);
    }
    wedge_dec(PCI_BLK_HOLDER_PID[slot].load(core::sync::atomic::Ordering::Acquire) as u64);
    for &b in b" ra=0x" {
        wedge_put(b);
    }
    wedge_hex(PCI_BLK_HOLDER_RA[slot].load(core::sync::atomic::Ordering::Acquire));
    for &b in b" ctx=" {
        wedge_put(b);
    }
    wedge_dec(PCI_BLK_HOLDER_CTX[slot].load(core::sync::atomic::Ordering::Acquire) as u64);
    for &b in b" held_ms=" {
        wedge_put(b);
    }
    let at = PCI_BLK_HOLDER_AT[slot].load(core::sync::atomic::Ordering::Acquire);
    let held_ms = crate::drivers::hotpath::timer::jiffies_to_msecs(now.saturating_sub(at).min(0xFFFF));
    wedge_dec(held_ms);
    for &b in b" waiter_cpu=" {
        wedge_put(b);
    }
    wedge_dec(crate::arch::smp::cpu_id() as u64);
    for &b in b" waiter_site=0x" {
        wedge_put(b);
    }
    wedge_hex(waiter_site);
    // Ring state (all atomics / RAM reads — safe under any wedge).
    for &b in b" used=" {
        wedge_put(b);
    }
    let used = match get_pci_device_queue_at(slot) {
        Some(q) => used_ring_idx(q.used_ring_ptr()) as u64,
        None => 0,
    };
    wedge_dec(used);
    for &b in b" walked=" {
        wedge_put(b);
    }
    wedge_dec(
        PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire) as u64,
    );
    for &b in b" expected=" {
        wedge_put(b);
    }
    wedge_dec(PCI_BLK_EXPECTED_USED_IDX[slot].load(core::sync::atomic::Ordering::Acquire) as u64);
    wedge_put(b'\n');
}


impl PciBlkGuard {
    #[inline]
    fn arm(slot: usize, inner: SpinlockIrqGuard<'static, ()>, site: usize) -> Self {
        use core::sync::atomic::Ordering;
        PCI_BLK_HOLDER_CPU[slot].store(crate::arch::smp::cpu_id() as usize + 1, Ordering::Release);
        PCI_BLK_HOLDER_PID[slot].store(crate::sched::hotpath::get_current_pid(), Ordering::Release);
        PCI_BLK_HOLDER_RA[slot].store(site, Ordering::Release);
        let mut ctx = 0u32;
        if crate::interrupt::preempt::in_irq() {
            ctx |= 1;
        }
        if crate::interrupt::preempt::in_softirq() {
            ctx |= 2;
        }
        PCI_BLK_HOLDER_CTX[slot].store(ctx, Ordering::Release);
        PCI_BLK_HOLDER_AT[slot].store(crate::drivers::hotpath::timer::get_jiffies(), Ordering::Release);
        Self { slot, inner }
    }
}

// INV-SLOT-3 (OnceSlot::get_mut_locked): PCI_BLK_LOCKS[slot] governs
// every mutation of PCI_BLK_QUEUES[slot]'s queue — submit paths, the
// completion walker, and set_pci_device_queue's one-time publication
// all run under (or before any) holder of this lock.
impl crate::safety::slot::ExclusivityProof for PciBlkGuard {}

impl Drop for PciBlkGuard {
    #[inline]
    fn drop(&mut self) {
        PCI_BLK_HOLDER_CPU[self.slot].store(0, core::sync::atomic::Ordering::Release);
        // inner drops here: unlock + preempt_enable + irq_restore.
    }
}

/// Acquire `PCI_BLK_LOCKS[slot]` with holder forensics.
///
/// Uncontended cost is one CAS (identical to lock_irqsave). When
/// contended, the waiter bounded-spins on try_lock for a moment, prints
/// one rate-limited forensics line naming the current holder (lock-free
/// console — printk may be wedged), then falls into the blocking
/// acquire.
pub(crate) fn pci_blk_lock(slot: usize) -> PciBlkGuard {
    // Same order of magnitude as RawSpinlock's DEADLOCK_WARN_ITERS: the
    // forensics line fires only when the wait has already outlived half
    // the budget AND the lock is still held — pathological contention,
    // not the healthy sub-millisecond bursts a storm produces (observed
    // with a print-at-first-contention version: 50 transient holds in
    // 3 min of dd/sync churn, all held_ms=0).
    const CONTEND_SPIN: u32 = 100_000_000;
    let site = pci_blk_acquire_site();
    let mut i = 0u32;
    while i < CONTEND_SPIN {
        if let Some(g) = PCI_BLK_LOCKS[slot].try_lock_irqsave() {
            return PciBlkGuard::arm(slot, g, site);
        }
        core::hint::spin_loop();
        i += 1;
        if i == CONTEND_SPIN / 2 && PCI_BLK_LOCKS[slot].is_locked() {
            // Half the budget spent and still contended: near-wedge.
            pci_blk_contended_forensics(slot, site);
        }
    }
    PciBlkGuard::arm(slot, PCI_BLK_LOCKS[slot].lock_irqsave(), site)
}

/// Bounded-try acquire for contexts that must NOT spin behind
/// task-context lock holders (the Block softirq walker): returns None
/// after `tries` failed attempts instead of blocking. Arm the same
/// holder forensics on success.
///
/// Rationale (the OH kvm-r1 observation): under a completion storm the
/// softirq walker re-acquires the BLK lock pass after pass while task
/// submitters on the other CPU hammer the same TTAS lock — a waiter can
/// spin for SECONDS without winning the race (PCI-BLK-CONTEND with
/// ctx=2 holders, 159 events in one 7-minute boot; the b125 wedge is
/// the extreme end of that starvation curve). Softirq has no deadline;
/// submitters' waiters do. A softirq that cannot get the lock in a
/// bounded microsecond budget defers (re-raises the Block softirq and
/// returns) — the restart loop / ksoftirqd / the next IRQ each get a
/// fresh chance while task-context holders always win the race.
pub(crate) fn pci_blk_lock_defer(slot: usize, tries: u32) -> Option<PciBlkGuard> {
    let site = pci_blk_acquire_site();
    let mut i = 0u32;
    while i < tries {
        if let Some(g) = PCI_BLK_LOCKS[slot].try_lock_irqsave() {
            return Some(PciBlkGuard::arm(slot, g, site));
        }
        i += 1;
        if i & 63 == 63 {
            core::hint::spin_loop();
        }
    }
    None
}

/// Per-jiffy holder watchdog (called from the Timer softirq via
/// vw_lag_watchdog): any slot whose BLK lock has been continuously held
/// for > 5 s is a wedge in progress — print holder identity, rate-limited
/// to one line per 10 s. printk is safe here (timer-softirq context, not
/// the wedged context).
pub fn pci_blk_holder_watchdog(now: u64) {
    const WEDGE_JIFFIES: u64 = crate::drivers::hotpath::timer::msecs_to_jiffies(5_000);
    static LAST_PRINT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    for slot in 0..MAX_PCI_BLK_DISKS {
        if !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire) {
            continue;
        }
        if !PCI_BLK_LOCKS[slot].is_locked() {
            continue;
        }
        let at = PCI_BLK_HOLDER_AT[slot].load(core::sync::atomic::Ordering::Acquire);
        if at == 0 || now.saturating_sub(at) < WEDGE_JIFFIES {
            continue;
        }
        let last = LAST_PRINT.load(core::sync::atomic::Ordering::Acquire);
        if last != 0 && now.saturating_sub(last) < 1000 {
            return; // rate limit 10 s
        }
        LAST_PRINT.store(now, core::sync::atomic::Ordering::Release);
        crate::pr_err!(
            "PCI-BLK-WEDGE slot={} holder_cpu={} pid={} ra={:#x} ctx={} \
             held_ms={} used={} walked={} expected={} unkicked={} irqs={} \
             walks={} defers={}",
            slot,
            PCI_BLK_HOLDER_CPU[slot]
                .load(core::sync::atomic::Ordering::Acquire)
                .saturating_sub(1),
            PCI_BLK_HOLDER_PID[slot].load(core::sync::atomic::Ordering::Acquire),
            PCI_BLK_HOLDER_RA[slot].load(core::sync::atomic::Ordering::Acquire),
            PCI_BLK_HOLDER_CTX[slot].load(core::sync::atomic::Ordering::Acquire),
            crate::drivers::hotpath::timer::jiffies_to_msecs(now.saturating_sub(at)),
            get_pci_device_queue_at(slot)
                .map(|q| used_ring_idx(q.used_ring_ptr()) as u64)
                .unwrap_or(0),
            PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire) as u64,
            PCI_BLK_EXPECTED_USED_IDX[slot].load(core::sync::atomic::Ordering::Acquire) as u64,
            PCI_BLK_UNKICKED[slot].load(core::sync::atomic::Ordering::Acquire) as u64,
            VW_IRQS[slot].load(core::sync::atomic::Ordering::Acquire),
            VW_WALKS.load(core::sync::atomic::Ordering::Acquire),
            VW_LOCK_DEFERS[slot].load(core::sync::atomic::Ordering::Acquire),
        );
    }
}

/// Per-slot wait queue for PCI VirtIO block I/O completion (interrupt-driven wakeup).
static PCI_BLK_WAIT_QUEUES: [crate::process::types::WaitQueueHead; MAX_PCI_BLK_DISKS] =
    [const { crate::process::types::WaitQueueHead::new() }; MAX_PCI_BLK_DISKS];

/// Wait queue for MMIO VirtIO block I/O completion (interrupt-driven wakeup)
static VIRTIO_BLK_WAIT_QUEUE: crate::process::types::WaitQueueHead =
    crate::process::types::WaitQueueHead::new();

/// Global VirtIO MMIO block device expected used.idx (for tracking I/O completion status)
/// Incremented each time a request is submitted under the queue lock.
/// Each caller reads the value before submit to know which used ring slot to wait for,
/// avoiding the race where two cores read the same queue.get_used() value.
static VIRTIO_MMIO_EXPECTED_USED_IDX: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

/// Maximum number of in-flight async I/O requests per device.
const MAX_PENDING_IO: usize = 16;


// SAFETY: PendingIo is stored in a Spinlock-protected table and only accessed
// from IRQ/softirq context; raw pointers within are not shared across threads.
unsafe impl Send for PendingIo {}

/// Pending async I/O requests for MMIO VirtIO block device.
/// Indexed by (expected_used_idx % MAX_PENDING_IO).
static VIRTIO_MMIO_PENDING: Spinlock<[Option<PendingIo>; MAX_PENDING_IO]> =
    Spinlock::new([const { None }; MAX_PENDING_IO]);

/// Last processed used index for async completions (MMIO).
static VIRTIO_MMIO_LAST_PROCESSED: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

/// Used-ring entries on the MMIO device that matched no live pending
/// (device-truthed walk — see the walker). Loud, rate-limited counter:
/// every hit is a dispatch accounting hole, not a silent misfire.
static VIRTIO_MMIO_UNMATCHED_EVENTS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Get current MMIO expected used index (call before submitting request, under queue lock)
#[inline]
fn get_mmio_expected_used_idx() -> u16 {
    VIRTIO_MMIO_EXPECTED_USED_IDX.load(core::sync::atomic::Ordering::Acquire)
}

/// Increment MMIO expected used index (call after submitting request, under queue lock)
#[inline]
fn increment_mmio_expected_used_idx() {
    VIRTIO_MMIO_EXPECTED_USED_IDX.fetch_add(1, core::sync::atomic::Ordering::Release);
}

/// Publish a NULL-completion reservation ("tombstone") for a synchronous
/// MMIO chain, mirroring the PCI path (`pci_publish_sync_chain`): the
/// sync chains consume a submission ordinal and produce a used-ring entry
/// but have no IoCompletion. Without a tombstone their used entries hit
/// the walker's unmatched accounting (noise), and under the old
/// positional walk they could be misattributed to a live async pending
/// sitting at the same slot index. The tombstone carries the chain's
/// head_desc so the device-truthed walker finds and discards it.
///
/// Published only into a FREE slot: an occupied slot means the walker is
/// lagging behind MAX_PENDING_IO chains — in that case skip (the used
/// entry counts as unmatched, loud but harmless) rather than overwrite a
/// live reservation. Callers hold the MMIO virtqueue lock (mutual
/// exclusion against the walker's collection pass) and have already
/// submitted the chain.
fn mmio_publish_sync_tombstone(ordinal: u16, head_desc: u16) {
    let slot = ordinal as usize % MAX_PENDING_IO;
    let mut table = VIRTIO_MMIO_PENDING.lock_irqsave();
    if table[slot].is_some() {
        return;
    }
    table[slot] = Some(PendingIo {
        completion: core::ptr::null_mut(),
        resp_ptr: core::ptr::null_mut(),
        resp_layout: alloc::alloc::Layout::from_size_align(1, 1).unwrap(),
        header_ptr: core::ptr::null_mut(),
        header_layout: alloc::alloc::Layout::from_size_align(1, 1).unwrap(),
        head_desc: head_desc as u32,
        ordinal,
        timed_out: false,
        data_ptr: 0,
        is_write: false,
        trace_dev: 0,
        trace_sector: 0,
        trace_nr_sector: 0,
        n_descs: 3,
    });
}

/// Per-slot expected used.idx (for tracking I/O completion status).
/// Incremented each time a request is submitted on that slot's queue
/// (under the slot's BLK lock); each submitter reads the value before
/// submit to know which used-ring slot to wait for.
static PCI_BLK_EXPECTED_USED_IDX: [core::sync::atomic::AtomicU16; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU16::new(0) }; MAX_PCI_BLK_DISKS];

// ============================================================================
// LOCK ORDER (virtio-blk ABBA fix — the GNOME final6 deadlock family)
//
// INVARIANT VIRTIO-WQ-1: a wait-queue lock (WaitQueueHead's internal
// Spinlock<Vec<WaitQueueEntry>>, taken by wake_up_all / prepare_to_wait /
// finish_wait / add / remove) must NEVER be acquired while holding a virtio
// driver lock (VIRTIO_PCI_BLK_LOCK, the MMIO device's `virtqueue` lock, or a
// VIRTIO_*_PENDING table lock).
//
// Why: WaitQueueHead::wake_up() holds the wait-queue lock across
// sched::wake_up_process() (→ GRQ lock, per-waiter, slow under TCG), and
// IoCompletion::wait() waiters run prepare_to_wait() on their OWN stack
// completion (fill_page_cache_batch keeps [IoCompletion; 128] there) while
// other CPUs submit block I/O. The old completion walker ran
// `(*pending.completion).complete()` (→ wake_up_all → wait-queue lock) while
// STILL HOLDING VIRTIO_PCI_BLK_LOCK with IRQs off; every submitter then piled
// onto VIRTIO_PCI_BLK_LOCK (deadlock watchdog: cpu1/cpu3 stuck on
// VIRTIO_PCI_BLK_LOCK @BSS, cpu0 stuck on a heap wait-queue lock inside
// `Spinlock<Vec<WaitQueueEntry>>` lock_irqsave — fire-gnome-final6
// attempt2/k7542fc3 serial logs) and timer IRQs stopped system-wide.
//
// The fix is the timer.rs R12-3 pattern: completion DELIVERY (dealloc +
// IoCompletion::complete + wake_up_all) happens strictly OUTSIDE the virtio
// locks. The walkers collect finished PendingIo entries under the locks, drop
// the locks, then deliver. `VirtioLockNest` below arms a per-CPU counter so
// debug builds assert the invariant at every wake entry point
// (IoCompletion::complete and the two sync-queue wake_up_all sites).
// ============================================================================

/// Per-CPU depth of held virtio driver locks (debug nesting guard for
/// INVARIANT VIRTIO-WQ-1). 0 on release builds.
#[cfg(debug_assertions)]
static VIRTIO_LOCK_DEPTH: [core::sync::atomic::AtomicUsize; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; crate::config::MAX_CPUS];

/// True while the current CPU holds any virtio driver lock (debug only).
#[cfg(debug_assertions)]
pub fn virtio_lock_held() -> bool {
    let cpu = crate::arch::smp::cpu_id() as usize;
    VIRTIO_LOCK_DEPTH[cpu.min(crate::config::MAX_CPUS - 1)]
        .load(core::sync::atomic::Ordering::Acquire)
        > 0
}


#[cfg(debug_assertions)]
impl VirtioLockNest {
    #[inline]
    pub(crate) fn new() -> Self {
        let cpu = (crate::arch::smp::cpu_id() as usize).min(crate::config::MAX_CPUS - 1);
        VIRTIO_LOCK_DEPTH[cpu].fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        Self
    }
}

#[cfg(debug_assertions)]
impl core::ops::Drop for VirtioLockNest {
    #[inline]
    fn drop(&mut self) {
        let cpu = (crate::arch::smp::cpu_id() as usize).min(crate::config::MAX_CPUS - 1);
        VIRTIO_LOCK_DEPTH[cpu].fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
    }
}


#[cfg(not(debug_assertions))]
impl VirtioLockNest {
    #[inline]
    pub(crate) fn new() -> Self { Self }
}

/// Panic-at-the-violation-point check for INVARIANT VIRTIO-WQ-1 (debug
/// builds only): callers must not be inside a virtio driver-lock critical
/// section when they reach a wait-queue wake.
#[cfg(debug_assertions)]
pub fn assert_no_virtio_lock(what: &str) {
    if virtio_lock_held() {
        panic!(
            "virtio: {} called with a virtio driver lock held — \
             VIRTIO-WQ-1 lock-order violation (wait-queue wake inside the \
             virtio lock; see drivers/virtio/mod.rs)",
            what
        );
    }
}

// ============================================================================
// PCI VirtIO-Blk async read support
//
// The PCI device is the boot/root disk on QEMU virt (-device virtio-blk-pci),
// but only the MMIO device ever registered an `async_read_fn`. bio::bread_async
// therefore returned ENXIO on the root disk and the ext4 read-ahead path was
// silently dead there: every page-cache miss paid a fully synchronous
// submit/sleep/wake virtio round trip (~1.8 ms under TCG), capping sequential
// file reads at ~2 MB/s. This table + submit function give the PCI disk the
// same fire-and-forget read path the MMIO device already had.
// ============================================================================

/// Pending async I/O slots for the PCI VirtIO block device.
/// Must exceed the maximum in-flight chains (queue_size / 3 descriptors);
/// QEMU's virtio-blk-pci default queue size is 128 → 41 chains max.
const MAX_PENDING_IO_PCI: usize = 64;

/// Upper bound on data segments in one SG chain. Well under QEMU's
/// seg_max (queue_size - 2; 254 on a 256-deep queue) and keeps a fused
/// chain's descriptor footprint (N+2) modest so the descs_held budget
/// rarely binds. Callers split larger groups.
pub const MAX_SG_SEGS_PER_CHAIN: usize = 16;


/// Pending async I/O requests for the PCI VirtIO block device.
///
/// ORDINAL-SLOT DISPATCH + DEVICE-TRUTHED MATCHING: every chain — async
/// read, synchronous read/write/flush — takes one submission ordinal from
/// VIRTIO_PCI_EXPECTED_USED_IDX (under the BLK lock) and publishes at
/// `ordinal % MAX_PENDING_IO_PCI`. The completion walker tries that
/// positional mapping first (used-ring entry i → slot i % 64, exact when
/// the device completes in submission order) and falls back to matching
/// the used-ring entry's actual `UsedElem.id` against each entry's
/// `head_desc`. The fallback is what makes reordering harmless:
///
/// The pure ordinal design (used-ring index == submission ordinal, slot
/// i for entry i) only holds when the device completes chains exactly in
/// submission order. QEMU virtio-blk completes mixed-size chains OUT OF
/// ORDER (a 4 KiB single-block read routinely overtakes an in-flight
/// 256 KiB coalesced readahead chain — counted live in
/// VIRTIO_PCI_REORDER_EVENTS), and the positional-only walker then fired
/// the WRONG pending on every reordering:
///   - a pending whose chain was still in flight fired EARLY with a
///     fabricated -EIO (its response byte still 0xFF); the waiter returned,
///     the caller freed the DMA target (`bfree_multi`) and could exit while
///     the device still wrote into it, and the late used-ring entry then
///     fired yet another innocent pending — a cascade (the wandering heap
///     corruption behind the r1/r3/r4 families and AC-2's 263 mis-matched
///     multi-block-read EIOs), and
///   - when the walker lagged >= 64 entries (Block-softirq starvation), the
///     submit path's stale-slot takeover fired still-waiting completions
///     with a fabricated -EIO, and any waiter that left via the 10s
///     deadline left a pending pointing at its kernel-stack IoCompletion —
///     the walker later fired it into the freed (recycled-to-userspace)
///     stack page: the r2 WAKE-WILD-PTR panic.
/// Matching the device's own UsedElem.id makes completion order
/// irrelevant: a pending fires exactly when ITS chain's used-ring entry
/// appears, never earlier. UNIQUENESS of head_desc among live entries is
/// enforced at publish time (pci_pending_slot_reservable's window check):
/// the free-running descriptor allocator would otherwise recycle a head
/// id every queue_size descriptors (~43 chains on a 128-desc queue) while
/// its previous chain was still in flight, and two live pendings sharing
/// head_desc would make a used-ring entry ambiguous again.
///
/// Synchronous chains publish NULL-completion reservations ("tombstones")
/// at their own ordinal slot so their windows and slots are equally
/// protected and their used-ring entries cannot be misattributed to a
/// later async pending; their waiter polls its own response byte and owns
/// the io_buf. A timing-out waiter flags its tombstone and converts async
/// entries into tombstones via abandon_pending_completion, so a late
/// completion can never touch memory its owner has stopped owning.
///
/// LIVENESS: a still-occupied slot or overlapping window is walker lag,
/// never fatal — submitters drain + retry (see pci_submit_data_async), and
/// because ordinal N+64 cannot publish until entry N was walked, the
/// walker can never trail far enough for the used ring (queue_size deep)
/// to overwrite an unconsumed entry.
static PCI_BLK_PENDING: [Spinlock<[Option<PendingIo>; MAX_PENDING_IO_PCI]>; MAX_PCI_BLK_DISKS] =
    [const { Spinlock::new([const { None }; MAX_PENDING_IO_PCI]) }; MAX_PCI_BLK_DISKS];

/// Last used-ring index processed by the async completion walker, per slot.
static PCI_BLK_PENDING_LAST: [core::sync::atomic::AtomicU16; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU16::new(0) }; MAX_PCI_BLK_DISKS];

/// Chains published to a slot's avail ring since the last device kick.
/// Batch submitters publish quietly and the waiter kicks once — see
/// pci_submit_data_async.
static PCI_BLK_UNKICKED: [core::sync::atomic::AtomicUsize; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; MAX_PCI_BLK_DISKS];

/// Kick the PCI virtio-blk disk in `slot` if any quietly-submitted chains
/// are pending. Callers must invoke this BEFORE sleeping on an async
/// completion (drain paths); it is idempotent and cheap when nothing is
/// unked.
pub fn pci_blk_kick(slot: usize) {
    if slot >= MAX_PCI_BLK_DISKS {
        return;
    }
    if PCI_BLK_UNKICKED[slot].swap(0, core::sync::atomic::Ordering::AcqRel) > 0 {
        if let Some(q) = get_pci_device_queue_at(slot) {
            q.notify();
        }
    }
}

/// Kick EVERY PCI virtio-blk disk with quietly-submitted chains. For
/// disk-agnostic recovery paths (batch drains, lost-completion
/// compensation) where the owning disk is not known; notify() on an idle
/// queue is a harmless MMIO write.
pub fn pci_blk_kick_all() {
    for slot in 0..MAX_PCI_BLK_DISKS {
        if PCI_BLK_UNKICKED[slot].swap(0, core::sync::atomic::Ordering::AcqRel) > 0 {
            if let Some(q) = get_pci_device_queue_at(slot) {
                q.notify();
            }
        }
    }
}

/// True if the descriptor windows of two chains share any descriptor.
/// Chain A occupies [a, a+la) and chain B [b, b+lb), both mod q. The
/// classic chain is la=lb=3 (header/data/resp); an SG chain with N data
/// segments holds N+2. Ring wrap-safe: distance is taken mod q in both
/// directions.
fn chain_windows_overlap_len(a: u32, la: u32, b: u32, lb: u32, q: u32) -> bool {
    if q == 0 {
        return false;
    }
    let d = (a as i64 - b as i64).rem_euclid(q as i64);
    d < la as i64 || (q as i64 - d) < lb as i64
}

/// True if the 3-descriptor windows of two chains (heads `a` and `b` on a
/// queue of `q` descriptors) share any descriptor. Chains always occupy
/// [head, head+1, head+2] consecutively (header/data/resp).
fn chain_windows_overlap(a: u32, b: u32, q: u32) -> bool {
    chain_windows_overlap_len(a, 3, b, 3, q)
}

/// Check that a chain CAN be published to the pending table: its ordinal
/// slot must be free and its descriptor window [head, head+2] must not
/// overlap any live entry's window (async pendings, synchronous-chain
/// tombstones, and timeout tombstones alike).
///
/// ORDINAL-SLOT DISPATCH (the 7b7e847 skeleton): every chain — async or
/// synchronous — takes one submission ordinal from
/// VIRTIO_PCI_EXPECTED_USED_IDX and publishes at `ordinal %
/// MAX_PENDING_IO_PCI`. A still-occupied slot means the walker has not
/// consumed the entry from 64 ordinals ago; a window overlap means the
/// free-running descriptor allocator (id mod queue_size, recycled every
/// ~queue_size/3 chains) has wrapped onto a live entry. BOTH are the same
/// condition — the completion walker is lagging — and neither is fatal:
/// unlike the previous design, which BURNED colliding windows and could
/// exhaust the whole descriptor ring into a permanent -EIO, the caller
/// treats "not reservable" as a RETRYABLE error: drop the (BLK) lock,
/// kick + drain completions, and try again. Draining retires finished
/// entries, freeing slots and windows; the steady state (walker keeping
/// up) never hits this path at all.
///
/// Head-descriptor uniqueness among live entries is the invariant the
/// walker's device-truthed matching rests on: a used-ring entry carries
/// only the chain's head id, and with recycled-but-live duplicates the
/// first match could fire the WRONG pending (premature -EIO with the DMA
/// target freed under the device — the AC-2 corruption family).
///
/// Caller must hold the PCI BLK lock (the walker collects under the same
/// lock, so the reservation cannot race it).
pub fn pci_pending_slot_reservable(disk: usize, queue_size: u32, ordinal: u16, head_desc: u16) -> bool {
    pci_pending_slot_reservable_len(disk, queue_size, ordinal, head_desc, 3)
}

/// Length-aware reservation check (SG chains): the candidate occupies
/// [head_desc, head_desc+n_descs) mod queue_size; every live entry's own
/// window length is honored.
pub fn pci_pending_slot_reservable_len(
    disk: usize,
    queue_size: u32,
    ordinal: u16,
    head_desc: u16,
    n_descs: u16,
) -> bool {
    if disk >= MAX_PCI_BLK_DISKS {
        return false;
    }
    let slot = ordinal as usize % MAX_PENDING_IO_PCI;
    // SUBMIT-2X: the caller holds the BLK lock, which serializes every
    // pending-table access (submit publish, walker collect, sync
    // tombstones, abandon under BLK) — the table's own irqsave lock was
    // a redundant second acquisition inside that window (one of the 5
    // per-submit acquisitions the dio-conc open list named).
    // SAFETY: BLK lock held by the caller on this disk.
    let table = unsafe { PCI_BLK_PENDING[disk].get_ref() };
    if table[slot].is_some() {
        return false;
    }
    let n = if n_descs == 0 { 3 } else { n_descs } as u32;
    !table.iter().any(|e| match e {
        Some(p) => chain_windows_overlap_len(
            p.head_desc,
            p.n_descs.max(1) as u32,
            head_desc as u32,
            n,
            queue_size,
        ),
        None => false,
    })
}

/// Publish a NULL-completion reservation ("tombstone") for a synchronous
/// PCI chain (read_block_once / write_block_once / flush_block_once), at
/// the chain's own ordinal slot. Keeps the chain's descriptor window
/// reserved until the completion walker consumes its used-ring entry, and
/// ensures the chain's eventual used-ring entry is matched (and discarded)
/// instead of being misattributed to a later async pending that recycled
/// the head descriptor id. Their waiter polls its own response byte and
/// owns the io_buf — the walker must never fire or free anything for them.
///
/// Caller must hold the PCI BLK lock, have passed
/// pci_pending_slot_reservable for this (ordinal, head_desc) under the
/// same lock hold, and have already submitted the chain. Between the
/// reservation check and this publish only abandon_pending_completion can
/// touch the table, and it never frees a slot — so the publish cannot
/// fail; the debug assert documents that invariant.
pub fn pci_publish_sync_chain(disk: usize, queue_size: u32, ordinal: u16, head_desc: u16) {
    if disk >= MAX_PCI_BLK_DISKS {
        return;
    }
    let slot = ordinal as usize % MAX_PENDING_IO_PCI;
    // SUBMIT-2X: BLK lock held by the caller serializes the table —
    // direct access, no second lock acquisition inside the window.
    // SAFETY: BLK lock held by the caller on this disk.
    let table = unsafe { PCI_BLK_PENDING[disk].get_mut_unchecked() };
    debug_assert!(
        table[slot].is_none()
            || !table.iter().any(|e| match e {
                Some(p) => chain_windows_overlap(p.head_desc, head_desc as u32, queue_size),
                None => false,
            }),
        "virtio: sync-chain publish lost its reserved slot"
    );
    table[slot] = Some(PendingIo {
        completion: core::ptr::null_mut(),
        resp_ptr: core::ptr::null_mut(),
        resp_layout: alloc::alloc::Layout::from_size_align(1, 1).unwrap(),
        header_ptr: core::ptr::null_mut(),
        header_layout: alloc::alloc::Layout::from_size_align(1, 1).unwrap(),
        head_desc: head_desc as u32,
        ordinal,
        timed_out: false,
        data_ptr: 0,
        is_write: false,
        trace_dev: 0,
        trace_sector: 0,
        trace_nr_sector: 0,
        n_descs: 3,
    });
    // Sync chains hold their 3 descriptors until the walker consumes this
    // tombstone (pairs with the walker's release_descs).
    if let Some(vq) = get_pci_device_queue_at(disk) {
        vq.hold_descs(3);
    }
}

/// Flag a synchronous chain's tombstone as timed out.
///
/// Called by read_block_once / write_block_once / flush_block_once when
/// their 10s deadline expires (after note_timed_out_chain bumped the
/// queue's leaked counter): when the walker later consumes this
/// tombstone's used-ring entry it will pair the increment with
/// resolve_leaked_chain instead of letting leaked_chains accumulate
/// forever.
pub fn pci_flag_sync_tombstone_timed_out(disk: usize, ordinal: u16) {
    if disk >= MAX_PCI_BLK_DISKS {
        return;
    }
    let slot = ordinal as usize % MAX_PENDING_IO_PCI;
    // SUBMIT-2X: caller holds the BLK lock (see pci_publish_sync_chain).
    // SAFETY: BLK lock held by the caller on this disk.
    let table = unsafe { PCI_BLK_PENDING[disk].get_mut_unchecked() };
    if let Some(p) = table[slot].as_mut() {
        if p.completion.is_null() {
            p.timed_out = true;
        }
    }
}

/// Abandon in-flight async I/O bound to one IoCompletion pointer.
///
/// GSD fix (10s-deadline UAF): `IoCompletion::wait` bounds itself at 10s
/// and returns -ETIMEDOUT to escape wedged I/O. That waiter's
/// `IoCompletion` usually lives on its KERNEL STACK
/// (fill_page_cache_batch), so once wait() returns, the stack frame —
/// and the completion with it — is gone. But the pending tables kept the
/// raw pointer, and every later completion walker
/// (`pci_process_async_completions`, the MMIO IRQ path) would have
/// called `complete()` through it: a wake_up_all() over a wait queue
/// that no longer exists, corrupting whatever now owns that stack. The
/// resulting wild `wake_up_process` on a garbage Task pointer was the
/// recurring `KERNPANIC pfault badaddr=0x5e` under gnome-session load.
///
/// Matching entries are converted into TOMBSTONES (null completion, null
/// io_buf pointers, head_desc kept) instead of being removed: nothing
/// dereferences the abandoning waiter's memory after it unwinds, but the
/// chain's descriptor window stays reserved until the walker consumes
/// its used-ring entry — otherwise the free-running allocator could hand
/// the head id to a NEW chain, and the abandoned chain's late used-ring
/// entry would fire that new pending prematurely. The tombstone is
/// flagged timed_out and the queue's leaked counter is bumped
/// (note_timed_out_chain): the admission guard stops counting the chain
/// as in-flight, and when the walker finally consumes the tombstone the
/// decrement pairs again (resolve_leaked_chain). The device may still
/// finish the abandoned chain and DMA into the request buffer — the
/// waiter must therefore NOT free its DMA buffers on the WAIT_TIMED_OUT
/// path (see fill_page_cache_batch / bread_wait callers).
///
/// The chain's header/resp io_buf is deliberately LEAKED, not freed: the
/// device writes the response byte (inside that block) exactly when the
/// chain completes — possibly long after this abandon — so freeing it
/// would hand a late 1-byte DMA a freed heap block (R21-N2 discipline:
/// integrity over a bounded leak; the window reservation additionally
/// guarantees no descriptor of the chain is reused before that moment).
///
/// Returns the number of entries abandoned.
pub fn abandon_pending_completion(
    comp: *mut crate::fs::io_completion::IoCompletion,
) -> usize {
    let mut abandoned = 0usize;
    // Per-disk tally so the admission-guard decrements land on the OWNING
    // queue (note_timed_out_chain pairs with resolve_leaked_chain there).
    let mut abandoned_per_disk = [0usize; MAX_PCI_BLK_DISKS];

    // PCI tables: tombstone (window stays reserved; nothing left to fire).
    // The completion may be in flight on ANY disk — sweep every slot.
    // SUBMIT-2X: each disk's table is now serialized by that disk's BLK
    // lock (the hot paths access it lock-free under BLK), so the sweep
    // takes the BLK lock per disk, strictly sequentially (never two at
    // once). Timeout context: no virtio lock is held on entry.
    for disk in 0..MAX_PCI_BLK_DISKS {
        let _blk = pci_blk_lock(disk);
        // SAFETY: BLK lock held on this disk; abandon only tombstones.
        let table = unsafe { PCI_BLK_PENDING[disk].get_mut_unchecked() };
        for slot in table.iter_mut() {
            if let Some(p) = slot {
                if p.completion == comp && !p.completion.is_null() {
                    abandoned_per_disk[disk] += 1;
                    *slot = Some(PendingIo {
                        completion: core::ptr::null_mut(),
                        resp_ptr: core::ptr::null_mut(),
                        resp_layout: p.resp_layout,
                        header_ptr: core::ptr::null_mut(),
                        header_layout: p.header_layout,
                        head_desc: p.head_desc,
                        ordinal: p.ordinal,
                        timed_out: true,
                        data_ptr: 0,
                        is_write: false,
                        trace_dev: 0,
                        trace_sector: 0,
                        trace_nr_sector: 0,
                        n_descs: p.n_descs,
                    });
                    // io_buf intentionally leaked (see above).
                    abandoned += 1;
                }
            }
        }
    }
    if abandoned > 0 {
        // Discount the abandoned chains from the in-flight admission
        // guard: their used-ring entries may land arbitrarily late (or
        // never). The walker pairs this with resolve_leaked_chain when
        // the tombstones' entries are consumed. No next_desc skipping:
        // the tombstones' window reservations already keep every
        // descriptor of these chains out of new allocations' reach, and
        // skipping from process context could interleave with a
        // concurrent chain build under the BLK lock (its three
        // descriptors must stay consecutive for the window math).
        // Multi-disk: bump each disk's queue for the chains abandoned from
        // ITS table (tallied per disk above).
        for disk in 0..MAX_PCI_BLK_DISKS {
            let n = abandoned_per_disk[disk];
            if n == 0 {
                continue;
            }
            if let Some(vq) = get_pci_device_queue_at(disk) {
                for _ in 0..n {
                    vq.note_timed_out_chain();
                }
            }
        }
    }
    // MMIO table: same tombstone discipline (ordinal-indexed table).
    {
        let mut table = VIRTIO_MMIO_PENDING.lock_irqsave();
        for slot in table.iter_mut() {
            if let Some(p) = slot {
                if p.completion == comp && !p.completion.is_null() {
                    *slot = Some(PendingIo {
                        completion: core::ptr::null_mut(),
                        resp_ptr: core::ptr::null_mut(),
                        resp_layout: p.resp_layout,
                        header_ptr: core::ptr::null_mut(),
                        header_layout: p.header_layout,
                        head_desc: p.head_desc,
                        ordinal: p.ordinal,
                        timed_out: false,
                        data_ptr: 0,
                        is_write: false,
                        trace_dev: 0,
                        trace_sector: 0,
                        trace_nr_sector: 0,
                        n_descs: p.n_descs,
                    });
                    // header + resp intentionally leaked (same discipline).
                    abandoned += 1;
                }
            }
        }
    }
    if abandoned > 0 {
        crate::pr_err!(
            "virtio: abandoned {} in-flight I/O(s) of a timed-out waiter",
            abandoned
        );
    }
    // Wait out any walker delivery of THIS completion that is still in
    // flight (the entry was taken from a table before our scan, so the
    // tombstone pass above could not retire it — see IoCompletion's
    // `delivering` protocol). The waiter returns as soon as we do, and its
    // IoCompletion usually lives on its kernel stack: without this
    // handoff, a late complete() fired into the recycled stack page was
    // the wandering corruption behind ftest01's cross-file bad-verify.
    if !unsafe { (*comp).wait_deliveries() } {
        crate::pr_err!(
            "virtio: delivery handoff overran spin budget during abandon"
        );
    }
    abandoned
}

/// Submit an async data chain (read or write per `is_write`) on the PCI
/// VirtIO block device (no waiting). Single-segment form; see
/// `pci_submit_data_async_sg` for the scatter-gather form and the shared
/// design notes.
///
/// The DMA-stability contract (`data_ptr` valid until the completion
/// fires, unmodified for writes) is the registered-fn contract documented
/// at the blkdev edge (INV-BLKDEV-2) and on the segment form below.
fn pci_submit_data_async(
    disk: &super::types::GenDisk,
    sector: u64,
    data_ptr: *mut u8,
    data_len: usize,
    is_write: bool,
    completion: &crate::fs::io_completion::IoCompletion,
) -> Result<(), i32> {
    pci_submit_data_async_sg(
        disk,
        sector,
        &[crate::drivers::types::DmaSeg { ptr: data_ptr, len: data_len }],
        is_write,
        completion,
    )
}

/// Submit an async SCATTER-GATHER data chain on the PCI VirtIO block
/// device (no waiting) — the virtio-blk driver's `blk_rq_map_sg` shape:
/// ONE request header (one starting sector), N data descriptors forming
/// the request's scatter list, one response descriptor. The segments must
/// cover ONE device-contiguous sector range [sector, sector + total/512);
/// their MEMORY may be discontiguous (physically-separate buffers). This
/// is the blk-merge.c front/back-merge endpoint: a 1 MiB read fused from
/// four 256 KiB spans becomes one descriptor chain and one device round
/// trip instead of four serialized ones.
///
/// The direction parameterizes exactly three things: the request type
/// (T_IN/T_OUT), the data descriptors' F_WRITE bit (set for reads — the
/// device writes the buffers; clear for writes — the device READS the
/// buffers), and the pending's is_write trace bit. Everything else is
/// direction-neutral.
///
/// Walker compatibility is unchanged: an SG chain is still ONE chain —
/// one head descriptor, one submission ordinal, one pending-table entry,
/// one used-ring entry matched by head id. `n_descs` (N+2) is carried in
/// the pending so the window-overlap check and the descs_held budget see
/// the chain's real footprint.
///
/// The segments' DMA-stability contract (valid until the completion
/// fires, unmodified for writes) rides INV-BLKDEV-2 from the blkdev
/// registration edge.
fn pci_submit_data_async_sg(
    disk: &super::types::GenDisk,
    sector: u64,
    segs: &[super::types::DmaSeg],
    is_write: bool,
    completion: &crate::fs::io_completion::IoCompletion,
) -> Result<(), i32> {


    // Route to the disk's own slot: register_pci_gen_disk stashes the
    // slot index in GenDisk.private_data.
    let slot = disk.private_data.map(|p| p as usize).unwrap_or(0);
    if slot >= MAX_PCI_BLK_DISKS
        || !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire)
    {
        return Err(-5); // EIO
    }
    if segs.is_empty() {
        return Err(-22); // EINVAL — caller must cap group size
    }
    {
        // Chain footprint vs the negotiated ring depth (header + data
        // + response). MAX_SG_SEGS_PER_CHAIN is the fusion lanes'
        // conservative default, not the device limit.
        let (seg_max, _) = pci_blk_sg_limits(slot);
        if segs.len() > seg_max {
            return Err(-22); // caller must split
        }
    }
    let mut data_len_total: usize = 0;
    for s in segs {
        if s.len == 0 || s.len % 512 != 0 {
            return Err(-22); // segments must be whole sectors
        }
        data_len_total = data_len_total.saturating_add(s.len);
    }

    // One 64-byte block carries header (offset 0) + response (offset 48) —
    // the same R17-C combined allocation the sync path uses. SUBMIT-2X:
    // popped from the per-CPU io_buf pool (no global-allocator lock on
    // the submit path).
    let io_buf = io_buf_alloc();
    if io_buf.is_null() {
        return Err(-12); // ENOMEM
    }
    let io_layout = alloc::alloc::Layout::from_size_align(64, 16).unwrap();
    // INV-SAFECALL: typed split+init of the pooled R17-C block.
    let (header_ptr, resp_ptr) = io_buf_init(
        io_buf,
        if is_write {
            req_type::VIRTIO_BLK_T_OUT
        } else {
            req_type::VIRTIO_BLK_T_IN
        },
        sector,
    );

    const VIRTQ_DESC_F_NEXT: u16 = 1;
    const VIRTQ_DESC_F_WRITE: u16 = 2;

    let header_phys = crate::arch::mm::virt_to_phys(
        crate::arch::mm::VirtAddr::new(header_ptr as u64),
    ).0;
    let resp_phys = crate::arch::mm::virt_to_phys(
        crate::arch::mm::VirtAddr::new(resp_ptr as u64),
    ).0;

    // Submission attempts. "Walker lag" (ordinal slot still occupied or
    // descriptor window overlapping a live entry) and a full in-flight
    // guard are both SOFT conditions: the completion walker retires
    // finished entries as soon as it runs, so the right response is to
    // drop every lock, kick + drain once, and try again — NOT to burn
    // descriptor windows or fail outright (the old design converted
    // Block-softirq starvation into permanent submission failure, and
    // the sync-read fallback storm that followed piled waiters onto the
    // sync wait queue). The steady state never enters the retry path.
    let mut submitted = false;
    // Safety kick deferred OUT of the BLK critical section: notify() is
    // an MMIO write (VM exit under KVM) and must never run under the
    // irqsave lock (the b125 wedge hunt — see PCI_BLK_LOCKS forensics).
    let mut kick_after_release = false;
    for _attempt in 0..3 {
        let mut lagging = false;
        {
            // The BLK lock is held across submit + pending-store so the
            // completion walker (Block softirq, same lock) can never
            // observe the used-ring advance before the pending entry is
            // published — the lost-completion race documented on the MMIO
            // path.
            let _guard = pci_blk_lock(slot);
            let _nest = VirtioLockNest::new();

            let virt_queue = match get_pci_device_queue_mut_at(slot, &_guard) {
                Some(q) => q,
                None => {
                    // io_buf is unreferenced; no chain was built for it.
                    io_buf_free(io_buf);
                    return Err(-5);
                }
            };
            let q = virt_queue.queue_size as u32;
            let n_descs = (segs.len() + 2) as u16;
            if q < n_descs as u32 {
                // Queue too small for this chain (never on QEMU's 256-deep
                // blk queues); caller falls back to per-span chains.
                io_buf_free(io_buf);
                return Err(-22);
            }

            // The whole chain occupies [head, head+n_descs) — one
            // free-running reservation (consecutive by construction under
            // the BLK lock).
            let header_desc_idx = match virt_queue.alloc_desc_run(n_descs) {
                Some(h) => h,
                None => {
                    // In-flight/held guard exhausted. Draining completions
                    // advances the used ring and lowers the guard, so
                    // retry after the drain below; the burned descriptor
                    // ids are harmless (free-running counter, nothing
                    // submitted through them).
                    lagging = true;
                    0
                }
            };

            if !lagging {
                // Ordinal slot BEFORE submit: every chain — async or
                // synchronous — takes one submission ordinal and
                // publishes at `ordinal % MAX_PENDING_IO_PCI`; the
                // walker's positional fast path expects it there.
                let prev_expected = get_expected_used_idx_at(slot);
                if !pci_pending_slot_reservable_len(
                    slot, q, prev_expected, header_desc_idx, n_descs,
                ) {
                    // Walker lag: retry after drain below.
                    lagging = true;
                } else {
                    virt_queue.set_desc(header_desc_idx, header_phys,
                        core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
                        VIRTQ_DESC_F_NEXT, (header_desc_idx + 1) % virt_queue.queue_size);
                    // Data descriptors: one per segment. Direction: a
                    // WRITE's data descriptors have NO VIRTQ_DESC_F_WRITE —
                    // the device READS the buffers (they are the DMA
                    // source; the write-batch contract keeps them stable
                    // until completion).
                    for (i, seg) in segs.iter().enumerate() {
                        let di = (header_desc_idx + 1 + i as u16) % virt_queue.queue_size;
                        let next = (di + 1) % virt_queue.queue_size;
                        let seg_phys = crate::arch::mm::virt_to_phys(
                            crate::arch::mm::VirtAddr::new(seg.ptr as u64),
                        ).0;
                        virt_queue.set_desc(di, seg_phys, seg.len as u32,
                            if is_write { VIRTQ_DESC_F_NEXT } else { VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT },
                            next);
                    }
                    let resp_desc_idx = (header_desc_idx + 1 + segs.len() as u16) % virt_queue.queue_size;
                    virt_queue.set_desc(resp_desc_idx, resp_phys,
                        core::mem::size_of::<VirtIOBlkResp>() as u32,
                        VIRTQ_DESC_F_WRITE, 0);

                    // Batch submission discipline: publish quietly and
                    // count. Every notify is an MMIO trap (device
                    // emulation under TCG, ~hundreds of µs), so a
                    // 128-block read-ahead window kicks once
                    // (pci_blk_kick from the waiter) instead of 128
                    // times. The bound below is a safety kick: past it
                    // the virtqueue is nearly exhausted anyway and a
                    // lost waiter must never depend on someone else's
                    // kick. Deferred to after the lock release — the
                    // MMIO write never runs under the BLK lock.
                    virt_queue.submit_quiet(header_desc_idx);
                    // Keep the submission counter aligned with the used
                    // ring: the walker's positional fast path maps
                    // used-ring entry i to slot i % MAX_PENDING_IO_PCI.
                    increment_expected_used_idx_at(slot);
                    let unkicked = PCI_BLK_UNKICKED[slot].fetch_add(1, core::sync::atomic::Ordering::AcqRel) + 1;
                    if unkicked >= 32 {
                        PCI_BLK_UNKICKED[slot].store(0, core::sync::atomic::Ordering::Release);
                        kick_after_release = true;
                    }

                    // Publish the pending entry at its ordinal slot,
                    // carrying the chain's head descriptor id for
                    // device-truthed matching. The BLK lock is still
                    // held, so the completion walker (same lock) cannot
                    // observe the used-ring advance before this entry is
                    // visible — no matter how fast the device completes.
                    // SUBMIT-2X: that same BLK lock serializes every
                    // pending-table access, so the store itself needs no
                    // second table lock (was one of the 5 per-submit
                    // acquisitions).
                    // SAFETY: BLK lock held (the _guard above).
                    let table = unsafe { PCI_BLK_PENDING[slot].get_mut_unchecked() };
                    table[prev_expected as usize % MAX_PENDING_IO_PCI] = Some(PendingIo {
                        completion: completion as *const _ as *mut _,
                        resp_ptr: resp_ptr as *mut u8,
                        resp_layout: io_layout,
                        header_ptr: header_ptr as *mut u8,
                        header_layout: io_layout,
                        head_desc: header_desc_idx as u32,
                        ordinal: prev_expected,
                        timed_out: false,
                        // Zero-serve guard: the walker stamps this into the
                        // delivered completion; bread_wait verifies it. The
                        // FIRST segment's pointer is the group's token (the
                        // caller's expect-value is the fused range's first
                        // buffer).
                        data_ptr: segs[0].ptr as usize,
                        is_write,
                        // block:block_rq_complete geometry for the walker.
                        trace_dev: (((*disk).major << 20) | (*disk).first_minor) as u32,
                        trace_sector: sector,
                        trace_nr_sector: (data_len_total / 512) as u32,
                        n_descs,
                    });
                    // SG budget: the chain holds n_descs descriptors until
                    // the walker consumes its used-ring entry.
                    virt_queue.hold_descs(n_descs);

                    submitted = true;
                }
            }
        }

        if submitted {
            // Safety kick outside every virtio lock (see above).
            if kick_after_release {
                if let Some(q) = get_pci_device_queue_at(slot) {
                    q.notify();
                }
            }
            return Ok(());
        }
        if !lagging {
            break;
        }
        // Outside every virtio lock: kick the device and run the walker
        // once so finished entries retire and slots/windows free up.
        pci_blk_kick(slot);
        pci_process_async_completions_slot(slot);
    }

    // Still lagging after the retries — fail; the caller (bread_async)
    // falls back to a synchronous read, which is correct if slower.
    // No chain was ever built from io_buf (every attempt bailed before
    // submit), so it is dead from the device's point of view.
    io_buf_free(io_buf);
    Err(-5)
}

/// Static wrapper matching GenDisk's `async_read_fn` signature for the PCI
/// disk (INV-BLKDEV-2: called only for the registered PCI GenDisk; `buf`
/// stays a valid DMA target until the completion fires).
fn pci_async_read_fn(
    disk: &super::types::GenDisk,
    sector: u64,
    buf: &mut [u8],
    completion: &crate::fs::io_completion::IoCompletion,
) -> i32 {
    match pci_submit_data_async(disk, sector, buf.as_mut_ptr(), buf.len(), false, completion) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Submit an async write on the PCI VirtIO block device (no waiting) — the
/// plugged-write primitive's device side. Mirrors the async read end to end
/// (quiet submit, ordinal slot + descriptor-window reservation, pending
/// publish under the BLK lock); the only direction-specific deltas are in
/// `pci_submit_data_async` (T_OUT header, no F_WRITE on the data descriptor).
///
/// `buf` is the DMA SOURCE: it must remain valid and unmodified until the
/// completion fires. Callers that cannot promise that submit from a private
/// copy (bio's WriteBatch does).
///
/// The write's DMA-source stability contract rides INV-BLKDEV-2.
fn pci_submit_write_async(
    disk: &super::types::GenDisk,
    sector: u64,
    buf: &[u8],
    completion: &crate::fs::io_completion::IoCompletion,
) -> Result<(), i32> {
    pci_submit_data_async(disk, sector, buf.as_ptr() as *mut u8, buf.len(), true, completion)
}

/// Static wrapper matching GenDisk's `async_write_fn` signature for the PCI
/// disk (INV-BLKDEV-2: called only for the registered PCI GenDisk; `buf`
/// is a stable DMA source until the completion fires).
fn pci_async_write_fn(
    disk: &super::types::GenDisk,
    sector: u64,
    buf: &[u8],
    completion: &crate::fs::io_completion::IoCompletion,
) -> i32 {
    match pci_submit_write_async(disk, sector, buf, completion) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Static wrapper matching GenDisk's `async_read_sg_fn` signature: one
/// fused multi-segment READ chain covering the segments' contiguous
/// sector range (INV-BLKDEV-2: every segment stays a valid DMA target
/// until the completion fires).
fn pci_async_read_sg_fn(
    disk: &super::types::GenDisk,
    sector: u64,
    segs: &[super::types::DmaSeg],
    completion: &crate::fs::io_completion::IoCompletion,
) -> i32 {
    match pci_submit_data_async_sg(disk, sector, segs, false, completion) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Static wrapper matching GenDisk's `async_write_sg_fn` signature: one
/// fused multi-segment WRITE chain (INV-BLKDEV-2: every segment is a DMA
/// source that must stay valid and unmodified until the completion
/// fires).
fn pci_async_write_sg_fn(
    disk: &super::types::GenDisk,
    sector: u64,
    segs: &[super::types::DmaSeg],
    completion: &crate::fs::io_completion::IoCompletion,
) -> i32 {
    match pci_submit_data_async_sg(disk, sector, segs, true, completion) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Out-of-order completion instrumentation: counts used-ring entries whose
/// head descriptor id does NOT match the pending published at the entry's
/// positional slot (`i % MAX_PENDING_IO_PCI`). Zero for the whole boot =
/// the device completed every chain in submission order and the pure
/// ordinal mapping of the 7b7e847 design would have sufficed; a large
/// count proves QEMU virtio-blk reorders (mixed-size chains: a 4 KiB read
/// overtakes an in-flight 256 KiB readahead chain) and justifies the
/// device-truthed fallback match. Rate-limited log so the fact shows up
/// in ordinary serial logs without flooding.
static VIRTIO_PCI_REORDER_EVENTS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Used-ring entries the walker consumed that matched NO live pending
/// (neither positionally nor by head_desc). Under the ordinal+window
/// publish discipline every entry must match — a nonzero count means a
/// submit path published a chain without a pending (or a pending was
/// consumed twice), and the entry's chain has no completion to fire.
/// Counted loudly (rate-limited) so a dispatch bug cannot hide.
static VIRTIO_PCI_UNMATCHED_EVENTS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

// ---- VW forensic instrumentation (temporary; wedge hunt r3) ----------
// Counters that make the ftest01 wedge self-describing: which exit the
// completion walker last took, how many PCI IRQs actually fired per disk,
// and a timer-cadence watchdog that prints the full ring/walker state the
// moment PENDING_LAST lags the used ring with no scheduled consumer.
pub static VW_IRQS: [core::sync::atomic::AtomicU64; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_PCI_BLK_DISKS];
pub static VW_WALKS: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
/// Exit-kind tally: 0=fast-path caught up, 1=loop caught up,
/// 2=watermark moved by other, 3=budget exhaustion (re-armed),
/// 4=collected==0 (loop re-check).
pub static VW_EXITS: [core::sync::atomic::AtomicU64; 5] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 5];
/// Jiffy when the walker last exited WITH residual lag (0 = never).
pub static VW_LAST_LAG_EXIT_AT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
/// Times the softirq-context walker DEFERRED its BLK-lock acquire to
/// task-context holders (the kvm-r1 starvation mitigation — see
/// pci_blk_lock_defer). Steady state: zero; a burst during storms is
/// healthy; monotonic growth with no I/O progress is a lock-pressure
/// signal.
pub static VW_LOCK_DEFERS: [core::sync::atomic::AtomicU64; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_PCI_BLK_DISKS];

#[inline]
fn vw_exit(kind: usize, lag: bool) {
    VW_EXITS[kind].fetch_add(1, core::sync::atomic::Ordering::AcqRel);
    if lag {
        VW_LAST_LAG_EXIT_AT.store(
            crate::drivers::hotpath::timer::get_jiffies(),
            core::sync::atomic::Ordering::Release,
        );
    }
}

/// On-demand VW forensic report (called from waiter deadline paths and
/// the DUMP! handler — contexts that run even when timers die).
pub fn vw_report(tag: &str) {
    // EXT4 big lock census first (VW forensic): the f05rep2 wedge leaves
    // children D-sleeping while the virtio rings are fully drained.
    {
        let (locked, owner, depth, queued, woken) =
            crate::fs::journal_seam::EXT4_BIG_LOCK.vw_state();
        crate::pr_err!(
            "VW-EXT4LOCK {} locked={} owner={} depth={} queued={} wokenflag={}",
            tag,
            locked,
            owner,
            depth,
            queued,
            woken
        );
    }
    // PLIC delivery state (the riscv-hang wedge hunt): distinguish
    // "device asserted but nobody claims" (delivery break) from "device
    // never asserted" (driver side).
    #[cfg(feature = "riscv64")]
    crate::drivers::hotpath::intc::plic::vw_plic_dump();
    for disk in 0..MAX_PCI_BLK_DISKS {
        if !PCI_BLK_READY[disk].load(core::sync::atomic::Ordering::Acquire) {
            continue;
        }
        let (used_ring, queue_sz, avail_shadow) = match get_pci_device_queue_at(disk) {
            Some(q) => (q.used_ring_ptr(), q.queue_size, q.avail_shadow_snapshot()),
            None => continue,
        };
        if queue_sz == 0 {
            continue;
        }
        // SAFETY: same idx read as the walker's fast path.
        let used_idx =             used_ring_idx(used_ring);
        let last = PCI_BLK_PENDING_LAST[disk].load(core::sync::atomic::Ordering::Acquire);
        crate::pr_err!(
            "VW-REPORT {} disk={} used={} last={} lag={} expected={} \
             unkicked={} at_device={} walks={} exits fp={} cu={} wm={} \
             budget={} zero={} last_lag_exit={} softirq=0x{:x}/0x{:x}",
            tag,
            disk,
            used_idx,
            last,
            (used_idx.wrapping_sub(last)) & 0xFFFF,
            PCI_BLK_EXPECTED_USED_IDX[disk]
                .load(core::sync::atomic::Ordering::Acquire),
            PCI_BLK_UNKICKED[disk]
                .load(core::sync::atomic::Ordering::Acquire),
            (avail_shadow.wrapping_sub(used_idx)) & 0xFFFF,
            VW_WALKS.load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[0].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[1].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[2].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[3].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[4].load(core::sync::atomic::Ordering::Acquire),
            VW_LAST_LAG_EXIT_AT.load(core::sync::atomic::Ordering::Acquire),
            crate::interrupt::softirq::softirq_pending_raw(0),
            crate::interrupt::softirq::softirq_pending_raw(1),
        );
    }
}

/// x86 INTx Block-service delivery counter (the fn-pointer registry has
/// no slot identity — see intx_blk_service). Printed by VW-SNAP.
pub static VW_BLK_INTX: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Increment the x86 INTx delivery counter (intx_blk_service).
pub fn vw_blk_intx_bump() {
    VW_BLK_INTX.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
}


/// x86_64 IDT vector carrying virtio-blk MSI-X completions (DIO-CONC).
/// Chosen in the free space between the PIC vectors (0x20-0x2F) and the
/// LAPIC timer/IPI block (0xE0-0xE5); wired in trap.rs's IDT build and
/// trap.S's stub, programmed into every virtio-blk function's MSI-X
/// table entry 0 by the PCI probe.
pub const MSIX_BLK_VECTOR: u8 = 0x81;

/// MSI-X completion service for the virtio-blk disks (hard-IRQ context,
/// vector MSIX_BLK_VECTOR). Runs the per-disk completion walker inline —
/// the reference kernel completes vring requests in hardirq too. The
/// walker's hardirq lock policy (DIO-CONC: in_irq() joins the bounded
/// try-lock deferral) keeps the R12-3 discipline: a task-context
/// submitter holding a BLK lock makes the walker defer to the Block
/// softirq instead of spinning with IF=0.
pub fn msix_blk_isr() {
    for disk in 0..MAX_PCI_BLK_DISKS {
        if PCI_BLK_READY[disk].load(core::sync::atomic::Ordering::Acquire) {
            pci_process_async_completions_slot(disk);
        }
    }
}


/// One-line direct-console virtio ring snapshot (VW-SNAP): the same
/// fields as the watchdog's VW-WEDGE, printed through putchar_no_lock —
/// pr_err-based reports were silently dropped from the audit context on
/// the OH boots (lb70/lb71: dump printed, VW-REPORT never landed).
/// Called from the txn-hang-20s trigger.
pub fn vw_snap_direct() {
    use crate::console::putchar_no_lock as putchar;
    fn pd(mut v: u64) {
        use crate::console::putchar_no_lock as putchar;
        let mut b = [0u8; 20];
        let mut n = 0usize;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 && n < b.len() {
            b[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        let mut i = n;
        while i > 0 {
            i -= 1;
            putchar(b[i]);
        }
    }
    fn ps(s: &[u8]) {
        use crate::console::putchar_no_lock as putchar;
        for &c in s {
            putchar(c);
        }
    }
    ps(b"\nVW-SNAP pending=0x");
    pd(crate::interrupt::softirq::softirq_pending_raw(0) as u64);
    putchar(b'/');
    pd(crate::interrupt::softirq::softirq_pending_raw(1) as u64);
    ps(b" walks=");
    pd(VW_WALKS.load(core::sync::atomic::Ordering::Acquire));
    #[cfg(feature = "x86_64")]
    {
        ps(b" intx=");
        pd(VW_BLK_INTX.load(core::sync::atomic::Ordering::Acquire));
    }
    for disk in 0..MAX_PCI_BLK_DISKS {
        if !PCI_BLK_READY[disk].load(core::sync::atomic::Ordering::Acquire) {
            continue;
        }
        let (used_ring, _qsz) = match get_pci_device_queue_at(disk) {
            Some(q) => (q.used_ring_ptr(), q.queue_size),
            None => continue,
        };
        // SAFETY: same idx read as the walker's fast path.
        let used_idx =             used_ring_idx(used_ring);
        ps(b" | d");
        pd(disk as u64);
        ps(b" used=");
        pd(used_idx as u64);
        ps(b" last=");
        pd(PCI_BLK_PENDING_LAST[disk].load(core::sync::atomic::Ordering::Acquire) as u64);
        ps(b" shadow=");
        pd(get_pci_device_queue_at(disk)
            .map(|q| q.avail_shadow_snapshot())
            .unwrap_or(0) as u64);
        ps(b" unkicked=");
        pd(PCI_BLK_UNKICKED[disk].load(core::sync::atomic::Ordering::Acquire) as u64);
        ps(b" irqs=");
        pd(VW_IRQS[disk].load(core::sync::atomic::Ordering::Acquire));
    }
    putchar(b'\n');
}

/// Timer-cadence lag watchdog: called once per jiffy from the Timer
/// softirq (the context that keeps running when everything else wedges).
/// When a ready disk's used ring runs ahead of the walker for >= 3s,
/// print the full forensic snapshot (rate-limited to one per 10s).
pub fn vw_lag_watchdog(now: u64) {
    // Holder watchdog first: a BLK lock held > 5 s is a wedge in
    // progress even when the rings look healthy (b125 shape — spinner
    // on the lock, no lag anywhere).
    pci_blk_holder_watchdog(now);
    static FIRST_LAG: [core::sync::atomic::AtomicU64; MAX_PCI_BLK_DISKS] =
        [const { core::sync::atomic::AtomicU64::new(0) }; MAX_PCI_BLK_DISKS];
    static LAST_PRINT: core::sync::atomic::AtomicU64 =
        core::sync::atomic::AtomicU64::new(0);
    for disk in 0..MAX_PCI_BLK_DISKS {
        if !PCI_BLK_READY[disk].load(core::sync::atomic::Ordering::Acquire) {
            continue;
        }
        let (used_ring, queue_sz) = match get_pci_device_queue_at(disk) {
            Some(q) => (q.used_ring_ptr(), q.queue_size),
            None => continue,
        };
        if queue_sz == 0 {
            continue;
        }
        // SAFETY: same idx read as the walker's fast path.
        let used_idx =             used_ring_idx(used_ring);
        let last = PCI_BLK_PENDING_LAST[disk].load(core::sync::atomic::Ordering::Acquire);
        // Device-side stall component: chains published to the avail ring
        // that the device has NOT completed (avail_shadow - used). Combined
        // with unkicked, separates "device working" from "LOST KICK"
        // (quiet-submitted chains nobody ever notified).
        let _ = &used_ring; // keep the ptr alive for both branches
        let lag_used = used_idx != last;
        // avail_shadow lives in the VirtQueue; get it via the queue helper.
        let avail_shadow = get_pci_device_queue_at(disk)
            .map(|q| q.avail_shadow_snapshot())
            .unwrap_or(0);
        let at_device = (avail_shadow.wrapping_sub(used_idx)) & 0xFFFF;
        let unkicked_now = PCI_BLK_UNKICKED[disk]
            .load(core::sync::atomic::Ordering::Acquire);
        let stalled = lag_used || (at_device != 0 && unkicked_now != 0);
        if !stalled {
            FIRST_LAG[disk].store(0, core::sync::atomic::Ordering::Release);
            continue;
        }
        let first = FIRST_LAG[disk].load(core::sync::atomic::Ordering::Acquire);
        if first == 0 {
            FIRST_LAG[disk].store(now, core::sync::atomic::Ordering::Release);
            continue;
        }
        if now.saturating_sub(first) < 300 {
            continue; // < 3s of lag: not yet a wedge
        }
        let last_print = LAST_PRINT.load(core::sync::atomic::Ordering::Acquire);
        if now.saturating_sub(last_print) < 1000 {
            continue; // rate limit 10s
        }
        LAST_PRINT.store(now, core::sync::atomic::Ordering::Release);
        let expected = PCI_BLK_EXPECTED_USED_IDX[disk]
            .load(core::sync::atomic::Ordering::Acquire);
        let unkicked = PCI_BLK_UNKICKED[disk]
            .load(core::sync::atomic::Ordering::Acquire);
        crate::pr_err!(
            "VW-WEDGE disk={} used={} last={} lag={} expected={} unkicked={} \
             at_device={} avail_shadow={} \
             irqs={} walks={} exits fp={} cu={} wm={} budget={} zero={} \
             last_lag_exit={} softirq_pending=0x{:x}/0x{:x}",
            disk,
            used_idx,
            last,
            (used_idx.wrapping_sub(last)) & 0xFFFF,
            expected,
            unkicked,
            at_device,
            avail_shadow,
            VW_IRQS[disk].load(core::sync::atomic::Ordering::Acquire),
            VW_WALKS.load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[0].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[1].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[2].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[3].load(core::sync::atomic::Ordering::Acquire),
            VW_EXITS[4].load(core::sync::atomic::Ordering::Acquire),
            VW_LAST_LAG_EXIT_AT.load(core::sync::atomic::Ordering::Acquire),
            crate::interrupt::softirq::softirq_pending_raw(0),
            crate::interrupt::softirq::softirq_pending_raw(1),
        );
        // One-shot full task dump at the FIRST wedge of the boot: names the
        // stranded waiters' states (polling with a guard vs asleep on the
        // sync queue), any preempt-marked incumbent, and ksoftirqd's
        // runnability — the candidate engines of this wedge (lb66 installs
        // mkdir/journal hang + make-test sgid_mode VW-WEDGE used=28).
        static WEDGE_DUMPED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if !WEDGE_DUMPED.swap(true, core::sync::atomic::Ordering::AcqRel) {
            crate::debug::taskdump::dump_all_tasks("vw-wedge");
        }
    }
}

/// Process completed PCI async reads: walk the used ring from the last
/// processed index and fire the pending I/O matching each entry's chain.
///
/// ORDINAL FAST PATH + DEVICE-TRUTHED FALLBACK: used-ring entry i is first
/// checked against the pending at its positional slot
/// (`i % MAX_PENDING_IO_PCI`) — the submission-ordinal mapping, which is
/// correct whenever the device completes chains in submission order. On a
/// mismatch the walker scans the table for the entry whose `head_desc`
/// equals the used-ring entry's UsedElem.id — the chain the device
/// ACTUALLY completed — so out-of-order completion (QEMU virtio-blk
/// reorders mixed-size chains; counted in VIRTIO_PCI_REORDER_EVENTS)
/// cannot fire the wrong pending: a pending fires exactly when ITS
/// chain's used-ring entry appears, never earlier. Head-descriptor
/// uniqueness among live entries is guaranteed by the publish-time window
/// check (pci_pending_slot_reservable).
///
/// Runs in the Block softirq (raised by the PCI IRQ handler) — never in
/// hard-IRQ context — and from process-context drain/rescue paths.
/// VIRTIO-WQ-1 discipline (the virtio-blk ABBA fix): the walk COLLECTS
/// finished PendingIo entries under the BLK lock, then drops every lock
/// before delivering (dealloc + IoCompletion::complete, which takes a
/// wait-queue lock and calls wake_up_process per waiter).
///
/// LOSSLESS DISPATCH (the ftest01 lost-wakeup fix): the walk loops until
/// it is caught up with the used ring (or its one-window budget is
/// spent), and on EVERY exit with residual lag — budget exhaustion, or a
/// completion that landed mid-pass — it re-raises the Block softirq. The
/// old walker could return with used.idx ahead of PENDING_LAST and NO
/// interrupt scheduled: the device was idle (nothing will interrupt
/// again), the softirq pending bits were clear, and the waiters of the
/// unconsumed entries slept until their 10s deadlines — the ftest01
/// wedge (freeze-dumped live: PENDING_LAST one behind, everything else
/// quiescent, six children asleep).
///
/// The sync-queue wake is GATED on real progress: wake_up_all on the
/// disk's sync queue costs a wake_up_process (GRQ lock) per waiter, and
/// firing it from every Block softirq — including timer-driven passes
/// that consumed nothing — put the six ftest01 children into a permanent
/// wake/re-queue herd that starved the very completions they waited for
/// (freeze-dumps show both CPUs grinding in the GRQ/CFS btree while the
/// used ring runs 1-8 entries ahead of the walker).
///
/// Returns the number of used-ring entries consumed (matched or skipped).
pub fn pci_process_async_completions() {
    for disk in 0..MAX_PCI_BLK_DISKS {
        if PCI_BLK_READY[disk].load(core::sync::atomic::Ordering::Acquire) {
            pci_process_async_completions_slot(disk);
        }
    }
}

/// Per-disk completion walker (see pci_process_async_completions for the
/// design notes; every table/lock reference below is disk-local).
pub fn pci_process_async_completions_slot(slot: usize) -> usize {
    /// Collected-per-lock-pass bound. 16 × sizeof(PendingIo) ≈ 1.3 KiB of
    /// stack per pass; the outer loop repeats until caught up or the total
    /// budget (one queue window) is spent.
    const CHUNK: usize = 16;

    // Fast path: nothing pending. Read the used ring first; if the walker is
    // already caught up, skip the lock entirely.
    let (used_ring, queue_sz) = match get_pci_device_queue_at(slot) {
        Some(q) => (q.used_ring_ptr(), q.queue_size),
        None => return 0,
    };
    if queue_sz == 0 {
        return 0;
    }
    // SAFETY: used ring offset 2 is the idx field (u16); the queue is alive
    // (PCI_BLK_READY was checked by get_pci_device_queue_at).
    let used_idx =         used_ring_idx(used_ring);
    let last = PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire);
    if used_idx == last {
        vw_exit(0, false);
        return 0;
    }
    VW_WALKS.fetch_add(1, core::sync::atomic::Ordering::AcqRel);

    // Bounded by one queue window per call; u16 wrap-safe walk.
    let mut budget = MAX_PENDING_IO_PCI as u16;
    let mut consumed_total = 0usize;
    loop {
        // ---- Phase 1: collect under the BLK lock (short irqsave section) --
        let mut done: [Option<PendingIo>; CHUNK] = [const { None }; CHUNK];
        let mut collected = 0usize;
        let mut walked = 0usize; // entries consumed this pass (matched+skipped)
        let mut unmatched = 0usize;
        let i_end;
        {
            // SOFTIRQ DEFERRAL (the kvm-r1 starvation observation): a
            // walker in softirq context takes the lock with a bounded
            // try budget and defers (re-raise + return) when task-context
            // submitters hold it — softirq has no deadline, submitters'
            // waiters do, and the TTAS lock gives an endlessly-reacquiring
            // softirq the power to starve them for seconds. Process-context
            // callers (drain/rescue paths — the last-line consumers when
            // no interrupt will come) keep the blocking acquire.
            // DIO-CONC: HARDIRQ joins the deferral class — the MSI-X
            // completion service runs this walker inline in hardirq
            // context, and spinning there with IF=0 on a task-context
            // submitter's BLK lock is the same starvation shape the
            // softirq rule exists to prevent.
            let _guard = if crate::interrupt::preempt::in_softirq()
                || crate::interrupt::preempt::in_irq()
            {
                match pci_blk_lock_defer(slot, 4096) {
                    Some(g) => g,
                    None => {
                        VW_LOCK_DEFERS[slot].fetch_add(1, core::sync::atomic::Ordering::AcqRel);
                        // Lossless dispatch preserved: a consumer is
                        // re-scheduled (the softirq restart loop, ksoftirqd
                        // via raise_softirq's wakeup, or the next device
                        // IRQ).
                        crate::interrupt::softirq::raise_softirq(
                            crate::interrupt::softirq::SoftirqIndex::Block as usize,
                        );
                        return 0;
                    }
                }
            } else {
                pci_blk_lock(slot)
            };
            let _nest = VirtioLockNest::new();
            // Re-read under the lock (a submission may have landed since).
            // SAFETY: same field, queue alive.
            let used_idx =                 used_ring_idx(used_ring);
            let mut i = PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire);
            while i != used_idx && budget > 0 && collected < CHUNK {
                budget -= 1;
                core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
                // The completed chain's head descriptor id (UsedElem.id at
                // used_ring + 4 + pos*8; the device publishes it only after
                // the chain — including its DMA writes — is done).
                // SAFETY: ring position `i % queue_sz` is within the used ring.
                let ring_pos = (i as usize) % queue_sz as usize;
                let entry_id = used_ring_elem_id(used_ring, ring_pos);
                // Positional fast path: entry i belongs at ordinal slot
                // i % MAX_PENDING_IO_PCI when completion order equals
                // submission order. (NOTE: `slot` is the DISK; the pending
                // table index below is `ordinal_slot` — do not conflate.)
                let ordinal_slot = i as usize % MAX_PENDING_IO_PCI;
                let mut fired: Option<PendingIo> = None;
                {
                    // SUBMIT-2X: the BLK lock held by this walker pass
                    // serializes all pending-table access (submit publish
                    // takes the same lock) — direct access, no second
                    // table-lock acquisition per completed chain.
                    // SAFETY: BLK lock held (the _guard above).
                    let table = unsafe { PCI_BLK_PENDING[slot].get_mut_unchecked() };
                    let positional_match = match table[ordinal_slot].as_ref() {
                        Some(p) => p.head_desc == entry_id,
                        None => false,
                    };
                    if positional_match {
                        fired = table[ordinal_slot].take();
                    } else {
                        // Out-of-order (or the positional slot holds a
                        // different live chain): scan for the entry whose
                        // chain the device ACTUALLY completed. The
                        // publish-time window check makes head_desc unique
                        // among live entries, so the match is unambiguous.
                        // Tombstones match too (their used-ring entry
                        // releases the descriptor window and ordinal slot).
                        for e in table.iter_mut() {
                            if let Some(p) = e {
                                if p.head_desc == entry_id {
                                    fired = e.take();
                                    break;
                                }
                            }
                        }
                    }
                    // ABANDON-vs-DELIVERY protocol (the bad-verify fix):
                    // mark the waiter's completion as under delivery BEFORE
                    // the table lock drops. abandon_pending_completion
                    // (the 10s-deadline retire path) scans the tables under
                    // this same lock; if it does NOT find the entry it
                    // knows a walker is mid-delivery and waits for this
                    // count to drain — so a walker can never fire into a
                    // waiter's kernel stack after its wait() returned (the
                    // ftest01 "2048*25 bad verify" wandering corruption).
                    if let Some(p) = fired.as_ref() {
                        if !p.completion.is_null() {
                            unsafe {
                                (*p.completion).begin_delivery();
                            }
                        }
                    }
                }
                match fired.as_ref() {
                    Some(p) => {
                        // The chain's descriptor window and held-descriptor
                        // budget are released HERE (the entry leaves the
                        // table under this lock): sync tombstones and
                        // timeout tombstones carry n_descs like every
                        // other published chain.
                        if let Some(vq) = get_pci_device_queue_at(slot) {
                            vq.release_descs(p.n_descs);
                        }
                        // Exact per-entry disorder measurement: the entry
                        // was submitted at ordinal p.ordinal but completed
                        // at used-ring index i.
                        if p.ordinal != i {
                            let n = VIRTIO_PCI_REORDER_EVENTS
                                .fetch_add(1, core::sync::atomic::Ordering::AcqRel)
                                + 1;
                            if n == 1 || n % 8192 == 0 {
                                crate::pr_info!(
                                    "virtio-blk: out-of-order completion #{} \
                                     (used-ring order != submission order)",
                                    n
                                );
                            }
                        }
                        done[collected] = fired.take();
                        collected += 1;
                    }
                    None => {
                        // No live pending claims this used-ring entry. Every
                        // submitted chain publishes a pending or tombstone,
                        // so this is a dispatch invariant violation — count
                        // it loudly. The entry is consumed (PENDING_LAST
                        // advances) so one bad entry cannot stall the whole
                        // queue; the log makes the accounting hole visible.
                        unmatched += 1;
                        let n = VIRTIO_PCI_UNMATCHED_EVENTS
                            .fetch_add(1, core::sync::atomic::Ordering::AcqRel)
                            + 1;
                        if n == 1 || n % 4096 == 0 {
                            crate::pr_err!(
                                "virtio-blk: used entry {} (head {}) matched \
                                 no pending — dispatch accounting hole #{}",
                                i,
                                entry_id,
                                n
                            );
                        }
                    }
                }
                walked += 1;
                i = i.wrapping_add(1);
            }
            i_end = i;
            PCI_BLK_PENDING_LAST[slot].store(i, core::sync::atomic::Ordering::Release);
            // _nest, table guards and the BLK lock all drop HERE.
        }
        consumed_total += walked;

        // ---- Phase 2: deliver OUTSIDE every virtio lock (R12-3) -----------
        // Each complete() takes the waiter's wait-queue lock and runs
        // wake_up_process per waiter; doing that under the BLK lock is the
        // ABBA this walker must never reintroduce.
        for k in 0..collected {
            let pending = done[k].take().unwrap();
            // NULL completion = TOMBSTONE (synchronous chain or timed-out
            // abandon): its used-ring entry just released the descriptor
            // window — nothing to read, free, or fire. Sync chains' io_buf
            // is owned and freed by their own waiter; abandoned io_bufs
            // were deliberately leaked at abandon time.
            if pending.completion.is_null() {
                // A TIMED-OUT tombstone's completion finally landed: pair
                // the note_timed_out_chain increment from its deadline path
                // so leaked_chains tracks only genuinely unresolved chains
                // (an ever-growing counter permanently shrank the
                // in-flight admission guard).
                if pending.timed_out {
                    if let Some(vq) = get_pci_device_queue_at(slot) {
                        vq.resolve_leaked_chain();
                    }
                }
                continue;
            }
            let status = resp_status_byte(pending.resp_ptr as *const u8);
            let io_status = if status == 0 { 0 } else { -5i32 };
            // The combined R17-C block: header at base, resp at +48, both
            // inside the single 64-byte allocation. SUBMIT-2X: return it
            // to the per-CPU pool (layout-checked; anything not the exact
            // R17-C shape keeps the heap dealloc). The used-ring entry was
            // consumed above, so the device is done DMA-ing into it.
            io_buf_release(pending.header_ptr, pending.header_layout);
            // block:block_rq_complete — the async chain's IRQ-side
            // completion, with the geometry stored at submit time
            // (tombstones and sync chains carry zeros and skip).
            if pending.trace_dev != 0 {
                crate::trace::trace_block_rq_complete(
                    pending.trace_dev,
                    pending.trace_sector,
                    pending.trace_nr_sector as u32,
                    io_status,
                    0,
                    if pending.is_write { "W" } else { "R" },
                );
            }
            // SAFETY: the delivering-protocol count taken under the table
            // lock keeps the waiter (and its stack) alive across this call
            // — abandon waits for it before unwinding. Token = the chain's
            // DMA data pointer: bread_wait's zero-serve guard uses it to
            // prove THIS buffer's chain completed.
            deliver_completion(pending.completion, io_status, pending.data_ptr);
        }

        if i_end != PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire) {
            // Another CPU (or our own nested context) already moved the
            // watermark — it owns the remaining entries.
            vw_exit(2, true);
            break;
        }
        // Caught up? Re-read the ring: completions may have landed while we
        // delivered (budget re-check keeps the loop bounded either way).
        // SAFETY: same field read as the fast path.
        let fresh_used =             used_ring_idx(used_ring);
        if fresh_used == i_end {
            vw_exit(1, false);
            break;
        }
        if budget == 0 {
            // One queue window consumed; more remain. Re-arm the Block
            // softirq so a scheduled consumer always exists for the rest.
            vw_exit(3, true);
            crate::interrupt::softirq::raise_softirq(
                crate::interrupt::softirq::SoftirqIndex::Block as usize,
            );
            break;
        }
        vw_exit(4, true);
    }

    // GATED sync-queue wake: only when this pass actually consumed
    // completions on THIS disk. The unconditional wake (every Block
    // softirq pass, including timer-driven no-op passes) was the wake-all
    // herd that turned six concurrent waiters into a GRQ convoy under
    // TCG — the completion delivery starvation behind the ftest01 wedge.
    // Waking here (rather than only in the BH) also covers the
    // process-context drain/rescue callers: a sync waiter is why they ran.
    if consumed_total > 0 {
        #[cfg(debug_assertions)]
        crate::drivers::hotpath::assert_no_virtio_lock(
            "PCI_BLK_WAIT_QUEUES wake (walker)",
        );
        PCI_BLK_WAIT_QUEUES[slot].wake_up_all();
    }
    consumed_total
}

/// Waiter-side rescue for lagged completions (companion to the walker's
/// own re-arm): if this disk's used ring is ahead of the walker and no
/// interrupt is coming (device idle, softirq bits clear), the sleeping
/// waiter is the only context that can make progress — drain the ring
/// ourselves before going back to sleep. Cheap when caught up (two
/// loads). No-op for out-of-range/unready slots.
pub fn pci_rescue_lagged_completions(slot: usize) {
    if slot >= MAX_PCI_BLK_DISKS
        || !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire)
    {
        return;
    }
    let used_ring = match get_pci_device_queue_at(slot) {
        Some(q) => q.used_ring_ptr(),
        None => return,
    };
    // SAFETY: same used-ring idx read as the walker's fast path.
    let used_idx =         used_ring_idx(used_ring);
    let last = PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire);
    if used_idx != last {
        pci_process_async_completions_slot(slot);
    }
}

/// All-disks variant of the waiter-side rescue for the async
/// IoCompletion::wait path (which has no slot identity): drain every
/// ready PCI disk whose used ring is ahead of the completion walker.
/// Cheap when caught up (two loads per disk); converts a lost interrupt
/// engine into bounded poll latency instead of a 10 s deadline stall.
pub fn pci_rescue_all_lagged() {
    for slot in 0..MAX_PCI_BLK_DISKS {
        pci_rescue_lagged_completions(slot);
    }
}

/// Per-slot ready flags (set after the device + queue + GenDisk are all in
/// place; multi-core visibility via SeqCst).
static PCI_BLK_READY: [core::sync::atomic::AtomicBool; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicBool::new(false) }; MAX_PCI_BLK_DISKS];

/// Number of PCI virtio-blk slots actually registered (boot probe order:
/// slot i is vd<'a'+i>). Exposed for devtmpfs node creation and letter->
/// disk resolution.
static PCI_BLK_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Initialize VirtIO block device
///
/// # Parameters
/// - `base_addr`: MMIO base address (QEMU virt platform typically 0x10001000)
pub fn virtio_blk_init(base_addr: u64) -> Result<(), &'static str> {
    let mut device = VirtIOBlkDevice::new(base_addr);

    device.init()?;

    // Publish into the static slot; private_data receives the device's
    // own final static address (INV-SLOT-1/2 — OnceSlot owns the
    // publication protocol the hand-rolled static mut used to carry).
    VIRTIO_BLK.publish_with_addr(|addr| {
        device.disk.private_data = Some(addr as *mut u8);
        device
    });
    Ok(())
}

/// Register PCI VirtIO device
///
/// # Parameters
/// - `device`: PCI VirtIO device
pub fn register_pci_device(slot: usize, mut device: super::types::VirtIOPCI) {
    if slot >= MAX_PCI_BLK_DISKS {
        return;
    }
    // Stamp the slot into the device: every I/O path reaches its queue,
    // lock, wait queue and pending table THROUGH this number.
    device.blk_slot = slot;
    // INV-SLOT-1/2: publish (Release) then the ready flag (SeqCst keeps
    // the probe's original ordering guarantee for the READY readers).
    PCI_BLK_DEVICES[slot].publish(device);
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    PCI_BLK_READY[slot].store(true, core::sync::atomic::Ordering::SeqCst);
}

/// Number of registered PCI virtio-blk disks (slots 0..n).
pub fn pci_blk_disk_count() -> usize {
    PCI_BLK_COUNT.load(core::sync::atomic::Ordering::Acquire)
}

/// Register slot `slot` as a live disk (called by the probe after the
/// GenDisk + sysfs registration, once per disk).
pub fn pci_blk_slot_online(slot: usize) {
    PCI_BLK_COUNT.fetch_max(slot + 1, core::sync::atomic::Ordering::AcqRel);
}

/// Is the PCI virtio-blk disk in `slot` ready for I/O?
pub fn pci_blk_slot_ready(slot: usize) -> bool {
    slot < MAX_PCI_BLK_DISKS && PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire)
}

/// Get VirtIO block device
///
/// Returns PCI VirtIO device first, or MMIO device if unavailable
pub fn virtio_blk_get_device() -> Option<&'static VirtIOBlkDevice> {
    // If PCI device exists, use it for I/O
    // Note: Currently PCI device uses separate I/O interface, returning MMIO device as fallback
    VIRTIO_BLK.get()
}

/// Get the BOOT PCI virtio-blk device (slot 0). Legacy single-disk
/// callers (flush, devfs presence checks) keep this view.
pub fn get_pci_device() -> Option<&'static super::types::VirtIOPCI> {
    get_pci_device_at(0)
}

/// Get the PCI virtio-blk device in `slot`.
pub fn get_pci_device_at(slot: usize) -> Option<&'static super::types::VirtIOPCI> {
    if slot >= MAX_PCI_BLK_DISKS {
        return None;
    }
    // Check if device is ready
    if !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    PCI_BLK_DEVICES[slot].get()
}

/// Set a slot's configured VirtQueue.
pub fn set_pci_device_queue(slot: usize, queue: VirtQueue) {
    if slot >= MAX_PCI_BLK_DISKS {
        return;
    }
    PCI_BLK_QSIZE[slot].store(queue.queue_size, core::sync::atomic::Ordering::Release);
    PCI_BLK_QUEUES[slot].publish(queue);
    // Initialize expected used.idx to 0 (new queue starts at 0)
    PCI_BLK_EXPECTED_USED_IDX[slot].store(0, core::sync::atomic::Ordering::Release);
}

/// Per-slot negotiated virtqueue depth (GUP-DIO: the SG arms size their
/// chains/waves from it). 0 = slot not configured.
static PCI_BLK_QSIZE: [core::sync::atomic::AtomicU16; MAX_PCI_BLK_DISKS] =
    [const { core::sync::atomic::AtomicU16::new(0) }; MAX_PCI_BLK_DISKS];

/// SG chain limits of `slot`'s virtqueue: (max data segments per chain,
/// descriptor budget for concurrent chains). Conservative fallback
/// (16 / 100) when the slot is unconfigured.
pub fn pci_blk_sg_limits(slot: usize) -> (usize, usize) {
    let q = PCI_BLK_QSIZE
        .get(slot)
        .map(|q| q.load(core::sync::atomic::Ordering::Acquire) as usize)
        .unwrap_or(0);
    if q < 16 {
        return (16, 100);
    }
    // seg_max = queue_size - 2 (header + response). The descriptor
    // budget keeps a small margin for other in-flight chains (sync
    // reads, journal traffic) that share the ring; alloc_desc_run's own
    // desc budget (q) is the hard bound.
    (q - 2, q - 8)
}

/// Get a slot's VirtQueue (mutable reference).
///
/// Exclusivity is PROVEN by the guard: `proof` must be a live
/// `pci_blk_lock(slot)` guard (INV-SLOT-3 — PciBlkGuard's
/// ExclusivityProof impl is the audited assertion that this lock
/// governs all queue mutation).
pub fn get_pci_device_queue_mut_at(
    slot: usize,
    proof: &PciBlkGuard,
) -> Option<&'static mut VirtQueue> {
    if slot >= MAX_PCI_BLK_DISKS {
        return None;
    }
    // Check if device is ready
    if !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    PCI_BLK_QUEUES[slot].get_mut_locked(proof)
}

/// Get a slot's VirtQueue (read-only reference).
pub fn get_pci_device_queue_at(slot: usize) -> Option<&'static VirtQueue> {
    if slot >= MAX_PCI_BLK_DISKS {
        return None;
    }
    // Check if device is ready
    if !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::Acquire) {
        return None;
    }
    PCI_BLK_QUEUES[slot].get()
}

/// Get a slot's expected used.idx (for waiting I/O completion).
pub fn get_expected_used_idx_at(slot: usize) -> u16 {
    if slot >= MAX_PCI_BLK_DISKS {
        return 0;
    }
    PCI_BLK_EXPECTED_USED_IDX[slot].load(core::sync::atomic::Ordering::Acquire)
}

/// Walker-lag snapshot for one slot's forensic prints (VW-POLLSTALL
/// family): (walker position in the used ring, quietly-submitted-but-
/// unkicked chain count). Splits "the device completed it" from "the
/// completion walker never consumed it" and "nobody ever kicked" in a
/// single line.
pub fn pci_blk_lag_snapshot(slot: usize) -> (u16, usize) {
    if slot >= MAX_PCI_BLK_DISKS {
        return (0, 0);
    }
    (
        PCI_BLK_PENDING_LAST[slot].load(core::sync::atomic::Ordering::Acquire),
        PCI_BLK_UNKICKED[slot].load(core::sync::atomic::Ordering::Acquire),
    )
}

/// Increment a slot's expected used.idx (called after submitting request,
/// under that slot's BLK lock).
pub fn increment_expected_used_idx_at(slot: usize) {
    if slot >= MAX_PCI_BLK_DISKS {
        return;
    }
    PCI_BLK_EXPECTED_USED_IDX[slot].fetch_update(
        core::sync::atomic::Ordering::Release,
        core::sync::atomic::Ordering::Relaxed,
        |v| Some(v.wrapping_add(1))
    ).ok();
}

/// Get a slot's sync-I/O wait queue (for interrupt handler / softirq wake).
pub fn get_pci_blk_wait_queue(slot: usize) -> &'static crate::process::types::WaitQueueHead {
    &PCI_BLK_WAIT_QUEUES[slot.min(MAX_PCI_BLK_DISKS - 1)]
}

/// Get reference to MMIO VirtIO block wait queue (for interrupt handler)
pub fn get_mmio_blk_wait_queue() -> &'static crate::process::types::WaitQueueHead {
    &VIRTIO_BLK_WAIT_QUEUE
}

/// Register PCI VirtIO device's GenDisk
///
/// Creates a GenDisk wrapper so ext4 driver can access PCI VirtIO device through standard block device interface
pub fn register_pci_gen_disk(slot: usize) {
    use alloc::boxed::Box;

    if slot >= MAX_PCI_BLK_DISKS {
        return;
    }

    // One GenDisk per PCI function. Majors are allocated 8+slot: the block
    // device manager keys disks by major alone, so every disk needs its
    // own. The slot index rides in GenDisk.private_data — request_fn and
    // async_read_fn route to the disk's own queue/lock/wait-queue/pending
    // table through it. (sysfs/devfs numbers stay Linux-shaped: the
    // virtio-blk major 254 with per-disk minors.)
    let mut disk = Box::new(GenDisk::new(
        "pci-virtblk",
        8 + slot as u32, // major number (unique per disk in the manager)
        1,               // minors
        512,             // block size
        Some(&VIRTIO_BLK_OPS), // rung4 prep move 2: the ops table carries
                               // the kick so the block layer's drains
                               // never name this driver.
    ));
    disk.set_private_data(slot as *mut u8);

    // Read device capacity from this slot's function.
    if let Some(pci_dev) = get_pci_device_at(slot) {
        let device_cfg_addr = if pci_dev.device_cfg_bar != 0 {
            pci_dev.device_cfg_bar
        } else {
            pci_dev.common_cfg_bar + 0x2000
        };
        // INV-MMIO-1..3: the device-config region for this function
        // (dedicated BAR, or common-cfg + 0x2000 per the fallback above).
        let capacity_sectors =
            crate::safety::mmio::MmioRegion::new(device_cfg_addr as usize, 8).read_u64(0);
        disk.set_capacity(capacity_sectors as u64);
    }

    // Set request handler function
    disk.set_request_fn(pci_virtio_handle_request);
    // Async read path: without this, bio::bread_async failed with ENXIO
    // on the root disk and ext4 read-ahead was silently disabled there
    // (every page miss = one synchronous virtio round trip).
    disk.set_async_read_fn(pci_async_read_fn);
    // Async write path (the plugged-write primitive): without it,
    // bio's WriteBatch falls back to serialized synchronous writes —
    // correct, but the fsync flush wall stays.
    disk.set_async_write_fn(pci_async_write_fn);
    // Scatter-gather forms (the blk-merge front/back-merge endpoint):
    // one fused chain per group of device-adjacent spans. Without them
    // the batch layers fall back to one chain per span (the pre-fusion
    // shape).
    disk.set_async_read_sg_fn(pci_async_read_sg_fn);
    disk.set_async_write_sg_fn(pci_async_write_sg_fn);

    // Register to block device manager
    let _ = crate::drivers::hotpath::register_disk(disk);
    pci_blk_slot_online(slot);
}

/// PCI VirtIO block device request handler — the registered
/// `request_fn` for every PCI virtio-blk disk (INV-BLKDEV-2/3: called
/// only for disks registered by register_pci_gen_disk, whose
/// private_data is the slot index).
fn pci_virtio_handle_request(req: &mut Request) {
    use crate::drivers::types::ReqCmd;

    // Route to the OWNING disk: GenDisk.private_data carries the slot.
    let slot = req.device.private_data.map(|p| p as usize).unwrap_or(0);

    // Check if device is ready (use SeqCst for strongest memory visibility)
    if slot >= MAX_PCI_BLK_DISKS || !PCI_BLK_READY[slot].load(core::sync::atomic::Ordering::SeqCst) {
        crate::pr_err!("virtio: PCI device not ready");
        req.error.store(-6, core::sync::atomic::Ordering::Release);
        return;
    }

    // Get PCI device
    let pci_dev = match get_pci_device_at(slot) {
        Some(dev) => dev,
        None => {
            crate::pr_err!("virtio: No PCI device for request");
            req.error.store(-6, core::sync::atomic::Ordering::Release);
            return;
        }
    };

    // Execute operation based on command type
    let result = match req.cmd_type {
        ReqCmd::Read => {
            // Read block
            pci_virtio_read_block(pci_dev, req.sector, &mut req.buffer)
        }
        ReqCmd::Write => {
            // Write block
            pci_virtio_write_block(pci_dev, req.sector, &req.buffer)
        }
        ReqCmd::Flush => {
            // Flush operation (return success for now)
            Ok(())
        }
    };

    // Record the device status (R9-4: blkdev_read/write check it — the
    // PCI path previously swallowed errors entirely).
    match result {
        Ok(()) => req.error.store(0, core::sync::atomic::Ordering::Release),
        Err(err) => req.error.store(err, core::sync::atomic::Ordering::Release),
    }
}

/// Read block using PCI VirtIO device
fn pci_virtio_read_block(
    pci_dev: &super::types::VirtIOPCI,
    sector: u64,
    buf: &mut [u8],
) -> Result<(), i32> {
    // PCI lock is now managed inside read_block_once()
    use read_block_using_configured_queue;

    match read_block_using_configured_queue(pci_dev, sector, buf) {
        Ok(_) => Ok(()),
        Err(_) => Err(-5),  // EIO
    }
}

/// Write block using PCI VirtIO device
fn pci_virtio_write_block(
    pci_dev: &super::types::VirtIOPCI,
    sector: u64,
    buf: &[u8],
) -> Result<(), i32> {
    // PCI lock is now managed inside write_block_once()
    use write_block_using_configured_queue;

    match write_block_using_configured_queue(pci_dev, sector, buf) {
        Ok(_) => Ok(()),
        Err(_) => Err(-5),  // EIO
    }
}

/// Flush the PCI VirtIO block device's write cache (VIRTIO_BLK_T_FLUSH).
///
/// Persists the disk's volatile cache after the kernel's write-through
/// buffer cache has been drained. Returns Ok(()) when no PCI blk device
/// is registered (nothing to flush) so callers can invoke it blindly.
pub fn flush_pci_blk() -> Result<(), i32> {
    // Flush EVERY registered disk: callers (ext4 sync, fsync paths) want
    // persistence guarantees and a disk name is not threaded through the
    // generic paths. Sequential per-disk flushes — never nested locks.
    for slot in 0..MAX_PCI_BLK_DISKS {
        if let Some(pci_dev) = get_pci_device_at(slot) {
            use flush_block_using_configured_queue;
            if flush_block_using_configured_queue(&pci_dev).is_err() {
                return Err(-5); // EIO
            }
        }
    }
    Ok(())
}

/// Get the BOOT PCI VirtIO GenDisk (slot 0, major 8).
pub fn get_pci_gen_disk() -> Option<&'static GenDisk> {
    get_pci_gen_disk_at(0)
}

/// Get the PCI VirtIO GenDisk in `slot` (major 8 + slot).
pub fn get_pci_gen_disk_at(slot: usize) -> Option<&'static GenDisk> {
    if slot >= MAX_PCI_BLK_DISKS || !pci_blk_slot_ready(slot) {
        return None;
    }
    // SAFETY: get_disk returns a valid raw pointer to a registered GenDisk.
    crate::drivers::hotpath::get_disk(8 + slot as u32).map(|ptr| unsafe { &*ptr })
}

/// Resolve a vdX disk NAME to its GenDisk ("vdb" -> slot 1). Returns None
/// for non-vdX names or slots with no registered disk.
pub fn get_pci_gen_disk_by_name(name: &str) -> Option<&'static GenDisk> {
    let b = name.as_bytes();
    if b.len() != 3 || b[0] != b'v' || b[1] != b'd' || !b[2].is_ascii_lowercase() {
        return None;
    }
    get_pci_gen_disk_at((b[2] - b'a') as usize)
}

/// PCI VirtIO-Blk interrupt handler (Modern VirtIO 1.0+)
///
/// Registered via request_irq. EOI (PLIC complete) is done by the IRQ framework.
///
/// Top half ONLY (the MMIO ISR discipline, now applied to PCI too): the ISR
/// acknowledges the device and raises the Block softirq. Every wait-queue
/// wakeup — async IoCompletion delivery AND the sync queues — happens in
/// `block_bh_handler` (softirq context, no virtio lock held). The old ISR
/// called wake_up_all() on both sync queues directly in hard-IRQ context:
/// each wake holds the wait-queue lock across wake_up_process (→ GRQ) for
/// EVERY waiter, a long IRQ-off window that fed the BLK↔waitqueue lock
/// convoy (VIRTIO-WQ-1) and stopped timer IRQs system-wide.
pub fn virtio_blk_interrupt_handler_pci(_irq: u32, dev_id: usize) -> crate::interrupt::IrqReturn {
    // dev_id is the disk SLOT (registered with IRQF_SHARED so several
    // virtio-blk functions can share a swizzled INTx line; the framework
    // demuxes per dev_id and each instance checks its own ISR register).
    let slot = dev_id;
    if slot >= MAX_PCI_BLK_DISKS {
        return crate::interrupt::IrqReturn::None;
    }
    // (Registry access via the audited accessor; the ISR read is the
    // device-side EOI — INV-MMIO-1..3.)
    if let Some(pci_device) = get_pci_device_at(slot) {
        // Read ISR status FIRST: per the virtio 1.1 spec the read drops
        // the device's interrupt line (device-side EOI for level-
        // triggered INTx). Skipping it caused immediate re-entry
        // storms once the IRQ line actually fires (review DRIV-H3).
        // Bit 0 = queue used, bit 1 = configuration change.
        if pci_device.isr_cfg_bar != 0 {
            let isr = crate::safety::mmio::MmioRegion::new(
                pci_device.isr_cfg_bar as usize,
                4,
            )
            .read_u32(0);
            if isr == 0 {
                // Not ours — spurious or another device on the shared line.
                return crate::interrupt::IrqReturn::None;
            }
        }
        // Deferred-wake discipline (R12-3): collect in the top half,
        // deliver in the bottom half. The Block softirq runs at
        // irq_exit() time, so wakeup latency stays bounded while the
        // hard-IRQ section stays minimal.
        VW_IRQS[slot].fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::Block as usize,
        );
    }
    crate::interrupt::IrqReturn::Handled
}

/// VirtIO-Blk interrupt handler (Legacy MMIO VirtIO) — top half only.
///
/// Acknowledges the device interrupt and defers completion processing
/// to the Block softirq bottom half.
/// Registered via request_irq. EOI is done by the IRQ framework.
pub fn virtio_blk_interrupt_handler(_irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    // INV-MMIO-1..3: the device's register window; registry access via
    // the audited get_device accessor.
    if let Some(device) = virtio_blk_get_device() {
        // MMIO VirtIO device
        // Read interrupt status (INTERRUPT_STATUS at 0x60)
        let mmio = crate::safety::mmio::MmioRegion::new(device.base_addr as usize, 0x120);
        let irq_status = mmio.read_u32(0x60);

        if irq_status != 0 {
            // Clear interrupt (INTERRUPT_ACK at 0x64)
            mmio.write_u32(0x64, irq_status);

            // Defer completion processing to Block softirq bottom half
            crate::interrupt::softirq::raise_softirq_irqoff(
                crate::interrupt::softirq::SoftirqIndex::Block as usize,
            );
            return crate::interrupt::IrqReturn::Handled;
        }
    }
    crate::interrupt::IrqReturn::None
}

/// Block softirq bottom half handler.
///
/// Processes completed VirtIO Block I/O descriptors deferred from
/// the interrupt handler. Runs in softirq context.
///
/// VIRTIO-WQ-1 discipline (virtio-blk ABBA fix): completed pendings are
/// COLLECTED under the MMIO virtqueue lock, the lock is dropped, and only
/// then are the IoCompletions delivered (complete() takes wait-queue locks
/// and wakes tasks). The final sync-queue wake_up_all()s also run here,
/// OUTSIDE every virtio lock — they moved out of the PCI hard-ISR top half
/// (see interrupt_handler_pci) and out from under the virtqueue guard.
pub fn block_bh_handler(_vec: usize) {
    // SAFETY: Runs in softirq context; VIRTIO_BLK is initialized. The irqsave
    // lock on virtqueue ensures mutual exclusion with hardirq handlers.
    // (Registry access via the audited get_device accessor; every ring
    // read routes through the typed volatile helpers.)
    {
        let mut mmio_collected = 0usize;
        if let Some(device) = virtio_blk_get_device() {
            // MMIO pending table holds at most MAX_PENDING_IO live entries,
            // so one collection array covers a full catch-up pass.
            let mut done: [Option<PendingIo>; MAX_PENDING_IO] =
                [const { None }; MAX_PENDING_IO];
            let mut collected = 0usize;
            {
                // Read current used ring index (irqsafe: runs in softirq, can
                // be preempted by hard IRQ that also takes this lock)
                let queue_guard = device.virtqueue.lock_irqsave();
                let _nest = VirtioLockNest::new();
                if let Some(ref queue) = *queue_guard {
                    let used_ring = queue.used_ring_ptr();
                    let used_idx = used_ring_idx(used_ring);
                    let last_processed = VIRTIO_MMIO_LAST_PROCESSED
                        .load(core::sync::atomic::Ordering::Acquire);

                    // Collect each newly completed descriptor's pending I/O,
                    // matched by DEVICE TRUTH (the completed chain's head
                    // descriptor id, read from the used ring) — the same
                    // discipline as the PCI walker. TOMBSTONEs (timed-out
                    // waiter / sync chain) are taken too — their used-ring
                    // entry releases the slot — but are skipped at delivery
                    // time below.
                    //
                    // The walk used to be purely positional (used-ring index
                    // i -> table slot i % MAX_PENDING_IO) on the assumption
                    // that the single-in-flight MMIO queue keeps completion
                    // order equal to submission order. That assumption
                    // breaks the moment ANY chain consumes a submission
                    // ordinal without producing a used-ring entry (a chain
                    // the device dropped, or a submit that failed after the
                    // ordinal was taken): every later positional take is
                    // then off by one and fires each pending one chain EARLY
                    // — with a later chain's OK status while the fired
                    // pending's own DMA never lands. bread_wait then
                    // publishes the uninit (zero) buffer as uptodate: the
                    // no-journal ext4 root-lookup zero-serve family. The
                    // head_desc match makes a misfire impossible: an entry
                    // is fired ONLY by the used-ring entry that names its
                    // own chain.
                    let queue_sz = queue.queue_size as usize;
                    let mut unmatched = 0usize;
                    let mut i = last_processed;
                    while i != used_idx {
                        core::sync::atomic::fence(
                            core::sync::atomic::Ordering::Acquire,
                        );
                        // The completed chain's head descriptor id
                        // (UsedElem.id; the device publishes it only after
                        // the chain — including its DMA writes — is done).
                        // SAFETY: ring position `i % queue_sz` is within the
                        // used ring.
                        let ring_pos = (i as usize) % queue_sz;
                        let entry_id = used_ring_elem_id(used_ring, ring_pos);
                        let ordinal_slot = i as usize % MAX_PENDING_IO;
                        let mut fired: Option<PendingIo> = None;
                        {
                            let mut table = VIRTIO_MMIO_PENDING.lock_irqsave();
                            let positional_match = match table[ordinal_slot]
                                .as_ref()
                            {
                                Some(p) => p.head_desc == entry_id,
                                None => false,
                            };
                            if positional_match {
                                fired = table[ordinal_slot].take();
                            } else {
                                // Out-of-order or slot skew: scan for the
                                // entry whose chain the device ACTUALLY
                                // completed. Tombstones match too (their
                                // used-ring entry releases the slot).
                                for e in table.iter_mut() {
                                    if let Some(p) = e {
                                        if p.head_desc == entry_id {
                                            fired = e.take();
                                            break;
                                        }
                                    }
                                }
                            }
                            // Same walker-vs-abandon delivery handshake as
                            // the PCI path (see IoCompletion::delivering):
                            // mark as under delivery BEFORE the table lock
                            // drops.
                            if let Some(p) = fired.as_ref() {
                                if !p.completion.is_null() {
                                    begin_delivery(p.completion);
                                }
                            }
                        }
                        match fired {
                            Some(p) => {
                                if collected < MAX_PENDING_IO {
                                    done[collected] = Some(p);
                                    collected += 1;
                                }
                                // Overflow cannot happen (table holds at
                                // most MAX_PENDING_IO Somes); keep the
                                // walker moving.
                            }
                            None => {
                                // No live pending claims this used-ring
                                // entry. With head_desc truthing this is a
                                // dispatch accounting hole (an untracked
                                // chain completed), never a silent
                                // misfire: count it loudly and move on so
                                // one bad entry cannot stall the queue.
                                unmatched += 1;
                                let n = VIRTIO_MMIO_UNMATCHED_EVENTS
                                    .fetch_add(
                                        1,
                                        core::sync::atomic::Ordering::AcqRel,
                                    )
                                    + 1;
                                if n == 1 || n % 4096 == 0 {
                                    crate::pr_err!(
                                        "virtio-blk mmio: used entry {} \
                                         (head {}) matched no pending — \
                                         dispatch accounting hole #{}",
                                        i,
                                        entry_id,
                                        n
                                    );
                                }
                            }
                        }
                        i = i.wrapping_add(1);
                    }
                    VIRTIO_MMIO_LAST_PROCESSED.store(used_idx,
                        core::sync::atomic::Ordering::Release);
                }
                // queue_guard drops HERE — no virtio lock is held below.
            }

            // Deliver outside the virtqueue lock (R12-3): free buffers and
            // complete, waking waiters without holding any virtio lock.
            for k in 0..collected {
                if let Some(pending) = done[k].take() {
                    // TOMBSTONE (timed-out waiter / sync chain): nothing to
                    // read, free (the io_buf was deliberately leaked), or
                    // fire into unwound memory.
                    if pending.completion.is_null() {
                        continue;
                    }
                    // Read response status (volatile: device-written)
                    let status = if !pending.resp_ptr.is_null() {
                        resp_status_byte(pending.resp_ptr as *const u8)
                    } else {
                        0
                    };
                    let io_status = if status == 0 { 0 } else { -5i32 };

                    // Free allocated buffers (pool-aware release)
                    io_buf_release(pending.header_ptr, pending.header_layout);
                    if pending.resp_layout.size() != 0
                        && pending.resp_ptr != pending.header_ptr
                    {
                        io_buf_release(pending.resp_ptr, pending.resp_layout);
                    }

                    // block:block_rq_complete — the async chain's
                    // softirq-side completion, geometry from submit.
                    if pending.trace_dev != 0 {
                        crate::trace::trace_block_rq_complete(
                            pending.trace_dev,
                            pending.trace_sector,
                            pending.trace_nr_sector,
                            io_status,
                            0,
                            if pending.is_write { "W" } else { "R" },
                        );
                    }

                    // Signal completion, stamped with the chain's DMA data
                    // pointer: bread_wait's zero-serve guard verifies it
                    // against the waiter's own buffer.
                    deliver_completion(pending.completion, io_status, pending.data_ptr);
                }
            }
            mmio_collected = collected;
        }
        // PCI async reads complete here too (raised by
        // interrupt_handler_pci); independent of the MMIO device so a
        // PCI-only boot still drains its pending table. The per-disk
        // walkers wake their own sync queues, gated on real progress.
        pci_process_async_completions();

        // FIX8 (lost-wakeup wedge), relocated from the PCI hard-ISR top
        // half: every sync waiter whose response byte landed between its
        // used-ring re-check and its schedule() needs a wake from the
        // completion path. Doing it here (softirq, no virtio lock held,
        // IRQs enabled) keeps the hard-IRQ section minimal and cannot
        // nest the wait-queue lock inside a virtio lock.
        //
        // GATED on real progress (the ftest01 herd fix): wake_up_all
        // costs a wake_up_process (GRQ lock) per waiter and every woken
        // waiter re-runs prepare_to_wait/finish_wait on this queue.
        // Firing the wake from EVERY Block softirq pass — including
        // passes raised by timer/other activity that consumed nothing —
        // turned six concurrent sync waiters into a permanent
        // wake/re-queue convoy that starved completion delivery itself
        // (freeze-dumped: both CPUs in the GRQ/CFS machinery, used ring
        // running 1-8 entries ahead of the walker, ftest01 wedged until
        // its 30s kill). The PCI queues' wakes live INSIDE
        // pci_process_async_completions_slot (gated on consumed > 0) so
        // process-context rescue walks wake sync waiters too.
        if mmio_collected > 0 {
            #[cfg(debug_assertions)]
            crate::drivers::hotpath::assert_no_virtio_lock(
                "VIRTIO_BLK_WAIT_QUEUE wake (BH)",
            );
            VIRTIO_BLK_WAIT_QUEUE.wake_up_all();
        }
    }
}

/// Enable VirtIO-Blk device interrupt
///
/// # Parameters
/// - `base_addr`: VirtIO device's MMIO base address
///
/// # Notes
/// Calculates corresponding IRQ number based on MMIO base address and enables it
pub fn virtio_blk_enable_device_interrupt(base_addr: u64) {
    // QEMU RISC-V virt platform:
    // - VirtIO devices start at 0x10001000
    // - Each device occupies 0x1000 bytes
    // - IRQ starts at 1, one IRQ per device
    const VIRTIO_MMIO_BASE: u64 =
        crate::arch::mm::memory_layout::mmio_alias(0x10001000);
    const VIRTIO_MMIO_SIZE: u64 = 0x1000;

    let slot = ((base_addr - VIRTIO_MMIO_BASE) / VIRTIO_MMIO_SIZE) as u32;
    let irq = (slot + 1) as u32;  // IRQ 1-8 correspond to slot 0-7

    // Structured line (serial-clean): the raw "virtio-blk: Registering
    // IRQ…" pr_info polluted the boot table on virtio-mmio boots.
    crate::print_status(
        "driver",
        &alloc::format!("virtio-blk MMIO vda IRQ {}", irq),
        true,
    );

    // Register handler via IRQ framework (unmasks automatically).
    // RT-plan R5: IRQF_NO_THREAD — the block completion queue opts out
    // of forced threading (every qd1 completion would pay the thread
    // wake on its critical path; the x86_64 MSI-X twin carries the
    // measured -28% 4k randwrite trade, run/RT-R5.md). `irqthread=all`
    // overrides opt-outs for measurement.
    crate::interrupt::request_irq(
        irq,
        virtio_blk_interrupt_handler,
        crate::interrupt::irqdesc::IRQF_NO_THREAD,
        "virtio-blk",
        base_addr as usize,
    ).ok();

    // (round-5: the device's `irq` field was written here once and
    // never read anywhere — dead store on a dead field, both deleted.)
}

// ════════════════ virtio::queue ═══════════════════════
// (was virtio/queue.rs)
// VirtIO virtual queue
//
// Queue implementation fully compliant with VirtIO specification
unsafe impl Send for VirtQueue {}
unsafe impl Sync for VirtQueue {}

// ---- Safe used-ring / response readers (safe-call discipline,
// INV-SAFECALL-1..3): the ONE audited volatile edge per shape; every
// poll loop and forensic printer in the virtio stack calls these. ----

/// Read the used ring's `idx` (device-written) volatily.
///
/// INV: `used_ring` originates from `VirtQueue::used_ring_ptr()` on a
/// queue whose vring is still allocated (the callers hold the queue
/// reference); offset 2 is the idx field of the v1.0 used ring.
pub fn used_ring_idx(used_ring: *const UsedRing) -> u16 {
    if used_ring.is_null() {
        return 0;
    }
    // SAFETY: the invariant above — a live vring's used ring; the u16
    // at +2 is within the allocation; volatile because the device
    // writes it (INV-DMA-3).
    unsafe { core::ptr::read_volatile((used_ring as usize + 2) as *const u16) }
}

/// Read a used-ring entry's descriptor id (device-written) volatily.
///
/// INV: as `used_ring_idx`; `slot` must be < queue_size (callers wrap
/// with `% queue_size`, which the device protocol also guarantees).
pub fn used_ring_elem_id(used_ring: *const UsedRing, slot: usize) -> u32 {
    if used_ring.is_null() {
        return u32::MAX;
    }
    // SAFETY: live used ring; the entry at +4+slot*8 is within the
    // allocation for slot < queue_size; volatile device-written memory.
    unsafe {
        core::ptr::read_volatile((used_ring as usize + 4 + slot * 8) as *const u32)
    }
}

/// True once the device has written THIS request's response byte.
///
/// INV: `resp_status` points at this request's response status byte
/// (device writes it exactly once on completion; initialized 0xFF at
/// submit) or is null (treated as not-done).
pub fn resp_done(resp_status: *const u8) -> bool {
    resp_status_byte(resp_status) != 0xFF
}

/// Volatile read of a request's response status byte (null reads as
/// the 0xFF "pending" sentinel). Same INV as [`resp_done`].
pub fn resp_status_byte(resp_status: *const u8) -> u8 {
    if resp_status.is_null() {
        return 0xFF;
    }
    // SAFETY: caller-owned response byte of a live request; volatile
    // so each poll re-reads DMA memory (INV-DMA-3).
    unsafe { core::ptr::read_volatile(resp_status) }
}

/// r-strand instrument (lat_fs conviction round, 2026-11-19): the sync
/// wait loop's phase counts, printed by the periodic dump. A stranded
/// waiter shows SLEWED (schedule returned many times but RESP stayed
/// pending = wake reached us yet our byte never landed) vs NEVER
/// (schedule never returned = the wake never reached the task; the
/// strand is scheduler-side).
pub static VW_WAIT_ENTER: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub static VW_WAIT_SCHED: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub static VW_WAIT_RESP: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub static VW_WAIT_TIMEOUT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);


impl VirtQueue {
    /// Create new VirtQueue (using contiguous memory layout)
    ///
    /// # Parameters
    /// - `queue_size`: Queue size (must be power of 2)
    /// - `queue_index`: Queue index (written when notifying device)
    /// - `queue_notify`: Queue notification register address
    /// - `interrupt_status`: Interrupt status register address
    /// - `interrupt_ack`: Interrupt acknowledge register address
    pub fn new(queue_size: u16, queue_index: u16, queue_notify: u64, interrupt_status: u64, interrupt_ack: u64) -> Option<Self> {
        Self::with_notify_width(queue_size, queue_index, queue_notify, interrupt_status, interrupt_ack, NotifyWidth::W16)
    }

    /// Create new VirtQueue with an explicit notify access width.
    pub fn with_notify_width(queue_size: u16, queue_index: u16, queue_notify: u64, interrupt_status: u64, interrupt_ack: u64, notify_width: NotifyWidth) -> Option<Self> {
        let desc_size = queue_size as usize * 16;
        let avail_size = 2 + 2 + queue_size as usize * 2 + 2;
        let used_size = 2 + 2 + queue_size as usize * 8 + 2;

        // VirtIO 1.0 specification: descriptor table, available ring, and used ring must be page-aligned (at least 4096 bytes)
        const PAGE_SIZE: usize = 4096;

        let desc_size_aligned = (desc_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let avail_size_aligned = (avail_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let used_size_aligned = (used_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        let total_size = desc_size_aligned + avail_size_aligned + used_size_aligned;

        // INV-DMA-1/2: one page-aligned tracked allocation; the carve
        // below checks alignment, bounds, and disjointness so the old
        // hand offset arithmetic is gone. Each v1.0 section starts on
        // its own page boundary; the ring arrays after each header are
        // carved too, so every device-shared byte belongs to a carved
        // range.
        let mut block = crate::safety::dma::DmaBlock::new(total_size, PAGE_SIZE)?;
        let (mut desc, off_desc) =
            block.carve_array_aligned::<Desc>(queue_size as usize, PAGE_SIZE)?;
        if off_desc != 0 {
            crate::println!("virtio: ERROR: vring desc carve not page-aligned!");
            return None;
        }
        let avail_off_expected = off_desc + desc_size_aligned;
        let (mut avail, off_avail) = block.carve_aligned::<AvailRing>(PAGE_SIZE)?;
        if off_avail != avail_off_expected {
            crate::println!("virtio: ERROR: vring avail carve offset wrong!");
            return None;
        }
        // avail ring entries + used_event (driver-written).
        let (_avail_ring, _) =
            block.carve_array::<u16>(queue_size as usize + 1)?;
        let used_off_expected = off_avail + avail_size_aligned;
        let (mut used, off_used) = block.carve_aligned::<UsedRing>(PAGE_SIZE)?;
        if off_used != used_off_expected {
            crate::println!("virtio: ERROR: vring used carve offset wrong!");
            return None;
        }
        // used ring entries + avail_event (device-written).
        let (_used_ring, _) =
            block.carve_array::<UsedElem>(queue_size as usize)?;
        let (_avail_event, _) = block.carve_array::<u16>(1)?;
        let expected_total = used_off_expected + used_size_aligned;
        if block.frontier_offset() > expected_total {
            crate::println!("virtio: ERROR: vring carve exceeded section math!");
            return None;
        }

        // INV-DMA-2: driver-owned init through the disjoint slots
        // (the struct below adopts them after init).
        for i in 0..queue_size as usize {
            desc[i] = Desc { addr: 0, len: 0, flags: 0, next: 0 };
        }
        avail.flags = 0;
        avail.idx = 0;
        used.flags = 0;
        used.idx = 0;

        Some(Self {
            queue_size,
            queue_index,
            queue_notify,
            notify_width,
            interrupt_status,
            interrupt_ack,
            vring: Some(Vring {
                // MPK phase 1: tag the AVAIL section's pages pkey=1
                // (the guard is a no-op returning 0 pages when
                // mpkguard is off). The section is page-isolated by
                // construction; the tagged leaves are in the shared
                // kernel-half tables (visible in every address space).
                // This is the ONLY tagging call — the window discipline
                // lives in DmaBlock's volatile accessors.
                block: {
                    #[allow(unused_mut)]
                    let mut b = block;
                    let _pages = b.set_pkey_range(
                        off_avail,
                        off_avail + avail_size_aligned,
                    );
                    b
                },
                desc,
                avail,
                used,
                off_avail,
                off_used,
            }),
            next_desc: AtomicU16::new(0),
            leaked_chains: AtomicU16::new(0),
            avail_shadow: AtomicU16::new(0),
            descs_held: AtomicU16::new(0),
        })
    }

    /// Free the vring allocation backing this queue.
    ///
    /// VirtQueue has no Drop impl (the long-lived configured queues are
    /// intentionally never freed), so transient per-call queues — the
    /// legacy read_block/write_block paths — MUST call this on every exit
    /// path or leak a page-granular vring per I/O (review BUG: "write_block
    /// desc 失败清理" — the allocation leak fired on the desc-failure early
    /// returns too).
    ///
    /// Now safe (INV-DMA-1): dropping the DmaBlock deallocates with the
    /// stored layout; the slots are zero-drop views, so drop order cannot
    /// use-after-free.
    ///
    /// Precondition (unchanged, still the caller's contract): the queue
    /// must not be registered with a device (no in-flight DMA).
    pub fn free_vring(&mut self) {
        // MPK phase 1: untag BEFORE the block drops — a freed tagged
        // page re-enters the allocator still carrying pkey=1 and every
        // later user faults (the transient per-call queues' path).
        if let Some(vr) = self.vring.as_mut() {
            vr.block.clear_pkey_range();
        }
        self.vring = None;
    }

    /// Get current available index
    pub fn get_avail(&self) -> u16 {
        match self.vring.as_ref() {
            // INV-DMA-3: device-visible ring index, bounds-checked
            // volatile read at the carve offset.
            Some(vr) => vr.block.read_volatile_at::<u16>(vr.off_avail + 2),
            None => 0,
        }
    }

    /// Get current used index
    pub fn get_used(&self) -> u16 {
        match self.vring.as_ref() {
            // INV-DMA-3: device-written used index.
            Some(vr) => vr.block.read_volatile_at::<u16>(vr.off_used + 2),
            None => 0,
        }
    }

    /// Get element from used ring
    ///
    /// # Parameters
    /// - `idx`: Index in used ring
    ///
    /// # Returns
    /// UsedElem containing descriptor ID and length
    pub fn get_used_elem(&self, idx: u16) -> Option<UsedElem> {
        let vr = self.vring.as_ref()?;
        // Used ring structure: flags (2) + idx (2) + ring (queue_size * 8)
        let slot = (idx % self.queue_size) as usize;
        Some(vr.block.read_volatile_at::<UsedElem>(vr.off_used + 4 + slot * 8))
    }

    /// Get last processed used index (for tracking)
    pub fn get_last_used(&self) -> u16 {
        // This should be maintained by driver, simplified implementation here
        self.get_used()
    }

    /// Notify device of new request
    // IRREDUCIBLE F-ROW (F-VIO-NOTIFY): the queue-notify store is the
// virtio device doorbell — W16 for virtio-pci, W32 for virtio-mmio
// (QEMU drops != 4-byte accesses there); reference kernel
// drivers/virtio/virtio_ring.c vring_notify. A register-width device
// store has no safe shape (the width IS the protocol).
pub fn notify(&self) {
        // Disable external interrupts during MMIO operations.
        // NOTE: SEIE is bit 9 of sie — beyond csrci's 5-bit immediate. The
        // old "csrci sie, 9" wrote mask 0b01001 (bits 0 and 3, both WPRI)
        // and masked nothing; use a register-based csrc with the real mask.
        #[cfg(feature = "riscv64")]
        let sie_backup: u64;
        #[cfg(feature = "riscv64")]
        // SAFETY: reading sie and clearing SEIE (bit 9) via a register-based
        // csrc is valid in S-mode; the mask register is a scratch local.
        unsafe {
            core::arch::asm!(
                "csrr {sie}, sie",
                "li {mask}, {seie}",
                "csrc sie, {mask}",
                sie = out(reg) sie_backup,
                mask = out(reg) _,
                seie = const 1 << 9,
            );
        }

        // RISC-V MMIO fence: fence w, o
        // This ensures all previous writes (to descriptor table, available ring)
        // are visible before the MMIO write to the notify register.
        // MMIO write fence: RISCV_FENCE(w, o)
        #[cfg(feature = "riscv64")]
        unsafe {
            core::arch::asm!("fence w, o");
        }

        // SAFETY: queue_notify is a valid MMIO address for the VirtIO notify register;
        // volatile write ensures the store reaches the device.
        unsafe {
            match self.notify_width {
                NotifyWidth::W16 => {
                    // virtio-pci notify capability (matches Linux iowrite16)
                    let queue_notify = self.queue_notify as *mut u16;
                    core::ptr::write_volatile(queue_notify, self.queue_index);
                }
                NotifyWidth::W32 => {
                    // virtio-mmio QueueNotify: QEMU drops any access != 4 bytes
                    let queue_notify = self.queue_notify as *mut u32;
                    core::ptr::write_volatile(queue_notify, self.queue_index as u32);
                }
            }
        }

        // RISC-V MMIO fence after write: fence iorw, iorw
        // This is the WRITE-side device fence (Linux __io_aw(): "fence
        // iorw, iorw"): the MMIO store must reach the device before any
        // subsequent memory or MMIO access by this hart. The old
        // "fence i, ir" is the READ-side ordering fence (__io_ar) used in
        // the wrong place — it ordered device INPUT against instruction
        // fetch and did not order the notify store at all (review ARCH:
        // fence i,ir 用反, weakly-ordered platform risk).
        #[cfg(feature = "riscv64")]
        unsafe {
            core::arch::asm!("fence iorw, iorw");
        }

        // Restore interrupts
        #[cfg(feature = "riscv64")]
        // SAFETY: writing sie CSR to restore the previous interrupt-enable state is valid on S-mode.
        unsafe {
            core::arch::asm!(
                "csrw sie, {sie}",
                sie = in(reg) sie_backup,
            );
        }
    }

    /// Wait for device to complete request
    pub fn wait_for_completion(&self, prev_used: u16) -> u16 {
        // Timeout value from config (in loop iterations, approximately microseconds)
        self.wait_for_completion_max(prev_used, crate::config::VIRTIO_QUEUE_TIMEOUT_US)
    }

    /// Wait for device to complete request with an explicit iteration budget.
    ///
    /// `max_iters` is a spin-loop iteration budget (~µs each under TCG).
    /// Callers that hold a lock_irqsave across this wait (e.g. the net TX
    /// path) MUST pass a small budget: every iteration runs with external
    /// interrupts disabled, so the budget directly bounds the worst-case
    /// IRQ-blackout window (review TIMING: xmit used 10M+50M iters — tens
    /// of seconds of disabled interrupts under TCG).
    pub fn wait_for_completion_max(&self, prev_used: u16, max_iters: u64) -> u16 {
        if self.vring.is_none() {
            return prev_used;
        }

        let mut timeout = max_iters;

        loop {
            // Use memory barrier to ensure read ordering
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

            let used_idx = self.get_used();

            if used_idx != prev_used {
                return used_idx;
            }

            core::hint::spin_loop();

            timeout -= 1;
            if timeout == 0 {
                return used_idx;
            }
        }
    }

    /// Get raw pointer to used ring (for interrupt-driven wait outside
    /// queue lock). INV: valid while the queue's vring is allocated;
    /// read it through [`used_ring_idx`]/[`used_ring_elem_id`].
    pub fn used_ring_ptr(&self) -> *const UsedRing {
        match self.vring.as_ref() {
            Some(vr) => vr.used.as_ptr(),
            None => core::ptr::null(),
        }
    }

    /// Kernel-side submission ordinal snapshot (VW forensic
    /// instrumentation): equals avail.idx under a healthy submit path.
    pub fn avail_shadow_snapshot(&self) -> u16 {
        self.avail_shadow.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Direct-console one-line snapshot of a stalling poller's world (the
    /// VW-POLLSTALL family): the poller's own response byte, the device's
    /// used-ring index, the caller's prev_used ordinal, the walker's
    /// position, the submission shadow, and the un-kicked chain count.
    /// Direct console (putchar_no_lock) because pr_err is silently dropped
    /// on the OH boots (lb70/lb71 lesson).
    ///
    /// Class split, one line:
    /// - used > prev_used, resp still 0xFF: the device completed entries
    ///   past our ordinal but never wrote OUR byte — completion attribution
    ///   or window-reuse corruption;
    /// - used == prev_used, shadow > used: chains are published but the
    ///   device never consumed them — lost kick (or a wedged ring);
    /// - used > prev_used with resp written is unreachable here (the loop
    ///   exits on the byte).
    #[allow(clippy::too_many_arguments)]
    fn vw_pollstall_print(
        blk_slot: usize,
        resp_status: *const u8,
        used_ring: *const UsedRing,
        prev_used: u16,
        tag: &[u8],
    ) {
        use crate::console::putchar_no_lock as putchar;
        fn pdec(mut v: u64) {
            use crate::console::putchar_no_lock as putchar;
            let mut buf = [0u8; 20];
            let mut n = 0usize;
            if v == 0 {
                putchar(b'0');
                return;
            }
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
            let mut i = n;
            while i > 0 {
                i -= 1;
                putchar(buf[i]);
            }
        }
        for &c in b"\nVW-" {
            putchar(c);
        }
        for &c in tag {
            putchar(c);
        }
        for &c in b" slot=" {
            putchar(c);
        }
        pdec(blk_slot as u64);
        let rv = resp_status_byte(resp_status);
        for &c in b" resp=0x" {
            putchar(c);
        }
        pdec(rv as u64);
        let used_idx = used_ring_idx(used_ring);
        for &c in b" used=" {
            putchar(c);
        }
        pdec(used_idx as u64);
        for &c in b" prev_used=" {
            putchar(c);
        }
        pdec(prev_used as u64);
        let (walker_last, unkicked) =
            crate::drivers::hotpath::pci_blk_lag_snapshot(blk_slot);
        for &c in b" walker=" {
            putchar(c);
        }
        pdec(walker_last as u64);
        let shadow = crate::drivers::hotpath::get_pci_device_queue_at(blk_slot)
            .map(|q| q.avail_shadow_snapshot())
            .unwrap_or(0);
        for &c in b" shadow=" {
            putchar(c);
        }
        pdec(shadow as u64);
        for &c in b" unkicked=" {
            putchar(c);
        }
        pdec(unkicked as u64);
        for &c in b" pc=0x" {
            putchar(c);
        }
        pdec(crate::interrupt::preempt::preempt_count() as u32 as u64);
        putchar(b'\n');
    }

    /// Wait for device to complete request using interrupt-driven sleep.
    ///
    /// Instead of busy-wait polling, the current task sleeps on a wait queue
    /// and is woken by the VirtIO interrupt handler when the device completes
    /// the request. The BKL is released during sleep and re-acquired on wakeup.
    ///
    /// Completion is tracked through THIS request's response byte
    /// (`resp_status`, initialized to 0xFF at submit and written by the
    /// device on completion). Waiting on the shared used-ring index alone is
    /// wrong with concurrent submitters: ANY other request's completion
    /// advances it, releasing this waiter while its own response is still
    /// 0xFF — the caller then reads a garbage status and frees the response
    /// buffer while the device still DMAs into it.
    ///
    /// Callers that hold preempt-disable state (spinlocks — e.g. the VFS
    /// mutation lock over an O_CREAT that reaches here through the journal
    /// write path) never sleep: switching such a task out parks it with the
    /// lock held, and every later acquirer spins with preemption disabled,
    /// so the sleeping holder can never be rescheduled — the jchurn VFS
    /// wedge. They poll instead; the device writes the response via DMA
    /// regardless of CPU IRQ state.
    ///
    /// Pointer contracts (INV-SAFECALL-2 boundary; reads route through
    /// [`used_ring_idx`]/[`resp_done`]):
    /// - `used_ring` must point to a valid VirtIO used ring
    /// - `wait_queue` must be the correct wait queue for this device's interrupt
    /// - `resp_status` must point to this request's response status byte and
    ///   stay valid until this returns
    pub fn wait_for_used_interruptible(
        used_ring: *const UsedRing,
        wait_queue: &crate::process::types::WaitQueueHead,
        prev_used: u16,
        resp_status: *const u8,
        blk_slot: usize,
    ) -> u16 {
        if used_ring.is_null() {
            return prev_used;
        }

        // Detect early boot phase: no current task or PID 0 (idle/boot thread).
        // During early boot (e.g., ext4 mount before scheduler/IRQ init),
        // interrupts are not enabled and there is no scheduler, so we must
        // use synchronous polling with a large timeout (matching the MMIO
        // path's budget). The normal path relies on interrupt-driven wakeup
        // with a 10s wall-clock deadline.
        let is_early_boot = match crate::sched::hotpath::current() {
            Some(task) if task.pid() != 0 => false,
            _ => true,
        };

        // Early boot and preempt-disabled callers both poll.
        let poll_only =
            is_early_boot || crate::interrupt::preempt::preempt_count() != 0;

        // POLL-PATH DEADLINE + PERIODIC RECOVERY (the lb66 installs stall):
        // a preempt-guarded poller cannot sleep, but it CAN burn its whole
        // iteration budget (and the caller's retry stack, 441s observed)
        // when the completion never lands. The sleep path escapes via its
        // 10s deadline + kick-and-walk leg; the poll path had NEITHER —
        // poll_only skipped the deadline entirely and its only "recovery"
        // was the caller's post-timeout retry, which re-poll the same dead
        // ring. Now the poll path gets the same discipline, wall-clock
        // based (read_time: TSC-derived, IRQ-independent — a deadline on
        // jiffies would never fire under an irqsave guard):
        //   - every 10 ms: re-kick the device (covers a lost/quiet kick —
        //     an un-kicked chain's completion NEVER arrives no matter how
        //     long we spin) and run the completion walker in this task's
        //     context (covers irq_exit softirq deferral: with a preempt
        //     guard held, irq_exit defers Block softirqs to ksoftirqd,
        //     and ksoftirqd cannot run under our guard on this CPU — the
        //     walker is the last-line consumer when no interrupt dispatch
        //     will ever come; process-context calls take the blocking
        //     acquire, exactly like the drain/rescue paths);
        //   - at the 10 s deadline: same terminal behavior as the sleep
        //     path — return the prev_used sentinel so the caller runs its
        //     bounded late-drain, flags the tombstone, and fails loudly.
        // Early-boot polling keeps the pure spin (no scheduler, no IRQ
        // machinery up yet; its budget stays the 5M-lap cap).
        const POLL_RECOVER_TICKS: u64 =
            10 * crate::config::TIMER_CLOCK_FREQ_HZ / 1000;
        const POLL_DEADLINE_TICKS: u64 = 10 * crate::config::TIMER_CLOCK_FREQ_HZ;
        let poll_recover = poll_only && !is_early_boot;
        let poll_t0 = if poll_recover {
            Some(crate::drivers::hotpath::timer::read_time())
        } else {
            None
        };
        let mut poll_next_recover = poll_t0.map(|t0| t0 + POLL_RECOVER_TICKS);

        // Early boot / lock holders: large timeout for reliable synchronous
        // polling. Under TCG a completion lands within ~10-20ms (device BH
        // runs on the timer cadence); the budget must comfortably exceed
        // that or the poll times out just before completion and the CALLER's
        // late-drain (which runs with the caller's locks held) burns seconds.
        // Normal path: bounded by the 10s wall-clock deadline below, NOT by
        // an iteration count — every Block softirq wakes the whole sync
        // queue, so a 5000-wake cap could expire in well under a second of
        // false wakes and convert healthy I/O into timeout+EIO storms (the
        // AC-2 "leaked multi-block read" failure family). The preempt-guard
        // poll is bounded by POLL_DEADLINE_TICKS above.
        let max_iterations: u64 = if poll_only {
            5_000_000
        } else {
            1 << 40
        };

        // 10s wall-clock deadline for the sleeping path (the
        // IoCompletion::wait discipline): a wakeup timer armed AT the
        // deadline guarantees schedule() returns even if no completion
        // ever wakes us, so a lost interrupt or a wedged device cannot
        // strand a synchronous waiter forever (one lost completion used
        // to strand it permanently — no retry, no timeout, no way out).
        let mut deadline = 0u64;
        let mut deadline_timer = 0u64; // 0 = not armed
        let mut poll_fallback = false; // timer budget exhausted: 1-jiffy re-arms
        // SLEEP-PATH RECOVER LEG (oh-speed, the bistable 10s boot cliff):
        // the waiter-side rescue below only runs when a WAKE arrives; a
        // fully lost interrupt (device completed, no wake ever comes)
        // left the sleeper stranded until the 10s deadline — measured on
        // the OH boots as one 10.00s EXT4-BIGLOCK hold (holder blocked in
        // a single sync read while EVERY other ext4 op queued behind the
        // global lock; samgr InitProfile spent 10037ms locked out; the
        // face hits ~1/8-1/14 boots and turns a 4.5s boot into 14.5s).
        // The poll path got a 10ms re-kick+walk leg (lb66); the sleep
        // path now gets the same discipline at a coarser cadence: a
        // periodic recover WAKE (timer) so the kick/walk/rescue below
        // runs every SLEEP_RECOVER_MS instead of only at the 10s edge.
        // The timer is armed per sleep and deleted on wake — a healthy
        // completion (interrupt wake in µs/ms) never observes it.
        const SLEEP_RECOVER_MS: u64 = 250;
        let mut recover_at = u64::MAX; // gate: only the !poll_only path arms it
        let mut wait_started = 0u64;
        if !poll_only {
            deadline = crate::drivers::hotpath::timer::get_jiffies()
                .saturating_add(crate::drivers::hotpath::timer::msecs_to_jiffies(10_000));
            let pid = crate::sched::hotpath::get_current_pid();
            deadline_timer = crate::timer::add_timer_wakeup(deadline, pid);
            if deadline_timer == 0 {
                poll_fallback = true;
            }
            let now = crate::drivers::hotpath::timer::get_jiffies();
            wait_started = now;
            recover_at = now
                .saturating_add(crate::drivers::hotpath::timer::msecs_to_jiffies(SLEEP_RECOVER_MS));
        }

        let result = 'wait_done: {
            for _iteration in 0..max_iterations {
                // Early boot / lock holder: pure spin-poll (no scheduler).
                if poll_only {
                    // SAFETY: resp_done reads the caller-owned response byte.
                    if resp_done(resp_status) {
                        break 'wait_done used_ring_idx(used_ring);
                    }

                    // Wall-clock recovery + deadline (see the constants
                    // block above): every 4096 laps read the TSC clock —
                    // a few ns of overhead per µs of spin — and at each
                    // 10ms boundary re-kick + walk + re-check; at the 10s
                    // deadline give up with the sentinel, exactly like
                    // the sleeping path's sync-deadline leg.
                    if let Some(next_recover) = poll_next_recover.as_mut() {
                        if _iteration & 0xFFF == 0 {
                            let now = crate::drivers::hotpath::timer::read_time();
                            if now >= *next_recover {
                                let t0 = poll_t0.unwrap_or(now);
                                if now.saturating_sub(t0) >= POLL_DEADLINE_TICKS {
                                    // Terminal: direct-console verdict line
                                    // (pr_err is dropped on the OH boots —
                                    // lb70/lb71 lesson), then the sentinel.
                                    // SAFETY: same reads as the loop above.
                                    Self::vw_pollstall_print(
                                        blk_slot,
                                        resp_status,
                                        used_ring,
                                        prev_used,
                                        b"POLLSTALL-DL",
                                    );
                                    break 'wait_done prev_used;
                                }
                                // Recovery pass: lost/quiet kick, or a
                                // completion the deferred softirq never
                                // consumed. Cheap when caught up.
                                crate::drivers::hotpath::pci_blk_kick(blk_slot);
                                crate::drivers::hotpath::pci_process_async_completions_slot(
                                    blk_slot,
                                );
                                if resp_done(resp_status) {
                                    break 'wait_done used_ring_idx(used_ring);
                                }
                                *next_recover = now + POLL_RECOVER_TICKS;
                            }
                        }
                    }

                    // lb66 installs hang instrument (2M laps ≈ 40% of the
                    // poll budget with no completion): one direct-console
                    // snapshot per call — the poller's own view of the
                    // rings splits the engine (used advanced past this
                    // request = delivery loss; chain still at_device =
                    // submission/kick loss; resp byte stuck at 0xFF with
                    // the ring drained = descriptor-window reuse).
                    if _iteration == 2_000_000 {
                        Self::vw_pollstall_print(
                            blk_slot,
                            resp_status,
                            used_ring,
                            prev_used,
                            b"POLLSTALL",
                        );
                    }
                    core::hint::spin_loop();
                    continue;
                }

                VW_WAIT_ENTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                // Normal path: get current task and sleep on wait queue.
                // The idle task has SchedPolicy::Idle which enqueue_task() ignores,
                // so sleeping would be a permanent deadlock.
                let current = match crate::sched::hotpath::current() {
                    Some(task) if task.pid() != 0 => task,
                    _ => {
                        core::hint::spin_loop();
                        continue;
                    }
                };

                // Fast check: our own request may have completed already.
                // SAFETY: resp_done reads the caller-owned response byte.
                if resp_done(resp_status) {
                    break 'wait_done used_ring_idx(used_ring);
                }

                // SLEEP-PATH RECOVER PASS (see the SLEEP_RECOVER_MS block
                // above): the completion has NOT landed and the recover
                // gate is due — re-kick the device (a lost/quiet kick's
                // completion never arrives no matter how long we sleep)
                // and run the completion walker in this task's context
                // (covers a used ring the deferred softirq never consumed
                // AND the lagged-completion freeze the waiter-side rescue
                // below handles per-wake — here it runs even with no wake
                // at all, because the recover TIMER woke us). Mirrors the
                // poll path's 10ms leg at this path's coarser cadence.
                // The forensic print fires only for waits already >=1s
                // outstanding (a genuinely anomalous IO under this rig's
                // µs-ms completion class) so a host-load hiccup cannot
                // spam the serial.
                if crate::drivers::hotpath::timer::get_jiffies() >= recover_at {
                    let now = crate::drivers::hotpath::timer::get_jiffies();
                    if now >= wait_started
                        + crate::drivers::hotpath::timer::msecs_to_jiffies(1_000)
                    {
                        crate::drivers::hotpath::vw_report("sync-recover");
                    }
                    crate::drivers::hotpath::pci_blk_kick(blk_slot);
                    crate::drivers::hotpath::pci_process_async_completions_slot(blk_slot);
                    // SAFETY: resp_done reads the caller-owned response byte.
                    if resp_done(resp_status) {
                        break 'wait_done used_ring_idx(used_ring);
                    }
                    recover_at = now.saturating_add(
                        crate::drivers::hotpath::timer::msecs_to_jiffies(SLEEP_RECOVER_MS),
                    );
                }

                // Register on the wait queue with the task state set to
                // INTERRUPTIBLE under the queue lock (prepare_to_wait), then
                // re-check the response and truly sleep in schedule().
                //
                // The old code only ran wait_queue.add() — the task state was
                // never set to INTERRUPTIBLE, so schedule() found it still
                // RUNNING and returned IMMEDIATELY: the "wait" was a 5000-lap
                // spin that could expire before a TCG completion (10-20ms)
                // landed. The caller then treated its still-healthy request as
                // timed out, leaked the 64B block, and the retry's fresh
                // descriptor chain overwrote the still-in-flight one — the
                // device silently dropped the corrupted chain and every later
                // waiter on those slots starved (observed: execve of a
                // buffer-cold binary, e.g. toybox cp, hanging forever in this
                // loop with 55s+ of CPU burned).
                //
                // prepare_to_wait closes the lost-wakeup race the old 256-lap
                // spin patched over: the state change and queue insertion are
                // atomic w.r.t. the IRQ handler's wake_up_all().
                wait_queue.prepare_to_wait(current, false, true);

                // Re-check AFTER registering: a completion whose interrupt
                // fired between the fast check above and prepare_to_wait is
                // caught here instead of sleeping on a queue nobody will wake.
                if resp_done(resp_status) {
                    VW_WAIT_RESP.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    wait_queue.finish_wait(current);
                    // R36-B2 compensation (IoCompletion::wait discipline): a
                    // racing fire may have enqueued us between prepare and
                    // finish — take ourselves back off the GRQ or nr_running
                    // stays inflated until our next context switch.
                    crate::sched::hotpath::dequeue_task(&*current);
                    break 'wait_done used_ring_idx(used_ring);
                }

                // Deadline check AFTER prepare_to_wait: we are about to sleep,
                // so this is the last point the deadline can abort without a
                // wake. The deadline timer (or the 1-jiffy fallback re-arm)
                // makes sure schedule() comes back at or after it.
                if (deadline_timer != 0 || poll_fallback)
                    && crate::drivers::hotpath::timer::get_jiffies() >= deadline
                {
                    wait_queue.finish_wait(current);
                    // Same R36-B2 compensation as above.
                    crate::sched::hotpath::dequeue_task(&*current);
                    // Final chance: a lost KICK (quiet batch submit nobody
                    // drained) recovers here — kick and walk once, then
                    // re-check before reporting the timeout sentinel.
                    crate::drivers::hotpath::vw_report("sync-deadline");
                    crate::drivers::hotpath::pci_blk_kick(blk_slot);
                    crate::drivers::hotpath::pci_process_async_completions_slot(blk_slot);
                    if resp_done(resp_status) {
                        break 'wait_done used_ring_idx(used_ring);
                    }
                    // prev_used sentinel: caller runs its late-drain and
                    // retry (R21-N2 — never free a buffer the device may
                    // still DMA into).
                    VW_WAIT_TIMEOUT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
break 'wait_done prev_used;
                }

                if poll_fallback {
                    // Re-arm a 1-jiffy wake so the deadline check above runs
                    // every jiffy even without a completion wake.
                    let pid = crate::sched::hotpath::get_current_pid();
                    let dl = crate::drivers::hotpath::timer::get_jiffies().saturating_add(1);
                    let id = crate::timer::add_timer_wakeup(dl, pid);
                    if id != 0 {
                        // R54: schedule() now restores the caller's SIE state;
                        // wait-path callers re-arm explicitly (semaphore.rs
                        // discipline) so ticks/IPIs reach this CPU across the
                        // wait loop.
                        crate::arch::cpu::restore_irq(true);
                        crate::sched::hotpath::schedule();
                        VW_WAIT_SCHED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        crate::timer::del_timer(id);
                    } else {
                        // Timer table STILL exhausted: never schedule()
                        // without a wake source — bounded spin with IRQs
                        // enabled (the completion walker can still run) and
                        // re-check.
                        wait_queue.finish_wait(current);
                        crate::arch::cpu::restore_irq(true);
                        for _ in 0..100_000 {
                            core::hint::spin_loop();
                        }
                        continue;
                    }
                } else {
                    // Sleep until woken by interrupt
                    // R54: schedule() now restores the caller's SIE state; wait-path callers re-arm explicitly (semaphore.rs discipline) so ticks/IPIs reach this CPU across the wait loop.
                    //
                    // SLEEP-PATH RECOVER WAKE (see the SLEEP_RECOVER_MS
                    // block): with ONLY the wait-queue as a wake source, a
                    // fully lost interrupt meant schedule() never returned
                    // until the 10s deadline timer. Arm a short one-shot
                    // wakeup so the recover pass above runs every
                    // SLEEP_RECOVER_MS even with no interrupt at all; the
                    // timer is deleted on wake (a healthy completion in
                    // µs/ms never observes it). Timer-table exhaustion
                    // degrades to the old behavior (wait-queue + 10s
                    // deadline) — never worse than before.
                    let rid = {
                        let pid = crate::sched::hotpath::get_current_pid();
                        let at = crate::drivers::hotpath::timer::get_jiffies().saturating_add(
                            crate::drivers::hotpath::timer::msecs_to_jiffies(SLEEP_RECOVER_MS),
                        );
                        crate::timer::add_timer_wakeup(at, pid)
                    };
                    crate::arch::cpu::restore_irq(true);
                    crate::sched::hotpath::schedule();
                    VW_WAIT_SCHED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if rid != 0 {
                        crate::timer::del_timer(rid);
                    }
                }

                // Remove from wait queue and restore RUNNING state, then loop
                // back to re-check the response.
                wait_queue.finish_wait(current);

                // Waiter-side rescue (the ftest01 lost-wakeup fix): a wake
                // whose response is still pending can mean the used ring is
                // ahead of the completion walker AND no interrupt is coming
                // (the device is idle; the walker exited with lag and the
                // softirq pending bits are clear — freeze-dumped live with
                // PENDING_LAST one behind used.idx and six waiters asleep).
                // Drain the lag ourselves before re-sleeping: this is cheap
                // when caught up (two loads) and converts a lost wake into
                // at most one extra loop. No-op on MMIO-only boots.
                crate::drivers::hotpath::pci_rescue_lagged_completions(blk_slot);
            }

            // Budget exhausted without seeing OUR response (only reachable
            // on the poll_only spin budgets): the prev_used sentinel.
            // Callers treat `new_used == prev_expected` as "request
            // possibly still in flight" and run their bounded late-drain
            // before erroring (R21-N2: never free a buffer the device may
            // still DMA into — a true timeout leaks the 64B block instead).
            // Returning the current used-ring index here would let
            // concurrent completions of OTHER requests mask our timeout
            // and skip that protection.
            prev_used
        };

        // Deadline-timer bookkeeping on EVERY exit path: an armed timer
        // left behind would fire a stray wakeup into whatever the task is
        // doing ten seconds later.
        if deadline_timer != 0 {
            crate::timer::del_timer(deadline_timer);
        }
        result
    }

    /// Wait for a specific descriptor to appear in the used ring.
    ///
    /// Unlike `wait_for_used_interruptible` which returns when ANY new entry
    /// appears, this function scans the used ring for the entry matching
    /// `expected_desc_id` and only returns when that specific request has
    /// been completed by the device. This is necessary when multiple requests
    /// may be in flight concurrently on the same VirtQueue.
    ///
    /// # Safety
    /// - `used_ring` must point to a valid VirtIO used ring
    /// - `wait_queue` must be the correct wait queue for this device's interrupt
    pub fn wait_for_desc_completion(
        used_ring: *const UsedRing,
        wait_queue: &crate::process::types::WaitQueueHead,
        start_idx: u16,
        expected_desc_id: u32,
        queue_size: u16,
    ) -> bool {
        if used_ring.is_null() || queue_size == 0 {
            return false;
        }

        // Detect early boot phase (same logic as wait_for_used_interruptible)
        let is_early_boot = match crate::sched::hotpath::current() {
            Some(task) if task.pid() != 0 => false,
            _ => true,
        };

        // Preempt-disabled callers must not sleep (same wedge discipline as
        // wait_for_used_interruptible): poll the used ring instead.
        let poll_only =
            is_early_boot || crate::interrupt::preempt::preempt_count() != 0;

        let max_iterations = if poll_only { 1_000_000 } else { 5000 };

        for _iteration in 0..max_iterations {
            // Read the current used ring index
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
            let used_idx = used_ring_idx(used_ring);

            // Scan all new entries since start_idx looking for our descriptor
            let mut scan_idx = start_idx;
            while scan_idx != used_idx {
                let ring_slot = scan_idx as usize % queue_size as usize;
                let entry_id = used_ring_elem_id(used_ring, ring_slot);
                if entry_id == expected_desc_id {
                    return true;  // Our descriptor was completed
                }
                scan_idx = scan_idx.wrapping_add(1);
            }

            // Early boot / lock holder: pure spin-poll
            if poll_only {
                core::hint::spin_loop();
                continue;
            }

            // Normal path: check scheduler availability
            let current = match crate::sched::hotpath::current() {
                Some(task) if task.pid() != 0 => task,
                _ => {
                    core::hint::spin_loop();
                    continue;
                }
            };

            // Register with the task state set to INTERRUPTIBLE under the
            // queue lock — same discipline as wait_for_used_interruptible
            // (a bare add() never slept: schedule() saw a RUNNING task and
            // returned immediately, turning the wait into a timed spin).
            wait_queue.prepare_to_wait(current, false, true);

            // Fast re-check after registering (lost-wakeup protection):
            // a completion that landed between the scan above and the
            // prepare_to_wait is caught here without sleeping.
            let used_idx2 = used_ring_idx(used_ring);
            let mut si = start_idx;
            while si != used_idx2 {
                let ring_slot = si as usize % queue_size as usize;
                let eid = used_ring_elem_id(used_ring, ring_slot);
                if eid == expected_desc_id {
                    wait_queue.finish_wait(current);
                    // R36-B2 compensation (same as wait_for_used_interruptible
                    // and the wait_event! macro): a racing wake between
                    // prepare_to_wait and this re-check may have enqueued us
                    // while we never slept — take ourselves back off the GRQ
                    // or nr_running stays inflated until our next context
                    // switch. dequeue_task is a no-op when nothing enqueued us.
                    crate::sched::hotpath::dequeue_task(&*current);
                    return true;
                }
                si = si.wrapping_add(1);
            }

            // Sleep until woken by interrupt
            // R54: schedule() now restores the caller's SIE state; wait-path callers re-arm explicitly (semaphore.rs discipline) so ticks/IPIs reach this CPU across the wait loop.
            crate::arch::cpu::restore_irq(true);
            crate::sched::hotpath::schedule();
            wait_queue.finish_wait(current);
        }

        false  // Timeout
    }

    /// Add descriptor chain to queue WITHOUT notifying the device.
    ///
    /// For batch submission: publish N chains quietly, then `notify()` once —
    /// every notify is an MMIO trap (device emulation under TCG), so a
    /// 128-block read-ahead window pays one kick instead of 128. Callers MUST
    /// notify before waiting for completions (or rely on the safety kick in
    /// the submit path when too many chains go unked).
    pub fn submit_quiet(&mut self, head_idx: u16) {
        // The slot ordinal comes from the kernel-side shadow, never from a
        // read-back of the shared ring: the vring lives in heap memory, and
        // a stray kernel write that smashed avail.idx made every later
        // submit publish a bogus index — the device then walked garbage
        /// ring slots, read a header-less chain, and stopped completing
        /// anything (permanent I/O wedge).
        let idx = self.avail_shadow.fetch_add(1, Ordering::AcqRel) as usize;
        let ring_idx = idx % self.queue_size as usize;
        let Some(vr) = self.vring.as_ref() else { return };
        // INV-DMA-2/3: bounds-checked volatile writes into the carved
        // avail section (the device reads this memory concurrently).

        // Memory barrier before writing to available ring
        core::sync::atomic::fence(Ordering::Release);

        // Write descriptor head index to available ring
        vr.block
            .write_volatile_at(vr.off_avail + 4 + ring_idx * 2, head_idx);

        // Memory barrier to ensure ring write completes before index update
        core::sync::atomic::fence(Ordering::Release);

        // Update available index (this signals to device that new
        // request is ready). Derived from the shadow — this also
        // REPAIRS a corrupted ring index on every submit.
        let new_idx = (idx as u16).wrapping_add(1);
        vr.block.write_volatile_at(vr.off_avail + 2, new_idx);
    }

    /// Add descriptor chain to queue and notify device
    pub fn submit(&mut self, head_idx: u16) {
        self.submit_quiet(head_idx);

        // Full memory barrier before notify
        core::sync::atomic::fence(Ordering::SeqCst);

        // Notify device
        Self::notify(self);
    }

    /// Get descriptor
    pub fn get_desc(&mut self, idx: u16) -> Option<Desc> {
        let vr = self.vring.as_ref()?;
        // INV-DMA-2: bounds-checked disjoint slot read.
        vr.desc.get(idx as usize).copied()
    }

    /// Allocate new descriptor (reclaims from used ring when possible).
    /// Assumes the chain being built consumes 3 descriptors (the virtio-blk
    /// header/data/resp layout) — see `alloc_desc_chain`.
    pub fn alloc_desc(&mut self) -> Option<u16> {
        self.alloc_desc_chain(3)
    }

    /// Allocate the head descriptor of a chain that will consume
    /// `chain_len` descriptors in total.
    ///
    /// R24 (R14 MED "RX 因 blk 的链限流只剩 2 缓冲"): the in-flight limiter
    /// was hard-wired to 3 descriptors per chain. virtio-net RX buffers are
    /// single-descriptor chains, so the RX queue was capped at 2 posted
    /// buffers (2*3+3 > 8) even though the queue holds 8. Passing the real
    /// chain length restores the full ring for 1-desc (RX) and 2-desc
    /// (net TX) users; blk keeps the 3-desc accounting via alloc_desc().
    pub fn alloc_desc_chain(&mut self, _chain_len: u16) -> Option<u16> {
        self.alloc_desc_run_checked(1)
    }

    /// Common admission + allocation core. Reserves `n` CONSECUTIVE
    /// descriptor ids (one free-running fetch_add — callers build chains
    /// that occupy exactly [base, base+n) mod queue_size) after both
    /// admission guards pass:
    ///
    /// 1. the legacy chains-in-flight guard (unchanged semantics: caps the
    ///    MMIO size-8 queue at 2 chains, keeps the PCI queue's coarse
    ///    chain-count bound);
    /// 2. the descriptors-held budget (`descs_held + n <= queue_size`) —
    ///    accurate for SG chains whose length the legacy guard cannot see.
    ///
    /// The held budget counts only PUBLISHED chains, so a reservation here
    /// that later fails to publish must not (and does not) touch it.
    fn alloc_desc_run_checked(&mut self, n: u16) -> Option<u16> {
        let used_idx = self.get_used();
        let avail_idx = self.get_avail();

        // Descriptor allocation is a free-running counter taken modulo
        // queue_size. Do NOT clamp it against the used-ring index: next_desc
        // counts DESCRIPTORS (2-3 per request) while used_idx counts
        // COMPLETED CHAINS — different domains. After next_desc's first
        // u16 wrap (~21k requests) the old clamp
        // `if next_desc < used_idx { next_desc = used_idx }` started to
        // fire and re-based the allocator on the completion watermark,
        // handing out the slots of still-in-flight chains; the overwritten
        // chain head then produced a header-less descriptor chain that the
        // device silently drops — used.idx stalls forever and every later
        // waiter starves (the journal-churn VFS wedge family).
        //
        // Slot safety comes from the in-flight guard below plus the fact
        // that live chains always occupy CONSECUTIVE descriptor ranges: at
        // most two chains may be in flight, so their ranges are adjacent
        // and disjoint (queue_size 8, chains of 2-3 descriptors).
        //
        // Check if all descriptors are in flight (avail - used >= queue_size)
        // Note: indices wrap at u16::MAX, not queue_size.
        // Discount chains that timed out and leaked: they will never
        // advance the used ring (or may do so arbitrarily late), but
        // counting them here forever would starve allocation after two
        // timeouts — the permanent-I/O-wedge cascade. SATURATING, never
        // wrapping: leaked only ever counts UNRESOLVED timed-out chains
        // (see note_timed_out_chain / resolve_leaked_chain), but a stale
        // overcount must clamp in_flight to zero (queue very idle), not
        // wrap it to ~65535 — the old wrapping_sub turned one accounting
        // drift into a PERMANENT "all descriptors in flight" refusal and
        // every submission failed with -5 forever after.
        let leaked = self.leaked_chains.load(Ordering::Acquire);
        let in_flight = avail_idx.wrapping_sub(used_idx).saturating_sub(leaked);
        // Bound by the WORST-CASE chain length (3: blk header/data/resp),
        // not the caller's: a 2-descriptor flush admitted under the old
        // per-caller guard could become the 4th concurrent chain and wrap
        // onto the oldest live chain's slots (R9-5/R10-3 family).
        if in_flight.saturating_mul(3).saturating_add(3) > self.queue_size {
            return None;
        }
        // SG-aware budget (see `descs_held`): the legacy guard above
        // assumes 3 descriptors per chain and under-counts whenever long
        // chains dominate the ring. This is the accurate gate.
        if n > 0 {
            let held = self.descs_held.load(Ordering::Acquire);
            if held.saturating_add(n) > self.queue_size {
                return None;
            }
        }

        let idx = self.next_desc.fetch_add(n, Ordering::AcqRel) % self.queue_size;
        Some(idx)
    }

    /// Allocate `n` consecutive descriptors for one multi-segment chain
    /// (header + n data descriptors + response is n+2 total; callers pass
    /// the TOTAL). Returns the first (head) descriptor index; the chain
    /// occupies [head, head+n) mod queue_size. Under the BLK lock, so the
    /// free-running counter cannot interleave with another chain build.
    pub fn alloc_desc_run(&mut self, n: u16) -> Option<u16> {
        if n == 0 || n > self.queue_size {
            return None;
        }
        self.alloc_desc_run_checked(n)
    }

    /// Mark `n` descriptors as held by a chain being PUBLISHED (pending
    /// store or sync tombstone). Called under the BLK lock after
    /// submit_quiet; pairs with the completion walker's release_descs.
    pub fn hold_descs(&self, n: u16) {
        self.descs_held.fetch_add(n, Ordering::AcqRel);
    }

    /// Release `n` descriptors whose chain's used-ring entry the walker
    /// consumed. Saturating: a stale overcount must clamp to zero, never
    /// wrap (the leaked_chains lesson applied symmetrically).
    pub fn release_descs(&self, n: u16) {
        let _ = self
            .descs_held
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                if v >= n {
                    Some(v - n)
                } else {
                    Some(0)
                }
            });
    }

    /// Snapshot of the held-descriptor count (diagnostics).
    pub fn descs_held_snapshot(&self) -> u16 {
        self.descs_held.load(Ordering::Acquire)
    }

    /// Reclaim descriptors that the device has finished processing.
    ///
    /// Called after I/O completion. Descriptor slots are recycled purely by
    /// the allocator's modulo cycle under the in-flight guard (see
    /// alloc_desc_chain for why next_desc must never be re-based on the
    /// used-ring index).
    pub fn reclaim_descs(&mut self) {
    }

    /// Record that a chain's wait TIMED OUT and its slots are leaked.
    ///
    /// The leaked count discounts the chain from the in-flight admission
    /// guard until its (possibly very late) completion lands; the PCI
    /// completion walker pairs every increment with resolve_leaked_chain
    /// when it consumes a TIMED-OUT tombstone's used-ring entry, so the
    /// counter tracks genuinely unresolved chains instead of accumulating
    /// forever.
    ///
    /// Descriptor-slot safety no longer needs the old next_desc window
    /// skip: timed-out PCI chains keep a tombstone in the pending table,
    /// and pci_pending_slot_reservable refuses to publish any chain whose
    /// descriptor window overlaps a live tombstone's. (The skip also raced
    /// concurrent chain builds: it advanced next_desc from process
    /// context while another CPU held the BLK lock mid-build, breaking
    /// the three-descriptor consecutiveness the window math relies on.)
    pub fn note_timed_out_chain(&self) {
        let n = self.leaked_chains.fetch_add(1, Ordering::AcqRel) + 1;
        // Cap warning: enough leaked chains to pin a meaningful slice of
        // the descriptor ring (queue_size/3 windows) means the device or
        // the completion path is losing I/Os — rate-limited, but loud.
        let cap = (self.queue_size / 3) as u16;
        if n >= cap && (n == cap || (n - cap) % 16 == 0) {
            crate::pr_warn!(
                "virtio-blk: {} chains leaked (timed out, unresolved) — \
                 completion loss suspected",
                n
            );
        }
    }

    /// Pair a note_timed_out_chain increment: a timed-out chain's
    /// completion finally landed (the walker consumed its tombstone's
    /// used-ring entry), so stop discounting it from the admission guard.
    /// Saturating: never decrements past zero.
    pub fn resolve_leaked_chain(&self) {
        let _ = self
            .leaked_chains
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                if v > 0 {
                    Some(v - 1)
                } else {
                    None
                }
            });
    }

    /// Reset descriptor allocator
    ///
    /// Note: This is UNSAFE under concurrent I/O and should only be used
    /// during single-threaded initialization.
    pub fn reset_desc_allocator(&mut self) {
        self.next_desc.store(0, Ordering::Release);
    }

    /// Set descriptor content
    pub fn set_desc(&mut self, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
        if let Some(vr) = self.vring.as_mut() {
            // INV-DMA-2: bounds-checked disjoint slot write (device
            // reads; the release fence orders the publish).
            if let Some(d) = vr.desc.get_mut(idx as usize) {
                *d = Desc { addr, len, flags, next };
                core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
            }
        }
    }

    /// Get descriptor table address (0 when freed)
    pub fn get_desc_addr(&self) -> u64 {
        self.vring
            .as_ref()
            .map(|vr| vr.desc.as_ptr() as *mut Desc as u64)
            .unwrap_or(0)
    }

    /// Get Available Ring address (0 when freed)
    pub fn get_avail_addr(&self) -> u64 {
        self.vring.as_ref().map(|vr| vr.avail.as_ptr() as u64).unwrap_or(0)
    }

    /// Get Used Ring address (0 when freed)
    pub fn get_used_addr(&self) -> u64 {
        self.vring.as_ref().map(|vr| vr.used.as_ptr() as u64).unwrap_or(0)
    }

    /// Get vring base address (0 when freed)
    pub fn get_vring_addr(&self) -> u64 {
        self.vring.as_ref().map(|vr| vr.block.addr() as u64).unwrap_or(0)
    }

    /// Get queue notification address
    pub fn get_notify_addr(&self) -> u64 {
        self.queue_notify
    }

    // ---- Typed ring access for drivers that program the vring
    // directly (GPU/input control queues). INV-DMA-2: bounds-checked
    // disjoint-slot views; device-visible ring-entry writes are
    // volatile. ----

    /// Bounds-checked mutable view of descriptor `idx` through a
    /// shared queue handle.
    ///
    /// The vring's descriptor table is driver-owned memory that the
    /// device only READS; submission is serialized by each driver's
    /// device lock (the same exclusion the pre-conversion raw-pointer
    /// code relied on). This is the ONE audited reborrow edge for that
    /// pattern (INV-DMA-2: bounds-checked disjoint slot).
    pub(crate) fn desc_mut(&self, idx: u16) -> Option<&mut Desc> {
        let vr = self.vring.as_ref()?;
        let base = vr.desc.as_ptr() as *mut Desc;
        if (idx as usize) < vr.desc.len() {
            // SAFETY: idx bounds-checked above; the slot's range is
            // disjoint from every other slot (INV-DMA-2); caller's
            // device lock provides the exclusion.
            Some(unsafe { &mut *base.add(idx as usize) })
        } else {
            None
        }
    }

    /// Mutable view of the available-ring header (driver-owned; same
    /// caller-lock discipline as [`desc_mut`]).
    pub(crate) fn avail_header_mut(&self) -> Option<&mut AvailRing> {
        let vr = self.vring.as_ref()?;
        let base = vr.avail.as_ptr();
        // SAFETY: disjoint slot (INV-DMA-2); the avail header is
        // driver-written; caller's device lock serializes.
        Some(unsafe { &mut *base })
    }

    /// Volatile write of avail ring slot `ring_idx` (device reads).
    pub(crate) fn write_avail_ring_slot(&self, ring_idx: usize, head: u16) {
        if let Some(vr) = self.vring.as_ref() {
            vr.block
                .write_volatile_at(vr.off_avail + 4 + ring_idx * 2, head);
        }
    }

    /// Volatile write of the avail header idx (publish).
    pub(crate) fn write_avail_idx(&self, new_idx: u16) {
        if let Some(vr) = self.vring.as_ref() {
            vr.block.write_volatile_at(vr.off_avail + 2, new_idx);
        }
    }

    /// Volatile read of the avail header idx (= [`get_avail`]).
    pub(crate) fn avail_idx(&self) -> u16 {
        self.get_avail()
    }
}


impl core::fmt::Display for VirtIOBlkResp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.status {
            0 => write!(f, "OK"),
            1 => write!(f, "IOERR"),
            2 => write!(f, "UNSUPPORTED"),
            _ => write!(f, "UNKNOWN({})", self.status),
        }
    }
}

pub mod req_type {
    pub const VIRTIO_BLK_T_IN: u32 = 0;
    pub const VIRTIO_BLK_T_OUT: u32 = 1;
    pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
}

pub mod queue_status {
    pub const VIRTIO_BLK_S_OK: u8 = 0;
    pub const VIRTIO_BLK_S_IOERR: u8 = 1;
    pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;
}

// ════════════════ virtio::offset ══════════════════════
// (was virtio/offset.rs)
// VirtIO PCI register offsets
/// VirtIO Common CFG register offsets (Modern VirtIO 1.0+)
pub const DEVICE_FEATURE_SELECT: u32 = 0;    // VIRTIO_PCI_COMMON_DFSELECT
pub const DEVICE_FEATURES: u32 = 4;          // VIRTIO_PCI_COMMON_DF
pub const DRIVER_FEATURE_SELECT: u32 = 8;    // VIRTIO_PCI_COMMON_GFSELECT
pub const DRIVER_FEATURES: u32 = 12;         // VIRTIO_PCI_COMMON_GF
pub const CONFIG_MSIX_VECTOR: u32 = 16;      // VIRTIO_PCI_COMMON_MSIX
pub const NUM_QUEUES: u32 = 18;              // VIRTIO_PCI_COMMON_NUMQ
/// Device status register
pub const DEVICE_STATUS: u32 = 20;           // VIRTIO_PCI_COMMON_STATUS
pub const CONFIG_GENERATION: u32 = 21;       // VIRTIO_PCI_COMMON_CFGGENERATION

/// Queue-related registers
pub const COMMON_CFG_QUEUE_SELECT: u32 = 22; // VIRTIO_PCI_COMMON_Q_SELECT
/// VIRTIO_PCI_COMMON_Q_NUM — the number of queues the device offers.
/// For virtio-gpu this is 3 (control, cursor, events/input): a driver
/// that only sets up control+cursor never sees the input queue.
pub const COMMON_CFG_QUEUE_NUM: u32 = 18; // VIRTIO_PCI_COMMON_Q_NUM (0x12)
pub const COMMON_CFG_QUEUE_SIZE: u32 = 24;   // VIRTIO_PCI_COMMON_Q_SIZE
pub const COMMON_CFG_QUEUE_MSIX_VECTOR: u32 = 26;  // VIRTIO_PCI_COMMON_Q_MSIX
pub const COMMON_CFG_QUEUE_ENABLE: u32 = 28; // VIRTIO_PCI_COMMON_Q_ENABLE
pub const COMMON_CFG_QUEUE_NOTIFY_OFF: u32 = 30;   // VIRTIO_PCI_COMMON_Q_NOFF

/// Queue address registers (64-bit, split into lo/hi)
pub const COMMON_CFG_QUEUE_DESC_LO: u32 = 32;  // VIRTIO_PCI_COMMON_Q_DESCLO
pub const COMMON_CFG_QUEUE_DESC_HI: u32 = 36;  // VIRTIO_PCI_COMMON_Q_DESCHI
pub const COMMON_CFG_QUEUE_DRIVER_LO: u32 = 40; // VIRTIO_PCI_COMMON_Q_AVAILLO
pub const COMMON_CFG_QUEUE_DRIVER_HI: u32 = 44; // VIRTIO_PCI_COMMON_Q_AVAILHI
pub const COMMON_CFG_QUEUE_DEVICE_LO: u32 = 48; // VIRTIO_PCI_COMMON_Q_USEDLO
pub const COMMON_CFG_QUEUE_DEVICE_HI: u32 = 52; // VIRTIO_PCI_COMMON_Q_USEDHI

/// VirtIO Interrupt CFG register offsets
pub const INTERRUPT_STATUS: u32 = 0x000;
pub const INTERRUPT_ACK: u32 = 0x004;

/// VirtIO Notify CFG register offsets
pub const QUEUE_NOTIFY: u32 = 0x000;

/// VirtIO device status bits
pub mod status {
    pub const ACKNOWLEDGE: u32 = 0x01;
    pub const DRIVER: u32 = 0x02;
    pub const FAILED: u32 = 0x80;
    pub const FEATURES_OK: u32 = 0x08;
    pub const DRIVER_OK: u32 = 0x04;
    pub const DEVICE_NEEDS_RESET: u32 = 0x40;
}

// ════════════════ virtio::virtio_pci ══════════════════
// (was virtio/virtio_pci.rs)
// VirtIO PCI transport layer
//
// Implements VirtIO device PCI transport (Modern VirtIO 1.0+)
/// One-shot latch for the out-of-window BAR warning in `bar_va` (x86_64):
/// four capability BARs per function would otherwise spam the log.
#[cfg(feature = "x86_64")]
fn device_alias_oob_warned() -> bool {
    use core::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    !WARNED.swap(true, Ordering::AcqRel)
}


// VirtioPCICap — the definition moved to drivers/types.rs (its only
// user here, VirtioPCINotifyCap's cap field, moved with it), so there
// is no re-export.

// VirtioPCINotifyCap — the definition moved to drivers/types.rs; no re-export.

/// PCI Capability list pointer
const PCI_CAPABILITY_LIST: u8 = 0x34;
const PCI_CAP_ID_VNDR: u8 = 0x09;  // Vendor-specific capability

/// VirtIO device status bits
pub mod virtio_pci_status {
    pub const ACKNOWLEDGE: u32 = 0x01;
    pub const DRIVER: u32 = 0x02;
    pub const FAILED: u32 = 0x80;
    pub const FEATURES_OK: u32 = 0x08;
    pub const DRIVER_OK: u32 = 0x04;
    pub const DEVICE_NEEDS_RESET: u32 = 0x40;
}


impl VirtIOPCI {
    /// The common-config register window (INV-MMIO-1: the BAR mapping
    /// proven at probe; every register offset below is a spec constant,
    /// so accesses are safe bounds-checked volatiles — INV-MMIO-2/3).
    fn common_cfg(&self) -> crate::safety::mmio::MmioRegion {
        crate::safety::mmio::MmioRegion::new(self.common_cfg_bar as usize, 0x100)
    }

    /// Find VirtIO PCI capability
    ///
    /// # Parameters
    /// - `cap_type`: Capability type to find
    ///
    /// # Returns
    /// Returns capability offset position, or 0 if not found
    fn find_virtio_capability(&self, cap_type: VirtIOCapType) -> Option<u8> {
        // (Config-space access through the mapped ECAM window; the
        // byte reads are safe accessors.)
        {
            // Start from capabilities list pointer
            let mut cap_ptr = self.pci_config.read_config_byte(PCI_CAPABILITY_LIST);
            let mut iterations = 0;
            const MAX_ITERATIONS: u8 = crate::config::VIRTIO_PCI_MAX_CAPABILITIES as u8;  // From config

            while cap_ptr != 0 && iterations < MAX_ITERATIONS {
                // Read capability ID
                let cap_id = self.pci_config.read_config_byte(cap_ptr);

                if cap_id == PCI_CAP_ID_VNDR {
                    // This is a vendor-specific capability, check type
                    let cfg_type = self.pci_config.read_config_byte(cap_ptr + 3);

                    if cfg_type == cap_type as u8 {
                        return Some(cap_ptr);
                    }
                }

                // Move to next capability
                let next_ptr = self.pci_config.read_config_byte(cap_ptr + 1);
                if next_ptr == cap_ptr {
                    // Loop detected, exit
                    crate::println!("virtio-pci: WARNING - capability loop detected at {}", cap_ptr);
                    break;
                }
                cap_ptr = next_ptr;
                iterations += 1;
            }

            if iterations >= MAX_ITERATIONS {
                crate::println!("virtio-pci: WARNING - too many capability iterations");
            }
        }

        None
    }

    /// Read VirtIO PCI capability info
    ///
    /// # Parameters
    /// - `cap_offset`: Capability offset in PCI configuration space
    ///
    /// # Returns
    /// (bar_index, bar_offset, length)
    fn read_virtio_cap(&self, cap_offset: u8) -> Option<(u8, u32, u32)> {
        // (Config-space access through the mapped ECAM window.)
        {
            // Read capability fields
            let bar = self.pci_config.read_config_byte(cap_offset + 4);

            // Read offset and length (little-endian, 32-bit fields per VirtIO PCI spec)
            let b0 = self.pci_config.read_config_byte(cap_offset + 8) as u32;
            let b1 = self.pci_config.read_config_byte(cap_offset + 9) as u32;
            let b2 = self.pci_config.read_config_byte(cap_offset + 10) as u32;
            let b3 = self.pci_config.read_config_byte(cap_offset + 11) as u32;
            let offset = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);

            let b0 = self.pci_config.read_config_byte(cap_offset + 12) as u32;
            let b1 = self.pci_config.read_config_byte(cap_offset + 13) as u32;
            let b2 = self.pci_config.read_config_byte(cap_offset + 14) as u32;
            let b3 = self.pci_config.read_config_byte(cap_offset + 15) as u32;
            let length = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);

            if bar >= 6 {
                // Reserved BAR value
                return None;
            }

            Some((bar, offset, length))
        }
    }
    /// Create new VirtIO PCI device
    ///
    /// # Parameters
    /// - `pci_base`: PCI configuration space base address (ECAM)
    pub fn new(pci_base: u64) -> Result<Self, &'static str> {
        // pci_base is a raw ECAM identity address; config access goes
        // through the kernel-half alias (see ecam_va).
        let pci_config = PCIConfig::new_at_ecam(pci_base);

        // Calculate PCI slot number (for IRQ calculation)
        // True PCI slot number: ECAM addr bit 15..19. Dividing by the ECAM
        // function size (0x1000) inflated the slot 8x and fed a wrong IRQ
        // swizzle (review DRIV NEW-1).
        #[cfg(feature = "riscv64")]
        let pci_slot =
            (((pci_base - crate::drivers::hotpath::RISCV_PCIE_ECAM_BASE) >> 15) & 0x1F) as u8;
        // x86_64: no ECAM slot derivation until the PCI host bridge driver
        // lands; the IRQ path is unreachable with an empty probe set.
        #[cfg(feature = "x86_64")]
        let pci_slot = 0u8;
        // aarch64: ECAM slot derivation identical to riscv64 (gpex model,
        // virt machine ECAM base differs — stage-3 probes the DTB PCI node)
        #[cfg(feature = "aarch64")]
        let pci_slot =
            (((pci_base - crate::arch::mm::memory_layout::PCIE_ECAM_BASE) >> 15) & 0x1F) as u8;

        // Verify vendor ID and device ID
        let vendor_id = pci_config.vendor_id();
        let device_id = pci_config.device_id();

        if vendor_id != vendor::RED_HAT {
            return Err("Not a VirtIO device (wrong vendor)");
        }

        match device_id {
            virtio_device::VIRTIO_BLK_MODERN => {
                // VirtIO block device (Modern VirtIO 1.0+)
            }
            virtio_device::VIRTIO_BLK => {
                // VirtIO block device (Legacy/Transitional)
                // Try using modern VirtIO 1.0 interface (transitional devices support both modes)
            }
            virtio_device::VIRTIO_NET => {
                // VirtIO network device
            }
            virtio_device::VIRTIO_GPU => {
                // VirtIO GPU device
            }
            virtio_device::VIRTIO_NET_MODERN | virtio_device::VIRTIO_INPUT => {
                // Modern net / input devices (accepted alongside their
                // transitional IDs; 0x1052 used to be rejected outright,
                // making /dev/input unproducible — review DRIV NEW-2).
            }
            _ => {
                if device_id != 0 {
                    return Err("Unknown VirtIO device");
                }
            }
        }

        // Enable bus master and memory space access
        pci_config.enable_bus_master();

        // Create temporary instance to use capability scanning methods
        let temp_device = Self {
            pci_config,
            pci_slot,  // Add pci_slot here
            common_cfg_bar: 0,
            common_cfg_offset: 0,
            device_cfg_bar: 0,
            device_cfg_offset: 0,
            notify_cfg_bar: 0,
            notify_cfg_offset: 0,
            notify_off_multiplier: 0,
            isr_cfg_bar: 0,
            isr_cfg_offset: 0,
            base_addr: 0,
            blk_slot: 0,
            msix_table_va: 0,
            msix_cap_off: 0,
        };

        // ========== Scan VirtIO PCI capabilities ==========
        // 1. Find Common CFG capability
        let (common_bar, common_offset, _) = match temp_device.find_virtio_capability(VirtIOCapType::CommonCfg) {
            Some(cap_offset) => {
                match temp_device.read_virtio_cap(cap_offset) {
                    Some(info) => info,
                    None => return Err("Failed to read Common CFG capability"),
                }
            }
            None => return Err("Common CFG capability not found (not a Modern VirtIO device)"),
        };

        // 2. Find Notify CFG capability
        let (notify_bar, notify_offset, _) = match temp_device.find_virtio_capability(VirtIOCapType::NotifyCfg) {
            Some(cap_offset) => {
                match temp_device.read_virtio_cap(cap_offset) {
                    Some(info) => info,
                    None => return Err("Failed to read Notify CFG capability"),
                }
            }
            None => return Err("Notify CFG capability not found"),
        };

        // 2.5. Find ISR CFG capability (required for interrupt status)
        let (isr_bar, isr_offset, _) = match temp_device.find_virtio_capability(VirtIOCapType::IsrCfg) {
            Some(cap_offset) => {
                match temp_device.read_virtio_cap(cap_offset) {
                    Some(info) => info,
                    None => return Err("Failed to read ISR CFG capability"),
                }
            }
            None => return Err("ISR CFG capability not found"),
        };

        // 3. Find Device CFG capability (optional)
        let (device_bar, device_offset, _) = temp_device.find_virtio_capability(VirtIOCapType::DeviceCfg)
            .and_then(|cap_offset| temp_device.read_virtio_cap(cap_offset))
            .unwrap_or((0xFF, 0, 0));  // 0xFF indicates not present

        // ========== PCI BAR address assignment ==========
        // VirtIO PCI devices require kernel to assign BAR addresses
        // Use fixed MMIO region: 0x40000000 - 0x50000000 (256MB)
        #[cfg(feature = "riscv64")]
        const PCI_MMIO_BASE: u64 = 0x40000000;
        // aarch64/GPEX: no firmware ran (direct -kernel boot), so the
        // kernel assigns BARs like riscv64 — from the DTB-declared
        // 32-bit PCI MMIO window (ranges: 0x10000000..0x3eff0000). The
        // window sits below RAM base, so every assigned BAR resolves
        // through the low kernel-half MMIO alias with no extra mapping.
        #[cfg(feature = "aarch64")]
        const PCI_MMIO_BASE: u64 = 0x1000_0000;
        // x86_64/q35: the BIOS has already enumerated PCI and assigned BARs
        // inside the 32-bit MMIO hole; re-assigning them at a fixed window
        // would fight the firmware (and the old window overlaps the kernel
        // heap at phys 0x40000000). Keep the firmware values: BAR_ASSIGN_BASE
        // is only used for size probing fallback.
        #[cfg(feature = "x86_64")]
        const PCI_MMIO_BASE: u64 = 0xfd000000;

        // Use global static variable to track MMIO offset, avoiding address conflicts between devices
        use core::sync::atomic::{AtomicU64, Ordering};
        static MMIO_OFFSET: AtomicU64 = AtomicU64::new(0);

        let mut mmio_offset = MMIO_OFFSET.load(Ordering::SeqCst);

        // Collect BAR indices to assign (deduplicated)
        let mut bars_to_assign = alloc::vec::Vec::new();
        bars_to_assign.push(common_bar);
        if notify_bar != common_bar {
            bars_to_assign.push(notify_bar);
        }
        if isr_bar != common_bar && isr_bar != notify_bar {
            bars_to_assign.push(isr_bar);
        }
        if device_bar != 0xFF && device_bar != common_bar && device_bar != notify_bar && device_bar != isr_bar {
            bars_to_assign.push(device_bar);
        }

        // DIO-CONC: locate the standard MSI-X capability (id 0x11) and
        // include its table BAR in the assignment set so the table is
        // mapped/programmed alongside the virtio modern BARs. The cap is
        // optional; without it the function stays on its arch's legacy
        // interrupt path.
        let msix_cap = temp_device.find_pci_capability(0x11);
        let msix_table_bir = msix_cap.map(|cap| {
            let bir = (pci_config.read_config_dword(cap + 4) & 0x7) as u8;
            if !bars_to_assign.contains(&bir) {
                bars_to_assign.push(bir);
            }
            bir
        });

        // Store assigned BAR info
        let mut assigned_bars: alloc::collections::btree_map::BTreeMap<u8, PCIBAR> = alloc::collections::btree_map::BTreeMap::new();

        // Assign address for each BAR
        for &bar_idx in &bars_to_assign {
            // Probe BAR size
            let bar_size = pci_config.probe_bar_size(bar_idx);

            // Calculate aligned address
            let aligned_addr = if mmio_offset % bar_size != 0 {
                ((mmio_offset / bar_size) + 1) * bar_size
            } else {
                mmio_offset
            };

        #[cfg(any(feature = "riscv64", feature = "aarch64"))]
        let bar_addr = PCI_MMIO_BASE + aligned_addr;

            // x86_64: use the firmware-assigned address verbatim.
            #[cfg(feature = "x86_64")]
            let bar_addr = {
                let _ = (mmio_offset, aligned_addr); // silence unused on x86
                pci_config.read_bar(bar_idx).base_addr
            };

        // Write BAR address and store returned PCIBAR object
        #[cfg(any(feature = "riscv64", feature = "aarch64"))]
        match pci_config.assign_bar(bar_idx, bar_addr) {
                Ok(bar_obj) => {
                    mmio_offset = aligned_addr + bar_size;
                    assigned_bars.insert(bar_idx, bar_obj);
                }
                Err(e) => {
                    crate::println!("virtio-pci: ERROR - Failed to assign BAR{}: {}", bar_idx, e);
                    return Err("Failed to assign PCI BAR");
                }
            }

            // x86_64: the firmware BAR is already programmed; keep it.
            #[cfg(feature = "x86_64")]
            {
                let bar_obj = pci_config.read_bar(bar_idx);
                if bar_obj.base_addr == 0 {
                    crate::println!("virtio-pci: BAR{} unassigned by firmware", bar_idx);
                    return Err("Firmware left PCI BAR unassigned");
                }
                // 64-bit BARs land ABOVE 4GB in QEMU's PCI window (the
                // 32-bit hole fills up with many devices). Under KVM the
                // q35 host bridge places them FAR beyond the fixed
                // DEVICE_ALIAS64 window (observed 0xe0000008000 = 14TiB
                // with the full OH device set; TCG on the same machine
                // stayed under 1TB), so the old identity ioremap —
                // boot-root low half only, invisible under every user
                // CR3 — KERNPANICed the first notify MMIO from a user
                // task. ioremap the BAR's full probed size into the
                // kernel-half arena instead (K1-BAR): the mapping lands
                // in the static root before the first user mm exists,
                // and device_alias resolves it under any CR3.
                if bar_obj.base_addr >= 0x1_0000_0000
                    && crate::arch::mm::memory_layout::device_alias_fixed(bar_obj.base_addr)
                        .is_none()
                {
                    // read_bar does not populate .size; bar_size was probed
                    // above in this loop. A zero probe (BAR unimplemented?)
                    // still gets a 2MB chunk — cheap and always sufficient
                    // for virtio modern capabilities living in this BAR.
                    let map_size = if bar_size > 0 { bar_size } else { 0x20_0000 };
                    match crate::arch::mm::mmu_init::ioremap_bar_kernel(
                        bar_obj.base_addr,
                        map_size,
                    ) {
                        Some(va) => {
                            crate::println!(
                                "virtio-pci: BAR {:#x}+{:#x} -> ioremap arena {:#x}",
                                bar_obj.base_addr, map_size, va
                            );
                        }
                        None => {
                            crate::pr_err!(
                                "virtio-pci: BAR {:#x} above-4G ioremap FAILED \
                                 (registry/arena exhausted)",
                                bar_obj.base_addr
                            );
                        }
                    }
                }
                assigned_bars.insert(bar_idx, bar_obj);
            }
        }

        // Update global MMIO offset, avoiding next device address conflict
        MMIO_OFFSET.store(mmio_offset, Ordering::SeqCst);

        // ========== Use assigned BAR info ==========
        // BAR base_addr values are hardware addresses in the PCI MMIO
        // window (riscv64: 0x40000000+, identity-mapped). Translate them
        // to the kernel-half MMIO alias before any dereference — a user
        // MAP_FIXED mapping may replace the low identity copy in its own
        // address space (gjs cage over 0x0c000000 PLIC did exactly that
        // and KERNPANICed the PLIC claim read).
        #[cfg(any(feature = "riscv64", feature = "aarch64"))]
        fn bar_va(bar: u64) -> u64 {
            crate::arch::mm::memory_layout::mmio_alias(bar)
        }
        #[cfg(feature = "x86_64")]
        fn bar_va(bar: u64) -> u64 {
            // Linux ioremap discipline: BARs are never dereferenced at
            // their firmware-assigned physical addresses — all runtime
            // MMIO (common cfg, notify, ISR reads from interrupt context)
            // goes through the kernel-half device alias.
            let va = crate::arch::mm::memory_layout::device_alias(bar);
            if va == bar
                && bar >= 0x1_0000_0000
                && !device_alias_oob_warned()
            {
                // Out-of-window above-4G BAR: the identity fallback only
                // translates under the BOOT root CR3. The first access
                // from interrupt context under a USER CR3 (intx_dispatch
                // ISR read) would KERNPANIC — say so now, at the source.
                crate::pr_err!(
                    "virtio-pci: BAR {:#x} outside every device-alias window; \
                     MMIO will fault under user CR3 — enlarge the window",
                    bar
                );
            }
            va
        }

        let common_bar_obj = assigned_bars.get(&common_bar)
            .ok_or("Common CFG BAR not assigned")?;
        if common_bar_obj.bar_type != BARType::MemoryMapped {
            return Err("Common CFG BAR is not memory mapped");
        }
        let common_cfg_bar = bar_va(common_bar_obj.base_addr);

        let notify_bar_obj = assigned_bars.get(&notify_bar)
            .ok_or("Notify CFG BAR not assigned")?;
        if notify_bar_obj.bar_type != BARType::MemoryMapped {
            return Err("Notify CFG BAR is not memory mapped");
        }
        let notify_cfg_bar = bar_va(notify_bar_obj.base_addr);

        let device_cfg_bar = if device_bar != 0xFF {
            match assigned_bars.get(&device_bar) {
                Some(bar_obj) if bar_obj.bar_type == BARType::MemoryMapped => {
                    bar_va(bar_obj.base_addr)
                }
                _ => 0,
            }
        } else {
            0
        };

        // Extract ISR CFG BAR (critical for interrupt status reading)
        let isr_cfg_bar = match assigned_bars.get(&isr_bar) {
            Some(bar_obj) if bar_obj.bar_type == BARType::MemoryMapped => {
                bar_va(bar_obj.base_addr)
            }
            _ => return Err("ISR CFG BAR not assigned or not memory mapped"),
        };

        // ========== Read notify_off_multiplier ==========
        // From Notify CFG capability offset 16 (notify_off_multiplier field)
        // notify_off_multiplier is part of Notify CFG capability structure, located in PCI configuration space
        let notify_off_multiplier = match temp_device.find_virtio_capability(VirtIOCapType::NotifyCfg) {
            Some(cap_offset) => {
                // notify_off_multiplier at capability structure offset 16
                pci_config.read_config_dword(cap_offset + 16)
            }
            None => 0,
        };

        // DIO-CONC: resolve the MSI-X table VA (BAR base + capability
        // offset) when the cap was found and its BAR is memory-mapped.
        let msix_table_va = match (msix_cap, msix_table_bir) {
            (Some(cap), Some(bir)) => {
                let table_off = pci_config.read_config_dword(cap + 4) & 0xFFFF_FFF8;
                match assigned_bars.get(&bir) {
                    Some(bar_obj) if bar_obj.bar_type == BARType::MemoryMapped => {
                        bar_va(bar_obj.base_addr) + table_off as u64
                    }
                    _ => 0,
                }
            }
            _ => 0,
        };

        let dev = Self {
            pci_config,
            pci_slot,
            common_cfg_bar: common_cfg_bar + common_offset as u64,
            common_cfg_offset: common_offset,
            device_cfg_bar: device_cfg_bar + device_offset as u64,
            device_cfg_offset: device_offset,
            // Critical fix: notify_cfg_bar should be pure BAR base address, without offset
            // get_notify_addr will add queue_index * multiplier when used
            notify_cfg_bar: notify_cfg_bar,
            notify_cfg_offset: notify_offset,
            notify_off_multiplier,
            isr_cfg_bar: isr_cfg_bar + isr_offset as u64,
            isr_cfg_offset: isr_offset,
            base_addr: common_cfg_bar + common_offset as u64,  // Use Common CFG as primary access address
            blk_slot: 0,
            msix_table_va,
            msix_cap_off: msix_cap.unwrap_or(0),
        };

        // x86_64: every constructed function goes on the shared INTx
        // dispatcher's poll list NOW, ack-only (see register_intx_ack).
        // Construction precedes every device operation (feature writes,
        // queue setup, DRIVER_OK), so no window exists where the function
        // can assert the shared line before its ISR is reachable.
        #[cfg(feature = "x86_64")]
        dev.register_intx_ack();

        Ok(dev)
    }

    /// x86_64/q35: put every live virtio-pci function on the shared INTx
    /// dispatcher's poll list IMMEDIATELY, with an ack-only default
    /// service that drivers may upgrade later
    /// (`enable_pci_intx_irq`).
    ///
    /// Why this is not optional: virtio-pci INTx is level-triggered and
    /// shared. The first registered device on a PIC line unmasks it, and
    /// from then on ANY function on that line can assert it. A function
    /// missing from the poll list asserts, nobody reads its ISR
    /// capability (the virtio device-side EOI), the dispatcher EOIs the
    /// PIC while the level is still high, and the line redelivers forever
    /// — an interrupt storm that starves every task on the CPU (observed:
    /// virtio-gpu's first eventq completion after DRIVER_OK wedged PID 1
    /// before its first syscall on the Ubuntu amd64 image; riscv64 never
    /// saw this because the PLIC masks IRQs nobody requested).
    #[cfg(feature = "x86_64")]
    pub fn register_intx_ack(&self) {
        let int_pin = self.pci_config.read_config_byte(0x3D);
        let int_line = self.pci_config.read_config_byte(0x3C);
        if int_pin == 0 || int_line >= 16 {
            crate::pr_warn!(
                "virtio-pci: INTx unprogrammed (pin {} line {})",
                int_pin,
                int_line
            );
            return;
        }
        if intx::register(int_line, self.isr_cfg_bar, intx_ack_only_service) {
            crate::pr_debug!(
                "virtio-pci: INTx ack-registered pin {} line {} (ISR @{:#x})",
                int_pin,
                int_line,
                self.isr_cfg_bar
            );
        }
    }

    /// Reset device
    pub fn reset_device(&self) {
        self.common_cfg().write_u32(0x14, 0);
    }

    /// Set device status
    pub fn set_status(&self, status: u32) {
        self.common_cfg().write_u32(0x14, status);
    }

    /// Get device status
    pub fn get_status(&self) -> u32 {
        self.common_cfg().read_u32(0x14)
    }

    /// Read device features
    ///
    /// VirtIO 1.0 PCI specification:
    /// - 0x00: device_feature_select (write-only) - select feature bit set
    /// - 0x04: device_feature (read-only) - actual feature bits
    pub fn read_device_features(&self) -> u32 {
        let cfg = self.common_cfg();
        // First write 0 to device_feature_select to select feature bits 0-31
        cfg.write_u32(0x00, 0u32);
        // Then read actual feature bits from device_feature
        cfg.read_u32(0x04)
    }

    /// Write driver features
    ///
    /// VirtIO 1.0 PCI specification:
    /// - 0x08: driver_feature_select (write-only) - select feature bit set
    /// - 0x0C: driver_feature (write-only) - actual feature bits
    pub fn write_driver_features(&self, features: u32) {
        let cfg = self.common_cfg();
        // First write 0 to driver_feature_select to select feature bits 0-31
        cfg.write_u32(0x08, 0u32);
        // Then write to driver_feature
        cfg.write_u32(0x0C, features);

        // Word 1: acknowledge VIRTIO_F_VERSION_1 (bit 32). Modern-only
        // devices (disable-legacy=on) reject negotiation without it.
        cfg.write_u32(0x08, 1u32);
        cfg.write_u32(0x0C, 1u32);
    }

    /// Setup queue
    pub fn setup_queue(&self, queue_index: u16, virt_queue: &VirtQueue) -> Result<(), &'static str> {
        let cfg = self.common_cfg();
        // Writing queue_select selects the queue register set per spec.
        cfg.write_u16(COMMON_CFG_QUEUE_SELECT as usize, queue_index);

        {
            let queue_max_size = cfg.read_u16(COMMON_CFG_QUEUE_SIZE as usize);
            if queue_max_size == 0 {
                return Err("Queue not available");
            }

            // Use maximum queue size supported by device
            let _queue_size = queue_max_size;
        }

        // Get descriptor table, available ring, used ring physical addresses
        let desc_addr = virt_queue.get_desc_addr();
        let avail_addr = virt_queue.get_avail_addr();
        let used_addr = virt_queue.get_used_addr();

        // Convert to physical addresses
        let desc_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(desc_addr)
        ).0;
        let avail_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(avail_addr)
        ).0;
        let used_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(used_addr)
        ).0;

        cfg.write_u32(COMMON_CFG_QUEUE_DESC_LO as usize, (desc_phys & 0xFFFFFFFF) as u32);
        cfg.write_u32(COMMON_CFG_QUEUE_DESC_HI as usize, (desc_phys >> 32) as u32);

        cfg.write_u32(COMMON_CFG_QUEUE_DRIVER_LO as usize, (avail_phys & 0xFFFFFFFF) as u32);
        cfg.write_u32(COMMON_CFG_QUEUE_DRIVER_HI as usize, (avail_phys >> 32) as u32);

        cfg.write_u32(COMMON_CFG_QUEUE_DEVICE_LO as usize, (used_phys & 0xFFFFFFFF) as u32);
        cfg.write_u32(COMMON_CFG_QUEUE_DEVICE_HI as usize, (used_phys >> 32) as u32);

        // Writing queue enable flag.
        cfg.write_u16(COMMON_CFG_QUEUE_ENABLE as usize, 1);

        Ok(())
    }

    /// Get notification address
    ///
    /// Per VirtIO 1.0 spec 4.1.4.4:
    ///   notify_off    = common_cfg.queue_notify_off (read per queue)
    ///   notify_addr   = notify_cap.bar_base + notify_cap.offset
    ///                 + notify_off * notify_off_multiplier
    ///
    /// The old code computed `queue_index * multiplier * 2` (review BUG
    /// "notify 地址多乘 2"): it used the queue INDEX instead of the queue's
    /// notify_off field, and scaled the multiplier (already in bytes) by an
    /// extra ×2. It only "worked" because QEMU sets multiplier=0 so every
    /// queue notifies at the same address — any queue with a real
    /// multiplier would hit the wrong doorbell.
    pub fn get_notify_addr(&self, queue_index: u16) -> u64 {
        // Select the queue and read its queue_notify_off.
        // SAFETY: common_cfg_bar points to a valid MMIO-mapped VirtIO
        // common config region; queue_select/queue_notify_off are standard
        // per-queue registers.
        let cfg = self.common_cfg();
        cfg.write_u16(COMMON_CFG_QUEUE_SELECT as usize, queue_index);
        let notify_off: u16 = cfg.read_u16(COMMON_CFG_QUEUE_NOTIFY_OFF as usize);
        self.notify_cfg_bar + self.notify_cfg_offset as u64
            + notify_off as u64 * self.notify_off_multiplier as u64
    }

    /// Notify device
    pub fn notify(&self, queue_index: u16) {
        let notify_addr = self.get_notify_addr(queue_index);
        // INV-MMIO-1..3: notify_addr comes from the mapped notify BAR
        // plus the spec's multiplier arithmetic; the 16-bit write is the
        // virtio-pci doorbell (matches Linux iowrite16).
        let notify_region =
            crate::safety::mmio::MmioRegion::new(notify_addr as usize, 2);
        notify_region.write_u16(0, queue_index);
    }

    /// Enable device interrupt
    ///
    /// RISC-V QEMU virt platform PCIe IRQ routing:
    /// PCIE_IRQ base = 32, total 4 IRQs (32-35)
    /// Formula: IRQ = 32 + ((INT_PIN + PCI_slot) % 4)
    #[cfg(feature = "riscv64")]
    pub fn enable_device_interrupt(&self) {
        // Read INT_PIN to determine IRQ offset
        let int_pin = self.pci_config.read_config_byte(0x3D);

        // PCIe IRQ swizzle. INT_PIN is 1-based (INTA=1): slot n, pin p →
        // IRQ = 32 + ((p - 1 + n) % 4). The old formula forgot the -1, so
        // the line was off by one for every device (review DRIV-H3).
        let irq = 32 + (((int_pin as u32).saturating_sub(1) + self.pci_slot as u32) % 4);

        // Register handler via IRQ framework (unmasks automatically).
        // IRQF_SHARED: QEMU's gpex swizzle maps slot n, pin A to IRQ
        // 32 + (n % 4) — a sixth disk SHARES its line with the second.
        // dev_id is this function's virtio-blk slot; the handler checks
        // its own ISR register and returns None when the line was raised
        // by another device (proper shared-IRQ discipline).
        crate::interrupt::request_irq(
            irq,
            virtio_blk_interrupt_handler_pci,
            crate::interrupt::irqdesc::IRQF_SHARED
                | crate::interrupt::irqdesc::IRQF_NO_THREAD,
            "virtio-blk-pci",
            self.blk_slot,
        ).ok();
    }

    /// Enable device interrupt on x86_64/q35: PCI INTx through the PIC.
    ///
    /// SeaBIOS has already programmed the ICH9 PIRQ router and the device's
    /// INT_LINE register with the PIC IRQ the function's INTx pin lands on
    /// (observed q35 values: 10/11 for the virtio slots). Multiple
    /// virtio-pci functions share one line, so the line carries a shared
    /// dispatcher (see `intx`) that polls every registered device's ISR
    /// capability instead of a single per-device handler.
    #[cfg(feature = "x86_64")]
    pub fn enable_device_interrupt(&self) {
        self.enable_pci_intx_irq(intx_blk_service);
    }

    /// Enable device interrupt on aarch64: PCI INTx arrives as a GICv3
    /// SPI through the gpex interrupt-map (DTB pcie@10000000: device n,
    /// pin p [1-based] → GIC_SPI 3 + ((p-1+n) % 4), level-high). GIC
    /// INTID = 32 + SPI, so INTID = 35 + ((p-1+n) % 4). Same swizzle
    /// discipline as the riscv64 arm (shared line, ISR-read demux).
    #[cfg(feature = "aarch64")]
    pub fn enable_device_interrupt(&self) {
        let int_pin = self.pci_config.read_config_byte(0x3D);
        let intid =
            35 + (((int_pin as u32).saturating_sub(1) + self.pci_slot as u32) % 4);

        crate::interrupt::request_irq(
            intid,
            virtio_blk_interrupt_handler_pci,
            crate::interrupt::irqdesc::IRQF_SHARED
                | crate::interrupt::irqdesc::IRQF_NO_THREAD,
            "virtio-blk-pci",
            self.blk_slot,
        ).ok();
    }

    /// Register this device on its PIC INTx line with a shared dispatcher.
    ///
    /// `service` runs in hard-IRQ context only when THIS device's ISR
    /// capability reads nonzero (the dispatcher's ISR read also drops the
    /// device's level-triggered INTx assertion).
    ///
    /// Returns the PIC line on success, or None when the firmware left the
    /// line unprogrammed (INT_PIN 0 / INT_LINE >= 16).
    #[cfg(feature = "x86_64")]
    pub fn enable_pci_intx_irq(&self, service: fn()) -> Option<u8> {
        let int_pin = self.pci_config.read_config_byte(0x3D);
        let int_line = self.pci_config.read_config_byte(0x3C);
        if int_pin == 0 || int_line >= 16 {
            crate::pr_warn!(
                "virtio-pci: INTx unprogrammed (pin {} line {})",
                int_pin, int_line
            );
            return None;
        }
        if intx::register(int_line, self.isr_cfg_bar, service) {
            crate::pr_info!(
                "virtio-pci: INTx pin {} routed to PIC line {} (shared dispatch)",
                int_pin,
                int_line
            );
            Some(int_line)
        } else {
            None
        }
    }

    /// Find a STANDARD PCI capability (MSI-X = 0x11 etc.) by walking the
    /// config-space capability list. Distinct from find_virtio_capability,
    /// which matches vendor capabilities (id 0x09) by virtio cfg type.
    fn find_pci_capability(&self, cap_id: u8) -> Option<u8> {
        // (Config-space access through the mapped ECAM window; offsets
        // bounded by the cap-list iteration guard.)
        {
            let mut cap_ptr = self.pci_config.read_config_byte(PCI_CAPABILITY_LIST);
            let mut iterations = 0;
            while cap_ptr != 0 && iterations < 16 {
                let id = self.pci_config.read_config_byte(cap_ptr);
                if id == cap_id {
                    return Some(cap_ptr);
                }
                cap_ptr = self.pci_config.read_config_byte(cap_ptr + 1);
                iterations += 1;
            }
            None
        }
    }

    /// Program MSI-X entry 0 with `vector`, enable MSI-X on the function,
    /// and set INTx-disable (the device stops asserting its line; with
    /// MSI-X the ISR capability stays silent).
    ///
    /// DIO-CONC, x86_64: replaces the per-completion chain
    /// PIC-INTx -> shared dispatcher (ISR poll of EVERY registered
    /// virtio function, each an MMIO trap to the hypervisor) ->
    /// Block-softirq walker, with one posted MSI to MSIX_BLK_VECTOR that
    /// the local APIC delivers in-kernel (no config-space round trip).
    /// On q35/KVM that chain measured ~33 us of the ~83 us synchronous
    /// 4 KiB direct-read (16.3 us ISR polls + 17.1 us softirq hop).
    ///
    /// Returns false (and leaves the legacy path in place) when the
    /// function has no MSI-X capability or its table is unmapped.
    #[cfg(feature = "x86_64")]
    pub fn enable_msix_vector0(&self, vector: u8) -> bool {
        use crate::drivers::hotpath::MSIX_BLK_VECTOR;
        let _ = vector; // single well-known vector for all blk functions
        if self.msix_cap_off == 0 || self.msix_table_va == 0 {
            return false;
        }
        let cap = self.msix_cap_off as u64;
        // Entry layout: addr_lo @0, addr_hi @4, data @8, vector_control @12.
        // Program masked first, then fill, then unmask (PCI spec 6.8.2.5:
        // software must not touch an unmasked entry's fields).
        // INV-MMIO-1..3: the MSI-X table entry as a 16-byte register
        // window (the table BAR mapping proven at probe).
        let e = crate::safety::mmio::MmioRegion::new(self.msix_table_va as usize, 16);
        e.write_u32(12, 1); // masked
        // Delivery: fixed, edge-triggered, physical destination, the
        // BSP (APIC ID 0). 0xFEE00000 base + dest<<12.
        e.write_u32(0, 0xFEE0_0000);
        e.write_u32(4, 0);
        e.write_u32(8, MSIX_BLK_VECTOR as u32);
        e.write_u32(12, 0); // unmask
        // Message Control (cap+2): MSI-X Enable (bit 14) set, Function
        // Mask (bit 15) clear. Bits 10:0 are the read-only table size-1.
        let msg_ctrl = self.pci_config.read_config_word((cap + 2) as u8);
        self.pci_config
            .write_config_word((cap + 2) as u8, (msg_ctrl | 0x8000) & !0x4000);

        // Command register: set INTx_DISABLE (bit 10); keep memory space
        // (bit 1) and bus master (bit 2) as enable_bus_master left them.
        let cmd = self.pci_config.read_config_word(0x04);
        self.pci_config.write_config_word(0x04, cmd | 0x0400);

        // RT-plan R5: the completion vector's dispatch registration —
        // under `irqthread=all` (the measurement arm), spawn the
        // per-vector service thread here (idempotent; the first blk
        // function to program the vector wins). The default irqthread=1
        // shape opts the block queue out (IRQF_NO_THREAD class): the
        // vector's gate finds no thread and runs the walker inline.
        // A completion posting before the thread is up is not lost:
        // wake() no-ops and the inline fallback runs the walker this
        // once — same registration-race shape as the irqdesc threads.
        crate::arch::x86_64::trap::spawn_blk_msix_irq_thread();
        true
    }

    /// Set queue MSI-X vector
    ///
    /// VirtIO 1.0 specification requires setting MSI-X vector before queue_enable
    /// This tells the device which MSI-X vector to use for queue completion interrupts
    ///
    /// # Parameters
    /// - `queue_index`: Queue index (0 for first queue)
    /// - `vector`: MSI-X vector number (0 means don't use MSI-X, use legacy INTx)
    pub fn set_queue_vector(&self, queue_index: u16, vector: u16) {
        // SAFETY: common_cfg_bar points to valid MMIO region; selecting the
        // queue then writing its msix_vector register. NOTE: the vector
        // register is COMMON_CFG_QUEUE_MSIX_VECTOR (0x1A) — the old code
        // wrote 0x1C, which is QUEUE_ENABLE, and would have disabled the
        // queue had anything ever called this.
        {
            let cfg = self.common_cfg();
            cfg.write_u16(COMMON_CFG_QUEUE_SELECT as usize, queue_index);
            cfg.write_u16(COMMON_CFG_QUEUE_MSIX_VECTOR as usize, vector);
        }
    }

    /// Read data from block device
    ///
    /// # Parameters
    /// - `sector`: Starting sector number
    /// - `buf`: Data buffer
    ///
    /// # Returns
    /// Returns bytes read on success, error code on failure
    pub fn read_block(&self, sector: u64, buf: &mut [u8]) -> Result<usize, &'static str> {

        use crate::arch::mm::VirtAddr;

        // Allocate three descriptors
        let virt_queue_opt: Option<VirtQueue> = VirtQueue::new(8u16,
            0,  // queue_index
            self.notify_cfg_bar + QUEUE_NOTIFY as u64,
            self.common_cfg_bar + INTERRUPT_STATUS as u64,
            self.common_cfg_bar + INTERRUPT_ACK as u64);
        let mut virt_queue = match virt_queue_opt {
            None => return Err("Failed to create VirtQueue"),
            Some(q) => q,
        };

        let header_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err("Failed to alloc header descriptor"),
        };
        let data_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err("Failed to alloc data descriptor"),
        };
        let resp_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => return Err("Failed to alloc response descriptor"),
        };

        // Construct VirtIO block request header
        let req_header = VirtIOBlkReqHeader {
            type_: req_type::VIRTIO_BLK_T_IN,
            reserved: 0,
            sector,
        };

        // INV-KPTR-1: one audited allocation per buffer (Drop owns the
        // frees; the timeout path below keeps its deliberate leak via
        // mem::forget).
        let header = match crate::safety::ptr::KPtr::new(req_header) {
            Some(h) => h,
            None => return Err("Failed to allocate header"),
        };
        let resp = match crate::safety::ptr::KPtr::new(VirtIOBlkResp {
            status: 0xFF,  // Initialize to invalid status
        }) {
            Some(r) => r,
            None => return Err("Failed to allocate response"),
        };
        let header_ptr = header.as_ptr();
        let resp_ptr = resp.as_ptr();

        // VirtIO descriptor flags
        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        // Convert virtual addresses to physical addresses
        let header_phys_addr = crate::arch::mm::virt_to_phys(
            VirtAddr::new(header_ptr as u64)
        ).0;
        let resp_phys_addr = crate::arch::mm::virt_to_phys(
            VirtAddr::new(resp_ptr as u64)
        ).0;

        // Set request header descriptor
        virt_queue.set_desc(
            header_desc_idx,
            header_phys_addr,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // Set data buffer descriptor (device writes)
        // For PCI VirtIO, we need to ensure buffer is accessible in physical memory
        let data_phys_addr = crate::arch::mm::virt_to_phys(
            VirtAddr::new(buf.as_ptr() as u64)
        ).0;

        virt_queue.set_desc(
            data_desc_idx,
            data_phys_addr,
            buf.len() as u32,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            resp_desc_idx,
        );

        // Set response descriptor
        virt_queue.set_desc(
            resp_desc_idx,
            resp_phys_addr,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            VIRTQ_DESC_F_WRITE,
            0,
        );

        // Submit to available ring
        virt_queue.submit(header_desc_idx);

        // Notify device
        virt_queue.notify();

        // Wait for completion
        let prev_used = virt_queue.get_used();
        let new_used = virt_queue.wait_for_completion(prev_used);

        if new_used == prev_used {
            // Request failed, device did not update used ring.
            // NOTE: the chain may still be in flight, so the vring is NOT
            // freed here — only the request buffers are (device-visible
            // DMA for them ended with the used-ring stall check above).
            // R21-N2 unchanged: LEAK on a true timeout — KPtr must NOT
            // drop while the chain may still be in flight.
            core::mem::forget(header);
            core::mem::forget(resp);
            return Err("VirtIO request timeout");
        }

        // INV-KPTR-5: device-written status, volatile read.
        let status = resp.read_volatile();

        drop(header);
        drop(resp);
        virt_queue.free_vring();

        match status.status {
            crate::drivers::hotpath::queue_status::VIRTIO_BLK_S_OK => Ok(buf.len()),
            _ => Err("VirtIO block I/O error"),
        }
    }

    /// Write a block to the VirtIO PCI block device
    ///
    /// # Parameters
    /// - `sector`: Starting sector number
    /// - `buf`: Data to write
    ///
    /// # Returns
    /// Returns bytes written on success, error on failure
    pub fn write_block(&self, sector: u64, buf: &[u8]) -> Result<usize, &'static str> {

        use crate::arch::mm::VirtAddr;

        let virt_queue_opt: Option<VirtQueue> = VirtQueue::new(8u16,
            0,
            self.notify_cfg_bar + QUEUE_NOTIFY as u64,
            self.common_cfg_bar + INTERRUPT_STATUS as u64,
            self.common_cfg_bar + INTERRUPT_ACK as u64);
        let mut virt_queue = match virt_queue_opt {
            None => return Err("Failed to create VirtQueue"),
            Some(q) => q,
        };

        // The per-call vring must be freed on EVERY exit path (review BUG:
        // desc-failure early returns leaked a page-granular vring per call).
        let header_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => {
                // SAFETY: queue was never submitted to the device.
                virt_queue.free_vring();
                return Err("Failed to alloc header descriptor");
            }
        };
        let data_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => {
                // SAFETY: queue was never submitted to the device.
                virt_queue.free_vring();
                return Err("Failed to alloc data descriptor");
            }
        };
        let resp_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => {
                // SAFETY: queue was never submitted to the device.
                virt_queue.free_vring();
                return Err("Failed to alloc response descriptor");
            }
        };

        // Write request: VIRTIO_BLK_T_OUT
        let req_header = VirtIOBlkReqHeader {
            type_: req_type::VIRTIO_BLK_T_OUT,
            reserved: 0,
            sector,
        };

        // INV-KPTR-1: one audited allocation per buffer.
        let header = match crate::safety::ptr::KPtr::new(req_header) {
            Some(h) => h,
            None => {
                virt_queue.free_vring();
                return Err("Failed to allocate header");
            }
        };
        let resp = match crate::safety::ptr::KPtr::new(VirtIOBlkResp { status: 0xFF }) {
            Some(r) => r,
            None => {
                virt_queue.free_vring();
                return Err("Failed to allocate response");
            }
        };
        let header_ptr = header.as_ptr();
        let resp_ptr = resp.as_ptr();

        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        let header_phys_addr = crate::arch::mm::virt_to_phys(
            VirtAddr::new(header_ptr as u64)
        ).0;
        let resp_phys_addr = crate::arch::mm::virt_to_phys(
            VirtAddr::new(resp_ptr as u64)
        ).0;

        // Header: device reads (no F_WRITE)
        virt_queue.set_desc(
            header_desc_idx,
            header_phys_addr,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // Data: device reads from host (no F_WRITE, opposite of read)
        let data_phys_addr = crate::arch::mm::virt_to_phys(
            VirtAddr::new(buf.as_ptr() as u64)
        ).0;

        virt_queue.set_desc(
            data_desc_idx,
            data_phys_addr,
            buf.len() as u32,
            VIRTQ_DESC_F_NEXT,
            resp_desc_idx,
        );

        // Response: device writes
        virt_queue.set_desc(
            resp_desc_idx,
            resp_phys_addr,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            VIRTQ_DESC_F_WRITE,
            0,
        );

        virt_queue.submit(header_desc_idx);
        virt_queue.notify();

        let prev_used = virt_queue.get_used();
        let new_used = virt_queue.wait_for_completion(prev_used);

        if new_used == prev_used {
            // Chain possibly still in flight — vring not freed (see
            // above); R21-N2 leak via forget.
            core::mem::forget(header);
            core::mem::forget(resp);
            return Err("VirtIO write request timeout");
        }

        // INV-KPTR-5: device-written status, volatile read.
        let status = resp.read_volatile();

        drop(header);
        drop(resp);
        virt_queue.free_vring();

        match status.status {
            crate::drivers::hotpath::queue_status::VIRTIO_BLK_S_OK => Ok(buf.len()),
            _ => Err("VirtIO block write error"),
        }
    }
}

/// Read block device using configured VirtQueue
///
/// # Parameters
/// - `pci_dev`: VirtIO PCI device
/// - `sector`: Starting sector number
/// - `buf`: Data buffer
///
/// # Returns
/// Returns bytes read on success, error code on failure
pub fn read_block_using_configured_queue(
    pci_dev: &VirtIOPCI,
    sector: u64,
    buf: &mut [u8]
) -> Result<usize, &'static str> {
    let slot = pci_dev.blk_slot;
    // Add retry mechanism to resolve VirtIO block device random timeout issues
    const MAX_RETRIES: usize = 5;
    let mut retries = 0;

    loop {
        match read_block_once(pci_dev, sector, buf) {
            Ok(size) => {
                return Ok(size);
            }
            Err(e) => {
                retries += 1;
                if retries >= MAX_RETRIES {
                    return Err(e);
                }
                // Drain completions first (process context, no virtio lock
                // held — VIRTIO-WQ-1 safe): retires finished pendings so
                // ordinal slots and descriptor windows free up before the
                // retry, turning "table lag" and in-flight-guard rejections
                // into one-shot retry conditions instead of a 5-attempt
                // failure.
                crate::drivers::hotpath::pci_blk_kick(slot);
                crate::drivers::hotpath::pci_process_async_completions();
                // Short delay before retry
                for _ in 0..10000 {
                    core::hint::spin_loop();
                }
            }
        }
    }
}

/// Single read attempt
fn read_block_once(
    pci_dev: &VirtIOPCI,
    sector: u64,
    buf: &mut [u8]
) -> Result<usize, &'static str> {
    let slot = pci_dev.blk_slot;


    // R17-C: ONE allocation carries header (offset 0) + response
    // (offset 48). Two separate page-granular allocations per I/O was
    // the system's highest-volume order-0 firehose — every mis-handed
    // heap page cycled through here within milliseconds (ring-proven
    // in round 17). 48 keeps the resp cacheline-separate from header.
    //
    // Allocated BEFORE the BLK lock (Linux parity: request allocation
    // happens outside the queue lock): the heap allocator's internal
    // lock nested inside this lock's IRQ-off window was a contention
    // amplifier, and the b125 wedge hunt demands the BLK critical
    // section contain only ring RAM writes (see mod.rs forensics).
    // SUBMIT-2X: per-CPU io_buf pool — no global-allocator lock on the
    // synchronous submit path either.
    let io_buf: *mut u8 = crate::drivers::hotpath::io_buf_alloc();
    if io_buf.is_null() {
        return Err("Failed to allocate header");
    }
    let header_layout = alloc::alloc::Layout::from_size_align(64, 16).unwrap(); // dealloc layout for the combined block
    // INV-SAFECALL: typed split+init of the pooled R17-C block.
    let (header_ptr, resp_ptr) = crate::drivers::hotpath::io_buf_init(
        io_buf,
        req_type::VIRTIO_BLK_T_IN,
        sector,
    );

    // VirtIO descriptor flags
    const VIRTQ_DESC_F_NEXT: u16 = 1;
    const VIRTQ_DESC_F_WRITE: u16 = 2;

    // Phase 1: Set up and submit request (under PCI lock)
    let phase1: Result<(*const UsedRing, u16), &'static str> = 'blk: {
        let _guard = crate::drivers::hotpath::pci_blk_lock(slot);
        let _nest = crate::drivers::types::VirtioLockNest::new();

        // Get configured VirtQueue (mutable reference)
        let virt_queue = match crate::drivers::hotpath::get_pci_device_queue_mut_at(slot, &_guard) {
            Some(q) => q,
            None => {
                // SAFETY: io_buf was allocated above and no chain was
                // built from it; freed by the Err handling below,
                // OUTSIDE the lock.
                break 'blk Err("No configured VirtQueue found");
            }
        };

        // Reclaim descriptors that the device has finished with
        virt_queue.reclaim_descs();

        // Allocate the chain's three descriptors (consecutive free-running
        // ids). Slot/window safety against live pendings is checked BELOW
        // (pci_pending_slot_reservable) and is RETRYABLE — a still-live
        // occupant means the completion walker is lagging, not a hard
        // failure.
        let header_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => break 'blk Err("Failed to alloc header descriptor"),
        };
        let data_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => break 'blk Err("Failed to alloc data descriptor"),
        };
        let resp_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => break 'blk Err("Failed to alloc response descriptor"),
        };

        // Convert virtual addresses to physical addresses
        let header_phys_addr = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(header_ptr as u64),
        ).0;
        let resp_phys_addr = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(resp_ptr as u64),
        ).0;

        // For PCI VirtIO, we need to ensure buffer is accessible in physical memory
        let data_phys_addr = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(buf.as_ptr() as u64),
        ).0;

        // Set request header descriptor
        virt_queue.set_desc(
            header_desc_idx,
            header_phys_addr,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // Set data buffer descriptor (device writes)
        virt_queue.set_desc(
            data_desc_idx,
            data_phys_addr,
            buf.len() as u32,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            resp_desc_idx,
        );

        // Set response descriptor
        virt_queue.set_desc(
            resp_desc_idx,
            resp_phys_addr,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            VIRTQ_DESC_F_WRITE,
            0,
        );

        // Get current expected value (used.idx expected value before submit)
        let prev_expected = crate::drivers::hotpath::get_expected_used_idx_at(slot);

        // Reserve OUR ordinal slot (submission order) + verify the chain's
        // descriptor window does not overlap a live pending/tombstone
        // BEFORE touching the device: if this chain cannot be tracked,
        // it must not be submitted (an untracked chain's used-ring entry
        // would be misattributed). "Not reservable" = walker lag — the
        // outer retry drains completions and tries again.
        if !crate::drivers::hotpath::pci_pending_slot_reservable(
            slot,
            virt_queue.queue_size as u32,
            prev_expected,
            header_desc_idx,
        ) {
            // SAFETY: io_buf was allocated above and nothing references it
            // yet (no chain was built from these descriptors); freed
            // below, OUTSIDE the lock.
            break 'blk Err("Pending table lag (completion walker behind)");
        }

        // Submit to the available ring QUIETLY (no MMIO under the lock —
        // the kick happens after the guard drops; see below).
        virt_queue.submit_quiet(header_desc_idx);

        // Increment expected used.idx (track our expected completion count)
        crate::drivers::hotpath::increment_expected_used_idx_at(slot);

        // Publish a NULL-completion reservation ("tombstone") for this
        // synchronous chain AT ITS ORDINAL SLOT: keeps the chain's
        // descriptor window reserved until the completion walker consumes
        // its used-ring entry, so (a) no async submit can recycle these
        // descriptors while the chain is still at the device and (b) this
        // chain's used-ring entry can never be misattributed to a later
        // async pending that recycled the head descriptor id. Still under
        // the BLK lock here — the walker cannot observe the used-ring
        // advance first, and the reservation above cannot be lost.
        crate::drivers::hotpath::pci_publish_sync_chain(
            slot,
            virt_queue.queue_size as u32,
            prev_expected,
            header_desc_idx,
        );

        // Snapshot used ring pointer for interrupt-driven wait
        let used_ptr = virt_queue.used_ring_ptr();

        break 'blk Ok((used_ptr, prev_expected));
    };
    // PCI lock dropped here — other I/O operations can proceed while we wait
    let (used_ring_ptr, prev_expected) = match phase1 {
        Ok(v) => v,
        Err(e) => {
            // io_buf was allocated above; every error above left no
            // submitted chain referencing it. Freed outside the BLK lock's
            // IRQ window.
            crate::drivers::hotpath::io_buf_free(io_buf);
            return Err(e);
        }
    };

    // Kick the device OUTSIDE the BLK lock: notify() is an MMIO write
    // (a VM exit under KVM; under TCG an emulator trap). Running it
    // inside the irqsave critical section made the lock's hold time
    // hostage to host-side device emulation latency — one of the two
    // unbounded windows the b125 spin-wedge hunt closed. Order is safe:
    // the tombstone above was published under the same lock any walker
    // must take before observing the used ring, and the device cannot
    // complete an un-kicked chain.
    if let Some(q) = crate::drivers::hotpath::get_pci_device_queue_at(slot) {
        q.notify();
    }

    // Phase 2: Wait for completion (interrupt-driven, releases BKL during
    // sleep; bounded by a 10s wall-clock deadline — see
    // wait_for_used_interruptible)
    let new_used = VirtQueue::wait_for_used_interruptible(
        used_ring_ptr,
        crate::drivers::hotpath::get_pci_blk_wait_queue(slot),
        prev_expected,
        // status is the response struct's first byte; the typed
        // volatile reader (INV-SAFECALL) handles the polling.
        resp_ptr as *const u8,
        slot,
    );

    // Phase 3: Check response
    if new_used == prev_expected {
        // R21-N2: descriptors STILL SUBMITTED — freeing here + the
        // caller's retry onto rewound indices let the device DMA into
        // freed memory on the ROOT FS path. Late-drain the used-ring
        // index (R8-M2 discipline); a true timeout leaks the 64B block
        // instead of freeing in-flight DMA targets.
        let mut late = false;
        for _ in 0..2_000_000u64 {
            let idx = used_ring_idx(used_ring_ptr);
            if idx != prev_expected {
                late = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !late {
            // True timeout (10s deadline expired): the chain may still be
            // in flight on the device. Flag our tombstone so the walker's
            // eventual consumption pairs the leaked-chain accounting, and
            // record the leak so (a) the in-flight guard stops counting it
            // and (b) the caller's retry cannot reuse its slots.
            crate::drivers::hotpath::pci_flag_sync_tombstone_timed_out(slot, prev_expected);
            if let Some(vq) = crate::drivers::hotpath::get_pci_device_queue_at(slot) {
                vq.note_timed_out_chain();
            }
            return Err("VirtIO request timeout");
        }
        // completed late — fall through to status handling
    }

    // INV-DMA-3: device-written status byte, volatile read.
    let status = VirtIOBlkResp { status: crate::drivers::hotpath::resp_status_byte(resp_ptr as *const u8) };

    // header_ptr is the base of the single combined R17-C block; resp
    // lives inside it at +48. The device completed the chain, so the
    // block is dead from its point of view — return it to the pool.
    crate::drivers::hotpath::io_buf_free(header_ptr as *mut u8);

    match status.status {
        crate::drivers::hotpath::queue_status::VIRTIO_BLK_S_OK => Ok(buf.len()),
        _ => Err("VirtIO block I/O error"),
    }
}

/// # Write block using pre-configured VirtQueue (for PCI VirtIO)
///
/// # Returns
/// Returns bytes written on success, error code on failure
pub fn write_block_using_configured_queue(
    pci_dev: &VirtIOPCI,
    sector: u64,
    buf: &[u8]
) -> Result<usize, &'static str> {
    let slot = pci_dev.blk_slot;
    // Add retry mechanism to resolve VirtIO block device random timeout issues
    const MAX_RETRIES: usize = 5;
    let mut retries = 0;

    loop {
        match write_block_once(pci_dev, sector, buf) {
            Ok(size) => {
                return Ok(size);
            }
            Err(e) => {
                retries += 1;
                if retries >= MAX_RETRIES {
                    return Err(e);
                }
                // Drain completions before retrying (see the read path).
                crate::drivers::hotpath::pci_blk_kick(slot);
                crate::drivers::hotpath::pci_process_async_completions();
                // Short delay before retry
                for _ in 0..10000 {
                    core::hint::spin_loop();
                }
            }
        }
    }
}

/// Single write attempt using pre-configured VirtQueue
fn write_block_once(
    pci_dev: &VirtIOPCI,
    sector: u64,
    buf: &[u8]
) -> Result<usize, &'static str> {

    let slot = pci_dev.blk_slot;

    // R17-C combined header+resp block, allocated BEFORE the BLK lock
    // (same discipline as read_block_once — see the comment there).
    // SUBMIT-2X: per-CPU io_buf pool — no global-allocator lock on the
    // synchronous submit path either.
    let io_buf: *mut u8 = crate::drivers::hotpath::io_buf_alloc();
    if io_buf.is_null() {
        return Err("Failed to allocate header");
    }
    let header_layout = alloc::alloc::Layout::from_size_align(64, 16).unwrap(); // dealloc layout for the combined block
    // INV-SAFECALL: typed split+init of the pooled R17-C block.
    let (header_ptr, resp_ptr) = crate::drivers::hotpath::io_buf_init(
        io_buf,
        req_type::VIRTIO_BLK_T_OUT,
        sector,
    );

    // VirtIO descriptor flags
    const VIRTQ_DESC_F_NEXT: u16 = 1;
    const VIRTQ_DESC_F_WRITE: u16 = 2;

    // Phase 1: Set up and submit request (under PCI lock)
    let phase1: Result<(*const UsedRing, u16), &'static str> = 'blk: {
        let _guard = crate::drivers::hotpath::pci_blk_lock(slot);
        let _nest = crate::drivers::types::VirtioLockNest::new();

        // Get configured VirtQueue (mutable reference)
        let virt_queue = match crate::drivers::hotpath::get_pci_device_queue_mut_at(slot, &_guard) {
            Some(q) => q,
            None => break 'blk Err("No configured VirtQueue found"),
        };

        // Reclaim descriptors that the device has finished with
        virt_queue.reclaim_descs();

        // Allocate the chain's three descriptors (consecutive free-running
        // ids); slot/window safety is checked below and is RETRYABLE.
        let header_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => break 'blk Err("Failed to alloc header descriptor"),
        };
        let data_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => break 'blk Err("Failed to alloc data descriptor"),
        };
        let resp_desc_idx = match virt_queue.alloc_desc() {
            Some(idx) => idx,
            None => break 'blk Err("Failed to alloc response descriptor"),
        };

        // Convert virtual addresses to physical addresses
        let header_phys_addr = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(header_ptr as u64),
        ).0;
        let resp_phys_addr = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(resp_ptr as u64),
        ).0;

        // For PCI VirtIO, we need to ensure buffer is accessible in physical memory
        let data_phys_addr = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(buf.as_ptr() as u64),
        ).0;

        // Set request header descriptor (device reads this)
        virt_queue.set_desc(
            header_desc_idx,
            header_phys_addr,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // Set data buffer descriptor (device reads data - NO VIRTQ_DESC_F_WRITE)
        virt_queue.set_desc(
            data_desc_idx,
            data_phys_addr,
            buf.len() as u32,
            VIRTQ_DESC_F_NEXT,
            resp_desc_idx,
        );

        // Set response descriptor (device writes this)
        virt_queue.set_desc(
            resp_desc_idx,
            resp_phys_addr,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            VIRTQ_DESC_F_WRITE,
            0,
        );

        // Get current expected value (used.idx expected value before submit)
        let prev_expected = crate::drivers::hotpath::get_expected_used_idx_at(slot);

        // Reserve OUR ordinal slot + verify the window before submitting
        // (same discipline as the read path — see read_block_once).
        if !crate::drivers::hotpath::pci_pending_slot_reservable(
            slot,
            virt_queue.queue_size as u32,
            prev_expected,
            header_desc_idx,
        ) {
            // SAFETY: io_buf was allocated above and nothing references it
            // yet (no chain was built from these descriptors); freed
            // below, OUTSIDE the lock.
            break 'blk Err("Pending table lag (completion walker behind)");
        }

        // Submit quietly (no MMIO under the lock; kick after release).
        virt_queue.submit_quiet(header_desc_idx);

        // Increment expected used.idx (track our expected completion count)
        crate::drivers::hotpath::increment_expected_used_idx_at(slot);

        // Publish a NULL-completion reservation ("tombstone") for this
        // synchronous chain AT ITS ORDINAL SLOT (same discipline as the
        // read path — see read_block_once).
        crate::drivers::hotpath::pci_publish_sync_chain(
            slot,
            virt_queue.queue_size as u32,
            prev_expected,
            header_desc_idx,
        );

        // Snapshot used ring pointer for interrupt-driven wait
        let used_ptr = virt_queue.used_ring_ptr();

        break 'blk Ok((used_ptr, prev_expected));
    };
    // PCI lock dropped here — other I/O operations can proceed while we wait
    let (used_ring_ptr, prev_expected) = match phase1 {
        Ok(v) => v,
        Err(e) => {
            // io_buf was allocated above; every error above left no
            // submitted chain referencing it. Freed outside the BLK lock's
            // IRQ window.
            crate::drivers::hotpath::io_buf_free(io_buf);
            return Err(e);
        }
    };

    // Kick the device OUTSIDE the BLK lock (see read_block_once).
    if let Some(q) = crate::drivers::hotpath::get_pci_device_queue_at(slot) {
        q.notify();
    }

    // Phase 2: Wait for completion (interrupt-driven, releases BKL during
    // sleep; bounded by a 10s wall-clock deadline — see
    // wait_for_used_interruptible)
    let new_used = VirtQueue::wait_for_used_interruptible(
        used_ring_ptr,
        crate::drivers::hotpath::get_pci_blk_wait_queue(slot),
        prev_expected,
        // status is the response struct's first byte; the typed
        // volatile reader (INV-SAFECALL) handles the polling.
        resp_ptr as *const u8,
        slot,
    );

    // Phase 3: Check response
    if new_used == prev_expected {
        // R21-N2: descriptors STILL SUBMITTED — freeing here + the
        // caller's retry onto rewound indices let the device DMA into
        // freed memory on the ROOT FS path. Late-drain the used-ring
        // index (R8-M2 discipline); a true timeout leaks the 64B block
        // instead of freeing in-flight DMA targets.
        let mut late = false;
        for _ in 0..2_000_000u64 {
            let idx = used_ring_idx(used_ring_ptr);
            if idx != prev_expected {
                late = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !late {
            // True timeout (10s deadline expired) — flag the tombstone and
            // leak-note the chain (same pairing as the read path).
            crate::drivers::hotpath::pci_flag_sync_tombstone_timed_out(slot, prev_expected);
            if let Some(vq) = crate::drivers::hotpath::get_pci_device_queue_at(slot) {
                vq.note_timed_out_chain();
            }
            return Err("VirtIO write request timeout");
        }
        // completed late — fall through to status handling
    }

    // INV-DMA-3: device-written status byte, volatile read.
    let status = VirtIOBlkResp { status: crate::drivers::hotpath::resp_status_byte(resp_ptr as *const u8) };

    // header_ptr is the base of the single combined R17-C block; resp
    // lives inside it at +48. The device completed the chain, so the
    // block is dead from its point of view — return it to the pool.
    crate::drivers::hotpath::io_buf_free(header_ptr as *mut u8);

    match status.status {
        crate::drivers::hotpath::queue_status::VIRTIO_BLK_S_OK => Ok(buf.len()),
        _ => Err("VirtIO block write I/O error"),
    }
}

/// Flush the device's write cache using the pre-configured VirtQueue.
///
/// Sends a real VIRTIO_BLK_T_FLUSH request (header + resp, no data
/// descriptor) so the disk's volatile cache is persisted. The old fsync
/// path only drained the kernel's write-through buffer cache and never
/// told the DEVICE to persist — "Flush 假成功" (review BUG: fsync 无持久化).
///
/// # Returns
/// Ok(0) when the device acknowledges the flush.
pub fn flush_block_using_configured_queue(
    pci_dev: &VirtIOPCI,
) -> Result<usize, &'static str> {


    let slot = pci_dev.blk_slot;
    const MAX_RETRIES: usize = 5;
    let mut retries = 0;
    loop {
        match flush_block_once(slot) {
            Ok(n) => return Ok(n),
            Err(e) => {
                retries += 1;
                if retries >= MAX_RETRIES {
                    return Err(e);
                }
                // Drain completions before retrying (see the read path).
                crate::drivers::hotpath::pci_blk_kick(slot);
                crate::drivers::hotpath::pci_process_async_completions();
                for _ in 0..10000 {
                    core::hint::spin_loop();
                }
            }
        }
    }
}

/// Single flush attempt using the pre-configured VirtQueue.
fn flush_block_once(slot: usize) -> Result<usize, &'static str> {

    use crate::arch::mm::VirtAddr;

    // Combined header+resp block (R17-C discipline, same as RW paths),
    // allocated BEFORE the BLK lock (see read_block_once).
    // SUBMIT-2X: per-CPU io_buf pool — no global-allocator lock on the
    // synchronous submit path either.
    let io_buf: *mut u8 = crate::drivers::hotpath::io_buf_alloc();
    if io_buf.is_null() {
        return Err("Failed to allocate flush header");
    }
    let header_layout = alloc::alloc::Layout::from_size_align(64, 16).unwrap();
    // INV-SAFECALL: typed split+init of the pooled R17-C block.
    let (header_ptr, resp_ptr) =
        crate::drivers::hotpath::io_buf_init(io_buf, req_type::VIRTIO_BLK_T_FLUSH, 0);

    const VIRTQ_DESC_F_NEXT: u16 = 1;
    const VIRTQ_DESC_F_WRITE: u16 = 2;

    let phase1: Result<(*const UsedRing, u16), &'static str> = 'blk: {
        let _guard = crate::drivers::hotpath::pci_blk_lock(slot);
        let _nest = crate::drivers::types::VirtioLockNest::new();

        let virt_queue = match crate::drivers::hotpath::get_pci_device_queue_mut_at(slot, &_guard) {
            Some(q) => q,
            None => break 'blk Err("No configured VirtQueue found"),
        };

        virt_queue.reclaim_descs();

        // FLUSH is a 2-descriptor chain: header -> resp (no data).
        let header_desc_idx = match virt_queue.alloc_desc_chain(2) {
            Some(idx) => idx,
            None => break 'blk Err("Failed to alloc header descriptor"),
        };
        let resp_desc_idx = match virt_queue.alloc_desc_chain(2) {
            Some(idx) => idx,
            None => break 'blk Err("Failed to alloc response descriptor"),
        };

        let header_phys_addr = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(header_ptr as u64),
        ).0;
        let resp_phys_addr = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(resp_ptr as u64),
        ).0;

        virt_queue.set_desc(
            header_desc_idx,
            header_phys_addr,
            core::mem::size_of::<VirtIOBlkReqHeader>() as u32,
            VIRTQ_DESC_F_NEXT,
            resp_desc_idx,
        );
        virt_queue.set_desc(
            resp_desc_idx,
            resp_phys_addr,
            core::mem::size_of::<VirtIOBlkResp>() as u32,
            VIRTQ_DESC_F_WRITE,
            0,
        );

        let prev_expected = crate::drivers::hotpath::get_expected_used_idx_at(slot);

        // Reserve OUR ordinal slot + verify the window before submitting
        // (same discipline as the read/write paths; a FLUSH chain is only
        // 2 descriptors wide, covered conservatively by the 3-wide window
        // check).
        if !crate::drivers::hotpath::pci_pending_slot_reservable(
            slot,
            virt_queue.queue_size as u32,
            prev_expected,
            header_desc_idx,
        ) {
            // SAFETY: io_buf was allocated above and nothing references it
            // yet (no chain was built from these descriptors); freed
            // below, OUTSIDE the lock.
            break 'blk Err("Pending table lag (completion walker behind)");
        }

        // Submit quietly (no MMIO under the lock; kick after release).
        virt_queue.submit_quiet(header_desc_idx);
        crate::drivers::hotpath::increment_expected_used_idx_at(slot);

        // Publish the sync-chain tombstone at its ordinal slot (same
        // discipline as the read/write paths): the flush's used-ring
        // entry must be consumed as OURS, never misattributed.
        crate::drivers::hotpath::pci_publish_sync_chain(
            slot,
            virt_queue.queue_size as u32,
            prev_expected,
            header_desc_idx,
        );

        let used_ptr = virt_queue.used_ring_ptr();
        break 'blk Ok((used_ptr, prev_expected));
    };
    let (used_ring_ptr, prev_expected) = match phase1 {
        Ok(v) => v,
        Err(e) => {
            // io_buf was allocated above; every error above left no
            // submitted chain referencing it. Freed outside the BLK lock's
            // IRQ window.
            crate::drivers::hotpath::io_buf_free(io_buf);
            return Err(e);
        }
    };

    // Kick the device OUTSIDE the BLK lock (see read_block_once).
    if let Some(q) = crate::drivers::hotpath::get_pci_device_queue_at(slot) {
        q.notify();
    }

    let new_used = VirtQueue::wait_for_used_interruptible(
        used_ring_ptr,
        crate::drivers::hotpath::get_pci_blk_wait_queue(slot),
        prev_expected,
        // status is the response struct's first byte; the typed
        // volatile reader (INV-SAFECALL) handles the polling.
        resp_ptr as *const u8,
        slot,
    );

    if new_used == prev_expected {
        // Same R21-N2 discipline as the RW paths: descriptors still
        // submitted, late-drain bounded; a true timeout leaks the 64B
        // block instead of freeing in-flight DMA targets.
        let mut late = false;
        for _ in 0..2_000_000u64 {
            let idx = used_ring_idx(used_ring_ptr);
            if idx != prev_expected {
                late = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !late {
            // True timeout (10s deadline expired) — flag the tombstone and
            // leak-note the chain (same pairing as the read path).
            crate::drivers::hotpath::pci_flag_sync_tombstone_timed_out(slot, prev_expected);
            if let Some(vq) = crate::drivers::hotpath::get_pci_device_queue_at(slot) {
                vq.note_timed_out_chain();
            }
            return Err("VirtIO flush request timeout");
        }
    }

    // INV-DMA-3: device-written status byte, volatile read.
    let status = VirtIOBlkResp { status: crate::drivers::hotpath::resp_status_byte(resp_ptr as *const u8) };
    // header_ptr is the base of the combined R17-C block; resp at +48.
    // Device completed the chain — return the block to the pool.
    crate::drivers::hotpath::io_buf_free(header_ptr as *mut u8);

    match status.status {
        crate::drivers::hotpath::queue_status::VIRTIO_BLK_S_OK => Ok(0),
        _ => Err("VirtIO block flush I/O error"),
    }
}

// ============================================================================
// x86_64/q35 PCI INTx dispatch (minimal viable PCI IRQ path)
//
// q35 routes every virtio-pci function's INTx through the ICH9 PIRQ router
// onto a PIC line (SeaBIOS programs the router AND the device's INT_LINE —
// observed values 10/11 with several functions sharing each line). The
// kernel's IRQ entry for PIC lines (arch::x86_64::trap) takes one `fn()`
// per line with no sharing, so virtio devices register (ISR capability
// address, service fn) pairs here and ONE dispatcher per claimed line
// polls them all:
//
//   IRQ 10/11 → intx_dispatch() → for each registered device:
//       read ISR cap (drops that device's level INTx assertion);
//       if nonzero → service() (raise the device's softirq).
//
// The LAPIC/IOAPIC (GSI) path is x86-smp territory; this PIC path stays
// until that lands.
// ============================================================================

/// Block-completion service: hand used-ring processing to the Block
/// softirq (same bottom half the PLIC top half raises on riscv64).
/// The fn-pointer registry carries no slot identity, so x86 counts INTx
/// deliveries in one global (vw_blk_intx_count) — per-disk VW_IRQS
/// attribution stays with the riscv/aarch64 per-device handlers
/// (lb73's VW-SNAP showed irqs=0 on all disks while "first PCI INTx
/// interrupt delivered" proved the path live; the riscv-only wiring
/// made the snapshot lie).
#[cfg(feature = "x86_64")]
pub fn intx_blk_service() {
    crate::drivers::hotpath::vw_blk_intx_bump();
    crate::interrupt::softirq::raise_softirq_irqoff(
        crate::interrupt::softirq::SoftirqIndex::Block as usize,
    );
}

#[cfg(feature = "x86_64")]
mod intx {
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// Registered virtio-pci functions per shared PIC line. 16 covers the
    /// full OH emulator set (6 virtio-blk + 2 net + keyboard + tablet +
    /// gpu + headroom) — a function that fails to register asserts the
    /// shared line with nobody acking its ISR (level-triggered storm).
    const MAX_DEVICES: usize = 16;

    /// (ISR capability address, service fn) pairs. The address doubles as
    /// the occupancy marker (0 = free slot).
    static ISR_ADDRS: [AtomicU64; MAX_DEVICES] =
        [const { AtomicU64::new(0) }; MAX_DEVICES];
    static SERVICES: [AtomicU64; MAX_DEVICES] =
        [const { AtomicU64::new(0) }; MAX_DEVICES];

    /// PIC lines already hooked by `intx_dispatch`.
    static LINES_HOOKED: [AtomicBool; 16] = [const { AtomicBool::new(false) }; 16];

    /// One-shot boot diagnostic: the first PCI INTx interrupt delivered
    /// proves the SeaBIOS PIRQ routing + PIC delivery path end to end.
    /// Its absence pinpoints a broken routing (firmware INT_LINE lies,
    /// line masked, ExtINT gate off).
    static FIRST_DELIVERY: AtomicBool = AtomicBool::new(true);

    /// Per-slot one-shot: log the first INTx event each polling-driven
    /// device (ack-only service) delivers, so "who asserted the shared
    /// line" is visible in the boot log without a real bottom half.
    static ACK_ONLY_LOGGED: [AtomicBool; MAX_DEVICES] =
        [const { AtomicBool::new(false) }; MAX_DEVICES];

    /// Shared per-line top half. Registered via
    /// arch::x86_64::trap::request_irq_line (which also unmasks the line).
    fn intx_dispatch() {
        if FIRST_DELIVERY.swap(false, Ordering::AcqRel) {
            crate::pr_info!("virtio-pci: first PCI INTx interrupt delivered");
        }
        for i in 0..MAX_DEVICES {
            let isr_addr = ISR_ADDRS[i].load(Ordering::Acquire);
            if isr_addr == 0 {
                continue;
            }
            // INV-MMIO-1..3: registered address points at a virtio-pci
            // ISR capability (MMIO, 1 meaningful byte); the read is the
            // device-side interrupt acknowledge per virtio 1.0 spec
            // 4.1.4.5 and returns 0 when this device did not assert the
            // line.
            let isr = crate::safety::mmio::MmioRegion::new(isr_addr as usize, 4)
                .read_u32(0)
                & 0x3;
            if isr != 0 {
                let svc = SERVICES[i].load(Ordering::Acquire);
                if svc != 0 {
                    // One-shot visibility for polling-driven devices: their
                    // events are consumed by poll loops, not this service.
                    if svc == crate::drivers::hotpath::intx_ack_only_service as u64
                        && !ACK_ONLY_LOGGED[i].swap(true, Ordering::AcqRel)
                    {
                        crate::pr_info!(
                            "virtio-pci: INTx event for polling device (ISR @{:#x}) - acked",
                            isr_addr
                        );
                    }
                    // INV-SAFECAST-1: nonzero values were stored from
                    // fn pointers by `register` (u64 and fn() are both
                    // 8 bytes on all Rux targets).
                    let f: fn() = crate::safety::call::safety_cast::<u64, fn()>(svc);
                    f();
                }
            }
        }
    }

    /// Add (isr_addr, service) to the shared registry and hook the PIC
    /// line on first use. Registration happens during single-threaded
    /// boot bring-up, before `sti`; the atomics keep the dispatcher's
    /// reads race-free anyway.
    ///
    /// Re-registering an already-known `isr_addr` UPDATES the service in
    /// place (construction installs `intx_ack_only_service`, drivers
    /// later upgrade their slot to a real bottom-half raise) instead of
    /// consuming a second slot for the same function.
    pub fn register(int_line: u8, isr_addr: u64, service: fn()) -> bool {
        // Upgrade path first: one slot per virtio function.
        for i in 0..MAX_DEVICES {
            if ISR_ADDRS[i].load(Ordering::Acquire) == isr_addr {
                SERVICES[i].store(service as u64, Ordering::Release);
                return hook_line(int_line);
            }
        }

        let mut slot = usize::MAX;
        for i in 0..MAX_DEVICES {
            if ISR_ADDRS[i]
                .compare_exchange(0, isr_addr, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                SERVICES[i].store(service as u64, Ordering::Release);
                slot = i;
                break;
            }
        }
        if slot == usize::MAX {
            crate::pr_err!("virtio-pci: INTx registry full");
            return false;
        }

        if !hook_line(int_line) {
            // Line taken by a non-virtio owner: back the entry out.
            ISR_ADDRS[slot].store(0, Ordering::Release);
            SERVICES[slot].store(0, Ordering::Release);
            return false;
        }
        true
    }

    /// Hook `intx_dispatch` onto a PIC line on first use (idempotent via
    /// LINES_HOOKED; returns false when the line is owned by a non-virtio
    /// handler).
    fn hook_line(int_line: u8) -> bool {
        if !LINES_HOOKED[int_line as usize].swap(true, Ordering::AcqRel) {
            if !crate::arch::x86_64::trap::request_irq_line_named(
                int_line,
                "virtio-pci",
                intx_dispatch,
            ) {
                LINES_HOOKED[int_line as usize].store(false, Ordering::Release);
                crate::pr_err!("virtio-pci: PIC line {} unavailable", int_line);
                return false;
            }
        }
        true
    }
}

/// Default INTx service for functions whose driver is polling-based
/// (virtio-gpu command completion, virtio-input event queues): the
/// dispatcher's ISR-capability read has already acknowledged the device —
/// this hook exists so those completions are visible in the boot log once
/// (which function asserted) without a real bottom half.
#[cfg(feature = "x86_64")]
pub fn intx_ack_only_service() {
    // Intentionally empty: the dispatcher's ISR read already acknowledged
    // the asserting device (the whole point of the default registration).
    // `intx_dispatch` logs, once per device, which function arrived here.
}

// ════════════════ net ═════════════════════════════════
// (was net/mod.rs)
// Network device driver

pub use self::virtio_net_get_device as get_virtio_net_device;
pub use self::get_net_device as get_virtio_net_device_net;

// ════════════════ net::space ══════════════════════════
// (was net/space.rs)
// Network device base class
unsafe impl Send for NetDevice {}
// SAFETY (round-5): NetDevice's cross-thread state (stats, flags) is
// mutated under the NetRx softirq / syscall discipline each op
// documents; the priv_ raw slot is driver-owned and set before
// registration. Sync was implicit in the old `static mut` usage.
unsafe impl Sync for NetDevice {}

/// Device status flags
pub mod dev_flags {
    /// Interface is up
    pub const IFF_UP: u32 = 0x1;
    /// Interface is broadcast
    pub const IFF_BROADCAST: u32 = 0x2;
    /// Interface is loopback
    pub const IFF_LOOPBACK: u32 = 0x8;
    /// Interface is running
    pub const IFF_RUNNING: u32 = 0x40;
    /// Interface has multicast enabled
    pub const IFF_MULTICAST: u32 = 0x1000;
}

/// Network device registry
///
/// Simplified implementation: uses counter to track device count (protected by Mutex)
static DEV_COUNT: Spinlock<usize> = Spinlock::new(0);

impl NetDevice {
    /// Set hardware address
    ///
    /// # Parameters
    /// - `addr`: Hardware address
    /// - `len`: Address length
    pub fn set_address(&mut self, addr: &[u8], len: u8) {
        self.addr_len = len;
        self.addr[..len as usize].copy_from_slice(&addr[..len as usize]);
    }

    /// Get device name
    pub fn get_name(&self) -> &str {
        // Checked UTF-8: interface names are userspace-writable (ioctl
        // SIOCSIFNAME) arbitrary bytes — the KOT-KILL comm panic class
        // (lb266) applies to every unchecked name print.
        let len = self.name.iter().position(|&c| c == 0).unwrap_or(IFNAMSIZ);
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }

    /// Transmit packet
    ///
    /// # Parameters
    /// - `skb`: Packet to transmit
    ///
    /// # Returns
    /// 0 on success, negative error code on failure
    pub fn xmit(&mut self, skb: SkBuff) -> i32 {
        (self.netdev_ops.xmit)(skb)
    }

    /// Get statistics
    pub fn get_stats(&self) -> DeviceStats {
        if let Some(get_stats_fn) = self.netdev_ops.get_stats {
            get_stats_fn()
        } else {
            self.stats
        }
    }

    /// Bring up device
    pub fn up(&mut self) {
        self.flags |= dev_flags::IFF_UP | dev_flags::IFF_RUNNING;
    }

    /// Shut down device
    pub fn down(&mut self) {
        self.flags &= !(dev_flags::IFF_UP | dev_flags::IFF_RUNNING);
    }

    /// Check if device is up
    pub fn is_up(&self) -> bool {
        (self.flags & dev_flags::IFF_UP) != 0
    }

    /// Check if device is running
    pub fn is_running(&self) -> bool {
        (self.flags & dev_flags::IFF_RUNNING) != 0
    }
}

/// Register network device
///
/// # Parameters
/// - `device`: Device to register
///
/// # Returns
/// Assigned device index on success, negative error code on failure
///
/// # Notes
/// - Adds device to global device list
/// - Assigns device index
pub fn register_netdevice(device: &'static mut NetDevice) -> i32 {
    let mut count = DEV_COUNT.lock();
    // Assign device index
    device.ifindex = *count as u32;

    // Increment count
    *count += 1;

    device.ifindex as i32
}

/// Unregister network device
///
/// # Parameters
/// - `device`: Device to unregister
pub fn unregister_netdevice(device: &mut NetDevice) {
    // Remove from global list
    // Simplified implementation: just mark device
    device.flags &= !dev_flags::IFF_UP;
}

/// Find network device by index
///
/// # Parameters
/// - `ifindex`: Device index
///
/// # Returns
/// Found device, or None if not found
pub fn get_netdevice_by_index(ifindex: u32) -> Option<&'static mut NetDevice> {
    // Simplified implementation: currently only supports finding loopback device
    // Full implementation needs to maintain device list
    if ifindex == 0 {
        crate::api::drivers::get_loopback_device()
    } else {
        None
    }
}

/// Find network device by name
///
/// # Parameters
/// - `name`: Device name
///
/// # Returns
/// Found device, or None if not found
pub fn get_netdevice_by_name(name: &str) -> Option<&'static mut NetDevice> {
    // Simplified implementation: currently only supports finding loopback device
    // Full implementation needs to maintain device list
    if name == "lo" {
        crate::api::drivers::get_loopback_device()
    } else {
        None
    }
}

/// Get total network device count
pub fn get_netdevice_count() -> usize {
    *DEV_COUNT.lock()
}

// ════════════════ net::virtio_net ═════════════════════
// (was net/virtio_net.rs)
// VirtIO network device driver
// SAFETY: All shared state is protected by Spinlocks (irqsave where needed),
// ensuring no data races across threads/CPUs.
unsafe impl Send for VirtIONetDevice {}

impl VirtIONetDevice {
    /// Create new VirtIO network device
    pub fn new(base_addr: u64) -> Self {
        Self {
            base_addr,
            mac: [0; 6],
            mtu: 1500,
            initialized: Spinlock::new(false),
            tx_queue: Spinlock::new(None),
            rx_queue: Spinlock::new(None),
            queue_size: 0,
            stats: Spinlock::new(DeviceStats::default()),
            rx_buffers: Spinlock::new(alloc::vec::Vec::new()),
            rx_last_used: Spinlock::new(0),
        }
    }

    /// Initialize device
    pub fn init(&mut self) -> Result<(), &'static str> {
        // INV-MMIO-1..3 (unsafe-slim round 3): the device's register
        // window as a bounds-checked MmioRegion; every access below is
        // a safe volatile op (the 0x120 window covers the config area).
        let mmio = crate::safety::mmio::MmioRegion::new(self.base_addr as usize, 0x120);
        {
            // VirtIO MMIO register offsets (linux/virtio_mmio.h — the
            // shared legacy/modern layout; queueSel=0x30, status=0x70,
            // ready=0x44). R34: the previous code was missing feature
            // negotiation entirely (DriverFeatures 0x20/0x24 were never
            // written, so QEMU's virtio-net ran with the legacy 10-byte
            // vnet header while this driver submits 12-byte headers).
            const MAGIC_VALUE: u64 = 0x00;
            const VERSION: u64 = 0x04;
            const DEVICE_ID: u64 = 0x08;
            const DEVICE_FEATURES: u64 = 0x10;
            const DEVICE_FEATURES_SEL: u64 = 0x14;
            const DRIVER_FEATURES: u64 = 0x20;
            const DRIVER_FEATURES_SEL: u64 = 0x24;
            const QUEUE_SEL: u64 = 0x30;
            const QUEUE_NUM_MAX: u64 = 0x34;
            const QUEUE_NUM: u64 = 0x38;
            const QUEUE_READY: u64 = 0x44;
            const QUEUE_NOTIFY: u64 = 0x50;
            const INTERRUPT_STATUS: u64 = 0x60;
            const INTERRUPT_ACK: u64 = 0x64;
            const STATUS: u64 = 0x70;
            const QUEUE_DESC_LO: u64 = 0x80;
            const QUEUE_DESC_HI: u64 = 0x84;
            const QUEUE_DRIVER_LO: u64 = 0x90;
            const QUEUE_DRIVER_HI: u64 = 0x94;
            const QUEUE_DEVICE_LO: u64 = 0xA0;
            const QUEUE_DEVICE_HI: u64 = 0xA4;
            const CONFIG: u64 = 0x100;

            // Feature bits (word << 5 | bit within word)
            const F_NET_MAC: u32 = 1 << 5;          // word 0: device MAC in config
            const F_VERSION_1: u32 = 1 << 0;        // word 1: VIRTIO_F_VERSION_1

            // Status bits
            const S_ACKNOWLEDGE: u32 = 0x01;
            const S_DRIVER: u32 = 0x02;
            const S_FEATURES_OK: u32 = 0x08;
            const S_DRIVER_OK: u32 = 0x04;

            // Verify magic number
            let magic = mmio.read_u32(MAGIC_VALUE as usize);
            if magic != 0x74726976 {
                return Err("Invalid VirtIO magic value");
            }

            // Verify version — only Modern (v2) is supported. The register
            // block below uses the modern split-address layout (0x80/0x90/
            // 0xa0); legacy v1 devices need the QueuePFN model instead.
            let version = mmio.read_u32(VERSION as usize);
            if version != 2 {
                return Err("Unsupported VirtIO version (only Modern v2)");
            }

            // Verify device ID (network device = 1)
            let device_id = mmio.read_u32(DEVICE_ID as usize);
            if device_id != 1 {
                return Err("Not a VirtIO network device");
            }

            // Reset device, then walk the standard status sequence.
            mmio.write_u32(STATUS as usize, 0);
            for _ in 0..100_000u32 {
                if mmio.read_u32(STATUS as usize) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            // Set driver status: ACKNOWLEDGE, then DRIVER
            mmio.write_u32(STATUS as usize, S_ACKNOWLEDGE);
            mmio.write_u32(STATUS as usize, S_ACKNOWLEDGE | S_DRIVER);

            // R34: feature negotiation was missing entirely. VIRTIO_F_VERSION_1
            // MUST be accepted on a modern (v2) device — without it QEMU's
            // virtio-net operates with the legacy 10-byte vnet header while
            // this driver submits 12-byte headers, corrupting every frame by
            // 2 bytes. Accept exactly VERSION_1 (+MAC); MRG_RXBUF/CTRL_VQ/
            // EVENT_IDX/GSO are deliberately NOT accepted.
            mmio.write_u32(DEVICE_FEATURES_SEL as usize, 1);
            let feats_hi = mmio.read_u32(DEVICE_FEATURES as usize);
            if feats_hi & F_VERSION_1 == 0 {
                return Err("Device does not offer VIRTIO_F_VERSION_1");
            }
            mmio.write_u32(DEVICE_FEATURES_SEL as usize, 0);
            let feats_lo = mmio.read_u32(DEVICE_FEATURES as usize);
            // Word 0: accept only VIRTIO_NET_F_MAC (informative — we read the
            // MAC from config space either way), and only if offered.
            let accept_lo = feats_lo & F_NET_MAC;
            // Word 1: accept VIRTIO_F_VERSION_1.
            let accept_hi = feats_hi & F_VERSION_1;
            mmio.write_u32(DRIVER_FEATURES_SEL as usize, 0);
            mmio.write_u32(DRIVER_FEATURES as usize, accept_lo);
            mmio.write_u32(DRIVER_FEATURES_SEL as usize, 1);
            mmio.write_u32(DRIVER_FEATURES as usize, accept_hi);

            mmio.write_u32(STATUS as usize, S_ACKNOWLEDGE | S_DRIVER | S_FEATURES_OK);
            let status = mmio.read_u32(STATUS as usize);
            if status & S_FEATURES_OK == 0 {
                return Err("Device rejected negotiated features (features_ok cleared)");
            }

            // Read MAC address (from config space, offset 0x100).
            for i in 0..6usize {
                self.mac[i] = mmio.read_u8(CONFIG as usize + i);
            }

            // Read MTU — virtio-net config: mac[6], status u16 @6,
            // max_virtqueue_pairs u16 @8, mtu u16 @10 → MMIO 0x10a.
            // (0x106 was the old wrong offset: that is the STATUS field.)
            self.mtu = u16::from_le(mmio.read_u16(CONFIG as usize + 10));
            if self.mtu == 0 || self.mtu > 1500 {
                self.mtu = 1500; // No VIRTIO_NET_F_MTU negotiated
            }

            // R34: queue roles were INVERTED. Virtio-net 1.x with a single
            // queue pair is queue 0 = receiveq1, queue 1 = transmitq1 (spec
            // 5.1.3). The old code posted RX buffers on the transmit queue
            // and submitted TX chains on the receive queue — with a real
            // device neither direction could ever work.
            //
            // ========== Setup RX queue (Queue 0) ==========
            mmio.write_u32(QUEUE_SEL as usize, 0);

            // Read max queue size
            let max_queue_size = mmio.read_u32(QUEUE_NUM_MAX as usize);
            if max_queue_size == 0 {
                return Err("VirtIO device has zero queue size");
            }

            // Set queue size
            self.queue_size = if max_queue_size < 8 { 4 } else { 8 };

            // Create VirtQueue (single contiguous desc+avail+used allocation).
            // W32: virtio-mmio rejects notify writes that are not 32-bit.
            let rx_queue = match VirtQueue::with_notify_width(
                self.queue_size,
                0,  // queue_index: RX queue is queue 0
                self.base_addr + QUEUE_NOTIFY,
                self.base_addr + INTERRUPT_STATUS,
                self.base_addr + INTERRUPT_ACK,
                NotifyWidth::W32,
            ) {
                Some(q) => q,
                None => return Err("Failed to create RX VirtQueue"),
            };

            // Register the VirtQueue's OWN rings with the device (modern
            // virtio-mmio split-address layout). R32 fixed the addresses;
            // R34 fixed the register offsets they are written through.
            let rx_desc_phys = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(rx_queue.get_desc_addr())
            ).0;
            let rx_avail_phys = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(rx_queue.get_avail_addr())
            ).0;
            let rx_used_phys = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(rx_queue.get_used_addr())
            ).0;

            // Set queue count
            mmio.write_u32(QUEUE_NUM as usize, self.queue_size as u32);

            mmio.write_u32(QUEUE_DESC_LO as usize, (rx_desc_phys & 0xFFFFFFFF) as u32);
            mmio.write_u32(QUEUE_DESC_HI as usize, (rx_desc_phys >> 32) as u32);
            mmio.write_u32(QUEUE_DRIVER_LO as usize, (rx_avail_phys & 0xFFFFFFFF) as u32);
            mmio.write_u32(QUEUE_DRIVER_HI as usize, (rx_avail_phys >> 32) as u32);
            mmio.write_u32(QUEUE_DEVICE_LO as usize, (rx_used_phys & 0xFFFFFFFF) as u32);
            mmio.write_u32(QUEUE_DEVICE_HI as usize, (rx_used_phys >> 32) as u32);

            // Set queue ready
            mmio.write_u32(QUEUE_READY as usize, 1);

            *self.rx_queue.lock() = Some(rx_queue);

            // ========== Setup TX queue (Queue 1) ==========
            mmio.write_u32(QUEUE_SEL as usize, 1);

            let max_queue_size_tx = mmio.read_u32(QUEUE_NUM_MAX as usize);
            if max_queue_size_tx < self.queue_size as u32 {
                return Err("VirtIO TX queue smaller than RX queue");
            }

            let tx_queue = match VirtQueue::with_notify_width(
                self.queue_size,
                1,  // queue_index: TX queue is queue 1
                self.base_addr + QUEUE_NOTIFY,
                self.base_addr + INTERRUPT_STATUS,
                self.base_addr + INTERRUPT_ACK,
                NotifyWidth::W32,
            ) {
                Some(q) => q,
                None => return Err("Failed to create TX VirtQueue"),
            };

            let tx_desc_phys = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(tx_queue.get_desc_addr())
            ).0;
            let tx_avail_phys = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(tx_queue.get_avail_addr())
            ).0;
            let tx_used_phys = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(tx_queue.get_used_addr())
            ).0;

            // Set queue count
            mmio.write_u32(QUEUE_NUM as usize, self.queue_size as u32);

            mmio.write_u32(QUEUE_DESC_LO as usize, (tx_desc_phys & 0xFFFFFFFF) as u32);
            mmio.write_u32(QUEUE_DESC_HI as usize, (tx_desc_phys >> 32) as u32);
            mmio.write_u32(QUEUE_DRIVER_LO as usize, (tx_avail_phys & 0xFFFFFFFF) as u32);
            mmio.write_u32(QUEUE_DRIVER_HI as usize, (tx_avail_phys >> 32) as u32);
            mmio.write_u32(QUEUE_DEVICE_LO as usize, (tx_used_phys & 0xFFFFFFFF) as u32);
            mmio.write_u32(QUEUE_DEVICE_HI as usize, (tx_used_phys >> 32) as u32);

            // Set queue ready
            mmio.write_u32(QUEUE_READY as usize, 1);

            *self.tx_queue.lock() = Some(tx_queue);

            // Set driver status: DRIVER_OK (acknowledge | driver |
            // features_ok | driver_ok)
            mmio.write_u32(STATUS as usize, S_ACKNOWLEDGE | S_DRIVER | S_FEATURES_OK | S_DRIVER_OK);

            // Mark as initialized
            *self.initialized.lock() = true;

            // Fill initial RX buffers
            drop(());  // Release all locks
            self.refill_rx_buffers();

            Ok(())
        }
    }

    /// Get MAC address
    pub fn get_mac(&self) -> [u8; 6] {
        self.mac
    }

    /// Get MTU
    pub fn get_mtu(&self) -> u16 {
        self.mtu
    }

    /// Transmit packet
    ///
    /// # Parameters
    /// - `skb`: Packet to transmit
    ///
    /// # Returns
    /// 0 on success, negative error code on failure
    pub fn xmit(&self, skb: SkBuff) -> i32 {
        if !*self.initialized.lock_irqsave() {
            return -5; // EIO
        }

        // Get TX queue (irqsafe: IRQ handler may access rx/tx queues)
        let mut queue_guard = self.tx_queue.lock_irqsave();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return -5, // EIO
        };

        // INV-KPTR-1: one audited allocation; error paths below drop
        // it automatically.
        let hdr = match crate::safety::ptr::KPtr::new(VirtIONetHdr {
            flags: 0,
            gso_type: 0,
            hdr_len: 0,
            gso_size: 0,
            csum_start: 0,
            csum_offset: 0,
            num_buffers: 0, // unused on TX (spec 5.1.6.1)
        }) {
            Some(h) => h,
            None => return -12, // ENOMEM
        };
        let hdr_ptr = hdr.as_ptr();

        // VirtIO descriptor flags
        const VIRTQ_DESC_F_NEXT: u16 = 1;
        const VIRTQ_DESC_F_WRITE: u16 = 2;

        // Allocate two descriptors (chain_len=2: this header+data pair —
        // R24: passing the real chain length lets the limiter account net
        // TX chains correctly instead of the blk-shaped 3-desc default)
        let header_desc_idx = match queue.alloc_desc_chain(2) {
            Some(idx) => idx,
            None => return -5,  // EIO (hdr drops via KPtr)
        };
        let data_desc_idx = match queue.alloc_desc_chain(2) {
            Some(idx) => idx,
            None => return -5,  // EIO
        };

        // Set packet header descriptor (use physical address for DMA)
        let hdr_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(hdr_ptr as u64)
        ).0;
        queue.set_desc(
            header_desc_idx,
            hdr_phys,
            core::mem::size_of::<VirtIONetHdr>() as u32,
            VIRTQ_DESC_F_NEXT,
            data_desc_idx,
        );

        // Set data descriptor (use physical address for DMA)
        let data_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(skb.data as u64)
        ).0;
        queue.set_desc(
            data_desc_idx,
            data_phys,
            skb.len,
            0,  // Last descriptor
            0,
        );

        // Snapshot used.idx BEFORE submit: QEMU's iothread can complete the
        // TX before a post-submit get_used() runs, and a snapshot that
        // already includes our completion makes the wait below time out —
        // reporting a false EIO for every fast completion.
        let prev_used = queue.get_used();

        // Submit to available ring
        queue.submit(header_desc_idx);

        // Notify device
        queue.notify();

        // Wait for completion — with a BOUNDED spin budget.
        //
        // This wait runs while holding the tx_queue lock_irqsave: external
        // interrupts are disabled for the whole window. The old budget was
        // 10M iterations here plus a 50M-iteration late-drain below (tens
        // of seconds of IRQ blackout under QEMU TCG — review TIMING).
        // 200K iterations is a few ms of TCG time, orders of magnitude
        // beyond what a healthy virtio-net TX completion needs (the slirp
        // backend completes in microseconds); a genuinely stuck device now
        // fails fast instead of freezing every CPU's interrupts.
        const TX_WAIT_BUDGET: u64 = 200_000;
        let new_used = queue.wait_for_completion_max(prev_used, TX_WAIT_BUDGET);

        if new_used == prev_used {
            // R8-M2 / R21-N2 discipline (same as virtio-blk): the TX chain
            // is STILL SUBMITTED — the device may be DMA-reading hdr_ptr and
            // the skb data right now. Late-drain the used ring with a
            // bounded spin (500K iters — see TX_WAIT_BUDGET above; the old
            // 50M-iteration drain held the IRQ-off lock for tens of
            // seconds); on a true timeout LEAK both buffers instead of
            // freeing in-flight DMA targets.
            const TX_LATE_DRAIN_BUDGET: u64 = 500_000;
            let used_ring = queue.used_ring_ptr();
            let mut late = false;
            for _ in 0..TX_LATE_DRAIN_BUDGET {
                // SAFETY: used_ring points to this queue's used ring; offset 2
                // is the idx field (u16) within the ring structure.
                let idx = used_ring_idx(used_ring);
                if idx != prev_used {
                    late = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !late {
                let mut stats = self.stats.lock_irqsave();
                stats.tx_errors += 1;
                drop(stats);
                // hdr and skb deliberately NOT freed — device still owns
                // them. SkBuff's Drop releases its buffer, so forget() it
                // to make the leak explicit (R21-N2: integrity over a
                // leak); same for the KPtr'd header.
                core::mem::forget(skb);
                core::mem::forget(hdr);
                return -5;  // EIO
            }
            // Completed late — fall through to the normal cleanup.
        }

        // Free packet header (KPtr drop — INV-KPTR-3)
        drop(hdr);

        // Update statistics
        let mut stats = self.stats.lock_irqsave();
        stats.tx_packets += 1;
        stats.tx_bytes += skb.len as u64;

        // Free skb
        skb.free();

        0
    }

    /// Receive packet
    ///
    /// # Returns
    /// Received packet, or None if no packet available
    pub fn poll(&self) -> Option<SkBuff> {
        if !*self.initialized.lock_irqsave() {
            return None;
        }

        // Get RX queue (irqsafe: IRQ handler may access these)
        let mut queue_guard = self.rx_queue.lock_irqsave();
        let queue = queue_guard.as_mut()?;

        // Get last processed index
        let mut last_used = *self.rx_last_used.lock_irqsave();
        let current_used = queue.get_used();

        if last_used == current_used {
            return None; // No new packets
        }

        // Get completed descriptor from used ring
        let used_elem = queue.get_used_elem(last_used)?;

        // Update last_used
        last_used = last_used.wrapping_add(1);
        *self.rx_last_used.lock_irqsave() = last_used;

        let desc_idx = used_elem.id as u16;
        let desc = match queue.get_desc(desc_idx) {
            Some(d) => d,
            None => {
                // R24: bogus descriptor id from the device — the used entry
                // is consumed, so still account the buffer and refill
                // (was a bare `?` that lost the slot forever).
                drop(queue_guard);
                let stale = self.rx_buffers.lock_irqsave().pop();
                if let Some(addr) = stale {
                    self.dealloc_rx_buffer(addr);
                }
                self.refill_rx_buffers();
                return None;
            }
        };

        // desc.addr is the PHYSICAL (DMA) address programmed into the
        // descriptor; the kernel must touch the buffer through the linear
        // mapping. The old code used desc.addr directly as a kernel pointer
        // AND dealloc'd it — a physical address dereference that only
        // "worked" while nothing arrived (the RX rings were never armed).
        let buf_virt = crate::arch::mm::phys_to_virt(
            crate::arch::mm::PhysAddr::new(desc.addr)
        ).bits();

        // VirtIO-Net packet structure:
        // - 12 bytes VirtIONetHdr
        // - Followed by Ethernet frame data
        let total_len = used_elem.len as usize;
        if total_len <= core::mem::size_of::<VirtIONetHdr>() {
            // R24 (R14 MED "poll 早退泄漏 RX 缓冲"): every early return after
            // this point has consumed a used entry — the buffer must be
            // recycled or the buffer AND its descriptor are lost forever
            // (enough of them wedge RX at zero posted buffers).
            drop(queue_guard);
            self.recycle_rx_buffer(buf_virt);
            return None; // Data too short
        }

        let pkt_data_len = total_len - core::mem::size_of::<VirtIONetHdr>();
        // SAFETY: buf_virt is the linear-mapping address of the device-completed
        // RX buffer of total_len bytes; the buffer remains valid until after
        // this function returns.
        let hdr_and_data = unsafe {
            core::slice::from_raw_parts(buf_virt as *const u8, total_len)
        };

        // Skip VirtIO-Net header, keep only Ethernet frame
        let eth_data = &hdr_and_data[core::mem::size_of::<VirtIONetHdr>()..];

        // Create SkBuff
        let mut skb = match crate::net::buffer::alloc_skb(pkt_data_len as u32 + 64) {
            Some(skb) => skb,
            None => {
                // R24: recycle on allocation failure (was a leak).
                drop(queue_guard);
                self.recycle_rx_buffer(buf_virt);
                return None;
            }
        };
        if skb.skb_put_data(eth_data).is_err() {
            // R24: recycle and free the just-allocated skb (was a leak).
            skb.free();
            drop(queue_guard);
            self.recycle_rx_buffer(buf_virt);
            return None;
        }

        // Update statistics. The stats guard MUST be dropped before
        // recycle_rx_buffer(): recycle -> refill_rx_buffers re-takes
        // rx_queue, so a guard held across line 623 inverts the lock
        // order against the rx_queue -> stats nesting six lines up
        // (this poll on another CPU holds rx_queue and waits for stats)
        // — the two-CPU AB-BA wedge the watchdog flagged as
        // lock=VIRTIO_NET+0x68 (rx_queue) vs +0xE8 (stats) under
        // sustained RX (hdc file send, boot lr1/lr2).
        {
            let mut stats = self.stats.lock_irqsave();
            stats.rx_packets += 1;
            stats.rx_bytes += pkt_data_len as u64;
        }

        // Free old RX buffer and post a replacement
        drop(queue_guard);
        self.recycle_rx_buffer(buf_virt);

        Some(skb)
    }

    /// R24: recycle one RX buffer — dealloc the DMA buffer (same layout as
    /// refill_rx_buffers allocates) and post a replacement. Every poll()
    /// path that consumes a used-ring entry must run this; previously only
    /// the full-success path did, so any early return permanently lost the
    /// buffer and its descriptor.
    ///
    /// `addr` is the buffer's KERNEL VIRTUAL address (as tracked in
    /// rx_buffers), not the physical address stored in the descriptor.
    fn recycle_rx_buffer(&self, addr: u64) {
        self.dealloc_rx_buffer(addr);
        // Pop one entry from rx_buffers to reflect the freed buffer,
        // so refill_rx_buffers() knows to allocate a replacement.
        self.rx_buffers.lock_irqsave().pop();
        self.refill_rx_buffers();
    }

    /// Dealloc one RX buffer with the layout refill_rx_buffers() allocates:
    ///   buf_size = size_of::<VirtIONetHdr>() + mtu + 64, align = 64
    fn dealloc_rx_buffer(&self, addr: u64) {
        let buf_size = core::mem::size_of::<VirtIONetHdr>() + self.mtu as usize + 64;
        // SAFETY: Buffer was allocated with identical layout in refill_rx_buffers();
        // from_size_align cannot fail because buf_size and align are the same constants.
        unsafe {
            if let Ok(layout) = alloc::alloc::Layout::from_size_align(buf_size, 64) {
                alloc::alloc::dealloc(addr as *mut u8, layout);
            } else {
                crate::pr_err!("virtio_net: invalid RX dealloc layout buf_size={}", buf_size);
            }
        }
    }

    /// Refill RX buffers
    fn refill_rx_buffers(&self) {
        let mut queue_guard = self.rx_queue.lock_irqsave();
        let queue = match queue_guard.as_mut() {
            Some(q) => q,
            None => return,
        };

        let mut rx_buffers = self.rx_buffers.lock_irqsave();

        // Check how many buffers need to be filled
        let need_refill = self.queue_size as usize - rx_buffers.len();

        for _ in 0..need_refill.min(4) {  // Fill at most 4 at a time
            // Allocate RX buffer (VirtIO-Net header + MTU + some margin)
            let buf_size = core::mem::size_of::<VirtIONetHdr>() + self.mtu as usize + 64;
            let layout = alloc::alloc::Layout::from_size_align(buf_size, 64);
            let layout = match layout {
                Ok(l) => l,
                Err(_) => continue,
            };

            // SAFETY: Layout is valid (checked above); null check follows immediately.
            let buf_ptr = unsafe { alloc::alloc::alloc(layout) as *mut u8 };
            if buf_ptr.is_null() {
                continue;
            }

            // Allocate descriptor (single-descriptor chain — R24 chain
            // limiter fix: 1-desc RX buffers now fill the whole ring
            // instead of stopping at 2 under the blk-shaped *3 guard).
            let desc_idx = match queue.alloc_desc_chain(1) {
                Some(idx) => idx,
                None => {
                    // SAFETY: buf_ptr was just allocated with layout; no submit occurred.
                    unsafe { alloc::alloc::dealloc(buf_ptr, layout); }
                    continue;
                }
            };

            // Set descriptor — DMA needs physical address
            // SAFETY: buf_ptr is a valid kernel virtual address from alloc; virt_to_phys
            // converts to the corresponding physical address for DMA.
            let buf_phys = crate::arch::mm::virt_to_phys(
                crate::arch::mm::VirtAddr::new(buf_ptr as u64)
            ).0 as u64;
            // VIRTQ_DESC_F_WRITE = 2 means device can write
            queue.set_desc(desc_idx, buf_phys, buf_size as u32, 2, 0);

            // Record buffer address
            rx_buffers.push(buf_ptr as u64);

            // Submit to available ring
            queue.submit(desc_idx);
        }
    }

    /// Get statistics
    pub fn get_stats(&self) -> DeviceStats {
        *self.stats.lock_irqsave()
    }
}

/// VirtIO network device transmit function (for NetDevice calls)
fn virtio_net_xmit(skb: SkBuff) -> i32 {
    // (Registry access through the audited get_device accessor.)
    if let Some(device) = virtio_net_get_device() {
        device.xmit(skb)
    } else {
        skb.free();
        -5 // EIO
    }
}

/// VirtIO network device statistics function
fn virtio_net_get_stats() -> DeviceStats {
    if let Some(device) = virtio_net_get_device() {
        device.get_stats()
    } else {
        DeviceStats::default()
    }
}

/// VirtIO network device operation interface
static VIRTIO_NET_OPS: NetDeviceOps = NetDeviceOps {
    xmit: virtio_net_xmit,
    init: None,
    uninit: None,
    get_stats: Some(virtio_net_get_stats),
};


impl crate::safety::slot::ExclusivityProof for Eth0Phase {}
static NO_LOCK_NEEDED: Eth0Phase = Eth0Phase;

static VIRTIO_NET: crate::safety::slot::OnceSlot<VirtIONetDevice> =
    crate::safety::slot::OnceSlot::new();
static VIRTIO_NET_DEVICE: crate::safety::slot::OnceSlot<NetDevice> =
    crate::safety::slot::OnceSlot::new();

/// Initialize VirtIO network device
///
/// # Parameters
/// - `base_addr`: MMIO base address (QEMU virt platform typically 0x10001000)
pub fn virtio_net_init(base_addr: u64) -> Result<(), &'static str> {
    // Called once during kernel init (INV-SLOT-1/2).
    let mut device = VirtIONetDevice::new(base_addr);
    device.init()?;
    publish_eth0(device);
    Ok(())
}

/// Publish a fully initialized VirtIONetDevice as eth0 (shared tail of the
/// MMIO and PCI transport init paths).
///
/// Called once during kernel init from a single thread (the OnceSlots
/// own the publication protocol).
fn publish_eth0(device: VirtIONetDevice) {
    // Get MAC address
    let mac = device.get_mac();

    // Create NetDevice
    let mut net_device = NetDevice {
        name: [0u8; 16],
        ifindex: 0,
        mtu: device.get_mtu() as u32,
        type_: ArpHrdType::ARPHRD_ETHER,
        addr: [0u8; 32],
        addr_len: 6,
        netdev_ops: &VIRTIO_NET_OPS,
        priv_: core::ptr::null_mut(),
        stats: DeviceStats::default(),
        flags: dev_flags::IFF_UP | dev_flags::IFF_RUNNING | dev_flags::IFF_BROADCAST,
        rx_queue_len: 0,
    };

    // Set device name
    let name = b"eth0\0";
    net_device.name[..name.len()].copy_from_slice(name);

    // Set MAC address
    net_device.set_address(&mac, 6);

    // Store device (INV-SLOT-1/2), then register; the pre-publication
    // mutable fill below runs before any reader can exist.
    VIRTIO_NET.publish(device);
    VIRTIO_NET_DEVICE.publish_with_addr(|_| net_device);
    if let Some(dev) = VIRTIO_NET_DEVICE.get_mut_locked(&NO_LOCK_NEEDED) {
        crate::drivers::hotpath::register_netdevice(dev);
    }
}

/// Get VirtIO network device
pub fn virtio_net_get_device() -> Option<&'static VirtIONetDevice> {
    VIRTIO_NET.get()
}

/// Get VirtIO network device's NetDevice
pub fn get_net_device() -> Option<&'static mut NetDevice> {
    VIRTIO_NET_DEVICE.get_mut_locked(&NO_LOCK_NEEDED)
}

/// Get VirtIO network device's base address
fn get_device_base_addr() -> Option<u64> {
    VIRTIO_NET.get().map(|dev| dev.base_addr)
}

/// VirtIO-Net interrupt handler (top half)
///
/// Called when VirtIO-Net device generates interrupt.
/// Only acknowledges the interrupt and defers packet processing
/// to NetRx softirq bottom half.
/// Registered via request_irq. EOI is done by the IRQ framework.
pub fn virtio_net_interrupt_handler(_irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    // Get device base address
    let base_addr = match get_device_base_addr() {
        Some(addr) => addr,
        None => return crate::interrupt::IrqReturn::None,
    };

    // INV-MMIO-1..3: the device's register window (same mapping the
    // init path proved), register offsets per VirtIO spec.
    let mmio = crate::safety::mmio::MmioRegion::new(base_addr as usize, 0x120);
    // Read interrupt status (INTERRUPT_STATUS at 0x60)
    let irq_status = mmio.read_u32(0x60);
    if irq_status != 0 {
        // Clear interrupt (INTERRUPT_ACK at 0x64)
        mmio.write_u32(0x64, irq_status);

        // Defer packet processing to NetRx softirq bottom half
        crate::interrupt::softirq::raise_softirq_irqoff(
            crate::interrupt::softirq::SoftirqIndex::NetRx as usize,
        );
        return crate::interrupt::IrqReturn::Handled;
    }
    crate::interrupt::IrqReturn::None
}

/// NetRx softirq handler (bottom half).
///
/// Processes received network packets deferred from the interrupt handler.
/// Runs in softirq context (at `irq_exit` time or from ksoftirqd).
pub fn net_rx_softirq_handler(_vec: usize) {
    crate::api::net::ethernet_poll();
}

/// Enable VirtIO-Net device interrupt
///
/// Registers the handler via request_irq.
pub fn virtio_net_enable_device_interrupt(base_addr: u64) {
    const VIRTIO_MMIO_BASE: u64 =
        crate::arch::mm::memory_layout::mmio_alias(0x10001000);
    const VIRTIO_MMIO_SIZE: u64 = 0x1000;

    let slot = ((base_addr - VIRTIO_MMIO_BASE) / VIRTIO_MMIO_SIZE) as u32;
    let irq = (slot + 1) as u32;  // IRQ 1-8

    crate::pr_info!("virtio-net: Registering IRQ {} for device at 0x{:x} (slot {})", irq, base_addr, slot);

    // Register handler via IRQ framework (unmasks automatically)
    crate::interrupt::request_irq(
        irq,
        virtio_net_interrupt_handler,
        0,
        "virtio-net",
        base_addr as usize,
    ).ok();
}

// ============================================================================
// PCI transport (virtio-net-pci)
//
// The MMIO `init()` path drives the virtio-mmio register block (riscv64
// QEMU virt). This path drives the same VirtIO 1.0 state machine through
// the PCI common config parsed by VirtIOPCI — on x86_64/q35 the firmware
// (SeaBIOS) has already assigned the BARs, exactly like the virtio-blk PCI
// path. Queue roles follow the single queue pair the driver negotiates:
// queue 0 = receiveq1, queue 1 = transmitq1 (virtio spec 5.1.3).
// ============================================================================

/// ISR capability address of the PCI net device (riscv64 PLIC path top
/// half; the x86_64 INTx dispatcher keeps its own registry).
#[cfg(feature = "riscv64")]
static PCI_ISR_ADDR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Select `queue_index` in the PCI common config and return its maximum
/// size (0 = queue not available).
fn pci_select_queue(pci_dev: &VirtIOPCI, queue_index: u16) -> u16 {
    // INV-MMIO-1..3: the common-config window (BAR mapping proven at
    // probe).
    let cfg = crate::safety::mmio::MmioRegion::new(pci_dev.common_cfg_bar as usize, 0x100);
    cfg.write_u16(
        crate::drivers::hotpath::COMMON_CFG_QUEUE_SELECT as usize,
        queue_index,
    );
    cfg.read_u16(crate::drivers::hotpath::COMMON_CFG_QUEUE_SIZE as usize)
}

/// Program a smaller queue size than the device maximum.
///
/// `setup_queue` (virtio_pci.rs) only publishes ring addresses and enables
/// the queue; the size register must be programmed while the queue is
/// selected and still disabled.
fn pci_set_queue_size(pci_dev: &VirtIOPCI, queue_size: u16) {
    // Queue still selected by pci_select_queue and not yet enabled.
    let cfg = crate::safety::mmio::MmioRegion::new(pci_dev.common_cfg_bar as usize, 0x100);
    cfg.write_u16(
        crate::drivers::hotpath::COMMON_CFG_QUEUE_SIZE as usize,
        queue_size,
    );
}

/// Initialize a PCI virtio-net device.
///
/// On success the device is DRIVER_OK, its queues hold the posted RX
/// buffers, and eth0 is registered (the MMIO path's global device and
/// NetDevice singletons are reused).
pub fn init_pci(pci_dev: &mut VirtIOPCI) -> Result<(), &'static str> {
    use crate::drivers::hotpath::status;

    // Word 0: accept only VIRTIO_NET_F_MAC. Word 1 VIRTIO_F_VERSION_1 is
    // added unconditionally by write_driver_features (required for
    // modern-only devices with disable-legacy=on).
    const F_NET_MAC: u32 = 1 << 5;
    // RX ring depth: the MMIO path runs with 8 and DHCP over slirp is
    // proven at that depth; 32 gives headroom without 256 x ~1.6KB of
    // posted RX buffers on a 256-deep default queue.
    const QUEUE_SIZE_CAP: u16 = 32;

    // VirtIO 1.0 state machine: reset, ACK|DRIVER, features, FEATURES_OK.
    pci_dev.reset_device();
    let mut reset_timeout = crate::config::VIRTIO_RESET_TIMEOUT_TICKS;
    while pci_dev.get_status() != 0 && reset_timeout > 0 {
        core::hint::spin_loop();
        reset_timeout -= 1;
    }
    if pci_dev.get_status() != 0 {
        return Err("virtio-net-pci: reset did not complete");
    }

    pci_dev.set_status(status::ACKNOWLEDGE | status::DRIVER);

    let features = pci_dev.read_device_features();
    pci_dev.write_driver_features(features & F_NET_MAC);

    pci_dev.set_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);
    if pci_dev.get_status() & status::FEATURES_OK == 0 {
        return Err("virtio-net-pci: device rejected negotiated features");
    }

    // Device config (virtio_net_config): mac[6] at +0, status u16 at +6,
    // max_virtqueue_pairs u16 at +8, mtu u16 at +10.
    let cfg_base = pci_dev.device_cfg_bar;
    if cfg_base == 0 {
        return Err("virtio-net-pci: no device config capability");
    }
    let mut mac = [0u8; 6];
    let mut mtu;
    // INV-MMIO-1..3: the device-config capability window; bytes 0..6
    // and 10..12 are the virtio-net config fields.
    let cfg = crate::safety::mmio::MmioRegion::new(cfg_base as usize, 12);
    for i in 0..6usize {
        mac[i] = cfg.read_u8(i);
    }
    {
        let dev_mtu = u16::from_le(cfg.read_u16(10));
        // 0 means VIRTIO_NET_F_MTU was not offered — fall back to 1500.
        mtu = dev_mtu;
        if mtu == 0 || mtu > 1500 {
            mtu = 1500;
        }
    }

    // RX queue (queue 0).
    let rx_max = pci_select_queue(pci_dev, 0);
    if rx_max == 0 {
        return Err("virtio-net-pci: no RX queue");
    }
    let queue_size = if rx_max < 8 { rx_max } else { rx_max.min(QUEUE_SIZE_CAP) };
    pci_set_queue_size(pci_dev, queue_size);
    let isr_base = pci_dev.isr_cfg_bar;
    let rx_queue = VirtQueue::new(
        queue_size,
        0,
        pci_dev.get_notify_addr(0),
        isr_base,
        isr_base,
    )
    .ok_or("virtio-net-pci: RX vring allocation failed")?;
    pci_dev.setup_queue(0, &rx_queue)?;

    // TX queue (queue 1).
    let tx_max = pci_select_queue(pci_dev, 1);
    if tx_max < queue_size {
        return Err("virtio-net-pci: TX queue smaller than RX queue");
    }
    pci_set_queue_size(pci_dev, queue_size);
    let tx_queue = VirtQueue::new(
        queue_size,
        1,
        pci_dev.get_notify_addr(1),
        isr_base,
        isr_base,
    )
    .ok_or("virtio-net-pci: TX vring allocation failed")?;
    pci_dev.setup_queue(1, &tx_queue)?;

    pci_dev.set_status(
        status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK,
    );

    // IRQ hookup. x86_64/q35: shared PIC line via the INTx dispatcher.
    // riscv64: PCIe swizzle formula (same as virtio-blk-pci).
    #[cfg(feature = "x86_64")]
    {
        pci_dev.enable_pci_intx_irq(intx_net_service);
    }
    #[cfg(feature = "riscv64")]
    {
        PCI_ISR_ADDR.store(pci_dev.isr_cfg_bar, core::sync::atomic::Ordering::Release);
        let int_pin = pci_dev.pci_config.read_config_byte(0x3D);
        let irq = 32 + (((int_pin as u32).saturating_sub(1) + pci_dev.pci_slot as u32) % 4);
        // IRQF_SHARED, same as virtio-blk-pci: the gpex swizzle maps
        // several functions onto IRQs 32..35, and this registration runs
        // AFTER init_pci_block_devices claimed those lines. A non-shared
        // registration on an already-claimed line is refused by
        // request_irq ("IRQ already in use and not shared") and the .ok()
        // silently dropped it — the net function then had no top half and
        // RX interrupts never fired on any line shared with a disk.
        crate::interrupt::request_irq(
            irq,
            virtio_net_interrupt_handler_pci,
            crate::interrupt::irqdesc::IRQF_SHARED,
            "virtio-net-pci",
            pci_dev.isr_cfg_bar as usize,
        )
        .ok();
    }

    crate::pr_info!(
        "virtio-net-pci: eth0 up, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, queues rx0/tx1 depth {}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
        queue_size
    );

    // Called once during single-threaded kernel init (INV-SLOT-1/2).
    {
        let mut device = VirtIONetDevice::new(0);
        device.mac = mac;
        device.mtu = mtu;
        device.queue_size = queue_size;
        *device.rx_queue.lock() = Some(rx_queue);
        *device.tx_queue.lock() = Some(tx_queue);
        *device.initialized.lock() = true;

        // Post initial RX buffers (ethernet_poll drains them from the
        // NetRx softirq the INTx/PLIC top half raises). Runs on the local
        // device before publication — nothing can poll eth0 yet.
        device.refill_rx_buffers();

        publish_eth0(device);
    }

    Ok(())
}

/// NetRx-completion service for the x86_64 shared INTx dispatcher: the
/// dispatcher already read (and thereby acknowledged) this device's ISR
/// capability; hand packet processing to the NetRx softirq bottom half.
#[cfg(feature = "x86_64")]
pub fn intx_net_service() {
    crate::interrupt::softirq::raise_softirq_irqoff(
        crate::interrupt::softirq::SoftirqIndex::NetRx as usize,
    );
}

/// VirtIO-Net PCI interrupt handler (top half, riscv64 PLIC path).
///
/// Reads the ISR capability (which acknowledges the device's INTx) and
/// defers packet processing to the NetRx softirq.
#[cfg(feature = "riscv64")]
pub fn virtio_net_interrupt_handler_pci(_irq: u32, _dev_id: usize) -> crate::interrupt::IrqReturn {
    let isr_addr = PCI_ISR_ADDR.load(core::sync::atomic::Ordering::Acquire);
    if isr_addr == 0 {
        return crate::interrupt::IrqReturn::None;
    }
    // INV-MMIO-1..3: isr_addr was stored from a VirtIOPCI whose ISR
    // capability is MMIO-mapped; the read is the device-side interrupt
    // acknowledge.
    let isr = crate::safety::mmio::MmioRegion::new(isr_addr as usize, 4).read_u32(0);
    if isr == 0 {
        // Not ours — spurious or another device on the shared line.
        return crate::interrupt::IrqReturn::None;
    }
    crate::interrupt::softirq::raise_softirq_irqoff(
        crate::interrupt::softirq::SoftirqIndex::NetRx as usize,
    );
    crate::interrupt::IrqReturn::Handled
}

// ════════════════ gpu ═════════════════════════════════
// (was gpu/mod.rs)
// GPU driver module
//
// Provides graphics display support
//
// Current implementation:
// - VirtIO-GPU driver (compliant with VirtIO 1.2 specification)
// - Simplified MMIO framebuffer (QEMU RISC-V virt)
/// Global GPU device storage
/// Used for flushing framebuffer
static GPU_DEVICE: Spinlock<Option<VirtioGpuDevice>> = Spinlock::new(None);

// (FRAMEBUFFER_INFO registry hoisted to crate::console — rung4 prep
// move 3; the driver-side twins delegate so historical callers compile.)

/// Set global framebuffer info (delegates to the console registry).
pub fn set_framebuffer_info(info: FrameBufferInfo) {
    crate::console::set_framebuffer_info(crate::console::FramebufferInfo {
        addr: info.addr,
        size: info.size,
        width: info.width,
        height: info.height,
        stride: info.stride,
        format: info.format,
    });
}

/// Get global framebuffer info (delegates to the console registry).
pub fn get_framebuffer_info() -> Option<FrameBufferInfo> {
    crate::console::get_framebuffer_info().map(|i| FrameBufferInfo {
        addr: i.addr,
        size: i.size,
        width: i.width,
        height: i.height,
        stride: i.stride,
        format: i.format,
    })
}

/// Set global GPU device (called during initialization)
pub fn set_gpu_device(device: VirtioGpuDevice) {
    *GPU_DEVICE.lock() = Some(device);
}

/// Flush framebuffer to display device
/// VirtIO-GPU requires explicit flush to display updated content
pub fn flush_framebuffer() -> bool {
    // Snapshot the device pointer under the lock and RELEASE before the
    // flush. device.flush() submits virtio commands and polls for the
    // device response, which under TCG/BQL contention can take hundreds
    // of ms; holding GPU_DEVICE across that stall made every concurrent
    // flusher (kfbflush every 30ms, each FBIO_FLUSH ioctl) spin on this
    // lock for a whole device transaction — observed as
    // "DEADLOCK: spinlock stuck cpu=1 lock=GPU_DEVICE" in soak-r1
    // (smp4 MTTCG + I/O churn, t=948s).
    // SAFETY: GPU_DEVICE is a static set once by set_gpu_device() during
    // boot and never cleared or replaced afterwards, so the pointed-to
    // VirtioGpuDevice outlives this function; flush() takes &self only.
    let device: *const VirtioGpuDevice = {
        let guard = GPU_DEVICE.lock();
        match guard.as_ref() {
            Some(d) => d as *const VirtioGpuDevice,
            None => return false,
        }
    };
    unsafe { (*device).flush() };
    true
}

/// Run `f` with the global VirtIO-GPU device (None when no GPU was
/// probed). Same lock discipline as flush_framebuffer: the device
/// pointer is snapshotted and the lock released BEFORE `f` runs — virtio
/// transactions inside `f` can take milliseconds under TCG and must not
/// hold GPU_DEVICE.
// SAFETY: GPU_DEVICE is set once during boot and never cleared; the
// pointed-to VirtioGpuDevice outlives the call (see flush_framebuffer).
pub fn with_gpu_device<R>(f: impl FnOnce(&VirtioGpuDevice) -> R) -> Option<R> {
    let device: *const VirtioGpuDevice = {
        let guard = GPU_DEVICE.lock();
        match guard.as_ref() {
            Some(d) => d as *const VirtioGpuDevice,
            None => return None,
        }
    };
    Some(f(unsafe { &*device }))
}

// ════════════════ gpu::framebuffer ════════════════════
// (was gpu/framebuffer.rs)
// Framebuffer basic drawing interface
//
// Provides basic pixel-level drawing operations
/// Color constants (xRGB format)
pub mod color {
    pub const BLACK: u32 = 0xFF000000;
    pub const WHITE: u32 = 0xFFFFFFFF;
    pub const RED: u32 = 0xFFFF0000;
    pub const GREEN: u32 = 0xFF00FF00;
    pub const BLUE: u32 = 0xFF0000FF;
    pub const YELLOW: u32 = 0xFFFFFF00;
    pub const CYAN: u32 = 0xFF00FFFF;
    pub const MAGENTA: u32 = 0xFFFF00FF;
    pub const GRAY: u32 = 0xFF808080;
    pub const DARK_GRAY: u32 = 0xFF404040;
    pub const LIGHT_BLUE: u32 = 0xFF0000FF;
}


unsafe impl Send for FrameBuffer {}
unsafe impl Sync for FrameBuffer {}

impl FrameBuffer {
    /// Create a new Framebuffer
    ///
    /// INV-MMIO-1 (the audited construction): `addr` is the framebuffer
    /// aperture the display driver mapped (identity-mapped for now) and
    /// `info` describes its geometry; the window covers the whole frame.
    pub fn new(addr: u64, info: FrameBufferInfo) -> Self {
        let len = (info.stride as u32 * info.height as u32 * 4) as usize;
        Self {
            info,
            fb: crate::safety::mmio::MmioRegion::new(addr as usize, len.max(4)),
        }
    }

    /// Get width
    #[inline]
    pub fn width(&self) -> u32 {
        self.info.width
    }

    /// Get height
    #[inline]
    pub fn height(&self) -> u32 {
        self.info.height
    }

    /// Get bytes per row
    #[inline]
    pub fn stride(&self) -> u32 {
        self.info.stride
    }

    /// Draw a single pixel
    #[inline]
    pub fn put_pixel(&self, x: u32, y: u32, color: u32) {
        if x >= self.width() || y >= self.height() {
            return;
        }

        self.fb
            .write_u32((y * self.stride() + x * 4) as usize, color);
    }

    /// Get pixel color
    #[inline]
    pub fn get_pixel(&self, x: u32, y: u32) -> u32 {
        if x >= self.width() || y >= self.height() {
            return 0;
        }

        self.fb.read_u32((y * self.stride() + x * 4) as usize)
    }

    /// Fill a rectangle
    pub fn fill_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32) {
        let x_end = (x + width).min(self.width());
        let y_end = (y + height).min(self.height());

        for py in y..y_end {
            for px in x..x_end {
                self.put_pixel(px, py, color);
            }
        }
    }

    /// Draw a rectangle border
    pub fn blit_rect(&self, x: u32, y: u32, width: u32, height: u32, color: u32, thickness: u32) {
        // Guard against underflow when thickness > width/height
        if thickness > width || thickness > height {
            return;
        }
        // Top edge
        self.fill_rect(x, y, width, thickness, color);
        // Bottom edge
        self.fill_rect(x, y + height - thickness, width, thickness, color);
        // Left edge
        self.fill_rect(x, y, thickness, height, color);
        // Right edge
        self.fill_rect(x + width - thickness, y, thickness, height, color);
    }

    /// Clear the screen
    pub fn clear(&self, color: u32) {
        self.fill_rect(0, 0, self.width(), self.height(), color);
    }

    /// Draw a horizontal line
    pub fn draw_line_h(&self, x: u32, y: u32, width: u32, color: u32) {
        self.fill_rect(x, y, width, 1, color);
    }

    /// Draw a vertical line
    pub fn draw_line_v(&self, x: u32, y: u32, height: u32, color: u32) {
        self.fill_rect(x, y, 1, height, color);
    }

    /// Draw a line segment (Bresenham's algorithm)
    pub fn draw_line(&self, x0: u32, y0: u32, x1: u32, y1: u32, color: u32) {
        let mut x0 = x0 as i32;
        let mut y0 = y0 as i32;
        let x1 = x1 as i32;
        let y1 = y1 as i32;

        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };

        let mut err = dx + dy;

        loop {
            self.put_pixel(x0 as u32, y0 as u32, color);

            if x0 == x1 && y0 == y1 {
                break;
            }

            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Draw a circle
    pub fn draw_circle(&self, cx: u32, cy: u32, radius: u32, color: u32, fill: bool) {
        let cx = cx as i32;
        let cy = cy as i32;
        let radius = radius as i32;

        if fill {
            // Filled circle
            for y in -radius..=radius {
                for x in -radius..=radius {
                    if x * x + y * y <= radius * radius {
                        self.put_pixel((cx + x) as u32, (cy + y) as u32, color);
                    }
                }
            }
        } else {
            // Hollow circle (Midpoint algorithm)
            let mut x = radius;
            let mut y = 0i32;
            let mut err = 0i32;

            while x >= y {
                self.put_pixel((cx + x) as u32, (cy + y) as u32, color);
                self.put_pixel((cx + y) as u32, (cy + x) as u32, color);
                self.put_pixel((cx - y) as u32, (cy + x) as u32, color);
                self.put_pixel((cx - x) as u32, (cy + y) as u32, color);
                self.put_pixel((cx - x) as u32, (cy - y) as u32, color);
                self.put_pixel((cx - y) as u32, (cy - x) as u32, color);
                self.put_pixel((cx + y) as u32, (cy - x) as u32, color);
                self.put_pixel((cx + x) as u32, (cy - y) as u32, color);

                y += 1;
                err += 1 + 2 * y;
                if 2 * (err - x) + 1 > 0 {
                    x -= 1;
                    err += 1 - 2 * x;
                }
            }
        }
    }

    /// Draw a bitmap
    pub fn draw_bitmap(&self, x: u32, y: u32, width: u32, height: u32, data: &[u8], color: u32) {
        for py in 0..height {
            for px in 0..width {
                let byte_index = ((py * width + px) / 8) as usize;
                let bit_index = 7 - ((py * width + px) % 8);

                if byte_index < data.len() {
                    let bit = (data[byte_index] >> bit_index) & 1;
                    if bit != 0 {
                        self.put_pixel(x + px, y + py, color);
                    }
                }
            }
        }
    }

    /// Get framebuffer starting address (escape hatch for DMA-style
    /// bulk copies; INV-MMIO-1 covers the window).
    #[inline]
    pub fn as_ptr(&self) -> *mut u8 {
        self.fb.base() as *mut u8
    }

    /// Get framebuffer information
    #[inline]
    pub fn info(&self) -> &FrameBufferInfo {
        &self.info
    }
}

// ════════════════ gpu::virtio_cmd ═════════════════════
// (was gpu/virtio_cmd.rs)
// VirtIO-GPU command constants
/// 2D command types
pub mod cmd {
    // Control queue commands
    pub const GET_DISPLAY_INFO: u32 = 0x0100;
    pub const RESOURCE_CREATE_2D: u32 = 0x0101;
    pub const RESOURCE_UNREF: u32 = 0x0102;
    pub const SET_SCANOUT: u32 = 0x0103;
    pub const RESOURCE_FLUSH: u32 = 0x0104;
    pub const TRANSFER_TO_HOST_2D: u32 = 0x0105;
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
    pub const RESOURCE_DETACH_BACKING: u32 = 0x0107;
    pub const GET_CAPSET_INFO: u32 = 0x0108;
    pub const GET_CAPSET: u32 = 0x0109;

    // Cursor queue commands
    pub const UPDATE_CURSOR: u32 = 0x0200;
    pub const MOVE_CURSOR: u32 = 0x0201;

    // Response types
    pub const RESP_OK_NODATA: u32 = 0x1100;
    pub const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
    pub const RESP_OK_CAPSET_INFO: u32 = 0x1102;
    pub const RESP_OK_CAPSET: u32 = 0x1103;
    pub const RESP_ERR_UNSPEC: u32 = 0x1200;
    pub const RESP_ERR_OUT_OF_MEMORY: u32 = 0x1201;
    pub const RESP_ERR_INVALID_SCANOUT_ID: u32 = 0x1202;
    pub const RESP_ERR_INVALID_RESOURCE_ID: u32 = 0x1203;
    pub const RESP_ERR_INVALID_CONTEXT_ID: u32 = 0x1204;
    pub const RESP_ERR_INVALID_PARAMETER: u32 = 0x1205;
}

/// Format constants — virtio-gpu 1.2 spec enum virtio_gpu_formats.
/// The 3/4 vs 67/68 columns were previously SWAPPED (R8G8B8X8 sent as 4
/// = X8R8G8B8 to the device), which QEMU happily accepted while
/// scrambling the DRM XBGR8888 scanout channels (measured via
/// drm_probe + screendump: red channel read the X byte).
pub mod format {
    pub const B8G8R8A8_UNORM: u32 = 1;
    pub const B8G8R8X8_UNORM: u32 = 2;
    pub const A8R8G8B8_UNORM: u32 = 3;
    pub const X8R8G8B8_UNORM: u32 = 4;
    pub const R8G8B8A8_UNORM: u32 = 67;
    pub const R8G8B8X8_UNORM: u32 = 68;
    pub const A8B8G8R8_UNORM: u32 = 121;
    pub const X8B8G8R8_UNORM: u32 = 122;
}

// ════════════════ gpu::virtio_gpu ═════════════════════
// (was gpu/virtio_gpu.rs)
// VirtIO-GPU device driver
//
// Implements VirtIO-GPU PCI device initialization and framebuffer management
/// VirtIO-GPU vendor ID (Red Hat)
const VIRTIO_GPU_PCI_VENDOR: u16 = 0x1AF4;

/// VirtIO queue indices
const CTRL_QUEUE: u16 = 0;   // Control queue
const CURSOR_QUEUE: u16 = 1; // Cursor queue


unsafe impl Send for VirtioGpuDevice {}
unsafe impl Sync for VirtioGpuDevice {}

impl VirtioGpuDevice {
    /// Create a new VirtIO-GPU device
    pub fn new(pci: VirtIOPCI) -> Option<Self> {
        let mut device = Self {
            pci,
            ctrl_queue: None,
            cmd_lock: crate::sync::types::Spinlock::new(()),
            fb_info: None,
            fb_ptr: core::ptr::null_mut(),
            fb_layout: None,
            resource_id: 1,
            display_rect: Rect::default(),
        };

        // Initialize VirtIO device
        device.init_virtio()?;

        Some(device)
    }

    /// Initialize VirtIO device
    fn init_virtio(&mut self) -> Option<()> {
        // INV-MMIO-1: the common-config and ISR windows are the BARs the
        // PCI probe mapped for this function (MmioRegion owns the
        // volatile words from here down — round-5).
        let common = crate::safety::mmio::MmioRegion::new(
            (self.pci.common_cfg_bar + self.pci.common_cfg_offset as u64) as usize,
            0x1000,
        );
        let isr = crate::safety::mmio::MmioRegion::new(
            (self.pci.isr_cfg_bar + self.pci.isr_cfg_offset as u64) as usize,
            0x1000,
        );
        let _ = isr; // (read below via the same window helper)

        // Step 1: Reset device
        common.write_u8(DEVICE_STATUS as usize, 0);
        fence(Ordering::SeqCst);

        // Step 2: Set ACKNOWLEDGE
        common.write_u8(DEVICE_STATUS as usize, virtio_pci_status::ACKNOWLEDGE as u8);
        fence(Ordering::SeqCst);

        // Step 3: Set DRIVER
        common.write_u8(DEVICE_STATUS as usize,
            (virtio_pci_status::ACKNOWLEDGE | virtio_pci_status::DRIVER) as u8);
        fence(Ordering::SeqCst);

        // Step 4: Read device features
        let device_features_low = {
            common.write_u32(DEVICE_FEATURE_SELECT as usize, 0);
            fence(Ordering::SeqCst);
            common.read_u32(DEVICE_FEATURES as usize)
        };
        let device_features_high = {
            common.write_u32(DEVICE_FEATURE_SELECT as usize, 1);
            fence(Ordering::SeqCst);
            common.read_u32(DEVICE_FEATURES as usize)
        };
        let _ = (device_features_low, device_features_high); // suppress unused warning

        // Step 5: Write driver features
        // VIRTIO_F_VERSION_1 (bit 32) must be negotiated, but it's in the second feature word
        // First word (feature_select = 0): no special features needed
        // Second word (feature_select = 1): VIRTIO_F_VERSION_1 = bit 0

        // Write first feature word (no GPU special features needed)
        {
            common.write_u32(DRIVER_FEATURE_SELECT as usize, 0);
            fence(Ordering::SeqCst);
            common.write_u32(DRIVER_FEATURES as usize, 0);
        }
        fence(Ordering::SeqCst);

        // Write second feature word (VIRTIO_F_VERSION_1 = bit 0 of word 1)
        {
            common.write_u32(DRIVER_FEATURE_SELECT as usize, 1);
            fence(Ordering::SeqCst);
            common.write_u32(DRIVER_FEATURES as usize, 1); // bit 0 = VIRTIO_F_VERSION_1
        }
        fence(Ordering::SeqCst);

        // Step 6: Set FEATURES_OK
        common.write_u8(DEVICE_STATUS as usize,
            (virtio_pci_status::ACKNOWLEDGE | virtio_pci_status::DRIVER | virtio_pci_status::FEATURES_OK) as u8);
        fence(Ordering::SeqCst);

        // Step 7: Verify FEATURES_OK
        let status_val = common.read_u8(DEVICE_STATUS as usize);
        if (status_val & virtio_pci_status::FEATURES_OK as u8) == 0 {
            return None;
        }

        // The device's queue count tells us whether an INPUT (events)
        // queue exists at all — the virtio-gpu spec puts absolute pointer
        // events on queue 2, and a driver that never services that queue
        // silently drops every gesture.
        let qnum = common.read_u16(COMMON_CFG_QUEUE_NUM as usize);
        crate::pr_debug!("virtio-gpu: device queue count = {}", qnum);

        // Step 8: Initialize control queue
        common.write_u16(COMMON_CFG_QUEUE_SELECT as usize, CTRL_QUEUE);
        fence(Ordering::SeqCst);

        let queue_size = common.read_u16(COMMON_CFG_QUEUE_SIZE as usize);

        if queue_size == 0 {
            return None;
        }

        // Notify address per VirtIO 1.0 spec 4.1.4.4:
        //   notify_cap.bar_base + notify_cap.offset
        //                 + queue_notify_off * notify_off_multiplier
        // read through get_notify_addr (it selects the queue and reads the
        // queue's own queue_notify_off register). The old inline
        // `queue_index * multiplier * 2` used the queue INDEX instead of
        // queue_notify_off AND scaled the multiplier — which is already in
        // bytes — a second time (the same "notify ×2" review BUG fixed in
        // VirtIOPCI::get_notify_addr). It only worked because QEMU sets
        // multiplier = 0; any device with a real multiplier hit the wrong
        // doorbell.
        let queue = VirtQueue::new(
            queue_size,
            CTRL_QUEUE,
            self.pci.get_notify_addr(CTRL_QUEUE),
            isr.base() as u64,
            (isr.base() + 4) as u64,
        )?;

        // Convert virtual addresses to physical addresses (typed
        // getters; no raw field access anymore).
        let desc_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(queue.get_desc_addr())
        ).0;
        let avail_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(queue.get_avail_addr())
        ).0;
        let used_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(queue.get_used_addr())
        ).0;

        common.write_u32(COMMON_CFG_QUEUE_DESC_LO as usize, desc_phys as u32);
        common.write_u32(COMMON_CFG_QUEUE_DESC_HI as usize, (desc_phys >> 32) as u32);
        common.write_u32(COMMON_CFG_QUEUE_DRIVER_LO as usize, avail_phys as u32);
        common.write_u32(COMMON_CFG_QUEUE_DRIVER_HI as usize, (avail_phys >> 32) as u32);
        common.write_u32(COMMON_CFG_QUEUE_DEVICE_LO as usize, used_phys as u32);
        common.write_u32(COMMON_CFG_QUEUE_DEVICE_HI as usize, (used_phys >> 32) as u32);
        fence(Ordering::SeqCst);

        common.write_u16(COMMON_CFG_QUEUE_ENABLE as usize, 1);
        fence(Ordering::SeqCst);

        self.ctrl_queue = Some(queue);

        // Step 9: Set DRIVER_OK
        common.write_u8(DEVICE_STATUS as usize,
            (virtio_pci_status::ACKNOWLEDGE | virtio_pci_status::DRIVER | virtio_pci_status::FEATURES_OK | virtio_pci_status::DRIVER_OK) as u8);
        fence(Ordering::SeqCst);

        Some(())
    }

    /// Initialize framebuffer and send GPU commands
    pub fn init_framebuffer(&mut self) -> Option<&FrameBufferInfo> {
        // Step 1: Get display info
        let display_info = self.get_display_info()?;

        // Use first pmode (scanout 0)
        let pmode0 = &display_info.pmodes[0];

        // Even if enabled is 0, try to use this display configuration
        // Some QEMU versions may return enabled=0 but still support scanout
        self.display_rect = pmode0.rect;

        let width = pmode0.rect.width;
        let height = pmode0.rect.height;

        if width == 0 || height == 0 {
            return None;
        }

        let stride = width * 4;
        let fb_size = (stride * height) as usize;

        // Step 2: Allocate framebuffer
        let layout = Layout::from_size_align(fb_size, 4096).ok()?;
        let fb_ptr = unsafe { alloc_zeroed(layout) };

        if fb_ptr.is_null() {
            return None;
        }

        self.fb_ptr = fb_ptr;
        self.fb_layout = Some(layout);

        // Step 3: Create 2D resource
        self.create_resource_2d(width, height)?;

        // Step 4: Attach backing storage (use physical address)
        let fb_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(fb_ptr as u64)
        ).0;

        self.attach_backing(fb_phys, fb_size as u32)?;

        // Define full rectangle
        let full_rect = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };

        // Step 5: Set scanout first (important: must be before transferring data)
        self.set_scanout(0, self.resource_id, &full_rect)?;

        // Step 6: Transfer framebuffer to device
        let _ = self.transfer_to_host_2d(self.resource_id, 0, &full_rect);

        // Step 7: Flush resource to display
        let _ = self.resource_flush(self.resource_id, &full_rect);

        // Save framebuffer information
        self.fb_info = Some(FrameBufferInfo {
            // PHYSICAL address — userspace mmap maps these frames; the
            // old VA here built PTEs pointing outside RAM and every
            // store faulted in an unbreakable handled-fault loop.
            addr: fb_phys,
            size: fb_size as u32,
            width,
            height,
            stride,
            format: 1,
        });

        self.fb_info.as_ref()
    }

    /// Get display info
    fn get_display_info(&self) -> Option<RespDisplayInfo> {
        let cmd = GpuCtrlHeader {
            hdr_type: cmd::GET_DISPLAY_INFO,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            padding: 0,
        };

        let mut resp = RespDisplayInfo {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            pmodes: [DisplayPmode::default(); 16],
        };

        self.send_command(&cmd, core::mem::size_of::<GpuCtrlHeader>(),
                         &mut resp, core::mem::size_of::<RespDisplayInfo>())?;

        if resp.header.hdr_type != cmd::RESP_OK_DISPLAY_INFO {
            return None;
        }

        Some(resp)
    }

    /// Create 2D resource
    fn create_resource_2d(&self, width: u32, height: u32) -> Option<()> {
        self.create_resource_2d_id(self.resource_id, width, height, 2)
    }

    /// Create a 2D resource with an explicit resource id and pixel
    /// format (DRM/KMS path: one virtio resource per GEM dumb buffer,
    /// format matching the DRM fourcc's memory byte order).
    pub fn create_resource_2d_id(&self, resource_id: u32, width: u32, height: u32, format: u32) -> Option<()> {
        let cmd = CmdResourceCreate2d {
            header: GpuCtrlHeader {
                hdr_type: cmd::RESOURCE_CREATE_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            // The fbdev boot framebuffer keeps format 2 =
            // VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM, which QEMU maps
            // to PIXMAN_BE_b8g8r8x8: memory bytes B,G,R,X — i.e. the
            // standard X TrueColor byte layout (screen R from V[23:16],
            // G from V[15:8], B from V[7:0], V[31:24] ignored). This is
            // the same format the Linux virtio-gpu DRM driver picks for
            // DRM_FORMAT_XRGB8888, and it makes every pixel producer
            // agree: Xorg's fbdev layer (which packs per the
            // FBIOGET_VSCREENINFO bitfields), Mesa's GLX swrast (which
            // ALWAYS packs the standard X layout and drops its alpha
            // byte into V[31:24]), and native apps packing R<<16|G<<8|B.
            // The old format 3 (PIXMAN_BE_a8r8g8b8: R=V[15:8],
            // G=V[23:16], B=V[31:24]) forced a rotated bitfield
            // advertisement to keep bitfield-honoring clients correct,
            // while GLX swrast output (mutter/llvmpipe) rendered with
            // blue pinned at the alpha byte's value — a full-screen
            // color rotation measured via QEMU screendump on x86_64
            // (2026-10-14, agent-paint calibration run).
            format,
            width,
            height,
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdResourceCreate2d>(),
                         &mut resp, core::mem::size_of::<RespNoData>())?;

        if resp.header.hdr_type != cmd::RESP_OK_NODATA {
            return None;
        }

        Some(())
    }

    /// Attach backing storage
    fn attach_backing(&self, addr: u64, size: u32) -> Option<()> {
        self.attach_backing_id(self.resource_id, addr, size)
    }

    /// Attach backing storage to an explicit resource id.
    pub fn attach_backing_id(&self, resource_id: u32, addr: u64, size: u32) -> Option<()> {
        let cmd = CmdResourceAttachBacking {
            header: GpuCtrlHeader {
                hdr_type: cmd::RESOURCE_ATTACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            nr_entries: 1,
            entry: MemEntry {
                addr,
                length: size,
                padding: 0,
            },
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdResourceAttachBacking>(),
                         &mut resp, core::mem::size_of::<RespNoData>())?;

        if resp.header.hdr_type != cmd::RESP_OK_NODATA {
            return None;
        }

        Some(())
    }

    /// Set scanout
    fn set_scanout(&self, scanout_id: u32, resource_id: u32, rect: &Rect) -> Option<()> {
        let cmd = CmdSetScanout {
            header: GpuCtrlHeader {
                hdr_type: cmd::SET_SCANOUT,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            rect: *rect,
            scanout_id,
            resource_id,
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdSetScanout>(),
                         &mut resp, core::mem::size_of::<RespNoData>())?;

        if resp.header.hdr_type != cmd::RESP_OK_NODATA {
            return None;
        }

        Some(())
    }

    /// Bind a resource to scanout 0 covering (0,0,w,h) — DRM flip path.
    pub fn set_scanout_id(&self, scanout_id: u32, resource_id: u32, width: u32, height: u32) -> bool {
        let rect = Rect { x: 0, y: 0, width, height };
        self.set_scanout(scanout_id, resource_id, &rect).is_some()
    }

    /// Bind a resource to scanout 0 showing the (0,0,w,h) subrect of a
    /// resource that is `res_w` pixels wide (pitch-aligned GEMs: the
    /// scanout rect crops by the resource stride, so the left `w`
    /// columns display unsheared).
    pub fn set_scanout_rect(
        &self,
        scanout_id: u32,
        resource_id: u32,
        width: u32,
        height: u32,
        res_w: u32,
    ) -> bool {
        debug_assert!(width <= res_w);
        let rect = Rect { x: 0, y: 0, width, height };
        let _ = res_w; // stride comes from the resource itself on the host
        self.set_scanout(scanout_id, resource_id, &rect).is_some()
    }

    /// Release a host-side resource (DRM GEM teardown).
    pub fn unref_resource(&self, resource_id: u32) -> bool {
        // RESOURCE_UNREF: header(24) + resource_id(4) + padding(4) = 32.
        #[repr(C)]
        struct CmdResourceUnref {
            header: GpuCtrlHeader,
            resource_id: u32,
            padding: u32,
        }
        let cmd = CmdResourceUnref {
            header: GpuCtrlHeader {
                hdr_type: cmd::RESOURCE_UNREF,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            padding: 0,
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdResourceUnref>(),
                         &mut resp, core::mem::size_of::<RespNoData>())
            .is_some()
    }

    /// Transfer data to host
    fn transfer_to_host_2d(&self, resource_id: u32, offset: u64, rect: &Rect) -> Option<()> {
        let cmd = CmdTransferToHost2d {
            header: GpuCtrlHeader {
                hdr_type: cmd::TRANSFER_TO_HOST_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            rect: *rect,
            offset,
            resource_id,
            padding: 0,
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdTransferToHost2d>(),
                         &mut resp, core::mem::size_of::<RespNoData>())?;

        if resp.header.hdr_type != cmd::RESP_OK_NODATA {
            return None;
        }

        Some(())
    }

    /// Flush resource to display
    fn resource_flush(&self, resource_id: u32, rect: &Rect) -> Option<()> {
        let cmd = CmdResourceFlush {
            header: GpuCtrlHeader {
                hdr_type: cmd::RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            rect: *rect,          // rect comes BEFORE resource_id!
            resource_id,
            padding: 0,
        };

        let mut resp = RespNoData {
            header: GpuCtrlHeader {
                hdr_type: 0,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
        };

        self.send_command(&cmd, core::mem::size_of::<CmdResourceFlush>(),
                         &mut resp, core::mem::size_of::<RespNoData>())?;

        if resp.header.hdr_type != cmd::RESP_OK_NODATA {
            // RESOURCE_FLUSH may return error, but doesn't affect display
            // Some implementations only need TRANSFER_TO_HOST_2D
        }

        Some(())
    }

    /// Transfer an explicit resource's full (0,0,w,h) rect to host
    /// memory — DRM flip path.
    pub fn transfer_to_host_2d_id(&self, resource_id: u32, offset: u64, width: u32, height: u32) -> bool {
        let rect = Rect { x: 0, y: 0, width, height };
        self.transfer_to_host_2d(resource_id, offset, &rect).is_some()
    }

    /// Present an explicit resource's full (0,0,w,h) rect — DRM flip
    /// path.
    pub fn resource_flush_id(&self, resource_id: u32, width: u32, height: u32) -> bool {
        let rect = Rect { x: 0, y: 0, width, height };
        self.resource_flush(resource_id, &rect).is_some()
    }

    /// Send command to VirtIO-GPU
    fn send_command<CMD, RESP>(&self,
                               cmd: &CMD,
                               cmd_size: usize,
                               resp: &mut RESP,
                               resp_size: usize) -> Option<()> {
        let queue = self.ctrl_queue.as_ref()?;

        // Serialize the hardcoded-desc submission (see cmd_lock field).
        // The guard spans the whole submit-and-wait: releasing earlier
        // would let a second CPU overwrite descriptors 0/1 before the
        // device reads them.
        let _cmd_guard = self.cmd_lock.lock();

        // DMA staging from the linear-mapped kernel heap, NOT from the
        // caller's stack storage: on x86_64 the boot-time kernel stack
        // lives at low identity VAs where virt_to_phys's linear-map
        // VA-PA subtraction underflows (riscv64 kernel stacks satisfy
        // the offset, which is why passing &cmd worked there). Same
        // single-block discipline as the virtio-blk io_buf (R17-C).
        let io_layout = Layout::from_size_align(cmd_size + resp_size, 16).ok()?;
        // SAFETY: Layout is non-zero-sized (both parts are >= 24 bytes);
        // null check follows immediately.
        let io_buf = unsafe { alloc(io_layout) };
        if io_buf.is_null() {
            return None;
        }
        let cmd_ptr = io_buf as *const u8;
        // SAFETY: io_buf is cmd_size + resp_size bytes; +cmd_size is in
        // bounds and resp_size bytes remain after it.
        let resp_ptr = unsafe { io_buf.add(cmd_size) } as *mut u8;
        // SAFETY: cmd references at least cmd_size initialized bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(cmd as *const CMD as *const u8, io_buf, cmd_size);
        }

        // Set by the polling loop below; read after the block is released.
        let mut completed = false;

        // Convert virtual addresses to physical addresses
        let cmd_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(cmd_ptr as u64)
        ).0;
        let resp_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(resp_ptr as u64)
        ).0;

        // Use first descriptor to send command, second descriptor to receive response
        // LIMITATION: Hardcodes descriptors 0 and 1, so only one GPU command can
        // be in-flight at a time. Proper fix would use the descriptor allocator
        // (queue.alloc_desc()) for concurrent command support.
        {
            // INV-DMA-2: typed bounds-checked ring views replace the raw
            // desc/avail pointer programming.
            // Set command descriptor
            let desc0 = queue.desc_mut(0).unwrap();
            desc0.addr = cmd_phys;
            desc0.len = cmd_size as u32;
            desc0.flags = 0x01; // VIRTQ_DESC_F_NEXT
            desc0.next = 1;

            // Set response descriptor
            {
                let desc1 = queue.desc_mut(1).unwrap();
                desc1.addr = resp_phys;
                desc1.len = resp_size as u32;
                desc1.flags = 0x02; // VIRTQ_DESC_F_WRITE
                desc1.next = 0;
            }

            // Update available ring
            // AvailRing structure: flags (u16) + idx (u16) + ring[] (u16 array)
            // ring array starts at offset 4
            let idx = queue.avail_idx() as usize;
            let ring_idx = idx % queue.queue_size as usize;

            // ring array immediately follows AvailRing struct
            queue.write_avail_ring_slot(ring_idx, 0); // Descriptor index 0

            // Ensure all writes are visible to device (memory barrier)
            fence(Ordering::SeqCst);

            // RISC-V needs to flush CPU cache to memory
            #[cfg(feature = "riscv64")]
            // SAFETY: bare cache/memory fence instruction, no operands.
            unsafe {
                core::arch::asm!("fence");
            }

            queue.write_avail_idx(((idx as u16).wrapping_add(1)));
            fence(Ordering::SeqCst);

            // Notify device
            queue.notify();
            fence(Ordering::SeqCst);

            // Wait for response (simple polling), under two bounds:
            //
            // (a) WALL-CLOCK: 10 s of jiffies (checked every ~1M
            //     iterations). At runtime the polling CPU has irqs
            //     enabled, so its timer tick keeps jiffies advancing even
            //     while spinning. 10 s keeps a 5x margin over the
            //     documented worst case (host display-backend window
            //     realization answering GET_DISPLAY_INFO can take seconds
            //     under load) while bounding the flush latency other
            //     CPUs can tolerate. A time bound is REQUIRED, not an
            //     iteration count: under TCG MTTCG with BQL contention
            //     every polled iteration (SeqCst fence + used-ring read)
            //     stretches, so iteration caps do not bound wall time —
            //     soak-r1 (smp4 + I/O churn) measured a >500 ms lock
            //     hold (spinlock DEADLOCK warning) from exactly this
            //     loop's iteration budget.
            // (b) ITERATIONS: 100M — the fallback during early boot,
            //     when the probe runs before the scheduler/timer start
            //     and jiffies are frozen; same bound as before the
            //     wall-clock deadline existed.
            // The fast path returns as soon as used.idx advances, so
            // these bounds only cost time on a genuinely dead device.
            let jiffy_deadline = crate::drivers::hotpath::timer::get_jiffies()
                + 10 * crate::config::KERNEL_HZ as u64;
            let mut spins: u32 = 0;
            loop {
                fence(Ordering::SeqCst);
                // INV-DMA-3: device-written used index via the typed
                // volatile getter.
                let used_idx_now = queue.get_used();
                // Wrap-safe completion check: vring indices are free-running
                // u16 counters. Entry state is used.idx == idx (the cmd_lock
                // spans the previous command's wait, so everything submitted
                // before us has completed), so "advanced past our snapshot"
                // is exactly `used.idx != idx`. The old `>= idx + 1`
                // comparison broke at the 65536th command: idx = 0xFFFF can
                // never satisfy `used.idx >= 0x10000`, so EVERY command from
                // the wrap on timed out after 10 s — a long GUI session
                // (2-3 commands per flip) reaches the wrap in hours.
                if used_idx_now != idx as u16 {
                    completed = true;
                    break;
                }
                spins = spins.wrapping_add(1);
                if spins >= 100_000_000 {
                    break;
                }
                if spins & 0xF_FFFF == 0
                    && crate::drivers::hotpath::timer::get_jiffies() >= jiffy_deadline
                {
                    break;
                }
            }

            if completed {
                // Copy the device's response out of the staging block into
                // the caller's buffer before the block is freed.
                // SAFETY: the device wrote resp_size bytes at resp_ptr
                // (desc1 was the device-writable half of the chain).
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        resp_ptr,
                        resp as *mut RESP as *mut u8,
                        resp_size,
                    );
                }
            }
        }

        // R21-N2 discipline (the virtio-blk timeout rule): on completion the
        // staging block is freed; on TIMEOUT it is deliberately LEAKED — the
        // chain is still submitted and the device may DMA its response into
        // the block arbitrarily late. Freeing a still-DMA-targeted heap block
        // hands a late device write to whatever reallocates it (the wandering
        // corruption class). The leak is one 32-224 byte block per dead-
        // device timeout, and the next command's poll consumes the late
        // used-ring entry with a zeroed response, which callers reject.
        if completed {
            // SAFETY: io_buf was allocated above with io_layout; the chain is
            // complete, so the device no longer accesses the block.
            unsafe { dealloc(io_buf, io_layout) };
        }

        if completed {
            Some(())
        } else {
            None
        }
    }

    /// Flush display
    /// Raw ECAM address of this function's PCI config space (for the
    /// sysfs BDF derivation in drm::init_drm).
    pub fn pci_ecam_addr(&self) -> u64 {
        self.pci.pci_config.base_addr
    }

    pub fn flush(&self) {        let rect = self.display_rect;

        // 1. Transfer framebuffer data to device
        let _ = self.transfer_to_host_2d(self.resource_id, 0, &rect);

        // 2. Flush resource to display
        let _ = self.resource_flush(self.resource_id, &rect);
    }

    /// Get framebuffer
    pub fn get_framebuffer(&self) -> Option<FrameBuffer> {
        let info = self.fb_info.as_ref()?;
        {
            Some(FrameBuffer::new(info.addr, FrameBufferInfo {
                addr: info.addr,
                size: info.size,
                width: info.width,
                height: info.height,
                stride: info.stride,
                format: info.format,
            }))
        }
    }
}

impl Drop for VirtioGpuDevice {
    fn drop(&mut self) {
        if !self.fb_ptr.is_null() {
            if let Some(layout) = self.fb_layout {
                unsafe {
                    dealloc(self.fb_ptr, layout);
                }
            }
        }
    }
}

/// Probe VirtIO-GPU device
pub fn probe_virtio_gpu() -> Option<VirtioGpuDevice> {
    // Shared ECAM walker: 0x8000 stride per slot + all 8 functions (the
    // old 0x1000 stride hid every slot >= 4 — review BUG).
    for ecam_addr in find_ecam_devices(VIRTIO_GPU_PCI_VENDOR, &[virtio_device::VIRTIO_GPU]) {
        let virtio_pci = VirtIOPCI::new(ecam_addr).ok()?;
        return VirtioGpuDevice::new(virtio_pci);
    }

    None
}

// ════════════════ input ═══════════════════════════════
// (was input/mod.rs)
// Input subsystem
//
// Provides unified input device interface, including:
// - VirtIO Input driver (main input device for RISC-V)
// - PS/2 driver (x86 compatible, not available on RISC-V)
// - evdev character device interface
// - Input event definitions
// Re-export common types

// ============================================================================
// Global input devices
// ============================================================================

/// VirtIO keyboard device
pub static INPUT_KEYBOARD: Spinlock<Option<VirtioInputDevice>> = Spinlock::new(None);

/// VirtIO pointer device (mouse/touchscreen)
pub static INPUT_POINTER: Spinlock<Option<VirtioInputDevice>> = Spinlock::new(None);

// ============================================================================
// IRQ-driven event delivery (producer wake for virtio-input)
// ============================================================================
//
// Before this wiring the event flow was pull-only: events moved off the
// virtqueues into the evdev buffers solely inside evdev read/poll
// dispatches. A waiter that BLOCKS has no drain trigger of its own —
// push_event (the evdev poll_waitq's only waker) runs at drain time, and
// nobody drains. Xorg's input thread blocks exactly like that (epoll_wait
// with no timeout, registered on the evdev poll_waitq by the P3 wait
// path), so keystrokes sat in the used ring forever: the r13live session
// logged zero INPTRC for 1500 s while the desktop kept rendering, while
// timeout-based pollers (inputcat's 5 s loops) got 14/14 events. The IRQ
// path closes the loop the way Linux's virtio-input does it: the device
// interrupt schedules the input tasklet, which drains both virtqueues
// and delivers through push_input_event, waking every
// poll/select/epoll waiter the moment an event lands.

/// Input bottom-half tasklet. Armed (init + enable) once during
/// virtio-input probe, scheduled by the hard-IRQ top half.
// SAFETY: touched from the probe (single-threaded init context) and from
// tasklet_schedule in hard-IRQ context, which is the API's designed use.
static mut INPUT_BH_TASKLET: crate::interrupt::tasklet::TaskletStruct =
    crate::interrupt::tasklet::TaskletStruct::new();

/// dev_id → ISR capability address for the registered input functions
/// (0 = keyboard, 1 = pointer). Stored before the IRQ registration that
/// can dispatch against them, read lock-free in the hard-IRQ top half
/// (same discipline as virtio-blk's ISR read).
static INPUT_ISR_BARS: [core::sync::atomic::AtomicU64; 2] =
    [core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0)];

/// Input tasklet bottom half: drain both event queues and deliver.
///
/// Collect/deliver split (INV-LOCK-6 discipline, same shape as
/// virtio-blk's block_bh_handler): events are popped off the used rings
/// under the device locks — read_event also re-posts each buffer to the
/// avail ring, so the device can keep streaming — and pushed into the
/// evdev buffers (which wakes the poll/select/epoll waiters) only after
/// every device lock is dropped.
/// rux:inputtrace (retired debug=inputtrace) tick probe: drain both event queues from timer
/// context. If IN-EV prints here while no hard IRQ ever scheduled the
/// tasklet, the INTx wiring is the break; if the queues are empty, the
/// device never wrote (QEMU-side drop).
pub fn input_debug_tick() {
    if !crate::trace::EVENT_INPUTTRACE.armed() {
        return;
    }
    static TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let n = TICKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if n % 100 != 7 {
        return; // ~once a second at 100 Hz
    }
    input_bh_run(core::ptr::null_mut());
    static BEAT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let b = BEAT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if b < 3 {
        crate::pr_warn!("IN-TICK beat={} ptr={:?}", b, INPUT_POINTER.lock().is_some());
    }
}

fn input_bh_run(_t: *mut crate::interrupt::tasklet::TaskletStruct) {
    const BURST: usize = 16;
    const ROUNDS: usize = 8;
    for _ in 0..ROUNDS {
        let mut kbd = [InputEvent::default(); BURST];
        let mut n_kbd = 0usize;
        let mut ptr = [InputEvent::default(); BURST];
        let mut n_ptr = 0usize;
        {
            // SAFETY: INPUT_KEYBOARD is initialized before the IRQ
            // registration that can schedule this tasklet.
            let mut guard = INPUT_KEYBOARD.lock_irqsave();
            if let Some(ref mut dev) = *guard {
                while n_kbd < BURST {
                    match dev.read_event() {
                        Some(e) => {
                            kbd[n_kbd] = e;
                            n_kbd += 1;
                        }
                        None => break,
                    }
                }
            }
        }
        {
            // SAFETY: INPUT_POINTER is initialized before the IRQ
            // registration that can schedule this tasklet.
            let mut guard = INPUT_POINTER.lock_irqsave();
            if let Some(ref mut dev) = *guard {
                while n_ptr < BURST {
                    match dev.read_event() {
                        Some(e) => {
                            ptr[n_ptr] = e;
                            n_ptr += 1;
                        }
                        None => break,
                    }
                }
            }
        }
        if n_kbd == 0 && n_ptr == 0 {
            break; // both queues drained
        }
        if (n_kbd + n_ptr) > 0
            && crate::trace::EVENT_INPUTTRACE.armed()
        {
            crate::pr_warn!("IN-BH kbd={} ptr={}", n_kbd, n_ptr);
        }
        for e in &kbd[..n_kbd] {
            push_input_event(false, *e);
        }
        for e in &ptr[..n_ptr] {
            push_input_event(true, *e);
        }
    }
}

/// Hard-IRQ top half for a virtio-input PCI function (riscv64: the gpex
/// swizzled INTx line, possibly shared with virtio-blk or the sibling
/// input function). Reads the ISR capability FIRST — per the virtio spec
/// that read drops the device's level-triggered line (the DRIV-H3
/// discipline) — then defers all virtqueue work to the input tasklet: no
/// queue lock and no wait-queue wake in hard-IRQ context (the same split
/// as virtio-blk's interrupt_handler_pci).
#[cfg(feature = "riscv64")]
fn virtio_input_irq_handler(_irq: u32, dev_id: usize) -> crate::interrupt::IrqReturn {
    use core::sync::atomic::Ordering;
    let idx = if dev_id == 0 { 0 } else { 1 };
    let isr_bar = INPUT_ISR_BARS[idx].load(Ordering::Relaxed);
    if isr_bar == 0 {
        return crate::interrupt::IrqReturn::None;
    }
    // INV-MMIO-1: isr_bar was stored from a probed device's ISR
    // capability before this handler was registered; the volatile read
    // is the device-side EOI.
    let isr = crate::safety::mmio::MmioRegion::new(isr_bar as usize, 4).read_u32(0);
    if isr == 0 {
        // Not ours — the shared line was raised by another function.
        return crate::interrupt::IrqReturn::None;
    }
    // INPUT_BH_TASKLET is initialized by register_input_irq() before
    // any IRQ registration that can reach this point; a static tasklet
    // struct lives forever (tasklet_schedule's documented contract).
    crate::interrupt::tasklet::tasklet_schedule(core::ptr::addr_of_mut!(
        INPUT_BH_TASKLET
    ));
    crate::interrupt::IrqReturn::Handled
}

/// x86_64 INTx service (registered through the shared virtio INTx
/// dispatcher, which has already read the ISR — dropping the line — and
/// only calls us when it is nonzero).
#[cfg(feature = "x86_64")]
fn input_intx_service() {
    // INPUT_BH_TASKLET is initialized by register_input_irq() before
    // the INTx registration that can reach this point.
    crate::interrupt::tasklet::tasklet_schedule(core::ptr::addr_of_mut!(
        INPUT_BH_TASKLET
    ));
}

/// Arm the input tasklet and hook the device's interrupt line. Called
/// from init_virtio_input() once per probed function, after the device
/// is stored in its INPUT_* static and before any event can be waited
/// on (`dev_id` 0 = keyboard, 1 = pointer).
pub fn register_input_irq(dev: &VirtioInputDevice, dev_id: usize) {
    let idx = if dev_id == 0 { 0 } else { 1 };
    INPUT_ISR_BARS[idx].store(dev.isr_cfg_bar(), core::sync::atomic::Ordering::Release);

    #[cfg(feature = "riscv64")]
    {
        let irq = dev.intx_irq_line();
        let _ = crate::interrupt::request_irq(
            irq,
            virtio_input_irq_handler,
            crate::interrupt::irqdesc::IRQF_SHARED,
            "virtio-input",
            dev_id,
        );
    }
    #[cfg(feature = "x86_64")]
    {
        let _ = dev.enable_pci_intx_irq(input_intx_service);
    }
}

// ============================================================================
// Initialization
// ============================================================================

/// Initialize input subsystem
pub fn input_init() {
    // Initialize PS/2 driver (does nothing on RISC-V)
    init_keyboard();
    init_mouse();
    // rung4 prep move 3: own the tick diagnostic through the framework hook.
    crate::timer::register_input_debug_tick(input_debug_tick);

}

/// Initialize VirtIO Input devices
pub fn init_virtio_input() -> (usize, usize) {
    let mut keyboard_count = 0;
    let mut pointer_count = 0;

    // Arm the bottom-half tasklet before any device IRQ is registered
    // (single-threaded kernel-init context).
    unsafe { INPUT_BH_TASKLET.init(input_bh_run) };

    // Probe VirtIO Input devices via the shared ECAM walker (0x8000 stride
    // per slot + all 8 functions). The old inline walk here enumerated
    // ECAM slot bases only (function 0 of the first slots) — QEMU's virt
    // machine places virtio-keyboard-pci and virtio-tablet-pci on later
    // slots/functions, so the tablet was silently never probed and the
    // guest ended up with a keyboard but no pointer (review BUG:
    // "tablet 从未被探测").
    for ecam_addr in crate::drivers::hotpath::find_ecam_devices(0x1AF4, &[0x1052]) {
        if let Ok(virtio_pci) = crate::drivers::types::VirtIOPCI::new(ecam_addr) {
            if let Some(input_dev) = VirtioInputDevice::new(virtio_pci) {
                let is_pointer = input_dev.is_pointer();

                if is_pointer {
                    if INPUT_POINTER.lock().is_none() {
                        *INPUT_POINTER.lock() = Some(input_dev);
                        pointer_count += 1;
                    }
                } else {
                    if INPUT_KEYBOARD.lock().is_none() {
                        *INPUT_KEYBOARD.lock() = Some(input_dev);
                        keyboard_count += 1;
                    }
                }
            }
        }
    }

    // Initialize evdev devices (capability bitmaps are read under the
    // device locks with plain guards — safe because the device IRQs are
    // not hooked yet).
    init_evdev();

    // IRQ registration LAST: once the line is hooked, the tasklet BH can
    // drain the queues on any virtio-input interrupt, and every INPUT_*
    // guard from here on must be irqsave (the evdev read/poll paths and
    // the BH are; nothing between the store above and this point is).
    // The lookup guards here are irqsave too: once the FIRST function is
    // registered its BH can preempt a plain holder of the second lock.
    if let Some(ref dev) = *INPUT_KEYBOARD.lock_irqsave() {
        register_input_irq(dev, 0);
    }
    if let Some(ref dev) = *INPUT_POINTER.lock_irqsave() {
        register_input_irq(dev, 1);
    }

    (keyboard_count, pointer_count)
}

/// Poll input events
pub fn poll_events() {
    // Poll keyboard (irqsafe: called from softirq which may be preempted by hard IRQ)
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock_irqsave() {
        while kb.has_event() {
            if let Some(event) = kb.read_event() {
                push_input_event(false, event);
            }
        }
    }

    // Poll pointer device
    if let Some(ref mut ptr) = *INPUT_POINTER.lock_irqsave() {
        while ptr.has_event() {
            if let Some(event) = ptr.read_event() {
                push_input_event(true, event);
            }
        }
    }
}

/// Get keyboard event (legacy interface compatibility)
pub fn get_keyboard_event() -> Option<InputEvent> {
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock_irqsave() {
        kb.read_event()
    } else {
        None
    }
}

/// Get pointer event (legacy interface compatibility)
pub fn get_pointer_event() -> Option<InputEvent> {
    if let Some(ref mut ptr) = *INPUT_POINTER.lock_irqsave() {
        ptr.read_event()
    } else {
        None
    }
}

// ════════════════ input::event ════════════════════════
// (was input/event.rs)
// Input event definitions
//
// evdev compatible input event interface
/// Key event
pub const EV_KEY: u16 = 0x01;
/// Relative coordinate event (mouse movement)
pub const EV_REL: u16 = 0x02;
/// Absolute coordinate event (touchscreen, tablet)
pub const EV_ABS: u16 = 0x03;
/// Sync event
pub const EV_SYN: u16 = 0x00;
/// Miscellaneous event
pub const EV_MSC: u16 = 0x04;

// ============================================================================
// Relative coordinate axis codes (EV_REL)
// ============================================================================

/// X-axis relative movement
pub const REL_X: u16 = 0x00;
/// Y-axis relative movement
pub const REL_Y: u16 = 0x01;
/// Wheel
pub const REL_WHEEL: u16 = 0x08;

// ============================================================================
// Absolute coordinate axis codes (EV_ABS)
// ============================================================================

/// X-axis absolute position
pub const ABS_X: u16 = 0x00;
/// Y-axis absolute position
pub const ABS_Y: u16 = 0x01;
/// Multi-touch slot
pub const ABS_MT_SLOT: u16 = 0x2f;
/// Multi-touch X
pub const ABS_MT_POSITION_X: u16 = 0x35;
/// Multi-touch Y
pub const ABS_MT_POSITION_Y: u16 = 0x36;
/// Multi-touch tracking ID
pub const ABS_MT_TRACKING_ID: u16 = 0x39;

// ============================================================================
// Button codes (EV_KEY) - mouse
// ============================================================================

/// Mouse left button
pub const BTN_LEFT: u16 = 0x110;
/// Mouse right button
pub const BTN_RIGHT: u16 = 0x111;
/// Mouse middle button
pub const BTN_MIDDLE: u16 = 0x112;
/// Mouse side button
pub const BTN_SIDE: u16 = 0x113;
/// Mouse extra button
pub const BTN_EXTRA: u16 = 0x114;

// ============================================================================
// Button codes (EV_KEY) - keyboard
// ============================================================================

/// Keyboard key base (KEY_0 = 11)
pub const KEY_ESC: u16 = 0x01;
pub const KEY_1: u16 = 0x02;
pub const KEY_2: u16 = 0x03;
pub const KEY_3: u16 = 0x04;
pub const KEY_4: u16 = 0x05;
pub const KEY_5: u16 = 0x06;
pub const KEY_6: u16 = 0x07;
pub const KEY_7: u16 = 0x08;
pub const KEY_8: u16 = 0x09;
pub const KEY_9: u16 = 0x0a;
pub const KEY_0: u16 = 0x0b;
pub const KEY_MINUS: u16 = 0x0c;
pub const KEY_EQUAL: u16 = 0x0d;
pub const KEY_BACKSPACE: u16 = 0x0e;
pub const KEY_TAB: u16 = 0x0f;
pub const KEY_Q: u16 = 0x10;
pub const KEY_W: u16 = 0x11;
pub const KEY_E: u16 = 0x12;
pub const KEY_R: u16 = 0x13;
pub const KEY_T: u16 = 0x14;
pub const KEY_Y: u16 = 0x15;
pub const KEY_U: u16 = 0x16;
pub const KEY_I: u16 = 0x17;
pub const KEY_O: u16 = 0x18;
pub const KEY_P: u16 = 0x19;
pub const KEY_LEFTBRACE: u16 = 0x1a;
pub const KEY_RIGHTBRACE: u16 = 0x1b;
pub const KEY_ENTER: u16 = 0x1c;
pub const KEY_LEFTCTRL: u16 = 0x1d;
pub const KEY_A: u16 = 0x1e;
pub const KEY_S: u16 = 0x1f;
pub const KEY_D: u16 = 0x20;
pub const KEY_F: u16 = 0x21;
pub const KEY_G: u16 = 0x22;
pub const KEY_H: u16 = 0x23;
pub const KEY_J: u16 = 0x24;
pub const KEY_K: u16 = 0x25;
pub const KEY_L: u16 = 0x26;
pub const KEY_SEMICOLON: u16 = 0x27;
pub const KEY_APOSTROPHE: u16 = 0x28;
pub const KEY_GRAVE: u16 = 0x29;
pub const KEY_LEFTSHIFT: u16 = 0x2a;
pub const KEY_BACKSLASH: u16 = 0x2b;
pub const KEY_Z: u16 = 0x2c;
pub const KEY_X: u16 = 0x2d;
pub const KEY_C: u16 = 0x2e;
pub const KEY_V: u16 = 0x2f;
pub const KEY_B: u16 = 0x30;
pub const KEY_N: u16 = 0x31;
pub const KEY_M: u16 = 0x32;
pub const KEY_COMMA: u16 = 0x33;
pub const KEY_DOT: u16 = 0x34;
pub const KEY_SLASH: u16 = 0x35;
pub const KEY_RIGHTSHIFT: u16 = 0x36;
pub const KEY_KPASTERISK: u16 = 0x37;
pub const KEY_LEFTALT: u16 = 0x38;
pub const KEY_SPACE: u16 = 0x39;
pub const KEY_CAPSLOCK: u16 = 0x3a;

/// Function keys F1-F12
pub const KEY_F1: u16 = 0x3b;
pub const KEY_F2: u16 = 0x3c;
pub const KEY_F3: u16 = 0x3d;
pub const KEY_F4: u16 = 0x3e;
pub const KEY_F5: u16 = 0x3f;
pub const KEY_F6: u16 = 0x40;
pub const KEY_F7: u16 = 0x41;
pub const KEY_F8: u16 = 0x42;
pub const KEY_F9: u16 = 0x43;
pub const KEY_F10: u16 = 0x44;
pub const KEY_F11: u16 = 0x57;
pub const KEY_F12: u16 = 0x58;

/// Arrow keys
pub const KEY_UP: u16 = 0x67;
pub const KEY_DOWN: u16 = 0x6c;
pub const KEY_LEFT: u16 = 0x69;
pub const KEY_RIGHT: u16 = 0x6a;

/// Right modifier keys
pub const KEY_RIGHTCTRL: u16 = 0x61;
pub const KEY_RIGHTALT: u16 = 0x64;

/// Delete key (the Ctrl-Alt-Del trio's terminal)
pub const KEY_DELETE: u16 = 0x6f;

// ============================================================================
// Key values
// ============================================================================

/// Key released
pub const KEY_RELEASE: i32 = 0;
/// Key pressed
pub const KEY_PRESS: i32 = 1;
/// Key repeat
pub const KEY_REPEAT: i32 = 2;

// ============================================================================
// Input event structure (input_event)
// ============================================================================


impl InputEvent {
    /// Create new input event
    ///
    /// The timestamp is taken from the CLINT timer (review LINUX-DIFF:
    /// "InputEvent 时间戳恒 0" — libinput rejects/fabricates timing when
    /// input_event.time is zero). read_time() runs at
    /// TIMER_CLOCK_FREQ_HZ; convert to the struct timeval
    /// (seconds + microseconds) the uapi expects.
    pub fn new(type_: u16, code: u16, value: i32) -> Self {
        let cycles = crate::arch::cpu::read_time();
        let freq = crate::config::TIMER_CLOCK_FREQ_HZ;
        let tv_sec = cycles / freq;
        let tv_usec = (cycles % freq) * 1_000_000 / freq;
        Self {
            tv_sec,
            tv_usec,
            type_,
            code,
            value,
        }
    }

    /// Create keyboard event
    pub fn key_event(code: u16, pressed: bool) -> Self {
        Self::new(EV_KEY, code, if pressed { KEY_PRESS } else { KEY_RELEASE })
    }

    /// Create relative motion event
    pub fn rel_event(axis: u16, value: i32) -> Self {
        Self::new(EV_REL, axis, value)
    }

    /// Create absolute position event
    pub fn abs_event(axis: u16, value: i32) -> Self {
        Self::new(EV_ABS, axis, value)
    }

    /// Create sync event
    pub fn sync_event() -> Self {
        Self::new(EV_SYN, 0, 0)
    }
}

// ============================================================================
// High-level input event enum
// ============================================================================

// ════════════════ input::ps2 ══════════════════════════
// (was input/ps2.rs)
// PS/2 keyboard and mouse driver
//
// Note: PS/2 ports (0x60/0x64) are not available on RISC-V virt platform
// This driver is kept as a framework only, actual input should use VirtIO Input
// ============================================================================
// PS/2 port definitions (x86 only)
// ============================================================================

/// PS/2 data port
const PS2_DATA_PORT: u16 = 0x60;
/// PS/2 command/status port
const PS2_CMD_PORT: u16 = 0x64;

// ============================================================================
// PS/2 keyboard scancodes (Set 1)
// ============================================================================

pub mod scancode {
    pub const KEY_A: u16 = 0x1E;
    pub const KEY_B: u16 = 0x30;
    pub const KEY_C: u16 = 0x2E;
    pub const KEY_D: u16 = 0x20;
    pub const KEY_E: u16 = 0x12;
    pub const KEY_F: u16 = 0x21;
    pub const KEY_G: u16 = 0x22;
    pub const KEY_H: u16 = 0x23;
    pub const KEY_I: u16 = 0x17;
    pub const KEY_J: u16 = 0x24;
    pub const KEY_K: u16 = 0x25;
    pub const KEY_L: u16 = 0x26;
    pub const KEY_M: u16 = 0x27;
    pub const KEY_N: u16 = 0x31;
    pub const KEY_O: u16 = 0x18;
    pub const KEY_P: u16 = 0x19;
    pub const KEY_Q: u16 = 0x10;
    pub const KEY_R: u16 = 0x13;
    pub const KEY_S: u16 = 0x1F;
    pub const KEY_T: u16 = 0x14;
    pub const KEY_U: u16 = 0x16;
    pub const KEY_V: u16 = 0x2F;
    pub const KEY_W: u16 = 0x11;
    pub const KEY_X: u16 = 0x2D;
    pub const KEY_Y: u16 = 0x15;
    pub const KEY_Z: u16 = 0x2C;

    pub const KEY_1: u16 = 0x02;
    pub const KEY_2: u16 = 0x03;
    pub const KEY_3: u16 = 0x04;
    pub const KEY_4: u16 = 0x05;
    pub const KEY_5: u16 = 0x06;
    pub const KEY_6: u16 = 0x07;
    pub const KEY_7: u16 = 0x08;
    pub const KEY_8: u16 = 0x09;
    pub const KEY_9: u16 = 0x0A;
    pub const KEY_0: u16 = 0x0B;

    pub const KEY_ENTER: u16 = 0x1C;
    pub const KEY_SPACE: u16 = 0x39;
    pub const KEY_BACKSPACE: u16 = 0x0E;
    pub const KEY_TAB: u16 = 0x0F;
    pub const KEY_ESCAPE: u16 = 0x01;

    pub const KEY_LSHIFT: u16 = 0x2A;
    pub const KEY_RSHIFT: u16 = 0x36;
    pub const KEY_LCTRL: u16 = 0x1D;
    pub const KEY_RCTRL: u16 = 0x11D;
    pub const KEY_LALT: u16 = 0x38;
    pub const KEY_RALT: u16 = 0x138;

    pub const KEY_F1: u16 = 0x3B;
    pub const KEY_F2: u16 = 0x3C;
    pub const KEY_F3: u16 = 0x3D;
    pub const KEY_F4: u16 = 0x3E;
    pub const KEY_F5: u16 = 0x3F;
    pub const KEY_F6: u16 = 0x40;
    pub const KEY_F7: u16 = 0x41;
    pub const KEY_F8: u16 = 0x42;
    pub const KEY_F9: u16 = 0x43;
    pub const KEY_F10: u16 = 0x44;
    pub const KEY_F11: u16 = 0x57;
    pub const KEY_F12: u16 = 0x58;

    /// Release flag
    pub const BREAK_CODE: u16 = 0x80;
}

// ============================================================================
// PS/2 keyboard driver
// ============================================================================


impl PS2Keyboard {
    pub const fn new() -> Self {
        Self {
            shift_pressed: false,
            ctrl_pressed: false,
            alt_pressed: false,
        }
    }

    /// Read scancode (not available on RISC-V)
    pub fn read_scancode(&mut self) -> Option<InputEvent> {
        // PS/2 ports not available on RISC-V virt platform
        None
    }

    /// Check if data is available (always returns false on RISC-V)
    pub fn has_data(&self) -> bool {
        false
    }

    /// Convert scancode to ASCII
    pub fn scancode_to_ascii(&self, scancode: u16) -> Option<u8> {
        let shifted = self.shift_pressed;

        let ascii = match scancode {
            scancode::KEY_A => if shifted { b'A' } else { b'a' },
            scancode::KEY_B => if shifted { b'B' } else { b'b' },
            scancode::KEY_C => if shifted { b'C' } else { b'c' },
            scancode::KEY_D => if shifted { b'D' } else { b'd' },
            scancode::KEY_E => if shifted { b'E' } else { b'e' },
            scancode::KEY_F => if shifted { b'F' } else { b'f' },
            scancode::KEY_G => if shifted { b'G' } else { b'g' },
            scancode::KEY_H => if shifted { b'H' } else { b'h' },
            scancode::KEY_I => if shifted { b'I' } else { b'i' },
            scancode::KEY_J => if shifted { b'J' } else { b'j' },
            scancode::KEY_K => if shifted { b'K' } else { b'k' },
            scancode::KEY_L => if shifted { b'L' } else { b'l' },
            scancode::KEY_M => if shifted { b'M' } else { b'm' },
            scancode::KEY_N => if shifted { b'N' } else { b'n' },
            scancode::KEY_O => if shifted { b'O' } else { b'o' },
            scancode::KEY_P => if shifted { b'P' } else { b'p' },
            scancode::KEY_Q => if shifted { b'Q' } else { b'q' },
            scancode::KEY_R => if shifted { b'R' } else { b'r' },
            scancode::KEY_S => if shifted { b'S' } else { b's' },
            scancode::KEY_T => if shifted { b'T' } else { b't' },
            scancode::KEY_U => if shifted { b'U' } else { b'u' },
            scancode::KEY_V => if shifted { b'V' } else { b'v' },
            scancode::KEY_W => if shifted { b'W' } else { b'w' },
            scancode::KEY_X => if shifted { b'X' } else { b'x' },
            scancode::KEY_Y => if shifted { b'Y' } else { b'y' },
            scancode::KEY_Z => if shifted { b'Z' } else { b'z' },

            scancode::KEY_1 => if shifted { b'!' } else { b'1' },
            scancode::KEY_2 => if shifted { b'@' } else { b'2' },
            scancode::KEY_3 => if shifted { b'#' } else { b'3' },
            scancode::KEY_4 => if shifted { b'$' } else { b'4' },
            scancode::KEY_5 => if shifted { b'%' } else { b'5' },
            scancode::KEY_6 => if shifted { b'^' } else { b'6' },
            scancode::KEY_7 => if shifted { b'&' } else { b'7' },
            scancode::KEY_8 => if shifted { b'*' } else { b'8' },
            scancode::KEY_9 => if shifted { b'(' } else { b'9' },
            scancode::KEY_0 => if shifted { b')' } else { b'0' },

            scancode::KEY_SPACE => b' ',
            scancode::KEY_ENTER => b'\n',
            scancode::KEY_BACKSPACE => 0x08,
            scancode::KEY_TAB => b'\t',
            scancode::KEY_ESCAPE => 0x1B,
            _ => return None,
        };

        Some(ascii)
    }
}

/// Global PS/2 keyboard instance
pub static mut PS2_KEYBOARD: PS2Keyboard = PS2Keyboard::new();

// ============================================================================
// PS/2 mouse driver
// ============================================================================

/// Mouse packet flags
pub mod mouse_flags {
    pub const LEFT_BUTTON: u8 = 0x01;
    pub const RIGHT_BUTTON: u8 = 0x02;
    pub const MIDDLE_BUTTON: u8 = 0x04;
    pub const ALWAYS_SET: u8 = 0x08;
    pub const X_SIGN: u8 = 0x10;
    pub const Y_SIGN: u8 = 0x20;
    pub const X_OVERFLOW: u8 = 0x40;
    pub const Y_OVERFLOW: u8 = 0x80;
}


impl PS2Mouse {
    pub const fn new() -> Self {
        Self {
            packet_index: 0,
            packet: [0; 3],
            x: 0,
            y: 0,
            left_pressed: false,
            right_pressed: false,
            middle_pressed: false,
        }
    }

    /// Read mouse event (not available on RISC-V)
    pub fn read_event(&mut self) -> Option<InputEvent> {
        // PS/2 ports not available on RISC-V virt platform
        None
    }

    /// Check if data is available (always returns false on RISC-V)
    pub fn has_data(&self) -> bool {
        false
    }

    /// Get X position
    pub fn x(&self) -> i32 {
        self.x
    }

    /// Get Y position
    pub fn y(&self) -> i32 {
        self.y
    }
}

/// Global PS/2 mouse instance
pub static mut PS2_MOUSE: PS2Mouse = PS2Mouse::new();

// ============================================================================
// Initialization functions
// ============================================================================

/// Initialize PS/2 keyboard driver
pub fn init_keyboard() {
    // PS/2 keyboard not available on RISC-V
}

/// Initialize PS/2 mouse driver
pub fn init_mouse() {
    // PS/2 mouse not available on RISC-V
}

// ════════════════ input::virtio_input ═════════════════
// (was input/virtio_input.rs)
// VirtIO Input device driver
//
// Implements VirtIO Input PCI device initialization and event reading
// ============================================================================
// VirtIO Input PCI device IDs
// ============================================================================

/// VirtIO Input device Vendor ID (Red Hat)
const VIRTIO_INPUT_PCI_VENDOR: u16 = 0x1AF4;

/// VirtIO Input device Device ID (0x1040 + 18 = 0x1052)
const VIRTIO_INPUT_PCI_DEVICE: u16 = 0x1052;

// ============================================================================
// VirtIO Input queue indices
// ============================================================================

/// Event queue (device -> driver)
const EVENT_QUEUE: u16 = 0;
/// Status queue (driver -> device)
const STATUS_QUEUE: u16 = 1;

// ============================================================================
// VirtIO Input configuration structures
// ============================================================================

// VirtioInputConfig — the definition moved to drivers/types.rs. This
// file never referenced it (it names the device's config space through
// the device's own fields), so there is no re-export.


// ============================================================================
// Configuration selectors
// ============================================================================

/// Unused configuration
const VIRTIO_INPUT_CFG_UNSET: u8 = 0x00;
/// ID name string
const VIRTIO_INPUT_CFG_ID_NAME: u8 = 0x01;
/// ID serial number string
const VIRTIO_INPUT_CFG_ID_SERIAL: u8 = 0x02;
/// ID device IDs
const VIRTIO_INPUT_CFG_ID_DEVIDS: u8 = 0x03;
/// Property bitmap
const VIRTIO_INPUT_CFG_PROP_BITS: u8 = 0x10;
/// Event bitmap
const VIRTIO_INPUT_CFG_EV_BITS: u8 = 0x11;
/// Absolute axis info
const VIRTIO_INPUT_CFG_ABS_INFO: u8 = 0x12;

// ============================================================================
// VirtIO Input device
// ============================================================================


unsafe impl Send for VirtioInputDevice {}
unsafe impl Sync for VirtioInputDevice {}

impl VirtioInputDevice {
    /// Create new VirtIO Input device
    pub fn new(pci: VirtIOPCI) -> Option<Self> {
        let mut device = Self {
            pci,
            event_queue: None,
            event_buffer: core::ptr::null_mut(),
            event_buffer_layout: None,
            event_buffer_phys: 0,
            name: [0; 32],
            is_pointer: false,
            last_used: 0,
        };

        // A virtio-input device without a Device CFG capability is unusable
        // (every one of its registers lives there) — and it would make the
        // config reads below touch address 0.
        if device.pci.device_cfg_bar == 0 {
            return None;
        }

        device.init_virtio()?;
        device.read_device_info();

        Some(device)
    }

    /// Initialize VirtIO device
    fn init_virtio(&mut self) -> Option<()> {
        let common_cfg = self.pci.common_cfg_bar + self.pci.common_cfg_offset as u64;

        // INV-MMIO-1..3: the common-config register window for the
        // whole init sequence below.
        let common = crate::safety::mmio::MmioRegion::new(common_cfg as usize, 0x100);

        // Step 1: Reset device
        common.write_u8(DEVICE_STATUS as usize, 0);
        fence(Ordering::SeqCst);

        // Steps 2-3: Set ACKNOWLEDGE | DRIVER
        common.write_u8(
            DEVICE_STATUS as usize,
            (status::ACKNOWLEDGE | status::DRIVER) as u8,
        );
        fence(Ordering::SeqCst);

        // Steps 4-6: Feature negotiation (no special features needed)
        common.write_u8(
            DEVICE_STATUS as usize,
            (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK) as u8,
        );
        fence(Ordering::SeqCst);

        // Step 7: Verify FEATURES_OK
        let status_val = common.read_u8(DEVICE_STATUS as usize);
        if (status_val & status::FEATURES_OK as u8) == 0 {
            return None;
        }

        // Step 8: Initialize event queue
        common.write_u16(COMMON_CFG_QUEUE_SELECT as usize, EVENT_QUEUE);
        fence(Ordering::SeqCst);

        let queue_size = common.read_u16(COMMON_CFG_QUEUE_SIZE as usize);

        if queue_size == 0 {
            return None;
        }

        // Allocate event buffer
        let buffer_layout = Layout::from_size_align(
            queue_size as usize * core::mem::size_of::<VirtioInputEvent>(),
            4096,
        ).ok()?;

        let event_buffer = unsafe { alloc_zeroed(buffer_layout) };
        if event_buffer.is_null() {
            return None;
        }

        self.event_buffer = event_buffer as *mut VirtioInputEvent;
        self.event_buffer_layout = Some(buffer_layout);

        // Get physical address
        self.event_buffer_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(event_buffer as u64)
        ).0;

        // Create VirtQueue. The notify address comes from the spec formula
        // via get_notify_addr (reads queue_notify_off from common cfg; the
        // old inline `queue_index * multiplier * 2` scaled the multiplier
        // — already in bytes — a second time, review BUG "notify ×2").
        let isr_base = self.pci.isr_cfg_bar + self.pci.isr_cfg_offset as u64;

        let queue = VirtQueue::new(
            queue_size,
            EVENT_QUEUE,
            self.pci.get_notify_addr(EVENT_QUEUE),
            isr_base,
            isr_base + 4,
        )?;

        // Get queue physical addresses
        let desc_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(queue.get_desc_addr())
        ).0;
        let avail_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(queue.get_avail_addr())
        ).0;
        let used_phys = crate::arch::mm::virt_to_phys(
            crate::arch::mm::VirtAddr::new(queue.get_used_addr())
        ).0;

        // Set queue addresses (INV-MMIO-1..3: the common-config window).
        let common = crate::safety::mmio::MmioRegion::new(common_cfg as usize, 0x100);
        common.write_u32(COMMON_CFG_QUEUE_DESC_LO as usize, desc_phys as u32);
        common.write_u32(COMMON_CFG_QUEUE_DESC_HI as usize, (desc_phys >> 32) as u32);
        common.write_u32(COMMON_CFG_QUEUE_DRIVER_LO as usize, avail_phys as u32);
        common.write_u32(COMMON_CFG_QUEUE_DRIVER_HI as usize, (avail_phys >> 32) as u32);
        common.write_u32(COMMON_CFG_QUEUE_DEVICE_LO as usize, used_phys as u32);
        common.write_u32(COMMON_CFG_QUEUE_DEVICE_HI as usize, (used_phys >> 32) as u32);
        fence(Ordering::SeqCst);

        // Enable queue
        common.write_u16(COMMON_CFG_QUEUE_ENABLE as usize, 1);
        fence(Ordering::SeqCst);

        self.event_queue = Some(queue);

        // Step 9: Set DRIVER_OK
        common.write_u8(
            DEVICE_STATUS as usize,
            (status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK) as u8,
        );
        fence(Ordering::SeqCst);

        // Submit initial buffers to receive events
        self.submit_event_buffers();

        Some(())
    }

    /// Read device info
    fn read_device_info(&mut self) {
        // The virtio-input config registers (select/subsel/size/payload)
        // live in the DEVICE CFG capability BAR, NOT in common cfg. The
        // old code read/wrote config through common_cfg_bar — those writes
        // landed on device_feature_select and the "name" reads returned
        // common-cfg garbage, so is_pointer() misclassified every device
        // as a keyboard (review BUG: virtio-input 从错误 BAR 读 config).
        let config_base = self.pci.device_cfg_bar;

        // INV-MMIO-1..3: the device-config register window (select/
        // subsel/size + 32-byte payload at +8).
        let cfg = crate::safety::mmio::MmioRegion::new(config_base as usize, 8 + 128);
        {
            // Select ID_NAME configuration
            cfg.write_u8(0, VIRTIO_INPUT_CFG_ID_NAME);
            cfg.write_u8(1, 0);
            fence(Ordering::SeqCst);

            // Read name
            for i in 0..31usize {
                let c = cfg.read_u8(8 + i);
                if c == 0 {
                    break;
                }
                self.name[i] = c;
            }
        }

        // Detect if it is a pointer device
        self.is_pointer = self.check_pointer_device();
    }

    /// Check if it is a pointer device
    fn check_pointer_device(&self) -> bool {
        // Device CFG BAR — see read_device_info.
        let config_base = self.pci.device_cfg_bar;

        // INV-MMIO-1..3: device-config window (see read_device_info).
        let cfg = crate::safety::mmio::MmioRegion::new(config_base as usize, 8 + 128);
        {
            // Check EV_ABS event (absolute coordinates)
            cfg.write_u8(0, VIRTIO_INPUT_CFG_EV_BITS);
            cfg.write_u8(1, EV_ABS as u8);
            fence(Ordering::SeqCst);

            // Check ABS_X and ABS_Y bits
            let has_abs_x = (cfg.read_u8(8) & 0x01) != 0;

            if has_abs_x {
                return true;
            }

            // Check EV_REL event (relative coordinates)
            cfg.write_u8(0, VIRTIO_INPUT_CFG_EV_BITS);
            cfg.write_u8(1, EV_REL as u8);
            fence(Ordering::SeqCst);

            // Check REL_X and REL_Y bits
            let has_rel_x = (cfg.read_u8(8) & 0x01) != 0;

            has_rel_x
        }
    }

    /// Submit event buffers
    fn submit_event_buffers(&mut self) {
        let queue = match &self.event_queue {
            Some(q) => q,
            None => return,
        };

        let queue_size = queue.queue_size as usize;

        // INV-DMA-2: typed bounds-checked ring views.
        {
            // Submit all buffers
            for i in 0..queue_size {
                let event_ptr = self.event_buffer_phys + (i * core::mem::size_of::<VirtioInputEvent>()) as u64;

                let Some(desc) = queue.desc_mut(i as u16) else { continue };
                desc.addr = event_ptr;
                desc.len = core::mem::size_of::<VirtioInputEvent>() as u32;
                desc.flags = 0x02; // VIRTQ_DESC_F_WRITE
                desc.next = 0;

                // Add to available ring
                let idx = queue.avail_idx() as usize;
                queue.write_avail_ring_slot(idx % queue_size, i as u16);
                fence(Ordering::SeqCst);
                queue.write_avail_idx((idx as u16).wrapping_add(1));
            }

            fence(Ordering::SeqCst);
            queue.notify();
        }
    }

    /// Read input event
    pub fn read_event(&mut self) -> Option<InputEvent> {
        let queue = self.event_queue.as_ref()?;

        {
            let used_idx = queue.get_used() as usize;
            let last_used = self.last_used as usize;

            if crate::trace::EVENT_INPUTTRACE.armed() {
                static DBG_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
                let n = DBG_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 8 || used_idx != 0 {
                    let nm: alloc::vec::Vec<u8> = self.name_bytes_vec();
                    crate::pr_warn!("IN-RDBG used_idx={} last_used={} qs={} ptr={} name={}", used_idx, last_used, queue.queue_size, self.is_pointer, alloc::string::String::from_utf8_lossy(&nm));
                }
            }
            if used_idx == last_used {
                return None;
            }

            // Get used descriptor (volatile read: device writes via DMA)
            let used_elem = queue
                .get_used_elem((last_used % queue.queue_size as usize) as u16)
                .unwrap_or(crate::drivers::types::UsedElem { id: 0, len: 0 });

            let desc_idx = used_elem.id as usize;
            let _len = used_elem.len;

            // Read event
            // SAFETY: the event buffer is a live registered DMA buffer
            // of queue_size events (INV at init); volatile because the
            // device writes it.
            let event = unsafe { read_volatile(self.event_buffer.add(desc_idx)) };

            // Resubmit buffer
            let Some(desc) = queue.desc_mut(desc_idx as u16) else {
                return None;
            };
            desc.addr = self.event_buffer_phys + (desc_idx * core::mem::size_of::<VirtioInputEvent>()) as u64;
            desc.len = core::mem::size_of::<VirtioInputEvent>() as u32;
            desc.flags = 0x02;
            desc.next = 0;

            let aidx = queue.avail_idx() as usize;
            queue.write_avail_ring_slot(aidx % queue.queue_size as usize, desc_idx as u16);
            fence(Ordering::SeqCst);
            queue.write_avail_idx((aidx as u16).wrapping_add(1));
            self.last_used = self.last_used.wrapping_add(1);

            fence(Ordering::SeqCst);
            queue.notify();

            // Convert to standard InputEvent
            Some(InputEvent::new(event.type_, event.code, event.value))
        }
    }

    /// Check if there are events
    pub fn name_bytes_vec(&self) -> alloc::vec::Vec<u8> {
        self.name
            .iter()
            .take_while(|&&c| c != 0)
            .copied()
            .collect()
    }

    pub fn has_event(&self) -> bool {
        // INV-DMA-3: device-written used index via the typed volatile
        // getter.
        match &self.event_queue {
            Some(queue) => queue.get_used() as usize != self.last_used as usize,
            None => false,
        }
    }

    /// Get device name
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// Whether it is a pointer device
    pub fn is_pointer(&self) -> bool {
        self.is_pointer
    }

    /// ISR capability address of this function (the virtio device-side
    /// EOI register). Read lock-free by the hard-IRQ top half.
    pub fn isr_cfg_bar(&self) -> u64 {
        self.pci.isr_cfg_bar
    }

    /// riscv64 gpex INTx line for this function — the same swizzle the
    /// virtio-blk PCI registration uses: IRQ = 32 + ((pin-1 + slot) % 4).
    #[cfg(feature = "riscv64")]
    pub fn intx_irq_line(&self) -> u32 {
        let int_pin = self.pci.pci_config.read_config_byte(0x3D) as u32;
        32 + ((int_pin.saturating_sub(1) + self.pci.pci_slot as u32) % 4)
    }

    /// x86_64: register this function on its PIC INTx line with the
    /// shared virtio dispatcher (which reads the ISR and calls `service`
    /// only when this function raised the line).
    #[cfg(feature = "x86_64")]
    pub fn enable_pci_intx_irq(&self, service: fn()) -> Option<u8> {
        self.pci.enable_pci_intx_irq(service)
    }

    /// Read a code bitmap from the device config (VIRTIO_INPUT_CFG_EV_BITS).
    ///
    /// `ev_type` selects the event type (EV_SYN/EV_KEY/EV_REL/EV_ABS/...);
    /// the device answers with the bitmap of codes it can emit for that
    /// type. This is the authoritative capability source — exactly what a
    /// Linux virtio-input guest reports back through EVIOCGBIT(ev, len).
    ///
    /// Returns the number of valid bytes written to `out` (0 = the device
    /// does not support this event type).
    pub fn read_ev_bitmap(&mut self, ev_type: u8, out: &mut [u8]) -> usize {
        let config_base = self.pci.device_cfg_bar;
        if config_base == 0 {
            return 0;
        }
        // INV-MMIO-1..3: device-config window.
        let cfg = crate::safety::mmio::MmioRegion::new(config_base as usize, 8 + 128);
        {
            cfg.write_u8(0, VIRTIO_INPUT_CFG_EV_BITS);
            cfg.write_u8(1, ev_type);
            fence(Ordering::SeqCst);

            // u.size = bitmap size in bytes (spec: 5.10.6.2)
            let size = cfg.read_u8(2) as usize;
            if size == 0 {
                return 0;
            }
            let n = size.min(out.len()).min(128);
            for i in 0..n {
                out[i] = cfg.read_u8(8 + i);
            }
            n
        }
    }

    /// Read absolute-axis info from the device config
    /// (VIRTIO_INPUT_CFG_ABS_INFO): { min, max, fuzz, flat, res } as LE u32s
    /// (virtio_absinfo layout). Mirrors what Linux reports via
    /// EVIOCGABS(axis) — xf86-input-evdev reads ABS min/max to scale the
    /// tablet's absolute coordinates to the screen.
    pub fn read_absinfo(&mut self, axis: u8) -> Option<[u32; 5]> {
        let config_base = self.pci.device_cfg_bar;
        if config_base == 0 {
            return None;
        }
        // INV-MMIO-1..3: device-config window.
        let cfg = crate::safety::mmio::MmioRegion::new(config_base as usize, 8 + 128);
        {
            cfg.write_u8(0, VIRTIO_INPUT_CFG_ABS_INFO);
            cfg.write_u8(1, axis);
            fence(Ordering::SeqCst);

            let size = cfg.read_u8(2) as usize;
            if size < 20 {
                return None;
            }
            let mut vals = [0u32; 5];
            for (i, v) in vals.iter_mut().enumerate() {
                let mut b = [0u8; 4];
                for (j, bj) in b.iter_mut().enumerate() {
                    *bj = cfg.read_u8(8 + i * 4 + j);
                }
                *v = u32::from_le_bytes(b);
            }
            Some(vals)
        }
    }
}

// UsedElem — the definition moved to drivers/types.rs and, because
// virtio/queue.rs declares the same name for its own used-ring element
// (the one every caller uses), it was renamed VirtioInputUsedElem there.
// This file never used its local declaration (it names the virtio one by
// its full path), so there is no re-export.

impl Drop for VirtioInputDevice {
    fn drop(&mut self) {
        if let Some(layout) = self.event_buffer_layout {
            if !self.event_buffer.is_null() {
                unsafe {
                    dealloc(self.event_buffer as *mut u8, layout);
                }
            }
        }
    }
}

// ============================================================================
// Device probing
// ============================================================================

/// Probe VirtIO Input devices
pub fn probe_virtio_input_devices() -> Option<(VirtioInputDevice, Option<VirtioInputDevice>)> {
    let mut keyboard: Option<VirtioInputDevice> = None;
    let mut pointer: Option<VirtioInputDevice> = None;

    // Shared ECAM walker: 0x8000 stride per slot + all 8 functions (the
    // old 0x1000 stride hid every slot >= 4 — review BUG).
    for ecam_addr in find_ecam_devices(VIRTIO_INPUT_PCI_VENDOR, &[VIRTIO_INPUT_PCI_DEVICE]) {
        if let Ok(virtio_pci) = VirtIOPCI::new(ecam_addr) {
            if let Some(input_dev) = VirtioInputDevice::new(virtio_pci) {
                if input_dev.is_pointer() {
                    if pointer.is_none() {
                        pointer = Some(input_dev);
                    }
                } else {
                    if keyboard.is_none() {
                        keyboard = Some(input_dev);
                    }
                }
            }
        }

        // If both devices found, stop
        if keyboard.is_some() && pointer.is_some() {
            break;
        }
    }

    if keyboard.is_some() || pointer.is_some() {
        // Return (keyboard, pointer)
        Some((keyboard?, pointer))
    } else {
        None
    }
}

/// Probe single VirtIO Input device
pub fn probe_virtio_input() -> Option<VirtioInputDevice> {
    // Shared ECAM walker: 0x8000 stride per slot + all 8 functions (the
    // old 0x1000 stride hid every slot >= 4 — review BUG).
    for ecam_addr in find_ecam_devices(VIRTIO_INPUT_PCI_VENDOR, &[VIRTIO_INPUT_PCI_DEVICE]) {
        if let Ok(virtio_pci) = VirtIOPCI::new(ecam_addr) {
            return VirtioInputDevice::new(virtio_pci);
        }
    }

    None
}

// ════════════════ input::evdev ════════════════════════
// (was input/evdev.rs)
// evdev character device interface
//
// Provides compatible /dev/input/eventX device
//
// Linux-parity checklist (xf86-input-evdev / libevdev / evtest probe path):
// - EVIOCGVERSION / EVIOCGID / EVIOCGNAME(len) / EVIOCGPROP(len)
// - EVIOCGBIT(ev, len) from the virtio device's own capability bitmaps
// - EVIOCGABS(abs) from the virtio device's ABS_INFO config
// - EVIOCGRAB (exclusive access) / EVIOCGKEY / EVIOCGLED / EVIOCGSW
// - blocking read (sleep until an event arrives) and poll reporting POLLIN
// - 24-byte input_event passthrough including EV_SYN/SYN_REPORT packets
// ============================================================================
// evdev ioctl command decoding
// ============================================================================

/// ioctl NR (command byte) base values from <uapi/linux/input.h>. The size
/// field of EVIOCGNAME/EVIOCGBIT-style commands carries the user buffer
/// length and must be honored, so commands are decoded by NR + DIR instead
/// of compared as whole u32s.
const NR_VERSION: u32 = 0x01; // EVIOCGVERSION
const NR_ID: u32 = 0x02; // EVIOCGID
const NR_REP_GET: u32 = 0x03; // EVIOCGREP
const NR_REP_SET: u32 = 0x04; // EVIOCSREP
const NR_NAME: u32 = 0x06; // EVIOCGNAME(len)
const NR_PHYS: u32 = 0x07; // EVIOCGPHYS(len)
const NR_UNIQ: u32 = 0x08; // EVIOCGUNIQ(len)
const NR_PROP: u32 = 0x09; // EVIOCGPROP(len)
const NR_KEY_STATE: u32 = 0x18; // EVIOCGKEY(len)
const NR_LED_STATE: u32 = 0x19; // EVIOCGLED(len)
const NR_SND_STATE: u32 = 0x1a; // EVIOCGSND(len)
const NR_SW_STATE: u32 = 0x1b; // EVIOCGSW(len)
const NR_BIT_BASE: u32 = 0x20; // EVIOCGBIT(ev, len): 0x20 + ev
const NR_ABS_BASE: u32 = 0x40; // EVIOCGABS(abs): 0x40 + abs
const NR_GRAB: u32 = 0x90; // EVIOCGRAB
const NR_SCLOCKID: u32 = 0xa0; // EVIOCSCLOCKID

const IOC_READ: u32 = 2; // _IOC_WRITE/_IOC_READ from asm-generic/ioctl.h
const IOC_WRITE: u32 = 1;

#[inline]
fn ioc_nr(cmd: u32) -> u32 {
    cmd & 0xff
}

#[inline]
fn ioc_size(cmd: u32) -> usize {
    ((cmd >> 16) & 0x3fff) as usize
}

#[inline]
fn ioc_dir(cmd: u32) -> u32 {
    cmd >> 30
}

// ============================================================================
// Input device ID structure
// ============================================================================


// ============================================================================
// Capability bitmaps
// ============================================================================


/// Fill `out` with the derived "supported event types" bitmap (what Linux
/// reports for EVIOCGBIT(0, len)): a type is supported when its code bitmap
/// is non-empty; EV_SYN is always present (input core guarantees it).
fn evtype_bitmap(bits: &[[u8; BITS_LEN]; BIT_TYPES]) -> [u8; BITS_LEN] {
    let mut out = [0u8; BITS_LEN];
    out[0] |= 1 << (EVDEV_EV_SYN & 7); // bit 0
    if bits[1].iter().any(|&b| b != 0) {
        out[0] |= 1 << (EV_KEY & 7); // bit 1
    }
    if bits[2].iter().any(|&b| b != 0) {
        out[0] |= 1 << (EV_REL & 7); // bit 2
    }
    if bits[3].iter().any(|&b| b != 0) {
        out[0] |= 1 << (EV_ABS & 7); // bit 3
    }
    out
}

// ============================================================================
// evdev device
// ============================================================================

/// evdev event queue maximum capacity - from config
const EVENT_QUEUE_SIZE: usize = crate::config::EVDEV_EVENT_QUEUE_SIZE;


impl EvdevDevice {
    /// Create new evdev device
    pub fn new(name: &[u8], is_pointer: bool) -> Self {
        let mut name_arr = [0u8; 32];
        let len = name.len().min(31);
        name_arr[..len].copy_from_slice(&name[..len]);

        Self {
            name: name_arr,
            open_file_ids: Spinlock::new([0u64; 4]),
            open_file_count: core::sync::atomic::AtomicU32::new(0),
            events_pushed: core::sync::atomic::AtomicU32::new(0),
            id: InputId {
                bustype: 0x0019, // BUS_VIRTIO
                vendor: 0x1AF4,  // Red Hat
                product: if is_pointer { 0x1052 } else { 0x1052 },
                version: 0x0001,
            },
            is_pointer,
            ev_bits: [[0u8; BITS_LEN]; BIT_TYPES],
            absinfo: [InputAbsinfo::default(); 4],
            grabbed: AtomicBool::new(false),
            led_state: AtomicU32::new(0),
            event_queue: Spinlock::new(VecDeque::with_capacity(EVENT_QUEUE_SIZE)),
            poll_waitq: crate::process::types::WaitQueueHead::new(),
        }
    }

    /// Install a capability bitmap for event type `ev` (0..=3).
    pub fn set_ev_bits(&mut self, ev: usize, bits: &[u8; BITS_LEN]) {
        if ev < BIT_TYPES {
            self.ev_bits[ev] = *bits;
        }
    }

    /// Install ABS axis info. `axis` is the evdev ABS code: ABS_X(0),
    /// ABS_Y(1) map to slots 0/1; ABS_MT_POSITION_X(0x35)/Y(0x36) map to
    /// 2/3. Any other axis is ignored (no slot for it).
    pub fn set_absinfo(&mut self, axis: usize, min: i32, max: i32, fuzz: i32, flat: i32, res: i32) {
        let slot = match axis {
            0 => 0,
            1 => 1,
            0x35 => 2,
            0x36 => 3,
            _ => return,
        };
        self.absinfo[slot] = InputAbsinfo {
            value: 0,
            minimum: min,
            maximum: max,
            fuzz,
            flat,
            resolution: res,
        };
    }

    /// Register an open file description on this device so event
    /// arrivals can re-arm its epoll watchers.
    pub fn register_open_file(&self, file_id: u64) {
        if file_id == 0 {
            return;
        }
        let mut ids = self.open_file_ids.lock_irqsave();
        let n = self.open_file_count.load(Ordering::Acquire) as usize;
        for slot in ids.iter_mut() {
            if *slot == 0 {
                *slot = file_id;
                self.open_file_count.store((n + 1) as u32, Ordering::Release);
                return;
            }
        }
    }

    /// Forget a closed file description (its epoll watchers are gone).
    pub fn unregister_open_file(&self, file_id: u64) {
        let mut ids = self.open_file_ids.lock_irqsave();
        let mut n = self.open_file_count.load(Ordering::Acquire) as usize;
        let mut i = 0;
        while i < n && i < ids.len() {
            if ids[i] == file_id {
                for j in i + 1..n {
                    ids[j - 1] = ids[j];
                }
                ids[n - 1] = 0;
                n -= 1;
                i = 0;
                continue;
            }
            i += 1;
        }
        self.open_file_count.store(n as u32, Ordering::Release);
    }

/// Push event (called from softirq context)
pub fn push_event(&self, event: InputEvent) {
        {
            // rux:inputtrace (retired debug=inputtrace): the input arrival trace (release-visible via
            // the raw UART path — this is the datum that says whether the
            // guest's event source reaches the device at all). Line
            // rides the taskdump funnel (obs-hygiene): one atomic write,
            // not per-byte uart_putc_raw the kmsg flood could splice.
            if crate::trace::EVENT_INPUTTRACE.armed() {
                let n = self.events_pushed.fetch_add(1, Ordering::Relaxed);
                if n < 24 {
                    let nm: alloc::vec::Vec<u8> = self
                        .name
                        .iter()
                        .take_while(|&&c| c != 0)
                        .copied()
                        .collect();
                    let line = alloc::format!(
                        "IN-EV n={} name={} type={} code={} val={}\n",
                        n,
                        String::from_utf8_lossy(&nm),
                        event.type_,
                        event.code,
                        event.value
                    );
                    crate::debug::taskdump::taskdump_raw_line(line.as_bytes());
                }
            }
        }
        {
            let mut queue = self.event_queue.lock_irqsave();
            if queue.len() >= EVENT_QUEUE_SIZE {
                queue.pop_front();
            }
            queue.push_back(event);
        }
        // Wake poll/select waiters OUTSIDE the event_queue lock (INV-LOCK-6
        // discipline: never nest a wait-queue wake inside a held spinlock).
        // input:evdev_wake (wake-drop hunt): when armed, wrap the wake with
        // a per-entry verdict — pre/post entry snapshots around
        // wake_up_all() split EMPTY / DELIVERED / REFUSED / STICKY / GONE
        // for every parked poller, with the GRQ linkage snapshot after.
        // Off by default like every tracepoint; the serial echo rides the
        // same armed() gate with the INEV budget discipline.
        if crate::trace::EVENT_EVDEV_WAKE.armed() {
            self.wake_pollq_probed();
        } else {
            self.poll_waitq.wake_up_all();
        }
        // …and wake the EPOLL waiters: an epoll-based reader (OH's
        // multimodalinput reads /dev/input/eventN through epoll) that is
        // blocked in epoll_wait() is parked on the EPOLL wait queue, not
        // on this device's poll queue — the device wake above never
        // reaches it. Without this notification the event sits in the
        // queue and the UI never sees the gesture (the launcher swipe
        // stays dead while the screen renders fine).
        //
        // Notify each open file description on this device: the epoll
        // wake registry is keyed by File.file_id.
        let n = self.open_file_count.load(Ordering::Acquire) as usize;
        if n > 0 {
            let ids = self.open_file_ids.lock_irqsave();
            for &fid in ids.iter().take(n.min(ids.len())) {
                if fid != 0 {
                    crate::fs::event_notify::epoll_notify_file(fid);
                }
            }
        }
    }

    /// input:evdev_wake probe: wake the poll wait queue with a per-entry
    /// verdict (the wake-drop hunt's per-wake answer). Runs only when the
    /// tracepoint is armed; the serial echo carries the same verdicts so
    /// the OH rig (no in-guest tracefs) reads them off serial.
    ///
    /// Verdicts per pre-wake entry:
    ///   0 EMPTY     — queue had no entries at push time
    ///   1 DELIVERED — wake consumed the token AND the target left the
    ///                 sleeping state (linked for a pick, or already
    ///                 running through its poll re-scan)
    ///   2 REFUSED   — token returned (wake_up_process refused: the W1
    ///                 path — the entry stays eligible for the next push)
    ///   3 STICKY    — pre-woken token on a still-sleeping target at post
    ///                 time (the sc100 re-issue attempted this push)
    ///   4 GONE      — entry vanished inside the window (the sleeper's
    ///                 unwait raced the push — its own re-check owns the
    ///                 event)
    ///   5 RERAN     — token consumed and the target is parked AGAIN at
    ///                 post time: the wake ran it through the poll loop
    ///                 within the window (fast SMP round trip)
    fn wake_pollq_probed(&self) {
        use core::sync::atomic::{AtomicU64, Ordering};
        let (pre_waiters, pre) = self.poll_waitq.debug_entries_ptr();
        if pre.is_empty() {
            crate::trace::trace_evdev_wake(0, 0, 0, 0, 2, pre_waiters as u32);
            static EMPTY_N: AtomicU64 = AtomicU64::new(0);
            let n = EMPTY_N.fetch_add(1, Ordering::Relaxed);
            if input_echo_budget(n) {
                let q = self.event_queue.lock_irqsave().len();
                let mut l = HuntLine::new();
                l.push(b"INWK[");
                l.u64(n);
                l.push(b"]E ");
                for &c in self.name.iter().take_while(|&&c| c != 0).take(10) {
                    l.push(&[c]);
                }
                l.push(b" q=");
                l.u64(q as u64);
                // w= names the waiters COUNTER (not just list emptiness):
                // w>0 with an empty list is the counter/list divergence
                // signature; w=0 confirms nobody is registered at all.
                l.push(b" w=");
                l.u64(pre_waiters as u64);
                // stE= the R127 wake-time stale-eviction total: stE>0 on
                // this boot means the screen has already self-healed
                // leaked entries from dead parkers upstream of this push.
                l.push(b" stE=");
                l.u64(crate::process::hotpath::WQ_STALE_EVICTED.load(Ordering::Relaxed));
                l.push(b"\n");
                l.flush();
            }
            return;
        }
        self.poll_waitq.wake_up_all();
        let (post_waiters, post) = self.poll_waitq.debug_entries_ptr();
        static WK_ECHO_N: AtomicU64 = AtomicU64::new(0);
        for (pid, pre_state, pre_woken, task) in pre.iter() {
            if *pid == 0 {
                continue;
            }
            let post_e = post.iter().find(|(p, _, _, _)| p == pid);
            let (verdict, post_state) = match post_e {
                None => (4u32, 0u32),
                Some((_, st, woken, _)) => {
                    let st = *st;
                    let sleeping = st & 0x3 != 0; // INTERRUPTIBLE|UNINTERRUPTIBLE bits
                    if !*pre_woken {
                        if !*woken {
                            (2, st) // token returned — refused
                        } else if sleeping {
                            (5, st) // delivered, ran, re-parked inside window
                        } else {
                            (1, st) // delivered
                        }
                    } else if sleeping {
                        (3, st) // sticky token on sleeping target
                    } else {
                        (1, st) // re-issue delivered
                    }
                }
            };
            let linked = match crate::sched::hotpath::grq_diag_cfs_linked(*task) {
                Some(true) => 1,
                Some(false) => 0,
                None => 2,
            };
            crate::trace::trace_evdev_wake(
                *pid, verdict, *pre_state, post_state, linked, post_waiters as u32,
            );
            let n = WK_ECHO_N.fetch_add(1, Ordering::Relaxed);
            if input_echo_budget(n) {
                let mut l = HuntLine::new();
                l.push(b"INWK[");
                l.u64(n);
                l.push(b"] pid=");
                l.u64(*pid as u64);
                l.push(b" v=");
                l.u64(verdict as u64);
                l.push(b" pre=");
                l.u64(*pre_state as u64);
                l.push(b" post=");
                l.u64(post_state as u64);
                l.push(b" lnk=");
                l.u64(linked as u64);
                l.push(b" w=");
                l.u64(post_waiters as u64);
                l.push(b"\n");
                l.flush();
            }
        }
    }

    /// Read event
    pub fn pop_event(&self) -> Option<InputEvent> {
        let ev = self.event_queue.lock_irqsave().pop_front();
        // input:evdev_wake hunt: name the consumer even when the INRD
        // echo tears on the raw UART — one short line per pop (budgeted),
        // first-8-chars device name + remaining depth.
        if ev.is_some() && crate::trace::EVENT_EVDEV_WAKE.armed() {
            use core::sync::atomic::{AtomicU64, Ordering};
            static POP_N: AtomicU64 = AtomicU64::new(0);
            let n = POP_N.fetch_add(1, Ordering::Relaxed);
            if input_echo_budget(n) {
                let remain = self.event_queue.lock_irqsave().len();
                let mut l = HuntLine::new();
                l.push(b"INPOP[");
                l.u64(n);
                l.push(b"] ");
                l.u64(crate::process::hotpath::current_pid() as u64);
                l.push(b" ");
                for &c in self.name.iter().take_while(|&&c| c != 0).take(10) {
                    l.push(&[c]);
                }
                l.push(b" r=");
                l.u64(remain as u64);
                l.push(b"\n");
                l.flush();
            }
        }
        ev
    }

    /// Check if there are events
    pub fn has_event(&self) -> bool {
        !self.event_queue.lock_irqsave().is_empty()
    }
}

// ============================================================================
// Global evdev devices
// ============================================================================

/// Keyboard evdev device (round-5: the OnceSlot owns the init-once
/// publication the hand-rolled static mut carried).
pub static EVDEV_KEYBOARD: crate::safety::slot::OnceSlot<EvdevDevice> =
    crate::safety::slot::OnceSlot::new();

/// Pointer evdev device
pub static EVDEV_POINTER: crate::safety::slot::OnceSlot<EvdevDevice> =
    crate::safety::slot::OnceSlot::new();

/// Touchscreen evdev device (event2): mirrors pointer coordinates with
/// touch protocol for OH multimodalinput.
pub static EVDEV_TOUCHSCREEN: crate::safety::slot::OnceSlot<EvdevDevice> =
    crate::safety::slot::OnceSlot::new();

// Registry accessors (INV-SREF-1 shape, unsafe-slim round 3): the
// three device slots are written ONCE by init_evdev (single-threaded
// boot) and never cleared; every EvdevDevice field readers/writers
// touch is an atomic or Spinlock-guarded, so shared/mutable views
// handed out here are sound post-init. These are the ONLY audited
// edges — call sites carry no unsafe.

pub fn evdev_keyboard() -> Option<&'static EvdevDevice> {
    EVDEV_KEYBOARD.get()
}
pub fn evdev_pointer() -> Option<&'static EvdevDevice> {
    EVDEV_POINTER.get()
}
pub fn evdev_touchscreen() -> Option<&'static EvdevDevice> {
    EVDEV_TOUCHSCREEN.get()
}


/// Whether a file id belongs to one of the three open evdev file
/// descriptions (the epoll_ctl probe uses this to filter).
pub fn file_id_is_input(fid: u64) -> bool {
    [evdev_keyboard(), evdev_pointer(), evdev_touchscreen()]
        .iter()
        .flatten()
        .any(|d| {
            let ids = d.open_file_ids.lock_irqsave();
            let n = d.open_file_count.load(Ordering::Acquire) as usize;
            ids.iter().take(n.min(ids.len())).any(|&x| x == fid)
        })
}

/// Pids that opened an evdev node this boot (wake-drop hunt, max 8):
/// the PPARK/PRET echoes filter on this set so the input openers' poll
/// rounds are all visible without flooding the console with every
/// GLib main-loop park on the machine.
static EVDEV_OPENER_PIDS: [core::sync::atomic::AtomicU32; 8] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 8];

fn latch_opener_pid(pid: u32) {
    for slot in EVDEV_OPENER_PIDS.iter() {
        match slot.compare_exchange(
            0,
            pid,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(v) if v == pid => return,
            Err(_) => continue,
        }
    }
}

/// True when `pid` opened an evdev node this boot (hunt filter).
pub fn evdev_opener_pid(pid: u32) -> bool {
    EVDEV_OPENER_PIDS
        .iter()
        .any(|s| s.load(Ordering::Acquire) == pid)
}

/// Resolve an EvdevDevice by its devfs device number (the devfs open
/// path uses this to stamp the device with the new file description).
pub fn device_of_devno(devno: DevNo) -> Option<&'static EvdevDevice> {
    if devno == DEV_EVDEV_KEYBOARD {
        evdev_keyboard()
    } else if devno == DEV_EVDEV_POINTER {
        evdev_pointer()
    } else if devno == crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN {
        evdev_touchscreen()
    } else {
        None
    }
}

/// Resolve the EvdevDevice a file was opened against. Returns None for
/// non-evdev files (caller falls through to other ioctl handlers).
// EVDEV_* are initialized by init_evdev() before any file operations can
// occur; private_data holds a valid DevNo boxed by devfs_open for as long
// as the file lives (the one audited deref is below).
fn device_of_file(file: &File) -> Option<&'static EvdevDevice> {
    let devno = file.private_data_ptr().map(|p| {
        // SAFETY: devfs_open stored a Box<DevNo> (file-lifetime).
        unsafe { *(p as *const DevNo) }
    })?;
    if devno == DEV_EVDEV_KEYBOARD {
        evdev_keyboard()
    } else if devno == DEV_EVDEV_POINTER {
        evdev_pointer()
    } else if devno == crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN {
        evdev_touchscreen()
    } else {
        None
    }
}

// ============================================================================
// FileOps implementation
// ============================================================================

/// Interruptible timed sleep (milliseconds) used by the blocking read —
/// the task actually sleeps (yields the CPU) instead of busy-polling and
/// wakes early on signals.
///
/// Returns false if no task context / timer was available (caller should
/// fall back to a yield-style retry).
fn sleep_ms_interruptible(ms: u64) -> bool {
    use crate::process::types::{Task, TaskState};

    let target =
        crate::drivers::hotpath::timer::get_jiffies() + crate::drivers::hotpath::timer::msecs_to_jiffies(ms);

    let current = match crate::sched::hotpath::current() {
        Some(c) => c,
        None => return false,
    };
    // current() is the safe handle to the running task (we are it).
    let pid = current.pid();

    // The input tasklet BH (IRQ-driven delivery) takes the INPUT_* locks
    // with IRQs disabled, so every task-context taker must too — a plain
    // holder interrupted by the device IRQ would deadlock against the BH
    // at irq_exit on this CPU. Registration failure means we must NOT
    // sleep.
    let timer_id = crate::timer::add_timer_wakeup(target, pid);
    if timer_id == 0 {
        return false;
    }

    loop {
        let jiffies_now = crate::drivers::hotpath::timer::get_jiffies();
        if jiffies_now >= target || crate::signal::signal_pending() {
            crate::timer::del_timer(timer_id);
            return true;
        }

        // Mark INTERRUPTIBLE BEFORE the final re-check (state-first lost-
        // wakeup discipline, same as sys_nanosleep).
        current.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        let jiffies_now = crate::drivers::hotpath::timer::get_jiffies();
        if jiffies_now >= target || crate::signal::signal_pending() {
            current.set_state(TaskState::new(TaskState::RUNNING));
            // A racing wake may have enqueued us while still executing —
            // take ourselves back off (NEW-C2 discipline).
            crate::sched::hotpath::dequeue_task(current);
            crate::timer::del_timer(timer_id);
            return true;
        }

        // Syscall context runs with SIE=0 — re-arm so the timer tick can
        // reach this CPU and wake us.
        crate::arch::cpu::restore_irq(true);
        crate::sched::hotpath::schedule();
        // Woke up (timer expiry or signal): loop tail re-checks and exits.
    }
}

/// Resolve the EvdevDevice AND its devfs minor a file was opened
/// against (the minor is the input:evdev_read/evdev_open `node` field).
// Same contract as device_of_file.
fn node_of_file(file: &File) -> Option<(&'static EvdevDevice, u32)> {
    let devno = file.private_data_ptr().map(|p| {
        // SAFETY: as device_of_file (Box<DevNo>, file-lifetime).
        unsafe { *(p as *const DevNo) }
    })?;
    if devno == DEV_EVDEV_KEYBOARD {
        evdev_keyboard().map(|d| (d, devno.minor))
    } else if devno == DEV_EVDEV_POINTER {
        evdev_pointer().map(|d| (d, devno.minor))
    } else if devno == crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN {
        evdev_touchscreen().map(|d| (d, devno.minor))
    } else {
        None
    }
}

/// evdev read function
///
/// Linux semantics: a blocking fd sleeps until at least one event is
/// available (returning exactly one input_event per read); O_NONBLOCK
/// returns -EAGAIN when the queue is empty. The IRQ-driven tasklet BH
/// pushes events and wakes the poll_waitq; the drain here is the backstop
/// for events that raced the BH (the reader-side re-check is free).
fn evdev_file_read(file: &File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data contains a valid DevNo pointer set during device
    // open; the EVDEV_* statics are initialized by init_evdev() before any
    // file operations can occur.
    let (device, node) = match node_of_file(file) {
        Some(d) => d,
        None => return -9, // EBADF
    };

    let event_size = core::mem::size_of::<InputEvent>();
    if buf.len() < event_size {
        return -22; // EINVAL
    }

    let nonblock = file.flags_bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0;

    loop {
        // Poll for new events
        poll_virtio_events();

        match device.pop_event() {
            Some(event) => {
                // input:evdev_read — hop 2 of the input chain: a
                // userspace reader took this event off the node. The
                // reader's pid/comm ride the trace record's TASK-PID
                // column; the serial echo spells the pid out (the OH
                // rig reads its verdict off serial).
                if crate::trace::EVENT_EVDEV_READ.armed() {
                    crate::trace::trace_evdev_read(node, event.type_, event.code, event.value);
                    use core::sync::atomic::{AtomicU64, Ordering};
                    static RD_ECHO_N: AtomicU64 = AtomicU64::new(0);
                    let n = RD_ECHO_N.fetch_add(1, Ordering::Relaxed);
                    if input_echo_budget(n) {
                        let (pid, tgid) = current_pid_tgid();
                        itt_msg(b"INRD[");
                        itt_u64(n);
                        itt_msg(b"] pid=");
                        itt_u64(pid);
                        itt_msg(b"(");
                        itt_u64(tgid);
                        itt_msg(b") node=event");
                        itt_u64(node.saturating_sub(64) as u64);
                        itt_msg(b" type=");
                        itt_u64(event.type_ as u64);
                        itt_msg(b" code=");
                        itt_u64(event.code as u64);
                        itt_msg(b" value=");
                        itt_i64(event.value as i64);
                        itt_msg(b"\n");
                    }
                }
                // rux:inputtrace (retired debug=inputtrace): the userspace READ hop — pid, which node,
                // how many events remained after this one. If this prints
                // during a gesture the delivery chain is complete and any
                // remaining failure is consumer-side semantics.
                if crate::trace::EVENT_INPUTTRACE.armed() {
                    static RD_N: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let rn = RD_N.fetch_add(1, Ordering::Relaxed);
                    if rn < 40 {
                        let nm: alloc::vec::Vec<u8> = device
                            .name
                            .iter()
                            .take_while(|&&c| c != 0)
                            .copied()
                            .collect();
                        crate::pr_warn!(
                            "IN-READ #{} pid={} name={} type={} code={} val={} remain={}",
                            rn,
                            crate::process::hotpath::current_pid(),
                            alloc::string::String::from_utf8_lossy(&nm),
                            event.type_,
                            event.code,
                            event.value,
                            device.has_event() as u8
                        );
                    }
                }
                // Copy event to buffer (bounds-checked byte-image write).
                let _ = crate::safety::bytes::write_at(buf, 0, event);
                return event_size as isize;
            }
            None => {
                if nonblock {
                    return -11; // EAGAIN
                }
                if crate::signal::signal_pending() {
                    return -4; // EINTR
                }
                if !sleep_ms_interruptible(EVDEV_READ_POLL_MS) {
                    // No timer slot: yield rather than burn the CPU.
                    crate::sched::hotpath::yield_cpu();
                }
            }
        }
    }
}

/// Backstop poll interval for the blocking read sleep.
const EVDEV_READ_POLL_MS: u64 = 10;

/// rux:inputtrace (retired debug=inputtrace): announce which node the guest opened. Also records
/// input:evdev_open (hop 2a of the input chain — a consumer opened the
/// node; pid/comm name the opener in the trace record header and the
/// serial echo). `node` is the devfs minor (64/65/66).
pub fn trace_open(dev: &EvdevDevice, file: &File, node: u32) {
    // The wake-drop hunt's opener latch (PPARK/PRET filter) — armed
    // with the same gate as the rest of the hunt echoes.
    if crate::trace::EVENT_EVDEV_OPEN.armed() {
        latch_opener_pid(crate::process::hotpath::current_pid() as u32);
    }
    // The tracepoint path first (it's the policy instrument; the debug
    // prints below are the frozen legacy view of the same event).
    if crate::trace::EVENT_EVDEV_OPEN.armed() {
        crate::trace::trace_evdev_open(node);
        use core::sync::atomic::{AtomicU64, Ordering};
        static OP_ECHO_N: AtomicU64 = AtomicU64::new(0);
        let n = OP_ECHO_N.fetch_add(1, Ordering::Relaxed);
        if n < 64 {
            let (pid, tgid) = current_pid_tgid();
            itt_msg(b"INOP[");
            itt_u64(n);
            itt_msg(b"] pid=");
            itt_u64(pid);
            itt_msg(b"(");
            itt_u64(tgid);
            itt_msg(b") node=event");
            itt_u64(node.saturating_sub(64) as u64);
            itt_msg(b" file_id=");
            itt_u64(file.file_id);
            itt_msg(b"\n");
        }
    }
    if !crate::trace::EVENT_INPUTTRACE.armed() {
        return;
    }
    let nm: alloc::vec::Vec<u8> = dev
        .name
        .iter()
        .take_while(|&&c| c != 0)
        .copied()
        .collect();
    let s = alloc::format!("IN-OPEN pid={} name={} file_id={}\n", crate::process::hotpath::current_pid(), String::from_utf8_lossy(&nm), file.file_id);
    // obs-hygiene: line-atomic funnel instead of per-byte raw writes.
    crate::debug::taskdump::taskdump_raw_line(s.as_bytes());
}

/// evdev close function
fn evdev_file_close(file: &File) -> i32 {
    // Release a grab held through this fd and free the DevNo boxed by
    // devfs_open.
    {
        if let Some(ptr) = file.private_data_ptr() {
            // SAFETY: devfs_open stored a Box<DevNo> (file-lifetime).
            let devno = unsafe { *(ptr as *const DevNo) };
            // The file description is going away — drop its epoll
            // notification registration.
            if let Some(dev) = device_of_devno(devno) {
                dev.unregister_open_file(file.file_id);
            }
            if devno == DEV_EVDEV_KEYBOARD {
                if let Some(d) = EVDEV_KEYBOARD.get() {
                    d.grabbed.store(false, Ordering::Release);
                }
            } else if devno == DEV_EVDEV_POINTER {
                if let Some(d) = EVDEV_POINTER.get() {
                    d.grabbed.store(false, Ordering::Release);
                }
            }
            // SAFETY: the Box<DevNo> devfs_open stored (sole ref).
            drop(unsafe { Box::from_raw(ptr as *mut DevNo) });
        }
    }
    0
}

/// evdev poll function
///
/// Report readable whenever the queue holds an event. The virtqueues are
/// drained first so events that raced the tasklet BH still count (and so
/// a poll-only client — no blocking read — keeps receiving).
fn evdev_file_poll(file: &File, events: u16) -> u16 {
    use crate::fs::event_notify::poll_events::*;

    // SAFETY: private_data contains a valid DevNo pointer set during open;
    // the EVDEV_* statics are initialized by init_evdev() before any file
    // operations can occur.
    let device = match device_of_file(file) {
        Some(d) => d,
        None => return POLLERR,
    };

    let mut ready = 0u16;
    if events & POLLIN != 0 {
        // Drain the virtio queues so recently-arrived input counts.
        poll_virtio_events();
        if device.has_event() {
            ready |= POLLIN | POLLRDNORM;
        }
        // rux:inputtrace (retired debug=inputtrace) (leg-9 epoll hunt): one line per POLL dispatch,
        // raw-console (loglevel-proof; pr_info is dropped at loglevel=3).
        // The r13ep2/r13ep3 A/B proved the virtqueues ARE drained during
        // an epoll-only boot (14 INPTRC) yet epoll_wait returned idle —
        // this line ties every dispatch to its caller's file and result,
        // splitting "dispatch never ran" from "dispatch ran, reported
        // ready, but the waiter still idled". Rate-limited: first 600
        // then 1-in-50 (a spinning poller must not flood the console).
        if crate::trace::EVENT_INPUTTRACE.armed() {
            use core::sync::atomic::{AtomicU64, Ordering};
            static PL_N: AtomicU64 = AtomicU64::new(0);
            let n = PL_N.fetch_add(1, Ordering::Relaxed);
            if n < 600 || n % 50 == 0 {
                it_msg(b"INPPL[");
                it_u64(n);
                it_msg(b"] file=");
                it_u64(file.file_id);
                it_msg(b" mask=");
                it_u64(events as u64);
                it_msg(b" ready=");
                it_u64(ready as u64);
                it_msg(b" has=");
                it_u64(if device.has_event() { 1 } else { 0 });
                it_msg(b"\n");
            }
        }
    }
    if events & POLLOUT != 0 {
        // evdev is never writable.
    }
    ready
}

// ============================================================================
// Poll-wait registration (P3: event-driven poll/select)
// ============================================================================

/// Register `task` on the device's poll wait queue; push_event wakes it.
pub fn evdev_poll_wait(file: &File, task: *mut crate::process::types::Task) -> bool {
    // SAFETY: dispatcher identity-checked the File against EVDEV_OPS;
    // private_data holds a DevNo installed at open and the EVDEV_* statics
    // outlive every file.
    match device_of_file(file) {
        Some(device) => {
            device
                .poll_waitq
                .add(crate::process::types::WaitQueueEntry::new(task, false));
            // input:evdev_wake hunt: echo the park registration (names
            // WHO parks on the input nodes — the OH rig reads it off
            // serial). Same armed() gate as the verdict echo.
            if crate::trace::EVENT_EVDEV_WAKE.armed() {
                use core::sync::atomic::{AtomicU64, Ordering};
                static REG_N: AtomicU64 = AtomicU64::new(0);
                let n = REG_N.fetch_add(1, Ordering::Relaxed);
                if input_echo_budget(n) {
                    // SAFETY: task is the current task parking here (the
                    // wait-queue callback passes current's raw handle).
                    let pid = unsafe { (*task).pid() };
                    let mut l = HuntLine::new();
                    l.push(b"INPW[");
                    l.u64(n);
                    l.push(b"] reg pid=");
                    l.u64(pid as u64);
                    l.push(b" dev=");
                    for &c in device.name.iter().take_while(|&&c| c != 0).take(10) {
                        l.push(&[c]);
                    }
                    // f= correlates the registration with the INOP open
                    // line (which node this file really is).
                    l.push(b" f=");
                    l.u64(file.file_id);
                    l.push(b"\n");
                    l.flush();
                }
            }
            true
        }
        None => false,
    }
}

/// Remove a registration added by evdev_poll_wait.
pub fn evdev_poll_unwait(file: &File, task: *mut crate::process::types::Task) {
    // SAFETY: same contract as evdev_poll_wait.
    if let Some(device) = device_of_file(file) {
        let dropped = device.poll_waitq.remove_count(task);
        if crate::trace::EVENT_EVDEV_WAKE.armed() {
            use core::sync::atomic::{AtomicU64, Ordering};
            static UNREG_N: AtomicU64 = AtomicU64::new(0);
            let n = UNREG_N.fetch_add(1, Ordering::Relaxed);
            // d>1 is the pointer-recycling theft signature: this task
            // registered at most ONE entry on this queue per poll round,
            // so a remove dropping more says a stale entry from a DEAD
            // previous owner of the same Task address was stripped too
            // (exit leak + page recycle). With the R127 (ptr, pid) key a
            // live task's remove can no longer strip a predecessor's
            // entries, so d>1 now names either a dup-fd double
            // registration (legitimate) or a keying bug — print those
            // unconditionally (not just inside the budget).
            let steal = dropped > 1;
            if steal || input_echo_budget(n) {
                // SAFETY: task is the current task leaving the wait.
                let pid = unsafe { (*task).pid() };
                let mut l = HuntLine::new();
                l.push(b"INPW[");
                l.u64(n);
                l.push(b"] unreg pid=");
                l.u64(pid as u64);
                l.push(b" dev=");
                for &c in device.name.iter().take_while(|&&c| c != 0).take(10) {
                    l.push(&[c]);
                }
                l.push(b" d=");
                l.u64(dropped as u64);
                l.push(b" f=");
                l.u64(file.file_id);
                if steal {
                    l.push(b" MULTI");
                }
                l.push(b"\n");
                l.flush();
            }
        }
    }
}

/// Event type constants from <uapi/linux/input.h> needed by the write path.
const EVDEV_EV_SYN: u16 = 0x00;
const EV_LED: u16 = 0x11;

/// evdev write function
///
/// Linux semantics (drivers/input/evdev.c evdev_write): the buffer must be
/// a positive multiple of sizeof(struct input_event); each record is handed
/// to the input core, which silently drops events the device cannot take
/// and keeps LED/SND state for EVIOCGLED/EVIOCGSND. X servers write EV_LED
/// + SYN_REPORT packets here whenever core keyboard LED state changes —
/// returning an error (the old no-write-op EBADF) made every
/// xf86-input-evdev LED update log "(EE) Failed to set keyboard controls:
/// Bad file descriptor".
fn evdev_file_write(file: &File, buf: &[u8]) -> isize {
    // SAFETY: private_data contains a valid DevNo pointer set during device
    // open; the EVDEV_* statics are initialized by init_evdev() before any
    // file operations can occur.
    let device = match device_of_file(file) {
        Some(d) => d,
        None => return -9, // EBADF
    };

    let event_size = core::mem::size_of::<InputEvent>();
    if buf.is_empty() || buf.len() % event_size != 0 {
        return -22; // EINVAL
    }

    for chunk in buf.chunks(event_size) {
        // InputEvent is repr(C) with no padding: the byte view parses
        // in place.
        let (sec, rest) = chunk.split_at(8);
        let (usec, rest) = rest.split_at(8);
        let (tycode, val) = rest.split_at(4);
        let type_ = u16::from_ne_bytes([tycode[0], tycode[1]]);
        let code = u16::from_ne_bytes([tycode[2], tycode[3]]);
        let value = i32::from_ne_bytes([val[0], val[1], val[2], val[3]]);
        let _ = (sec, usec); // client-supplied timestamps are advisory

        if type_ == EV_LED && code < 32 {
            if value != 0 {
                device.led_state.fetch_or(1 << code, Ordering::AcqRel);
            } else {
                device.led_state.fetch_and(!(1 << code), Ordering::AcqRel);
            }
        }
        // Everything else (including the trailing EV_SYN/SYN_REPORT) needs
        // no action: there is no hardware to inject into, and Linux's input
        // core equally ignores events without a capable handler.
    }
    buf.len() as isize
}

/// evdev FileOps
pub static EVDEV_OPS: FileOps = FileOps::builder()
    .read(evdev_file_read)
    .write(evdev_file_write)
    .close(evdev_file_close)
    .poll(evdev_file_poll)
    .build();

// ============================================================================
// ioctl (per-file, dispatched from sys_ioctl via ops identity)
// ============================================================================

/// evdev ioctl entry — returns None when `file` is not an evdev node so the
/// generic ioctl path can continue.
///
/// Mirrors drivers/input/evdev.c: data-returning ioctls succeed with 0
/// (put_user/copy_to_user style), unknown requests get ENOTTY.
pub fn evdev_file_ioctl(file: &File, cmd: u32, arg: usize) -> Option<i64> {
    // Dispatch on the file's ops identity (pty_ioctl / is_fb_file pattern).
    if !file.get_ops().map_or(false, |ops| {
        core::ptr::eq(ops as *const _, &EVDEV_OPS as *const _)
    }) {
        return None;
    }

    // rux:inputtrace (retired debug=inputtrace): the capability probe — what we hand to the input
    // framework for classification. A device whose EVIOCGBIT(0) misses
    // EV_SYN/EV_KEY/EV_ABS, or whose name/id ioctls return garbage, is
    // rejected by consumers after open (opened-but-never-read is the
    // signature). Budgeted, first 30 ioctls per boot.
    if crate::trace::EVENT_INPUTTRACE.armed() {
        static IO_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = IO_N.fetch_add(1, Ordering::Relaxed);
        if n < 400 && (cmd >> 30) == 2 {
            // _IOR reads: EVIOCG* family
            let nm: alloc::vec::Vec<u8> = device_of_file(file)
                .map(|d| {
                    d.name
                        .iter()
                        .take_while(|&&c| c != 0)
                        .copied()
                        .collect()
                })
                .unwrap_or_default();
            crate::pr_warn!(
                "IN-IOCTL #{} pid={} name={} cmd={:#x}",
                n,
                crate::process::hotpath::current_pid(),
                alloc::string::String::from_utf8_lossy(&nm),
                cmd
            );
        }
    }

    // SAFETY: the EVDEV_* statics are initialized by init_evdev() before
    // any file operations can occur; device_of_file validates the DevNo.
    let device = match device_of_file(file) {
        Some(d) => d,
        None => return Some(-9), // EBADF
    };

    /// Copy `data` out honoring the length encoded in the ioctl command.
    /// Returns the ioctl return value.
    fn copy_out(arg: usize, cmd: u32, data: &[u8]) -> i64 {
        use crate::arch::uaccess::{access_ok, copy_to_user};
        let len = ioc_size(cmd).min(data.len());
        if len == 0 {
            return 0;
        }
        if !access_ok(arg, len) {
            return -14; // EFAULT
        }
        // SAFETY: arg validated with access_ok(len); data is a kernel buffer.
        if crate::arch::uaccess_safe::copy_out(arg, &data[..len]) > 0 {
            return -14; // EFAULT
        }
        0
    }

    let nr = ioc_nr(cmd);
    let dir = ioc_dir(cmd);

    let ret: i64 = match (nr, dir) {
        (NR_VERSION, IOC_READ) => copy_out(arg, cmd, &0x010001u32.to_ne_bytes()), // EV_VERSION
        (NR_ID, IOC_READ) => copy_out(
            arg,
            cmd,
            &{
                // SAFETY: InputId is repr(C), Copy; transmute-free byte view.
                let id = device.id;
                let mut b = [0u8; 8];
                b[0..2].copy_from_slice(&id.bustype.to_ne_bytes());
                b[2..4].copy_from_slice(&id.vendor.to_ne_bytes());
                b[4..6].copy_from_slice(&id.product.to_ne_bytes());
                b[6..8].copy_from_slice(&id.version.to_ne_bytes());
                b
            },
        ),
        (NR_NAME, IOC_READ) => {
            // NUL-terminated device name, truncated to the caller's buffer.
            let len = ioc_size(cmd);
            let name_len = device.name.iter().position(|&c| c == 0).unwrap_or(31) + 1;
            if len == 0 {
                0
            } else {
                copy_out(arg, cmd, &device.name[..name_len.min(32)])
            }
        }
        (NR_PHYS, IOC_READ) | (NR_UNIQ, IOC_READ) => {
            // No physical path / unique id on virtio-input: empty string.
            let len = ioc_size(cmd);
            if len == 0 {
                0
            } else {
                copy_out(arg, cmd, &[0u8])
            }
        }
        (NR_PROP, IOC_READ) => {
            // No INPUT_PROP_* flags.
            let len = ioc_size(cmd);
            if len == 0 {
                0
            } else {
                copy_out(arg, cmd, &[0u8; 32][..len.min(32)])
            }
        }
        (NR_REP_GET, IOC_READ) => {
            // Repeat settings {delay, period} in ms (kernel defaults).
            let rep = [250u32, 33u32];
            let mut b = [0u8; 8];
            b[0..4].copy_from_slice(&rep[0].to_ne_bytes());
            b[4..8].copy_from_slice(&rep[1].to_ne_bytes());
            copy_out(arg, cmd, &b)
        }
        (NR_REP_SET, IOC_WRITE) => 0,
        (NR_KEY_STATE, IOC_READ) | (NR_LED_STATE, IOC_READ) | (NR_SND_STATE, IOC_READ)
        | (NR_SW_STATE, IOC_READ) => {
            // Live state bitmap. LED bits track EV_LED writes (Linux
            // input-core semantics); key/sound/switch are not tracked and
            // report "all clear" — advisory, as on any kernel without
            // stateful drivers.
            let len = ioc_size(cmd);
            if len == 0 {
                0
            } else {
                let state = if nr == NR_LED_STATE {
                    device.led_state.load(Ordering::Acquire)
                } else {
                    0
                };
                let mut b = [0u8; 32];
                b[..4].copy_from_slice(&state.to_ne_bytes());
                copy_out(arg, cmd, &b[..len.min(32)])
            }
        }
        (NR_GRAB, IOC_WRITE) => {
            // arg is the grab flag passed by value (not a pointer).
            if arg != 0 {
                if device.grabbed.swap(true, Ordering::AcqRel) {
                    // Already grabbed by another client.
                    -16 // EBUSY
                } else {
                    0
                }
            } else {
                device.grabbed.store(false, Ordering::Release);
                0
            }
        }
        (NR_SCLOCKID, IOC_WRITE) => {
            // Accept the clock switch request; timestamps already come from
            // the monotonic CLINT timer.
            0
        }
        _ => {
            if (NR_BIT_BASE..NR_BIT_BASE + 0x20).contains(&nr) && dir == IOC_READ {
                // EVIOCGBIT(ev, len) for ev in 0..=0x1f (EV_MAX). Types we
                // carry bitmaps for answer from the virtio config; the rest
                // return an all-zero bitmap (success) like Linux — libevdev
                // probes every type and treats ENOTTY as an error.
                let ev = (nr - NR_BIT_BASE) as usize;
                if ev == 0 {
                    let types = evtype_bitmap(&device.ev_bits);
                    if crate::trace::EVENT_INPUTTRACE.armed() {
                        static TY_N: core::sync::atomic::AtomicU32 =
                            core::sync::atomic::AtomicU32::new(0);
                        let tn = TY_N.fetch_add(1, Ordering::Relaxed);
                        if tn < 12 {
                            let nm: alloc::vec::Vec<u8> = device
                                .name
                                .iter()
                                .take_while(|&&c| c != 0)
                                .copied()
                                .collect();
                            crate::pr_warn!(
                                "IN-TYPEBITS #{} pid={} name={} types={:#04x}",
                                tn,
                                crate::process::hotpath::current_pid(),
                                alloc::string::String::from_utf8_lossy(&nm),
                                types[0]
                            );
                        }
                    }
                    copy_out(arg, cmd, &types)
                } else if ev < BIT_TYPES {
                    copy_out(arg, cmd, &device.ev_bits[ev])
                } else {
                    let len = ioc_size(cmd);
                    if len == 0 {
                        0
                    } else {
                        copy_out(arg, cmd, &[0u8; 32][..len.min(32)])
                    }
                }
            } else if (NR_ABS_BASE..NR_ABS_BASE + 8).contains(&nr) && dir == IOC_READ {
                // EVIOCGABS(abs)
                let abs = (nr - NR_ABS_BASE) as usize;
                let slot = match abs {
                    0 | 1 => Some(abs),
                    0x35 => Some(2),
                    0x36 => Some(3),
                    _ => None,
                };
                let info = match slot {
                    Some(k) => device.absinfo[k],
                    None => InputAbsinfo::default(),
                };
                let mut b = [0u8; 24];
                b[0..4].copy_from_slice(&info.value.to_ne_bytes());
                b[4..8].copy_from_slice(&info.minimum.to_ne_bytes());
                b[8..12].copy_from_slice(&info.maximum.to_ne_bytes());
                b[12..16].copy_from_slice(&info.fuzz.to_ne_bytes());
                b[16..20].copy_from_slice(&info.flat.to_ne_bytes());
                b[20..24].copy_from_slice(&info.resolution.to_ne_bytes());
                copy_out(arg, cmd, &b)
            } else {
                -25 // ENOTTY
            }
        }
    };
    Some(ret)
}

// ============================================================================
// Initialization and registration
// ============================================================================

/// Format a capability bitmap the way Linux `input_print_bitmap()` prints
/// it into uevent env vars and /sys capabilities attrs: space-separated
/// lowercase-hex 64-bit words, most-significant word FIRST, leading zero
/// words skipped, always at least one word.
///
/// OH's libudev shim parses exactly this format (BitVector reads hex
/// u64 words high-word-first and reverses them), and libinput derives the
/// ID_INPUT_KEYBOARD / ID_INPUT_MOUSE / ... tags from those lines to
/// decide whether a device is usable at all.
pub fn format_input_bitmap(bits: &[u8]) -> String {
    fn word_at(bits: &[u8], w: usize) -> u64 {
        let mut word: u64 = 0;
        for i in 0..8 {
            let idx = w * 8 + i;
            if idx < bits.len() {
                word |= (bits[idx] as u64) << (8 * i);
            }
        }
        word
    }

    let words = (bits.len() + 7) / 8;
    let mut hi = words;
    while hi > 1 && word_at(bits, hi - 1) == 0 {
        hi -= 1;
    }

    use core::fmt::Write as _;
    let mut out = String::new();
    for w in (0..hi).rev() {
        if !out.is_empty() {
            out.push(' ');
        }
        let _ = write!(out, "{:x}", word_at(bits, w));
    }
    if out.is_empty() {
        out.push('0');
    }
    out
}

/// Build the sysfs registration (input_dev + event node pair) for one
/// evdev device from the same capability bitmaps EVIOCGBIT serves, so the
/// sysfs view and the ioctl view can never disagree.
fn sysfs_registration(
    input_name: &str,
    event_name: &str,
    devno: DevNo,
    device: &EvdevDevice,
) -> crate::fs::leaf_glue::InputEventReg {
    let name_len = device.name.iter().position(|&c| c == 0).unwrap_or(31);
    let dev_name = String::from(core::str::from_utf8(&device.name[..name_len]).unwrap_or("virtio input"));

    // EV word: which event types exist (bits 0..EV_MAX live in one word).
    let types = evtype_bitmap(&device.ev_bits);
    let ev = format_input_bitmap(&types[..8.min(types.len())]);

    // KEY/REL/ABS words only when the type is supported (Linux
    // input_dev_uevent emits the bitmap line only for set types).
    let type_bit = |t: usize| types[0] & (1u8 << (t & 7)) != 0;
    let key = if type_bit(EV_KEY as usize) {
        format_input_bitmap(&device.ev_bits[1])
    } else {
        String::new()
    };
    let rel = if type_bit(EV_REL as usize) {
        format_input_bitmap(&device.ev_bits[2])
    } else {
        String::new()
    };
    let abs = if type_bit(EV_ABS as usize) {
        format_input_bitmap(&device.ev_bits[3])
    } else {
        String::new()
    };

    crate::fs::leaf_glue::InputEventReg {
        input_name: String::from(input_name),
        event_name: String::from(event_name),
        major: devno.major,
        minor: devno.minor,
        dev_name,
        bustype: device.id.bustype as u32,
        vendor: device.id.vendor as u32,
        product: device.id.product as u32,
        version: device.id.version as u32,
        ev,
        key,
        rel,
        abs,
    }
}

/// Initialize evdev devices and register to devfs
pub fn init_evdev() {
    // Called once during kernel init (INV-SLOT-1/2: OnceSlot owns the
    // publication). Pull the capability bitmaps straight from the virtio
    // devices' config space (what a Linux guest would register), with the
    // previously-hardcoded bitmaps as fallback when the config read
    // yields nothing.
    EVDEV_KEYBOARD.publish_with_addr(|_| {
        let mut dev = EvdevDevice::new(b"VirtIO Keyboard", false);
        fill_capabilities(&mut dev, &INPUT_KEYBOARD, false);
        dev
    });
    EVDEV_POINTER.publish_with_addr(|_| {
        let mut dev = EvdevDevice::new(b"VirtIO Tablet", true);
        fill_capabilities(&mut dev, &INPUT_POINTER, true);
        dev
    });

    // Touchscreen device: same coordinates as the pointer, but with
    // touch protocol (BTN_TOUCH + ABS_MT_POSITION_X/Y). OH's
    // multimodalinput framework only recognizes touch input from
    // devices carrying these capabilities — the launcher's swipe
    // gesture needs it.
    EVDEV_TOUCHSCREEN.publish_with_addr(|_| {
        let mut dev = EvdevDevice::new(b"VirtIO Touchscreen", true);
        // KEY bitmap: BTN_TOUCH = 0x14a, BTN_LEFT = 0x110 (keep both
        // so single-touch and multi-touch consumers both bind).
        let mut key_bits = [0u8; 96];
        key_bits[0x14a / 8] |= 1 << (0x14a % 8); // BTN_TOUCH
        key_bits[0x110 / 8] |= 1 << (0x110 % 8); // BTN_LEFT
        dev.set_ev_bits(1, &key_bits); // EV_KEY
        // ABS bitmap: ABS_MT_POSITION_X=0x35, ABS_MT_POSITION_Y=0x36
        let mut abs_bits = [0u8; 96];
        abs_bits[0x35 / 8] |= 1 << (0x35 % 8); // ABS_MT_POSITION_X
        abs_bits[0x36 / 8] |= 1 << (0x36 % 8); // ABS_MT_POSITION_Y
        abs_bits[0x00 / 8] |= 1 << (0x00 % 8); // ABS_X
        abs_bits[0x01 / 8] |= 1 << (0x01 % 8); // ABS_Y
        dev.set_ev_bits(3, &abs_bits); // EV_ABS
        // Copy axis ranges from the pointer device. The MT axes
        // (0x35/0x36) MUST carry the same range as ABS_X/Y: consumers
        // (OH multimodalinput) read each axis's min/max via EVIOCGABS
        // to map events into screen space — an MT axis with no absinfo
        // reads as min=max=0 and every MT position clamps to (0,0),
        // killing the swipe gesture even though the events flow.
        dev.set_absinfo(0, 0, 32767, 0, 0, 0);  // ABS_X
        dev.set_absinfo(1, 0, 32767, 0, 0, 0);  // ABS_Y
        dev.set_absinfo(0x35, 0, 32767, 0, 0, 0); // ABS_MT_POSITION_X
        dev.set_absinfo(0x36, 0, 32767, 0, 0, 0); // ABS_MT_POSITION_Y
        dev
    });

    // Register device operations
    crate::fs::leaf_glue::devfs::register_char_device(DEV_EVDEV_KEYBOARD, &EVDEV_OPS)
        .expect("Failed to register keyboard evdev");
    crate::fs::leaf_glue::devfs::register_char_device(DEV_EVDEV_POINTER, &EVDEV_OPS)
        .expect("Failed to register pointer evdev");
    crate::fs::leaf_glue::devfs::register_char_device(
        crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN,
        &EVDEV_OPS,
    )
    .expect("Failed to register touchscreen evdev");

    // Create device nodes
    crate::fs::leaf_glue::devfs::mknod("/input/event0", DEV_EVDEV_KEYBOARD, 0o666)
        .expect("Failed to create /dev/input/event0");
    crate::fs::leaf_glue::devfs::mknod("/input/event1", DEV_EVDEV_POINTER, 0o666)
        .expect("Failed to create /dev/input/event1");
    crate::fs::leaf_glue::devfs::mknod("/input/event2", crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN, 0o666)
        .expect("Failed to create /dev/input/event2");

    // U3: registration-time sysfs entries + "add" uevents. The
    // two-kobject layout (inputN/eventM) mirrors Linux input_register_dev:
    // the inputN uevent carries PRODUCT/NAME/EV/KEY/... which OH's libudev
    // shim and libinput need to tag the device ID_INPUT_KEYBOARD etc.;
    // DEVNAME is "input/event0" — the /dev-relative node path.
    let kbd_reg = EVDEV_KEYBOARD
        .get()
        .map(|d| sysfs_registration("input0", "event0", DEV_EVDEV_KEYBOARD, d));
    let ptr_reg = EVDEV_POINTER
        .get()
        .map(|d| sysfs_registration("input1", "event1", DEV_EVDEV_POINTER, d));
    if let Some(reg) = kbd_reg {
        crate::fs::leaf_glue::sysfs::register_input_event(&reg);
    }
    if let Some(reg) = ptr_reg {
        crate::fs::leaf_glue::sysfs::register_input_event(&reg);
    }
    // The touchscreen needs the same sysfs presence: OH's input stack
    // enumerates devices through /sys/class/input (ID_INPUT_TOUCHSCREEN +
    // the uevent), so a touchscreen with a /dev node but no sysfs entry
    // is invisible to multimodalinput — the swipe gesture never starts.
    let ts_reg = EVDEV_TOUCHSCREEN
        .get()
        .map(|d| sysfs_registration("input2", "event2", crate::fs::dev_t::DEV_EVDEV_TOUCHSCREEN, d));
    if let Some(reg) = ts_reg {
        crate::fs::leaf_glue::sysfs::register_input_event(&reg);
    }
}

/// Fill an evdev device's capability bitmaps from the backing virtio
/// device (locks the INPUT_* static and reads its config space).
fn fill_capabilities(
    dev: &mut EvdevDevice,
    virt_dev: &Spinlock<Option<VirtioInputDevice>>,
    is_pointer: bool,
) {
    let mut bits = [[0u8; BITS_LEN]; BIT_TYPES];
    let mut have_any = false;
    {
        let mut guard = virt_dev.lock();
        if let Some(ref mut vd) = *guard {
            for ev in 0..BIT_TYPES {
                if vd.read_ev_bitmap(ev as u8, &mut bits[ev]) > 0 {
                    have_any = true;
                }
            }
            // Absolute axes: read ABS_INFO for X/Y.
            if let Some([min, max, fuzz, flat, res]) = vd.read_absinfo(ABS_X as u8) {
                dev.set_absinfo(0, min as i32, max as i32, fuzz as i32, flat as i32, res as i32);
            }
            if let Some([min, max, fuzz, flat, res]) = vd.read_absinfo(ABS_Y as u8) {
                dev.set_absinfo(1, min as i32, max as i32, fuzz as i32, flat as i32, res as i32);
            }
        }
    }

    if !have_any {
        // Config space unreadable: report what the driver actually emits.
        bits[0][0] = 1; // EV_SYN/SYN_REPORT
        if is_pointer {
            // BTN_LEFT/RIGHT/MIDDLE = 0x110..0x112
            bits[1][0x110 / 8] = 0b0000_0111;
            bits[2][0] = 0x03; // REL_X | REL_Y
            bits[2][1] = 0x01; // REL_WHEEL (0x08)
            bits[3][0] = 0x03; // ABS_X | ABS_Y
            dev.set_absinfo(0, 0, 32767, 0, 0, 0);
            dev.set_absinfo(1, 0, 32767, 0, 0, 0);
        } else {
            // Standard keyboard keys 0x01..=0x58.
            for code in 1u16..=0x58 {
                bits[1][(code / 8) as usize] |= 1 << (code % 8);
            }
        }
    }

    for (ev, b) in bits.iter().enumerate() {
        dev.set_ev_bits(ev, b);
    }
}

/// Push event to evdev device
pub fn push_input_event(is_pointer: bool, event: InputEvent) {
    // input:input_event — hop 1 of the input chain: the event crossed
    // from the virtio virtqueue into the kernel input core. The armed()
    // check inside the generated call is the one relaxed load this adds
    // when the event is off; the serial echo re-checks armed() so it
    // rides the same enable path (trace_event=).
    if crate::trace::EVENT_INPUT_EVENT.armed() {
        crate::trace::trace_input_event(
            if is_pointer { "ptr" } else { "kbd" },
            event.type_,
            event.code,
            event.value,
        );
        use core::sync::atomic::{AtomicU64, Ordering};
        static ECHO_N: AtomicU64 = AtomicU64::new(0);
        let n = ECHO_N.fetch_add(1, Ordering::Relaxed);
        if input_echo_budget(n) {
            itt_msg(b"INEV[");
            itt_u64(n);
            itt_msg(b"] dev=");
            itt_msg(if is_pointer { b"ptr" } else { b"kbd" });
            itt_msg(b" type=");
            itt_u64(event.type_ as u64);
            itt_msg(b" code=");
            itt_u64(event.code as u64);
            itt_msg(b" value=");
            itt_i64(event.value as i64);
            itt_msg(b"\n");
        }
    }

    if crate::trace::EVENT_INPUTTRACE.armed() {
        static PN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let pn = PN.fetch_add(1, Ordering::Relaxed);
        if pn < 10 {
            crate::pr_warn!("IN-PUSH enter #{} is_ptr={}", pn, is_pointer);
        }
    }

    // rux:inputtrace (retired debug=inputtrace): payload trace to hunt phantom event streams. Prints
    // every event with a monotonically increasing ordinal so serial-log
    // frequency analysis works; rate-limited so a flooding stream cannot
    // wedge the serial console; gated by a runtime switch (default OFF).
    // Writes go through the raw console path (putchar_no_lock), NOT
    // pr_info!: session boots run loglevel=3, and printk's pre-ring
    // filter drops KERN_INFO whole — the first deployment of this trace
    // (leg-6 r6win/r8win runs, kernel with b3af8cfc) silently produced
    // zero INPTRC lines for exactly that reason. Raw bytes are the same
    // convention as sched/wakewatch's ww_msg. EV_SYN (type 0) is the
    // high-rate filler: first 200 then 1-in-1000. Payload events
    // (type != 0) carry the actual signal: first 500 then 1-in-100,
    // enough to survive a phantom flood while keeping full detail in
    // the common case (a quiet -display none boot has no events at
    // all, so anything printed here IS the anomaly).
    if crate::trace::EVENT_INPUTTRACE.armed() {
        use core::sync::atomic::{AtomicU64, Ordering};
        static TRACE_SEQ: AtomicU64 = AtomicU64::new(0);
        static TRACE_SYN_SEQ: AtomicU64 = AtomicU64::new(0);
        let is_syn = event.type_ == 0;
        let print = if is_syn {
            let n = TRACE_SYN_SEQ.fetch_add(1, Ordering::Relaxed);
            n < 200 || n % 1000 == 0
        } else {
            let n = TRACE_SEQ.fetch_add(1, Ordering::Relaxed);
            n < 500 || n % 100 == 0
        };
        if print {
            let tag = if is_syn { "syn" } else if is_pointer { "ptr" } else { "kbd" };
            let n = if is_syn {
                TRACE_SYN_SEQ.load(Ordering::Relaxed)
            } else {
                TRACE_SEQ.load(Ordering::Relaxed)
            };
            // Hand-rolled formatting on the raw console path (same shape
            // as sched/wakewatch's ww_msg/ww_u64): this can fire from the
            // virtio-input IRQ context and must not touch printk locks.
            it_msg(b"INPTRC[");
            it_u64(n);
            it_msg(b"] dev=");
            it_msg(match tag {
                "syn" => b"syn",
                "ptr" => b"ptr",
                _ => b"kbd",
            });
            it_msg(b" type=");
            it_u64(event.type_ as u64);
            it_msg(b" code=");
            it_u64(event.code as u64);
            it_msg(b" value=");
            it_i64(event.value as i64);
            it_msg(b"\n");
        }
    }
    // Keyboard events feed the Ctrl-Alt-Del detector (SA_CAD semantics):
    // track Ctrl/Alt state and latch on a Delete press with both held.
    // Called from the evdev read/poll path — task context — but latching
    // only (atomic stores) keeps it safe from any context; execution
    // happens in cad_deliver_pending() on the next return to user.
    if !is_pointer {
        cad_detect(event);
    }
    // (Registry access through the audited accessors; event delivery
    // serializes through each device's internal lock.)
    if is_pointer {
        if let Some(dev) = evdev_pointer() {
            dev.push_event(event);
        }
        // Mirror to the touchscreen: translate pointer events to
        // touch protocol (OH multimodalinput). BTN_LEFT(0x110) ->
        // BTN_TOUCH(0x14a); ABS_X/Y -> also ABS_MT_POSITION_X/Y.
        if let Some(ts) = evdev_touchscreen() {
            let ev_type = event.type_;
            let ev_code = event.code;
            let mut ts_event = event;
            if ev_type == 1 && ev_code == 0x110 {
                // EV_KEY / BTN_LEFT -> BTN_TOUCH
                ts_event.code = 0x14a;
            } else if ev_type == 3 && (ev_code == 0x00 || ev_code == 0x01) {
                // EV_ABS / ABS_X(0) or ABS_Y(1) -> also emit
                // ABS_MT_POSITION_X(0x35) or ABS_MT_POSITION_Y(0x36)
                ts_event.code = if ev_code == 0x00 { 0x35 } else { 0x36 };
            }
            ts.push_event(ts_event);
        }
    } else {
        if let Some(dev) = evdev_keyboard() {
            dev.push_event(event);
        }
    }
}

// ============================================================================
// input:* tracepoint serial echo (OH input-chain hunts)
// ============================================================================
//
// The OH image gives the hunts no in-guest way to read tracefs (no
// shell on the serial console, debugfs not mounted), so the per-hop
// verdict is read off the serial log. When the input tracepoints are
// armed — through the reference's `trace_event=` boot parameter, the
// unified-framework enable path (not a legacy switch) — each traced hop
// ALSO echoes one budgeted line through uart_putc_raw (the raw UART
// path that is deliberately not compiled out of release builds; safe
// from the softirq tasklet context push_input_event runs in). Off by
// default exactly like the tracepoints themselves. Hand-rolled number
// formatting, no allocation: these fire from the input tasklet BH.

/// Serial-echo budget per class: first 300 lines in full, then every
/// 25th (a phantom event flood must not wedge the console; a quiet
/// injection window prints every line).
fn input_echo_budget(n: u64) -> bool {
    n < 300 || n % 25 == 0
}

/// Line-staging buffer for the hunt echoes (R127 wake-drop round): the
/// per-byte uart_putc_raw writes interleaved CHAR-BY-CHAR when two CPUs
/// echoed concurrently (r3's "IINPWCK_[11_7665]" mangling — two whole
/// lines lost), so the verdict lines are now staged into a stack buffer
/// and flushed as ONE critical section under a dedicated echo lock.
/// irqsave: the wake-side echoes fire from the input tasklet (BH).
static HUNT_ECHO_LOCK: crate::sync::types::Spinlock<()> =
    crate::sync::types::Spinlock::new(());


impl HuntLine {
    pub const fn new() -> Self {
        Self { buf: [0; 192], n: 0 }
    }
    pub fn push(&mut self, s: &[u8]) {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
    }
    pub fn u64(&mut self, v: u64) {
        let mut tmp = [0u8; 20];
        let mut k = 0usize;
        let mut x = v;
        if x == 0 {
            tmp[k] = b'0';
            k = 1;
        } else {
            while x > 0 && k < tmp.len() {
                tmp[k] = b'0' + (x % 10) as u8;
                x /= 10;
                k += 1;
            }
        }
        for i in (0..k).rev() {
            self.push(&[tmp[i]]);
        }
    }
    pub fn flush(&mut self) {
        if self.n == 0 {
            return;
        }
        let _g = HUNT_ECHO_LOCK.lock_irqsave();
        // obs-hygiene: the burst rides the taskdump funnel's emit-lock
        // coordination (one atomic write vs printk records too — the
        // kmsg-flood splice class), not the bare per-byte raw writes.
        crate::debug::taskdump::line_atomic_write(&self.buf[..self.n]);
        self.n = 0;
    }
}


static ITT_STAGES: [crate::sync::types::Spinlock<IttStage>; crate::config::MAX_CPUS] =
    [const {
        crate::sync::types::Spinlock::new(IttStage { buf: [0; 128], n: 0 })
    }; crate::config::MAX_CPUS];

fn itt_msg(msg: &[u8]) {
    let cpu = (crate::arch::cpu_id() as usize) % crate::config::MAX_CPUS;
    for &b in msg {
        let mut st = ITT_STAGES[cpu].lock_irqsave();
        let at = st.n;
        st.buf[at] = b;
        st.n = at + 1;
        if b == b'\n' || st.n == st.buf.len() {
            crate::debug::taskdump::line_atomic_write(&st.buf[..st.n]);
            st.n = 0;
        }
    }
}

fn itt_u64(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        buf[0] = b'0';
        n = 1;
    } else {
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    for i in (0..n).rev() {
        itt_msg(&[buf[i]]);
    }
}

fn itt_i64(v: i64) {
    if v < 0 {
        itt_msg(b"-");
        itt_u64(v.unsigned_abs());
    } else {
        itt_u64(v as u64);
    }
}

/// The current task's (tid, tgid) for the echo lines — the boot-5 hunt
/// spent a whole taskdump roundtrip to learn that the evdev opener
/// (pid 618) was audio_server's thread inside tgid 408; carrying the
/// tgid in the echo names the process at read/open time directly.
fn current_pid_tgid() -> (u64, u64) {
    match crate::sched::hotpath::current() {
        // SAFETY: cur is the running task; pid/tgid are const reads.
        Some(cur) => (cur.pid() as u64, cur.tgid() as u64),
        None => (0, 0),
    }
}

// Raw-console writers for the rux:inputtrace (retired debug=inputtrace) path (same shape as
// sched/wakewatch's ww_msg/ww_u64): printk is unusable here because the
// trace must survive loglevel=3 boots, and the callers can be in IRQ
// context, so no printk locks.
fn it_msg(msg: &[u8]) {
    if !cfg!(debug_assertions) { return; } // release: diag output off

    for &b in msg {
        crate::console::putchar_no_lock(b);
    }
}

fn it_u64(mut v: u64) {
    if !cfg!(debug_assertions) { return; } // release: diag output off

    let mut buf = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        buf[0] = b'0';
        n = 1;
    } else {
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    for i in (0..n).rev() {
        crate::console::putchar_no_lock(buf[i]);
    }
}

fn it_i64(v: i64) {
    if v < 0 {
        it_msg(b"-");
        it_u64(v.unsigned_abs());
    } else {
        it_u64(v as u64);
    }
}

// ============================================================================
// Ctrl-Alt-Del detection (virtio-keyboard)
// ============================================================================

/// Modifier state for the CAD detector (bit flags, IRQ-safe atomics not
/// needed: push_input_event callers hold no shared state and the worst
/// race is a missed/extra detection on a simultaneous key event).
static CAD_MOD_CTRL: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static CAD_MOD_ALT: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Track Ctrl/Alt and latch a Ctrl-Alt-Del: a Delete PRESS while at least
/// one Ctrl and one Alt are down (left or right either). Value semantics
/// per input.h: 0 = release, 1 = press, 2 = autorepeat (repeat counts —
/// a held Delete keeps telling the user wants out).
fn cad_detect(event: InputEvent) {
    if event.type_ != EV_KEY {
        return;
    }
    match event.code {
        KEY_LEFTCTRL | KEY_RIGHTCTRL => {
            CAD_MOD_CTRL.store(event.value != 0, core::sync::atomic::Ordering::Release);
        }
        KEY_LEFTALT | KEY_RIGHTALT => {
            CAD_MOD_ALT.store(event.value != 0, core::sync::atomic::Ordering::Release);
        }
        KEY_DELETE if event.value != 0 => {
            if CAD_MOD_CTRL.load(core::sync::atomic::Ordering::Acquire)
                && CAD_MOD_ALT.load(core::sync::atomic::Ordering::Acquire)
            {
                crate::api::process::ctrl_alt_del_latch();
            }
        }
        _ => {}
    }
}

/// Poll VirtIO input devices
///
/// irqsave on both device locks: the input tasklet BH drains the same
/// queues from softirq context, and a task-context holder interrupted by
/// the device IRQ must not deadlock against it.
fn poll_virtio_events() {


    // Poll keyboard
    if let Some(ref mut kb) = *INPUT_KEYBOARD.lock_irqsave() {
        while kb.has_event() {
            if let Some(event) = kb.read_event() {
                push_input_event(false, event);
            }
        }
    }

    // Poll pointer device
    if let Some(ref mut ptr) = *INPUT_POINTER.lock_irqsave() {
        while ptr.has_event() {
            if let Some(event) = ptr.read_event() {
                push_input_event(true, event);
            }
        }
    }
}

// ════════════════ loop_dev ════════════════════════════
// (was loop_dev.rs)
// Loop block devices (Linux drivers/block/loop): a file-backed block
// device. `losetup`/LTP's tst_acquire_device bind a preallocated image
// file to /dev/loopN with LOOP_SET_FD; block reads/writes on the loop
// are routed to the backing file at (sector * 512 + lo_offset), capped
// by lo_sizelimit. Every loop registers a GenDisk (major 7) whose
// request_fn performs the file I/O — the buffer cache and page cache
// key on the GenDisk pointer, so the loop gets the same caching
// discipline as a real disk for free.
//
// /dev/loop-control answers LOOP_CTL_GET_FREE with the first unbound
// minor (Linux loop control semantics).
/// Number of loop devices (Linux's historical default).
pub const N_LOOPS: usize = 8;

/// loop-control minor on the misc major.
pub const LOOP_CONTROL_MINOR: u32 = 237;

// ---- loop ioctl numbers (linux/loop.h) ----
pub const LOOP_SET_FD: u32 = 0x4C00;
pub const LOOP_CLR_FD: u32 = 0x4C01;
/// BLKGETSIZE64 (bytes) — mke2fs/tst_device size probe.
pub const BLKGETSIZE64: u32 = 0x8008_1272;
/// BLKSSZGET (logical sector size).
pub const BLKSSZGET: u32 = 0x1268;
pub const LOOP_SET_STATUS: u32 = 0x4C02;
pub const LOOP_GET_STATUS: u32 = 0x4C03;
pub const LOOP_SET_STATUS64: u32 = 0x4C04;
pub const LOOP_GET_STATUS64: u32 = 0x4C05;
pub const LOOP_CHANGE_FD: u32 = 0x4C06;
pub const LOOP_SET_CAPACITY: u32 = 0x4C07;
pub const LOOP_SET_DIRECT_IO: u32 = 0x4C08;
pub const LOOP_SET_BLOCK_SIZE: u32 = 0x4C09;
pub const LOOP_CONFIGURE: u32 = 0x4C0A;
pub const LOOP_CTL_ADD: u32 = 0x4C80;
pub const LOOP_CTL_REMOVE: u32 = 0x4C81;
pub const LOOP_CTL_GET_FREE: u32 = 0x4C82;

/// lo_flags bits (linux/loop.h).
pub const LO_FLAGS_READ_ONLY: u32 = 1;
pub const LO_FLAGS_AUTOCLEAR: u32 = 4;
pub const LO_FLAGS_PARTSCAN: u32 = 8;
pub const LO_FLAGS_DIRECT_IO: u32 = 16;


impl LoopDev {
    const fn new() -> Self {
        Self {
            file: Spinlock::new(None),
            offset: AtomicU64::new(0),
            sizelimit: AtomicU64::new(0),
            flags: AtomicU64::new(0),
            name: Spinlock::new([0; 64]),
            io_lock: crate::sync::types::SleepMutex::new_rt("loop-io", false),
            io_owner_pid: AtomicU32::new(0),
            disk: Spinlock::new(None),
        }
    }
}

static LOOPS: [LoopDev; N_LOOPS] = [
    LoopDev::new(), LoopDev::new(), LoopDev::new(), LoopDev::new(),
    LoopDev::new(), LoopDev::new(), LoopDev::new(), LoopDev::new(),
];

static LOOPS_INIT: AtomicBool = AtomicBool::new(false);

static LOOP_NAMES: [&str; N_LOOPS] = [
    "loop0", "loop1", "loop2", "loop3", "loop4", "loop5", "loop6", "loop7",
];

/// Device numbers served by this driver.
pub fn is_loop_devno(devno: DevNo) -> bool {
    (devno.major == LOOP_MAJOR && devno.minor < N_LOOPS as u32)
        || (devno.major == MISC_MAJOR && devno.minor == LOOP_CONTROL_MINOR)
}

/// Register the loop GenDisks with the block layer and /proc/partitions.
/// Idempotent; called from devfs init and mount.
pub fn init_loops() {
    if LOOPS_INIT.swap(true, Ordering::AcqRel) {
        return;
    }
    for i in 0..N_LOOPS {
        {
            let dev = &LOOPS[i];
            // The leaked GenDisk lives for the machine's lifetime (blkdev's
            // registry stores it; the buffer cache keys on its address).
            let disk = Box::leak(Box::new(GenDisk::new(
                loop_static_name(i),
                LOOP_MAJOR,
                N_LOOPS as u32,
                4096,
                None,
            )));
            disk.first_minor = i as u32;
            disk.minors = 1;
            disk.capacity.store(0, Ordering::Release);
            disk.request_fn = Some(loop_request_fn);
            *dev.disk.lock() = Some(disk);
            // The registry deduplicates by major; one representative row
            // exposes the loop family to /proc/partitions consumers.
            if i == 0 {
                let _ = crate::drivers::hotpath::register_disk(
                    alloc::boxed::Box::new(GenDisk {
                        name: loop_static_name(i),
                        major: LOOP_MAJOR,
                        first_minor: 0,
                        minors: N_LOOPS as u32,
                        capacity: core::sync::atomic::AtomicU64::new(0),
                        block_size: 4096,
                        ops: None,
                        private_data: None,
                        request_fn: Some(loop_request_fn),
                        async_read_fn: None,
                        async_write_fn: None,
                        async_read_sg_fn: None,
                        async_write_sg_fn: None,
                    }),
                );
            }
        }
    }
}

/// The canonical GenDisk for loop `idx` (the pointer ext4 mounts on).
pub fn loop_disk(idx: usize) -> Option<&'static GenDisk> {
    if idx >= N_LOOPS {
        return None;
    }
    init_loops();
    *LOOPS[idx].disk.lock()
}

fn loop_static_name(i: usize) -> &'static str {
    LOOP_NAMES[i]
}

/// LOOP_CTL_GET_FREE: first minor with no backing file.
pub fn loop_get_free() -> i32 {
    init_loops();
    for i in 0..N_LOOPS {
        let bound = LOOPS[i].file.lock().is_some();
        if !bound {
            return i as i32;
        }
    }
    -1 // ENXIO-equivalent: none free
}

/// LOOP_SET_FD: bind `file` as the backing store of loop `idx`.
/// Returns 0 or a negative errno.
pub fn loop_set_fd(idx: usize, file: Arc<File>) -> Result<(), i32> {
    if idx >= N_LOOPS {
        return return_errno(crate::errno::Errno::InvalidArgument);
    }
    init_loops();
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = &LOOPS[idx];
    let mut slot = dev.file.lock();
    if slot.is_some() {
        return return_errno(crate::errno::Errno::DeviceOrResourceBusy);
    }
    let size = file_size_bytes(&file)? as u64;
    if size == 0 {
        return return_errno(crate::errno::Errno::InvalidArgument);
    }
    *slot = Some(file);
    dev.offset.store(0, Ordering::Release);
    dev.sizelimit.store(0, Ordering::Release);
    dev.flags.store(0, Ordering::Release);
    *dev.name.lock() = [0; 64];
    if let Some(disk) = *dev.disk.lock() {
        disk.capacity.store(size / 512, Ordering::Release);
        // A re-bound loop is a NEW device image (mkfs follows): drop any
        // reused ext4 instance for this disk so the next mount reads the
        // device, never the stale in-memory superblock (see
        // ext4::mount_loop_instance's reuse block).
        crate::api::ext4::loop_invalidate(disk);
    }
    Ok(())
}

/// LOOP_SET_FD with the caller's backing path recorded for
/// LOOP_GET_STATUS(64).lo_file_name (util-linux `losetup` display).
pub fn loop_set_fd_named(idx: usize, file: Arc<File>, path: &[u8]) -> Result<(), i32> {
    loop_set_fd(idx, file)?;
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = &LOOPS[idx];
    let mut name = dev.name.lock();
    let n = path.len().min(63);
    name[..n].copy_from_slice(&path[..n]);
    name[n] = 0;
    Ok(())
}

/// LOOP_CLR_FD: detach the backing file.
pub fn loop_clr_fd(idx: usize) -> Result<(), i32> {
    if idx >= N_LOOPS {
        return return_errno(crate::errno::Errno::InvalidArgument);
    }
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = &LOOPS[idx];
    let mut slot = dev.file.lock();
    match slot.take() {
        Some(_) => {
            dev.flags.store(0, Ordering::Release);
            *dev.name.lock() = [0; 64];
            if let Some(disk) = *dev.disk.lock() {
                disk.capacity.store(0, Ordering::Release);
                // The device image is going away: a reused ext4 instance
                // must not outlive its disk (see loop_set_fd).
                crate::api::ext4::loop_invalidate(disk);
                // Drop every cached buffer of this loop, dirty ones
                // included (Linux invalidate_bdev on detach). A dirty
                // buffer whose writeback failed cannot be synced against
                // an unbound loop — leaving it in the shared cache let
                // the NEXT full-cache sync fail with this device's ENXIO
                // and poison unrelated syscalls (the mkdtemp ENXIO storm
                // after a failed mkfs-on-loop).
                crate::fs::bio::invalidate_device(disk);
            }
            Ok(())
        }
        // Linux loop_clr_fd: `lo->lo_state != Lo_bound` => ENXIO. LTP's
        // tst_detach_device loops LOOP_CLR_FD until it sees ENXIO —
        // answering EINVAL there made every device test end with
        // "ioctl(LOOP_CLR_FD) unexpectedly failed with: EINVAL" (TWARN,
        // r10: ~30 tests).
        None => return_errno(crate::errno::Errno::NoSuchDeviceOrAddress),
    }
}

/// Exposed byte length (sizelimit or file size, minus offset).
pub fn loop_size_bytes(idx: usize) -> u64 {
    if idx >= N_LOOPS {
        return 0;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = &LOOPS[idx];
    let guard = dev.file.lock();
    let Some(file) = guard.as_ref() else { return 0 };
    let total = file_size_bytes(file).unwrap_or(0) as u64;
    let off = dev.offset.load(Ordering::Acquire);
    let limit = dev.sizelimit.load(Ordering::Acquire);
    let avail = total.saturating_sub(off);
    if limit != 0 { limit.min(avail) } else { avail }
}

/// Is loop `idx` currently bound?
pub fn loop_is_bound(idx: usize) -> bool {
    if idx >= N_LOOPS {
        return false;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    LOOPS[idx].file.lock().is_some()
}

/// falloc-wedge (2026-11-16): does the CURRENT task hold the io claim of
/// any loop device — i.e. is it inside `loop_request_fn`'s backing-file
/// I/O right now? Reclaim's synchronous dirty writeback consults this
/// before issuing block I/O: a writeback that targets a filesystem
/// stacked on the SAME loop device would re-enter `loop_request_fn`
/// with the claim already held (the EDEADLK arm) — the reference kernel
/// never runs such writeback in the I/O submitter's context (loop I/O
/// is dispatched from the loop's own worker). Callers degrade to
/// clean-only reclaim instead, exactly like the WRITEBACK_GUARD nested
/// reclaimer does. N_LOOPS is small and the check is only paid on the
/// reclaim slow path.
pub fn loop_io_claim_held_by_current() -> bool {
    let me = crate::process::hotpath::current_pid() as u32;
    if me == 0 {
        return false;
    }
    for i in 0..N_LOOPS {
        // SAFETY: static array access bounded by N_LOOPS.
        if LOOPS[i].io_owner_pid.load(Ordering::Acquire) == me {
            return true;
        }
    }
    false
}

/// File I/O size: stat through the inode size (regular files).
fn file_size_bytes(file: &Arc<File>) -> Result<usize, i32> {
    match file.get_inode() {
        Some(inode) => Ok(inode.size.load(Ordering::Acquire) as usize),
        None => Ok(0),
    }
}

fn return_errno(e: crate::errno::Errno) -> Result<(), i32> {
    Err(e.as_neg_i32())
}

/// Block-layer request function shared by all loop GenDisks: translate
/// the request into a positioned read/write on the backing file.
/// (INV-BLKDEV-2: registered as `request_fn` on every loop GenDisk.)
fn loop_request_fn(req: &mut Request) {
    let idx = req.device.first_minor as usize;
    if idx >= N_LOOPS {
        req.error.store(-6, Ordering::Release); // ENXIO
        return;
    }
    let dev = &LOOPS[idx];
    // Re-entrancy guard (see io_owner_pid): claim the device for this
    // pid BEFORE waiting on io_lock; a recursive request (this pid
    // already inside) fails with EDEADLK instead of self-spinning.
    // Error 35 (-EDEADLK): the caller's write is failed, its pages stay
    // dirty, and the flush retries outside the loop-write context.
    {
        let me = crate::process::hotpath::current_pid() as u32;
        match dev.io_owner_pid.compare_exchange(
            0,
            me,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {}
            Err(owner) if owner == me => {
                static RECUR_REPORTS: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = RECUR_REPORTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 8 {
                    crate::pr_err!(
                        "loop{}: recursive request while pid {} owns io_lock — failing request (reclaim writeback re-entry)",
                        idx,
                        me
                    );
                    // falloc-wedge capture: symbolize the re-entry chain
                    // (the first two occurrences carry the kernel backtrace
                    // — error-path diagnostic, bounded like the print).
                    if n < 2 {
                        crate::debug::backtrace::dump_stack();
                    }
                }
                req.error.store(-35, Ordering::Release); // -EDEADLK
                return;
            }
            Err(_) => {}
        }
    }
    // Claimed — every exit path below must release the guard. The
    // release is a CAS(owner==me → 0): a task that arrived while another
    // owned the claim (its CAS above failed, it only WAITED on io_lock)
    // must not clear a claim it never took.
    loop_request_fn_claimed(req, dev, idx);
    let _ = dev.io_owner_pid.compare_exchange(
        crate::process::hotpath::current_pid() as u32,
        0,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

/// Body of [`loop_request_fn`] with the io_owner_pid re-entrancy claim
/// held; the wrapper releases it on return.
fn loop_request_fn_claimed(req: &mut Request, dev: &'static LoopDev, idx: usize) {
    let _io = dev.io_lock.guard();
    let guard = dev.file.lock();
    let Some(file) = guard.as_ref() else {
        req.error.store(-6, Ordering::Release); // ENXIO: unbound loop
        return;
    };
    let off = dev.offset.load(Ordering::Acquire);
    let limit = dev.sizelimit.load(Ordering::Acquire);
    let byte_start = req.sector
        .saturating_mul(512)
        .saturating_add(off)
        .min(if limit != 0 { off.saturating_add(limit) } else { u64::MAX });
    let ops = match file.get_ops() {
        Some(o) => o,
        None => {
            req.error.store(-5, Ordering::Release); // EIO
            return;
        }
    };
    let saved_pos = file.get_pos();
    file.set_pos(byte_start);
    let rc = match req.cmd_type {
        ReqCmd::Read => match ops.read {
            Some(f) => f(file, &mut req.buffer),
            None => {
                req.error.store(-5, Ordering::Release);
                return;
            }
        },
        ReqCmd::Write => match ops.write {
            Some(f) => f(file, &req.buffer),
            None => {
                req.error.store(-5, Ordering::Release);
                return;
            }
        },
        _ => 0,
    };
    file.set_pos(saved_pos);
    if rc < 0 {
        req.error.store(rc as i32, Ordering::Release);
    } else if req.cmd_type == ReqCmd::Read && (rc as usize) < req.buffer.len() {
        // Short read (past backing EOF): zero-fill the tail — a block
        // device is always block_size wide.
        for b in req.buffer[rc as usize..].iter_mut() {
            *b = 0;
        }
    }
}

// ============================================================================
// Device-node read/write (mkfs/blkid open /dev/loopN directly)
// ============================================================================

fn loop_devno_of(file: &crate::fs::file::File) -> Option<DevNo> {
    if !crate::fs::leaf_glue::devfs::is_loop_ops(file.get_ops()?) {
        return None;
    }
    file.private_data_ptr().map(|pd| {
        // SAFETY: devfs_open stored a Box<DevNo> (file-lifetime) in the
        // slot; read-only here.
        unsafe { *(pd.cast_const() as *const DevNo) }
    })
}

/// Read from the loop device at the file position (buffer-cache backed —
/// coherent with filesystem metadata I/O on the same loop).
pub fn loop_file_read(file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    let devno = match loop_devno_of(file) {
        Some(d) => d,
        None => return -9, // EBADF
    };
    if devno.major != LOOP_MAJOR {
        return -25; // ENOTTY-ish: loop-control has no data
    }
    let idx = devno.minor as usize;
    let disk = match loop_disk(idx) {
        Some(d) => d,
        None => return -6,
    };
    if !loop_is_bound(idx) {
        return -6; // ENXIO
    }
    let bs = 4096usize;
    let pos = file.get_pos() as usize;
    let mut done = 0usize;
    while done < buf.len() {
        let blocknr = (pos + done) / bs;
        let in_off = (pos + done) % bs;
        let want = core::cmp::min(bs - in_off, buf.len() - done);
        let bh = match crate::fs::bio::bread(disk, blocknr as u64) {
            Some(b) => b,
            None => break,
        };
        // SAFETY: bh from bread; b_data is block_size bytes.
        unsafe {
            let data = &(*bh).b_data;
            buf[done..done + want].copy_from_slice(&data[in_off..in_off + want]);
            crate::fs::bio::brelse(bh);
        }
        done += want;
    }
    if done == 0 {
        return 0;
    }
    file.set_pos((pos + done) as u64);
    done as isize
}

/// Write to the loop device at the file position (read-modify-write
/// through the buffer cache; partial tails preserved).
pub fn loop_file_write(file: &crate::fs::file::File, buf: &[u8]) -> isize {
    let devno = match loop_devno_of(file) {
        Some(d) => d,
        None => return -9,
    };
    if devno.major != LOOP_MAJOR {
        return -25;
    }
    let idx = devno.minor as usize;
    let disk = match loop_disk(idx) {
        Some(d) => d,
        None => return -6,
    };
    if !loop_is_bound(idx) {
        return -6; // ENXIO
    }
    let bs = 4096usize;
    let pos = file.get_pos() as usize;
    let mut done = 0usize;
    while done < buf.len() {
        let blocknr = (pos + done) / bs;
        let in_off = (pos + done) % bs;
        let want = core::cmp::min(bs - in_off, buf.len() - done);
        let bh = match crate::fs::bio::bread(disk, blocknr as u64) {
            Some(b) => b,
            None => break,
        };
        // F-BH-LOOP: bh from bread — the BufferHead API is raw-pointer
        // by design (fs/bio.rs, L5's bio lane owns its conversion);
        // b_data writable and block-sized here.
        // SAFETY: as above.
        unsafe {
            {
                let data = &mut (*bh).b_data;
                data[in_off..in_off + want].copy_from_slice(&buf[done..done + want]);
            }
            (*bh).set_state_bit(crate::fs::bio::BufferState::BH_Dirty);
            let r = crate::fs::bio::sync_dirty_buffer(bh);
            crate::fs::bio::brelse(bh);
            if r.is_err() {
                break;
            }
        }
        done += want;
    }
    if done == 0 {
        return -5;
    }
    file.set_pos((pos + done) as u64);
    done as isize
}

/// /proc/partitions lines for the loop devices (bound ones only, like
/// Linux: an unbound loop has no size and is not listed... Linux lists
/// loop minors present in the loop bitmap; we list bound devices).
pub fn partitions_lines() -> Vec<u8> {
    let mut out = alloc::string::String::new();
    for i in 0..N_LOOPS {
        if loop_is_bound(i) {
            let kb = loop_size_bytes(i) / 1024;
            out.push_str(&alloc::format!(
                " 7 {} {} {}\n",
                i, kb, loop_static_name(i)
            ));
        }
    }
    out.into_bytes()
}

// ============================================================================
// ioctl dispatcher (hooked from sys_ioctl on the LOOP_FILE_OPS identity)
// ============================================================================


fn loop_status_of(idx: usize) -> LoopStatus {
    let mut st = LoopStatus {
        device: 7 << 20 | (idx as u64),
        inode: 0,
        rdevice: 0,
        offset: 0,
        sizelimit: 0,
        number: idx as u32,
        encrypt_type: 0,
        encrypt_key_size: 0,
        flags: 0,
        name: [0; LO_NAME_SIZE],
        init: [0; 2],
    };
    if idx >= N_LOOPS {
        return st;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = &LOOPS[idx];
    st.offset = dev.offset.load(Ordering::Acquire);
    st.sizelimit = dev.sizelimit.load(Ordering::Acquire);
    st.flags = dev.flags.load(Ordering::Acquire) as u32;
    st.name = *dev.name.lock();
    if let Some(file) = dev.file.lock().as_ref() {
        if let Some(inode) = file.get_inode().as_ref() {
            st.inode = inode.ino;
        }
    }
    st
}

/// LOOP_CHANGE_FD: swap the backing file of a BOUND loop.
fn loop_change_fd(idx: usize, fd: usize) -> Result<(), i32> {
    let file = crate::fs::file::get_file_fd(fd)
        .ok_or_else(|| crate::errno::Errno::BadFileNumber.as_neg_i32())?;
    let size = file_size_bytes(&file)? as u64;
    // SAFETY: static array access bounded by N_LOOPS (checked by caller).
    let dev = &LOOPS[idx];
    let mut slot = dev.file.lock();
    if slot.is_none() {
        return Err(crate::errno::Errno::NoSuchDeviceOrAddress.as_neg_i32());
    }
    *slot = Some(file);
    if let Some(disk) = *dev.disk.lock() {
        disk.capacity.store(size / 512, Ordering::Release);
    }
    Ok(())
}

/// LOOP_SET_CAPACITY: the backing file may have grown (ftruncate after
/// attach); refresh the exported capacity from the live size.
fn loop_set_capacity(idx: usize) -> Result<(), i32> {
    let bytes = loop_size_bytes(idx);
    if bytes == 0 {
        return Err(crate::errno::Errno::NoSuchDeviceOrAddress.as_neg_i32());
    }
    // SAFETY: static array access bounded by N_LOOPS.
    if let Some(disk) = *LOOPS[idx].disk.lock() {
        disk.capacity.store(bytes / 512, Ordering::Release);
    }
    Ok(())
}

/// Handle a loop-family ioctl. `file` is the open /dev/loopN or
/// /dev/loop-control file (devno stashed in private_data by devfs_open).
/// Returns Some(ret) when handled, None when the file is not a loop.
pub fn loop_file_ioctl(
    file: &crate::fs::file::File,
    request: u32,
    arg: usize,
) -> Option<i64> {
    use crate::arch::uaccess_safe as uaccess;

    if !crate::fs::leaf_glue::devfs::is_loop_ops(file.get_ops()?) {
        return None;
    }
    let Some(pd) = file.private_data_ptr() else { return None };
    // SAFETY: devfs_open stored a Box<DevNo> (file-lifetime) in the
    // slot; read-only here.
    let devno = unsafe { *(pd.cast_const() as *const DevNo) };
    let is_loop = devno.major == LOOP_MAJOR && (devno.minor as usize) < N_LOOPS;
    let idx = devno.minor as usize;
    // Linux loop_ioctl order: ENXIO for any loop-specific request on an
    // unbound loop (except SET_FD/CONFIGURE which bind, and CLR_FD which
    // answers ENXIO itself).
    let bound = is_loop && loop_is_bound(idx);

    let ret: i64 = match request {
        LOOP_CTL_GET_FREE => {
            if devno.major != crate::fs::dev_t::MISC_MAJOR
                || devno.minor != LOOP_CONTROL_MINOR
            {
                return Some(-25i64); // ENOTTY on loop nodes
            }
            let free = loop_get_free();
            if free < 0 {
                -6i64 // ENXIO: none free
            } else {
                free as i64
            }
        }
        LOOP_CTL_ADD => {
            if devno.major != crate::fs::dev_t::MISC_MAJOR
                || devno.minor != LOOP_CONTROL_MINOR
            {
                return Some(-25i64);
            }
            // All N_LOOPS devices always exist here; a new one cannot be
            // created. Existing number => EEXIST, beyond the fixed range
            // => ENOSPC (Linux loop_control ADD semantics).
            let want = arg as u32;
            if want < N_LOOPS as u32 {
                -17i64 // EEXIST
            } else {
                -28i64 // ENOSPC: loop table full
            }
        }
        LOOP_CTL_REMOVE => {
            if devno.major != crate::fs::dev_t::MISC_MAJOR
                || devno.minor != LOOP_CONTROL_MINOR
            {
                return Some(-25i64);
            }
            let want = arg as usize;
            if want >= N_LOOPS {
                return Some(-6i64); // ENXIO
            }
            if loop_is_bound(want) {
                return Some(-16i64); // EBUSY: still in use
            }
            0
        }
        LOOP_SET_FD => {
            if !is_loop {
                return Some(-25i64);
            }
            let backing = crate::fs::file::get_file_fd(arg);
            match backing {
                Some(f) => {
                    let path = fd_backing_path(&f);
                    match loop_set_fd_named(idx, f, &path) {
                        Ok(()) => 0,
                        Err(e) => e as i64,
                    }
                }
                None => -9i64, // EBADF
            }
        }
        LOOP_CONFIGURE => {
            if !is_loop {
                return Some(-25i64);
            }
            if bound {
                return Some(-16i64); // EBUSY (Linux loop_configure)
            }
            // struct loop_config { __u32 fd; __u32 block_size;
            //                     struct loop_info64 info; ... }
            let mut cfg = [0u8; 8 + LOOP_INFO64_SIZE];
            if uaccess::copy_in(&mut cfg, arg) != 0 {
                return Some(-14i64); // EFAULT
            }
            let fd = u32::from_le_bytes(cfg[0..4].try_into().unwrap()) as usize;
            let offset = u64::from_le_bytes(cfg[8 + 24..8 + 32].try_into().unwrap());
            let sizelimit = u64::from_le_bytes(cfg[8 + 32..8 + 40].try_into().unwrap());
            let backing = match crate::fs::file::get_file_fd(fd) {
                Some(f) => f,
                None => return Some(-9i64),
            };
            let path = fd_backing_path(&backing);
            match loop_set_fd_named(idx, backing, &path) {
                Ok(()) => {
                    let _ = set_loop_geometry(idx, offset, sizelimit);
                    0
                }
                Err(e) => e as i64,
            }
        }
        LOOP_CLR_FD => {
            if !is_loop {
                return Some(-25i64);
            }
            match loop_clr_fd(idx) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        BLKGETSIZE64 | BLKSSZGET => {
            // mke2fs sizes the filesystem from BLKGETSIZE64 (and a failed
            // probe makes it fall back to an interactive "proceed?" prompt
            // that reads EOF from /dev/null and aborts — "mkfs.ext2 failed
            // with exit code 1" in every LTP tst_mkfs test).
            if !is_loop {
                return Some(-25i64);
            }
            let val: u64 = if request == BLKSSZGET {
                512
            } else {
                loop_size_bytes(idx)
            };
            // SAFETY: arg points to a user u64/u16; exception-table copy.
            let len = if request == BLKSSZGET { 2 } else { 8 };
            let bytes = val.to_le_bytes();
            let mut out = [0u8; 8];
            out[..len].copy_from_slice(&bytes[..len]);
            if uaccess::copy_out(arg, &out[..len]) != 0 {
                return Some(-14i64); // EFAULT
            }
            0
        }
        LOOP_GET_STATUS64 => {
            if !bound {
                return Some(-6i64); // ENXIO: unbound loop
            }
            let st = loop_status_of(idx);
            let mut info = [0u8; LOOP_INFO64_SIZE];
            let put64 = |buf: &mut [u8; LOOP_INFO64_SIZE], off: usize, v: u64| {
                buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
            };
            put64(&mut info, 0, st.device);
            put64(&mut info, 8, st.inode);
            put64(&mut info, 16, st.rdevice);
            put64(&mut info, 24, st.offset);
            put64(&mut info, 32, st.sizelimit);
            info[40..44].copy_from_slice(&st.number.to_le_bytes());
            info[44..48].copy_from_slice(&st.encrypt_type.to_le_bytes());
            info[48..52].copy_from_slice(&st.encrypt_key_size.to_le_bytes());
            info[52..56].copy_from_slice(&st.flags.to_le_bytes());
            info[56..56 + LO_NAME_SIZE].copy_from_slice(&st.name);
            put64(&mut info, 216, st.init[0]);
            put64(&mut info, 224, st.init[1]);
            if uaccess::copy_out(arg, &info) != 0 {
                return Some(-14i64); // EFAULT
            }
            0
        }
        LOOP_SET_STATUS64 => {
            if !bound {
                return Some(-6i64);
            }
            let mut info = [0u8; LOOP_INFO64_SIZE];
            if uaccess::copy_in(&mut info, arg) != 0 {
                return Some(-14i64); // EFAULT
            }
            let offset = u64::from_le_bytes(info[24..32].try_into().unwrap());
            let limit = u64::from_le_bytes(info[32..40].try_into().unwrap());
            let flags = u32::from_le_bytes(info[52..56].try_into().unwrap());
            // Validate BEFORE applying (Linux loop_set_status64 order):
            // reject unknown lo_flags bits.
            if flags
                & !(LO_FLAGS_READ_ONLY
                    | LO_FLAGS_AUTOCLEAR
                    | LO_FLAGS_PARTSCAN
                    | LO_FLAGS_DIRECT_IO)
                != 0
            {
                return Some(-22i64); // EINVAL
            }
            match set_loop_geometry(idx, offset, limit) {
                Ok(()) => {
                    {
                        LOOPS[idx].flags.store(flags as u64, Ordering::Release);
                        if !info[56..56 + LO_NAME_SIZE].iter().all(|&b| b == 0) {
                            let mut name = LOOPS[idx].name.lock();
                            name.copy_from_slice(&info[56..56 + LO_NAME_SIZE]);
                        }
                    }
                    0
                }
                Err(e) => e as i64,
            }
        }
        LOOP_GET_STATUS => {
            // Legacy struct loop_info (LP64 wire layout — see LOOP_INFO_SIZE).
            // Unbound -> ENXIO (LTP's free-loop probe relies on it).
            if !bound {
                return Some(-6i64);
            }
            let st = loop_status_of(idx);
            let mut info = [0u8; LOOP_INFO_SIZE];
            info[LI_OFF_NUMBER..LI_OFF_NUMBER + 4].copy_from_slice(&st.number.to_le_bytes());
            info[LI_OFF_DEVICE..LI_OFF_DEVICE + 4]
                .copy_from_slice(&((st.device & 0xFFFF_FFFF) as u32).to_le_bytes());
            info[LI_OFF_INODE..LI_OFF_INODE + 8].copy_from_slice(&st.inode.to_le_bytes());
            info[LI_OFF_RDEVICE..LI_OFF_RDEVICE + 4]
                .copy_from_slice(&((st.rdevice & 0xFFFF_FFFF) as u32).to_le_bytes());
            info[LI_OFF_OFFSET..LI_OFF_OFFSET + 4]
                .copy_from_slice(&(st.offset as u32).to_le_bytes());
            info[LI_OFF_ENCRYPT_TYPE..LI_OFF_ENCRYPT_TYPE + 4]
                .copy_from_slice(&st.encrypt_type.to_le_bytes());
            info[LI_OFF_KEY_SIZE..LI_OFF_KEY_SIZE + 4]
                .copy_from_slice(&st.encrypt_key_size.to_le_bytes());
            info[LI_OFF_FLAGS..LI_OFF_FLAGS + 4].copy_from_slice(&st.flags.to_le_bytes());
            info[LI_OFF_NAME..LI_OFF_NAME + LO_NAME_SIZE].copy_from_slice(&st.name);
            info[LI_OFF_INIT..LI_OFF_INIT + 8].copy_from_slice(&st.init[0].to_le_bytes());
            info[LI_OFF_INIT + 8..LI_OFF_INIT + 16].copy_from_slice(&st.init[1].to_le_bytes());
            if uaccess::copy_out(arg, &info) != 0 {
                return Some(-14i64); // EFAULT
            }
            0
        }
        LOOP_SET_STATUS => {
            // Legacy SET: LTP's tst_attach_device issues it right after
            // LOOP_SET_FD with lo_name = backing path.
            if !bound {
                return Some(-6i64);
            }
            let mut info = [0u8; LOOP_INFO_SIZE];
            if uaccess::copy_in(&mut info, arg) != 0 {
                return Some(-14i64); // EFAULT
            }
            let offset = u64::from(u32::from_le_bytes(
                info[LI_OFF_OFFSET..LI_OFF_OFFSET + 4].try_into().unwrap(),
            ));
            let flags = u32::from_le_bytes(
                info[LI_OFF_FLAGS..LI_OFF_FLAGS + 4].try_into().unwrap(),
            );
            if flags & !(LO_FLAGS_READ_ONLY | LO_FLAGS_AUTOCLEAR | LO_FLAGS_PARTSCAN) != 0 {
                return Some(-22i64); // EINVAL
            }
            {
                LOOPS[idx].offset.store(offset, Ordering::Release);
                LOOPS[idx].flags.store(flags as u64, Ordering::Release);
                if !info[LI_OFF_NAME..LI_OFF_NAME + LO_NAME_SIZE].iter().all(|&b| b == 0) {
                    let mut name = LOOPS[idx].name.lock();
                    name.copy_from_slice(&info[LI_OFF_NAME..LI_OFF_NAME + LO_NAME_SIZE]);
                }
            }
            0
        }
        LOOP_CHANGE_FD => {
            if !bound {
                return Some(-6i64);
            }
            match loop_change_fd(idx, arg) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        LOOP_SET_CAPACITY => {
            if !bound {
                return Some(-6i64);
            }
            match loop_set_capacity(idx) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        LOOP_SET_DIRECT_IO => {
            // Our loop always routes through the buffer cache; accepting
            // the request keeps DIRECT_IO-capable users (util-linux
            // `losetup --direct-io=on`) working.
            if !bound {
                return Some(-6i64);
            }
            0
        }
        LOOP_SET_BLOCK_SIZE => {
            if !bound {
                return Some(-6i64);
            }
            let bs = arg as u32;
            if bs < 512 || bs > 4096 || !bs.is_power_of_two() {
                return Some(-22i64); // EINVAL
            }
            0
        }
        _ => -25i64, // ENOTTY
    };
    Some(ret)
}

/// Best-effort backing path for a file fd (for lo_file_name). The VFS
/// tracks no per-open path, so ask the fd table for the inode identity —
/// empty when unknown (GET_STATUS then reports an empty name, same as a
/// detached loop; no consumer treats that as an error).
fn fd_backing_path(file: &Arc<File>) -> Vec<u8> {
    let _ = file;
    Vec::new()
}

#[allow(dead_code)]
fn loop_offset_of(idx: usize) -> u64 {
    if idx >= N_LOOPS {
        return 0;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    LOOPS[idx].offset.load(Ordering::Acquire)
}

#[allow(dead_code)]
fn loop_sizelimit_of(idx: usize) -> u64 {
    if idx >= N_LOOPS {
        return 0;
    }
    // SAFETY: static array access bounded by N_LOOPS.
    LOOPS[idx].sizelimit.load(Ordering::Acquire)
}

fn set_loop_geometry(idx: usize, offset: u64, sizelimit: u64) -> Result<(), i32> {
    if idx >= N_LOOPS {
        return Err(crate::errno::Errno::InvalidArgument.as_neg_i32());
    }
    // SAFETY: static array access bounded by N_LOOPS.
    let dev = &LOOPS[idx];
    dev.offset.store(offset, Ordering::Release);
    dev.sizelimit.store(sizelimit, Ordering::Release);
    Ok(())
}

/// Seek on the loop device node (position in bytes; sizes come from the
/// bound backing file, zero for an unbound loop).
pub fn loop_file_lseek(file: &crate::fs::file::File, offset: isize, whence: i32) -> isize {
    let devno = match loop_devno_of(file) {
        Some(d) => d,
        None => return -9,
    };
    if devno.major != LOOP_MAJOR {
        return -29; // ESPIPE on loop-control
    }
    let size = loop_size_bytes(devno.minor as usize) as isize;
    let cur = file.get_pos() as isize;
    let base = match whence {
        0 => 0,                       // SEEK_SET
        1 => cur,                     // SEEK_CUR
        2 => size,                    // SEEK_END
        _ => return -22,
    };
    let target = match base.checked_add(offset) {
        Some(t) if t >= 0 => t,
        _ => return -22,
    };
    file.set_pos(target as u64);
    target
}

// ════════════════ ashmem ══════════════════════════════
// (was ashmem.rs)
// /dev/ashmem — Anonymous Shared Memory (Android/OpenHarmony ABI).
//
// Port of the classic Linux `drivers/staging/android/ashmem.c` interface
// (the ABI OpenHarmony's libc buffer management uses):
//
// - open("/dev/ashmem") creates a fresh, unnamed, zero-sized area
// - ASHMEM_SET_NAME / ASHMEM_GET_NAME — advisory name (frozen at mmap)
// - ASHMEM_SET_SIZE / ASHMEM_GET_SIZE — byte size (frozen at mmap)
// - ASHMEM_SET_PROT_MASK / ASHMEM_GET_PROT_MASK — allowed PROT_* subset
//   (starts PROT_READ|PROT_WRITE|PROT_EXEC, can only narrow)
// - ASHMEM_PIN / ASHMEM_UNPIN / ASHMEM_GET_PIN_STATUS — page-range pin
//   bookkeeping (ranges are tracked exactly like the Linux driver's
//   ashmem_range list; nothing is ever actually reclaimed here, so PIN
//   always reports ASHMEM_NOT_PURGED)
// - ASHMEM_PURGE_ALL_CACHES — CAP_SYS_ADMIN only, no-op returning 0
//   freed pages (no shrinker exists)
// - mmap maps the area's private page pool into the caller; MAP_SHARED
//   mappings alias the pool pages (fork/COW-exempt via VmaFlags::SHARED,
//   the SysV shmat model), MAP_PRIVATE mappings are read-only+COW.
//
// Implementation model (Phase 2 prep, OH port plan §6 item 2): each open
// file description owns an `AshmemArea` (Box in File::private_data).
// The backing store is an eagerly allocated physical page pool created
// at the FIRST mmap — the same discipline SysV shm uses (ShmPages): the
// pool holds one reference per page, every mapping takes one per PTE
// (get_page at map, put_page at unmap/exit), so pages survive until the
// last mapping is gone even if every fd was closed first. Linux hangs
// the same lifetime off the shmem backing file's vm_file refcount.
/// Default name reported by ASHMEM_GET_NAME for unnamed areas.
const ASHMEM_NAME_DEF: &[u8] = b"dev/ashmem\0";

/// ASHMEM_PIN / ASHMEM_UNPIN result: content survived.
pub const ASHMEM_NOT_PURGED: i64 = 0;
/// ASHMEM_PIN result: content was purged while unpinned.
pub const ASHMEM_WAS_PURGED: i64 = 1;

/// ASHMEM_GET_PIN_STATUS result.
pub const ASHMEM_IS_UNPINNED: i64 = 0;
pub const ASHMEM_IS_PINNED: i64 = 1;


// ioctl command words, asm-generic encoding: dir<<30 | size<<16 | 0x77<<8 | nr
// (char[256] payload -> size field 0x100; verified against _IOW() on the
// riscv64 toolchain)
const ASHMEM_SET_NAME: u32 = 0x4100_7701; // _IOW(0x77, 1, char[256])
const ASHMEM_GET_NAME: u32 = 0x8100_7702; // _IOR(0x77, 2, char[256])
const ASHMEM_SET_SIZE: u32 = 0x4008_7703; // _IOW(0x77, 3, size_t)
const ASHMEM_GET_SIZE: u32 = 0x0000_7704; // _IO(0x77, 4)
const ASHMEM_SET_PROT_MASK: u32 = 0x4008_7705; // _IOW(0x77, 5, unsigned long)
const ASHMEM_GET_PROT_MASK: u32 = 0x0000_7706; // _IO(0x77, 6)
const ASHMEM_PIN: u32 = 0x4008_7707; // _IOW(0x77, 7, struct ashmem_pin)
const ASHMEM_UNPIN: u32 = 0x4008_7708; // _IOW(0x77, 8, struct ashmem_pin)
const ASHMEM_GET_PIN_STATUS: u32 = 0x0000_7709; // _IO(0x77, 9)
const ASHMEM_PURGE_ALL_CACHES: u32 = 0x0000_770a; // _IO(0x77, 10)

/// PROT_READ | PROT_WRITE | PROT_EXEC (asm-generic values).
const PROT_MASK: u32 = 0x1 | 0x2 | 0x4;

/// /dev/ashmem device number (misc 10:62 — no other misc minor in this
/// kernel occupies 62; Linux allocates the real ashmem minor
/// dynamically and userspace only ever opens by path).
pub const DEV_ASHMEM: DevNo = DevNo::new(MISC_MAJOR, 62);

// ============================================================================
// Page pool
// ============================================================================


impl AshmemPages {
    fn new(size: usize) -> Option<Self> {
        let page_count = size.div_ceil(PAGE_SIZE);
        let mut pages = alloc::vec::Vec::with_capacity(page_count);
        for _ in 0..page_count {
            let phys = get_zeroed_page(GfpFlags::GFP_USER);
            if phys == 0 {
                for p in &pages {
                    free_pages(*p, 0);
                }
                return None;
            }
            pages.push(phys);
        }
        Some(Self { pages })
    }

    fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn get_page(&self, idx: usize) -> Option<usize> {
        self.pages.get(idx).copied()
    }
}

impl Drop for AshmemPages {
    fn drop(&mut self) {
        // Drop the pool's owner reference per page. Live mappings hold
        // their own reference (taken at map time), so a page still mapped
        // by any process survives until its last unmap's put_page.
        for p in &self.pages {
            let page = crate::mm::hotpath::pfn_to_page_mut(crate::mm::hotpath::phys_to_pfn(*p));
            let freed_by_us = if page.is_null() {
                true
            } else {
                // SAFETY: descriptor exists for this in-RAM page.
                unsafe { (*page).put_page() == 0 }
            };
            if freed_by_us {
                free_pages(*p, 0);
            }
        }
    }
}


impl AshmemArea {
    fn new() -> Self {
        Self {
            inner: Spinlock::new(AreaInner {
                name: [0; ASHMEM_NAME_LEN],
                name_set: false,
                size: 0,
                prot_mask: PROT_MASK,
                pages: None,
                unpinned: alloc::vec::Vec::new(),
            }),
        }
    }
}

// ============================================================================
// Range algebra (port of ashmem_pin/ashmem_unpin/ashmem_get_pin_status)
// ============================================================================

/// page_range_in_range: does [rs,re] interact with [s,e]?
fn range_in_range(r: UnpinRange, s: usize, e: usize) -> bool {
    (r.pgstart <= s && r.pgend >= s)
        || (r.pgstart <= e && r.pgend >= e)
        || (r.pgstart >= s && r.pgend <= e)
}

/// Pin [pgstart,pgend]: remove/split/trim the overlapping unpinned ranges.
/// Returns the OR of purged flags of the removed parts (never purged here,
/// but the bookkeeping mirrors the Linux driver exactly).
fn pin_ranges(unpinned: &mut alloc::vec::Vec<UnpinRange>, pgstart: usize, pgend: usize) -> u32 {
    let mut ret = 0u32;
    let mut i = 0;
    while i < unpinned.len() {
        let r = unpinned[i];
        if r.pgend < pgstart {
            // Sorted list: ranges before our window.
            i += 1;
            continue;
        }
        if !range_in_range(r, pgstart, pgend) {
            i += 1;
            continue;
        }
        ret |= r.purged;
        if r.pgstart >= pgstart && r.pgend <= pgend {
            // Case 1: request subsumes the range — remove it whole.
            unpinned.remove(i);
            continue;
        }
        if r.pgstart >= pgstart {
            // Case 2: overlap from the front — trim the range's start.
            unpinned[i].pgstart = pgend + 1;
            i += 1;
            continue;
        }
        if r.pgend <= pgend {
            // Case 3: overlap from the rear — trim the range's end.
            unpinned[i].pgend = pgstart - 1;
            i += 1;
            continue;
        }
        // Case 4: the pin punches a hole — split into two ranges.
        let second = UnpinRange {
            pgstart: pgend + 1,
            pgend: r.pgend,
            purged: r.purged,
        };
        unpinned[i].pgend = pgstart - 1;
        unpinned.insert(i + 1, second);
        break;
    }
    ret
}

/// Unpin [pgstart,pgend]: merge with overlapping ranges, insert sorted.
fn unpin_ranges(unpinned: &mut alloc::vec::Vec<UnpinRange>, mut pgstart: usize, mut pgend: usize) {
    let mut purged = 0u32;
    let mut i = 0;
    while i < unpinned.len() {
        let r = unpinned[i];
        if r.pgend < pgstart {
            i += 1;
            continue;
        }
        // Entirely covered by an existing range: nothing to do.
        if r.pgstart <= pgstart && r.pgend >= pgend {
            return;
        }
        if range_in_range(r, pgstart, pgend) {
            // Widen the request and absorb the range (Linux's restart).
            pgstart = pgstart.min(r.pgstart);
            pgend = pgend.max(r.pgend);
            purged |= r.purged;
            unpinned.remove(i);
            i = 0;
            continue;
        }
        i += 1;
    }
    let pos = unpinned.partition_point(|x| x.pgstart < pgstart);
    unpinned.insert(
        pos,
        UnpinRange {
            pgstart,
            pgend,
            purged,
        },
    );
}

/// GET_PIN_STATUS: any unpinned range touching [pgstart,pgend]?
fn any_unpinned(unpinned: &[UnpinRange], pgstart: usize, pgend: usize) -> bool {
    unpinned.iter().any(|r| range_in_range(*r, pgstart, pgend))
}

// ============================================================================
// File operations
// ============================================================================

/// Extract the &AshmemArea a File's private_data points at.
// SAFETY: private_data was installed by devfs_open as a Box<AshmemArea>
// and is freed by ashmem_file_close (ops.close, exactly once per open
// file description). Callers hold an Arc<File> from the fd table, so the
// close op cannot have run yet.
fn area_of<'a>(file: &'a crate::fs::file::File) -> Option<&'a AshmemArea> {
    // The area box is the description's private data (INV-FS-FILEPRIV-1,
    // the framework edge); the Arc<File> callers hold pins it.
    crate::fs::r#unsafe::file_privdata_ref::<AshmemArea>(file)
}

fn ashmem_file_read(file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    // (area_of: the INV-FS-FILEPRIV-1 framework edge — safe fn.)
    let area = match area_of(file) {
        Some(a) => a,
        None => return -9, // EBADF
    };
    let mut inner = area.inner.lock_irqsave();
    if inner.size == 0 {
        return 0; // EOF, like Linux ashmem_read_iter
    }
    let Some(ref pages) = inner.pages else {
        return -9; // EBADF: no backing store until the first mmap
    };
    let pos = file.get_pos() as usize;
    if pos >= inner.size {
        return 0;
    }
    let want = buf.len().min(inner.size - pos);
    let mut done = 0;
    while done < want {
        let off = pos + done;
        let pg = off / PAGE_SIZE;
        let in_pg = off % PAGE_SIZE;
        let chunk = (want - done).min(PAGE_SIZE - in_pg);
        let phys = match pages.get_page(pg) {
            Some(p) => p,
            None => break,
        };
        let src = crate::arch::mm::memory_layout::phys_to_virt(MmPhysAddr::new(phys as u64))
            .bits() as *const u8;
        // SAFETY: src is the kernel linear map of a live pool page; the
        // destination is the syscall-validated user buffer slice.
        unsafe {
            core::ptr::copy_nonoverlapping(src.add(in_pg), buf.as_mut_ptr().add(done), chunk);
        }
        done += chunk;
    }
    file.set_pos((pos + done) as u64);
    done as isize
}

fn ashmem_file_lseek(file: &crate::fs::file::File, offset: isize, whence: i32) -> isize {
    // (area_of: the INV-FS-FILEPRIV-1 framework edge — safe fn.)
    let area = match area_of(file) {
        Some(a) => a,
        None => return -9, // EBADF
    };
    let inner = area.inner.lock_irqsave();
    if inner.size == 0 {
        return -22; // EINVAL, like Linux ashmem_llseek
    }
    if inner.pages.is_none() {
        return -9; // EBADF
    }
    let cur = file.get_pos() as i64;
    let size = inner.size as i64;
    let new_pos = match whence {
        0 => offset as i64,
        1 => cur + offset as i64,
        2 => size + offset as i64,
        _ => return -22,
    };
    if new_pos < 0 {
        return -22;
    }
    file.set_pos(new_pos as u64);
    new_pos as isize
}

fn ashmem_file_close(file: &crate::fs::file::File) -> i32 {
    // Linux ashmem_release: mappings do NOT pin the area — it dies on
    // close. Mapped pages survive on their own refcounts (AshmemPages
    // drop only releases the pool's owner references).
    // ops.close runs exactly once, on the final reference drop (close_fd
    // / fdtable teardown / deferred close_pending); the Box reclaim
    // rides the framework edge (INV-FS-FILEPRIV-1).
    drop(crate::fs::r#unsafe::file_privdata_take::<AshmemArea>(file));
    0
}

/// /dev/ashmem file operations. Write is a declared absence (the
/// Linux driver has no .write_iter either).
pub static ASHMEM_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(ashmem_file_read)
    .lseek(ashmem_file_lseek)
    .close(ashmem_file_close)
    .build();

/// True when `file` is an open /dev/ashmem description.
pub fn is_ashmem_file(file: &crate::fs::file::File) -> bool {
    match file.get_ops() {
        Some(ops) => core::ptr::eq(ops as *const _, &ASHMEM_OPS as *const _),
        None => false,
    }
}

/// devfs_open hook: allocate the per-open area.
pub fn ashmem_open(file: &crate::fs::file::File) -> i32 {
    file.set_private_data(
        alloc::boxed::Box::into_raw(alloc::boxed::Box::new(AshmemArea::new())) as *mut u8,
    );
    0
}

// ============================================================================
// ioctl
// ============================================================================

/// Per-fd ioctl dispatch (ops identity, the loop/evdev pattern).
/// Returns Some(ret) when the file is ashmem, None otherwise.
pub fn ashmem_file_ioctl(
    file: &crate::fs::file::File,
    request: u32,
    arg: usize,
) -> Option<i64> {
    if !is_ashmem_file(file) {
        return None;
    }
    Some(ashmem_ioctl(file, request, arg))
}

/// `arg` is the raw syscall argument; every user-pointer use rides the
/// exception-table uaccess paths (validated before dereference).
fn ashmem_ioctl(file: &crate::fs::file::File, request: u32, arg: usize) -> i64 {
    use crate::arch::uaccess::{access_ok, copy_to_user, strncpy_from_user};

    const EINVAL: i64 = 22;
    const ENOTTY: i64 = 25;
    const EPERM: i64 = 1;
    const EFAULT: i64 = 14;

    // (area_of: the INV-FS-FILEPRIV-1 framework edge — safe fn.)
    let area = match area_of(file) {
        Some(a) => a,
        None => return -EINVAL,
    };

    match request {
        ASHMEM_SET_NAME => {
            if arg == 0 || !access_ok(arg, ASHMEM_NAME_LEN) {
                return -EFAULT;
            }
            let mut local = [0u8; ASHMEM_NAME_LEN];
            // SAFETY: arg validated above; strncpy_from_user goes through
            // the exception-tabled byte fetcher.
            let name = match strncpy_from_user(arg as *const u8, ASHMEM_NAME_LEN, &mut local) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let len = name.len().min(ASHMEM_NAME_LEN - 1);
            let mut inner = area.inner.lock_irqsave();
            if inner.pages.is_some() {
                return -EINVAL; // cannot change name after mmap
            }
            inner.name = [0; ASHMEM_NAME_LEN];
            inner.name[..len].copy_from_slice(&name[..len]);
            inner.name_set = true;
            0
        }
        ASHMEM_GET_NAME => {
            if arg == 0 || !access_ok(arg, ASHMEM_NAME_LEN) {
                return -EFAULT;
            }
            let inner = area.inner.lock_irqsave();
            // Only the meaningful bytes are copied out (never the whole
            // 256-byte stack-buffer content).
            let (bytes, len): (&[u8], usize) = if inner.name_set {
                let len = inner
                    .name
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(ASHMEM_NAME_LEN - 1)
                    + 1;
                (&inner.name, len)
            } else {
                (ASHMEM_NAME_DEF, ASHMEM_NAME_DEF.len())
            };
            // arg validated above for ASHMEM_NAME_LEN bytes and
            // len <= ASHMEM_NAME_LEN (typed capsule copy).
            if crate::arch::uaccess_safe::copy_out(arg as usize, &bytes[..len]) != 0 {
                return -EFAULT;
            }
            0
        }
        ASHMEM_SET_SIZE => {
            let mut inner = area.inner.lock_irqsave();
            if inner.pages.is_some() {
                return -EINVAL; // cannot resize after mmap
            }
            inner.size = arg;
            0
        }
        ASHMEM_GET_SIZE => {
            let inner = area.inner.lock_irqsave();
            inner.size as i64
        }
        ASHMEM_SET_PROT_MASK => {
            let prot = arg as u32 & PROT_MASK;
            let mut inner = area.inner.lock_irqsave();
            // The mask can only narrow (Linux set_prot_mask).
            if inner.prot_mask & prot != prot {
                return -EINVAL;
            }
            inner.prot_mask = prot;
            0
        }
        ASHMEM_GET_PROT_MASK => {
            let inner = area.inner.lock_irqsave();
            inner.prot_mask as i64
        }
        ASHMEM_PIN | ASHMEM_UNPIN | ASHMEM_GET_PIN_STATUS => {
            // All three commands take a struct ashmem_pin from the user —
            // GET_PIN_STATUS is _IO-encoded but the Linux driver copies
            // the struct for it too (ashmem_pin_unpin handles all three).
            let mut pin = AshmemPin { offset: 0, len: 0 };
            if arg == 0 || !access_ok(arg, core::mem::size_of::<AshmemPin>()) {
                return -EFAULT;
            }
            // arg validated for the 8-byte struct (typed capsule copy).
            if crate::arch::uaccess_safe::copy_in_t(&mut pin, arg) != 0 {
                return -EFAULT;
            }

            let mut inner = area.inner.lock_irqsave();
            if inner.pages.is_none() {
                return -EINVAL; // Linux: no backing file yet
            }
            let aligned_size = (inner.size + PAGE_SIZE - 1) & !PAGE_MASK;
            // len == 0 means "everything onward".
            let mut len = pin.len as usize;
            if len == 0 {
                len = aligned_size - pin.offset as usize;
            }
            let offset = pin.offset as usize;
            if (offset | len) & PAGE_MASK != 0 {
                return -EINVAL; // not page-aligned
            }
            if u32::MAX as usize - offset < len {
                return -EINVAL;
            }
            if aligned_size < offset + len {
                return -EINVAL;
            }
            let pgstart = offset / PAGE_SIZE;
            let pgend = pgstart + len / PAGE_SIZE - 1;
            match request {
                ASHMEM_PIN => pin_ranges(&mut inner.unpinned, pgstart, pgend) as i64,
                ASHMEM_UNPIN => {
                    unpin_ranges(&mut inner.unpinned, pgstart, pgend);
                    0
                }
                _ => {
                    if any_unpinned(&inner.unpinned, pgstart, pgend) {
                        ASHMEM_IS_UNPINNED
                    } else {
                        ASHMEM_IS_PINNED
                    }
                }
            }
        }
        ASHMEM_PURGE_ALL_CACHES => {
            // CAP_SYS_ADMIN only, like the Linux driver; nothing is ever
            // reclaimed (no shrinker), so 0 pages were purged.
            if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
                return -EPERM;
            }
            0
        }
        _ => -ENOTTY,
    }
}

// ============================================================================
// mmap
// ============================================================================

/// ashmem mmap handler, called from sys_mmap once the file was verified
/// to be /dev/ashmem (is_ashmem_file).
///
/// `addr` 0 means "kernel chooses". `offset` is the file offset in bytes
/// (must be page-aligned; mapped pool pages come from that index).
///
/// Returns the mapping address or negative errno.
pub fn ashmem_mmap(
    file: &crate::fs::file::File,
    addr: usize,
    length: usize,
    offset: u64,
    prot: u32,
    map_shared: bool,
) -> Result<usize, i64> {
    const EINVAL: i64 = 22;
    const EPERM: i64 = 1;
    const ENOMEM: i64 = 12;

    if offset as usize % PAGE_SIZE != 0 {
        return Err(-EINVAL);
    }

    // (area_of: the INV-FS-FILEPRIV-1 framework edge — safe fn.)
    let area = area_of(file).ok_or(-EINVAL)?;

    // Validation + pool creation under the area lock; the actual PTE work
    // happens outside it (snapshot the page list first, shmat discipline).
    let (phys_pages, size): (alloc::vec::Vec<usize>, usize) = {
        let mut inner = area.inner.lock_irqsave();
        if inner.size == 0 {
            return Err(-EINVAL); // SET_SIZE required before mmap
        }
        let aligned_size = (inner.size + PAGE_SIZE - 1) & !PAGE_MASK;
        if length > aligned_size {
            return Err(-EINVAL); // mapping larger than the area
        }
        let off = offset as usize;
        if off + length > aligned_size {
            return Err(-EINVAL); // window past the end (stricter than the
                                 // classic driver, which SIGBUSes later)
        }
        // Requested protection must be within the allowed mask.
        if prot & PROT_MASK & !inner.prot_mask != 0 {
            return Err(-EPERM);
        }
        if inner.pages.is_none() {
            let pool = AshmemPages::new(inner.size).ok_or(-ENOMEM)?;
            inner.pages = Some(pool);
        }
        // SAFETY: pages is Some here (just created or pre-existing).
        let pages = inner.pages.as_ref().unwrap();
        let base_pg = off / PAGE_SIZE;
        let npages = length.div_ceil(PAGE_SIZE);
        let mut v = alloc::vec::Vec::with_capacity(npages);
        for i in 0..npages {
            match pages.get_page(base_pg + i) {
                Some(p) => v.push(p),
                None => return Err(-EINVAL),
            }
        }
        (v, inner.size)
    };

    let task = crate::sched::hotpath::current().ok_or(-ENOMEM)?;
    let address_space = task.address_space().ok_or(-ENOMEM)?;
    let root_ppn = address_space.root_ppn();

    if addr != 0 && addr % PAGE_SIZE != 0 {
        return Err(-EINVAL);
    }

    // VMA attributes FIRST (needed for the atomic reserve below).
    let mut vma_flags = VmaFlags::new();
    vma_flags.insert(VmaFlags::READ); // shmat discipline: mapping is readable
    if prot & 0x2 != 0 {
        vma_flags.insert(VmaFlags::WRITE);
    }
    if prot & 0x4 != 0 {
        vma_flags.insert(VmaFlags::EXEC);
    }
    if map_shared {
        vma_flags.insert(VmaFlags::SHARED);
    } else {
        vma_flags.insert(VmaFlags::PRIVATE);
    }

    // ATOMIC RESERVE (b91 corruption fix): the old shape picked on a READ
    // lock (or used the hint verbatim, clobbering whatever lived there),
    // mapped the pool pages eagerly, and only then added the VMA — the
    // add-failure "rollback" munmapped the COLLIDING mapping, and a same-mm
    // mapper winning the range in the window lost its frames to the PTE
    // overwrite. MmStruct::mmap picks + inserts under ONE vma_write (hint
    // honored only when free), so the range below is exclusively ours.
    // SHARED maps use the SysV-shm VMA type so fork inherits the frames
    // as-is (VmaFlags::SHARED is the COW exemption key in
    // copy_page_table_cow). file_fd stays unset — the SysV shm detach
    // accounting keys on it and must not see ashrem VMAs.
    let placement = crate::api::mm::mmap(
        &address_space,
        crate::mm::types::VirtAddr::new(addr),
        length,
        vma_flags,
        if map_shared {
            VmaType::SharedMemory
        } else {
            VmaType::Anonymous
        },
        crate::mm::types::Perm::ReadWrite,
        0,
    )
    .map_err(|_| -ENOMEM)?
        .as_usize();

    // PTE flags: MAP_SHARED maps pool pages writable (when PROT_WRITE);
    // MAP_PRIVATE maps them read-only with the COW marker so the first
    // store breaks to a private copy through the standard COW fault path.
    let mut pte_flags = PageTableEntry::V
        | PageTableEntry::A
        | PageTableEntry::D
        | PageTableEntry::U
        | PageTableEntry::R;
    if map_shared {
        if prot & 0x2 != 0 {
            pte_flags |= PageTableEntry::W;
        }
        if prot & 0x4 != 0 {
            pte_flags |= PageTableEntry::X;
        }
    } else if prot & 0x2 != 0 {
        pte_flags |= crate::arch::mm::mm_ops::cow_flags::COW;
    }

    let npages = phys_pages.len();
    for (i, &phys) in phys_pages.iter().enumerate() {
        // SAFETY: placement + i*PAGE_SIZE is page-aligned inside the range
        // being established; phys is a live pool page; root_ppn is the
        // caller's page-table root. Map + per-mapping reference under the
        // PTE lock (shmat discipline; the map walker is total-safe).
        #[cfg(feature = "x86_64")]
        let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(address_space).lock();
        #[cfg(not(feature = "x86_64"))]
        let _pte_guard = crate::arch::mm::mm_ops::pte_lock_for(address_space).lock_irqsave();
        map_user_page(
            root_ppn,
            MmVirtAddr::new((placement + i * PAGE_SIZE) as u64),
            MmPhysAddr::new(phys as u64),
            pte_flags,
        );
        if let Some(page) = crate::mm::hotpath::page_desc(crate::mm::hotpath::phys_to_pfn(phys)) {
            page.get_page();
        }
    }

    // Preserve the backing-extent attributes the old direct-VMA path
    // recorded (file_size/offset on a SharedMemory VMA are advisory, but
    // keep them for parity with shmat-built VMAs).
    {
        let mut vmas = address_space.vma_write();
        if let Some(v) = vmas.get_mut(crate::mm::types::VirtAddr::new(placement)) {
            v.set_file_size(size as u64);
            v.set_offset(offset as usize);
        }
    }
    address_space.add_rss(npages as u64);
    // SAFETY: fresh user PTEs need a flush before first use on any hart.
    unsafe { crate::arch::mm::asid::flush_tlb_all(); }
    Ok(placement)
}

// ════════════════ initcalls ═══════════════════════════
// (was initcalls.rs)
// Device initcall registration (the R4 crate seam, rung-4 recipe).
//
// The reference kernel's driver model boots through leveled initcall
// tables (`early_initcall` / `arch_initcall` / `subsys_initcall` /
// `device_initcall` / `late_initcall`): the boot core drains the table
// for a level and each registered probe EITHER binds a device (returns
// success) or reports "no device" — a NEGATIVE probe that is a normal
// outcome, never a boot failure (the reference's driver_probe_defer /
// ENODEV discipline). Rux's boot previously named every driver family
// directly (`drivers::probe::init_block_devices()` et al. from
// lib.rs); this module replaces the named call-graph with the same
// table shape so the upcoming drivers service crate can register
// its probes through one seam instead of being named by the framework.
//
// Registration happens before the drain (the framework's own probe
// module registers during early boot today; the service crate's
// `register_all` will append its rows before `run_initcalls` fires —
// the composition bin calls it ahead of `rust_main_impl`).
//
// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
const MAX_INITCALLS: usize = 32;

static TABLE: Spinlock<[Option<DeviceInit>; MAX_INITCALLS]> =
    Spinlock::new([const { None }; MAX_INITCALLS]);
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// Register a device probe row (the initcall-table write side). Rows
/// beyond the table capacity are dropped with a loud report — a silent
/// driver no-probe is the exact failure class this seam exists to make
/// visible.
pub fn register_initcall(row: DeviceInit) {
    let mut t = TABLE.lock();
    let i = COUNT.load(Ordering::Relaxed);
    if i >= MAX_INITCALLS {
        crate::pr_err!(
            "initcall: table full ({}), dropped probe '{}'",
            MAX_INITCALLS,
            row.name
        );
        return;
    }
    t[i] = Some(row);
    COUNT.store(i + 1, Ordering::Release);
}

/// Drain one level's probes in registration order (the reference's
/// `do_initcalls`). Negative probes (0 devices) print nothing at the
/// call site — the BOOT REPORT line only appears for bound devices,
/// matching the old named-call behavior exactly (the lib.rs print
/// sites keyed off nonzero counts).
pub fn run_initcalls(level: InitLevel) -> usize {
    let n = COUNT.load(Ordering::Acquire);
    let mut bound = 0usize;
    let table = TABLE.lock();
    for i in 0..n.min(MAX_INITCALLS) {
        let Some(row) = &table[i] else { continue };
        if row.level != level {
            continue;
        }
        let got = (row.probe)();
        if got > 0 {
            crate::pr_info!("initcall: {} bound {} device(s)", row.name, got);
        }
        bound += got;
    }
    bound
}

/// The boot-report helper the framework's print sites use (keeps the
/// existing `print_status` strings identical to the named-call era).
pub fn level_summary(level: InitLevel) -> alloc::string::String {
    let n = COUNT.load(Ordering::Acquire);
    let mut parts: Vec<&'static str> = Vec::new();
    let table = TABLE.lock();
    for i in 0..n.min(MAX_INITCALLS) {
        let Some(row) = &table[i] else { continue };
        if row.level == level {
            parts.push(row.name);
        }
    }
    format!("{:?}[{}]", level, parts.len())
}

// ════════════════ drm ═════════════════════════════════
// (was drm/mod.rs)
// Minimal DRM/KMS character devices (/dev/dri/card0 + /dev/dri/renderD128)
//
// Implements just enough of the Linux DRM ioctl surface for the
// OpenHarmony display HDI stack:
//
// - composer (drivers/peripheral/display/composer): drmSetClientCap
//   (UNIVERSAL_PLANES + ATOMIC), drmSetMaster/drmIsMaster,
//   drmModeGetResources/Crtc/Encoder/Connector/Plane(+Properties),
//   drmModeCreatePropertyBlob, drmModeAddFB2/RmFB, drmPrimeFDToHandle,
//   GEM_CLOSE, drmModeAtomicCommit and drmWaitVBlank.
// - buffer allocator (drivers/peripheral/display/buffer on renderD128):
//   drmDropMaster, DRM_IOCTL_MODE_CREATE_DUMB / DESTROY_DUMB,
//   drmPrimeHandleToFD, plus mmap() and DMA_BUF_IOCTL_SYNC on the
//   exported prime fd.
//
// Scanout is backed by the virtio-gpu 2D device: every GEM object that
// an atomic commit puts on the primary plane gets its own virtio
// RESOURCE_CREATE_2D + ATTACH_BACKING; a flip is TRANSFER_TO_HOST_2D +
// (SET_SCANOUT when the resource changes) + RESOURCE_FLUSH.
//
// The DRM object model is fixed and tiny (one CRTC/encoder/connector/
// primary plane), mirroring how QEMU virtio-gpu presents itself. The
// plane exposes the rockchip-style "NAME"/"PLANE_MASK" enum/bitmask
// properties the OH composer matches planes with.
// ============================================================================
// Device numbers / nodes
// ============================================================================

/// Linux DRM primary-node major.
const DRM_MAJOR: u32 = 226;
/// Same, exposed for the sysfs class registration (dev/char link names).
pub const DRM_MAJOR_NUM: u32 = DRM_MAJOR;
/// /dev/dri/card0 (primary node, opened by the composer).
pub const DEV_DRM_CARD0: crate::fs::dev_t::DevNo = crate::fs::dev_t::DevNo::new(DRM_MAJOR, 0);
/// /dev/dri/renderD128 (render node, opened by the gbm gralloc).
pub const DEV_DRM_RENDER: crate::fs::dev_t::DevNo = crate::fs::dev_t::DevNo::new(DRM_MAJOR, 128);

// ============================================================================
// KMS object IDs (single fixed pipeline)
// ============================================================================

const OBJ_CRTC: u32 = 40;
const OBJ_ENCODER: u32 = 41;
const OBJ_CONNECTOR: u32 = 42;
const OBJ_PLANE: u32 = 43;

/// Property IDs (the composer looks properties up by NAME through
/// OBJ_GETPROPERTIES + GETPROPERTY; the ids themselves are arbitrary).
const PROP_CRTC_ACTIVE: u32 = 1;
const PROP_CRTC_MODE_ID: u32 = 2;
const PROP_CRTC_OUTFENCE: u32 = 3;
const PROP_CRTC_PLANE_MASK: u32 = 4;
const PROP_CONN_DPMS: u32 = 5;
const PROP_CONN_CRTC_ID: u32 = 6;
const PROP_CONN_BRIGHTNESS: u32 = 7;
const PROP_PLANE_TYPE: u32 = 8;
const PROP_PLANE_FB_ID: u32 = 9;
const PROP_PLANE_CRTC_ID: u32 = 10;
const PROP_PLANE_CRTC_X: u32 = 11;
const PROP_PLANE_CRTC_Y: u32 = 12;
const PROP_PLANE_CRTC_W: u32 = 13;
const PROP_PLANE_CRTC_H: u32 = 14;
const PROP_PLANE_SRC_X: u32 = 15;
const PROP_PLANE_SRC_Y: u32 = 16;
const PROP_PLANE_SRC_W: u32 = 17;
const PROP_PLANE_SRC_H: u32 = 18;
const PROP_PLANE_ZPOS: u32 = 19;
const PROP_PLANE_IN_FENCE_FD: u32 = 20;
const PROP_PLANE_NAME: u32 = 21;

/// DRM_MODE_PROP_* flag bits (uapi).
const DRM_MODE_PROP_RANGE: u32 = 1 << 1;
const DRM_MODE_PROP_ENUM: u32 = 1 << 3;
const DRM_MODE_PROP_BLOB: u32 = 1 << 4;
const DRM_MODE_PROP_BITMASK: u32 = 1 << 5;
const DRM_MODE_PROP_SIGNED_RANGE: u32 = 2 << 6;
const DRM_MODE_PROP_ATOMIC: u32 = 0x8000_0000;
const DRM_MODE_PROP_IMMUTABLE: u32 = 1 << 2;


const U64_MAX: u64 = u64::MAX;
const I64_MIN: u64 = i64::MIN as u64;

/// Property set for one object: (prop, type-tag) is implicit per list.
fn crtc_props() -> [PropDesc; 4] {
    [
        PropDesc {
            prop_id: PROP_CRTC_ACTIVE,
            name: "ACTIVE",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 1,
            values: &[0, 1],
        },
        PropDesc {
            prop_id: PROP_CRTC_MODE_ID,
            name: "MODE_ID",
            flags: DRM_MODE_PROP_BLOB | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[],
        },
        PropDesc {
            prop_id: PROP_CRTC_OUTFENCE,
            name: "OUT_FENCE_PTR",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_CRTC_PLANE_MASK,
            name: "PLANE_MASK",
            flags: DRM_MODE_PROP_BITMASK | DRM_MODE_PROP_ATOMIC,
            // Bit positions mirror the rockchip plane-mask the composer
            // parses (DrmCrtc::Init): enum "Smart0" at bit 12 sets
            // mPlaneMask |= SMART0_MASK, which then matches a plane
            // whose NAME enum is "Smart0-win0".
            enums: &[
                ("Cluster0", 0),
                ("Cluster1", 2),
                ("Esmart0", 4),
                ("Esmart1", 8),
                ("Smart0", 12),
                ("Smart1", 16),
            ],
            value: (1 << 0) | (1 << 2) | (1 << 4) | (1 << 8) | (1 << 12) | (1 << 16),
            values: &[],
        },
    ]
}

fn connector_props() -> [PropDesc; 3] {
    [
        PropDesc {
            prop_id: PROP_CONN_DPMS,
            name: "DPMS",
            flags: DRM_MODE_PROP_ENUM | DRM_MODE_PROP_ATOMIC,
            enums: &[("On", 0), ("Standby", 1), ("Suspend", 2), ("Off", 3)],
            value: 0,
            values: &[0, 1, 2, 3],
        },
        PropDesc {
            prop_id: PROP_CONN_CRTC_ID,
            name: "CRTC_ID",
            flags: DRM_MODE_PROP_ATOMIC, // OBJECT type carries no flag bits
            enums: &[],
            value: OBJ_CRTC as u64,
            values: &[],
        },
        PropDesc {
            prop_id: PROP_CONN_BRIGHTNESS,
            name: "brightness",
            flags: DRM_MODE_PROP_RANGE,
            enums: &[],
            value: 255,
            values: &[0, 255],
        },
    ]
}

fn plane_props() -> [PropDesc; 13] {
    [
        PropDesc {
            prop_id: PROP_PLANE_TYPE,
            name: "type",
            flags: DRM_MODE_PROP_ENUM | DRM_MODE_PROP_IMMUTABLE,
            enums: &[("Overlay", 0), ("Primary", 1), ("Cursor", 2)],
            value: 1, // DRM_PLANE_TYPE_PRIMARY
            values: &[0, 1, 2],
        },
        PropDesc {
            prop_id: PROP_PLANE_FB_ID,
            name: "FB_ID",
            flags: DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[],
        },
        PropDesc {
            prop_id: PROP_PLANE_CRTC_ID,
            name: "CRTC_ID",
            flags: DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[],
        },
        PropDesc {
            prop_id: PROP_PLANE_CRTC_X,
            name: "CRTC_X",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[I64_MIN, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_CRTC_Y,
            name: "CRTC_Y",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[I64_MIN, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_CRTC_W,
            name: "CRTC_W",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[I64_MIN, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_CRTC_H,
            name: "CRTC_H",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[I64_MIN, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_SRC_X,
            name: "SRC_X",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_SRC_Y,
            name: "SRC_Y",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_SRC_W,
            name: "SRC_W",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_SRC_H,
            name: "SRC_H",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, U64_MAX],
        },
        PropDesc {
            prop_id: PROP_PLANE_ZPOS,
            name: "zpos",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: 0,
            values: &[0, 255],
        },
        PropDesc {
            prop_id: PROP_PLANE_IN_FENCE_FD,
            name: "IN_FENCE_FD",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            enums: &[],
            value: u64::MAX, // -1
            values: &[I64_MIN, 0x7fff_ffff],
        },
    ]
}

/// The plane "NAME" enum property (rockchip plane-name style; queried
/// separately because it rides on plane_props' tail).
fn plane_name_prop() -> PropDesc {
    PropDesc {
        prop_id: PROP_PLANE_NAME,
        name: "NAME",
        flags: DRM_MODE_PROP_ENUM,
        // "Smart0-win0" (value 12) is the win-type the composer's
        // planeTypeNames table maps to DRM_PLANE_TYPE_SMART0_WIN0, which
        // intersects the CRTC PLANE_MASK (Smart0 -> 0xf000) nonzero.
        enums: &[
            ("Cluster0-win0", 0),
            ("Cluster0-win1", 1),
            ("Cluster1-win0", 2),
            ("Cluster1-win1", 3),
            ("Esmart0-win0", 4),
            ("Esmart0-win1", 5),
            ("Esmart0-win2", 6),
            ("Esmart0-win3", 7),
            ("Esmart1-win0", 8),
            ("Esmart1-win1", 9),
            ("Esmart1-win2", 10),
            ("Esmart1-win3", 11),
            ("Smart0-win0", 12),
            ("Smart0-win1", 13),
            ("Smart0-win2", 14),
            ("Smart0-win3", 15),
            ("Smart1-win0", 16),
            ("Smart1-win1", 17),
            ("Smart1-win2", 18),
            ("Smart1-win3", 19),
        ],
        value: 12, // Smart0-win0
        values: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19],
    }
}

// ============================================================================
// Fixed mode (matches the virtio-gpu scanout QEMU was started with)
// ============================================================================


const DRM_MODE_TYPE_PREFERRED: u32 = 1 << 3;
const DRM_MODE_TYPE_DRIVER: u32 = 1 << 6;
const DRM_MODE_FLAG_NHSYNC: u32 = 1 << 1;
const DRM_MODE_FLAG_NVSYNC: u32 = 1 << 2;
/// DRM_MODE_CONNECTOR_DSI.
const DRM_MODE_CONNECTOR_DSI: u32 = 16;
/// DRM_MODE_ENCODER_DSI.
const DRM_MODE_ENCODER_DSI: u32 = 6;
/// DRM_MODE_CONNECTED.
const DRM_MODE_CONNECTED: u32 = 1;

fn fixed_mode_name(w: u32, h: u32) -> [u8; 32] {
    let mut name = [0u8; 32];
    let label = alloc::format!("{}x{}", w, h);
    let n = label.len().min(31);
    name[..n].copy_from_slice(&label.as_bytes()[..n]);
    name
}

fn current_mode() -> DrmModeInfo {
    let (w, h) = match crate::drivers::hotpath::get_framebuffer_info() {
        Some(i) => (i.width, i.height),
        None => (720, 1280),
    };
    // 60 Hz CVT-ish timing: htotal = w + 80, vtotal = h + 80.
    let htotal = w + 80;
    let vtotal = h + 80;
    let clock = (htotal as u64) * (vtotal as u64) * 60 / 1000;
    DrmModeInfo {
        name: fixed_mode_name(w, h),
        clock: clock as u32,
        hdisplay: w as u16,
        hsync_start: (w + 20) as u16,
        hsync_end: (w + 60) as u16,
        htotal: htotal as u16,
        hskew: 0,
        vdisplay: h as u16,
        vsync_start: (h + 20) as u16,
        vsync_end: (h + 50) as u16,
        vtotal: vtotal as u16,
        vscan: 0,
        vrefresh: 60,
        flags: DRM_MODE_FLAG_NHSYNC | DRM_MODE_FLAG_NVSYNC,
        type_: DRM_MODE_TYPE_PREFERRED | DRM_MODE_TYPE_DRIVER,
    }
}

// ============================================================================
// ioctl numbers (Linux uapi encodings, x86_64/riscv64 LP64 sizes)
// ============================================================================

const fn ioc(dir: u32, nr: u32, size: u32) -> u32 {
    (dir << 30) | (size << 16) | (0x64 << 8) | nr
}

const DRM_IOCTL_VERSION: u32 = ioc(3, 0x00, 64);
const DRM_IOCTL_SET_VERSION: u32 = ioc(3, 0x07, 16);
const DRM_IOCTL_GET_CAP: u32 = ioc(3, 0x0c, 16);
const DRM_IOCTL_SET_CLIENT_CAP: u32 = ioc(1, 0x0d, 16);
const DRM_IOCTL_SET_MASTER: u32 = ioc(0, 0x1e, 0);
const DRM_IOCTL_DROP_MASTER: u32 = ioc(0, 0x1f, 0);
const DRM_IOCTL_AUTH_MAGIC: u32 = ioc(1, 0x11, 4);
const DRM_IOCTL_GEM_CLOSE: u32 = ioc(1, 0x09, 4);
/// Upstream libdrm's drm_gem_close passes drm_gem_close (8 bytes:
/// handle + pad), not a bare u32 — accept both encodings (the
/// 4-byte one predates the ABI audit; real clients send 8).
const DRM_IOCTL_GEM_CLOSE_8B: u32 = ioc(1, 0x09, 8);
const DRM_IOCTL_PRIME_HANDLE_TO_FD: u32 = ioc(3, 0x2d, 16);
/// Same ioctl with the CORRECT upstream size: struct drm_prime_handle
/// is 12 bytes (handle, flags, fd — no pad). The 16-byte constant above
/// was baked from a wrong layout, so OH libdrm's real
/// DRM_IOWR(0x2d, sizeof 12) fell through to ENOTTY — the gralloc
/// DrmAllocator logged "can not get fd from prime handle errno: 25"
/// after a successful CREATE_DUMB and every AllocMem failed, killing
/// the first real frame (r12). Match BOTH encodings.
/// Same ioctl with the CORRECT upstream size: struct drm_prime_handle
/// is 12 bytes (handle, flags, fd — no pad). The 16-byte constant above
/// was baked from a wrong layout, so OH libdrm's real
/// DRM_IOWR(0x2d, sizeof 12) fell through to ENOTTY — the gralloc
/// DrmAllocator logged "can not get fd from prime handle errno: 25"
/// after a successful CREATE_DUMB and every AllocMem failed, killing
/// the first real frame (r12). Match BOTH encodings.
const DRM_IOCTL_PRIME_HANDLE_TO_FD_12: u32 = ioc(3, 0x2d, 12);
const DRM_IOCTL_PRIME_FD_TO_HANDLE: u32 = ioc(3, 0x2e, 16);
/// Correct-size alias (see DRM_IOCTL_PRIME_HANDLE_TO_FD_12).
const DRM_IOCTL_PRIME_FD_TO_HANDLE_12: u32 = ioc(3, 0x2e, 12);

/// True when `request` is a PRIME_FD_TO_HANDLE ioctl number (either
/// size encoding) — io.rs uses this to catch imports aimed at a fd that
/// is not a DRM node (cross-process fd-number leaks fail exactly there).
pub fn is_prime_import_request(request: u32) -> bool {
    request == DRM_IOCTL_PRIME_FD_TO_HANDLE || request == DRM_IOCTL_PRIME_FD_TO_HANDLE_12
}

/// Bring-up oracle (compose-content): a prime import attempt that never
/// reaches the DRM dispatch means the caller's fd is NOT a DRM file —
/// the classic cross-process fd-number leak (a BufferHandle parcelled
/// without binder FDA fixup carries the SENDER's fd number). The gallium
/// swrast import path fails silently then ("eglCreateImageKHR failed,
/// error EGL_SUCCESS", zero kernel ioctls). io.rs calls this when it
/// sees a prime-import request number on a non-DRM fd (or a dead fd).
pub fn trace_foreign_prime_ioctl(request: u32, fd: i32, err: i32) {
    // Hunt tap (known-gap 4): rux:ohhunt (retired debug=ohhunt) gates it off the default path.
    if !crate::trace::EVENT_OHHUNT.armed() {
        return;
    }
    if !is_prime_import_request(request) {
        return;
    }
    static FOREIGN_N: AtomicU32 = AtomicU32::new(0);
    let n = FOREIGN_N.fetch_add(1, Ordering::Relaxed);
    if n < 40 {
        crate::pr_warn!(
            "drm: prime-import on non-drm fd={} (err {}) pid={} #{}",
            fd,
            err,
            crate::process::hotpath::current_pid(),
            n
        );
    }
}
const DRM_IOCTL_WAIT_VBLANK: u32 = ioc(3, 0x3a, 24);
const DRM_IOCTL_MODE_GETRESOURCES: u32 = ioc(3, 0xa0, 64);
// Sizes verified against the IMAGE's libdrm.so (constants embedded in
// drmModeGetCrtc/Encoder: 0xc06864a1 = 104, 0xc01464a6 = 20) and the
// host kernel uapi headers: struct drm_mode_crtc is 104 bytes
// (modeinfo is 68), drm_mode_get_encoder is 20, GEM_CLOSE carries a
// single u32. The wrong sizes (112/24/8) changed the encoded request
// number, so every real-libdrm call landed in the ENOTTY arm and the
// composer saw "can not get crtc/encoder".
const DRM_IOCTL_MODE_GETCRTC: u32 = ioc(3, 0xa1, 104);
const DRM_IOCTL_MODE_SETCRTC: u32 = ioc(3, 0xa2, 104);
const DRM_IOCTL_MODE_GETENCODER: u32 = ioc(3, 0xa6, 20);
const DRM_IOCTL_MODE_GETCONNECTOR: u32 = ioc(3, 0xa7, 80);
const DRM_IOCTL_MODE_GETPROPERTY: u32 = ioc(3, 0xaa, 64);
const DRM_IOCTL_MODE_SETPROPERTY: u32 = ioc(3, 0xab, 16);
const DRM_IOCTL_MODE_GETPROPBLOB: u32 = ioc(3, 0xac, 16);
const DRM_IOCTL_MODE_ADDFB: u32 = ioc(3, 0xae, 28);
const DRM_IOCTL_MODE_RMFB: u32 = ioc(3, 0xaf, 4);
const DRM_IOCTL_MODE_PAGE_FLIP: u32 = ioc(3, 0xb0, 24);
// Upstream-libdrm encodings of the same two ioctls: libdrm builds
// MODE_RMFB with _IOR (dir=2) and MODE_PAGE_FLIP with _IOW (dir=1),
// while the table above used IOWR (dir=3) for both — a real libdrm
// client (drmModeRmFB / drmModePageFlip) would take ENOTTY (proven by
// the scanpaint painter: _IOW-encoded PAGE_FLIP returned ENOTTY until
// the painter switched to the dir-3 form). Accept both encodings; the
// dir-3 forms stay for whatever the baked-in image still sends.
const DRM_IOCTL_MODE_RMFB_LIBDRM: u32 = ioc(2, 0xaf, 4);
const DRM_IOCTL_MODE_PAGE_FLIP_LIBDRM: u32 = ioc(1, 0xb0, 24);
const DRM_IOCTL_MODE_CREATE_DUMB: u32 = ioc(3, 0xb2, 32);
const DRM_IOCTL_MODE_MAP_DUMB: u32 = ioc(3, 0xb3, 16);
const DRM_IOCTL_MODE_DESTROY_DUMB: u32 = ioc(3, 0xb4, 4);
const DRM_IOCTL_MODE_GETPLANERESOURCES: u32 = ioc(3, 0xb5, 16);
const DRM_IOCTL_MODE_GETPLANE: u32 = ioc(3, 0xb6, 32);
const DRM_IOCTL_MODE_OBJ_GETPROPERTIES: u32 = ioc(3, 0xb9, 32);
const DRM_IOCTL_MODE_ADDFB2: u32 = ioc(3, 0xb8, 104);
/// NOTE: the OH image ships a libdrm that still uses the PRE-4.0
/// drm_mode_atomic layout (objs_ptr/count_props_ptr/props_ptr/
/// prop_values_ptr/user_data, 56 bytes) — not the mainline count_props
/// layout. Both /dev nodes and this driver must speak that layout.
const DRM_IOCTL_MODE_ATOMIC: u32 = ioc(3, 0xbc, 56);
const DRM_IOCTL_MODE_CREATEPROPBLOB: u32 = ioc(3, 0xbd, 16);
const DRM_IOCTL_MODE_DESTROYPROPBLOB: u32 = ioc(3, 0xbe, 4);
/// DRM_IOCTL_VIRTGPU_GETPARAM (virtio-gpu driver-private, nr 0x43).
const DRM_IOCTL_VIRTGPU_GETPARAM: u32 = ioc(3, 0x43, 16);

/// DRM_CAP_* ids queried through DRM_IOCTL_GET_CAP.
const DRM_CAP_DUMB_BUFFER: u64 = 0x1;
const DRM_CAP_VBLANK_HIGH_CRTC: u64 = 0x2;
const DRM_CAP_PRIME: u64 = 0x5;
const DRM_CAP_CURSOR_WIDTH: u64 = 0x8;
const DRM_CAP_CURSOR_HEIGHT: u64 = 0x9;
/// DRM_PRIME_CAP_IMPORT | DRM_PRIME_CAP_EXPORT.
const DRM_PRIME_CAPS: u64 = 0x3;

/// DRM_CLIENT_CAP_UNIVERSAL_PLANES / _ATOMIC.
const DRM_CLIENT_CAP_UNIVERSAL_PLANES: u64 = 2;
const DRM_CLIENT_CAP_ATOMIC: u64 = 3;

/// DMA_BUF_IOCTL_SYNC (_IOW('b', 0, u64), issued on prime fds).
const DMA_BUF_IOCTL_SYNC: u32 = 0x4008_6200;

// ============================================================================
// GEM objects, framebuffers, blobs
// ============================================================================


// SAFETY: the object's mutable state is a single AtomicU32; kva/phys are
// stable for the object's lifetime.
unsafe impl Send for GemObject {}
unsafe impl Sync for GemObject {}

impl Drop for GemObject {
    fn drop(&mut self) {
        let res = self.virtio_res.load(Ordering::Acquire);
        if res != 0 {
            // Best effort: release the host-side resource. Never runs
            // under the DRM state lock (see drm_state discipline).
            let _ = crate::drivers::hotpath::with_gpu_device(|gpu| gpu.unref_resource(res));
        }
        // SAFETY: kva was allocated with exactly this layout in
        // create_dumb and never reallocated.
        unsafe { dealloc(self.kva, self.layout) };
    }
}


static DRM_STATE: Spinlock<DrmState> = Spinlock::new(DrmState {
    gems: BTreeMap::new(),
    fbs: BTreeMap::new(),
    blobs: BTreeMap::new(),
    scanout_res: 0,
});

static NEXT_GEM_ID: AtomicU32 = AtomicU32::new(1000);
static NEXT_FB_ID: AtomicU32 = AtomicU32::new(100);
static NEXT_BLOB_ID: AtomicU32 = AtomicU32::new(200);
static NEXT_VIRTIO_RES: AtomicU32 = AtomicU32::new(2); // 1 = fbdev fb

/// DRM fourcc -> virtio-gpu 2D format. Both describe the same memory
/// byte order; see the format table in virtio_cmd.rs.
fn virtio_format_for(fourcc: u32) -> Option<u32> {
    // Mirrors Linux virtio_gpu_translate_format (the fourcc char order
    // is MSB-first in the little-endian pixel word, the virtio enum
    // name likewise describes the pixel word MSB-first):
    //   XRGB8888 (X[31:24] R[23:16] G[15:8] B[7:0]) -> B8G8R8X8
    //   XBGR8888 (X[31:24] B[23:16] G[15:8] R[7:0]) -> R8G8B8X8
    // and the A-variants accordingly; the reversed-reading fourccs
    // (BGRX/BGRA) map to the mirrored constants.
    match fourcc {
        0x3432_5258 => Some(crate::drivers::hotpath::format::B8G8R8X8_UNORM), // XRGB8888
        0x3432_5241 => Some(crate::drivers::hotpath::format::B8G8R8A8_UNORM), // ARGB8888
        0x3432_4258 => Some(crate::drivers::hotpath::format::R8G8B8X8_UNORM), // XBGR8888
        0x3432_4241 => Some(crate::drivers::hotpath::format::R8G8B8A8_UNORM), // ABGR8888
        0x3432_5842 => Some(crate::drivers::hotpath::format::X8R8G8B8_UNORM), // BGRX8888
        0x3432_4742 => Some(crate::drivers::hotpath::format::A8R8G8B8_UNORM), // BGRA8888
        _ => None,
    }
}

// ============================================================================
// File operations (both DRM nodes share one ops table)
// ============================================================================

fn drmdev_read(_file: &crate::fs::file::File, _buf: &mut [u8]) -> isize {
    0 // EOF (no readable event stream yet)
}

fn drmdev_write(_file: &crate::fs::file::File, buf: &[u8]) -> isize {
    buf.len() as isize
}

/// File ops for /dev/dri/card0 and /dev/dri/renderD128.
pub static DRM_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps::builder()
    .read(drmdev_read)
    .write(drmdev_write)
    .build();

/// True when the File is one of the /dev/dri nodes.
pub fn is_drm_file(file: &crate::fs::file::File) -> bool {
    match file.get_ops() {
        Some(ops) => core::ptr::eq(ops as *const _, &DRM_OPS as *const _),
        None => false,
    }
}

// --- prime (dma-buf style) file ------------------------------------------------

fn prime_read(_file: &crate::fs::file::File, _buf: &mut [u8]) -> isize {
    -25 // ENOTTY: dma-buf is mmap/DMABUF_IOCTL_SYNC only
}

fn prime_write(_file: &crate::fs::file::File, _buf: &[u8]) -> isize {
    -25
}

/// Close: drop the Arc<GemObject> the export stashed in private_data.
fn prime_close(file: &crate::fs::file::File) -> i32 {
    // private_data was installed by prime_handle_to_fd as
    // Box<Arc<GemObject>>; reconstruct and drop it once here.
    if let Some(ptr) = file.private_data_ptr() {
        file.clear_private_data();
        // SAFETY: the Box<Arc<GemObject>> the export stored (sole ref).
        drop(unsafe { alloc::boxed::Box::from_raw(ptr as *mut Arc<GemObject>) });
    }
    0
}

/// lseek on a prime fd — Linux dma-buf parity (dma_buf_llseek): the
/// file position ranges over the buffer's bytes and SEEK_END yields the
/// buffer SIZE. The vendor kms-dri software winsys sizes every imported
/// gralloc buffer with lseek(fd, 0, SEEK_END) and fails the import
/// SILENTLY on -1 — with no lseek op the EGLImage path returned NULL
/// with no error ("eglCreateImageKHR failed, error EGL_SUCCESS"), RS
/// rendered into a shadow target, and every flipped GEM stayed zeroed
/// (the black-screen root cause).
fn prime_lseek(file: &crate::fs::file::File, offset: isize, whence: i32) -> isize {
    let Some(gem) = prime_file_gem(file) else { return -9 };
    let size = gem.size as i64;
    let cur = file.get_pos() as i64;
    let base = match whence {
        0 => 0,        // SEEK_SET
        1 => cur,      // SEEK_CUR
        2 => size,     // SEEK_END: the buffer size (dma-buf semantics)
        _ => return -22, // EINVAL
    };
    let target = base + offset as i64;
    if target < 0 || target > size {
        return -22; // EINVAL (dma_buf_llseek)
    }
    let _ = file.set_pos(target as u64);
    target as isize
}

pub static PRIME_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps::builder()
    .read(prime_read)
    .write(prime_write)
    .lseek(prime_lseek)
    .close(prime_close)
    .build();

/// True when the File is an exported DRM prime fd.
pub fn is_prime_file(file: &crate::fs::file::File) -> bool {
    match file.get_ops() {
        Some(ops) => core::ptr::eq(ops as *const _, &PRIME_OPS as *const _),
        None => false,
    }
}

/// fstat(2) for DRM files. Linux parity the importers depend on:
/// the /dev/dri nodes stat as character devices (rdev 226:0/226:128),
/// and an exported dma-buf (prime) fd stats as a REGULAR file whose
/// st_size is the backing buffer's size — gbm, Mesa's image-from-dmabuf
/// and the OH composer's DrmGemBuffer size and validate their mappings
/// through exactly that stat. Every anonymous-file stat path returned
/// None for these files before, so fstat answered EBADF and the
/// software EGLImage import of gralloc buffers bailed silently
/// ("eglCreateImageKHR failed, error EGL_SUCCESS" — RS renders into a
/// shadow target and the flipped GEMs stay zeroed/black).
pub fn drm_file_stat(file: &crate::fs::file::File, stat: &mut crate::fs::Stat) -> Option<i32> {
    if is_prime_file(file) {
        let gem = prime_file_gem(file)?;
        stat.st_dev = 0;
        stat.st_ino = gem.id as u64;
        stat.st_nlink = 1;
        stat.st_uid = 0;
        stat.st_gid = 0;
        stat.st_rdev = 0;
        stat.st_size = gem.size as i64;
        stat.st_blocks = gem.size as i64 / 512;
        stat.st_blksize = 4096;
        stat.set_regular_file();
        stat.st_mode |= 0o666;
        return Some(0);
    }
    if is_drm_file(file) {
        stat.st_dev = 0;
        stat.st_ino = 0;
        stat.st_nlink = 1;
        stat.st_uid = 0;
        stat.st_gid = 0;
        // Userspace makedev encoding of 226:0 (both nodes share it; the
        // minor only distinguishes renderD128, which nothing stats on).
        stat.st_rdev = ((DRM_MAJOR as u64 & 0xfff) << 8) | 0;
        stat.st_size = 0;
        stat.st_blocks = 0;
        stat.st_blksize = 4096;
        stat.set_char_device();
        stat.st_mode |= 0o666;
        return Some(0);
    }
    None
}

/// GEM object backing a prime fd.
fn prime_file_gem(file: &crate::fs::file::File) -> Option<Arc<GemObject>> {
    // private_data holds a Box<Arc<GemObject>> installed at export (the
    // one audited deref is the clone).
    let ptr = file.private_data_ptr()?;
    // SAFETY: Box<Arc<GemObject>> stored by the export path.
    Some(unsafe { (*(ptr as *const Arc<GemObject>)).clone() })
}

// ============================================================================
// uaccess helpers
// ============================================================================

/// Copy `size` bytes from user `arg`, zero-padded/extended into a fixed
/// stack buffer of N bytes. Returns None on EFAULT.
fn copy_in<const N: usize>(arg: usize, size: usize) -> Option<[u8; N]> {
    let n = size.min(N);
    let mut buf = [0u8; N];
    if n == 0 {
        return Some(buf);
    }
    if !crate::arch::uaccess::access_ok(arg, n) {
        return None;
    }
    if crate::arch::uaccess_safe::copy_in(&mut buf[..n], arg) != 0 {
        return None;
    }
    Some(buf)
}

/// Copy `size` bytes of a fixed buffer out to user `arg`.
fn copy_out<const N: usize>(arg: usize, buf: &[u8; N], size: usize) -> bool {
    let n = size.min(N);
    if n == 0 {
        return true;
    }
    if !crate::arch::uaccess::access_ok(arg, n) {
        return false;
    }
    crate::arch::uaccess_safe::copy_out(arg, &buf[..n]) == 0
}

/// Byte-image view of a u32 id array for the copy_out path (the u32
/// arrays are plain ids; the byte relabeling is layout-only).
mod bytemod {
    pub fn u32_slice_as_bytes(v: &[u32]) -> &[u8] {
        // SAFETY: [u32] and its byte image have identical size/align
        // requirements; the relabel borrows v's own lifetime.
        unsafe { core::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 4) }
    }
}

fn rdu32(b: &[u8], off: usize) -> u32 {
    u32::from_ne_bytes(b[off..off + 4].try_into().unwrap())
}
fn wru32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}
fn rdu64(b: &[u8], off: usize) -> u64 {
    u64::from_ne_bytes(b[off..off + 8].try_into().unwrap())
}
fn wru64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_ne_bytes());
}

// ============================================================================
// DRM ioctl implementation
// ============================================================================

/// Handle a DRM ioctl on a /dev/dri node.
pub fn drm_ioctl(request: u32, arg: usize) -> i64 {
    let ret = drm_ioctl_inner(request, arg);
    trace_seq(request, ret);
    trace_ioctl(request, ret);
    ret
}

// ============================================================================
// Console tracing (bring-up): first use of each ioctl plus throttled
// counters, so the composer's DRM handshake is visible on serial (hilog
// never reaches the console on this image).
// ============================================================================

// ============================================================================
// Full-sequence trace: the Mesa/EGL/composer handshake is a few dozen
// ioctls at most; per-category counters cannot show WHERE it stops. Log
// the first 128 ioctls on any /dev/dri node verbatim (pid + request +
// ret), then keep logging up to 16 DISTINCT request numbers never seen
// before (Mesa probes several driver-private ioctls mid-sequence).
// ============================================================================

static SEQ_N: AtomicU32 = AtomicU32::new(0);
static SEQ_SEEN_REQS: [AtomicU32; 16] = [
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
];
static SEQ_SEEN_N: AtomicU32 = AtomicU32::new(0);

fn trace_seq(request: u32, ret: i64) {
    // 110th: rides the ohhunt gate — the full-sequence handshake trace
    // printed the first 128 drm ioctls (~123 lines pre-launcher) of
    // every perf boot.
    if !crate::debug::switches::oh_hunt_prints_on() {
        return;
    }
    let pid = crate::process::hotpath::current_pid();
    let n = SEQ_N.fetch_add(1, Ordering::Relaxed);
    if n < 128 {
        crate::pr_warn!("drm[{}] ioctl#{n} {:#x} -> {}", pid, request, ret);
        return;
    }
    // Past the window: only first sightings of a NEW request number —
    // an unseen probe is the next thing to implement.
    for slot in SEQ_SEEN_REQS.iter() {
        if slot.load(Ordering::Relaxed) == request {
            return;
        }
    }
    let idx = SEQ_SEEN_N.fetch_add(1, Ordering::Relaxed) as usize;
    if idx < SEQ_SEEN_REQS.len() {
        SEQ_SEEN_REQS[idx].store(request, Ordering::Relaxed);
        crate::pr_warn!("drm[{}] new-req {:#x} -> {} (after #{} ioctls)", pid, request, ret, n);
    }
}
static TRACE_COUNTS: [AtomicU32; 8] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
const T_RES: usize = 0; // GETRESOURCES-family (crtc/encoder/connector/plane/props)
const T_DUMB: usize = 1; // CREATE_DUMB
const T_EXPORT: usize = 2; // PRIME_HANDLE_TO_FD
const T_IMPORT: usize = 3; // PRIME_FD_TO_HANDLE
const T_FB: usize = 4; // ADDFB2
const T_ATOMIC: usize = 5; // ATOMIC
const T_VBLANK: usize = 6; // WAIT_VBLANK
const T_UNHANDLED: usize = 7; // ENOTTY answers

fn trace_ioctl(request: u32, ret: i64) {
    // 110th: the whole census (VERSION probes, first-ioctl lines, the
    // every-100th atomic/dumb milestones — ~130 lines per perf boot in
    // the render window) rides the ohhunt gate like the other drm hunt
    // taps; the counters only feed these prints.
    if !crate::debug::switches::oh_hunt_prints_on() {
        return;
    }
    let (slot, name) = match request {
        DRM_IOCTL_VERSION => {
            // VERSION is every DRM client's first ioctl — an effective
            // "node open" marker. First 8 callers with pid.
            static VTRACE: AtomicU32 = AtomicU32::new(0);
            let v = VTRACE.fetch_add(1, Ordering::Relaxed);
            if v < 8 {
                crate::pr_warn!(
                    "drm: VERSION probe #{} pid={} (ret {})",
                    v,
                    crate::process::hotpath::current_pid(),
                    ret
                );
            }
            return;
        }
        DRM_IOCTL_MODE_GETRESOURCES
        | DRM_IOCTL_MODE_GETCRTC
        | DRM_IOCTL_MODE_GETENCODER
        | DRM_IOCTL_MODE_GETCONNECTOR
        | DRM_IOCTL_MODE_GETPROPERTY
        | DRM_IOCTL_MODE_OBJ_GETPROPERTIES
        | DRM_IOCTL_MODE_GETPLANERESOURCES
        | DRM_IOCTL_MODE_GETPLANE
        | DRM_IOCTL_MODE_CREATEPROPBLOB => (T_RES, "enum"),
        DRM_IOCTL_MODE_CREATE_DUMB => (T_DUMB, "create_dumb"),
        DRM_IOCTL_PRIME_HANDLE_TO_FD | DRM_IOCTL_PRIME_HANDLE_TO_FD_12 => (T_EXPORT, "prime_export"),
        DRM_IOCTL_PRIME_FD_TO_HANDLE | DRM_IOCTL_PRIME_FD_TO_HANDLE_12 => (T_IMPORT, "prime_import"),
        DRM_IOCTL_MODE_ADDFB2 | DRM_IOCTL_MODE_ADDFB => (T_FB, "addfb"),
        DRM_IOCTL_MODE_ATOMIC | DRM_IOCTL_MODE_PAGE_FLIP => (T_ATOMIC, "atomic"),
        DRM_IOCTL_WAIT_VBLANK => (T_VBLANK, "wait_vblank"),
        DRM_IOCTL_MODE_SETCRTC | DRM_IOCTL_SET_CLIENT_CAP => return,
        _ if ret == -25 => (T_UNHANDLED, "ENOTTY"),
        _ => return,
    };
    let n = TRACE_COUNTS[slot].fetch_add(1, Ordering::Relaxed);
    if n == 0 {
        if slot == T_UNHANDLED {
            // Include the raw request: the OH bring-up needs the exact
            // ioctl number userspace asked for (e.g. Mesa's gbm/EGL path
            // probing a driver-specific ioctl before falling back).
            crate::pr_warn!("drm: first ENOTTY request={:#x} (ret {})", request, ret);
        } else {
            crate::pr_warn!("drm: first {} (ret {})", name, ret);
        }
    } else if slot == T_ATOMIC && n % 100 == 0 {
        crate::pr_warn!("drm: atomic commits so far: {}", n + 1);
    } else if slot == T_DUMB && n % 50 == 0 {
        crate::pr_warn!("drm: dumb buffers so far: {}", n + 1);
    }
    if ret < 0 && ret != -25 && n < 8 {
        crate::pr_warn!("drm: {} failed ret={}", name, ret);
    }
}

fn drm_ioctl_inner(request: u32, arg: usize) -> i64 {
    match request {
        DRM_IOCTL_VERSION => drm_version(arg),
        DRM_IOCTL_SET_VERSION => 0, // accept whatever; nothing reads it back
        DRM_IOCTL_GET_CAP => drm_get_cap(arg),
        DRM_IOCTL_SET_CLIENT_CAP => drm_set_client_cap(arg),
        DRM_IOCTL_SET_MASTER | DRM_IOCTL_DROP_MASTER => 0, // single-client system
        DRM_IOCTL_AUTH_MAGIC => {
            // The IMAGE's libdrm (vendor libdisplay_composer_vendor's
            // chipset-sdk libdrm.so, drmIsMaster @0xd690) is OH-patched:
            // it probes with a single AUTH_MAGIC ioctl carrying magic 0
            // and reports master ONLY when the ioctl SUCCEEDS
            // (errno EINTR/EAGAIN retry, everything else -> not master).
            // Upstream libdrm instead expects EINVAL-as-master; answering
            // EINVAL here made DrmDevice::Init() fail at its
            // "is not master" check and the composer never enumerated
            // displays. Single-client DRM: accept every auth.
            0
        }
        DRM_IOCTL_GEM_CLOSE | DRM_IOCTL_GEM_CLOSE_8B => gem_close(rdu32_or(arg)),
        DRM_IOCTL_PRIME_HANDLE_TO_FD => prime_handle_to_fd(arg, 16),
        DRM_IOCTL_PRIME_HANDLE_TO_FD_12 => prime_handle_to_fd(arg, 12),
        DRM_IOCTL_PRIME_FD_TO_HANDLE => prime_fd_to_handle(arg, 16),
        DRM_IOCTL_PRIME_FD_TO_HANDLE_12 => prime_fd_to_handle(arg, 12),
        DRM_IOCTL_WAIT_VBLANK => wait_vblank(arg),
        DRM_IOCTL_MODE_GETRESOURCES => get_resources(arg),
        DRM_IOCTL_MODE_GETCRTC => get_crtc(arg),
        DRM_IOCTL_MODE_SETCRTC => set_crtc(arg),
        DRM_IOCTL_MODE_GETENCODER => get_encoder(arg),
        DRM_IOCTL_MODE_GETCONNECTOR => get_connector(arg),
        DRM_IOCTL_MODE_GETPROPERTY => get_property(arg),
        DRM_IOCTL_MODE_OBJ_GETPROPERTIES => obj_get_properties(arg),
        DRM_IOCTL_MODE_SETPROPERTY => 0, // DPMS et al: accept
        DRM_IOCTL_MODE_GETPROPBLOB => get_prop_blob(arg),
        DRM_IOCTL_MODE_ADDFB => addfb(arg),
        DRM_IOCTL_MODE_ADDFB2 => addfb2(arg),
        DRM_IOCTL_MODE_RMFB | DRM_IOCTL_MODE_RMFB_LIBDRM => rmfb(rdu32_or(arg)),
        DRM_IOCTL_MODE_PAGE_FLIP | DRM_IOCTL_MODE_PAGE_FLIP_LIBDRM => page_flip(arg),
        DRM_IOCTL_MODE_CREATE_DUMB => create_dumb(arg),
        DRM_IOCTL_MODE_MAP_DUMB => map_dumb(arg),
        DRM_IOCTL_MODE_DESTROY_DUMB => gem_close(rdu32_or(arg)),
        DRM_IOCTL_MODE_GETPLANERESOURCES => get_plane_resources(arg),
        DRM_IOCTL_MODE_GETPLANE => get_plane(arg),
        DRM_IOCTL_MODE_ATOMIC => mode_atomic(arg),
        DRM_IOCTL_MODE_CREATEPROPBLOB => create_prop_blob(arg),
        DRM_IOCTL_MODE_DESTROYPROPBLOB => destroy_prop_blob(rdu32_or(arg)),
        DRM_IOCTL_VIRTGPU_GETPARAM => virtgpu_getparam(arg),
        _ => -25, // ENOTTY
    }
}

/// DRM_IOCTL_VIRTGPU_GETPARAM (0xc0106443) — the first ioctl Mesa's
/// virtio_gpu dri driver issues after the loader picks it via the
/// DRM_IOCTL_VERSION name. The QEMU device here has no virgl/3D, so
/// Linux reports 0 for every parameter and Mesa's virtio_gpu screen
/// create fails cleanly, which is what routes gbm/EGL onto the
/// kms_swrast (llvmpipe + dumb-buffer) fallback — the reference
/// kernel's software-rendering path. Answering ENOTTY instead made
/// gbm's driver attempt die without the swrast retry (boot u7:
/// ENOTTY -> RS munmaps its 57MB Mesa image -> one card0 re-open ->
/// silence).
///
/// struct drm_virtgpu_getparam { __u64 param; __u64 value; } — param in,
/// value out, 0 on success; unknown param -> EINVAL (Linux).
fn virtgpu_getparam(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let param = rdu64(&b, 0);
    // VIRTGPU_PARAM_* (include/uapi/drm/virtgpu_drm.h). Linux's
    // virtio_gpu_getparam answers every param in the enum with 0 on a
    // 2D-only device (no virgl/crossven): 1 3D_FEATURES, 2 CAPSET_MIN_,
    // 3 RESOURCE_BASE_BUSY, 4 HOST_VISIBLE, 5 CROSS_DEVICE, 6 CONTEXT_
    // INIT, 7 SUPPORTED_CAPSET_IDS (mask 0), 8 CAPSET_MAX_FIXUPS, 9
    // BLOB_MEM, 10 HOST_VISIBLE_DEBUG, 11 CONTEXT_GROWABLE, 12 CAPSET_
    // MASK. u13 trace: Mesa probes 1..6 then 7; the old list lacked 7
    // and answered EINVAL where Linux says "param known, value 0".
    if !(1..=12).contains(&param) {
        return -22; // EINVAL: unknown param number
    }
    wru64(&mut b, 8, 0);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

/// Read a lone u32 ioctl argument (RMFB, GEM_CLOSE, DESTROY_DUMB...).
fn rdu32_or(arg: usize) -> u32 {
    match crate::arch::uaccess::access_ok(arg, 4)
        .then(|| crate::arch::uaccess_safe::read_user::<u32>(arg))
    {
        Some(Some(v)) => v,
        _ => 0,
    }
}

fn drm_version(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<64>(arg, 64) else { return -14 };
    // Driver name must be "virtio_gpu": libdrm/Mesa's loader maps the
    // kernel driver name onto the image's virtio_gpu_dri.so (with a
    // kms_swrast fallback) — this is what the reference Linux 6.6
    // virtio-gpu reports and what gbm's "failed to get driver name for
    // fd" path matches on. Anything else leaves EGL without a driver.
    let name = b"virtio_gpu\0";
    let date = b"20261014\0";
    let desc = b"virtio-gpu DRM/KMS (rux)\0";

    let name_len = rdu64(&b, 16) as usize;
    let name_ptr = rdu64(&b, 24) as usize;
    let date_len = rdu64(&b, 32) as usize;
    let date_ptr = rdu64(&b, 40) as usize;
    let desc_len = rdu64(&b, 48) as usize;
    let desc_ptr = rdu64(&b, 56) as usize;

    wru32(&mut b, 0, 1); // major
    wru32(&mut b, 4, 4); // minor
    wru32(&mut b, 8, 0); // patch

    // copy strings (truncated to the caller's buffers)
    if name_ptr != 0 && name_len > 0 {
        let n = name.len().min(name_len);
        if !crate::arch::uaccess::access_ok(name_ptr, n) {
            return -14;
        }
        // SAFETY: name_ptr validated.
        if crate::arch::uaccess_safe::copy_out(name_ptr, &name[..n]) != 0 {
            return -14;
        }
    }
    if date_ptr != 0 && date_len > 0 {
        let n = date.len().min(date_len);
        if !crate::arch::uaccess::access_ok(date_ptr, n) {
            return -14;
        }
        if crate::arch::uaccess_safe::copy_out(date_ptr, &date[..n]) != 0 {
            return -14;
        }
    }
    if desc_ptr != 0 && desc_len > 0 {
        let n = desc.len().min(desc_len);
        if !crate::arch::uaccess::access_ok(desc_ptr, n) {
            return -14;
        }
        if crate::arch::uaccess_safe::copy_out(desc_ptr, &desc[..n]) != 0 {
            return -14;
        }
    }
    wru64(&mut b, 16, name.len() as u64);
    wru64(&mut b, 32, date.len() as u64);
    wru64(&mut b, 48, desc.len() as u64);

    if !copy_out(arg, &b, 64) {
        return -14;
    }
    0
}

fn drm_get_cap(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let cap = rdu64(&b, 0);
    let val = match cap {
        DRM_CAP_DUMB_BUFFER | DRM_CAP_VBLANK_HIGH_CRTC => 1,
        DRM_CAP_PRIME => DRM_PRIME_CAPS,
        DRM_CAP_CURSOR_WIDTH | DRM_CAP_CURSOR_HEIGHT => 64,
        _ => return -22, // EINVAL: unknown capability
    };
    wru64(&mut b, 8, val);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

fn drm_set_client_cap(arg: usize) -> i64 {
    let Some(b) = copy_in::<16>(arg, 16) else { return -14 };
    let cap = rdu64(&b, 0);
    match cap {
        DRM_CLIENT_CAP_UNIVERSAL_PLANES | DRM_CLIENT_CAP_ATOMIC => 0,
        _ => -22, // EINVAL (Linux returns EOPNOTSUPP for unknown caps)
    }
}

// --- resources ------------------------------------------------------------

/// GETRESOURCES: one CRTC, one encoder, one connector (no fbs listed).
fn get_resources(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<64>(arg, 64) else { return -14 };
    let crtcs: [u32; 1] = [OBJ_CRTC];
    let connectors: [u32; 1] = [OBJ_CONNECTOR];
    let encoders: [u32; 1] = [OBJ_ENCODER];

    let crtc_ptr = rdu64(&b, 8) as usize;
    let conn_ptr = rdu64(&b, 16) as usize;
    let enc_ptr = rdu64(&b, 24) as usize;
    let n_crtc = rdu32(&b, 36);
    let n_conn = rdu32(&b, 40);
    let n_enc = rdu32(&b, 44);

    if n_crtc > 0 && crtc_ptr != 0 {
        let n = (n_crtc as usize).min(crtcs.len());
        if !crate::arch::uaccess::access_ok(crtc_ptr, n * 4) {
            return -14;
        }
        if crate::arch::uaccess_safe::copy_out(
            crtc_ptr,
            crate::safety::bytes::bytes_of(&crtcs)[..n * 4].as_ref(),
        ) != 0 {
            return -14;
        }
    }
    if n_conn > 0 && conn_ptr != 0 {
        let n = (n_conn as usize).min(connectors.len());
        if !crate::arch::uaccess::access_ok(conn_ptr, n * 4) {
            return -14;
        }
        let conn_bytes: &[u8] = bytemod::u32_slice_as_bytes(&connectors[..n]);
        if crate::arch::uaccess_safe::copy_out(conn_ptr, conn_bytes) != 0 {
            return -14;
        }
    }
    if n_enc > 0 && enc_ptr != 0 {
        let n = (n_enc as usize).min(encoders.len());
        if !crate::arch::uaccess::access_ok(enc_ptr, n * 4) {
            return -14;
        }
        let enc_bytes: &[u8] = bytemod::u32_slice_as_bytes(&encoders[..n]);
        if crate::arch::uaccess_safe::copy_out(enc_ptr, enc_bytes) != 0 {
            return -14;
        }
    }

    wru32(&mut b, 32, 0); // count_fbs
    wru32(&mut b, 36, crtcs.len() as u32);
    wru32(&mut b, 40, connectors.len() as u32);
    wru32(&mut b, 44, encoders.len() as u32);
    wru32(&mut b, 48, 0); // min_width
    wru32(&mut b, 52, 8192); // max_width
    wru32(&mut b, 56, 0); // min_height
    wru32(&mut b, 60, 8192); // max_height
    if !copy_out(arg, &b, 64) {
        return -14;
    }
    0
}

/// GETCRTC: report the fixed mode, no fb bound initially.
fn get_crtc(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<104>(arg, 104) else { return -14 };
    let mode = current_mode();
    wru32(&mut b, 12, OBJ_CRTC); // crtc_id
    wru32(&mut b, 16, 0); // fb_id
    wru32(&mut b, 28, 0); // gamma_size
    wru32(&mut b, 32, 1); // mode_valid
    let _ = crate::safety::bytes::write_at(&mut b, 36, mode);
    if !copy_out(arg, &b, 104) {
        return -14;
    }
    0
}

/// SETCRTC: legacy modeset — accept the fixed mode, reject others.
fn set_crtc(arg: usize) -> i64 {
    let Some(b) = copy_in::<104>(arg, 104) else { return -14 };
    let mode_valid = rdu32(&b, 32);
    if mode_valid == 0 {
        return 0; // dpms-off style disable: accept
    }
    let req_h = u16::from_ne_bytes(b[36 + 4..36 + 6].try_into().unwrap()) as u32;
    // drm_mode_modeinfo field order (uapi): clock, hdisplay, hsync_start,
    // hsync_end, htotal, hskew, vdisplay... — vdisplay is +14, NOT +10
    // (+10 is htotal). Reading htotal here rejected every non-square
    // native mode with EINVAL: gdm's Xorg (1280x800, vdisplay 800 vs
    // htotal 1360) died in "modeset(0): failed to set mode" and the
    // greeter retry-looped 11 times per attempt (gdm-hunt, 2026-11-16).
    let req_v = u16::from_ne_bytes(b[36 + 14..36 + 16].try_into().unwrap()) as u32;
    let cur = current_mode();
    if req_h != cur.hdisplay as u32 || req_v != cur.vdisplay as u32 {
        return -22; // EINVAL: only the native mode is supported
    }
    0
}

fn get_encoder(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<20>(arg, 20) else { return -14 };
    wru32(&mut b, 0, OBJ_ENCODER);
    wru32(&mut b, 4, DRM_MODE_ENCODER_DSI);
    wru32(&mut b, 8, 0); // crtc_id (attached later by atomic commit)
    wru32(&mut b, 12, 1); // possible_crtcs: CRTC index 0
    wru32(&mut b, 16, 1); // possible_clones
    if !copy_out(arg, &b, 20) {
        return -14;
    }
    0
}

/// GETCONNECTOR: connected DSI panel with the fixed mode and the
/// possible encoder.
fn get_connector(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<80>(arg, 80) else { return -14 };
    let enc_ptr = rdu64(&b, 0) as usize;
    let modes_ptr = rdu64(&b, 8) as usize;
    let n_enc = rdu32(&b, 40);
    let n_modes = rdu32(&b, 32);

    let encoders: [u32; 1] = [OBJ_ENCODER];
    let mode = current_mode();

    if n_enc > 0 && enc_ptr != 0 {
        let n = (n_enc as usize).min(encoders.len());
        if !crate::arch::uaccess::access_ok(enc_ptr, n * 4) {
            return -14;
        }
        // SAFETY: ptr validated.
        let enc_bytes: &[u8] = bytemod::u32_slice_as_bytes(&encoders[..n]);
        if crate::arch::uaccess_safe::copy_out(enc_ptr, enc_bytes) != 0 {
            return -14;
        }
    }
    if n_modes > 0 && modes_ptr != 0 {
        let n = (n_modes as usize).min(1);
        const MODEINFO_SZ: usize = core::mem::size_of::<DrmModeInfo>();
        if !crate::arch::uaccess::access_ok(modes_ptr, n * MODEINFO_SZ) {
            return -14;
        }
        for i in 0..n {
            if crate::arch::uaccess_safe::copy_out_t(modes_ptr + i * MODEINFO_SZ, &mode) != 0 {
                return -14;
            }
        }
    }

    wru32(&mut b, 32, 1); // count_modes
    wru32(&mut b, 36, 0); // count_props
    wru32(&mut b, 40, 1); // count_encoders
    wru32(&mut b, 44, OBJ_ENCODER); // encoder_id
    wru32(&mut b, 48, OBJ_CONNECTOR); // connector_id
    wru32(&mut b, 52, DRM_MODE_CONNECTOR_DSI);
    wru32(&mut b, 56, 1); // connector_type_id
    wru32(&mut b, 60, DRM_MODE_CONNECTED);
    wru32(&mut b, 64, 72); // mm_width
    wru32(&mut b, 68, 128); // mm_height
    wru32(&mut b, 72, 0); // subpixel
    wru32(&mut b, 76, 0); // pad
    if !copy_out(arg, &b, 80) {
        return -14;
    }
    0
}

fn get_plane_resources(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let ptr = rdu64(&b, 0) as usize;
    let n = rdu32(&b, 8);
    let planes: [u32; 1] = [OBJ_PLANE];
    if n > 0 && ptr != 0 {
        if !crate::arch::uaccess::access_ok(ptr, 4) {
            return -14;
        }
        if crate::arch::uaccess_safe::copy_out(ptr, bytemod::u32_slice_as_bytes(&planes[..1])) != 0 {
            return -14;
        }
    }
    wru32(&mut b, 8, planes.len() as u32);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

/// Formats advertised on the primary plane (32-bit RGB fourccs).
/// XBGR8888 must be listed: the gralloc's FB-path allocation converts
/// PIXEL_FMT_BGRX_8888 to DRM_FORMAT_XBGR8888 (DrmAllocator::Allocate
/// forces that format for HBM_USE_MEM_FB buffers), and the composer
/// hands those buffers to ADDFB2 on the primary plane.
const PLANE_FORMATS: [u32; 5] = [
    0x3432_5241, // ARGB8888
    0x3432_5258, // XRGB8888
    0x3432_4241, // ABGR8888 (RGBA_8888)
    0x3432_4742, // BGRA8888
    0x3432_4258, // XBGR8888 (BGRX_8888, gralloc FB buffers)
];

fn get_plane(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<32>(arg, 32) else { return -14 };
    wru32(&mut b, 0, OBJ_PLANE); // plane_id
    wru32(&mut b, 4, 0); // crtc_id
    wru32(&mut b, 8, 0); // fb_id
    wru32(&mut b, 12, 1); // possible_crtcs: bit 0
    wru32(&mut b, 16, 0); // gamma_size
    wru32(&mut b, 20, PLANE_FORMATS.len() as u32);
    let ptr = rdu64(&b, 24) as usize;
    if ptr != 0 {
        if !crate::arch::uaccess::access_ok(ptr, PLANE_FORMATS.len() * 4) {
            return -14;
        }
        // SAFETY: ptr validated.
        if crate::arch::uaccess_safe::copy_out(ptr, bytemod::u32_slice_as_bytes(PLANE_FORMATS.as_slice())) != 0 {
            return -14;
        }
    }
    if !copy_out(arg, &b, 32) {
        return -14;
    }
    0
}

// --- properties ------------------------------------------------------------

/// Property lists per object.
fn props_for_obj(obj_id: u32) -> Option<Vec<PropDesc>> {
    if obj_id == OBJ_CRTC {
        Some(crtc_props().to_vec())
    } else if obj_id == OBJ_CONNECTOR {
        Some(connector_props().to_vec())
    } else if obj_id == OBJ_PLANE {
        let mut v = plane_props().to_vec();
        v.push(plane_name_prop());
        Some(v)
    } else {
        None
    }
}

/// OBJ_GETPROPERTIES: (prop_id, value) pairs for one object.
fn obj_get_properties(arg: usize) -> i64 {
    const N: usize = 32;
    let Some(b) = copy_in::<N>(arg, N) else { return -14 };
    let props_ptr = rdu64(&b, 0) as usize;
    let values_ptr = rdu64(&b, 8) as usize;
    let count = rdu32(&b, 16) as usize;
    let obj_id = rdu32(&b, 20);

    let Some(props) = props_for_obj(obj_id) else { return -22 };
    let total = props.len();
    let n = count.min(total);
    static OGP_TRACE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    if OGP_TRACE.fetch_add(1, Ordering::Relaxed) < 4 {
        crate::pr_info!(
            "drm: obj_getprops obj={} count={} n={} first_id={} props_ptr={:#x}",
            obj_id,
            count,
            n,
            props.first().map(|p| p.prop_id).unwrap_or(0),
            props_ptr
        );
    }
    if n > 0 && props_ptr != 0 && values_ptr != 0 {
        if !crate::arch::uaccess::access_ok(props_ptr, n * 4)
            || !crate::arch::uaccess::access_ok(values_ptr, n * 8)
        {
            return -14;
        }
        for i in 0..n {
            let pid = props[i].prop_id.to_ne_bytes();
            if !crate::arch::uaccess_safe::write_user(props_ptr + i * 4, props[i].prop_id) {
                return -14;
            }
            if !crate::arch::uaccess_safe::write_user(values_ptr + i * 8, props[i].value) {
                return -14;
            }
        }
    }
    // Update ONLY count_props (and echo obj_id) in the caller's struct —
    // libdrm's two-phase protocol re-issues the ioctl with the same
    // struct; overwriting the pointer fields with zeros (a fresh reply
    // buffer did exactly that) made the second call pass NULL arrays.
    let mut b = b;
    wru32(&mut b, 16, total as u32);
    wru32(&mut b, 20, obj_id);
    if !copy_out(arg, &b, N) {
        return -14;
    }
    0
}

/// GETPROPERTY: full description of one property.
fn get_property(arg: usize) -> i64 {
    const N: usize = 64;
    let Some(mut b) = copy_in::<N>(arg, N) else { return -14 };
    let prop_id = rdu32(&b, 16);
    let Some(props) = props_for_obj_by_prop(prop_id) else { return -22 };

    let values_ptr = rdu64(&b, 0) as usize;
    let enums_ptr = rdu64(&b, 8) as usize;

    let mut name = [0u8; 32];
    let nb = props.name.len().min(31);
    name[..nb].copy_from_slice(&props.name.as_bytes()[..nb]);

    let mut vals: Vec<u64> = Vec::new();
    if props.flags & (DRM_MODE_PROP_ENUM | DRM_MODE_PROP_BITMASK) != 0 {
        for (_, v) in props.enums {
            vals.push(*v);
        }
    } else {
        vals.extend_from_slice(props.values);
    }

    if !vals.is_empty() && values_ptr != 0 {
        if !crate::arch::uaccess::access_ok(values_ptr, vals.len() * 8) {
            return -14;
        }
        for (i, v) in vals.iter().enumerate() {
            if !crate::arch::uaccess_safe::write_user(values_ptr + i * 8, *v) {
                return -14;
            }
        }
    }
    if !props.enums.is_empty() && enums_ptr != 0 {
        if !crate::arch::uaccess::access_ok(enums_ptr, props.enums.len() * 40) {
            return -14;
        }
        for (i, (ename, ev)) in props.enums.iter().enumerate() {
            let mut rec = [0u8; 40];
            rec[..8].copy_from_slice(&ev.to_ne_bytes());
            let en = ename.len().min(31);
            rec[8..8 + en].copy_from_slice(&ename.as_bytes()[..en]);
            if crate::arch::uaccess_safe::copy_out(enums_ptr + i * 40, &rec) != 0 {
                return -14;
            }
        }
    }

    b[24..56].copy_from_slice(&name);
    wru32(&mut b, 56, vals.len() as u32);
    wru32(&mut b, 60, props.enums.len() as u32);
    wru32(&mut b, 20, props.flags);
    if !copy_out(arg, &b, N) {
        return -14;
    }
    0
}

/// Find a property descriptor by its id (search all three lists).
fn props_for_obj_by_prop(prop_id: u32) -> Option<PropDesc> {
    if let Some(p) = crtc_props().iter().find(|p| p.prop_id == prop_id) {
        return Some(clone_prop(p));
    }
    if let Some(p) = connector_props().iter().find(|p| p.prop_id == prop_id) {
        return Some(clone_prop(p));
    }
    if let Some(p) = plane_props().iter().find(|p| p.prop_id == prop_id) {
        return Some(clone_prop(p));
    }
    if plane_name_prop().prop_id == prop_id {
        return Some(plane_name_prop());
    }
    None
}

/// PropDesc carries &'static data — cheap to clone out of the arrays.
fn clone_prop(p: &PropDesc) -> PropDesc {
    PropDesc {
        prop_id: p.prop_id,
        name: p.name,
        flags: p.flags,
        enums: p.enums,
        value: p.value,
        values: p.values,
    }
}

// --- blobs ------------------------------------------------------------

fn create_prop_blob(arg: usize) -> i64 {
    let Some(b) = copy_in::<16>(arg, 16) else { return -14 };
    let data = rdu64(&b, 0) as usize;
    let len = rdu32(&b, 8) as usize;
    if len > 4096 {
        return -22;
    }
    let mut buf = vec![0u8; len];
    if len > 0 {
        if !crate::arch::uaccess::access_ok(data, len) {
            return -14;
        }
        // SAFETY: data validated; buf is len bytes.
        if crate::arch::uaccess_safe::copy_in(&mut buf[..len], data) != 0 {
            return -14;
        }
    }
    let id = NEXT_BLOB_ID.fetch_add(1, Ordering::Relaxed);
    let mut st = DRM_STATE.lock_irqsave();
    st.blobs.insert(id, buf);
    drop(st);

    let mut out = [0u8; 16];
    wru32(&mut out, 12, id);
    if !copy_out(arg, &out, 16) {
        return -14;
    }
    0
}

fn destroy_prop_blob(blob_id: u32) -> i64 {
    let mut st = DRM_STATE.lock_irqsave();
    match st.blobs.remove(&blob_id) {
        Some(_) => 0,
        None => -22,
    }
}

fn get_prop_blob(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let blob_id = rdu32(&b, 0);
    let data_ptr = rdu64(&b, 8) as usize;

    let len = {
        let st = DRM_STATE.lock_irqsave();
        match st.blobs.get(&blob_id) {
            Some(v) => v.len(),
            None => return -22,
        }
    };
    if data_ptr != 0 {
        let st = DRM_STATE.lock_irqsave();
        if let Some(v) = st.blobs.get(&blob_id) {
            if !crate::arch::uaccess::access_ok(data_ptr, v.len()) {
                drop(st);
                return -14;
            }
            let ret = crate::arch::uaccess_safe::copy_out(data_ptr, v.as_slice());
            drop(st);
            if ret != 0 {
                return -14;
            }
        }
    }
    wru32(&mut b, 4, len as u32);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

// --- GEM / dumb buffers ------------------------------------------------------------

fn create_dumb(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<32>(arg, 32) else { return -14 };
    let height = rdu32(&b, 0);
    let width = rdu32(&b, 4);
    let bpp = rdu32(&b, 8);
    if width == 0 || height == 0 || (bpp != 32 && bpp != 16 && bpp != 24) {
        return -22;
    }
    let pitch = width * (bpp / 8);
    let size = (pitch as u64 * height as u64) as usize;
    let layout = match Layout::from_size_align(size, 4096) {
        Ok(l) => l,
        Err(_) => return -22,
    };
    // SAFETY: layout has non-zero size; null-checked below.
    let kva = unsafe { alloc_zeroed(layout) };
    if kva.is_null() {
        return -12; // ENOMEM
    }
    let phys = crate::arch::mm::virt_to_phys(crate::arch::mm::VirtAddr::new(kva as u64)).0;
    let id = NEXT_GEM_ID.fetch_add(1, Ordering::Relaxed);
    let gem = Arc::new(GemObject {
        id,
        kva,
        phys,
        size,
        layout,
        virtio_res: AtomicU32::new(0),
    });
    let mut st = DRM_STATE.lock_irqsave();
    st.gems.insert(id, gem);
    drop(st);

    wru32(&mut b, 16, id); // handle
    wru32(&mut b, 20, pitch);
    wru64(&mut b, 24, size as u64);
    if !copy_out(arg, &b, 32) {
        return -14;
    }
    0
}

fn gem_close(handle: u32) -> i64 {
    if handle == 0 {
        return -22;
    }
    // Reference drm_gem_handle_create gives each open file its own
    // handle namespace: an exporter's GEM_CLOSE only removes ITS file's
    // handle and the object lives while any other file's handle (or the
    // prime import) keeps a reference. This kernel's table is global, so
    // the faithful proxy is reference-count-shaped: when another holder
    // still has a clone (RS's cached EGLImage resolves through this
    // entry), keep the entry — removing it unconditionally left the
    // compositor's next MAP_DUMB of handle 1003 with a dead key and
    // Mesa's llvmpipe assert-aborted render_service on every first
    // composition (oh-svc lane, boot oh-glm5). The entry goes away when
    // the caller's close is the last reference.
    enum CloseOutcome {
        Removed(Arc<GemObject>),
        SharedKeep,
        NotFound,
    }
    let outcome = {
        let mut st = DRM_STATE.lock_irqsave();
        match st.gems.get(&handle) {
            Some(gem) if Arc::strong_count(gem) > 1 => CloseOutcome::SharedKeep,
            Some(_) => CloseOutcome::Removed(st.gems.remove(&handle).unwrap()),
            None => CloseOutcome::NotFound,
        }
    };
    match outcome {
        // Drop OUTSIDE the state lock: GemObject::drop runs virtio
        // RESOURCE_UNREF, which polls the device for milliseconds.
        CloseOutcome::Removed(gem) => {
            drop(gem);
            0
        }
        // The caller's handle close SUCCEEDED — the object merely stays
        // for its other holders. The reference's per-file handle tables
        // make a close never fail because another file shares the
        // OBJECT; reporting -EINVAL here made Mesa's GEM_CLOSE on
        // shared buffers look failed (errno-shape divergence; the
        // shared-keep behavior itself is the oh-glm5 fix above).
        CloseOutcome::SharedKeep => 0,
        CloseOutcome::NotFound => -22,
    }
}

fn map_dumb(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<16>(arg, 16) else { return -14 };
    let handle = rdu32(&b, 0);
    let exists = DRM_STATE.lock_irqsave().gems.contains_key(&handle);
    if !exists {
        return -22;
    }
    // Fake offset: the mmap path on the DRM fd decodes the GEM id from
    // the low bits of the file offset.
    wru64(&mut b, 8, (handle as u64) << 12);
    if !copy_out(arg, &b, 16) {
        return -14;
    }
    0
}

// --- prime import/export ------------------------------------------------------------

/// PRIME_HANDLE_TO_FD: install an anonymous dma-buf-style file that
/// carries the GEM object (mmap + DMA_BUF_IOCTL_SYNC + FD_TO_HANDLE).
fn prime_handle_to_fd(arg: usize, size: usize) -> i64 {
    // Copy exactly the userspace struct size (12 upstream, 16 for the
    // old baked constant): a fixed 16 would read/write 4 bytes past a
    // 12-byte caller struct (stack corruption — same class as the
    // modeinfo 72/68 bug).
    let Some(mut b) = copy_in::<16>(arg, size) else { return -14 };
    let handle = rdu32(&b, 0);
    let flags = rdu32(&b, 4);
    let gem = {
        let st = DRM_STATE.lock_irqsave();
        st.gems.get(&handle).cloned()
    };
    let Some(gem) = gem else { return -22 };

    let fdt = match crate::sched::hotpath::get_current_fdtable() {
        Some(t) => t,
        None => return -9,
    };
    let Some(fd) = fdt.alloc_fd() else { return -24 }; // EMFILE
    let mut file_flags = crate::fs::file::FileFlags::O_RDWR;
    if flags & 0x80000 != 0 {
        // DRM_CLOEXEC
        file_flags |= crate::fs::file::FileFlags::O_CLOEXEC;
    }
    let file = Arc::new(crate::fs::file::File::new(crate::fs::file::FileFlags::new(
        file_flags,
    )));
    file.set_ops(&PRIME_OPS);
    // SAFETY: private_data is a Box<Arc<GemObject>>; prime_close frees it.
    file.set_private_data(alloc::boxed::Box::into_raw(alloc::boxed::Box::new(gem)) as *mut u8);
    if fdt.install_fd(fd, file).is_err() {
        return -24;
    }
    if flags & 0x80000 != 0 {
        fdt.set_fd_cloexec(fd, true);
    }

    wru32(&mut b, 8, fd as u32);
    if !copy_out(arg, &b, size) {
        // Best effort: the fd is installed; userspace will just fail.
        return 0;
    }
    0
}

/// PRIME_FD_TO_HANDLE: resolve a prime fd back to a GEM handle. The fd
/// may have been exported on the other node (or another process).
fn prime_fd_to_handle(arg: usize, size: usize) -> i64 {
    // See prime_handle_to_fd: copy exactly the caller's struct size.
    let Some(mut b) = copy_in::<16>(arg, size) else { return -14 };
    let prime_fd = rdu32(&b, 8) as i32;
    if prime_fd < 0 {
        return -22;
    }
    let Some(file) = (crate::fs::file::get_file_fd(prime_fd as usize)) else {
        static DEAD_N: AtomicU32 = AtomicU32::new(0);
        let n = DEAD_N.fetch_add(1, Ordering::Relaxed);
        if n < 20 {
            crate::pr_warn!(
                "drm: prime import fd={} DEAD (EBADF) pid={}",
                prime_fd,
                crate::process::hotpath::current_pid()
            );
        }
        return -9; // EBADF
    };
    if !is_prime_file(&file) {
        static NOTPRIME_N: AtomicU32 = AtomicU32::new(0);
        let n = NOTPRIME_N.fetch_add(1, Ordering::Relaxed);
        if n < 20 {
            crate::pr_warn!(
                "drm: prime import fd={} NOT a dma-buf (EINVAL) pid={}",
                prime_fd,
                crate::process::hotpath::current_pid()
            );
        }
        return -22; // EINVAL: not a dma-buf
    }
    let Some(gem) = prime_file_gem(&file) else { return -22 };

    // Compose-content probe: layer-texture imports carry the CLIENT's
    // finished frame (the producer drew before queueing) — non-zero
    // content here means the client rendered; zero means its rendering
    // never reached the buffer. Hunt tap (known-gap 4): the probe walks
    // the WHOLE buffer (up to 3.7MB of volatile reads per import) —
    // gated behind rux:ohhunt (retired debug=ohhunt) so standard boots pay nothing.
    static IMPORT_TRACE_N: AtomicU32 = AtomicU32::new(0);
    if crate::trace::EVENT_OHHUNT.armed() {
        let it = IMPORT_TRACE_N.fetch_add(1, Ordering::Relaxed);
        if it < 24 || it % 20 == 0 {
            let mut nz: usize = 0;
            let mut clear: usize = 0;
            let mut samples = [0u32; 3];
            // SAFETY: gem.kva..gem.kva+size is the object's backing; every
            // sample offset below is clamped into that range (the fixed
            // 720p/3072-pitch sample geometry only fits full-size frame GEMs —
            // an imported smaller buffer made the unclamped reads walk past the
            // allocation).
            unsafe {
                let base = gem.kva as *const u32;
                let words = gem.size / 4;
                for i in 0..words {
                    let v = base.add(i).read_volatile();
                    if v != 0 {
                        nz += 1;
                        if v == 0xff00_0000 {
                            clear += 1;
                        }
                    }
                }
                let row_words = 983040 / 1280; // pitch/4 for these buffers
                let s = |off: usize| -> u32 {
                    if off < words {
                        base.add(off).read_volatile()
                    } else {
                        0
                    }
                };
                samples[0] = s(row_words * 640 + 360);
                samples[1] = s(row_words * 640 + 200);
                samples[2] = s(row_words * 300 + 360);
            }
            crate::pr_warn!(
                "drm: import#{} pid={} fd={} gem={} nz={} clear={} s0={:08x} s1={:08x} s2={:08x}",
                it,
                crate::process::hotpath::current_pid(),
                prime_fd,
                gem.id,
                nz,
                clear,
                samples[0],
                samples[1],
                samples[2]
            );
        }
    }

    // Re-register under the gem's id if the handle side was already
    // GEM_CLOSEd (cross-process import): the object stays alive through
    // the prime fd's Arc.
    let handle = gem.id;
    {
        let mut st = DRM_STATE.lock_irqsave();
        if !st.gems.contains_key(&handle) {
            st.gems.insert(handle, gem);
        }
    }
    wru32(&mut b, 0, handle);
    if !copy_out(arg, &b, size) {
        return -14;
    }
    0
}

/// Ioctls on a prime fd (dma-buf file).
pub fn prime_file_ioctl(_file: &crate::fs::file::File, request: u32, _arg: usize) -> Option<i64> {
    match request {
        DMA_BUF_IOCTL_SYNC => Some(0), // cache coherent: nothing to do
        _ => Some(-25),                // ENOTTY
    }
}

// --- framebuffers ------------------------------------------------------------

fn addfb(arg: usize) -> i64 {
    // struct drm_mode_fb_cmd is 28 bytes: fb_id(0) width(4) height(8)
    // pitch(12) bpp(16) depth(20) handle(24). The old code copied 24
    // bytes and read handle at offset 24 — an out-of-bounds slice index
    // that would panic the kernel; the wrong ioctl size (24 vs 28) also
    // made the request number mismatch libdrm's, so ADDFB answered
    // ENOTTY instead.
    let Some(b) = copy_in::<28>(arg, 28) else { return -14 };
    let width = rdu32(&b, 4);
    let height = rdu32(&b, 8);
    let pitch = rdu32(&b, 12);
    let bpp = rdu32(&b, 16);
    let handle = rdu32(&b, 24);
    let fourcc = match bpp {
        32 => 0x3432_5241, // ARGB8888
        _ => return -22,
    };
    add_fb_common(arg, 28, handle, width, height, pitch, fourcc)
}

fn addfb2(arg: usize) -> i64 {
    let Some(b) = copy_in::<104>(arg, 104) else { return -14 };
    let width = rdu32(&b, 4);
    let height = rdu32(&b, 8);
    let fourcc = rdu32(&b, 12);
    let handle = rdu32(&b, 20); // handles[0]
    let pitch = rdu32(&b, 36); // pitches[0]
    let offset = rdu32(&b, 52); // offsets[0]
    if offset != 0 {
        return -22; // single-plane only
    }
    add_fb_common(arg, 104, handle, width, height, pitch, fourcc)
}

fn add_fb_common(arg: usize, size: usize, handle: u32, width: u32, height: u32, pitch: u32, fourcc: u32) -> i64 {
    if width == 0 || height == 0 || pitch == 0 {
        return -22;
    }
    let Some(vfmt) = virtio_format_for(fourcc) else {
        return -22; // unsupported pixel format
    };
    let gem = {
        let st = DRM_STATE.lock_irqsave();
        st.gems.get(&handle).cloned()
    };
    let Some(gem) = gem else { return -22 };
    // The GEM buffer must actually cover the framebuffer.
    if pitch as u64 * height as u64 > gem.size as u64 {
        return -22;
    }

    let fb_id = NEXT_FB_ID.fetch_add(1, Ordering::Relaxed);
    let mut st = DRM_STATE.lock_irqsave();
    st.fbs.insert(
        fb_id,
        FbObject {
            _gem: gem,
            width,
            height,
            pitch,
            virtio_format: vfmt,
        },
    );
    drop(st);

    // Bring-up trace: every FB creation (the composer makes one per
    // present; the handle/pitch pair identifies which GEM backs it).
    // 110th: rides the ohhunt gate (~60 lines per perf boot).
    static FB_TRACE_N: AtomicU32 = AtomicU32::new(0);
    let fn_ = FB_TRACE_N.fetch_add(1, Ordering::Relaxed);
    if fn_ < 60 && crate::debug::switches::oh_hunt_prints_on() {
        crate::pr_warn!(
            "drm: addfb#{} fb={} gem={} {}x{} pitch={} fourcc={:#x}",
            fn_,
            fb_id,
            handle,
            width,
            height,
            pitch,
            fourcc
        );
    }

    let mut out = [0u8; 104];
    wru32(&mut out, 0, fb_id);
    if !copy_out(arg, &out, size) {
        return -14;
    }
    0
}

fn rmfb(fb_id: u32) -> i64 {
    let mut st = DRM_STATE.lock_irqsave();
    match st.fbs.remove(&fb_id) {
        Some(_) => 0,
        None => -22,
    }
}

// --- page flip ------------------------------------------------------------

/// Display one framebuffer on scanout 0 (the whole atomic model here:
/// single plane, single CRTC, mode fixed).
// F-DIAG-DRM (file-level): the two content probes (present_fb's FLIP
// scan, prime_fd_to_handle's import scan) are ohhunt-gated DIAGNOSTIC
// GEM-window scans — one audited block each (gem.kva..+size, the
// object's own backing; sample offsets clamped). Reference counterpart:
// the reference kernel's drm debugfs framebuffer-dump paths (likewise
// diagnostic-only, not a runtime API). Not runtime state: gated off in
// production boots.
fn present_fb(fb_id: u32) -> i64 {
    // Snapshot the FB data under the lock, then run the virtio sequence
    // lock-free (device transactions can take milliseconds under TCG).
    let (gem, width, height, pitch, vfmt) = {
        let st = DRM_STATE.lock_irqsave();
        match st.fbs.get(&fb_id) {
            Some(fb) => (
                fb._gem.clone(),
                fb.width,
                fb.height,
                fb.pitch,
                fb.virtio_format,
            ),
            None => return -22,
        }
    };

    // Bring-up content probe (compose-content): is the buffer the
    // composer wants on screen actually non-zero at flip time? Black
    // content -> userspace never rendered into this GEM; non-black with
    // a black screen -> scanout binding. Sample every word (1M volatile
    // reads ~ a few ms under KVM) for the first 40 flips, then 1-in-10.
    // `paint` counts pixels that are NEITHER zero NOR the RS clear color
    // (0xff000000) inside the visible columns — the bootanimation logo
    // shows up there even when the background is opaque black.
    static FLIP_TRACE_N: AtomicU32 = AtomicU32::new(0);
    let ft = FLIP_TRACE_N.fetch_add(1, Ordering::Relaxed);
    // 110th: rides the ohhunt gate — the probe itself (1M+ volatile
    // reads ~ a few ms) ran on the composer's flip path of every perf
    // boot, print on top (~55 lines).
    if (ft < 40 || ft % 10 == 0) && crate::debug::switches::oh_hunt_prints_on() {
        let mut nz: usize = 0;
        let mut paint: usize = 0;
        let mut samples = [0u32; 4];
        // SAFETY: gem.kva..gem.kva+size is the object's own backing store;
        // word-aligned reads stay inside the allocation (sample offsets are
        // clamped to words — the fixed 720-wide sample geometry only fits
        // full-size frame GEMs).
        unsafe {
            let base = gem.kva as *const u32;
            let words = gem.size / 4;
            for i in 0..words {
                let v = base.add(i).read_volatile();
                if v != 0 {
                    nz += 1;
                }
            }
            let row_words = (pitch / 4) as usize;
            let vis_words = width as usize;
            for row in 0..height as usize {
                let rowp = base.add(row * row_words);
                for col in (0..vis_words).step_by(2) {
                    let v = rowp.add(col).read_volatile();
                    if v != 0 && v != 0xff00_0000 {
                        paint += 1;
                    }
                }
            }
            let mid_row = (height as usize / 2) * row_words;
            let last_row = (height as usize - 1).saturating_mul(row_words);
            let s = |off: usize| -> u32 {
                if off < words {
                    base.add(off).read_volatile()
                } else {
                    0
                }
            };
            samples[0] = s(720 / 4);
            samples[1] = s(row_words + 720 / 4);
            samples[2] = s(mid_row + (width as usize / 2));
            samples[3] = s(last_row + 720 / 4);
        }
        crate::pr_warn!(
            "drm: flip#{} fb={} gem={} {}x{} pitch={} vfmt={} nz={}/{} paint={} ctr={:08x} cmid={:08x}",
            ft,
            fb_id,
            gem.id,
            width,
            height,
            pitch,
            vfmt,
            nz,
            gem.size / 4,
            paint,
            samples[0],
            samples[2]
        );
    }

    // (The scanout-binding oracle that used to live here painted a banded
    // checkerboard into the flipped GEM through kva before the host
    // transfer — on flips 1..=60 and EVERY 5th flip thereafter, with no
    // upper bound. It was bring-up tooling for the compose-content hunt,
    // but as permanent code it overwrote the composer's real frame content
    // with probe patterns on a cadence, corrupting every long-running
    // desktop session. Removed; the read-only nz/paint counters above keep
    // the content-vs-binding diagnosis without touching the buffer.)

    // Resource geometry: the GEM's real row pitch. Gralloc allocates
    // FB buffers width-aligned to 64 pixels (720 content on a 768-pixel
    // row, pitch 3072), so a resource built at the FB's 720 width makes
    // QEMU read guest rows 2880 bytes apart — every row drifts 192
    // bytes and the scanout shows a diagonally sheared frame. Build the
    // resource at the memory's true geometry and display the FB rect as
    // a scanout subrect (SET_SCANOUT crops by the resource stride).
    let res_w = if pitch >= width * 4 && pitch % 4 == 0 && pitch / 4 >= width {
        pitch / 4
    } else {
        width
    };

    let ok = crate::drivers::hotpath::with_gpu_device(|gpu| {
        // First flip of this buffer: create its host-side resource.
        let mut res = gem.virtio_res.load(Ordering::Acquire);
        if res == 0 {
            let id = NEXT_VIRTIO_RES.fetch_add(1, Ordering::Relaxed);
            if gpu.create_resource_2d_id(id, res_w, height, vfmt).is_none() {
                return false;
            }
            if gpu.attach_backing_id(id, gem.phys, gem.size as u32).is_none() {
                return false;
            }
            gem.virtio_res.store(id, Ordering::Release);
            res = id;
        }
        // Guest RAM -> host resource storage. Full-resource transfer:
        // row stride res_w*4 == the GEM pitch, so the guest backing is
        // read linearly, exactly as it is laid out.
        if !gpu.transfer_to_host_2d_id(res, 0, res_w, height) {
            return false;
        }
        // Bind to scanout when the resource changed since last flip;
        // the scanout rect is the FB's (sub)rectangle of the resource.
        let cur = DRM_STATE.lock_irqsave().scanout_res;
        if cur != res {
            if !gpu.set_scanout_rect(0, res, width, height, res_w) {
                return false;
            }
            DRM_STATE.lock_irqsave().scanout_res = res;
        }
        // Present the displayed rect.
        gpu.resource_flush_id(res, width, height)
    });
    match ok {
        Some(true) => {
            static FIRST_FLIP: AtomicU32 = AtomicU32::new(0);
            if FIRST_FLIP.fetch_add(1, Ordering::Relaxed) == 0 {
                crate::pr_warn!(
                    "drm: first page flip on screen (fb {} {}x{})",
                    fb_id,
                    width,
                    height
                );
            }
            0
        }
        Some(false) => {
            crate::pr_warn!("drm: flip to fb {} failed (virtio-gpu error)", fb_id);
            -5 // EIO
        }
        None => {
            crate::pr_warn!("drm: flip requested but no virtio-gpu device");
            -19 // ENODEV
        }
    }
}

fn page_flip(arg: usize) -> i64 {
    let Some(b) = copy_in::<24>(arg, 24) else { return -14 };
    let fb_id = rdu32(&b, 4);
    if rdu32(&b, 0) != OBJ_CRTC {
        return -22;
    }
    present_fb(fb_id)
}

/// MODE_ATOMIC: walk the OH-layout request arrays, pick up the primary
/// plane's FB_ID (and CRTC attach), then present.
fn mode_atomic(arg: usize) -> i64 {
    const MAX_OBJS: usize = 8;
    const MAX_PROPS: usize = 96;
    let Some(b) = copy_in::<56>(arg, 56) else { return -14 };
    let flags = rdu32(&b, 0);
    let count_objs = rdu32(&b, 4) as usize;
    let objs_ptr = rdu64(&b, 8) as usize;
    let counts_ptr = rdu64(&b, 16) as usize;
    let props_ptr = rdu64(&b, 24) as usize;
    let values_ptr = rdu64(&b, 32) as usize;

    const DRM_MODE_ATOMIC_TEST_ONLY: u32 = 0x0100;

    if count_objs > MAX_OBJS {
        return -7; // E2BIG
    }
    if count_objs == 0 {
        return -22;
    }
    if !crate::arch::uaccess::access_ok(objs_ptr, count_objs * 4)
        || !crate::arch::uaccess::access_ok(counts_ptr, count_objs * 4)
    {
        return -14;
    }
    let mut objs = [0u32; MAX_OBJS];
    let mut counts = [0u32; MAX_OBJS];
    let mut total = 0usize;
    // Pointers validated above; the safe scalar reader.
    for i in 0..count_objs {
        let Some(o) = crate::arch::uaccess_safe::read_user::<u32>(objs_ptr + i * 4) else {
            return -14;
        };
        let Some(c) = crate::arch::uaccess_safe::read_user::<u32>(counts_ptr + i * 4) else {
            return -14;
        };
        objs[i] = o;
        counts[i] = c;
        total += c as usize;
    }
    if total > MAX_PROPS {
        return -7;
    }
    if total > 0
        && (!crate::arch::uaccess::access_ok(props_ptr, total * 4)
            || !crate::arch::uaccess::access_ok(values_ptr, total * 8))
    {
        return -14;
    }
    let mut props = [0u32; MAX_PROPS];
    let mut values = [0u64; MAX_PROPS];
    // Pointers validated above; the safe scalar reader.
    for i in 0..total {
        let Some(p) = crate::arch::uaccess_safe::read_user::<u32>(props_ptr + i * 4) else {
            return -14;
        };
        props[i] = p;
        let Some(v) = crate::arch::uaccess_safe::read_user::<u64>(values_ptr + i * 8) else {
            return -14;
        };
        values[i] = v;
    }

    // Walk the flattened arrays tracking the object each property
    // belongs to.
    let mut flip_fb: u32 = 0;
    let mut idx = 0usize;
    // Bring-up trace (compose-content): the exact property combo of
    // every commit — the field-by-field diff against the painter's
    // known-good (obj=plane, FB_ID only, flags=0) is how a wrong
    // binding would show up. First 60 commits in full, then 1-in-25.
    static ATOM_TRACE_N: AtomicU32 = AtomicU32::new(0);
    let at = ATOM_TRACE_N.fetch_add(1, Ordering::Relaxed);
    // 110th: rides the ohhunt gate (~65 lines per perf boot).
    let trace_on = (at < 60 || at % 25 == 0) && crate::debug::switches::oh_hunt_prints_on();
    let mut trace = if trace_on {
        alloc::string::String::with_capacity(128)
    } else {
        alloc::string::String::new()
    };
    if trace_on {
        use core::fmt::Write as _;
        let _ = write!(
            &mut trace,
            "atom#{} flags={:#x} nobj={} ",
            at, flags, count_objs
        );
    }
    for oi in 0..count_objs {
        if trace_on {
            use core::fmt::Write as _;
            let _ = write!(&mut trace, "obj{}[", objs[oi]);
        }
        for _ in 0..counts[oi] as usize {
            if idx >= total {
                break;
            }
            let obj = objs[oi];
            let prop = props[idx];
            let value = values[idx];
            idx += 1;
            if trace_on {
                use core::fmt::Write as _;
                let _ = write!(&mut trace, "{}={:#x},", prop, value);
            }
            if obj == OBJ_PLANE && prop == PROP_PLANE_FB_ID {
                flip_fb = value as u32;
            }
            // ACTIVE / MODE_ID / CRTC_ID / zpos / OUT_FENCE_PTR: mode is
            // fixed, fences unsupported — accepted and ignored.
            let _ = (obj, value);
        }
        if trace_on {
            trace.push(']');
        }
    }
    if trace_on {
        if flags & DRM_MODE_ATOMIC_TEST_ONLY != 0 {
            trace.push_str(" TEST");
        }
        crate::pr_warn!("drm: {}", trace);
    }

    if flags & DRM_MODE_ATOMIC_TEST_ONLY != 0 {
        return 0; // validate only
    }

    if flip_fb != 0 {
        return present_fb(flip_fb);
    }
    0
}

// --- vblank ------------------------------------------------------------

/// Nominal vblank period (60 Hz), nanoseconds.
const VBLANK_NS: u64 = 16_666_666;
/// _DRM_VBLANK_RELATIVE.
const DRM_VBLANK_RELATIVE: u32 = 0x1;
/// _DRM_VBLANK_ABSOLUTE.
const DRM_VBLANK_TYPES_MASK: u32 = 0x3;
const DRM_VBLANK_EVENT: u32 = 0x400_0000;
/// _DRM_VBLANK_SECONDARY selects pipe 1 — only pipe 0 exists here, so it
/// is ignored (the composer only sets it for pipe-1 CRTCs).
const DRM_VBLANK_SECONDARY: u32 = 0x2000_0000;
const DRM_VBLANK_HIGH_CRTC_MASK: u32 = 0x3e;

fn vblank_seq_at(ns: u64) -> u64 {
    ns / VBLANK_NS
}

/// Sleep (interruptible) until `deadline_ns` (monotonic since boot).
/// Returns false when a signal interrupted the wait.
fn sleep_until_ns(deadline_ns: u64) -> bool {
    use crate::process::types::TaskState;

    if crate::drivers::hotpath::timer::monotonic_ns_since_boot() >= deadline_ns {
        return true;
    }
    let current = match crate::sched::hotpath::current() {
        Some(t) => t,
        None => return true,
    };
    // current() is this task's own safe handle (kfbflush discipline).
    let my_pid = current.pid();
    const NS_PER_JIFFY: u64 = 1_000_000_000 / (crate::config::KERNEL_HZ as u64);

    loop {
        if crate::drivers::hotpath::timer::monotonic_ns_since_boot() >= deadline_ns {
            return true;
        }
        if crate::signal::signal_pending() {
            return false;
        }
        let remaining = deadline_ns.saturating_sub(crate::drivers::hotpath::timer::monotonic_ns_since_boot());
        let target =
            crate::drivers::hotpath::timer::get_jiffies() + remaining / NS_PER_JIFFY + 1;
        let timer_id = crate::timer::add_timer_wakeup(target, my_pid);
        if timer_id != 0 {
            current.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
            if crate::drivers::hotpath::timer::get_jiffies() >= target {
                current.set_state(TaskState::new(TaskState::RUNNING));
                crate::sched::hotpath::dequeue_task(current);
                crate::timer::del_timer(timer_id);
            } else {
                crate::arch::cpu::restore_irq(true);
                crate::sched::hotpath::schedule();
                if crate::timer::timer_pending(timer_id) {
                    crate::timer::del_timer(timer_id);
                }
            }
        } else {
            crate::sched::hotpath::schedule();
        }
    }
}

/// WAIT_VBLANK: timer-driven 60 Hz vblank model (no real display IRQ in
/// virtio-gpu 2D mode). Relative waits sleep to the next boundary,
/// sequence 0 returns the current counter immediately.
fn wait_vblank(arg: usize) -> i64 {
    let Some(mut b) = copy_in::<24>(arg, 24) else { return -14 };
    let req_type = rdu32(&b, 0);
    let req_seq = rdu32(&b, 4);

    if req_type & DRM_VBLANK_EVENT != 0 {
        return -22; // no drm_event read() path
    }
    // Reject only unknown type/flag bits (SECONDARY/HIGH_CRTC are pipe
    // selectors — irrelevant on the single pipe, tolerated).
    const KNOWN_FLAGS: u32 =
        DRM_VBLANK_EVENT | DRM_VBLANK_SECONDARY | DRM_VBLANK_HIGH_CRTC_MASK | 0x800_0000; /* _DRM_VBLANK_SIGNAL */
    if req_type & !DRM_VBLANK_TYPES_MASK & !KNOWN_FLAGS != 0 {
        return -22;
    }

    let now_ns = crate::drivers::hotpath::timer::monotonic_ns_since_boot();
    let now_seq = vblank_seq_at(now_ns);
    let target_seq = if req_type & DRM_VBLANK_TYPES_MASK == DRM_VBLANK_RELATIVE {
        now_seq + req_seq as u64
    } else {
        // ABSOLUTE: a past sequence reports the current one.
        (req_seq as u64).max(now_seq)
    };
    if target_seq > now_seq {
        let deadline = target_seq * VBLANK_NS;
        if !sleep_until_ns(deadline) {
            return -4; // EINTR
        }
    }

    let fired_ns = crate::drivers::hotpath::timer::monotonic_ns_since_boot();
    let fired_seq = vblank_seq_at(fired_ns);
    wru32(&mut b, 0, req_type & DRM_VBLANK_TYPES_MASK);
    wru32(&mut b, 4, fired_seq as u32);
    wru64(&mut b, 8, (fired_ns / 1_000_000_000) as u64); // tval_sec
    wru64(&mut b, 16, ((fired_ns % 1_000_000_000) / 1000) as u64); // tval_usec
    if !copy_out(arg, &b, 24) {
        return -14;
    }
    0
}

// ============================================================================
// mmap resolution (called from the mmap syscall path)
// ============================================================================

/// Resolve an mmap on a DRM file into a (phys, len) pair the syscall
/// path can map. `offset` is 0 for prime fds (their private GEM) or the
/// fake offset from MAP_DUMB for /dev/dri nodes.
pub fn gem_mmap_resolve(
    file: &crate::fs::file::File,
    offset: u64,
    len: usize,
) -> Option<(u64, usize)> {
    // Bring-up trace (compose-content): who maps which GEM — RS's
    // llvmpipe must map the SAME objects the composer flips.
    // 110th: rides the ohhunt gate (~25 lines per perf boot).
    static MMAP_TRACE_N: AtomicU32 = AtomicU32::new(0);
    let mt = MMAP_TRACE_N.fetch_add(1, Ordering::Relaxed);
    let trace = mt < 30 && crate::debug::switches::oh_hunt_prints_on();
    if is_prime_file(file) {
        let gem = prime_file_gem(file)?;
        let n = len.min(gem.size);
        if trace {
            crate::pr_warn!(
                "drm: prime mmap#{} pid={} gem={} off={:#x} len={} -> phys={:#x}",
                mt,
                crate::process::hotpath::current_pid(),
                gem.id,
                offset,
                n,
                gem.phys
            );
        }
        return Some((gem.phys + offset, n));
    }
    if is_drm_file(file) {
        let handle = (offset >> 12) as u32;
        let gem = {
            let st = DRM_STATE.lock_irqsave();
            st.gems.get(&handle).cloned()
        }?;
        let off = offset & 0xfff;
        let n = len.min(gem.size.saturating_sub(off as usize));
        if trace {
            crate::pr_warn!(
                "drm: node mmap#{} gem={} off={:#x} len={} -> phys={:#x}",
                mt,
                gem.id,
                offset,
                n,
                gem.phys
            );
        }
        return Some((gem.phys + off, n));
    }
    None
}

// ============================================================================
// init
// ============================================================================

/// Register the DRM character devices and create the /dev/dri nodes.
/// Called after the GPU framebuffer came up (the flip path needs the
/// virtio-gpu device).
pub fn init_drm() -> Result<(), ()> {
    crate::fs::leaf_glue::devfs::register_char_device(DEV_DRM_CARD0, &DRM_OPS)?;
    crate::fs::leaf_glue::devfs::register_char_device(DEV_DRM_RENDER, &DRM_OPS)?;

    crate::fs::leaf_glue::devfs::mkdir("dri")?;
    crate::fs::leaf_glue::devfs::mknod("dri/card0", DEV_DRM_CARD0, 0o666)?;
    crate::fs::leaf_glue::devfs::mknod("dri/renderD128", DEV_DRM_RENDER, 0o666)?;

    // DrmDisplay::SetDisplayPowerStatus opens /dev/graphics/fb0 and
    // FBIOBLANKs it; alias the existing fb0 node (the OH power-state
    // path reports NOT_SUPPORT when it is missing).
    crate::fs::leaf_glue::devfs::mkdir("graphics")?;
    crate::fs::leaf_glue::devfs::mknod("graphics/fb0", crate::fs::dev_t::DEV_FB0, 0o666)?;

    // sysfs class entries for the DRM nodes, rooted at the virtio-gpu
    // PCI function: /sys/class/drm/{card0,renderD128} +
    // /sys/dev/char/226:{0,128} + the PCI device dir libdrm's
    // drmGetDevice(fd) walk needs (see register_drm_device).
    let slot_name = gpu_pci_slot_name();
    crate::fs::leaf_glue::sysfs::register_drm_device(&slot_name);

    crate::pr_info!(
        "drm: /dev/dri/card0 + renderD128 registered (crtc={} conn={} enc={} plane={})",
        OBJ_CRTC,
        OBJ_CONNECTOR,
        OBJ_ENCODER,
        OBJ_PLANE
    );
    Ok(())
}

/// PCI_SLOT_NAME of the probed virtio-gpu function ("0000:00:0b.0"),
/// derived from the device's ECAM address. "0000:00:00.0" when the BDF
/// cannot be recovered (the sysfs tree still gets a consistent shape).
fn gpu_pci_slot_name() -> alloc::string::String {
    use core::fmt::Write as _;
    let bdf = crate::drivers::hotpath::with_gpu_device(|dev| {
        let off = dev.pci_ecam_addr().wrapping_sub(crate::drivers::hotpath::ecam_base());
        (
            ((off >> 20) & 0xff) as u8,
            ((off >> 15) & 0x1f) as u8,
            ((off >> 12) & 0x7) as u8,
        )
    });
    let mut s = alloc::string::String::new();
    let _ = match bdf {
        Some((bus, slot, func)) => write!(s, "0000:{:02x}:{:02x}.{}", bus, slot, func),
        None => write!(s, "0000:00:00.0"),
    };
    s
}
