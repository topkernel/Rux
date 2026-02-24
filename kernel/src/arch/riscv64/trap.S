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
//! 1. 使用 sstatus.SPP 位判断 trap 来源（标准 RISC-V 方式）
//! 2. sscratch 保存内核 tp 值（hart ID + 1）
//! 3. PtRegs 结构体与 Linux `struct pt_regs` 布局一致
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

// ============================================================================
// trap_entry - 异常/中断入口点
// ============================================================================
//
// 与 Linux handle_exception 类似的设计
//
trap_entry:
    // 使用 sstatus.SPP 判断来源
    // SPP (bit 8): 1 = 从 S-mode 来，0 = 从 U-mode 来
    csrr t0, sstatus
    andi t0, t0, SR_SPP
    bnez t0, .Lfrom_kernel

// ============================================================================
// 从用户空间进入 trap
// ============================================================================
.Lfrom_user:
    // 步骤 1: 在切换栈之前保存用户 sp
    // 使用 t1 临时保存用户 sp
    mv t1, sp

    // 步骤 2: 交换 tp 和 sscratch
    // - tp 变成 hart ID + 1 (内核值)
    // - sscratch 变成用户 tp
    csrrw tp, sscratch, tp

    // 步骤 3: 转换为实际的 hart ID
    addi tp, tp, -1

    // 步骤 4: 获取内核栈指针
    la sp, __kernel_trap_stack_top

    // 步骤 5: 分配 PtRegs 空间
    addi sp, sp, -PT_SIZE

    // 步骤 6: 保存用户 sp (之前保存在 t1)
    sd t1, PT_SP(sp)

    // 步骤 7: 保存用户 tp (现在在 sscratch)
    csrr t0, sscratch
    sd t0, PT_TP(sp)

    // 步骤 8: 保存 CSR
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    csrr t3, scause
    sd t0, PT_STATUS(sp)
    sd t1, PT_EPC(sp)
    sd t2, PT_BADADDR(sp)
    sd t3, PT_CAUSE(sp)

    // 步骤 9: 保存通用寄存器
    sd x1,  PT_RA(sp)
    sd x3,  PT_GP(sp)
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

    // 步骤 10: 设置 sscratch = 内核 hart ID + 1 (用于下次 trap 入口)
    addi t0, tp, 1
    csrw sscratch, t0

    j .Lcall_handler

// ============================================================================
// 从内核进入 trap
// ============================================================================
.Lfrom_kernel:
    // 从内核来，tp 已经是正确的 hart ID
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
    sd x4,  PT_TP(sp)   // 保存内核 tp
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
    // 注意：sstatus 已经恢复，可以直接检查
    ld t0, PT_STATUS(sp)
    andi t0, t0, SR_SPP
    beqz t0, .Lreturn_user

// ============================================================================
// 返回内核
// ============================================================================
.Lreturn_kernel:
    // 恢复内核 tp 和 sp
    ld x4, PT_TP(sp)
    ld x2, PT_SP(sp)
    sret

// ============================================================================
// 返回用户空间
// ============================================================================
.Lreturn_user:
    // 在恢复用户 tp 之前，先保存内核 hart ID
    // tp 当前保存内核 hart ID
    // 我们需要设置 sscratch = hart_id + 1，用于下次 trap 入口
    addi t0, tp, 1
    csrw sscratch, t0

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
//
.global ret_from_fork
ret_from_fork:
    // 禁用中断
    csrci sstatus, 2

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

    // 在恢复用户 tp 之前，先设置 sscratch = hart_id + 1
    // tp 当前保存内核 hart ID
    addi t0, tp, 1
    csrw sscratch, t0

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
