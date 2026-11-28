//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 SMP: per-CPU state (GS-based), AP bring-up through the real
//! mode trampoline at 0x8000 (SIPI), and boot-completion gating.
//!
//! ## GS discipline
//!
//! Kernel GS base (IA32_GS_BASE) points at this CPU's [`PerCpu`];
//! the user shadow (IA32_KERNEL_GS_BASE) stays 0 — glibc keeps its TLS
//! on FS, so a NULL user GS is the correct ABI state.  Trap/syscall
//! entries from user mode execute SWAPGS (see trap.S) and the
//! return-to-user path pairs it back; the trap stubs read the CPU
//! number from `%gs:PC_CPU_NUMBER`.
//!
//! ## AP bring-up (per CPU)
//!
//! 1. BSP parses the ACPI MADT (apic driver) and numbers CPUs, BSP=0.
//! 2. BSP copies the trampoline blob to 0x8000, builds trampoline page
//!    tables at 0x9000-0xBFFF (identity low 1GB + kernel upper half)
//!    and pokes the data block (stack, GS base, kernel entry).
//! 3. INIT-SIPI-SIPI through the local APIC ICR.
//! 4. The AP walks real mode → 32-bit → 64-bit on the trampoline tables,
//!    loads its GS base, and lands in [`ap_entry64`].
//! 5. `ap_entry64` switches to the kernel CR3, brings up its GDT/TSS/
//!    IDT/LAPIC, publishes `started`, and parks (HLT with IRQs on)
//!    until the BSP signals boot completion, then joins the scheduler.
//!
//! Per-CPU data follows the cache-line-partitioned discipline
//! (docs/development/rust-kernel-best-practices.md §1 + survey A1):
//! CPU-private fields and cross-CPU fields never share a line, and the
//! CPU publishes its own id inside the struct.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::arch::cpu::{rdmsr, wrmsr};
use crate::config::MAX_CPUS;

// The trampoline blob (copied to physical 0x8000; see smp_trampoline.S).


// ============================================================================
// Per-CPU area — offsets are consumed by trap.S (PC_* asserts below)
// ============================================================================

/// GS-relative offsets used by assembly (trap.S `.set PC_*` mirrors).
pub const PC_CPU_NUMBER: usize = 0x00;
pub const PC_LAPIC_ID: usize = 0x04;
pub const PC_TSS_RSP0: usize = 0x08;
pub const PC_SYSCALL_SCRATCH: usize = 0x10;
pub const PC_CURRENT_TASK: usize = 0x18;
pub const PC_KERNEL_STACK_TOP: usize = 0x20;
pub const PC_CPU_ONLINE: usize = 0x28;
pub const PC_STARTED: usize = 0x2C;
/// GS-base authenticity magic (SCRIBBLE2): low dword mirrors the CPU
/// number, high dword is a fixed pattern.  Trap/syscall entries whose
/// %gs-relative read does not show the pattern know the ACTIVE GS base is
/// not a PerCpu slot and heal with SWAPGS before touching anything else.
/// A zero page (the classic user mapping at VA 0 that defeated the old
/// "cpu number < 64" heuristic) can never match.
pub const PC_MAGIC: usize = 0x30;
/// The high-dword pattern (0x52585258 = "RXRX").
pub const PC_MAGIC_HI: u32 = 0x5258_5258;

#[repr(C)]
#[repr(align(64))]
pub struct PerCpu {
    // --- line 0: asm-visible scalars ---
    /// This CPU's number (0 = BSP). Published before the CPU runs Rust.
    pub cpu_number: AtomicU32,
    /// LAPIC id (diagnostics; set at bring-up).
    pub lapic_id: AtomicU32,
    /// Kernel stack top mirror of TSS.rsp0 — syscall_entry reads this
    /// through %gs (TSS.rsp0 itself serves interrupt entries).
    pub tss_rsp0: AtomicU64,
    /// User-RSP stash for syscall_entry (TSS.sp2 role, per-CPU).
    pub syscall_scratch: AtomicU64,

    // --- line 0 (cont.): CPU-private ---
    /// Current task pointer (0 = idle/none)
    pub current_task: AtomicU64,
    /// Kernel stack top for TSS.rsp0 (private)
    pub kernel_stack_top: AtomicU64,

