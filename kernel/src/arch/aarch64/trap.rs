//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 trap handling — VBAR_EL1 vector table + exception routing
//! (stage 2).
//!
//! ## Shape (riscv64 twin)
//!
//! The entry assembly in trap.S pushes the full PtRegs frame and calls
//! [`aarch64_trap_handler`] with x0 = frame base. This module decodes
//! ESR_EL1 into the shared [`Cause`] vocabulary and routes:
//!
//! - SVC (EC 0x15) → syscall dispatch (generic `syscall::syscall_handler`)
//! - data/instruction aborts (EC 0x20/0x21/0x24/0x25) → generic mm's
//!   `do_page_fault` (the same interface riscv64's trap.rs feeds;
//!   FAR supplies the fault address, the ISS supplies R/W/X + origin EL)
//! - IRQ/FIQ (pseudo-causes 64/65) → irq_enter/exit bracket + the GIC
//!   dispatch point (the claim itself lands with stage-3 GICv3)
//! - SError (17) → fatal report
//! - illegal/unknown → oops (kernel) or SIGILL kill (user)
//!
//! ## TPIDR_EL1 / SP_EL1 protocol
//!
//! TPIDR_EL1 = current Task* at all times (no sscratch swap needed —
//! the aarch64 entry reads the source EL from SPSR.M, and the kernel
//! stack comes from SP_EL1, which the return path keeps parked at the
//! task's kernel-stack top while at EL0).


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::arch::asm;

use super::pt_regs::{PtRegs, PT_REGS_SIZE};

// Re-export PtRegs and related constants (interface parity)
pub use super::pt_regs::{Cause, Pstate};

// The vector table (aarch64_vector_table), the el0/el1 entry bodies, the
// exit loop and ret_from_fork live in arch/aarch64/trap.S — included
// here so the frame geometry below is the single source of truth the
// assembly is checked against.


// ============================================================================
// ESR_EL1 decode
// ============================================================================

/// Exception class (ESR.EC, bits [31:26])
#[inline]
pub fn esr_ec(esr: u64) -> u64 {
    (esr >> 26) & 0x3F
}

/// Exception syndrome (ESR.ISS, bits [24:0])
#[inline]
pub fn esr_iss(esr: u64) -> u64 {
    esr & 0x1FF_FFFF
}

/// Data abort ISS: WnR (bit 6) — 1 = write
pub const ISS_DF_WNR: u64 = 1 << 6;
/// Data abort ISS: FnV (bit 10) — FAR not valid
pub const ISS_DF_FNV: u64 = 1 << 10;
/// Data abort ISS: EA (bit 9) — external abort
pub const ISS_DF_EA: u64 = 1 << 9;
/// Data abort ISS: DFSC mask (bits [5:0]) — fault status code
pub const ISS_DFSC_MASK: u64 = 0x3F;

// DFSC rows referenced by dfsc_class (kept for documentation value)
pub const DFSC_SYNC_EXT_ABORT: u64 = 0x10; // 0b10100/0b10101 = external abort
pub const DFSC_ALIGNMENT: u64 = 0x21;

/// DFSC → fault-kind classification for the generic mm interface.
///
/// AArch64 DFSC encoding (ARM DDI 0487, ESR_ELx.ISS; the same rows Linux
/// arm64's esr.h names FSC_FAULT/FSC_ACCESS/FSC_PERM): the fault class
/// lives in bits [5:2], the level in [1:0] — translation = 0x04..0x07,
/// access-flag = 0x08..0x0B, permission = 0x0C..0x0F, alignment =
/// 0b100001 (0x21), synchronous external abort = 0x10. (The first draft
/// shifted the AF/permission rows by one: it read permission level 0
/// (0x0C) as an access-flag fault and classified the non-permission
/// 0x13 as permission.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultStatus {
    /// Translation fault (page not present at some level)
    Translation,
    /// Access-flag fault (present but AF clear — riscv64-style
    /// software-maintained accessed bit)
    AccessFlag,
    /// Permission fault (present, wrong rights — COW/W^X territory)
    Permission,
    /// Alignment fault
    Alignment,
    /// External/system error
    External,
    /// Anything else
    Other,
}

#[inline]
pub fn dfsc_class(dfsc: u64) -> FaultStatus {
    match dfsc {
        0x04..=0x07 => FaultStatus::Translation,
        0x08..=0x0B => FaultStatus::AccessFlag,
        0x0C..=0x0F => FaultStatus::Permission,
        DFSC_ALIGNMENT => FaultStatus::Alignment,
        0x10 => FaultStatus::External,
        _ => FaultStatus::Other,
    }
}

// ============================================================================
// Per-CPU PtRegs pointer (fork/exec + cross-CPU diagnostics)
// ============================================================================

pub(crate) static CURRENT_PT_REGS: [core::sync::atomic::AtomicU64; crate::arch::aarch64::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::arch::aarch64::MAX_CPUS];

/// Get current PtRegs pointer (trap frame; set by the entry path)
pub fn current_pt_regs() -> *const PtRegs {
    let cpu = crate::arch::cpu_id() as usize;
    CURRENT_PT_REGS[cpu].load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs
}
/// Current task's PtRegs (kernel stack top - sizeof(PtRegs))
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-11): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn current_task_pt_regs() -> Option<&'static mut PtRegs> {
    crate::arch::arch_floor::aarch64::trap::current_task_pt_regs()
}
/// RIP-equivalent (pc) of another CPU's most recent trap frame
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-10): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn pt_regs_rip_for_cpu(cpu: usize) -> u64 {
    crate::arch::arch_floor::aarch64::trap::pt_regs_rip_for_cpu(cpu)
}

// ============================================================================
// Early-trap dumper (bad vectors park through here)
// ============================================================================


// ============================================================================
// Dispatch
// ============================================================================


/// Register dump for the oops paths
pub(crate) fn oops_show_regs(w: &mut crate::debug::backtrace::ConsoleWriter, regs: &PtRegs) {
    use core::fmt::Write;
    let _ = write!(
        w,
        " pc={:#018x} lr={:#018x} sp={:#018x} pstate={:#x}\n",
        regs.pc, regs.regs[30], regs.sp, regs.pstate
    );
    let _ = write!(
        w,
        " esr={:#x} far={:#x} x0={:#x} x1={:#x} x2={:#x} x3={:#x}\n",
        regs.esr, regs.far, regs.regs[0], regs.regs[1], regs.regs[2], regs.regs[3]
    );
    for i in (4..31).step_by(4) {
        let _ = write!(w, " ");
        for j in 0..4 {
            let idx = i + j;
            if idx < 31 {
                let _ = write!(w, "x{}={:#x} ", idx, regs.regs[idx]);
            }
        }
        let _ = write!(w, "\n");
    }
}

/// SVC syscall dispatch (riscv64 handle_syscall shape)
///
/// PC / restart contract (the piece that breaks the first blocking read()
/// if any side gets it wrong):
///
/// - ELR_EL1 points at the instruction AFTER the `svc` (AArch64 SVC sets
///   the exception return address past the call — unlike the riscv64
///   ecall, whose SEPC sits on the ecall and must be advanced), so this
///   handler must NEVER advance pc before eret.
/// - Restart sentinels (-ERESTARTSYS/-ERESTARTNOHAND returned in x0 by the
///   generic layer) are converted by the SIGNAL path, not here:
///   signal.rs restart_syscall_no_handler / setup_frame rewind pc by 4
///   (back onto the svc) and restore orig_x0, the exact arm64 reference
///   do_signal shape (restart_addr = continue_addr - 4). The rewind
///   mirrors the +4 above — riscv64 is identical modulo instruction size.
pub(crate) fn handle_syscall(regs: &mut PtRegs) {
    // Re-enable interrupts for the syscall body (riscv64 twin: the trap
    // masks IRQs at entry; running the whole syscall masked would kill
    // ticks/preemption/softlockup detection. The frame's pstate keeps
    // the user's own DAIF; eret restores it unmodified.)
    crate::arch::cpu::enable_irq();

    let syscall_num = regs.syscall_nr() as u64;

    // Default -ENOSYS. The PC must NOT be advanced: unlike the riscv64
    // ecall (SEPC = the ecall itself), the AArch64 SVC exception leaves
    // ELR at the instruction AFTER the svc — the architectural return
    // address. The old unconditional pc += 4 (copied from the riscv64
    // twin) skipped one user instruction after EVERY syscall; eight
    // glibc wrapper calls survived because their skipped instruction was
    // a harmless compare, and the ninth (brk) skipped the adrp computing
    // __curbrk's page, so the following store faulted at [0, #0x28] and
    // killed PID 1. The ERESTARTSYS rewind (pc -= 4 back onto the svc)
    // stays correct: ELR-after-svc minus 4 is the svc.
    regs.regs[0] = crate::errno::constants::ENOSYS as u64;

    // U1c seccomp gate (riscv64 parity: policy runs before dispatch).
    if let Some(rv) = crate::syscall::process::seccomp_syscall_gate(syscall_num, regs) {
        regs.regs[0] = rv as u64;
        return;
    }

    crate::syscall::syscall_handler(regs);
}

