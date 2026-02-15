/*
 * Rux OS - musl 程序链接器脚本
 *
 * 用户空间程序内存布局 (Sv39 虚拟地址空间)
 * 精简版：减小内存占用以适应内核 ELF 加载器
 */

OUTPUT_FORMAT("elf64-littleriscv")
OUTPUT_ARCH(riscv)
ENTRY(_start)

/* 内存区域定义 - Rust std 版本 */
MEMORY
{
    /* 代码段: 从 0x10000 开始，1MB (Rust std 需要更大空间) */
    TEXT (rx)  : ORIGIN = 0x10000, LENGTH = 1M

    /* 数据段: 代码段之后，512KB */
    DATA (rwx) : ORIGIN = 0x110000, LENGTH = 512K

    /* 堆: 数据段之后，2MB */
    HEAP (rw)  : ORIGIN = 0x190000, LENGTH = 2M

    /* 栈: 128KB，在堆之后 */
    STACK (rw) : ORIGIN = 0x390000, LENGTH = 128K
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

    /* 堆 - 小型分配 */
    .heap : ALIGN(4096)
    {
        __heap_start = .;
        . = ORIGIN(HEAP) + LENGTH(HEAP);
        __heap_end = .;
    } > HEAP

    /* 栈 */
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
