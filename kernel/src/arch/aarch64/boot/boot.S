.section .text.secondary_entry
.global secondary_entry

secondary_entry:
    /* 次核启动入口点 */
    /* 注意：PSCI 确保 CPU 通过此入口启动，所以这里不应该是 CPU 0 */
    /* 但为了安全，仍然检查 CPU ID */
    mrs     x1, mpidr_el1
    and     x1, x1, #0xFF        /* 获取 CPU ID (Aff0) */
    cbz     x1, hang_secondary  /* CPU 0 不应该到达这里 */

    /* ========== 次核路径 ========== */
    /* PSCI 确保 CPU 通过此入口启动，无需 spin table 等待 */

    /* 获取 per-CPU 栈 */
    mrs     x1, mpidr_el1
    and     x1, x1, #0xFF        /* CPU ID */
    ldr     x2, =__per_cpu_stacks_start
    lsl     x1, x1, #14          /* 每个栈 16KB (2^14) */
    add     sp, x2, x1
    add     sp, sp, #0x4000      /* 栈顶 */

    /* 跳转到 Rust 次核入口 */
    bl      secondary_cpu_start

    /* 永远不应该到达这里 */
hang_secondary:
    wfe
    b       hang_secondary

    /* ========== 符号表 ========== */
.global smp_spin_table
smp_spin_table:
    .quad 0

.section .text.init
.global __boot_start

__boot_start:
    /* ========== 首先检查是否是次核 ========== */
    mrs     x1, mpidr_el1
    and     x1, x1, #0xFF        /* 获取 CPU ID (Aff0) */
    cbnz    x1, secondary_entry  /* 次核跳转到次核入口 */

    /* ========== 主核 (CPU 0) 路径 ========== */

    /* 检查当前异常级别 */
    mrs     x0, CurrentEL
    and     x0, x0, #0xC        /* 提取EL [3:2] */
    cmp     x0, #0x8            /* EL2? */
    b.eq    el2_entry
    cmp     x0, #0xC            /* EL3? */
    b.eq    el3_entry
    /* 否则是EL1，直接继续 */

el1_entry:
    /* 设置栈指针 - 使用adr获取栈顶地址 */
    adr     x30, boot_stack_top
    mov     sp, x30
    b       rust_entry

el2_entry:
    /* 从EL2降到EL1 */
    mov     x0, #(1 << 31)      /* EL1h, AArch64 */
    msr     spsr_el2, x0
    adr     x0, el1_entry
    msr     elr_el2, x0

    /* 不启用MMU，直接返回到EL1 */
    eret

el3_entry:
    /* 从EL3降到EL2 */
    mov     x0, #(1 << 31)      /* EL2h, AArch64 */
    msr     spsr_el3, x0
    adr     x0, el2_entry
    msr     elr_el3, x0

    /* 设置SCR_EL3 */
    mrs     x0, scr_el3
    orr     x0, x0, #(1 << 10)  /* RW = 1, AArch64 */
    msr     scr_el3, x0

    isb
    eret

rust_entry:
    /* 清零BSS段 */
    ldr     x0, __bss_start
    ldr     x1, __bss_end
    mov     x2, #0
.clear_bss:
    cmp     x0, x1
    b.ge    .bss_cleared
    str     x2, [x0], #8
    b       .clear_bss

.bss_cleared:
    /* 设置内核栈 */
    adr     x0, boot_stack_top
    mov     sp, x0

    /* 跳转到Rust代码 */
    bl      _start

    /* 永不返回 */
    hang:
    wfe
    b       hang

/* 定义栈符号 - 由linker script分配空间 */
.global boot_stack_bottom
.global boot_stack_top
boot_stack_bottom = __boot_stack_start;
boot_stack_top = __boot_stack_end;
