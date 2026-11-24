//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! System call dispatch module
//!
//! This module handles system call dispatch and common processing

use crate::arch::pt_regs::PtRegs;
use super::*;

/// System call argument array type
pub type SyscallArgs = [u64; 6];

/// FLIP M3-A: the two dispatch tables. Filled by the service crate's
/// register_all (rux-syscall) at boot through the BIN composition; when
/// filled, the generic match below is bypassed for table hits (the
/// fallback match stays as the in-lib migration default until the M3-B
/// package wiring removes the in-lib service files).
pub type SyscallArgsFn = fn(SyscallArgs) -> i64;

/// Native-number table dispatch for the x86_64 legacy preamble (the
/// numbers the pre-translate arms used to call directly — State B
/// final: the service crate registers them at their NATIVE slots).
#[cfg(feature = "x86_64")]
fn native_table_call(nr: u32, args: SyscallArgs, regs: &mut crate::arch::pt_regs::PtRegs) -> i64 {
    if let Some(r) = table_lookup(nr, args, regs) {
        return r;
    }
    -(crate::errno::constants::ENOSYS as i64)
}

pub type SyscallRegsFn = fn(&mut crate::arch::pt_regs::PtRegs) -> i64;

pub const NR_SYSCALLS: usize = 1200;

pub static SYSCALL_ARGS_TABLE: crate::safety::slot::OnceSlot<
    [Option<SyscallArgsFn>; NR_SYSCALLS],
> = crate::safety::slot::OnceSlot::new();

pub static SYSCALL_REGS_TABLE: crate::safety::slot::OnceSlot<
    [Option<SyscallRegsFn>; 4],
> = crate::safety::slot::OnceSlot::new();

/// Registration entry (called by rux-syscall::register_all through the
/// bin): args-table slice indexed by nr, plus the regs-variant entries
/// (nr 139 rt_sigreturn today).
pub fn syscall_register_tables(
    args: &'static [(usize, SyscallArgsFn)],
    regs: &'static [(usize, SyscallRegsFn)],
) {
    let mut atab = [None; NR_SYSCALLS];
    for &(nr, f) in args {
        if nr < NR_SYSCALLS {
            atab[nr] = Some(f);
        }
    }
    let _ = SYSCALL_ARGS_TABLE.publish(atab);
    let mut rtab: [Option<SyscallRegsFn>; 4] = [None; 4];
    for &(i, f) in regs.iter().take(4) {
        rtab[i] = Some(f);
    }
    let _ = SYSCALL_REGS_TABLE.publish(rtab);
}

#[inline]
fn table_lookup(nr: u32, args: SyscallArgs, regs: &mut crate::arch::pt_regs::PtRegs) -> Option<i64> {
    if let Some(atab) = SYSCALL_ARGS_TABLE.get() {
        if (nr as usize) < NR_SYSCALLS {
            if let Some(f) = atab[nr as usize] {
                return Some(f(args));
            }
        }
    }
    if nr == 139 {
        if let Some(rtab) = SYSCALL_REGS_TABLE.get() {
            if let Some(f) = rtab[0] {
                return Some(f(regs));
            }
        }
    }
    None
}

/// Get system call number from PtRegs
#[inline]
fn syscall_get_nr(regs: &PtRegs) -> u64 {
    regs.syscall_nr() as u64
}

/// Get system call arguments from PtRegs
#[inline]
fn syscall_get_arguments(regs: &PtRegs) -> SyscallArgs {
    regs.syscall_args()
}

/// Get the system call return value from PtRegs — a0 on riscv64, rax
/// on x86_64 (the register `set_return_value` writes).
#[inline]
fn syscall_get_return(regs: &PtRegs) -> i64 {
    #[cfg(feature = "riscv64")]
    {
        regs.a0 as i64
    }
    #[cfg(feature = "x86_64")]
    {
        regs.rax as i64
    }
    #[cfg(feature = "aarch64")]
    {
        regs.regs[0] as i64
    }
}

// FORENSIC: global syscall ring — replayed at NOVMA crashes to reconstruct
// the victim's final syscall sequence.
pub struct SyscallEntry {
    pub pid: core::sync::atomic::AtomicU32,
    pub nr: core::sync::atomic::AtomicU32,
    pub a0: core::sync::atomic::AtomicU64,
    pub a1: core::sync::atomic::AtomicU64,
    pub ret: core::sync::atomic::AtomicU64,
}
impl SyscallEntry {
    const fn new() -> Self {
        Self {
            pid: core::sync::atomic::AtomicU32::new(0),
            nr: core::sync::atomic::AtomicU32::new(0),
            a0: core::sync::atomic::AtomicU64::new(0),
            a1: core::sync::atomic::AtomicU64::new(0),
            ret: core::sync::atomic::AtomicU64::new(0),
        }
    }
    fn record(&self, pid: u32, nr: u32, a0: u64, a1: u64, ret: u64) {
        use core::sync::atomic::Ordering::Relaxed;
        self.pid.store(pid, Relaxed);
        self.nr.store(nr, Relaxed);
        self.a0.store(a0, Relaxed);
        self.a1.store(a1, Relaxed);
        self.ret.store(ret, Relaxed);
    }
}
const SYSCALL_NEW: SyscallEntry = SyscallEntry::new();
pub static SYSCALL_RING: [SyscallEntry; 2048] = [SYSCALL_NEW; 2048];
pub static SYSCALL_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Set system call return value
#[inline]
fn syscall_set_return_value(regs: &mut PtRegs, value: i64) {
    regs.set_return_value(value);
}

/// rux:strace1 (retired debug=strace1): one console line per loader-relevant syscall of PID 1.
///
/// Numbers are the GENERIC (asm-generic) dispatch numbers — the x86_64
/// native numbers were translated before the dispatch match.
fn strace1_line(nr: u32, args: &SyscallArgs, result: i64) {
    // (name, pathname argument index or None)
    let entry: Option<(&'static str, Option<usize>)> = match nr {
        48 => Some(("faccessat", Some(1))),   // (dirfd, path, mode)
        56 => Some(("openat", Some(1))),      // (dirfd, path, flags, mode)
        57 => Some(("close", None)),
        61 => Some(("getdents64", None)),
        62 => Some(("lseek", None)),
        63 => Some(("read", None)),
        64 => Some(("write", None)),
        67 => Some(("pread64", None)),
        78 => Some(("readlinkat", Some(1))),
        79 => Some(("fstatat", Some(1))),
        80 => Some(("fstat", None)),
        214 => Some(("brk", None)),
        215 => Some(("munmap", None)),
        221 => Some(("execve", Some(0))),
        222 => Some(("mmap", None)),          // fd/off shown numerically
        226 => Some(("mprotect", None)),
        // 118th ladder hunt: the loop/event family — where does init's
        // job engine diverge from the reference (eventfd/timerfd/epoll/
        // signalfd/rt_sigreturn + the usual wait family).
        9 => Some(("mmap", None)),            // mmap legacy
        10 => Some(("mprotect", None)),       // mprotect legacy
        11 => Some(("munmap", None)),         // munmap legacy
        232 => Some(("epoll_wait", None)),
        281 => Some(("epoll_pwait", None)),
        233 => Some(("epoll_ctl", None)),
        291 => Some(("epoll_create1", None)),
        290 => Some(("eventfd2", None)),
        283 => Some(("timerfd_create", None)),
        286 => Some(("timerfd_settime", None)),
        126 => Some(("sigaltstack", None)),
        34 => Some(("rt_sigprocmask", None)),
        13 => Some(("rt_sigaction", None)),
        135 => Some(("rt_sigprocmask", None)), // legacy sigprocmask
        431 => Some(("clone3", None)),        // modern spawn path
        437 => Some(("clock_nanosleep", None)),
        230 => Some(("clock_nanosleep", None)),
        _ => None,
    };
    let (name, path_arg) = match entry {
        Some(e) => e,
        None => return,
    };

    let mut line: alloc::string::String = alloc::format!("S1 {}(", name);
    match path_arg {
        Some(i) => {
            let mut buf = [0u8; 128];
            let shown = match crate::arch::uaccess::strncpy_from_user(
                args[i] as *const u8, buf.len(), &mut buf,
            ) {
                Ok(s) => core::str::from_utf8(s).unwrap_or("<non-utf8>"),
                Err(_) => "<efault>",
            };
            line.push_str(&alloc::format!("\"{}\"", shown));
            if i > 0 {
                line.push_str(&alloc::format!(", dirfd={}", args[0]));
            }
            if name == "openat" {
                line.push_str(&alloc::format!(", flags={:#x}", args[2]));
            }
        }
        None => match name {
            "mmap" => line.push_str(&alloc::format!(
                "addr={:#x} len={:#x} prot={:#x} flags={:#x} fd={} off={:#x}",
                args[0], args[1], args[2], args[3], args[4] as i64, args[5]
            )),
            "read" | "pread64" => line.push_str(&alloc::format!(
                "fd={} count={:#x}{}", args[0] as i64, args[2],
                if name == "pread64" { alloc::format!(", off={:#x}", args[3]) } else { alloc::string::String::new() }
            )),
            _ => line.push_str(&alloc::format!(
                "{}{}", args[0] as i64,
                if name == "write" || name == "lseek" || name == "fstat" || name == "close" {
                    alloc::format!(", {:#x}", args[1])
                } else if name == "lseek" || name == "mprotect" {
                    alloc::format!(", {:#x}, {:#x}", args[1], args[2])
                } else {
                    alloc::string::String::new()
                }
            )),
        },
    }
    line.push_str(&alloc::format!(") = {}", result));
    crate::pr_warn!("{}", line);
}

