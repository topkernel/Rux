//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! RISC-V 64-bit context switching
//!
//! Context switch implementation:
//! - switch_mm(): Switch page table (write satp) - called FIRST
//! - __switch_to(): Switch registers (ra, sp, s0-s11) - called SECOND
//!
//! Reference: kernel/sched/core.c context_switch()
//!
//! # Safety Invariants — Context Switch Atomicity
//!
//! - **INV-CS-1**: `__switch_to` saves and restores all callee-saved registers
//!   (ra, sp, s0–s11). The caller-saved registers are already saved by the
//!   compiler in the caller's stack frame before `schedule()` is invoked.
//!
//! - **INV-CS-2**: FPU state (if `Fs != Off`) is saved/restored via
//!   `fpu_save_for_switch` / `__fstate_restore` around the register switch.
//!
//! - **INV-CS-3**: The MMU (satp) is switched via `switch_mm()` **before**
//!   `__switch_to` runs, so the new task's page table is active when its
//!   registers are restored.
//!
//! - **INV-CS-4**: `__switch_to` does NOT enable SIE (interrupts). The caller
//!   (`__schedule`) manages SIE via `lock_irqsave()` / `restore_irq()`. Each
//!   task has its own saved IRQ state in `__schedule()`'s stack frame, and
//!   after context_switch the new task restores its own IRQ state. This matches
//!   the reference implementation where `__switch_to` only restores SUM, never SIE.
//!
//! - **INV-CS-5**: After `__switch_to`, the previous task's stack pointer
//!   (`prev_sp`) is stale; the only valid way to retrieve the current task
//!   is via the `tp` (thread pointer) register, as set by `__switch_to`.

use crate::process::task::Task;
use crate::process::Task as ProcessTask;
use core::arch::asm;
use super::mm::PageTable;
use super::mm::PageTableEntry;

/// sstatus.SUM bit mask
pub const SR_SUM: u64 = 1 << 18;

/// Get current task pointer from tp register
///
/// After __switch_to, the tp register contains a pointer to the current task.
/// This function reads tp and returns it as a Task reference.
///
/// # Safety
/// This function is safe to call after __switch_to has set tp to a valid task pointer.
#[inline]
fn current_task() -> &'static mut Task {
    // SAFETY: tp was set to a valid Task pointer by __switch_to (assembly context switch);
    // the Task object is allocated by the task subsystem and lives for the task's lifetime.
    unsafe {
        let tp: u64;
        asm!("mv {}, tp", out(reg) tp, options(nomem, nostack, pure));
        // tp is guaranteed to be a valid task pointer after __switch_to
        &mut *(tp as *mut Task)
    }
}

// ============================================================================
// __switch_to
// ============================================================================

core::arch::global_asm!(
    r#"
.section .text.__switch_to
.align 2

.global __switch_to
.type __switch_to, @function
__switch_to:
    # a0 = prev task, a1 = next task
    # Calculate thread struct pointers
    li    a4, {task_thread}
    add   a3, a0, a4
    add   a4, a1, a4

    # Save prev's context
    sd    ra,  {thread_ra}(a3)
    sd    sp,  {thread_sp}(a3)
    sd    s0,  {thread_s0} + 0*8(a3)
    sd    s1,  {thread_s0} + 1*8(a3)
    sd    s2,  {thread_s0} + 2*8(a3)
    sd    s3,  {thread_s0} + 3*8(a3)
    sd    s4,  {thread_s0} + 4*8(a3)
    sd    s5,  {thread_s0} + 5*8(a3)
    sd    s6,  {thread_s0} + 6*8(a3)
    sd    s7,  {thread_s0} + 7*8(a3)
    sd    s8,  {thread_s0} + 8*8(a3)
    sd    s9,  {thread_s0} + 9*8(a3)
    sd    s10, {thread_s0} + 10*8(a3)
    sd    s11, {thread_s0} + 11*8(a3)

    # Save sstatus.SUM bit
    csrr  t0, sstatus
    sd    t0, {thread_sum}(a3)

    # NEW2 root cause fix (R8-1b): prev's full context is now saved — make
    # it pickable by other CPUs. The release fence guarantees that a CPU
    # observing on_cpu == 0 also observes the register stores above (its
    # load of thread.sp will see the values stored here).
    #
    # R65-timer (timer regression fix): the release store must write the
    # FULL ownership qword — ti_on_cpu=false AND running_on_cpu=-1 — not
    # plain zero. ti_on_cpu (1 byte) and running_on_cpu (4 bytes at +4)
    # share one 8-byte word; the old `sd zero` cleared BOTH halves to 0,
    # leaving running_on_cpu == 0 == "claimed by CPU 0". With the dual-bit
    # pick predicate (R65, e87a8b40: pickable only when claim == -1 or the
    # picker's own id), every switched-out task became pickable by CPU 0
    # ALONE — CPUs 1..N starved and timer wakes stalled 6x (timer_probe
    # TIMER-STALL, state=R). The claim itself is acquired at switch-in by
    # the Rust wrapper (the twin of x86_switch_publish's claim swap);
    # releasing both bits in one aligned 8-byte store keeps the word
    # internally consistent for the non-atomic pick-side readers.
    fence rw, rw
    li    t0, {task_on_cpu}
    add   t0, a0, t0
    li    t1, -1
    slli  t1, t1, 32          # 0xFFFFFFFF_00000000: on_cpu=0, claim=-1
    sd    t1, 0(t0)

    # Restore next's context
    # Restore SUM bit: clear first, then conditionally set from next's
    # saved value. This per-task save/restore is load-bearing (do NOT
    # "harden" it into an unconditional clear): a task preempted while
    # inside a uaccess copy window runs with SUM=1, and unless that value
    # is restored on switch-in, the resumed copy loop would fault with
    # SUM=0 and spuriously fail with EFAULT for a valid user pointer.
    # Outside the bracketed uaccess windows every task saves SUM=0 (SUM
    # convergence, review SEC), so no SUM=1 leaks between tasks.
    ld    t0, {thread_sum}(a4)
    li    t1, {sr_sum}
    csrc  sstatus, t1         // Clear SUM unconditionally
    and   t0, t0, t1
    csrs  sstatus, t0         // Set SUM if next's saved value has it

    # Now restore callee-saved registers (s0 last)
    ld    ra,  {thread_ra}(a4)
    ld    sp,  {thread_sp}(a4)
    ld    s11, {thread_s0} + 11*8(a4)
    ld    s10, {thread_s0} + 10*8(a4)
    ld    s9,  {thread_s0} + 9*8(a4)
    ld    s8,  {thread_s0} + 8*8(a4)
    ld    s7,  {thread_s0} + 7*8(a4)
    ld    s6,  {thread_s0} + 6*8(a4)
    ld    s5,  {thread_s0} + 5*8(a4)
    ld    s4,  {thread_s0} + 4*8(a4)
    ld    s3,  {thread_s0} + 3*8(a4)
    ld    s2,  {thread_s0} + 2*8(a4)
    ld    s1,  {thread_s0} + 1*8(a4)
    ld    s0,  {thread_s0} + 0*8(a4)

    # Update tp = next task
    mv    tp, a1

    # Do NOT enable SIE here.
    #
    # The caller (__schedule) manages SIE via lock_irqsave / restore_irq.
    # Each task has its own saved IRQ state; after context_switch, the new
    # task's __schedule() restores its own IRQ state from its local variable.
    # Enabling SIE here creates a window where a timer interrupt can fire
    # between csrsi and ret, entering .Lrestore_kernel_and_exit which calls
    # schedule() again — causing a nested context_switch that corrupts the
    # caller's stack frame.
    #
    # This matches the reference implementation: __switch_to only restores
    # the SUM (Supervisor User Memory Access) bit, never SIE.

    ret
.size __switch_to, . - __switch_to
"#,
    task_thread = const core::mem::offset_of!(Task, thread),
    task_on_cpu = const core::mem::offset_of!(Task, ti_on_cpu),
    thread_ra = const core::mem::offset_of!(crate::arch::thread::ThreadStruct, ra),
    thread_sp = const core::mem::offset_of!(crate::arch::thread::ThreadStruct, sp),
    thread_s0 = const core::mem::offset_of!(crate::arch::thread::ThreadStruct, s),
    thread_sum = const core::mem::offset_of!(crate::arch::thread::ThreadStruct, sum),
    sr_sum = const SR_SUM,
);

