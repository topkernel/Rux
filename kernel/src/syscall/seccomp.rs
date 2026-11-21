//! The seccomp syscall gate (framework home; moved from
//! syscall/process.rs — crate-flip State B): the arch traps call this
//! BEFORE dispatch — entry-seam by nature (the step-4 decision). The
//! filter interpreter moved with it; the syscall-side loading arms
//! (prctl) reference the pub consts and the task's mode setters.

use crate::errno::constants as errno;
use super::process::{
    BPF_LD, BPF_W, BPF_H, BPF_B, BPF_IMM, BPF_ABS, BPF_JMP, BPF_JEQ,
    BPF_JGT, BPF_JGE, BPF_JSET, BPF_RET, BPF_K,
};

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
            let action = seccomp_run(prog, nr, regs);
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

/// Classic-BPF interpreter over the seccomp_data pseudo-packet:
///   offset  0: nr (u32)           offset  4: arch (u32)
///   offset  8: instruction_pointer offset 16: args[0..6] (u64 each)
/// Returns the RET value (default KILL on malformed access — Linux kills
/// the task for out-of-bounds loads).
fn seccomp_run(
    prog: &[crate::process::task::SockFilter],
    nr: u64,
    regs: &crate::arch::pt_regs::PtRegs,
) -> u32 {
    let mut a: u32 = 0;
    let mut pc: usize = 0;

    // seccomp_data as u32 words (little-endian layout).
    let mut data = [0u32; 16];
    data[0] = nr as u32;
    data[1] = AUDIT_ARCH;
    let ip = regs.instruction_pointer();
    data[2] = ip as u32;
    data[3] = (ip >> 32) as u32;
    let arg_words = regs.syscall_args();
    for (i, w) in arg_words.iter().enumerate() {
        data[4 + i * 2] = *w as u32;
        data[5 + i * 2] = (*w >> 32) as u32;
    }

    let load_word = |off: u32| -> Option<u32> {
        if off % 4 != 0 || off + 4 > SECCOMP_DATA_SIZE {
            return None;
        }
        Some(data[(off / 4) as usize])
    };

    while pc < prog.len() {
        let ins = &prog[pc];
        let class = ins.code & 0x07;
        match class {
            x if x == BPF_LD => {
                let mode = ins.code & 0xe0;
                if mode == BPF_IMM {
                    a = ins.k;
                } else if mode == BPF_ABS {
                    match load_word(ins.k) {
                        Some(v) => a = v,
                        // Out-of-bounds load → kill (RET 0).
                        None => return 0,
                    }
                } else {
                    return 0;
                }
                pc += 1;
            }
            x if x == BPF_JMP => {
                let op = ins.code & 0xf0;
                let cond = match op {
                    o if o == BPF_JEQ => a == ins.k,
                    o if o == BPF_JGT => a > ins.k,
                    o if o == BPF_JGE => a >= ins.k,
                    o if o == BPF_JSET => a & ins.k != 0,
                    _ => return 0,
                };
                pc += if cond { ins.jt as usize } else { ins.jf as usize } + 1;
            }
            x if x == BPF_RET => {
                return ins.k;
            }
            _ => return 0,
        }
    }
    // Fell off the end: kill.
    0
}