/// System call entry function
///
/// Called by trap.rs, dispatches to specific system call handlers.
/// perf-audit wrapper: whole-syscall wall-time accounting
/// (debug/syscost.rs, cmdline `syscost=1`) around the real dispatcher.
/// Diverging paths (do_exit on exit_group / fatal signals) skip the
/// exit hook, so those calls are undercounted — acceptable for a
/// ranking probe.
/// Whether the per-task LASTSYS latch runs on every syscall (perf:
/// 1 per-CPU load + 3 stores; the reference carries this in
/// thread_info flags that are only set under ptrace/audit). Turned on
/// by taskdump / debug when a dump is requested; off = zero cost.
static LASTSYS_LATCH_ON: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// 118th ladder-stall hunt: arm the LASTSYS latch from the first syscall
/// when schedtrace is armed (debug hunt boots only — the per-syscall cost
/// is 3 stores, acceptable under debug) so EVERY parked task's true syscall
/// is visible at dump time. Without this, pid 1 parks BEFORE the first
/// taskdump runs and its row reads lastsys=? — and the ladder break is
/// exactly "which syscall is init's main thread parked in".
pub fn lastsys_latch_arm() {
    LASTSYS_LATCH_ON.store(true, core::sync::atomic::Ordering::Release);
}

pub extern "C" fn syscall_handler(regs: &mut PtRegs) {
    let sc_nr = syscall_get_nr(regs) as usize;
    let sc_t0 = crate::debug::syscost::enter();
    // The raw_syscalls:sys_enter / sys_exit tracepoints, recorded
    // through the generated typed call functions. (gap-net) The six-word
    // argument marshal is guarded by the armed check itself — the typed
    // call's internal check alone still paid the copy on every syscall
    // when tracing was off.
    if crate::trace::EVENT_SYS_ENTER.armed() {
        let trace_args = syscall_get_arguments(regs);
        crate::trace::trace_sys_enter(
            sc_nr as i64,
            trace_args[0],
            trace_args[1],
            trace_args[2],
            trace_args[3],
            trace_args[4],
            trace_args[5],
        );
    }
    syscall_handler_inner(regs);
    // The return value: a0 (riscv64) / rax (x86_64).
    if crate::trace::EVENT_SYS_EXIT.armed() {
        crate::trace::trace_sys_exit(sc_nr as i64, syscall_get_return(regs) as i64);
    }
    // 118th EIO hunt (rux:eiotrace (retired debug=eiotrace) rides DfxSwitch::EpollTrace): name the
    // EXACT syscall returning -EIO ('init_hisysevent.c:140 Failed to write
    // event ret -5' — the write(2) path logger never fired, so the -5 rides
    // a different syscall). Budgeted direct-console line: nr, args head,
    // pid, comm. Reentrancy-guarded (this is every syscall's exit).
    {
        let rv = syscall_get_return(regs);
        if rv == -5
            && crate::trace::EVENT_EPOLL_DELIVERY.armed()
        {
            crate::syscall::support::syscall_eio_line(sc_nr as u64, &syscall_get_arguments(regs));
        }
        // launcher-errno lane: every failing syscall (EAGAIN excluded —
        // the idle-park storm would drown the signal) feeds the
        // per-(nr, errno) census. Two compares when syscost is off.
        if rv < 0 && rv != -11 {
            let sc_args = syscall_get_arguments(regs);
            crate::debug::syscost::note_errno(sc_nr, rv, sc_args[0], sc_args[1]);
        }
    }
    crate::debug::syscost::exit(sc_nr, sc_t0);
    // pa15 recv-family fd census (hilogd.server dual-drain evidence for
    // the ipc lane; fd/flags/ret profile per (comm, nr), zero cost when
    // syscost off). x86_64 native nrs 47=recvmsg 45=recvfrom.
    #[cfg(feature = "x86_64")]
    if sc_nr == 47 || sc_nr == 45 {
        let a = syscall_get_arguments(regs);
        // recvmsg(fd, msg, flags) -> flags at a[2]; recvfrom(fd, buf,
        // len, flags, ...) -> flags at a[3].
        let flags = if sc_nr == 47 { a[2] } else { a[3] } as i32;
        crate::debug::syscost::recv_census(sc_nr, a[0] as usize, flags, regs.rax as i64);
    }
}

