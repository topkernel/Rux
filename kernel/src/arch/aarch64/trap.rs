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

use core::arch::asm;

use super::pt_regs::{PtRegs, PT_REGS_SIZE};

// Re-export PtRegs and related constants (interface parity)
pub use super::pt_regs::{Cause, Pstate};

// The vector table (aarch64_vector_table), the el0/el1 entry bodies, the
// exit loop and ret_from_fork live in arch/aarch64/trap.S — included
// here so the frame geometry below is the single source of truth the
// assembly is checked against.
core::arch::global_asm!(
    // Operand cross-reference: the template's `.equ S_X, {S_X}` block
    // consumes each operand via textual substitution; this comment keeps
    // the asm usage check satisfied for the scanner (same trick the
    // compiler suggests for intentionally "unused" asm operands).
    "/* {S_FRAME_SIZE} {S_X0} {S_X8} {S_X20} {S_X21} {S_X22} {S_X30} {S_SP} {S_PC} {S_PSTATE} {S_ESR} {S_FAR} {S_ORIG_X0} {S_SYSCALLNO} {TI_PREEMPT} {TI_KSTACK_BOTTOM} {KSTACK_SIZE} */",
    include_str!("trap.S"),
    S_FRAME_SIZE = const PT_REGS_SIZE,
    S_X0 = const 0,
    S_X8 = const 8 * 8,
    S_X20 = const 8 * 20,
    S_X21 = const 8 * 21,
    S_X22 = const 8 * 22,
    S_X30 = const 8 * 30,
    S_SP = const core::mem::offset_of!(PtRegs, sp),
    S_PC = const core::mem::offset_of!(PtRegs, pc),
    S_PSTATE = const core::mem::offset_of!(PtRegs, pstate),
    S_ESR = const core::mem::offset_of!(PtRegs, esr),
    S_FAR = const core::mem::offset_of!(PtRegs, far),
    S_ORIG_X0 = const core::mem::offset_of!(PtRegs, orig_x0),
    S_SYSCALLNO = const core::mem::offset_of!(PtRegs, syscallno),
    TI_PREEMPT = const crate::process::task::task_offsets::TI_PREEMPT_COUNT,
    TI_KSTACK_BOTTOM = const crate::process::task::task_offsets::TASK_KERNEL_STACK_BOTTOM,
    KSTACK_SIZE = const crate::config::KERNEL_STACK_SIZE,
);

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
/// AArch64 DFSC encoding: translation = 0b00100..0b00111 (0x04-0x07),
/// access-flag = 0b01001..0b01100 (0x09-0x0C), permission =
/// 0b01101..0b01111 + 0b10011 (0x0D-0x0F, 0x13 — L3 permission has its
/// own row), alignment = 0b100001 (0x21), external = 0b10100/0b10101.
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
        0x09..=0x0C => FaultStatus::AccessFlag,
        0x0D..=0x0F | 0x13 => FaultStatus::Permission,
        DFSC_ALIGNMENT => FaultStatus::Alignment,
        0x10 | 0x11 => FaultStatus::External,
        _ => FaultStatus::Other,
    }
}

// ============================================================================
// Per-CPU PtRegs pointer (fork/exec + cross-CPU diagnostics)
// ============================================================================

static CURRENT_PT_REGS: [core::sync::atomic::AtomicU64; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Get current PtRegs pointer (trap frame; set by the entry path)
pub fn current_pt_regs() -> *const PtRegs {
    let cpu = crate::arch::cpu_id() as usize;
    CURRENT_PT_REGS[cpu].load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs
}

/// Current task's PtRegs (kernel stack top - sizeof(PtRegs))
pub fn current_task_pt_regs() -> Option<&'static mut PtRegs> {
    use crate::sched::current;

    // SAFETY: pt_regs lives at the fixed offset from the task's kernel
    // stack top established by the trap entry path.
    unsafe {
        let task = current()?;
        let stack_top = (*task).get_kernel_stack()? as u64;
        let pt_regs_ptr = (stack_top - PT_REGS_SIZE as u64) as *mut PtRegs;
        Some(&mut *pt_regs_ptr)
    }
}

/// RIP-equivalent (pc) of another CPU's most recent trap frame
/// (diagnostic; riscv64 twin semantics)
pub fn pt_regs_rip_for_cpu(cpu: usize) -> u64 {
    if cpu >= crate::config::MAX_CPUS {
        return 0;
    }
    let p = CURRENT_PT_REGS[cpu].load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs;
    if p.is_null()
        || (p as u64) < 0xffff_8000_0000_0000
        || !crate::sched::sched::kernel_ptr_canonical(p as *const u8)
    {
        return 0;
    }
    // SAFETY: diagnostic scalar read of a live-or-stale kernel frame.
    unsafe { core::ptr::addr_of!((*p).pc).read() }
}

