//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 CPU primitives: port I/O, control registers, MSRs,
//! interrupt-state management.

/// Read a byte from an I/O port
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Write a byte to an I/O port
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
}

/// Read a word (16-bit) from an I/O port
#[inline]
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    unsafe {
        core::arch::asm!("in ax, dx", out("ax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Write a word (16-bit) to an I/O port
#[inline]
pub unsafe fn outw(port: u16, value: u16) {
    unsafe {
        core::arch::asm!("out dx, ax", in("ax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
}

/// Read a dword (32-bit) from an I/O port
#[inline]
pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    unsafe {
        core::arch::asm!("in eax, dx", out("eax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Write a dword (32-bit) to an I/O port
#[inline]
pub unsafe fn outl(port: u16, value: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("eax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
}

/// I/O wait (dummy port write to serialize slow legacy devices)
#[inline]
pub fn io_wait() {
    // SAFETY: writing to port 0x80 (POST diagnostic port) is harmless.
    unsafe { outb(0x80, 0) };
}

// ---------------------------------------------------------------------------
// Control registers / MSRs
// ---------------------------------------------------------------------------

/// Read CR2 (page-fault address)
#[inline]
pub fn read_cr2() -> u64 {
    let value: u64;
    // SAFETY: CR2 is a read-only control register; reading is always safe.
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) value, options(nomem, nostack)) };
    value
}

/// Read CR3 (page-table root)
#[inline]
/// Read CR0.
pub fn read_cr0() -> u64 {
    let value: u64;
    // SAFETY: reading CR0 is always safe.
    unsafe { core::arch::asm!("mov {}, cr0", out(reg) value, options(nomem, nostack)) };
    value
}

/// Write CR0.
///
/// # Safety
/// The value must keep PE/PG/PAE as required by the current mode.
pub unsafe fn write_cr0(value: u64) {
    // SAFETY: caller keeps the mode-defining bits intact.
    unsafe { core::arch::asm!("mov cr0, {}", in(reg) value, options(nomem, nostack)) };
}

pub fn read_cr3() -> u64 {
    let value: u64;
    // SAFETY: reading CR3 is always safe.
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) value, options(nomem, nostack)) };
    value
}

/// Write CR3 (page-table root)
///
/// # Safety
/// Caller must ensure value is a valid PML4 physical address.
#[inline]
pub unsafe fn write_cr3(value: u64) {
    unsafe { core::arch::asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags)) };
}

/// Read an MSR
///
/// # Safety
/// Caller must ensure the MSR index is valid for this CPU.
#[inline]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!("rdmsr", out("eax") lo, out("edx") hi, in("ecx") msr, options(nomem, nostack));
    }
    (lo as u64) | ((hi as u64) << 32)
}

/// Write an MSR
///
/// # Safety
/// Caller must ensure the MSR index and value are valid for this CPU.
#[inline]
pub unsafe fn wrmsr(msr: u32, value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    unsafe {
        core::arch::asm!("wrmsr", in("eax") lo, in("edx") hi, in("ecx") msr, options(nomem, nostack));
    }
}

/// Invalidate a TLB entry
///
/// # Safety
/// Caller must ensure the address is canonical.
#[inline]
pub unsafe fn invlpg(addr: u64) {
    unsafe { core::arch::asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags)) };
}

// ---------------------------------------------------------------------------
// Interface parity with riscv64::cpu
// ---------------------------------------------------------------------------

/// Get current core ID
#[inline]
pub fn get_core_id() -> u64 {
    super::cpu_id()
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
#[inline]
pub fn read_counter() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC is a pure counter read.
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)) };
    (lo as u64) | ((hi as u64) << 32)
}

/// Query CPUID. Returns (eax, ebx, ecx, edx) for (leaf, subleaf).
///
/// NOTE: `subleaf` is only meaningful for leaves that use ECX as a
/// sub-leaf selector (4, 7, 0xb, 0xd, ...); pass 0 otherwise. rbx is
/// reserved by LLVM, so save/restore it around the instruction.
#[inline]
pub fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let (a, b, c, d): (u32, u32, u32, u32);
    // SAFETY: CPUID is a pure query. rbx is reserved by LLVM, so stash
    // EBX in a scratch register around the push/pop (same discipline as
    // cpu::isb, extended to recover the EBX result).
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {tmp:e}, ebx",
            "pop rbx",
            in("eax") leaf,
            in("ecx") subleaf,
            tmp = out(reg) b,
            lateout("eax") a,
            lateout("ecx") c,
            lateout("edx") d,
            options(nostack)
        );
    }
    (a, b, c, d)
}

/// Enable interrupts (STI)
#[inline]
pub fn enable_irq() {
    // SAFETY: setting IF is safe at any point the caller wants IRQs on.
    unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
}

/// Disable interrupts (CLI)
#[inline]
pub fn disable_irq() {
    // SAFETY: clearing IF is always safe.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
}

/// Wait for interrupt (HLT)
#[inline]
pub fn wfi() {
    // SAFETY: HLT halts until the next interrupt; interrupts must be
    // enabled or the CPU hangs forever — the idle loop guarantees that.
    unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
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
#[inline]
pub fn isb() {
    // SAFETY: CPUID serializes the pipeline. rbx is reserved by LLVM, so
    // save/restore it around the instruction explicitly.
    unsafe {
        core::arch::asm!(
            "xor eax, eax",
            "push rbx",
            "cpuid",
            "pop rbx",
            out("eax") _, out("ecx") _, out("edx") _,
            options(nostack)
        );
    }
}

/// Data synchronization barrier
#[inline]
pub fn dsb() {
    // SAFETY: MFENCE orders all memory operations; no side effects beyond ordering.
    unsafe { core::arch::asm!("mfence", options(nomem, nostack, preserves_flags)) };
}

/// Data memory barrier
#[inline]
pub fn dmb() {
    // SAFETY: LFENCE orders loads; no side effects beyond ordering.
    unsafe { core::arch::asm!("lfence", options(nomem, nostack, preserves_flags)) };
}

/// Get interrupt mask state (true = interrupts enabled)
#[inline]
pub fn get_interrupts_state() -> bool {
    let flags: u64;
    // SAFETY: PUSHF/POPQ reads RFLAGS without modifying state.
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {}",
            out(reg) flags,
            options(nostack)
        );
    }
    (flags & (1 << 9)) != 0
}

/// Save interrupt state and disable interrupts
#[inline]
pub fn save_and_disable_irq() -> bool {
    let enabled = get_interrupts_state();
    if enabled {
        disable_irq();
    }
    enabled
}

/// Restore interrupt state
#[inline]
pub fn restore_irq(state: bool) {
    if state {
        enable_irq();
    }
}
