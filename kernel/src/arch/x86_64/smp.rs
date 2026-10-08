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

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::arch::cpu::{rdmsr, wrmsr};
use crate::config::MAX_CPUS;

// The trampoline blob (copied to physical 0x8000; see smp_trampoline.S).
core::arch::global_asm!(include_str!("smp_trampoline.S"), options(att_syntax));

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
    _pad: [u32; 14],
}

impl PerCpu {
    const fn new() -> Self {
        PerCpu {
            cpu_number: AtomicU32::new(0),
            lapic_id: AtomicU32::new(u32::MAX),
            tss_rsp0: AtomicU64::new(0),
            syscall_scratch: AtomicU64::new(0),
            current_task: AtomicU64::new(0),
            kernel_stack_top: AtomicU64::new(0),
            cpu_online: AtomicU32::new(0),
            started: AtomicU32::new(0),
            _pad: [0; 14],
        }
    }
}

/// The BSP uses slot 0; SIPI secondaries take 1..MAX_CPUS
pub static PER_CPU: [PerCpu; MAX_CPUS] = [const { PerCpu::new() }; MAX_CPUS];

// Pin the asm-visible offsets to the actual struct layout.
const _: () = assert!(core::mem::offset_of!(PerCpu, cpu_number) == PC_CPU_NUMBER);
const _: () = assert!(core::mem::offset_of!(PerCpu, lapic_id) == PC_LAPIC_ID);
const _: () = assert!(core::mem::offset_of!(PerCpu, tss_rsp0) == PC_TSS_RSP0);
const _: () = assert!(core::mem::offset_of!(PerCpu, syscall_scratch) == PC_SYSCALL_SCRATCH);
const _: () = assert!(core::mem::offset_of!(PerCpu, current_task) == PC_CURRENT_TASK);
const _: () = assert!(core::mem::offset_of!(PerCpu, kernel_stack_top) == PC_KERNEL_STACK_TOP);
const _: () = assert!(core::mem::offset_of!(PerCpu, cpu_online) == PC_CPU_ONLINE);
const _: () = assert!(core::mem::offset_of!(PerCpu, started) == PC_STARTED);

// ============================================================================
// GS base management
// ============================================================================

const MSR_GS_BASE: u32 = 0xC000_0101;
const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;

/// Point this CPU's GS base at its PerCpu slot and zero the user shadow
/// (glibc TLS lives on FS; a NULL user GS is the correct state).
///
/// Kernel GS = percpu / user GS = 0; entries/exits pair SWAPGS (trap.S).
pub fn load_per_cpu_base(cpu: usize) {
    let base = &PER_CPU[cpu] as *const PerCpu as u64;
    // SAFETY: WRMSR of the GS base MSRs with a kernel .bss address and
    // zero; both values are valid in any context that runs kernel code.
    unsafe {
        wrmsr(MSR_GS_BASE, base);
        wrmsr(MSR_KERNEL_GS_BASE, 0);
    }
}

// ============================================================================
// Boot-completion gate (interface parity with the riscv64 twin)
// ============================================================================

static BOOT_COMPLETE: AtomicBool = AtomicBool::new(false);

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
#[inline]
pub fn gs_cpu_id() -> u64 {
    let id: u32;
    // SAFETY: %gs:0 reads this CPU's PerCpu.cpu_number; the GS base is
    // established before any code that can reach here runs.
    unsafe {
        core::arch::asm!(
            "mov {0:e}, dword ptr gs:[0]",
            out(reg) id,
            options(nomem, nostack, preserves_flags)
        );
    }
    id as u64
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

pub fn set_current_task_ptr(task: u64) {
    PER_CPU[crate::arch::cpu_id() as usize].current_task.store(task, Ordering::Release);
}

// ============================================================================
// Trampoline
// ============================================================================

/// Trampoline physical base (Linux-real-mode-trampoline convention).
const TRAMP_PHYS: usize = 0x8000;
/// Trampoline blob size (see smp_trampoline.S layout: end at org 0x0F00).
const TRAMP_SIZE: usize = 0x0F00;

// Data-block org offsets inside the trampoline (smp_trampoline.S).
const TRAMP_GDTR_BASE: usize = 0x0E42; // u32 gdtr base (after the u16 limit)
const TRAMP_PML4: usize = 0x0E50; // u32 trampoline PML4 PA
const TRAMP_CPU: usize = 0x0E54; // u32 cpu number
const TRAMP_STACK: usize = 0x0E58; // u64 stack top VA
const TRAMP_GSBASE: usize = 0x0E60; // u64 GS base VA
const TRAMP_ENTRY: usize = 0x0E68; // u64 ap_entry64 VA
const TRAMP_KCR3: usize = 0x0E70; // u64 kernel CR3 (diagnostics)
const TRAMP_FAR16_PM32: usize = 0x0E80; // u16 org-offset of pm32
const TRAMP_FAR64_LONG: usize = 0x0E88; // u32 LINEAR offset of long64

/// Trampoline page tables: PML4 at 0x9000, PDPT at 0xA000, PD at 0xB000
/// (identity-map the first 1GB with 2MB leaves).
const TRAMP_PML4_PHYS: usize = 0x9000;
const TRAMP_PDPT_PHYS: usize = 0xA000;
const TRAMP_PD_PHYS: usize = 0xB000;

const PDE_P: u64 = 1 << 0;
const PDE_RW: u64 = 1 << 1;
const PDE_PS: u64 = 1 << 7;

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
struct BootStack([u8; STACK_SIZE]);

/// Per-AP boot stacks (static so the BSP can poke the pointer before
/// kmalloc-safe concurrency exists).
#[link_section = ".bss.stack"]
static AP_BOOT_STACKS: [BootStack; MAX_CPUS] = [const { BootStack([0; STACK_SIZE]) }; MAX_CPUS];

/// Write into the trampoline copy at low memory through the linear map.
fn tramp_write(off: usize, bytes: &[u8]) {
    let va = crate::arch::mm::memory_layout::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr::new((TRAMP_PHYS + off) as u64),
    );
    // SAFETY: the trampoline window 0x8000..0xC000 is inside the
    // memblock-reserved low region and mapped by the linear map.
    unsafe {
        let dst = va.as_usize() as *mut u8;
        for (i, b) in bytes.iter().enumerate() {
            core::ptr::write_volatile(dst.add(i), *b);
        }
    }
}

fn tramp_write_u64(off: usize, v: u64) {
    tramp_write(off, &v.to_le_bytes());
}

fn tramp_write_u32(off: usize, v: u32) {
    tramp_write(off, &v.to_le_bytes());
}

fn tramp_write_u16(off: usize, v: u16) {
    tramp_write(off, &v.to_le_bytes());
}

/// Copy the trampoline blob to 0x8000 and build its page tables.
fn setup_trampoline(kernel_cr3: u64) {
    // SAFETY: symbols bound by the linker; the blob is read-only data.
    unsafe {
        let start = &raw const __ap_trampoline_start as usize;
        let end = &raw const __ap_trampoline_end as usize;
        let len = (end - start).min(TRAMP_SIZE);
        let blob = core::slice::from_raw_parts(start as *const u8, len);
        tramp_write(0, blob);
    }

    // gdtr base: linear address of the GDT inside the copy.
    // SAFETY: link-time symbol arithmetic against the blob base symbol.
    let gdt_off = (&raw const tramp_gdt as usize) - (&raw const __ap_trampoline_start as usize);
    tramp_write_u32(TRAMP_GDTR_BASE, (TRAMP_PHYS + gdt_off) as u32);

    // Far-jump targets (org offset for the 16:16 pm32 pointer, LINEAR
    // for the 32:16 long64 pointer).
    // SAFETY: link-time symbol arithmetic.
    let pm32_off = (&raw const pm32 as usize) - (&raw const __ap_trampoline_start as usize);
    let long64_off = (&raw const long64 as usize) - (&raw const __ap_trampoline_start as usize);
    tramp_write_u16(TRAMP_FAR16_PM32, pm32_off as u16);
    tramp_write_u32(TRAMP_FAR64_LONG, (TRAMP_PHYS + long64_off) as u32);

    tramp_write_u32(TRAMP_PML4, TRAMP_PML4_PHYS as u32);
    tramp_write_u64(TRAMP_ENTRY, ap_entry64 as usize as u64);
    tramp_write_u64(TRAMP_KCR3, kernel_cr3);

    // ---- Trampoline page tables ----
    // PML4[0] -> PDPT -> PD (identity low 1GB, 2MB leaves); the upper
    // half (entries 256..511) is copied from the kernel root so the AP
    // can run on kernel VA (stack + entry) before switching to the real
    // kernel CR3.
    let pml4 = tramp_va(TRAMP_PML4_PHYS);
    let pdpt = tramp_va(TRAMP_PDPT_PHYS);
    let pd = tramp_va(TRAMP_PD_PHYS);
    // SAFETY: freshly zero-initialized scratch tables in low reserved
    // memory, written via the linear map before any AP can see them.
    unsafe {
        for p in [pml4, pdpt, pd] {
            core::ptr::write_bytes(p as *mut u8, 0, 4096);
        }
        let pml4 = pml4 as *mut u64;
        let pdpt = pdpt as *mut u64;
        let pd = pd as *mut u64;
        *pml4.add(0) = (TRAMP_PDPT_PHYS as u64) | PDE_P | PDE_RW;
        *pdpt.add(0) = (TRAMP_PD_PHYS as u64) | PDE_P | PDE_RW;
        for i in 0..512u64 {
            *pd.add(i as usize) = (i << 21) | PDE_P | PDE_RW | PDE_PS;
        }
        // Copy the kernel upper half out of the static kernel root.
        let kernel_root = crate::arch::mm::memory_layout::phys_to_virt(
            crate::arch::mm::memory_layout::PhysAddr::new(kernel_cr3 & !0xFFF),
        )
        .as_usize() as *const u64;
        for i in 256..512usize {
            let e = core::ptr::read_volatile(kernel_root.add(i));
            if e & PDE_P != 0 {
                *pml4.add(i) = e;
            }
        }
    }
}

/// Linear-map VA of a low physical address.
fn tramp_va(phys: usize) -> usize {
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
fn smp_selftest() {
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
fn wait_started(cpu: usize, spins: u64) -> bool {
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

/// C entry for application processors.  The trampoline has already:
/// enabled long mode on the trampoline page tables, loaded the AP boot
/// stack, and set GS base to this CPU's PerCpu slot.
#[no_mangle]
pub extern "C" fn ap_entry64() -> ! {
    let cpu = crate::arch::cpu_id() as usize;

    // RACE-FORENSICS (x86-smprace): a second execution of ap_entry64 for
    // an already-started CPU is the ghost-CPU signature (re-SIPI/reset of
    // a running AP): it would clobber the per-CPU slots while the real
    // scheduler state keeps running tasks. Never observed; cheap to keep.
    if PER_CPU[cpu].started.load(Ordering::Acquire) == 1 {
        crate::println!("\nAP-REENTRY!!! cpu={} already started — ghost bring-up", cpu);
    }

    // The AP inherits the power-on CR0 with CD|NW set (uncached, no
    // write-through) — the trampoline only adds PE/PG.  Clear them so
    // this CPU runs cached like the BSP (NW=1 also violates the
    // WB-memory SMP coherence contract).
    // SAFETY: only CD/NW are changed; PE/PG/PAE stay set.
    unsafe {
        let cr0 = crate::arch::cpu::read_cr0();
        crate::arch::cpu::write_cr0(cr0 & !(0x6000_0000));
    }

    // Switch to the real kernel CR3 — both roots map the kernel upper
    // half identically, so the fetch after the switch is seamless.
    // SAFETY: root_static_phys is the permanent kernel PML4.
    unsafe {
        crate::arch::cpu::write_cr3(crate::arch::mm::mmu_init::root_static_phys());
    }

    // Per-CPU descriptor tables, syscall MSRs and FPU enable.
    crate::arch::trap::init_secondary(cpu);

    // Local APIC on this CPU (also arms the periodic timer; delivery
    // waits for STI below).
    crate::drivers::intc::apic::init_secondary();

    // Publish the idle task as `current` so the first timer tick (which
    // can arrive right after STI) finds a valid task on this CPU, and
    // touch this CPU's watchdog so the boot-time gap (idle task created
    // on the BSP seconds ago under TCG) cannot read as a lockup.
    crate::arch::cpu::set_thread_id(crate::sched::idle_task_ptr(cpu) as u64);
    crate::dfx::softlockup::touch(cpu);

    PER_CPU[cpu].cpu_online.store(1, Ordering::Release);
    PER_CPU[cpu].started.store(1, Ordering::Release);
    crate::println!(
        "smp: CPU {} online (lapic {:#x})",
        cpu,
        PER_CPU[cpu].lapic_id.load(Ordering::Acquire)
    );

    // Park until the BSP finished all single-CPU initialization.  HLT
    // with IRQs on: the local timer tick wakes us to re-check the flag
    // (cheap under TCG — a spinning AP would steal host time slices).
    while !is_boot_complete() {
        crate::arch::cpu::enable_irq();
        crate::arch::cpu::wfi();
    }

    // Join the scheduler (registers this CPU's idle task in the
    // per-CPU scheduler state, then never returns).
    crate::sched::init_secondary(cpu);
    crate::sched::cpu_idle_loop();
}
