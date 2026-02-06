//! RISC-V 64位内核启动代码
//!
//! 入口点，负责：
//! - 设置栈指针
//! - 清零 BSS 段
//! - 跳转到 Rust 代码

.section .init
.global _start

_start:
    // 1. 设置栈指针
    // 栈大小: 16KB
    la sp, _stack_top

    // 2. 清零 BSS 段
    la t0, __bss_start
    la t1, __bss_end
_bss_clear_loop:
    beq t0, t1, _bss_clear_done
    sw zero, 0(t0)
    addi t0, t0, 8
    j _bss_clear_loop
_bss_clear_done:

    // 3. 跳转到 Rust 代码 (boot.rs 的 _start 函数)
    call _start

.section .bss
.align 3
_stack_bottom:
    .skip 16384  // 16KB 栈
_stack_top:

    .align 3
__bss_start:
    .skip __bss_end - __bss_start
__bss_end:
