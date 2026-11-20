//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! ptrace — process tracing (P1: strace/gdb foundation)
//!
//! Supported operations:
//! - PTRACE_TRACEME / PTRACE_ATTACH / PTRACE_DETACH
//! - PTRACE_PEEKTEXT / PEEKDATA / PEEKUSR (read target memory / registers
//!   through the TARGET's page tables — same discipline as procfs
//!   read_target_user_mm)
//! - PTRACE_POKETEXT / POKEDATA / POKEUSR
//! - PTRACE_GETREGSET / SETREGSET (NT_PRSTATUS) and the legacy
//!   GETREGS/SETREGS form (RISC-V user_regs_struct = 32 × u64, exactly the
//!   first 32 words of PtRegs: pc, ra, sp, gp, tp, t0..t6, s0..s11, a0..a7)
//! - PTRACE_CONT / PTRACE_SINGLESTEP. RISC-V has no hardware single-step:
//!   SINGLESTEP displaces the instruction at the tracee's epc with a
//!   temporary EBREAK and reports SIGTRAP from the trap handler, which
//!   restores the displaced instruction (see trap.rs handle_breakpoint).
//! - PTRACE_KILL, PTRACE_SETOPTIONS, PTRACE_GETSIGINFO / SETSIGINFO
//!
//! Explicitly out of scope: PTRACE_SYSCALL (syscall interception),
//! hardware watchpoints, PTRACE_SEIZE/LISTEN/INTERRUPT, multithreaded
//! attach (only the addressed thread is traced).
//!
//! Design notes:
//! - The tracee relationship is stored as `tracer_pid` on the tracee (not a
//!   raw Task pointer): a tracer that exits would leave a dangling pointer
//!   behind, while a stale pid is validated by every lookup.
//! - A traced task intercepts EVERY delivered signal (except SIGKILL) in
//!   do_signal(): instead of running the disposition it enters
//!   group-stop-like STOPPED state and wakes the tracer with SIGCHLD. The
//!   signal is dequeued into task.ptrace_siginfo so PTRACE_CONT(data=0)
//!   swallows it and CONT(data=sig) re-injects it.
//! - Tracee stops use the regular STOPPED state + stop_reported=false, so
//!   the existing wait4 WUNTRACED/WIFSTOPPED machinery reports them without
//!   modification (works when the tracer is the tracee's real parent — the
//!   TRACEME and fork+ATTACH usage patterns; ATTACH to an unrelated process
//!   stops the tracee but wait4 cannot reap it, matching no-reparent
//!   simplicity).

use crate::process::task::Task;
use crate::signal::{SigInfo, Signal};
use crate::syscall::SyscallArgs;

// ==================== ptrace request numbers (asm-generic) ====================

pub const PTRACE_TRACEME: u32 = 0;
pub const PTRACE_PEEKTEXT: u32 = 1;
pub const PTRACE_PEEKDATA: u32 = 2;
pub const PTRACE_PEEKUSR: u32 = 3;
pub const PTRACE_POKETEXT: u32 = 4;
pub const PTRACE_POKEDATA: u32 = 5;
pub const PTRACE_POKEUSR: u32 = 6;
pub const PTRACE_CONT: u32 = 7;
pub const PTRACE_KILL: u32 = 8;
pub const PTRACE_SINGLESTEP: u32 = 9;
pub const PTRACE_GETREGS: u32 = 12;
pub const PTRACE_SETREGS: u32 = 13;
pub const PTRACE_ATTACH: u32 = 16;
pub const PTRACE_DETACH: u32 = 17;
pub const PTRACE_SETOPTIONS: u32 = 0x4200;
pub const PTRACE_GETSIGINFO: u32 = 0x4202;
pub const PTRACE_SETSIGINFO: u32 = 0x4203;
pub const PTRACE_GETREGSET: u32 = 0x4204;
pub const PTRACE_SETREGSET: u32 = 0x4205;

/// NT_PRSTATUS note type (elf.h).
pub const NT_PRSTATUS: u32 = 1;

/// RISC-V user_regs_struct size: 32 × u64 (pc, x1..x31).
pub const GREGSET_BYTES: usize = 32 * 8;

/// EBREAK instruction encoding (used by PTRACE_SINGLESTEP).
pub const EBREAK_INSN: u64 = 0x0010_0073;

const EPERM: i64 = 1;
const ESRCH: i64 = 3;
const EIO: i64 = 5;
const EINVAL: i64 = 22;

// ==================== permission helpers ====================

/// ptrace_may_access (minimal): same-uid credentials or CAP_SYS_PTRACE.
pub fn ptrace_may_access(target_cred: &crate::process::task::Cred) -> bool {
    if let Some(cur) = crate::sched::current() {
        // SAFETY: current is the running task; cred() is immutable.
        let cred = crate::process::unsafe_edges::borrow_task(cur as *mut Task).cred();
        if cred.euid == 0
            || cred.euid == target_cred.euid
            || cred.uid == target_cred.uid
        {
            return true;
        }
    }
    crate::security::capable(crate::security::CAP_SYS_PTRACE)
}

// ==================== target memory access ====================

/// Read one word of target user memory at `addr`.
///
/// If the page is not present, a read fault is driven on the TARGET's mm
/// first (this is what makes PEEKTEXT work on demand-paged, not-yet-loaded
/// text). Returns None when the address is unmapped/unreadable.
pub fn read_target_word(task: *mut Task, addr: u64) -> Option<u64> {
    use crate::arch::mm::{phys_to_virt, PhysAddr};

    // Word must not straddle a page boundary (simplification: callers peek
    // aligned instruction/data words).
    if addr & 0o7 != 0 {
        return None;
    }
    let page_fault_addr = addr & !0xFFFu64;

    // task is pinned by the caller (sys_ptrace) for the call.
    {
        let mm = crate::process::unsafe_edges::borrow_task(task).address_space_arc()?;

        if crate::process::unsafe_edges::ptrace_translate(&mm, page_fault_addr).is_none() {
            // Fault the page in through the target's mm (file-backed VMAs
            // are filled from their pinned File, anonymous ones zeroed).
            let r = crate::arch::mm::handle_mm_fault(
                &mm,
                crate::arch::mm::VirtAddr::new(page_fault_addr),
                crate::arch::mm::FaultFlags::READ,
            );
            if !matches!(
                r,
                crate::arch::mm::MmFaultResult::Handled
            ) {
                return None;
            }
        }

        let (phys, page_len) = crate::process::unsafe_edges::ptrace_translate(&mm, addr)?;
        let off = (addr & (page_len - 1)) as usize;
        if off + 8 > page_len as usize {
            return None;
        }
        // Kernel linear map of a user page of the target
        // (INV-PROC-PTRACE-MEM-1).
        Some(crate::process::unsafe_edges::ptrace_read_word(
            phys_to_virt(PhysAddr::new(phys + off as u64)).0 as usize,
        ))
    }
}

/// Write one word into target user memory at `addr`.
///
/// The write goes through the kernel linear mapping, so it bypasses the
/// target PTE write permission (this is exactly what a debugger needs to
/// plant breakpoints in R+X text). Flushes the D-cache-coherent TLB/I-cache
/// afterwards so the tracee sees the new bytes.
pub fn write_target_word(task: *mut Task, addr: u64, value: u64) -> bool {
    use crate::arch::mm::{phys_to_virt, PhysAddr};

    if addr & 0o7 != 0 {
        return false;
    }
    let page_fault_addr = addr & !0xFFFu64;

    // task is pinned by the caller (sys_ptrace) for the call.
    {
        let mm = match crate::process::unsafe_edges::borrow_task(task).address_space_arc() {
            Some(m) => m,
            None => return false,
        };

        if crate::process::unsafe_edges::ptrace_translate(&mm, page_fault_addr).is_none() {
            let r = crate::arch::mm::handle_mm_fault(
                &mm,
                crate::arch::mm::VirtAddr::new(page_fault_addr),
                crate::arch::mm::FaultFlags::READ,
            );
            if !matches!(r, crate::arch::mm::MmFaultResult::Handled) {
                return false;
            }
        }

        let (phys, page_len) = match crate::process::unsafe_edges::ptrace_translate(&mm, addr) {
            Some(v) => v,
            None => return false,
        };
        let off = (addr & (page_len - 1)) as usize;
        if off + 8 > page_len as usize {
            return false;
        }
        // Kernel linear map of a user page of the target
        // (INV-PROC-PTRACE-MEM-1).
        crate::process::unsafe_edges::ptrace_write_word(
            phys_to_virt(PhysAddr::new(phys + off as u64)).0 as usize,
            value,
        );
        // Coherent ordering: make the store visible before any fetch on
        // any hart (text writes — POKETEXT breakpoints).
        crate::process::unsafe_edges::ptrace_flush_icache(addr);
        true
    }
}

/// process_vm_readv/writev core: copy `len` bytes between a KERNEL buffer
/// `kbuf` and the TARGET task's user address `remote` (`to_remote` picks
/// the direction). Returns the number of bytes moved for this call; a
/// short count means the first unmapped/protected page stopped the copy.
/// Err(EFAULT) when nothing at all could be moved (bad start address,
/// PROT_NONE VMA, or a fault-in that did not resolve).
///
/// Shares the PEEK/POKE machinery: translate() walks the target's page
/// table, absent pages fault in through the target's own mm (demand-paged
/// text works), and the data path is the kernel linear map. The VMA
/// permission gate runs FIRST for every page: a PROT_NONE mapping keeps
/// a present V-only PTE (see handle_mm_fault), so the page walk alone
/// would happily copy through it.
pub fn process_vm_copy(
    task: *mut Task,
    remote: u64,
    kbuf: *mut u8,
    len: usize,
    to_remote: bool,
) -> Result<usize, i32> {
    use crate::arch::mm::{phys_to_virt, PhysAddr};

    if len == 0 {
        return Ok(0);
    }
    // task is pinned by the caller for the whole call; the mm Arc keeps
    // the address space alive even if the target exits mid-copy.
    let mm = match crate::process::unsafe_edges::borrow_task(task).address_space_arc() {
        Some(m) => m,
        None => return Err(14), // EFAULT: kernel-thread style target
    };

    let mut done = 0usize;
    let mut r = remote;
    let mut k = kbuf;
    let mut remaining = len;
    while remaining > 0 {
        // VMA gate: the covering mapping must permit this direction.
        let vma = mm.find_vma(crate::mm::page::VirtAddr::new(r as usize));
        let permitted = match vma.as_ref() {
            Some(v) => {
                if to_remote {
                    v.flags().is_writable()
                } else {
                    v.flags().is_readable()
                }
            }
            None => false,
        };
        if !permitted {
            break;
        }
        let page = r & !0xFFFu64;
        // SAFETY: translate only reads the target's page tables.
        if crate::process::unsafe_edges::ptrace_translate(&mm, page).is_none() {
            let fl = if to_remote {
                crate::arch::mm::FaultFlags::WRITE
            } else {
                crate::arch::mm::FaultFlags::READ
            };
            let res = crate::arch::mm::handle_mm_fault(
                &mm,
                crate::arch::mm::VirtAddr::new(page),
                fl,
            );
            if !matches!(res, crate::arch::mm::MmFaultResult::Handled) {
                break;
            }
        }
        // SAFETY: translate only reads the target's page tables.
        let (phys, page_len) = match crate::process::unsafe_edges::ptrace_translate(&mm, r) {
            Some(v) => v,
            None => break,
        };
        let off = (r & (page_len - 1)) as usize;
        let chunk = core::cmp::min(remaining, page_len as usize - off);
        // dst points into the kernel linear map of the target's user
        // page; kbuf is a kernel buffer supplied by the caller
        // (INV-PROC-PTRACE-MEM-1).
        let lin = phys_to_virt(PhysAddr::new(phys + off as u64)).0 as *mut u8;
        crate::process::unsafe_edges::ptrace_copy_chunk(lin, k, chunk, to_remote);
        r += chunk as u64;
        k = k.wrapping_add(chunk);
        done += chunk;
        remaining -= chunk;
    }
    if done == 0 {
        return Err(14); // EFAULT
    }
    Ok(done)
}

