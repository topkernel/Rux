//! RISC-V 异常向量表
//!
//! 参考 Linux 内核的设计 (arch/riscv/kernel/entry.S)
//!
//! 核心设计：
//! - 使用 sstatus.SPP 位判断 trap 来源（SPP=1 从 S-mode，SPP=0 从 U-mode）
//! - sscratch 保存内核 tp 值（hart ID + 1），用于恢复 tp
//!   - 注意：+1 是为了避免 hart ID = 0 时的歧义
//! - trap 入口：csrrw tp, sscratch, tp 交换 tp
//!   - 如果从用户空间来：sscratch 非零，交换后 tp = sscratch 值
//!   - 如果从内核来：sscratch = 0，tp 不变
//!
//! 栈帧布局（从 sp 偏移）：
//!   0:       用户 tp（从 sscratch 读取）
//!   8:       原始 sp（用户栈或内核栈）
//!   16-232:  调用者寄存器（ra, t0-t6, a0-a7, s2-s11）
//!   240-256: CSR 寄存器 (sstatus, sepc, stval)

.section .text.trap
.align 2
.global trap_entry
.extern trap_handler

// 异常向量表
// stvec[1:0] = 0 => Direct mode
trap_entry:
    // 使用 sstatus.SPP 判断来源（RISC-V 标准方式）
    // SPP (bit 8): 1 = 从 S-mode 来，0 = 从 U-mode 来
    csrr t0, sstatus
    andi t0, t0, 0x100    // 检查 SPP 位
    bnez t0, .Lfrom_kernel

.Lfrom_user:
    // 从用户空间来
    // 先保存用户 sp（在切换到内核栈之前）
    mv t1, sp

    // 交换 tp 和 sscratch，获取内核 tp (hart ID + 1)
    csrrw tp, sscratch, tp

    // tp 现在是 hart ID + 1（如果 sscratch 被正确设置）
    // 转换为实际的 hart ID
    addi tp, tp, -1

    // 获取内核栈指针
    la sp, __kernel_trap_stack_top

    // 分配 TrapFrame 空间
    addi sp, sp, -264

    // 保存用户 tp（从 sscratch 读取）
    // sscratch 现在包含用户 tp（之前的 tp 被换出去了）
    csrr t0, sscratch
    sd t0, 0(sp)

    // 保存原始用户 sp（从 t1）
    sd t1, 8(sp)

    // 设置 t0 = 用户 sp，供 .Lsave_context 使用
    mv t0, t1

    j .Lsave_context

.Lfrom_kernel:
    // 从内核来，tp 已经是正确的 hart ID
    // 保存原始 sp
    mv t0, sp

    // 检查是否已经在 trap 栈上（处理嵌套 trap）
    // 如果 sp 在 __kernel_trap_stack 和 __kernel_trap_stack_top 之间，
    // 说明我们已经在 trap 栈上，直接在当前栈上分配空间
    la t1, __kernel_trap_stack
    la t2, __kernel_trap_stack_top

    // 检查 sp >= __kernel_trap_stack && sp <= __kernel_trap_stack_top
    // 即：sp 在 trap 栈范围内
    blt sp, t1, .Luse_trap_stack    // 如果 sp < __kernel_trap_stack，切换栈
    bgt sp, t2, .Luse_trap_stack    // 如果 sp > __kernel_trap_stack_top，切换栈

    // 已经在 trap 栈上，直接分配空间
    j .Lalloc_frame

.Luse_trap_stack:
    // 不在 trap 栈上，切换到 trap 栈
    mv sp, t2

.Lalloc_frame:
    // 分配 TrapFrame 空间
    addi sp, sp, -264

    // 标记这是从内核来的（tp = 0）
    sd zero, 0(sp)

