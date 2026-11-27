//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 process/thread arch hooks: exec entry state, thread flush.
//! (fork/copy_thread is generic — process/fork.rs writes the child's
//! frame at `Task::pt_regs()` = stack_top - 168 and sets
//! thread.callee.ret_addr = ret_from_fork, thread.sp = frame address.)


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

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
///
/// Collected into `arch::arch_floor` (INV-ARCH-PROCESS-6): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn flush_thread() {
    crate::arch::arch_floor::x86_64::process::flush_thread()
}

// ============================================================================
// arch_prctl(2) — x86_64-only syscall (nr 158)
// ============================================================================

// Codes per asm/prctl.h (verified against the host header).
pub(crate) const ARCH_SET_GS: u64 = 0x1001;
pub(crate) const ARCH_SET_FS: u64 = 0x1002;
pub(crate) const ARCH_GET_FS: u64 = 0x1003;
pub(crate) const ARCH_GET_GS: u64 = 0x1004;
pub(crate) const MSR_FS_BASE: u32 = 0xC000_0100;
pub(crate) const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;
/// arch_prctl(code, addr) — TLS base management. glibc/musl startup calls
///
/// Collected into `arch::arch_floor` (INV-ARCH-PROCESS-5): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn sys_arch_prctl(args: crate::syscall::SyscallArgs) -> i64 {
    crate::arch::arch_floor::x86_64::process::sys_arch_prctl(args)
}

/// ioperm(2) — x86_64 native nr 173. The reference maintains a per-task
/// I/O permission bitmap consulted by user-mode port instructions. Rux
/// runs user mode without port-I/O facilities, so the bitmap has nothing
/// to gate; the syscall CONTRACT surface is what callers probe (LTP
/// ioperm01, legacy hw tools): CAP_SYS_RAWIO for permission, EINVAL when
/// [from, from+num) leaves the 0..0x10000 port space, 0 otherwise.
/// History: implemented in 4081cf356, lost in a later dispatch refactor;
/// the fallthrough era answered via generic getppid (positive pid read
/// as success by LTP's `rc != -1` check — a fake PASS), and the
/// native-table sweep (a1dce23b7) closed the fallthrough with ENOSYS,
/// which exposed the loss as ioperm01 FAIL. This restores the contract.
pub fn sys_ioperm(args: crate::syscall::SyscallArgs) -> i64 {
    let from = args[0];
    let num = args[1];
    if !crate::security::capable(crate::security::CAP_SYS_RAWIO) {
        return -crate::errno::constants::EPERM as i64;
    }
    if (from + num) > 0x10000 || (from + num) < from {
        return -crate::errno::constants::EINVAL as i64;
    }
    0
}

/// iopl(2) — x86_64 native nr 172. Same contract surface: levels 0..=3
/// accepted (0 = normal, 3 = all ports), anything else EINVAL; requires
/// CAP_SYS_RAWIO. No EFLAGS.IOPL switch exists to perform.
pub fn sys_iopl(args: crate::syscall::SyscallArgs) -> i64 {
    let level = args[0] as i32;
    if !crate::security::capable(crate::security::CAP_SYS_RAWIO) {
        return -crate::errno::constants::EPERM as i64;
    }
    if !(0..=3).contains(&level) {
        return -crate::errno::constants::EINVAL as i64;
    }
    0
}
