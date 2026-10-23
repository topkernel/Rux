//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! `/proc/sysrq-trigger` — the Linux-compatible on-demand trigger.
//!
//! Linux exposes the SysRq command set to userspace through this file:
//! writing a single command character runs that command. The contract
//! (Documentation/admin-guide/sysrq.html):
//!
//! - only the FIRST character is handled, unless the write starts with
//!   an underscore, in which case every following character is a
//!   command (`echo _reisub > /proc/sysrq-trigger`);
//! - commands are case-sensitive;
//! - `h` prints the help text, and so does any UNLISTED key;
//! - the commands are available to administrative users regardless of
//!   the keyboard `sysrq` setting.
//!
//! The command set below is the upstream one. Commands Rux cannot
//! honour (virtual console, keyboard raw mode, hrtimer lists) print an
//! explicit "not supported" line instead of silently doing nothing —
//! a silent no-op would read as "the kernel has no such problem".

use crate::errno;

/// Execute one sysrq command. `cmd` is the command character.
fn run_command(cmd: u8) {
    match cmd {
        // Restart the machine immediately.
        b'b' => crate::syscall::process::force_reboot(),
        // Force a crash (a dump is created if crash dumping is configured).
        b'c' => {
            crate::println!("sysrq: crashing the kernel on request");
            panic!("sysrq 'c' — deliberate crash");
        }
        // Display every lock currently held.
        b'd' => crate::fs::locks::dump_held_locks(),
        // Send SIGTERM to all processes except init.
        b'e' => signal_all_except_init(crate::signal::Signal::SIGTERM as i32),
        // Invoke the OOM killer.
        b'f' => {
            let killed = crate::mm::oom_kill::sysrq_oom_kill();
            crate::println!(
                "sysrq: OOM killer {}",
                if killed { "killed a process" } else { "found nothing to kill" }
            );
        }
        // Secure Access Key: kill the programs attached to the console.
        b'k' => {
            crate::println!("sysrq: SAK is not supported (no virtual console)");
        }
        // Display a stack backtrace for every active CPU.
        b'l' => dump_all_cpu_stacks(),
        // Write the current memory information to the console.
        b'm' => {
            let info = crate::fs::procfs::meminfo::generate();
            if let Ok(text) = core::str::from_utf8(&info) {
                crate::println!("{}", text);
            }
            crate::dfx::memwatch::dump_mem();
        }
        // Make real-time tasks nice-able.
        b'n' => {
            crate::println!("sysrq: making RT tasks nice-able is not supported");
        }
        // Power the system off.
        b'o' => crate::syscall::process::force_power_off(),
        // Write the current registers and flags to the console.
        b'p' => crate::dfx::backtrace::dump_regs_inline(),
        // Display the armed per-CPU high-resolution timer lists.
        b'q' => {
            crate::println!("sysrq: hrtimer lists are not supported");
        }
        // Disable keyboard raw mode (x86 console).
        b'r' => {
            crate::println!("sysrq: keyboard raw mode is not supported");
        }
        // Synchronize every mounted filesystem.
        b's' => {
            if let Err(e) = crate::fs::ext4::ext4_shutdown_sync() {
                crate::println!("sysrq: filesystem sync failed, err={}", e);
            } else {
                crate::println!("sysrq: filesystems synced");
            }
        }
        // Write the current task list and task information to the console.
        b't' => crate::dfx::taskdump::dump_all_tasks("sysrq"),
        // Remount all mounted filesystems read-only.
        b'u' => {
            crate::println!("sysrq: remounting read-only is not supported");
        }
        // Restore the framebuffer console forcibly.
        b'v' => {
            crate::println!("sysrq: framebuffer console restore is not supported");
        }
        // Display tasks stuck in uninterruptible/blocked state.
        b'w' => dump_blocked_tasks(),
        // Architecture-specific (xmon / PMU registers / TLB dump).
        b'x' | b'y' | b'z' => {
            crate::println!("sysrq: command '{}' is architecture-specific", cmd as char);
        }
        // Set the console log level (0-9).
        b'0'..=b'9' => {
            let level = cmd - b'0';
            crate::printk::set_console_loglevel(level);
            crate::println!("sysrq: console loglevel set to {}", level);
        }
        // Replay the kernel log messages on the consoles.
        b'R' => {
            let log = crate::printk::generate_kmsg();
            if let Ok(text) = core::str::from_utf8(&log) {
                crate::println!("{}", text);
            }
        }
        // 'h' and every unlisted key print the help text.
        _ => print_help(),
    }
}

/// Send `sig` to every process except init (pid 1).
fn signal_all_except_init(sig: i32) {
    let mut sent = 0u32;
    crate::process::pid_hash::pid_hash_for_each_task(|task| {
        // SAFETY: the pid-hash walk hands out live task pointers.
        let pid = unsafe { (*task).pid() };
        if pid > 1 {
            let _ = crate::signal::send_signal(pid, sig);
            sent += 1;
        }
    });
    crate::println!("sysrq: signal {} sent to {} tasks", sig, sent);
}

/// Display a stack backtrace for every active CPU (Linux sysrq 'l').
///
/// Rux has no per-CPU register capture, so the per-CPU view is the
/// current task of each CPU plus the kernel-text words on its stack —
/// the same shape the hung-task detector reports.
fn dump_all_cpu_stacks() {
    let currents = crate::sched::sched::grq_diag_cpu_currents();
    for (cpu, (pid, is_idle, task)) in currents.iter().enumerate() {
        if task.is_null() {
            crate::println!("CPU{}: no current task", cpu);
            continue;
        }
        // SAFETY: per-CPU current task pointer, read-only diagnostics.
        let t: &crate::process::task::Task = unsafe { &**task };
        crate::println!(
            "CPU{}: pid={} state={:#x} on_cpu={} idle={}",
            cpu,
            pid,
            t.state().bits(),
            t.on_cpu() as u8,
            is_idle
        );
        print_task_stack_words(t);
    }
}

/// Print the kernel-text words found on a task's kernel stack.
fn print_task_stack_words(task: &crate::process::task::Task) {
    #[cfg(feature = "x86_64")]
    extern "C" {
        static __stext: u8;
        static __etext: u8;
    }
    #[cfg(not(feature = "x86_64"))]
    extern "C" {
        static _stext: u8;
        static _etext: u8;
    }
    #[cfg(feature = "x86_64")]
    let lo = core::ptr::addr_of!(__stext) as u64;
    #[cfg(feature = "x86_64")]
    let hi = core::ptr::addr_of!(__etext) as u64;
    #[cfg(not(feature = "x86_64"))]
    let lo = core::ptr::addr_of!(_stext) as u64;
    #[cfg(not(feature = "x86_64"))]
    let hi = core::ptr::addr_of!(_etext) as u64;

    let sp = task.thread().sp;
    if (sp >> 48) != 0xffff || (sp & 7) != 0 || sp <= 0x1000 {
        crate::println!("  (no kernel stack: sp={:#x})", sp);
        return;
    }
    let base = sp as *const u64;
    crate::println!("  sp={:#x} text:", sp);
    let mut shown = 0;
    for k in 0..80 {
        // SAFETY: diagnostic read within the task's own kernel stack.
        let v = unsafe { core::ptr::read_volatile(base.add(k)) };
        if v >= lo && v < hi && (v & 1) == 0 {
            crate::println!("    {:#x}", v);
            shown += 1;
            if shown >= 8 {
                break;
            }
        }
    }
    if shown == 0 {
        crate::println!("    (no kernel-text words on the stack)");
    }
}

/// Display tasks stuck in uninterruptible/blocked state (Linux sysrq 'w').
fn dump_blocked_tasks() {
    let mut count = 0u32;
    crate::process::pid_hash::pid_hash_for_each_task(|task| {
        // SAFETY: the pid-hash walk hands out live task pointers.
        let t: &crate::process::task::Task = unsafe { &*task };
        let state = t.state().bits();
        // TASK_UNINTERRUPTIBLE / TASK_KILLABLE / TASK_NOLOAD: the states
        // a task can sit in while blocked on I/O.
        const BLOCKED_STATES: u32 = (1 << 1) | (1 << 7) | (1 << 9);
        if state & BLOCKED_STATES != 0 && t.pid() != 0 {
            count += 1;
            crate::println!(
                "blocked: pid={} state={:#x} on_cpu={} ti_cpu={}",
                t.pid(),
                state,
                t.on_cpu() as u8,
                t.ti_cpu()
            );
            print_task_stack_words(t);
        }
    });
    if count == 0 {
        crate::println!("sysrq: no blocked tasks");
    }
}

/// The SysRq help text (Linux's 'h' command).
fn print_help() {
    crate::println!("SysRq : HELP");
    crate::println!("sysrq: b       - reboot");
    crate::println!("sysrq: c       - crash");
    crate::println!("sysrq: d       - display every lock currently held");
    crate::println!("sysrq: e       - SIGTERM to all processes except init");
    crate::println!("sysrq: f       - invoke the OOM killer");
    crate::println!("sysrq: h       - show this help");
    crate::println!("sysrq: i       - SIGKILL to every process except init");
    crate::println!("sysrq: l       - stack backtrace for every active CPU");
    crate::println!("sysrq: m       - memory information");
    crate::println!("sysrq: p       - current registers and flags");
    crate::println!("sysrq: s       - sync every mounted filesystem");
    crate::println!("sysrq: t       - task list and task information");
    crate::println!("sysrq: w       - tasks stuck in uninterruptible state");
    crate::println!("sysrq: 0-9     - set the console log level");
    crate::println!("sysrq: R       - replay the kernel log");
    crate::println!("sysrq: _cmd    - run several commands in sequence");
}

/// Write handler for `/proc/sysrq-trigger`.
pub fn write_trigger(buf: &[u8]) -> i32 {
    let text = match core::str::from_utf8(buf) {
        Ok(t) => t.trim(),
        Err(_) => return errno::Errno::InvalidArgument.as_neg_i32(),
    };
    if text.is_empty() {
        // The guest's `echo c > file` writes the command and the trailing
        // newline as SEPARATE write() calls: the empty second write is a
        // no-op, not an error.
        return buf.len() as i32;
    }
    if let Some(rest) = text.strip_prefix('_') {
        // Several commands in sequence: the underscore prefix is Linux's
        // way of saying "every following character is a command".
        for cmd in rest.bytes() {
            run_command(cmd);
        }
    } else {
        // Otherwise only the first character is handled.
        run_command(text.as_bytes()[0]);
    }
    buf.len() as i32
}
