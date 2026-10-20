//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V exception handling
//!
//! Handle various exceptions and interrupts

use core::arch::asm;
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
core::arch::global_asm!(
    include_str!("trap.S"),
    task_ti_preempt = const crate::process::task::task_offsets::TI_PREEMPT_COUNT,
    task_ti_kernel_sp = const crate::process::task::task_offsets::TI_KERNEL_SP,
    task_ti_user_sp = const crate::process::task::task_offsets::TI_USER_SP,
    task_ti_cpu = const crate::process::task::task_offsets::TI_CPU,
    task_ti_a0 = const crate::process::task::task_offsets::TI_A0,
    // R85 ksp gates: stack-top repair reads the task's own stack record.
    task_kstack_bottom = const crate::process::task::task_offsets::TASK_KERNEL_STACK_BOTTOM,
    kernel_stack_size = const crate::config::KERNEL_STACK_SIZE,
);

/// Get pt_regs for current task
///
/// pt_regs is always at (kernel_stack_top - sizeof(pt_regs))
/// This is more reliable than using ti_kernel_sp, which can get stale
/// when a task is preempted in kernel mode.
pub fn current_task_pt_regs() -> Option<&'static mut PtRegs> {
    use crate::sched::current;
    use crate::process::task::Task;

    // SAFETY: stack_top is the kernel stack base allocated by the task; pt_regs lives
    // at the fixed offset (stack_top - sizeof(PtRegs)) established by trap_entry in trap.S.
    unsafe {
        let task = current()?;

        // Get kernel stack top
        let stack_top = (*task).get_kernel_stack()?;
        let stack_top_addr = stack_top as u64;

        // pt_regs is at stack_top - sizeof(PtRegs)
        // pt_regs at (kernel_stack_top - sizeof(pt_regs))
        let pt_regs_ptr = (stack_top_addr - PT_REGS_SIZE as u64) as *mut PtRegs;

        Some(&mut *pt_regs_ptr)
    }
}

// Re-export PtRegs and related constants
pub use super::pt_regs::{PtRegs, Cause, PT_REGS_SIZE};
pub use super::pt_regs::{SR_SPP, SR_PIE, SR_SIE, SR_SUM};

/// Current CPU's PtRegs pointer (used for fork) — per-CPU to support SMP
static CURRENT_PT_REGS: [core::sync::atomic::AtomicU64; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::config::MAX_CPUS];

/// Get current PtRegs pointer
/// Used for fork to copy parent's trap state
pub fn current_pt_regs() -> *const PtRegs {
    let cpu = crate::arch::cpu_id() as usize;
    CURRENT_PT_REGS[cpu].load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs
}

/// Wakewatch: RIP-equivalent (epc) of another CPU's most recent trap
/// frame, for the TIMER-SOFTIRQ-STALL report. Diagnostic read of a slot
/// the target CPU owns; races live execution (worst case a torn-but-
/// bounded value from a frame being built). 0 when no frame is live.
pub fn pt_regs_rip_for_cpu(cpu: usize) -> u64 {
    if cpu >= crate::config::MAX_CPUS {
        return 0;
    }
    let p = CURRENT_PT_REGS[cpu].load(core::sync::atomic::Ordering::Relaxed) as *const PtRegs;
    if p.is_null() || !crate::sched::sched::kernel_ptr_canonical(p as *const u8) {
        return 0;
    }
    // SAFETY: the slot holds either a live trap frame on that CPU's
    // kernel stack or a stale-but-mapped one; one volatile-ish scalar
    // read of the epc field cannot fault (the frame page stays mapped
    // while any task on that stack exists, and the diagnostic consumer
    // tolerates any value).
    unsafe { core::ptr::addr_of!((*p).epc).read() }
}

/// Initialize trap handling
pub fn init() {
    // SAFETY: stvec and sscratch are supervisor CSRs; writing them at init time is safe
    // and required for trap handling. trap_entry is a valid function pointer defined in trap.S.
    unsafe {
        // Set stvec to point to trap_entry
        extern "C" {
            fn trap_entry();
        }

        let stvec_value = trap_entry as *const () as u64;
        asm!(
            "csrw stvec, {}",
            in(reg) stvec_value,
            options(nostack)
        );

        // Initialize sscratch to 0 for kernel mode
        // When kernel is running, sscratch should be 0 so that on trap:
        //   csrrw tp, sscratch, tp  ->  tp becomes 0
        //   beqz tp, .Lfrom_kernel  ->  taken, correct path
        // When switching to user mode, sscratch will be set to current task
        asm!(
            "csrw sscratch, zero",
            options(nomem, nostack)
        )
    }
}

pub fn init_syscall() {
    // RISC-V uses ecall instruction, dispatched in exception handler
}

pub fn enable_timer_interrupt() {
    // Step 1: Enable STIE (bit 5 in sie) using atomic bit set
    // SAFETY: csrs is an atomic read-modify-write on the sie CSR; STIE bit enable is safe.
    unsafe {
        let stie: u64 = 0x20;
        asm!(
            "csrs sie, {}",
            in(reg) stie,
            options(nomem, nostack)
        );
    }

    // Step 2: Set the timer trigger
    crate::drivers::timer::set_next_trigger();

    // Step 3: Enable global interrupts (sstatus.SIE = 1) if not already enabled
    // SAFETY: csrsi atomically sets the SIE bit in sstatus; enabling interrupts is safe.
    unsafe {
        asm!(
            "csrsi sstatus, 2",
            options(nomem, nostack)
        );
    }
}

pub fn disable_timer_interrupt() {
    // SAFETY: sie::clear_stimer clears the STIE bit in the sie CSR, which is safe.
    unsafe {
        sie::clear_stimer();
    }
}

pub fn enable_external_interrupt() {
    // SUM convergence (review SEC): the kernel now runs with sstatus.SUM=0
    // at all times — user memory is only reachable through the uaccess
    // exception-table paths, which bracket their copy loops with explicit
    // SUM set/clear. Enabling SUM here (the old behavior) silently let
    // syscall code dereference raw user pointers: a bad pointer then
    // skipped the exception table and hit the KernelPanic path instead of
    // returning EFAULT.
    // SAFETY: csrs atomically sets the SEIE bit in the sie CSR; enabling
    // external interrupts is safe.
    unsafe {
        // Enable external interrupt (SEIE bit) - use csrs to preserve other bits
        let seie: u64 = 512;  // SEIE bit (2^9)
        asm!(
            "csrs sie, {}",
            in(reg) seie,
            options(nomem, nostack)
        );
    }
}

