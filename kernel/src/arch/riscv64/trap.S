//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V exception vector table
//!
//! ## Design Points
//!
//! 1. Use sscratch swap to detect trap source
//!    - Kernel mode: sscratch = 0, tp = current task
//!    - User mode: sscratch = current task, tp = user TLS
//! 2. Support compatible detection during early boot phase
//! 3. PtRegs structure layout is consistent
//!
//! ## sscratch/tp Protocol
//!
//! | State       | sscratch      | tp          |
//! |-----------|---------------|-------------|
//! | Kernel running | 0             | current     |
//! | User running | current       | user TLS    |
//!
//! When trap occurs:
//!   csrrw tp, sscratch, tp
//!   - from kernel: tp becomes 0 (original sscratch value)
//!   - from user: tp becomes current task (original sscratch value)
//!
//! ## PtRegs Layout (288 bytes)
//!
//! ```text
//! Offset  Field       Description
//! ------  -----       -----------
//! 0x00    epc         Program counter (sepc)
//! 0x08    ra          x1
//! 0x10    sp          x2
//! 0x18    gp          x3
//! 0x20    tp          x4
//! 0x28    t0          x5
//! 0x30    t1          x6
//! 0x38    t2          x7
//! 0x40    s0/fp       x8
//! 0x48    s1          x9
//! 0x50    a0          x10
//! ...     ...         ...
//! 0x100   status      sstatus CSR
//! 0x108   badaddr     stval CSR
//! 0x110   cause       scause CSR
//! 0x118   orig_a0     Original a0
//! ```

.section .text.trap
.align 2
.global trap_entry
.extern trap_handler
.extern asm_need_resched
.extern asm_schedule

// ============================================================================
// Per-CPU interrupt stack helper
// ============================================================================

// GET_PER_CPU_INTR_STACK - Get current CPU's interrupt stack top
//
// Usage: GET_PER_CPU_INTR_STACK dest_reg
//
// Clobbers: t0, t1, t2
//
// Implementation:
//   1. Check tp value to determine CPU ID source
//   2. If tp < TASK_STRUCT_THRESHOLD: tp = hart_id (early boot)
//   3. Otherwise: read cpu_id from task_struct.ti_cpu at offset 0x18
//   4. Calculate stack address: base + cpu_id * INTR_STACK_SIZE + INTR_STACK_SIZE
.macro GET_PER_CPU_INTR_STACK dest
    // Check if tp is a small value (early boot phase)
    li t0, TASK_STRUCT_THRESHOLD
    bltu tp, t0, .Learly_boot_stack\@

    // Validate tp is a plausible kernel virtual address (SV39).
    // Valid kernel pointers have bits [63:47] all set (upper half).
    // A corrupted tp (e.g., 0x8000000000000000) would cause a
    // recursive page fault when dereferenced below.
    srli t1, tp, 47
    li t0, 0x1FFFF
    bne t1, t0, .Learly_boot_stack\@

    // Normal operation: tp points to task_struct
    // Get CPU ID from task_struct.ti_cpu (offset 0x18)
    // IMPORTANT: Must use lw (4-byte load), NOT ld!
    // ti_cpu is AtomicI32 (4 bytes) at offset 0x18, followed by
    // state (AtomicU32) at offset 0x1c. Using ld would load both
    // fields, corrupting the CPU ID with the task state when
    // state != RUNNING (0), producing a wrong interrupt stack address.
    li t0, TASK_STRUCT_THRESHOLD
    lw t1, TASK_TI_CPU(tp)
    j .Lcalc_stack\@

.Learly_boot_stack\@:
    // Early boot / corrupted tp: tp = hart_id directly
    // When tp is corrupted (e.g., 0x8000000000000000), clamp to 0
    // as a safe fallback — uses CPU 0's interrupt stack.
    li t0, 4
    bgeu tp, t0, .Lfallback_stack\@
    mv t1, tp
    j .Lcalc_stack\@

.Lfallback_stack\@:
    // Corrupted tp — fallback to CPU 0's stack
    li t1, 0

.Lcalc_stack\@:
    // Calculate per-CPU interrupt stack top
    // stack_top = __per_cpu_intr_stacks_base + cpu_id * INTR_STACK_SIZE + INTR_STACK_SIZE
    la t0, __per_cpu_intr_stacks_base
    ld t0, 0(t0)           // t0 = base address of stacks array

    // t1 = cpu_id, calculate offset: cpu_id * INTR_STACK_SIZE
    li t2, INTR_STACK_SIZE
    mul t1, t1, t2          // t1 = cpu_id * INTR_STACK_SIZE

    add t0, t0, t1          // t0 = base + offset
    add \dest, t0, t2       // dest = stack_top = base + offset + size
.endm

// PtRegs offset constants (consistent with pt_regs.rs)
.set PT_EPC,      0x00
.set PT_RA,       0x08
.set PT_SP,       0x10
.set PT_GP,       0x18
.set PT_TP,       0x20
.set PT_T0,       0x28
.set PT_T1,       0x30
.set PT_T2,       0x38
.set PT_S0,       0x40
.set PT_S1,       0x48
.set PT_A0,       0x50
.set PT_A1,       0x58
.set PT_A2,       0x60
.set PT_A3,       0x68
.set PT_A4,       0x70
.set PT_A5,       0x78
.set PT_A6,       0x80
.set PT_A7,       0x88
.set PT_S2,       0x90
.set PT_S3,       0x98
.set PT_S4,       0xa0
.set PT_S5,       0xa8
.set PT_S6,       0xb0
.set PT_S7,       0xb8
.set PT_S8,       0xc0
.set PT_S9,       0xc8
.set PT_S10,      0xd0
.set PT_S11,      0xd8
.set PT_T3,       0xe0
.set PT_T4,       0xe8
.set PT_T5,       0xf0
.set PT_T6,       0xf8
.set PT_STATUS,   0x100
.set PT_BADADDR,  0x108
.set PT_CAUSE,    0x110
.set PT_ORIG_A0,  0x118
.set PT_SIZE,     0x120

// sstatus bits
.set SR_SPP,      0x100

// Task structure offsets (consistent with task.rs TASK_OFFSETS)
.set TASK_TI_FLAGS,      0x00    // ti_flags offset
.set TASK_TI_PREEMPT,    0x04    // ti_preempt_count offset
.set TASK_TI_KERNEL_SP,  0x08    // ti_kernel_sp offset
.set TASK_TI_USER_SP,    0x10    // ti_user_sp offset
.set TASK_TI_CPU,        0x18    // ti_cpu offset
.set TASK_STATE,         0x1C    // state offset
.set TASK_PID,           0x20    // pid offset

// Per-CPU interrupt stack constants
.set INTR_STACK_SIZE,    16384   // 16KB per CPU
.set TASK_STRUCT_THRESHOLD, 0x1000  // tp < this means early boot (tp = hart_id)
.set KERNEL_STACK_SIZE,  32768   // 32KB per task (matches config::KERNEL_STACK_SIZE)

// ============================================================================
// trap_entry - Exception/interrupt entry point
// ============================================================================
//
// sscratch detection:
//   csrrw tp, sscratch, tp  # swap tp and sscratch
//   bnez tp, .Lfrom_user    # tp != 0 means from user mode
//
// Also supports early boot phase (when sscratch is uninitialized)
//
trap_entry:
    // Step 1: Atomically swap tp and sscratch
    // - From kernel: sscratch=0, tp=current -> tp becomes 0
    // - From user: sscratch=current, tp=user_tls -> tp becomes current
    csrrw tp, sscratch, tp

    // Step 2: Detect source
    // tp == 0: from kernel mode (normal case, original sscratch value was 0)
    // tp != 0: possibly from user mode, or could be early boot phase
    beqz tp, .Lfrom_kernel

    // tp != 0, check if it's a valid task pointer
    // Valid kernel SV39 address has bits [63:47] all set (canonical form).
    // If tp is not a valid kernel address, treat as early boot (tp = hart_id).
    srli t1, tp, 47
    li t0, 0x1FFFF
    bne t1, t0, .Learly_boot

    // tp >= 0x80000000, valid task pointer, from user mode
    j .Lfrom_user

// ============================================================================
// Enter trap from kernel (normal mode)
// ============================================================================
//
// tp = 0 (from sscratch swap)
// sscratch = current task (previously saved tp value)
//
.Lfrom_kernel:
    // Restore tp = original value before csrrw swap (from sscratch)
    csrr tp, sscratch

    // Check if tp is a valid task_struct pointer or a hart_id.
    // Secondary CPUs during WFI loop have tp = hart_id (small value),
    // not a task_struct pointer.  Trying to read task_struct fields
    // from a hart_id causes a page fault.
    li t0, TASK_STRUCT_THRESHOLD
    bltu tp, t0, .Lfrom_kernel_early

    // Get per-CPU interrupt stack top (needed for .Luse_trap_stack)
    GET_PER_CPU_INTR_STACK t2

    // on_thread_stack(): Check if sp is on current task's kernel stack.
    //
    // task kernel stack: [ti_kernel_sp - KERNEL_STACK_SIZE, ti_kernel_sp)
    //
    // If sp is within these bounds, we're on the task's kernel stack
    // and can safely reuse it. Otherwise (boot stack, IRQ stack, etc.)
    // switch to the per-CPU interrupt stack.
    ld t0, TASK_TI_KERNEL_SP(tp)    // t0 = kernel stack top (ti_kernel_sp)
    beqz t0, .Luse_trap_stack       // No kernel stack → must use IRQ stack

    li t1, KERNEL_STACK_SIZE        // 32768
    sub t1, t0, t1                  // t1 = kernel stack bottom

    blt sp, t1, .Luse_trap_stack    // sp < bottom → not on task stack
    bgt sp, t0, .Luse_trap_stack    // sp > top → not on task stack

    // sp is on task kernel stack — reuse it directly
    j .Lalloc_frame_kernel

.Luse_trap_stack:
    // CRITICAL: Save original sp BEFORE switching!
    // We need to save the kernel sp, not the interrupt stack address
    mv t0, sp                 // t0 = original sp (kernel stack)
    mv sp, t2                 // sp = interrupt stack top

    // Allocate PtRegs space
    addi sp, sp, -PT_SIZE

    // Save original kernel sp (not the interrupt stack!)
    sd t0, PT_SP(sp)
    j .Lsave_kernel_regs

.Lalloc_frame_kernel:
    // Save original sp (before allocating PtRegs)
    mv t1, sp

    // Allocate PtRegs space
    addi sp, sp, -PT_SIZE

    // Save original sp first (needed for kernel trap path)
    sd t1, PT_SP(sp)

.Lsave_kernel_regs:

    // CRITICAL FIX: Save general purpose registers FIRST
    // This must be done BEFORE reading CSR, because CSR reading uses t0-t3
    // as temporary registers, which would corrupt user-space values
    sd x1,  PT_RA(sp)
    sd x3,  PT_GP(sp)
    sd x4,  PT_TP(sp)   // Save kernel tp (= current task)
    sd x5,  PT_T0(sp)
    sd x6,  PT_T1(sp)
    sd x7,  PT_T2(sp)
    sd x8,  PT_S0(sp)
    sd x9,  PT_S1(sp)
    sd x10, PT_A0(sp)
    sd x11, PT_A1(sp)
    sd x12, PT_A2(sp)
    sd x13, PT_A3(sp)
    sd x14, PT_A4(sp)
    sd x15, PT_A5(sp)
    sd x16, PT_A6(sp)
    sd x17, PT_A7(sp)
    sd x18, PT_S2(sp)
    sd x19, PT_S3(sp)
    sd x20, PT_S4(sp)
    sd x21, PT_S5(sp)
    sd x22, PT_S6(sp)
    sd x23, PT_S7(sp)
    sd x24, PT_S8(sp)
    sd x25, PT_S9(sp)
    sd x26, PT_S10(sp)
    sd x27, PT_S11(sp)
    sd x28, PT_T3(sp)
    sd x29, PT_T4(sp)
    sd x30, PT_T5(sp)
    sd x31, PT_T6(sp)

    // NOW read CSR (after all registers are saved)
    // It's safe to use t0-t3 as temporary registers now
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    j .Lcall_handler

