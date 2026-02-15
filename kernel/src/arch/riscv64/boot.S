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
    // 这样即使 BSS 被其他 hart 清零，我们也能恢复 DTB 指针
    mv s0, a1

    // 计算 hart_id * 16KB
    li t1, 16384              // STACK_SIZE = 16KB
    mul t1, tp, t1            // t1 = hart_id * 16KB

    // 计算栈底地址
    la sp, _stack_bottom
    add sp, sp, t1            // sp = _stack_bottom + hart_id * 16KB

    // 加上 16KB 得到栈顶
    li t1, 16384
    add sp, sp, t1            // sp = 栈顶

    // ========== 清零 BSS 段（所有 hart 都清零） ==========
    //
    // 修复：OpenSBI 可能从任意 hart 启动内核（不限于 hart 0）
    // 如果只有 hart 0 清零 BSS，其他 hart 启动时全局变量（如 SMP 静态变量）
    // 会包含未初始化的随机值，导致系统卡住
    //
    // 解决方案：所有 hart 都清零 BSS（多次清零不会导致问题）
    //
    // 注意：dtb_pointer 在 .data 段（不在 .bss），不会被清零

    la t1, __bss_start
    la t2, __bss_end
_bss_clear_loop:
    beq t1, t2, _bss_clear_done
    sd zero, 0(t1)           // 使用 64 位存储对齐
    addi t1, t1, 8
    j _bss_clear_loop
_bss_clear_done:

    // ========== BSS 清零后，保存 DTB 指针到全局变量 ==========
    // 使用 s0（之前保存的 DTB 指针）
    // 只有当 s0 非零时才写入（避免次核覆盖 boot hart 的 DTB 指针）
    beqz s0, _skip_dtb
    la t1, dtb_pointer
    sd s0, 0(t1)
_skip_dtb:

    // 跳转到 Rust 代码
    // RISC-V SMP 启动流程：
    // 1. OpenSBI 可以从任意 hart 启动内核
    // 2. 第一个到达的 hart 成为启动核（通过 smp::init() 中的 CAS 操作）
    // 3. 启动核执行完整初始化（trap、MMU、SMP、PLIC、IPI、Timer）
    // 4. 次核由启动核通过 SBI HSM 唤醒，也调用 rust_main()
    // 5. 在 smp::init() 中，非启动核检测到已被启动核初始化，直接进入 WFI

    call rust_main

    // 不应返回，如果返回则停止
_hang:
    wfi
    j _hang

    // ========== 数据段 ==========
    // dtb_pointer 放在单独的 .data 段，不会被 BSS 清零
    // 这样即使次核运行 BSS 清零循环，也不会覆盖 boot hart 写入的值
    .section .data
    .align 3
    .global dtb_pointer
dtb_pointer:
    .dword 0
