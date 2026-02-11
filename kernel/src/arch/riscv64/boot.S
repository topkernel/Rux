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

    // 保存 a1 (DTB 指针) 到临时寄存器（在清零 BSS 之前）
    mv t0, a1

    // 计算 hart_id * 16KB
    li t1, 16384              // STACK_SIZE = 16KB
    mul t1, tp, t1            // t1 = hart_id * 16KB

    // 计算栈底地址
    la sp, _stack_bottom
    add sp, sp, t1            // sp = _stack_bottom + hart_id * 16KB

    // 加上 16KB 得到栈顶
    li t1, 16384
    add sp, sp, t1            // sp = 栈顶

    // 检查是否为 hart 0（主核）
    // 只有 hart 0 需要清零 BSS
    bnez tp, _skip_bss

    // ========== 清零 BSS 段（只有主核需要清零） ==========
    la t1, __bss_start
    la t2, __bss_end
_bss_clear_loop:
    beq t1, t2, _bss_clear_done
    sw zero, 0(t1)
    addi t1, t1, 8
    j _bss_clear_loop
_bss_clear_done:

    // ========== BSS 清零后，保存 DTB 指针到全局变量 ==========
    la t1, dtb_pointer
    sd t0, 0(t1)

_skip_bss:
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
    .section .bss
    .align 3
    // 设备树指针存储（8字节，64位）
dtb_pointer:
    .dword 0
