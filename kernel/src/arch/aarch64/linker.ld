ENTRY(__boot_start)

SECTIONS {
    . = 0x40000000;

    .text : {
        *(.text.init)
        *(.text.vbar)
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    .data : {
        *(.data .data.*)
    }

    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* Per-CPU 栈空间 (SMP) */
    . = ALIGN(16);
    . = ALIGN(0x4000);  /* 16KB 对齐 */
    __per_cpu_stacks_start = .;
    . += 0x4000 * 4;    /* 4 个 CPU，每个 16KB */
    __per_cpu_stacks_end = .;

    /* SMP spin table */
    . = ALIGN(8);
    __smp_spin_table = .;
    . += 8;

    /* 主核栈空间 */
    . = ALIGN(16);
    __boot_stack_start = .;
    . += 0x4000;
    __boot_stack_end = .;

    /* 页表空间 */
    __boot_page_table = .;
    . += 0x4000;

    /* 丢弃调试信息 */
    /DISCARD/ : {
        *(.comment .note .note.* .eh_frame*)
        *(.debug_abbrev .debug_info .debug_line .debug_str .debug_ranges)
        *(.debug_loc .debug_macinfo .debug_pubnames .debug_pubtypes)
        *(.debug_aranges .debug_frame .debug_rnglists .debug_str_offsets)
        *(.debug_addr .debug_line_str)
    }
}
