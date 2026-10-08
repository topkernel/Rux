//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 context switching.
//!
//! # Safety Invariants — Context Switch Atomicity (x86 port of the
//! riscv64 twin's INV-CS-* series)
//!
//! - **INV-CS-1**: `__switch_to` saves/restores the callee-saved GPRs
//!   (rbx, rbp, r12–r15) and rsp; caller-saved registers are already
//!   spilled in `schedule()`'s frame.
//!
//! - **INV-CS-2**: FPU state is saved/restored via `fpu_save_for_switch`
//!   / `restore_fpu` (fxsave64/fxrstor64) around the register switch.
//!
//! - **INV-CS-3**: CR3 is switched in `context_switch` BEFORE
//!   `__switch_to`, so the new task's page table is active when its
//!   registers are restored.  User PML4s share the kernel half
//!   (PML4[256..511] links), so the switch is safe from anywhere.
//!
//! - **INV-CS-4**: `__switch_to` does NOT enable IF.  The caller
//!   (`__schedule`) owns interrupt state via lock_irqsave/restore_irq.
//!
//! - **INV-CS-5**: After `__switch_to`, the previous task's stack is
//!   stale; the current task is only reachable via the per-CPU slot
//!   (`arch::smp::current_task_ptr`), published inside `__switch_to`.
//!
//! Layout note: fork builds child thread.sp = stack_top - 168 (the
//! complete user frame) and thread.callee.ret_addr = ret_from_fork, so
//! the first switch into the child "returns" into ret_from_fork which
//! pops the frame back to user mode.

use crate::process::task::Task;
use super::cpu::{read_cr3, rdmsr, wrmsr, write_cr3};

/// MSR_FS_BASE — per-task TLS pointer.
const MSR_FS_BASE: u32 = 0xC000_0100;
/// MSR_KERNEL_GS_BASE — the SWAPGS shadow (task's user GS base).
const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;

extern "C" {
    fn __switch_to(prev: *mut Task, next: *mut Task);
}

/// Switch the address-space root (interface parity with the riscv64
/// twin's switch_mm; called from exec/exit paths outside context_switch).
///
/// # Safety
/// `next_ppn` must be a live page-table root PPN.  `asid` is ignored —
/// PCID is disabled at bring-up (see mm/asid.rs) and the CR3 reload
/// flushes non-global translations by itself.
pub unsafe fn switch_mm(next_ppn: u64, _asid: u16) {
    // SAFETY: caller guarantees a valid PML4 physical address.
    unsafe { write_cr3(super::mm::asid::build_satp(0, next_ppn)) };
}

// ============================================================================
// __switch_to
// ============================================================================

