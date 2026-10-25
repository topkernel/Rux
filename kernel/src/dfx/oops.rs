//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The non-fatal Oops path (Tranche 5's remaining bullet).
//!
//! The reference kernel's contract: a kernel fault that has no
//! exception-table fixup prints an `Oops` report — the faulting
//! instruction, the registers, the stack — taints the kernel
//! (`TAINT_DIE`, the `D` letter), and then POLICY decides what dies:
//!
//! - an oops in interrupt context, or with `panic_on_oops` set, is a
//!   full panic (Linux `oops_end`: `panic("Fatal exception in
//!   interrupt")` / `panic("Fatal exception")`);
//! - the idle task cannot die (`panic("Attempted to kill the idle
//!   task!")`);
//! - anything else faults inside SOME task's system call — the task is
//!   killed with SIGSEGV (`make_task_dead(SIGSEGV)`) and the machine
//!   keeps running.
//!
//! Until now every one of those faults took the whole machine down (a
//! KERNPANIC halt or `panic!`). The trade the reference makes and this
//! module adopts: the faulting task may hold locks when it dies, and
//! whatever it held stays held (the usual limp-wedge); in exchange the
//! other CPUs, the console, and every instrument keep working, so the
//! oops is diagnosable from a live system instead of a corpse. Hunts
//! that prefer the old loud halt boot `oops=panic`.
//!
//! Report shape: the first line and the die banner are the reference
//! kernel's, verbatim from Linux 5.15 (the Ubuntu 22.04 guest
//! generation) — `arch/riscv/mm/fault.c` `die_kernel_fault` /
//! `arch/x86/mm/fault.c` `show_fault_oops` for the head line,
//! `arch/{riscv,x86}/kernel/traps.c` `die()` for the banner, and
//! `show_regs_print_info` for the CPU/PID/Comm/Tainted context line.
//! The register rows follow the arch `__show_regs` layout from the
//! same source. A byte-exact transcript needs a live reference oops,
//! which this host cannot provoke — every line names its source in a
//! comment instead.
//!
//! Capture: the report stages into the pstore sink exactly like the
//! panic report (`arm_sink` before the first byte), and the record is
//! written into the reserved region as the reference's `dmesg` flavor
//! — `/sys/fs/pstore/dmesg-ramoops-0` after the reboot (the panic
//! record stays `console-ramoops-0`; a later panic overwrites either).

use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::dfx::backtrace::ConsoleWriter;
use crate::dfx::taint;

/// What class of kernel fault produced the oops — selects the head
/// line's wording (the reference derives the same distinction from the
/// trap cause / fault address).
#[derive(Clone, Copy)]
pub enum Kind {
    /// A kernel page fault with no fixup entry. `addr` chooses the
    /// reference's wording: below one page it is a NULL pointer
    /// dereference, above it a paging request.
    PageFault { addr: u64 },
    /// Kernel-mode illegal instruction (no fixup class).
    IllegalInstruction,
    /// A kernel-mode exception the arch has no specific handler for
    /// (`cause` is the arch's trap-cause word, riscv64 scause).
    UnknownException { cause: u64 },
}

/// The oops counter — the `[#N]` in the die banner (Linux's
/// `die_counter`, per boot).
static OOPS_COUNT: AtomicU32 = AtomicU32::new(0);

/// Re-entrancy latch: a fault INSIDE this report (a wild frame pointer
/// in the stack walk, a bad comm pointer) must not recurse — the
/// second entry parks that CPU with one marker line, the same
/// discipline as the trap printers' `KP_DUMP_ENTERED`.
static OOPS_ENTERED: AtomicBool = AtomicBool::new(false);

/// `panic_on_oops` — seeded from the `oops=panic` kernel parameter
/// (Linux panic.c's `oops_setup`) and writable at runtime through
/// `/proc/sys/kernel/panic_on_oops`. Default off, like the reference.
static PANIC_ON_OOPS: AtomicBool = AtomicBool::new(false);

/// Read the `oops=` kernel parameter once at boot (dfx init). Only
/// `oops=panic` is defined by the reference; any other value is
/// ignored, as upstream's `oops_setup` does.
pub fn init_from_cmdline() {
    if let Some(value) = crate::cmdline::get_param("oops") {
        if value.trim().eq_ignore_ascii_case("panic") {
            PANIC_ON_OOPS.store(true, Ordering::Release);
        }
    }
}

/// The live `panic_on_oops` value (the sysctl's read side).
pub fn panic_on_oops() -> bool {
    PANIC_ON_OOPS.load(Ordering::Acquire)
}

/// Set `panic_on_oops` (the sysctl's write side).
pub fn set_panic_on_oops(on: bool) {
    PANIC_ON_OOPS.store(on, Ordering::Release);
}

