//! RISC-V 异常向量表
//!
//! RISC-V 的异常向量表由 mtvec 寄存器指向
//! BASE 地址必须是 4 字节对齐

.section .tramp
.align 2
.global trap_entry

// 异常向量表格式
// mtvec[1:0] = 0 => Direct mode
// 所有异常跳转到 trap_entry
trap_entry:
    // 保存调用者寄存器
    addi sp, sp, -256

    sw x1, 0(sp)
    sw x5, 4(sp)
    sw x6, 8(sp)
    sw x7, 12(sp)
    sw x10, 16(sp)
    sw x11, 20(sp)
    sw x12, 24(sp)
    sw x13, 28(sp)
    sw x14, 32(sp)
    sw x15, 36(sp)
    sw x16, 40(sp)
    sw x17, 44(sp)
    sw x18, 48(sp)
    sw x19, 52(sp)
    sw x20, 56(sp)
    sw x21, 60(sp)
    sw x22, 64(sp)
    sw x23, 68(sp)
    sw x24, 72(sp)
    sw x25, 76(sp)
    sw x26, 80(sp)
    sw x27, 84(sp)
    sw x28, 88(sp)
    sw x29, 92(sp)
    sw x30, 96(sp)
    sw x31, 100(sp)

    // 保存 mstatus, mepc, mtval
    csrrs x5, mstatus
    csrrs x6, mepc
    csrrs x7, mtval
    sw x5, 104(sp)
    sw x6, 108(sp)
    sw x7, 112(sp)

    // 调用 Rust trap 处理函数
    // 传递异常信息
    addi x10, sp, 0  // 传递栈指针作为 TrapFrame
    call trap_handler

    // 恢复寄存器
    lw x5, 104(sp)
    lw x6, 108(sp)
    lw x7, 112(sp)
    csrrw mstatus, x5
    csrrw mepc, x6
    csrrw mtval, x7

    lw x1, 0(sp)
    lw x5, 4(sp)
    lw x6, 8(sp)
    lw x7, 12(sp)
    lw x10, 16(sp)
    lw x11, 20(sp)
    lw x12, 24(sp)
    lw x13, 28(sp)
    lw x14, 32(sp)
    lw x15, 36(sp)
    lw x16, 40(sp)
    lw x17, 44(sp)
    lw x18, 48(sp)
    lw x19, 52(sp)
    lw x20, 56(sp)
    lw x21, 60(sp)
    lw x22, 64(sp)
    lw x23, 68(sp)
    lw x24, 72(sp)
    lw x25, 76(sp)
    lw x26, 80(sp)
    lw x27, 84(sp)
    lw x28, 88(sp)
    lw x29, 92(sp)
    lw x30, 96(sp)
    lw x31, 100(sp)

    addi sp, sp, 256

    // 返回异常处理
    mret

.global trap_entry_end
trap_entry_end:
