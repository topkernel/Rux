/* AArch64 kernel linker script — QEMU `virt` direct-boot (ELF).
 *
 * Linux-style: link the kernel at its virtual address (KERNEL_VIRT),
 * load at the physical address (KERNEL_PHYS = RAM base + text_offset,
 * 0x40080000) via AT() directives — same shape as the riscv64 script.
 *
 * The `.text.entry` section is placed FIRST: QEMU -kernel loads ELF
 * segments at their paddr and enters at e_entry (_start, in .text.entry
 * at the physical load address).
 *
 * Memory layout:
 * - VMA (virtual):  0xffffffff80080000 (KERNEL_VIRT — the 512KB-offset
 *                    position inside the early window whose base
 *                    0xffffffff80000000 maps phys 0x40000000)
 * - LMA (physical): 0x40080000 (KERNEL_PHYS, where QEMU loads us)
 * - VA_OFFSET = KERNEL_VIRT - KERNEL_PHYS (0xffffffff40000000)
 */

OUTPUT_ARCH(aarch64)
/* QEMU's arm -kernel loader jumps to e_entry with the MMU OFF, so the
 * entry must be the PHYSICAL address of _start (the riscv64 flow enters
 * through OpenSBI which handles the high VMA; aarch64 direct boot does
 * not). _start's code still lives at its high VMA/physical LMA -- only
 * the reported entry point is translated. */
_start_phys = _start - VA_OFFSET;
ENTRY(_start_phys)

KERNEL_PHYS = 0x40080000;
KERNEL_VIRT = 0xffffffff80080000;

/* VA-PA offset: used to compute LMAs from VMAs */
VA_OFFSET = KERNEL_VIRT - KERNEL_PHYS;

/* Image budget: 32MB (the boot.S early window maps 34MB from phys
 * 0x40000000; heap/reservation constants in memory_layout.rs and
 * main.rs follow the window end at 0x42800000). Grow these together —
 * the riscv64 LAYOUT-4 lesson (boot stack on an unmapped page = silent
 * boot death) applies verbatim. */
KERNEL_WINDOW = 0x2000000;

SECTIONS
{
	. = KERNEL_VIRT;

	_text = .;
	_stext = .;
	__stext = .;

	.text : AT(KERNEL_PHYS) ALIGN(4096)
	{
		KEEP(*(.text.entry))
		*(.text .text.*)
	}

	_etext = .;
	__etext = .;

	. = ALIGN(4096);
	.rodata : AT(ADDR(.rodata) - VA_OFFSET) { *(.rodata .rodata.*) }

	. = ALIGN(8);
	__ex_table_start = .;
	/* KEEP: nothing relocates against .ex_table — the fixup search reads
	 * it through the __ex_table_start/end bounds — so without KEEP a
	 * --gc-sections link (rustc's default for embedded targets) collects
	 * the table and every uaccess fault becomes an oops (observed: the
	 * linked section came out empty). The x86_64 twin guards its table
	 * the same way. */
	.ex_table : AT(ADDR(.ex_table) - VA_OFFSET) { KEEP(*(.ex_table)) }
	__ex_table_end = .;

	. = ALIGN(4096);
	_sdata = .;
	.data : AT(ADDR(.data) - VA_OFFSET)
	{
		*(.data .data.*)
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

	/* Fail the BUILD if the image outgrows the boot.S early window
	 * (see the riscv64 twin's LAYOUT-4 history). */
	ASSERT(_end <= KERNEL_VIRT + KERNEL_WINDOW,
	       "aarch64 kernel image outgrew the 32MB boot window: grow the window (boot.S kern_l2 fill count), HEAP_START and KERNEL_RESERVE_SIZE together")

	/DISCARD/ :
	{
		*(.comment .note*)
		*(.eh_frame .eh_frame_hdr)
	}
}
