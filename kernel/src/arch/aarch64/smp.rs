//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 SMP support — PSCI 0.2 CPU_ON via the HVC conduit.
//!
//! Boot flow (QEMU `virt`, `-kernel` ELF direct boot):
//!   1. QEMU powers on only the boot core (cpu 0) into the kernel entry;
//!      every other core stays powered off in the PSCI "off" state.
//!   2. Boot core: boot.S `_start` → rust_main → full kernel init.
//!   3. After scheduler init, boot core calls start_secondaries().
//!   4. start_secondaries() saves the live boot-core MMU state (TTBR1
//!      root, TCR, MAIR), then issues PSCI CPU_ON for each secondary via
//!      the conduit the DTB `/psci` node advertises (hvc on QEMU virt).
//!   5. Each secondary wakes in boot.S `secondary_start` (MMU off, at the
//!      physical entry point), restores the boot core's tables, turns the
//!      MMU on, and branches to secondary_cpu_entry() at its linked VA.
//!
//! Per-CPU identity: MPIDR_EL1 affinity fields hash to a linear cpu
//! index (Linux arm64 shape: Aff0 | Aff1<<8 | Aff2<<16). QEMU virt puts
//! cpu N at Aff0=N for the first 4 cores and folds cluster numbers into
//! Aff1 beyond that, so the pack is the identity map on our MAX_CPUS=4
//! operating range. The index travels to the stub as the PSCI context_id
//! (x0 at entry) and lands in TPIDR_EL1 — the same two-phase protocol the
//! riscv64 backend runs on the tp register (small value = cpu index until
//! the scheduler stores the task pointer).


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use crate::arch::aarch64::MAX_CPUS;
use core::sync::atomic::{AtomicU32, Ordering};

/// SMP boot stack size - from config
pub const STACK_SIZE: usize = crate::config::SMP_BOOT_STACK_SIZE;

// The boot.S secondary stub hardcodes the stack stride and the legal cpu
// index ceiling as immediates; pin them here so a config change cannot
// silently desynchronize the two languages.
pub(crate) const _: () = assert!(
    STACK_SIZE == 0x10000,
    "boot.S secondary_start assumes 64KB (0x10000) per-cpu boot stacks"
);
pub(crate) const _: () = assert!(
    MAX_CPUS <= 4,
    "boot.S secondary_start parks cpu indices >= 4 (MAX_CPUS must stay <= 4)"
);

pub(crate) static CPU_STARTED: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];

/// Actual boot CPU ID, saved by init()
pub(crate) static BOOT_CPU_ID: AtomicU32 = AtomicU32::new(u32::MAX);

/// Get the boot CPU ID (the core that ran rust_main)
pub fn boot_hart_id() -> usize {
    BOOT_CPU_ID.load(Ordering::Acquire) as usize
}

/// Alias (riscv64 naming)
pub fn boot_cpu_id() -> usize {
    boot_hart_id()
}

pub(crate) fn mark_cpu_started(cpu: usize) {
    if cpu < MAX_CPUS {
        CPU_STARTED[cpu].store(1, Ordering::Release);
    }
}

/// Whether cpu `cpu` has entered secondary_cpu_entry (or is the boot cpu)
pub fn cpu_started(cpu: usize) -> bool {
    cpu < MAX_CPUS && CPU_STARTED[cpu].load(Ordering::Acquire) == 1
}
/// Get current CPU's index.
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-12): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn cpu_id() -> usize {
    crate::arch::arch_floor::aarch64::smp::cpu_id()
}

#[inline]
pub fn is_boot_hart() -> bool {
    let boot = BOOT_CPU_ID.load(Ordering::Acquire);
    if boot == u32::MAX {
        cpu_id() == 0
    } else {
        cpu_id() == boot as usize
    }
}

/// SMP init — called from rust_main by the boot CPU.
/// QEMU -kernel direct boot releases only the boot core into the kernel;
/// secondaries stay powered off until PSCI CPU_ON (stage 3).
pub fn init() -> bool {
    let my_cpu = cpu_id();
    BOOT_CPU_ID.store(my_cpu as u32, Ordering::Release);
    mark_cpu_started(my_cpu);
    true
}

// ==================== MPIDR_EL1 → cpu index ====================
/// Read MPIDR_EL1 (affinity fields of the current core).
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-11): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn read_mpidr() -> u64 {
    crate::arch::arch_floor::aarch64::smp::read_mpidr()
}

/// Linux arm64 affinity pack: MPIDR affinity levels folded into a linear
/// id — Aff0 | Aff1<<8 | Aff2<<16 (bit31 is RES1 and ignored). On QEMU
/// virt cpu N carries Aff0=N for N<4 and cluster numbers appear in Aff1
/// above that, so this is the identity map on the MAX_CPUS=4 range.
#[inline]
pub fn mpidr_to_index(mpidr: u64) -> usize {
    let aff0 = mpidr & 0xFF;
    let aff1 = (mpidr >> 8) & 0xFF;
    let aff2 = (mpidr >> 16) & 0xFF;
    (aff0 | (aff1 << 8) | (aff2 << 16)) as usize
}

