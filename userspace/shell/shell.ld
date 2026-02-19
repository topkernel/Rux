/*
 * Rux OS - musl 程序简化链接脚本
 *
 * 把栈放在数据段内，避免创建单独的 PT_LOAD 段
 */

OUTPUT_FORMAT("elf64-littleriscv")
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY
{
    /* 统一的用户空间内存区域 */
    USER (rwx) : ORIGIN = 0x10000, LENGTH = 1M
}

SECTIONS
{
    /* 代码段 */
    .text : ALIGN(4096)
    {
        *(.text.init)
        *(.text .text.*)
    } > USER

    /* 只读数据 */
    .rodata : ALIGN(4096)
    {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    } > USER

    /* 初始化数据 */
    .data : ALIGN(4096)
    {
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        *(.got .got.*)
        *(.got.plt)
        __data_end = .;
    } > USER

    /* 未初始化数据 (BSS) */
    .bss : ALIGN(16)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    } > USER

    /* RISC-V 全局指针符号
     * __global_pointer$ 用于 gp 相对寻址
     * 必须放在小数据段附近（.sdata + .sbss）
     * 通常放在 .sdata 开头或 .sbss 开头
     */
    PROVIDE(__global_pointer$ = __bss_start);

    /* 栈段 - 16KB */
    .stack (NOLOAD) : ALIGN(16)
    {
        __stack_bottom = .;
        . += 16384;  /* 16KB 栈 */
        __stack_top = .;
    } > USER

    /* 堆 - 剩余空间 */
    .heap (NOLOAD) : ALIGN(4096)
    {
        __heap_start = .;
        . = ORIGIN(USER) + LENGTH(USER);
        __heap_end = .;
    } > USER

    /* 程序结束标记 */
    _end = .;
    __end = .;

    /* 丢弃不需要的段 */
    /DISCARD/ :
    {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
        *(.dynsym)
        *(.dynstr)
        *(.dynamic)
        *(.plt)
        *(.interp)
        *(.gnu.hash)
        *(.hash)
    }
}

/* 提供符号给 C 代码使用 */
PROVIDE(__stack_top = __stack_top);
PROVIDE(__heap_start = __heap_start);
PROVIDE(__heap_end = __heap_end);
