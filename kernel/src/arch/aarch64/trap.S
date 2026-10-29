// MIT License
//
// Copyright (c) 2026 Fei Wang
//
// AArch64 trap entry — VBAR_EL1 vector table + full PtRegs frame
// protocol (stage 2).
//
// ## Frame protocol
//
// Every live vector pushes the full PtRegs (S_FRAME_SIZE bytes, see
// pt_regs.rs) below the current SP and calls the Rust dispatcher
// `aarch64_trap_handler(regs)` with x0 = frame base and SP 16-aligned.
//
// SP discipline (the aarch64 replacement for the riscv64 sscratch/tp
// dance — none is needed: TPIDR_EL1 holds the current Task* at ALL
// times, and SP_EL1 holds the current task's kernel-stack top while at
// EL0):
//
//   EL0 running:  SP_EL0 = user sp; SP_EL1 = task kernel-stack top.
//                 A trap switches to SP_EL1 (hardware) and the frame is
//                 pushed at stack_top - S_FRAME_SIZE — the outermost
//                 frame position, the same place generic fork seeds.
//   EL1 running:  sp is wherever the kernel was; the frame is pushed
//                 below it (nesting falls out naturally).
//
// SPSR_EL1.M[3:2] distinguishes the two sources at save/restore time:
// 0b00 = EL0 (restore via sp_el0 and unwind SP_EL1 back to the stack
// top), 0b01 = EL1 (restore sp directly).
//
// ESR policy: synchronous vectors store the real ESR_EL1; IRQ, FIQ and
// SError vectors store the pseudo-causes (64 / 65 / 17) that
// Cause::from_cause() maps to Cause::Irq / Fiq / SError — the same
// encoding riscv64 uses for its scause interrupts.
//
// All frame offsets and the frame size are interpolated from
// pt_regs.rs (single source of truth) via the global_asm! in trap.rs
// (the S_*/TI_*/KSTACK_SIZE placeholders below), then re-published
// as assembler symbols so the body reads plain.

.equ S_FRAME_SIZE, {S_FRAME_SIZE}
.equ S_X0, {S_X0}
.equ S_X8, {S_X8}
.equ S_X20, {S_X20}
.equ S_X21, {S_X21}
.equ S_X22, {S_X22}
.equ S_X30, {S_X30}
.equ S_SP, {S_SP}
.equ S_PC, {S_PC}
.equ S_PSTATE, {S_PSTATE}
.equ S_ESR, {S_ESR}
.equ S_FAR, {S_FAR}
.equ S_ORIG_X0, {S_ORIG_X0}
.equ S_SYSCALLNO, {S_SYSCALLNO}
.equ TI_PREEMPT, {TI_PREEMPT}
.equ TI_KSTACK_BOTTOM, {TI_KSTACK_BOTTOM}
.equ KSTACK_SIZE, {KSTACK_SIZE}
// Offset of the interrupted x22's frame slot relative to the restored
// EL1 sp (frame_base + S_FRAME_SIZE): negative — read after the switch.
.equ S_X22_POST, S_X22 - S_FRAME_SIZE

// ---------------------------------------------------------------------------
// Vector table: 16 slots, 0x80 apart, 2KB-aligned base (VBAR_EL1).
// ---------------------------------------------------------------------------

.macro VEC, handler
    .align 7
    b \handler
.endm

.section .text.aarch64_vectors, "ax"
.align 11
.global aarch64_vector_table
aarch64_vector_table:
    VEC v_bad_sp0_sync      // 0x000 sync, current EL, SP0   (never: EL1 runs SP_EL1)
    VEC v_bad_sp0_irq       // 0x080 irq,  current EL, SP0
    VEC v_bad_sp0_fiq       // 0x100 fiq,  current EL, SP0
    VEC v_bad_sp0_serr      // 0x180 serr, current EL, SP0
    VEC el1_sync            // 0x200 sync, current EL, SPx   (EL1 kernel trap)
    VEC el1_irq             // 0x280 irq,  current EL, SPx
    VEC el1_fiq             // 0x300 fiq,  current EL, SPx
    VEC el1_serr            // 0x380 serr, current EL, SPx
    VEC el0_sync            // 0x400 sync, lower EL, AArch64 (user trap: SVC, aborts)
    VEC el0_irq             // 0x480 irq,  lower EL, AArch64
    VEC el0_fiq             // 0x500 fiq,  lower EL, AArch64
    VEC el0_serr            // 0x580 serr, lower EL, AArch64
    VEC v_bad_a32_sync      // 0x600 sync, lower EL, AArch32 (never: no 32-bit userspace)
    VEC v_bad_a32_irq       // 0x680 irq,  lower EL, AArch32
    VEC v_bad_a32_fiq       // 0x700 fiq,  lower EL, AArch32
    VEC v_bad_a32_serr      // 0x780 serr, lower EL, AArch32

