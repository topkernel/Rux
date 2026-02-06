//! RISC-V 64位内核启动代码
//!
//! 支持 SMP 多核启动
//!
//! OpenSBI 在跳转到内核时，a0 寄存器包含 hart ID

.section .init
.global _start

_start:
    // a0 寄存器包含 OpenSBI 传递的 hart ID
    // 保存 hart ID 到 tp (x4) 寄存器（thread pointer）
    mv tp, a0

    // 计算 hart_id * 16KB
    li t0, 16384              // STACK_SIZE = 16KB
    mul t1, tp, t0            // t1 = hart_id * 16KB

    // 计算栈底地址
    la sp, _stack_bottom
    add sp, sp, t1            // sp = _stack_bottom + hart_id * 16KB

    // 加上 16KB 得到栈顶
    li t0, 16384
    add sp, sp, t0            // sp = 栈顶

    // 检查是否为 hart 0（主核）
    // 只有 hart 0 需要清零 BSS
    bnez tp, _skip_bss

    // ========== 清零 BSS 段（只有主核需要清零） ==========
    la t0, __bss_start
    la t1, __bss_end
_bss_clear_loop:
    beq t0, t1, _bss_clear_done
    sw zero, 0(t0)
    addi t0, t0, 8
    j _bss_clear_loop
_bss_clear_done:

_skip_bss:
    // 跳转到 Rust 代码 (main.rs 的 rust_main 函数)
    // 所有 CPU 都从 rust_main 开始，SMP 初始化代码会区分主核和次核
    call rust_main

    // 不应返回，如果返回则停止
_hang:
    wfi
    j _hang
