//! The machine reboot/shutdown cascade (framework home; moved from
//! syscall/process.rs, crate-flip step 3 2026-11-21): the shutdown
//! cascade (SIGTERM-to-init -> bounded wait -> fs sync -> machine
//! reset), the Ctrl-Alt-Del latch/deliver pair, the panic-path reboot,
//! and the sysrq force pair. Process-lifecycle plumbing MORE THAN ONE
//! service consumes (panic_policy, console, evdev, exit, sysrq,
//! signal) — the crate-plan rule. The arch reset repertoire lives in
//! boot_edges.rs (INV-SYSC-4); this module orchestrates.

/// reboot(2) CAD mode: false = send SIGINT to init (Linux default),
/// true = reboot immediately (LINUX_REBOOT_CMD_CAD_ON).
static CAD_IMMEDIATE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// CAD latched by an input detector (UART RX IRQ / evdev push) and executed
/// in task context by `cad_deliver_pending()` — the ISIG pending pattern:
/// the pid-hash walk inside send_signal must not run from IRQ context.
static PENDING_CAD: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Last jiffy a CAD was executed (rate-limit: ignore repeats within the
/// window so a held key or a retried sequence cannot re-enter).
static LAST_CAD_JIFFY: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// CAD de-dup window in jiffies (~2s at 100Hz).
const CAD_RATELIMIT_JIFFIES: u64 = 200;

/// Latch a Ctrl-Alt-Del from an input detector. IRQ-safe: only atomic
/// stores, no locks, no pid-hash walks.
pub fn ctrl_alt_del_latch() {
    crate::println!("kernel: Ctrl-Alt-Del detected");
    PENDING_CAD.store(true, core::sync::atomic::Ordering::Release);
}

/// Execute any latched Ctrl-Alt-Del. Called in TASK context from
/// check_and_deliver_signals (every return to user space).
/// (perf-syscall lane) Load-first for the same reason as the ISIG
/// latch: one plain load per return instead of an RMW.
pub fn cad_deliver_pending() {
    // Load-first (syscall-exit hot path): called on EVERY return to
    // user space, so the disabled path must be a plain Acquire load —
    // the swap is a locked RMW on x86 and is paid only when a
    // Ctrl-Alt-Del is actually latched. Claim semantics are unchanged:
    // only the CPU whose swap observes `true` runs the cascade.
    if !PENDING_CAD.load(core::sync::atomic::Ordering::Acquire) {
        return;
    }
    if !PENDING_CAD.swap(false, core::sync::atomic::Ordering::AcqRel) {
        return; // another CPU claimed it
    }
    let now = crate::timer::get_jiffies();
    let last = LAST_CAD_JIFFY.load(core::sync::atomic::Ordering::Acquire);
    if now.saturating_sub(last) < CAD_RATELIMIT_JIFFIES {
        return; // duplicate within the window
    }
    LAST_CAD_JIFFY.store(now, core::sync::atomic::Ordering::Release);

    if CAD_IMMEDIATE.load(core::sync::atomic::Ordering::Acquire) {
        // C.A.D enabled (reboot(2) CAD_ON): Linux runs an emergency
        // restart. Rux routes it through the full cascade so the root
        // filesystem still gets synced before the SBI reset. Runs in the
        // delivering task's context and never returns.
        crate::println!("kernel: CAD immediate mode -> reboot");
        shutdown_cascade(LINUX_REBOOT_CMD_RESTART);
        loop {
            core::hint::spin_loop();
        }
    }

    // Linux default (C.A_D disabled): SIGINT to PID 1 — init decides what
    // a Ctrl-Alt-Del means (systemd maps it to reboot; udesk-init calls
    // reboot(2) itself). A tiny init that ignores SIGINT just keeps the
    // session alive, exactly like an unhandled CAD under Linux.
    crate::println!("kernel: CAD -> sending SIGINT to init (pid 1)");
    let _ = crate::signal::send_signal(1, crate::signal::Signal::SIGINT as i32);
}

/// The RESTART cmd value, shared by sys_reboot and the CAD immediate path.
const LINUX_REBOOT_CMD_RESTART: u32 = 0x01234567;

/// P1: how long to wait for PID 1 to exit after SIGTERM (5 seconds).
const SHUTDOWN_PID1_TIMEOUT_SECS: u64 = 5;

/// PID 1 "has exited" test for the cascade wait: PID 1 is parented to the
/// kernel, so a dead init lingers as a ZOMBIE nobody reaps — count ZOMBIE
/// (and DEAD) as exited instead of burning the whole timeout.
pub(crate) fn pid1_exited() -> bool {
    match crate::find_task_by_pid(1) {
        None => true,
        Some(t) => t.state().is_dead(),
    }
}

/// Set when a shutdown cascade is in flight (reboot(2) caller, the CAD
/// immediate path, or the init-exit fallback): a second entry must not
/// re-drive the teardown (double ext4 sync / racing SBI calls).
static SHUTDOWN_CASCADE_ACTIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Latch a Ctrl-Alt-Del from an input detector. IRQ-safe: only atomic


/// Execute any latched Ctrl-Alt-Del. Called in TASK context from
/// check_and_deliver_signals (every return to user space).
/// (perf-syscall lane) Load-first for the same reason as the ISIG


/// PID 1 "has exited" test for the cascade wait: PID 1 is parented to the
/// kernel, so a dead init lingers as a ZOMBIE nobody reaps — count ZOMBIE


/// Kernel fallback for "PID 1 exited" — the shutdown(8) init-signal path
/// ends here: init tears down every user process (including shutdown(8)
/// itself), exits, and the kernel completes the cascade. Without init no
/// orphan is ever reaped or respawned, so a real OS powers off (Linux
/// panics instead). Called from do_exit() of PID 1 before the ZOMBIE
/// transition; never returns unless another cascade already owns the
/// teardown (a reboot(2) caller is then waiting for exactly this exit).
/// The POWER_OFF cmd value (shared by the fallback cascade and
/// sys_reboot's callers).
const LINUX_REBOOT_CMD_POWER_OFF: u32 = 0x4321FEDC;

pub fn init_exited_fallback_cascade() {
    if SHUTDOWN_CASCADE_ACTIVE.load(core::sync::atomic::Ordering::Acquire) {
        return; // reboot(2)/CAD cascade in flight — it finishes the job
    }
    crate::println!("kernel: init exited — powering system down");
    shutdown_cascade(LINUX_REBOOT_CMD_POWER_OFF);
    // shutdown_cascade returns only on total SBI failure — park.
    loop {
        core::hint::spin_loop();
    }
}

/// Reboot the machine from the panic path (Linux's panic ->
/// kernel_restart).
///
/// Unlike the reboot(2) path this neither signals init nor syncs the
/// filesystems: Linux's panic path reboots without touching the disks,
/// because the panic may well be the storage stack itself. A sync that
/// hangs in a panic would defeat the whole point of the timeout.
pub fn panic_reboot() -> ! {
    #[cfg(feature = "riscv64")]
    {
        // SBI SRST warm reset (QEMU supports it); fall back to the
        // legacy shutdown ecall when the SBI lacks SRST.
        if !crate::sbi::sbi_system_reset(
            crate::sbi::srst_type::WARM_REBOOT,
            crate::sbi::srst_reason::NONE,
        ) {
            sbi_legacy_shutdown();
        }
    }
    #[cfg(feature = "x86_64")]
    {
        // The PC reset repertoire Linux's emergency_restart walks:
        // pulse the 8042 keyboard-controller reset line (port 0x64,
        // 0xFE — the classic hardware reset QEMU's q35 honours), then,
        // if the platform ignores it, force a triple fault through a
        // NULL IDT (the architectural last resort: every reboot path
        // ends in a reset). Before this, the x86 panic path only
        // parked — `panic=` timed out into machine_park and the pstore
        // guest-cycle smoke could never reach its boot-2 read.
        // The whole never-returning sequence lives in the
        // INV-SYSC-4 edge.
        crate::boot_edges::x86_panic_reset();
        machine_park();
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Restart the machine now (Linux sysrq 'b': "immediately restarts the
/// computer without syncing disks or unmounting them").
///
/// Never returns. The cascade still syncs what it can before the reset
/// — a debug trigger must not leave the image torn.
pub fn force_reboot() -> ! {
        crate::println!("sysrq: rebooting");
    shutdown_cascade(LINUX_REBOOT_CMD_RESTART);
    loop {
        core::hint::spin_loop();
    }
}

/// Power the machine off (Linux sysrq 'o').
///
/// Never returns.
pub fn force_power_off() -> ! {
    const LINUX_REBOOT_CMD_POWER_OFF: u32 = 0x4321FEDC;
    crate::println!("sysrq: powering off");
    shutdown_cascade(LINUX_REBOOT_CMD_POWER_OFF);
    loop {
        core::hint::spin_loop();
    }
}

/// P1 shutdown cascade body. Performs the ordered teardown and drives the
/// machine into the reset/shutdown SBI call; returns only if every SBI
/// path failed (caller parks).
pub fn shutdown_cascade(cmd: u32) {
        const LINUX_REBOOT_CMD_HALT: u32 = 0xCDEF0123;
    
    // Single-driver rule: the first cascade entry claims the teardown;
    // later entries (e.g. a reboot(2) racing the init-exit fallback)
    // park forever instead of double-driving it.
    if SHUTDOWN_CASCADE_ACTIVE.swap(true, core::sync::atomic::Ordering::AcqRel) {
        crate::println!("reboot: cascade already in progress");
        loop {
            // SAFETY: wfi halts the hart until an interrupt; the machine
            // is going down on the other CPU.
            crate::arch::cpu::wfi();
        }
    }

    // debug: final counter dump — short-lived probe boots (lmbench single-
    // case rigs power off at ~35 s, before the 60 s fsw tick) would
    // otherwise never print their accumulators.
    crate::debug::fsw::dump();
    crate::debug::cctx::dump();
    crate::debug::unixrd::dump();
    crate::debug::hilogcensus::dump();

    // --- 1. Tell init (PID 1) to shut down (SIGTERM, systemd sequence) ---
    let my_pid = crate::sched::get_current_pid();
    if my_pid != 1 {
        crate::println!("reboot: sending SIGTERM to init (pid 1)");
        let _ = crate::signal::send_signal(1, crate::signal::Signal::SIGTERM as i32);

        // --- 2. Wait for PID 1 to exit (bounded) ---
        let deadline = crate::timer::get_jiffies()
            + crate::timer::msecs_to_jiffies(SHUTDOWN_PID1_TIMEOUT_SECS * 1000);
        while crate::timer::get_jiffies() < deadline {
            if pid1_exited() {
                crate::println!("reboot: init exited");
                break;
            }
            sleep_one_tick();
        }
        if !pid1_exited() {
            crate::println!("reboot: init did not exit in time, continuing");
        }
    }

    // --- 3. Sync filesystems + minimal rootfs umount (flush + clean) ---
    crate::println!("reboot: syncing filesystems");
    // forkprof (cmdline forkprof=1): ride the shutdown cascade — every
    // probe rig ends with reboot(2), so the phase table lands in the
    // serial log without a dedicated reader.
    crate::debug::forkprof::report();
    // mmprof (cmdline mmprof=1): vm-op stage table, same discipline.
    crate::debug::mmprof::report_line(0);
    if let Err(e) = rux::api::ext4::shutdown_sync() {
        crate::pr_err!("reboot: filesystem sync failed, err={}", e);
    }

    // --- 4. Drive the machine down ---
    match cmd {
        LINUX_REBOOT_CMD_RESTART => {
            crate::println!("reboot: Restarting system");
            #[cfg(feature = "riscv64")]
            {
                // SBI SRST warm reset (QEMU supports it); fall back to the
                // legacy shutdown ecall when the SBI lacks SRST.
                if !crate::sbi::sbi_system_reset(
                    crate::sbi::srst_type::WARM_REBOOT,
                    crate::sbi::srst_reason::NONE,
                ) {
                    sbi_legacy_shutdown();
                }
            }
            #[cfg(feature = "x86_64")]
            {
                // mmi-endgame (116th): a REAL reset on q35. The old
                // stand-in parked forever — a guest whose init decides
                // to reboot (OH's critical-service policy) froze in a
                // half-dead zombie state instead of restarting, so the
                // image never got its crash-recovery loop. Linux's PC
                // reboot ladder, ACPI RESET_REG unparsed: the 8042
                // keyboard-controller pulse (outb 0xFE -> port 0x64,
                // honored directly by QEMU), then the PCI/ATX reset
                // port 0xcf9 (bit 2 = RSTDRV; QEMU resets on it).
                // machine_park only runs on hardware that ignored both
                // (the INV-SYSC-4 edge owns the port writes).
                crate::println!("reboot: resetting (8042 pulse / 0xcf9)");
                crate::boot_edges::x86_reboot_pulse();
                machine_park();
            }
        }
        _ => {
            // HALT / POWER_OFF
            if cmd == LINUX_REBOOT_CMD_HALT {
                crate::println!("reboot: System halted");
            } else {
                crate::println!("reboot: Power down");
            }
            #[cfg(feature = "riscv64")]
            sbi_legacy_shutdown();
            #[cfg(feature = "x86_64")]
            machine_park();
        }
    }
}

/// Park the CPU with interrupts waiting (x86_64 stand-in for the SBI
/// shutdown path until an ACPI/8259 reset driver lands).
#[cfg(feature = "x86_64")]
pub(crate) fn machine_park() -> ! {
    loop {
        crate::arch::cpu::wfi();
    }
}

/// SBI legacy shutdown ecall (FID 0x8) — the pre-0.3 power-off path.
#[cfg(feature = "riscv64")]
pub(crate) fn sbi_legacy_shutdown() {
    // The privileged ecall lives in the INV-SYSC-4 edge; reached only
    // after the CAP_SYS_BOOT gate in sys_reboot.
    crate::boot_edges::sbi_shutdown_ecall();
    // Some SBI implementations return instead of halting — park.
    loop {
        // wfi halts the hart until an interrupt.
        crate::arch::cpu::wfi();
    }
}

/// P1: sleep ~1 jiffy (10ms) so PID 1 and the exit path can run while the
/// reboot syscall waits for it (INTERRUPTIBLE sleep + one-shot timer, the
/// nanosleep/linger discipline).
pub(crate) fn sleep_one_tick() {
    use rux::process::types::{Task, TaskState};

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return, // no task context: busy caller
    };
    // current is the running task's reference (we are it).
    let pid = current.pid();
    let wake_at = crate::timer::get_jiffies() + 1;
    let timer_id = crate::timer::add_timer_wakeup(wake_at, pid);
    // Setting our own task's state.
    current.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    // R54: re-arm IRQs so the timer tick reaches this CPU.
    crate::arch::cpu::restore_irq(true);
    crate::sched::schedule();
    if timer_id != 0 {
        crate::timer::del_timer(timer_id);
    }
}

/// The POWER_OFF cmd value, shared by sys_reboot's match arms' callers
/// and the init-exit fallback (RESTART already has a module-level const
/// further up).

/// reboot(2) CAD mode switch (util-linux ctrlaltdel): the syscall arm
/// calls this setter — the static lives with its deliver path.
pub fn set_cad_immediate(on: bool) {
    CAD_IMMEDIATE.store(on, core::sync::atomic::Ordering::Release);
}
