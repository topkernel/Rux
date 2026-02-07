//! RISC-V 异常向量表
//!
//! RISC-V 的异常向量表由 stvec 寄存器指向
//! BASE 地址必须是 4 字节对齐
//!
//! Trampoline 页：这个页在内核和用户页表中都有映射
//! 从用户模式进入陷阱时，CPU 跳转到这个页执行

.section .text.trampoline
.align 12  // 4KB 对齐，确保 trampoline 占据完整的页
.global trap_entry
.global trampoline_start
.global trampoline_end

trampoline_start:
    // trampoline 从这里开始，确保在页的起始位置

// 异常向量表格式
// stvec[1:0] = 0 => Direct mode
// 所有异常跳转到 trap_entry
trap_entry:
    // 交换 sp 和 sscratch
    // csrrw sp, sscratch, sp 执行：sp = sscratch, sscratch = sp
    // 这确实是一个原子交换操作！
    csrrw sp, sscratch, sp

    // 现在 sp 可能是内核栈或用户栈
    // 如果 sscratch（旧 sp）是 0，则来自内核模式
    // 如果 sscratch（旧 sp）不是 0，则来自用户模式

    // 检查 sscratch 是否为 0（判断来自内核还是用户）
    // 注意：此时 sscratch 包含旧的 sp 值
    csrr t0, sscratch
    bnez t0, from_user

from_kernel:
    // 来自内核模式，sp 不变（内核栈）
    // 不需要保存任何额外的栈指针
    j save_regs

from_user:
    // 来自用户模式，sp 现在指向内核栈
    // t0 包含用户栈指针（旧的 sp 值，现在在 sscratch 中）
    // 将用户栈指针保存到内核栈的一个特殊位置（在 TrapFrame 之外）
    addi sp, sp, -280  // 多预留 8 字节用于保存用户栈指针

save_regs:
    // 保存调用者寄存器 (使用 64位指令)
    // 注意：此时 sp 已经减去了 272（或 280，如果来自用户模式）

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

    // 检查是否来自用户模式
    // 如果 sp 是 272 字节对齐，则来自内核模式
    // 如果 sp 是 280 字节对齐，则来自用户模式
    // 使用简单方法：检查 sscratch 是否为 0
    csrr t0, sscratch
    bnez t0, restore_user_sp

    // 来自内核模式：sp += 272
    addi sp, sp, 272
    j do_sret

restore_user_sp:
    // 来自用户模式：sp += 280，然后恢复用户栈指针
    addi sp, sp, 280
    // 恢复用户栈指针
    csrrw sp, sscratch, sp  # sp = sscratch (user stack), sscratch = sp (kernel stack)

do_sret:
    // 返回异常处理 (使用 S-mode 的 sret)
    sret

.global trap_entry_end
trap_entry_end:

trampoline_end:
    // trampoline 在这里结束
