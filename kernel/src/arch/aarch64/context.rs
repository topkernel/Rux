//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! AArch64 context switching.
//!
//! __switch_to asm: callee-saved register swap (x19-x28, sp, lr) with
//! EAGER FPSIMD save/restore inline (see thread.rs for the discipline
//! rationale). The wrapper mirrors the riscv64 backend's steps:
//! pick-state abort check → prev FPU bookkeeping → switch_mm (TTBR0_EL1
//! + CONTEXTIDR_EL1 ASID) → __switch_to.

use crate::process::task::Task;

// Field offsets for the asm (thread::ThreadStruct)
use super::thread::{
    THREAD_FPU, THREAD_RA, THREAD_S0, THREAD_SP, THREAD_TP_VALUE,
    THREAD_USER_FPSIMD, THREAD_USER_FPSIMD_FPCR, THREAD_USER_FPSIMD_FPSR,
};

core::arch::global_asm!(
    r#"
    // Kernel codegen is general-regs-only (see .cargo/config.toml); the
    // eager FPSIMD save/restore blocks below re-enable fp/simd for this
    // asm unit only.
    .arch armv8-a+fp+simd
    .align 4
    .global __switch_to
    .type __switch_to, %function
// x0 = prev Task*, x1 = next Task*; thread is an INLINE Task field
__switch_to:
    // ---- save prev (x0): x8 = &prev.thread ----
    mov x9, #{t_off}                 // TASK_THREAD offset (may exceed imm12)
    add x8, x0, x9
    stp x19, x20, [x8, #{s_off}]
    stp x21, x22, [x8, #{s_off} + 16]
    stp x23, x24, [x8, #{s_off} + 32]
    stp x25, x26, [x8, #{s_off} + 48]
    stp x27, x28, [x8, #{s_off} + 64]
    mov x9, sp
    str x9, [x8, #{sp_off}]
    str lr, [x8, #{ra_off}]

    // ---- user TLS (TPIDR_EL0): save prev ----
    // TPIDR_EL0 is per-CPU hardware state that EL0 writes directly
    // (glibc/musl TLS base; there is no set-tls syscall on this ABI).
    // The kernel never reads or writes it outside these two spots, so
    // while a task is current its live TPIDR_EL0 always equals its
    // saved tp_value — capture that here, reinstall for next below.
    // Without this, a task resuming on a CPU whose TPIDR_EL0 was last
    // set by a DIFFERENT image runs userspace with a foreign (or zero)
    // TLS base: glibc's first TLS access then faults at
    // ~(tp + slot-offset), which is exactly the post-fork SIGSEGV
    // storm that killed every external command in the interactive
    // shell (parent in __free at 0*4097+0x90, child in
    // pthread_rwlock_unlock at 0-0x720+208).
    mrs x9, tpidr_el0
    str x9, [x8, #{tp_off}]

    // ---- eager FPSIMD save: prev ----
    add x10, x8, #{fpu_off}
    stp q0, q1, [x10], #32
    stp q2, q3, [x10], #32
    stp q4, q5, [x10], #32
    stp q6, q7, [x10], #32
    stp q8, q9, [x10], #32
    stp q10, q11, [x10], #32
    stp q12, q13, [x10], #32
    stp q14, q15, [x10], #32
    stp q16, q17, [x10], #32
    stp q18, q19, [x10], #32
    stp q20, q21, [x10], #32
    stp q22, q23, [x10], #32
    stp q24, q25, [x10], #32
    stp q26, q27, [x10], #32
    stp q28, q29, [x10], #32
    stp q30, q31, [x10]
    mrs x11, fpcr
    mrs x12, fpsr
    mov x13, #{fpcr_off}
    mov x14, #{fpsr_off}
    add x13, x8, x13
    add x14, x8, x14
    str w11, [x13]
    str w12, [x14]

    // ---- R56-heal affirm gate: PICKED(0) -> LIVE(1) on next ----
    // (idle task pid==0 is never pick-marked)
    ldr w13, [x1, #{pid_off}]
    cbz w13, .Lswitch_go
    mov x9, #{pick_off}
    add x9, x1, x9
.Lr56_affirm:
    ldaxr w13, [x9]
    cmp w13, #1
    b.eq .Lswitch_abort          // already LIVE: protocol breach
    cbnz w13, .Lswitch_abort     // 2 = revoked (anything else = garbage)
    mov w14, #1
    stxr w15, w14, [x9]
    cbnz w15, .Lr56_affirm       // lost exclusive: retry
.Lswitch_go:

    // ---- R74 thread.sp validation: kernel half, 16-aligned, not in
    //      [_stext, _etext). Repair path lands with stage 2's full
    //      protocol; a bad sp parks in the abort loop for now. ----
    mov x15, #{t_off}
    add x8, x1, x15                // x8 = &next.thread for validation
    ldr x9, [x8, #{sp_off}]
    cbz x9, .Lswitch_sp_bad
    tbnz x9, #63, 1f
    b .Lswitch_sp_bad
1:  tbz x9, #4, 2f               // bit 4/3 alignment check below
    tbnz x9, #3, .Lswitch_sp_bad
2:  adrp x16, _stext
    add x16, x16, :lo12:_stext
    cmp x9, x16
    b.lo 3f                      // below text: fine (.bss stacks)
    adrp x16, _etext
    add x16, x16, :lo12:_etext
    cmp x9, x16
    b.lo .Lswitch_sp_bad         // inside [_stext, _etext): reject
3:
    // ---- NEW2/R65: release prev's ownership qword (on_cpu=0,
    //      running_on_cpu=-1) after the full context save ----
    dmb ish
    mov x16, #{on_cpu_off}
    add x16, x0, x16
    mov x17, #-1
    lsl x17, x17, #32            // 0xFFFFFFFF_00000000
    str x17, [x16]

    // ---- eager FPSIMD restore: next (x8 = &next.thread, set above) ----
    add x10, x8, #{fpu_off}
    ldp q0, q1, [x10], #32
    ldp q2, q3, [x10], #32
    ldp q4, q5, [x10], #32
    ldp q6, q7, [x10], #32
    ldp q8, q9, [x10], #32
    ldp q10, q11, [x10], #32
    ldp q12, q13, [x10], #32
    ldp q14, q15, [x10], #32
    ldp q16, q17, [x10], #32
    ldp q18, q19, [x10], #32
    ldp q20, q21, [x10], #32
    ldp q22, q23, [x10], #32
    ldp q24, q25, [x10], #32
    ldp q26, q27, [x10], #32
    ldp q28, q29, [x10], #32
    ldp q30, q31, [x10]
    mov x13, #{fpcr_off}
    mov x14, #{fpsr_off}
    add x13, x8, x13
    add x14, x8, x14
    ldr w11, [x13]
    ldr w12, [x14]
    msr fpcr, x11
    msr fpsr, x12

    // ---- switch the current-task identity to NEXT (aarch64 tp twin:
    //      the riscv64 __switch_to does `mv tp, a1`; without this every
    //      post-switch current() keeps naming the PREVIOUS task — the
    //      trap entry, sched::current and the R56 slot machinery all
    //      read TPIDR_EL1). Written at the COMMIT point only (after the
    //      affirm/sp gates): an abort path must keep naming prev. ----
    msr tpidr_el1, x1

    // ---- user TLS (TPIDR_EL0): restore next (commit point, same
    //      discipline as the identity switch above) ----
    ldr x9, [x8, #{tp_off}]
    msr tpidr_el0, x9

    ldp x19, x20, [x8, #{s_off}]
    ldp x21, x22, [x8, #{s_off} + 16]
    ldp x23, x24, [x8, #{s_off} + 32]
    ldp x25, x26, [x8, #{s_off} + 48]
    ldp x27, x28, [x8, #{s_off} + 64]
    ldr x9, [x8, #{sp_off}]
    ldr lr, [x8, #{ra_off}]
    mov sp, x9

    ret

.Lswitch_abort:
    mov x0, x0                   // prev already in x0
    b aarch64_switch_abort
.Lswitch_sp_bad:
    // STAGE 2 TODO: repair to the task's own kernel stack top (riscv64
    // R74 twin). Park for now — never resume on a corrupt sp.
    wfe
    b .Lswitch_sp_bad
"#,
    t_off = const crate::process::task::task_offsets::TASK_THREAD,
    pid_off = const crate::process::task::task_offsets::TASK_PID,
    pick_off = const crate::process::task::task_offsets::TI_PICK_STATE,
    on_cpu_off = const crate::process::task::task_offsets::TI_ON_CPU,
    s_off = const THREAD_S0,
    sp_off = const THREAD_SP,
    ra_off = const THREAD_RA,
    tp_off = const THREAD_TP_VALUE,
    fpu_off = const THREAD_FPU,
    fpcr_off = const { core::mem::offset_of!(super::thread::ThreadStruct, fpcr) },
    fpsr_off = const { core::mem::offset_of!(super::thread::ThreadStruct, fpsr) },
);

// ============================================================================
// EL0 FPSIMD save/restore (user-state integrity across kernel FP usage)
// ============================================================================

// Kernel FP discipline: compiler-generated code (core/alloc rlibs) may
// use NEON, which would corrupt the USER's live v0-v31 across a trap.
// trap.S calls el0_entry_fpsimd_save at every EL0->EL1 entry and
// el0_exit_fpsimd_restore just before eret; the faithful user image
// lives in task.thread.user_fpsimd. Both helpers use ONLY x16/x17 as
// scratch (their frame slots are already saved at entry / not yet
// loaded at exit) and need no stack.
core::arch::global_asm!(
    r#"
    .arch armv8-a+fp+simd
    .align 4
    .global el0_entry_fpsimd_save
    .type el0_entry_fpsimd_save, %function
// C-ABI: no args; task from TPIDR_EL1. Clobbers x16/x17 + q0-q31 + NZCV.
el0_entry_fpsimd_save:
    mrs x16, tpidr_el1
    cbz x16, 1f                    // early boot: no task, nothing to save
    mov x17, #{t_off}
    add x16, x16, x17              // x16 = &task.thread
    mov x17, #{uv_off}
    add x17, x16, x17              // x17 = &thread.user_fpsimd
    stp q0, q1, [x17], #32
    stp q2, q3, [x17], #32
    stp q4, q5, [x17], #32
    stp q6, q7, [x17], #32
    stp q8, q9, [x17], #32
    stp q10, q11, [x17], #32
    stp q12, q13, [x17], #32
    stp q14, q15, [x17], #32
    stp q16, q17, [x17], #32
    stp q18, q19, [x17], #32
    stp q20, q21, [x17], #32
    stp q22, q23, [x17], #32
    stp q24, q25, [x17], #32
    stp q26, q27, [x17], #32
    stp q28, q29, [x17], #32
    stp q30, q31, [x17]
    mrs x16, fpsr
    str w16, [x17]                 // x17 == &user_fpsimd.fpsr now
    mrs x16, fpcr
    str w16, [x17, #4]
1:  ret
"#,
    t_off = const crate::process::task::task_offsets::TASK_THREAD,
    uv_off = const THREAD_USER_FPSIMD,
);

core::arch::global_asm!(
    r#"
    .arch armv8-a+fp+simd
    .align 4
    .global el0_exit_fpsimd_restore
    .type el0_exit_fpsimd_restore, %function
// C-ABI: no args; task from TPIDR_EL1. Clobbers x16/x17 + q0-q31 + NZCV.
el0_exit_fpsimd_restore:
    mrs x16, tpidr_el1
    cbz x16, 1f                    // no task: leave FP regs untouched
    mov x17, #{t_off}
    add x16, x16, x17              // x16 = &task.thread
    mov x17, #{uv_off}
    add x17, x16, x17              // x17 = &thread.user_fpsimd
    ldp q0, q1, [x17], #32
    ldp q2, q3, [x17], #32
    ldp q4, q5, [x17], #32
    ldp q6, q7, [x17], #32
    ldp q8, q9, [x17], #32
    ldp q10, q11, [x17], #32
    ldp q12, q13, [x17], #32
    ldp q14, q15, [x17], #32
    ldp q16, q17, [x17], #32
    ldp q18, q19, [x17], #32
    ldp q20, q21, [x17], #32
    ldp q22, q23, [x17], #32
    ldp q24, q25, [x17], #32
    ldp q26, q27, [x17], #32
    ldp q28, q29, [x17], #32
    ldp q30, q31, [x17]
    ldr w16, [x17]                 // x17 == &user_fpsimd.fpsr now
    ldr w17, [x17, #4]
    msr fpcr, x17
    msr fpsr, x16
1:  ret
"#,
    t_off = const crate::process::task::task_offsets::TASK_THREAD,
    uv_off = const THREAD_USER_FPSIMD,
);

// ============================================================================
// Per-CPU variable for prev task
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

#[inline]
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
// switch_mm (TTBR0_EL1 + ASID)
// ============================================================================

/// Low-level linear page-table switch (full-flush form; the abort path's
/// obviously-correct choice, riscv64 twin semantics)
///
/// # Safety
/// Caller must ensure next_ppn is a valid page table root PPN
#[inline(never)]
#[no_mangle]
pub unsafe fn __switch_mm_linear(next_ppn: u64) {
    // SAFETY: TTBR0 switch with full TLB invalidation; the kernel half
    // (TTBR1) is untouched.
    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {ttbr}",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            ttbr = in(reg) next_ppn << 12,
            options(nostack)
        );
    }
}

/// Switch the user address space root (TTBR0_EL1) with an ASID.
///
/// # Safety
/// next_ppn must be a valid user root PPN; asid must belong to this mm
#[inline]
pub unsafe fn switch_mm(next_ppn: u64, asid: u16) {
    // SAFETY: TTBR0 + CONTEXTIDR(ASID) switch with an ASID-scoped
    // invalidation (covers ASID reuse; kernel half unaffected).
    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {ttbr}",
            "msr contextidr_el1, {asid}",
            "tlbi aside1is, {asid}",
            "dsb nsh",
            "isb",
            ttbr = in(reg) next_ppn << 12,
            asid = in(reg) asid as u64,
            options(nostack)
        );
    }
}

/// Get the current composed "satp" (mode/ASID/TTBR0 root PPN)
#[inline]
pub fn get_current_satp() -> u64 {
    let satp = super::mm::pagetable::get_satp();
    satp.bits()
}

// ============================================================================
// High-level context_switch
// ============================================================================

extern "C" {
    fn __switch_to(prev: *mut Task, next: *mut Task);
}

/// Context switch wrapper (see the module doc; steps mirror the riscv64
/// twin — the scheduler-integration logic is generic Task API)
///
/// # Safety
/// Must be called with interrupts disabled (caller's responsibility)
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // R56-heal early revoked-pick check (generic pick-state protocol)
    if (*next).pid() != 0 {
        let st = next.pick_state();
        if st != crate::process::task::PICK_PICKED {
            aarch64_switch_abort(prev, next);
            return;
        }
    }

    // Store prev task for ret_from_fork
    set_prev_task(prev as *mut Task);

    // Step 1: switch_mm — switch address space FIRST
    if let Some(next_mm) = next.address_space() {
        let next_ppn = next_mm.root_ppn();
        let current_ppn = get_current_satp() & 0xFFFF_FFFF_FFFF;
        if current_ppn != next_ppn {
            let asid = next_mm.asid();
            let asid = if asid != 0 { asid } else { next_mm.alloc_asid().unwrap_or(0) };
            switch_mm(next_ppn, asid);
        }
    } else {
        // Kernel thread / idle: switch to the kernel root so a freed user
        // root can't stay live (see the riscv64 twin's do_exit rationale)
        let kernel_ppn = super::mm::mmu_init::root_page_table_ppn();
        let current_ppn = get_current_satp() & 0xFFFF_FFFF_FFFF;
        if current_ppn != kernel_ppn {
            switch_mm(kernel_ppn, 0);
        }
    }

    // Step 2: __switch_to — registers + eager FPSIMD (inside the asm).
    // WARNING: after this call all locals are INVALID (riscv64 twin note).
    __switch_to(prev, next);
}

/// R56-heal switch-in abort repair (aarch64 twin of riscv_switch_abort)
#[no_mangle]
pub unsafe extern "C" fn aarch64_switch_abort(prev: *mut Task, next: *mut Task) {
    // 1. Address space: compare-based (only repair if the wrapper already
    //    switched — the early-check site has not)
    let prev_root = if let Some(mm) = (*prev).address_space() {
        mm.root_ppn()
    } else {
        super::mm::mmu_init::root_page_table_ppn()
    };
    if get_current_satp() & 0xFFFF_FFFF_FFFF != prev_root {
        __switch_mm_linear(prev_root);
    }

    // 2. Sched slot + tripwire
    let cpu = crate::arch::cpu_id() as usize;
    crate::sched::sched::republish_current_after_abort(cpu, prev);
    crate::sched::sched::r56_report_switch_abort(cpu, (*prev).pid());

    let _ = next;
}
