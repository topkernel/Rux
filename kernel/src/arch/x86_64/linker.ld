/* MIT License
 *
 * Copyright (c) 2026 Fei Wang
 *
 * x86_64 kernel linker script.
 *
 * Two load regions:
 *   - .boot  : 32-bit multiboot entry stub + early page tables, VMA=LMA=0x100000
 *              (QEMU -kernel ELF enters here with paging off, 32-bit protected mode)
 *   - kernel : higher-half VMA 0xffffffff80200000, LMA 0x200000
 *              VMA - LMA == 0xffffffff80000000 (constant, like __START_KERNEL_map
 *              on x86 Linux; the early 2MB pages map phys i*2MB -> VMA base + i*2MB)
 */

OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

KERNEL_VMA = 0xffffffff80000000;
BOOT_BASE = 0x100000;
KERNEL_LMA = 0x200000;

SECTIONS
{
    /* ---- 32-bit boot stub (identity-mapped, page-aligned) ---- */
    . = BOOT_BASE;

    .boot : ALIGN(4096)
    {
        KEEP(*(.multiboot_header))
        *(.boot.text)
        *(.boot.data)
        . = ALIGN(4096);
        __boot_page_tables_start = .;
        *(.boot.pt)
        . = ALIGN(4096);
        __boot_page_tables_end = .;
    }

    .boot_stack (NOLOAD) : ALIGN(16)
    {
        __boot_stack_bottom = .;
        . += 16K;
        __boot_stack_top = .;
    }

    . = ALIGN(4096);
    __boot_end = .;

    /* ---- 64-bit higher-half kernel ---- */
    . = KERNEL_VMA + KERNEL_LMA;

    __kernel_start = .;
    __kernel_lma_start = KERNEL_LMA;

    .text : AT(KERNEL_LMA)
    {
        __stext = .;
        KEEP(*(.text.boot64))          /* first instructions in the high map */
        *(.text .text.*)
        __etext = .;
    }

    .rodata : AT(LOADADDR(.text) + SIZEOF(.text))
    {
        __srodata = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        . = ALIGN(4096);
        __ex_table_start = .;
        KEEP(*(__ex_table))
        __ex_table_end = .;
        . = ALIGN(4096);
        __end_rodata = .;
    }

    .data : AT(LOADADDR(.rodata) + SIZEOF(.rodata))
    {
        __sdata = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        . = ALIGN(4096);
        *(.bss.stack)                    /* per-CPU IRQ stacks etc. */
        __edata = .;
    }

    .bss : AT(LOADADDR(.data) + SIZEOF(.data))
    {
        __sbss = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        . = ALIGN(4096);
        __ebss = .;
    }

    . = ALIGN(4096);
    __kernel_end = .;
    __kernel_lma_end = KERNEL_LMA + (__kernel_end - (KERNEL_VMA + KERNEL_LMA));

    /DISCARD/ :
    {
        *(.eh_frame .eh_frame_hdr)
        *(.note .note.*)
        *(.comment)
    }
}