// ============================================================================
// Early-trap dumper (bad vectors park through here)
// ============================================================================

/// Bad-vector dumper (called from the 0x000/0x600 band stubs)
#[no_mangle]
extern "C" fn aarch64_trap_dump() {
    let (esr, elr, far, spsr, sp_el1): (u64, u64, u64, u64, u64);
    // SAFETY: pure system-register reads; safe in any exception context.
    unsafe {
        asm!(
            "mrs {}, esr_el1",
            "mrs {}, elr_el1",
            "mrs {}, far_el1",
            "mrs {}, spsr_el1",
            "mov {}, sp",
            out(reg) esr,
            out(reg) elr,
            out(reg) far,
            out(reg) spsr,
            out(reg) sp_el1,
            options(nomem, nostack)
        );
    }
    crate::pr_err!(
        "AARCH64-BAD-VECTOR: esr={:#x} elr={:#x} far={:#x} spsr={:#x} sp={:#x}",
        esr, elr, far, spsr, sp_el1
    );
    crate::pr_err!(
        "  EC={:#x} ISS={:#x} (bad mode — see trap.S vector table)",
        esr_ec(esr),
        esr_iss(esr)
    );
}

// ============================================================================
// Dispatch
// ============================================================================

/// Trap handler — called by trap.S with the PtRegs pointer (frame base
/// == x0, SP 16-aligned, DAIF masked).
#[no_mangle]
pub extern "C" fn aarch64_trap_handler(regs: *mut PtRegs) {
    let cpu_id = crate::arch::cpu_id() as usize;

    // Save and swap PtRegs pointer (fork/exec read current_pt_regs();
    // the swap handles nested traps the same way the riscv64 twin does).
    let prev_pt_regs = CURRENT_PT_REGS[cpu_id].load(core::sync::atomic::Ordering::Relaxed);
    CURRENT_PT_REGS[cpu_id].store(regs as u64, core::sync::atomic::Ordering::Relaxed);

    // SAFETY: regs points at the frame trap.S just pushed; it stays
    // valid across the dispatch (the caller asm restores from it after
    // we return, possibly on a freshly thawed stack after schedule()).
    let regs_ref = unsafe { &mut *regs };
    let esr = regs_ref.esr;
    let cause = Cause::from_cause(esr);

    // WFI/WFE restart: an IRQ taken from a kernel wfi/wfe leaves ELR at
    // the instruction itself (riscv64 twin advances past wfi — same
    // hazard: the eret would re-execute it). Only advance for
    // interrupt-class traps, only inside kernel text, only on an exact
    // encoding match.
    if matches!(cause, Cause::Irq | Cause::Fiq)
        && regs_ref.kernel_mode()
        && regs_ref.pc % 4 == 0
    {
        extern "C" {
            static _stext: u8;
            static _etext: u8;
        }
        // SAFETY: linker symbols _stext/_etext bound the kernel text.
        let (lo, hi) = unsafe {
            (&raw const _stext as usize as u64, &raw const _etext as usize as u64)
        };
        if regs_ref.pc >= lo && regs_ref.pc < hi {
            // WFI = 0xD503207F, WFE = 0xD503205F
            // SAFETY: pc is provably inside kernel text and 4-aligned.
            let insn = unsafe { core::ptr::read_volatile(regs_ref.pc as *const u32) };
            if insn == 0xD503_207F || insn == 0xD503_205F {
                regs_ref.pc += 4;
            }
        }
    }

    crate::pr_debug!(
        "trap: cause={:?}, pc={:#x}, sp={:#x}, esr={:#x}, far={:#x}, mode={}",
        cause, regs_ref.pc, regs_ref.sp, regs_ref.esr, regs_ref.far,
        if regs_ref.user_mode() { "user" } else { "kernel" }
    );

    match cause {
        // Syscall from EL0 (SVC #0): x8 = nr (snapshotted into
        // syscallno by the entry), x0-x5 args, return in x0.
        Cause::EcallUser => handle_syscall(regs_ref),

        // HVC from EL1 — the PSCI conduit lands here in stage 3.
        // Nothing issues HVC today; treat it as a kernel bug.
        Cause::EcallSupervisor => {
            crate::pr_err!(
                "aarch64: unexpected HVC at pc={:#x} (PSCI conduit lands in stage 3)",
                regs_ref.pc
            );
            crate::dfx::oops::oops(
                crate::dfx::oops::Kind::UnknownException { cause: esr },
                &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                    oops_show_regs(w, regs_ref);
                },
            );
        }

        // Page faults — the generic mm's fault path (riscv64 parity:
        // do_page_fault receives R/W/X + origin-EL access flags; FAR is
        // the fault address).
        Cause::DataAbort | Cause::DataAbortCur => {
            let iss = esr_iss(esr);
            let mut access = if iss & ISS_DF_WNR != 0 {
                crate::arch::mm::FaultFlags::WRITE
            } else {
                crate::arch::mm::FaultFlags::READ
            };
            access |= if regs_ref.user_mode() {
                crate::arch::mm::FaultFlags::USER
            } else {
                crate::arch::mm::FaultFlags::KERNEL
            };
            handle_page_fault(regs_ref, access);
        }
        Cause::InstructionAbort | Cause::InstructionAbortCur => {
            let access = crate::arch::mm::FaultFlags::EXEC
                | if regs_ref.user_mode() {
                    crate::arch::mm::FaultFlags::USER
                } else {
                    crate::arch::mm::FaultFlags::KERNEL
                };
            handle_page_fault(regs_ref, access);
        }

        // IRQ / FIQ — dispatch through the irq_enter/exit bracket. The
        // GIC claim + INTID routing lands in stage 3 (DAIF stays masked
        // until then; this arm is the shape, not a live path yet).
        Cause::Irq => {
            crate::interrupt::preempt::irq_enter();
            handle_external_interrupt(regs_ref, cpu_id);
            crate::interrupt::preempt::irq_exit();
        }
        Cause::Fiq => {
            // QEMU routes nothing to FIQ in this configuration; report
            // and continue (masked-again on eret).
            crate::pr_warn!("aarch64: spurious FIQ at pc={:#x}", regs_ref.pc);
        }

        // Asynchronous external abort — fatal.
        Cause::SError => {
            crate::pr_err!(
                "aarch64: SError at pc={:#x} far={:#x} esr={:#x}",
                regs_ref.pc, regs_ref.far, esr
            );
            crate::dfx::oops::oops(
                crate::dfx::oops::Kind::UnknownException { cause: esr },
                &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                    oops_show_regs(w, regs_ref);
                },
            );
        }

        // Illegal instruction / illegal execution state: user gets the
        // SIGILL kill path, kernel is a bug.
        Cause::IllegalInstruction | Cause::IllegalState => {
            if regs_ref.user_mode() {
                handle_illegal_instruction(regs_ref);
            } else {
                crate::dfx::oops::oops(
                    crate::dfx::oops::Kind::IllegalInstruction,
                    &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs_ref);
                    },
                );
            }
        }

        // Trapped WFx / everything else: bad mode.
        _ => {
            crate::pr_err!(
                "aarch64: unhandled exception cause={:?} ec={:#x} iss={:#x} pc={:#x} far={:#x}",
                cause, esr_ec(esr), esr_iss(esr), regs_ref.pc, regs_ref.far
            );
            if regs_ref.user_mode() {
                handle_illegal_instruction(regs_ref);
            } else {
                crate::dfx::oops::oops(
                    crate::dfx::oops::Kind::UnknownException { cause: esr },
                    &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs_ref);
                    },
                );
            }
        }
    }

    // Restore previous PtRegs pointer (nested-trap case, riscv64 twin).
    CURRENT_PT_REGS[cpu_id].store(prev_pt_regs, core::sync::atomic::Ordering::Relaxed);

    // Leak-origin tripwire (riscv64 twin): a user-return frame must
    // carry preempt_count == 0.
    if regs_ref.user_mode() {
        crate::dfx::leakorigin::check_user_return();
    }
}

/// Register dump for the oops paths
fn oops_show_regs(w: &mut crate::dfx::backtrace::ConsoleWriter, regs: &PtRegs) {
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
fn handle_syscall(regs: &mut PtRegs) {
    // Re-enable interrupts for the syscall body (riscv64 twin: the trap
    // masks IRQs at entry; running the whole syscall masked would kill
    // ticks/preemption/softlockup detection. The frame's pstate keeps
    // the user's own DAIF; eret restores it unmodified.)
    crate::arch::cpu::enable_irq();

    let syscall_num = regs.syscall_nr() as u64;

    // Default -ENOSYS; advance pc past the SVC (fixed 4-byte encoding
    // on AArch64 — no compressed-instruction dance like riscv64).
    regs.regs[0] = crate::errno::constants::ENOSYS as u64;
    regs.pc += 4;

    // U1c seccomp gate (riscv64 parity: policy runs before dispatch).
    if let Some(rv) = crate::syscall::process::seccomp_syscall_gate(syscall_num, regs) {
        regs.regs[0] = rv as u64;
        return;
    }

    crate::syscall::syscall_handler(regs);
}

/// IRQ dispatch point (GIC claim lands with stage 3; DAIF-masked
/// boots never reach this — the warn is the tripwire if one does).
fn handle_external_interrupt(_regs: &mut PtRegs, cpu: usize) {
    // (The old draft called a phantom crate::drivers::timer::interrupts
    // module that no backend provides; the external-IRQ accounting hook
    // is wired with the real GIC claim path in stage 3.)
    crate::pr_warn!(
        "aarch64: IRQ delivered with the GIC still disabled (cpu={}) — stage-3 stub path",
        cpu
    );
}

/// User-mode illegal instruction: route through signal delivery with
/// the riscv64 twin's handler-first policy (an installed SIGILL handler
/// must run; the default disposition kills).
fn handle_illegal_instruction(regs: &mut PtRegs) {
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
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGILL as i32));
    }
}

// ---------------------------------------------------------------------------
// Signal-handler routing helpers for the fault kill paths (verbatim
// semantics of the riscv64 twins: has-handler / blocked probes against
// the current task's signal state).
// ---------------------------------------------------------------------------

/// True when the CURRENT task has a user handler installed for `signo`.
fn sig_has_handler(signo: usize) -> bool {
    crate::sched::current()
        .and_then(|t| unsafe { (*t).signal.as_ref().map(|s| s.get_action(signo as i32)) })
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// True when the CURRENT task blocks `signo` — Linux force_sig_info
/// semantics: a FORCED signal that is blocked is reset to SIG_DFL (a
/// synchronous fault on a blocked handler-signal kills the task;
/// unblocking alone would re-enter the handler and fault forever).
fn sig_blocked_current(signo: usize) -> bool {
    crate::sched::current()
        .map(|t| (*t).sigmask & (1u64 << signo) != 0)
        .unwrap_or(false)
}

fn sigsegv_has_handler() -> bool {
    sig_has_handler(11)
}

fn sigsegv_blocked_current() -> bool {
    sig_blocked_current(11)
}

fn sigbus_has_handler() -> bool {
    sig_has_handler(7)
}

fn sigill_has_handler() -> bool {
    sig_has_handler(4)
}

/// Pend SIGSEGV for delivery by a handler, or kill — the riscv64
/// Segfault/PermissionDenied twin's exact routing.
fn fault_sigsegv_route() {
    let pid = crate::process::current_pid();
    if sigsegv_has_handler() && !sigsegv_blocked_current() {
        let _ = crate::signal::send_signal_directed(
            pid,
            crate::signal::Signal::SIGSEGV as i32,
        );
        // Synchronous fault: deliver despite a blocked mask (Linux
        // force_sig parity).
        crate::signal::force_unblock_current(crate::signal::Signal::SIGSEGV as i32);
        return;
    }
    let _ = crate::signal::send_signal_directed(
        pid,
        crate::signal::Signal::SIGSEGV as i32,
    );
    crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
}

// (aarch64_bad_user_frame lives below, with the signal-routing helpers)

/// R74 twin (garbage-frame gate): called from trap.S when a user-return
/// frame is provably unexecutable (pc==0 or sp < 0x1000) — kill the
/// task instead of ereting into the void. Reached by `b` from
/// restore_and_eret, where x0 no longer holds the frame pointer (the
/// GPR restore already ran): read the frame off the current task's
/// kernel stack instead — the outermost frame position is fixed at
/// stack_top - PT_REGS_SIZE.
#[no_mangle]
pub extern "C" fn aarch64_bad_user_frame(regs_x0: u64) -> ! {
    let _ = regs_x0;
    if let Some(regs) = current_task_pt_regs() {
        crate::pr_err!(
            "trap: BADFRAME pid={} pc={:#x} sp={:#x} pstate={:#x}",
            crate::sched::get_current_pid(),
            regs.pc,
            regs.sp,
            regs.pstate
        );
    } else {
        crate::pr_err!("trap: BADFRAME (no current task)");
    }
    crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
    // do_exit schedules away and never returns; park defensively.
    loop {
        // SAFETY: wfi halts the core until an interrupt; safe in a dead
        // task's final spin.
        unsafe { core::arch::asm!("wfi") };
    }
}

// ============================================================================
// Page fault routing (ESR/FAR → generic mm)
// ============================================================================

/// Handle a data/instruction abort: decode the fault kind and route
/// into the generic mm's do_page_fault; apply the result policy
/// (riscv64 handle_page_fault shape, trimmed to its load-bearing arms).
fn handle_page_fault(regs: &mut PtRegs, access_type: u32) {
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
                fault_sigsegv_route();
            } else {
                crate::dfx::oops::oops(
                    crate::dfx::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
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
                let _ = crate::signal::send_signal_directed(
                    pid,
                    crate::signal::Signal::SIGBUS as i32,
                );
                if !sigbus_has_handler() {
                    crate::process::exit::do_exit(-(crate::signal::Signal::SIGBUS as i32));
                } else {
                    crate::signal::force_unblock_current(crate::signal::Signal::SIGBUS as i32);
                }
            } else {
                crate::dfx::oops::oops(
                    crate::dfx::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
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
                crate::dfx::oops::oops(
                    crate::dfx::oops::Kind::PageFault { addr: fault_addr },
                    &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                        oops_show_regs(w, regs);
                    },
                );
            }
        }
        MmFaultResult::KernelPanic => {
            crate::dfx::oops::oops(
                crate::dfx::oops::Kind::PageFault { addr: fault_addr },
                &|w: &mut crate::dfx::backtrace::ConsoleWriter| {
                    oops_show_regs(w, regs);
                },
            );
        }
        MmFaultResult::AlreadyMapped | MmFaultResult::CowPending => {
            // Nothing to do; the instruction retries.
        }
    }
}

/// Kernel-thread trampoline helper (riscv64 twin contract): called by
/// ret_from_fork_kernel_asm with the fn/arg restored from
/// thread.s[0]/s[1]; runs the thread function and do_exit — it never
/// returns (the asm parks defensively).
#[no_mangle]
pub extern "C" fn ret_from_fork_kernel_helper(
    fn_arg: *mut core::ffi::c_void,
    fn_ptr: extern "C" fn(*mut core::ffi::c_void) -> i32,
    _regs: &mut PtRegs,
) {
    let ret = fn_ptr(fn_arg);
    crate::process::exit::do_exit(ret);
}

// ============================================================================
// Vector-table install + timer/IRQ arming (interface parity)
// ============================================================================

/// Initialize trap handling: install the vector table (VBAR_EL1)
pub fn init() {
    extern "C" {
        fn aarch64_vector_table();
    }
    // SAFETY: writing VBAR_EL1 installs the exception base; the table is
    // 2KB-aligned by the asm. ISB makes the new vectors effective.
    unsafe {
        asm!(
            "msr vbar_el1, {v}",
            "isb",
            v = in(reg) aarch64_vector_table as u64,
            options(nostack)
        );
    }
}

pub fn init_syscall() {
    // SVC #0 is dispatched in aarch64_trap_handler (EC 0x15)
}

/// Enable the EL1 timer interrupt (unmask CNTV_CTL; GIC PPI wiring is
/// stage 3 — until then the PPI stays masked at the distributor)
pub fn enable_timer_interrupt() {
    // SAFETY: unmasking the virtual timer only arms a PPI the GIC still
    // holds masked until stage 3; no interrupt can fire from this alone.
    unsafe {
        asm!(
            "msr cntv_ctl_el0, {v}",
            v = in(reg) 1u64, // enable, imask=0
            options(nomem, nostack)
        );
    }
}

/// Disable the EL1 timer interrupt
pub fn disable_timer_interrupt() {
    // SAFETY: masking the virtual timer; no side effects beyond the mask.
    unsafe {
        asm!(
            "msr cntv_ctl_el0, {v}",
            v = in(reg) 3u64, // enable=1, imask=1 (stay armed, masked)
            options(nomem, nostack)
        );
    }
}

/// Enable external interrupts (GICv3 CPU interface — stage 3)
pub fn enable_external_interrupt() {
    crate::drivers::intc::gicv3::enable_cpu_interface();
}

/// TrapFrame / ExceptionCause aliases (interface parity)
pub type TrapFrame = PtRegs;
pub type ExceptionCause = Cause;
