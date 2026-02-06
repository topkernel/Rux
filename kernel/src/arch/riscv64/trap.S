//! RISC-V 异常向量表
//!
//! RISC-V 的异常向量表由 stvec 寄存器指向
//! BASE 地址必须是 4 字节对齐

.section .text.trampoline
.align 2
.global trap_entry

// 异常向量表格式
// stvec[1:0] = 0 => Direct mode
// 所有异常跳转到 trap_entry
trap_entry:
    // 保存调用者寄存器 (使用 64位指令)
    addi sp, sp, -272

    sd x1, 0(sp)
    sd x5, 8(sp)
    sd x6, 16(sp)
    sd x7, 24(sp)
    sd x10, 32(sp)
    sd x11, 40(sp)
    sd x12, 48(sp)
    sd x13, 56(sp)
    sd x14, 64(sp)
    sd x15, 72(sp)
    sd x16, 80(sp)
    sd x17, 88(sp)
    sd x18, 96(sp)
    sd x19, 104(sp)
    sd x20, 112(sp)
    sd x21, 120(sp)
    sd x22, 128(sp)
    sd x23, 136(sp)
    sd x24, 144(sp)
    sd x25, 152(sp)
    sd x26, 160(sp)
    sd x27, 168(sp)
    sd x28, 176(sp)
    sd x29, 184(sp)
    sd x30, 192(sp)
    sd x31, 200(sp)

    // 保存 sstatus, sepc, stval (S-mode CSR)
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, stval
    sd t0, 208(sp)
    sd t1, 216(sp)
    sd t2, 224(sp)

    // 调用 Rust trap 处理函数
    // 传递栈指针作为 TrapFrame
    mv a0, sp
    call trap_handler

    // 恢复寄存器
    ld t0, 208(sp)
    ld t1, 216(sp)
    ld t2, 224(sp)
    csrw sstatus, t0
    csrw sepc, t1
    csrw stval, t2

    ld x1, 0(sp)
    ld x5, 8(sp)
    ld x6, 16(sp)
    ld x7, 24(sp)
    ld x10, 32(sp)
    ld x11, 40(sp)
    ld x12, 48(sp)
    ld x13, 56(sp)
    ld x14, 64(sp)
    ld x15, 72(sp)
    ld x16, 80(sp)
    ld x17, 88(sp)
    ld x18, 96(sp)
    ld x19, 104(sp)
    ld x20, 112(sp)
    ld x21, 120(sp)
    ld x22, 128(sp)
    ld x23, 136(sp)
    ld x24, 144(sp)
    ld x25, 152(sp)
    ld x26, 160(sp)
    ld x27, 168(sp)
    ld x28, 176(sp)
    ld x29, 184(sp)
    ld x30, 192(sp)
    ld x31, 200(sp)

    addi sp, sp, 272

    // 返回异常处理 (使用 S-mode 的 sret)
    sret

.global trap_entry_end
trap_entry_end:
