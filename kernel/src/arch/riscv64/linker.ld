/* RISC-V 64-bit kernel linker script - Linux style
 *
 * Kernel is linked at PAGE_OFFSET + physical load address
 * This ensures all kernel code references use kernel virtual addresses
 * PAGE_OFFSET = 0xffffffd600000000 for Sv39 (Linux PAGE_OFFSET_L3)
 *
 * Physical load address: 0x80200000 (after OpenSBI)
 * Virtual address: PAGE_OFFSET + 0x200000 = 0xffffffd602000000
 */

OUTPUT_ARCH(riscv)
ENTRY(_start)

/* PAGE_OFFSET for Sv39 (from Linux page.h: PAGE_OFFSET_L3) */
PAGE_OFFSET = 0xffffffd600000000;

/* Physical load address */
PHYS_LOAD_ADDR = 0x80200000;

/* Virtual address = PAGE_OFFSET + offset from RAM base */
KERNEL_VIRT_ADDR = PAGE_OFFSET + 0x200000;

SECTIONS
{
    /* Kernel code linked at virtual address PAGE_OFFSET + 0x200000 */
    . = KERNEL_VIRT_ADDR;

    _start = .;
    _stext = .;

    /* .text section: Code segment */
    .text : AT(PHYS_LOAD_ADDR) {
        KEEP(*(.text.entry))
        *(.init)
        . = ALIGN(4);
        KEEP(*(.text.*))
        *(.text)
    }

    _etext = .;

    /* .rodata section: Read-only data */
    .rodata : {
        *(.rodata .rodata.*)
    }

    /* Exception table section */
    .ex_table : {
        . = ALIGN(8);
        __ex_table_start = .;
        KEEP(*(.ex_table .ex_table.*))
        . = ALIGN(8);
        __ex_table_end = .;
    }

    /* .data section: Initialized data segment */
    .data : {
        *(.data .data.*)
        . = ALIGN(16);
    }

    /* .bss section: Uninitialized data segment */
    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* Stack guard region (4KB) */
    .stack_guard . (NOLOAD) : {
        . = ALIGN(4096);
        . += 4096;
    }

    /* Stack section (4 CPUs x 64KB = 256KB) */
    .stack . (NOLOAD) : {
        _stack_bottom = .;
        . = ALIGN(16);
        . += 262144;
        _stack_top = .;
    }

    /* End of kernel image */
    _end = .;

    /DISCARD/ : {
        *(.comment .note*)
    }
}
