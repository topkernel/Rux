// ARM64 异常向量表
//
// ARMv8 异常向量表布局（0x000开始的2KB对齐）：
// 每个异常类型有4种指令指针选择（SP选择、EL选择）
// 总共16个异常处理入口，每个128字节

.section .text.vbar
.align 11  // 2KB 对齐 (异常向量表必须2KB对齐)
.global vector_table

vector_table:
    // ==================== 当前异常级别，SP0 (从EL0进入) ====================

    // 0x000: 同步异常
    vbar_el1_sp0_sync:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0x0
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x080: IRQ异常
    vbar_el1_sp0_irq:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0x1
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x100: FIQ异常
    vbar_el1_sp0_fiq:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0x2
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x180: SError异常
    vbar_el1_sp0_serror:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0x3
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // ==================== 当前异常级别，SPx (从EL1进入) ====================

    // 0x200: 同步异常
    vbar_el1_spx_sync:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0x4
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x280: IRQ异常
    vbar_el1_spx_irq:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0x5
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x300: FIQ异常
    vbar_el1_spx_fiq:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0x6
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x380: SError异常
    vbar_el1_spx_serror:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0x7
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // ==================== 从较低异常级别进入 (EL0) ====================

    // 0x400: 同步异常 - 包括系统调用
    vbar_el1_el0_sync:
        // 保存所有寄存器（包括x8用于系统调用号）
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]       // x8保存系统调用号
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]

        mrs     x1, elr_el1
        mrs     x2, spsr_el1
        mrs     x3, esr_el1
        stp     x1, x3, [sp, #248]
        str     x2, [sp, #264]

        mov     x0, #0x8                // 异常类型
        mov     x1, sp                  // 传递栈指针作为上下文指针
        bl      trap_handler

        // 恢复用户寄存器（从栈帧中恢复）
        // 注意：trap_handler可能修改了栈帧中的值（如x0的返回值）
        ldp     x2, x3, [sp, #16]
        ldp     x4, x5, [sp, #32]
        ldp     x6, x7, [sp, #48]
        ldp     x8, x9, [sp, #64]
        ldp     x10, x11, [sp, #80]
        ldp     x12, x13, [sp, #96]
        ldp     x14, x15, [sp, #112]
        ldp     x16, x17, [sp, #128]
        ldp     x18, x19, [sp, #144]
        ldp     x20, x21, [sp, #160]
        ldp     x22, x23, [sp, #176]
        ldp     x24, x25, [sp, #192]
        ldp     x26, x27, [sp, #208]
        ldp     x28, x29, [sp, #224]
        ldr     x30, [sp, #240]
        // 恢复 elr 和 spsr
        ldp     x2, x3, [sp, #248]   // x2=elr, x3=esr
        ldr     x4, [sp, #264]       // x4=spsr
        // 恢复 x0, x1 并调整栈指针
        ldp     x0, x1, [sp], #272
        // 恢复 ELR 和 SPSR（从临时寄存器）
        msr     elr_el1, x2
        msr     spsr_el1, x4
        eret

    // 0x480: IRQ异常
    vbar_el1_el0_irq:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0x9
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x500: FIQ异常
    vbar_el1_el0_fiq:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0xA
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x580: SError异常
    vbar_el1_el0_serror:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0xB
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x600: 同步异常 - 32位指令
    // 注意：这个向量也会被某些 AArch64 同步异常触发
    // 我们需要像 0x400 向量那样保存 ESR/ELR/SPSR
    vbar_el1_el0_sync_32:
        // 保存所有寄存器
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]

        // 保存异常状态寄存器
        mrs     x1, elr_el1
        mrs     x2, spsr_el1
        mrs     x3, esr_el1
        stp     x1, x3, [sp, #248]
        str     x2, [sp, #264]

        mov     x0, #0xC                // 异常类型
        mov     x1, sp                  // 传递栈指针作为上下文指针
        bl      trap_handler

        // 恢复寄存器
        ldp     x0, x1, [sp], #256
        eret

    // 0x680: IRQ异常 - 32位指令
    vbar_el1_el0_irq_32:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0xD
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x700: FIQ异常 - 32位指令
    vbar_el1_el0_fiq_32:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0xE
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

    // 0x780: SError异常 - 32位指令
    vbar_el1_el0_serror_32:
        stp     x0, x1, [sp, #-256]!
        stp     x2, x3, [sp, #16]
        stp     x4, x5, [sp, #32]
        stp     x6, x7, [sp, #48]
        stp     x8, x9, [sp, #64]
        stp     x10, x11, [sp, #80]
        stp     x12, x13, [sp, #96]
        stp     x14, x15, [sp, #112]
        stp     x16, x17, [sp, #128]
        stp     x18, x19, [sp, #144]
        stp     x20, x21, [sp, #160]
        stp     x22, x23, [sp, #176]
        stp     x24, x25, [sp, #192]
        stp     x26, x27, [sp, #208]
        stp     x28, x29, [sp, #224]
        str     x30, [sp, #240]
        mov     x0, #0xF
        bl      trap_handler
        ldp     x0, x1, [sp], #256
        eret

.size vector_table, . - vector_table

// 异常向量表结束标记
.global vector_table_end
vector_table_end:
