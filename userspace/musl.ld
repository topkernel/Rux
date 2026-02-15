/*
 * Rux OS - musl 程序链接器脚本
 *
 * 用户空间程序内存布局 (Sv39 虚拟地址空间)
 *
 * 与 Linux RISC-V 用户空间布局兼容
 */

OUTPUT_FORMAT("elf64-littleriscv")
OUTPUT_ARCH(riscv)
ENTRY(_start)

/* 用户空间地址范围 */
USER_SPACE_BASE = 0x10000;

/* 内存区域定义 */
MEMORY
{
    /* 代码段: 从 0x10000 开始，1MB */
    TEXT (rx)  : ORIGIN = 0x10000, LENGTH = 1M

    /* 数据段: 代码段之后，1MB */
    DATA (rwx) : ORIGIN = 0x110000, LENGTH = 1M

    /* 堆: 数据段之后，8MB */
    HEAP (rw)  : ORIGIN = 0x210000, LENGTH = 8M

    /* 栈: 高地址向下增长，1MB */
    STACK (rw) : ORIGIN = 0x7FFFF000, LENGTH = 1M
}

SECTIONS
{
    /* 代码段 */
    .text : ALIGN(4096)
    {
        *(.text.init)       /* 启动代码 */
        *(.text .text.*)    /* 所有代码 */
    } > TEXT

    /* 只读数据 */
    .rodata : ALIGN(4096)
    {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        *(.gcc_except_table .gcc_except_table.*)
    } > TEXT

    /* 初始化数据 */
    .data : ALIGN(4096)
    {
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        __data_end = .;
    } > DATA

    /* 未初始化数据 (BSS) */
    .bss : ALIGN(4096)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    } > DATA

    /* 堆起始标记 */
    .heap : ALIGN(4096)
    {
        __heap_start = .;
        . = ORIGIN(HEAP) + LENGTH(HEAP);
        __heap_end = .;
    } > HEAP

    /* 栈 (不初始化) */
    .stack (NOLOAD) : ALIGN(16)
    {
        __stack_bottom = .;
        . = ORIGIN(STACK) + LENGTH(STACK) - 16;
        __stack_top = .;
    } > STACK

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
PROVIDE(__stack_top = ORIGIN(STACK) + LENGTH(STACK) - 16);
PROVIDE(__heap_start = ORIGIN(HEAP));
PROVIDE(__heap_end = ORIGIN(HEAP) + LENGTH(HEAP));