// ==================== gregset access ====================

// ==================== stop / notify machinery ====================

/// Stop a traced task for its tracer: enter STOPPED, record the stop
/// signal + siginfo, wake the tracer with SIGCHLD.
///
/// Called from the tracee's own context (do_signal / trap handler), so the
/// trap frame is quiescent once the task schedules away. The stop PARKS
/// the task (schedule loop) until the tracer resumes it (PTRACE_CONT /
/// DETACH) or a signal arrives — Linux ptrace_stop() discipline.
///
/// `task` is the current (about to stop) task.
pub fn ptrace_stop(task: *mut Task, sig: i32, info: SigInfo) {
    use crate::process::task::TaskState;

    // task is current (INV-PROC-BORROW-1's current-task arm).
    {
        let t = crate::process::unsafe_edges::borrow_task_mut(task);
        t.set_ptrace_siginfo(info);
        t.set_stop_signal(sig);
        t.stop_reported().store(false, core::sync::atomic::Ordering::Release);
        t.set_state(TaskState::new(TaskState::STOPPED));

        let tracer_pid = t.tracer_pid();
        if tracer_pid != 0 {
            // Tracer visibility. SIGCHLD alone is NOT reliable: its default
            // disposition is SIG_IGN (send_signal discards it for tracers
            // without a handler), so the waitqueue wake below is the real
            // notifier — it unblocks a tracer parked in wait4/waitid
            // exactly like the exit path's wake_group_chldexit.
            let _ = crate::signal::send_signal(tracer_pid, Signal::SIGCHLD as i32);
            if let Some(tr) = crate::process::task_ref::TaskRef::lookup(tracer_pid) {
                crate::process::exit::wake_group_chldexit(&tr);
                crate::signal::signal_wake_up(tr.as_ptr());
            }
        }
        // Park in the stop until resumed. The old code only set
        // need_resched and relied on the (disabled) trap-exit preemption
        // check, so a traced task never actually stopped.
        crate::signal::park_stopped_task(task);
    }
}

// ==================== syscall entry ====================

/// sys_ptrace — main dispatcher.
pub fn sys_ptrace(args: SyscallArgs) -> i64 {
    let request = args[0] as u32;
    let pid = args[1] as u32;
    let addr = args[2];
    let data = args[3];

    // SAFETY: sched::current() returns the running task or None.
    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => return -ESRCH,
    };
    // SAFETY: dereferenced below only after null check.
    let my_pid = crate::process::unsafe_edges::borrow_task(current as *mut Task).pid();

    // ---- PTRACE_TRACEME: no target lookup ----
    if request == PTRACE_TRACEME {
        if pid != 0 {
            return -EINVAL;
        }
        // Linux: a task that is already being traced may not TRACEME again
        // (LTP ptrace03 "process which is already been traced" expects EPERM).
        // SAFETY: tracer_pid is an atomic field.
        if crate::process::unsafe_edges::borrow_task(current as *mut Task).tracer_pid() != 0 {
            return -EPERM;
        }
        // Future stops of THIS task are reported to its real parent.
        // SAFETY: parent_ptr returns the real parent or None.
        let ppid = crate::process::unsafe_edges::borrow_task(current as *mut Task).parent_ptr().map(|p| unsafe { (*p).pid() }).unwrap_or(0);
        if ppid == 0 {
            return -ESRCH;
        }
        // SAFETY: tracer_pid is an atomic field.
        crate::process::unsafe_edges::borrow_task(current as *mut Task).set_tracer_pid(ppid);
        return 0;
    }

    // ---- all other requests operate on a pinned target ----
    // SAFETY: pinned lookup keeps the Task alive until task_put below.
    let target = crate::process::pid_hash::pid_hash_lookup_pinned(pid);
    if target.is_null() {
        return -ESRCH;
    }
    let ret = dispatch_request(current, my_pid, target, request, addr, data);
    // SAFETY: release the pin taken above.
    crate::process::task::Task::task_put(target);
    ret
}

