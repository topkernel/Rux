//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 CPU primitives: port I/O, control registers, MSRs,
//! interrupt-state management.


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

// ============================================================================
// Port I/O (INV-XE-PIO-1)
//
// The six word-width port primitives are SAFE by the same rule as
// clts/stts/invlpg (round 1): the instructions touch only registers —
// no kernel memory operand exists in `in`/`out`, so no port value can
// violate Rust memory safety. Device-side ordering effects (a stray outb
// reprogramming a PIC or UART) are a driver-protocol concern the callers
// own, exactly like the reference kernel's C inlines (arch/x86/include/
// asm/io.h), which carry no annotation either.
// ============================================================================
/// Read a byte from an I/O port
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-60): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn inb(port: u16) -> u8 {
    crate::arch::arch_floor::x86_64::cpu::inb(port)
}
/// Write a byte to an I/O port
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-59): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn outb(port: u16, value: u8) {
    crate::arch::arch_floor::x86_64::cpu::outb(port, value)
}
/// Read a word (16-bit) from an I/O port
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-58): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn inw(port: u16) -> u16 {
    crate::arch::arch_floor::x86_64::cpu::inw(port)
}
/// Write a word (16-bit) to an I/O port
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-57): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn outw(port: u16, value: u16) {
    crate::arch::arch_floor::x86_64::cpu::outw(port, value)
}
/// Read a dword (32-bit) from an I/O port
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-56): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn inl(port: u16) -> u32 {
    crate::arch::arch_floor::x86_64::cpu::inl(port)
}
/// Write a dword (32-bit) to an I/O port
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-55): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn outl(port: u16, value: u32) {
    crate::arch::arch_floor::x86_64::cpu::outl(port, value)
}
/// I/O wait (dummy port write to serialize slow legacy devices)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-54): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn io_wait() {


             // SAFETY: writing to port 0x80 (POST diagnostic port) is harmless.
              outb(0x80, 0) ;

}

// ---------------------------------------------------------------------------
// Control registers / MSRs
// ---------------------------------------------------------------------------
/// Read CR2 (page-fault address)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-53): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn read_cr2() -> u64 {
    crate::arch::arch_floor::x86_64::cpu::read_cr2()
}
/// Read CR0.
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-52): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn read_cr0() -> u64 {
    crate::arch::arch_floor::x86_64::cpu::read_cr0()
}
/// Write CR0.
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-51): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn write_cr0(value: u64) {
    crate::arch::arch_floor::x86_64::cpu::write_cr0(value)
}

/// CR0.TS (task switched) — set on every context switch-in; the first
/// FP instruction of the incoming task takes #NM, which restores that
/// task's image (lazy FPU). See context_switch / handle_device_not_available.
pub const CR0_TS: u64 = 1 << 3;

/// Is CR0.TS set (live FPU registers are NOT the current task's)?
#[inline]
pub fn ts_set() -> bool {
    read_cr0() & CR0_TS != 0
}
/// Clear CR0.TS (claim the live FPU for the current task). Memory-safe
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-50): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn clts() {
    crate::arch::arch_floor::x86_64::cpu::clts()
}
/// Set CR0.TS (drop the live FPU; next FP instruction traps #NM).
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-49): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn stts() {


          // SAFETY: single-bit set, mode bits preserved.
           write_cr0(read_cr0() | CR0_TS) ;

}
/// Set CR0.WP (write protect, bit 16) on the CURRENT cpu, idempotent.
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-48): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn enable_write_protect() {


                          // SAFETY: single-bit RMW; PE/PG/PAE and the rest of CR0 are preserved.

                              let cr0 = read_cr0();
                              write_cr0(cr0 | 0x1_0000);


}
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-47): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn read_cr3() -> u64 {
    crate::arch::arch_floor::x86_64::cpu::read_cr3()
}
/// Write CR3 (page-table root)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-46): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn write_cr3(value: u64) {
    crate::arch::arch_floor::x86_64::cpu::write_cr3(value)
}
/// Read an MSR
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-45): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn rdmsr(msr: u32) -> u64 {
    crate::arch::arch_floor::x86_64::cpu::rdmsr(msr)
}
/// Write an MSR
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-44): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn wrmsr(msr: u32, value: u64) {
    crate::arch::arch_floor::x86_64::cpu::wrmsr(msr, value)
}
/// Invalidate a TLB entry
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-43): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn invlpg(addr: u64) {
    crate::arch::arch_floor::x86_64::cpu::invlpg(addr)
}
/// Read CR4.
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-42): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn read_cr4() -> u64 {
    crate::arch::arch_floor::x86_64::cpu::read_cr4()
}
/// Write CR4.
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-41): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn write_cr4(value: u64) {
    crate::arch::arch_floor::x86_64::cpu::write_cr4(value)
}