    // --- cross-CPU tail ---
    /// Published once at startup (0 = offline)
    pub cpu_online: AtomicU32,
    /// Started-secondary wait flag (SIPI handshake)
    pub started: AtomicU32,
    /// Authenticity magic (see PC_MAGIC): {pattern, cpu_number}.
    pub magic: AtomicU64,
    pub(crate) _pad: [u32; 12],
}

impl PerCpu {
    pub(crate) const fn new() -> Self {
        PerCpu {
            cpu_number: AtomicU32::new(0),
            lapic_id: AtomicU32::new(u32::MAX),
            tss_rsp0: AtomicU64::new(0),
            syscall_scratch: AtomicU64::new(0),
            current_task: AtomicU64::new(0),
            kernel_stack_top: AtomicU64::new(0),
            cpu_online: AtomicU32::new(0),
            started: AtomicU32::new(0),
            magic: AtomicU64::new(0),
            _pad: [0; 12],
        }
    }

    /// Arm this slot's authenticity magic (cpu embedded, pattern on top).
    pub(crate) fn arm_magic(&self, cpu: usize) {
        let m = ((PC_MAGIC_HI as u64) << 32) | cpu as u64;
        self.magic.store(m, Ordering::Release);
    }
}

/// The BSP uses slot 0; SIPI secondaries take 1..MAX_CPUS
pub static PER_CPU: [PerCpu; MAX_CPUS] = [const { PerCpu::new() }; MAX_CPUS];

// Pin the asm-visible offsets to the actual struct layout.
pub(crate) const _: () = assert!(core::mem::offset_of!(PerCpu, cpu_number) == PC_CPU_NUMBER);
pub(crate) const _: () = assert!(core::mem::offset_of!(PerCpu, lapic_id) == PC_LAPIC_ID);
pub(crate) const _: () = assert!(core::mem::offset_of!(PerCpu, tss_rsp0) == PC_TSS_RSP0);
pub(crate) const _: () = assert!(core::mem::offset_of!(PerCpu, syscall_scratch) == PC_SYSCALL_SCRATCH);
pub(crate) const _: () = assert!(core::mem::offset_of!(PerCpu, current_task) == PC_CURRENT_TASK);
pub(crate) const _: () = assert!(core::mem::offset_of!(PerCpu, kernel_stack_top) == PC_KERNEL_STACK_TOP);
pub(crate) const _: () = assert!(core::mem::offset_of!(PerCpu, cpu_online) == PC_CPU_ONLINE);
pub(crate) const _: () = assert!(core::mem::offset_of!(PerCpu, started) == PC_STARTED);
pub(crate) const _: () = assert!(core::mem::offset_of!(PerCpu, magic) == PC_MAGIC);

// ============================================================================
// GS base management
// ============================================================================

pub(crate) const MSR_GS_BASE: u32 = 0xC000_0101;
pub(crate) const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;
/// Point this CPU's GS base at its PerCpu slot and zero the user shadow
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-25): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn load_per_cpu_base(cpu: usize) {


                                 let base = &PER_CPU[cpu] as *const PerCpu as u64;
                                 // SAFETY: WRMSR of the GS base MSRs with a kernel .bss address and
                                 // zero; both values are valid in any context that runs kernel code.

                                     wrmsr(MSR_GS_BASE, base);
                                     wrmsr(MSR_KERNEL_GS_BASE, 0);


}

// ============================================================================
// Boot-completion gate (interface parity with the riscv64 twin)
// ============================================================================

pub(crate) static BOOT_COMPLETE: AtomicBool = AtomicBool::new(false);

/// BSP signal: secondaries may join the scheduler.
pub fn signal_boot_complete() {
    BOOT_COMPLETE.store(true, Ordering::Release);
}

pub fn is_boot_complete() -> bool {
    BOOT_COMPLETE.load(Ordering::Acquire)
}

// ============================================================================
// Init / status
// ============================================================================

/// BSP early init: GS base + slot 0.  Called as the FIRST statement of
/// rust_main, before anything can read the per-CPU area.
///
/// NOTE: no LAPIC register access here — 0xFEE00000 lives in the 4GB
/// hole, outside the boot stub's identity map (0..2GB); the lapic_id
/// field is filled by the intc driver's init, after the device mapping
/// exists.
pub fn init() {
    load_per_cpu_base(0);
    PER_CPU[0].cpu_number.store(0, Ordering::Release);
    PER_CPU[0].cpu_online.store(1, Ordering::Release);
    PER_CPU[0].started.store(1, Ordering::Release);
    PER_CPU[0].arm_magic(0);
}

/// (interface parity — per-CPU interrupt stacks are IST/TSS on x86)
pub fn init_per_cpu_intr_stacks() {}

/// Number of CPUs online (started flag set).
pub fn num_started_cpus() -> usize {
    PER_CPU.iter().filter(|c| c.started.load(Ordering::Acquire) == 1).count()
}

pub fn cpu_started(cpu: usize) -> bool {
    cpu < MAX_CPUS && PER_CPU[cpu].started.load(Ordering::Acquire) == 1
}

/// Is this the boot CPU? (x86 numbering: BSP is always CPU 0.)
pub fn is_boot_hart() -> bool {
    crate::arch::cpu_id() == 0
}
/// Current CPU number via GS (used by trap.S and mod::cpu_id).
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-24): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn gs_cpu_id() -> u64 {
    crate::arch::arch_floor::x86_64::smp::gs_cpu_id()
}

/// Current CPU number (interface parity with the riscv64 twin — generic
/// code calls `arch::smp::cpu_id`).
#[inline]
pub fn cpu_id() -> u64 {
    gs_cpu_id()
}

// ---- current-task slot ----

pub fn current_task_ptr() -> u64 {
    PER_CPU[crate::arch::cpu_id() as usize].current_task.load(Ordering::Acquire)
}

/// Read ANOTHER CPU's current-task slot (adjudicates a
/// sched-slot "parked" classification against the arch tp truth — a
/// stale sched slot names someone else while the task still runs).
pub fn current_task_ptr_of(cpu: usize) -> u64 {
    if cpu < PER_CPU.len() {
        PER_CPU[cpu].current_task.load(Ordering::Acquire)
    } else {
        0
    }
}

pub fn set_current_task_ptr(task: u64) {
    PER_CPU[crate::arch::cpu_id() as usize].current_task.store(task, Ordering::Release);
}

// ============================================================================
// Trampoline
// ============================================================================

/// Trampoline physical base (Linux-real-mode-trampoline convention).
pub(crate) const TRAMP_PHYS: usize = 0x8000;
/// Trampoline blob size (see smp_trampoline.S layout: end at org 0x0F00).
pub(crate) const TRAMP_SIZE: usize = 0x0F00;

// Data-block org offsets inside the trampoline (smp_trampoline.S).
pub(crate) const TRAMP_GDTR_BASE: usize = 0x0E42; // u32 gdtr base (after the u16 limit)
pub(crate) const TRAMP_PML4: usize = 0x0E50; // u32 trampoline PML4 PA
pub(crate) const TRAMP_CPU: usize = 0x0E54; // u32 cpu number
pub(crate) const TRAMP_STACK: usize = 0x0E58; // u64 stack top VA
pub(crate) const TRAMP_GSBASE: usize = 0x0E60; // u64 GS base VA
pub(crate) const TRAMP_ENTRY: usize = 0x0E68; // u64 ap_entry64 VA
pub(crate) const TRAMP_KCR3: usize = 0x0E70; // u64 kernel CR3 (diagnostics)
pub(crate) const TRAMP_FAR16_PM32: usize = 0x0E80; // u16 org-offset of pm32
pub(crate) const TRAMP_FAR64_LONG: usize = 0x0E88; // u32 LINEAR offset of long64

/// Trampoline page tables: PML4 at 0x9000, PDPT at 0xA000, PD at 0xB000
/// (identity-map the first 1GB with 2MB leaves).
pub(crate) const TRAMP_PML4_PHYS: usize = 0x9000;
pub(crate) const TRAMP_PDPT_PHYS: usize = 0xA000;
pub(crate) const TRAMP_PD_PHYS: usize = 0xB000;

pub(crate) const PDE_P: u64 = 1 << 0;
pub(crate) const PDE_RW: u64 = 1 << 1;
pub(crate) const PDE_PS: u64 = 1 << 7;

extern "C" {
    static __ap_trampoline_start: u8;
    static __ap_trampoline_end: u8;
    static tramp_gdt: u8;
    static pm32: u8;
    static long64: u8;
}

/// SMP boot stack size (interface parity with the riscv64 twin).
pub const STACK_SIZE: usize = crate::config::SMP_BOOT_STACK_SIZE;

#[repr(align(16))]
pub(crate) struct BootStack(pub(crate) [u8; STACK_SIZE]);


/// R87 (stale-rsp0 feeder, oh-launch round 10): the address range of the
/// static per-CPU boot stacks. The idle tasks run on these stacks — their
/// tops lie OUTSIDE the kernel-heap range that x86_update_rsp0's
/// corruption screen accepts, so every switch to idle silently kept the
/// PREVIOUS task's rsp0 (the dying task's top staying installed while its
/// stack was freed and recycled — the live-stack foreign-content writer
/// fr3 caught with STACKFREE-LIVE-RSP0). The screen now accepts this
/// range explicitly.
pub fn ap_boot_stacks_range() -> (usize, usize) {
    let base = AP_BOOT_STACKS.as_ptr() as usize;
    (base, base + STACK_SIZE * MAX_CPUS)
}
/// Write into the trampoline copy at low memory through the linear map.
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-22): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn tramp_write(off: usize, bytes: &[u8]) {
    crate::arch::arch_floor::x86_64::smp::tramp_write(off, bytes)
}

pub(crate) fn tramp_write_u64(off: usize, v: u64) {
    tramp_write(off, &v.to_le_bytes());
}

pub(crate) fn tramp_write_u32(off: usize, v: u32) {
    tramp_write(off, &v.to_le_bytes());
}

pub(crate) fn tramp_write_u16(off: usize, v: u16) {
    tramp_write(off, &v.to_le_bytes());
}
/// Copy the trampoline blob to 0x8000 and build its page tables.
///
/// Collected into `arch::arch_floor` (INV-ARCH-SMP-21): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn setup_trampoline(kernel_cr3: u64) {
    crate::arch::arch_floor::x86_64::smp::setup_trampoline(kernel_cr3)
}

/// Linear-map VA of a low physical address.
pub(crate) fn tramp_va(phys: usize) -> usize {
    crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new(phys as u64),
    )
    .as_usize()
}

// ============================================================================
// Secondary bring-up (BSP side)
// ============================================================================

/// Bring up secondary CPUs via INIT-SIPI-SIPI.  Must run after the
/// scheduler pre-created the idle tasks and after the APIC driver is
/// initialized; degrades to a no-op without a MADT/APIC.
pub fn start_secondaries() {
    let discovered = crate::drivers::intc::apic::cpu_count();
    if discovered <= 1 || !crate::drivers::intc::apic::ready() {
        crate::print_status(
            "smp",
            "single CPU (no APIC/MADT secondaries)",
            true,
        );
        return;
    }

    let kernel_cr3 = crate::arch::cpu::read_cr3();
    setup_trampoline(kernel_cr3);

    let target = discovered.min(MAX_CPUS);
    let mut expected = 1usize;
    let mut failures = 0usize;

    for cpu in 1..target {
        let lapic = crate::drivers::intc::apic::lapic_id(cpu);
        if lapic == u32::MAX {
            continue;
        }

        // Reset the slot and poke the per-CPU truth + trampoline data.
        PER_CPU[cpu].cpu_number.store(cpu as u32, Ordering::Release);
        PER_CPU[cpu].lapic_id.store(lapic, Ordering::Release);
        PER_CPU[cpu].started.store(0, Ordering::Release);
        PER_CPU[cpu].cpu_online.store(0, Ordering::Release);
        PER_CPU[cpu].arm_magic(cpu);

        let stack_top = AP_BOOT_STACKS[cpu].0.as_ptr() as usize + STACK_SIZE;
        tramp_write_u32(TRAMP_CPU, cpu as u32);
        tramp_write_u64(TRAMP_STACK, stack_top as u64);
        tramp_write_u64(
            TRAMP_GSBASE,
            &PER_CPU[cpu] as *const PerCpu as u64,
        );

        // INIT assert -> SIPI -> SIPI (MP spec bring-up sequence).
        crate::drivers::intc::apic::send_init(lapic);
        crate::drivers::intc::apic::delay_ms(10);
        crate::drivers::intc::apic::send_sipi(lapic, (TRAMP_PHYS >> 12) as u8);
        // Spec allows up to 200us before the second SIPI; the bounded
        // spin also covers slow TCG APs racing to set `started`.
        if !wait_started(cpu, 2_000_000) {
            crate::drivers::intc::apic::send_sipi(lapic, (TRAMP_PHYS >> 12) as u8);
        }
        if wait_started(cpu, 200_000_000) {
            expected += 1;
        } else {
            failures += 1;
            crate::println!("smp: CPU {} (lapic {:#x}) failed to start", cpu, lapic);
        }
    }

    let online = num_started_cpus();
    // Post-bring-up affinity refresh (see Task::refresh_default_affinity_to_online).
    crate::process::task::Task::refresh_default_affinity_to_online();
    if failures == 0 {
        crate::print_status(
            "smp",
            &alloc::format!("{} CPU{} online", online, if online > 1 { "s" } else { "" }),
            online == expected,
        );
    } else {
        crate::print_status(
            "smp",
            &alloc::format!("{} CPUs online, {} SIPI failure(s)", online, failures),
            false,
        );
    }

    smp_selftest();
}

/// Boot-time SMP smoke test (cheap, always on): one CSD round trip to
/// every AP (exercises the CALL_FUNCTION IPI vector + CSD queue +
/// completion handshake) and one TLB-flush broadcast (TLB_FLUSH vector
/// on every CPU — full local CR3 reload).
pub(crate) fn smp_selftest() {
    if num_started_cpus() < 2 {
        return;
    }
    static REMOTE_BUMPS: AtomicU64 = AtomicU64::new(0);
    static REMOTE_TICKS: AtomicU64 = AtomicU64::new(0);
    fn bump(_arg: *mut core::ffi::c_void) {
        // Executed ON the remote CPU: snapshot this CPU's timer-IRQ
        // counter as proof the per-CPU LAPIC timer ticks there.
        let cpu = crate::arch::cpu_id() as usize;
        REMOTE_TICKS.fetch_add(
            crate::fs::procfs::interrupts::timer_count(cpu),
            Ordering::SeqCst,
        );
        REMOTE_BUMPS.fetch_add(1, Ordering::SeqCst);
    }

    let me = crate::arch::cpu_id() as usize;
    let mut targets = 0usize;
    for cpu in 0..MAX_CPUS {
        if cpu != me && cpu_started(cpu) {
            crate::arch::ipi::smp_call_function(cpu, bump, core::ptr::null_mut());
            targets += 1;
        }
    }
    let bumps = REMOTE_BUMPS.load(Ordering::Acquire);

    // Fire-and-forget TLB flush broadcast: exercises the remote vector
    // and the CR3-reload handler on every peer.
    crate::arch::ipi::flush_tlb_others(0, 0);

    crate::print_status(
        "smp",
        &alloc::format!(
            "IPI selftest {}/{} calls, {} remote ticks",
            bumps, targets, REMOTE_TICKS.load(Ordering::Acquire)
        ),
        bumps == targets as u64,
    );
}

/// Bounded spin until `cpu` publishes its started flag.
pub(crate) fn wait_started(cpu: usize, spins: u64) -> bool {
    for _ in 0..spins {
        if PER_CPU[cpu].started.load(Ordering::Acquire) == 1 {
            return true;
        }
        core::hint::spin_loop();
    }
    PER_CPU[cpu].started.load(Ordering::Acquire) == 1
}

// ============================================================================
// AP entry (runs on the AP, on its boot stack, GS already set)
// ============================================================================

// Collected-unsafe re-exports: the unsafe floor for these names lives in
// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.
// INV-ARCH note: ap_entry64 moved to arch::arch_floor::x86_64::smp (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::x86_64::smp::ap_entry64;
pub(crate) use crate::arch::arch_floor::x86_64::smp::AP_BOOT_STACKS;
