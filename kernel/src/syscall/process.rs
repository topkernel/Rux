//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Process-related system calls
//!
//! Includes: clone, execve, exit, wait4, getpid, getppid, kill, set_tid_address, uname, etc.

use super::*;
use crate::arch::mm::{phys_to_virt, PhysAddr};
use crate::arch::uaccess::strncpy_from_user;
use crate::process::exec::do_execve_elf;

/// sys_clone - Create child process/thread
///
/// RISC-V 64 ABI (Linux arch/riscv kernel/sys_riscv.c, musl __clone):
/// - args[0]: flags - clone flags
/// - args[1]: stack - new stack pointer
/// - args[2]: parent_tid - parent TID pointer (CLONE_PARENT_SETTID)
/// - args[3]: child_tid - child TID pointer (CLONE_CHILD_SETTID/CLEARTID)
/// - args[4]: tls - TLS pointer (CLONE_SETTLS → child tp register)
///
/// (The old code had a3/a4 swapped — pthread_create passed its TLS value
/// as child_tid and the tid address as TLS, breaking every musl thread.)
///
/// # Returns
/// Returns child process PID in parent, 0 in child, negative error code on failure
/// Heap headroom a task-creating clone must leave for the rest of the
/// kernel (F12 thread-storm guard — see sys_clone). 8 MB ≈ 60 kernel
/// stacks of slack: enough for exit-path bookkeeping of a mass unwind
/// plus normal kernel traffic, small against the 128 MB heap.
pub const CLONE_HEAP_RESERVE_BYTES: usize = 8 << 20;

/// Whether the kernel heap is too low to safely create another task.
pub fn clone_heap_reserve_low() -> bool {
    let st = crate::mm::buddy_allocator::buddy_stats();
    st.heap_size > 0 && st.free_bytes < CLONE_HEAP_RESERVE_BYTES
}

pub fn sys_clone(args: SyscallArgs) -> i64 {

    use crate::process::fork::{do_clone, CloneArgs};

    // F12 heap-reserve gate: a thread storm (LTP pth_str02 spawns until
    // failure) eats the 128 MB kernel heap with 128 KB kernel stacks;
    // once the heap truly empties, the stack allocation fails cleanly
    // but every FOLLOW-UP small allocation on the unwind path (280-byte
    // BTree/Vec nodes) hit alloc_error_handler and PANICKED the kernel.
    // Linux refuses clone with ENOMEM before exhausting the last of its
    // allocator (musl maps that to EAGAIN for pthread_create callers —
    // the graceful "unable to create more threads" path).
    //
    // sc116 probe: every OH app child (launcher/settingsdata/systemui)
    // ends up SINGLE-THREADED with the main thread parked in a private
    // stack futex — the shape of a runtime whose FIRST pthread_create
    // failed silently and now waits forever for a worker that was never
    // born. If the 1 GiB OH boot drains the buddy heap under the 8 MiB
    // reserve mid-boot (the fake-OOM family), this gate refuses EVERY
    // thread from then on with no trace. Name each refusal (budgeted):
    // errno + free_bytes + comm is one line and settles gate-vs-other.
    if clone_heap_reserve_low() {
        static N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
            let st = crate::mm::buddy_allocator::buddy_stats();
            let comm = crate::sched::current()
                .map(|t| unsafe { (*t).comm() })
                .map(|c| core::str::from_utf8(c).unwrap_or(""))
                .unwrap_or("")
                .trim_end_matches('\0');
            crate::pr_warn!(
                "clone-gate: ENOMEM comm='{}' free={}/{} bytes",
                comm,
                st.free_bytes,
                st.heap_size
            );
        }
        return -(errno::ENOMEM as i64);
    }

    let flags = args[0];
    // Legacy clone takes `int flags` in every libc wrapper; a flag with
    // bit 31 set (CLONE_IO 0x80000000) arrives sign-extended to
    // 0xffffffff_80000000. Linux's SYSCALL_DEFINE5(clone) builds
    // kernel_clone_args with lower_32_bits(clone_flags) — the upper word
    // is DISCARDED, never validated (only clone3 rejects bad high bits).
    // kcmp03's clone(CLONE_IO|SIGCHLD) was refused EINVAL before the mask.
    let flags = flags & 0xffff_ffff;
    let stack = args[1];
    let parent_tid = args[2] as *mut i32;
    // clone syscall argument ORDER differs by arch (verified against
    // toolchain/musl-1.2.5 clone.s for both):
    //
    // - riscv64: musl passes (flags, stack, ptid, TLS, CTID) — a2=ptid,
    //   a3=tls, a4=ctid, and the riscv Linux ABI adopted that order.
    //
    // - x86_64: the STANDARD order (flags, stack, ptid, CTID, TLS):
    //   musl's wrapper moves ctid into r10 (arg 4) and tls into r8
    //   (arg 5). Reading the riscv order here programmed fsbase with
    //   the CHILD-TID POINTER, so every pthread's `mov %fs:0x0,%rbp`
    //   loaded the tid integer as the TCB self-pointer and the next
    //   `0x40(%rbp)` access faulted at tid+0x40 (observed: fcntl34_64
    //   threads faulting at 0x40 and 0x181 with tid 0x141 — the X3
    //   thread-crash family).
    // aarch64 raw clone order matches riscv64: (flags, sp, ptid, TLS, ctid)
    #[cfg(any(feature = "riscv64", feature = "aarch64"))]
    let (tls, child_tid) = (args[3], args[4] as *mut i32);
    #[cfg(feature = "x86_64")]
    let (tls, child_tid) = (args[4], args[3] as *mut i32);

    // Legacy clone: the low byte of `flags` is the exit signal (Linux
    // copy_process: p->exit_signal = args->exit_signal == CSIGNAL mask).
    let exit_signal = (flags & 0xff) as u8;

    // sc135 (Session Q): the appspawn parent's tgid, recorded at its
    // first child fork so the census-side APP-THR dump can EXCLUDE the
    // parent's own thread batches (OS_GC_Thread pool, comm 'appspawn')
    // and keep only forked-children thread creations — the working-shape
    // init ring the divergence comparison needs.
    static APPSPAWN_TGID: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);

    // sc123 (Session P): APP-PARK decoded the children's eternal wait as
    // pthread_cond_timedwait's MUTEX RELOCK (return chain: wait wrapper <-
    // pthread_mutex_lock <- pthread_cond_timedwait at musl+0x11849d) on
    // a stack-local mutex at 0x7fffffffc754 whose word reads 2 (locked +
    // waiters) in a SINGLE-THREADED child. Two theories: (A) fork-inherited
    // held lock — an appspawn parent-side task held this mutex when fork()
    // copied the main stack, and no child thread can ever unlock it; (B)
    // the child locked it itself and re-locks on an error path Rux takes
    // but Linux does not. This probe fires AT FORK (clone without
    // CLONE_VM/CLONE_THREAD from comm appspawn) in the forking thread's
    // own context: it reads BOTH candidate words on the forking stack and
    // lists every sibling thread of the tgid (comm + state + last syscall
    // nr) — the siblings awake at fork instant are the theory-A holder
    // candidates. word==1/2 at fork settles A outright.
    if flags & 0x00000100 == 0 /* !CLONE_VM: real fork, stack copied */
        && flags & 0x00010000 == 0 /* !CLONE_THREAD */
    {
        let comm = crate::sched::current()
            .map(|t| unsafe { (*t).comm() })
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        if comm == "appspawn" {
            if APPSPAWN_TGID.load(core::sync::atomic::Ordering::Relaxed) == 0 {
                if let Some(t) = crate::sched::current() {
                    // SAFETY: t is the current task.
                    unsafe {
                        APPSPAWN_TGID.store(
                            (*t).tgid() as u32,
                            core::sync::atomic::Ordering::Relaxed,
                        );
                    }
                }
            }
            static NF: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if NF.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 10 {
                use crate::arch::uaccess::get_user;
                let w1 = unsafe { get_user(0x7fffffffc754 as *const u32) };
                let w2 = unsafe { get_user(0x7fffffffc7f4 as *const u32) };
                crate::pr_warn!(
                    "forkgate: appspawn fork flags={:#x} word@c754={:?} word@c7f4={:?}",
                    flags,
                    w1,
                    w2
                );
                // Sibling snapshot: who else of this tgid is alive at
                // fork, and what are they doing (holder candidates).
                let my_pid = crate::process::current_pid();
                let listed_cell = core::cell::Cell::new(0usize);
                crate::sched::for_each_task(|t| {
                    if listed_cell.get() >= 12 {
                        return;
                    }
                    let tk = unsafe { &*t };
                    if tk.tgid() != my_pid || tk.pid() == my_pid {
                        return;
                    }
                    let nc = core::str::from_utf8(tk.comm())
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    let nr = tk.last_syscall_nr.load(core::sync::atomic::Ordering::Relaxed);
                    let running = tk.state().is_running();
                    crate::pr_warn!(
                        "forkgate:  sib tid {} comm='{}' lastsys={} {}",
                        tk.pid(),
                        nc,
                        nr,
                        if running { "RUNNING" } else { "sleep" }
                    );
                    listed_cell.set(listed_cell.get() + 1);
                });
            }
        }
    }

    let clone_args = CloneArgs {
        flags,
        stack,
        parent_tid,
        child_tid,
        tls,
        exit_signal,
    };

    // sc118 decode (Session P): the sc117-kernel boot with the refusal
    // probes produced ZERO clone-gate / clone-fail lines while app
    // children (os.settingsdata, telephonydataability) STILL parked
    // single-threaded on the stack futex 0x7fffffffc754 — no clone
    // FAILED, so either no clone was ever ATTEMPTED by these runtimes,
    // or the worker clone SUCCEEDED and the thread died before doing
    // its work. This census names every clone attempt (success or
    // failure) from the app-child binaries, budgeted 80 lines: with it,
    // "no attempt" (userspace never reaches pthread_create) separates
    // from "success then instant death" (worker fault/exit).
    {
        let comm = crate::sched::current()
            .map(|t| unsafe { (*t).comm() })
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        let is_app = comm.contains("launcher")
            || comm.contains("settingsdata")
            || comm.contains("systemui")
            || comm.contains("dataability");
        // sc124 (Session P): the comm filter was BLIND for the pre-rename
        // window — an appspawn child keeps comm 'appspawn' until late
        // (dlopen of the engine + its .init_array constructors run under
        // the OLD name; sc118 caught pid 1200 comm=appspawn RUNNING as a
        // fresh child). The park decode (pthread_cond_timedwait on a
        // stack cond, PRIVATE futex — same-process signaler required)
        // makes the constructor-phase thread creation the prime suspect:
        // on Linux engine constructors create the worker that later
        // signals this cond; whether that clone HAPPENS on Rux was never
        // measured. Log clones from comm 'appspawn' too — the flags
        // field separates the parent's fork (SIGCHLD, no CLONE_VM) from
        // the child's engine thread creation (CLONE_VM|CLONE_THREAD).
        let is_spawn = comm == "appspawn";
        if is_app || is_spawn {
            static N3: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if N3.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 120 {
                return match do_clone(clone_args) {
                    Ok(pid) => {
                        crate::pr_warn!(
                            "appclone: comm='{}' flags={:#x} -> pid {}",
                            comm,
                            flags,
                            pid
                        );
                        // sc140: let the binder tracer see transactions
                        // targeting forked appspawn children.
                        if flags & 0x00000100 == 0 && flags & 0x00010000 == 0 {
                            crate::ipc::binder::register_app_child_pid(pid as u32);
                        }
                        // sc139 (Session Q): the WORKING reference — the
                        // appspawn daemon's own filtered init ring, dumped
                        // in the forking thread's context at a real fork
                        // (no CLONE_VM). The daemon reaches pthread_create
                        // and runs its whole loader cascade; APP-PARK shows
                        // the parked child's ring. The diff of the two is
                        // the divergence hunt. Budget 2 (first two forks).
                        if flags & 0x00000100 == 0 {
                            use core::fmt::Write;
                            use core::sync::atomic::Ordering::Relaxed;
                            static NPAR: core::sync::atomic::AtomicUsize =
                                core::sync::atomic::AtomicUsize::new(0);
                            if NPAR.fetch_add(1, Relaxed) < 2 {
                                if let Some(t) = crate::sched::current() {
                                    unsafe {
                                        const RL: usize =
                                            crate::process::task::INIT_CALLS_LEN;
                                        let cur = (*t).init_calls_cur.load(Relaxed);
                                        let mut s2 = alloc::string::String::new();
                                        let _ = write!(
                                            s2,
                                            "APP-PAR: tgid {} comm='{}' init:",
                                            (*t).tgid(),
                                            core::str::from_utf8((*t).comm())
                                                .unwrap_or("")
                                                .trim_end_matches('\0')
                                        );
                                        for k in 0..RL {
                                            let i = (cur + RL - k) % RL;
                                            let v = (*t).init_calls[i].load(Relaxed);
                                            if v != 0 {
                                                let _ = write!(s2, " {}", v);
                                            }
                                        }
                                        crate::pr_warn!("{}", s2);
                                        let mut s3 = alloc::string::String::new();
                                        let _ = write!(s3, "APP-PAR: initA:");
                                        for k in 0..RL {
                                            let i = (cur + RL - k) % RL;
                                            let v = (*t).init_calls[i].load(Relaxed);
                                            if v != 0 {
                                                let a0 = (*t).init_a0[i].load(Relaxed);
                                                let _ = write!(s3, " {}@{:#x}", v, a0);
                                            }
                                        }
                                        crate::pr_warn!("{}", s3);
                                    }
                                }
                            }
                        }
                        // sc135 (Session Q): a forked appspawn child that
                        // CREATES a thread is the WORKING shape (sc125:
                        // tgid 1146 -> OS_IPC_0 + OS_DfxWatchdog, alive)
                        // while its siblings park single-threaded before
                        // their first pthread_create. Dump the CREATING
                        // thread's full init ring (all-calls + fails, same
                        // format as APP-PARK) so the two sequences can be
                        // diffed — the divergence point is the abort.
                        // Parent's own batches excluded via APPSPAWN_TGID.
                        if flags & 0x00010000 != 0 /* CLONE_THREAD */
                            && APPSPAWN_TGID.load(core::sync::atomic::Ordering::Relaxed) != 0
                        {
                            if let Some(t) = crate::sched::current() {
                                // SAFETY: t is the current (forking) task.
                                unsafe {
                                    let spawn_tgid = APPSPAWN_TGID
                                        .load(core::sync::atomic::Ordering::Relaxed);
                                    if (*t).tgid() as u32 != spawn_tgid {
                                        static NT: core::sync::atomic::AtomicUsize =
                                            core::sync::atomic::AtomicUsize::new(0);
                                        if NT.fetch_add(
                                            1,
                                            core::sync::atomic::Ordering::Relaxed,
                                        ) < 6
                                        {
                                            use core::fmt::Write;
                                            const RL: usize =
                                                crate::process::task::LAST_CALLS_LEN;
                                            let cur = (*t)
                                                .last_calls_cur
                                                .load(core::sync::atomic::Ordering::Relaxed);
                                            let mut s2 = alloc::string::String::new();
                                            let _ = write!(
                                                s2,
                                                "APP-THR: tgid {} comm='{}' leader='{}' calls:",
                                                (*t).tgid(),
                                                core::str::from_utf8((*t).comm())
                                                    .unwrap_or("")
                                                    .trim_end_matches('\0'),
                                                core::str::from_utf8(
                                                    (*(*t).group_leader_ptr()).comm()
                                                )
                                                .unwrap_or("")
                                                .trim_end_matches('\0')
                                            );
                                            for k in 0..RL {
                                                let i = (cur + RL - k) % RL; // newest first
                                                let v = (*t).last_calls[i]
                                                    .load(core::sync::atomic::Ordering::Relaxed);
                                                if v != 0 {
                                                    let _ = write!(s2, " {}", v);
                                                }
                                            }
                                            crate::pr_warn!("{}", s2);
                                            let cur8 = (*t)
                                                .last_fails_cur
                                                .load(core::sync::atomic::Ordering::Relaxed);
                                            let mut s = alloc::string::String::new();
                                            let _ = write!(s, "APP-THR: tgid {} fails:", (*t).tgid());
                                            for k in 0..8 {
                                                let i = (cur8 + 8 - 1 - k) % 8;
                                                let v = (*t).last_fails[i]
                                                    .load(core::sync::atomic::Ordering::Relaxed);
                                                if v != 0 {
                                                    let a0 = (*t).last_fails_a0[i]
                                                        .load(core::sync::atomic::Ordering::Relaxed);
                                                    let a1 = (*t).last_fails_a1[i]
                                                        .load(core::sync::atomic::Ordering::Relaxed);
                                                    let _ = write!(
                                                        s,
                                                        " nr{}=-{} a0={:#x} a1={:#x}",
                                                        v & 0xffff,
                                                        v >> 16,
                                                        a0,
                                                        a1
                                                    );
                                                }
                                            }
                                            crate::pr_warn!("{}", s);
                                            // sc138 (Session Q): the FILTERED
                                            // init ring for the WORKING side —
                                            // this creator reached
                                            // pthread_create; APP-PARK dumps
                                            // the parked side. The diff
                                            // between the two sequences is
                                            // the abort point.
                                            {
                                                use core::sync::atomic::Ordering::Relaxed;
                                                const RL: usize =
                                                    crate::process::task::INIT_CALLS_LEN;
                                                let icur = (*t)
                                                    .init_calls_cur
                                                    .load(Relaxed);
                                                let mut s4 = alloc::string::String::new();
                                                let _ = write!(
                                                    s4,
                                                    "APP-THR: tgid {} init:",
                                                    (*t).tgid()
                                                );
                                                for k in 0..RL {
                                                    let i = (icur + RL - k) % RL;
                                                    let v = (*t).init_calls[i]
                                                        .load(Relaxed);
                                                    if v != 0 {
                                                        let _ = write!(s4, " {}", v);
                                                    }
                                                }
                                                crate::pr_warn!("{}", s4);
                                                let mut s5 = alloc::string::String::new();
                                                let _ = write!(
                                                    s5,
                                                    "APP-THR: tgid {} initA:",
                                                    (*t).tgid()
                                                );
                                                for k in 0..32.min(RL) {
                                                    let i = (icur + RL - k) % RL;
                                                    let v = (*t).init_calls[i]
                                                        .load(Relaxed);
                                                    if v != 0 {
                                                        let a0 = (*t).init_a0[i]
                                                            .load(Relaxed);
                                                        let _ =
                                                            write!(s5, " {}@{:#x}", v, a0);
                                                    }
                                                }
                                                crate::pr_warn!("{}", s5);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        // Parent-namespace view, same as the main return.
                        let parent_ns = crate::sched::current().and_then(|t| unsafe { (*t).ns_pid.clone() });
                        crate::process::ns::translate_sender_pid_ns(pid, parent_ns.as_ref()) as i64
                    }
                    Err(e) => {
                        crate::pr_warn!(
                            "appclone: comm='{}' flags={:#x} -> errno {}",
                            comm,
                            flags,
                            -(e as i32)
                        );
                        e as i64
                    }
                };
            }
        }
    }

    // fork returns the child's pid in the PARENT's pid namespace (a
    // container task's fork must hand back the ns-local pid — LTP pidns17
    // forks 10 children in-namespace and waitpid()s them by that value;
    // the global pid made every wait fail ECHILD).
    match do_clone(clone_args) {
        Ok(pid) => {
            let parent_ns = crate::sched::current().and_then(|t| unsafe { (*t).ns_pid.clone() });
            crate::process::ns::translate_sender_pid_ns(pid, parent_ns.as_ref()) as i64
        }
        Err(e) => {
            // sc116 probe (see the reserve-gate comment): any other
            // clone failure is equally invisible to the taskdump — the
            // caller's runtime often ignores pthread_create errors and
            // parks on a rendezvous that never completes.
            static N2: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if N2.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 40 {
                let comm = crate::sched::current()
                    .map(|t| unsafe { (*t).comm() })
                    .map(|c| core::str::from_utf8(c).unwrap_or(""))
                    .unwrap_or("")
                    .trim_end_matches('\0');
                crate::pr_warn!("clone-fail: errno {} comm='{}'", -(e as i32), comm);
            }
            e as i64
        }
    }
}

/// Maximum shebang line length
const BINPRM_BUF_SIZE: usize = 256;

/// Maximum recursion depth for shebang interpretation
const MAX_BINPRM_RECURSION: u32 = 4;

/// Parse shebang (#!) line from file data.
/// Returns (interpreter_path, optional_argument) if present.
///
/// Mirrors the reference kernel's fs/binfmt_script.c load_script():
/// - the interpreter line is the first BINPRM_BUF_SIZE bytes, terminated
///   at the first '\n' or at the buffer end (a 255-byte line with no
///   newline is still a valid interpreter line, not an error);
/// - trailing ' '/'\t' of the line are trimmed;
/// - the interpreter name is the first word; the OPTIONAL ARGUMENT is
///   the ENTIRE rest of the line after the separating whitespace run —
///   including embedded spaces (`#!/usr/bin/env -S python3 -u` must
///   hand env the single argument "-S python3 -u"; env -S then splits
///   it). Truncating at the next whitespace dropped every word after
///   the first, breaking the `env -S` script convention.
/// - the whitespace class is ' ' and '\t' only (binfmt_script's checks
///   are exactly these two bytes; a broader trim also diverges on
///   exotic-but-legal interpreter paths).
pub(crate) fn parse_shebang(data: &[u8]) -> Option<(&str, Option<&str>)> {
    if data.len() < 2 || data[0] != b'#' || data[1] != b'!' {
        return None;
    }

    let window = BINPRM_BUF_SIZE.min(data.len());
    let raw = if let Some(rel) = data[2..window].iter().position(|&c| c == b'\n') {
        &data[2..2 + rel]
    } else {
        // No newline in the window: the line runs to the buffer end —
        // same as binfmt_script (cp = buf + BINPRM_BUF_SIZE - 1).
        &data[2..window]
    };

    // binfmt_script first trims trailing spaces/tabs of the whole line.
    let mut line_end = raw.len();
    while line_end > 0 && (raw[line_end - 1] == b' ' || raw[line_end - 1] == b'\t') {
        line_end -= 1;
    }
    let line = core::str::from_utf8(&raw[..line_end]).ok()?;

    // Skip leading spaces/tabs after "#!".
    let line = line.trim_start_matches([' ', '\t']);

    // Interpreter name = first word; optional argument = the WHOLE rest.
    match line.find([' ', '\t']) {
        Some(space_pos) => {
            let interp = &line[..space_pos];
            let rest = line[space_pos..].trim_start_matches([' ', '\t']);
            if interp.is_empty() {
                return None;
            }
            Some((interp, if rest.is_empty() { None } else { Some(rest) }))
        }
        None => {
            if line.is_empty() {
                None
            } else {
                Some((line, None))
            }
        }
    }
}

/// Read file content from filesystem (ext4 or rootfs)
fn read_exec_file(path: &str) -> Option<alloc::vec::Vec<u8>> {
    // VFS-first (OH Phase 2): the raw-ext4 shortcut below resolves
    // against the ROOT ext4 superblock only — executables on other
    // mounts (/vendor/bin/hdf_devhost for every display *_host service)
    // came back ENOENT even though the exec pre-flight path_lookup
    // succeeded. Open through the VFS (any mount) and read in chunks.
    if let Ok(fd) = crate::fs::vfs::file_open(path, 0 /* O_RDONLY */, 0) {
        let file = unsafe { crate::fs::file::get_file_fd(fd) };
        if let Some(file) = file {
            let mut data = alloc::vec::Vec::new();
            let mut chunk = [0u8; 16384];
            loop {
                let n = unsafe { file.read(chunk.as_mut_ptr(), chunk.len()) };
                if n <= 0 {
                    break;
                }
                data.extend_from_slice(&chunk[..n as usize]);
                if (n as usize) < chunk.len() {
                    break;
                }
            }
            // SAFETY: fd came from file_open on the current task's table.
            let _ = unsafe { crate::fs::file::close_file_fd(fd) };
            if !data.is_empty() {
                return Some(data);
            }
        } else {
            // SAFETY: fd came from file_open on the current task's table.
            let _ = unsafe { crate::fs::file::close_file_fd(fd) };
        }
    }
    if crate::fs::ext4::is_mounted() {
        crate::fs::ext4::read_file_from_mounted(path)
    } else {
        crate::fs::read_file_from_rootfs(path)
    }
}

/// Open `path` through the VFS and return the file as an Arc WITHOUT
/// keeping an fd (Linux vm_file semantics): the Arc outlives any closed fd
/// number, so pinning it on the exec-time segment VMAs gives
/// /proc/<pid>/maps a stable path identity even though exec holds no
/// descriptor for the exe or the interpreter. Delegates to the fd-free VFS
/// resolver, which also works in the PID-1 boot exec where no fdtable
/// exists yet (the old file_open-based body returned None there — PID 1's
/// maps lost the path column on every boot). Best-effort: None simply
/// leaves the VMAs without a pinned file (offset bookkeeping stands).
pub(crate) fn open_file_arc(path: &str) -> Option<alloc::sync::Arc<crate::fs::file::File>> {
    crate::fs::vfs::open_file_arc_nofd(path)
}

/// One-resolution exec file handle (abi-review session 1): a single
/// `path_lookup` yields the inode for the MAY_EXEC gate, the setuid
/// stat bits, the content read, AND the VMA-pinning File Arc. The old
/// ladder walked the path FOUR times per exec (pre-flight lookup,
/// read_exec_file's internal file_open, stat_file_by_path,
/// open_file_arc) plus twice more for a dynamic interpreter — the
/// EXECCOST decomposition measured ~3 us of string build + ~35 us per
/// walk on the p4_fork_exec shape.
pub(crate) struct ExecFile {
    /// Kernel-internal read-only File on the resolved inode (no fdtable
    /// slot, invisible to inotify — Linux get_empty_filp semantics).
    pub file: alloc::sync::Arc<crate::fs::file::File>,
    /// st_mode bits (S_ISUID/S_ISGID/x-bit checks without re-statting).
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    /// Inode size, used to presize the content read buffer.
    pub size: u64,
}

/// Resolve an exec target with ONE path walk. Error shapes match the
/// reference kernel's do_open_execat: walk errors propagate verbatim
/// (ENOENT/ENOTDIR/ENAMETOOLONG/EACCES...), a directory is EACCES
/// (may_open: S_ISDIR), missing execute permission is EACCES (even
/// root needs one x bit — LTP execve03).
pub(crate) fn resolve_exec_file(path: &str) -> Result<ExecFile, i32> {
    let vpath = crate::fs::vfs::path_lookup(path, 0)?;
    let inode = vpath
        .inode
        .clone()
        .or_else(|| vpath.dentry.as_ref().and_then(|d| d.get_inode()))
        .ok_or(-(errno::ENOENT as i32))?;
    if inode.mode.is_directory() {
        return Err(-(errno::EACCES as i32));
    }
    if !crate::fs::permission::inode_permission(&inode, crate::fs::permission::MAY_EXEC) {
        return Err(-(errno::EACCES as i32));
    }
    // SAFETY: vpath came from a successful path_lookup; construction is
    // the fd-free kernel open (same as open_file_arc_nofd).
    let file = unsafe { crate::fs::vfs::open_file_arc_from_vpath(&vpath) }
        .ok_or(-(errno::EACCES as i32))?;
    // ETXTBSY (Linux deny_write_access after may_open; LTP execve04): a
    // regular file some open description still holds for writing must
    // not be executed — the new text would alias a concurrent writer's
    // bytes. The lease layer's per-inode writer counts are the
    // i_writecount analogue; our own read-only open above is not a
    // writer, so the check cannot self-trigger. Regular files only:
    // non-regular targets fail exec earlier (directory EACCES above,
    // devices ENOEXEC at validate) and a busy device is not ETXTBSY.
    if inode.mode.is_regular_file()
        && crate::fs::locks::inode_write_open_count(&inode) > 0
    {
        return Err(-(errno::ETXTBSY as i32));
    }
    use core::sync::atomic::Ordering::Relaxed;
    Ok(ExecFile {
        file,
        mode: inode.mode.bits(),
        uid: inode.uid.load(Relaxed),
        gid: inode.gid.load(Relaxed),
        size: inode.size.load(Relaxed),
    })
}

/// Read the whole file through a kernel-internal File Arc, presized from
/// the inode size so the doubling-growth realloc copies of the old loop
/// disappear (the exec target is a regular file; a bogus hint only costs
/// one wrong-size allocation, correctness never depends on it).
/// Returns None when nothing could be read.
fn read_file_via(
    file: &alloc::sync::Arc<crate::fs::file::File>,
    size_hint: u64,
) -> Option<alloc::vec::Vec<u8>> {
    let cap = usize::try_from(size_hint).unwrap_or(0).min(64 * 1024 * 1024);
    let mut data = alloc::vec::Vec::with_capacity(cap);
    let mut chunk = [0u8; 16384];
    loop {
        // SAFETY: file is a live Arc<File>; chunk is a valid kernel buffer.
        let n = unsafe { file.read(chunk.as_mut_ptr(), chunk.len()) };
        if n <= 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n as usize]);
        if (n as usize) < chunk.len() {
            break;
        }
    }
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

/// ARG_MAX approximation for execve argument limits (Linux uses a 2MB-ish
/// total with per-string 32-page caps; we take a middle road so real
/// shells/python do not get silently truncated — the old 65-entry/1KB caps
/// broke anything with a moderate environment):
/// - per array (argv, envp): 2048 entries
/// - per string: 8 KB
/// - combined total (argv+envp bytes): 2 MB
/// Exceeding a limit fails exec with E2BIG instead of silent truncation.
const MAX_ARG_STRINGS: usize = 2048;
const MAX_ARG_STRLEN: usize = 8192;
const MAX_ARG_TOTAL: usize = 2 * 1024 * 1024;

/// Read argv array from user space using fault-safe uaccess helpers.
/// Returns Err(E2BIG) when a limit is exceeded (execve must fail, not
/// truncate).
fn copy_argv_from_user(argv_ptr: *const *const u8) -> Result<alloc::vec::Vec<alloc::string::String>, i64> {
    use alloc::string::String;
    let mut args = alloc::vec::Vec::new();
    if argv_ptr.is_null() {
        return Ok(args);
    }

    let mut total = 0usize;
    let mut buf = [0u8; MAX_ARG_STRLEN];
    for i in 0..MAX_ARG_STRINGS {
        if i == MAX_ARG_STRINGS - 1 {
            // Hit the entry cap with no NULL terminator seen.
            return Err(-(errno::E2BIG as i64));
        }
        // Read one pointer from the user argv array via get_user (handles SUM + exception table).
        let arg_ptr = match unsafe { crate::arch::uaccess::get_user(argv_ptr.add(i)) } {
            Some(p) => p,
            None => break, // fault or end of array
        };
        if arg_ptr.is_null() {
            break;
        }
        // Read the null-terminated string via strncpy_from_user (byte-by-byte get_user).
        match crate::arch::uaccess::strncpy_from_user(arg_ptr, MAX_ARG_STRLEN, &mut buf) {
            Ok(slice) => {
                total = total.saturating_add(slice.len());
                if total > MAX_ARG_TOTAL {
                    return Err(-(errno::E2BIG as i64));
                }
                // Non-UTF-8 bytes become U+FFFD instead of silently
                // dropping the entry (paths with stray bytes still exec).
                args.push(String::from_utf8_lossy(slice).into_owned());
            }
            Err(_) => break, // page fault reading user string
        }
    }
    Ok(args)
}

/// Read envp array from user space using fault-safe uaccess helpers.
fn copy_envp_from_user(envp_ptr: *const *const u8) -> Result<alloc::vec::Vec<alloc::string::String>, i64> {
    use alloc::string::String;
    let mut envs = alloc::vec::Vec::new();
    if envp_ptr.is_null() {
        return Ok(envs);
    }

    let mut total = 0usize;
    let mut buf = [0u8; MAX_ARG_STRLEN];
    for i in 0..MAX_ARG_STRINGS {
        if i == MAX_ARG_STRINGS - 1 {
            return Err(-(errno::E2BIG as i64));
        }
        // Read one pointer from the user envp array via get_user (handles SUM + exception table).
        let env_str_ptr = match unsafe { crate::arch::uaccess::get_user(envp_ptr.add(i)) } {
            Some(p) => p,
            None => break,
        };
        if env_str_ptr.is_null() {
            break;
        }
        match crate::arch::uaccess::strncpy_from_user(env_str_ptr, MAX_ARG_STRLEN, &mut buf) {
            Ok(slice) => {
                total = total.saturating_add(slice.len());
                if total > MAX_ARG_TOTAL {
                    return Err(-(errno::E2BIG as i64));
                }
                envs.push(String::from_utf8_lossy(slice).into_owned());
            }
            Err(_) => break,
        }
    }
    Ok(envs)
}

/// EXECCOST (dfx=execcost, abi-review lane): per-stage execve cost
/// breakdown in raw read_time() ticks. Two lines per exec — the ladder
/// stages here, the image stages in do_execve_elf (EXECCOST2). Prints
/// only when the switch is armed, and only for recursion depth 0
/// (user-visible execs), rate-limited to the first 64 plus every 512th
/// afterwards. Purpose: decompose the p4_fork_exec reference gap
/// (~1.1 ms exec vs ~11 us reference median).
fn execcost_armed(n: u64) -> bool {
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::ExecCost) {
        return false;
    }
    n < 64 || n % 512 == 0
}

/// Core execve implementation: load and execute an ELF binary.
/// Handles shebang (#!) scripts by recursing with the interpreter.
fn do_execve(pathname: &str, argv: &[alloc::string::String], envp: &[alloc::string::String], recursion_depth: u32) -> u64 {
    use crate::fs::elf::{ElfLoader, Elf64Ehdr};
    use alloc::string::String;
    use alloc::vec::Vec;

    // EXECCOST: single rate-limit counter, armed check once.
    static EXECCOST_N: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let ec_n = if recursion_depth == 0 {
        EXECCOST_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
    } else {
        u64::MAX
    };
    let ec = execcost_armed(ec_n);
    let ec_t0: u64 = if ec { crate::arch::cpu::read_time() } else { 0 };
    let (mut ec_path, mut ec_lookup, mut ec_read, mut ec_parse, mut ec_interp, mut ec_cred, mut ec_pre) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);

    // EXECTRACE (dbus-activation hunt): activation children have been
    // observed dying with their FORK comm (never exec'd). Trace execs of
    // the binaries involved in D-Bus activation, and every exec issued by
    // a task whose comm still says dbus-daemon (posix_spawn/vfork child
    // pre-exec). Rate-limited; prints jiffies at start/end so the
    // fork->exec latency is measurable from the console.
    let xt_now = crate::drivers::timer::get_jiffies();
    let xt_pid = crate::process::current_pid();
    let xt_on = {
        static XT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let caller_is_dbus = crate::sched::current()
            .map(|c| unsafe {
                let comm = (*c).comm();
                comm.starts_with(b"dbus-daemon".as_slice())
            })
            .unwrap_or(false);
        let path_interesting = pathname.contains("python3.10")
            || pathname.contains("dconf-service")
            || pathname.contains("launch-helper")
            || pathname.contains("dbus-daemon");
        (caller_is_dbus || path_interesting)
            && recursion_depth == 0
            && XT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 400
    };
    if xt_on {
        let comm = crate::sched::current()
            .map(|c| unsafe {
                let raw = (*c).comm();
                let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                core::str::from_utf8(&raw[..len]).unwrap_or("?")
            })
            .unwrap_or("?");
        crate::pr_info!(
            "EXECTRACE: START pid={} comm={} path={} jiffies={}",
            xt_pid, comm, pathname, xt_now
        );
    }

    // Build full path
    let full_path = if pathname.starts_with('/') {
        alloc::borrow::Cow::Borrowed(pathname)
    } else {
        if let Some(current) = crate::sched::current() {
            // SAFETY: current is guaranteed non-null from sched::current() above.
            let cwd = unsafe { (*current).get_cwd() };
            if let Ok(cwd_str) = core::str::from_utf8(&cwd) {
                let mut path = String::with_capacity(cwd_str.len() + pathname.len() + 1);
                path.push_str(cwd_str);
                if !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(pathname);
                alloc::borrow::Cow::Owned(path)
            } else {
                alloc::borrow::Cow::Borrowed(pathname)
            }
        } else {
            alloc::borrow::Cow::Borrowed(pathname)
        }
    };

    // ONE resolution gates and feeds everything downstream (Linux
    // do_open_execat: the walk, MAY_EXEC, and the struct file are one
    // operation): a >NAME_MAX component is ENAMETOOLONG, walking through
    // a non-directory is ENOTDIR, a directory itself is EACCES (may_open),
    // and the target must carry at least one execute bit for the caller
    // (EACCES — even root needs one x bit). The resolved inode also
    // serves the setuid stat and the VMA file pinning below.
    if ec { ec_path = crate::arch::cpu::read_time(); }
    let exec_res = resolve_exec_file(full_path.as_ref());
    if ec { ec_lookup = crate::arch::cpu::read_time(); }
    let exec_file = match exec_res {
        Ok(f) => f,
        Err(e) => return (e as i64) as u64,
    };

    // Read file content through the resolved kernel File (fd-free). The
    // read can fail TRANSIENTLY under concurrent block I/O (same family
    // as the PT_INTERP read below — buffer-cache pressure right after a
    // killed task's teardown), which surfaced in the LTP sweep as
    // spurious execve ENOENT of binaries that plainly exist (creat08,
    // asapi_02, pipeio, fs_fill, ...). Retry a bounded number of times,
    // then fall back to the raw path readers once, before reporting
    // ENOENT.
    let mut program_data = read_file_via(&exec_file.file, exec_file.size);
    let mut main_attempt = 1;
    while program_data.is_none() && main_attempt < 3 {
        crate::pr_err!(
            "exec: transient read of {} failed (pid {}, attempt {}), retrying",
            full_path.as_ref(),
            crate::process::current_pid(),
            main_attempt
        );
        program_data = read_file_via(&exec_file.file, exec_file.size);
        main_attempt += 1;
    }
    if program_data.is_none() {
        program_data = read_exec_file(full_path.as_ref());
    }
    let program_data = match program_data {
        Some(d) => d,
        None => return -errno::ENOENT as u64,
    };
    if ec { ec_read = crate::arch::cpu::read_time(); }

    // Check for shebang (#!) script — binfmt_script behavior
    if let Some((interp, opt_arg)) = parse_shebang(&program_data) {
        if recursion_depth >= MAX_BINPRM_RECURSION {
            return -errno::ELOOP as u64;
        }
        // Build new argv: [interpreter, opt_arg?, script_path, original_argv[1:]...]
        let mut new_argv = alloc::vec::Vec::with_capacity(argv.len() + 2);
        new_argv.push(String::from(interp));
        if let Some(arg) = opt_arg {
            new_argv.push(String::from(arg));
        }
        new_argv.push(String::from(full_path.as_ref()));
        new_argv.extend_from_slice(&argv[1..]);

        return do_execve(interp, &new_argv, envp, recursion_depth + 1);
    }

    // Validate ELF format
    if ElfLoader::validate(&program_data).is_err() {
        return -errno::ENOEXEC as u64;
    }

    // Get entry point
    let entry = match ElfLoader::get_entry(&program_data) {
        Ok(e) => e,
        Err(_) => return -errno::ENOEXEC as u64,
    };

    // Get program header count
    let phdr_count = match ElfLoader::get_program_headers(&program_data) {
        Ok(n) => n,
        Err(_) => return -errno::ENOEXEC as u64,
    };

    // SAFETY: program_data is a valid slice read from a verified ELF file;
    // from_bytes validates the ELF header structure before returning.
    let ehdr = match unsafe { Elf64Ehdr::from_bytes(&program_data) } {
        Some(e) => e,
        None => return -errno::ENOEXEC as u64,
    };
    if ec { ec_parse = crate::arch::cpu::read_time(); }

    // Check for PT_INTERP (dynamic executable)
    let mut interp_exec: Option<ExecFile> = None;
    let interp_data: Option<alloc::vec::Vec<u8>> = if let Some(interp_path) = ElfLoader::get_interpreter(&program_data) {
        let interp_str = match core::str::from_utf8(interp_path) {
            Ok(s) => s,
            Err(_) => return -errno::ENOEXEC as u64,
        };
        // A dynamic executable MUST have its interpreter, opened with the
        // same exec semantics as the main file (Linux open_exec: walk +
        // MAY_EXEC; a missing or non-executable interpreter is the walk's
        // own errno, ENOENT/EACCES). The old code fell back to running
        // the image STATICALLY when this read failed transiently —
        // main() then jumped through file-initial PLTGOT entries
        // (0x4710 -> unresolved resolver path) and died at an unmapped
        // heap address (the 0x1c29c family). Retry a few times for
        // transient block-I/O failures, then fail the exec.
        let mut data = None;
        let mut attempt = 0;
        while attempt < 3 {
            match resolve_exec_file(interp_str) {
                Ok(f) => {
                    data = read_file_via(&f.file, f.size);
                    if data.is_some() {
                        interp_exec = Some(f);
                        break;
                    }
                }
                Err(e) => {
                    crate::pr_err!(
                        "exec: interpreter resolve failed for {} (pid {}) — err {}",
                        interp_str,
                        crate::process::current_pid(),
                        e
                    );
                    return (e as i64) as u64;
                }
            }
            attempt += 1;
        }
        match data {
            Some(d) => Some(d),
            None => {
                crate::pr_err!(
                    "exec: interpreter read failed for {} (pid {}) — ENOENT",
                    interp_str,
                    crate::process::current_pid()
                );
                return -errno::ENOENT as u64;
            }
        }
    } else {
        None
    };
    if ec { ec_interp = crate::arch::cpu::read_time(); }

    // Build final argv (use provided argv, fallback to full_path if empty)
    let final_argv: Vec<String> = if argv.is_empty() {
        alloc::vec![String::from(full_path.as_ref())]
    } else {
        argv.to_vec()
    };

    // Get current process
    let current = match crate::sched::current() {
        Some(c) => c,
        None => return -errno::ESRCH as u64,
    };

    // ---- setuid / setgid exec handling ----
    // Check file mode bits for S_ISUID / S_ISGID and update credentials
    // accordingly.  The new credentials are committed before the image load
    // (so AT_UID/auxv see them) but restored verbatim if the load fails: a
    // half-valid setuid binary must never elevate the *old* image
    // (review C-02).
    let mut secure_exec = false;
    let mut cred_saved: Option<crate::process::task::Cred> = None;
    {
        use crate::security::capability::Cap;

        const S_ISUID: u32 = 0o4000;
        const S_ISGID: u32 = 0o2000;

        // Mode/uid/gid come from the ONE resolution at the ladder head —
        // stat_file_by_path re-walked the path just for these bits.
        let file_mode = exec_file.mode;
        let file_uid = exec_file.uid;
        let file_gid = exec_file.gid;
        // no_new_privs (prctl 39): execve grants NO privileges — the
        // setuid/setgid bits are IGNORED (capabilities(7): "execve(2)
        // will not grant privileges ... the set-user-ID and set-group-ID
        // mode bits and file capabilities are all ignored"). The task
        // keeps its credentials; the normal-exec capability transition
        // below still runs with the unchanged ids.
        let nnp = (*current).no_new_privs();
        let is_setuid = if nnp { false } else { (file_mode & S_ISUID) != 0 };
        let is_setgid = if nnp { false } else { (file_mode & S_ISGID) != 0 };

        // Execute permission check (review P26/M-07): without any x bit
        // the file must not be executable regardless of setuid bits.
        // (Redundant with the resolve gate today — kept as a guard right
        // at the credential decision, like Linux re-checks in
        // bprm_fill_uid's caller.)
        const S_IXUSR: u32 = 0o100;
        const S_IXGRP: u32 = 0o010;
        const S_IXOTH: u32 = 0o001;
        let has_x = (file_mode & (S_IXUSR | S_IXGRP | S_IXOTH)) != 0;
        if !has_x {
            return -(errno::EACCES as i64) as u64;
        }

        // SAFETY: current is a valid, non-null task pointer from
        // sched::current(); cred_mut() gives exclusive cred access.
        unsafe {
            let cred = (*current).cred_mut();
                // Snapshot BEFORE any mutation — the rollback path must
                // restore the pre-exec credentials, not the elevated ones.
                cred_saved = Some(cred.clone());
                let old_euid = cred.euid;
                let old_egid = cred.egid;

                if is_setuid && file_uid != old_euid {
                    if file_uid == 0 {
                        // setuid root: elevate to root with full caps
                        cred.euid = 0;
                        cred.fsuid = 0;
                        cred.cap_effective = Cap::FULL;
                        cred.cap_permitted = Cap::FULL;
                        cred.cap_inheritable = Cap::EMPTY;
                        cred.cap_ambient = Cap::EMPTY;
                    } else {
                        // setuid non-root: drop all caps, change euid
                        cred.euid = file_uid;
                        cred.fsuid = file_uid;
                        cred.cap_effective = Cap::EMPTY;
                        cred.cap_permitted = Cap::EMPTY;
                        cred.cap_inheritable = Cap::EMPTY;
                        cred.cap_ambient = Cap::EMPTY;
                    }
                    cred.suid = cred.euid;
                }

                if is_setgid && file_gid != old_egid {
                    cred.egid = file_gid;
                    cred.fsgid = file_gid;
                    cred.sgid = cred.egid;
                    // setgid non-root also drops caps (unless setuid root above)
                    if !(is_setuid && file_uid == 0) {
                        cred.cap_effective = Cap::EMPTY;
                        cred.cap_permitted = Cap::EMPTY;
                        cred.cap_inheritable = Cap::EMPTY;
                        cred.cap_ambient = Cap::EMPTY;
                    }
                }

                if !is_setuid && !is_setgid {
                    // Normal exec capability transition. Root keeps a full
                    // permitted+effective set (Linux: euid==0 → pP' |= bset,
                    // pE' = pP'); non-root computes effective from
                    // inheritable ∪ ambient.
                    let ambient = cred.cap_inheritable
                        .intersect(cred.cap_bounding)
                        .intersect(cred.cap_permitted);
                    if cred.euid == 0 {
                        cred.cap_permitted = cred.cap_permitted.union(cred.cap_bounding);
                        cred.cap_effective = cred.cap_permitted;
                    } else {
                        cred.cap_effective = cred.cap_permitted.intersect(
                            cred.cap_inheritable.union(ambient)
                        );
                    }
                    // Clear ambient for non-setuid exec
                    cred.cap_ambient = Cap::EMPTY;
                }

                // AT_SECURE / secureexec. Linux semantics: 1 only when the
                // exec CHANGES the effective ids (setuid/setgid bits taken,
                // or euid != uid / egid != gid). Capabilities alone do NOT
                // make an exec secure — the old `!cap_effective.is_empty()`
                // clause marked EVERY root process AT_SECURE=1 (root keeps
                // its caps across exec), so ld.so scrubbed LD_PRELOAD for
                // all root binaries and glibc entered secure mode globally
                // (LD_*, tunables, $ORIGIN all disabled).
                secure_exec = is_setuid || is_setgid
                    || cred.euid != cred.uid
                    || cred.egid != cred.gid;
        }
    }

    // On secure exec, strip dynamic-loader tunables (LD_PRELOAD,
    // LD_LIBRARY_PATH, ...) from the environment so they cannot hijack a
    // privileged binary.
    let mut final_envp: Vec<String> = envp.to_vec();
    if secure_exec {
        final_envp.retain(|e| {
            let name = e.split('=').next().unwrap_or("");
            !name.starts_with("LD_")
        });
    }
    if ec { ec_cred = crate::arch::cpu::read_time(); }

    // Execute ELF loading
    let phdr_count_usize = if phdr_count > 1024 {
        return -(crate::errno::constants::EINVAL as i64) as u64;
    } else {
        phdr_count as usize
    };

    // ---- de_thread (Linux begin_new_exec): past this point any exec
    // failure still leaves the process single-threaded, exactly like
    // Linux. Must run BEFORE the address space is replaced: sibling
    // threads sharing this mm would otherwise keep executing the OLD
    // image against the NEW mm / already-cloexec'd fdtable. ----
    // SAFETY: current is the running task (checked above).
    unsafe {
        crate::process::exit::de_thread(current);
    }

    // Pinned file identities for the exec-time segment VMAs (vm_file
    // semantics — see ExecFile). Both come from the single resolutions
    // above; a None only drops the /proc/<pid>/maps path column, never
    // the exec.
    let exe_file_arc = Some(exec_file.file);
    let interp_file_arc = interp_exec.map(|f| f.file);
    if ec { ec_pre = crate::arch::cpu::read_time(); }
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SyscallTrace) {
        crate::pr_info!("LVMA-DBG pid {} exe_pin={} interp_pin={} path {}",
            crate::process::current_pid(),
            exe_file_arc.is_some(), interp_file_arc.is_some(), full_path.as_ref());
    }

    let elf_result = do_execve_elf(current, &program_data, &final_argv, &final_envp, entry, phdr_count_usize, &ehdr, full_path.as_ref(), interp_data.as_deref(), secure_exec, exe_file_arc.as_ref(), interp_file_arc.as_ref());
    let ec_t1: u64 = if ec { crate::arch::cpu::read_time() } else { 0 };
    if ec {
        crate::pr_info!(
            "EXECCOST n={} pid={} path={} pstr={} lk={} rd={} parse={} it={} cred={} pre={} elf={} tot={} sz={}",
            ec_n,
            crate::process::current_pid(),
            full_path.as_ref(),
            ec_path - ec_t0,
            ec_lookup - ec_path,
            ec_read - ec_lookup,
            ec_parse - ec_read,
            ec_interp - ec_parse,
            ec_cred - ec_interp,
            ec_pre - ec_cred,
            ec_t1 - ec_pre,
            ec_t1 - ec_t0,
            program_data.len() + interp_data.as_ref().map(|d| d.len()).unwrap_or(0)
        );
    }
    match elf_result {
        Ok(()) => {
            // The image changed: the parent may no longer setpgid us
            // (Linux sys_setpgid EACCES after exec; LTP setpgid03).
            // SAFETY: current is the running task.
            unsafe { (*current).mark_execed(); }
            if xt_on {
                crate::pr_info!(
                    "EXECTRACE: OK pid={} path={} dt_jiffies={}",
                    xt_pid, full_path.as_ref(),
                    crate::drivers::timer::get_jiffies().wrapping_sub(xt_now)
                );
            }
            // sc209 (leg 14): latch render_service's tgid for the FXRS
            // futex lifecycle tracer (SA 10 publish-wall forensics; see
            // the FXRS block in sync/futex.rs). The exec'ing task is the
            // group leader, so pid == tgid here.
            if full_path.as_ref().ends_with("/system/bin/render_service") {
                crate::sync::futex::fxrs_latch(crate::process::current_pid());
            }
            crate::pr_debug!("exec: pid={} path={}", crate::process::current_pid(), full_path.as_ref());
            0
        }
        Err(e) => {
            if xt_on {
                crate::pr_info!(
                    "EXECTRACE: ERR pid={} path={} err={} dt_jiffies={}",
                    xt_pid, full_path.as_ref(), e,
                    crate::drivers::timer::get_jiffies().wrapping_sub(xt_now)
                );
            }
            // Roll back the credential changes: the old image keeps running.
            if let Some(saved) = cred_saved {
                // SAFETY: current task pointer still valid on the error path.
                unsafe { *(*current).cred_mut() = saved; }
            }
            // Do NOT wake a vfork parent here: exec failed means we still
            // hold the CLONE_VM-shared address space (and typically the
            // parent's stack). The parent may only run again once we exit;
            // a well-formed vfork child _exit()s after a failed exec, which
            // performs the wake (exit path).
            e as i64 as u64
        }
    }
}

/// sys_execve - Execute program
///
/// # Arguments
/// - args[0]: pathname - program path
/// - args[1]: argv - argument array
/// - args[2]: envp - environment variable array
///
/// # Returns
/// Does not return on success, negative error code on failure
pub fn sys_execve(args: SyscallArgs) -> i64 {
    let pathname_ptr = args[0] as *const u8;
    let argv_ptr = args[1] as *const *const u8;
    let envp_ptr = args[2] as *const *const u8;

    // Check path pointer
    if pathname_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Read pathname from user space safely — PATH_MAX (4096) like Linux
    // (the old 256-byte cap rejected perfectly legal long paths).
    let mut kernel_buf = alloc::vec![0u8; 4096];
    let pathname = match strncpy_from_user(pathname_ptr, 4096, &mut kernel_buf[..]) {
        Ok(s) => s,
        Err(e) => return e as i64,
    };

    let pathname_str = match core::str::from_utf8(pathname) {
        Ok(s) => s,
        Err(_) => return -(errno::EINVAL as i64),
    };

    // Copy argv and envp from user space (E2BIG on limit overflow)
    let argv = match copy_argv_from_user(argv_ptr) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let envp = match copy_envp_from_user(envp_ptr) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let rc = do_execve(pathname_str, &argv, &envp, 0) as i64;
    // sc39 gate: the launcher spawn child completes every sandbox hook
    // ('Child process:com.ohos.launcher success') then exits 0 WITHOUT
    // exec'ing — name any failed exec of an app-shaped path (a bare
    // execve ENOENT is otherwise invisible on the serial).
    if rc < 0
        && (pathname_str.starts_with("/data/")
            || pathname_str.starts_with("/mnt/sandbox/"))
    {
        static EXEC_FAIL_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let n = EXEC_FAIL_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 120 {
            let root = crate::sched::current()
                .and_then(|t| unsafe { (*t).fs_arc() })
                .map(|fs| fs.get_root())
                .unwrap_or_default();
            let root_str = core::str::from_utf8(&root).unwrap_or("?");
            crate::pr_warn!(
                "execfail[{}] '{}' -> {} root='{}' (pid {})",
                n,
                pathname_str,
                rc,
                root_str,
                unsafe { crate::process::current_pid() }
            );
        }
    }
    rc
}

/// sys_exit - Exit the CALLING THREAD only (Linux exit(2) semantics)
///
/// # Arguments
/// - args[0]: status - exit status code
///
/// # Returns
/// Does not return
pub fn sys_exit(args: SyscallArgs) -> i64 {
    // Linux ABI (kernel/exit.c): the user code is masked to 8 bits and
    // pre-shifted into wait-status layout at the syscall boundary —
    // sys_exit stores `(error_code & 0xff) << 8`. Storing the raw value
    // made `exit(-1)` indistinguishable from death-by-SIGHUP everywhere
    // downstream (Rux encodes signal deaths as negative exit codes), so
    // every OH service exiting with -1 was misreported as a
    // kernel-origin SIGHUP kill (SIGDEATH sig=1, si_pid=0) and init's
    // wait4 saw WIFSIGNALED where Linux reports WIFEXITED/255.
    let status = (((args[0] as u32) & 0xFF) << 8) as i32;
    crate::process::exit::do_exit(status);
    0 // unreachable
}

/// sys_exit_group - Exit the WHOLE thread group (Linux exit_group(2))
///
/// SIGKILLs every other member of the caller's thread group (leader and
/// siblings alike) and then runs the caller's exit. musl's `_exit` /
/// `exit` / return-from-main all come through here — without it a
/// multi-threaded process kept running after "exit".
pub fn sys_exit_group(args: SyscallArgs) -> i64 {
    // Same Linux pre-shift as sys_exit (see the comment there).
    let status = (((args[0] as u32) & 0xFF) << 8) as i32;
    // sc40 gate: every appspawn child completes its sandbox hooks then
    // exits 0 here without ever running the ability runtime — dump the
    // child's last syscalls from the forensic ring so the terminal
    // sequence (what it read/checked right before deciding to die) is
    // on the serial. Deep-root (app sandbox) processes only.
    {
        let root = crate::sched::current()
            .and_then(|t| unsafe { (*t).fs_arc() })
            .map(|fs| fs.get_root())
            .unwrap_or_default();
        let root_str = core::str::from_utf8(&root).unwrap_or("");
        let deep = root_str
            .split('/')
            .filter(|s| !s.is_empty())
            .count() >= 4;
        if deep {
            use core::sync::atomic::Ordering::Relaxed;
            let pid = unsafe { crate::process::current_pid() } as u32;
            let mut picks: [(u32, u64, u64, u64); 12] = [(0, 0, 0, 0); 12];
            let mut n = 0usize;
            for e in crate::syscall::dispatch::SYSCALL_RING.iter() {
                if e.pid.load(Relaxed) == pid && n < picks.len() {
                    picks[n] = (
                        e.nr.load(Relaxed),
                        e.a0.load(Relaxed),
                        e.a1.load(Relaxed),
                        e.ret.load(Relaxed),
                    );
                    n += 1;
                }
            }
            crate::pr_warn!(
                "childexit pid={} root='{}' status={:#x} last{}:",
                pid,
                root_str,
                args[0] as u32,
                n
            );
            for i in (0..n).rev() {
                let (nr, a0, a1, ret) = picks[i];
                crate::pr_warn!(
                    "  [{:02}] nr={} a0={:#x} a1={:#x} -> {}",
                    i,
                    nr,
                    a0,
                    a1,
                    ret as i64
                );
            }
        }
    }
    crate::process::exit::do_exit_group(status);
    0 // unreachable
}

/// wait4 option bits (Linux wait.h)
const WNOHANG_OPT: i32 = 0x00000001;
const WUNTRACED_OPT: i32 = 0x00000002;
const WCONTINUED_OPT: i32 = 0x00000008;
const WEXITED_OPT: i32 = 0x00000004;
const WNOWAIT_OPT: i32 = 0x01000000;
/// __WNOTHREAD/__WCLONE/__WALL — Linux private wait bits, accepted (and
/// ignored: every child is waited the same way here). LTP clone301 passes
/// __WALL to waitpid for a clone(SIGCHLD) child; rejecting it made the
/// wait fail with EINVAL.
const WNOTHREAD_OPT: i32 = 0x20000000;
const WCLONE_OPT: i32 = -0x80000000; // __WCLONE bit pattern as i32
const WALL_OPT: i32 = 0x40000000;

/// sys_wait4 - Wait for child process
///
/// # Arguments
/// - args[0]: pid - process ID to wait for (>0 exact, 0 = caller's pgid,
///   -1 = any, <-1 = pgid == -pid)
/// - args[1]: status - pointer to store exit status
/// - args[2]: options - wait options (WNOHANG | WUNTRACED | WCONTINUED)
/// - args[3]: rusage - resource usage statistics pointer (zero-filled,
///   144-byte Linux layout)
///
/// # Returns
/// Returns child process PID on success, negative error code on failure
pub fn sys_wait4(args: SyscallArgs) -> i64 {
    let pid = args[0] as i32;
    let wstatus = args[1] as *mut i32;
    let options = args[2] as i32;
    let rusage = args[3] as *mut u8;

    // Validate wstatus pointer
    if !wstatus.is_null() && !crate::arch::uaccess::access_ok(wstatus as usize, 4) {
        return -(errno::EFAULT as i64);
    }

    // Unknown option bits → EINVAL (Linux). The __W* private bits
    // (__WCLONE/__WALL/__WNOTHREAD) are accepted like Linux's WNOWAIT.
    if options
        & !(WNOHANG_OPT
            | WUNTRACED_OPT
            | WCONTINUED_OPT
            | WNOWAIT_OPT
            | WEXITED_OPT
            | WNOTHREAD_OPT
            | WCLONE_OPT
            | WALL_OPT)
        != 0
    {
        return -(errno::EINVAL as i64);
    }

    // Zero-fill rusage on every successful wait path (struct rusage is
    // 144 bytes on riscv64; no accounting yet, but userspace parsers
    // like `time`/`make` read the full struct).
    let fill_rusage = || {
        if !rusage.is_null() {
            if crate::arch::uaccess::access_ok(rusage as usize, 144) {
                let zeros = [0u8; 144];
                // SAFETY: access_ok-validated pointer; exception-table copy.
                unsafe {
                    let _ = crate::arch::uaccess::copy_to_user(
                        rusage, &zeros as *const u8, 144,
                    );
                }
            }
        }
    };

    // U1c pid-namespace semantics: the wait target pid is interpreted in
    // the CALLER's pid namespace, and the reaped child is REPORTED in it.
    let wait_pid: i32 = if pid > 0 {
        match crate::process::ns::resolve_vpid(pid as u32) {
            Some(g) => g as i32,
            None => return -(errno::ECHILD as i64),
        }
    } else {
        pid
    };

    // INT32_MIN as a (negative) "pgid" overflows the kernel's pgid
    // negation (Linux: -upid->nr wraps to itself), so no such process
    // group can ever exist — ESRCH, not ECHILD (LTP waitpid04).
    if wait_pid == i32::MIN {
        return -(errno::ESRCH as i64);
    }

    if options & WNOHANG_OPT != 0 {
        // WNOHANG mode: non-blocking check (options now honored: WUNTRACED
        // reports eligible stopped children too)
        match crate::process::exit::do_wait_nonblock(wait_pid, wstatus, options) {
            Ok(child_pid) => {
                fill_rusage();
                report_wait_pid(child_pid)
            }
            Err(e) if e == -11 => 0, // EAGAIN -> return 0 means no child process exited
            Err(e) => e as i32 as i64,
        }
    } else {
        // Blocking wait for child process to exit
        match crate::process::exit::do_wait(wait_pid, wstatus, options) {
            Ok(child_pid) => {
                fill_rusage();
                report_wait_pid(child_pid)
            }
            Err(e) => e as i32 as i64,
        }
    }
}

/// Report a reaped child's pid in the CALLER's pid namespace (U1c).
/// Falls back to the global pid when the task is already gone from the
/// hash (reaped) or lives in another namespace.
fn report_wait_pid(global_pid: u32) -> i64 {
    crate::process::ns::wait_vpid(global_pid) as i64
}

/// sys_waitid - wait for child process state change (Linux ABI)
///
/// Arguments: (idtype, id, infop, options, rusage)
/// - idtype: P_ALL(0), P_PID(1), P_PGID(2)
/// - id: PID or PGID
/// - infop: user pointer to siginfo_t
/// - options: WNOHANG | WEXITED | WSTOPPED | WCONTINUED | WNOWAIT
/// - rusage: ignored
///
/// Returns: 0 on success (including WNOHANG with no event — infop
/// untouched, si_pid stays 0), negative errno on error
pub fn sys_waitid(args: SyscallArgs) -> i64 {
    let idtype = args[0] as i32;
    let id = args[1] as i32;
    let infop = args[2] as *mut u8;
    let options = args[3] as i32;
    let _rusage = args[4] as *mut u8;

    // Validate idtype
    if idtype < 0 || idtype > 2 {
        return -(errno::EINVAL as i64);
    }

    // Validate infop pointer
    if infop.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(infop as usize, 128) {
        return -(errno::EFAULT as i64);
    }

    match crate::process::exit::do_waitid(idtype, id, infop, options) {
        // Ok(false) = WNOHANG, no event: Linux returns 0 and leaves
        // infop untouched (the old -EAGAIN broke every poll loop).
        Ok(_) => 0,
        Err(e) => e as i32 as i64,
    }
}

/// sys_getpid - Get process ID (== thread group ID; all threads of one
/// process see the leader's pid). U1c: reported in the caller's pid
/// namespace (ns-local pid).
pub fn sys_getpid(_args: SyscallArgs) -> i64 {
    if let Some(current) = crate::sched::current() {
        // SAFETY: current is guaranteed valid and non-null by
        // sched::current(); group_leader_ptr() heals NULL. The LEADER's
        // ns-local pid is the process's pid in the caller's namespace.
        unsafe {
            let leader = (*current).group_leader_ptr();
            (*leader).ns_pid_local() as i64
        }
    } else {
        0
    }
}

/// sys_gettid - Get thread ID (per-thread unique; == pid)
///
/// In single-threaded processes, tid == pid.
/// U1c: reported in the caller's pid namespace.
/// RISC-V syscall number: 178
pub fn sys_gettid(_args: SyscallArgs) -> i64 {
    if let Some(current) = crate::sched::current() {
        // SAFETY: current is guaranteed valid and non-null by sched::current().
        unsafe { (*current).ns_pid_local() as i64 }
    } else {
        0
    }
}

/// sys_getppid - Get parent process ID (threads report the leader's
/// parent — the whole group shares one real parent).
/// U1c: when the parent lives in a different (ancestor) pid namespace it
/// is invisible — the namespace init reports 0, like Linux.
pub fn sys_getppid(_args: SyscallArgs) -> i64 {
    if let Some(current) = crate::sched::current() {
        // SAFETY: current is a valid task; group_leader_ptr() heals NULL.
        unsafe {
            let leader = (*current).group_leader_ptr();
            let ppid = (*leader).ppid();
            if ppid == 0 {
                return 0;
            }
            match crate::process::find_task_by_pid(ppid) {
                Some(p) => {
                    if crate::process::ns::task_same_pid_ns(&*current, p) {
                        p.ns_pid_local() as i64
                    } else {
                        // Parent in another namespace: invisible from here.
                        // Only the ns init legitimately sees this.
                        0
                    }
                }
                None => 0,
            }
        }
    } else {
        0
    }
}

/// sys_kill - Send signal
///
/// Thread-group semantics: a positive pid targets the whole thread group
/// of the task holding that pid (process-directed signal): every live
/// member gets its own pending copy. kill(0)/kill(-pgid)/kill(-1)
/// broadcasts skip the CALLER'S WHOLE GROUP (not just the calling thread).
pub fn sys_kill(args: SyscallArgs) -> i64 {
    let pid = args[0] as i32;
    let sig = args[1] as i32;

    if sig < 0 || sig > 64 {
        return -(errno::EINVAL as i64);
    }

    if pid == 0 || pid == -1 || pid < 0 {
        // Broadcast targets: kill(0) = caller's process group,
        // kill(-pgid) = that process group, kill(-1) = all processes
        // except self and PID 1. Permission is checked per target
        // (Linux check_kill_permission); EPERM is returned when no target
        // accepted the signal but at least one denied it.
        let my_tgid = match crate::sched::current() {
            // SAFETY: task pointer from sched::current() is valid when Some.
            Some(t) => unsafe { (*t).tgid() },
            None => return -(errno::ESRCH as i64),
        };
        let group_filter: Option<u32> = if pid == 0 {
            // SAFETY: same current task as above.
            Some(unsafe { (*crate::sched::current().unwrap()).pgid() })
        } else if pid == -1 {
            None
        } else if pid == i32::MIN {
            // -pid overflows — no process group can ever match, so no
            // target accepts the signal and kill returns ESRCH (LTP
            // kill03). u32::MAX is outside the allocatable pid space.
            Some(u32::MAX)
        } else {
            Some((-pid) as u32)
        };

        let found = core::cell::Cell::new(false);
        let denied = core::cell::Cell::new(false);
        // kill(-1) excludes the caller's own thread group (Linux
        // group_send_sig_info over every process but self). But kill(0)
        // and kill(-pgid) DO target the caller's own group — the caller
        // is a member of its process group like any other (LTP kill06:
        // kill(-getpgrp(), SIGKILL) must SIGKILL the caller itself so
        // the parent observes WIFSIGNALED/WTERMSIG==SIGKILL).
        let skip_self_group = pid == -1;
        // Broadcast scope is the CALLER'S PID NAMESPACE (reference
        // group_send_sig_info iterates the namespace's task list): a
        // container's kill(-1) reaches only container members — hitting
        // ancestor-namespace tasks killed the LTP runner itself (pidns10
        // expected ESRCH from an empty namespace; pidns17's parent died
        // by the container's own SIGUSR1 broadcast).
        let caller_ns = crate::sched::current().and_then(|t| unsafe { (*t).ns_pid.clone() });
        // pid_hash_for_each_task covers sleeping tasks too (unlike the
        // per-CPU for_each_task which only sees running/idle tasks) — and
        // every THREAD is its own hash entry, so each live member of a
        // multithreaded group receives its own copy.
        crate::process::pid_hash::pid_hash_for_each_task(|task| unsafe {
            if let Some(ref ns) = caller_ns {
                let same_ns = (*task)
                    .ns_pid
                    .as_ref()
                    .map(|a| alloc::sync::Arc::ptr_eq(a, ns))
                    .unwrap_or(false);
                if !same_ns {
                    return;
                }
            }
            let tp = (*task).pid();
            if tp == 1 {
                return; // kill(-1) skips init
            }
            // Only kill(-1) excludes the caller's own thread group.
            if skip_self_group && (*task).tgid() == my_tgid {
                return;
            }
            if let Some(g) = group_filter {
                if (*task).pgid() != g {
                    return;
                }
            }
            found.set(true);
            if sig > 0 {
                if crate::security::can_send_signal((*task).cred()) {
                    let _ = crate::signal::send_signal_from_user(tp, sig);
                } else {
                    denied.set(true);
                }
            }
        });
        if !found.get() {
            return -(errno::ESRCH as i64);
        }
        if denied.get() && sig > 0 {
            return -(errno::EPERM as i64);
        }
        return 0;
    }

    // pid > 0: process-directed send — spread over the target's thread group.
    // U1c pid-namespace translation: the target pid is interpreted in the
    // CALLER's pid namespace (init ns: identity).
    let global_pid = match crate::process::ns::resolve_vpid(pid as u32) {
        Some(g) => g,
        None => return -(errno::ESRCH as i64),
    };
    // SAFETY: find_task_by_pid returns a valid pointer when non-null; we check
    // null before dereferencing and verify permissions before sending signal.
    unsafe {
        let target = crate::sched::find_task_by_pid(global_pid);
        if target.is_null() {
            return -(errno::ESRCH as i64);
        }

        if sig > 0 {
            let target_task = &*target;
            if !crate::security::can_send_signal(target_task.cred()) {
                return -(errno::EPERM as i64);
            }
            // Collect the group's member pids (leader's ring), then send
            // outside the ring lock. Skips dead members; zombies ignore.
            let leader = (*target).group_leader_ptr();
            let mut members: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
            (*leader).for_each_group_member(|m| {
                if !(*m).state().is_dead() {
                    members.push((*m).pid());
                }
            });
            for m in members {
                let _ = crate::signal::send_signal_from_user(m, sig);
            }
        }
    }

    0
}

/// sys_set_tid_address - Set TID address
pub fn sys_set_tid_address(args: SyscallArgs) -> i64 {
    let tidptr = args[0] as *mut i32;

    // Validate tidptr pointer
    if !tidptr.is_null() && !crate::arch::uaccess::access_ok(tidptr as usize, 4) {
        return -(errno::EFAULT as i64);
    }

    if let Some(current) = crate::sched::current() {
        // SAFETY: current is a valid task pointer from sched::current().
        unsafe {
            (*current).set_clear_child_tid(tidptr);
            return (*current).pid() as i64;
        }
    }

    0
}

/// sys_set_robust_list - Set robust list
///
/// args[0] = head pointer, args[1] = length. The length must equal
/// sizeof(struct robust_list_head) == 24 on 64-bit (Linux check).
/// The registered list is walked at thread exit: held futexes get
/// FUTEX_OWNER_DIED + wake (see exit_robust_list).
pub fn sys_set_robust_list(args: SyscallArgs) -> i64 {
    let head = args[0] as *const u8;
    let len = args[1] as usize;

    if head.is_null() && len != 0 {
        return -(errno::EINVAL as i64);
    }
    // Linux: len must be exactly sizeof(struct robust_list_head).
    if len != 24 {
        return -(errno::EINVAL as i64);
    }
    if !head.is_null() && !crate::arch::uaccess::access_ok(head as usize, 24) {
        return -(errno::EFAULT as i64);
    }

    if let Some(current) = crate::sched::current() {
        // SAFETY: current is a valid task pointer from sched::current().
        unsafe {
            (*current).set_robust_list(head, len);
        }
    }
    0
}

/// sys_uname - Get system information
pub fn sys_uname(args: SyscallArgs) -> i64 {
    #[repr(C)]
    struct Utsname {
        sysname: [u8; 65],
        nodename: [u8; 65],
        release: [u8; 65],
        version: [u8; 65],
        machine: [u8; 65],
        domainname: [u8; 65],
    }

    let buf = args[0] as *mut Utsname;

    if buf.is_null() {
        return -(errno::EFAULT as i64);
    }

    // Validate user pointer
    if !crate::arch::uaccess::access_ok(buf as usize, core::mem::size_of::<Utsname>()) {
        return -(errno::EFAULT as i64);
    }

    // Build Utsname on stack, then copy to user with copy_to_user
    let uname = Utsname {
        sysname: {
            let mut a = [0u8; 65];
            // Report "Linux": glibc's minimum-kernel check (dl-osinfo)
            // and dpkg/apt build-id logic key off the sysname and a
            // parseable release "x.y.z"; a novel name/version fails the
            // glibc gate with "FATAL: kernel too old" before main runs.
            let s = b"Linux\0";
            a[..s.len()].copy_from_slice(s);
            a
        },
        nodename: {
            // U1c: nodename comes from the caller's UTS namespace.
            let mut a = [0u8; 65];
            let hn = crate::process::ns::current_uts_ns().get_hostname();
            let n = hn.len().min(64);
            a[..n].copy_from_slice(&hn[..n]);
            a
        },
        release: {
            let mut a = [0u8; 65];
            // x.y.z prefix must satisfy glibc's __LINUX_KERNEL_VERSION
            // floor (>= 3.2.0 for glibc 2.35+); the -rux suffix keeps
            // the Rux identity visible.
            let s = b"6.8.0-rux\0";
            a[..s.len()].copy_from_slice(s);
            a
        },
        version: {
            let mut a = [0u8; 65];
            let s = b"#1 SMP Rux 6.8.0\0";
            a[..s.len()].copy_from_slice(s);
            a
        },
        machine: {
            let mut a = [0u8; 65];
            #[cfg(feature = "riscv64")]
            let s = b"riscv64\0";
            #[cfg(feature = "x86_64")]
            let s = b"x86_64\0";
            #[cfg(feature = "aarch64")]
            let s = b"aarch64\0";
            a[..s.len()].copy_from_slice(s);
            a
        },
        domainname: {
            // U1c: domainname comes from the caller's UTS namespace.
            let mut a = [0u8; 65];
            let dn = crate::process::ns::current_uts_ns().get_domainname();
            let n = dn.len().min(64);
            a[..n].copy_from_slice(&dn[..n]);
            a
        },
    };

    // SAFETY: buf validated with access_ok above; copy_to_user handles SUM bit.
    unsafe {
        let remaining = crate::arch::uaccess::copy_to_user(
            buf as *mut u8,
            &uname as *const Utsname as *const u8,
            core::mem::size_of::<Utsname>(),
        );
        if remaining != 0 {
            return -(errno::EFAULT as i64);
        }
    }

    0
}

/// sys_getuid - Get user ID
pub fn sys_getuid(_args: SyscallArgs) -> i64 {
    if let Some(task) = crate::sched::current() {
        task.cred().uid as i64
    } else {
        0
    }
}

/// sys_getgid - Get group ID
pub fn sys_getgid(_args: SyscallArgs) -> i64 {
    if let Some(task) = crate::sched::current() {
        task.cred().gid as i64
    } else {
        0
    }
}

/// sys_geteuid - Get effective user ID
pub fn sys_geteuid(_args: SyscallArgs) -> i64 {
    if let Some(task) = crate::sched::current() {
        task.cred().euid as i64
    } else {
        0
    }
}

/// sys_getegid - Get effective group ID
pub fn sys_getegid(_args: SyscallArgs) -> i64 {
    if let Some(task) = crate::sched::current() {
        task.cred().egid as i64
    } else {
        0
    }
}

/// sys_setuid - Set user ID
///
/// # Arguments
/// - args[0]: uid - user ID to set
pub fn sys_setuid(args: SyscallArgs) -> i64 {
    let uid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();
            if crate::security::capable(crate::security::CAP_SETUID) {
                // CAP_SETUID: set all uid fields
                cred.uid = uid;
                cred.euid = uid;
                cred.suid = uid;
                cred.fsuid = uid;
            } else if cred.uid == uid || cred.suid == uid {
                // Unprivileged: can set euid to real or saved uid
                cred.euid = uid;
                cred.fsuid = uid;
            } else {
                return -(errno::EPERM as i64);
            }
            // Leaving euid 0 drops all capabilities (Linux commit_creds)
            // unless PR_SET_KEEPCAPS was set (libcap-ng capng_change_id),
            // or SECBIT_NO_SETUID_FIXUP is in effect (OH init's service
            // path — see Cred::keeps_caps_on_setuid).
            // RETURNING to euid 0 (ruid or suid still 0) restores them from
            // the bounding set — see sys_setresuid for the full rationale.
            if cred.euid != 0 && !cred.keeps_caps_on_setuid() {
                cred.cap_effective = crate::security::capability::Cap::EMPTY;
                cred.cap_permitted = crate::security::capability::Cap::EMPTY;
            } else if cred.euid == 0 && (cred.uid == 0 || cred.suid == 0) {
                let restored =
                    crate::security::capability::Cap::new(cred.cap_bounding.bits());
                cred.cap_effective = restored;
                cred.cap_permitted = restored;
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_setgid - Set group ID
///
/// # Arguments
/// - args[0]: gid - group ID to set
pub fn sys_setgid(args: SyscallArgs) -> i64 {
    let gid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();
            if crate::security::capable(crate::security::CAP_SETGID) {
                // CAP_SETGID: set all gid fields
                cred.gid = gid;
                cred.egid = gid;
                cred.sgid = gid;
                cred.fsgid = gid;
            } else if cred.gid == gid || cred.sgid == gid {
                // Unprivileged: can set egid to real or saved gid
                cred.egid = gid;
                cred.fsgid = gid;
            } else {
                return -(errno::EPERM as i64);
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_setreuid - Set real and effective user ID
///
/// # Arguments
/// - args[0]: ruid - real user ID (-1 to leave unchanged)
/// - args[1]: euid - effective user ID (-1 to leave unchanged)
pub fn sys_setreuid(args: SyscallArgs) -> i64 {
    let ruid = args[0] as i32;
    let euid = args[1] as i32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();
            let old_ruid = cred.uid;
            let old_euid = cred.euid;
            let old_suid = cred.suid;

            // Determine new ruid
            let new_ruid = if ruid == -1 {
                old_ruid
            } else if crate::security::capable(crate::security::CAP_SETUID) || ruid as u32 == old_ruid || ruid as u32 == old_euid || ruid as u32 == old_suid {
                ruid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new euid
            let new_euid = if euid == -1 {
                old_euid
            } else if crate::security::capable(crate::security::CAP_SETUID) || euid as u32 == old_ruid || euid as u32 == old_euid || euid as u32 == old_suid {
                euid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            cred.uid = new_ruid;
            cred.euid = new_euid;
            cred.fsuid = new_euid;
            // Linux __sys_setreuid: the saved uid follows the new euid
            // only when ruid was set, OR when euid was set to a value
            // DIFFERENT from the old real uid ("suid = euid" if
            // ruid != -1 || (euid != -1 && euid != old->uid)). The old
            // "either argument" rule updated suid on setreuid(-1, root)
            // from uid 0 — LTP setreuid02 expects the saved uid to stay
            // nobody there, and setregid03's setregid(-1, secondary)
            // wrongly EPERM'd because sgid had followed the wrong egid.
            if ruid != -1 || (euid != -1 && new_euid != old_ruid) {
                cred.suid = new_euid;
            }
            // Leaving euid 0 drops all capabilities (Linux commit_creds)
            // unless PR_SET_KEEPCAPS was set (libcap-ng capng_change_id).
            // RETURNING to euid 0 (ruid or suid still 0) restores them from
            // the bounding set (LTP creat08: root → setreuid(-1,nobody) →
            // setreuid(-1,0) must regain CAP_FSETID to keep S_ISGID on
            // files created in a setgid directory).
            if cred.euid != 0 && !cred.keeps_caps_on_setuid() {
                cred.cap_effective = crate::security::capability::Cap::EMPTY;
                cred.cap_permitted = crate::security::capability::Cap::EMPTY;
            } else if cred.euid == 0 && (new_ruid == 0 || cred.suid == 0) {
                let restored =
                    crate::security::capability::Cap::new(cred.cap_bounding.bits());
                cred.cap_effective = restored;
                cred.cap_permitted = restored;
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_setregid - Set real and effective group ID
///
/// # Arguments
/// - args[0]: rgid - real group ID (-1 to leave unchanged)
/// - args[1]: egid - effective group ID (-1 to leave unchanged)
pub fn sys_setregid(args: SyscallArgs) -> i64 {
    let rgid = args[0] as i32;
    let egid = args[1] as i32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();
            let old_rgid = cred.gid;
            let old_egid = cred.egid;
            let old_sgid = cred.sgid;

            // Determine new rgid
            let new_rgid = if rgid == -1 {
                old_rgid
            } else if crate::security::capable(crate::security::CAP_SETGID) || rgid as u32 == old_rgid || rgid as u32 == old_egid || rgid as u32 == old_sgid {
                rgid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new egid
            let new_egid = if egid == -1 {
                old_egid
            } else if crate::security::capable(crate::security::CAP_SETGID) || egid as u32 == old_rgid || egid as u32 == old_egid || egid as u32 == old_sgid {
                egid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            cred.gid = new_rgid;
            cred.egid = new_egid;
            cred.fsgid = new_egid;
            // Linux __sys_setregid mirror of setreuid: the saved gid
            // follows the new egid only when rgid was set, OR when egid
            // was set to a value DIFFERENT from the old real gid.
            if rgid != -1 || (egid != -1 && new_egid != old_rgid) {
                cred.sgid = new_egid;
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_setresuid - Set real, effective, and saved user ID
///
/// # Arguments
/// - args[0]: ruid - real user ID (-1 to leave unchanged)
/// - args[1]: euid - effective user ID (-1 to leave unchanged)
/// - args[2]: suid - saved user ID (-1 to leave unchanged)
pub fn sys_setresuid(args: SyscallArgs) -> i64 {
    let ruid = args[0] as i32;
    let euid = args[1] as i32;
    let suid = args[2] as i32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();
            let old_euid = cred.euid;

            // Determine new ruid
            let new_ruid = if ruid == -1 {
                cred.uid
            } else if crate::security::capable(crate::security::CAP_SETUID)
                || ruid as u32 == cred.uid
                || ruid as u32 == cred.euid
                || ruid as u32 == cred.suid
            {
                ruid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new euid
            let new_euid = if euid == -1 {
                cred.euid
            } else if crate::security::capable(crate::security::CAP_SETUID)
                || euid as u32 == cred.uid
                || euid as u32 == cred.euid
                || euid as u32 == cred.suid
            {
                euid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new suid
            let new_suid = if suid == -1 {
                cred.suid
            } else if crate::security::capable(crate::security::CAP_SETUID)
                || suid as u32 == cred.uid
                || suid as u32 == cred.euid
                || suid as u32 == cred.suid
            {
                suid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            cred.uid = new_ruid;
            cred.euid = new_euid;
            cred.suid = new_suid;
            cred.fsuid = new_euid;
            // Linux credential-transition rules (capabilities(7)):
            // - leaving euid 0 → capabilities are cleared (unless
            //   PR_SET_KEEPCAPS), and
            // - RETURNING to euid 0 (from a nonzero euid, with ruid or
            //   suid still 0) → the permitted/effective sets are restored
            //   from the bounding set. Without the restore, a root process
            //   that setresuid()s to another user and back keeps EMPTY
            //   capability sets and loses CAP_SETUID forever (LTP
            //   setresuid01: setresuid(main,-1,-1) after the round trip).
            if cred.euid != 0 && !cred.keeps_caps_on_setuid() {
                cred.cap_effective = crate::security::capability::Cap::EMPTY;
                cred.cap_permitted = crate::security::capability::Cap::EMPTY;
            } else if cred.euid == 0
                && old_euid != 0
                && (new_ruid == 0 || new_suid == 0)
            {
                let restored = crate::security::capability::Cap::new(cred.cap_bounding.bits());
                cred.cap_effective = restored;
                cred.cap_permitted = restored;
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_getresuid - Get real, effective, and saved user ID
///
/// # Arguments
/// - args[0]: ruid - pointer to store real user ID
/// - args[1]: euid - pointer to store effective user ID
/// - args[2]: suid - pointer to store saved user ID
pub fn sys_getresuid(args: SyscallArgs) -> i64 {
    let ruid_ptr = args[0] as *mut u32;
    let euid_ptr = args[1] as *mut u32;
    let suid_ptr = args[2] as *mut u32;

    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred() returns a reference.
        let cred = unsafe { (*task).cred() };
        // SAFETY: task is valid; user pointers are validated with access_ok before each write.
        unsafe {
            if !ruid_ptr.is_null() {
                if !crate::arch::uaccess::access_ok(ruid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                let _ = crate::arch::uaccess::put_user(ruid_ptr, cred.uid);
            }
            if !euid_ptr.is_null() {
                if !crate::arch::uaccess::access_ok(euid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                let _ = crate::arch::uaccess::put_user(euid_ptr, cred.euid);
            }
            if !suid_ptr.is_null() {
                if !crate::arch::uaccess::access_ok(suid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                let _ = crate::arch::uaccess::put_user(suid_ptr, cred.suid);
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_setresgid - Set real, effective, and saved group ID
///
/// # Arguments
/// - args[0]: rgid - real group ID (-1 to leave unchanged)
/// - args[1]: egid - effective group ID (-1 to leave unchanged)
/// - args[2]: sgid - saved group ID (-1 to leave unchanged)
pub fn sys_setresgid(args: SyscallArgs) -> i64 {
    let rgid = args[0] as i32;
    let egid = args[1] as i32;
    let sgid = args[2] as i32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred_mut() returns
        // a mutable reference to the task's credential structure.
        unsafe {
            let cred = (*task).cred_mut();

            // Determine new rgid
            let new_rgid = if rgid == -1 {
                cred.gid
            } else if crate::security::capable(crate::security::CAP_SETGID)
                || rgid as u32 == cred.gid
                || rgid as u32 == cred.egid
                || rgid as u32 == cred.sgid
            {
                rgid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new egid
            let new_egid = if egid == -1 {
                cred.egid
            } else if crate::security::capable(crate::security::CAP_SETGID)
                || egid as u32 == cred.gid
                || egid as u32 == cred.egid
                || egid as u32 == cred.sgid
            {
                egid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            // Determine new sgid
            let new_sgid = if sgid == -1 {
                cred.sgid
            } else if crate::security::capable(crate::security::CAP_SETGID)
                || sgid as u32 == cred.gid
                || sgid as u32 == cred.egid
                || sgid as u32 == cred.sgid
            {
                sgid as u32
            } else {
                return -(errno::EPERM as i64);
            };

            cred.gid = new_rgid;
            cred.egid = new_egid;
            cred.sgid = new_sgid;
            cred.fsgid = new_egid;
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_getresgid - Get real, effective, and saved group ID
///
/// # Arguments
/// - args[0]: rgid - pointer to store real group ID
/// - args[1]: egid - pointer to store effective group ID
/// - args[2]: sgid - pointer to store saved group ID
pub fn sys_getresgid(args: SyscallArgs) -> i64 {
    let rgid_ptr = args[0] as *mut u32;
    let egid_ptr = args[1] as *mut u32;
    let sgid_ptr = args[2] as *mut u32;

    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred() returns a reference.
        let cred = unsafe { (*task).cred() };
        // SAFETY: task is valid; user pointers validated with access_ok before each write.
        unsafe {
            if !rgid_ptr.is_null() {
                if !crate::arch::uaccess::access_ok(rgid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                let _ = crate::arch::uaccess::put_user(rgid_ptr, cred.gid);
            }
            if !egid_ptr.is_null() {
                if !crate::arch::uaccess::access_ok(egid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                let _ = crate::arch::uaccess::put_user(egid_ptr, cred.egid);
            }
            if !sgid_ptr.is_null() {
                if !crate::arch::uaccess::access_ok(sgid_ptr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                let _ = crate::arch::uaccess::put_user(sgid_ptr, cred.sgid);
            }
        }
        0
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_getgroups - Get supplementary group IDs
///
/// # Arguments
/// - args[0]: size - size of groups array
/// - args[1]: list - pointer to group ID array
///
/// # Returns
/// Number of groups on success, negative error on failure
pub fn sys_getgroups(args: SyscallArgs) -> i64 {
    let size = args[0] as i32;
    let list_ptr = args[1] as *mut u32;

    let task = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    let ngroups = task.cred().groups.len();

    if size == 0 {
        return ngroups as i64;
    }
    if size < 0 {
        return -(errno::EINVAL as i64);
    }
    if (size as usize) < ngroups {
        return -(errno::EINVAL as i64);
    }

    // Copy group list to userspace using copy_to_user
    if ngroups > 0 {
        let groups = &task.cred().groups;
        let bytes = ngroups * core::mem::size_of::<u32>();
        unsafe {
            crate::arch::uaccess::copy_to_user(
                list_ptr as *mut u8,
                groups.as_ptr() as *const u8,
                bytes,
            );
        }
    }

    ngroups as i64
}

/// sys_setgroups - Set supplementary group IDs
///
/// # Arguments
/// - args[0]: size - number of groups
/// - args[1]: list - pointer to group ID array
pub fn sys_setgroups(args: SyscallArgs) -> i64 {
    let size = args[0] as i32;
    let list_ptr = args[1] as *const u32;

    // Only CAP_SETGID can set supplementary groups
    if !crate::security::capable(crate::security::CAP_SETGID) {
        return -(errno::EPERM as i64);
    }

    if size < 0 || (size as usize) > 65536 {
        return -(errno::EINVAL as i64);
    }
    if size == 0 {
        // setgroups(0, NULL) CLEARS the supplementary group list (POSIX);
        // the old code returned success without touching the current set
        // (review P28).
        if let Some(t) = crate::sched::current() {
            // SAFETY: t is the running task; groups lives on its Cred.
            unsafe { (*t).cred_mut().groups.clear(); }
        }
        return 0;
    }

    // Validate user pointer
    if !crate::arch::uaccess::access_ok(list_ptr as usize, (size as usize) * 4) {
        return -(errno::EFAULT as i64);
    }

    // Read group IDs from userspace through the exception-table copy path
    // (SUM=0 safe). copy_from_user zero-fills the uncopied tail on failure,
    // so a partial read leaves zero group ids rather than garbage.
    let mut groups = alloc::vec::Vec::new();
    groups.resize(size as usize, 0u32);
    // SAFETY: copy_from_user is the exception-table copy path; a partial
    // fault (guarded buffers, bad pointers) MUST surface as EFAULT — the
    // return value was previously dropped and the syscall silently
    // installed zero-filled groups (LTP setgroups03's EFAULT case).
    unsafe {
        if crate::arch::uaccess::copy_from_user(
            groups.as_mut_ptr() as *mut u8,
            list_ptr as *const u8,
            size as usize * 4,
        ) != 0
        {
            return -(errno::EFAULT as i64);
        }
    }

    // Store in task credentials
    if let Some(task) = crate::sched::current() {
        task.cred_mut().groups = groups;
    }

    0
}

/// sys_setpgid - Set process group ID
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
/// - args[1]: pgid - process group ID (0 = pid)
pub fn sys_setpgid(args: SyscallArgs) -> i64 {
    let target_pid = args[0] as i32;
    let pgid = args[1] as i32;

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // SAFETY: current is a valid task pointer from sched::current().
    let current_pid = unsafe { (*current).pid() };

    // A negative pgid is EINVAL (LTP setpgid02: setpgid(pid, -1)).
    if pgid < 0 {
        return -(errno::EINVAL as i64);
    }

    // Resolve target pid
    let target_pid = if target_pid == 0 {
        current_pid as i32
    } else {
        target_pid
    };

    // Resolve pgid
    let pgid = if pgid == 0 {
        target_pid
    } else {
        pgid
    };

    // The target process group must exist: a pgid names a process-group
    // LEADER (a live pid). Joining a nonexistent group is EPERM (LTP
    // setpgid02's pid_max case).
    if pgid != target_pid
        && unsafe { crate::sched::find_task_by_pid(pgid as u32) }.is_null()
    {
        return -(errno::EPERM as i64);
    }

    // Cannot set pgid for processes in different sessions
    if target_pid as u32 == current_pid {
        // Setting own pgid
        // SAFETY: current is a valid task pointer from sched::current().
        unsafe {
            // Joining a process group whose LEADER lives in another
            // session is EPERM (LTP setpgid03: parent joins the
            // setsid()'d child's group).
            if pgid as u32 != current_pid {
                let leader = crate::sched::find_task_by_pid(pgid as u32);
                if leader.is_null() || (*leader).sid() != (*current).sid() {
                    return -(errno::EPERM as i64);
                }
            }
            (*current).set_pgid(pgid as u32);
        }
    } else {
        // Setting child's pgid
        // SAFETY: find_task_by_pid returns valid pointer when non-null.
        let target = unsafe { crate::sched::find_task_by_pid(target_pid as u32) };
        if target.is_null() {
            return -(errno::ESRCH as i64);
        }
        // SAFETY: target is validated non-null; current is valid from sched::current().
        unsafe {
            // Target must be a child of current process
            if (*target).ppid() != current_pid {
                return -(errno::ESRCH as i64);
            }
            // Target must be in same session
            if (*target).sid() != (*current).sid() {
                return -(errno::EPERM as i64);
            }
            // Once the child has exec'd, the parent may no longer move it
            // between groups (Linux sys_setpgid EACCES; LTP setpgid03's
            // exec'd-child case).
            if (*target).has_execed() {
                return -(errno::EACCES as i64);
            }
            (*target).set_pgid(pgid as u32);
        }
    }

    0
}

/// sys_getpgid - Get process group ID
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
pub fn sys_getpgid(args: SyscallArgs) -> i64 {
    let pid = args[0] as i32;

    if pid == 0 {
        if let Some(task) = crate::sched::current() {
            // SAFETY: task pointer from sched::current() is valid.
            return unsafe { (*task).pgid() as i64 };
        }
        return -(errno::ESRCH as i64);
    }

    // SAFETY: find_task_by_pid returns valid pointer when non-null.
    let target = unsafe { crate::sched::find_task_by_pid(pid as u32) };
    if target.is_null() {
        return -(errno::ESRCH as i64);
    }
    // SAFETY: target validated non-null above.
    unsafe { (*target).pgid() as i64 }
}

/// sys_setsid - Create a new session
///
/// # Returns
/// New session ID on success, negative error on failure
pub fn sys_setsid(_args: SyscallArgs) -> i64 {
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // SAFETY: current is a valid, non-null task pointer from sched::current().
    unsafe {
        let pid = (*current).pid();

        // Process must not be a process group leader
        if (*current).pgid() == pid {
            return -(errno::EPERM as i64);
        }

        // Create new session and process group
        (*current).set_sid(pid);
        (*current).set_pgid(pid);

        pid as i64
    }
}

/// sys_getsid - Get session ID
///
/// # Arguments
/// - args[0]: pid - process ID (0 = current)
pub fn sys_getsid(args: SyscallArgs) -> i64 {
    let pid = args[0] as i32;

    if pid == 0 {
        if let Some(task) = crate::sched::current() {
            // SAFETY: task pointer from sched::current() is valid.
            return unsafe { (*task).sid() as i64 };
        }
        return -(errno::ESRCH as i64);
    }

    // SAFETY: find_task_by_pid returns valid pointer when non-null.
    let target = unsafe { crate::sched::find_task_by_pid(pid as u32) };
    if target.is_null() {
        return -(errno::ESRCH as i64);
    }
    // SAFETY: target validated non-null above.
    unsafe { (*target).sid() as i64 }
}

/// sys_prlimit64 - Get/set resource limits (NR 261)
///
/// prlimit64(pid, resource, const struct rlimit64 *new, struct rlimit64 *old)
///
/// Real per-task storage (task.rlimits) with Linux semantics:
///  - EINVAL: resource out of range or new rlim_cur > rlim_max
///  - EPERM:  raising rlim_max (or rlim_cur above the old max) without
///            CAP_SYS_RESOURCE; targeting another process (no ptrace
///            infrastructure yet, so only self is permitted)
///  - EFAULT: bad user pointers
/// RLIMIT_STACK stores also reprogram the mm stack-limit (GROWSDOWN
/// fault bound), keeping try_expand_stack in sync with the rlimit.
pub fn sys_prlimit64(args: SyscallArgs) -> i64 {
    use crate::process::task::{rlimit_res, RLIM_NLIMITS};

    let pid = args[0] as i32;
    let resource = args[1] as i32;
    let new_rlim = args[2] as *const u8;
    let old_rlim = args[3] as *mut u8;

    if resource < 0 || resource as usize >= RLIM_NLIMITS {
        return -(errno::EINVAL as i64);
    }
    let resource = resource as usize;

    // Validate pointers
    if !new_rlim.is_null() && !crate::arch::uaccess::access_ok(new_rlim as usize, 16) {
        return -(errno::EFAULT as i64);
    }
    if !old_rlim.is_null() && !crate::arch::uaccess::access_ok(old_rlim as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    // Cross-process prlimit64 needs PTRACE_MODE_ATTACH; ptrace is not
    // implemented, so only self (pid 0 or own pid) is accepted.
    if pid != 0 && pid != current.pid() as i32 {
        return -(errno::EPERM as i64);
    }

    // Snapshot old values (and apply the set below while holding the lock).
    // lock_irqsave: scheduler_tick reads rlimits from IRQ context — an
    // interrupted holder would wedge the tick (see Task::rlimit).
    let old_vals = *current.rlimits.lock_irqsave();

    if !new_rlim.is_null() {
        // SAFETY: new_rlim validated with access_ok(16); reads two u64s.
        let new_vals = unsafe {
            let mut buf = [0u64; 2];
            if crate::arch::uaccess::copy_from_user(
                buf.as_mut_ptr() as *mut u8,
                new_rlim,
                core::mem::size_of::<[u64; 2]>(),
            ) != 0 {
                return -(errno::EFAULT as i64);
            }
            buf
        };
        let (req_cur, req_max) = (new_vals[0], new_vals[1]);

        if req_cur > req_max {
            return -(errno::EINVAL as i64);
        }

        // RLIMIT_NOFILE hard-limit ceiling (Linux do_prlimit): raising
        // rlim_max above the fs.nr_open sysctl default (1024*1024) is
        // EPERM even for root. musl's setrlimit() maps to prlimit64 on
        // the generic 64-bit syscall table (no setrlimit there), so this
        // check must live HERE as well (LTP setrlimit03 sets
        // rlim_max = nr_open+1 as root and requires EPERM).
        if resource == rlimit_res::NOFILE && req_max > 1024 * 1024 {
            return -(errno::EPERM as i64);
        }

        // Raising the hard limit — or the soft above the old hard —
        // needs CAP_SYS_RESOURCE (kernel/fork.c / do_prlimit).
        let (_, old_max) = old_vals[resource];
        let privileged = current.cred().euid == 0
            || crate::security::capable(crate::security::CAP_SYS_RESOURCE);
        if (req_max > old_max || req_cur > old_max) && !privileged {
            return -(errno::EPERM as i64);
        }

        current.rlimits.lock_irqsave()[resource] = (req_cur, req_max);

        // Side effect: keep the mm stack bound in sync with RLIMIT_STACK
        // (try_expand_stack faults on addresses below mm.stack_limit).
        if resource == rlimit_res::STACK {
            sync_stack_limit(current, req_cur);
        }
    }

    // Return old/current limits
    if !old_rlim.is_null() {
        let rlimit: [u64; 2] = [old_vals[resource].0, old_vals[resource].1];
        // SAFETY: old_rlim validated non-null and access_ok above.
        let uncopied = unsafe {
            crate::arch::uaccess::copy_to_user(
                old_rlim,
                rlimit.as_ptr() as *const u8,
                core::mem::size_of::<[u64; 2]>(),
            )
        };
        if uncopied != 0 {
            return -(errno::EFAULT as i64);
        }
    }

    0
}

/// Reprogram the address space's stack growth bound from an
/// RLIMIT_STACK soft limit (used by setrlimit/prlimit64 and exec).
fn sync_stack_limit(task: &crate::process::task::Task, rlim_cur: u64) {
    use crate::arch::mm::user_addr::STACK_MAX_SIZE;

    if let Some(aspace) = task.address_space() {
        let stack_top = aspace.start_stack();
        if stack_top != 0 {
            let size = (rlim_cur.min(STACK_MAX_SIZE as u64)) as usize;
            let limit = stack_top.saturating_sub(size) + crate::mm::page::PAGE_SIZE;
            aspace.set_stack_limit(limit);
        }
    }
}

/// sys_prctl - manipulate process attributes
///
/// Arguments: (option, arg2, arg3, arg4, arg5)
pub fn sys_prctl(args: SyscallArgs) -> i64 {
    use crate::arch::uaccess::{copy_to_user, strncpy_from_user};

    let option = args[0] as i32;
    let arg2 = args[1];
    let arg3 = args[2];
    let arg4 = args[3];
    let _arg5 = args[4];

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    match option {
        1 => {
            // PR_SET_PDEATHSIG
            let sig = arg2 as i32;
            if sig < 0 || sig > 64 {
                return -(errno::EINVAL as i64);
            }
            // SAFETY: current is a valid task pointer from sched::current().
            unsafe { (*current).pdeath_signal = sig as u32; }
            0
        }
        2 => {
            // PR_GET_PDEATHSIG
            let ptr = arg2 as *mut u32;
            if ptr.is_null() {
                return -(errno::EFAULT as i64);
            }
            if !crate::arch::uaccess::access_ok(ptr as usize, 4) {
                return -(errno::EFAULT as i64);
            }
            // SAFETY: ptr validated with access_ok; put_user is the
            // exception-table copy path.
            unsafe {
                let _ = crate::arch::uaccess::put_user(ptr, (*current).pdeath_signal);
            }
            0
        }
        3 => {
            // PR_GET_DUMPABLE
            // SAFETY: current is a valid task pointer.
            unsafe { (*current).dumpable as i64 }
        }
        4 => {
            // PR_SET_DUMPABLE
            let val = arg2 as u32;
            if val > 1 {
                return -(errno::EINVAL as i64);
            }
            // SAFETY: current is a valid task pointer.
            unsafe { (*current).dumpable = val; }
            0
        }
        15 => {
            // PR_SET_NAME
            let ptr = arg2 as *const u8;
            if ptr.is_null() {
                return -(errno::EFAULT as i64);
            }
            let mut buf = [0u8; 16];
            match strncpy_from_user(ptr, 16, &mut buf) {
                // SAFETY: current is valid; buf contains safely copied user data.
                Ok(_) => unsafe { (*current).set_comm(&buf); },
                Err(_) => return -(errno::EFAULT as i64),
            }
            0
        }
        16 => {
            // PR_GET_NAME
            let ptr = arg2 as *mut u8;
            if ptr.is_null() {
                return -(errno::EFAULT as i64);
            }
            if !crate::arch::uaccess::access_ok(ptr as usize, 16) {
                return -(errno::EFAULT as i64);
            }
            // SAFETY: ptr validated with access_ok; current is valid; comm() returns a
            // reference to a fixed-size internal buffer of 16 bytes.
            unsafe {
                let comm = (*current).comm();
                let _ = copy_to_user(ptr, comm.as_ptr(), 16);
            }
            0
        }
        36 => {
            // PR_SET_CHILD_SUBREAPER
            let val = arg2 as u32;
            // SAFETY: current is a valid task pointer; signal field is an Option.
            unsafe {
                if let Some(ref sig) = (*current).signal {
                    sig.is_child_subreaper.store(val != 0, core::sync::atomic::Ordering::Relaxed);
                }
            }
            0
        }
        37 => {
            // PR_GET_CHILD_SUBREAPER: the value is WRITTEN to the (int *)
            // in arg2 and the syscall returns 0 (Linux kernel/sys.c:
            // put_user(!!sig->is_child_subreaper, (int __user *)arg2)).
            // Returning the flag itself left the caller's variable
            // uninitialized — LTP prctl03's final check read stack garbage
            // (0) after a successful PR_SET, and the check that expected 0
            // passed only by luck.
            // SAFETY: current is a valid task pointer; signal field is an Option.
            unsafe {
                let val: i32 = if let Some(ref sig) = (*current).signal {
                    sig.is_child_subreaper.load(core::sync::atomic::Ordering::Relaxed) as i32
                } else {
                    0
                };
                // Returns uncopied byte count (0 = success).
                if crate::arch::uaccess::copy_to_user(
                    arg2 as *mut u8,
                    val.to_ne_bytes().as_ptr(),
                    4,
                ) == 0
                {
                    0
                } else {
                    -14  // EFAULT
                }
            }
        }
        29 => {
            // PR_SET_TIMERSLACK: nanoseconds; 0 resets to the process's
            // DEFAULT slack (Linux kernel/sys.c PR_SET_TIMERSLACK). The
            // default is set at fork to the parent's CURRENT value
            // (kernel/fork.c copy_process), so a forked child that resets
            // lands on the inherited value, not the boot 50 us — LTP
            // prctl08 "Child process" case.
            // SAFETY: current is a valid task pointer; signal field is an Option.
            unsafe {
                if let Some(ref sig) = (*current).signal {
                    let slack_ns = if arg2 == 0 {
                        sig.timer_slack_default_ns
                            .load(core::sync::atomic::Ordering::Relaxed)
                    } else {
                        arg2
                    };
                    sig.timer_slack_ns.store(slack_ns, core::sync::atomic::Ordering::Relaxed);
                }
            }
            0
        }
        30 => {
            // PR_GET_TIMERSLACK
            // SAFETY: current is a valid task pointer; signal field is an Option.
            unsafe {
                if let Some(ref sig) = (*current).signal {
                    sig.timer_slack_ns.load(core::sync::atomic::Ordering::Relaxed) as i64
                } else {
                    50_000
                }
            }
        }
        21 => {
            // PR_GET_SECCOMP (U1c): current seccomp mode.
            // SAFETY: current is a valid task pointer.
            unsafe { (*current).seccomp_mode() as i64 }
        }
        22 => {
            // PR_SET_SECCOMP (U1c): arg2 = mode (1 strict / 2 filter with
            // arg3 = struct sock_fprog *).
            prctl_set_seccomp(arg2, arg3)
        }
        39 => {
            // PR_SET_NO_NEW_PRIVS (seccomp-grade sandboxing prerequisite).
            // Reference shape (kernel/sys.c, pinned by LTP prctl02): the
            // attribute is ONE-WAY — arg2 must be exactly 1 (there is no
            // clear; un-setting would defeat the flag's purpose), and
            // arg3..arg5 must all be 0; anything else is EINVAL. Once
            // set: execve ignores setuid/setgid bits (the exec'd program
            // runs with the caller's credentials) and seccomp filter
            // attach is legal without CAP_SYS_ADMIN.
            if arg2 != 1 || arg3 != 0 || arg4 != 0 || args[4] != 0 {
                return -(errno::EINVAL as i64);
            }
            // SAFETY: current is a valid task pointer.
            unsafe { (*current).set_no_new_privs(true); }
            0
        }
        40 => {
            // PR_GET_NO_NEW_PRIVS: returns the attribute (0 or 1); any
            // nonzero arg2..arg5 is EINVAL (reference shape, prctl02).
            if arg2 != 0 || arg3 != 0 || arg4 != 0 || args[4] != 0 {
                return -(errno::EINVAL as i64);
            }
            // SAFETY: current is a valid task pointer.
            unsafe { (*current).no_new_privs() as i64 }
        }
        7 => {
            // PR_GET_KEEPCAPS
            // SAFETY: current is a valid task pointer.
            unsafe { (*current).cred().keepcaps as i64 }
        }
        8 => {
            // PR_SET_KEEPCAPS: arg2 must be 0 or 1. Setting the flag keeps
            // the capability sets across a uid transition away from 0
            // (used by libcap-ng capng_change_id before setuid).
            if arg2 > 1 {
                return -(errno::EINVAL as i64);
            }
            // SAFETY: current is a valid task pointer; cred_mut() returns
            // a mutable reference to the task's credential structure.
            unsafe { (*current).cred_mut().keepcaps = arg2 != 0; }
            0
        }
        23 => {
            // PR_CAPBSET_READ: is capability arg2 in the bounding set?
            if arg2 as u32 > crate::security::capability::CAP_LAST_CAP {
                return -(errno::EINVAL as i64);
            }
            // SAFETY: current is a valid task pointer.
            unsafe {
                (*current).cred().cap_bounding.has(arg2 as u32) as i64
            }
        }
        24 => {
            // PR_CAPBSET_DROP: remove capability arg2 from the bounding
            // set. Requires CAP_SETPCAP (Linux cap_capable check).
            if arg2 as u32 > crate::security::capability::CAP_LAST_CAP {
                return -(errno::EINVAL as i64);
            }
            if !crate::security::capable(crate::security::CAP_SETPCAP) {
                return -(errno::EPERM as i64);
            }
            // SAFETY: current is a valid task pointer.
            unsafe {
                (*current).cred_mut().cap_bounding.clear(arg2 as u32);
            }
            0
        }
        27 => {
            // PR_GET_SECUREBITS: current SECBIT_* mask.
            // SAFETY: current is a valid task pointer.
            unsafe { (*current).cred().securebits as i64 }
        }
        28 => {
            // PR_SET_SECUREBITS: arg2 = SECBIT_* mask (8 defined bits).
            // OH's init programs SECBIT_NO_SETUID_FIXUP(+_LOCKED) and
            // SECBIT_KEEP_CAPS(+_LOCKED) on its display services before
            // exec; EINVAL there made init abort the child
            // (INIT_EKEEPCAP) and composer_host/render_service/
            // bootanimation never started. SECBIT_KEEP_CAPS mirrors the
            // PR_SET_KEEPCAPS flag so the setuid cap-retention path sees
            // one consistent bit.
            if arg2 > 0xff {
                return -(errno::EINVAL as i64);
            }
            // Capability gate (prctl(2)): raising securebits requires
            // CAP_SETPCAP, else EPERM. Root init holds every capability;
            // LTP prctl02 case #16 drops CAP_SETPCAP+CAP_SYS_ADMIN and
            // requires EPERM where we used to succeed.
            if !crate::security::capable(crate::security::CAP_SETPCAP) {
                return -(errno::EPERM as i64);
            }
            // SAFETY: current is a valid task pointer; cred_mut returns
            // the task's own credential structure.
            unsafe {
                let cred = (*current).cred_mut();
                // Locked bits never clear once set (issecure() rule);
                // honor it by OR-ing the locked-alive bits back.
                let old = cred.securebits;
                let mut new_bits = arg2 as u32;
                for (bit, lock) in [(0u32, 1u32), (2, 3), (4, 5), (6, 7)] {
                    if old & (1 << lock) != 0 {
                        new_bits |= 1 << bit; // locked-set bits stay set
                        new_bits |= 1 << lock;
                    }
                }
                cred.securebits = new_bits;
                cred.keepcaps = new_bits & crate::process::task::SECBIT_KEEP_CAPS != 0;
            }
            0
        }
        47 => {
            // PR_CAP_AMBIENT: arg2 selects the sub-operation
            // (1 IS_SET / 2 RAISE / 3 LOWER / 4 CLEAR_ALL), arg3 is the
            // capability. OH's init raises ambient caps on its services
            // (SetAmbientCapability) — EINVAL aborted the service child
            // with INIT_ECAP before exec.
            const PR_CAP_AMBIENT_IS_SET: u64 = 1;
            const PR_CAP_AMBIENT_RAISE: u64 = 2;
            const PR_CAP_AMBIENT_LOWER: u64 = 3;
            const PR_CAP_AMBIENT_CLEAR_ALL: u64 = 4;
            let cap = arg3 as u32;
            match arg2 {
                PR_CAP_AMBIENT_IS_SET => {
                    if cap > crate::security::capability::CAP_LAST_CAP {
                        return -(errno::EINVAL as i64);
                    }
                    // SAFETY: current is a valid task pointer.
                    unsafe { (*current).cred().cap_ambient.has(cap) as i64 }
                }
                PR_CAP_AMBIENT_RAISE => {
                    if cap > crate::security::capability::CAP_LAST_CAP {
                        return -(errno::EINVAL as i64);
                    }
                    // Linux requires the cap in BOTH the permitted and
                    // inheritable sets; init's service children run with
                    // full root sets before dropping.
                    // SAFETY: current is a valid task pointer.
                    unsafe {
                        let cred = (*current).cred();
                        if !cred.cap_permitted.has(cap) || !cred.cap_inheritable.has(cap) {
                            return -(errno::EPERM as i64);
                        }
                    }
                    // SAFETY: current is a valid task pointer.
                    unsafe { (*current).cred_mut().cap_ambient.set(cap); }
                    0
                }
                PR_CAP_AMBIENT_LOWER => {
                    if cap > crate::security::capability::CAP_LAST_CAP {
                        return -(errno::EINVAL as i64);
                    }
                    // SAFETY: current is a valid task pointer.
                    unsafe { (*current).cred_mut().cap_ambient.clear(cap); }
                    0
                }
                PR_CAP_AMBIENT_CLEAR_ALL => {
                    // Linux validates the unused arguments: arg3/arg4/arg5
                    // must be zero for CLEAR_ALL (kernel/sys.c PR_CAP_AMBIENT
                    // arg validation; LTP prctl02 case #13 passes arg3=1 and
                    // requires EINVAL where we used to succeed).
                    if arg3 != 0 || arg4 != 0 || _arg5 != 0 {
                        return -(errno::EINVAL as i64);
                    }
                    // SAFETY: current is a valid task pointer.
                    unsafe { (*current).cred_mut().cap_ambient = crate::security::capability::Cap::EMPTY; }
                    0
                }
                _ => -(errno::EINVAL as i64),
            }
        }
        // PR_SET_VMA (Linux with CONFIG_ANON_VMA_NAME): name an anonymous
        // VMA — (option, addr, size, PR_SET_VMA_ANON_NAME, name). OH's
        // userspace names thread stacks/arenas at creation and treats
        // EINVAL as a failure it retries (bootanimation's thread-creation
        // loop). Accept and ignore the name (we don't track VMA names).
        0x5356_4d41 => {
            // prctl(PR_SET_VMA, PR_SET_VMA_ANON_NAME, addr, size, name):
            // the SUB-OPERATION is arg2 (addr in arg3, size in arg4).
            const PR_SET_VMA_ANON_NAME: u64 = 0;
            if arg2 != PR_SET_VMA_ANON_NAME {
                return -(errno::EINVAL as i64);
            }
            0
        }
        _ => {
            // Bring-up visibility: which unimplemented options callers
            // actually exercise (throttled — hdf_devhost retries some in
            // a loop).
            static UNK_PRCTL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let n = UNK_PRCTL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 32 {
                crate::pr_warn!(
                    "prctl: unimplemented option {} arg2={:#x} (pid {})",
                    option,
                    arg2,
                    crate::process::current_pid()
                );
            }
            -(errno::EINVAL as i64)
        }
    }
}

/// sys_tgkill - send signal to a specific thread of a thread group
///
/// # Arguments
/// - args[0]: tgid - thread group ID (must match the target's tgid)
/// - args[1]: tid - thread ID (exact thread)
/// - args[2]: sig - signal number
pub fn sys_tgkill(args: SyscallArgs) -> i64 {
    // Linux do_tgkill() rejects tgid <= 0 || tid <= 0 with EINVAL
    // BEFORE looking up any task. The naked `as u32` cast below would
    // otherwise turn -1 into u32::MAX and produce ESRCH for what should
    // be a clear EINVAL. LTP tgkill03 / tgkill02 both rely on this.
    let tgid_raw = args[0] as i32;
    let tid_raw = args[1] as i32;
    if tgid_raw <= 0 || tid_raw <= 0 {
        return -(errno::EINVAL as i64);
    }
    let tgid = tgid_raw as u32;
    let tid = tid_raw as u32;
    let sig = args[2] as i32;

    if sig < 0 || sig > 64 {
        return -(errno::EINVAL as i64);
    }

    // Validate target exists and caller has permission, even for sig==0.
    // SAFETY: find_task_by_pid returns valid pointer when non-null; we check null.
    let target = unsafe { crate::sched::find_task_by_pid(tid) };
    if target.is_null() {
        return -(errno::ESRCH as i64);
    }
    // SAFETY: target validated non-null above.
    let target_task = unsafe { &*target };
    // tgid must match: a stale tgid with a live tid is ESRCH (Linux).
    if target_task.tgid() != tgid {
        return -(errno::ESRCH as i64);
    }
    if !crate::security::can_send_signal(target_task.cred()) {
        return -(errno::EPERM as i64);
    }

    // sig==0: just a permission check, don't actually send a signal.
    if sig == 0 {
        return 0;
    }

    // Precise THREAD-directed delivery (no group spread). Uses the
    // SI_TKILL-attributed sender (Linux do_send_specific): glibc raise()
    // and pthread_kill() both land here, so the queued siginfo must carry
    // the sender's pid — otherwise a raise()d fatal signal (SIGTERM
    // suicide etc.) reports si_pid=0 and the sender is unattributable.
    crate::signal::send_signal_from_user_tkill(tid, sig)
        .map(|_| 0)
        // send_signal_* returns the negated errno in Err (e.g. -EAGAIN
        // when RLIMIT_SIGPENDING is hit on an RT signal). Surface that
        // to userspace instead of clobbering every failure into EINVAL
        // — LTP tgkill02 expects EAGAIN (errno 11), not EINVAL (22).
        .unwrap_or_else(|e| e as i64)
}

/// sys_rt_sigqueueinfo - send signal with data to a PROCESS
///
/// Linux ABI: exactly 3 parameters — (pid tgid, sig, siginfo_t *uinfo).
/// The old 4-parameter decode fed garbage as uinfo and always missed the
/// real pointer.
pub fn sys_rt_sigqueueinfo(args: SyscallArgs) -> i64 {
    let tgid = args[0] as u32;
    let sig = args[1] as i32;
    let uinfo = args[2] as *const u8;

    if sig < 0 || sig > 64 {
        return -(errno::EINVAL as i64);
    }
    if uinfo.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(uinfo as usize, 128) {
        return -(errno::EFAULT as i64);
    }

    // Target: the process whose tgid this is (the leader carries the pid
    // == tgid). Deliver to the leader (Linux picks any unblocked thread;
    // leader is the deterministic choice).
    // SAFETY: find_task_by_pid returns valid pointer when non-null.
    let target = unsafe { crate::sched::find_task_by_pid(tgid) };
    if target.is_null() {
        return -(errno::ESRCH as i64);
    }
    // SAFETY: target validated non-null above.
    let target_task = unsafe { &*target };
    if !crate::security::can_send_signal(target_task.cred()) {
        return -(errno::EPERM as i64);
    }

    if sig == 0 {
        return 0;
    }

    // Read si_code (offset 8) — user-sent siginfo must carry a NEGATIVE
    // si_code (Linux: si_code >= 0 is reserved for the kernel → EPERM).
    let mut code_buf = [0u8; 4];
    if unsafe {
        crate::arch::uaccess::copy_from_user(
            code_buf.as_mut_ptr(),
            uinfo.add(8),
            4,
        )
    } != 0
    {
        return -(errno::EFAULT as i64);
    }
    let si_code = i32::from_le_bytes(code_buf);
    if si_code >= 0 {
        return -(errno::EPERM as i64);
    }

    // The payload (si_value at offset 24) MUST survive to the receiver:
    // SA_SIGINFO handlers read info->si_value (LTP rt_sigqueueinfo01 sends
    // sival_int=777 and checks it). Linux rebuilds si_pid/si_uid from the
    // SENDER but keeps the user's si_errno/si_value bytes.
    let mut payload = [0u8; 8]; // sival_int/sival_ptr at siginfo offset 24
    if unsafe {
        crate::arch::uaccess::copy_from_user(
            payload.as_mut_ptr(),
            uinfo.add(24),
            8,
        )
    } != 0
    {
        return -(errno::EFAULT as i64);
    }
    let mut info = crate::signal::SigInfo::new(
        sig,
        si_code,
        crate::process::current_pid(),
        crate::sched::current().map(|c| unsafe { (*c).cred().uid }).unwrap_or(0),
    );
    info.set_value_bytes(&payload);

    crate::signal::send_signal_with_info(tgid, info)
        .map(|_| 0)
        // Propagate the actual errno from send_signal_with_info (e.g.
        // EAGAIN on RLIMIT_SIGPENDING exhaustion). Previous behavior
        // masked every send failure into EINVAL.
        .unwrap_or_else(|e| e as i64)
}

/// sys_rt_tgsigqueueinfo - send signal with data to a precise THREAD
/// (Linux asm-generic NR 240: 4 parameters — tgid, tid, sig, uinfo).
///
/// HILOGD fix: this previously parsed THREE arguments (tgid, tid,
/// uinfo=args[2]) — args[2] is the SIGNAL number, so uinfo became a
/// garbage pointer the size of the signo (e.g. 0x23), failed
/// access_ok, and every caller got EFAULT. hilogd's DFX stack-dump
/// path (rt_tgsigqueueinfo(self-tgid, self-tid, SIGDUMP, &si on its
/// own stack) — b154/sweep1: `EFAULTTRAC: nr=240 a0=<tgid> a1=<tid>`
/// immediately before the hilogd.server SIGSEGV at 0x18) depends on
/// this syscall succeeding; with EFAULT the whole hilog pipeline was
/// dead from boot. Linux reads sig=args[2], uinfo=args[3]
/// (kernel/signal.c sys_rt_tgsigqueueinfo).
pub fn sys_rt_tgsigqueueinfo(args: SyscallArgs) -> i64 {
    let tgid = args[0] as u32;
    let tid = args[1] as u32;
    let sig = args[2] as i32;
    let uinfo = args[3] as *const u8;

    // Linux order (kernel/signal.c): copy siginfo from user FIRST
    // (EFAULT), then valid_signal (EINVAL), then the thread lookup
    // (ESRCH), then the si_code forge check (EPERM), then permission,
    // and sig==0 validates only — no delivery.
    if uinfo.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(uinfo as usize, 128) {
        return -(errno::EFAULT as i64);
    }
    // si_signo (offset 0), si_code (offset 8), si_value.sival_ptr
    // (offset 24) — the fields we deliver; Linux copies the full
    // 128-byte struct and overwrites si_signo with `sig`.
    let mut buf = [0u8; 32];
    if unsafe { crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), uinfo, 32) } != 0 {
        return -(errno::EFAULT as i64);
    }
    let si_code = i32::from_le_bytes(buf[8..12].try_into().unwrap());

    if sig < 0 || sig > 64 {
        return -(errno::EINVAL as i64);
    }

    // Resolve the exact thread.
    // SAFETY: find_task_by_pid returns valid pointer when non-null.
    let target = unsafe { crate::sched::find_task_by_pid(tid) };
    if target.is_null() {
        return -(errno::ESRCH as i64);
    }
    // SAFETY: target validated non-null above.
    let target_task = unsafe { &*target };
    if target_task.tgid() != tgid {
        return -(errno::ESRCH as i64);
    }

    // Linux (do_rt_tgsigqueueinfo): si_code >= 0 or SI_TKILL (-6) means
    // the caller tries to forge a kernel signal source — EPERM.
    if si_code >= 0 || si_code == -6 {
        return -(errno::EPERM as i64);
    }
    if !crate::security::can_send_signal(target_task.cred()) {
        return -(errno::EPERM as i64);
    }
    if sig == 0 {
        return 0;
    }

    let payload: [u8; 8] = buf[24..32].try_into().unwrap();
    let mut info = crate::signal::SigInfo::new(
        sig,
        si_code,
        crate::process::current_pid(),
        crate::sched::current().map(|c| unsafe { (*c).cred().uid }).unwrap_or(0),
    );
    info.set_value_bytes(&payload);

    crate::signal::send_signal_with_info_directed(tid, info)
        .map(|_| 0)
        // Propagate the actual errno (EAGAIN on RLIMIT_SIGPENDING, etc.).
        .unwrap_or_else(|e| e as i64)
}

/// sys_rt_sigtimedwait - synchronously wait for signals
///
/// # Arguments
/// - args[0]: uthese - pointer to signal mask (sigset_t)
/// - args[1]: uinfo - siginfo_t pointer (user output)
/// - args[2]: uts - timeout timespec pointer (user, NULL = block forever)
/// - args[3]: sigsetsize - size of signal mask
pub fn sys_rt_sigtimedwait(args: SyscallArgs) -> i64 {
    use crate::signal::SigInfo;

    let uthese = args[0] as *const u64;
    let uinfo = args[1] as *mut u8;
    let uts = args[2] as *const u8;
    let sigsetsize = args[3] as usize;

    if uthese.is_null() {
        return -(errno::EFAULT as i64);
    }
    if sigsetsize < 8 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::uaccess::access_ok(uthese as usize, 8) {
        return -(errno::EFAULT as i64);
    }
    if !uinfo.is_null()
        && !crate::arch::uaccess::access_ok(uinfo as usize, 128)
    {
        return -(errno::EFAULT as i64);
    }
    if !uts.is_null() && !crate::arch::uaccess::access_ok(uts as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // Signal set to wait for (exception-table copy).
    // SAFETY: uthere validated non-null + access_ok above.
    let sigset = match unsafe {
        crate::arch::uaccess::get_user(uthese)
    } {
        Some(v) => v,
        None => return -(errno::EFAULT as i64),
    };
    // An EMPTY wait set is legal (Linux do_sigtimedwait): the call then
    // blocks until any signal OUTSIDE the set (i.e. any deliverable
    // signal) arrives and returns EINTR, or until the timeout (EAGAIN).
    // Returning EINVAL here broke the LTP sigwait family's test_empty_set
    // and left its killer child's signal unreported.

    // Optional timeout (timespec {tv_sec, tv_nsec}).
    let deadline = if !uts.is_null() {
        let mut ts = [0u8; 16];
        // SAFETY: uts validated with access_ok above.
        let uncopied = unsafe {
            crate::arch::uaccess::copy_from_user(
                ts.as_mut_ptr(), uts as *const u8, 16,
            )
        };
        if uncopied > 0 {
            return -(errno::EFAULT as i64);
        }
        let sec = i64::from_le_bytes(ts[0..8].try_into().unwrap());
        let nsec = i64::from_le_bytes(ts[8..16].try_into().unwrap());
        if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
            return -(errno::EINVAL as i64);
        }
        let hz = crate::drivers::timer::HZ as i64;
        let jiffies = sec.saturating_mul(hz).saturating_add(nsec.saturating_mul(hz) / 1_000_000_000);
        Some(crate::drivers::timer::get_jiffies().saturating_add(jiffies.max(1) as u64))
    } else {
        None
    };

    let current = match crate::sched::current() {
        Some(c) => c as *const _ as *mut crate::process::task::Task,
        None => return -(errno::EPERM as i64),
    };

    loop {
        // Dequeue the lowest-numbered pending signal in the set —
        // sigtimedwait CONSUMES the signal (no handler runs for it).
        // SAFETY: current is the running task pointer.
        let pending = unsafe { (*current).pending.get_all() };
        let ready = pending & sigset;
        if ready != 0 {
            let sig = ready.trailing_zeros() as i32 + 1;
            // SAFETY: current is the running task pointer.
            let info = unsafe { (*current).pending.remove_one(sig) }
                .unwrap_or_else(|| SigInfo::new(sig, crate::signal::si_code::SI_USER, 0, 0));
            if !uinfo.is_null() {
                let src = &info as *const SigInfo as *const u8;
                // SAFETY: uinfo validated access_ok(128); exception-table copy.
                let uncopied = unsafe {
                    crate::arch::uaccess::copy_to_user(uinfo, src, 128)
                };
                if uncopied > 0 {
                    return -(errno::EFAULT as i64);
                }
            }
            return sig as i64;
        }

        // Timeout expired?
        if let Some(dl) = deadline {
            if crate::drivers::timer::get_jiffies() >= dl {
                return -(errno::EAGAIN as i64);
            }
        }

        // Any signal OUTSIDE the wait set (including SIGKILL) must also
        // break the loop so it can be delivered on the way to userspace —
        // without this check a SIGKILL'd task slept forever in this loop
        // (regression round 5, HIGH).
        // SAFETY: current is the running task pointer.
        unsafe {
            if (*current).pending.get_all() & !(*current).sigmask != 0 {
                return -(errno::EINTR as i64);
            }
            (*current).set_state(crate::process::task::TaskState::new(
                crate::process::task::TaskState::INTERRUPTIBLE,
            ));
            // Re-check after marking sleeping (lost-wakeup guard).
            if (*current).pending.get_all() & sigset != 0 {
                (*current).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::RUNNING,
                ));
                // R8-5 (NEW-C2): undo a concurrent wake enqueue.
                crate::sched::dequeue_task(&*current);
                continue;
            }
        }
        // Arm the deadline wakeup BEFORE sleeping: nothing else would wake
        // a task that only has a timeout pending (same pattern as futex).
        crate::arch::cpu::restore_irq(true);
        let timer_id = deadline
            .map(|dl| crate::timer::add_timer_wakeup(
                dl, crate::sched::get_current_pid(),
            ))
            .unwrap_or(0);
        // R32 (NEW-3, sixth site — the futex/sem/mq fix missed this one):
        // pool exhausted → timer_id == 0. Sleeping now would hang forever
        // with no waker — the awaited signal may ALSO be gone (an Ignore
        // disposition discards it in send_signal_locked before it pends),
        // so neither the timer nor signal delivery can wake the task.
        // Undo the INTERRUPTIBLE marking and yield as RUNNABLE instead;
        // the loop re-checks pending and the deadline every pass.
        if deadline.is_some() && timer_id == 0 {
            // SAFETY: current is the running task pointer.
            unsafe {
                (*current).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::RUNNING,
                ));
            }
        }
        crate::sched::schedule();
        if timer_id != 0 {
            crate::timer::del_timer(timer_id);
        }
    }
}

/// sys_getcpu - get CPU number and node
///
/// # Arguments
/// - args[0]: cpuset_ptr - CPU set pointer
/// - args[1]: node_ptr - NUMA node pointer
/// - args[2]: cache_ptr - cache ID pointer
pub fn sys_getcpu(args: SyscallArgs) -> i64 {
    use crate::arch::smp::cpu_id;

    let cpuset_ptr = args[0] as *mut u32;
    let node_ptr = args[1] as *mut u32;
    let _cache_ptr = args[2] as *mut u32;

    // Each output pointer is a single `unsigned` (4 bytes). The old code
    // wrote 16 bytes through each, overflowing user buffers by 12
    // (review syscallb-H-03); writes go through the exception-table copy.
    if !cpuset_ptr.is_null() {
        if !crate::arch::uaccess::access_ok(cpuset_ptr as usize, 4) {
            return -(errno::EFAULT as i64);
        }
        let cpu = cpu_id() as u32;
        // SAFETY: 4-byte validated pointer, exception-table path.
        let uncopied = unsafe {
            crate::arch::uaccess::copy_to_user(
                cpuset_ptr as *mut u8,
                &cpu as *const u32 as *const u8,
                4,
            )
        };
        if uncopied > 0 {
            return -(errno::EFAULT as i64);
        }
    }
    if !node_ptr.is_null() {
        if !crate::arch::uaccess::access_ok(node_ptr as usize, 4) {
            return -(errno::EFAULT as i64);
        }
        let node = 0u32;
        // SAFETY: 4-byte validated pointer, exception-table path.
        let uncopied = unsafe {
            crate::arch::uaccess::copy_to_user(
                node_ptr as *mut u8,
                &node as *const u32 as *const u8,
                4,
            )
        };
        if uncopied > 0 {
            return -(errno::EFAULT as i64);
        }
    }
    0
}

/// sys_execveat - execute program relative to directory fd
///
/// # Arguments
/// - args[0]: dirfd - directory file descriptor
/// - args[1]: pathname - program path pointer
/// - args[2]: argv - argument vector pointer
/// - args[3]: envp - environment pointer
/// - args[4]: flags - AT_EMPTY_PATH, AT_SYMLINK_NOFOLLOW, etc.
pub fn sys_execveat(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let argv_ptr = args[2] as *const *const u8;
    let envp_ptr = args[3] as *const *const u8;
    let _flags = args[4] as i32;

    if pathname_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    let path = match crate::syscall::file::resolve_user_path(dirfd, pathname_ptr) {
        Ok(p) => p,
        Err(e) => return e as i64,
    };

    let argv = match copy_argv_from_user(argv_ptr) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let envp = match copy_envp_from_user(envp_ptr) {
        Ok(v) => v,
        Err(e) => return e,
    };

    do_execve(&path, &argv, &envp, 0) as i64
}

/// sys_setfsuid - Set filesystem user ID
///
/// # Arguments
/// - args[0]: fsuid - filesystem user ID
pub fn sys_setfsuid(args: SyscallArgs) -> i64 {
    let fsuid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred()/cred_mut()
        // return references to the task's credential structure.
        unsafe {
            let old_fsuid = (*task).cred().fsuid;
            // (uid_t)-1 leaves fsuid unchanged (verified on Linux 7.0:
            // setfsuid((uid_t)-1) twice as root returns 0 both times —
            // LTP setfsuid02; the old code let -1 become the new fsuid).
            let mut changed_to = None;
            if fsuid != u32::MAX {
                let cred = (*task).cred_mut();
                if crate::security::capable(crate::security::CAP_SETUID) {
                    cred.fsuid = fsuid;
                    changed_to = Some(fsuid);
                } else if fsuid == cred.uid || fsuid == cred.euid || fsuid == cred.suid {
                    cred.fsuid = fsuid;
                    changed_to = Some(fsuid);
                }
            }
            // Linux commit_creds CAP_FS_MASK rule: leaving fsuid 0 drops
            // the filesystem capabilities (CHOWN, DAC_OVERRIDE,
            // DAC_READ_SEARCH, FOWNER, FSETID — bits 0..4) from the
            // EFFECTIVE set; returning to fsuid 0 restores them from the
            // permitted set. Without this, setfsuid(nobody) still opened
            // root files O_RDWR through CAP_DAC_OVERRIDE (LTP setfsuid04).
            if let Some(new_fsuid) = changed_to {
                const CAP_FS_MASK: u64 = 0x1f;
                let cred = (*task).cred_mut();
                if old_fsuid == 0 && new_fsuid != 0 {
                    let eff = cred.cap_effective.bits() & !CAP_FS_MASK;
                    cred.cap_effective = crate::security::capability::Cap::new(eff);
                } else if old_fsuid != 0 && new_fsuid == 0 {
                    let eff = cred.cap_effective.bits()
                        | (cred.cap_permitted.bits() & CAP_FS_MASK);
                    cred.cap_effective = crate::security::capability::Cap::new(eff);
                }
            }
            old_fsuid as i64
        }
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_setfsgid - Set filesystem group ID
///
/// # Arguments
/// - args[0]: fsgid - filesystem group ID
pub fn sys_setfsgid(args: SyscallArgs) -> i64 {
    let fsgid = args[0] as u32;
    if let Some(task) = crate::sched::current() {
        // SAFETY: task pointer from sched::current() is valid; cred()/cred_mut()
        // return references to the task's credential structure.
        unsafe {
            let old_fsgid = (*task).cred().fsgid;
            // (gid_t)-1 leaves fsgid unchanged (LTP setfsgid01: the old
            // code stored 0xFFFFFFFF and the next call's return, read as
            // a signed syscall result, surfaced as -1).
            if fsgid != u32::MAX {
                let cred = (*task).cred_mut();
                if crate::security::capable(crate::security::CAP_SETGID) {
                    cred.fsgid = fsgid;
                } else if fsgid == cred.gid || fsgid == cred.egid || fsgid == cred.sgid {
                    cred.fsgid = fsgid;
                }
            }
            old_fsgid as i64
        }
    } else {
        -(errno::ESRCH as i64)
    }
}

/// sys_times - Get process times
///
/// # Arguments
/// - args[0]: buf - pointer to struct tms
///
/// # Returns
/// Clock ticks since system boot on success
pub fn sys_times(args: SyscallArgs) -> i64 {
    let buf_ptr = args[0] as *mut u64;
    if !buf_ptr.is_null() {
        if !crate::arch::uaccess::access_ok(buf_ptr as usize, 32) {
            return -(errno::EFAULT as i64);
        }
        // struct tms: tms_utime, tms_stime, tms_cutime, tms_cstime (clock_t = i64)
        // Self ticks from the timer-IRQ CPU accounting; child ticks are
        // folded into this task at each child's exit (see do_exit).
        let (utime, stime, cutime, cstime) = match crate::sched::current() {
            Some(t) => unsafe {
                use core::sync::atomic::Ordering::Relaxed;
                (
                    (*t).utime_ticks.load(Relaxed) as i64,
                    (*t).stime_ticks.load(Relaxed) as i64,
                    (*t).cutime_ticks.load(Relaxed) as i64,
                    (*t).cstime_ticks.load(Relaxed) as i64,
                )
            },
            None => (0, 0, 0, 0),
        };
        // SAFETY: buf_ptr validated with access_ok above; each field write
        // goes through put_user (exception-table path, SUM=0 safe).
        unsafe {
            let put = crate::arch::uaccess::put_user::<u64>;
            if !put(buf_ptr as *mut u64, utime as u64)
                || !put(buf_ptr.add(1) as *mut u64, stime as u64)
                || !put(buf_ptr.add(2) as *mut u64, cutime as u64)
                || !put(buf_ptr.add(3) as *mut u64, cstime as u64)
            {
                return -(errno::EFAULT as i64);
            }
        }
    }
    // Return clock ticks since boot (simplified: use jiffies)
    crate::drivers::timer::get_jiffies() as i64
}

/// sys_sysinfo - Get system information
///
/// # Arguments
/// - args[0]: info - pointer to struct sysinfo
pub fn sys_sysinfo(args: SyscallArgs) -> i64 {
    let info_ptr = args[0] as *mut u8;
    if info_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(info_ptr as usize, 112) {
        return -(errno::EFAULT as i64);
    }
    // struct sysinfo: 112 bytes, fill with available info
    // SAFETY: info_ptr validated with access_ok above; every field write goes
    // through put_user (exception-table copy path, SUM=0 safe).
    unsafe {
        let put_u64 = crate::arch::uaccess::put_user::<u64>;
        // uptime (seconds) - from jiffies
        let uptime = crate::drivers::timer::get_jiffies() as u64 / crate::drivers::timer::HZ as u64;
        let _ = put_u64(info_ptr as *mut u64, uptime);
        // loads[1],2,3] - zero
        let _ = put_u64(info_ptr.add(8) as *mut u64, 0);
        let _ = put_u64(info_ptr.add(16) as *mut u64, 0);
        let _ = put_u64(info_ptr.add(24) as *mut u64, 0);
        // totalram (bytes): the discovered memory map (runtime descriptor
        // count), not the Kernel.toml nominal size — sysinfo(2) callers
        // (installers, OOM-aware daemons) size their behavior off these
        // fields; freeram used to be a hard-coded "half of nominal".
        let mem = crate::mm::meminfo::get_basic_memory_info();
        let _ = put_u64(info_ptr.add(32) as *mut u64, mem.mem_total as u64);
        // freeram
        let _ = put_u64(info_ptr.add(40) as *mut u64, mem.mem_free as u64);
        // sharedram, bufferram
        let _ = put_u64(info_ptr.add(48) as *mut u64, 0);
        let _ = put_u64(info_ptr.add(56) as *mut u64, 0);
        // totalswap, freeswap
        let _ = put_u64(info_ptr.add(64) as *mut u64, 0);
        let _ = put_u64(info_ptr.add(72) as *mut u64, 0);
        // procs (current process count)
        use core::sync::atomic::{AtomicU16, Ordering};
        static PROC_COUNT: AtomicU16 = AtomicU16::new(0);
        PROC_COUNT.store(0, Ordering::Relaxed);
        crate::sched::for_each_task(|_| { PROC_COUNT.fetch_add(1, Ordering::Relaxed); });
        let _ = crate::arch::uaccess::put_user::<u16>(
            info_ptr.add(80) as *mut u16, PROC_COUNT.load(Ordering::Relaxed));
        // totalhigh, freehigh, mem_unit
        let _ = put_u64(info_ptr.add(88) as *mut u64, 0);
        let _ = put_u64(info_ptr.add(96) as *mut u64, 0);
        let _ = crate::arch::uaccess::put_user::<u32>(
            info_ptr.add(104) as *mut u32, 1); // mem_unit = 1 (bytes)
    }
    0
}

/// sys_membarrier - Issue memory barriers
///
/// # Arguments
/// - args[0]: cmd - MEMBARRIER_CMD_QUERY, MEMBARRIER_CMD_GLOBAL, etc.
/// - args[1]: flags - 0 or MEMBARRIER_FLAG_SYNC_CORE
///
/// MEMBARRIER_CMD_GLOBAL serializes caller vs every CPU running a Rux
/// task: each remote CPU is IPI'd (smp_call_function) into a full
/// read/write fence and the caller waits for completion — a local-only
/// fence would not order the caller's stores against a peer CPU's loads.
pub fn sys_membarrier(args: SyscallArgs) -> i64 {
    let cmd = args[0] as i32;
    let _flags = args[1] as u32;

    const MEMBARRIER_CMD_QUERY: i32 = 0;
    const MEMBARRIER_CMD_GLOBAL: i32 = (1 << 0);
    const MEMBARRIER_CMD_GLOBAL_EXPEDITED: i32 = (1 << 1);

    match cmd {
        MEMBARRIER_CMD_QUERY => {
            // Report which commands are supported
            MEMBARRIER_CMD_GLOBAL as i64 | MEMBARRIER_CMD_GLOBAL_EXPEDITED as i64
        }
        MEMBARRIER_CMD_GLOBAL | MEMBARRIER_CMD_GLOBAL_EXPEDITED => {
            // Remote fence on every other started CPU, then fence locally.
            // smp_call_function spins for completion, so the caller returns
            // only after all peers have fenced.
            fn remote_fence(_arg: *mut core::ffi::c_void) {
                // SAFETY: n/a — no unsafe. fence.i not needed (no
                // SYNC_CORE flag); the read/write fence orders this CPU's
                // memory operations.
                core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            }
            let online = crate::arch::smp::num_started_cpus().max(1);
            let me = crate::arch::cpu_id() as usize;
            for cpu in 0..online.min(crate::config::MAX_CPUS) {
                if cpu != me {
                    crate::arch::ipi::smp_call_function(cpu, remote_fence, core::ptr::null_mut());
                }
            }
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            0
        }
        _ => -(errno::EINVAL as i64),
    }
}

/// sys_userfaultfd - Create userfaultfd file descriptor
pub fn sys_userfaultfd(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_kcmp - Compare two processes
///
/// Compares the kernel-resource identity the two tasks hold for `type`:
/// 0 = "identical resource", 1/2 = "differs" (ordering by address, the
/// same information kernel kcmp exposes; callers must treat any nonzero
/// as "different"). Error contracts (man 2 kcmp, kernel/kcmp.c):
/// ESRCH unknown pid, EINVAL type out of [0, KCMP_TYPES), EBADF bad fd
/// for KCMP_FILE, EPERM caller may not ptrace either task.
///
/// Identity sources mirror clone's sharing decisions, so the answers
/// track what the task actually shares:
/// - KCMP_VM      address_space Arc (shared exactly on CLONE_VM)
/// - KCMP_FILES   fdtable Arc       (shared exactly on CLONE_FILES)
/// - KCMP_FS      fs Arc            (shared exactly on CLONE_FS)
/// - KCMP_SIGHAND signal Arc        (shared exactly on CLONE_SIGHAND)
/// - KCMP_FILE    the Arc<File> behind each fd (dup/fork keep the same
///                file object — Linux compares struct file pointers)
/// - KCMP_IO / KCMP_SYSVSEM: no task in Rux owns an io_context or a
///                private sem-undo list, so both sides hold the same
///                (absent) resource — Linux's NULL==NULL case, 0.
/// - KCMP_EPOLL:  not backed by a kernel-wide epoll registry — ENOSYS.
/// - KCMP_CAN:    no socketcan domain — ENOSYS.
pub fn sys_kcmp(args: SyscallArgs) -> i64 {
    use alloc::sync::Arc;

    // uapi/linux/kcmp.h enum kcmp_type — FILE is 0 (an LTP build with a
    // shifted enum made kcmp01's KCMP_FILE calls compare ADDRESS SPACES
    // instead: fork'd children differ there, so every cross-process
    // same-file case returned 1).
    const KCMP_FILE: i64 = 0;
    const KCMP_VM: i64 = 1;
    const KCMP_FILES: i64 = 2;
    const KCMP_FS: i64 = 3;
    const KCMP_SIGHAND: i64 = 4;
    const KCMP_IO: i64 = 5;
    const KCMP_SYSVSEM: i64 = 6;
    const KCMP_EPOLL: i64 = 7;
    const KCMP_TYPES: i64 = 8;

    let pid1 = args[0] as u32;
    let pid2 = args[1] as u32;
    // int in the ABI: the type is validated as SIGNED so -1/INT_MIN/INT_MAX
    // all land in the EINVAL arm (kcmp02).
    let ktype = args[2] as i64;
    let idx1 = args[3] as usize;
    let idx2 = args[4] as usize;

    // Pid lookup first (kernel kcmp: ESRCH precedes type validation).
    let (t1, t2) = match (
        crate::process::find_task_by_pid(pid1),
        crate::process::find_task_by_pid(pid2),
    ) {
        (Some(a), Some(b)) => (
            a as *const crate::process::task::Task,
            b as *const crate::process::task::Task,
        ),
        _ => return -(errno::ESRCH as i64),
    };
    let (t1, t2) = unsafe { (&*t1, &*t2) };

    if ktype < 0 || ktype >= KCMP_TYPES {
        return -(errno::EINVAL as i64);
    }

    // PTRACE_MODE_ATTACH_REALCREDS parity: the caller must be able to
    // ptrace BOTH tasks. Rux has no capability sets per se; the root uid
    // is the capability stand-in (every LTP kcmp case runs as root).
    if let Some(cur) = crate::sched::current() {
        let cur_euid = unsafe { (*cur).cred().euid };
        if cur_euid != 0 && (cur_euid != t1.cred().euid || cur_euid != t2.cred().euid) {
            return -(errno::EPERM as i64);
        }
    }

    // Pointer ordering — kernel kcmp's (a<b)?1:(a>b)?2:0 over the two
    // resource addresses.
    fn cmp_usize(a: usize, b: usize) -> i64 {
        if a == b {
            0
        } else if a < b {
            1
        } else {
            2
        }
    }

    match ktype {
        KCMP_FILE => {
            let f1 = t1.fdtable_arc().and_then(|ft| ft.get_file(idx1));
            let f2 = t2.fdtable_arc().and_then(|ft| ft.get_file(idx2));
            match (f1, f2) {
                (Some(a), Some(b)) => {
                    let (pa, pb) = (Arc::as_ptr(&a) as usize, Arc::as_ptr(&b) as usize);
                    drop(a);
                    drop(b);
                    cmp_usize(pa, pb)
                }
                _ => -(errno::EBADF as i64),
            }
        }
        KCMP_VM => {
            let (a, b) = (t1.address_space_arc(), t2.address_space_arc());
            match (a, b) {
                (Some(x), Some(y)) => cmp_usize(Arc::as_ptr(&x) as usize, Arc::as_ptr(&y) as usize),
                _ => 0, // both absent (kernel thread): Linux NULL==NULL
            }
        }
        KCMP_FILES => {
            let (a, b) = (t1.fdtable_arc(), t2.fdtable_arc());
            match (a, b) {
                (Some(x), Some(y)) => cmp_usize(Arc::as_ptr(&x) as usize, Arc::as_ptr(&y) as usize),
                _ => 0,
            }
        }
        KCMP_FS => {
            let (a, b) = (t1.fs_arc(), t2.fs_arc());
            match (a, b) {
                (Some(x), Some(y)) => cmp_usize(Arc::as_ptr(&x) as usize, Arc::as_ptr(&y) as usize),
                _ => 0,
            }
        }
        KCMP_SIGHAND => {
            let (a, b) = (t1.signal_arc(), t2.signal_arc());
            match (a, b) {
                (Some(x), Some(y)) => cmp_usize(Arc::as_ptr(&x) as usize, Arc::as_ptr(&y) as usize),
                _ => 0,
            }
        }
        // No io_context / sem-undo ownership exists in Rux; both sides
        // hold the same absent resource (kernel NULL==NULL → 0).
        KCMP_IO | KCMP_SYSVSEM => 0,
        // Would need the epoll interest list keyed by (epfd, file); no
        // such registry exists yet. Untested by the LTP list in use.
        KCMP_EPOLL => -(errno::ENOSYS as i64),
        _ => -(errno::EINVAL as i64),
    }
}

/// sys_finit_module - Load kernel module from file descriptor
///
/// U2 kmod support: reads the blob from the fd and registers it in the
/// module table (no relocation/execution — see kernel/src/module/mod.rs).
/// This is the syscall kmod's modprobe actually issues at udev coldboot.
pub fn sys_finit_module(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let flags = args[2] as u64;
    crate::module::sys_finit_module_impl(fd, flags)
}

/// sys_init_module - Load kernel module
///
/// U2 kmod support: blob passed inline from user memory.
pub fn sys_init_module(args: SyscallArgs) -> i64 {
    let umod = args[0] as usize;
    let len = args[1] as usize;
    crate::module::sys_init_module_impl(umod, len)
}

/// sys_delete_module - Unload kernel module
///
/// U2 kmod support: removes the registration (refcount-gated).
pub fn sys_delete_module(args: SyscallArgs) -> i64 {
    let name_user = args[0] as usize;
    let flags = args[1] as u64;
    crate::module::sys_delete_module_impl(name_user, flags)
}

/// sys_kexec_load - Load new kernel for reboot
pub fn sys_kexec_load(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// iovec as passed by process_vm_readv/writev (repr(C), same layout as
/// readv/writev iovecs).
#[repr(C)]
#[derive(Clone, Copy)]
struct PvmIovec {
    iov_base: *mut u8,
    iov_len: usize,
}

/// sys_process_vm_readv - Read from another process memory
///
/// process_vm_readv(2): `lvec` (caller memory) receives the bytes at
/// `rvec` (same struct layout, but its addresses live in the TARGET's
/// address space). Returns the byte count moved; a short count is legal
/// when a later segment faults; -errno when nothing moved.
pub fn sys_process_vm_readv(args: SyscallArgs) -> i64 {
    process_vm_rw(args, false)
}

/// sys_process_vm_writev - Write to another process memory
///
/// process_vm_writev(2): the mirror of readv; see process_vm_rw.
pub fn sys_process_vm_writev(args: SyscallArgs) -> i64 {
    process_vm_rw(args, true)
}

/// Shared process_vm_readv/writev body (`to_remote` selects direction).
///
/// Contract order (LTP process_vm01):
///   flags != 0                     -> EINVAL (no flags are defined yet)
///   liovcnt/riovcnt > IOV_MAX      -> EINVAL
///   iov_len sums overflowing       -> EINVAL
///   bad lvec/rvec ARRAY pointer    -> EFAULT
///   no such pid                    -> ESRCH
///   credentials do not grant       -> EPERM (ptrace_may_access rule:
///                                     same uid, or CAP_SYS_PTRACE)
///   bad local/remote iov_base      -> EFAULT (short count if partial)
fn process_vm_rw(args: SyscallArgs, to_remote: bool) -> i64 {
    use crate::arch::uaccess::{copy_from_user, copy_to_user};

    const IOV_MAX: usize = 1024;
    // Matches readv/writev's MAX_RW_COUNT clamp source.
    const MAX_RW_COUNT: usize = i32::MAX as usize & !0xFFF;

    let pid = args[0] as u32;
    let lvec = args[1] as *const PvmIovec;
    let liovcnt = args[2] as usize;
    let rvec = args[3] as *const PvmIovec;
    let riovcnt = args[4] as usize;
    let flags = args[5] as u32;

    if flags != 0 {
        return -(errno::EINVAL as i64);
    }
    if liovcnt == 0 || riovcnt == 0 {
        return 0;
    }
    if liovcnt > IOV_MAX || riovcnt > IOV_MAX {
        return -(errno::EINVAL as i64);
    }

    // Copy both iovec arrays in from the caller (bad array pointer is
    // EFAULT before any pid/permission work, matching the documented
    // argument-validation order).
    // SAFETY: lvec/rvec point at caller-supplied arrays of liovcnt /
    // riovcnt PvmIovecs; copy_from_user faults safely on bad pointers.
    let mut liovs: alloc::vec::Vec<PvmIovec> =
        alloc::vec![PvmIovec { iov_base: core::ptr::null_mut(), iov_len: 0 }; liovcnt];
    let mut riovs: alloc::vec::Vec<PvmIovec> =
        alloc::vec![PvmIovec { iov_base: core::ptr::null_mut(), iov_len: 0 }; riovcnt];
    unsafe {
        if copy_from_user(
            liovs.as_mut_ptr() as *mut u8,
            lvec as *const u8,
            liovcnt * core::mem::size_of::<PvmIovec>(),
        ) != 0
        {
            return -(errno::EFAULT as i64);
        }
        if copy_from_user(
            riovs.as_mut_ptr() as *mut u8,
            rvec as *const u8,
            riovcnt * core::mem::size_of::<PvmIovec>(),
        ) != 0
        {
            return -(errno::EFAULT as i64);
        }
    }
    // iov_len sanity: each length and both totals must stay bounded
    // (LTP test_iov_len_overflow passes iov_len = -1 and expects EINVAL).
    let mut ltotal = 0usize;
    for iov in &liovs {
        if iov.iov_len > MAX_RW_COUNT {
            return -(errno::EINVAL as i64);
        }
        ltotal = match ltotal.checked_add(iov.iov_len) {
            Some(t) if t <= MAX_RW_COUNT => t,
            _ => return -(errno::EINVAL as i64),
        };
    }
    let mut rtotal = 0usize;
    for iov in &riovs {
        if iov.iov_len > MAX_RW_COUNT {
            return -(errno::EINVAL as i64);
        }
        rtotal = match rtotal.checked_add(iov.iov_len) {
            Some(t) if t <= MAX_RW_COUNT => t,
            _ => return -(errno::EINVAL as i64),
        };
    }

    // Target lookup. pid 0 / unused pid -> ESRCH.
    // SAFETY: the pin keeps the Task alive until task_put below.
    let target = unsafe { crate::process::pid_hash::pid_hash_lookup_pinned(pid) };
    if target.is_null() {
        return -(errno::ESRCH as i64);
    }
    let ret = process_vm_rw_locked(target, &liovs, &riovs, to_remote);
    // SAFETY: release the pin taken above.
    unsafe { crate::process::task::Task::task_put(target) };
    ret
}

/// Body with the target pinned: permission gate, then the segment walk.
fn process_vm_rw_locked(
    target: *mut crate::process::task::Task,
    liovs: &[PvmIovec],
    riovs: &[PvmIovec],
    to_remote: bool,
) -> i64 {
    use crate::arch::uaccess::{copy_from_user, copy_to_user};

    // SAFETY: current() returns the running task or None; target is
    // pinned by the caller.
    let current = match crate::sched::current() {
        Some(c) => c as *mut crate::process::task::Task,
        None => return -(errno::ESRCH as i64),
    };
    if target != current {
        // SAFETY: cred() is immutable after task creation.
        let target_cred = unsafe { (*target).cred() };
        if !crate::process::ptrace::ptrace_may_access(target_cred) {
            return -(errno::EPERM as i64);
        }
    }

    // Walk the local/remote iovec lists together, transferring
    // min(local-left, remote-left) per step through a kernel staging
    // buffer (the local side is the CALLER's user memory — uaccess only).
    let mut total = 0usize;
    let (mut li, mut ri) = (0usize, 0usize);
    let (mut loff, mut roff) = (0usize, 0usize);
    while li < liovs.len() && ri < riovs.len() {
        let ln = liovs[li].iov_len.saturating_sub(loff);
        let rn = riovs[ri].iov_len.saturating_sub(roff);
        if ln == 0 {
            li += 1;
            loff = 0;
            continue;
        }
        if rn == 0 {
            ri += 1;
            roff = 0;
            continue;
        }
        let n = core::cmp::min(core::cmp::min(ln, rn), 256 * 1024usize);
        let lptr = unsafe { liovs[li].iov_base.add(loff) };
        let raddr = riovs[ri].iov_base as u64 + roff as u64;

        let mut stage: alloc::vec::Vec<u8> = alloc::vec![0u8; n];
        if to_remote {
            // SAFETY: lptr is a caller user pointer; copy_from_user
            // returns the uncopied tail.
            let bad = unsafe {
                copy_from_user(stage.as_mut_ptr(), lptr, n)
            };
            if bad == n {
                return if total == 0 { -(errno::EFAULT as i64) } else { total as i64 };
            }
            let got = n - bad;
            match crate::process::ptrace::process_vm_copy(
                target, raddr, stage.as_mut_ptr(), got, true,
            ) {
                Ok(moved) => {
                    total += moved;
                    if moved < got {
                        return total as i64;
                    }
                }
                Err(_) => {
                    return if total == 0 { -(errno::EFAULT as i64) } else { total as i64 };
                }
            }
        } else {
            match crate::process::ptrace::process_vm_copy(
                target, raddr, stage.as_mut_ptr(), n, false,
            ) {
                Ok(moved) => {
                    if moved > 0 {
                        // SAFETY: lptr is a caller user pointer.
                        let bad = unsafe {
                            copy_to_user(lptr, stage.as_ptr(), moved)
                        };
                        total += moved - bad;
                        if bad > 0 || moved < n {
                            return if total == 0 { -(errno::EFAULT as i64) } else { total as i64 };
                        }
                    } else {
                        return if total == 0 { -(errno::EFAULT as i64) } else { total as i64 };
                    }
                }
                Err(_) => {
                    return if total == 0 { -(errno::EFAULT as i64) } else { total as i64 };
                }
            }
        }
        loff += n;
        roff += n;
        if loff >= liovs[li].iov_len {
            li += 1;
            loff = 0;
        }
        if roff >= riovs[ri].iov_len {
            ri += 1;
            roff = 0;
        }
    }
    total as i64
}

/// sys_perf_event_open - Open performance event
///
/// PERF_TYPE_SOFTWARE cpu-clock/task-clock sampling (sample_period +
/// PERF_SAMPLE_IP records via read(2)). The validation, event table
/// and record layout live in `kernel/src/perf/mod.rs`.
pub fn sys_perf_event_open(args: SyscallArgs) -> i64 {
    crate::perf::sys_perf_event_open(args)
}

// ============================================================================
// seccomp (U1c)
// ============================================================================

/// Seccomp modes (uapi/linux/seccomp.h).
pub const SECCOMP_MODE_DISABLED: u32 = 0;
pub const SECCOMP_MODE_STRICT: u32 = 1;
pub const SECCOMP_MODE_FILTER: u32 = 2;

/// seccomp(2) operations.
const SECCOMP_SET_MODE_STRICT: u32 = 0;
const SECCOMP_SET_MODE_FILTER: u32 = 1;
const SECCOMP_GET_ACTION_AVAIL: u32 = 2;
const SECCOMP_GET_NOTIF_SIZES: u32 = 3;

/// SECCOMP_FILTER_FLAG_* (accepted and ignored except TSYNC semantics).
#[allow(dead_code)]
const SECCOMP_FILTER_FLAG_TSYNC: u32 = 1;
#[allow(dead_code)]
const SECCOMP_FILTER_FLAG_LOG: u32 = 2;
#[allow(dead_code)]
const SECCOMP_FILTER_FLAG_SPEC_ALLOW: u32 = 4;
#[allow(dead_code)]
const SECCOMP_FILTER_FLAG_NEW_LISTENER: u32 = 8;
#[allow(dead_code)]
const SECCOMP_FILTER_FLAG_TSYNC_ESRCH: u32 = 16;

/// SECCOMP_RET_* action values.
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
#[allow(dead_code)]
const SECCOMP_RET_KILL_THREAD: u32 = 0x0000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
#[allow(dead_code)]
const SECCOMP_RET_TRACE: u32 = 0x7ff0_0000;
#[allow(dead_code)]
const SECCOMP_RET_LOG: u32 = 0x7ffc_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;

/// AUDIT_ARCH: __AUDIT_ARCH_64BIT | __AUDIT_ARCH_LE | EM_<arch>.
/// The filter's seccomp_data.arch token must name the arch whose NATIVE
/// syscall numbers the filter sees (the gate runs on the raw trap number:
/// riscv64 native = the generic table, x86_64 native = the x86_64 table —
/// the gate runs BEFORE x86_compat translation, which matches the
/// reference kernel where filters always see native numbers).
#[cfg(feature = "riscv64")]
const AUDIT_ARCH: u32 = 0x8000_0000 | 0x4000_0000 | 243; // AUDIT_ARCH_RISCV64
#[cfg(feature = "x86_64")]
const AUDIT_ARCH: u32 = 0x8000_0000 | 0x4000_0000 | 62; // AUDIT_ARCH_X86_64
#[cfg(feature = "aarch64")]
const AUDIT_ARCH: u32 = 0x8000_0000 | 0x4000_0000 | 183; // AUDIT_ARCH_AARCH64

/// Classic-BPF instruction classes/ops (uapi/linux/filter.h subset).
const BPF_LD: u16 = 0x00;
const BPF_W: u16 = 0x00;
#[allow(dead_code)]
const BPF_H: u16 = 0x08;
#[allow(dead_code)]
const BPF_B: u16 = 0x10;
const BPF_IMM: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JMP: u16 = 0x05;
const BPF_JEQ: u16 = 0x10;
const BPF_JGT: u16 = 0x20;
const BPF_JGE: u16 = 0x30;
const BPF_JSET: u16 = 0x40;
const BPF_RET: u16 = 0x06;
const BPF_K: u16 = 0x00;

/// BPF_MAXINSNS.
const BPF_MAXINSNS: usize = 4096;

/// seccomp_data size: nr + arch + ip + 6 args = 64 bytes.
const SECCOMP_DATA_SIZE: u32 = 4 + 4 + 8 + 6 * 8;

/// User sock_fprog layout (riscv64): { u16 len; pad; sock_filter *filter; }.
#[repr(C)]
struct SockFprogUser {
    len: u16,
    _pad: u16,
    _pad2: u32,
    filter: *const crate::process::task::SockFilter,
}

/// sys_seccomp - Operate on seccomp state (NR 277, U1c)
///
/// seccomp(operation, flags, args):
/// - SECCOMP_SET_MODE_STRICT(0): switch to STRICT mode (read/write/exit/
///   exit_group/rt_sigreturn only; anything else kills the task).
/// - SECCOMP_SET_MODE_FILTER(1): load the classic-BPF program pointed to by
///   args (struct sock_fprog) and switch to FILTER mode.
/// - SECCOMP_GET_ACTION_AVAIL(2) / GET_NOTIF_SIZES(3): accepted minimal.
pub fn sys_seccomp(args: SyscallArgs) -> i64 {
    use crate::arch::uaccess::copy_from_user;

    let operation = args[0] as u32;
    let flags = args[1] as u32;
    let uargs = args[2] as usize;

    match operation {
        SECCOMP_SET_MODE_STRICT => {
            if flags != 0 || uargs != 0 {
                return -(errno::EINVAL as i64);
            }
            match seccomp_set_strict() {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        SECCOMP_SET_MODE_FILTER => {
            // Only the benign flag bits are accepted (TSYNC has no thread
            // scope here; NEW_LISTENER has no notification support).
            if flags & !(SECCOMP_FILTER_FLAG_TSYNC
                | SECCOMP_FILTER_FLAG_LOG
                | SECCOMP_FILTER_FLAG_SPEC_ALLOW
                | SECCOMP_FILTER_FLAG_TSYNC_ESRCH)
                != 0
            {
                return -(errno::EINVAL as i64);
            }
            if uargs == 0 {
                return -(errno::EFAULT as i64);
            }

            // Copy the sock_fprog header.
            let mut fprog = [0u8; 16];
            // SAFETY: uargs validated non-zero; copy_from_user uses the
            // exception-table path and fails cleanly on bad pointers.
            unsafe {
                if copy_from_user(fprog.as_mut_ptr(), uargs as *const u8, 16) != 0 {
                    return -(errno::EFAULT as i64);
                }
            }
            let prog: SockFprogUser = unsafe { core::ptr::read_unaligned(fprog.as_ptr() as *const SockFprogUser) };
            if prog.len == 0 || prog.len as usize > BPF_MAXINSNS {
                return -(errno::EINVAL as i64);
            }
            if prog.filter.is_null() {
                return -(errno::EFAULT as i64);
            }

            // Copy the filter program.
            let count = prog.len as usize;
            let mut filter =
                alloc::vec::Vec::<crate::process::task::SockFilter>::with_capacity(count);
            let mut raw = alloc::vec![0u8; count * 8];
            // SAFETY: prog.filter is a user pointer validated non-null and
            // the length is bounded by BPF_MAXINSNS; copy_from_user uses
            // the exception-table path and fails cleanly on bad pointers.
            unsafe {
                if copy_from_user(raw.as_mut_ptr(), prog.filter as *const u8, count * 8) != 0 {
                    return -(errno::EFAULT as i64);
                }
            }
            for chunk in raw.chunks_exact(8) {
                filter.push(crate::process::task::SockFilter {
                    code: u16::from_ne_bytes([chunk[0], chunk[1]]),
                    jt: chunk[2],
                    jf: chunk[3],
                    k: u32::from_ne_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
                });
            }

            // Privilege gate at attach time (Linux order: program
            // copy/validation above reports EFAULT/EINVAL first; the
            // capability check happens when the filter attaches).
            // seccomp(2)/prctl(2): installing a filter requires the
            // no_new_privs attribute OR CAP_SYS_ADMIN → EACCES otherwise.
            // (LTP prctl02 case #5 drops CAP_SYS_ADMIN and requires
            // EACCES; prctl03-style NNP users attach unprivileged.)
            if !seccomp_attach_allowed() {
                return -(errno::EACCES as i64);
            }

            match seccomp_set_filter(filter) {
                Ok(()) => 0,
                Err(e) => e as i64,
            }
        }
        SECCOMP_GET_ACTION_AVAIL => {
            // args[0] holds the action pointer in Linux; we accept the
            // known actions.
            let mut val = [0u8; 4];
            if uargs == 0 {
                return -(errno::EFAULT as i64);
            }
            // SAFETY: validated non-null; exception-table copy.
            unsafe {
                if copy_from_user(val.as_mut_ptr(), uargs as *const u8, 4) != 0 {
                    return -(errno::EFAULT as i64);
                }
            }
            let action = u32::from_ne_bytes(val);
            match action {
                SECCOMP_RET_KILL_PROCESS
                | SECCOMP_RET_KILL_THREAD
                | SECCOMP_RET_ERRNO
                | SECCOMP_RET_LOG
                | SECCOMP_RET_ALLOW => 0,
                _ => -(errno::EOPNOTSUPP as i64),
            }
        }
        SECCOMP_GET_NOTIF_SIZES => {
            // struct seccomp_notif_sizes { u16 notif, id, pidfd; } = 0,0,0.
            if uargs == 0 {
                return -(errno::EFAULT as i64);
            }
            // SAFETY: validated non-null; exception-table zero-write.
            unsafe {
                let zeros = [0u8; 6];
                if crate::arch::uaccess::copy_to_user(uargs as *mut u8, zeros.as_ptr(), 6) != 0 {
                    return -(errno::EFAULT as i64);
                }
            }
            0
        }
        _ => -(errno::EINVAL as i64),
    }
}

/// Filter-attach privilege decision (seccomp(2)): the no_new_privs
/// attribute OR CAP_SYS_ADMIN. Exposed pub(crate) for the unit rows.
pub(crate) fn seccomp_attach_allowed() -> bool {
    crate::sched::current()
        .map(|t| t.no_new_privs())
        .unwrap_or(false)
        || crate::security::capable(crate::security::CAP_SYS_ADMIN)
}

/// STRICT-mode allowed set, for the unit rows (per-arch native numbers).
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
pub(crate) fn strict_allowed_for_test() -> &'static [u64; 5] {
    &STRICT_ALLOWED
}

/// Enter STRICT mode. Cannot be entered once FILTER mode is active
/// (Linux: EPERM — filters cannot be detached).
fn seccomp_set_strict() -> Result<(), i32> {    let Some(task) = crate::sched::current() else {
        return Err(-(errno::ESRCH as i32));
    };
    if task.seccomp_mode() == SECCOMP_MODE_FILTER {
        return Err(-(errno::EPERM as i32));
    }
    task.set_seccomp_mode(SECCOMP_MODE_STRICT);
    Ok(())
}

/// Validate and install a BPF filter program, entering FILTER mode.
/// Supported instruction subset: BPF_LD|BPF_W|BPF_ABS (load seccomp_data
/// word), BPF_LD|BPF_W|BPF_IMM (A = k), BPF_JMP|BPF_{JEQ,JGT,JGE,JSET}|BPF_K
/// (compare), BPF_RET|BPF_K (return action). Anything else → EINVAL.
fn seccomp_set_filter(filter: alloc::vec::Vec<crate::process::task::SockFilter>) -> Result<(), i32> {
    let Some(task) = crate::sched::current() else {
        return Err(-(errno::ESRCH as i32));
    };

    // Walk once for validation: opcodes supported, jump targets in range.
    for (pc, ins) in filter.iter().enumerate() {
        let class = ins.code & 0x07;
        match class {
            x if x == BPF_LD => {
                let size = ins.code & 0x18;
                let mode = ins.code & 0xe0;
                if size != BPF_W || (mode != BPF_ABS && mode != BPF_IMM) {
                    return Err(-(errno::EINVAL as i32));
                }
            }
            x if x == BPF_JMP => {
                let op = ins.code & 0xf0;
                let src = ins.code & 0x08;
                if src != BPF_K
                    || (op != BPF_JEQ && op != BPF_JGT && op != BPF_JGE && op != BPF_JSET)
                {
                    return Err(-(errno::EINVAL as i32));
                }
                // A nonzero offset must land strictly inside the program.
                let end = filter.len() as u32;
                let base = pc as u32;
                if ins.jt != 0 && base + ins.jt as u32 + 1 >= end {
                    return Err(-(errno::EINVAL as i32));
                }
                if ins.jf != 0 && base + ins.jf as u32 + 1 >= end {
                    return Err(-(errno::EINVAL as i32));
                }
            }
            x if x == BPF_RET => {
                if ins.code & 0x18 != BPF_K {
                    return Err(-(errno::EINVAL as i32));
                }
            }
            _ => return Err(-(errno::EINVAL as i32)),
        }
    }
    // A program must return eventually — require at least one RET.
    if !filter.iter().any(|i| i.code & 0x07 == BPF_RET) {
        return Err(-(errno::EINVAL as i32));
    }

    // Single-filter model: the new program replaces the old one (Linux
    // stacks multiple programs — recorded divergence).
    *task.seccomp_filter.lock() = Some(filter);

    task.set_seccomp_mode(SECCOMP_MODE_FILTER);
    Ok(())
}

/// Syscalls allowed in STRICT mode — the RUNNING ARCH'S NATIVE numbers,
/// because the trap gate hands the raw trap number to seccomp_syscall_gate
/// (riscv64: the generic table; x86_64: regs.rax BEFORE x86_compat
/// translation). The single riscv64-only list was an ABI bug: on x86_64
/// a STRICT task was SIGKILLed on its first native read(0)/write(1) and
/// uname(63) slipped through as if it were read.
#[cfg(feature = "riscv64")]
const STRICT_ALLOWED: [u64; 5] = [
    63,  // read
    64,  // write
    93,  // exit
    94,  // exit_group
    139, // rt_sigreturn
];
#[cfg(feature = "x86_64")]
const STRICT_ALLOWED: [u64; 5] = [
    0,   // read
    1,   // write
    60,  // exit
    231, // exit_group
    15,  // rt_sigreturn
];
/// aarch64: asm-generic syscall numbers — identical to riscv64's set
#[cfg(feature = "aarch64")]
const STRICT_ALLOWED: [u64; 5] = [
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
                Some(-(errno::ENOSYS as i64))
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
                    Some(-(errno::ENOSYS as i64))
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

/// prctl PR_SET_SECCOMP(22) / PR_GET_SECCOMP(21) helpers (U1c).
fn prctl_set_seccomp(mode: u64, prog_ptr: u64) -> i64 {
    match mode as u32 {
        SECCOMP_MODE_STRICT => match seccomp_set_strict() {
            Ok(()) => 0,
            Err(e) => e as i64,
        },
        SECCOMP_MODE_FILTER => {
            // PR_SET_SECCOMP(2, prog): prog points at sock_fprog.
            let args: SyscallArgs = [
                SECCOMP_SET_MODE_FILTER as u64,
                0,
                prog_ptr,
                0,
                0,
                0,
            ];
            sys_seccomp(args)
        }
        _ => -(errno::EINVAL as i64),
    }
}

/// sys_bpf - BPF system call
pub fn sys_bpf(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_capget - Get capabilities for a process
///
/// # Arguments
/// - args[0]: hdr_ptr - pointer to __user_cap_header_struct { version: u32, pid: i32 }
/// - args[1]: data_ptr - pointer to __user_cap_data_struct array(s)
pub fn sys_capget(args: SyscallArgs) -> i64 {
    use crate::arch::uaccess::{copy_from_user, copy_to_user};

    const _LINUX_CAPABILITY_VERSION_1: u32 = 0x1998_0330;
    const _LINUX_CAPABILITY_VERSION_2: u32 = 0x2007_1026;
    const _LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

    let hdr_ptr = args[0] as usize;
    let data_ptr = args[1] as usize;

    // Header is 8 bytes: version (u32) + pid (i32)
    if !crate::arch::uaccess::access_ok(hdr_ptr, 8) {
        return -(errno::EFAULT as i64);
    }

    // Read header
    let mut hdr = [0u32; 2];
    // SAFETY: hdr_ptr validated with access_ok; copy_from_user safely copies from user space.
    unsafe {
        if copy_from_user(hdr.as_mut_ptr() as *mut u8, hdr_ptr as *const u8, 8) != 0 {
            return -(errno::EFAULT as i64);
        }
    }
    let version = hdr[0];
    let pid = hdr[1] as i32;

    // Version probe (Linux capget semantics): version == 0 asks the
    // kernel to report the highest supported capability ABI version —
    // written back into the header, returning EINVAL. This runs BEFORE
    // the data-pointer check: libcap-ng's init() calls capget(&hdr, NULL)
    // and classifies by the version written back (an EFAULT there broke
    // capng_change_id and killed dbus-daemon startup).
    if version == 0 {
        let supported = _LINUX_CAPABILITY_VERSION_3;
        // SAFETY: hdr_ptr validated with access_ok; copy_to_user handles user writes.
        unsafe {
            copy_to_user(hdr_ptr as *mut u8, &supported as *const u32 as *const u8, 4);
        }
        return -(errno::EINVAL as i64);
    }

    // A real query needs the data array
    if data_ptr == 0 {
        return -(errno::EFAULT as i64);
    }

    // Determine the target task
    let target = if pid == 0 {
        match crate::sched::current() {
            Some(t) => t,
            None => return -(errno::ESRCH as i64),
        }
    } else {
        // Linux capget: a negative pid is EINVAL, not ESRCH (LTP capget02
        // "bad pid" passes -1 and expects EINVAL).
        if pid < 0 {
            return -(errno::EINVAL as i64);
        }
        // SAFETY: find_task_by_pid returns valid pointer when non-null.
        let ptr = unsafe { crate::sched::find_task_by_pid(pid as u32) };
        if ptr.is_null() {
            return -(errno::ESRCH as i64);
        }
        // SAFETY: ptr validated non-null above.
        unsafe { &*ptr }
    };

    let cred = target.cred();

    // Validate version and determine data size
    let data_count: usize;
    match version {
        _LINUX_CAPABILITY_VERSION_1 => data_count = 1,  // 1 x 3 u32s = 12 bytes
        _LINUX_CAPABILITY_VERSION_2 => data_count = 2,  // 2 x 3 u32s = 24 bytes
        _LINUX_CAPABILITY_VERSION_3 => data_count = 2,  // 2 x 3 u32s = 24 bytes
        _ => {
            // Unknown version: write back the highest supported version and return -EINVAL
            let supported = _LINUX_CAPABILITY_VERSION_3;
            // SAFETY: hdr_ptr validated with access_ok; copy_to_user handles user writes.
            unsafe {
                copy_to_user(hdr_ptr as *mut u8, &supported as *const u32 as *const u8, 4);
            }
            return -(errno::EINVAL as i64);
        }
    }

    let data_size = data_count * 3 * 4; // each entry is 3 u32s
    if !crate::arch::uaccess::access_ok(data_ptr, data_size) {
        return -(errno::EFAULT as i64);
    }

    // Build the data array: [effective_lo, permitted_lo, inheritable_lo, effective_hi, permitted_hi, inheritable_hi]
    let mut data: [u32; 6] = [
        cred.cap_effective.lo(),
        cred.cap_permitted.lo(),
        cred.cap_inheritable.lo(),
        cred.cap_effective.hi(),
        cred.cap_permitted.hi(),
        cred.cap_inheritable.hi(),
    ];

    // SAFETY: data_ptr validated with access_ok; copy_to_user handles user writes.
    unsafe {
        if copy_to_user(data_ptr as *mut u8, data.as_mut_ptr() as *const u8, data_size) != 0 {
            return -(errno::EFAULT as i64);
        }
    }

    0
}

/// sys_capset - Set capabilities for the calling process
///
/// # Arguments
/// - args[0]: hdr_ptr - pointer to __user_cap_header_struct { version: u32, pid: i32 }
/// - args[1]: data_ptr - pointer to __user_cap_data_struct array(s)
pub fn sys_capset(args: SyscallArgs) -> i64 {
    use crate::arch::uaccess::{copy_from_user, copy_to_user};
    use crate::security::capability::Cap;

    const _LINUX_CAPABILITY_VERSION_1: u32 = 0x1998_0330;
    const _LINUX_CAPABILITY_VERSION_2: u32 = 0x2007_1026;
    const _LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

    let hdr_ptr = args[0] as usize;
    let data_ptr = args[1] as usize;

    if hdr_ptr == 0 || data_ptr == 0 {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(hdr_ptr, 8) {
        return -(errno::EFAULT as i64);
    }

    // Read header
    let mut hdr = [0u32; 2];
    // SAFETY: hdr_ptr validated with access_ok; copy_from_user safely copies from user space.
    unsafe {
        if copy_from_user(hdr.as_mut_ptr() as *mut u8, hdr_ptr as *const u8, 8) != 0 {
            return -(errno::EFAULT as i64);
        }
    }
    let version = hdr[0];
    let pid = hdr[1] as i32;

    // Determine data size from version
    let data_count: usize;
    match version {
        _LINUX_CAPABILITY_VERSION_1 => data_count = 1,
        _LINUX_CAPABILITY_VERSION_2 => data_count = 2,
        _LINUX_CAPABILITY_VERSION_3 => data_count = 2,
        _ => {
            // Linux: an unsupported version fails EINVAL but the kernel's
            // preferred version is written back into the header first
            // (libcap and LTP capset02 probe this way).
            let supported = _LINUX_CAPABILITY_VERSION_3;
            // SAFETY: hdr_ptr validated with access_ok above.
            unsafe {
                copy_to_user(hdr_ptr as *mut u8, &supported as *const u32 as *const u8, 4);
            }
            return -(errno::EINVAL as i64);
        }
    }
    let data_size = data_count * 3 * 4;
    if !crate::arch::uaccess::access_ok(data_ptr, data_size) {
        return -(errno::EFAULT as i64);
    }

    // Read data from userspace
    let mut data = [0u32; 6];
    // SAFETY: data_ptr validated with access_ok; copy_from_user safely copies from user space.
    unsafe {
        if copy_from_user(data.as_mut_ptr() as *mut u8, data_ptr as *const u8, data_size) != 0 {
            return -(errno::EFAULT as i64);
        }
    }

    // Reconstruct capabilities from the data array
    let new_effective = Cap::from_halves(data[0], data[3]);
    let new_permitted = Cap::from_halves(data[1], data[4]);
    let new_inheritable = Cap::from_halves(data[2], data[5]);

    // capset can only operate on the current process. Linux returns EPERM
    // when the header pid names any other task (kernel/capability.c:
    // "if (pid && pid != task_pid_vnr(current)) return -EPERM"). LTP
    // capset04 forks a child and expects capset(child_pid, ...) = EPERM.
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };
    if pid != 0 && pid != unsafe { (*current).pid() } as i32 && pid != unsafe { (*current).tgid() } as i32
    {
        return -(errno::EPERM as i64);
    }

    // Permission checks (Linux capset semantics):
    // 1. new permitted must be a subset of old permitted
    // 2. new inheritable must be a subset of old inheritable, unless
    //    CAP_SETPCAP is held (then it may expand up to the bounding set)
    //    (F20: the old check was ⊆ old PERMITTED, letting any process grow
    //    its inheritable set to its full permitted set — those bits then
    //    survive exec through the ambient synthesis in execve)
    // 3. new effective must be a subset of new permitted
    let cred = current.cred();
    if !new_permitted.is_subset_of(cred.cap_permitted) {
        crate::pr_info!(
            "capset: pid {} uid {} EPERM: new permitted {:#x} not subset of {:#x} (securebits {:#x} keepcaps {})",
            unsafe { (*current).pid() },
            cred.euid,
            new_permitted.bits(),
            cred.cap_permitted.bits(),
            cred.securebits,
            cred.keepcaps
        );
        return -(errno::EPERM as i64);
    }
    if !new_inheritable.is_subset_of(cred.cap_inheritable) {
        if !cred.cap_effective.has(crate::security::CAP_SETPCAP)
            || !new_inheritable.is_subset_of(cred.cap_bounding)
        {
            return -(errno::EPERM as i64);
        }
    }
    if !new_effective.is_subset_of(new_permitted) {
        return -(errno::EPERM as i64);
    }

    // Apply changes (bounding set is not modified by capset)
    // SAFETY: current is a valid task pointer; cred_mut() returns mutable reference.
    unsafe {
        let cred_mut = (*current).cred_mut();
        cred_mut.cap_effective = new_effective;
        cred_mut.cap_permitted = new_permitted;
        cred_mut.cap_inheritable = new_inheritable;
    }

    0
}

/// sys_personality - Set process execution domain
///
/// Store-and-readback contract: personality(x) stores x and returns the
/// PREVIOUS personality; personality(0xffffffff) is the query sentinel —
/// the value is returned unchanged with nothing stored. The stored value
/// inherits across fork and survives execve (setarch-style flag
/// personalities like ADDR_NO_RANDOMIZE depend on the exec survival).
/// 0 = PER_LINUX. No validation: the execution-domain table semantics
/// were retired upstream, so any value stores and reads back verbatim.
pub fn sys_personality(args: SyscallArgs) -> i64 {
    let new = args[0] as u32;
    match crate::sched::current() {
        Some(current) => {
            let old = unsafe { (*current).personality };
            if new != 0xffffffff {
                unsafe { (*current).personality = new; }
            }
            old as i64
        }
        None => 0,
    }
}

/// sys_pivot_root - Change root filesystem (NR 41, U1c)
///
/// pivot_root(new_root, put_old):
/// - new_root must be a directory AND a mountpoint of the current mount
///   namespace (a tmpfs/procfs/... mounted there via mount(2));
/// - put_old must be a directory underneath new_root and different from it;
/// - both must be on the same filesystem (minimal: both resolvable);
/// - new_root must not be "/".
///
/// Minimal implementation (recorded divergence from Linux): the VFS dentry
/// tree is global and shared, so instead of moving the root mount object
/// the kernel (a) reparents the fs_struct root prefix to new_root — every
/// subsequent absolute lookup resolves inside the new root, exactly like
/// the post-pivot view, (b) records the OLD root as a mount-table row at
/// put_old (so /proc/mounts shows the old root there and umount(put_old)
/// can drop it), and (c) rewrites the remaining mount-table rows relative
/// to the new root, and (d) resets the cwd to the new root when it now
/// lies outside the jail.
pub fn sys_pivot_root(args: SyscallArgs) -> i64 {
    use crate::arch::uaccess::strncpy_from_user;

    // CAP_SYS_ADMIN required (Linux).
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return -(errno::EPERM as i64);
    }

    let new_root_ptr = args[0] as *const u8;
    let put_old_ptr = args[1] as *const u8;
    if new_root_ptr.is_null() || put_old_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    let mut nr_buf = [0u8; 256];
    let mut po_buf = [0u8; 256];
    let new_root = match strncpy_from_user(new_root_ptr, 255, &mut nr_buf) {
        Ok(s) => core::str::from_utf8(s).unwrap_or(""),
        Err(_) => return -(errno::EFAULT as i64),
    };
    let put_old = match strncpy_from_user(put_old_ptr, 255, &mut po_buf) {
        Ok(s) => core::str::from_utf8(s).unwrap_or(""),
        Err(_) => return -(errno::EFAULT as i64),
    };

    let new_root = crate::fs::path::path_normalize(new_root);
    let put_old = crate::fs::path::path_normalize(put_old);
    if !new_root.starts_with('/') || !put_old.starts_with('/') {
        return -(errno::EINVAL as i64);
    }
    if new_root == "/" {
        return -(errno::EINVAL as i64);
    }

    // Both must exist and be directories.
    let mut st = crate::fs::Stat::new();
    if let Err(e) = crate::fs::vfs::stat_file_by_path(&new_root, &mut st) {
        return e as i64;
    }
    if st.st_mode & 0o170000 != 0o040000 {
        return -(errno::ENOTDIR as i64);
    }
    let mut st2 = crate::fs::Stat::new();
    if let Err(e) = crate::fs::vfs::stat_file_by_path(&put_old, &mut st2) {
        return e as i64;
    }
    if st2.st_mode & 0o170000 != 0o040000 {
        return -(errno::ENOTDIR as i64);
    }

    // put_old must be underneath new_root — or EQUAL to it: Linux's check
    // is is_subdir(put_old, new_root), and is_subdir(d, d) is true, so the
    // documented idiom pivot_root(new_root, new_root) is legal. OH's
    // appspawn sandbox (sandbox_core.cpp:1269) uses it; the old
    // put_old == new_root → EINVAL rejected every sandbox pivot.
    let under = put_old == new_root
        || put_old.starts_with(&if new_root == "/" {
            alloc::string::String::from("/")
        } else {
            alloc::format!("{}/", new_root)
        });
    if !under {
        crate::pr_info!(
            "pivot_root EINVAL(not-under): new='{}' put_old='{}'",
            new_root,
            put_old
        );
        return -(errno::EINVAL as i64);
    }

    // new_root must be a mountpoint of the current mount namespace.
    if !crate::process::ns::ns_is_mountpoint(&new_root) {
        crate::pr_info!(
            "pivot_root EINVAL(not-mountpoint): new='{}' put_old='{}'",
            new_root,
            put_old
        );
        return -(errno::EINVAL as i64);
    }

    let Some(current) = crate::sched::current() else {
        return -(errno::ESRCH as i64);
    };

    // (a) jail: fs root prefix → new_root (chroot-style, ".." clamped).
    // Record the jail-root DENTRY too (same as chroot): getcwd derives
    // root-relative paths from it, and the string self-heals from it if
    // mounts move. Best-effort — a failed lookup keeps string-only root.
    let root_dentry = match crate::fs::vfs::path_lookup(
        &new_root,
        crate::fs::vfs::LOOKUP_FOLLOW,
    ) {
        Ok(vpath) => vpath.dentry.clone(),
        Err(_) => None,
    };
    // sc29 hunt: the recorded root STRING is the user argument, but the
    // resolved dentry may live at a different GLOBAL path (a chroot prefix
    // like /usr can be in effect at pivot time) — every later prefixed
    // lookup walks the string against the global tree. Print both.
    if let Some(rd) = root_dentry.as_ref() {
        crate::pr_info!(
            "pivot_root: arg='{}' resolved dentry global='{}'",
            new_root,
            rd.build_path()
        );
    }
    if let Some(fs) = current.fs_arc() {
        fs.set_root_with_dentry(new_root.as_bytes(), root_dentry);
        // Record put_old for the post-pivot cleanup umount2 (Linux leaves
        // the old root MOUNTED at put_old; our jail-style pivot stacks
        // nothing, so the matching umount2(MNT_DETACH) consumes the marker
        // and reports success — OH's sandbox teardowns fail otherwise).
        fs.set_pivot_put_old(put_old.as_bytes());
    }
    // (d) cwd: move into the new root when the old cwd fell outside it.
    let cwd = current.get_cwd();
    let cwd_str = core::str::from_utf8(&cwd).unwrap_or("/");
    let cwd_norm = crate::fs::path::path_normalize(cwd_str);
    let inside = cwd_norm == "/" || cwd_norm.starts_with(&new_root);
    if !inside {
        // String-only reset (clears any cwd dentry — the string is the
        // truth again for this legacy-compat branch).
        current.set_cwd(b"/");
    }

    // (b) + (c) mount-namespace bookkeeping: the old root becomes a row at
    // put_old (new-root-relative), other rows are rebased onto new_root.
    {
        use crate::process::ns::MountEntry;
        let ns = crate::process::ns::current_mnt_ns();
        let mut table = ns.mounts.lock();
        // Old root row ("/" or the previous root device) moves to put_old.
        let old_root_entry = table
            .iter()
            .find(|e| e.mount_point == "/")
            .cloned()
            .unwrap_or(MountEntry {
                device: alloc::string::String::from("rootfs"),
                mount_point: alloc::string::String::from("/"),
                fs_type: alloc::string::String::from("rootfs"),
                flags: alloc::string::String::from("rw"),
            });
        let rel_put_old = put_old
            .strip_prefix(&new_root)
            .unwrap_or("")
            .trim_start_matches('/');
        table.retain(|e| e.mount_point != "/");
        // Rebase rows that live under new_root.
        let prefix = if new_root == "/" {
            alloc::string::String::from("/")
        } else {
            new_root.clone()
        };
        for e in table.iter_mut() {
            if e.mount_point.starts_with(&prefix) && e.mount_point.len() >= prefix.len() {
                let rel = e.mount_point[prefix.len()..].trim_start_matches('/');
                e.mount_point = if rel.is_empty() {
                    alloc::string::String::from("/")
                } else {
                    alloc::format!("/{}", rel)
                };
            }
        }
        // The old root at put_old (relative to the NEW root).
        table.push(MountEntry {
            device: old_root_entry.device,
            mount_point: if rel_put_old.is_empty() {
                alloc::string::String::from("/oldroot")
            } else {
                alloc::format!("/{}", rel_put_old)
            },
            fs_type: old_root_entry.fs_type,
            flags: old_root_entry.flags,
        });
        // Update the namespace root anchor.
        *ns.root.lock() = crate::fs::vfs::get_vfs_root();
    }

    crate::pr_info!("pivot_root: new root '{}' (old root at '{}')", new_root, put_old);
    0
}

/// sys_setns - reassociate thread with a namespace (U1c)
///
/// # Arguments
/// - args[0]: fd - namespace file descriptor
/// - args[1]: nstype - namespace type (0 = any; CLONE_NEW* otherwise)
///
/// The fd comes from opening /proc/[pid]/ns/{uts,mnt,pid,net,ipc,user}.
/// PID ns fds only change where future children are created.
pub fn sys_setns(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let nstype = args[1];

    match crate::process::ns::setns(fd, nstype) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_getrlimit - Get resource limits (deprecated, use prlimit64)
///
/// # Arguments
/// - args[0]: resource - resource type (RLIMIT_*)
/// - args[1]: rlim - pointer to struct rlimit
///
/// Reads the real per-task values from task.rlimits.
pub fn sys_getrlimit(args: SyscallArgs) -> i64 {
    use crate::process::task::RLIM_NLIMITS;

    let resource = args[0] as i32;
    let rlim_ptr = args[1] as *mut u64;

    if resource < 0 || resource as usize >= RLIM_NLIMITS {
        return -(errno::EINVAL as i64);
    }
    if rlim_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(rlim_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    let (cur, max) = match crate::sched::current() {
        Some(task) => task.rlimit(resource as usize),
        None => return -(errno::ESRCH as i64),
    };

    // SAFETY: rlim_ptr validated with access_ok above; put_user is the
    // exception-table copy path.
    unsafe {
        let _ = crate::arch::uaccess::put_user(rlim_ptr, cur);
        let _ = crate::arch::uaccess::put_user(rlim_ptr.add(1), max);
    }
    0
}

/// sys_setrlimit - Set resource limits (deprecated, use prlimit64)
///
/// # Arguments
/// - args[0]: resource - resource type (RLIMIT_*)
/// - args[1]: rlim - pointer to struct rlimit
///
/// Stores into task.rlimits with the same validation as prlimit64
/// (see there); RLIMIT_STACK also reprograms the mm stack bound.
pub fn sys_setrlimit(args: SyscallArgs) -> i64 {
    use crate::process::task::{rlimit_res, RLIM_NLIMITS};

    let resource = args[0] as i32;
    let rlim_ptr = args[1] as *const u64;

    if resource < 0 || resource as usize >= RLIM_NLIMITS {
        return -(errno::EINVAL as i64);
    }
    let resource = resource as usize;
    if rlim_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(rlim_ptr as usize, 16) {
        return -(errno::EFAULT as i64);
    }

    // SAFETY: rlim_ptr validated with access_ok; get_user is the
    // exception-table copy path (SUM=0 safe).
    let rlim_cur = unsafe { crate::arch::uaccess::get_user(rlim_ptr).unwrap_or(0) };
    let rlim_max = unsafe { crate::arch::uaccess::get_user(rlim_ptr.add(1)).unwrap_or(0) };

    if rlim_cur > rlim_max {
        return -(errno::EINVAL as i64);
    }

    // RLIMIT_NOFILE hard-limit ceiling (Linux do_prlimit): raising
    // rlim_max above the fs.nr_open sysctl default (1024*1024) is EPERM
    // even for root — the kernel refuses descriptors beyond the fdtable
    // encoding. LTP setrlimit03 sets rlim_max = nr_open+1 as root and
    // requires EPERM, not success.
    if resource == rlimit_res::NOFILE && rlim_max > 1024 * 1024 {
        return -(errno::EPERM as i64);
    }

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::ESRCH as i64),
    };

    let old_max = current.rlimit(resource).1;
    let privileged = current.cred().euid == 0
        || crate::security::capable(crate::security::CAP_SYS_RESOURCE);
    if (rlim_max > old_max || rlim_cur > old_max) && !privileged {
        return -(errno::EPERM as i64);
    }

    current.rlimits.lock_irqsave()[resource] = (rlim_cur, rlim_max);

    // Keep the mm stack growth bound in sync with RLIMIT_STACK.
    if resource == rlimit_res::STACK {
        sync_stack_limit(current, rlim_cur);
    }

    0
}

/// sys_getrusage - Get resource usage
///
/// # Arguments
/// - args[0]: who - RUSAGE_SELF (0), RUSAGE_CHILDREN (-1), RUSAGE_THREAD (1)
/// - args[1]: rusage - pointer to struct rusage
///
/// struct rusage is 144 bytes on riscv64 (the old 136-byte write left the
/// last fields — ru_maxrss etc. — as garbage for `time`/`make` parsers).
pub fn sys_getrusage(args: SyscallArgs) -> i64 {
    let who = args[0] as i32;
    let rusage_ptr = args[1] as *mut u8;

    if rusage_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(rusage_ptr as usize, 144) {
        return -(errno::EFAULT as i64);
    }

    const RUSAGE_SELF: i32 = 0;
    const RUSAGE_CHILDREN: i32 = -1;
    const RUSAGE_THREAD: i32 = 1;
    // Linux getrusage: any `who` outside {-1, 0, 1} is EINVAL before the
    // pointer checks (LTP getrusage02 probes -2).
    if who != RUSAGE_SELF && who != RUSAGE_CHILDREN && who != RUSAGE_THREAD {
        return -(errno::EINVAL as i64);
    }

    // struct rusage layout (bytes):
    //   0  ru_utime.tv_sec     8  ru_utime.tv_usec
    //   16 ru_stime.tv_sec     24 ru_stime.tv_usec
    //   32 ru_maxrss .. 40 ru_minflt 48 ru_majflt 56 ru_inblock 64 ru_oublock
    //   72 ru_nvcsw 80 ru_nivcsw (padded to 96) — struct is 96 bytes on musl;
    //   the old code zeroed 144 (glibc layout), keep zeroing the whole 144.
    let (utime_ticks, stime_ticks) = match crate::sched::current() {
        Some(t) => unsafe {
            use core::sync::atomic::Ordering::Relaxed;
            if who == RUSAGE_CHILDREN {
                (
                    (*t).cutime_ticks.load(Relaxed),
                    (*t).cstime_ticks.load(Relaxed),
                )
            } else {
                // SELF and THREAD both report the calling task's own time
                // (threads are Tasks here; thread-group totals live on the
                // leader after the folding in do_exit).
                (
                    (*t).utime_ticks.load(Relaxed),
                    (*t).stime_ticks.load(Relaxed),
                )
            }
        },
        None => (0, 0),
    };

    // Ticks (HZ=100) -> timeval: tv_sec = ticks/HZ, tv_usec = (ticks%HZ)*1e4.
    const HZ: u64 = crate::drivers::timer::HZ;
    let utv = (utime_ticks / HZ, (utime_ticks % HZ) * 10_000);
    let stv = (stime_ticks / HZ, (stime_ticks % HZ) * 10_000);

    // ru_maxrss (KB): SELF/THREAD — resident pages of the current mm;
    // CHILDREN — max reaped-child resident set at exit (cmaxrss_kb,
    // folded in do_exit; LTP getrusage03's inherit_fork2/grandchild
    // assertions read it).
    let maxrss_kb: u64 = if who == RUSAGE_CHILDREN {
        crate::sched::current()
            .map(|t| unsafe {
                (*t).cmaxrss_kb
                    .load(core::sync::atomic::Ordering::Relaxed)
            })
            .unwrap_or(0)
    } else {
        crate::sched::current()
            .and_then(|t| unsafe { (*t).address_space() })
            .map(|mm| mm.rss() * 4)
            .unwrap_or(0)
    };

    // SAFETY: rusage_ptr validated with access_ok; zero fill via the
    // exception-table copy, then per-field put_user updates.
    unsafe {
        let zeros = [0u8; 144];
        if crate::arch::uaccess::copy_to_user(rusage_ptr, zeros.as_ptr(), 144) != 0 {
            return -(errno::EFAULT as i64);
        }
        let put = crate::arch::uaccess::put_user::<u64>;
        let _ = put(rusage_ptr as *mut u64, utv.0);
        let _ = put(rusage_ptr.add(8) as *mut u64, utv.1);
        let _ = put(rusage_ptr.add(16) as *mut u64, stv.0);
        let _ = put(rusage_ptr.add(24) as *mut u64, stv.1);
        let _ = put(rusage_ptr.add(32) as *mut u64, maxrss_kb);
    }
    0
}

/// sys_sethostname - Set hostname (U1c: writes the CALLER's UTS namespace)
///
/// # Arguments
/// - args[0]: name - pointer to hostname string
/// - args[1]: len - hostname length
pub fn sys_sethostname(args: SyscallArgs) -> i64 {
    let name_ptr = args[0] as *const u8;
    let len = args[1] as usize;

    // CAP_SYS_ADMIN required to set hostname
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return -(errno::EPERM as i64);
    }

    if len == 0 || len > 64 {
        return -(errno::EINVAL as i64);
    }
    if name_ptr.is_null() {
        // Linux fails the strncpy_from_user on NULL with EFAULT (LTP
        // setdomainname02 / sethostname02 "name == NULL" cases) — not
        // EINVAL.
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(name_ptr as usize, len) {
        return -(errno::EFAULT as i64);
    }

    let mut buf = [0u8; 64];
    // SAFETY: name_ptr validated with access_ok(len); exception-table copy.
    unsafe {
        if crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), name_ptr, len) != 0 {
            return -(errno::EFAULT as i64);
        }
    }
    // Only the current UTS namespace is affected (isolation contract).
    crate::process::ns::current_uts_ns().set_hostname(&buf[..len]);
    0
}

/// sys_setdomainname - Set NIS domain name (U1c: caller's UTS namespace)
///
/// # Arguments
/// - args[0]: name - pointer to domain name string
/// - args[1]: len - domain name length
pub fn sys_setdomainname(args: SyscallArgs) -> i64 {
    let name_ptr = args[0] as *const u8;
    let len = args[1] as usize;

    // CAP_SYS_ADMIN required to set domain name
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return -(errno::EPERM as i64);
    }

    if len == 0 || len > 64 {
        return -(errno::EINVAL as i64);
    }
    if name_ptr.is_null() {
        // Linux fails the strncpy_from_user on NULL with EFAULT (LTP
        // setdomainname02 / sethostname02 "name == NULL" cases) — not
        // EINVAL.
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(name_ptr as usize, len) {
        return -(errno::EFAULT as i64);
    }

    let mut buf = [0u8; 64];
    // SAFETY: name_ptr validated with access_ok(len); exception-table copy.
    unsafe {
        if crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), name_ptr, len) != 0 {
            return -(errno::EFAULT as i64);
        }
    }
    crate::process::ns::current_uts_ns().set_domainname(&buf[..len]);
    0
}

/// sys_reboot - Reboot or halt the system
///
/// # Arguments
/// - args[0]: magic1 - magic number (LINUX_REBOOT_MAGIC1 = 0xfee1dead)
/// - args[1]: magic2 - magic number (LINUX_REBOOT_MAGIC2 or MAGIC2C)
/// - args[2]: cmd - reboot command
///
/// P1 shutdown cascade (never returns to the caller on the happy path):
/// 1. SIGTERM to PID 1 (systemd-style init shutdown sequence) — skipped
///    when the caller IS PID 1;
/// 2. wait for PID 1 to exit, bounded by a 5s timeout;
/// 3. sync: flush the buffer cache and the disk's volatile cache, then
///    mark the ext4 root superblock clean (minimal rootfs umount);
/// 4. SBI: SRST warm reset for RESTART, legacy shutdown ecall otherwise.
pub fn sys_reboot(args: SyscallArgs) -> i64 {
    let magic1 = args[0] as u32;
    let magic2 = args[1] as u32;
    let cmd = args[2] as u32;

    const LINUX_REBOOT_MAGIC1: u32 = 0xfee1dead;
    const LINUX_REBOOT_MAGIC2: u32 = 672274793;
    const LINUX_REBOOT_MAGIC2C: u32 = 85072278;

    const LINUX_REBOOT_CMD_RESTART: u32 = 0x01234567;
    const LINUX_REBOOT_CMD_HALT: u32 = 0xCDEF0123;
    const LINUX_REBOOT_CMD_POWER_OFF: u32 = 0x4321FEDC;
    const LINUX_REBOOT_CMD_CAD_OFF: u32 = 0x00000000;
    const LINUX_REBOOT_CMD_CAD_ON: u32 = 0x89ABCDEF;

    // CAP_SYS_BOOT required to reboot
    if !crate::security::capable(crate::security::CAP_SYS_BOOT) {
        return -(errno::EPERM as i64);
    }

    if magic1 != LINUX_REBOOT_MAGIC1 || (magic2 != LINUX_REBOOT_MAGIC2 && magic2 != LINUX_REBOOT_MAGIC2C) {
        return -(errno::EINVAL as i64);
    }

    match cmd {
        LINUX_REBOOT_CMD_RESTART
        | LINUX_REBOOT_CMD_HALT
        | LINUX_REBOOT_CMD_POWER_OFF => {
            shutdown_cascade(cmd);
            // shutdown_cascade only returns on a total SBI failure.
            loop {
                core::hint::spin_loop();
            }
        }
        // C.A.D mode switches (util-linux `ctrlaltdel`): 0 = SIGINT to
        // init on Ctrl-Alt-Del (Linux default), 1 = immediate reboot.
        LINUX_REBOOT_CMD_CAD_OFF => {
            CAD_IMMEDIATE.store(false, core::sync::atomic::Ordering::Release);
            0
        }
        LINUX_REBOOT_CMD_CAD_ON => {
            CAD_IMMEDIATE.store(true, core::sync::atomic::Ordering::Release);
            0
        }
        _ => -(errno::EINVAL as i64),
    }
}

// ============================================================================
// Ctrl-Alt-Del (SA_CAD semantics)
// ============================================================================

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
pub fn cad_deliver_pending() {
    if !PENDING_CAD.swap(false, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let now = crate::drivers::timer::get_jiffies();
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
fn pid1_exited() -> bool {
    match crate::process::find_task_by_pid(1) {
        None => true,
        Some(t) => t.state().is_dead(),
    }
}

/// Set when a shutdown cascade is in flight (reboot(2) caller, the CAD
/// immediate path, or the init-exit fallback): a second entry must not
/// re-drive the teardown (double ext4 sync / racing SBI calls).
static SHUTDOWN_CASCADE_ACTIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// The POWER_OFF cmd value, shared by sys_reboot's match arms' callers
/// and the init-exit fallback (RESTART already has a module-level const
/// further up).
const LINUX_REBOOT_CMD_POWER_OFF: u32 = 0x4321FEDC;

/// Kernel fallback for "PID 1 exited" — the shutdown(8) init-signal path
/// ends here: init tears down every user process (including shutdown(8)
/// itself), exits, and the kernel completes the cascade. Without init no
/// orphan is ever reaped or respawned, so a real OS powers off (Linux
/// panics instead). Called from do_exit() of PID 1 before the ZOMBIE
/// transition; never returns unless another cascade already owns the
/// teardown (a reboot(2) caller is then waiting for exactly this exit).
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
        // SAFETY: bare port I/O on the reset port and a deliberate
        // faulting instruction; neither returns.
        unsafe {
            // Drain the 8042 input buffer first (writing the command
            // while it is full is dropped on real hardware).
            for _ in 0..100_000 {
                if crate::arch::cpu::inb(0x64) & 0x02 == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            crate::arch::cpu::outb(0x64, 0xFE);
            // Give the pulse time to land.
            for _ in 0..10_000_000 {
                core::hint::spin_loop();
            }
            // Triple fault: load a null/limit-0 IDT and int3 — the
            // vector lookup must fail, the double fault must fail the
            // same way, and the CPU (QEMU included) resets.
            static NULL_IDT: [u64; 2] = [0; 2];
            core::arch::asm!(
                "lidt [{}]",
                in(reg) &NULL_IDT,
                options(nomem, nostack)
            );
            core::arch::asm!("int3", options(nomem, nostack));
        }
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
    const LINUX_REBOOT_CMD_RESTART: u32 = 0x01234567;
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
fn shutdown_cascade(cmd: u32) {
    const LINUX_REBOOT_CMD_RESTART: u32 = 0x01234567;
    const LINUX_REBOOT_CMD_HALT: u32 = 0xCDEF0123;
    const LINUX_REBOOT_CMD_POWER_OFF: u32 = 0x4321FEDC;

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

    // --- 1. Tell init (PID 1) to shut down (SIGTERM, systemd sequence) ---
    let my_pid = crate::sched::get_current_pid();
    if my_pid != 1 {
        crate::println!("reboot: sending SIGTERM to init (pid 1)");
        let _ = crate::signal::send_signal(1, crate::signal::Signal::SIGTERM as i32);

        // --- 2. Wait for PID 1 to exit (bounded) ---
        let deadline = crate::drivers::timer::get_jiffies()
            + crate::drivers::timer::msecs_to_jiffies(SHUTDOWN_PID1_TIMEOUT_SECS * 1000);
        while crate::drivers::timer::get_jiffies() < deadline {
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
    if let Err(e) = crate::fs::ext4::ext4_shutdown_sync() {
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
                // No 8259/ACPI reset yet — park with interrupts waiting.
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
fn machine_park() -> ! {
    loop {
        crate::arch::cpu::wfi();
    }
}

/// SBI legacy shutdown ecall (FID 0x8) — the pre-0.3 power-off path.
#[cfg(feature = "riscv64")]
fn sbi_legacy_shutdown() {
    // SAFETY: privileged SBI ecall; reached only after the CAP_SYS_BOOT
    // gate in sys_reboot.
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") 0x8u64,
            out("a0") _,
            out("a1") _,
            options(nomem)
        );
    }
    // Some SBI implementations return instead of halting — park.
    loop {
        // SAFETY: wfi halts the hart until an interrupt.
        crate::arch::cpu::wfi();
    }
}

/// P1: sleep ~1 jiffy (10ms) so PID 1 and the exit path can run while the
/// reboot syscall waits for it (INTERRUPTIBLE sleep + one-shot timer, the
/// nanosleep/linger discipline).
fn sleep_one_tick() {
    use crate::process::task::{Task, TaskState};

    let current = match crate::sched::current() {
        Some(t) => t as *mut Task,
        None => return, // no task context: busy caller
    };
    // SAFETY: current is the running task's pointer (we are it).
    let pid = unsafe { (*current).pid() };
    let wake_at = crate::drivers::timer::get_jiffies() + 1;
    let timer_id = crate::timer::add_timer_wakeup(wake_at, pid);
    // SAFETY: setting our own task's state.
    unsafe {
        (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    }
    // R54: re-arm IRQs so the timer tick reaches this CPU.
    crate::arch::cpu::restore_irq(true);
    crate::sched::schedule();
    if timer_id != 0 {
        crate::timer::del_timer(timer_id);
    }
}

/// sys_unshare - Create new namespace (U1c)
///
/// # Arguments
/// - args[0]: flags - CLONE_NEWNS, CLONE_NEWUTS, CLONE_NEWIPC, etc.
///
/// Namespace semantics (the only unshare domains implemented):
/// - CLONE_NEWUTS/NEWNS/NEWNET/NEWIPC/NEWUSER: the caller itself moves into
///   a fresh namespace (mount table copy-on-write, UTS names copied).
/// - CLONE_NEWPID: only future children are created in the new pid
///   namespace (Linux semantics — the caller keeps its current pids).
/// - CLONE_NEWCGROUP: accepted, no-op (no cgroup subsystem).
///
/// CAP_SYS_ADMIN is required unless CLONE_NEWUSER is the only ns bit.
pub fn sys_unshare(args: SyscallArgs) -> i64 {
    let flags = args[0];

    // Only CLONE_* bits are meaningful; unknown bits (outside the CSIGNAL
    // byte) are EINVAL like clone.
    const UNSHARE_KNOWN: u64 = crate::process::ns::CLONE_NEWMASK
        | 0x00000200 /* CLONE_FS */
        | 0x00000400 /* CLONE_FILES */
        | 0x00010000 /* CLONE_THREAD (refused below, Linux EINVAL) */
        | 0x00400000 /* CLONE_DETACHED legacy */
        | 0x00800000 /* CLONE_UNTRACED legacy */;
    if flags & !UNSHARE_KNOWN & !0xff != 0 {
        return -(errno::EINVAL as i64);
    }
    if flags & 0x00010000 != 0 {
        // CLONE_THREAD cannot be unshared.
        return -(errno::EINVAL as i64);
    }

    // CLONE_FILES: copy the fd table away from any CLONE_FILES sharers
    // (Linux unshare_fdtable) — a thread that unshares must not close the
    // group's descriptors. CLONE_FS stays a no-op (the fs struct is
    // per-fork here; nothing shares it).
    if flags & 0x00000400 != 0 {
        if let Some(task) = crate::sched::current() {
            unsafe {
                if let Some(arc) = (*task).fdtable_arc() {
                    if alloc::sync::Arc::strong_count(&arc) > 2 {
                        let copy = alloc::sync::Arc::new(crate::fs::FdTable::fork_from(&*arc));
                        (*task).set_fdtable(Some(copy));
                    }
                }
            }
        }
    }

    match crate::process::ns::unshare_namespaces(flags) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// sys_syncfs - Sync filesystem of a file descriptor
///
/// # Arguments
/// - args[0]: fd - file descriptor
pub fn sys_syncfs(_args: SyscallArgs) -> i64 {
    // Flush all buffer cache (simplified: sync everything)
    let _ = crate::fs::bio::sync_buffers();
    0
}

/// sys_memfd_create - Create anonymous memory file (NR 279)
///
/// Implemented by `fs::memfd` (anonymous memory-backed regular file;
/// read/write/lseek/ftruncate/mmap all work, seals via fcntl
/// F_ADD_SEALS/F_GET_SEALS when MFD_ALLOW_SEALING is passed).
///
/// # Arguments
/// - args[0]: name - file name (advisory only)
/// - args[1]: flags - MFD_CLOEXEC, MFD_ALLOW_SEALING, MFD_NOEXEC_SEAL,
///   MFD_EXEC, MFD_NONBLOCK (MFD_HUGETLB is EINVAL — no hugetlbfs)
pub fn sys_memfd_create(args: SyscallArgs) -> i64 {
    let name_ptr = args[0] as *const u8;
    let flags = args[1] as u32;

    // The reference kernel reads the name with strnlen_user() bounded by
    // MFD_NAME_MAX_LEN + 1: a NULL pointer faults in that read and the
    // kernel answers EFAULT (the read returns a length of zero, which the
    // reference rejects), and a name longer than MFD_NAME_MAX_LEN is
    // EINVAL. The name itself is advisory in this kernel (kept out — there
    // is no dentry to name).
    if name_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    let mut name_buf = [0u8; 256];
    let name: &[u8] = match crate::arch::uaccess::strncpy_from_user(name_ptr, 256, &mut name_buf) {
        Ok(s) => s,
        Err(e) => return e as i64,
    };
    if name.len() > crate::fs::memfd::MFD_NAME_MAX_LEN {
        return -(errno::EINVAL as i64);
    }

    match crate::fs::memfd::memfd_create(name, flags) {
        Ok(fd) => fd as i64,
        Err(e) => e as i64,
    }
}

/// sys_ioprio_set - Set I/O scheduling priority
///
/// # Arguments
/// - args[0]: which - PRIO_PROCESS (0), PRIO_PGRP (1), PRIO_USER (2)
/// - args[1]: who - target PID/PGID/UID (0 = current)
/// - args[2]: ioprio - I/O priority class + value
pub fn sys_ioprio_set(args: SyscallArgs) -> i64 {
    let _which = args[0] as i32;
    let _who = args[1] as i32;
    let ioprio = args[2] as i32;

    // IOPRIO_CLASS_SHIFT = 13
    // Class bits: who << 13 | data
    let class = (ioprio >> 13) & 0x7;

    // IOPRIO_CLASS_NONE = 0, IOPRIO_CLASS_RT = 1, IOPRIO_CLASS_BE = 2, IOPRIO_CLASS_IDLE = 3
    if class > 3 {
        return -(errno::EINVAL as i64);
    }

    // CAP_SYS_NICE required to set RT or idle class
    if class == 1 || class == 3 {
        if !crate::security::capable(crate::security::CAP_SYS_NICE) {
            return -(errno::EPERM as i64);
        }
    }

    // Accept and ignore — no I/O priority storage yet
    0
}

/// sys_ioprio_get - Get I/O scheduling priority
///
/// # Arguments
/// - args[0]: which - PRIO_PROCESS (0), PRIO_PGRP (1), PRIO_USER (2)
/// - args[1]: who - target PID/PGID/UID (0 = current)
pub fn sys_ioprio_get(args: SyscallArgs) -> i64 {
    let _which = args[0] as i32;
    let _who = args[1] as i32;
    // Default I/O priority: IOPRIO_PRIO_VALUE(IO_PRIO_CLASS_BE, 0) = 0
    0
}

/// sys_quotactl - Disk quota operations
///
/// # Arguments
/// - args[0]: cmd - Q_QUOTAON, Q_QUOTAOFF, Q_GETQUOTA, etc.
/// - args[1]: special - path to filesystem
/// - args[2]: id - user/group ID
/// - args[3]: addr - pointer to dqblk structure
pub fn sys_quotactl(args: SyscallArgs) -> i64 {
    let cmd = args[0] as u32;
    let _special = args[1] as *const u8;
    let _id = args[2] as u32;
    let addr = args[3] as *mut u8;

    // Q_QUOTAOFF = 0x800001, Q_GETINFO = 0x800007, Q_GETFMT = 0x800800
    let subcmd = cmd >> 8;

    match subcmd {
        // Q_GETFMT: return quota format (4 bytes at addr)
        0x800 => {
            // Return -ENOTSUP to indicate no quota format
            if !addr.is_null() {
                if !crate::arch::uaccess::access_ok(addr as usize, 4) {
                    return -(errno::EFAULT as i64);
                }
                // SAFETY: addr validated with access_ok; put_user is the
                // exception-table copy path.
                unsafe { let _ = crate::arch::uaccess::put_user(addr as *mut i32, -1); }
            }
            0
        }
        // Q_GETINFO: return struct if_dqinfo (16 bytes)
        0x8000 => {
            if !addr.is_null() {
                if !crate::arch::uaccess::access_ok(addr as usize, 16) {
                    return -(errno::EFAULT as i64);
                }
                // SAFETY: addr validated with access_ok; clear_user is the
                // exception-table zeroing path.
                unsafe { crate::arch::uaccess::clear_user(addr, 16); }
            }
            0
        }
        _ => -(errno::ENOSYS as i64),
    }
}

/// sys_riscv_hwprobe - Probe RISC-V hardware features
///
/// # Arguments
/// - args[0]: pairs - pointer to key-value pairs
/// - args[1]: count - number of pairs
/// - args[2]: cpu_count - pointer to CPU count (or NULL)
/// - args[3]: cpus - pointer to CPU set (or NULL)
pub fn sys_riscv_hwprobe(args: SyscallArgs) -> i64 {
    let pairs_ptr = args[0] as *mut u64;
    let count = args[1] as usize;
    let _cpu_count_ptr = args[2] as *mut u32;
    let _cpus_ptr = args[3] as *const usize;

    if pairs_ptr.is_null() || count == 0 {
        return 0;
    }
    if !crate::arch::uaccess::access_ok(pairs_ptr as usize, count.saturating_mul(16)) {
        return -(errno::EFAULT as i64);
    }

    // struct riscv_hwprobe_pair { key, value }
    const KEY_MVENDORID: u64 = 0;
    const KEY_MARCHID: u64 = 1;
    const KEY_IMPID: u64 = 2;
    const KEY_BASE_BEHAVIOR: u64 = 3;
    const BASE_BEHAVIOR_IMA: u64 = 1 << 0;
    const KEY_IMA_EXT_0: u64 = 4;
    const IMA_EXT_FD: u64 = 1 << 0;
    const IMA_EXT_C: u64 = 1 << 1;
    const IMA_EXT_V: u64 = 1 << 2;

    // The V bit follows the boot-primed hart capability (DTB riscv,isa):
    // on a V-capable hart the kernel lazily enables RVV for user tasks
    // and the bit is honestly set; on the standard -cpu rv64 model it is
    // clear. The other letters are what the kernel itself runs on
    // (rv64gc). x86_64 compiles this handler too (the dispatch table is
    // shared); no riscv64 hart there, so the bit is a constant 0.
    #[cfg(feature = "riscv64")]
    let v_bit = if crate::arch::riscv64::vext::hart_has_v() {
        IMA_EXT_V
    } else {
        0
    };
    #[cfg(any(feature = "x86_64", feature = "aarch64"))]
    let v_bit = 0;

    // SAFETY: pairs_ptr validated with access_ok; get_user/put_user are the
    // exception-table copy paths (SUM=0 safe).
    unsafe {
        for i in 0..count {
            let key = crate::arch::uaccess::get_user(pairs_ptr.add(i * 2)).unwrap_or(u64::MAX);
            let value = match key {
                // mvendorid/marchid/mimpid are M-mode CSRs and trap in S-mode;
                // report 0 ("not implemented") like an SBI-less platform would.
                KEY_MVENDORID => 0,
                KEY_MARCHID => 0,
                KEY_IMPID => 0,
                KEY_MMU => 1, // sv39
                // Base behavior: IMA, the one defined bit.
                KEY_BASE_BEHAVIOR => BASE_BEHAVIOR_IMA,
                // The ISA-extension bitmap: the letters the kernel runs
                // on (rv64gc — IMAFD + C) plus the V bit when this hart
                // has V (the lazy-enable makes RVV user code executable,
                // so the bit is then true). The catch-all u64::MAX this
                // key used to return advertised EVERY extension, and a
                // current glibc's ifunc resolver read the phantom V bit,
                // selected its vector memset/memcpy variants and SIGILLed
                // on the -cpu rv64 model of the standard flows (observed
                // 2026-10: vsetivli inside glibc __memset after the
                // toolchain moved to GCC 15, whose default march carries
                // V).
                KEY_IMA_EXT_0 => IMA_EXT_FD | IMA_EXT_C | v_bit,
                _ => u64::MAX,
            };
            let _ = crate::arch::uaccess::put_user(pairs_ptr.add(i * 2 + 1), value);
        }
    }

    // Linux returns 0 (not the pair count) on success.
    0
}

/// sys_riscv_flush_icache - Flush instruction cache
///
/// # Arguments
/// - args[0]: start - start address
/// - args[1]: size - size in bytes
/// - args[2]: flags - SYS_RISCV_FLUSH_ICACHE_ALL
pub fn sys_riscv_flush_icache(args: SyscallArgs) -> i64 {
    let _start = args[0] as usize;
    let _size = args[1] as usize;
    let flags = args[2] as u32;

    const SYS_RISCV_FLUSH_ICACHE_ALL: u32 = 1;

    if flags & SYS_RISCV_FLUSH_ICACHE_ALL != 0 {
        // Flush entire I-cache: use fence.i
        // SAFETY: fence.i is a valid RISC-V instruction, always safe to execute.
        unsafe { crate::arch::cpu::isb(); }
    } else {
        // Flush specific range: fence.i is sufficient for RISC-V
        // SAFETY: fence.i is a valid RISC-V instruction, always safe to execute.
        unsafe { crate::arch::cpu::isb(); }
    }

    0
}

// ============================================================================
// NR 294: kexec_file_load
// ============================================================================

/// sys_kexec_file_load - Load new kernel from file descriptor (NR 294)
pub fn sys_kexec_file_load(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

// ============================================================================
// NR 424-440: pidfd, io_uring, clone3, close_range, etc.
// ============================================================================

/// sys_pidfd_send_signal - Send signal to process via pidfd (NR 424)
pub fn sys_pidfd_send_signal(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_io_uring_setup - Setup io_uring instance (NR 425)
pub fn sys_io_uring_setup(args: SyscallArgs) -> i64 {
    crate::io_uring::sys_io_uring_setup(args) as i64
}

/// sys_io_uring_enter - Enter io_uring (NR 426)
pub fn sys_io_uring_enter(args: SyscallArgs) -> i64 {
    crate::io_uring::sys_io_uring_enter(args) as i64
}

/// sys_io_uring_register - Register io_uring buffers/files (NR 427)
pub fn sys_io_uring_register(args: SyscallArgs) -> i64 {
    crate::io_uring::sys_io_uring_register(args) as i64
}

/// sys_clone3 - Create child process (extended) (NR 435)
///
/// Reads `struct clone_args` (uapi, 64-byte v1 layout) from user memory:
///   +0 flags, +8 pidfd, +16 child_tid, +24 parent_tid, +32 exit_signal,
///   +40 stack, +48 stack_size, +56 tls
/// and maps it onto the plain-clone machinery.
pub fn sys_clone3(args: SyscallArgs) -> i64 {
    use crate::process::fork::{do_clone, CloneArgs};

    let uargs = args[0] as *const u8;
    let size = args[1] as usize;

    if uargs.is_null() {
        return -(errno::EFAULT as i64);
    }
    // Linux copy_struct_from_user semantics (kernel/fork.c
    // copy_clone_args_from_user): size below the 64-byte v1 struct is
    // EINVAL; a larger size must still be READ from user space (EFAULT
    // when the extra bytes are unmapped — LTP clone302 "extra size"
    // places the struct at the end of a mapping) and the extra bytes
    // must be zero (E2BIG otherwise).
    if size < 64 {
        return -(errno::EINVAL as i64);
    }
    if size > 4096 {
        return -(errno::E2BIG as i64);
    }
    if !crate::arch::uaccess::access_ok(uargs as usize, size) {
        return -(errno::EFAULT as i64);
    }

    let mut buf = [0u8; 4096];
    // SAFETY: access_ok-validated pointer; exception-table copy.
    if unsafe {
        crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), uargs, size)
    } != 0
    {
        return -(errno::EFAULT as i64);
    }
    if size > 64 && buf[64..size].iter().any(|&b| b != 0) {
        return -(errno::E2BIG as i64);
    }

    let rd64 = |off: usize| u64::from_le_bytes(buf[off..off + 8].try_into().unwrap());
    let flags = rd64(0);
    let pidfd_ptr = rd64(8) as *mut u32;
    let child_tid = rd64(16) as *mut i32;
    let parent_tid = rd64(24) as *mut i32;
    let exit_signal = rd64(32) as u64;
    let stack = rd64(40);
    let stack_size = rd64(48);
    let tls = rd64(56);

    // clone3 keeps the exit signal in its own field; legacy CSIGNAL bits
    // in flags are rejected (kernel/fork.c clone3: "make the CSIGNAL bits
    // unusable"), as is an out-of-range exit_signal (LTP clone302
    // "invalid signal": CSIGNAL+1).
    const CSIGNAL_MASK: u64 = 0xff;
    if flags & CSIGNAL_MASK != 0 {
        return -(errno::EINVAL as i64);
    }
    if exit_signal & !CSIGNAL_MASK != 0 || exit_signal > 64 {
        return -(errno::EINVAL as i64);
    }

    // stack/stack_size must come as a pair (LTP clone302 "zero-stack-size"
    // passes stack != 0 with size 0; "invalid-stack" passes stack 0 with
    // size 4 — both EINVAL).
    if (stack == 0) != (stack_size == 0) {
        return -(errno::EINVAL as i64);
    }

    // CLONE_PIDFD: the pidfd field points at the user word that receives
    // the new pidfd. Probe it for writability now (before the child is
    // created): an unmapped/word address fails EFAULT (LTP clone302
    // "invalid pidfd"). Writing the actual fd stays future work — only
    // the fault probe is observable by current callers.
    if flags & 0x00001000 != 0 && !pidfd_ptr.is_null() {
        let root = crate::sched::current()
            .and_then(|t| t.address_space())
            .map(|a| a.root_ppn());
        let ok = match root {
            Some(root_ppn) => unsafe {
                crate::arch::mm::mm_ops::PageTableWalker::walk(
                    root_ppn,
                    pidfd_ptr as usize as u64 & !0xfff,
                )
                .map(|(_, pte)| pte & crate::arch::mm::PageTableEntry::W != 0
                    && pte & crate::arch::mm::PageTableEntry::U != 0)
                .unwrap_or(false)
            },
            None => false,
        };
        if !ok {
            return -(errno::EFAULT as i64);
        }
    }

    // The ABI expects `stack` to be the stack TOP (sp), matching plain
    // clone's a1 semantics when stack_size is present; if a caller passes
    // a range we honor sp = stack + size like musl's gcompat handling.
    let sp = if stack_size > 0 { stack.wrapping_add(stack_size) } else { stack };

    // F12 heap-reserve gate — same rationale as sys_clone.
    if clone_heap_reserve_low() {
        return -(errno::ENOMEM as i64);
    }

    let clone_args = CloneArgs {
        flags,
        stack: sp,
        parent_tid,
        child_tid,
        tls,
        exit_signal: exit_signal as u8,
    };

    // sc119 insurance: the appclone census lives in sys_clone; a caller
    // using clone3 (this path) would bypass it, muddying the "no clone
    // ever attempted" decode. Same filter, same 80-line budget counter
    // is NOT shared (this one has its own — clone3 use by OH runtimes
    // is unlikely, the budget here is small and separate).
    {
        let comm = crate::sched::current()
            .map(|t| unsafe { (*t).comm() })
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        let is_app = comm.contains("launcher")
            || comm.contains("settingsdata")
            || comm.contains("systemui")
            || comm.contains("dataability");
        let is_spawn = comm == "appspawn";
        if is_app || is_spawn {
            static N4: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if N4.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 20 {
                crate::pr_warn!("appclone3: comm='{}' flags={:#x}", comm, flags);
            }
        }
    }

    match do_clone(clone_args) {
        Ok(pid) => {
            // Parent-namespace view, same as sys_clone's return.
            let parent_ns = crate::sched::current().and_then(|t| unsafe { (*t).ns_pid.clone() });
            crate::process::ns::translate_sender_pid_ns(pid, parent_ns.as_ref()) as i64
        }
        Err(e) => e as i64,
    }
}

/// sys_close_range - Close file descriptors in range (NR 436)
///
/// Linux clamps the upper bound to the fdtable ceiling, so
/// close_range(3, ~0U, 0) — what GLib's g_spawn uses between fork and
/// exec — must not walk the whole u32 space. Flags: bit 0
/// CLOSE_RANGE_CLOEXEC sets FD_CLOEXEC instead of closing; bit 1
/// CLOSE_RANGE_UNSHARE only matters for CLONE_FILES threads (no-op
/// here). Unknown flags are EINVAL like Linux.
pub fn sys_close_range(args: SyscallArgs) -> i64 {
    const CLOSE_RANGE_CLOEXEC: u32 = 1 << 0;
    const CLOSE_RANGE_UNSHARE: u32 = 1 << 1;

    let fd = args[0] as u32;
    let max_fd = args[1] as u32;
    let flags = args[2] as u32;

    if fd > max_fd {
        return -(errno::EINVAL as i64);
    }
    if flags & !(CLOSE_RANGE_CLOEXEC | CLOSE_RANGE_UNSHARE) != 0 {
        return -(errno::EINVAL as i64);
    }

    // CLOSE_RANGE_UNSHARE: copy the fd table before closing when it is
    // shared with CLONE_FILES siblings (Linux unshare_fdtable_221b8ba —
    // the close must not reach the sharers' table). LTP close_range01/02:
    // a CLONE_FILES child doing close_range(..., UNSHARE) must leave the
    // parent's descriptors open; without the copy the child closed the
    // shared table and the parent's very next fcntl died EBADF.
    if flags & CLOSE_RANGE_UNSHARE != 0 {
        if let Some(task) = crate::sched::current() {
            unsafe {
                if let Some(arc) = (*task).fdtable_arc() {
                    // fdtable_arc() itself holds one reference: >2 means a
                    // real sharer exists beyond the caller's own.
                    if alloc::sync::Arc::strong_count(&arc) > 2 {
                        let copy = alloc::sync::Arc::new(crate::fs::FdTable::fork_from(&*arc));
                        (*task).set_fdtable(Some(copy));
                    }
                }
            }
        }
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    // The table cannot hold fds >= MAX_FDS: clamp once instead of
    // spinning through billions of always-EBADF close_fd() calls.
    let upper = (max_fd as usize).min(crate::fs::file::MAX_FDS - 1);

    if flags & CLOSE_RANGE_CLOEXEC != 0 {
        for target_fd in (fd as usize)..=upper {
            fdtable.set_fd_cloexec(target_fd, true);
        }
        return 0;
    }

    for target_fd in (fd as usize)..=upper {
        // fd not open → skip (Linux semantics: silently ignore)
        let _ = fdtable.close_fd(target_fd);
    }
    // Linux returns 0 (not the number of closed fds).
    0
}

/// sys_pidfd_open - Get pidfd for process (NR 434)
pub fn sys_pidfd_open(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_pidfd_getfd - Get file descriptor from process via pidfd (NR 438)
pub fn sys_pidfd_getfd(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_faccessat2 - Check file access permissions (extended) (NR 439)
pub fn sys_faccessat2(args: SyscallArgs) -> i64 {
    let dirfd = args[0] as i32;
    let pathname_ptr = args[1] as *const u8;
    let mode = args[2] as i32;
    let _flags = args[3] as i32;
    // Delegate to faccessat (NR 48), ignoring extra flags
    let faccessat_args = [dirfd as u64, pathname_ptr as u64, mode as u64, 0, 0, 0];
    crate::syscall::file::sys_faccessat(faccessat_args)
}

/// sys_process_madvise - Advise kernel about process memory (NR 440)
pub fn sys_process_madvise(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_memfd_secret - Create anonymous memory file (secret) (NR 447)
pub fn sys_memfd_secret(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}

/// sys_process_mrelease - Release process memory (NR 448)
pub fn sys_process_mrelease(_args: SyscallArgs) -> i64 {
    -(errno::ENOSYS as i64)
}