/// INVPCID descriptor types.
pub const INVPCID_ADDR: u64 = 0; // one linear address under one PCID
pub const INVPCID_SINGLE_CTX: u64 = 1; // all entries under one PCID
pub const INVPCID_ALL_INCL_GLOBAL: u64 = 2; // every PCID, global included
pub const INVPCID_ALL_NON_GLOBAL: u64 = 3; // every PCID, global excluded
/// Invalidate by PCID descriptor (`invpcid`). The 16-byte descriptor is
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-40): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn invpcid(kind: u64, pcid: u64, addr: u64) {
    crate::arch::arch_floor::x86_64::cpu::invpcid(kind, pcid, addr)
}

// ---------------------------------------------------------------------------
// Interface parity with riscv64::cpu
// ---------------------------------------------------------------------------

/// Get current core ID
#[inline]
pub fn get_core_id() -> u64 {
    super::cpu_id()
}

/// Read ANOTHER CPU's current-task pointer slot (diagnostic).
#[inline]
pub fn get_thread_id_of(cpu: usize) -> u64 {
    super::smp::current_task_ptr_of(cpu)
}

/// Get the current-task pointer slot value.
///
/// On riscv64 the equivalent reads `tp`; x86_64 keeps the current task
/// in a per-CPU slot maintained by trap entry / __switch_to.
#[inline]
pub fn get_thread_id() -> u64 {
    crate::arch::smp::current_task_ptr()
}

/// Set the current-task pointer slot.
#[inline]
pub fn set_thread_id(tid: u64) {
    crate::arch::smp::set_current_task_ptr(tid);
}

/// Get counter frequency — the rate of [`read_counter`] in Hz, i.e. the
/// raw TSC frequency (0 until the APIC driver calibrated it against the
/// PIT; informational only — timekeeping uses [`read_time`]).
#[inline]
pub fn get_counter_freq() -> u64 {
    crate::drivers::intc::apic::TSC_FREQ.load(core::sync::atomic::Ordering::Acquire)
}
/// Read the TSC cycle counter
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-39): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn read_counter() -> u64 {
    crate::arch::arch_floor::x86_64::cpu::read_counter()
}

/// CR4.SMAP (bit 21): Supervisor-Mode Access Prevention. When set, the
/// kernel can only touch user pages through the uaccess primitives
/// (which bracket the copy with STAC/CLAC); any stray direct
/// dereference of a user pointer faults into the exception table — the
/// same default-deny the riscv64 port gets from leaving sstatus.SUM
/// clear. Enables usercopy semantics like the reference kernel's;
/// without it the low identity-mapped PML4[0] window let
/// copy_from_user(VA 1) silently read physical page 1 instead of
/// returning EFAULT (prlimit64 new_rlim test, 2026-10-31 review).
#[allow(dead_code)] // pending the kernel-wide user-pointer audit (2026-10-31)
/// Set by enable_smap(); read by the page-fault path to detect SMAP
/// violations (kernel-mode access to a U=0 user page outside a STAC
/// window must not be demand-filled — it must hit the exception table).
pub static SMAP_ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Whether CR4.SMAP is active on this boot.
pub fn smap_enabled() -> bool {
    SMAP_ENABLED.load(core::sync::atomic::Ordering::Relaxed)
}

