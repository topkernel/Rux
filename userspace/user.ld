/* RISC-V 用户程序链接脚本 */

ENTRY(_start)

/* 用户程序内存布局
 *
 * RISC-V 64位 用户空间地址范围（Sv39）：
 * - 0x0000_0000_0000_0000 ~ 0x0000_ffff_ffff_ffff: 用户空间
 * - 内核将程序加载到合适的虚拟地址
 *
 * 这是一个简单的布局，内核加载器会在实际加载时重定位
 */

MEMORY
{
    /* 用户空间内存（虚拟地址）
     * 实际地址由内核的 ELF 加载器决定
     */
    USER (rwx) : ORIGIN = 0x10000, LENGTH = 1M
}

SECTIONS
{
    /* .text 段: 代码段 */
    .text : {
        *(.text.init)
        *(.text .text.*)
    } > USER

    /* .rodata 段: 只读数据 */
    .rodata : {
        *(.rodata .rodata.*)
    } > USER

    /* .data 段: 初始化数据 */
    .data : {
        *(.data .data.*)
        . = ALIGN(16);
    } > USER

    /* .bss 段: 未初始化数据 */
    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    } > USER

    /* 栈段（仅用于大小计算） */
    .stack (NOLOAD) : {
        . = ALIGN(16);
        __stack_bottom = .;
        . += 16384; /* 16KB 栈 */
        __stack_top = .;
    } > USER

    /* 提供符号信息 */
    /DISCARD/ : {
        *(.comment .note*)
    }
}