/// CPU index → the MPIDR value PSCI CPU_ON expects in its target_cpu
/// parameter. Inverse of mpidr_to_index for the QEMU virt topology
/// (4 cores per cluster; core-in-cluster in Aff0, cluster in Aff1).
#[inline]
pub fn index_to_mpidr(cpu: usize) -> u64 {
    ((cpu / 4) << 8 | (cpu % 4)) as u64
}

// ==================== PSCI 0.2 conduit ====================

/// PSCI 0.2 function IDs (SMC64/HVC64 calling convention where the
/// entry point is 64-bit wide).
pub(crate) const PSCI_VERSION: u64 = 0x8400_0000;
pub(crate) const PSCI_CPU_ON: u64 = 0xC400_0003;

/// PSCI return codes (negative errno-style, delivered in x0).
pub(crate) const PSCI_NOT_SUPPORTED: i64 = -1;
pub(crate) const PSCI_INVALID_PARAMS: i64 = -2;
pub(crate) const PSCI_DENIED: i64 = -3;
pub(crate) const PSCI_ALREADY_ON: i64 = -4;

/// Conduit instruction detected from the DTB /psci node ("hvc" or "smc").
/// QEMU virt advertises hvc; the fallback default matches.
pub(crate) static CONDUIT_HVC: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);
/// CPU_ON function id from the DTB /psci node (QEMU 0.2 node carries
/// 0xc4000003; kept overridable for a 1.x node that renumbers it).
pub(crate) static CPU_ON_FN: AtomicU32 = AtomicU32::new(PSCI_CPU_ON as u32);
/// PSCI version reported by PSCI_VERSION (0 = probe not run / unsupported).
pub(crate) static PSCI_VERSION_REG: AtomicU32 = AtomicU32::new(0);
/// Issue a PSCI call through the detected conduit. Returns x0 (signed).
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-10): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn psci_call(fnid: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    crate::arch::arch_floor::aarch64::smp::psci_call(fnid, a1, a2, a3)
}

/// Parse the DTB `/psci` node for the conduit method and the CPU_ON
/// function id. Mirrors the manual FDT walk in cmdline.rs (arch-neutral,
/// no heap). Returns false when no /psci node exists (caller defaults).
pub(crate) fn detect_psci_from_dtb() -> bool {
    // The DTB physical address is fixed by QEMU before boot and the
    // linear map covers all of RAM; the walk uses the shared FDT read
    // edge (INV-FDT-1, cmdline.rs) — same proof, one audited site.
    use crate::cmdline::{fdt_u8, fdt_add, fdt_u32};
    let dtb_phys = super::boot::get_dtb_pointer();
    let dtb_virt = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(dtb_phys)).bits();
    let fdt = dtb_virt as *const u8;

    let read_u32 = |p: *const u8| -> u32 { fdt_u32(p) };

    if read_u32(fdt) != 0xd00d_feed {
        return false;
    }
    let off_dt_struct = read_u32(fdt_add(fdt, 0x08)) as usize;
    let off_dt_strings = read_u32(fdt_add(fdt, 0x0C)) as usize;
    let size_dt_struct = read_u32(fdt_add(fdt, 0x24)) as usize;
    let strings = fdt_add(fdt, off_dt_strings as isize);

    const FDT_BEGIN_NODE: u32 = 0x1;
    const FDT_END_NODE: u32 = 0x2;
    const FDT_PROP: u32 = 0x3;
    const FDT_END: u32 = 0x9;

    let mut ptr = fdt_add(fdt, off_dt_struct as isize);
    let end = fdt_add(ptr, size_dt_struct as isize);
    let mut depth = 0usize;
    let mut in_psci = false;
    let mut found = false;

    while ptr < end {
        let token = read_u32(ptr);
        ptr = fdt_add(ptr, 4);
        match token {
            FDT_BEGIN_NODE => {
                let mut name = [0u8; 32];
                let mut i = 0;
                while fdt_u8(ptr) != 0 && i < 31 {
                    name[i] = fdt_u8(ptr);
                    ptr = fdt_add(ptr, 1);
                    i += 1;
                }
                ptr = fdt_add(ptr, 1); // NUL
                ptr = fdt_add(ptr, ((4 - ((ptr as usize) & 3)) & 3) as isize);
                // Root children carry bare names ("psci"); unit
                // addresses ("psci@...") are matched too. A root
                // child is announced while depth == 1 (the root
                // BEGIN_NODE already bumped it).
                let ok = name[..i] == *b"psci" || name[..i].starts_with(b"psci@");
                if depth == 1 && ok {
                    in_psci = true;
                    found = true;
                }
                depth += 1;
            }
            FDT_END_NODE => {
                if in_psci && depth == 2 {
                    in_psci = false;
                }
                depth -= 1;
            }
            FDT_PROP => {
                let len = read_u32(ptr) as usize;
                let nameoff = read_u32(fdt_add(ptr, 4)) as usize;
                ptr = fdt_add(ptr, 8);
                let mut name_ptr = fdt_add(strings, nameoff as isize);
                let mut pname = [0u8; 16];
                let mut i = 0;
                while fdt_u8(name_ptr) != 0 && i < 15 {
                    pname[i] = fdt_u8(name_ptr);
                    name_ptr = fdt_add(name_ptr, 1);
                    i += 1;
                }
                if in_psci {
                    match &pname[..i] {
                        b"method" => {
                            // INV-FDT-1 read: at most 4 bytes of the prop
                            // payload, inside the blob.
                            let bytes = [fdt_u8(fdt_add(ptr, 0)),
                                         fdt_u8(fdt_add(ptr, 1)),
                                         fdt_u8(fdt_add(ptr, 2)),
                                         fdt_u8(fdt_add(ptr, 3))];
                            let hvc = bytes.starts_with(b"hvc");
                            CONDUIT_HVC.store(hvc, Ordering::Release);
                        }
                        b"cpu_on" => {
                            // <u32 function-id>; 64-bit variant expected
                            let v = read_u32(ptr);
                            if v != 0 {
                                CPU_ON_FN.store(v, Ordering::Release);
                            }
                        }
                        _ => {}
                    }
                }
                ptr = fdt_add(ptr, len as isize);
                ptr = fdt_add(ptr, ((4 - ((ptr as usize) & 3)) & 3) as isize);
            }
            FDT_END => break,
            _ => break,
        }
    }
    found
}

