//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V exception handling
//!
//! Handle various exceptions and interrupts


// The unsafe floor for this file lives in kernel/src/arch/unsafe.rs
// (named INV-contracted operations); this file is compiler-enforced safe.
#![forbid(unsafe_code)]

use core::arch::asm;
use super::unsafe_edges::SavedFrame;
use crate::process::task::TaskState;
use riscv::register::sie;
use crate::fs::procfs::interrupts;

// Include trap.S assembly code.
//
// R57: the Task-field offsets this asm uses are injected as template
// constants derived from offset_of! (task.rs task_offsets) — the
// hand-maintained .set constants they replace drifted by 8 when
// ti_pick_state was inserted into Task, and every user trap's scratch
// store then wrote a live user register over the pick-state word (the
// GNOME wedge scribbler). Derived constants cannot drift.

/// Get pt_regs for current task
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-37): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn current_task_pt_regs() -> Option<&'static mut PtRegs> {
    crate::arch::arch_floor::riscv64::trap::current_task_pt_regs()
}

// Re-export PtRegs and related constants
pub use super::pt_regs::{PtRegs, Cause, PT_REGS_SIZE};
pub use super::pt_regs::{SR_SPP, SR_PIE, SR_SIE, SR_SUM};

/// Current CPU's PtRegs pointer (used for fork) — per-CPU to support SMP
pub(crate) static CURRENT_PT_REGS: [core::sync::atomic::AtomicU64; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Get current PtRegs pointer
/// Used for fork to copy parent's trap state
pub fn current_pt_regs() -> *const PtRegs {
    let cpu = crate::arch::cpu_id() as usize;
    CURRENT_PT_REGS[cpu].load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs
}
/// Wakewatch: RIP-equivalent (epc) of another CPU's most recent trap
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-36): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn pt_regs_rip_for_cpu(cpu: usize) -> u64 {
    crate::arch::arch_floor::riscv64::trap::pt_regs_rip_for_cpu(cpu)
}
/// Initialize trap handling
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-35): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn init() {
    crate::arch::arch_floor::riscv64::trap::init()
}

pub fn init_syscall() {
    // RISC-V uses ecall instruction, dispatched in exception handler
}
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-34): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn enable_timer_interrupt() {
    crate::arch::arch_floor::riscv64::trap::enable_timer_interrupt()
}
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-33): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn disable_timer_interrupt() {
    crate::arch::arch_floor::riscv64::trap::disable_timer_interrupt()
}
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-32): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub fn enable_external_interrupt() {
    crate::arch::arch_floor::riscv64::trap::enable_external_interrupt()
}


// ============================================================================
// Raw-uart bounded reporters (the panic-path printers — the x86_64
// context.rs twins). The KERNPANIC path may run on a broken machine:
// no locks, no allocation, straight UART bytes.
// ============================================================================

/// One line fragment straight to the raw UART.
#[inline]
pub(crate) fn putfrag(msg: &[u8]) {
    for &b in msg {
        crate::console::putchar_no_lock(b);
    }
}

/// `v` as hex (no 0x, 16 nibbles) straight to the raw UART.
#[inline]
pub(crate) fn puthex(v: u64) {
    for sh in (0..64).step_by(4).rev() {
        let n = ((v >> sh) & 0xF) as u8;
        crate::console::putchar_no_lock(if n < 10 { b'0' + n } else { b'a' + n - 10 });
    }
}

/// One byte straight to the raw UART.
#[inline]
pub(crate) fn putb(b: u8) {
    crate::console::putchar_no_lock(b);
}


