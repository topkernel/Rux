/* MIT License
 *
 * Copyright (c) 2026 Fei Wang
 *
 * x86_64 trap entry/exit + syscall entry (IDT stubs).
 *
 * ## Frame layout (uniform 168 bytes == sizeof(PtRegs))
 *
 * In 64-bit mode the CPU pushes SS:RSP on EVERY interrupt/exception
 * entry (even same-CPL; Intel SDM Vol 3A 6.14) and IRETQ always pops all
 * five hardware words.  So every stub only has to:
 *
 *   1. push a dummy 0 for vectors WITHOUT a hardware error code (vectors
 *      8,10,11,12,13,14,17,21 push the real code) — the slot overlays
 *      pt_regs.orig_rax at 0x78;
 *   2. push the 15 GPRs so the struct overlays the stack 1:1:
 *
 *   offset  field      pushed by
 *   ------  -----      -------------------------------
 *   0x00    r15        stub (last)
 *   ...     ...        stub
 *   0x70    rdi        stub (first)
 *   0x78    orig_rax   stub dummy / hw error code / syscall nr
 *   0x80    rip        CPU (stub for the SYSCALL path)
 *   0x88    cs         CPU (stub)
 *   0x90    rflags     CPU (stub)
 *   0x98    rsp        CPU (stub)
 *   0xa0    ss         CPU (stub)
 *
 * User-mode entries: the CPU loaded TSS.rsp0 before pushing, so the
 * frame lands exactly at stack_top - 168 — the same contract the riscv64
 * twin keeps and `Task::pt_regs()` computes.  Kernel-mode entries push
 * on the current stack (nested frames, like Linux).
 *
 * ## C ABI alignment
 *
 * Kernel-mode interrupts arrive with arbitrary RSP alignment (the
 * interrupted code is mid-function), so the stub parks the frame
 * pointer in RBP and aligns RSP down to 16 before calling Rust.  RBP is
 * safe to reuse: its interrupted value is already stored in the frame.
 * The rustc target x86_64-unknown-none disables the red zone, so the
 * CPU-pushed frame cannot clobber live stack slots below the
 * interrupted RSP.
 *
 * ## No swapgs
 *
 * The kernel never uses a user GS base (single kernel GS everywhere),
 * so entries do not swap GS.  SYSCALL therefore also cannot recover the
 * user stack from GS; it stashes RSP in TSS.sp2 instead (the Linux
 * entry_SYSCALL_64 trick).
 */

.section .text.trap
.align 8

/* pt_regs offsets — cross-checked by the static asserts in trap.rs */
.set PT_R15,      0x00
.set PT_R14,      0x08
.set PT_R13,      0x10
.set PT_R12,      0x18
.set PT_RBP,      0x20
.set PT_RBX,      0x28
.set PT_R11,      0x30
.set PT_R10,      0x38
.set PT_R9,       0x40
.set PT_R8,       0x48
.set PT_RAX,      0x50
.set PT_RCX,      0x58
.set PT_RDX,      0x60
.set PT_RSI,      0x68
.set PT_RDI,      0x70
.set PT_ORIG_RAX, 0x78
.set PT_RIP,      0x80
.set PT_CS,       0x88
.set PT_RFLAGS,   0x90
.set PT_RSP,      0x98
.set PT_SS,       0xa0
.set PT_SIZE,     0xa8

/* selector values (GDT indices pinned in trap.rs) */
.set __KERNEL_CS, 0x08
.set __USER_CS,   0x33
.set __USER_DS,   0x2b

.set SYSCALL_VECTOR, 0x80

/* TSS field offsets (see Tss in trap.rs) */
.set TSS_RSP0, 0x04
.set TSS_SP2,  0x14

.extern trap_handler
.extern trap_exit
.extern x86_double_fault_dump

/* ============================================================================
 * Exception/IRQ stub macro
 *
 * \vec: vector number (also passed to trap_handler in RDX)
 * \ec:  1 when the CPU pushed an error code (8,10,11,12,13,14,17,21)
 * ============================================================================
 */
.macro TRAP_STUB vec, ec=0
.global stub_\vec
.hidden stub_\vec
stub_\vec:
    .if \ec == 0
    pushq $0                     /* dummy error code -> orig_rax slot */
    .endif
    /* GPRs in reverse struct order: rdi ends highest (0x70), r15 lowest */
    pushq %rdi
    pushq %rsi
    pushq %rdx
    pushq %rcx
    pushq %rax
    pushq %r8
    pushq %r9
    pushq %r10
    pushq %r11
    pushq %rbx
    pushq %rbp
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    /* frame complete; RBP is dead (saved above) — park frame ptr in it */
    movq %rsp, %rbp
    andq $-16, %rsp              /* SysV: RSP 16-aligned at call sites */
    movq %rbp, %rdi              /* arg0: regs */
    xorl %esi, %esi              /* arg1: cpu id (single CPU bring-up) */
    movq $\vec, %rdx             /* arg2: vector */
    call trap_handler
    movq %rbp, %rsp              /* unwind to the exact frame */
    jmp trap_exit
.endm

/* ---- CPU exceptions 0..31 (error-code vectors: 8,10-14,17,21) ---- */
TRAP_STUB 0
TRAP_STUB 1
TRAP_STUB 2
TRAP_STUB 3
TRAP_STUB 4
TRAP_STUB 5
TRAP_STUB 6
TRAP_STUB 7
/* vector 8 (double fault) has its own IST stub below */
TRAP_STUB 9
TRAP_STUB 10, 1
TRAP_STUB 11, 1
TRAP_STUB 12, 1
TRAP_STUB 13, 1
TRAP_STUB 14, 1
TRAP_STUB 15
TRAP_STUB 16
TRAP_STUB 17, 1
TRAP_STUB 18
TRAP_STUB 19
TRAP_STUB 20
TRAP_STUB 21, 1
TRAP_STUB 22
TRAP_STUB 23
TRAP_STUB 24
TRAP_STUB 25
TRAP_STUB 26
TRAP_STUB 27
TRAP_STUB 28
TRAP_STUB 29
TRAP_STUB 30
TRAP_STUB 31

/* ---- PIC IRQ vectors 32..47 ---- */
TRAP_STUB 32
TRAP_STUB 33
TRAP_STUB 34
TRAP_STUB 35
TRAP_STUB 36
TRAP_STUB 37
TRAP_STUB 38
TRAP_STUB 39
TRAP_STUB 40
TRAP_STUB 41
TRAP_STUB 42
TRAP_STUB 43
TRAP_STUB 44
TRAP_STUB 45
TRAP_STUB 46
TRAP_STUB 47

/* ---- vectors 48..255: shared catch-all (unexpected IRQ/exception) ----
 * Passed as vector 255; trap_handler routes it through the conservative
 * external-IRQ EOI path (early-Linux style). */
.global stub_spurious
.hidden stub_spurious
stub_spurious:
    pushq $0
    pushq %rdi
    pushq %rsi
    pushq %rdx
    pushq %rcx
    pushq %rax
    pushq %r8
    pushq %r9
    pushq %r10
    pushq %r11
    pushq %rbx
    pushq %rbp
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, %rbp
    andq $-16, %rsp
    movq %rbp, %rdi
    xorl %esi, %esi
    movq $255, %rdx
    call trap_handler
    movq %rbp, %rsp
    jmp trap_exit

/* ============================================================================
 * Double fault (#DF, vector 8) — IST1
 *
 * The kernel is unrecoverable here: no pt_regs, no unwinding, no locks.
 * The IST switch gave us a fresh 16K stack; just raw-print and halt.
 * x86_double_fault_dump must not take locks or allocate (it uses the
 * same raw serial path as the dfx taskdump helpers).
 * ============================================================================
 */
.global stub_8
.hidden stub_8
stub_8:
    call x86_double_fault_dump
1:  cli
    hlt
    jmp 1b

/* ============================================================================
 * SYSCALL entry (LSTAR target)
 *
 * On entry: RCX = user rip, R11 = user rflags, RSP = user rsp,
 * CS/SS loaded from STAR (0x08 / 0x10), RFLAGS &= ~FMASK (IF cleared).
 *
 * The CPU pushed NOTHING and did NOT switch stacks — unlike interrupts
 * there is no TSS.rsp0 switch for SYSCALL.  The stub builds the whole
 * frame itself on the TSS.rsp0 stack.  rcx/r11 are ABI-clobbered by the
 * syscall itself (hardware), so their pt_regs slots carry the user
 * rip/rflags — same as Linux.
 *
 * The user RSP is stashed in TSS.sp2 because the stack switch happens
 * before it can be pushed and no register is free to hold it.
 * ============================================================================
 */
.global syscall_entry
.hidden syscall_entry
syscall_entry:
    movq %rsp, TSS_X86+TSS_SP2(%rip)   /* stash user rsp (Linux trick) */
    movq TSS_X86+TSS_RSP0(%rip), %rsp  /* kernel stack top (TSS.rsp0)  */
    pushq $__USER_DS                   /* 0xa0 ss                       */
    pushq TSS_X86+TSS_SP2(%rip)        /* 0x98 rsp (user)               */
    pushq %r11                         /* 0x90 rflags (user)            */
    pushq $__USER_CS                   /* 0x88 cs                       */
    pushq %rcx                         /* 0x80 rip (user)               */
    pushq %rax                         /* 0x78 orig_rax = syscall nr    */
    pushq %rdi
    pushq %rsi
    pushq %rdx
    pushq %rcx
    pushq %rax
    pushq %r8
    pushq %r9
    pushq %r10
    pushq %r11
    pushq %rbx
    pushq %rbp
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    /* frame at rsp0 - 168 == Task::pt_regs(), same as IRQ entries */
    movq %rsp, %rbp
    andq $-16, %rsp
    movq %rbp, %rdi
    xorl %esi, %esi
    movq $SYSCALL_VECTOR, %rdx
    call trap_handler
    movq %rbp, %rsp
    jmp trap_exit

/* ============================================================================
 * trap_exit — common return path (rsp -> pt_regs)
 *
 * Mirror of the riscv64 ret_from_exception: user-origin returns check
 * pending signals (may rewrite the frame) and need_resched in a loop;
 * kernel-origin returns go straight to the register restore.  IF is
 * never touched here — iretq restores it from the saved rflags.
 *
 * Kernel-exit preemption is deliberately absent (the riscv64 twin keeps
 * it effectively disabled too): the idle loop calls schedule() itself,
 * and preempting mid-kernel-frame is CONFIG_PREEMPT territory.
 * ============================================================================
 */
.global trap_exit
.hidden trap_exit
trap_exit:
    movq %rsp, %rbp              /* frame pointer; RBP is callee-saved */
    andq $-16, %rsp              /* across the C calls below          */
    testb $3, PT_CS(%rbp)
    jz .Lrestore_kernel

.Lreturn_user:
    movq %rbp, %rdi
    call check_and_deliver_signals
.Lcheck_resched:
    call asm_need_resched
    testq %rax, %rax
    jz .Lrestore_all
    call x86_preemptible
    testq %rax, %rax
    jz .Lrestore_all
    call asm_schedule
    /* After the switch back, RSP/RBP are restored from thread.sp /
     * thread.callee.rbp saved at switch-out — both still point at this
     * frame, so just re-run the signal/resched checks. */
    jmp .Lcheck_resched

.Lrestore_kernel:
    /* no signal delivery on kernel-origin returns */

.Lrestore_all:
    movq %rbp, %rsp
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %rbp
    popq %rbx
    popq %r11
    popq %r10
    popq %r9
    popq %r8
    popq %rax
    popq %rcx
    popq %rdx
    popq %rsi
    popq %rdi
    addq $8, %rsp                /* skip orig_rax/error-code slot */
    iretq                        /* pops rip,cs,rflags,rsp,ss */

/* ============================================================================
 * ret_from_fork — first instruction of a forked child
 *
 * fork sets child.thread.sp = stack_top - 168 (the complete user frame)
 * and child.thread.callee.ret_addr = ret_from_fork; __switch_to's final
 * `push ret_addr; ret` lands here with RSP at the frame and RBP holding
 * callee.rbp from the child's thread struct (zero for a fresh child).
 *
 * Runs schedule_tail first (finish the switch bookkeeping — deferred
 * exit notification etc.), then returns through the common exit path so
 * the child gets the same signal/resched checks as any trap return.
 * ============================================================================
 */
.global ret_from_fork
.hidden ret_from_fork
ret_from_fork:
    movq %rsp, %rbp
    andq $-16, %rsp
    call get_prev_task           /* RAX = prev task (set by context_switch) */
    movq %rax, %rdi
    call schedule_tail
    movq %rbp, %rsp
    jmp trap_exit

/* ============================================================================
 * ret_from_fork_kernel — first instruction of a kernel thread
 *
 * Convention (mirrors the riscv64 twin's s0/s1 protocol): kthread setup
 * stores fn in thread.callee.r12 and fn_arg in thread.callee.r13, and
 * thread.sp points at a scratch pt_regs-sized region at the top of the
 * kthread stack.  After __switch_to restores the callee registers:
 *   R12 = fn, R13 = fn_arg, RSP = scratch regs.
 * The helper never returns (it calls do_exit).
 * ============================================================================
 */
.global ret_from_fork_kernel
.hidden ret_from_fork_kernel
ret_from_fork_kernel:
    movq %rsp, %rbp
    andq $-16, %rsp
    call get_prev_task
    movq %rax, %rdi
    call schedule_tail
    movq %r13, %rdi              /* fn_arg */
    movq %r12, %rsi              /* fn_ptr */
    movq %rbp, %rdx              /* regs  */
    call ret_from_fork_kernel_helper
    /* unreachable — helper calls do_exit */
1:  cli
    hlt
    jmp 1b