core::arch::global_asm!(
    r#"
.section .text.__switch_to
.align 8

.global __switch_to
.type __switch_to, @function
__switch_to:
    # rdi = prev task, rsi = next task

    # 1. Save prev's callee-saved registers + sp into prev->thread.
    #    (Everything is spilled through memory, no scratch needed.)
    movq %rbx, {task_thread}+{callee_rbx}(%rdi)
    movq %rbp, {task_thread}+{callee_rbp}(%rdi)
    movq %r12, {task_thread}+{callee_r12}(%rdi)
    movq %r13, {task_thread}+{callee_r13}(%rdi)
    movq %r14, {task_thread}+{callee_r14}(%rdi)
    movq %r15, {task_thread}+{callee_r15}(%rdi)
    # Return-address protocol (x86 keeps the return address on the
    # stack, the twin's ra is a register): stash the live return address
    # into callee.ret_addr and thread.sp = entry_rsp + 8 (the stack
    # position ABOVE the return-address slot).  The resume side's
    # `push callee.ret_addr; ret` then re-enters context_switch with
    # exactly the post-call rsp; for newborn tasks fork sets
    # thread.sp = stack_top - 168 (frame base) and ret_addr =
    # ret_from_fork, so the same push/ret lands on the child frame.
    movq 0(%rsp), %rax
    movq %rax, {task_thread}+{callee_ret}(%rdi)
    leaq 8(%rsp), %rax
    movq %rax, {task_thread}+{thread_sp}(%rdi)

    # 2. prev/next survive the helper calls in callee-saved registers
    #    (safe: their interrupted values were just stored in prev->thread).
    movq %rdi, %rbx            # rbx = prev
    movq %rsi, %r12            # r12 = next

    # ABI: entry rsp == 8 (mod 16); the resume point is already saved, so
    # aligning down here is free scratch space for the C helper calls.
    subq $8, %rsp

    # 3. Publish: current = next (release), claim next for the double-run
    #    detector, and TSS.rsp0 = next's kernel stack top so the next user
    #    entry lands at stack_top - 168.
    movq %rbx, %rdi
    movq %r12, %rsi
    call x86_switch_publish
    movq %r12, %rdi
    call x86_update_rsp0

    # R65 (pickability release point): prev becomes pickable ONLY HERE.
    # The old discipline cleared prev->ti_on_cpu/running_on_cpu at the TOP
    # of x86_switch_publish, while this CPU still had its whole switch
    # chain (publish -> update_rsp0 -> diagnostics) living on PREV's
    # kernel stack — and publish does real work after the clear (claim
    # swap, the FOREIGN-SP report, a 40-entry ring dump, quiesce). A
    # second CPU could legally pick prev the instant the bits read clear,
    # resume it, and the resumed task's very next trap/frame would be
    # pushed onto the frames the switching CPU was still executing in —
    # torn return addresses, a switcher returning through foreign slots
    # (the wild jumps to static-kernel addresses), two CPUs inside one
    # task's trap path. The riscv64 twin never had the bug: it clears
    # on_cpu in asm with only the register restore + ret left. Match it:
    # everything below is register loads and the ret itself.
    testq %rbx, %rbx
    jz   1f
    movq $0,  {task_on_cpu}(%rbx)   # prev->ti_on_cpu = false (x86-TSO release)
    movq $-1, {task_claim}(%rbx)    # prev->running_on_cpu = -1
1:

    # 4. Restore next's context.  r12 (holding `next`) is reloaded LAST.
    movq {task_thread}+{callee_r13}(%r12), %r13
    movq {task_thread}+{callee_r14}(%r12), %r14
    movq {task_thread}+{callee_r15}(%r12), %r15
    movq {task_thread}+{callee_rbp}(%r12), %rbp
    movq {task_thread}+{thread_sp}(%r12), %rsp
    movq {task_thread}+{callee_ret}(%r12), %rax
    movq {task_thread}+{callee_rbx}(%r12), %rbx
    movq {task_thread}+{callee_r12}(%r12), %r12

    # 5. "Return" into the next task's saved return address —
    #    ret_from_fork for a newborn child.
    pushq %rax
    ret

.size __switch_to, . - __switch_to
"#,
    task_thread = const core::mem::offset_of!(Task, thread),
    task_on_cpu = const core::mem::offset_of!(Task, ti_on_cpu),
    task_claim = const core::mem::offset_of!(Task, running_on_cpu),
    callee_rbx = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, rbx),
    callee_rbp = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, rbp),
    callee_r12 = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, r12),
    callee_r13 = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, r13),
    callee_r14 = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, r14),
    callee_r15 = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, r15),
    callee_ret = const core::mem::offset_of!(crate::arch::thread::CalleeSaved, ret_addr),
    thread_sp = const core::mem::offset_of!(crate::arch::thread::ThreadStruct, sp),
    options(att_syntax),
);

/// Publish the switch (called from `__switch_to` between the prev-save
/// and the next-restore).  Claims next's continuation, publishes the
/// per-CPU current slot, and runs the scribble diagnostics.
///
/// R65 (protocol order): prev's pickability bits ({ti_on_cpu,
/// running_on_cpu}) are NOT cleared here anymore — `__switch_to` clears
/// them in asm AFTER this helper and the rsp0 install return, right
/// before the register restore. Clearing them at the top of this helper
/// (the old order) left a window where another CPU could legally resume
/// prev while THIS CPU was still executing the switch chain (helper
/// calls, reports, ring dumps) on prev's kernel stack — the resumed
/// task's traps then pushed frames through the switcher's live frames.
/// Everything this helper does now runs while prev is still marked
/// on-CPU, i.e. unpickable.
///
/// # Safety
/// Called only from `__switch_to` with the prev context fully saved.
#[no_mangle]
pub unsafe extern "C" fn x86_switch_publish(prev: *mut Task, next: *mut Task) {
    let cpu = crate::arch::cpu_id() as i32;
    // R65 witness: prev must still carry its pick mark here. A clear mark
    // at publish entry means the R49 heal (or a scribble) reset the
    // ownership word inside the switch-out window — name it.
    if !prev.is_null()
        && crate::dfx::scribble::ENABLED.load(core::sync::atomic::Ordering::Relaxed)
        && !unsafe { (*prev).on_cpu() }
    {
        crate::dfx::scribble::switch_window_heal(prev as u64);
    }
    // RACE-FORENSICS (x86-smprace): claim next's continuation BEFORE its
    // registers are restored. A claim held by another CPU is a double-run
    // — the two CPUs would execute one task on one kernel stack (the SMP
    // fork/exec/exit crash family; caught live twice with the pre-GS-fix
    // kernel, silent since the trap_exit cli/swapgs fix).
    if !next.is_null() {
        // R65 hunt: never publish a non-Task `next` into the per-CPU
        // current slot — a poisoned slot makes the NEXT __schedule run
        // its whole prev-side bookkeeping through garbage (the
        // self-propagating half of the x86-scribble3 cascade). Report and
        // keep the previous current instead.
        if crate::dfx::scribble::ENABLED.load(core::sync::atomic::Ordering::Relaxed)
            && !crate::dfx::scribble::task_plausible(next)
        {
            crate::dfx::scribble::bad_switch_arg(2, next as u64, prev as u64);
            crate::dfx::scribble::ring_log(
                crate::arch::cpu_id() as usize,
                2,
                prev as u64,
                next as u64,
            );
            return;
        }
        let old = unsafe {
            (*next).running_on_cpu.swap(cpu, core::sync::atomic::Ordering::AcqRel)
        };
        if old != -1 {
            // Scribble hunter: report through the locked buffer (the
            // raw print raced the concurrent panic output in earlier
            // captures), dump the event ring, and park for gdb.
            crate::dfx::scribble::double_run(
                unsafe { (*next).pid() } as u64,
                cpu as u64,
                old as i64 as u64,
                if prev.is_null() {
                    0
                } else {
                    unsafe { (*prev).pid() as u64 }
                },
                next as u64,
            );
        }
    }
    crate::arch::smp::set_current_task_ptr(next as u64);
    // SCRIBBLE2-GUARD (switch-in sp): the resume point __switch_to is
    // about to load must lie inside next's own kernel stack.  A foreign
    // sp (stale/double-run save or a scribbled thread.sp) makes the task
    // execute on someone else's stack — frames tear both owners' data
    // while kernel_stack stays pristine.  NOTE: report-only — the idle
    // tasks intentionally run on the static boot stacks, so the report
    // is a lead, not proof.
    if !next.is_null() {
        // SAFETY: header reads of the task we are switching into; its
        // thread.sp/kstack fields are stable during the switch.
        unsafe {
            let sp = (*next).thread().sp as usize;
            let top = (*next).get_kernel_stack().map_or(0, |p| p as usize);
            let bottom = (*next).kernel_stack_bottom();
            // Static tasks (idle storages, the boot task) legitimately run
            // on the static boot stacks — their kstack fields never match
            // the live sp. Reporting them spammed the console on every
            // idle switch-in (and, pre-R65, widened the on-prev-stack
            // window of this very helper). Heap tasks only. (Range check:
            // kernel-image addresses are numerically ABOVE the heap.)
            let is_static =
                !(0xffff_8880_4000_0000..0xffff_8880_4800_0000).contains(&(next as usize));
            if !is_static && top != 0 && !(sp > bottom && sp <= top) {
                use crate::console::putchar_no_lock as putchar;
                const M: &[u8] = b"\nSWITCHIN-FOREIGN-SP task=0x";
                for &b in M {
                    putchar(b);
                }
                let mut v = next as u64;
                for _ in 0..16 {
                    let n = (v >> 60) as u8;
                    putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                    v <<= 4;
                }
                const M2: &[u8] = b" sp=0x";
                for &b in M2 {
                    putchar(b);
                }
                let mut v = sp as u64;
                for _ in 0..16 {
                    let n = (v >> 60) as u8;
                    putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                    v <<= 4;
                }
                const M3: &[u8] = b" kstack=[0x";
                for &b in M3 {
                    putchar(b);
                }
                let mut v = bottom as u64;
                for _ in 0..16 {
                    let n = (v >> 60) as u8;
                    putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                    v <<= 4;
                }
                putchar(b',');
                let mut v = top as u64;
                for _ in 0..16 {
                    let n = (v >> 60) as u8;
                    putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                    v <<= 4;
                }
                const M4: &[u8] = b") report-only\n";
                for &b in M4 {
                    putchar(b);
                }
                // Report-only (SCRIBBLE2 lesson): the AP idles legitimately
                // run on the static AP boot stacks (their kstack fields are
                // never used), so a "foreign" sp here is not automatically
                // corruption — repinning would break a mid-loop resume.
                crate::dfx::scribble::ring_dump();
            }
        }
    }
    // Scribble hunter (dfx=scribble): record the switch completion.
    crate::dfx::scribble::ring_log(
        crate::arch::cpu_id() as usize,
        2,
        prev as u64,
        next as u64,
    );
    // Scribble hunter (dfx=scribble): quiesce point — prev's context is
    // fully saved here, so any tracked field off its shadow was written
    // by someone else while it ran.
    if !prev.is_null() {
        crate::dfx::scribble::quiesce(prev);
    }
}

/// Set TSS.rsp0 (and only that) to the task's kernel stack top.
/// Called from `__switch_to`; also usable from the boot path.
///
/// R64 (cross-stack execution guard): TSS.rsp0 is the stack EVERY
/// user-origin trap/syscall entry on this CPU pushes its frame onto. A
/// garbage value (scribbled `kernel_stack` field — the byte-flip family)
/// silently redirects every subsequent ring crossing onto a foreign
/// stack: frames tear the owner's data, the victim returns through
/// corrupted slots (single-byte rip flips), and the switch path itself
/// then reads ITS fields from the torn pages — the self-propagating
/// cross-stack engine caught twice with the scribble detector (c1/e2:
/// tasks executing 4MB away from their own stacks with intact
/// kernel_stack fields, i.e. an earlier bad rsp0 install). Validate
/// before installing: the top must be a canonical heap-range,
/// 16-aligned, non-zero address. On failure keep the PREVIOUS rsp0 (the
/// task's next entry lands on the old stack — wrong but mapped and
/// attributable) and report loudly.
///
/// # Safety
/// `task` must be a valid Task (or null).
#[no_mangle]
pub unsafe extern "C" fn x86_update_rsp0(task: *mut Task) {
    let top = if task.is_null() {
        0
    } else {
        // SAFETY: get_kernel_stack returns the stack top for live tasks;
        // Option's niche encodes None as 0.
        unsafe { (*task).get_kernel_stack().map_or(0, |p| p as u64) }
    };
    // Kernel stacks live in the 128MB heap region of the direct map
    // (VIRTUAL 0xffff8880_40000000 — the physical 0x40000000 base plus
    // the linear-map prefix). Byte-flipped stack pointers can still land
    // inside — this catches the coarse corruption classes (null, small
    // ints, user pointers, text/data addresses, prefix-byte flips).
    const HEAP_LO: u64 = 0xffff_8880_4000_0000;
    const HEAP_HI: u64 = 0xffff_8880_4800_0000;
    if top != 0 && (top < HEAP_LO || top >= HEAP_HI || top & 0xF != 0) {
        use crate::console::putchar_no_lock as putchar;
        const MSG: &[u8] = b"\nR64-BAD-RSP0 task=0x";
        // SAFETY: raw console write, no locks, no allocation.
        unsafe {
            for &b in MSG {
                putchar(b);
            }
            let mut v = task as u64;
            for _ in 0..16 {
                let n = (v >> 60) as u8;
                putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                v <<= 4;
            }
            const M2: &[u8] = b" top=0x";
            for &b in M2 {
                putchar(b);
            }
            let mut v = top;
            for _ in 0..16 {
                let n = (v >> 60) as u8;
                putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                v <<= 4;
            }
            putchar(b'\n');
        }
        return; // keep the previous (valid) rsp0
    }
    super::trap::set_tss_rsp0(top);
}