.Lsave_context:
    // 保存原始 sp
    sd t0, 8(sp)

    // 保存调用者寄存器
    sd x1, 16(sp)       // ra
    sd x5, 24(sp)       // t0
    sd x6, 32(sp)       // t1
    sd x7, 40(sp)       // t2
    sd x10, 48(sp)      // a0
    sd x11, 56(sp)      // a1
    sd x12, 64(sp)      // a2
    sd x13, 72(sp)      // a3
    sd x14, 80(sp)      // a4
    sd x15, 88(sp)      // a5
    sd x16, 96(sp)      // a6
    sd x17, 104(sp)     // a7
    sd x28, 112(sp)     // t3
    sd x29, 120(sp)     // t4
    sd x30, 128(sp)     // t5
    sd x31, 136(sp)     // t6
    sd x18, 144(sp)     // s2
    sd x19, 152(sp)     // s3
    sd x20, 160(sp)     // s4
    sd x21, 168(sp)     // s5
    sd x22, 176(sp)     // s6
    sd x23, 184(sp)     // s7
    sd x24, 192(sp)     // s8
    sd x25, 200(sp)     // s9
    sd x26, 208(sp)     // s10
    sd x27, 216(sp)     // s11

    // 保存 S-mode CSR 寄存器
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    sd t0, 240(sp)
    sd t1, 248(sp)
    sd t2, 256(sp)

    // 设置 sscratch = 0，标记现在在内核态
    // 这样如果发生嵌套 trap，可以识别是从内核来的
    csrw sscratch, zero

    // 调用 Rust trap 处理函数
    // 传递 TrapFrame 指针（跳过用户 tp 和原始 sp）
    addi a0, sp, 16
    call trap_handler

    // 禁用中断 - 在恢复上下文前关闭中断，确保原子性
    csrci sstatus, 2   // 清除 SIE 位 (bit 1)

    // 恢复 CSR 寄存器
    ld t0, 240(sp)
    ld t1, 248(sp)
    ld t2, 256(sp)
    csrw sstatus, t0
    csrw sepc, t1
    csrw stval, t2

    // 恢复调用者寄存器
    ld x1, 16(sp)       // ra
    ld x5, 24(sp)       // t0
    ld x6, 32(sp)       // t1
    ld x7, 40(sp)       // t2
    ld x10, 48(sp)      // a0
    ld x11, 56(sp)      // a1
    ld x12, 64(sp)      // a2
    ld x13, 72(sp)      // a3
    ld x14, 80(sp)      // a4
    ld x15, 88(sp)      // a5
    ld x16, 96(sp)      // a6
    ld x17, 104(sp)     // a7
    ld x28, 112(sp)     // t3
    ld x29, 120(sp)     // t4
    ld x30, 128(sp)     // t5
    ld x31, 136(sp)     // t6
    ld x18, 144(sp)     // s2
    ld x19, 152(sp)     // s3
    ld x20, 160(sp)     // s4
    ld x21, 168(sp)     // s5
    ld x22, 176(sp)     // s6
    ld x23, 184(sp)     // s7
    ld x24, 192(sp)     // s8
    ld x25, 200(sp)     // s9
    ld x26, 208(sp)     // s10
    ld x27, 216(sp)     // s11

    // 检查是否返回用户空间
    ld t0, 0(sp)        // 用户 tp（如果为 0 说明是从内核来的）
    beqz t0, .Lreturn_to_kernel

.Lreturn_to_user:
    // 返回用户空间
    // 将内核 tp + 1 (hart ID + 1) 保存到 sscratch
    // 这样下次 trap 入口时可以正确恢复 hart ID
    addi t1, tp, 1
    csrw sscratch, t1

    // 恢复用户 tp
    mv tp, t0

    // 恢复原始 sp（用户栈）
    ld sp, 8(sp)

    sret

.Lreturn_to_kernel:
    // 返回内核
    // 恢复原始 sp（内核栈）
    ld sp, 8(sp)

    sret

.global trap_entry_end
trap_entry_end:

// 内核 trap 栈（16KB）
.section .bss
.align 16
__kernel_trap_stack:
    .space 16384
__kernel_trap_stack_top:
