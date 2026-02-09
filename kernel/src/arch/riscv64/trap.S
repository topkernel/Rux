//! RISC-V 异常向量表
//!
//! RISC-V 的异常向量表由 stvec 寄存器指向
//! BASE 地址必须是 4 字节对齐
//!
//! 栈帧布局（从 sp 偏移）：
//!   0:       原始 sp（用户栈或内核栈）
//!   8-208:   调用者寄存器
//!   216-232: CSR 寄存器 (sstatus, sepc, stval)

.section .text.trap
.align 2
.global trap_entry
.extern trap_handler

// 异常向量表
// stvec[1:0] = 0 => Direct mode
trap_entry:
    // 保存当前 sp（可能是用户栈或内核栈）
    mv t0, sp

    // 切换到内核 trap 栈（sscratch 包含内核栈指针）
    csrrw sp, sscratch, sp

    // 在内核栈上分配 TrapFrame 空间
    addi sp, sp, -272

    // 保存原始 sp（用户栈或内核栈）
    sd t0, 0(sp)

    // 保存调用者寄存器
    sd x1, 8(sp)
    sd x5, 16(sp)
    sd x6, 24(sp)
    sd x7, 32(sp)
    sd x10, 40(sp)
    sd x11, 48(sp)
    sd x12, 56(sp)
    sd x13, 64(sp)
    sd x14, 72(sp)
    sd x15, 80(sp)
    sd x16, 88(sp)
    sd x17, 96(sp)
    sd x18, 104(sp)
    sd x19, 112(sp)
    sd x20, 120(sp)
    sd x21, 128(sp)
    sd x22, 136(sp)
    sd x23, 144(sp)
    sd x24, 152(sp)
    sd x25, 160(sp)
    sd x26, 168(sp)
    sd x27, 176(sp)
    sd x28, 184(sp)
    sd x29, 192(sp)
    sd x30, 200(sp)
    sd x31, 208(sp)

    // 保存 S-mode CSR 寄存器
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    sd t0, 216(sp)
    sd t1, 224(sp)
    sd t2, 232(sp)

    // 调用 Rust trap 处理函数
    // 传递 TrapFrame 指针（跳过原始 sp，与 TrapFrame 结构对齐）
    addi a0, sp, 8
    call trap_handler

    // 恢复 CSR 寄存器
    ld t0, 216(sp)
    ld t1, 224(sp)
    ld t2, 232(sp)
    csrw sstatus, t0
    csrw sepc, t1
    csrw stval, t2

    // 恢复调用者寄存器
    ld x1, 8(sp)
    ld x5, 16(sp)
    ld x6, 24(sp)
    ld x7, 32(sp)
    ld x10, 40(sp)
    ld x11, 48(sp)
    ld x12, 56(sp)
    ld x13, 64(sp)
    ld x14, 72(sp)
    ld x15, 80(sp)
    ld x16, 88(sp)
    ld x17, 96(sp)
    ld x18, 104(sp)
    ld x19, 112(sp)
    ld x20, 120(sp)
    ld x21, 128(sp)
    ld x22, 136(sp)
    ld x23, 144(sp)
    ld x24, 152(sp)
    ld x25, 160(sp)
    ld x26, 168(sp)
    ld x27, 176(sp)
    ld x28, 184(sp)
    ld x29, 192(sp)
    ld x30, 200(sp)
    ld x31, 208(sp)

    // 恢复原始 sp 并切换回
    ld t0, 0(sp)
    addi sp, sp, 272
    csrrw sp, sscratch, t0

    // 返回异常处理
    sret

.global trap_entry_end
trap_entry_end:
