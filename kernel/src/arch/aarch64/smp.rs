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

use crate::config::MAX_CPUS;
use core::sync::atomic::{AtomicU32, Ordering};

/// SMP boot stack size - from config
pub const STACK_SIZE: usize = crate::config::SMP_BOOT_STACK_SIZE;

// The boot.S secondary stub hardcodes the stack stride and the legal cpu
// index ceiling as immediates; pin them here so a config change cannot
// silently desynchronize the two languages.
const _: () = assert!(
    STACK_SIZE == 0x10000,
    "boot.S secondary_start assumes 64KB (0x10000) per-cpu boot stacks"
);
const _: () = assert!(
    MAX_CPUS <= 4,
    "boot.S secondary_start parks cpu indices >= 4 (MAX_CPUS must stay <= 4)"
);

static CPU_STARTED: [AtomicU32; MAX_CPUS] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

/// Actual boot CPU ID, saved by init()
static BOOT_CPU_ID: AtomicU32 = AtomicU32::new(u32::MAX);

/// Get the boot CPU ID (the core that ran rust_main)
pub fn boot_hart_id() -> usize {
    BOOT_CPU_ID.load(Ordering::Acquire) as usize
}

/// Alias (riscv64 naming)
pub fn boot_cpu_id() -> usize {
    boot_hart_id()
}

fn mark_cpu_started(cpu: usize) {
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
/// Protocol (same shape as the riscv64 tp protocol, via TPIDR_EL1):
/// - Early boot: TPIDR_EL1 = cpu index (small value set by boot.S /
///   the PSCI secondary stub)
/// - After scheduling: TPIDR_EL1 = task_struct pointer; the cpu index
///   lives in task_struct.ti_cpu at the fixed offset
#[inline]
pub fn cpu_id() -> usize {
    // SAFETY: reading TPIDR_EL1 is a pure register read. When the value is
    // small it holds a cpu index; otherwise it is the current task_struct
    // pointer set by the trap entry path, and the fixed ti_cpu offset is
    // within the struct layout.
    unsafe {
        let tp: u64;
        core::arch::asm!("mrs {}, tpidr_el1", out(reg) tp, options(nomem, nostack, pure));

        if tp < 0x1000 {
            tp as usize
        } else {
            let ti_cpu_offset = crate::process::task::task_offsets::TI_CPU;
            let cpu_ptr = (tp as usize + ti_cpu_offset) as *const core::sync::atomic::AtomicI32;
            let raw = (*cpu_ptr).load(Ordering::Relaxed);
            // Defensive: clamp negative or out-of-range (see the riscv64
            // twin — initial -1 sign-extends to usize::MAX)
            if raw < 0 || (raw as usize) >= crate::config::MAX_CPUS {
                0
            } else {
                raw as usize
            }
        }
    }
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
#[inline]
pub fn read_mpidr() -> u64 {
    // SAFETY: MPIDR_EL1 is a read-only identification register accessible
    // at EL1; the read has no side effects.
    unsafe {
        let v: u64;
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) v, options(nomem, nostack, pure));
        v
    }
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
const PSCI_VERSION: u64 = 0x8400_0000;
const PSCI_CPU_ON: u64 = 0xC400_0003;

/// PSCI return codes (negative errno-style, delivered in x0).
const PSCI_NOT_SUPPORTED: i64 = -1;
const PSCI_INVALID_PARAMS: i64 = -2;
const PSCI_DENIED: i64 = -3;
const PSCI_ALREADY_ON: i64 = -4;

/// Conduit instruction detected from the DTB /psci node ("hvc" or "smc").
/// QEMU virt advertises hvc; the fallback default matches.
static CONDUIT_HVC: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);
/// CPU_ON function id from the DTB /psci node (QEMU 0.2 node carries
/// 0xc4000003; kept overridable for a 1.x node that renumbers it).
static CPU_ON_FN: AtomicU32 = AtomicU32::new(PSCI_CPU_ON as u32);
/// PSCI version reported by PSCI_VERSION (0 = probe not run / unsupported).
static PSCI_VERSION_REG: AtomicU32 = AtomicU32::new(0);

/// Issue a PSCI call through the detected conduit. Returns x0 (signed).
#[inline]
fn psci_call(fnid: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    if CONDUIT_HVC.load(Ordering::Relaxed) {
        // SAFETY: HVC #0 at EL1 is the PSCI conduit (firmware call); the
        // register contract is the PSCI 0.2 calling convention. The call
        // clobbers x0-x3 (return + returns-in-x1/x2/x3 for CPU_ON none).
        unsafe {
            core::arch::asm!(
                "hvc #0",
                inlateout("x0") fnid => ret,
                in("x1") a1,
                in("x2") a2,
                in("x3") a3,
                lateout("x1") _,
                lateout("x2") _,
                lateout("x3") _,
                options(nostack)
            );
        }
    } else {
        // SAFETY: SMC #0 conduit variant (same calling convention).
        unsafe {
            core::arch::asm!(
                "smc #0",
                inlateout("x0") fnid => ret,
                in("x1") a1,
                in("x2") a2,
                in("x3") a3,
                lateout("x1") _,
                lateout("x2") _,
                lateout("x3") _,
                options(nostack)
            );
        }
    }
    ret
}

/// Parse the DTB `/psci` node for the conduit method and the CPU_ON
/// function id. Mirrors the manual FDT walk in cmdline.rs (arch-neutral,
/// no heap). Returns false when no /psci node exists (caller defaults).
unsafe fn detect_psci_from_dtb() -> bool {
    // SAFETY: the DTB physical address is fixed by QEMU before boot and
    // the linear map covers all of RAM (same translation the memory-region
    // parser in main.rs uses post-MMU).
    let dtb_phys = super::boot::get_dtb_pointer();
    let dtb_virt = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(dtb_phys)).bits();
    let fdt = dtb_virt as *const u8;

    let read_u32 = |p: *const u8| -> u32 {
        // SAFETY: FDT structure is 4-byte aligned big-endian; the walker
        // only advances within [fdt, fdt+totalsize) as validated below.
        unsafe {
            ((*p as u32) << 24)
                | ((*p.add(1) as u32) << 16)
                | ((*p.add(2) as u32) << 8)
                | (*p.add(3) as u32)
        }
    };

    // SAFETY: header reads stay inside the FDT blob (validated by magic).
    unsafe {
        if read_u32(fdt) != 0xd00d_feed {
            return false;
        }
        let off_dt_struct = read_u32(fdt.add(0x08)) as usize;
        let off_dt_strings = read_u32(fdt.add(0x0C)) as usize;
        let size_dt_struct = read_u32(fdt.add(0x24)) as usize;
        let strings = fdt.add(off_dt_strings);

        const FDT_BEGIN_NODE: u32 = 0x1;
        const FDT_END_NODE: u32 = 0x2;
        const FDT_PROP: u32 = 0x3;
        const FDT_END: u32 = 0x9;

        let mut ptr = fdt.add(off_dt_struct);
        let end = ptr.add(size_dt_struct);
        let mut depth = 0usize;
        let mut in_psci = false;
        let mut found = false;

        while ptr < end {
            let token = read_u32(ptr);
            ptr = ptr.add(4);
            match token {
                FDT_BEGIN_NODE => {
                    let mut name = [0u8; 32];
                    let mut i = 0;
                    while *ptr != 0 && i < 31 {
                        name[i] = *ptr;
                        ptr = ptr.add(1);
                        i += 1;
                    }
                    ptr = ptr.add(1); // NUL
                    ptr = ptr.add((4 - ((ptr as usize) & 3)) & 3);
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
                    let nameoff = read_u32(ptr.add(4)) as usize;
                    ptr = ptr.add(8);
                    let mut name_ptr = strings.add(nameoff);
                    let mut pname = [0u8; 16];
                    let mut i = 0;
                    while *name_ptr != 0 && i < 15 {
                        pname[i] = *name_ptr;
                        name_ptr = name_ptr.add(1);
                        i += 1;
                    }
                    if in_psci {
                        match &pname[..i] {
                            b"method" => {
                                let m = core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                                    ptr,
                                    len.min(4),
                                ));
                                let hvc = m.trim_end_matches('\0') == "hvc";
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
                    ptr = ptr.add(len);
                    ptr = ptr.add((4 - ((ptr as usize) & 3)) & 3);
                }
                FDT_END => break,
                _ => break,
            }
        }
        found
    }
}

// ==================== Boot-complete handshake ====================

/// Flag set by boot CPU after ALL initialization is complete.
static BOOT_COMPLETE: core::sync::atomic::AtomicBool =
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

/// Saved TTBR1_EL1 (kernel root) from the boot core, for the secondary
/// stub in boot.S (the aarch64 counterpart of riscv64's BOOT_HART_SATP).
#[no_mangle]
pub static BOOT_HART_TTBR: AtomicU64 = AtomicU64::new(0);
/// Saved TCR_EL1 from the boot core.
#[no_mangle]
pub static BOOT_HART_TCR: AtomicU64 = AtomicU64::new(0);
/// Saved MAIR_EL1 from the boot core.
#[no_mangle]
pub static BOOT_HART_MAIR: AtomicU64 = AtomicU64::new(0);

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
struct BootStack([u8; STACK_SIZE]);

#[no_mangle]
#[used]
pub static SECONDARY_BOOT_STACKS: [BootStack; MAX_CPUS] = [
    BootStack([0u8; STACK_SIZE]),
    BootStack([0u8; STACK_SIZE]),
    BootStack([0u8; STACK_SIZE]),
    BootStack([0u8; STACK_SIZE]),
];

/// Start secondary CPUs via PSCI CPU_ON.
/// Must be called AFTER console + scheduler initialization.
pub fn start_secondaries() {
    // 1. Publish the boot core's live MMU state for the secondary stub.
    // SAFETY: plain EL1 system-register reads, no side effects.
    let (ttbr1, tcr, mair): (u64, u64, u64);
    unsafe {
        core::arch::asm!(
            "mrs {}, ttbr1_el1",
            "mrs {}, tcr_el1",
            "mrs {}, mair_el1",
            out(reg) ttbr1,
            out(reg) tcr,
            out(reg) mair,
            options(nomem, nostack)
        );
    }
    BOOT_HART_TTBR.store(ttbr1, Ordering::Release);
    BOOT_HART_TCR.store(tcr, Ordering::Release);
    BOOT_HART_MAIR.store(mair, Ordering::Release);

    // 2. Conduit + version probe. The DTB /psci node decides hvc vs smc;
    //    defaults (hvc, PSCI 0.2 CPU_ON id) already match QEMU virt.
    let have_dtb = unsafe {
        // SAFETY: FDT walker only reads the blob through the linear map.
        detect_psci_from_dtb()
    };
    let ver = psci_call(PSCI_VERSION, 0, 0, 0);
    if ver >= 0 {
        PSCI_VERSION_REG.store(ver as u32, Ordering::Release);
    }

    // 3. Physical entry address of the secondary stub: the symbol is
    //    linked in the high VA window; PSCI starts the core with the MMU
    //    off, so hand it the physical address (same VA-PA delta boot.S
    //    published in KERNEL_MAP).
    // SAFETY: KERNEL_MAP is populated by boot.S before rust_main runs.
    let va_kernel_pa_offset = unsafe { crate::arch::mm::memory_layout::KERNEL_MAP.va_kernel_pa_offset };
    // Two-step cast: the fn item has type `unsafe extern "C" fn()` — cast
    // to its own pointer type first (a safe-fn pointer cast is rejected),
    // then fn-pointer → usize is a primitive cast.
    let entry_pa = (secondary_start as unsafe extern "C" fn() as usize)
        .wrapping_sub(va_kernel_pa_offset);

    // 4. CPU_ON every secondary. Expected-online accounting follows the
    //    riscv64 twin: QEMU may run fewer cores than MAX_CPUS (-smp 2
    //    with MAX_CPUS=4); CPU_ON for an absent core returns
    //    INVALID_PARAMS and that core can never come online.
    let my_cpu = cpu_id();
    let mut expected_cpus = 1usize; // the boot core itself
    let mut failed = 0usize;

    for cpu in 0..MAX_CPUS {
        if cpu == my_cpu {
            continue;
        }
        // dsb before the firmware call so the published MMU state and the
        // .bss stacks are visible to the waking core.
        // SAFETY: memory barrier, no side effects.
        unsafe { core::arch::asm!("dsb sy", options(nomem, nostack)) };
        let ret = psci_call(
            CPU_ON_FN.load(Ordering::Acquire) as u64,
            index_to_mpidr(cpu),
            entry_pa as u64,
            cpu as u64, // context_id → x0 at the secondary entry point
        );
        if ret == 0 || ret == PSCI_ALREADY_ON {
            expected_cpus += 1;
        } else if ret == PSCI_INVALID_PARAMS || ret == PSCI_NOT_SUPPORTED {
            // Core absent on this machine (or PSCI CPU_ON missing) —
            // folded into the online count, same policy as the twin's
            // SBI_INVALID_PARAM.
        } else {
            failed += 1;
            crate::println!("smp: cpu {} PSCI CPU_ON failed (error={})", cpu, ret);
        }
    }

    // 5. Bounded wait for secondaries to reach secondary_cpu_entry (a
    //    core that started but wedged must not hang boot forever).
    for _ in 0..50_000_000 {
        if num_started_cpus() >= expected_cpus {
            break;
        }
        core::hint::spin_loop();
    }

    // 6. Structured status row.
    let count = num_started_cpus();
    let v = PSCI_VERSION_REG.load(Ordering::Relaxed);
    let conduit = if CONDUIT_HVC.load(Ordering::Relaxed) {
        "hvc"
    } else {
        "smc"
    };
    if failed == 0 {
        crate::print_status(
            "smp",
            &alloc::format!(
                "{} CPU{} online (boot cpu {}, PSCI {}.{} {}, dtb-psci {})",
                count,
                if count > 1 { "s" } else { "" },
                my_cpu,
                v >> 16,
                v & 0xFFFF,
                conduit,
                have_dtb
            ),
            count == expected_cpus,
        );
    } else {
        crate::print_status(
            "smp",
            &alloc::format!(
                "{} CPUs online, {} CPU_ON failure(s) (PSCI {}.{} {})",
                count, failed, v >> 16, v & 0xFFFF, conduit
            ),
            false,
        );
    }
}

/// Rust entry point for secondary CPUs — called from boot.S
/// `secondary_start` after the MMU is on, with x0 = cpu index (the PSCI
/// context_id; TPIDR_EL1 is already set by the stub).
#[no_mangle]
pub extern "C" fn secondary_cpu_entry(cpu: usize) -> ! {
    // Install the high-VA vector table BEFORE marking this CPU started:
    // the stub parked VBAR_EL1 at the table's PHYSICAL address (fetched
    // through the early identity map). cpu_started(n) must imply "n no
    // longer needs any low VA" — mmu_init::teardown_early_identity_map()
    // runs on the boot core right after start_secondaries() and relies
    // on exactly that invariant (a started-but-late core would fault on
    // its own bad-vector park otherwise).
    crate::arch::trap::init();

    // Mark this CPU as started so the boot CPU's start_secondaries()
    // spin-wait can detect us and proceed.
    mark_cpu_started(cpu);

    // Configure THIS cpu's GIC bank and unmask DAIF BEFORE the
    // boot-complete wait: the wait parks in wfi, and a halted core whose
    // pending interrupt is masked never wakes to re-evaluate the loop
    // condition — the secondary parked there for the whole boot
    // (invisible while the GICD_CTLR bug kept every interrupt line
    // deasserted). With the CPU interface up and DAIF clear, the armed
    // vtimer (and the boot cpu's completion kick) break the wfi properly
    // and the timer's early-boot guard makes the pre-init ticks harmless.
    crate::arch::trap::enable_external_interrupt();
    crate::arch::trap::enable_timer_interrupt();

    // Wait for boot CPU to finish ALL single-CPU initialization.
    // WFI yields the CPU in QEMU tcg mode.
    while !is_boot_complete() {
        crate::arch::cpu::wfi();
    }

    // Boot CPU has finished init — safe to call kmalloc now.
    crate::sched::init_secondary(cpu);

    // Enter scheduler idle loop (timer interrupts enabled inside the loop)
    crate::sched::cpu_idle_loop();
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
struct AlignedStack([u8; INTR_STACK_SIZE]);

impl AlignedStack {
    /// Get a pointer to the underlying stack buffer
    pub const fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr()
    }
}

/// Per-CPU interrupt stacks (16-byte aligned for PtRegs access)
#[link_section = ".bss"]
#[used]
pub static mut PER_CPU_INTR_STACKS: [AlignedStack; MAX_CPUS] =
    [const { AlignedStack([0u8; INTR_STACK_SIZE]) }; MAX_CPUS];

/// Initialize per-CPU interrupt stack base pointer for assembly code
pub fn init_per_cpu_intr_stacks() {
    // SAFETY: only called once during early boot before any traps occur.
    unsafe {
        extern "C" {
            static mut __per_cpu_intr_stacks_base: usize;
        }
        __per_cpu_intr_stacks_base = PER_CPU_INTR_STACKS.as_ptr() as usize;
    }
}

/// Get current CPU's interrupt stack top address
#[no_mangle]
#[inline(never)]
pub extern "C" fn get_per_cpu_intr_stack_top() -> usize {
    let cpu = cpu_id();
    if cpu < MAX_CPUS {
        // SAFETY: bounds-checked index into the static per-CPU array.
        unsafe {
            let stack_base = PER_CPU_INTR_STACKS[cpu].as_ptr() as usize;
            stack_base + INTR_STACK_SIZE
        }
    } else {
        // SAFETY: PER_CPU_INTR_STACKS[0] always exists (MAX_CPUS >= 1).
        unsafe {
            let stack_base = PER_CPU_INTR_STACKS[0].as_ptr() as usize;
            stack_base + INTR_STACK_SIZE
        }
    }
}