// R65-timer: __switch_to releases the ownership qword with a single
// aligned 8-byte store of 0xFFFFFFFF_00000000. Pin the two layout facts
// that store assumes at compile time — ti_on_cpu is 8-byte aligned (the
// `sd` must be naturally aligned) and running_on_cpu sits at +4 inside
// the same qword — so a future Task field reorder fails the build here
// instead of silently corrupting a neighboring field.
const _: () = {
    assert!(core::mem::offset_of!(Task, ti_on_cpu) % 8 == 0);
    assert!(
        core::mem::offset_of!(Task, running_on_cpu)
            == core::mem::offset_of!(Task, ti_on_cpu) + 4
    );
};

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
// switch_mm
// ============================================================================

/// Low-level page table switch function
/// This MUST be placed in the linear mapping region (VPN2 >= 256)
/// to work correctly after the page table switch.
///
/// # Safety
/// Caller must ensure next_ppn is a valid page table root PPN
#[inline(never)]
#[no_mangle]
pub unsafe fn __switch_mm_linear(next_ppn: u64) {
    let satp = (8u64 << 60) | next_ppn;

    // Use inline asm for the entire switch sequence
    // This code runs in the linear mapping region (VPN2 >= 256)
    asm!(
        // First sfence
        "sfence.vma zero, zero",
        // Switch page table
        "csrw satp, {satp}",
        // Second sfence
        "sfence.vma zero, zero",
        satp = in(reg) satp,
        options(nostack)
    );
}

#[inline]
pub unsafe fn switch_mm(next_ppn: u64, asid: u16) {
    // satp = MODE(Sv39) | ASID | PPN. Wiring the mm's ASID (review PERF:
    // the asid.rs allocator existed with zero consumers, so every switch
    // used ASID 0 and needed full TLB flushes).
    let satp = (8u64 << 60) | ((asid as u64) << 44) | next_ppn;

    // Switch page table
    // The user page table has identity mapping (VPN2[2]) and kernel mappings (VPN2[256-511])
    // so kernel code remains accessible after the switch
    //
    // One ASID-scoped sfence AFTER the satp write (the old code did a full
    // "sfence.vma zero,zero" both before and after — the pre-switch one was
    // pure waste). The scoped fence also covers ASID reuse: a freshly
    // reallocated ASID may still have stale TLB entries from its previous
    // owner, and flushing just that ASID's entries on switch-in is cheap
    // (typically none exist). Kernel/global mappings are unaffected by an
    // rs2!=0 sfence, so the shared kernel half never thrashes.
    core::arch::asm!(
        "csrw satp, {satp}",
        "sfence.vma zero, {asid}",
        satp = in(reg) satp,
        asid = in(reg) asid as u64,
        options(nostack, preserves_flags)
    );
}

#[inline]
pub fn get_current_satp() -> u64 {
    let satp: u64;
    // SAFETY: satp is a supervisor CSR; reading it is always safe.
    unsafe {
        asm!("csrr {}, satp", out(reg) satp, options(nomem, nostack));
    }
    satp
}

// ============================================================================
// High-level context_switch
// ============================================================================

extern "C" {
    fn __switch_to(prev: *mut Task, next: *mut Task);
}

