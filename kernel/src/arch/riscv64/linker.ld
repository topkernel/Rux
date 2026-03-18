/* RISC-V 64-bit kernel linker script */

ENTRY(_start)

MEMORY
{
    /* QEMU virt machine memory layout
     * 0x80000000 - 0x8001ffff: OpenSBI firmware (~128KB)
     * 0x80200000+: Kernel code
     */
    RAM : ORIGIN = 0x80200000, LENGTH = 126M
}

SECTIONS
{
    /* .text section: Code segment */
    .text : {
        *(.text.entry)
        *(.init)
        . = ALIGN(4);
        *(.text.*)
        *(.text)
    } > RAM

    /* .rodata section: Read-only data */
    .rodata : {
        *(.rodata .rodata.*)
    } > RAM

    /* Exception table section: For exception fixup when kernel accesses user space
     * Each entry is a (insn_addr, fixup_addr) pair
     * When kernel causes an exception at insn_addr, jump to fixup_addr to continue
     */
    .ex_table : {
        . = ALIGN(8);
        __ex_table_start = .;
        KEEP(*(.ex_table .ex_table.*))
        . = ALIGN(8);
        __ex_table_end = .;
    } > RAM

    /* .data section: Initialized data segment */
    .data : {
        *(.data .data.*)
        . = ALIGN(16);
    } > RAM

    /* .bss section: Uninitialized data segment */
    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    } > RAM

    /* Stack guard region (4KB) - Prevents stack overflow from corrupting .bss */
    .stack_guard . (NOLOAD) : {
        . = ALIGN(4096);
        . += 4096; /* 4KB guard region */
    } > RAM

    /* Stack section (4 CPUs x 64KB = 256KB) */
    .stack . (NOLOAD) : {
        _stack_bottom = .;
        . = ALIGN(16);
        . += 262144; /* 256KB stack (4 CPUs x 64KB) */
        _stack_top = .;
    } > RAM

    /* Page table section removed - page tables are now dynamically allocated
     * from kernel heap via frame allocator (Linux-style)
     */

    /* Provide kernel symbol information */
    /DISCARD/ : {
        *(.comment .note*)
    }
}