/// riscv64 twin of the x86_64 IRQ-exit kernel-preempt hook
/// (trap.rs there, same name): after the timer or IPI handler and its
/// irq_exit(), a KERNEL-mode incumbent whose need_resched is set gets
/// schedule()d out instead of running until its next syscall return.
/// Without this, every wake behind a kernel-executing CPU is quantized
/// by the incumbent's syscall length (measured on the x86_64 twin with
/// the pbench p2f futex suite: TCG 5608us vs reference 86us; KVM
/// 163us vs 8.2us; hook collapses both to parity — see the x86_64
/// comment for the full table and the sc70-era leak-amplification
/// history that kept this OFF until WAKE-HEAL bounded it).
///
/// Guards, all mirroring the x86_64 twin:
/// - kpreempt=1 arms / kpreempt=0 disarms / absent = arch default OFF
///   on riscv64 (this twin is not yet stable — see the verdict-cache
///   comment below; the x86_64 twin defaults ON since RT-plan R1,
///   2026-11-18 — flip this twin only after the R53/R84/R85 asm frame
///   families are stabilized, with its own suite + soak numbers).
/// - user-origin frames are skipped: ret_from_exception's asm loop
///   owns user preemption.
/// - preempt_count != 0 (spinlock/bh/RCU held) refuses the switch.
/// - TIF_IN_SCHED refuses re-entry (a tick inside __schedule's tail).
/// - SIE discipline (R53 form-B): the asm kernel path twin does
///   `csrsi sstatus,2` before asm_schedule and `csrci sstatus,2`
///   after; __schedule returns with SIE per R54's syscall-caller
///   contract, so the hook re-disables before unwinding the handler.
pub(crate) fn irq_exit_maybe_preempt_kernel(regs: &PtRegs) {
    use crate::process::task::task_flags::TaskFlags;
    static KP_ARMED: core::sync::atomic::AtomicU8 =
        core::sync::atomic::AtomicU8::new(0);
    match KP_ARMED.load(core::sync::atomic::Ordering::Relaxed) {
        0 => {
            match crate::cmdline::get_cmdline() {
                None => return, // cmdline not parsed yet: stay disarmed
                Some(_) => {
                    // Same cmdline contract as the x86_64 twin:
                    // kpreempt=1 arms, kpreempt=0 disarms, ABSENT takes
                    // the arch default. riscv64 default: OFF — this
                    // twin's first A/B (pbench p2f, 2026-10-31) hit the
                    // pick-machinery corruption the asm comments warn
                    // about (R46/R49 phantom-heal cascade into an idle-
                    // task user-frame KERNPANIC ~30s into the quiet
                    // regime) while the x86_64 twin, whose guards were
                    // tuned through sc65-sc73, runs the same suite
                    // clean. Stabilize against the R53/R84/R85 asm
                    // families first (the .Lcheck_resched_kernel dead
                    // path documents the missing frame discipline), then
                    // flip this default with suite + soak numbers.
                    let v = if crate::cmdline::get_param("kpreempt")
                        .as_deref()
                        == Some("1")
                    {
                        2u8
                    } else {
                        1u8
                    };
                    KP_ARMED.store(v, core::sync::atomic::Ordering::Relaxed);
                    if v == 2 {
                        // HOTPATH-PRINT: preempt-leak report — one-shot latch above
                        crate::pr_info!(
                            "sched: kernel-preempt armed by cmdline (cpu={})",
                            crate::arch::cpu_id()
                        );
                    }
                }
            }
        }
        1 => return, // disarmed (default; kpreempt=0)
        _ => {}
    }
    // Early-boot secondaries have no current task: nothing to preempt.
    let cur = match crate::sched::current() {
        Some(t) => t,
        None => return,
    };
    if regs.user_mode() {
        return; // user path: ret_from_exception asm owns preemption
    }
    if !crate::sched::need_resched() {
        return;
    }
    if !crate::interrupt::preempt::preemptible() {
        return; // holds a lock / bh / RCU / nested IRQ
    }
    if cur.test_ti_flag(TaskFlags::TIF_IN_SCHED.bits()) {
        return;
    }
    // Enable around schedule() like the asm twin (preempt_schedule_irq
    // discipline); re-disable after so the remaining handler frames and
    // the sret ritual run SIE=0 (R53 form-B window).
    crate::arch::cpu::restore_irq(true);
    crate::sched::schedule();
    crate::arch::cpu::restore_irq(false);
}
/// Handle timer interrupt
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-30): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) 
fn handle_timer_interrupt(regs: &mut PtRegs, cpu: usize) {


                                                         // Increment interrupt counter for /proc/interrupts
                                                         interrupts::timer_inc(cpu);

                                                         // Re-arm timer: set stimecmp to a future deadline.
                                                         crate::drivers::timer::set_next_trigger();

                                                         // Skip scheduler/schedule logic during early SMP boot (tp = hart_id,
                                                         // no current task).  scheduler_tick() may call wake_up_process() which
                                                         // interacts with the runqueue; schedule() with null current returns
                                                         // early but the intermediate state can be inconsistent.
                                                         if crate::sched::current().is_none() {
                                                             return;
                                                         }

                                                         // 0. Per-CPU time accounting for /proc/stat (Linux
                                                         // account_process_tick): the idle task means the CPU had nothing to
                                                         // run, otherwise the tick is user time (the timer hit the task in
                                                         // user mode) or system time (it hit it in kernel mode).
                                                         crate::fs::procfs::stat::charge_tick(regs.user_mode());

                                                         // 0b. CPU-time accounting (Linux account_process_tick): charge this
                                                         // tick to the interrupted task as user or system time depending on
                                                         // the privilege mode the timer hit. Only user tasks are charged
                                                         // (kthreads are born without an address space).
                                                         if let Some(task) = crate::sched::current() {
                                                             // SAFETY: current() returned a live task pointer; we only touch
                                                             // per-task atomic counters, no teardown race on these fields.

                                                                 if (*task).address_space().is_some() {
                                                                     use core::sync::atomic::Ordering::Relaxed;
                                                                     if regs.user_mode() {
                                                                         (*task).utime_ticks.fetch_add(1, Relaxed);
                                                                         crate::debug::syscost::tick_user();
                                                                         // perf events (PERF_TYPE_SOFTWARE sampling): stash the
                                                                         // interrupted PC for the softirq charger. Lock-free.
                                                                         crate::perf::note_tick(regs.epc, crate::perf::MODE_USER, (*task).pid());
                                                                     } else {
                                                                         (*task).stime_ticks.fetch_add(1, Relaxed);
                                                                         crate::debug::syscost::tick_kern(regs.epc);
                                                                         crate::perf::note_tick(regs.epc, crate::perf::MODE_KERNEL, (*task).pid());
                                                                     }
                                                                 } else {
                                                                     crate::debug::syscost::tick_idle();
                                                                     crate::perf::note_tick(regs.epc, crate::perf::MODE_IDLE, 0);
                                                                 }

                                                         } else {
                                                             crate::debug::syscost::tick_idle();
                                                             crate::perf::note_tick(regs.epc, crate::perf::MODE_IDLE, 0);
                                                         }

                                                         // 1. Update jiffies
                                                         crate::drivers::timer::timer_interrupt_handler();

                                                         // 2. Scheduler tick
                                                         crate::sched::scheduler_tick();

                                                         // 3. Check for soft lockups
                                                         crate::debug::softlockup::check();

                                                         // 4. Reschedule if needed
                                                         if crate::sched::need_resched() && crate::interrupt::preempt::preemptible() {
                                                             crate::sched::schedule();
                                                         }

}
/// Handle software interrupt (IPI)
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-29): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_software_interrupt(_regs: &mut PtRegs, cpu: usize) {
    crate::arch::arch_floor::riscv64::trap::handle_software_interrupt(_regs, cpu)
}

/// Handle external interrupt
///
/// Claims the highest-priority pending IRQ from PLIC and dispatches
/// through the IRQ framework. EOI (PLIC complete) is done by the
/// flow handler via irq_chip.irq_eoi. Fallback complete only for
/// spurious/unmapped IRQs that bypass the flow handler.
pub(crate) fn handle_external_interrupt(_regs: &mut PtRegs, cpu: usize) {

    if let Some(hwirq) = crate::drivers::intc::plic::claim(cpu) {
        let handled = if let Some(domain) = crate::interrupt::get_default_domain() {
            crate::interrupt::generic_handle_domain_irq(domain, hwirq as u32)
        } else {
            false
        };
        // Only do fallback EOI for spurious/unmapped IRQs that bypassed
        // handle_fasteoi_irq. Normal IRQs already got EOI via irq_chip.irq_eoi.
        if !handled {
            crate::drivers::intc::plic::complete(cpu, hwirq);
        }
    }
}
/// Handle system call
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-28): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_syscall(regs: &mut PtRegs) {
    crate::arch::arch_floor::riscv64::trap::handle_syscall(regs)
}
/// Handle illegal instruction
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-27): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) 
fn handle_illegal_instruction(regs: &mut PtRegs) {


                                                 let epc = regs.epc;

                                                 // Read the instruction through the exception-table copy path — for
                                                 // user-mode faults epc is a USER address, which a raw read_unaligned
                                                 // would fault on with SUM=0 (review ARCH).
                                                 let mut insn_buf = [0u8; 4];
                                                 let uncopied16 = 
                                                     crate::arch::riscv64::uaccess::copy_from_user(insn_buf.as_mut_ptr(), epc as *const u8, 2)
                                                 ;
                                                 if uncopied16 > 0 {
                                                     // Instruction stream unreadable: cannot decode or retry safely.
                                                     crate::pr_debug!("trap: unreadable instruction at epc={:#x}", epc);
                                                     if regs.user_mode() {
                                                         crate::process::exit::do_exit(-(crate::signal::Signal::SIGILL as i32));
                                                     }
                                                     return;
                                                 }
                                                 let instr16 = u16::from_ne_bytes([insn_buf[0], insn_buf[1]]);

                                                 // Check if this is a compressed (16-bit) instruction
                                                 let is_compressed = (instr16 & 0x3) != 0x3;
                                                 let instr_size = if is_compressed { 2 } else { 4 };

                                                 // Check if FPU is disabled (FS = OFF) and this might be an FP instruction
                                                 const SR_FS: u64 = 0x3 << 13;
                                                 const SR_FS_INITIAL: u64 = 0x1 << 13;
                                                 let fs = regs.status & SR_FS;
                                                 if fs == 0 {
                                                     // FPU is off - check if this is an FP instruction
                                                     let is_fp = if is_compressed {
                                                         // Compressed FP instructions on RV64 with D extension:
                                                         // Quadrant 0 (bits[1:0]=00): C.FLD (funct3=001), C.FSD (funct3=101)
                                                         // Quadrant 2 (bits[1:0]=10): C.FLDSP (funct3=001), C.FSDSP (funct3=101)
                                                         // So for any compressed inst: funct3 (bits[15:13]) = 001 or 101 means FP
                                                         let funct3 = (instr16 >> 13) & 0x7;
                                                         funct3 == 1 || funct3 == 5
                                                     } else {
                                                         // 32-bit FP instructions:
                                                         // Load/Store: opcode[6:0] = 0000111 (FLW/FLD) or 0100111 (FSW/FSD)
                                                         // FP compute: opcode[6:0] = 0000101 (FMADD etc) or 0001001 (FMSUB etc)
                                                         //             or 0001101 (FNMSUB etc) or 0001110 (FNMADD etc) or 1010011 (FP ops)
                                                         let instr32: u32 = if instr_size == 4 {
                                                             let _ = 
                                                                 crate::arch::riscv64::uaccess::copy_from_user(
                                                                     insn_buf.as_mut_ptr(),
                                                                     epc as *const u8,
                                                                     4,
                                                                 )
                                                             ;
                                                             u32::from_ne_bytes(insn_buf)
                                                         } else {
                                                             instr16 as u32
                                                         };
                                                         let opcode = instr32 & 0x7F;
                                                         // FP-state CSR accesses also require FS != Off: csrrw/csrrs/
                                                         // csrrc to fflags(0x001)/frm(0x002)/fcsr(0x003) — the frrm/
                                                         // frflags/fsrm... assembler forms — trap illegal exactly like
                                                         // compute FP when FS=Off. compiler-rt's __multf3/__udivti3
                                                         // (i128 emulation, so any shell/libc integer arithmetic that
                                                         // widens) reads the rounding mode with `frrm` before touching
                                                         // the FP registers, so this class is reachable from binaries
                                                         // that never execute an explicit FP instruction (repro: mrsh
                                                         // SIGILL at a deterministic __multf3+frrm under the pa49
                                                         // trace-capture boot).
                                                         let is_fp_csr = opcode == 0x73 && {
                                                             let csr = instr32 >> 20;
                                                             csr == 0x001 || csr == 0x002 || csr == 0x003
                                                         };
                                                         opcode == 0x07 || opcode == 0x27 ||   // FLW/FLD, FSW/FSD
                                                         opcode == 0x05 || opcode == 0x09 ||   // FMADD, FMSUB
                                                         opcode == 0x0D || opcode == 0x0E ||   // FNMSUB, FNMADD
                                                         opcode == 0x53 ||                     // FP ops (FADD, FSUB, FMUL, FDIV, etc.)
                                                         is_fp_csr                             // fflags/frm/fcsr via SYSTEM opcode
                                                     };

                                                     if is_fp {
                                                         // First FP use: enable the FPU AND zero the FP registers/fcsr.
                                                         // FS=Initial means the registers architecturally read as zero;
                                                         // without the explicit clear, the registers still hold whatever
                                                         // the previous task on this CPU left there (cross-task numeric
                                                         // pollution + information leakage — review ARCH, fpu_init had
                                                         // zero callers).
                                                         // SAFETY: fpu_init sets live sstatus.FS=Initial before touching
                                                         // the FP registers (executing FP with FS=Off would trap) and
                                                         // zeroes f0-f31 + fcsr. The interrupted context cannot have live
                                                         // FP state: FS was Off, so every FP instruction trapped.
                                                          crate::arch::riscv64::thread::fpu_init(); 
                                                         // Enable FPU in the SAVED status too, so sret does not undo it.
                                                         regs.status = (regs.status & !SR_FS) | SR_FS_INITIAL;
                                                         return;
                                                     }
                                                 }

                                                 // V-extension first use: while sstatus.VS = Off, EVERY vector
                                                 // instruction traps illegal-instruction (any privilege mode).
                                                 // Lazy-enable like the FP path above: enable VS, zero the live
                                                 // vector state (initial-state semantics), retry. Without this,
                                                 // toolchain-auto-vectorized binaries SIGILL on V-capable harts
                                                 // even though the hardware can run them (reference-kernel
                                                 // semantics: RVV code executes when the ISA advertises V; see
                                                 // arch/riscv64/vext.rs).
                                                 //
                                                 // HOISTED out of the `fs == 0` block (session 9, modchurn member
                                                 // repro): the branch used to live INSIDE the FS-disabled decode,
                                                 // so a task whose FS was already enabled (any prior FP first-use —
                                                 // fpu_init sets FS=Initial; the dynamic-loader path enables it
                                                 // before main even runs) but whose VS was still Off SIGILLed on
                                                 // its first vector instruction with the V handler never reached.
                                                 // VS and FS are independent sstatus fields; the first-use enable
                                                 // of one must not be gated on the state of the other. Repro:
                                                 // dlopen-churn probe, pid 1, vsetvli at text+0x13ee, forensic
                                                 // fs=0x2000 (FS=Initial) — SIGILL death on a v=true hart.
                                                 // Note: a vector LOAD/STORE (opcode 0x07/0x27, funct3 000/101/110/
                                                 // 111) still matches the `is_fp` shape above when FS==0, costs one
                                                 // extra trap (fpu_init, retry, re-trap) and then lands here —
                                                 // correct, just one extra round trip.
                                                 const SR_VS_FIELD: u64 = 0x3 << 9;
                                                 const SR_VS_INITIAL_VAL: u64 = 0x1 << 9;
                                                 if (regs.status & SR_VS_FIELD) == 0
                                                     && crate::arch::riscv64::vext::hart_has_v()
                                                     && !is_compressed
                                                     && instr_size == 4
                                                 {
                                                     // Fetch the full word (only the first halfword was read above).
                                                     let _ = 
                                                         crate::arch::riscv64::uaccess::copy_from_user(
                                                             insn_buf.as_mut_ptr(),
                                                             epc as *const u8,
                                                             4,
                                                         )
                                                     ;
                                                     let instr32: u32 = u32::from_ne_bytes(insn_buf);
                                                     if crate::arch::riscv64::vext::is_vector_instr32(instr32) {
                                                         let mut ok = false;
                                                         if let Some(task) = crate::sched::current() {
                                                             // SAFETY: task is the current task; the trap path
                                                             // holds it on this hart with IRQs disabled.

                                                                 ok = (*task).thread_mut().vstate_first_use();

                                                         }
                                                         if ok {
                                                             // Enable V in the SAVED status too, so sret does
                                                             // not undo it; retry the instruction.
                                                             regs.status = (regs.status & !SR_VS_FIELD)
                                                                 | SR_VS_INITIAL_VAL;
                                                             return;
                                                         }
                                                     }
                                                 }

                                                 // Not an FP instruction or FPU already enabled - terminate the process
                                                 crate::pr_debug!("trap: illegal instruction at epc={:#x}, mode={}",
                                                     epc, if regs.user_mode() { "user" } else { "kernel" });

                                                 // Forensics for user SIGILL deaths (BUG-S004/S005 family: userspace
                                                 // built for extensions the QEMU CPU model lacks): the re-fetched
                                                 // instruction word + task identity. This is how "sh died at iter2" was
                                                 // traced to a c.zext.w (Zcb) halfword from an unpinned libgcc member.
                                                 if regs.user_mode() {
                                                     static EMPTY_COMM: [u8; 16] = [0u8; 16];
                                                     let (pid, comm) = crate::sched::current()
                                                         .map(|t|  ((*t).pid(), (*t).comm()) )
                                                         .unwrap_or((0, &EMPTY_COMM));
                                                     let comm_len = comm.iter().position(|&b| b == 0).unwrap_or(comm.len());
                                                     // HOTPATH-PRINT: fault-path — see the gate/one-shot context above
                                                     crate::pr_info!(
                                                         "trap: user SIGILL pid={} comm={:?} epc={:#x} insn16={:#06x} fs={:#x}",
                                                         pid,
                                                         core::str::from_utf8(&comm[..comm_len]),
                                                         epc,
                                                         instr16,
                                                         regs.status & SR_FS
                                                     );
                                                     // SIGILL-FORENSIC (gnome-sigill): when userspace manages to jump
                                                     // into a garbage/data page, the halfword alone is not enough —
                                                     // print WHICH mapping swallowed epc, the page's PTE, a hexdump of
                                                     // the page around epc, and the full VMA table so the layout of the
                                                     // dying process is recoverable from the console alone.
                                                     //
                                                     // Serial-cleanliness (print-audit lane, 2026-11): one dump is
                                                     // ~100 lines (~5 KB, tens of ms of polled UART) — "one-shot per
                                                     // death" was only cheap while deaths were rare. A respawn loop
                                                     // over a broken binary (the sh-retry family that produced this
                                                     // probe) paid it per crash. Budget 2 per boot: the first two
                                                     // deaths carry full attribution, later ones keep the one-liner
                                                     // above.
                                                     {
                                                         static SIGILL_DUMPS: core::sync::atomic::AtomicUsize =
                                                             core::sync::atomic::AtomicUsize::new(0);
                                                         if SIGILL_DUMPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 2 {
                                                             sigill_mapping_forensics(pid as usize, epc as usize);
                                                         }
                                                     }
                                                 }

                                                 if regs.user_mode() {
                                                     crate::process::exit::do_exit(-(crate::signal::Signal::SIGILL as i32));
                                                 }

                                                 // Do NOT advance epc — the task is now ZOMBIE and will not resume

}
/// SIGILL-FORENSIC (gnome-sigill): console dump of the dying task's VMA
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-26): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn sigill_mapping_forensics(pid: usize, epc: usize) {
    crate::arch::arch_floor::riscv64::trap::sigill_mapping_forensics(pid, epc)
}
/// Handle breakpoint
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-25): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) 
fn handle_breakpoint(regs: &mut PtRegs) {


                                        if regs.user_mode() {
                                            // gdmr28/r29 bring-up instrumentation (temporary, REMOVE before
                                            // merge): an untraced user ebreak is a silent SIGTRAP death
                                            // otherwise (gdm-session-worker died this way at gdmr27 with no
                                            // diagnostic). Print the epc so the site can be symbolized.
                                            let comm_now = crate::sched::current()
                                                .map(|t| {
                                                    let c = t.comm();
                                                    let len = c.iter().position(|&b| b == 0).unwrap_or(c.len());
                                                    core::str::from_utf8(&c[..len]).unwrap_or("?")
                                                })
                                                .unwrap_or("?");
                                            // PTRACE_SINGLESTEP (P1): the ebreak we displaced at epc fired —
                                            // restore the original instruction and report the step to the
                                            // tracer (or pend SIGTRAP when nobody traces). epc is NOT
                                            // advanced: the restored instruction re-executes on resume.
                                            if let Some(task) = crate::sched::current() {
                                                if (*task).single_step_active() && regs.epc == (*task).single_step_addr() {
                                                    let saved = (*task).single_step_saved_insn();
                                                    let restored = crate::process::ptrace::write_target_word(
                                                        task, regs.epc, saved,
                                                    );
                                                    (*task).clear_single_step();
                                                    if !restored {
                                                        // Could not put the instruction back — the tracee's
                                                        // text is gone (unmapped racing the step). Kill it.
                                                        crate::process::exit::do_exit(
                                                            -(crate::signal::Signal::SIGSEGV as i32));
                                                    }
                                                    if (*task).tracer_pid() != 0 {
                                                        // SAFETY: task is the current task in its trap path.

                                                            crate::process::ptrace::ptrace_stop(
                                                                task,
                                                                crate::signal::Signal::SIGTRAP as i32,
                                                                crate::signal::SigInfo::new(
                                                                    crate::signal::Signal::SIGTRAP as i32,
                                                                    crate::signal::si_code::SI_KERNEL,
                                                                    (*task).pid(), 0),
                                                            );

                                                    } else {
                                                        // Untraced single-step (raced DETACH): plain SIGTRAP.
                                                        // gdmr29 (temporary): this path is a SIGTRAP-death
                                                        // candidate for the gdm-session-worker kill — name it.
                                                        // REMOVE before merge.
                                                        //
                                                        // Demoted to pr_debug (print-audit lane, 2026-11):
                                                        // an untraced ebreak left in text hits this on EVERY
                                                        // step attempt — a per-trap pr_info through the
                                                        // polled UART, the EXITRP class (e2717687). The hunt
                                                        // is closed; release builds pay nothing.
                                                        crate::pr_debug!(
                                                            "SSBRK: comm={} epc={:#x} (untraced single-step ebreak)",
                                                            comm_now,
                                                            regs.epc
                                                        );
                                                        (*task).pending.add(crate::signal::Signal::SIGTRAP as i32);
                                                    }
                                                    return;
                                                }
                                                // A real user ebreak under a tracer stops for the tracer
                                                // instead of dying (gdb inserts/relies on breakpoints).
                                                if (*task).tracer_pid() != 0 {
                                                    // Advance past the 4-byte ebreak so CONT resumes at the
                                                    // following instruction (Linux do_trap_break).
                                                    regs.epc += 4;
                                                    // SAFETY: task is the current task in its trap path.

                                                        crate::process::ptrace::ptrace_stop(
                                                            task,
                                                            crate::signal::Signal::SIGTRAP as i32,
                                                            crate::signal::SigInfo::new(
                                                                crate::signal::Signal::SIGTRAP as i32,
                                                                crate::signal::si_code::SI_KERNEL,
                                                                (*task).pid(), 0),
                                                        );

                                                    return;
                                                }
                                            }
                                            // HOTPATH-PRINT: fault-path — see context above
                                            crate::pr_info!(
                                                "UBRK: comm={} epc={:#x} ra={:#x}",
                                                comm_now,
                                                regs.epc,
                                                regs.ra
                                            );
                                            crate::process::exit::do_exit(-(crate::signal::Signal::SIGTRAP as i32));
                                            // Do NOT advance epc — the task is now ZOMBIE and will not resume
                                        } else {
                                            // Kernel-mode ebreak is a kernel bug, not something to step over.
                                            panic!(
                                                "trap: ebreak in kernel mode at epc={:#x}",
                                                regs.epc
                                            );
                                        }

}
/// Handle page fault
///
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-24): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) 
fn sigsegv_has_handler() -> bool {


                                 crate::sched::current()
                                     .and_then(|t|  (*t).signal.as_ref().map(|s| s.get_action(11)) )
                                     .map(|a| a.is_some_and(|a| a.has_handler()))
                                     .unwrap_or(false)

}

