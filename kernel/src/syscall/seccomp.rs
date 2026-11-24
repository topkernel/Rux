//! The seccomp syscall gate (framework home; moved from
//! syscall/process.rs — crate-flip State B): the arch traps call this
//! BEFORE dispatch — entry-seam by nature (the step-4 decision). The
//! filter interpreter moved with it; the syscall-side loading arms
//! (prctl) reference the pub consts and the task's mode setters.

use crate::errno::constants as errno;
// The BPF instruction consts (moved with the interpreter — the loader
// in the service crate uses its own #[path] copy's scope).
pub const BPF_LD: u16 = 0x00;
pub const BPF_W: u16 = 0x00;
pub const BPF_H: u16 = 0x08;
pub const BPF_B: u16 = 0x10;
pub const BPF_IMM: u16 = 0x00;
pub const BPF_ABS: u16 = 0x20;
pub const BPF_JMP: u16 = 0x05;
pub const BPF_JEQ: u16 = 0x10;
pub const BPF_JGT: u16 = 0x20;
pub const BPF_JGE: u16 = 0x30;
pub const BPF_JSET: u16 = 0x40;
pub const BPF_RET: u16 = 0x06;
pub const BPF_K: u16 = 0x00;

/// Seccomp modes (uapi/linux/seccomp.h).
pub const SECCOMP_MODE_DISABLED: u32 = 0;
pub const SECCOMP_MODE_STRICT: u32 = 1;
pub const SECCOMP_MODE_FILTER: u32 = 2;

/// seccomp(2) operations.
pub const SECCOMP_SET_MODE_STRICT: u32 = 0;
pub const SECCOMP_SET_MODE_FILTER: u32 = 1;
const SECCOMP_GET_ACTION_AVAIL: u32 = 2;
const SECCOMP_GET_NOTIF_SIZES: u32 = 3;

/// SECCOMP_FILTER_FLAG_* (accepted and ignored except TSYNC semantics).
#[allow(dead_code)]
pub const SECCOMP_FILTER_FLAG_TSYNC: u32 = 1;
#[allow(dead_code)]
pub const SECCOMP_FILTER_FLAG_LOG: u32 = 2;
#[allow(dead_code)]
pub const SECCOMP_FILTER_FLAG_SPEC_ALLOW: u32 = 4;
#[allow(dead_code)]
pub const SECCOMP_FILTER_FLAG_NEW_LISTENER: u32 = 8;
#[allow(dead_code)]
pub const SECCOMP_FILTER_FLAG_TSYNC_ESRCH: u32 = 16;

/// SECCOMP_RET_* action values.
pub const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
#[allow(dead_code)]
pub const SECCOMP_RET_KILL_THREAD: u32 = 0x0000_0000;
pub const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
#[allow(dead_code)]
pub const SECCOMP_RET_TRACE: u32 = 0x7ff0_0000;
#[allow(dead_code)]
pub const SECCOMP_RET_LOG: u32 = 0x7ffc_0000;
pub const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;

/// AUDIT_ARCH: __AUDIT_ARCH_64BIT | __AUDIT_ARCH_LE | EM_<arch>.
/// The filter's seccomp_data.arch token must name the arch whose NATIVE
/// syscall numbers the filter sees (the gate runs on the raw trap number:
/// riscv64 native = the generic table, x86_64 native = the x86_64 table —
/// the gate runs BEFORE x86_compat translation, which matches the
/// reference kernel where filters always see native numbers).
#[cfg(feature = "riscv64")]
pub const AUDIT_ARCH: u32 = 0x8000_0000 | 0x4000_0000 | 243; // AUDIT_ARCH_RISCV64
#[cfg(feature = "x86_64")]
pub const AUDIT_ARCH: u32 = 0x8000_0000 | 0x4000_0000 | 62; // AUDIT_ARCH_X86_64
#[cfg(feature = "aarch64")]
pub const AUDIT_ARCH: u32 = 0x8000_0000 | 0x4000_0000 | 183; // AUDIT_ARCH_AARCH64

/// seccomp_data size: nr + arch + ip + 6 args = 64 bytes.
const SECCOMP_DATA_SIZE: u32 = 4 + 4 + 8 + 6 * 8;


/// Syscalls allowed in STRICT mode — the RUNNING ARCH'S NATIVE numbers,
/// because the trap gate hands the raw trap number to seccomp_syscall_gate
/// (riscv64: the generic table; x86_64: regs.rax BEFORE x86_compat
/// translation). The single riscv64-only list was an ABI bug: on x86_64
/// a STRICT task was SIGKILLed on its first native read(0)/write(1) and
/// uname(63) slipped through as if it were read.
#[cfg(feature = "riscv64")]
pub const STRICT_ALLOWED: [u64; 5] = [
    63,  // read
    64,  // write
    93,  // exit
    94,  // exit_group
    139, // rt_sigreturn
];

#[cfg(feature = "x86_64")]
pub const STRICT_ALLOWED: [u64; 5] = [
    0,   // read
    1,   // write
    60,  // exit
    231, // exit_group
    15,  // rt_sigreturn
];

/// aarch64: asm-generic syscall numbers — identical to riscv64's set
#[cfg(feature = "aarch64")]
pub const STRICT_ALLOWED: [u64; 5] = [
    63,  // read
    64,  // write
    93,  // exit
    94,  // exit_group
    139, // rt_sigreturn
];

/// seccomp gate result: None = allow, Some(rv) = block with syscall return
/// value rv. Kill actions queue SIGKILL on the current task (terminated on
/// return to user) and report -ENOSYS — the dying task never observes it.
pub fn seccomp_syscall_gate(nr: u64, regs: &crate::arch::pt_regs::PtRegs) -> Option<i64> {
    let task = crate::sched::current()?;
    match task.seccomp_mode() {
        SECCOMP_MODE_STRICT => {
            if STRICT_ALLOWED.contains(&nr) {
                None
            } else {
                crate::pr_warn!(
                    "seccomp: STRICT violation pid={} nr={} — SIGKILL",
                    task.pid(),
                    nr
                );
                task.seccomp_kill();
                Some(-(crate::errno::constants::ENOSYS as i64))
            }
        }
        SECCOMP_MODE_FILTER => {
            let guard = task.seccomp_filter.lock();
            let Some(prog) = guard.as_ref() else {
                return None; // no program loaded — treat as allow
            };
            let action = crate::syscall::support::seccomp_run(prog, nr, regs);
            drop(guard);
            match action & 0xffff_0000 {
                SECCOMP_RET_ALLOW => None,
                SECCOMP_RET_ERRNO => Some(-((action & 0xffff) as i64)),
                SECCOMP_RET_KILL_PROCESS | _ => {
                    crate::pr_warn!(
                        "seccomp: filter kill pid={} nr={} action={:#x}",
                        task.pid(),
                        nr,
                        action
                    );
                    task.seccomp_kill();
                    Some(-(crate::errno::constants::ENOSYS as i64))
                }
            }
        }
        _ => None,
    }
}

/// The classic-BPF filter interpreter MOVED to the rux-syscall service
/// crate (cold-four lane): it is a pure function over &[SockFilter]
/// that only runs when the current task has a filter installed (the
/// gate below reaches it through the SyscallHooks row — tasks without
/// a filter, i.e. every ordinary syscall, never touch the table).
pub use crate::syscall::support::seccomp_run;

