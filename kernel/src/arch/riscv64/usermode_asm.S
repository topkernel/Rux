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
    // 调试输出：进入函数
    li t4, 0x10000000     // UART base
    li t3, 0x4C           // 'L'
    sb t3, 0(t4)

    // 保存参数到临时寄存器
    mv t5, a0              // t5 = entry
    mv t6, a1              // t6 = user_stack

    // 调试输出：参数已保存
    li t3, 0x31           // '1'
    sb t3, 0(t4)

    // 参考 Linux: regs->status = SR_PIE | SR_UXL_64
    // SR_PIE = 0x20 (Previous Interrupt Enable)
    // SR_UXL_64 = 0x200000000 (U-mode XLEN = 64)
    // 关键：需要确保 SPP=0 才能返回用户模式！
    // SPP (bit 8) = 0 表示从 U-mode 返回
    // 我们需要先读取当前 sstatus，清除 SPP，然后设置其他位

    csrr t1, sstatus       // 读取当前 sstatus
    li t0, 0x20000020      // SR_UXL_64 | SR_PIE
    and t1, t1, -257       // 清除低 9 位 (包括 SPP)
    or t0, t0, t1          // 合并
    csrw sstatus, t0

    // 调试输出：sstatus已设置
    li t3, 0x32           // '2'
    sb t3, 0(t4)

    // 设置用户程序入口点
    csrw sepc, t5

    // 调试输出：sepc已设置
    li t3, 0x33           // '3'
    sb t3, 0(t4)

    // 刷新指令缓存和TLB
    fence.i
    sfence.vma

    // 调试输出：cache已刷新
    li t3, 0x34           // '4'
    sb t3, 0(t4)

    // 设置用户栈指针
    mv sp, t6

    // 调试输出：栈已设置
    li t3, 0x35           // '5'
    sb t3, 0(t4)

    // 关键：不切换 satp！使用当前页表（内核页表）
    // Linux使用单一页表，通过U-bit控制权限
    // 直接执行 sret 返回用户模式

    // 调试输出：准备sret
    li t3, 0x36           // '6'
    sb t3, 0(t4)

    sret                    // 返回用户模式

    // 不应该到达这里
1:  j 1b