/// Trap handler
///
/// Called by trap.S with PtRegs pointer
#[no_mangle]
pub extern "C" fn trap_handler(regs: *mut PtRegs, cpu_id: usize) {
    // SAFETY: regs points to a valid PtRegs on the kernel stack, allocated by trap_entry
    // in trap.S. The pointer remains valid for the duration of this handler.
    unsafe {
        // Save and swap PtRegs pointer (used for fork/exec).
        // When interrupts are enabled during syscalls (like Linux), a timer
        // interrupt can nest inside a syscall.  Without save/restore, the
        // inner trap_handler would overwrite CURRENT_PT_REGS with its own
        // pt_regs and then clear it to 0 on return, causing the outer
        // syscall's fork/exec to see current_pt_regs() == NULL.
        let prev_pt_regs = CURRENT_PT_REGS[cpu_id].load(core::sync::atomic::Ordering::Relaxed);
        CURRENT_PT_REGS[cpu_id].store(regs as u64, core::sync::atomic::Ordering::Relaxed);

        let regs_ref = &mut *regs;
        let cause = Cause::from_cause(regs_ref.cause);

        // Skip WFI instruction when interrupted in kernel mode.
        // On RISC-V, when WFI is interrupted, sepc points to WFI itself.
        // After sret, the CPU would re-execute WFI, causing the idle loop
        // to never advance past WFI. Advancing epc by 4 skips WFI.
        //
        // Safety: the dereference below is a raw kernel-mode read, so the
        // epc must be provably inside the kernel text mapping before we
        // touch it. Restricting to [_stext, _etext] (instead of merely
        // "canonical upper half") excludes every other kernel VA range —
        // during early SMP boot sepc may hold an unmapped physical address,
        // and a fault on this read is a nested-trap KernelPanic (review
        // RISK). WFI can only legally execute in kernel text anyway.
        if !regs_ref.user_mode() && regs_ref.epc % 4 == 0 {
            extern "C" {
                static _stext: u8;
                static _etext: u8;
            }
            // SAFETY: linker symbols _stext/_etext bound the kernel text
            // section; only their addresses are read.
            let (lo, hi) = unsafe {
                (&raw const _stext as usize as u64, &raw const _etext as usize as u64)
            };
            if regs_ref.epc >= lo && regs_ref.epc < hi {
                const WFI_INSN: u32 = 0x10500073;
                let insn = core::ptr::read_volatile(regs_ref.epc as *const u32);
                if insn == WFI_INSN {
                    regs_ref.epc += 4;
                }
            }
        }

        // Trap cause debug (minimal, for development)

        crate::pr_debug!("trap: cause={:?}, epc={:#x}, sp={:#x}, tp={:#x}, mode={}",
            cause, regs_ref.epc, regs_ref.sp, regs_ref.tp,
            if regs_ref.user_mode() { "user" } else { "kernel" });

        match cause {
            // Timer interrupt
            Cause::SupervisorTimer => {
                // FORENSIC (r74 irq-bracket leak hunt): the irq_enter/
                // irq_exit pair books HARDIRQ_OFFSET on the task tp names.
                // If tp changes between them (a context switch inside the
                // handler), the decrement lands on the wrong task and the
                // old task keeps a leaked +65536 — its next user-mode page
                // fault then classifies as in_interrupt() and dies through
                // the no_context → KERNPANIC path (r72-fire6 family).
                let tp_entry = crate::arch::cpu::get_thread_id() as usize;
                crate::interrupt::preempt::irq_enter();
                handle_timer_interrupt(regs_ref, cpu_id);
                crate::interrupt::preempt::irq_exit();
                let tp_exit = crate::arch::cpu::get_thread_id() as usize;
                if tp_entry != tp_exit {
                    let a = tp_entry as *mut crate::process::task::Task;
                    let b = tp_exit as *mut crate::process::task::Task;
                    let (pa, pb) = unsafe {
                        ((*a).preempt_count(), (*b).preempt_count())
                    };
                    use core::fmt::Write;
                    let mut w = crate::dfx::backtrace::ConsoleWriter::new();
                    let _ = write!(w,
                        "IRQ-BRACKET-LEAK timer cpu={} entry_tp={:#x} (pid={} preempt={:#x}) exit_tp={:#x} (pid={} preempt={:#x})\n",
                        cpu_id, tp_entry,
                        unsafe { (*a).pid() }, pa,
                        tp_exit, unsafe { (*b).pid() }, pb);
                }
            }

            // Software interrupt (IPI)
            Cause::SupervisorSoft => {
                let tp_entry = crate::arch::cpu::get_thread_id() as usize;
                crate::interrupt::preempt::irq_enter();
                handle_software_interrupt(regs_ref, cpu_id);
                crate::interrupt::preempt::irq_exit();
                let tp_exit = crate::arch::cpu::get_thread_id() as usize;
                if tp_entry != tp_exit {
                    let a = tp_entry as *mut crate::process::task::Task;
                    let b = tp_exit as *mut crate::process::task::Task;
                    let (pa, pb) = unsafe {
                        ((*a).preempt_count(), (*b).preempt_count())
                    };
                    use core::fmt::Write;
                    let mut w = crate::dfx::backtrace::ConsoleWriter::new();
                    let _ = write!(w,
                        "IRQ-BRACKET-LEAK soft cpu={} entry_tp={:#x} (pid={} preempt={:#x}) exit_tp={:#x} (pid={} preempt={:#x})\n",
                        cpu_id, tp_entry,
                        unsafe { (*a).pid() }, pa,
                        tp_exit, unsafe { (*b).pid() }, pb);
                }
            }

            // External interrupt
            Cause::SupervisorExternal => {
                let tp_entry = crate::arch::cpu::get_thread_id() as usize;
                crate::interrupt::preempt::irq_enter();
                handle_external_interrupt(regs_ref, cpu_id);
                crate::interrupt::preempt::irq_exit();
                let tp_exit = crate::arch::cpu::get_thread_id() as usize;
                if tp_entry != tp_exit {
                    let a = tp_entry as *mut crate::process::task::Task;
                    let b = tp_exit as *mut crate::process::task::Task;
                    let (pa, pb) = unsafe {
                        ((*a).preempt_count(), (*b).preempt_count())
                    };
                    use core::fmt::Write;
                    let mut w = crate::dfx::backtrace::ConsoleWriter::new();
                    let _ = write!(w,
                        "IRQ-BRACKET-LEAK ext cpu={} entry_tp={:#x} (pid={} preempt={:#x}) exit_tp={:#x} (pid={} preempt={:#x})\n",
                        cpu_id, tp_entry,
                        unsafe { (*a).pid() }, pa,
                        tp_exit, unsafe { (*b).pid() }, pb);
                }
            }

            // User mode system call
            Cause::EcallUser => {
                handle_syscall(regs_ref);
            }

            // Illegal instruction
            Cause::IllegalInstruction => {
                if regs_ref.user_mode() {
                    handle_illegal_instruction(regs_ref);
                } else {
                    // SR-PROBE (round 15, temporary): dump the REAL trap frame
                    // before dying — the panic handler's register dump is a
                    // save_regs() artifact (memset remnants), not the faulting
                    // context. ra identifies the indirect branch that jumped
                    // to the linear-map page.
                    {
                        use core::fmt::Write;
                        let mut w = crate::dfx::backtrace::ConsoleWriter::new();
                        let _ = w.write_str("SR-REGS:\n");
                        let _ = write!(w, "  epc={:#x} ra={:#x} sp={:#x}\n",
                            regs_ref.epc, regs_ref.ra, regs_ref.sp);
                        let _ = write!(w, "  tp={:#x} s0={:#x} s1={:#x} gp={:#x}\n",
                            regs_ref.tp, regs_ref.s0, regs_ref.s1, regs_ref.gp);
                        let _ = write!(w, "  a0={:#x} a1={:#x} a2={:#x} a3={:#x}\n",
                            regs_ref.a0, regs_ref.a1, regs_ref.a2, regs_ref.a3);
                        let _ = write!(w, "  a4={:#x} a5={:#x} a6={:#x} a7={:#x}\n",
                            regs_ref.a4, regs_ref.a5, regs_ref.a6, regs_ref.a7);
                        let _ = write!(w, "  t0={:#x} t1={:#x} t2={:#x} t3={:#x}\n",
                            regs_ref.t0, regs_ref.t1, regs_ref.t2, regs_ref.t3);
                        let _ = w.write_str("SR-STACK (ra chain via s0):\n");
                        let mut fp = regs_ref.s0 as usize;
                        for _ in 0..12 {
                            if fp < 0xffffffd600000000 || fp > 0xffffffd700000000 {
                                break;
                            }
                            let next = core::ptr::read_volatile(fp as *const usize);
                            let ra = core::ptr::read_volatile((fp + 8) as *const usize);
                            let _ = write!(w, "  fp={:#x} ra={:#x}\n", fp, ra);
                            if ra < 0xffffffff80000000 || next <= fp {
                                break;
                            }
                            fp = next;
                        }
                    }
                    // A kernel-mode illegal instruction is always a kernel bug
                    // (e.g., an M-mode CSR executed in S-mode). Skipping it
                    // corrupts execution; die loudly instead.
                    panic!(
                        "trap: illegal instruction in kernel mode at epc={:#x} badaddr={:#x}",
                        regs_ref.epc, regs_ref.badaddr
                    );
                }
            }

            // Breakpoint
            Cause::Breakpoint => {
                handle_breakpoint(regs_ref);
            }

            // Instruction page fault
            Cause::InstructionPageFault => {
                handle_page_fault(regs_ref, crate::arch::mm::FaultFlags::EXEC);
            }

            // Load page fault
            Cause::LoadPageFault => {
                handle_page_fault(regs_ref, crate::arch::mm::FaultFlags::READ);
            }

            // Store page fault
            Cause::StoreAmoPageFault => {
                handle_page_fault(regs_ref, crate::arch::mm::FaultFlags::WRITE);
            }

            // Other exceptions
            _ => {
                handle_unknown_exception(regs_ref, cause);
            }
        }

        // Restore previous PtRegs pointer (handles nested interrupt case)
        CURRENT_PT_REGS[cpu_id].store(prev_pt_regs, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Handle timer interrupt
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

    // 0. CPU-time accounting (Linux account_process_tick): charge this
    // tick to the interrupted task as user or system time depending on
    // the privilege mode the timer hit. Only user tasks are charged
    // (kthreads are born without an address space).
    if let Some(task) = crate::sched::current() {
        // SAFETY: current() returned a live task pointer; we only touch
        // per-task atomic counters, no teardown race on these fields.
        unsafe {
            if (*task).address_space().is_some() {
                use core::sync::atomic::Ordering::Relaxed;
                if regs.user_mode() {
                    (*task).utime_ticks.fetch_add(1, Relaxed);
                } else {
                    (*task).stime_ticks.fetch_add(1, Relaxed);
                }
            }
        }
    }

    // 1. Update jiffies
    crate::drivers::timer::timer_interrupt_handler();

    // 2. Scheduler tick
    crate::sched::scheduler_tick();

    // 3. Check for soft lockups
    crate::dfx::softlockup::check();

    // 4. Reschedule if needed
    if crate::sched::need_resched() && crate::interrupt::preempt::preemptible() {
        crate::sched::schedule();
    }
}

/// Handle software interrupt (IPI)
fn handle_software_interrupt(_regs: &mut PtRegs, cpu: usize) {
    // Increment software interrupt counter for /proc/interrupts
    interrupts::soft_inc(cpu);

    // Clear software interrupt
    // SAFETY: csrc atomically clears bit 1 (SSIP) in the sip CSR; safe at interrupt handler level.
    unsafe {
        core::arch::asm!("csrc sip, 0x2", options(nomem, nostack));
    }

    // Handle IPI
    crate::arch::ipi::handle_software_ipi(cpu);
}

/// Handle external interrupt
///
/// Claims the highest-priority pending IRQ from PLIC and dispatches
/// through the IRQ framework. EOI (PLIC complete) is done by the
/// flow handler via irq_chip.irq_eoi. Fallback complete only for
/// spurious/unmapped IRQs that bypass the flow handler.
fn handle_external_interrupt(_regs: &mut PtRegs, cpu: usize) {

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
fn handle_syscall(regs: &mut PtRegs) {
    // Fix sscratch protocol: set sscratch = 0 to mark "in kernel mode" before
    // re-enabling interrupts.  During a syscall, sscratch holds the user's TLS
    // pointer (set by the ecall entry swap).  If a timer interrupt fires with
    // sscratch != 0, the trap entry incorrectly routes through .Lfrom_user
    // (user-mode return) instead of .Lfrom_kernel, which corrupts the return
    // path and can sret to a kernel address while in user mode.
    //
    // Setting sscratch = 0 here is safe because:
    // - .Lrestore_and_exit restores sscratch = tp before sret to user space
    // - .Lrestore_kernel_and_exit already expects sscratch = 0
    unsafe { core::arch::asm!("csrw sscratch, zero", options(nomem, nostack)); }

    // Re-enable interrupts before entering the syscall handler.
    //
    // The ecall instruction clears SIE (saves to SPIE).  Running the entire
    // syscall with SIE=0 means timer interrupts cannot fire, which prevents
    // scheduler ticks, softlockup detection, and preemption — any syscall
    // that takes > SOFTLOCKUP_THRESHOLD_SECS triggers a false positive.
    //
    // Linux does the same via syscall_enter_from_user_mode() → local_irq_enable().
    // The saved sstatus in pt_regs (SIE=0) is restored unmodified by the
    // trap-return path, so user-mode return semantics are preserved.
    crate::arch::cpu::enable_irq();

    let orig_epc = regs.epc;
    let syscall_num = regs.a7;  // syscall number is in a7, not orig_a0!

    // Default return value is -ENOSYS
    regs.a0 = crate::errno::constants::ENOSYS as u64;

    // Skip ecall instruction
    // RISC-V has both 32-bit ecall and 16-bit c.ecall instructions
    // Check if the instruction is compressed (lowest 2 bits != 11)
    let instr_size = if orig_epc % 4 == 0 {
        4 // 32-bit instruction
    } else {
        // Read the instruction through the exception-table copy path — a
        // raw read_volatile of the USER epc faults the kernel with SUM=0
        // (review ARCH: epc must only be touched via uaccess).
        let mut insn_buf = [0u8; 2];
        let uncopied = unsafe {
            super::uaccess::copy_from_user(
                insn_buf.as_mut_ptr(),
                orig_epc as *const u8,
                2,
            )
        };
        if uncopied > 0 {
            // Unmapped epc: the retried instruction will fault in user mode
            // and be reported as SIGSEGV there. Assume 4 bytes for now.
            4
        } else {
            let instr16 = u16::from_ne_bytes(insn_buf);
            if (instr16 & 0x3) != 0x3 {
                2 // 16-bit compressed instruction
            } else {
                4 // 32-bit instruction
            }
        }
    };
    regs.epc = orig_epc + instr_size;

    // U1c seccomp gate: check the syscall against the current task's
    // seccomp policy BEFORE any dispatch. STRICT mode allows only
    // read/write/exit/exit_group/rt_sigreturn (anything else queues SIGKILL
    // — the task terminates on return to user). FILTER mode runs the
    // classic-BPF program; ERRNO actions replace the return value, KILL
    // actions queue SIGKILL. Returns Some(rv) when the syscall is blocked
    // (rv is reported to user), None when dispatch may proceed.
    if let Some(rv) = crate::syscall::process::seccomp_syscall_gate(syscall_num, regs) {
        regs.a0 = rv as u64;
        return;
    }

    // Call syscall handler
    crate::syscall::syscall_handler(regs);
}

/// Handle illegal instruction
///
/// Check for FPU first-use before terminating.
/// When sstatus.FS = OFF, any FP instruction causes IllegalInstruction.
/// We detect this case and enable FPU lazily (set FS = INITIAL),
/// zero the FP registers (initial-state semantics), then retry the
/// instruction.
fn handle_illegal_instruction(regs: &mut PtRegs) {
    let epc = regs.epc;

    // Read the instruction through the exception-table copy path — for
    // user-mode faults epc is a USER address, which a raw read_unaligned
    // would fault on with SUM=0 (review ARCH).
    let mut insn_buf = [0u8; 4];
    let uncopied16 = unsafe {
        super::uaccess::copy_from_user(insn_buf.as_mut_ptr(), epc as *const u8, 2)
    };
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
                let _ = unsafe {
                    super::uaccess::copy_from_user(
                        insn_buf.as_mut_ptr(),
                        epc as *const u8,
                        4,
                    )
                };
                u32::from_ne_bytes(insn_buf)
            } else {
                instr16 as u32
            };
            let opcode = instr32 & 0x7F;
            opcode == 0x07 || opcode == 0x27 ||   // FLW/FLD, FSW/FSD
            opcode == 0x05 || opcode == 0x09 ||   // FMADD, FMSUB
            opcode == 0x0D || opcode == 0x0E ||   // FNMSUB, FNMADD
            opcode == 0x53                         // FP ops (FADD, FSUB, FMUL, FDIV, etc.)
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
            unsafe { super::thread::fpu_init(); }
            // Enable FPU in the SAVED status too, so sret does not undo it.
            regs.status = (regs.status & !SR_FS) | SR_FS_INITIAL;
            return;
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
            .map(|t| unsafe { ((*t).pid(), (*t).comm()) })
            .unwrap_or((0, &EMPTY_COMM));
        let comm_len = comm.iter().position(|&b| b == 0).unwrap_or(comm.len());
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
        sigill_mapping_forensics(pid as usize, epc as usize);
    }

    if regs.user_mode() {
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGILL as i32));
    }

    // Do NOT advance epc — the task is now ZOMBIE and will not resume
}

/// SIGILL-FORENSIC (gnome-sigill): console dump of the dying task's VMA
/// table + the faulting page's PTE/content. One-shot per death; only runs
/// on the (rare) user-mode illegal-instruction path, so the serial cost is
/// acceptable even for large processes.
fn sigill_mapping_forensics(pid: usize, epc: usize) {
    use crate::mm::vma::VmaType;

    let task = match crate::sched::current() {
        Some(t) => t,
        None => return,
    };
    // SAFETY: current task pointer is valid for the trap context.
    let mm = unsafe { (*task).address_space_arc() };
    let mm = match mm {
        Some(m) => m,
        None => {
            crate::pr_info!("SIGILL-FORENSIC pid={} no-mm", pid);
            return;
        }
    };

    let page = epc & !0xFFFusize;
    // (start, end, "rwx", "anon|file|dev|shm", contains_epc, file-start or 0)
    let mut rows: alloc::vec::Vec<(usize, usize, alloc::string::String, &'static str, bool, usize)> =
        alloc::vec::Vec::new();
    {
        let vma_mgr = mm.vma_read();
        for vma in vma_mgr.iter() {
            let f = vma.flags();
            let perms = [
                if f.is_readable() { "r" } else { "-" },
                if f.is_writable() { "w" } else { "-" },
                if f.is_executable() { "x" } else { "-" },
            ]
            .concat();
            let ty = match vma.vma_type() {
                VmaType::Anonymous => "anon",
                VmaType::FileBacked => "file",
                VmaType::Device => "dev",
                VmaType::SharedMemory => "shm",
            };
            let contains = epc >= vma.start().as_usize() && epc < vma.end().as_usize();
            let file_start = if vma.vma_type() == VmaType::FileBacked {
                vma.start().as_usize()
            } else {
                0
            };
            rows.push((
                vma.start().as_usize(),
                vma.end().as_usize(),
                perms,
                ty,
                contains,
                file_start,
            ));
        }
    }
    crate::pr_info!(
        "SIGILL-FORENSIC pid={} epc={:#x} page={:#x} vmas={}",
        pid, epc, page, rows.len()
    );
    for (s, e, perms, ty, contains, file_start) in &rows {
        let path = if *file_start != 0 {
            mm.get_vma_file(*file_start)
                .map(|fl| fl.path())
                .unwrap_or_default()
        } else {
            alloc::string::String::new()
        };
        crate::pr_info!(
            "  VMA {:#x}-{:#x} {} {} {}{}",
            s, e, perms, ty,
            if *contains { "=> EPC " } else { "" },
            path
        );
    }
    if !rows.iter().any(|r| r.4) {
        crate::pr_info!("SIGILL-FORENSIC: epc NOT COVERED by any VMA");
    }

    // PTE + page content via a direct page-table walk (independent of the
    // fault path; same technique as coredump's dump_read).
    use crate::arch::mm::{phys_to_virt, PhysAddr};
    let root = mm.root_ppn();
    let page = page as u64;
    let mut pte_addr = (root << 12) + ((page >> 30) & 0x1FF) * 8;
    // SAFETY: walk reads valid page-table pages of the pinned mm.
    let mut pte = unsafe {
        core::ptr::read_volatile(phys_to_virt(PhysAddr::new(pte_addr)).0 as *const u64)
    };
    let mut level = 2u32;
    loop {
        let leaf = pte & 0xE != 0;
        if leaf || level == 0 {
            break;
        }
        let next = ((pte >> 10) & 0xFFF_FFFF_FFFF) << 12;
        let shift = 12 + 9 * (level - 1);
        pte_addr = next + ((page >> shift) & 0x1FF) * 8;
        // SAFETY: same as above.
        pte = unsafe {
            core::ptr::read_volatile(phys_to_virt(PhysAddr::new(pte_addr)).0 as *const u64)
        };
        if pte & 1 == 0 {
            break;
        }
        level -= 1;
    }
    crate::pr_info!("SIGILL-FORENSIC epc-page PTE @{:#x} = {:#x}", pte_addr, pte);
    // SIGILL-stamp report: is the epc frame one the file-fill path stamped
    // (a genuine text page), and for whom?
    if pte & 1 != 0 {
        let pfn = ((pte >> 10) & 0xFFF_FFFF_FFFF) as usize;
        match crate::mm::page_desc::text_stamp_get(pfn) {
            Some((owner_pid, owner_va)) => {
                crate::pr_info!(
                    "SIGILL-FORENSIC epc-frame STAMPED pfn={:#x} owner_pid={} owner_va={:#x}",
                    pfn, owner_pid, owner_va
                );
            }
            None => {
                crate::pr_info!(
                    "SIGILL-FORENSIC epc-frame NOT stamped pfn={:#x} (never a file-fill page, or stamp lost)",
                    pfn
                );
            }
        }
    }
    if pte & 1 == 0 {
        return;
    }
    // Leaf PTE at `level` (0=4K, 1=2M, 2=1G): physical base of the mapping
    // with the low PPN bits (large-page alignment) masked off.
    let leaf_shift = 12 + 9 * level; // bytes covered by this leaf
    let leaf_mask = (1u64 << leaf_shift) - 1;
    let phys_base = ((pte >> 10) << 12) & !leaf_mask;
    let kv_page =
        phys_to_virt(PhysAddr::new(phys_base + (page & leaf_mask))).0 as usize;
    // SAFETY: page is resident (PTE valid); dump raw words.
    unsafe {
        let p64 = kv_page as *const u64;
        let mut words = [0u64; 8];
        for (i, w) in words.iter_mut().enumerate() {
            *w = core::ptr::read_volatile(p64.add(i));
        }
        crate::pr_info!(
            "SIGILL-FORENSIC page[0..8]: {:016x} {:016x} {:016x} {:016x} {:016x} {:016x} {:016x} {:016x}",
            words[0], words[1], words[2], words[3], words[4], words[5], words[6], words[7]
        );
        let off_words = (((epc as u64) & 0xFFF) as usize / 8).saturating_sub(2);
        let mut w2 = [0u64; 5];
        for (i, w) in w2.iter_mut().enumerate() {
            *w = core::ptr::read_volatile(p64.add(off_words + i));
        }
        crate::pr_info!(
            "SIGILL-FORENSIC epc-16..+24: {:016x} {:016x} {:016x} {:016x} {:016x}",
            w2[0], w2[1], w2[2], w2[3], w2[4]
        );
    }
}

/// Handle breakpoint
fn handle_breakpoint(regs: &mut PtRegs) {
    if regs.user_mode() {
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
                    unsafe {
                        crate::process::ptrace::ptrace_stop(
                            task,
                            crate::signal::Signal::SIGTRAP as i32,
                            crate::signal::SigInfo::new(
                                crate::signal::Signal::SIGTRAP as i32,
                                crate::signal::si_code::SI_KERNEL,
                                (*task).pid(), 0),
                        );
                    }
                } else {
                    // Untraced single-step (raced DETACH): plain SIGTRAP.
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
                unsafe {
                    crate::process::ptrace::ptrace_stop(
                        task,
                        crate::signal::Signal::SIGTRAP as i32,
                        crate::signal::SigInfo::new(
                            crate::signal::Signal::SIGTRAP as i32,
                            crate::signal::si_code::SI_KERNEL,
                            (*task).pid(), 0),
                    );
                }
                return;
            }
        }
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
/// Delegate to mm::exception::do_page_fault for complete handling
/// True when the CURRENT task has a user handler installed for SIGSEGV
/// (used by the page-fault termination paths to decide between "pend the
/// signal and let do_signal deliver it to the handler" and "kill now").
fn sigsegv_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| unsafe { (*t).signal.as_ref().map(|s| s.get_action(11)) })
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
fn sigsegv_blocked_current() -> bool {
    crate::sched::current()
        .map(|t| (*t).sigmask & (1u64 << 10) != 0)
        .unwrap_or(false)
}

