/* RISC-V 64-bit kernel linker script
 *
 * Simplified version: Run at physical address first
 * Virtual address mapping will be set up later
 *
 * Memory layout:
 * - Physical load address: 0x80200000 (OpenSBI jumps here)
 * - All code linked at physical address
 */

OUTPUT_ARCH(riscv)
ENTRY(_start)

/* Physical load address (2MB from RAM start) */
KERNEL_PHYS = 0x80200000;

SECTIONS
{
	. = KERNEL_PHYS;

	_start = .;
	_stext = .;

	.text : ALIGN(4096) {
		KEEP(*(.text.entry))
		*(.init)
		. = ALIGN(4);
		KEEP(*(.text.*))
		*(.text)
	}

	_etext = .;

	/* Read-only data */
	. = ALIGN(4096);
	.rodata : {
		*(.rodata .rodata.*)
	}

	/* Exception table */
	. = ALIGN(4);
	__ex_table_start = .;
	.ex_table : {
		*(.ex_table)
	}
	__ex_table_end = .;

	/* Data section */
	. = ALIGN(4096);
	_sdata = .;
	.data : {
		*(.data .data.*)
		*(.sdata .sdata.*)
	}
	_edata = .;

	/* BSS section */
	. = ALIGN(4096);
	__bss_start = .;
	.bss : {
		*(.bss .bss.*)
		*(COMMON)
		. = ALIGN(16);
	}
	__bss_end = .;

	/* Stack (4 CPUs x 64KB = 256KB) */
	. = ALIGN(4096);
	_stack_bottom = .;
	. += 262144;
	_stack_top = .;

	_end = .;

	/DISCARD/ : {
		*(.comment .note*)
	}
}