// ============================================================================
// Early boot phase (compatible mode)
// ============================================================================
//
// tp < 0x1000, indicates sscratch uninitialized, still in early boot
// Use sstatus.SPP for detection
//
.Learly_boot:
    // Restore tp (swap caused tp to become original sscratch value)
    // In early boot phase, tp was originally hart_id, sscratch undefined
    // We need to restore tp = hart_id
    // Note: tp is now original sscratch value (could be any value)
    // But we cannot restore original tp, so use sstatus.SPP for detection

    // Use sstatus.SPP to determine source
    csrr t0, sstatus
    andi t0, t0, SR_SPP
    bnez t0, .Lfrom_kernel_early

// ============================================================================
// Enter trap from user space (early boot phase)
// ============================================================================
.Lfrom_user_early:
    // In early boot phase, only CPU 0 is running
    // Save user sp
    mv t1, sp

    // Get per-CPU interrupt stack (will use CPU 0 during early boot)
    GET_PER_CPU_INTR_STACK sp

    // Allocate PtRegs space
    addi sp, sp, -PT_SIZE

    // Save user sp
    sd t1, PT_SP(sp)

    // Save user tp (hart_id)
    sd tp, PT_TP(sp)

    // CRITICAL FIX: Save general purpose registers FIRST
    sd x1,  PT_RA(sp)
    sd x3,  PT_GP(sp)
    // tp already saved
    sd x5,  PT_T0(sp)
    sd x6,  PT_T1(sp)
    sd x7,  PT_T2(sp)
    sd x8,  PT_S0(sp)
    sd x9,  PT_S1(sp)
    sd x10, PT_A0(sp)
    sd x11, PT_A1(sp)
    sd x12, PT_A2(sp)
    sd x13, PT_A3(sp)
    sd x14, PT_A4(sp)
    sd x15, PT_A5(sp)
    sd x16, PT_A6(sp)
    sd x17, PT_A7(sp)
    sd x18, PT_S2(sp)
    sd x19, PT_S3(sp)
    sd x20, PT_S4(sp)
    sd x21, PT_S5(sp)
    sd x22, PT_S6(sp)
    sd x23, PT_S7(sp)
    sd x24, PT_S8(sp)
    sd x25, PT_S9(sp)
    sd x26, PT_S10(sp)
    sd x27, PT_S11(sp)
    sd x28, PT_T3(sp)
    sd x29, PT_T4(sp)
    sd x30, PT_T5(sp)
    sd x31, PT_T6(sp)

    // NOW read CSR (after all registers are saved)
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    j .Lcall_handler

// ============================================================================
// Enter trap from kernel (early boot phase)
// ============================================================================
.Lfrom_kernel_early:
    // Early boot phase, tp may not be valid task_struct
    // Get per-CPU interrupt stack (will use CPU 0 during early boot)
    GET_PER_CPU_INTR_STACK t2

    // Check if already on this CPU's interrupt stack
    li t1, INTR_STACK_SIZE
    sub t1, t2, t1          // t1 = stack_bottom

    blt sp, t1, .Luse_trap_stack_early
    bgt sp, t2, .Luse_trap_stack_early
    j .Lalloc_frame_kernel_early

.Luse_trap_stack_early:
    mv sp, t2

.Lalloc_frame_kernel_early:
    mv t1, sp
    addi sp, sp, -PT_SIZE
    sd t1, PT_SP(sp)

    // CRITICAL FIX: Save general purpose registers FIRST
    sd x1,  PT_RA(sp)
    sd x3,  PT_GP(sp)
    sd x4,  PT_TP(sp)   // Save kernel tp (= hart_id)
    sd x5,  PT_T0(sp)
    sd x6,  PT_T1(sp)
    sd x7,  PT_T2(sp)
    sd x8,  PT_S0(sp)
    sd x9,  PT_S1(sp)
    sd x10, PT_A0(sp)
    sd x11, PT_A1(sp)
    sd x12, PT_A2(sp)
    sd x13, PT_A3(sp)
    sd x14, PT_A4(sp)
    sd x15, PT_A5(sp)
    sd x16, PT_A6(sp)
    sd x17, PT_A7(sp)
    sd x18, PT_S2(sp)
    sd x19, PT_S3(sp)
    sd x20, PT_S4(sp)
    sd x21, PT_S5(sp)
    sd x22, PT_S6(sp)
    sd x23, PT_S7(sp)
    sd x24, PT_S8(sp)
    sd x25, PT_S9(sp)
    sd x26, PT_S10(sp)
    sd x27, PT_S11(sp)
    sd x28, PT_T3(sp)
    sd x29, PT_T4(sp)
    sd x30, PT_T5(sp)
    sd x31, PT_T6(sp)

    // NOW read CSR (after all registers are saved)
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    j .Lcall_handler

// ============================================================================
// Enter trap from user space (normal mode)
// ============================================================================
//
// tp = current task (from sscratch swap)
// sscratch = user TLS (previous tp value)
//
.Lfrom_user:
    // tp now points to current task_struct (validated by SV39 canonical check above)

    // Save user sp (before switching stack)
    mv t1, sp

    // Get kernel stack pointer from task_struct
    // task_struct.ti_kernel_sp at offset 0x08
    ld sp, TASK_TI_KERNEL_SP(tp)

    // If ti_kernel_sp is 0 or looks invalid, use dedicated trap stack
    beqz sp, .Luse_trap_stack_user
    // Sanity check: kernel stack should be in upper half (bit 63 set)
    srli t2, sp, 47
    li t0, 0x1FFFF
    bne t2, t0, .Luse_trap_stack_user

    // Allocate PtRegs space
    addi sp, sp, -PT_SIZE
    j .Lsave_user_context

.Luse_trap_stack_user:
    // Use per-CPU interrupt stack
    GET_PER_CPU_INTR_STACK sp
    addi sp, sp, -PT_SIZE

.Lsave_user_context:
    // Save user sp
    sd t1, PT_SP(sp)

    // CRITICAL FIX: Save ALL general purpose registers FIRST
    // This must be done BEFORE using any temp register for debug calls
    sd x1,  PT_RA(sp)
    sd x3,  PT_GP(sp)
    // tp will be saved later from sscratch
    sd x5,  PT_T0(sp)
    sd x6,  PT_T1(sp)
    sd x7,  PT_T2(sp)
    sd x8,  PT_S0(sp)
    sd x9,  PT_S1(sp)
    sd x10, PT_A0(sp)
    sd x11, PT_A1(sp)
    sd x12, PT_A2(sp)
    sd x13, PT_A3(sp)
    sd x14, PT_A4(sp)
    sd x15, PT_A5(sp)
    sd x16, PT_A6(sp)
    sd x17, PT_A7(sp)
    sd x18, PT_S2(sp)
    sd x19, PT_S3(sp)
    sd x20, PT_S4(sp)
    sd x21, PT_S5(sp)
    sd x22, PT_S6(sp)
    sd x23, PT_S7(sp)
    sd x24, PT_S8(sp)
    sd x25, PT_S9(sp)
    sd x26, PT_S10(sp)
    sd x27, PT_S11(sp)
    sd x28, PT_T3(sp)
    sd x29, PT_T4(sp)
    sd x30, PT_T5(sp)
    sd x31, PT_T6(sp)

    // NOW it's safe to use temp registers
    // Save user tp (now in sscratch)
    csrr t0, sscratch
    sd t0, PT_TP(sp)

    // Read CSR (safe to use t0-t3 now)
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    // tp still has current task pointer, no need to restore

    // Clear sscratch to indicate we're now in kernel mode
    // This ensures future interrupts from kernel mode are correctly detected
    csrw sscratch, zero

    j .Lcall_handler

// ============================================================================
// Call Rust trap handler
// ============================================================================
.Lcall_handler:
    // Set orig_a0 = a0 (for system call rollback)
    ld t0, PT_A0(sp)
    sd t0, PT_ORIG_A0(sp)

    // Call Rust trap_handler
    // Argument: a0 = PtRegs pointer
    mv a0, sp
    call trap_handler

