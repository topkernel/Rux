// 用户模式切换 - 纯汇编实现
//
// 使用汇编语言实现 switch_to_user，避免内联汇编的优化问题

.global switch_to_user_linux_asm

// switch_to_user_linux_asm(entry, user_stack)
// Linux风格的用户模式切换 - 单页表，不切换satp
// 参数：
//   a0 = entry point
//   a1 = user stack pointer
//
// 参考 Linux ret_from_exception() 实现
.section .text
switch_to_user_linux_asm:
    // 保存参数到临时寄存器
    mv t5, a0              // t5 = entry
    mv t6, a1              // t6 = user_stack

    // 参考 Linux: regs->status = SR_PIE | SR_UXL_64
    // SR_PIE = 0x20 (Previous Interrupt Enable)
    // SR_UXL_64 = 0x200000000 (U-mode XLEN = 64)
    // 关键：需要确保 SPP=0 才能返回用户模式！
    // SPP (bit 8) = 0 表示从 U-mode 返回
    // SPIE (bit 5) = 1 表示在用户模式使能中断

    // 方法：直接设置 sstatus 的关键位
    // 先读取当前 sstatus 保留其他位
    csrr t1, sstatus       // 读取当前 sstatus

    // 清除 SPP 位 (bit 8)，确保返回用户模式
    li t0, 0x100           // SPP 位的掩码
    not t0, t0             // 取反: 0xFFFFFFFEFFFFFFFF
    and t1, t1, t0         // 清除 SPP 位

    // 设置 SPIE 位 (bit 5) = 1，使能在用户模式的中断
    li t0, 0x20            // SPIE 位的掩码
    or t1, t1, t0          // 设置 SPIE

    // 设置 UXL (bits 63:62) = 2 (64-bit user mode)
    li t0, 0x200000000     // UXL = 2
    or t1, t1, t0          // 设置 UXL

    // 写入 sstatus
    csrw sstatus, t1

    // 设置用户程序入口点
    csrw sepc, t5

    // 刷新指令缓存和TLB
    fence.i
    sfence.vma

    // ===== 关键：确保 sscratch 指向内核栈 =====
    // 在切换到用户模式之前，我们需要确保 sscratch 包含内核栈指针
    // 这样当用户程序执行 ecall 时，trap 入口代码的 csrrw sp, sscratch, sp
    // 会将 sp 切换回内核栈
    //
    // 注意：sscratch 在 trap init 中已经设置为内核栈指针
    // 这里不需要修改，只需确认它的值是正确的
    csrr t1, sscratch      // 读取当前的 sscratch 值（应该是 trap 栈指针）

    // 设置用户栈指针（在 sret 之后生效）
    mv sp, t6             // sp = 用户栈指针

    // 关键：不切换 satp！使用当前页表（内核页表）
    // Linux使用单一页表，通过U-bit控制权限
    // 直接执行 sret 返回用户模式
    sret                    // 返回用户模式

    // 不应该到达这里
1:  j 1b
