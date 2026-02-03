ENTRY(__boot_start)

SECTIONS {
    . = 0x40000000;

    .text : {
        *(.text.init)
        *(.text.vbar)
        *(.text .text.*)
    }

    /* 强制导出 __rust_alloc 符号为全局可见 */
    PROVIDE(__rust_alloc = _RNvCsvJHrxua5YM_7___rustc12___rust_alloc);
    PROVIDE(__rust_alloc_zeroed = _RNvCsvJHrxua5YM_7___rustc19___rust_alloc_zeroed);

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

    /* 栈空间 */
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
