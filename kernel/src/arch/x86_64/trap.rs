//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 trap handling — IDT, TSS, syscall entry.
//!
//! X86-TODO(agent x86-trap): full implementation. Contract pinned here:
//!
//! - `init()`: load GDT (null, code64@0x08, data@0x10, user data@0x2b,
//!   user code64@0x33, TSS@0x38), per-CPU TSS with IST1 = double-fault
//!   stack, load IDT covering vectors 0..255.
//! - `init_syscall()`: set MSR STAR = (0x20<<48)|(0x08<<32), LSTAR =
//!   syscall entry, FMASK = TF|DF|IF, plus EFER.SCE via mod.rs or here.
//! - trap.S: vector stubs push a full Linux-layout pt_regs
//!   (r15..orig_rax then the hardware frame); user entries land on the
//!   TSS rsp0 stack so the frame sits exactly at stack_top - sizeof(PtRegs).
//! - `trap_handler(regs, cpu)`: mirror the riscv64 dispatch: timer IRQ →
//!   scheduler_tick, external IRQ → PIC/IOAPIC ack + do_IRQ path,
//!   syscall → crate::syscall dispatch with x86_64 NR mapping
//!   (args rdi rsi rdx r10 r8 r9, nr in rax),
//!   #PF → decode CR2 + error code → exception::do_page_fault,
//!   #UD/#GP/#DE in user → SIGILL/SIGSEGV/SIGFPE delivery.
//! - `ret_from_fork` label: restores the pt_regs at child thread.sp.
//! - Timer bring-up: PIT channel 0 @ HZ (drivers::timer::set_next_trigger
//!   parity), PIC 8259 remapped to vectors 32..47.

pub use super::pt_regs::{Cause, PtRegs, PT_REGS_SIZE};

/// Current CPU's pt_regs pointer (used by fork) — per-CPU slots
static CURRENT_PT_REGS: [core::sync::atomic::AtomicU64; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::config::MAX_CPUS];

pub fn current_pt_regs() -> *const PtRegs {
    let cpu = crate::arch::cpu_id() as usize;
    CURRENT_PT_REGS[cpu].load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs
}

/// pt_regs of the current task's user frame (stack_top - sizeof(PtRegs))
pub fn current_task_pt_regs() -> Option<&'static mut PtRegs> {
    let task = crate::sched::current()?;
    // SAFETY: kernel stack top is allocated by the task subsystem; the user
    // entry frame sits at the fixed offset from it.
    unsafe {
        let stack_top = (*task).get_kernel_stack()? as u64;
        Some(&mut *((stack_top - PT_REGS_SIZE as u64) as *mut PtRegs))
    }
}

/// Initialize trap handling (GDT/TSS/IDT). X86-TODO(agent x86-trap)
pub fn init() {}

/// Initialize syscall MSRs. X86-TODO(agent x86-trap)
pub fn init_syscall() {}

/// Enable the timer interrupt (unmask PIC IRQ0 + arm PIT). X86-TODO
pub fn enable_timer_interrupt() {
    // X86-TODO(agent x86-trap): PIC unmask + crate::drivers::timer::set_next_trigger
}

/// Disable the timer interrupt. X86-TODO
pub fn disable_timer_interrupt() {}

/// Enable external interrupts (unmask PIC lines). X86-TODO
pub fn enable_external_interrupt() {}