// ---------------------------------------------------------------------------
// Frame save (shared by all live vectors).
//
//   \class  0  -> store the real ESR_EL1 (synchronous exception)
//   \class >0  -> store the pseudo-cause constant (IRQ 64 / FIQ 65 /
//                 SError 17) — ESR_EL1 is UNK on those vectors
//   \exit     label branched to after aarch64_trap_handler returns:
//             aarch64_exit_to_user (EL0 sources) or restore_and_eret
//             (EL1 sources — kernel return takes no signal/resched detour)
//
// Clobbers x21-x23 only AFTER their frame slots are written. No GPR is
// touched before the save block — the frame must capture entry state.
// SP stays 16-aligned throughout (S_FRAME_SIZE is a 16 multiple).
// ---------------------------------------------------------------------------
.macro TRAP_BODY, class, exit
    sub     sp, sp, #S_FRAME_SIZE
    stp     x0, x1,  [sp, #16*0]
    stp     x2, x3,  [sp, #16*1]
    stp     x4, x5,  [sp, #16*2]
    stp     x6, x7,  [sp, #16*3]
    stp     x8, x9,  [sp, #16*4]
    stp     x10, x11, [sp, #16*5]
    stp     x12, x13, [sp, #16*6]
    stp     x14, x15, [sp, #16*7]
    stp     x16, x17, [sp, #16*8]
    stp     x18, x19, [sp, #16*9]
    stp     x20, x21, [sp, #16*10]
    stp     x22, x23, [sp, #16*11]
    stp     x24, x25, [sp, #16*12]
    stp     x26, x27, [sp, #16*13]
    stp     x28, x29, [sp, #16*14]
    str     x30, [sp, #S_X30]

    mrs     x21, spsr_el1
    mrs     x22, esr_el1
    mrs     x23, far_el1
    str     x21, [sp, #S_PSTATE]
    mrs     x21, elr_el1
    str     x21, [sp, #S_PC]
.if \class == 0
    str     x22, [sp, #S_ESR]        // synchronous: real syndrome
.else
    mov     x22, #\class
    str     x22, [sp, #S_ESR]        // IRQ/FIQ/SError: pseudo-cause
.endif
    str     x23, [sp, #S_FAR]

    // orig_x0 / syscallno snapshots (riscv64 orig_a0 / a7 parity)
    ldr     x22, [sp, #S_X0]
    str     x22, [sp, #S_ORIG_X0]
    ldr     x22, [sp, #S_X8]
    str     x22, [sp, #S_SYSCALLNO]

    // frame->sp: user sp (sp_el0) or the interrupted EL1 sp
    tbz     x21, #2, 990f            // SPSR.M[3:2] == 0b00 -> EL0
    add     x22, sp, #S_FRAME_SIZE
    str     x22, [sp, #S_SP]
    b       991f
990:
    mrs     x22, sp_el0
    str     x22, [sp, #S_SP]
991:
    mov     x0, sp
    bl      aarch64_trap_handler
    b       \exit
.endm

// EL1 (kernel) entries — straight restore, no user-exit detour
el1_sync:
    TRAP_BODY 0, restore_and_eret
el1_irq:
    TRAP_BODY 64, restore_and_eret
el1_fiq:
    TRAP_BODY 65, restore_and_eret
el1_serr:
    TRAP_BODY 17, restore_and_eret

// EL0 (user) entries — through the signal/reschedule exit loop
el0_sync:
    TRAP_BODY 0, aarch64_exit_to_user
el0_irq:
    TRAP_BODY 64, aarch64_exit_to_user
el0_fiq:
    TRAP_BODY 65, aarch64_exit_to_user
el0_serr:
    TRAP_BODY 17, aarch64_exit_to_user

// ---------------------------------------------------------------------------
// Bad vectors: dump and park. Nothing may run in these modes (EL1 uses
// SP_EL1 exclusively; no AArch32 state exists). The stage-1 dumper
// (aarch64_trap_dump, trap.rs) prints ESR/ELR/FAR/SPSR over PL011.
// ---------------------------------------------------------------------------
v_bad_sp0_sync:
v_bad_sp0_irq:
v_bad_sp0_fiq:
v_bad_sp0_serr:
v_bad_a32_sync:
v_bad_a32_irq:
v_bad_a32_fiq:
v_bad_a32_serr:
    bl      aarch64_trap_dump
992:
    wfe
    b       992b

// ---------------------------------------------------------------------------
// User exit path: pending-signal delivery + need_resched loop, then the
// frame restore. Mirrors riscv64's .Lcheck_signals_and_resched.
//
// On entry sp = frame base. After asm_schedule returns we resume on the
// stack __switch_to froze (mid-stack), so the outer frame is re-anchored
// from the task's own kernel-stack bottom (the riscv64 ti_kernel_sp
// reload; bottom + KSTACK_SIZE = top) before looping. TPIDR_EL1 names
// the task throughout.
// ---------------------------------------------------------------------------

.global aarch64_exit_to_user
aarch64_exit_to_user:
    mov     x0, sp
    bl      check_and_deliver_signals
    bl      asm_need_resched
    cbz     x0, restore_and_eret

    // Preempt only if preempt_count == 0 (a user return holds no
    // kernel locks — the riscv64 USER-path gate; load width matters:
    // ti_preempt_count is an i32, a wide load straddles the neighbour
    // field and silently disables preemption machine-wide).
    mrs     x20, tpidr_el1           // x20 = current Task*
    cbz     x20, restore_and_eret    // early boot: no task, no preemption
    ldr     w21, [x20, #TI_PREEMPT]
    cbnz    w21, restore_and_eret

    bl      asm_schedule
    // __schedule returns with interrupts enabled (restore_irq(true) at
    // its tail) — re-mask for the rest of the trap return; eret
    // restores the user's DAIF from the frame's pstate.
    msr     daifset, #0xf
    // Re-anchor sp at the outermost frame: stack_top - S_FRAME_SIZE.
    mrs     x20, tpidr_el1
    cbz     x20, restore_and_eret    // no task: keep the live frame
    ldr     x21, [x20, #TI_KSTACK_BOTTOM]
    cbz     x21, restore_and_eret    // no stack record: frame is live already
    mov     x22, #KSTACK_SIZE
    add     x21, x21, x22            // stack top
    sub     sp, x21, #S_FRAME_SIZE   // outer frame base
    b       aarch64_exit_to_user

// ---------------------------------------------------------------------------
// Frame restore + eret (shared tail).
//
//   EL0 return: SP_EL0 = frame->sp, SP_EL1 unwound to the kernel-stack
//               top (so the NEXT el0 trap frames at
//               stack_top - S_FRAME_SIZE again), elr/spsr from the frame.
//   EL1 return: sp = frame->sp directly (mid-stack nesting).
// ---------------------------------------------------------------------------
.global restore_and_eret
restore_and_eret:
    // Pull the exit triple out of the frame BEFORE their GPR slots are
    // restored over them.
    ldr     x20, [sp, #S_PSTATE]
    ldr     x21, [sp, #S_PC]
    ldr     x22, [sp, #S_SP]         // saved sp (user or interrupted EL1)

    // R74 twin (garbage-frame gate): a user frame with pc==0 or a
    // sub-0x1000 sp is a scribbled/stale frame — restoring it jumps to
    // 0 with a junk stack. Kill the task instead (riscv64 r74d shape).
    tbnz    x20, #2, 993f            // kernel (EL1) frames skip the gate
    cbz     x21, aarch64_bad_user_frame
    cmp     x22, #0x1000
    b.lo    aarch64_bad_user_frame
993:
    // Restore the GPR file. x20/x21/x22 hold the exit triple; their own
    // slots are reloaded last, just before sp is switched.
    ldp     x0, x1,  [sp, #16*0]
    ldp     x2, x3,  [sp, #16*1]
    ldp     x4, x5,  [sp, #16*2]
    ldp     x6, x7,  [sp, #16*3]
    ldp     x8, x9,  [sp, #16*4]
    ldp     x10, x11, [sp, #16*5]
    ldp     x12, x13, [sp, #16*6]
    ldp     x14, x15, [sp, #16*7]
    ldp     x16, x17, [sp, #16*8]
    ldp     x18, x19, [sp, #16*9]
    ldp     x23, x24, [sp, #16*11]
    ldp     x25, x26, [sp, #16*12]
    ldp     x27, x28, [sp, #16*13]
    ldp     x29, x30, [sp, #16*14]

    msr     elr_el1, x21
    msr     spsr_el1, x20
    tbnz    x20, #2, el1_return_tail // M[3:2] bit 2 set -> EL1 return

    // ---- EL0 tail: user sp via SP_EL0, SP_EL1 unwound to stack top ----
    msr     sp_el0, x22
    ldr     x20, [sp, #S_X20]
    ldr     x21, [sp, #S_X21]
    ldr     x22, [sp, #S_X22]
    add     sp, sp, #S_FRAME_SIZE    // SP_EL1 = kernel-stack top
    eret

el1_return_tail:
    ldr     x20, [sp, #S_X20]
    ldr     x21, [sp, #S_X21]
    mov     sp, x22                  // back to the interrupted EL1 sp
    // x22's own slot sits at S_X22 from the (now-abandoned) frame base
    // = S_X22 - S_FRAME_SIZE from the new sp.
    ldr     x22, [sp, #S_X22_POST]
    eret

// ---------------------------------------------------------------------------
// ret_from_fork — the first switch-in of a forked child "returns" here
// from __switch_to with sp = child's pt_regs (thread.sp seeded by the
// generic copy_thread) and x0 = the task that switched out (schedule_tail
// argument, riscv64 parity). Runs the common user-exit loop into the
// seeded frame: the child erets into EL0 with x0 = 0 (fork return).
// ---------------------------------------------------------------------------
.section .text.ret_from_fork, "ax"
.align 4
.global ret_from_fork
ret_from_fork:
    bl      schedule_tail
    b       aarch64_exit_to_user
