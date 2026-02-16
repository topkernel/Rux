//! RISC-V 64位内核启动代码
//!
//! 支持 SMP 多核启动
//!
//! OpenSBI 在跳转到内核时：
//! - a0 寄存器包含 hart ID
//! - a1 寄存器包含设备树指针（dtb）

.section .init
.global _start
.global dtb_pointer

_start:
    // a0 寄存器包含 OpenSBI 传递的 hart ID
    // 保存 hart ID 到 tp (x4) 寄存器（thread pointer）
    mv tp, a0

    // 保存 a1 (DTB 指针) 到 s0 寄存器（callee-saved，在整个函数中保持）
    mv s0, a1

    // 计算 hart_id * 64KB
    li t1, 65536              // STACK_SIZE = 64KB
    mul t1, tp, t1            // t1 = hart_id * 64KB

    // 计算栈底地址
    la sp, _stack_bottom
    add sp, sp, t1            // sp = _stack_bottom + hart_id * 64KB

    // 加上 64KB 得到栈顶
    li t1, 65536
    add sp, sp, t1            // sp = 栈顶

    // ========== 清零 BSS 段（仅第一个 hart 执行） ==========
    //
    // 修复：使用原子标志确保只有一个 hart 清零 BSS
    // 防止次核启动时覆盖已初始化的全局变量
    //
    // bss_initialized 在 .data 段，初始值为 0
    // 使用 amoadd.w 指令原子地检查并设置

    la t1, bss_initialized
    li t2, 1
    amoadd.w t2, t2, (t1)     // t2 = *t1; *t1 += 1 (原子操作)
    bnez t2, _bss_clear_done  // 如果 t2 != 0，说明已经有 hart 清零过了

    // 第一个 hart 执行 BSS 清零
    la t1, __bss_start
    la t2, __bss_end
_bss_clear_loop:
    beq t1, t2, _bss_clear_done
    sd zero, 0(t1)           // 使用 64 位存储对齐
    addi t1, t1, 8
    j _bss_clear_loop
_bss_clear_done:

    // 内存屏障：确保 BSS 清零对所有 hart 可见
    fence rw, rw

    // ========== 保存 DTB 指针 ==========
    // 使用 s0（之前保存的 DTB 指针）
    // 只有当 s0 非零时才写入
    beqz s0, _skip_dtb
    la t1, dtb_pointer
    sd s0, 0(t1)
_skip_dtb:

    // 跳转到 Rust 代码
    call rust_main

    // 不应返回，如果返回则停止
_hang:
    wfi
    j _hang

    // ========== 数据段 ==========
    // bss_initialized 和 dtb_pointer 放在 .data 段
    .section .data
    .align 3
    .global bss_initialized
bss_initialized:
    .word 0
    .align 3
    .global dtb_pointer
dtb_pointer:
    .dword 0