fn syscall_handler_inner(regs: &mut PtRegs) {
    let syscall_no = syscall_get_nr(regs);
    let mut args = syscall_get_arguments(regs);

    // LASTSYS latch at the very top — BEFORE the x86_64 legacy early
    // returns (arch_prctl 158, poll 7, select 23, dup2 33, pause 34,
    // epoll_create 213, epoll_wait 232): glibc's epoll_wait() issues the
    // NATIVE 232, so a task parked in an event loop latched under the
    // later entry point showed its PREVIOUS syscall (a prctl!) instead
    // of the epoll_wait it is sleeping in. The raw native number is
    // stored; decode against the x86_64 table.
    // LASTSYS latch: serves taskdump lastsys attribution (parked tasks).
    // Gated by a static AtomicBool (set by taskdump when it runs; zero
    // cost on the fast path when no dump has been requested — saves a
    // per-CPU load + 3 stores per syscall).
    if LASTSYS_LATCH_ON.load(core::sync::atomic::Ordering::Relaxed) {
        let cur = crate::sched::current();
        if let Some(cur) = cur {
            use core::sync::atomic::Ordering::Relaxed;
            cur.last_syscall_nr.store(syscall_no as u32, Relaxed);
            cur.last_syscall_a0.store(args[0], Relaxed);
            cur.last_syscall_a1.store(args[1], Relaxed);
        }
    }

    // x86_64: translate the native number/ABI onto the generic table
    // (identity for same-numbered entries, argument remaps for the
    // legacy no-`at` ABI). Untranslatable numbers report as unknown.
    // x86_64-native syscalls with no asm-generic equivalent.
    #[cfg(feature = "x86_64")]
    if syscall_no == 158 {
        // arch_prctl(2): TLS base (glibc/musl startup requirement).
        let result = crate::arch::process::sys_arch_prctl(args);
        syscall_set_return_value(regs, result);
        return;
    }

    // x86_64 legacy syscalls that have NO asm-generic number — handled
    // BEFORE translate() so they can never collide with the same-numbered
    // generic entry (poll(7) vs lsetxattr(7), select(23) vs dup(23),
    // pause(34) vs mkdirat(34), epoll_create(213) vs readahead(213),
    // epoll_wait(232) vs nothing → ENOSYS). asm-generic libc builds
    // emulate these over ppoll/pselect6/sigsuspend, but x86_64 musl/glibc
    // issue the native numbers (LTP x86-r2 N2 collaterals: poll01 EBADF,
    // pause01 EFAULT-from-mkdirat, epoll_create01, epoll_wait01).
    #[cfg(feature = "x86_64")]
    match syscall_no {
        7 => {
            // poll(fds, nfds, timeout_ms) — handler exists, no generic slot.
            let r = native_table_call(7, args, regs); syscall_set_return_value(regs, r);
            return;
        }
        23 => {
            // select(nfds, r, w, e, timeval*) — timeval timeout variant.
            let r = native_table_call(23, args, regs); syscall_set_return_value(regs, r);
            return;
        }
        33 => {
            // dup2(oldfd, newfd). musl/glibc on x86_64 issue the native
            // NR 33 (on asm-generic archs their libc emulates the equal-fd
            // case over fcntl+dup3 before reaching the kernel — musl
            // dup2.c). Linux dup2 semantics: oldfd == newfd returns newfd
            // if it is open, else EBADF; dup3() itself would EINVAL.
            let result = if args[0] == args[1] {
                let probe = native_table_call(72, [args[0], 1 /* F_GETFD */, 0, 0, 0, 0], regs);
                if probe < 0 {
                    probe // -EBADF from the lookup itself
                } else {
                    args[0] as i64
                }
            } else {
                native_table_call(24, [args[0], args[1], 0, 0, 0, 0], regs)
            };
            syscall_set_return_value(regs, result);
            return;
        }
        37 => {
            // alarm(seconds) — one-shot ITIMER_REAL, returns the previous
            // remaining seconds (rounded up). No asm-generic number;
            // untranslated, 37 fell through to generic 37 = linkat and
            // every glibc alarm() caller parsed a linkat errno as the
            // return value (LTP alarm02/03/05/06/07 family).
            let r = native_table_call(37, args, regs); syscall_set_return_value(regs, r);
            return;
        }
        78 => {
            // getdents(fd, buf, count) — legacy record layout (name at
            // 18, d_type at 18+nlen+1). No asm-generic number (generic
            // libcs use getdents64); untranslated, 78 fell through to
            // generic 78 = readlinkat.
            let r = native_table_call(141, args, regs); syscall_set_return_value(regs, r);
            return;
        }
        34 => {
            // pause(): sleep until a signal; always -EINTR.
            let r = native_table_call(34, args, regs); syscall_set_return_value(regs, r);
            return;
        }
        131 => {
            // sigaltstack(ss, old_ss) — x86_64 native number. The args
            // table is GENERIC-numbered, where 131 is tgkill: passing
            // the native number through answered every sigaltstack(2)
            // with sys_tgkill(ESRCH) while the sigaltstack state was
            // never touched (verified in-guest: the handler ran 0 times
            // system-wide across full boots). musl's pthread_exit
            // proceeds after the failed sigaltstack(NULL,&old) and
            // munmaps the old stack using the caller's UNINITIALIZED
            // stack slots as (ss_sp, ss_size) — on the OH image that
            // carved the interpreter's image at 0x3fbf000000 and
            // produced the NOVMA-EXEC storm. Route to the generic
            // sigaltstack slot (132).
            let r = native_table_call(132, args, regs); syscall_set_return_value(regs, r);
            return;
        }
        132 => {
            // utime(path, utimbuf*) — legacy x86_64 ABI (no asm-generic
            // number). Slot 132 in the GENERIC table is sigaltstack:
            // passing the native number through ran sys_sigaltstack with
            // utime's (path, times) as (ss, old_ss) — the SET arm parsed
            // path-string bytes as a SignalStack and installed garbage
            // into the task's sigaltstack state, which a later real
            // sigaltstack(GET) by pthread_exit would munmap. Route to
            // the dedicated free-slot registration (1181).
            let r = native_table_call(1181, args, regs); syscall_set_return_value(regs, r);
            return;
        }
        139 => {
            // sysfs(2): query the registered filesystem list. No
            // asm-generic number (riscv64/aarch64 never issue it);
            // untranslated, 139 would hit the generic table's
            // rt_sigreturn and pop a garbage signal frame (LTP
            // sysfs01..05 "Test killed by SIGSEGV"). Slot 135 is
            // rt_sigprocmask: the arm used to pass sysfs's
            // (option, idx, buf) into it — how=option is a valid SIG_*
            // value, so callers could corrupt their signal mask.
            // Route to the dedicated free-slot registration (1182).
            let r = native_table_call(1182, args, regs); syscall_set_return_value(regs, r);
            return;
        }
        213 => {
            // epoll_create(size): Linux rejects size <= 0 with EINVAL.
            let result = if (args[0] as i64) <= 0 {
                -(errno::EINVAL as i64)
            } else {
                native_table_call(0, args, regs) /* misc::sys_epoll_create */
            };
            syscall_set_return_value(regs, result);
            return;
        }
        235 => {
            // utimes(path, timeval[2]) — microsecond utime(2), no
            // asm-generic number. Untranslated, 235 fell through to
            // generic 235 = mbind; glibc's utimes() on x86_64 issues
            // the native number (LTP utimes01 on x86).
            let r = native_table_call(0, args, regs); /* file::sys_utimes */ syscall_set_return_value(regs, r);
            return;
        }
        261 => {
            // futimesat(dirfd, path, timeval[2]) — no asm-generic
            // number. Untranslated, 261 fell through to generic 261 =
            // prlimit64, whose EINVAL was read as the test outcome by
            // LTP futimesat01 on every x86 run.
            let r = native_table_call(0, args, regs); /* file::sys_futimesat_native */ syscall_set_return_value(regs, r);
            return;
        }
        232 => {
            // epoll_wait(epfd, events, maxevents, timeout_ms) — handler
            // exists, no generic slot (generic uses epoll_pwait).
            let r = native_table_call(0, args, regs); /* misc::sys_epoll_wait */ syscall_set_return_value(regs, r);
            return;
        }
        172 => {
            // iopl(2) — x86_64 native, no asm-generic number. Contract
            // surface (CAP_SYS_RAWIO + level 0..=3): user mode has no
            // IOPL to switch. Unhandled, 172 hit generic getpid and LTP
            // iopl01 read the pid as the return value — first a fake
            // PASS, then ENOSYS after the native-table sweep (a1dce23b7).
            syscall_set_return_value(regs, crate::arch::process::sys_iopl(args));
            return;
        }
        173 => {
            // ioperm(2) — x86_64 native, same story (generic 173 is
            // getppid). Contract surface: CAP_SYS_RAWIO gate + the
            // 0..0x10000 port-space range check.
            syscall_set_return_value(regs, crate::arch::process::sys_ioperm(args));
            return;
        }
        _ => {}
    }

    #[cfg(feature = "x86_64")]
    let syscall_no = match x86_compat::translate(syscall_no as u64, &args) {
        Some((generic_nr, generic_args)) => {
            args = generic_args;
            generic_nr as i64
        }
        // A hole in the reference's own x86_64 table: ENOSYS, never the
        // same-numbered generic syscall (x86 156 _sysctl vs generic
        // getsid — LTP sysctl01/03/04 read ESRCH instead of ENOSYS).
        // Routed to the unmapped-number arm below, which answers ENOSYS.
        None if x86_compat::native_enosys_hole(syscall_no as u64) => {
            u64::MAX as i64
        }
        None => syscall_no as i64,
    };

    // FORENSIC ring ENTRY record (ret sentinel = in-flight): a task
    // parked INSIDE a syscall (epoll_wait/binder ioctl/futex/read on a
    // slow fd) never reaches the exit-side record below, so the exit-only
    // ring showed its last COMPLETED call instead of the blocking one.
    // Also latched into the task itself (last_syscall_*) — the ring is
    // only 2048 entries, so a parked waiter's record is overwritten
    // within milliseconds under system-wide traffic; the taskdump's
    // lastsys= field reads the per-task latch.
    // (perf-syscall lane) Both the ring record and this SECOND latch
    // (post-translate numbers) are hunt instrumentation — 12 atomics
    // per syscall — and now sit behind rux:sysforensic (retired debug=sysforensic). The
    // pre-translate latch above stays always-on: it is what makes a
    // parked task's taskdump show the syscall it is parked IN, and it
    // costs 3 stores.
    if crate::trace::EVENT_SYSFORENSIC.armed() {
        SYSCALL_RING[SYSCALL_CURSOR.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % SYSCALL_RING.len()]
            .record(crate::process::current_pid(), syscall_no as u32, args[0], args[1], u64::MAX);
        if let Some(cur) = crate::sched::current() {
            use core::sync::atomic::Ordering::Relaxed;
            cur.last_syscall_nr.store(syscall_no as u32, Relaxed);
            cur.last_syscall_a0.store(args[0], Relaxed);
            cur.last_syscall_a1.store(args[1], Relaxed);
        }
    }

    // Dispatch based on system call number (sorted by number).
    // unknown_nr: set by the unmapped-number arm below so the
    // dispatch-tail ENOSYS-TRACE can exclude numbers that already
    // carry their own attribution (the keyctl budget lesson).
    let mut unknown_nr = false;
    // FLIP M3-A: table-first dispatch (the service crate's registered
    // fns); the match below is the in-lib fallback (identical bodies).
    if let Some(r) = table_lookup(syscall_no as u32, args, regs) {
        syscall_set_return_value(regs, r);
        return;
    }
    // (The 349-arm in-lib match is DELETED — State B final: the
    // service crate is the sole implementation; its register_all
    // published the table the lookup above consults. Unregistered
    // numbers reach the budgeted ENOSYS attribution below.)
    unknown_nr = true;
            unknown_nr = true;
            // Budgeted PER-NR (8 lines each, 16 distinct numbers, 400
            // lines total), WITH the caller's pid+comm. The keyctl
            // lesson (ub2404-gaps): native 250 landed here 57x/boot
            // from systemd's per-service KeyringMode=private session
            // join — unbudgeted it flooded the console, and its volume
            // ate the dispatch-tail ENOSYS-TRACE budget before
            // journald's write-path ENOSYS ever printed; without the
            // comm the line could not be attributed to any caller.
            use core::sync::atomic::Ordering::Relaxed;
            static UNK_NR: [core::sync::atomic::AtomicU32; 16] =
                [const { core::sync::atomic::AtomicU32::new(0) }; 16];
            static UNK_N: [core::sync::atomic::AtomicUsize; 16] =
                [const { core::sync::atomic::AtomicUsize::new(0) }; 16];
            static UNK_TOTAL: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let mut slot = 16;
            for i in 0..16 {
                let s = UNK_NR[i].load(Relaxed);
                if s == syscall_no as u32 {
                    slot = i;
                    break;
                }
                if s == 0
                    && UNK_NR[i]
                        .compare_exchange(0, syscall_no as u32, Relaxed, Relaxed)
                        .is_ok()
                {
                    slot = i;
                    break;
                }
            }
            let budget_ok = slot < 16
                && UNK_N[slot].fetch_add(1, Relaxed) < 8
                && UNK_TOTAL.fetch_add(1, Relaxed) < 400;
            if budget_ok {
                let comm = match crate::sched::current() {
                    Some(t) => {
                        // SAFETY: sched::current() returns the running task;
                        // comm_str is the lossy NUL-aware decoder.
                        t.comm_str()
                    }
                    None => alloc::borrow::Cow::Borrowed("?"),
                };
                crate::println!(
                    "syscall: unknown syscall {} (args: {:#x}, {:#x}, {:#x}) pid={} comm={}",
                    syscall_no, args[0], args[1], args[2],
                    crate::process::current_pid(),
                    comm
                );
            }
    let result: i64 = -(errno::ENOSYS as i64);

    // EFAULTTRAC (sleep-EFAULT hunt): python's time.sleep surfaces EFAULT
    // from some syscall, but not from any of the instrumented sleep paths.
    // Name every syscall that returns exactly -EFAULT so the culprit is
    // identifiable from the console. Rate-limited.
    //
    // RETIRED (probe-retire): the trace was unconditional (37 lines per
    // ub2404 boot). It now lives behind the existing OH hunt-print
    // master gate (rux:ohhunt (retired debug=ohhunt)); the switch is checked only on the
    // EFAULT return path, so the hot path is untouched.
    if result == -(errno::EFAULT as i64)
        && crate::debug::switches::oh_hunt_prints_on()
    {
        static EFT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if EFT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 60 {
            crate::pr_info!(
                "EFAULTTRAC: pid={} nr={} a0={:#x} a1={:#x}",
                crate::process::current_pid(), syscall_no, args[0], args[1]
            );
        }
    }

    // ub2404 bring-up: budgeted ENOSYS tracer -- every syscall that
    // answers ENOSYS gets one line (pid, generic nr, first two args).
    // journald's "Failed to map main seqnum file: Function not
    // implemented" needs exactly this attribution; the generic nr maps
    // through syscall/x86_compat.rs comments.
    // ub2404-gaps blind-spot fix: numbers that hit the unknown-syscall
    // arm are EXCLUDED (they carry their own per-nr budgeted
    // attribution above) — keyctl's 57 joins/boot used to eat this
    // whole budget before the implemented-but-ENOSYS families printed.
    //
    // RETIRED (probe-retire): unconditional (budget 200 lines/boot);
    // behind the existing OH hunt-print master gate (rux:ohhunt (retired debug=ohhunt)).
    if result == -(38i64)
        && !unknown_nr
        && crate::debug::switches::oh_hunt_prints_on()
    {
        static ENOSYS_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if ENOSYS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 200 {
            crate::pr_info!(
                "ENOSYS-TRACE: pid={} nr={} a0={:#x} a1={:#x}",
                crate::process::current_pid(),
                syscall_no,
                args[0],
                args[1]
            );
        }
    }

    // STRACE1 (loader hunts, rux:strace1 (retired debug=strace1)): one line per file/memory
    // syscall of PID 1, with the pathname decoded for path-taking calls.
    // musl ld.so's "Error loading shared library" carries no errno — this
    // turns that into "which step failed" (search-open, fstat, mmap,
    // header read).
    if crate::trace::EVENT_STRACE1.armed()
        && crate::process::current_pid() == 1
    {
        strace1_line(syscall_no as u32, &args, result);
    }

    syscall_set_return_value(regs, result);

    // STEPTRACE (rux:steptrace (retired debug=steptrace)): one line per syscall return. Left
    // ungated this flooded the emulated UART (2.7GB per OH boot) and the
    // printing alone slowed services enough to lose races — see the
    // switch catalog note in debug/switches.rs.
    if crate::trace::EVENT_STEPTRACE.armed() {
        crate::pr_debug!("syscall: pid={}, nr={}, ret={:#x} ({})",
            crate::process::current_pid(), syscall_no, result,
            if result < 0 { "error" } else { "ok" });
    }

    // 50th (crash-storm hunt): latch first-execve (failed-spawn
    // forensics — see Task::did_exec and the do_exit printer). One
    // integer compare on the exec numbers only.
    if syscall_no == 59 || syscall_no == 322 {
        if let Some(cur) = crate::sched::current() {
            cur.did_exec.store(true, core::sync::atomic::Ordering::Relaxed);
        }
    }

    // 50th (crash-storm hunt): per-task ring of address-space edits for
    // the USEGV forensics (see Task::mm_ops for the encoding and the
    // mallocng hole-write family it adjudicates). mmap(9)/mprotect(10)/
    // munmap(11)/brk(12)/mremap(25)/madvise(28) — same numbers on every
    // 64-bit arch we ship (asm-generic table). madvise joined the ring
    // in the hp89 hunt (op 6): MADV_DONTNEED/FREE/REMOVE zap live PTEs
    // by design, so a covering DONTNEED is the userspace-owned face of
    // the zeroed-mallocng-metadata signature and must be visible at #GP
    // time. Also ungated by design: a handful of Relaxed atomics on six
    // syscalls; the fault printer must see these on quiet boots.
    match syscall_no {
        9 | 10 | 11 | 12 | 25 | 28 => {
            if let Some(cur) = crate::sched::current() {
                use core::sync::atomic::Ordering::Relaxed;
                let op: u64 = match syscall_no {
                    9 => 1,
                    10 => 2,
                    11 => 3,
                    12 => 4,
                    28 => 6,
                    _ => 5,
                };
                let rc: u64 = if result < 0 && result > -4096 {
                    ((-(result)) as u64) & 0xff
                } else {
                    0
                };
                // User addresses are < 2^48; hints above that are ignored
                // by the kernel anyway, so truncation only loses evidence
                // of an already-rejected request.
                let w0 = ((args[0] & 0x0000_ffff_ffff_ffff) << 16) | (rc << 8) | op;
                let prot: u64 = if syscall_no == 9 || syscall_no == 10 {
                    args[2] & 0xff
                } else if syscall_no == 28 {
                    // madvise: pack the advice into the prot field slot.
                    args[2] & 0xff
                } else {
                    0
                };
                let w1 = (args[1] & 0x0000_ffff_ffff_ffff) | (prot << 48);
                let idx = (*cur).mm_ops_cur.fetch_add(1, Relaxed) % 16;
                (*cur).mm_ops[idx * 2].store(w0, Relaxed);
                (*cur).mm_ops[idx * 2 + 1].store(w1, Relaxed);
            }
        }
        _ => {}
    }

    // (perf-syscall lane) Everything from here to the end of the
    // function — the FORENSIC ring exit record, the per-task
    // last_calls/init_calls/birth rings, the fail-tail ring and the
    // sc-series comm-gated hooks (appioctl/applog/appconn/prctl-einval/
    // missstat/openmiss) — is hunt instrumentation costing ~15 atomics
    // plus comm scans on EVERY syscall. Gated behind rux:sysforensic (retired debug=sysforensic);
    // quiet boots pay one atomic load.
    if !crate::trace::EVENT_SYSFORENSIC.armed() {
        return;
    }

    // FORENSIC ring: last N syscalls per boot (pid, nr, a0, a1, ret).
    // Same rux:sysring (retired debug=sysring) gate as the entry-side record — see above.
    if crate::trace::EVENT_SYSRING.armed() {
        SYSCALL_RING[SYSCALL_CURSOR.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % SYSCALL_RING.len()]
            .record(crate::process::current_pid(), syscall_no as u32, args[0], args[1], result as u64);
    }

    // sc128 (Session P): per-task nr-only ring of the last syscalls
    // (successes included) — read at APP-PARK to see the child's final
    // action sequence, not just its failures. sc135 (Session Q): ring
    // deepened 12 -> LAST_CALLS_LEN (96) — sc129's 12-deep ring saw only
    // the clock_gettime retry loop masking the real pre-park sequence.
    if let Some(cur) = crate::sched::current() {
        use core::sync::atomic::Ordering::Relaxed;
        let idx = cur.last_calls_cur.fetch_add(1, Relaxed)
            % crate::process::task::LAST_CALLS_LEN;
        cur.last_calls[idx].store((syscall_no as u32) & 0xffff, Relaxed);
    }

    // sc138 (Session Q): FILTERED init ring — skips the parked child's
    // retry-loop noise ({113 clock_gettime, 66 writev, 203 connect} and
    // main-stack-top futex waits) so APP-PARK / APP-THR dumps carry the
    // init sequence itself (loader/prctl/mprotect/clone cascade) instead
    // of a clock_gettime wall. This is the working-vs-parked byte-diff
    // substrate: park dumps show the aborted init; APP-THR dumps show a
    // full one that reached pthread_create.
    {
        let skip = match syscall_no {
            113 | 66 | 203 => true,
            // futex: sc138 showed the park loop also spins FUTEX WAITS on
            // the runtime-arena words (0x7ff00002aec0 family, short
            // timeouts) — those flooded the init ring as badly as the
            // clocks. sc139: skip ALL futex WAITs (op FUTEX_WAIT(0) /
            // FUTEX_WAIT_BITSET(9), with or without PRIVATE(0x80)) in the
            // init ring; WAKEs and requeues stay recorded — a wake is a
            // worker-handshake event, exactly what the diff needs. The
            // final park wait is named by the APP-PARK header itself.
            98 => {
                let op = (args[1] & 0x7f) as u32;
                op == 0 /* FUTEX_WAIT */ || op == 9 /* FUTEX_WAIT_BITSET */
                    || args[0] >= 0x7ffffffd0000 && args[0] < 0x7ffffffff000
            }
            _ => false,
        };
        if !skip {
            if let Some(cur) = crate::sched::current() {
                use core::sync::atomic::Ordering::Relaxed;
                let n = cur.init_calls_cur.fetch_add(1, Relaxed);
                let idx = n % crate::process::task::INIT_CALLS_LEN;
                cur.init_calls[idx].store((syscall_no as u32) & 0xffff, Relaxed);
                cur.init_a0[idx].store(args[0], Relaxed);
                // sc143: latch the birth window once (first 64 filtered
                // calls of this task's life) — the deep ring's tail gets
                // churned by retry loops, the birth window cannot.
                if n < crate::process::task::BIRTH_CALLS_LEN as usize {
                    cur.birth_calls[n].store((syscall_no as u32) & 0xffff, Relaxed);
                    cur.birth_a0[n].store(args[0], Relaxed);
                }
            }
        }
    }

    // sc125 (Session P): per-task ring of the last 8 FAILING syscalls —
    // the park decode needs the pre-park failure tail (musl pthread_create
    // mmaps the thread stack BEFORE clone; a failed mmap means no clone
    // ever reaches the kernel census and nothing else records it).
    // sc135 (Session Q): a0/a1 stored alongside — the nr167=-22 entries
    // need the prctl OPTION named.
    if result < 0 && result > -4096 {
        if let Some(cur) = crate::sched::current() {
            let packed = ((-(result) as u32) << 16) | ((syscall_no as u32) & 0xffff);
            let idx = cur.last_fails_cur.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % 8;
            cur.last_fails[idx].store(packed, core::sync::atomic::Ordering::Relaxed);
            cur.last_fails_a0[idx].store(args[0], core::sync::atomic::Ordering::Relaxed);
            cur.last_fails_a1[idx].store(args[1], core::sync::atomic::Ordering::Relaxed);
        }
    }

    // 50th (crash-storm hunt): latch first-execve — do_exit prints the
    // birth syscall window of process leaders that die WITHOUT exec'ing
    // (the failed-spawn family; see Task::did_exec). NOTE: this and the
    // mm_ops ring below were moved BEFORE the sysforensic early-return
    // (see the "(perf-syscall lane)" block further down): the fault
    // printer and the exit printer must see them on QUIET boots too.
    // sc140 (Session Q): the parked child's retry loop runs ioctl x3-5
    // between dlopen attempts — name the FD and COMMAND (binder
    // BINDER_WRITE_READ? /dev/xpm? a param-area ioctl?). The fd set at
    // park time has no xpm/dec fd, so the loop polls an open service fd.
    if syscall_no == 29 {
        let comm = crate::sched::current()
            .map(|t| t.comm())
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        let family = comm.contains("launcher")
            || comm.contains("settingsdata")
            || comm.contains("systemui")
            || comm.contains("dataability")
            || comm == "appspawn";
        if family {
            static IO_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if IO_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 80 {
                crate::pr_warn!(
                    "appioctl: pid={} comm='{}' fd={} cmd={:#x} ret={}",
                    crate::process::current_pid(),
                    comm,
                    args[0],
                    args[1],
                    result
                );
            }
        }
    }

    // sc144-149: the parked children's OWN hilog lines never reach
    // hilogd (socket EAGAIN backpressure) — yet they are the narration
    // of exactly where the runtime init stalls. Dump the first 3 iovecs
    // of every writev to fd 3 from the app family (daemon excluded):
    // sc149 showed the child's sandbox-phase narration; the post-sandbox
    // runtime phase writes SUCCEED and need printing too (print every
    // result, budget 700).
    if syscall_no == 66 && args[0] == 3 {
        let comm = crate::sched::current()
            .map(|t| t.comm())
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        let family = comm.contains("launcher")
            || comm.contains("settingsdata")
            || comm.contains("systemui")
            || comm.contains("dataability")
            || comm == "appspawn"
            || comm.contains("foundation")
            || comm.contains("sa_main");
        // rux:writetrace (retired debug=writetrace): the fd3 hilog writev capture settled the
        // chain questions (sandbox narration, SA-501 attempts); in
        // milestone boots it only adds ~2.4k kernel lines for
        // hilogd.rd_kmsg to copy (the b128/b129 vCPU lesson).
        if family
            && crate::trace::EVENT_WRITETRACE.armed()
        {
            // sc157: comm-based gating. The sc155/156 pid latch (first
            // family task to write fd3 = "the daemon") was fragile — in
            // sc156 an earlier appspawn-family writer latched a
            // non-daemon pid and the daemon's preload storm (ImplClass/
            // APPSPAWN/MMG/...) printed UNFILTERED, eating 1062 of the
            // 1200 budget before the launcher phase. Gate by comm
            // instead: 'appspawn' (daemon AND pre-rename children — the
            // sandbox-phase narration was decoded in Session Q already),
            // foundation and sa_main are keyword-gated; renamed children
            // print everything.
            let keyword_daemon = comm == "appspawn"
                || comm.contains("foundation")
                || comm.contains("sa_main");
            // sc152 (Session Q): for the DAEMON itself, print only the
            // SA-registration lines — the SA 501 (AppMgrService,
            // libappms loaded into the daemon) never registers with
            // samgr, and the daemon's own narration says whether the
            // registration was attempted and how it failed.
            // sc155 (Session R): BUDGET FIX — only lines that PASS the
            // print filter consume budget.
            static HI_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if HI_N.load(core::sync::atomic::Ordering::Relaxed) < 2400 {
                // iovec { void* base; size_t len }
                let base = args[1];
                if base != 0 {
                    // sc145: OH's hilog writev carries [header(bin), tag,
                    // text] as SEPARATE iovecs — walk the first 3.
                    let mut parts: alloc::vec::Vec<alloc::string::String> =
                        alloc::vec::Vec::new();
                    for vi in 0..3u64 {
                        let vp = base + vi * 16;
                        let vp_h = crate::arch::uaccess_safe::UserSlice::new(vp as usize, 16);
                        let iobase = vp_h.read_at::<u64>(0);
                        let iolen = vp_h.read_at::<u64>(8);
                        if let (Some(b), Some(l)) = (iobase, iolen) {
                            if l > 0 && l < 0x10000 {
                                let n = (l as usize).min(110);
                                let mut buf = [0u8; 110];
                                let ok = crate::arch::uaccess_safe::UserSlice::new(
                                    b as usize, n)
                                    .copy_in(&mut buf[..n]) == 0;
                                if ok {
                                    let text: alloc::string::String = buf[..n]
                                        .iter()
                                        .map(|c| {
                                            if (0x20..=0x7f).contains(c) || *c == b'\n' {
                                                *c as char
                                            } else {
                                                '.'
                                            }
                                        })
                                        .collect();
                                    parts.push(text);
                                }
                            }
                        }
                    }
                    if !parts.is_empty() {
                        let joined = parts.join("|");
                        // sc156: the forked children's ArkCompiler GC
                        // chatter ("[gc] AddRegion/RemoveRegion/...")
                        // burned ~700 of the 1200 budget lines in sc155
                        // before the launcher phase — engine noise, drop.
                        let gc_noise = joined.contains("ArkCompiler.|[gc]");
                        // sc155: the print filter decides FIRST; only
                        // passing lines consume the budget (children
                        // always pass; daemon/foundation/sa_main need a
                        // registration keyword).
                        let daemon_regn = keyword_daemon
                            && (joined.contains("SystemAbility")
                                || joined.contains("501")
                                || joined.contains("amgr")
                                || joined.contains("SAM"));
                        if !gc_noise && (!keyword_daemon || daemon_regn) {
                            if HI_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 2400 {
                                crate::pr_warn!(
                                    "applog pid={} comm='{}' ret={} |{}",
                                    crate::process::current_pid(),
                                    comm,
                                    result,
                                    joined
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // sc136 (Session Q): the parked children's pre-park loop is
    // {clock_gettime xN, writev, connect, writev} — the connect TARGET
    // names the service the child talks to right before parking (hilog?
    // samgr? the appspawn engine?). sc136 run 1: the appspawn PARENT's
    // own connect storm ate the whole budget before the first child
    // forked, and fd 3's sockaddr is an ABSTRACT unix socket (sun_path
    // starts NUL — the empty-string parse printed nothing). Fix: skip
    // the boot's original appspawn pid (first comm='appspawn' task seen;
    // forked children carry the same comm but a different pid), and
    // hex-dump the first 16 sockaddr bytes for abstract sockets.
    if syscall_no == 203 {
        let cur_task = crate::sched::current();
        let comm = cur_task
            .map(|t| t.comm())
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        let is_app = comm.contains("launcher")
            || comm.contains("settingsdata")
            || comm.contains("systemui")
            || comm.contains("dataability")
            || comm == "appspawn";
        if is_app {
            static APPSPAWN_MAIN_PID: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let my_pid = crate::process::current_pid() as u32;
            let parent_pid = APPSPAWN_MAIN_PID.load(core::sync::atomic::Ordering::Relaxed);
            let skip = if comm == "appspawn" {
                if parent_pid == 0 {
                    APPSPAWN_MAIN_PID.store(my_pid, core::sync::atomic::Ordering::Relaxed);
                    true // this first one IS the parent — skip but remember it
                } else {
                    parent_pid == my_pid
                }
            } else {
                false
            };
            if !skip {
                static CONN_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if CONN_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 60 {
                    let mut sa = [0u8; 48];
                    use core::fmt::Write;
                    let mut desc = alloc::string::String::new();
                    if args[1] != 0 {
                        // sc141: RAW copy — strncpy_from_user stops at the
                        // first NUL, so an abstract sockaddr printed as
                        // "all zeros" when its real name started later.
                        let raw_ok = crate::arch::uaccess_safe::copy_in(
                            &mut sa, args[1] as usize) == 0;
                        if raw_ok {
                            let fam = u16::from_ne_bytes([sa[0], sa[1]]);
                            if fam == 1 {
                                let p = &sa[2..];
                                let end = p.iter().position(|c| *c == 0).unwrap_or(p.len());
                                if end == 0 {
                                    // abstract socket: the name is the
                                    // raw bytes after the leading NUL
                                    let _ = write!(desc, "abstract:");
                                    // sc140: 40 bytes + the caller's
                                    // addrlen — the name may be longer
                                    // than the old 18-byte window and
                                    // NUL bytes are legal in abstract
                                    // names (length IS the name).
                                    for b in sa.iter().take(40) {
                                        let _ = write!(desc, "{:02x}", b);
                                    }
                                } else {
                                    let _ = write!(
                                        desc,
                                        "{}",
                                        core::str::from_utf8(&p[..end]).unwrap_or("<utf8>")
                                    );
                                }
                            } else {
                                let _ = write!(desc, "fam{}", fam);
                            }
                        } else {
                            let _ = write!(desc, "<efault>");
                        }
                    } else {
                        let _ = write!(desc, "<null>");
                    }
                    crate::pr_warn!(
                        "appconn: pid {} comm='{}' fd={} -> {} addrlen={} ret={}",
                        my_pid,
                        comm,
                        args[0],
                        desc,
                        args[2],
                        result
                    );
                }
            }
        }
    }

    // sc135 (Session Q): name the prctl OPTION behind the parked app
    // children's nr167=-22 fail-tail entries (1-2 per child). The
    // unimplemented-option warning inside sys_prctl has a 32-line budget
    // that hdf_devhost retry loops eat long before the app window, and
    // EINVAL also leaves IMPLEMENTED arms (PR_SET_VMA sub-op, securebits
    // mask > 0xff, capbset > CAP_LAST_CAP, PR_CAP_AMBIENT sub-op) which
    // print nothing. Catch every prctl-EINVAL at dispatch exit with all
    // five args, scoped to the app family, budget 40.
    if syscall_no == 167 && result == -22 {
        let comm = crate::sched::current()
            .map(|t| t.comm())
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        let is_app = comm.contains("launcher")
            || comm.contains("settingsdata")
            || comm.contains("systemui")
            || comm.contains("dataability")
            || comm == "appspawn";
        if is_app {
            static PRCTL_EI_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if PRCTL_EI_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
                crate::pr_warn!(
                    "prctl-einval: pid {} comm='{}' option={} a1={:#x} a2={:#x} a3={:#x} a4={:#x}",
                    crate::process::current_pid(),
                    comm,
                    args[0],
                    args[1],
                    args[2],
                    args[3],
                    args[4]
                );
            }
        }
    }

    // OH bring-up: name the call behind any ENOTDIR — the hilogd
    // service-socket chmod fails with errno 20 and no in-handler trace
    // sees it, so decode it here (pathname arg for the *at family).
    // NOTE the label: this hook fires for BOTH shapes — real ENOTDIR
    // (result -20) and ENOENT (result -2) on stat-family numbers
    // [53,79,90] — and prints "missstat:" with the actual return value.
    // The old constant "ENOTDIR:" label made correct ENOENT returns on
    // missing musl-namespace paths ("/vendor/lib64/chipset-sdk-sp",
    // "/dev/watchdog" before the node existed) read as an errno-20 bug
    // across three independent investigations (b151/wdg1-wdg4; resolved
    // by shimming the nr-79 arm — zero hits while the hook printed).
    if (result == -20 || (result == -2
            && [53u64, 79, 90].contains(&(syscall_no as u64))))
        && crate::process::current_pid() >= 300
    {
        static ENOTDIR_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if ENOTDIR_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
            let mut pb = [0u8; 96];
            let path = if [257u64, 48, 79, 263, 264, 53, 54].contains(&(syscall_no as u64)) && args[1] != 0 {
                crate::arch::uaccess::strncpy_from_user(
                    args[1] as *const u8, pb.len(), &mut pb)
                    .map(|s| core::str::from_utf8(s).unwrap_or("?"))
                    .unwrap_or("<efault>")
            } else { "-" };
            crate::pr_info!(
                "missstat: ret={} nr={} a0={:#x} path='{}' a2={:#x} pid={}",
                result,
                syscall_no,
                args[0],
                path,
                args[2],
                crate::process::current_pid()
            );
        }
    }

    // b26: open/openat ENOENT with its own budget — the missstat hook's
    // 40 lines are gone long before the appspawn children pivot, and the
    // sandbox-side "/dev/null" ENOENT (appspawn_common.c:350 'open
    // dev_null error: 2') needs the failing path + the task's root
    // context to diagnose the in-sandbox walk.
    // sc126 (Session P): THREE fixes — (1) the nr test checked the RAW
    // x86_64 257, but this exit-side hook sees the TRANSLATED generic
    // number (openat=56), so openat misses never printed; (2) pr_info
    // was filtered at boot loglevel=4 — now pr_warn; (3) scoped to
    // app/appspawn comms with a 120 budget — the children's pre-park
    // fail tail (APP-PARK fails:) is 6x openat ENOENT, and the paths
    // name exactly which sandbox files the runtime needs and misses.
    if result == -2
        && (syscall_no as u64 == 2 || syscall_no as u64 == 56 || syscall_no as u64 == 257)
    {
        let comm_ok = crate::sched::current()
            .map(|t| t.comm())
            .map(|c| {
                let s = core::str::from_utf8(&c[..]).unwrap_or("").trim_end_matches('\0');
                s.contains("launcher")
                    || s.contains("settingsdata")
                    || s.contains("systemui")
                    || s.contains("dataability")
            })
            .unwrap_or(false);
        if comm_ok {
            static OPEN_ENOENT_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if OPEN_ENOENT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 200 {
                let mut pb = [0u8; 96];
                let parg = if syscall_no as u64 != 2 { args[1] } else { args[0] };
                let path = if parg != 0 {
                    crate::arch::uaccess::strncpy_from_user(
                        parg as *const u8, pb.len(), &mut pb)
                        .map(|s| core::str::from_utf8(s).unwrap_or("?"))
                        .unwrap_or("<efault>")
                } else { "-" };
                // Root context: a pivoted child resolves against its jail.
                let root = crate::sched::current()
                    .and_then(|t| t.fs_arc())
                    .map(|fs| fs.get_root())
                    .unwrap_or_default();
                let root_str = core::str::from_utf8(&root).unwrap_or("?");
                crate::pr_warn!(
                    "openmiss: path='{}' root='{}' pid={}",
                    path,
                    root_str,
                    crate::process::current_pid()
                );
            }
        }
    }
}

// ============================================================================
// x86_64 native->generic translation (moved from syscall/x86_compat.rs —
// State B final: the SEAM owns the number translation; the service crate
// keeps its copy for the sys_x86_* arms it still serves).
// ============================================================================
#[cfg(feature = "x86_64")]
pub mod x86_compat {
    use super::SyscallArgs;
    // The const table moved WITH translate (x86_compat's own locals).
    const AT_FDCWD: u64 = -100i64 as u64;
    const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
    const AT_SYMLINK_FOLLOW: u64 = 0x400;
    const AT_REMOVEDIR: u64 = 0x200;
    const SIGCHLD: u64 = 17;
    const CLONE_VM: u64 = 0x100;
    const CLONE_VFORK: u64 = 0x4000;
    const O_CREAT_WRONLY_TRUNC: u64 = 0x241;
    pub fn translate(nr: u64, args: &SyscallArgs) -> Option<(u64, SyscallArgs)> {
        let [a0, a1, a2, _a3, _a4, _a5] = *args;
        match nr {
            0 => Some((63, *args)), // read
            1 => Some((64, *args)), // write
            2 => Some((56, [AT_FDCWD, a0, a1, a2, 0, 0])), // open
            3 => Some((57, *args)), // close
            4 => Some((79, [AT_FDCWD, a0, a1, 0, 0, 0])), // stat
            5 => Some((80, *args)), // fstat
            6 => Some((79, [AT_FDCWD, a0, a1, AT_SYMLINK_NOFOLLOW, 0, 0])), // lstat
            8 => Some((62, *args)), // lseek
            9 => Some((222, *args)), // mmap
            10 => Some((226, *args)), // mprotect
            11 => Some((215, *args)), // munmap
            12 => Some((214, *args)), // brk
            13 => Some((134, *args)), // rt_sigaction
            14 => Some((135, *args)), // rt_sigprocmask
            15 => Some((139, *args)), // rt_sigreturn
            16 => Some((29, *args)), // ioctl
            17 => Some((67, *args)), // pread64
            18 => Some((68, *args)), // pwrite64
            19 => Some((65, *args)), // readv
            20 => Some((66, *args)), // writev
            21 => Some((48, [AT_FDCWD, a0, a1, 0, 0, 0])), // access
            22 => Some((59, [a0, 0, 0, 0, 0, 0])), // pipe
            24 => Some((124, *args)), // sched_yield
            25 => Some((216, *args)), // mremap
            26 => Some((227, *args)), // msync
            27 => Some((232, *args)), // mincore
            28 => Some((233, *args)), // madvise
            // SysV shm: x86_64 has direct calls (no ipc() multiplexer), same
            // argument order as asm-generic — pure number remaps. Without
            // these, shmget(29) fell through to generic 29 (ioctl → ENOTTY).
            29 => Some((194, *args)), // shmget
            30 => Some((196, *args)), // shmat
            31 => Some((195, *args)), // shmctl
            32 => Some((23, *args)), // dup
            // 33 dup2: handled by the dispatch's x86_64 legacy block (needs the
            // oldfd==newfd special case that dup3 rejects with EINVAL).
            // 34 pause: handled by the dispatch's x86_64 legacy block (no
            // asm-generic number exists; needs the pause sleep loop).
            35 => Some((101, *args)), // nanosleep
            36 => Some((102, *args)), // getitimer
            38 => Some((103, *args)), // setitimer
            39 => Some((172, *args)), // getpid
            40 => Some((71, *args)), // sendfile
            41 => Some((198, *args)), // socket
            42 => Some((203, *args)), // connect
            43 => Some((202, *args)), // accept
            44 => Some((206, *args)), // sendto
            45 => Some((207, *args)), // recvfrom
            46 => Some((211, *args)), // sendmsg
            47 => Some((212, *args)), // recvmsg
            48 => Some((210, *args)), // shutdown
            49 => Some((200, *args)), // bind
            50 => Some((201, *args)), // listen
            51 => Some((204, *args)), // getsockname
            52 => Some((205, *args)), // getpeername
            53 => Some((199, *args)), // socketpair
            54 => Some((208, *args)), // setsockopt
            55 => Some((209, *args)), // getsockopt
            56 => Some((220, *args)), // clone
            57 => Some((220, [SIGCHLD, 0, 0, 0, 0, 0])), // fork
            58 => Some((220, [CLONE_VM | CLONE_VFORK | SIGCHLD, 0, 0, 0, 0, 0])), // vfork
            59 => Some((221, *args)), // execve
            60 => Some((93, *args)), // exit
            61 => Some((260, *args)), // wait4
            62 => Some((129, *args)), // kill
            63 => Some((160, *args)), // uname
            // SysV sem: same ABI, number remap only. Without these, semget(64)
            // fell through to generic 64 (write → EBADF on the key-as-fd).
            64 => Some((190, *args)), // semget
            65 => Some((193, *args)), // semop
            66 => Some((191, *args)), // semctl
            // SysV msg: without these, msgget(68) fell through to generic 68
            // (pwrite64 → EBADF) — the LTP x86-r2 N2 family (45+ tests).
            67 => Some((197, *args)), // shmdt
            68 => Some((186, *args)), // msgget
            69 => Some((189, *args)), // msgsnd
            70 => Some((188, *args)), // msgrcv
            71 => Some((187, *args)), // msgctl
            72 => Some((25, *args)), // fcntl
            73 => Some((32, *args)), // flock
            74 => Some((82, *args)), // fsync
            75 => Some((83, *args)), // fdatasync
            76 => Some((45, *args)), // truncate
            77 => Some((46, *args)), // ftruncate
            79 => Some((17, *args)), // getcwd
            80 => Some((49, *args)), // chdir
            81 => Some((50, *args)), // fchdir
            82 => Some((38, [AT_FDCWD, a0, AT_FDCWD, a1, 0, 0])), // rename
            83 => Some((34, [AT_FDCWD, a0, a1, 0, 0, 0])), // mkdir
            84 => Some((35, [AT_FDCWD, a0, AT_REMOVEDIR, 0, 0, 0])), // rmdir
            85 => Some((56, [AT_FDCWD, a0, O_CREAT_WRONLY_TRUNC, a1, 0, 0])), // creat
            86 => Some((37, [AT_FDCWD, a0, AT_FDCWD, a1, 0, 0])), // link
            87 => Some((35, [AT_FDCWD, a0, 0, 0, 0, 0])), // unlink
            88 => Some((36, [a0, AT_FDCWD, a1, 0, 0, 0])), // symlink
            89 => Some((78, [AT_FDCWD, a0, a1, a2, 0, 0])), // readlink
            90 => Some((53, [AT_FDCWD, a0, a1, 0, 0, 0])), // chmod
            91 => Some((52, *args)), // fchmod
            92 => Some((54, [AT_FDCWD, a0, a1, a2, 0, 0])), // chown
            93 => Some((55, *args)), // fchown
            94 => Some((54, [AT_FDCWD, a0, a1, a2, AT_SYMLINK_NOFOLLOW, 0])), // lchown
            95 => Some((166, *args)), // umask
            96 => Some((169, *args)), // gettimeofday
            97 => Some((163, *args)), // getrlimit
            98 => Some((165, *args)), // getrusage
            99 => Some((179, *args)), // sysinfo
            100 => Some((153, *args)), // times
            // ptrace: was missing → generic 101 (nanosleep) EFAULTed on the
            // request register (LTP x86-r2 ptrace03 "EFAULT").
            101 => Some((117, *args)), // ptrace
            102 => Some((174, *args)), // getuid
            103 => Some((116, *args)), // syslog
            104 => Some((176, *args)), // getgid
            105 => Some((146, *args)), // setuid
            106 => Some((144, *args)), // setgid
            107 => Some((175, *args)), // geteuid
            108 => Some((177, *args)), // getegid
            109 => Some((154, *args)), // setpgid
            110 => Some((173, *args)), // getppid
            111 => Some((155, [0, 0, 0, 0, 0, 0])), // getpgrp = getpgid(0)
            112 => Some((157, *args)), // setsid
            113 => Some((145, *args)), // setreuid
            114 => Some((143, *args)), // setregid
            115 => Some((158, *args)), // getgroups
            116 => Some((159, *args)), // setgroups
            117 => Some((147, *args)), // setresuid
            118 => Some((148, *args)), // getresuid
            119 => Some((149, *args)), // setresgid
            120 => Some((150, *args)), // getresgid
            121 => Some((155, *args)), // getpgid
            122 => Some((151, *args)), // setfsuid
            123 => Some((152, *args)), // setfsgid
            124 => Some((156, *args)), // getsid
            125 => Some((90, *args)), // capget
            126 => Some((91, *args)), // capset
            127 => Some((136, *args)), // rt_sigpending
            128 => Some((137, *args)), // rt_sigtimedwait
            129 => Some((138, *args)), // rt_sigqueueinfo
            130 => Some((133, *args)), // rt_sigsuspend
            131 => Some((132, *args)), // sigaltstack
            133 => Some((33, [AT_FDCWD, a0, a1, a2, 0, 0])), // mknod
            135 => Some((92, *args)), // personality
            137 => Some((43, *args)), // statfs
            138 => Some((44, *args)), // fstatfs
            140 => Some((141, *args)), // getpriority
            141 => Some((140, *args)), // setpriority
            142 => Some((118, *args)), // sched_setparam
            143 => Some((121, *args)), // sched_getparam
            144 => Some((119, *args)), // sched_setscheduler
            145 => Some((120, *args)), // sched_getscheduler
            146 => Some((125, *args)), // sched_get_priority_max
            147 => Some((126, *args)), // sched_get_priority_min
            148 => Some((127, *args)), // sched_rr_get_interval
            149 => Some((228, *args)), // mlock
            150 => Some((229, *args)), // munlock
            151 => Some((230, *args)), // mlockall
            152 => Some((231, *args)), // munlockall
            153 => Some((58, *args)), // vhangup
            155 => Some((41, *args)), // pivot_root
            157 => Some((167, *args)), // prctl
            159 => Some((171, *args)), // adjtimex
            160 => Some((164, *args)), // setrlimit
            161 => Some((51, *args)), // chroot
            162 => Some((81, *args)), // sync
            163 => Some((89, *args)), // acct
            164 => Some((170, *args)), // settimeofday
            165 => Some((40, *args)), // mount
            166 => Some((39, *args)), // umount2
            167 => Some((224, *args)), // swapon
            168 => Some((225, *args)), // swapoff
            169 => Some((142, *args)), // reboot
            170 => Some((161, *args)), // sethostname
            171 => Some((162, *args)), // setdomainname
            175 => Some((105, *args)), // init_module
            176 => Some((106, *args)), // delete_module
            179 => Some((60, *args)), // quotactl
            180 => Some((42, *args)), // nfsservctl
            186 => Some((178, *args)), // gettid
            187 => Some((213, *args)), // readahead
            188 => Some((5, *args)), // setxattr
            189 => Some((6, *args)), // lsetxattr
            190 => Some((7, *args)), // fsetxattr
            191 => Some((8, *args)), // getxattr
            192 => Some((9, *args)), // lgetxattr
            193 => Some((10, *args)), // fgetxattr
            194 => Some((11, *args)), // listxattr
            195 => Some((12, *args)), // llistxattr
            196 => Some((13, *args)), // flistxattr
            197 => Some((14, *args)), // removexattr
            198 => Some((15, *args)), // lremovexattr
            199 => Some((16, *args)), // fremovexattr
            200 => Some((130, *args)), // tkill
            202 => Some((98, *args)), // futex
            203 => Some((122, *args)), // sched_setaffinity
            204 => Some((123, *args)), // sched_getaffinity
            206 => Some((0, *args)), // io_setup
            207 => Some((1, *args)), // io_destroy
            208 => Some((4, *args)), // io_getevents
            209 => Some((2, *args)), // io_submit
            210 => Some((3, *args)), // io_cancel
            212 => Some((18, *args)), // lookup_dcookie
            216 => Some((234, *args)), // remap_file_pages
            217 => Some((61, *args)), // getdents64
            218 => Some((96, *args)), // set_tid_address
            219 => Some((128, *args)), // restart_syscall
            220 => Some((192, *args)), // semtimedop
            221 => Some((223, *args)), // fadvise64
            222 => Some((107, *args)), // timer_create
            223 => Some((110, *args)), // timer_settime
            224 => Some((108, *args)), // timer_gettime
            225 => Some((109, *args)), // timer_getoverrun
            226 => Some((111, *args)), // timer_delete
            227 => Some((112, *args)), // clock_settime
            228 => Some((113, *args)), // clock_gettime
            229 => Some((114, *args)), // clock_getres
            // time(2): return value = seconds, optional tloc store — NOT
            // shape-compatible with gettimeofday (buffer layout + return
            // semantics differ), so it gets its own internal generic slot.
            // Without this, static glibc's time() returns -1 forever (no
            // vDSO) — ohlite's second-stage never computed a nonzero uptime.
            201 => Some((1180, *args)), // time
            230 => Some((115, *args)), // clock_nanosleep
            231 => Some((94, *args)), // exit_group
            233 => Some((21, *args)), // epoll_ctl
            234 => Some((131, *args)), // tgkill
            237 => Some((235, *args)), // mbind
            238 => Some((237, *args)), // set_mempolicy
            239 => Some((236, *args)), // get_mempolicy
            // POSIX mq block: was missing → generic 240-245 dispatched
            // rt_tgsigqueueinfo/perf_event_open/recvmmsg/fanotify/… instead.
            240 => Some((180, *args)), // mq_open
            241 => Some((181, *args)), // mq_unlink
            242 => Some((182, *args)), // mq_timedsend
            243 => Some((183, *args)), // mq_timedreceive
            244 => Some((184, *args)), // mq_notify
            245 => Some((185, *args)), // mq_getsetattr
            246 => Some((104, *args)), // kexec_load
            247 => Some((95, *args)), // waitid
            // Key-retention block: was missing → keyctl (250) surfaced as an
            // unknown-syscall ENOSYS from every systemd-spawned service
            // (KeyringMode=private joins a session keyring at spawn — 57
            // lines/boot on ub2404), and add_key/request_key probes from
            // libkeyutils/LTP TCONF'd on the ENOSYS.
            248 => Some((217, *args)), // add_key
            249 => Some((218, *args)), // request_key
            250 => Some((219, *args)), // keyctl
            251 => Some((30, *args)), // ioprio_set
            252 => Some((31, *args)), // ioprio_get
            253 => Some((26, [0, 0, 0, 0, 0, 0])), // inotify_init = inotify_init1(0)
            254 => Some((27, *args)), // inotify_add_watch
            255 => Some((28, *args)), // inotify_rm_watch
            256 => Some((238, *args)), // migrate_pages
            257 => Some((56, *args)), // openat
            258 => Some((34, *args)), // mkdirat
            259 => Some((33, *args)), // mknodat
            260 => Some((54, *args)), // fchownat
            262 => Some((79, *args)), // newfstatat
            263 => Some((35, *args)), // unlinkat
            264 => Some((38, *args)), // renameat
            265 => Some((37, *args)), // linkat
            266 => Some((36, *args)), // symlinkat
            267 => Some((78, *args)), // readlinkat
            268 => Some((53, *args)), // fchmodat
            269 => Some((48, *args)), // faccessat
            270 => Some((72, *args)), // pselect6
            271 => Some((73, *args)), // ppoll
            272 => Some((97, *args)), // unshare
            273 => Some((99, *args)), // set_robust_list
            274 => Some((100, *args)), // get_robust_list
            275 => Some((76, *args)), // splice
            276 => Some((77, *args)), // tee
            277 => Some((84, *args)), // sync_file_range
            278 => Some((75, *args)), // vmsplice
            279 => Some((239, *args)), // move_pages
            280 => Some((88, *args)), // utimensat
            281 => Some((22, *args)), // epoll_pwait
            282 => Some((74, [a0, a1, a2, 0, 0, 0])), // signalfd = signalfd4(…, flags=0)
            283 => Some((85, *args)), // timerfd_create
            285 => Some((47, *args)), // fallocate
            286 => Some((86, *args)), // timerfd_settime
            287 => Some((87, *args)), // timerfd_gettime
            288 => Some((242, *args)), // accept4
            289 => Some((74, *args)), // signalfd4
            290 => Some((19, *args)), // eventfd2
            284 => Some((19, [a0, 0, 0, 0, 0, 0])), // eventfd = eventfd2(init, flags=0)
            291 => Some((20, *args)), // epoll_create1
            292 => Some((24, *args)), // dup3
            293 => Some((59, *args)), // pipe2
            294 => Some((26, *args)), // inotify_init1
            295 => Some((69, *args)), // preadv
            296 => Some((70, *args)), // pwritev
            297 => Some((240, *args)), // rt_tgsigqueueinfo
            298 => Some((241, *args)), // perf_event_open
            299 => Some((243, *args)), // recvmmsg
            300 => Some((262, *args)), // fanotify_init
            301 => Some((263, *args)), // fanotify_mark
            302 => Some((261, *args)), // prlimit64
            303 => Some((264, *args)), // name_to_handle_at
            304 => Some((265, *args)), // open_by_handle_at
            305 => Some((266, *args)), // clock_adjtime
            306 => Some((267, *args)), // syncfs
            307 => Some((269, *args)), // sendmmsg
            308 => Some((268, *args)), // setns
            309 => Some((168, *args)), // getcpu
            310 => Some((270, *args)), // process_vm_readv
            311 => Some((271, *args)), // process_vm_writev
            312 => Some((272, *args)), // kcmp
            313 => Some((273, *args)), // finit_module
            314 => Some((274, *args)), // sched_setattr
            315 => Some((275, *args)), // sched_getattr
            316 => Some((276, *args)), // renameat2
            317 => Some((277, *args)), // seccomp
            318 => Some((278, *args)), // getrandom
            319 => Some((279, *args)), // memfd_create
            320 => Some((294, *args)), // kexec_file_load
            321 => Some((280, *args)), // bpf
            322 => Some((281, *args)), // execveat
            323 => Some((282, *args)), // userfaultfd
            324 => Some((283, *args)), // membarrier
            325 => Some((284, *args)), // mlock2
            326 => Some((285, *args)), // copy_file_range
            327 => Some((286, *args)), // preadv2
            328 => Some((287, *args)), // pwritev2
            329 => Some((288, *args)), // pkey_mprotect
            330 => Some((289, *args)), // pkey_alloc
            331 => Some((290, *args)), // pkey_free
            332 => Some((291, *args)), // statx
            333 => Some((292, *args)), // io_pgetevents
            334 => Some((293, *args)), // rseq
            424 => Some((424, *args)), // pidfd_send_signal: same nr
            425 => Some((425, *args)), // io_uring_setup: same nr
            426 => Some((426, *args)), // io_uring_enter: same nr
            427 => Some((427, *args)), // io_uring_register: same nr
            428 => Some((428, *args)), // open_tree: same nr
            429 => Some((429, *args)), // move_mount: same nr
            430 => Some((430, *args)), // fsopen: same nr
            431 => Some((431, *args)), // fsconfig: same nr
            432 => Some((432, *args)), // fsmount: same nr
            433 => Some((433, *args)), // fspick: same nr
            434 => Some((434, *args)), // pidfd_open: same nr
            435 => Some((435, *args)), // clone3: same nr
            436 => Some((436, *args)), // close_range: same nr
            437 => Some((437, *args)), // openat2: same nr
            438 => Some((438, *args)), // pidfd_getfd: same nr
            439 => Some((439, *args)), // faccessat2: same nr
            440 => Some((440, *args)), // process_madvise: same nr
            441 => Some((441, *args)), // epoll_pwait2: same nr
            442 => Some((442, *args)), // mount_setattr: same nr
            443 => Some((443, *args)), // quotactl_fd: same nr
            444 => Some((444, *args)), // landlock_create_ruleset: same nr
            445 => Some((445, *args)), // landlock_add_rule: same nr
            446 => Some((446, *args)), // landlock_restrict_self: same nr
            447 => Some((447, *args)), // memfd_secret: same nr
            448 => Some((448, *args)), // process_mrelease: same nr
            449 => Some((449, *args)), // futex_waitv: same nr
            450 => Some((450, *args)), // set_mempolicy_home_node: same nr
            451 => Some((451, *args)), // cachestat: same nr
            452 => Some((452, *args)), // fchmodat2: same nr
            453 => Some((453, *args)), // map_shadow_stack: same nr
            454 => Some((454, *args)), // futex_wake: same nr
            455 => Some((455, *args)), // futex_wait: same nr
            456 => Some((456, *args)), // futex_requeue: same nr
            457 => Some((457, *args)), // statmount: same nr
            458 => Some((458, *args)), // listmount: same nr
            459 => Some((459, *args)), // lsm_get_self_attr: same nr
            460 => Some((460, *args)), // lsm_set_self_attr: same nr
            461 => Some((461, *args)), // lsm_list_modules: same nr
            _ => None,
        }
    }
    
    pub fn native_enosys_hole(nr: u64) -> bool {
        // Reference-x86_64 sys_ni slots: the number exists in the ABI but
        // the syscall was removed or never wired — the reference answers
        // ENOSYS. Left unmapped, these fall through to the same-numbered
        // GENERIC syscall, which is a DIFFERENT call:
        //   156 _sysctl → generic getsid (ESRCH; LTP sysctl01/03/04)
        //   134 uselib → rt_sigaction; 174 create_module → getuid;
        //   177 get_kernel_syms → getegid; 178 query_module → gettid;
        //   181/182 getpmsg/putpmsg → mq_unlink/mq_timedsend;
        //   183 afs_syscall / 184 tuxcall / 185 security → mq_timedreceive/
        //   mq_notify/mq_getsetattr; 214/215 epoll_{ctl,wait}_old →
        //   brk/munmap (!); 236 vserver → get_mempolicy.
        if matches!(
            nr,
            134 | 156 | 174 | 177 | 178 | 181 | 182 | 183 | 184 | 185 | 214 | 215
                | 236
        ) {
            return true;
        }
        // Implemented on the reference x86_64 but not here. No 64-bit
        // Ubuntu caller issues these natives (glibc-2.39 uses arch_prctl
        // for TLS; there is no LDT consumer), and the same-numbered
        // generic rows are DIFFERENT calls — 154 modify_ldt → setpgid,
        // 205 set_thread_area → getpeername, 211 get_thread_area →
        // sendmsg — so ENOSYS (the honest "not implemented") is safer
        // than the silent wrong-syscall fallthrough. Recorded as an
        // honest gap: implement properly if an LTP/compat case ever
        // needs them. 172 iopl / 173 ioperm lived here too until the
        // LTP ioperm01/iopl01 rows demanded the real contract surface
        // (arch/x86_64/process.rs + the dispatch pre-translate arms —
        // the sweep note's "implement properly if an LTP case ever
        // needs them" case arrived).
        // 136 ustat: the modern toolchain has no <sys/ustat.h>, so LTP
        // ustat01/02 TCONF on the reference too; generic 136 is
        // rt_sigpending.
        matches!(nr, 136 | 154 | 205 | 211)
    }
    
}