fn sigbus_has_handler() -> bool {
    crate::sched::current()
        .and_then(|t| unsafe { (*t).signal.as_ref().map(|s| s.get_action(7)) })
        .map(|a| a.is_some_and(|a| a.has_handler()))
        .unwrap_or(false)
}

/// R74 (garbage-frame gate): called from trap.S when a user-return frame
/// is provably unexecutable (epc==0 or a sub-0x1000 sp) — the zeroed/
/// stale-frame scribble family (r74d: epc=0 ra=0 sp=0xf). Restoring such
/// a frame jumps to 0 with a junk stack; kill the task instead. The print
/// is SBI-direct so a wedged printk cannot eat the evidence.
#[no_mangle]
pub extern "C" fn riscv_bad_user_frame(regs: *mut PtRegs) -> ! {
    let r = unsafe { &*regs };
    unsafe {
        let mut put = |b: u8| crate::console::putchar_no_lock(b);
        for &b in b"trap: BADFRAME pid=0x" { put(b); }
        let pid = crate::sched::get_current_pid() as u64;
        for sh in (0..32).step_by(4).rev() {
            let n = ((pid >> sh) & 0xF) as u8;
            put(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        }
        for &b in b" epc=0x" { put(b); }
        for sh in (0..64).step_by(4).rev() {
            let n = ((r.epc >> sh) & 0xF) as u8;
            put(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        }
        for &b in b" sp=0x" { put(b); }
        for sh in (0..64).step_by(4).rev() {
            let n = ((r.sp >> sh) & 0xF) as u8;
            put(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        }
        put(b'\n');
    }
    crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
    // do_exit schedules away and never returns; park defensively.
    loop {
        // SAFETY: wfi halts the hart until an interrupt; safe in a dead
        // task's final spin.
        unsafe { core::arch::asm!("wfi") };
    }
}

fn handle_page_fault(regs: &mut PtRegs, access_type: u32) {    use crate::arch::mm::exception::{do_page_fault, MmFaultResult};

    let fault_addr = regs.badaddr;

    crate::pr_debug!("trap: page fault addr={:#x}, epc={:#x}, type={}, mode={}",
        fault_addr, regs.epc, access_type,
        if regs.kernel_mode() { "kernel" } else { "user" });

    let result = do_page_fault(regs, access_type);

    match result {
        MmFaultResult::Handled | MmFaultResult::Fixed => {
            // Page handled, re-execute instruction
        }
        MmFaultResult::Segfault => {
            crate::pr_err!("pagefault: Segfault at {:#x}, epc={:#x}, ra={:#x}, sp={:#x}, t6={:#x}, pid={}, mode={}",
                fault_addr, regs.epc, regs.ra, regs.sp, regs.t6,
                crate::sched::get_current_pid(),
                if regs.kernel_mode() { "kernel" } else { "user" });
            if regs.user_mode() {
                // Linux semantics: route user-mode faults through normal
                // signal delivery (force_sig_fault) — an installed
                // SIGSEGV handler (debuggers, crash catchers, LTP mmap05)
                // must run. With a handler (and SIGSEGV not blocked —
                // i.e. not already inside that handler): pend the signal
                // and RETURN; do_signal delivers it on the way back to
                // userspace. Without one — or with the fault happening
                // inside the handler itself — the default action kills
                // here (and avoids a fault-retry loop).
                let pid = crate::process::current_pid();
                if sigsegv_has_handler() && !sigsegv_blocked_current() {
                    let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGSEGV as i32);
                    // Synchronous fault: deliver despite a blocked mask
                    // (Linux force_sig parity — see force_unblock_current).
                    crate::signal::force_unblock_current(crate::signal::Signal::SIGSEGV as i32);
                    return;
                }
                let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGSEGV as i32);
                crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
            }
        }
        MmFaultResult::PermissionDenied => {
            crate::pr_err!("pagefault: Permission denied at {:#x}", fault_addr);
            // Terminate user process via do_exit (properly notifies parent)
            if regs.user_mode() {
                // Same handler-first routing as Segfault: a PROT_NONE
                // access with an installed SIGSEGV handler must run it
                // (LTP mmap05); a fault inside the (blocked) handler
                // itself, or the default disposition, kills directly.
                let pid = crate::process::current_pid();
                if sigsegv_has_handler() && !sigsegv_blocked_current() {
                    let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGSEGV as i32);
                    // Synchronous fault: deliver despite a blocked mask.
                    crate::signal::force_unblock_current(crate::signal::Signal::SIGSEGV as i32);
                    return;
                }
                let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGSEGV as i32);
                crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
            }
        }
        MmFaultResult::BusError => {
            crate::pr_err!("pagefault: Bus error (past EOF) at {:#x}", fault_addr);
            if regs.user_mode() {
                // Handler-first routing like Segfault: LTP mmap13 installs
                // a SIGBUS handler and must catch it; the default action
                // kills with SIGBUS.
                let pid = crate::process::current_pid();
                let _ = crate::signal::send_signal(pid, crate::signal::Signal::SIGBUS as i32);
                if !sigbus_has_handler() {
                    crate::process::exit::do_exit(-(crate::signal::Signal::SIGBUS as i32));
                } else {
                    crate::signal::force_unblock_current(crate::signal::Signal::SIGBUS as i32);
                }
            }
        }
        MmFaultResult::OutOfMemory => {
            crate::pr_err!("pagefault: Out of memory at {:#x}", fault_addr);
            // FORENSIC (r74): an SBI-direct line printk cannot lose. The
            // r72-fire family's 570s gnome-shell-cal kill left NO trace in
            // any serial — either printk was wedged on a lock the stuck
            // allocator held, or the kill came from a different path
            // entirely. This line names pid/comm/addr so the next
            // occurrence is attributable; the full zone dump follows via
            // oom_forensic_dump (printk-based, best effort).
            unsafe {
                let mut put = |b: u8| crate::console::putchar_no_lock(b);
                for &b in b"trap: PF-OOM pid=0x" { put(b); }
                let pid = crate::sched::get_current_pid() as u64;
                for sh in (0..32).step_by(4).rev() {
                    let n = ((pid >> sh) & 0xF) as u8;
                    put(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                }
                for &b in b" addr=0x" { put(b); }
                for sh in (0..64).step_by(4).rev() {
                    let n = ((fault_addr >> sh) & 0xF) as u8;
                    put(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                }
                put(b'\n');
            }
            // FORENSIC (fake-OOM family): allocator state at the kill moment.
            crate::mm::page_alloc::oom_forensic_dump("pagefault OOM");
            // Terminate user process via do_exit (properly notifies parent)
            if regs.user_mode() {
                crate::process::exit::do_exit(-(crate::signal::Signal::SIGKILL as i32));
            }
        }
        MmFaultResult::KernelPanic => {
            // R9: print via SBI directly — printk can be wedged on a lock
            // this CPU holds (that is exactly how the earlier wedges lost
            // their diagnostics). Then halt.
            unsafe {
                let mut put = |b: u8| crate::console::putchar_no_lock(b);
                for &b in b"trap: KERNPANIC pfault badaddr=0x" { put(b); }
                for sh in (0..64).step_by(4).rev() {
                    let n = ((fault_addr >> sh) & 0xF) as u8;
                    put(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                }
                for &b in b" epc=0x" { put(b); }
                for sh in (0..64).step_by(4).rev() {
                    let n = ((regs.epc >> sh) & 0xF) as u8;
                    put(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                }
                for &b in b" ra=0x" { put(b); }
                for sh in (0..64).step_by(4).rev() {
                    let n = ((regs.ra >> sh) & 0xF) as u8;
                    put(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                }
                for &b in b" sp=0x" { put(b); }
                for sh in (0..64).step_by(4).rev() {
                    let n = ((regs.sp >> sh) & 0xF) as u8;
                    put(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                }
                put(b'\n');
                // WP1: kernel-stack backtrace. The one-line epc/ra print
                // cannot name the CALLER of the faulting function (ra is
                // inside it); the saved-ra chain up the stack can. Dump 24
                // words above sp — enough frames for wake-path panics
                // (observed: epc=Task::pid ra=Task::wake_up with a wild
                // 0xFFFFFFFF task pointer whose origin lived two frames up).
                // The kernel stack is always mapped; words that happen to
                // be data are simply ignored during symbolization.
                // R73: USER-origin faults skip the dump — their sp is a
                // USER address, and reading it with a direct kernel VA
                // faults again (nested panic on top of panic; the r72-fire3
                // KERNPANIC prints tripled exactly this way).
                if regs.kernel_mode() {
                    for &b in b"trap: stack:" { put(b); }
                    for i in 0..24usize {
                        let addr = regs.sp as usize + i * 8;
                        let v = unsafe { core::ptr::read_volatile(addr as *const usize) };
                        put(b' ');
                        for sh in (0..64).step_by(4).rev() {
                            let n = ((v >> sh) & 0xF) as u8;
                            put(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                        }
                        if i % 4 == 3 { put(b'\n'); }
                    }
                    if 24 % 4 != 0 { put(b'\n'); }
                } else {
                    for &b in b"trap: user-sp (not dumped)\n" { put(b); }
                }
                // FORENSIC (r74): user-mode faults can only reach
                // KernelPanic via no_context — current()==None,
                // address_space()==None, or in_interrupt() (leaked
                // preempt_count). Name which one fired. tp is validated
                // first: a trap mis-entered through .Lfrom_kernel can
                // leave a USER pointer in tp, and dereferencing it here
                // would fault inside the panic printer (nested panic).
                {
                    let tp = crate::arch::cpu::get_thread_id() as usize;
                    // Bit 63 set = kernel VA; Sv39 user pointers always
                    // have bits 63:39 clear. Covers both heap task structs
                    // (0xffffffd6...) and static idle tasks (0xffffffff...).
                    let kernel_tp = (tp >> 63) == 1;
                    use core::fmt::Write;
                    let mut w = crate::dfx::backtrace::ConsoleWriter::new();
                    if kernel_tp {
                        let task = unsafe {
                            &*(tp as *const crate::process::task::Task)
                        };
                        unsafe {
                            let _ = write!(w,
                                "trap: noctx pid={} preempt={:#x} in_intr={} aspace={}\n",
                                (*task).pid(),
                                (*task).preempt_count(),
                                crate::interrupt::preempt::in_interrupt(),
                                (*task).address_space().is_some());
                        }
                    } else {
                        let _ = write!(w,
                            "trap: noctx tp={:#x} NOT-A-KERNEL-TASK (misrouted entry?)\n",
                            tp);
                    }
                }
                // R9: dump the faulting task's children list raw — the
                // recurring NULL-walk corruption must be photographed at
                // the instant it faults (other CPUs sanitize the list if
                // we wait for GDB). Offsets via offset_of!, SBI output.
                if let Some(task) = crate::sched::current() {
                    // Offsets derived from the live Task layout via
                    // offset_of! (task.rs::task_offsets). The previous
                    // hardcoded 0x48/0x4c/0x758/0x768 all drifted by +8 when
                    // journal_handle/ti_on_cpu/task_refcnt were inserted
                    // before `state` — the walk was reading the wrong fields
                    // (state=0x50, pid=0x54, children=0x760, sibling=0x770
                    // at HEAD a6219f8). These are diagnostic-only reads.
                    use crate::process::task::task_offsets as tsk_off;
                    const OFF_STATE: usize = tsk_off::TASK_STATE;
                    const OFF_PID: usize = tsk_off::TASK_PID;
                    const OFF_CHILDREN: usize = tsk_off::TASK_CHILDREN;
                    const OFF_SIBLING: usize = tsk_off::TASK_SIBLING;
                    let t = (task as *mut crate::process::task::Task) as usize;
                    let head = (t + OFF_CHILDREN) as *const usize;
                    let mut pos = unsafe { core::ptr::read_volatile(head) };
                    let head_v = head as usize;
                    let mut n = 0u32;
                    let mut last_node = 0usize;
                    while pos != head_v && pos != 0 && pos > OFF_SIBLING && n < 8 {
                        let ct = pos - OFF_SIBLING;
                        let pid = unsafe { core::ptr::read_volatile((ct + OFF_PID) as *const u32) };
                        let state = unsafe { core::ptr::read_volatile((ct + OFF_STATE) as *const u32) };
                        // R12-4: recognize freed-task poison.
                        if pid == 0xDEAD_BEEF || state == 0xDEAD_BEEF {
                            for &b in b" POISONED-FREED-TASK" { put(b); }
                        }
                        for &b in b" child[" { put(b); }
                        let mut digs = [0u8; 10]; let mut k = 0; let mut v = n;
                        if v == 0 { digs[0] = b'0'; k = 1; }
                        while v > 0 { digs[k] = b'0' + (v % 10) as u8; k += 1; v /= 10; }
                        while k > 0 { k -= 1; put(digs[k]); }
                        for &b in b"] pid=0x" { put(b); }
                        let mut w = pid;
                        if w == 0 { put(b'0'); }
                        while w > 0 { digs[k] = b'0' + (w % 10) as u8; k += 1; w /= 10; }
                        while k > 0 { k -= 1; put(digs[k]); }
                        for &b in b" state=0x" { put(b); }
                        let mut w = state;
                        let mut kk = 0;
                        if w == 0 { put(b'0'); }
                        while w > 0 { digs[kk] = b'0' + (w % 10) as u8; kk += 1; w /= 10; }
                        while kk > 0 { kk -= 1; put(digs[kk]); }
                        put(b'\n');
                        last_node = pos;
                        pos = unsafe { core::ptr::read_volatile(pos as *const usize) };
                        n += 1;
                    }
                    if pos == 0 {
                        for &b in b" CHILDREN-CORRUPT: node .next==NULL after node=0x" { put(b); }
                        let mut sh2 = 64;
                        while sh2 > 0 { sh2 -= 4; let nb = ((last_node >> sh2) & 0xF) as u8; put(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
                        put(b'\n');
                    }
                }
            }
            // R9-7: unconditional — in release builds the debug-only loop
            // compiled out and the handler sret'd straight back into the
            // faulting kernel epc: an unbounded fault/print storm with the
            // locks still held.
            // SAFETY: wfi halts the hart until an interrupt; safe in a panic halt loop.
            loop {
                unsafe { core::arch::asm!("wfi") };
            }
        }
        _ => {}
    }
}

/// Handle unknown exception
fn handle_unknown_exception(regs: &mut PtRegs, cause: Cause) {
    crate::pr_debug!("trap: unknown exception {:?}, epc={:#x}, badaddr={:#x}",
        cause, regs.epc, regs.badaddr);
    crate::pr_err!("trap: Unknown exception: {:?}, epc={:#x}, badaddr={:#x}",
        cause, regs.epc, regs.badaddr);

    if regs.user_mode() {
        crate::process::exit::do_exit(-(crate::signal::Signal::SIGKILL as i32));
        // Do NOT advance epc — the task is now ZOMBIE and will not resume
    } else {
        // Kernel-mode unknown exceptions (misaligned access, access fault,
        // ...) indicate a kernel bug; skipping the instruction corrupts state.
        panic!(
            "trap: unknown exception {:?} in kernel mode at epc={:#x} badaddr={:#x}",
            cause, regs.epc, regs.badaddr
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

/// Debug function to print clone regs (called from assembly)
#[no_mangle]
pub extern "C" fn debug_print_clone_regs(_s1: u64, _sp: u64, _a7: u64, _epc: u64) {
    // Debug output disabled
}

/// Debug function called before schedule() in trap.S
/// Arguments: a0 = sp, a1 = tp, a2 = ti_kernel_sp
#[no_mangle]
pub extern "C" fn debug_before_schedule(_sp: u64, _tp: u64, _ti_kernel_sp: u64) {
    // Debug disabled
}

/// Debug function called after schedule() returns in trap.S
/// Arguments: a0 = sp, a1 = tp, a2 = ti_kernel_sp
#[no_mangle]
pub extern "C" fn debug_after_schedule(_sp: u64, _tp: u64, _ti_kernel_sp: u64) {
    // Debug disabled
}

/// Debug function to verify sepc was written correctly
/// Arguments: a0 = actual sepc value, a1 = expected value
#[no_mangle]
pub extern "C" fn debug_sepc_verify(_actual: u64, _expected: u64) {
    // Debug disabled
}

/// Debug function called at trap entry
/// Arguments: a0 = pt_regs location, a1 = cause, a2 = tp
#[no_mangle]
pub extern "C" fn debug_trap_entry(_regs_ptr: u64, _cause: u64, _tp: u64) {
    // Debug disabled
}

/// Debug function to print trap exit info (called from assembly)
#[no_mangle]
pub extern "C" fn debug_trap_exit(_sp: u64, _tp: u64) {
    // Debug disabled
}

// ============================================================================
// ret_from_fork functions
// ============================================================================

/// ret_from_fork_user - Called when a forked child returns to user mode
///
/// This is called from assembly ret_from_fork_user_asm after schedule_tail.
/// The child process will return to user space via ret_from_exception.
///
/// # Arguments
/// - `regs`: Pointer to the child's pt_regs (already set up by copy_thread)
#[no_mangle]
pub extern "C" fn ret_from_fork_user(_regs: *mut PtRegs) {
    // Called from assembly after schedule_tail
    // Child process returns to user mode via ret_from_exception
}

/// ret_from_fork_kernel - Called when a kernel thread starts execution
///
/// This is called from assembly ret_from_fork_kernel_asm after schedule_tail.
/// Kernel threads call their function and then exit.
///
/// # Arguments
/// - `fn_arg`: Argument to pass to the kernel thread function
/// - `fn_ptr`: Kernel thread function pointer
/// - `regs`: Pointer to pt_regs (for returning to user mode after thread exits)
#[no_mangle]
pub extern "C" fn ret_from_fork_kernel(fn_arg: *mut core::ffi::c_void,
                                       fn_ptr: extern "C" fn(*mut core::ffi::c_void) -> i32,
                                       _regs: *mut PtRegs) {
    // Call the kernel thread function
    let _ret = fn_ptr(fn_arg);

    // Kernel thread has finished, call do_exit
    crate::process::exit::do_exit(_ret);
}