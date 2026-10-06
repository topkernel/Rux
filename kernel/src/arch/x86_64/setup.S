/* MIT License
 *
 * Copyright (c) 2026 Fei Wang
 *
 * bzImage setup part — Linux boot protocol header + 16-bit trampoline.
 *
 * QEMU's linuxboot_dma ROM loads the setup sectors at 0x10000
 * (FW_CFG_SETUP_ADDR) and transfers control to CS=(0x10000>>4)+0x20,
 * IP=0 — i.e. offset 0x200 here. All label addresses in this file are
 * section-relative (linked at 0); at runtime the setup segment is
 * loaded into DS so absolute memory operands become correct, and the
 * protected-mode far jumps use hardcoded absolute addresses.
 *
 * Flow: 16-bit trampoline -> protected mode -> 0x100000 (the 32-bit
 * boot stub in boot.S) -> long mode -> kernel.
 */

	.code16
	.section .setup, "ax"
	.org 0
	.space 0x1f1
	/* ---- Linux setup header ---- */
	.byte	3			/* 0x1f1 setup_sects (4 sectors, patched) */
	.word	0			/* 0x1f2 ram_flags */
	.long	0			/* 0x1f4 syssize (patched) */
	.word	0			/* 0x1f8 swap_dev */
	.word	0xaa55			/* 0x1fa boot_flag */
	.org 0x200
	.byte	0xeb, 0x3e		/* 0x200 jmp +0x3e -> 0x240 (rel8 is signed!) */
	.ascii	"HdrS"			/* 0x202 */
	.short	0x020b			/* 0x206 version */
	.long	0			/* 0x208 realmode_swtch */
	.short	0x1000, 0		/* 0x20c start_sys_seg, kernel_version */
	.byte	0			/* 0x210 type_of_loader */
	.byte	0x01			/* 0x211 loadflags: LOADED_HIGH */
	.short	0			/* 0x212 setup_move_size */
	.long	0x100000		/* 0x214 code32_start */
	.long	0			/* 0x218 ramdisk_image */
	.long	0			/* 0x21c ramdisk_size */
	.short	0			/* 0x220 xloadflags */
	.short	0			/* 0x222 heap_end_ptr */
	.long	0			/* 0x224 ext_ramdisk_image */
	.long	0			/* 0x228 ext_ramdisk_size */
	.long	0x7fffffff		/* 0x22c ramdisk_max */

	.org 0x240
setup_entry16:
	cli
	cld
	/* DS = setup segment (CS at entry is setup_seg + 0x20) */
	mov	%cs, %ax
	sub	$0x20, %ax
	mov	%ax, %ds
	mov	%ax, %ss
	mov	$0x4000, %sp

	/* ---- BIOS int 15h AX=E820 memory map (real mode only) ----
	 * Entries (20B: addr u64, size u64, type u32) land at setup+0x660,
	 * count (u16) at setup+0x658. Cap 32 entries. DS/ES = setup seg. */
	sti
	cld
	xor	%ebx, %ebx		# continuation id
	mov	$0x660, %di
	mov	$0, %bp		# entry count
e820_loop:
	mov	$0xe820, %eax
	mov	$0x534d4150, %edx	# 'SMAP'
	mov	$20, %ecx
	int	$0x15
	jc	e820_done
	cmp	$0x534d4150, %eax
	jne	e820_done
	inc	%bp
	add	$20, %di
	test	%ebx, %ebx
	jz	e820_done
	cmp	$32, %bp
	jb	e820_loop
e820_done:
	mov	%bp, %es:0x658		# store count
	cli
	cld

	/* A20 via port 0x92 (QEMU has it on already) */
	inb	$0x92, %al
	orb	$0x02, %al
	andb	$0xfe, %al
	outb	%al, $0x92

	/* Load GDT. The memory OPERAND is DS-relative, but the GDTR.base
	 * it loads is a LINEAR address — setup always sits at 0x10000, so
	 * the descriptor stores 0x10000 + section offset. */
	data32 lgdt (boot_gdt_desc)

	/* CR0 moves need explicit 32-bit operands in 16-bit mode */
	.byte	0x66, 0x0f, 0x20, 0xc0	/* mov %cr0, %eax */
	orb	$0x01, %al		/* PE */
	.byte	0x66, 0x0f, 0x22, 0xc0	/* mov %eax, %cr0 */
	/* Far jump to 32-bit code at absolute 0x10400: 66 EA off32 sel16 */
	.byte	0x66, 0xea
	.long	0x10400
	.word	0x08

	.org 0x400
	.code32
pm_entry32:
	movw	$0x10, %ax
	movw	%ax, %ds
	movw	%ax, %es
	movw	%ax, %ss
	movw	%ax, %fs
	movw	%ax, %gs
	movl	$0x70000, %esp
	ljmp	$0x08, $0x100000	/* the 32-bit boot stub in boot.S */

	.align 8
boot_gdt:
	.quad	0			/* 0x00 null */
	.quad	0x00cf9a000000ffff	/* 0x08 32-bit code */
	.quad	0x00cf92000000ffff	/* 0x10 data */
boot_gdt_end:
boot_gdt_desc:
	.word	boot_gdt_end - boot_gdt - 1
	.long	0x10000 + boot_gdt	/* GDTR.base is a LINEAR address */

	.org 0x800			/* exactly 4 sectors */