/// True when the CURRENT task blocks SIGSEGV (bit 10) — the thread is
/// inside a SIGSEGV handler (delivery blocks the signal unless
/// SA_NODEFER) or blocked it via sigprocmask. Linux force_sig_info
/// semantics: a FORCED signal that is blocked is reset to SIG_DFL — a
/// synchronous fault on a blocked SIGSEGV kills the task. Unblocking
/// alone re-enters the handler and faults forever (see the x86_64 twin:
/// OH's DFX_SignalHandler faulting inside itself starved the machine).
pub(crate) fn sigsegv_blocked_current() -> bool {
    crate::sched::current()
        .map(|t| (*t).sigmask & (1u64 << 10) != 0)
        .unwrap_or(false)
}
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-23): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) 
fn sigbus_has_handler() -> bool {


                                crate::sched::current()
                                    .and_then(|t|  (*t).signal.as_ref().map(|s| s.get_action(7)) )
                                    .map(|a| a.is_some_and(|a| a.has_handler()))
                                    .unwrap_or(false)

}


/// R73: one-shot latch for the KERNPANIC raw dump — mirrors the x86_64
/// twin. A fault DURING the dump (stack walk past the mapping, tp deref,
/// frame walk on a wild fp) re-enters the handler; without the latch the
/// print/read recursion never terminates (the riscv64 unit-test
/// double-fault family). Second entry prints one marker and halts.
pub(crate) static KP_DUMP_ENTERED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
/// Collected into `arch::arch_floor` (INV-ARCH-TRAP-21): the unsafe floor moved there
/// verbatim; this safe wrapper keeps the original call path.
pub(crate) fn handle_page_fault(regs: &mut PtRegs, access_type: u32) {
    crate::arch::arch_floor::riscv64::trap::handle_page_fault(regs, access_type)
}

/// The oops register block — the FAULTING frame in the reference's
/// `__show_regs` layout (arch/riscv/kernel/traps.c, Linux 5.15): rows
/// of three `%s: %016lx` cells, epc/ra/sp first, then the GPRs in
/// x-order, then the status/badaddr/cause line. Rendered from the trap
/// frame, not `save_regs()` — the SR-PROBE lesson: a save_regs capture
/// shows the printer's frame, not the faulting one.
pub(crate) fn oops_show_regs(w: &mut crate::debug::backtrace::ConsoleWriter, regs: &PtRegs) {
    use core::fmt::Write;
    let cells: [(&str, u64); 32] = [
        ("epc", regs.epc),
        ("ra", regs.ra),
        ("sp", regs.sp),
        ("gp", regs.gp),
        ("tp", regs.tp),
        ("t0", regs.t0),
        ("t1", regs.t1),
        ("t2", regs.t2),
        ("s0", regs.s0),
        ("s1", regs.s1),
        ("a0", regs.a0),
        ("a1", regs.a1),
        ("a2", regs.a2),
        ("a3", regs.a3),
        ("a4", regs.a4),
        ("a5", regs.a5),
        ("a6", regs.a6),
        ("a7", regs.a7),
        ("s2", regs.s2),
        ("s3", regs.s3),
        ("s4", regs.s4),
        ("s5", regs.s5),
        ("s6", regs.s6),
        ("s7", regs.s7),
        ("s8", regs.s8),
        ("s9", regs.s9),
        ("s10", regs.s10),
        ("s11", regs.s11),
        ("t3", regs.t3),
        ("t4", regs.t4),
        ("t5", regs.t5),
        ("t6", regs.t6),
    ];
    for row in cells.chunks(3) {
        for (i, (name, val)) in row.iter().enumerate() {
            if i > 0 {
                let _ = w.write_str(" ");
            }
            // The reference's first row is the literal "epc : " (a
            // hardcoded format string, 4 columns before the colon);
            // the other names justify in 3 (" ra : ", "s10: ").
            if *name == "epc" {
                let _ = write!(w, "epc : {:016x}", val);
            } else {
                let _ = write!(w, "{:<3}: {:016x}", name, val);
            }
        }
        let _ = w.write_str("\n");
    }
    let _ = write!(
        w,
        "status: {:016x} badaddr: {:016x} cause: {:016x}\n",
        regs.status, regs.badaddr, regs.cause
    );
}