// ==================== Boot-complete handshake ====================

/// Flag set by boot CPU after ALL initialization is complete.
pub(crate) static BOOT_COMPLETE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Called by boot CPU just before entering cpu_idle_loop.
pub fn signal_boot_complete() {
    BOOT_COMPLETE.store(true, core::sync::atomic::Ordering::Release);
    // Kick every started secondary out of its boot-complete wfi park.
    // The SGI is a real GIC event now that the ICC_SGI1R encoding and
    // Group-1 forwarding are fixed; its handler is a no-op — the wake
    // itself is the point. (The parked secondary's own timer also wakes
    // it within one tick; the kick makes it immediate and deterministic.)
    let me = cpu_id();
    for cpu in 0..MAX_CPUS {
        if cpu != me && cpu_started(cpu) {
            crate::arch::ipi::send_ipi_type(cpu, crate::arch::ipi::IpiType::Wakeup);
        }
    }
}

/// Called by secondary CPUs to check if boot is complete.
pub fn is_boot_complete() -> bool {
    BOOT_COMPLETE.load(core::sync::atomic::Ordering::Acquire)
}

// ==================== Saved boot-core MMU state ====================




use core::sync::atomic::AtomicU64;

/// Assembly entry point in boot/boot.S for PSCI-on secondaries (the stub
/// restores the saved MMU state and jumps to secondary_cpu_entry with
/// x0 = cpu index).
extern "C" {
    fn secondary_start();
}

/// Per-CPU boot stacks for secondary CPUs (physical == image .bss; the
/// stub consumes them before the MMU is on via the identity map).
/// `#[no_mangle]` + `#[used]`: boot.S `secondary_start` indexes this
/// array by cpu index before any Rust code runs on the core.
#[repr(align(16))]
pub(crate) struct BootStack(pub(crate) [u8; STACK_SIZE]);

/// Start secondary CPUs via PSCI CPU_ON.
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn start_secondaries() {
    crate::arch::arch_floor::aarch64::smp::start_secondaries()
}


pub fn num_started_cpus() -> usize {
    let mut count = 0;
    for i in 0..MAX_CPUS {
        if CPU_STARTED[i].load(Ordering::Acquire) == 1 {
            count += 1;
        }
    }
    count
}

// ==================== Per-CPU Interrupt Stack ====================

/// Per-CPU interrupt stack size (16KB each)
pub const INTR_STACK_SIZE: usize = 16384;

/// Aligned stack wrapper to ensure 16-byte alignment
#[repr(C, align(16))]
pub(crate) struct AlignedStack(pub(crate) [u8; INTR_STACK_SIZE]);

impl AlignedStack {
    /// Get a pointer to the underlying stack buffer
    pub const fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr()
    }
}

/// Initialize per-CPU interrupt stack base pointer for assembly code
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn init_per_cpu_intr_stacks() {
    crate::arch::arch_floor::aarch64::smp::init_per_cpu_intr_stacks()
}

// Collected-unsafe re-exports: the unsafe floor for these names lives in
// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.
// INV-ARCH note: get_per_cpu_intr_stack_top moved to arch::arch_floor::aarch64::smp (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::aarch64::smp::get_per_cpu_intr_stack_top;
pub(crate) use crate::arch::arch_floor::aarch64::smp::PER_CPU_INTR_STACKS;
// INV-ARCH note: secondary_cpu_entry moved to arch::arch_floor::aarch64::smp (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::aarch64::smp::secondary_cpu_entry;
pub(crate) use crate::arch::arch_floor::aarch64::smp::SECONDARY_BOOT_STACKS;
pub(crate) use crate::arch::arch_floor::aarch64::smp::BOOT_HART_MAIR;
pub(crate) use crate::arch::arch_floor::aarch64::smp::BOOT_HART_TCR;
pub(crate) use crate::arch::arch_floor::aarch64::smp::BOOT_HART_TTBR;