fn dispatch_request(
    _current: *mut Task,
    my_pid: u32,
    target: *mut Task,
    request: u32,
    addr: u64,
    data: u64,
) -> i64 {
    use crate::arch::uaccess_safe as ua;
    use crate::arch::uaccess::access_ok;

    // SAFETY: target is pinned for the whole call.
    let is_stopped = crate::process::unsafe_edges::borrow_task(target).state().contains(crate::process::task::TaskState::STOPPED);

    if request == PTRACE_ATTACH {
        if target == _current {
            return -EPERM;
        }
        // SAFETY: cred is immutable.
        let cred = crate::process::unsafe_edges::borrow_task(target).cred().clone();
        if !ptrace_may_access(&cred) {
            return -EPERM;
        }
        let cur_tracer = crate::process::unsafe_edges::borrow_task(target).tracer_pid();
        if cur_tracer != 0 && cur_tracer != my_pid {
            return -EPERM; // already traced by someone else
        }
        // SAFETY: atomics.
        crate::process::unsafe_edges::borrow_task(target).set_tracer_pid(my_pid);
        // Stop the tracee: queue SIGSTOP — do_signal() sees tracer_pid set
        // and routes it to ptrace_stop instead of the default stop.
        // SAFETY: pending set is lock-protected.
        crate::process::unsafe_edges::borrow_task(target).pending.add(Signal::SIGSTOP as i32);
        crate::signal::signal_wake_up(target);
        return 0;
    }

    // Everything else requires the caller to be the configured tracer.
    if crate::process::unsafe_edges::borrow_task(target).tracer_pid() != my_pid {
        return -ESRCH;
    }

    match request {
        PTRACE_DETACH => {
            // Resume and forget the tracing relationship.
            let sig = data as i32;
            if sig >= 1 && sig <= 64 && sig != Signal::SIGKILL as i32 {
                // SAFETY: pending set is lock-protected.
                crate::process::unsafe_edges::borrow_task(target).pending.add(sig);
            }
            {
                let t = crate::process::unsafe_edges::borrow_task(target);
                t.set_tracer_pid(0);
                t.set_ptrace_options(0);
                t.clear_single_step();
            }
            if is_stopped {
                crate::signal::signal_wake_up(target);
            }
            0
        }

        PTRACE_CONT | PTRACE_SINGLESTEP => {
            if !is_stopped {
                return -ESRCH;
            }
            // Re-inject the stashed signal when data names one.
            let sig = data as i32;
            if sig >= 1 && sig <= 64 {
                // SAFETY: pending set is lock-protected.
                crate::process::unsafe_edges::borrow_task(target).pending.add(sig);
                // Mark it for direct delivery so do_signal does not
                // re-intercept it into another trace stop.
                // SAFETY: atomics.
                crate::process::unsafe_edges::borrow_task(target).set_ptrace_sigdeliver(sig as u32);
            }
            // Clear the stashed stop info: it has been consumed.
            // SAFETY: lock-protected field.
            crate::process::unsafe_edges::borrow_task(target).clear_ptrace_siginfo();

            if request == PTRACE_SINGLESTEP {
                // Displace the instruction at epc with EBREAK; the trap
                // handler reports SIGTRAP and restores it.
                // Pinned + stopped target has a quiescent frame
                // (INV-PROC-FRAME-1's read arm).
                let regs = crate::process::unsafe_edges::borrow_task(target).pt_regs();
                if regs.is_null() {
                    return -EIO;
                }
                {
                    let epc = crate::process::unsafe_edges::frame_ref(regs).instruction_pointer();
                    match read_target_word(target, epc) {
                        Some(word) => {
                            crate::process::unsafe_edges::borrow_task(target).arm_single_step(epc, word);
                            if !write_target_word(target, epc, EBREAK_INSN) {
                                crate::process::unsafe_edges::borrow_task(target).clear_single_step();
                                return -EIO;
                            }
                        }
                        None => return -EIO,
                    }
                }
            }
            crate::signal::signal_wake_up(target);
            0
        }

        PTRACE_KILL => {
            // Historical alias: deliver a SIGKILL to the tracee.
            // SAFETY: pending set is lock-protected.
            crate::process::unsafe_edges::borrow_task(target).pending.add(Signal::SIGKILL as i32);
            crate::signal::signal_wake_up(target);
            0
        }

        PTRACE_PEEKTEXT | PTRACE_PEEKDATA => {
            if !is_stopped {
                return -ESRCH;
            }
            match read_target_word(target, addr) {
                Some(word) => {
                    if data != 0 && access_ok(data as usize, 8) {
                        // access_ok-validated; write_user is the
                        // exception-table copy path.
                        let _ = ua::write_user(data as usize, word);
                    }
                    word as i64
                }
                None => -EIO,
            }
        }

        PTRACE_POKETEXT | PTRACE_POKEDATA => {
            if !is_stopped {
                return -ESRCH;
            }
            if write_target_word(target, addr, data) {
                0
            } else {
                -EIO
            }
        }

        PTRACE_PEEKUSR => {
            if !is_stopped {
                return -ESRCH;
            }
            if addr % 8 != 0 || addr as usize >= GREGSET_BYTES {
                return -EIO;
            }
            // Pinned + stopped; frame quiescent
            // (INV-PROC-PTRACE-GREG-1/MEM-1).
            let regs = crate::process::unsafe_edges::borrow_task(target).pt_regs();
            if regs.is_null() {
                return -EIO;
            }
            let word = crate::process::unsafe_edges::ptrace_read_word(
                regs as usize + addr as usize,
            );
            if data != 0 && access_ok(data as usize, 8) {
                let _ = ua::write_user(data as usize, word);
            }
            word as i64
        }

        PTRACE_POKEUSR => {
            if !is_stopped {
                return -ESRCH;
            }
            if addr % 8 != 0 || addr as usize >= GREGSET_BYTES {
                return -EIO;
            }
            // Pinned + stopped; frame quiescent (INV-PROC-PTRACE-GREG-1).
            let regs = crate::process::unsafe_edges::borrow_task(target).pt_regs();
            if regs.is_null() {
                return -EIO;
            }
            crate::process::unsafe_edges::ptrace_write_word(
                regs as usize + addr as usize,
                data,
            );
            crate::arch::cpu::isb();
            0
        }

        PTRACE_GETREGS | PTRACE_GETREGSET => {
            if !is_stopped {
                return -ESRCH;
            }
            let mut buf = [0u8; GREGSET_BYTES];
            // SAFETY: pinned + stopped.
            if !crate::process::unsafe_edges::ptrace_read_gregset(crate::process::unsafe_edges::borrow_task(target).pt_regs(), &mut buf) {
                return -EIO;
            }
            if request == PTRACE_GETREGS {
                // data points at a 256-byte user_regs_struct.
                if data == 0 || !access_ok(data as usize, GREGSET_BYTES) {
                    return -EINVAL;
                }
                // access_ok-validated; exception-table copy.
                if ua::copy_out(data as usize, &buf[..GREGSET_BYTES]) != 0 {
                    return -EIO;
                }
                0
            } else {
                // GETREGSET: addr = NT_* type (only NT_PRSTATUS here),
                // data = struct iovec { base, len } in user memory.
                if addr != NT_PRSTATUS as u64 {
                    return -EINVAL;
                }
                if data == 0 || !access_ok(data as usize, 16) {
                    return -EINVAL;
                }
                // Validated iovec reads via read_user.
                let base = ua::read_user::<u64>(data as usize).unwrap_or(0);
                let len = ua::read_user::<u64>(data as usize + 8).unwrap_or(0);
                if base == 0 || len == 0 || !access_ok(base as usize, GREGSET_BYTES) {
                    return -EINVAL;
                }
                if ua::copy_out(base as usize, &buf[..GREGSET_BYTES]) != 0 {
                    return -EIO;
                }
                // Report the actual copied length (256).
                let _ = ua::write_user(data as usize + 8, GREGSET_BYTES as u64);
                0
            }
        }

        PTRACE_SETREGS | PTRACE_SETREGSET => {
            if !is_stopped {
                return -ESRCH;
            }
            let mut buf = [0u8; GREGSET_BYTES];
            let (src, len) = if request == PTRACE_SETREGS {
                if data == 0 || !access_ok(data as usize, GREGSET_BYTES) {
                    return -EINVAL;
                }
                (data, GREGSET_BYTES as u64)
            } else {
                if addr != NT_PRSTATUS as u64 {
                    return -EINVAL;
                }
                if data == 0 || !access_ok(data as usize, 16) {
                    return -EINVAL;
                }
                // Validated iovec reads via read_user.
                let base = ua::read_user::<u64>(data as usize).unwrap_or(0);
                let len = ua::read_user::<u64>(data as usize + 8).unwrap_or(0);
                if base == 0 || len == 0 {
                    return -EINVAL;
                }
                (base, len)
            };
            let n = (len as usize).min(GREGSET_BYTES);
            if !access_ok(src as usize, n) {
                return -EINVAL;
            }
            // access_ok-validated; exception-table copy.
            if ua::copy_in(&mut buf[..n], src as usize) != 0 {
                return -EIO;
            }
            if !crate::process::unsafe_edges::ptrace_write_gregset(
                crate::process::unsafe_edges::borrow_task(target).pt_regs(),
                &buf[..n],
            ) {
                return -EIO;
            }
            0
        }

        PTRACE_GETSIGINFO => {
            // data points at a 128-byte siginfo_t.
            if data == 0 || !access_ok(data as usize, core::mem::size_of::<SigInfo>()) {
                return -EINVAL;
            }
            // SAFETY: lock-protected field read.
            let info = crate::process::unsafe_edges::borrow_task(target).ptrace_siginfo();
            match info {
                Some(i) => {
                    // Validated; exception-table copy (bytes_of owns the
                    // value-to-bytes view).
                    if ua::copy_out(
                        data as usize,
                        crate::safety::bytes::bytes_of(&i),
                    ) != 0 {
                        return -EIO;
                    }
                    0
                }
                None => -EINVAL,
            }
        }

        PTRACE_SETSIGINFO => {
            if data == 0 || !access_ok(data as usize, core::mem::size_of::<SigInfo>()) {
                return -EINVAL;
            }
            // Validated read into a local zeroed siginfo (copy_in_t
            // owns the typed user read).
            let mut info: SigInfo = crate::process::unsafe_edges::zeroed_siginfo();
            if ua::copy_in_t(&mut info, data as usize) != 0 {
                return -EIO;
            }
            crate::process::unsafe_edges::borrow_task(target).set_ptrace_siginfo(info);
            0
        }

        PTRACE_SETOPTIONS => {
            // Options are recorded (minimal): no option changes behavior
            // yet beyond O_EXITKILL-style bookkeeping.
            // SAFETY: atomic field.
            crate::process::unsafe_edges::borrow_task(target).set_ptrace_options(data);
            0
        }

        // PTRACE_SEIZE / LISTEN / INTERRUPT / GETFPREGS / syscall tracing:
        // not implemented — report EIO like Linux does for unsupported
        // requests on this architecture.
        _ => -EIO,
    }
}
