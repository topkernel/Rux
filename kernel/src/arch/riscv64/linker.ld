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
	/* x86_64-style alias consumed by dfx::hung_task's kernel-text window */
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
	.ex_table : AT(ADDR(.ex_table) - VA_OFFSET) { *(.ex_table) }
	__ex_table_end = .;

	. = ALIGN(4096);
	_sdata = .;
	.data : AT(ADDR(.data) - VA_OFFSET)
	{
		*(.data .data.*)
		*(.sdata .sdata.*)
	}
	PROVIDE(__global_pointer$ = . + 0x800);
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

	/* RUX-RISCV-LAYOUT-4: boot.S's early page tables (trampoline_pmd and
	 * early_pmd) map a 10-PMD (20MB) image window: KERNEL_VIRT ..
	 * KERNEL_VIRT + 0x1400000. (LAYOUT-3 grew it 14MB->18MB for kallsyms;
	 * the LTP lane's syscall-contract batch then outgrew 18MB.) (RUX-RISCV-LAYOUT-2 grew it to 14MB;
	 * an intermediate branch bump to 16MB for the static buddy
	 * BlockMeta/free-bitmap arrays was subsumed by the 14MB->18MB move.)
	 * If the image (text+rodata+data+bss incl.
	 * the 256K boot stack) outgrows it, the boot stack's top pages sit on
	 * the first unmapped page and the first stack push page-faults into
	 * an infinite trap_entry frame-push recursion - an unbootable kernel
	 * with a truncated KERNPANIC (observed: unit-test build at 12.67MB
	 * against the old 12MB window). Fail the BUILD instead: grow the
	 * window in boot.S (BOTH tables) and move heap/reservation/KERNEL_SIZE
	 * together (main.rs, memory_layout.rs, buddy_allocator.rs).
	 * RUX-RISCV-LAYOUT-3: 14MB->18MB so the full kernel symbol table
	 * (~3.1MB .rodata: 2.4MB names + 0.7MB index) fits in the image
	 * alongside the ~12.5MB image it annotates. */
	ASSERT(_end <= 0xffffffff81400000, "kernel image outgrew the boot.S 20MB early window: extend the window (both PMD tables) and move heap+reservation together")

	/DISCARD/ :
	{
		*(.comment .note*)
		*(.eh_frame .eh_frame_hdr)
	}
}
