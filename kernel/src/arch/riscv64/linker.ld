/* RISC-V 64-bit kernel linker script
 *
 * Linux-style: Link kernel at virtual address (KERNEL_LINK_ADDR)
 * Load at physical address (KERNEL_PHYS) via AT() directive
 *
 * boot.o is linked first via build.rs, ensuring _start is at KERNEL_PHYS.
 *
 * Memory layout:
 * - VMA (virtual):  0xffffffff80000000 (KERNEL_LINK_ADDR, last 2GB)
 * - LMA (physical): 0x80200000 (KERNEL_PHYS, where OpenSBI loads us)
 */

OUTPUT_ARCH(riscv)
ENTRY(_start)

KERNEL_PHYS = 0x80200000;
KERNEL_VIRT = 0xffffffff80000000;

/* VA-PA offset: used to compute LMA from VMA */
VA_OFFSET = KERNEL_VIRT - KERNEL_PHYS;

SECTIONS
{
	. = KERNEL_VIRT;

	_text = .;
	_stext = .;

	.text : AT(KERNEL_PHYS) ALIGN(4096)
	{
		KEEP(*(.text.entry))
		*(.text .text.*)
	}

	_etext = .;

	. = ALIGN(4096);
	.rodata : AT(ADDR(.rodata) - VA_OFFSET) { *(.rodata .rodata.*) }

	. = ALIGN(4);
	__ex_table_start = .;
	.ex_table : AT(ADDR(.ex_table) - VA_OFFSET) { *(.ex_table) }
	__ex_table_end = .;

	. = ALIGN(4096);
	_sdata = .;
	.data : AT(ADDR(.data) - VA_OFFSET)
	{
		*(.data .data.*)
		*(.sdata .sdata.*)
	}
	_edata = .;

	. = ALIGN(4096);
	__bss_start = .;
	.bss :
	{
		*(.bss .bss.*)
		*(COMMON)
		. = ALIGN(16);
	}
	__bss_end = .;

	. = ALIGN(4096);
	_stack_bottom = .;
	. += 262144;
	_stack_top = .;

	_end = .;

	/DISCARD/ :
	{
		*(.comment .note*)
		*(.eh_frame .eh_frame_hdr)
	}
}
