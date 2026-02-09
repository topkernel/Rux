/* 用户程序链接器脚本 - RISC-V 64位 */
/* 将用户程序链接到用户空间地址 */

ENTRY(_start)

/* 用户空间内存布局 */
MEMORY
{
    /* 用户代码段：0x10000 - 0x20000 */
    USER_CODE (rx)  : ORIGIN = 0x10000, LENGTH = 64K
    /* 用户数据段：0x20000 - 0x30000 */
    USER_DATA (rw)  : ORIGIN = 0x20000, LENGTH = 64K
    /* 用户栈：0x3ffe8000 (由内核设置) */
}

SECTIONS
{
    /* 代码段 */
    .text : {
        *(.text.entry)
        *(.text .text.*)
    } > USER_CODE

    /* 只读数据段 */
    .rodata : {
        *(.rodata .rodata.*)
    } > USER_CODE

    /* 数据段 */
    .data : {
        *(.data .data.*)
    } > USER_DATA

    /* BSS 段 */
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } > USER_DATA

    /* 栈（不在这里分配，由内核设置） */
    /DISCARD/ : {
        *(.comment .note .note.* .gnu*)
    }
}