/// IRQ dispatch point: drain the GIC (ICC_IAR1 acknowledge) and route
/// each INTID. The timer PPI and IPI SGI are special-cased exactly the
/// way the riscv64 twin's timer/software trap causes are (arch-core
/// interrupts that predate the irqchip framework); everything else goes
/// through the generic domain dispatch, whose flow handler EOIs via the
/// chip. Fallback EOI only for unmapped INTIDs that bypassed the flow.
pub(crate) fn handle_external_interrupt(regs: &mut PtRegs, cpu: usize) {
    use crate::drivers::intc::gicv3;

    // Bounded drain: a level source whose handler fails to deassert
    // would otherwise loop forever inside one bracket. On bound-break
    // the still-pending IRQ re-traps on eret (DAIF unmasked contexts).
    let mut budget = 64u32;
    while let Some(intid) = gicv3::claim() {
        budget = budget.saturating_sub(1);
        match intid {
            gicv3::TIMER_PPI_INTID => {
                handle_timer_interrupt(regs, cpu);
                gicv3::eoi(intid);
            }
            gicv3::IPI_SGI_INTID => {
                crate::fs::procfs::interrupts::soft_inc(cpu);
                crate::arch::ipi::handle_software_ipi(cpu);
                gicv3::eoi(intid);
            }
            _ => {
                let handled = crate::interrupt::get_default_domain()
                    .map(|d| crate::interrupt::generic_handle_domain_irq(d, intid))
                    .unwrap_or(false);
                if !handled {
                    // Spurious/unmapped: EOI here or the GIC stalls.
                    gicv3::eoi(intid);
                }
            }
        }
        if budget == 0 {
            break;
        }
    }
}
/// Timer interrupt (GIC PPI for the EL1 virtual timer) — the riscv64
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-7): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_timer_interrupt(regs: &mut PtRegs, cpu: usize) {
    crate::arch::arch_floor::aarch64::trap::handle_timer_interrupt(regs, cpu)
}

/// User-mode illegal instruction: route through signal delivery with
/// the riscv64 twin's handler-first policy (an installed SIGILL handler
/// must run; the default disposition kills).
pub(crate) fn handle_illegal_instruction(regs: &mut PtRegs) {
    crate::pr_err!(
        "aarch64: illegal instruction pid={} pc={:#x} esr={:#x}",
        crate::sched::get_current_pid(),
        regs.pc,
        regs.esr
    );
    let pid = crate::process::current_pid();
    let _ =
        crate::signal::send_signal_directed(pid, crate::signal::Signal::SIGILL as i32);
    if !sigill_has_handler() {
        // huks-wedge lane (hw5): group death (Linux do_group_exit).
        crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGILL as i32));
    }
}

// ---------------------------------------------------------------------------
// Signal-handler routing helpers for the fault kill paths (verbatim
// semantics of the riscv64 twins: has-handler / blocked probes against
// the current task's signal state).
// ---------------------------------------------------------------------------
/// True when the CURRENT task has a user handler installed for `signo`.
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn sig_has_handler(signo: usize) -> bool {
    crate::arch::arch_floor::aarch64::trap::sig_has_handler(signo)
}

/// True when the CURRENT task blocks `signo` — Linux force_sig_info
/// semantics: a FORCED signal that is blocked is reset to SIG_DFL (a
/// synchronous fault on a blocked handler-signal kills the task;
/// unblocking alone would re-enter the handler and fault forever).
pub(crate) fn sig_blocked_current(signo: usize) -> bool {
    crate::sched::current()
        .map(|t| (*t).sigmask & (1u64 << signo) != 0)
        .unwrap_or(false)
}

pub(crate) fn sigsegv_has_handler() -> bool {
    sig_has_handler(11)
}

pub(crate) fn sigsegv_blocked_current() -> bool {
    sig_blocked_current(11)
}

pub(crate) fn sigbus_has_handler() -> bool {
    sig_has_handler(7)
}

