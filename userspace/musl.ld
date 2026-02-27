/*
 * Rux OS - musl 程序链接脚本
 *
 * 用于 Rust 程序（使用 std）
 * 基于 musl libc 的静态链接
 */

OUTPUT_FORMAT("elf64-littleriscv")
OUTPUT_ARCH(riscv)
ENTRY(_start)

/*
 * 用户空间程序起始地址
 * 留出一些空间给内核和保留区域
 */
USER_BASE = 0x10000;

SECTIONS
{
    . = USER_BASE;

    /* 代码段 */
    .text : ALIGN(4096)
    {
        *(.text.init)
        *(.text .text.*)
    }

    /* 只读数据 */
    .rodata : ALIGN(4096)
    {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    /* 初始化数据 */
    .data : ALIGN(4096)
    {
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        *(.got .got.*)
        *(.got.plt)
        __data_end = .;
    }

    /* 未初始化数据 (BSS) */
    .bss : ALIGN(16)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* RISC-V 全局指针 */
    PROVIDE(__global_pointer$ = __bss_start);

    /* TLS (Thread Local Storage) */
    .tdata : ALIGN(4096)
    {
        *(.tdata .tdata.*)
        *(.tbss .tbss.*)
    }

    /* 初始化数组 */
    .init_array : ALIGN(8)
    {
        __init_array_start = .;
        *(.init_array .init_array.*)
        __init_array_end = .;
    }

    /* 析构数组 */
    .fini_array : ALIGN(8)
    {
        __fini_array_start = .;
        *(.fini_array .fini_array.*)
        __fini_array_end = .;
    }

    /* EH frame (异常处理) */
    .eh_frame_hdr : ALIGN(4)
    {
        *(.eh_frame_hdr)
        *(.eh_frame_entry)
    }
    .eh_frame : ALIGN(4)
    {
        KEEP(*(.eh_frame))
    }

    /* 栈 - 64KB */
    .stack (NOLOAD) : ALIGN(16)
    {
        __stack_bottom = .;
        . += 65536;
        __stack_top = .;
    }

    /* 堆 - 剩余空间 */
    .heap (NOLOAD) : ALIGN(4096)
    {
        __heap_start = .;
        . = 0x100000;  /* 1MB 总空间 */
        __heap_end = .;
    }

    /* 程序结束标记 */
    _end = .;
    __end = .;

    /* 丢弃不需要的段 */
    /DISCARD/ :
    {
        *(.comment)
        *(.note*)
        *(.dynsym)
        *(.dynstr)
        *(.dynamic)
        *(.plt)
        *(.interp)
        *(.gnu.hash)
        *(.hash)
    }
}

/* 提供符号 */
PROVIDE(__stack_top = __stack_top);
PROVIDE(__heap_start = __heap_start);
PROVIDE(__heap_end = __heap_end);