/// Handle unknown exception
pub(crate) fn handle_unknown_exception(regs: &mut PtRegs, cause: Cause) {
    crate::pr_debug!("trap: unknown exception {:?}, epc={:#x}, badaddr={:#x}",
        cause, regs.epc, regs.badaddr);
    // HOTPATH-PRINT: fatal-path — once per unknown-cause trap
    crate::pr_err!("trap: Unknown exception: {:?}, epc={:#x}, badaddr={:#x}",
        cause, regs.epc, regs.badaddr);

    if regs.user_mode() {
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGKILL as i32));
        // Do NOT advance epc — the task is now ZOMBIE and will not resume
    } else {
        // Kernel-mode unknown exceptions (misaligned access, access
        // fault, ...) indicate a kernel bug; skipping the instruction
        // corrupts state. The Oops path prints the report and applies
        // the policy (panic in interrupt context / oops=panic / idle;
        // task death otherwise — Tranche 5).
        crate::debug::oops::oops(
            crate::debug::oops::Kind::UnknownException { cause: cause as u64 },
            &|w: &mut crate::debug::backtrace::ConsoleWriter| {
                oops_show_regs(w, regs);
            },
        );
    }
}

// ============================================================================
// Compatibility: Keep old TrapFrame type alias
// ============================================================================

/// Old TrapFrame type alias (compatibility)
pub type TrapFrame = PtRegs;

/// Old ExceptionCause type alias (compatibility)
pub type ExceptionCause = Cause;

/// Get current TrapFrame pointer (compatibility)
#[deprecated(note = "Use current_pt_regs instead")]
pub fn current_trap_frame() -> *const TrapFrame {
    current_pt_regs()
}







// ============================================================================
// ret_from_fork functions
// ============================================================================

// Collected-unsafe re-exports: the unsafe floor for these names lives in
// kernel/src/arch/unsafe.rs (INV-ARCH-*); these keep the original paths.
// INV-ARCH note: ret_from_fork_kernel moved to arch::arch_floor::riscv64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::trap::ret_from_fork_kernel;
// INV-ARCH note: ret_from_fork_user moved to arch::arch_floor::riscv64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::trap::ret_from_fork_user;
// INV-ARCH note: debug_trap_exit moved to arch::arch_floor::riscv64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::trap::debug_trap_exit;
// INV-ARCH note: debug_trap_entry moved to arch::arch_floor::riscv64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::trap::debug_trap_entry;
// INV-ARCH note: debug_sepc_verify moved to arch::arch_floor::riscv64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::trap::debug_sepc_verify;
// INV-ARCH note: debug_after_schedule moved to arch::arch_floor::riscv64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::trap::debug_after_schedule;
// INV-ARCH note: debug_before_schedule moved to arch::arch_floor::riscv64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::trap::debug_before_schedule;
// INV-ARCH note: debug_print_clone_regs moved to arch::arch_floor::riscv64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::trap::debug_print_clone_regs;
// INV-ARCH note: riscv_bad_user_frame moved to arch::arch_floor::riscv64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::trap::riscv_bad_user_frame;
// INV-ARCH note: trap_handler moved to arch::arch_floor::riscv64::trap (naked
// trampoline — no wrapper; this re-export keeps the original
// path and fn identity).
pub(crate) use crate::arch::arch_floor::riscv64::trap::trap_handler;