pub(crate) fn sigill_has_handler() -> bool {
    sig_has_handler(4)
}

/// Pend SIGSEGV for delivery by a handler, or kill — the riscv64
/// Segfault/PermissionDenied twin's exact routing. si_code/si_addr
/// parity (arena-mmap lane 2026-11-14): the siginfo must carry the
/// faulting address (Linux force_sig_fault) or any handler that
/// demand-maps the faulting page installs the WRONG page and
/// re-faults forever.
pub(crate) fn fault_sigsegv_route(code: i32, addr: u64) {
    let pid = crate::process::current_pid();
    let info = crate::signal::SigInfo::fault(
        crate::signal::Signal::SIGSEGV as i32,
        code,
        addr,
    );
    if sigsegv_has_handler() && !sigsegv_blocked_current() {
        let _ = crate::signal::send_signal_with_info_directed(pid, info);
        // Synchronous fault: deliver despite a blocked mask (Linux
        // force_sig parity).
        crate::signal::force_unblock_current(crate::signal::Signal::SIGSEGV as i32);
        return;
    }
    let _ = crate::signal::send_signal_with_info_directed(pid, info);
    // huks-wedge lane (hw5): group death (Linux do_group_exit) — a
    // fault-path default death kills the whole process; the thread-only
    // exit orphaned the dead thread's userspace locks.
    crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGSEGV as i32));
}

// (aarch64_bad_user_frame lives below, with the signal-routing helpers)


// ============================================================================
// Page fault routing (ESR/FAR → generic mm)
// ============================================================================

/// Handle a data/instruction abort: decode the fault kind and route
/// into the generic mm's do_page_fault; apply the result policy
/// (riscv64 handle_page_fault shape, trimmed to its load-bearing arms).
pub(crate) fn handle_page_fault(regs: &mut PtRegs, access_type: u32) {
    use crate::arch::mm::exception::{do_page_fault, MmFaultResult};

    let fault_addr = regs.far;

    // exceptions:page_fault_user/kernel tracepoints (riscv64 twin
    // record point; access_type carries R/W/X).
    if crate::trace::EVENT_PAGE_FAULT_USER.armed() || crate::trace::EVENT_PAGE_FAULT_KERNEL.armed()
    {
        let event = if regs.kernel_mode() {
            &crate::trace::EVENT_PAGE_FAULT_KERNEL
        } else {
            &crate::trace::EVENT_PAGE_FAULT_USER
        };
        if event.armed() {
            crate::trace::record(
                event,
                [fault_addr, regs.pc, access_type as u64, 0, 0, 0, 0, 0, 0, 0],
            );
        }
    }

    crate::pr_debug!(
        "trap: page fault addr={:#x}, pc={:#x}, type={}, mode={}",
        fault_addr, regs.pc, access_type,
        if regs.kernel_mode() { "kernel" } else { "user" }
    );

    let result = do_page_fault(regs, access_type);

    match result {
        MmFaultResult::Handled | MmFaultResult::Fixed => {
            // Page handled (or uaccess fixup applied); retry the insn.
        }
        MmFaultResult::Segfault | MmFaultResult::PermissionDenied => {
            crate::pr_err!(
                "pagefault: Segfault at {:#x}, pc={:#x}, lr={:#x}, sp={:#x}, pid={}, mode={}",
                fault_addr,
                regs.pc,
                regs.regs[30],
                regs.sp,
                crate::sched::get_current_pid(),
                if regs.kernel_mode() { "kernel" } else { "user" }
            );
            if regs.user_mode() {
                // Linux semantics (riscv64 twin): an installed SIGSEGV
                // handler must run — pend the signal and let do_signal
                // deliver it on the way back to userspace; without one
                // (or faulting inside the handler itself) the default
                // action kills here, avoiding a fault-retry loop.
                fault_sigsegv_route(
                    if matches!(result, MmFaultResult::PermissionDenied) {
                        crate::signal::si_code::SEGV_ACCERR
                    } else {
                        crate::signal::si_code::SEGV_MAPERR
                    },
                    fault_addr as u64,
                );
            } else {
                crate::debug::oops::oops(
                    crate::debug::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::debug::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs);
                    },
                );
            }
        }
        MmFaultResult::BusError => {
            crate::pr_err!(
                "pagefault: Bus error (past EOF) at {:#x}, pid={}",
                fault_addr,
                crate::sched::get_current_pid()
            );
            if regs.user_mode() {
                let pid = crate::process::current_pid();
                // si_code/si_addr parity (arena-mmap lane): BUS_ADRERR +
                // fault_addr, Linux force_sig_bus shape.
                let _ = crate::signal::send_signal_with_info_directed(
                    pid,
                    crate::signal::SigInfo::fault(
                        crate::signal::Signal::SIGBUS as i32,
                        crate::signal::si_code::BUS_ADRERR,
                        fault_addr as u64,
                    ),
                );
                if !sigbus_has_handler() {
                    // huks-wedge lane (hw5): group death (Linux do_group_exit).
                    crate::process::exit::do_exit_group(-(crate::signal::Signal::SIGBUS as i32));
                } else {
                    crate::signal::force_unblock_current(crate::signal::Signal::SIGBUS as i32);
                }
            } else {
                crate::debug::oops::oops(
                    crate::debug::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::debug::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs);
                    },
                );
            }
        }
        MmFaultResult::OutOfMemory => {
            crate::pr_err!(
                "pagefault: Out of memory at {:#x}, pid={}",
                fault_addr,
                crate::sched::get_current_pid()
            );
            if regs.user_mode() {
                // Generic OOM kill policy (riscv64 parity): SIGKILL the
                // faulting task; kernel-mode OOM stays fatal.
                let pid = crate::process::current_pid();
                let _ = crate::signal::send_signal_directed(
                    pid,
                    crate::signal::Signal::SIGKILL as i32,
                );
                crate::process::exit::do_exit(-(crate::signal::Signal::SIGKILL as i32));
            } else {
                crate::debug::oops::oops(
                    crate::debug::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::debug::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs);
                    },
                );
            }
        }
        MmFaultResult::KernelPanic => {
            crate::debug::oops::oops(
                crate::debug::oops::Kind::PageFault { addr: fault_addr },
                &|w: &mut crate::debug::backtrace::ConsoleWriter| {
                    oops_show_regs(w, regs);
                },
            );
        }
        MmFaultResult::AlreadyMapped | MmFaultResult::CowPending => {
            // Nothing to do; the instruction retries.
        }
    }
}


