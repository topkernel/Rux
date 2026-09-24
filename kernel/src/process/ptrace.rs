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
fn ptrace_may_access(target_cred: &crate::process::task::Cred) -> bool {
    if let Some(cur) = crate::sched::current() {
        // SAFETY: current is the running task; cred() is immutable.
        let cred = unsafe { (*cur).cred() };
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

/// Translate a user VA in the target's address space to a physical address
/// plus the leaf page size (SV39 walk, 4 KiB / 2 MiB / 1 GiB leaves).
///
/// # Safety
/// `root_ppn` must be a valid page-table root PPN.
unsafe fn translate(root_ppn: u64, va: u64) -> Option<(u64, u64)> {
    use crate::arch::riscv64::mm::{phys_to_virt, PhysAddr};

    let a2 = (root_ppn << 12) + ((va >> 30) & 0x1FF) * 8;
    // SAFETY: a2 addresses a valid PTE in the target's level-2 table.
    let mut pte = unsafe {
        core::ptr::read_volatile(phys_to_virt(PhysAddr::new(a2)).0 as *const u64)
    };
    if pte & 1 == 0 {
        return None;
    }
    let mut level = 2u32;
    loop {
        let is_leaf = pte & 0xE != 0; // R|W|X
        if is_leaf || level == 0 {
            break;
        }
        let shift = 12 + 9 * (level - 1);
        let next_table = ((pte >> 10) & 0xFFF_FFFF_FFFF) << 12;
        let idx = (va >> shift) & 0x1FF;
        // SAFETY: next_table is a valid page-table page in the target mm.
        pte = unsafe {
            core::ptr::read_volatile(
                phys_to_virt(PhysAddr::new(next_table + idx * 8)).0 as *const u64,
            )
        };
        if pte & 1 == 0 {
            return None;
        }
        level -= 1;
    }

    let (page_phys, page_len) = if level == 0 {
        (((pte >> 10) & 0xFFF_FFFF_FFFF) << 12, 4096u64)
    } else {
        let shift = 12 + 9 * level;
        let mask: u64 = !((1u64 << shift) - 1);
        ((((pte >> 10) & 0xFFF_FFFF_FFFF) << 12) & mask, 1u64 << shift)
    };
    Some((page_phys, page_len))
}

/// Read one word of target user memory at `addr`.
///
/// If the page is not present, a read fault is driven on the TARGET's mm
/// first (this is what makes PEEKTEXT work on demand-paged, not-yet-loaded
/// text). Returns None when the address is unmapped/unreadable.
pub fn read_target_word(task: *mut Task, addr: u64) -> Option<u64> {
    use crate::arch::riscv64::mm::{phys_to_virt, PhysAddr};

    // Word must not straddle a page boundary (simplification: callers peek
    // aligned instruction/data words).
    if addr & 0o7 != 0 {
        return None;
    }
    let page_fault_addr = addr & !0xFFFu64;

    // SAFETY: task is pinned by the caller (sys_ptrace) for the call.
    unsafe {
        let mm = (*task).address_space_arc()?;
        let root_ppn = mm.root_ppn();

        if translate(root_ppn, page_fault_addr).is_none() {
            // Fault the page in through the target's mm (file-backed VMAs
            // are filled from their pinned File, anonymous ones zeroed).
            let r = crate::arch::riscv64::mm::handle_mm_fault(
                &mm,
                crate::arch::riscv64::mm::VirtAddr::new(page_fault_addr),
                crate::arch::riscv64::mm::FaultFlags::READ,
            );
            if !matches!(
                r,
                crate::arch::riscv64::mm::MmFaultResult::Handled
            ) {
                return None;
            }
        }

        let (phys, page_len) = translate(root_ppn, addr)?;
        let off = (addr & (page_len - 1)) as usize;
        if off + 8 > page_len as usize {
            return None;
        }
        // SAFETY: kernel linear map of a user page of the target.
        Some(unsafe {
            core::ptr::read_volatile(
                phys_to_virt(PhysAddr::new(phys + off as u64)).0 as *const u64
            )
        })
    }
}

/// Write one word into target user memory at `addr`.
///
/// The write goes through the kernel linear mapping, so it bypasses the
/// target PTE write permission (this is exactly what a debugger needs to
/// plant breakpoints in R+X text). Flushes the D-cache-coherent TLB/I-cache
/// afterwards so the tracee sees the new bytes.
pub fn write_target_word(task: *mut Task, addr: u64, value: u64) -> bool {
    use crate::arch::riscv64::mm::{phys_to_virt, PhysAddr};

    if addr & 0o7 != 0 {
        return false;
    }
    let page_fault_addr = addr & !0xFFFu64;

    // SAFETY: task is pinned by the caller (sys_ptrace) for the call.
    unsafe {
        let mm = match (*task).address_space_arc() {
            Some(m) => m,
            None => return false,
        };
        let root_ppn = mm.root_ppn();

        if translate(root_ppn, page_fault_addr).is_none() {
            let r = crate::arch::riscv64::mm::handle_mm_fault(
                &mm,
                crate::arch::riscv64::mm::VirtAddr::new(page_fault_addr),
                crate::arch::riscv64::mm::FaultFlags::READ,
            );
            if !matches!(r, crate::arch::riscv64::mm::MmFaultResult::Handled) {
                return false;
            }
        }

        let (phys, page_len) = match translate(root_ppn, addr) {
            Some(v) => v,
            None => return false,
        };
        let off = (addr & (page_len - 1)) as usize;
        if off + 8 > page_len as usize {
            return false;
        }
        // SAFETY: kernel linear map of a user page of the target.
        unsafe {
            core::ptr::write_volatile(
                phys_to_virt(PhysAddr::new(phys + off as u64)).0 as *mut u64,
                value,
            );
        }
        // Coherent ordering: make the store visible before any fetch on
        // any hart. sfence.vma covers the store buffer; fence.i the I-side
        // for text writes (POKETEXT breakpoints).
        unsafe {
            core::arch::asm!(
                "fence rw, rw",
                "sfence.vma {0}, zero",
                "fence.i",
                in(reg) addr,
                options(nostack, preserves_flags)
            );
        }
        true
    }
}

// ==================== gregset access ====================

/// Read the tracee's NT_PRSTATUS gregset (32 × u64) from its saved trap
/// frame. Returns None when the task has no kernel stack / trap frame.
///
/// # Safety
/// `task` must be pinned and stopped (its trap frame quiescent).
unsafe fn read_gregset(task: *mut Task, out: &mut [u8; GREGSET_BYTES]) -> bool {
    let regs = unsafe { (*task).pt_regs() };
    if regs.is_null() {
        return false;
    }
    // PtRegs stores epc, ra, sp, gp, tp, t0..t6, s0..s11, a0..a7 as its
    // first 32 u64 words — exactly the RISC-V user_regs_struct layout.
    // SAFETY: regs is the task's live trap frame; the first 256 bytes are
    // the 32 general-purpose words.
    unsafe {
        let src = regs as *const u8;
        core::ptr::copy_nonoverlapping(src, out.as_mut_ptr(), GREGSET_BYTES);
    }
    true
}

/// Write the tracee's NT_PRSTATUS gregset back into its trap frame.
///
/// # Safety
/// `task` must be pinned and stopped.
unsafe fn write_gregset(task: *mut Task, from: &[u8]) -> bool {
    let regs = unsafe { (*task).pt_regs() };
    if regs.is_null() {
        return false;
    }
    let n = from.len().min(GREGSET_BYTES);
    // SAFETY: regs is the task's live trap frame; only GP words written.
    unsafe {
        core::ptr::copy_nonoverlapping(from.as_ptr(), regs as *mut u8, n);
    }
    true
}

// ==================== stop / notify machinery ====================

/// Stop a traced task for its tracer: enter STOPPED, record the stop
/// signal + siginfo, wake the tracer with SIGCHLD.
///
/// Called from the tracee's own context (do_signal / trap handler), so the
/// trap frame is quiescent once the task schedules away.
///
/// # Safety
/// `task` is the current (about to stop) task.
pub unsafe fn ptrace_stop(task: *mut Task, sig: i32, info: SigInfo) {
    use crate::process::task::TaskState;

    // SAFETY: task is current.
    unsafe {
        (*task).set_ptrace_siginfo(info);
        (*task).set_stop_signal(sig);
        (*task).stop_reported().store(false, core::sync::atomic::Ordering::Release);
        (*task).set_state(TaskState::new(TaskState::STOPPED));

        let tracer_pid = (*task).tracer_pid();
        if tracer_pid != 0 {
            // Tracer visibility: SIGCHLD wakes an interruptible wait4.
            let _ = crate::signal::send_signal(tracer_pid, Signal::SIGCHLD as i32);
            // SIGCHLD may be blocked by the tracer — wake it regardless so
            // its wait4 loop re-scans children.
            let tr = crate::process::pid_hash::pid_hash_lookup_pinned(tracer_pid);
            if !tr.is_null() {
                crate::signal::signal_wake_up(tr);
                crate::process::task::Task::task_put(tr);
            }
        }
        crate::sched::set_need_resched();
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
    let my_pid = unsafe { (*current).pid() };

    // ---- PTRACE_TRACEME: no target lookup ----
    if request == PTRACE_TRACEME {
        if pid != 0 {
            return -EINVAL;
        }
        // Future stops of THIS task are reported to its real parent.
        // SAFETY: parent_ptr returns the real parent or None.
        let ppid = unsafe { (*current).parent_ptr() }.map(|p| unsafe { (*p).pid() }).unwrap_or(0);
        if ppid == 0 {
            return -ESRCH;
        }
        // SAFETY: tracer_pid is an atomic field.
        unsafe { (*current).set_tracer_pid(ppid) };
        return 0;
    }

    // ---- all other requests operate on a pinned target ----
    // SAFETY: pinned lookup keeps the Task alive until task_put below.
    let target = unsafe { crate::process::pid_hash::pid_hash_lookup_pinned(pid) };
    if target.is_null() {
        return -ESRCH;
    }
    let ret = dispatch_request(current, my_pid, target, request, addr, data);
    // SAFETY: release the pin taken above.
    unsafe { crate::process::task::Task::task_put(target) };
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
    use crate::arch::riscv64::uaccess::{access_ok, copy_from_user, copy_to_user, get_user, put_user};

    // SAFETY: target is pinned for the whole call.
    let is_stopped = unsafe { (*target).state().contains(crate::process::task::TaskState::STOPPED) };

    if request == PTRACE_ATTACH {
        if target == _current {
            return -EPERM;
        }
        // SAFETY: cred is immutable.
        let cred = unsafe { (*target).cred().clone() };
        if !ptrace_may_access(&cred) {
            return -EPERM;
        }
        let cur_tracer = unsafe { (*target).tracer_pid() };
        if cur_tracer != 0 && cur_tracer != my_pid {
            return -EPERM; // already traced by someone else
        }
        // SAFETY: atomics.
        unsafe { (*target).set_tracer_pid(my_pid) };
        // Stop the tracee: queue SIGSTOP — do_signal() sees tracer_pid set
        // and routes it to ptrace_stop instead of the default stop.
        // SAFETY: pending set is lock-protected.
        unsafe { (*target).pending.add(Signal::SIGSTOP as i32) };
        crate::signal::signal_wake_up(target);
        return 0;
    }

    // Everything else requires the caller to be the configured tracer.
    if unsafe { (*target).tracer_pid() } != my_pid {
        return -ESRCH;
    }

    match request {
        PTRACE_DETACH => {
            // Resume and forget the tracing relationship.
            let sig = data as i32;
            if sig >= 1 && sig <= 64 && sig != Signal::SIGKILL as i32 {
                // SAFETY: pending set is lock-protected.
                unsafe { (*target).pending.add(sig) };
            }
            // SAFETY: atomics.
            unsafe {
                (*target).set_tracer_pid(0);
                (*target).set_ptrace_options(0);
                (*target).clear_single_step();
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
                unsafe { (*target).pending.add(sig) };
                // Mark it for direct delivery so do_signal does not
                // re-intercept it into another trace stop.
                // SAFETY: atomics.
                unsafe { (*target).set_ptrace_sigdeliver(sig as u32) };
            }
            // Clear the stashed stop info: it has been consumed.
            // SAFETY: lock-protected field.
            unsafe { (*target).clear_ptrace_siginfo() };

            if request == PTRACE_SINGLESTEP {
                // Displace the instruction at epc with EBREAK; the trap
                // handler reports SIGTRAP and restores it.
                // SAFETY: pinned + stopped target has a quiescent frame.
                unsafe {
                    let regs = (*target).pt_regs();
                    if regs.is_null() {
                        return -EIO;
                    }
                    let epc = (*regs).epc;
                    match read_target_word(target, epc) {
                        Some(word) => {
                            (*target).arm_single_step(epc, word);
                            if !write_target_word(target, epc, EBREAK_INSN) {
                                (*target).clear_single_step();
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
            unsafe { (*target).pending.add(Signal::SIGKILL as i32) };
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
                        // SAFETY: access_ok-validated pointer; put_user is
                        // the exception-table copy path.
                        unsafe { let _ = put_user(data as *mut u64, word); }
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
            // SAFETY: pinned + stopped; frame quiescent.
            unsafe {
                let regs = (*target).pt_regs();
                if regs.is_null() {
                    return -EIO;
                }
                let word =
                    core::ptr::read_volatile((regs as *const u64).add((addr / 8) as usize));
                if data != 0 && access_ok(data as usize, 8) {
                    let _ = put_user(data as *mut u64, word);
                }
                word as i64
            }
        }

        PTRACE_POKEUSR => {
            if !is_stopped {
                return -ESRCH;
            }
            if addr % 8 != 0 || addr as usize >= GREGSET_BYTES {
                return -EIO;
            }
            // SAFETY: pinned + stopped; frame quiescent.
            unsafe {
                let regs = (*target).pt_regs();
                if regs.is_null() {
                    return -EIO;
                }
                core::ptr::write_volatile(
                    (regs as *mut u64).add((addr / 8) as usize),
                    data,
                );
                core::arch::asm!("fence.i", options(nostack, preserves_flags));
            }
            0
        }

        PTRACE_GETREGS | PTRACE_GETREGSET => {
            if !is_stopped {
                return -ESRCH;
            }
            let mut buf = [0u8; GREGSET_BYTES];
            // SAFETY: pinned + stopped.
            if !unsafe { read_gregset(target, &mut buf) } {
                return -EIO;
            }
            if request == PTRACE_GETREGS {
                // data points at a 256-byte user_regs_struct.
                if data == 0 || !access_ok(data as usize, GREGSET_BYTES) {
                    return -EINVAL;
                }
                // SAFETY: access_ok-validated; exception-table copy.
                unsafe {
                    let uncopied =
                        copy_to_user(data as *mut u8, buf.as_ptr(), GREGSET_BYTES);
                    if uncopied != 0 {
                        return -EIO;
                    }
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
                // SAFETY: validated iovec reads via get_user.
                unsafe {
                    let base = get_user(data as *const u64).unwrap_or(0);
                    let len = get_user((data as *const u64).add(1)).unwrap_or(0);
                    if base == 0 || len == 0 || !access_ok(base as usize, GREGSET_BYTES) {
                        return -EINVAL;
                    }
                    let uncopied =
                        copy_to_user(base as *mut u8, buf.as_ptr(), GREGSET_BYTES);
                    if uncopied != 0 {
                        return -EIO;
                    }
                    // Report the actual copied length (256).
                    let _ = put_user((data as *mut u64).add(1), GREGSET_BYTES as u64);
                }
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
                // SAFETY: validated iovec reads via get_user.
                unsafe {
                    let base = get_user(data as *const u64).unwrap_or(0);
                    let len = get_user((data as *const u64).add(1)).unwrap_or(0);
                    if base == 0 || len == 0 {
                        return -EINVAL;
                    }
                    (base, len)
                }
            };
            let n = (len as usize).min(GREGSET_BYTES);
            if !access_ok(src as usize, n) {
                return -EINVAL;
            }
            // SAFETY: access_ok-validated; exception-table copy.
            unsafe {
                let uncopied = copy_from_user(buf.as_mut_ptr(), src as *const u8, n);
                if uncopied != 0 {
                    return -EIO;
                }
                if !write_gregset(target, &buf[..n]) {
                    return -EIO;
                }
            }
            0
        }

        PTRACE_GETSIGINFO => {
            // data points at a 128-byte siginfo_t.
            if data == 0 || !access_ok(data as usize, core::mem::size_of::<SigInfo>()) {
                return -EINVAL;
            }
            // SAFETY: lock-protected field read.
            let info = unsafe { (*target).ptrace_siginfo() };
            match info {
                Some(i) => {
                    // SAFETY: validated; exception-table copy.
                    unsafe {
                        let uncopied = copy_to_user(
                            data as *mut u8,
                            &i as *const SigInfo as *const u8,
                            core::mem::size_of::<SigInfo>(),
                        );
                        if uncopied != 0 {
                            return -EIO;
                        }
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
            // SAFETY: validated read into a local zeroed siginfo.
            unsafe {
                let mut info: SigInfo = core::mem::zeroed();
                let uncopied = copy_from_user(
                    &mut info as *mut SigInfo as *mut u8,
                    data as *const u8,
                    core::mem::size_of::<SigInfo>(),
                );
                if uncopied != 0 {
                    return -EIO;
                }
                (*target).set_ptrace_siginfo(info);
            }
            0
        }

        PTRACE_SETOPTIONS => {
            // Options are recorded (minimal): no option changes behavior
            // yet beyond O_EXITKILL-style bookkeeping.
            // SAFETY: atomic field.
            unsafe { (*target).set_ptrace_options(data) };
            0
        }

        // PTRACE_SEIZE / LISTEN / INTERRUPT / GETFPREGS / syscall tracing:
        // not implemented — report EIO like Linux does for unsupported
        // requests on this architecture.
        _ => -EIO,
    }
}
