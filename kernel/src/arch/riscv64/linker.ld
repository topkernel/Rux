/* RISC-V 64位内核链接脚本 */

ENTRY(_start)

MEMORY
{
    /* QEMU virt 机器的内存布局
     * 0x80000000 - 0x8001ffff: OpenSBI firmware (约 128KB)
     * 0x80200000+: 内核代码
     */
    RAM : ORIGIN = 0x80200000, LENGTH = 126M
}

SECTIONS
{
    /* .text 段: 代码段 */
    .text : {
        *(.text.entry)
        *(.init)
        . = ALIGN(4);
        *(.text.trampoline)   /* 异常向量表 */
        *(.text.*)
        *(.text)
        *(.rodata .rodata.*)
    } > RAM

    /* .data 段: 初始化数据段 */
    .data : {
        *(.data .data.*)
        . = ALIGN(16);
    } > RAM

    /* .bss 段: 未初始化数据段 */
    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    } > RAM

    /* 用户栈段 (用于用户程序测试) */
    .bss.user_stack . (NOLOAD) : {
        _user_stack_bottom = .;
        . = ALIGN(16);
        . += 16384; /* 16KB 用户栈 */
        _user_stack_top = .;
    } > RAM

    /* 栈段 (4 CPUs × 16KB = 64KB) */
    .stack . (NOLOAD) : {
        _stack_bottom = .;
        . = ALIGN(16);
        . += 65536; /* 64KB 栈 (4 CPUs × 16KB) */
        _stack_top = .;
    } > RAM

    /* 提供内核符号信息 */
    /DISCARD/ : {
        *(.comment .note*)
    }
}
