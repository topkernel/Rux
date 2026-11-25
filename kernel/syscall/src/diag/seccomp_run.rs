//! The classic-BPF seccomp filter interpreter (moved from
//! kernel/core/src/syscall/seccomp.rs at the cold-four policy flip — sole
//! copy here; zero behavior change). A pure function over
//! &[SockFilter]; reached through the SyscallHooks::seccomp_run row
//! ONLY when the current task has a filter installed (the gate checks
//! seccomp_mode first — ordinary tasks never touch the table).

use crate::seccomp::*;

/// seccomp_data size (nr + arch + ip + 6 args = 64 bytes) — the
/// framework module's private const, restated at the sole consumer
/// (the interpreter) after the move.
const SECCOMP_DATA_SIZE: u32 = 4 + 4 + 8 + 6 * 8;

/// Classic-BPF interpreter over the seccomp_data pseudo-packet:
///   offset  0: nr (u32)           offset  4: arch (u32)
///   offset  8: instruction_pointer offset 16: args[0..6] (u64 each)
/// Returns the RET value (default KILL on malformed access — Linux kills
/// the task for out-of-bounds loads).
pub fn seccomp_run(
    prog: &[::process::task::SockFilter],
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
