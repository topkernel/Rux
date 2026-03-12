/*
 * Rux OS - musl Program Simplified Linker Script
 *
 * Place stack within data segment to avoid creating separate PT_LOAD segment
 */

OUTPUT_FORMAT("elf64-littleriscv")
OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY
{
    /* Unified user space memory region */
    USER (rwx) : ORIGIN = 0x10000, LENGTH = 1M
}

SECTIONS
{
    /* Code segment */
    .text : ALIGN(4096)
    {
        *(.text.init)
        *(.text .text.*)
    } > USER

    /* Read-only data */
    .rodata : ALIGN(4096)
    {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    } > USER

    /* Initialized data */
    .data : ALIGN(4096)
    {
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        *(.got .got.*)
        *(.got.plt)
        __data_end = .;
    } > USER

    /* Uninitialized data (BSS) */
    .bss : ALIGN(16)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    } > USER

    /* RISC-V global pointer symbol
     * __global_pointer$ is used for gp-relative addressing
     * Must be placed near small data sections (.sdata + .sbss)
     * Typically placed at the start of .sdata or .sbss
     */
    PROVIDE(__global_pointer$ = __bss_start);

    /* Stack segment - 16KB */
    .stack (NOLOAD) : ALIGN(16)
    {
        __stack_bottom = .;
        . += 16384;  /* 16KB stack */
        __stack_top = .;
    } > USER

    /* Heap - remaining space */
    .heap (NOLOAD) : ALIGN(4096)
    {
        __heap_start = .;
        . = ORIGIN(USER) + LENGTH(USER);
        __heap_end = .;
    } > USER

    /* Program end marker */
    _end = .;
    __end = .;

    /* Discard unneeded sections */
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

/* Provide symbols for C code use */
PROVIDE(__stack_top = __stack_top);
PROVIDE(__heap_start = __heap_start);
PROVIDE(__heap_end = __heap_end);
