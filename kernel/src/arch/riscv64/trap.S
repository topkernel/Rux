//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 异常向量表
//!
//! 与 Linux 内核 `arch/riscv/kernel/entry.S` 兼容的设计
//!
//! ## 设计要点
//!
//! 1. 使用 sscratch 交换检测 trap 来源（Linux 标准方式）
//!    - 内核态: sscratch = 0, tp = current task
//!    - 用户态: sscratch = current task, tp = user TLS
//! 2. 支持早期启动阶段的兼容检测
//! 3. PtRegs 结构体与 Linux `struct pt_regs` 布局一致
//!
//! ## sscratch/tp 协议
//!
//! | 状态       | sscratch      | tp          |
//! |-----------|---------------|-------------|
//! | 内核态运行 | 0             | current     |
//! | 用户态运行 | current       | user TLS    |
//!
//! 当发生 trap 时:
//!   csrrw tp, sscratch, tp
//!   - 来自内核: tp 变为 0 (sscratch 原值)
//!   - 来自用户: tp 变为 current task (sscratch 原值)
//!
//! ## PtRegs 布局 (288 字节)
//!
//! ```text
//! Offset  Field       Description
//! ------  -----       -----------
//! 0x00    epc         程序计数器 (sepc)
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
//! 0x118   orig_a0     原始 a0
//! ```

.section .text.trap
.align 2
.global trap_entry
.extern trap_handler
.extern kernel_lock_acquire
.extern kernel_lock_release

// ============================================================================
// 内核大锁汇编宏
// ============================================================================

// KERNEL_LOCK_ACQUIRE - 获取内核大锁
//
// 使用方法: KERNEL_LOCK_ACQUIRE
//
// 破坏寄存器: t0, t1, t2
//
// 实现原理:
//   使用 amoswap.w.aq 指令原子交换，带 acquire 语义
//   如果锁已被占用，自旋等待
.macro KERNEL_LOCK_ACQUIRE
    la t0, KERNEL_LOCK
    li t2, 1
1:
    amoswap.w.aq t1, t2, (t0)    // 原子交换，返回原值到 t1
    bnez t1, 1b                   // 如果原来非0，自旋等待
.endm

// KERNEL_LOCK_RELEASE - 释放内核大锁
//
// 使用方法: KERNEL_LOCK_RELEASE
//
// 破坏寄存器: t0
//
// 实现原理:
//   使用 amoswap.w.rl 指令原子写入0，带 release 语义
.macro KERNEL_LOCK_RELEASE
    la t0, KERNEL_LOCK
    amoswap.w.rl zero, zero, (t0)  // 原子写入0，带释放语义
.endm

// PtRegs 偏移量常量 (与 pt_regs.rs 一致)
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

// sstatus 位
.set SR_SPP,      0x100

// Task 结构体偏移量 (与 task.rs TASK_OFFSETS 一致)
.set TASK_TI_FLAGS,      0x00    // ti_flags 偏移
.set TASK_TI_PREEMPT,    0x04    // ti_preempt_count 偏移
.set TASK_TI_KERNEL_SP,  0x08    // ti_kernel_sp 偏移
.set TASK_TI_USER_SP,    0x10    // ti_user_sp 偏移
.set TASK_TI_CPU,        0x18    // ti_cpu 偏移

// ============================================================================
// trap_entry - 异常/中断入口点
// ============================================================================
//
// Linux 风格的 sscratch 检测:
//   csrrw tp, sscratch, tp  # 交换 tp 和 sscratch
//   bnez tp, .Lfrom_user    # tp != 0 表示来自用户态
//
// 同时支持早期启动阶段 (sscratch 未初始化时)
//
trap_entry:
    // 步骤 1: 原子交换 tp 和 sscratch
    // - 来自内核: sscratch=0, tp=current -> tp 变为 0
    // - 来自用户: sscratch=current, tp=user_tls -> tp 变为 current
    csrrw tp, sscratch, tp

    // 步骤 2: 检测来源
    // tp == 0: 来自内核态 (正常情况，sscratch 原值为 0)
    // tp != 0: 可能来自用户态，也可能是早期启动阶段
    beqz tp, .Lfrom_kernel

    // tp != 0，检查是否是有效的 task 指针
    // 有效的 task 指针应该 >= 0x80000000 (内核空间)
    // 如果 tp < 0x80000000，说明是早期启动阶段 (tp = hart_id)
    li t0, 0x80000000
    bltu tp, t0, .Learly_boot

    // tp >= 0x80000000，是有效的 task 指针，来自用户态
    j .Lfrom_user

// ============================================================================
// 从内核进入 trap (正常模式)
// ============================================================================
//
// tp = 0 (来自 sscratch 交换)
// sscratch = current task (之前保存的 tp 值)
//
.Lfrom_kernel:
    // 恢复 tp = current task (从 sscratch)
    csrr tp, sscratch

    // 检查是否已经在 trap 栈上
    la t1, __kernel_trap_stack
    la t2, __kernel_trap_stack_top

    // 如果 sp 不在 trap 栈范围内，切换栈
    blt sp, t1, .Luse_trap_stack
    bgt sp, t2, .Luse_trap_stack

    // 已经在 trap 栈上，直接分配空间
    j .Lalloc_frame_kernel

.Luse_trap_stack:
    // 切换到 trap 栈
    mv sp, t2

.Lalloc_frame_kernel:
    // 保存原始 sp (在分配 PtRegs 之前)
    mv t1, sp

    // 分配 PtRegs 空间
    addi sp, sp, -PT_SIZE

    // 保存原始 sp
    sd t1, PT_SP(sp)

    // 保存 CSR
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    // 保存通用寄存器
    sd x1,  PT_RA(sp)
    sd x3,  PT_GP(sp)
    sd x4,  PT_TP(sp)   // 保存内核 tp (= current task)
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
// 早期启动阶段 (兼容模式)
// ============================================================================
//
// tp < 0x1000，说明 sscratch 未初始化，还在早期启动
// 使用 sstatus.SPP 检测
//
.Learly_boot:
    // 恢复 tp (因为交换导致 tp 变成了 sscratch 的原值)
    // 在早期启动阶段，tp 原来是 hart_id，sscratch 未定义
    // 我们需要恢复 tp = hart_id
    // 注意：此时 tp 是 sscratch 的原值（可能是任意值）
    // 但我们无法恢复原来的 tp，所以使用 sstatus.SPP 检测

    // 使用 sstatus.SPP 判断来源
    csrr t0, sstatus
    andi t0, t0, SR_SPP
    bnez t0, .Lfrom_kernel_early

// ============================================================================
// 从用户空间进入 trap (早期启动阶段)
// ============================================================================
.Lfrom_user_early:
    // 在早期启动阶段，tp = hart_id
    // 保存用户 sp
    mv t1, sp

    // 获取内核栈指针
    la sp, __kernel_trap_stack_top

    // 分配 PtRegs 空间
    addi sp, sp, -PT_SIZE

    // 保存用户 sp
    sd t1, PT_SP(sp)

    // 保存用户 tp (hart_id)
    sd tp, PT_TP(sp)

    // 保存 CSR
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    // 保存通用寄存器
    sd x1,  PT_RA(sp)
    sd x3,  PT_GP(sp)
    // tp 已保存
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
// 从内核进入 trap (早期启动阶段)
// ============================================================================
.Lfrom_kernel_early:
    // 早期启动阶段，tp = hart_id
    // 检查是否已经在 trap 栈上
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

    // 保存 CSR
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    // 保存通用寄存器
    sd x1,  PT_RA(sp)
    sd x3,  PT_GP(sp)
    sd x4,  PT_TP(sp)   // 保存内核 tp (= hart_id)
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
// 从用户空间进入 trap (正常模式)
// ============================================================================
//
// tp = current task (来自 sscratch 交换)
// sscratch = user TLS (之前的 tp 值)
//
.Lfrom_user:
    // tp 现在指向 current task_struct

    // 保存用户 sp (在切换栈之前)
    mv t1, sp

    // 从 task_struct 获取内核栈指针
    // task_struct.ti_kernel_sp 在偏移 0x10
    ld sp, TASK_TI_KERNEL_SP(tp)

    // 如果 ti_kernel_sp 为 0，使用专用 trap 栈
    beqz sp, .Luse_trap_stack_user

    // 分配 PtRegs 空间
    addi sp, sp, -PT_SIZE
    j .Lsave_user_context

.Luse_trap_stack_user:
    la sp, __kernel_trap_stack_top
    addi sp, sp, -PT_SIZE

.Lsave_user_context:
    // 保存用户 sp
    sd t1, PT_SP(sp)

    // 保存用户 tp (现在在 sscratch 中)
    csrr t0, sscratch
    sd t0, PT_TP(sp)

    // 保存 current task 指针到 sscratch
    // 这样返回时可以恢复 tp = current
    csrw sscratch, tp

    // 保存 CSR
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    // 保存通用寄存器
    sd x1,  PT_RA(sp)
    sd x3,  PT_GP(sp)
    // tp 已保存 (user TLS)
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

    // 恢复 tp = current task (从 sscratch)
    csrr tp, sscratch

    // 获取内核大锁（从用户态进入内核）
    KERNEL_LOCK_ACQUIRE

    j .Lcall_handler

// ============================================================================
// 调用 Rust trap 处理函数
// ============================================================================
.Lcall_handler:
    // 设置 orig_a0 = a0 (用于系统调用回滚)
    ld t0, PT_A0(sp)
    sd t0, PT_ORIG_A0(sp)

    // 调用 Rust trap_handler
    // 参数: a0 = PtRegs 指针
    mv a0, sp
    call trap_handler

// ============================================================================
// 从 trap 返回
// ============================================================================
trap_exit:
    // 禁用中断
    csrci sstatus, 2

    // 清除 LR/SC 预留 (Load Reservation)
    //
    // RISC-V 的 LR/SC (Load-Reserved/Store-Conditional) 原子操作
    // 会在 hart 上设置一个 reservation。如果在 LR 和 SC 之间
    // 发生上下文切换，这个 reservation 可能会影响新任务的原子操作。
    //
    // 解决方案：在返回新上下文前执行一个必定失败的 SC 来清除 reservation。
    // 这是安全的，因为 RISC-V 允许在任何时候清除 reservation。
    //
    // 参考 Linux: arch/riscv/kernel/entry.S ret_from_exception
    addi t2, sp, PT_SIZE      // 计算地址: sp + PT_EPC
    sc.d x0, t2, (t2)           // 执行 SC（必定失败，但会清除 reservation）

    // 恢复 CSR (先恢复 sstatus，以便检查 SPP)
    ld t0, PT_STATUS(sp)
    ld t1, PT_EPC(sp)
    csrw sstatus, t0
    csrw sepc, t1

    // 恢复通用寄存器
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

    // 检查是否返回用户空间 (SPP=0)
    ld t0, PT_STATUS(sp)
    andi t0, t0, SR_SPP
    beqz t0, .Lreturn_user

// ============================================================================
// 返回内核
// ============================================================================
//
// tp = current task (内核态)
// sscratch = 0 (内核态标志)
//
.Lreturn_kernel:
    // 恢复内核 tp
    ld x4, PT_TP(sp)

    // 重置 sscratch = 0 (内核态标志)
    // 这样下次 trap 时可以正确检测到来自内核
    csrw sscratch, zero

    // 恢复 sp
    ld x2, PT_SP(sp)
    sret

// ============================================================================
// 返回用户空间
// ============================================================================
//
// tp = current task
// 需要设置: sscratch = tp, tp = user TLS
//
.Lreturn_user:
    // 释放内核大锁（返回用户态）
    KERNEL_LOCK_RELEASE

    // 保存 current task 到 sscratch
    // 下次 trap 入口时可以找到内核数据结构
    csrw sscratch, tp

    // 恢复用户 tp (从 PtRegs)
    ld x4, PT_TP(sp)

    // 恢复用户 sp (从 PtRegs)
    ld x2, PT_SP(sp)

    sret

.global trap_entry_end
trap_entry_end:

// ============================================================================
// ret_from_fork - fork 子进程入口点
// ============================================================================
//
// 当 fork 创建的子进程首次被调度时，从这里开始执行
// tp = current task (子进程)
//
.extern schedule_tail
.global ret_from_fork
ret_from_fork:
    // 禁用中断
    csrci sstatus, 2

    // 调用 schedule_tail 清理前一个任务
    // 参数: a0 = prev task (存储在 s1 中)
    // 参考 Linux: arch/riscv/kernel/entry.S ret_from_fork_user_asm
    //
    // 在 Linux 中，prev task 通过 s1 寄存器传递：
    // - __switch_to 返回 prev task
    // - schedule_tail(prev) 清理 prev task
    //
    // Rux 实现:
    // - s1 中存储了 prev task (由 context_switch_asm 设置)
    // - 由于使用内核大锁，prev task 的清理工作较少
    mv a0, s1              // a0 = prev task (从 s1 获取)
    call schedule_tail

    // 恢复 CSR
    ld t0, PT_STATUS(sp)
    ld t1, PT_EPC(sp)
    csrw sstatus, t0
    csrw sepc, t1

    // 恢复通用寄存器
    ld x1,  PT_RA(sp)
    ld x3,  PT_GP(sp)
    ld x5,  PT_T0(sp)
    ld x6,  PT_T1(sp)
    ld x7,  PT_T2(sp)
    ld x8,  PT_S0(sp)
    ld x9,  PT_S1(sp)
    ld x10, PT_A0(sp)      // a0 = 0 (fork 子进程返回值)
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

    // 检查是否返回用户空间
    ld t0, PT_STATUS(sp)
    andi t0, t0, SR_SPP
    beqz t0, .Lret_from_fork_user

    // 返回内核 (内核线程)
    ld x4, PT_TP(sp)
    ld x2, PT_SP(sp)
    sret

.Lret_from_fork_user:
    // 返回用户空间
    // fork 子进程需要先获取锁，再释放锁返回用户态
    // 因为 fork 子进程从 schedule() 返回，没有经过 trap 入口获取锁
    KERNEL_LOCK_ACQUIRE

    // 释放内核大锁（返回用户态）
    KERNEL_LOCK_RELEASE

    // 设置 sscratch = current task (tp)
    csrw sscratch, tp

    // 恢复用户 tp
    ld x4, PT_TP(sp)

    // 恢复用户 sp
    ld x2, PT_SP(sp)

    sret

// ============================================================================
// 内核 trap 栈 (16KB per CPU, 目前支持 4 CPU)
// ============================================================================
.section .bss
.align 16
__kernel_trap_stack:
    .space 16384 * 4
__kernel_trap_stack_top:
