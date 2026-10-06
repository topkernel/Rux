//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 process/thread arch hooks: exec entry state, thread flush.
//! (fork/copy_thread is generic — process/fork.rs writes the child's
//! frame at `Task::pt_regs()` = stack_top - 168 and sets
//! thread.callee.ret_addr = ret_from_fork, thread.sp = frame address.)

use crate::arch::pt_regs::PtRegs;

/// Initial register state for a new user program (execve).
///
/// - rip = entry point, rsp = user stack top
/// - argument registers cleared (argc/argv/envp arrive on the stack,
///   per the x86-64 SysV ABI initial-process layout)
/// - cs/ss = user selectors (0x33 / 0x2b) so iretq returns to CPL 3
/// - rflags = 0x202: reserved bit 1 (must be 1) + IF (user runs with
///   interrupts enabled)
#[inline]
pub fn start_thread(regs: &mut PtRegs, pc: u64, sp: u64) {
    regs.rip = pc;
    regs.rsp = sp;

    regs.rax = 0;
    regs.rdi = 0;
    regs.rsi = 0;
    regs.rdx = 0;
    regs.rcx = 0;
    regs.r10 = 0;
    regs.r8 = 0;
    regs.r9 = 0;
    regs.r11 = 0;
    regs.rbx = 0;
    regs.rbp = 0;
    regs.r12 = 0;
    regs.r13 = 0;
    regs.r14 = 0;
    regs.r15 = 0;

    regs.cs = super::trap::USER_CS;
    regs.ss = super::trap::USER_DS;
    regs.rflags = 0x202;

    // No pending syscall rollback on a fresh image.
    regs.orig_rax = 0;
}

/// Reset the FPU and TLS state on exec (the twin's flush_thread):
/// zeroed fxsave image marked valid (fresh tasks restore deterministic
/// FP state — ARCH-H1 parity) and fs/gs bases cleared.
pub fn flush_thread() {
    if let Some(current) = crate::sched::current() {
        // SAFETY: current() returns the running task; we are the only
        // context that can touch our own thread state here.
        unsafe {
            let thread = (*current).thread_mut();
            thread.mark_fpu_clean();
            thread.fs_base = 0;
            thread.gs_base = 0;

            // Drop the LIVE FS base too: without this the next
            // context-switch-out would resurrect the old image's TLS
            // pointer from the MSR (the twin's live-FS=Off discipline,
            // x86 form: the exec'ing image has no TLS until
            // set_thread_area/arch_prctl installs one).
            super::cpu::wrmsr(0xC000_0100, 0); // MSR_FS_BASE
            super::cpu::wrmsr(0xC000_0101, 0); // MSR_GS_BASE
        }
    }
}