/// Whether FSGSBASE (rdfsgsbase/wrfsbase/wrgsbase) is available and
/// armed — set by enable_fsgsbase() once per boot (the feature is
/// CPU-global), read on every context switch. With it armed the FS
/// switch avoids the intercepted `wrmsr(MSR_FS_BASE)` (the GS shadow
/// pair needs only native swapgs+wrgsbase), saving most of the MSR
/// bracket's cost per switch (cctx: msr bracket 89 ns avg under load,
/// gap-sched lane 2026-11-02).
pub static FSGSBASE_ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// True once `enable_fsgsbase` found CPUID.7.0:EBX[16].
#[inline(always)]
pub fn fsgsbase_enabled() -> bool {
    FSGSBASE_ENABLED.load(core::sync::atomic::Ordering::Relaxed)
}
/// Arm FSGSBASE for the context-switch path (WRFSBASE/WRGSBASE take
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-38): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn enable_fsgsbase() {


                     // CPUID leaf 7 subleaf 0, EBX bit 16 = FSGSBASE.
                     let (_, ebx, _, _) = cpuid(7, 0);
                     if ebx & (1 << 16) == 0 {
                         crate::println!("arch: FSGSBASE not available, switches keep wrmsr FS/GS");
                         return;
                     }
                     // SAFETY: CR4 update with the FSGSBASE bit only; CPUID gated and
                     // idempotent.

                         let cr4 = read_cr4();
                         write_cr4(cr4 | (1 << 16));

                     FSGSBASE_ENABLED.store(true, core::sync::atomic::Ordering::Relaxed);
                     crate::println!("arch: CR4.FSGSBASE enabled");

}
/// CR4.SMEP (bit 20): Supervisor-Mode Execution Prevention. When set, a
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-37): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub 
fn enable_smep() {


                 // CPUID leaf 7 subleaf 0, EBX bit 7 = SMEP.
                 let (_, ebx, _, _) = cpuid(7, 0);
                 if ebx & (1 << 7) == 0 {
                     crate::println!("arch: SMEP not available");
                     return;
                 }
                 // SAFETY: CR4 update with the SMEP bit only; CPUID gated and
                 // idempotent. Set on the BSP before SMP bring-up so every AP
                 // inherits CR4 via the boot trampoline.

                     let cr4 = read_cr4();
                     write_cr4(cr4 | (1 << 20));

                 crate::println!("arch: CR4.SMEP enabled (kernel EXEC of user pages faults at the fetch)");

}
/// WRFSBASE — set FS.base without an MSR write.
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-36): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn wrfsbase(base: u64) {
    crate::arch::arch_floor::x86_64::cpu::wrfsbase(base)
}
/// WRGSBASE — set the CURRENT GS.base (NOT the IA32_KERNEL_GS_BASE
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-35): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn wrgsbase(base: u64) {
    crate::arch::arch_floor::x86_64::cpu::wrgsbase(base)
}

/// Boot-log assertion of the SMAP armed state, from LIVE reads (sec-r1,
/// G1 follow-up).
///
/// Every boot must state the CPUID capability and the actual CR4 bit
/// side by side, so a silently-off SMAP (the original G1 defect class)
/// can never pass unnoticed again.
///
/// ARMING IS DELIBERATELY NOT ATTEMPTED (2026-11-02 finding, sec-r1):
/// calling enable_smap() at any pre-IDT boot stage (arch_init and
/// fpu_init were both tested) triple-faults the machine on the FIRST
/// kernel data access after the CR4 store — on KVM and on TCG. The
/// kernel dereferences at least one U/S-mapped page in early boot (prime
/// suspect: a kernel stack or per-CPU structure mapped with the user
/// bit). Because the old CPUID bug read EDX[20] (never set anywhere),
/// the arm path had never executed before this round; exercising it is
/// what exposed the latent fault. Root-causing it is its own lane
/// (see run/sec-r1-notes.md); until then this report honestly states
/// that user-mode access prevention is ABSENT.
pub fn smap_report() {
    let (_, ebx, _, _) = cpuid(7, 0);
    let present = ebx & (1 << 20) != 0;
    let cr4 = read_cr4();
    let armed = cr4 & (1u64 << 21) != 0;
    match (present, armed) {
        (true, true) => crate::println!(
            "arch: CR4.SMAP armed (CPUID.7.0:EBX[20]=1, live CR4={:#x}, bit 21 = 1)",
            cr4
        ),
        (true, false) => crate::println!(
            "arch: WARNING — CR4.SMAP NOT armed though CPUID.7.0:EBX[20]=1 (live CR4={:#x}); user-mode access prevention is ABSENT on this boot (arming path latent-broken, see sec-r1 notes)",
            cr4
        ),
        (false, true) => crate::println!(
            "arch: CR4.SMAP set by firmware though CPUID.7.0:EBX[20]=0 (live CR4={:#x})",
            cr4
        ),
        (false, false) => crate::println!(
            "arch: CR4.SMAP not armed (CPUID.7.0:EBX[20]=0, live CR4={:#x}); user-mode access prevention is ABSENT on this boot",
            cr4
        ),
    }
}
/// Query CPUID. Returns (eax, ebx, ecx, edx) for (leaf, subleaf).
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-34): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    crate::arch::arch_floor::x86_64::cpu::cpuid(leaf, subleaf)
}
/// Enable interrupts (STI)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-33): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn enable_irq() {
    crate::arch::arch_floor::x86_64::cpu::enable_irq()
}
/// Disable interrupts (CLI)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-32): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn disable_irq() {
    crate::arch::arch_floor::x86_64::cpu::disable_irq()
}
/// Wait for interrupt (HLT)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-31): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn wfi() {
    crate::arch::arch_floor::x86_64::cpu::wfi()
}

/// Read time since boot in the nominal clock domain
/// (`config::TIMER_CLOCK_FREQ_HZ`, 10 MHz) — the same contract as the
/// riscv64 `rdtime` twin.
///
/// X86-TIMEBASE: the raw TSC does NOT tick at the nominal frequency (its
/// rate is hypervisor/model specific — ~2.5 GHz under QEMU TCG), so this
/// must go through the timer driver instead of dividing
/// [`read_counter`] by the nominal frequency. Priority: HPET main
/// counter (QEMU virtual clock — exact), calibrated TSC (no HPET),
/// jiffies-derived PIT clock (0 until the first tick). Callers that need
/// the RAW cycle counter (TSC-frequency
/// calibration, busy delays) use [`read_counter`] + [`get_counter_freq`].
#[inline]
pub fn read_time() -> u64 {
    crate::drivers::timer::read_time()
}
/// Instruction serialization barrier
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-30): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn isb() {
    crate::arch::arch_floor::x86_64::cpu::isb()
}
/// Data synchronization barrier
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-29): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn dsb() {
    crate::arch::arch_floor::x86_64::cpu::dsb()
}
/// Data memory barrier
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-28): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn dmb() {
    crate::arch::arch_floor::x86_64::cpu::dmb()
}
/// Get interrupt mask state (true = interrupts enabled)
///
/// Collected into `arch::arch_floor` (INV-ARCH-CPU-27): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn get_interrupts_state() -> bool {
    crate::arch::arch_floor::x86_64::cpu::get_interrupts_state()
}

/// Save interrupt state and disable interrupts
#[inline]
#[track_caller]
pub fn save_and_disable_irq() -> bool {
    let enabled = get_interrupts_state();
    if enabled {
        disable_irq();
    }
    enabled
}

/// Restore interrupt state
#[inline]
#[track_caller]
pub fn restore_irq(state: bool) {
    if state {
        enable_irq();
    }
}