/// Context switch wrapper function
///
/// Flow:
/// 1. Save prev FPU state
/// 2. switch_mm() - switch page table if address space changed
/// 3. __switch_to() - switch registers
/// 4. Restore next FPU state (must be AFTER __switch_to!)
///
/// # Arguments
/// - `prev`: Previous task (being switched out)
/// - `next`: Next task (being switched in)
///
/// # Safety
/// Must be called with interrupts disabled (caller's responsibility)
///
/// # Note
/// The caller (schedule) must ensure interrupts are disabled before calling.
/// After __switch_to, the stack pointer changes, so we must use tp (thread pointer)
/// to get the current task for FPU restoration.
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // Step 1: Save prev FPU state
    prev.thread_mut().fpu_save_for_switch();

    // Store prev task for ret_from_fork
    set_prev_task(prev as *mut Task);

    // Step 2: switch_mm() - Switch address space FIRST
    if let Some(next_mm) = next.address_space() {
        let next_ppn = next_mm.root_ppn();
        let current_satp = get_current_satp();
        let current_ppn = current_satp & 0xFFFFFFFFFFFFF;

        if current_ppn != next_ppn {
            // ASID wiring: allocate (once) and use the mm's own ASID so the
            // TLB can hold multiple address spaces simultaneously. Falls
            // back to ASID 0 if the pool is exhausted — correctness is
            // preserved by the per-switch ASID-scoped sfence either way.
            let asid = next_mm.asid();
            let asid = if asid != 0 {
                asid
            } else {
                next_mm.alloc_asid().unwrap_or(0)
            };
            switch_mm(next_ppn, asid);
        }
    } else {
        // Next task is a kernel thread or idle task — switch to the
        // kernel's root page table.  Without this, the idle task would
        // continue using prev's user page table, which can be freed by
        // do_exit() dropping the last Arc<AddressSpace> reference.
        // A TLB miss on the freed page table causes a page fault.
        let kernel_ppn = super::mm::mmu_init::root_page_table_ppn();
        let current_satp = get_current_satp();
        let current_ppn = current_satp & 0xFFFFFFFFFFFFF;
        if current_ppn != kernel_ppn {
            // Kernel address space: ASID 0 (lazy-TLB style — no user
            // translations are needed while running kernel threads).
            switch_mm(kernel_ppn, 0);
        }
    }

    // Step 3: __switch_to() - Switch registers
    //
    // R65-timer (twin of x86_switch_publish's claim swap): record that
    // THIS cpu owns next's resumed continuation. riscv64 previously never
    // touched running_on_cpu, so the dual-bit pick predicate (R65) had no
    // valid claim to test: combined with the switch-out `sd zero` (which
    // left claim == 0), every task read as CPU 0's property after its
    // first switch-out. Acquiring the claim here at switch-in gives the
    // predicate the same protection x86 has: a stray write that clears
    // only ti_on_cpu while the task runs leaves claim == the running cpu,
    // so the task stays unpickable instead of double-running. The claim
    // is released together with ti_on_cpu by __switch_to's full-word
    // {false, -1} store at the next switch-out. A plain store suffices —
    // next is already dequeued and on_cpu-marked here, so no picker can
    // observe the word until the switch-out release makes it {false, -1}
    // again.
    next.running_on_cpu.store(
        crate::arch::cpu_id() as i32,
        core::sync::atomic::Ordering::Release,
    );

    // WARNING: After __switch_to returns, ALL local variables are INVALID.
    // __switch_to restores callee-saved registers (s0-s11) from the new
    // task's saved state, so any values the compiler stored there are gone.
    // The stack pointer (sp) also changes to the new task's stack, making
    // any stack-based locals inaccessible.  Do NOT read local variables
    // after this call.
    //
    // ti_cpu is already set correctly by sched::context_switch() BEFORE
    // calling us, so there is nothing to do here after the switch.
    __switch_to(prev, next);

    // Restore the incoming task's FPU state (review ARCH-H1). tp and the
    // per-CPU current now refer to `next`; locals above are invalid, so
    // resolve the task via sched::current(). Without this, every task
    // inherited whatever FP registers the previous task left in the FPU —
    // silent numeric corruption across tasks plus cross-task information
    // leakage.
    if let Some(cur) = crate::sched::current() {
        // SAFETY: cur is the task now running on this CPU.
        unsafe {
            (*cur).thread_mut().restore_fpu();
        }
    }
}