/// Print the Linux-shaped Oops report and apply the policy.
/// Never returns.
///
/// `regs_dump` renders the FAULTING trap frame — the register rows in
/// the arch `__show_regs` layout — not a `save_regs()` artifact (the
/// trap printers' SR-PROBE lesson: the panic register dump shows the
/// printer's own frame, which is exactly what an oops must not do).
#[cold]
pub fn oops(kind: Kind, regs_dump: &dyn Fn(&mut ConsoleWriter)) -> ! {
    // Latch first: recursion inside the report parks here instead of
    // nesting forever.
    if OOPS_ENTERED.swap(true, Ordering::Relaxed) {
        let mut put = |b: u8| unsafe { crate::console::putchar_no_lock(b) };
        for &b in b"trap: oops re-entered during report; halting\n" {
            put(b);
        }
        loop {
            crate::arch::cpu::wfi();
        }
    }

    // The die banner's counter, and the D taint, are added before the
    // report prints (Linux die() adds TAINT_DIE before show_regs), so
    // the report's own "Tainted:" line carries the D.
    let n = OOPS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    taint::add_taint(taint::TaintFlags::DIE);

    // Stage every report byte for the pstore record, like the panic
    // handler does FIRST (one relaxed load per byte on every other
    // boot).
    crate::dfx::pstore::arm_sink();

    {
        let mut w = ConsoleWriter::new();

        // ---- head line: the reference's, per arch and kind ----
        match kind {
            Kind::PageFault { addr } => {
                let what = if addr < 4096 {
                    "NULL pointer dereference"
                } else {
                    "paging request"
                };
                #[cfg(feature = "riscv64")]
                let _ = write!(
                    w,
                    "Unable to handle kernel {} at virtual address {:x}\n",
                    what, addr
                );
                #[cfg(feature = "x86_64")]
                let _ = write!(w, "BUG: unable to handle kernel {} at {:x}\n", what, addr);
            }
            Kind::IllegalInstruction => {
                // riscv do_trap_error's banner string ("Oops - illegal
                // instruction"); x86 reports illegal instructions through
                // the same die("Oops") path as faults, and prints the kind
                // from the trap nr line below.
                #[cfg(feature = "riscv64")]
                let _ = w.write_str("Oops - illegal instruction\n");
                #[cfg(feature = "x86_64")]
                let _ = w.write_str("BUG: kernel illegal instruction\n");
            }
            Kind::UnknownException { cause } => {
                #[cfg(feature = "riscv64")]
                let _ = write!(w, "Oops - unhandled exception, scause={:#x}\n", cause);
                #[cfg(feature = "x86_64")]
                let _ = write!(w, "BUG: unhandled kernel exception, vector={:#x}\n", cause);
            }
        }

        // ---- die banner: die()'s "%s [#%d]" (riscv) / __die's
        // "%s: %04lx [#%d] PREEMPT SMP" (x86). The x86 error-code word
        // has no Rux counterpart on the non-#PF kinds; 0000 prints.
        #[cfg(feature = "riscv64")]
        let _ = write!(w, "Oops [#{}]\n", n);
        #[cfg(feature = "x86_64")]
        let _ = write!(w, "Oops: 0000 [#{}] PREEMPT SMP\n", n);

        // ---- context line: show_regs_print_info ----
        let cpu = crate::arch::cpu_id();
        let (pid, comm): (u64, &[u8]) = match crate::sched::current() {
            Some(t) => {
                // SAFETY: current() returned a live task pointer; pid()
                // and comm() are plain reads.
                unsafe { ((*t).pid() as u64, (*t).comm() as &[u8]) }
            }
            None => (0, b"swapper".as_slice()),
        };
        let mut taint_buf = [0u8; 16];
        taint::taint_string(&mut taint_buf);
        let tainted = unsafe { core::str::from_utf8_unchecked(&taint_buf) };
        let _ = write!(w, "CPU: {} PID: {} Comm: {} Tainted: {}\n", cpu, pid, comm_str(comm), tainted);

        // ---- the faulting frame (arch __show_regs rows) ----
        regs_dump(&mut w);
    }

    // ---- Call Trace (the shared symbolized walker; prints its own
    // header) ----
    crate::dfx::backtrace::dump_stack();

    // ---- footer (the reference's dump_stack end marker) ----
    {
        let mut w = ConsoleWriter::new();
        let _ = w.write_str("---[ end trace 0000000000 ]---\n");
    }

    // ---- policy: Linux oops_end / make_task_dead ----
    if crate::interrupt::preempt::in_interrupt() {
        panic!("Fatal exception in interrupt");
    }
    if panic_on_oops() {
        panic!("Fatal exception");
    }
    match crate::sched::current() {
        // No current task (early boot) or the idle task: there is no
        // task to kill — the machine dies (make_task_dead's guards).
        None => panic!("Attempted to kill the idle task!"),
        Some(t) => {
            // SAFETY: current() returned a live task pointer.
            let pid = unsafe { (*t).pid() };
            if pid == 0 {
                panic!("Attempted to kill the idle task!");
            }
            // Recoverable: the faulting task dies with SIGSEGV and the
            // machine keeps running. The oops record goes to the pstore
            // region here (a policy-escalated oops does not: the panic
            // handler writes its own console record right after, and
            // the staged sink bytes — the oops report — ride inside it).
            crate::dfx::pstore::oops_capture(n);
            crate::process::exit::do_exit(-(crate::signal::Signal::SIGSEGV as i32));
        }
    }
}

/// Render a task comm as printable text (the reference prints comm with
/// "%.20s" — printable bytes, truncated; a comm with embedded control
/// bytes prints masked rather than corrupting the line).
fn comm_str(comm: &[u8]) -> &str {
    // comm fields are NUL-padded; stop at the first NUL like a C string.
    let end = comm.iter().position(|&b| b == 0).unwrap_or(comm.len());
    let bytes = &comm[..end.min(20)];
    match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => "?(invalid utf8)",
    }
}