// ============================================================================
// Vector-table install + timer/IRQ arming (interface parity)
// ============================================================================
/// Initialize trap handling: install the vector table (VBAR_EL1)
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-3): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn init() {
    crate::arch::arch_floor::aarch64::trap::init()
}

pub fn init_syscall() {
    // SVC #0 is dispatched in aarch64_trap_handler (EC 0x15)
}
/// Enable the EL1 timer interrupt — the riscv64 enable_timer_interrupt
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-2): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn enable_timer_interrupt() {
    crate::arch::arch_floor::aarch64::trap::enable_timer_interrupt()
}
/// Disable the EL1 timer interrupt
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-1): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn disable_timer_interrupt() {
    crate::arch::arch_floor::aarch64::trap::disable_timer_interrupt()
}

/// Enable external interrupts (GICv3 CPU interface: redistributor wake,
/// SGI/PPI bank config, ICC_* system-register enable — per CPU)
pub fn enable_external_interrupt() {
    crate::drivers::intc::gicv3::enable_cpu_interface();
    // CPU-side unmask — the daifclr equivalent of the riscv64 twin's
    // csrs-sie. Enabling the GIC CPU interface alone leaves
    // PSTATE.DAIF.I set, and the kernel runs its whole boot with every
    // interrupt (timer included) masked: QEMU -d int showed zero IRQ
    // exceptions taken.
    crate::arch::cpu::enable_irq();
}

/// TrapFrame / ExceptionCause aliases (interface parity)
pub type TrapFrame = PtRegs;
pub type ExceptionCause = Cause;

// Collected-unsafe re-exports: the unsafe floor for these names lives in
// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.
// INV-ARCH note: ret_from_fork_kernel_helper moved to arch::arch_floor::aarch64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::aarch64::trap::ret_from_fork_kernel_helper;
// INV-ARCH note: aarch64_bad_user_frame moved to arch::arch_floor::aarch64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::aarch64::trap::aarch64_bad_user_frame;
// INV-ARCH note: aarch64_trap_handler moved to arch::arch_floor::aarch64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::aarch64::trap::aarch64_trap_handler;
// INV-ARCH note: aarch64_trap_dump moved to arch::arch_floor::aarch64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::aarch64::trap::aarch64_trap_dump;
