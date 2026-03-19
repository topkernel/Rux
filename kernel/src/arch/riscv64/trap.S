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
.extern kernel_lock_acquire
.extern kernel_lock_release

// ============================================================================
// Kernel big lock assembly macros
// ============================================================================

// KERNEL_LOCK_ACQUIRE - Acquire kernel big lock
//
// Usage: KERNEL_LOCK_ACQUIRE
//
// Clobbers: t0, t1, t2
//
// Implementation:
//   Use amoswap.w.aq instruction for atomic swap with acquire semantics
//   If lock is held, spin wait
.macro KERNEL_LOCK_ACQUIRE
    la t0, KERNEL_LOCK
    li t2, 1
1:
    amoswap.w.aq t1, t2, (t0)    // Atomic swap, return original value to t1
    bnez t1, 1b                   // If originally non-zero, spin wait
.endm

// KERNEL_LOCK_RELEASE - Release kernel big lock
//
// Usage: KERNEL_LOCK_RELEASE
//
// Clobbers: t0
//
// Implementation:
//   Use amoswap.w.rl instruction to atomically write 0 with release semantics
.macro KERNEL_LOCK_RELEASE
    la t0, KERNEL_LOCK
    amoswap.w.rl zero, zero, (t0)  // Atomically write 0 with release semantics
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
    // Valid task pointer should be >= 0x80000000 (kernel space)
    // If tp < 0x80000000, it's early boot phase (tp = hart_id)
    li t0, 0x80000000
    bltu tp, t0, .Learly_boot

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
    // Restore tp = current task (from sscratch)
    csrr tp, sscratch

    // Check if already on trap stack
    la t1, __kernel_trap_stack
    la t2, __kernel_trap_stack_top

    // If sp is not within trap stack range, switch stack
    blt sp, t1, .Luse_trap_stack
    bgt sp, t2, .Luse_trap_stack

    // Already on trap stack, allocate space directly
    j .Lalloc_frame_kernel

.Luse_trap_stack:
    // Switch to trap stack
    mv sp, t2

.Lalloc_frame_kernel:
    // Save original sp (before allocating PtRegs)
    mv t1, sp

    // Allocate PtRegs space
    addi sp, sp, -PT_SIZE

    // Save original sp
    sd t1, PT_SP(sp)

    // Save CSR
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    // Save general purpose registers
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
    // In early boot phase, tp = hart_id
    // Save user sp
    mv t1, sp

    // Get kernel stack pointer
    la sp, __kernel_trap_stack_top

    // Allocate PtRegs space
    addi sp, sp, -PT_SIZE

    // Save user sp
    sd t1, PT_SP(sp)

    // Save user tp (hart_id)
    sd tp, PT_TP(sp)

    // Save CSR
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    // Save general purpose registers
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

    j .Lcall_handler

// ============================================================================
// Enter trap from kernel (early boot phase)
// ============================================================================
.Lfrom_kernel_early:
    // Early boot phase, tp = hart_id
    // Check if already on trap stack
    la t1, __kernel_trap_stack
    la t2, __kernel_trap_stack_top

    blt sp, t1, .Luse_trap_stack_early
    bgt sp, t2, .Luse_trap_stack_early
    j .Lalloc_frame_kernel_early

.Luse_trap_stack_early:
    mv sp, t2

.Lalloc_frame_kernel_early:
    mv t1, sp
    addi sp, sp, -PT_SIZE
    sd t1, PT_SP(sp)

    // Save CSR
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    // Save general purpose registers
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

    j .Lcall_handler

// ============================================================================
// Enter trap from user space (normal mode)
// ============================================================================
//
// tp = current task (from sscratch swap)
// sscratch = user TLS (previous tp value)
//
.Lfrom_user:
    // tp now points to current task_struct

    // Save user sp (before switching stack)
    mv t1, sp

    // Get kernel stack pointer from task_struct
    // task_struct.ti_kernel_sp at offset 0x10
    ld sp, TASK_TI_KERNEL_SP(tp)

    // If ti_kernel_sp is 0, use dedicated trap stack
    beqz sp, .Luse_trap_stack_user

    // Allocate PtRegs space
    addi sp, sp, -PT_SIZE
    j .Lsave_user_context

.Luse_trap_stack_user:
    la sp, __kernel_trap_stack_top
    addi sp, sp, -PT_SIZE

.Lsave_user_context:
    // Save user sp
    sd t1, PT_SP(sp)

    // Save user tp (now in sscratch)
    csrr t0, sscratch
    sd t0, PT_TP(sp)

    // Save current task pointer to sscratch
    // So on return we can restore tp = current
    csrw sscratch, tp

    // Save CSR
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    // Save general purpose registers
    sd x1,  PT_RA(sp)
    sd x3,  PT_GP(sp)
    // tp already saved (user TLS)
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

    // Restore tp = current task (from sscratch)
    csrr tp, sscratch

    // Acquire kernel big lock (entering kernel from user mode)
    KERNEL_LOCK_ACQUIRE

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
// Return from trap
// ============================================================================
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
    // Check pending signals (before restoring registers)
    //
    // Important: Must call before restoring registers, as signal handling may modify PtRegs
    // After call, PtRegs is still valid since we pass a pointer
    mv a0, sp
    call check_and_deliver_signals

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

    // Release kernel big lock (returning to user mode)
    KERNEL_LOCK_RELEASE

    // Save current task to sscratch
    // Next trap entry can find kernel data structure
    csrw sscratch, tp

    // Restore user tp (from PtRegs)
    ld x4, PT_TP(sp)

    // Restore user sp (from PtRegs)
    ld x2, PT_SP(sp)

    sret

.Lrestore_kernel_and_exit:
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

    // Restore original kernel stack pointer
    ld x2, PT_SP(sp)

    // Return from trap to kernel mode
    sret

.global trap_entry_end
trap_entry_end:

// ============================================================================
// ret_from_fork - fork child process entry point
// ============================================================================
//
// When a child process created by fork is first scheduled, execution starts here
// tp = current task (child process)
//
.extern schedule_tail
.extern get_prev_task
.global ret_from_fork
ret_from_fork:
    // Disable interrupts
    csrci sstatus, 2

    // Get prev task from per-CPU variable (set by context_switch)
    // This replaces the old approach of passing via s1, which corrupted s1
    call get_prev_task    // a0 = get_prev_task()

    // Call schedule_tail to clean up previous task
    // Argument: a0 = prev task (from get_prev_task)
    call schedule_tail

    // Restore CSR
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
    ld x10, PT_A0(sp)      // a0 = 0 (fork child return value)
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

    // Check if returning to user space
    ld t0, PT_STATUS(sp)
    andi t0, t0, SR_SPP
    beqz t0, .Lret_from_fork_user

    // Return to kernel (kernel thread)
    ld x4, PT_TP(sp)
    ld x2, PT_SP(sp)
    sret

.Lret_from_fork_user:
    // Return to user space
    // Fork child needs to acquire lock first, then release lock to return to user mode
    // Because fork child returns from schedule(), didn't go through trap entry to acquire lock
    KERNEL_LOCK_ACQUIRE

    // Release kernel big lock (returning to user mode)
    KERNEL_LOCK_RELEASE

    // Set sscratch = current task (tp)
    csrw sscratch, tp

    // Restore user tp
    ld x4, PT_TP(sp)

    // Restore user sp
    ld x2, PT_SP(sp)

    sret

// ============================================================================
// Kernel trap stack (16KB per CPU, currently supports 4 CPUs)
// ============================================================================
.section .bss
.align 16
__kernel_trap_stack:
    .space 16384 * 4
__kernel_trap_stack_top:
