//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! DFX Subsystem — Debug, Fault, and Diagnostics
//!
//! Unified diagnostic subsystem for the Rux kernel. Consolidates scattered
//! debugging features into a single module and implements critical missing
//! capabilities for production kernel debugging.
//!
//! # Modules
//!
//! - `taint`      — Kernel taint bitmask
//! - `backtrace`  — Reusable `dump_stack()`, `dump_regs()`, `dump_csrs()`
//! - `bug`        — `WARN()`, `BUG()` macros
//! - `hexdump`    — Hex/memory dump utility
//! - `softlockup` — Softlockup detector (per-CPU timestamp check)
//! - `hung_task`  — Hung task detector (khungtaskd kernel thread)
//!
//! # Macros
//!
//! ```rust
//! warn_on!(condition)       // Non-fatal warning if condition is true
//! warn_on_once!(condition)  // Same but fires only once per callsite
//! bug_on!(condition)        // Fatal BUG if condition is true
//! bug!()                    // Unconditional fatal BUG
//! ```

pub mod taint;
pub mod backtrace;
pub mod bug;
pub mod hexdump;
pub mod softlockup;
pub mod hung_task;
#[cfg(feature = "riscv64")]
pub mod sbi_debug;
pub mod switches;
/// Runtime kernel symbol table (/proc/kallsyms, symbolized traces).
pub mod ksyms;
pub mod taskdump;
pub mod memwatch;
/// Full physical-accounting ledger (fake-OOM family): census + counters
/// reconciliation, triggered at the OOM moment and via the UART magic.
pub mod ledger;
/// Zero-put / free-refusal attribution counters (freelist-drift family):
/// which release site orphaned the census's refcount-0 pages.
pub mod driftwatch;
#[cfg(feature = "x86_64")]
pub mod scribble;
/// R83 (R74 producer hunt): live task-header audit + victim dumps.
pub mod r83hunt;
/// CLOCK_MONOTONIC jump tripwire (dfx=monowatch): samples the shared
/// timebase ~1 Hz and reports backward motion or multi-second spikes
/// with the active clocksource named.
pub mod monowatch;
/// Wild-write attribution tripwire (dfx=wildwatch, x86_64): RO-guards
/// the hpet quiet-statics cluster in every page-table alias; see
/// dfx/wildwatch.rs (wildwrite-hunt line).
pub mod wildwatch;
/// Buddy-meta shadow scan (dfx=metawatch): periodic lock-confirmed
/// INV-HEAP-* audit of the buddy free lists + per-page bitmap; see
/// dfx/metawatch.rs (wildwrite-hunt line).
pub mod metawatch;
/// Whole-syscall cost table + tick-mode accounting + kernel-rip page
/// histogram (perf-audit line, cmdline `syscost=1`); see dfx/syscost.rs.
pub mod syscost;
/// pstore-lite: the last panic's record in reserved RAM, re-exposed as
/// /sys/fs/pstore/console-ramoops-0 after the reboot (Tranche 5); see
/// dfx/pstore.rs.
pub mod pstore;
/// The non-fatal Oops path: Linux-shaped report, pstore dmesg capture,
/// task-death policy (Tranche 5); see dfx/oops.rs.
pub mod oops;
/// Soft-wedge heartbeat + one-shot wedge census (sc-series quiet-quiet
/// hunt, cmdline `dfx=schb`); see dfx/schb.rs.
pub mod schb;
/// ext4/bio cost-anatomy counters (perf-audit line, cmdline `fsw=1`);
/// see dfx/fsw.rs.
pub mod fsw;
/// Leak-origin probe (cmdline `dfx=leakhunt`): records the call site of
/// every task's OUTERMOST PREEMPT_OFFSET acquire and reports it the
/// moment a task returns to userspace (or is seen parked) with a nonzero
/// preempt_count — names the producer of the leaked-count wake-strand
/// family; see dfx/leakorigin.rs.
pub mod leakorigin;
/// Last-open path stash for TXN-HANG (dfx=txnhunt): sys_openat stashes
/// the resolved path at entry so the 1 Hz audit can name the file a
/// hung server thread is stuck opening (leg-28 item 2: storage_daemon
/// D-state open family); see dfx/pathstash.rs.
pub mod pathstash;
/// Process-tree invariant validator (dfx=treehunt, leg-29A): catches a
/// live task whose parent pointer names released memory — the E8-REPAR
/// "unlinked snapshot" class that produced the sc304 signal-Arc Oops —
/// plus stale thread-group parent snapshots; see dfx/treehunt.rs.
pub mod treehunt;
/// Slow-kernel-entry audit (dfx=slowtask, leg-33B): tasks stuck on the
/// same syscall entry for >= 8 s get a line with args and the parked
/// kernel-stack chain — attributes the leaf of nested-call cascades
/// (fs stalls behind binder reader exhaustion). See dfx/slowtask.rs.
pub mod slowtask;

/// Initialize the DFX subsystem.
///
/// Called during kernel boot after `sched::init()`.
/// Starts the softlockup detector and hung task detector.
pub fn init() {
    switches::init_from_cmdline();
    // oops=panic seeds panic_on_oops (the sysctl can flip it later).
    oops::init_from_cmdline();
    // Wild-write tripwire arms between the timekeeper's init and
    // start_secondaries (no TLB shootdown needed; see wildwatch.rs).
    wildwatch::arm();
    softlockup::init();
    // hung_task detector is deferred — it requires kthread infrastructure
    // that may not be fully ready during early boot. Enable after testing.
    // hung_task::init();
    // Publish the instrument switches in debugfs so they can be armed
    // and disarmed while the system runs.
    register_debugfs_knobs();
    // Publish the tracepoint control surface (tracefs).
    crate::trace::files::register();

    // aarch64 bring-up probe (dfx-aa64-uaccess-selftest): exercise the
    // uaccess exception-table fault containment with a poisoned user
    // pointer before init exec — every access must come back EFAULT
    // with no oops. Compile-time gated (the aarch64 cmdline cannot
    // carry a dfx= switch yet: the EL2 trampoline clobbers the x0 DTB
    // pointer, so cmdline parsing falls back to the default string).
    #[cfg(all(feature = "aarch64", feature = "dfx-aa64-uaccess-selftest"))]
    crate::arch::aarch64::uaccess::selftest();
}

/// Register the DFX instrument switches in debugfs.
///
/// Linux keeps runtime debug knobs out of the stable userspace ABI and
/// exposes them in debugfs instead. Rux follows the same rule and the
/// same conventions:
///
/// - one file per switch, named after its `dfx=` spelling;
/// - the file reads `Y` or `N` (Linux's `debugfs_create_bool`
///   convention);
/// - writing `Y`/`y`/`1` arms the instrument, `N`/`n`/`0` disarms it,
///   anything else is `EINVAL` (Linux's `debugfs_bool_set`).
///
/// ```text
/// /sys/kernel/debug/dfx/taskdump   -> Y
/// ```
///
/// Returns the number of knobs registered.
pub fn register_debugfs_knobs() -> usize {
    use crate::fs::debugfs;
    if !debugfs::create_dir("/dfx") {
        return 0;
    }
    let mut count = 0;
    for sw in switches::ALL {
        let path = alloc::format!("/dfx/{}", sw.name());
        let gen = move || {
            // Linux debugfs bool files: "Y\n" / "N\n".
            let mut out = alloc::vec![b'N', b'\n'];
            if switches::enabled(sw) {
                out[0] = b'Y';
            }
            out
        };
        let store = move |buf: &[u8]| -> i32 {
            let text = core::str::from_utf8(buf).unwrap_or("").trim();
            if text.is_empty() {
                // The guest's `echo v > file` writes the value and the
                // trailing newline as SEPARATE write() calls: the empty
                // second write is a no-op, not an error (same convention
                // as /proc/[pid]/oom_score_adj).
                return 0;
            }
            let on = match text {
                "Y" | "y" | "1" | "true" | "on" => true,
                "N" | "n" | "0" | "false" | "off" => false,
                _ => return crate::errno::Errno::InvalidArgument.as_neg_i32(),
            };
            switches::set(sw, on);
            crate::pr_info!(
                "dfx: runtime switch '{}' {}",
                sw.name(),
                if on { "enabled" } else { "disabled" }
            );
            0
        };
        if debugfs::create_file(&path, 0o644, gen, Some(store)) {
            count += 1;
        }
    }

    // The oops provoquer — LKDTM's provoke-crash discipline: a debugfs
    // write that faults in kernel mode on purpose, so the Oops path
    // (report, pstore capture, task-death policy) is exercisable from
    // a guest shell instead of waiting for a real kernel bug. The
    // faulting read sits on no lock path: the write handler itself
    // performs it, so the recoverable oops kills only the writer.
    // Kinds: "pagefault" (the default) — a NULL-adjacent kernel read
    // with no exception-table entry; "illegal" — an M-mode CSR in
    // S-mode (riscv64) / ud2 (x86_64).
    {
        let gen = || {
            b"write: pagefault | illegal\n".to_vec()
        };
        let store = |buf: &[u8]| -> i32 {
            let text = core::str::from_utf8(buf).unwrap_or("").trim();
            match text {
                "" => 0, // the trailing-newline write, same convention
                "pagefault" | "1" => {
                    crate::pr_info!("dfx: provoking kernel page fault oops");
                    // SAFETY: deliberate fault — read address 0x10 (a
                    // NULL-adjacent page with no mapping and no fixup
                    // entry on this instruction).
                    let p = 0x10usize as *const u64;
                    let _ = unsafe { core::ptr::read_volatile(p) };
                    // Unreachable when the oops policy runs; if the
                    // fault was fixed up somehow, report it.
                    crate::pr_err!("dfx: provoke-oops fault did not fire");
                    crate::errno::Errno::IOError.as_neg_i32()
                }
                "illegal" => {
                    crate::pr_info!("dfx: provoking kernel illegal-instruction oops");
                    #[cfg(feature = "riscv64")]
                    // SAFETY: deliberate fault — machine-mode CSR access
                    // from S-mode traps IllegalInstruction.
                    unsafe {
                        core::arch::asm!("csrr x0, 0x300", options(nomem, nostack));
                    }
                    #[cfg(feature = "x86_64")]
                    // SAFETY: deliberate fault — ud2 is reserved for
                    // traps (the BUG instruction).
                    unsafe {
                        core::arch::asm!("ud2", options(nomem, nostack));
                    }
                    crate::pr_err!("dfx: provoke-oops illegal did not fire");
                    crate::errno::Errno::IOError.as_neg_i32()
                }
                _ => crate::errno::Errno::InvalidArgument.as_neg_i32(),
            }
        };
        if debugfs::create_file("/dfx/provoke-oops", 0o644, gen, Some(store)) {
            count += 1;
        }
    }
    count
}

// ============================================================================
// Re-exported macros
// ============================================================================

/// `warn_on!` — Non-fatal warning if condition is true.
///
/// Returns `true` if the condition was true (warning was printed),
/// `false` otherwise. Always evaluates the condition.
///
/// # Example
/// ```rust
/// if warn_on!(ptr.is_null()) {
///     // warning was printed
/// }
/// ```
#[macro_export]
macro_rules! warn_on {
    ($cond:expr) => {{
        if $cond {
            $crate::dfx::bug::warn(file!(), line!(), stringify!($cond));
            true
        } else {
            false
        }
    }};
}

/// `warn_on_once!` — Non-fatal warning, fires only once per callsite.
///
/// Uses a static `AtomicBool` per callsite to ensure the warning
/// is printed at most once per boot.
#[macro_export]
macro_rules! warn_on_once {
    ($cond:expr) => {{
        static ONCE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if $cond && $crate::dfx::bug::warn_on_once_check(&ONCE) {
            $crate::dfx::bug::warn(file!(), line!(), stringify!($cond));
            true
        } else {
            false
        }
    }};
}

/// `bug_on!` — Fatal BUG if condition is true.
///
/// If the condition is true, prints a BUG message with stack trace
/// and calls `panic!()`. The kernel will not continue.
#[macro_export]
macro_rules! bug_on {
    ($cond:expr) => {
        if $cond {
            $crate::dfx::bug::bug(file!(), line!());
        }
    };
}

/// `bug!` — Unconditional fatal BUG.
///
/// Always triggers a BUG and calls `panic!()`.
#[macro_export]
macro_rules! bug {
    () => {
        $crate::dfx::bug::bug(file!(), line!());
    };
}