// ============================================================================
// Return from trap / Return from exception
// ============================================================================
//
// ret_from_exception entry point.
// Both ret_from_exception and trap_exit point to the same code.
//
.global ret_from_exception
ret_from_exception:
trap_exit:
    // Disable interrupts
    csrci sstatus, 2

    // Clear LR/SC reservation (Load Reservation)
    //
    // RISC-V LR/SC (Load-Reserved/Store-Conditional) atomic operations
    // set a reservation on the hart. If a context switch occurs between
    // LR and SC, this reservation may affect atomic operations of the new task.
    //
    // Solution: Execute a guaranteed-to-fail SC before returning to new context to clear reservation.
    // This is safe because RISC-V allows clearing reservation at any time.
    //
    addi t2, sp, PT_SIZE      // Calculate address: sp + PT_EPC
    sc.d x0, t2, (t2)           // Execute SC (guaranteed to fail, but clears reservation)

    // Check if returning to user space (SPP=0)
    // Note: Must check before restoring registers, as signal handling may need to modify PtRegs
    ld t0, PT_STATUS(sp)
    andi t0, t0, SR_SPP
    beqz t0, .Lreturn_user

    // Return to kernel, skip signal check
    j .Lrestore_kernel_and_exit

.Lreturn_user:
    // *** CRITICAL: Save unwound kernel stack pointer to thread_info FIRST ***
    // This must be done BEFORE any code that might trigger schedule(), such as
    // signal handling.
    //
    // After context switch, sp is restored from thread.sp, which points to pt_regs.
    // We save ti_kernel_sp = sp + PT_SIZE so we can reload sp = ti_kernel_sp - PT_SIZE
    // after schedule() returns.

    addi t0, sp, PT_SIZE      // t0 = sp + PT_SIZE (unwound kernel stack pointer)
    sd t0, TASK_TI_KERNEL_SP(tp)  // Save to task_struct.ti_kernel_sp

    // Check signals and need_resched in a loop
    //
    // IMPORTANT: Unlike previous implementation, we DO NOT reload sp from ti_kernel_sp
    // after schedule() returns.
    // After schedule() returns (context switch back to this task), sp is already correct
    // because __switch_to restores it from thread.sp, which points to pt_regs.
.Lcheck_signals_and_resched:
    // Check pending signals (before restoring registers)
    //
    // Important: Must call before restoring registers, as signal handling may modify PtRegs.
    mv a0, sp
    call check_and_deliver_signals

    // NOTE: Do NOT reload sp here! After check_and_deliver_signals returns,
    // sp is already correct. Reloading from ti_kernel_sp would use a stale value.

    // Check need_resched flag
    // If need_resched is set, call schedule() and loop back
    call asm_need_resched
    beqz a0, .Lrestore_and_exit

    // need_resched - call schedule()

    // Check preempt_count — only preempt if preemptible (preempt_count == 0)
    ld t0, TASK_TI_PREEMPT(tp)
    bnez t0, .Lrestore_and_exit

    // Need resched - call schedule()
    // After schedule() returns (when this task is scheduled back in),
    // sp is restored from thread.sp by __switch_to, which points to pt_regs.
    call asm_schedule

    // CRITICAL FIX: After schedule() returns, we MUST reload sp from ti_kernel_sp.
    // For preempted tasks, thread.sp points to the kernel sp at preemption time,
    // NOT to pt_regs. We need to restore sp = ti_kernel_sp - PT_SIZE to access pt_regs.
    //
    // For newly forked children, thread.sp = pt_regs (set in copy_thread), but
    // __switch_to restores it to that value anyway. Reloading from ti_kernel_sp
    // is still correct because ti_kernel_sp = pt_regs + PT_SIZE.
    ld t0, TASK_TI_KERNEL_SP(tp)
    addi sp, t0, -PT_SIZE

    j .Lcheck_signals_and_resched

.Lrestore_and_exit:
    // CRITICAL: Reload sp from ti_kernel_sp before restoring registers.
    // This is necessary because schedule() may have been called from Rust code
    // (e.g., do_wait()), and when schedule() returns, sp is not at pt_regs.
    // We need to restore sp = ti_kernel_sp - PT_SIZE to access pt_regs.
    ld t0, TASK_TI_KERNEL_SP(tp)
    addi sp, t0, -PT_SIZE

    // Restore CSR (restore sstatus first to check SPP)
    ld t0, PT_STATUS(sp)
    ld t1, PT_EPC(sp)
    csrw sstatus, t0
    csrw sepc, t1

    // Restore general purpose registers
    ld x1,  PT_RA(sp)
    ld x3,  PT_GP(sp)
    ld x5,  PT_T0(sp)
    ld x6,  PT_T1(sp)
    ld x7,  PT_T2(sp)
    ld x8,  PT_S0(sp)
    ld x9,  PT_S1(sp)
    ld x10, PT_A0(sp)
    ld x11, PT_A1(sp)
    ld x12, PT_A2(sp)
    ld x13, PT_A3(sp)
    ld x14, PT_A4(sp)
    ld x15, PT_A5(sp)
    ld x16, PT_A6(sp)
    ld x17, PT_A7(sp)
    ld x18, PT_S2(sp)
    ld x19, PT_S3(sp)
    ld x20, PT_S4(sp)
    ld x21, PT_S5(sp)
    ld x22, PT_S6(sp)
    ld x23, PT_S7(sp)
    ld x24, PT_S8(sp)
    ld x25, PT_S9(sp)
    ld x26, PT_S10(sp)
    ld x27, PT_S11(sp)
    ld x28, PT_T3(sp)
    ld x29, PT_T4(sp)
    ld x30, PT_T5(sp)
    ld x31, PT_T6(sp)

    // Re-restore t0
    ld x5, PT_T0(sp)

    // Save current task to sscratch
    // Next trap entry can find kernel data structure
    csrw sscratch, tp

    // Restore user tp (from PtRegs)
    ld x4, PT_TP(sp)

    // Memory barrier to ensure all register loads complete before sret
    fence iorw, iorw

    // Restore user sp (from PtRegs)
    ld x2, PT_SP(sp)

    sret

.Lrestore_kernel_and_exit:
    // CRITICAL: Clear sscratch to 0 before returning to kernel mode.
    // Without this, the next kernel-mode trap will incorrectly enter
    // .Lfrom_user (because csrrw tp,sscratch,tp will see tp != 0).
    csrw sscratch, zero

    // Check if tp is a valid task_struct pointer.
    // During early boot on secondary CPUs, tp = hart_id (small value).
    // Accessing TASK_TI_* fields through a non-pointer tp would fault.
    // Skip the schedule/ti_kernel_sp logic and go straight to register restore.
    li t0, TASK_STRUCT_THRESHOLD
    bltu tp, t0, .Lrestore_kernel_csrs

    // Save pt_regs pointer to ti_kernel_sp before calling schedule().
    // schedule() may context_switch, changing sp to the new task's stack.
    // After returning, sp no longer points to our pt_regs, so we must
    // reload from ti_kernel_sp.  This matches the .Lreturn_user pattern.
    addi t0, sp, PT_SIZE
    sd   t0, TASK_TI_KERNEL_SP(tp)

    // Check need_resched for kernel preemption.
    // This is critical for idle tasks: when a timer interrupt wakes WFI,
    // the timer handler cannot call schedule() (preemptible()=false in IRQ).
    // Without this check, idle loops in WFI forever even though tasks are runnable.
.Lcheck_resched_kernel:
    call asm_need_resched
    beqz a0, .Lrestore_kernel_csrs

    // Check preempt_count — only preempt if not in IRQ/softirq context
    ld t0, TASK_TI_PREEMPT(tp)
    bnez t0, .Lrestore_kernel_csrs

    // Need reschedule — call schedule().
    //
    // Re-enable interrupts before calling schedule(). The ret_from_exception
    // path cleared SIE (csrci sstatus, 2). Without re-enabling, schedule()
    // saves SIE=0 via lock_irqsave. After context_switch, the new task's
    // __schedule() restores its own saved flags — which may be SIE=0 if the
    // new task was in syscall context, leaving it with interrupts disabled
    // until sret and preventing further timer interrupts.
    //
    // This matches Linux's preempt_schedule_irq() which explicitly enables
    // interrupts before calling __schedule().
    csrsi sstatus, 2

    call asm_schedule

    // Disable interrupts again for the rest of trap return
    csrci sstatus, 2

    // Reload sp from ti_kernel_sp (schedule() may have context-switched
    // to a different task and back, so sp changed).
    ld t0, TASK_TI_KERNEL_SP(tp)
    addi sp, t0, -PT_SIZE

    j .Lcheck_resched_kernel

.Lrestore_kernel_csrs:
    // Restore CSR
    ld t0, PT_STATUS(sp)
    ld t1, PT_EPC(sp)
    csrw sstatus, t0
    csrw sepc, t1

    // Restore general purpose registers
    ld x1,  PT_RA(sp)
    ld x3,  PT_GP(sp)
    ld x4,  PT_TP(sp)
    ld x5,  PT_T0(sp)
    ld x6,  PT_T1(sp)
    ld x7,  PT_T2(sp)
    // s0 and s1 restored last
    ld x28, PT_T3(sp)
    ld x29, PT_T4(sp)
    ld x30, PT_T5(sp)
    ld x31, PT_T6(sp)
    ld x18, PT_S2(sp)
    ld x19, PT_S3(sp)
    ld x20, PT_S4(sp)
    ld x21, PT_S5(sp)
    ld x22, PT_S6(sp)
    ld x23, PT_S7(sp)
    ld x24, PT_S8(sp)
    ld x25, PT_S9(sp)
    ld x26, PT_S10(sp)
    ld x27, PT_S11(sp)
    ld x8,  PT_S0(sp)
    ld x9,  PT_S1(sp)
    ld x10, PT_A0(sp)
    ld x11, PT_A1(sp)
    ld x12, PT_A2(sp)
    ld x13, PT_A3(sp)
    ld x14, PT_A4(sp)
    ld x15, PT_A5(sp)
    ld x16, PT_A6(sp)
    ld x17, PT_A7(sp)

    // Restore original kernel stack pointer
    ld x2, PT_SP(sp)

    // Return from trap to kernel mode
    sret

.global trap_entry_end
trap_entry_end:

// ============================================================================
// ret_from_fork_user_asm - Fork child entry (user mode)
// ============================================================================
//
// This is the entry point for newly forked user processes.
// Called when child process is first scheduled in via context_switch.
//
// Register state at entry:
//   a0 = prev task pointer (passed from __switch_to)
//   tp = current task pointer (child)
//   sp = pt_regs pointer (set by copy_thread)
//
//
.extern schedule_tail
.extern ret_from_fork_user
.global ret_from_fork_user_asm
ret_from_fork_user_asm:
    // a0 = prev task pointer (from __switch_to)
    // Save prev task pointer on stack
    addi sp, sp, -32
    sd ra, 0(sp)
    sd a0, 8(sp)

    // Call schedule_tail with prev task pointer
    // a0 already contains prev task pointer from __switch_to
    call schedule_tail

    // Restore ra
    ld ra, 0(sp)
    addi sp, sp, 32

    // Pass pt_regs pointer to ret_from_fork_user
    // Note: At this point, sp points to pt_regs (set by thread.sp)
    mv a0, sp              // a0 = pt_regs pointer
    call ret_from_fork_user

    // Jump to common exception return path
    j ret_from_exception

.size ret_from_fork_user_asm, . - ret_from_fork_user_asm

// ============================================================================
// ret_from_fork_kernel_asm - Fork child entry (kernel thread)
// ============================================================================
//
// Entry point for kernel threads created with kernel_thread().
//
// Register state at entry:
//   a0 = prev task pointer
//   tp = current task pointer
//   sp = pt_regs pointer
//   s0 = fn (kernel thread function)
//   s1 = fn_arg (argument to fn)
//
//
.extern ret_from_fork_kernel
.global ret_from_fork_kernel_asm
ret_from_fork_kernel_asm:
    // Call schedule_tail(prev)
    call schedule_tail

    // Prepare arguments for ret_from_fork_kernel
    mv a0, s1              // a0 = fn_arg
    mv a1, s0              // a1 = fn
    mv a2, sp              // a2 = pt_regs
    call ret_from_fork_kernel

    // Jump to common exception return path
    j ret_from_exception

.size ret_from_fork_kernel_asm, . - ret_from_fork_kernel_asm

// ============================================================================
// Legacy alias for compatibility
// ============================================================================
.global ret_from_fork
ret_from_fork:
    j ret_from_fork_user_asm

// ============================================================================
// Per-CPU interrupt stack pointer (defined in smp.rs)
// ============================================================================
.section .data
.align 8
.global __per_cpu_intr_stacks_base
__per_cpu_intr_stacks_base:
    .8byte 0  // Will be initialized by Rust code