// ============================================================================
// Per-CPU prev task (for ret_from_fork's schedule_tail call)
// ============================================================================

static CPU_PREV_TASK: [core::sync::atomic::AtomicU64; crate::config::MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; crate::config::MAX_CPUS];

#[inline]
pub fn set_prev_task(prev: *mut Task) {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        CPU_PREV_TASK[cpu].store(prev as u64, core::sync::atomic::Ordering::Relaxed);
    }
}

#[no_mangle]
pub extern "C" fn get_prev_task() -> *mut Task {
    let cpu = crate::arch::cpu_id() as usize;
    if cpu < crate::config::MAX_CPUS {
        CPU_PREV_TASK[cpu].load(core::sync::atomic::Ordering::Relaxed) as *mut Task
    } else {
        core::ptr::null_mut()
    }
}

// ============================================================================
// High-level context_switch
// ============================================================================

/// Context switch wrapper (same protocol as the riscv64 twin):
///
/// 1. save prev FPU (fxsave into thread)
/// 2. switch_mm via CR3 if the address space changed
/// 3. save/restore per-task FS base (TLS)
/// 4. `__switch_to` (registers, stacks, per-CPU current, TSS.rsp0)
/// 5. restore the incoming task's FPU
///
/// # Safety
/// Must be called with interrupts disabled (caller's responsibility).
/// After `__switch_to` all locals are invalid; the incoming task is
/// resolved through `sched::current()` (the per-CPU slot).
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // Step 1: save prev FPU state
    prev.thread_mut().fpu_save_for_switch();

    // Store prev task for ret_from_fork's schedule_tail(prev)
    set_prev_task(prev as *mut Task);

    // Step 2: switch_mm — user PML4s share the kernel half, so a CR3
    // write from anywhere keeps the kernel mapped (unlike the riscv64
    // twin, no linear-map region dance is needed).
    if let Some(next_mm) = next.address_space() {
        let next_ppn = next_mm.root_ppn();
        if read_cr3() >> 12 != next_ppn {
            // PCID is disabled at bring-up: asid is always 0 and the CR3
            // reload itself flushes non-global translations.
            write_cr3(super::mm::asid::build_satp(0, next_ppn));
        }
    } else {
        // Next is a kernel thread — switch to the kernel root so a
        // dying user mm (dropped by do_exit) can be freed under our feet.
        let kernel_ppn = super::mm::mmu_init::root_page_table_ppn();
        if read_cr3() >> 12 != kernel_ppn {
            write_cr3(super::mm::asid::build_satp(0, kernel_ppn));
        }
    }

    // Step 3: FS base (TLS pointer).  Saved from the live MSR so a task
    // switched out inside a set_thread_area window resumes correctly.
    // The GS shadow (IA32_KERNEL_GS_BASE) is programmed alongside so the
    // swapgs pairing in trap.S holds per task: the kernel runs with GS =
    // percpu and the SHADOW carries the task's user GS base (0 unless a
    // task used ARCH_SET_GS), which SWAPGS installs on the user return.
    let fs = rdmsr(MSR_FS_BASE);
    prev.thread_mut().fs_base = fs;
    prev.thread_mut().gs_base = rdmsr(MSR_KERNEL_GS_BASE);
    wrmsr(MSR_FS_BASE, next.thread().fs_base);
    wrmsr(MSR_KERNEL_GS_BASE, next.thread().gs_base);

    // Step 4: registers + stacks + per-CPU current + TSS.rsp0.
    //
    // WARNING: after this call all locals are INVALID (callee-saved
    // registers and rsp belong to the incoming task).
    __switch_to(prev, next);

    // Step 5: restore the incoming task's FPU (review ARCH-H1 parity):
    // resolved via sched::current() — the per-CPU slot __switch_to just
    // published — never through the stale `next` reference.
    if let Some(cur) = crate::sched::current() {
        // SAFETY: cur is the task now running on this CPU.
        unsafe {
            (*cur).thread_mut().restore_fpu();
        }
    }
}
