//! The ptrace STOP/WRITE core: the two ptrace entry points the arch
//! trap files call BY NAME (the 1a criterion — a hot site's direct
//! call must stay direct; no hook row sits in front of them). The
//! rest of ptrace (the request dispatcher, PEEK/POKE readers,
//! process_vm_copy) is process service policy reached through the
//! syscall layer; it calls DOWN into these two for the stop/write
//! primitives.

#![forbid(unsafe_code)]

use crate::process::task::Task;
use crate::signal::{SigInfo, Signal};

/// `task` is the current (about to stop) task.
pub fn ptrace_stop(task: *mut Task, sig: i32, info: SigInfo) {
    use crate::process::task::TaskState;

    // task is current (INV-PROC-BORROW-1's current-task arm).
    {
        let t = crate::process::r#unsafe::borrow_task_mut(task);
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
        let mm = match crate::process::r#unsafe::borrow_task(task).address_space_arc() {
            Some(m) => m,
            None => return false,
        };

        if crate::process::r#unsafe::ptrace_translate(&mm, page_fault_addr).is_none() {
            let r = crate::api::mm::handle_mm_fault(
                &mm,
                crate::arch::mm::VirtAddr::new(page_fault_addr),
                crate::arch::mm::FaultFlags::READ,
            );
            if !matches!(r, crate::arch::mm::MmFaultResult::Handled) {
                return false;
            }
        }

        let (phys, page_len) = match crate::process::r#unsafe::ptrace_translate(&mm, addr) {
            Some(v) => v,
            None => return false,
        };
        let off = (addr & (page_len - 1)) as usize;
        if off + 8 > page_len as usize {
            return false;
        }
        // Kernel linear map of a user page of the target
        // (INV-PROC-PTRACE-MEM-1).
        crate::process::r#unsafe::ptrace_write_word(
            phys_to_virt(PhysAddr::new(phys + off as u64)).0 as usize,
            value,
        );
        // Coherent ordering: make the store visible before any fetch on
        // any hart (text writes — POKETEXT breakpoints).
        crate::process::r#unsafe::ptrace_flush_icache(addr);
        true
    }
}
