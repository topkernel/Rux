/* MIT License
 *
 * Copyright (c) 2026 Fei Wang
 *
 * x86_64 SMP trampoline — copied by the BSP to physical 0x8000 at
 * bring-up; APs enter it in real mode via SIPI (CS:IP = 0x0800:0x0000).
 *
 * ## Position independence discipline
 *
 * Symbols in this section carry their kernel link VMA, which is
 * meaningless at the 0x8000 copy site — so the code NEVER references a
 * symbol address, and padding uses `.space <org> - (. - start)` (never
 * `.org`, which is section-VMA-relative and this section links high).
 *
 * The 16/32-bit stages run with segment bases 0x8000 (real-mode
 * segments 0x0800; the 32-bit GDT descriptors have base 0x8000), so
 * their memory displacements are ORG OFFSETS (offset from the
 * trampoline base).  The 64-bit stage keeps the trampoline linear base
 * (0x8000) in %r15 and addresses the data block as 0x8000-relative —
 * mandatory because a bare displacement in 64-bit mode encodes
 * RIP-relative, and the RIP here is the low copy site.
 *
 * Every absolute value the code cannot compute (PML4, stack, kernel
 * entry, GS base, far-jump targets, gdtr base) is written into the data
 * block of the COPY by the BSP before the SIPI (see smp.rs TRAMP_*).
 *
 * ## Layout (ORG OFFSETS from the trampoline base)
 *
 *   0x0000  16-bit SIPI entry
 *   0x0040  32-bit protected-mode stage
 *   0x0080  64-bit stage (stack + GS base, jump to ap_entry64)
 *   0x0E00  GDT (null, code32 base 0x8000, data32 base 0x8000,
 *           code64, data64)
 *   0x0E40  gdtr pseudo-descriptor (base filled by BSP)
 *   0x0E50  BSP-written data block (see smp.rs)
 *   0x0E80  16:16 far pointer -> pm32
 *   0x0E88  32:16 far pointer -> long64 (linear offset)
 *   0x0E90  null idtr
 *   0x0F00  end (trampoline page tables follow at 0x9000 physical)
 */

	.section .trampoline, "ax"
	.code16

	.global __ap_trampoline_start
__ap_trampoline_start:
	/* ---- real mode: CS=0x0800 IP=0 ---- */
	cli
	cld
	movw	%cs, %ax
	movw	%ax, %ds
	movw	%ax, %es
	movw	%ax, %ss
	xorw	%sp, %sp		/* SS:SP = 0x8000:0, grows down below */

	/* Null IDT: any exception in here is fatal by definition. */
	lidt	(0x0E90)

	/* Trampoline GDT (gdtr base written by the BSP). */
	lgdt	(0x0E40)

	/* Protected mode. */
	movl	%cr0, %eax
	orl	$1, %eax
	movl	%eax, %cr0

	/* Far jump through the BSP-written 16:16 pointer at org 0x0E80
	 * (offset = pm32 org-offset, selector 0x08). */
	ljmp	*(0x0E80)

	/* ---- 32-bit protected mode (CS/DS bases = 0x8000: org offsets) ---- */
	.code32
	.space 0x0040 - (. - __ap_trampoline_start)
pm32:
	movw	$0x10, %ax
	movw	%ax, %ds
	movw	%ax, %es
	movw	%ax, %ss

	/* EFER.LME */
	movl	$0xC0000080, %ecx
	rdmsr
	orl	$0x100, %eax
	wrmsr

	/* CR4.PAE */
	movl	%cr4, %eax
	orl	$0x20, %eax
	movl	%eax, %cr4

	/* Trampoline page tables (identity low 1GB + kernel upper half). */
	movl	0x0E50, %eax
	movl	%eax, %cr3

	/* CR0.PG */
	movl	%cr0, %eax
	orl	$0x80000000, %eax
	movl	%eax, %cr0

	/* Far jump to 64-bit through the BSP-written 32:16 pointer at org
	 * 0x0E88 (offset = long64 LINEAR address, selector 0x18). */
	ljmp	*(0x0E88)

	/* ---- 64-bit long mode.  %r15 = trampoline linear base (0x8000):
	 * absolute addressing for the data block (a bare displacement in
	 * 64-bit mode would encode RIP-relative). ---- */
	.code64
	.space 0x0080 - (. - __ap_trampoline_start)
long64:
	movabsq	$0x8000, %r15

	/* AP boot stack (higher-half VA, mapped by the kernel upper half). */
	movq	0x0E58(%r15), %rsp

	/* GS base = &PER_CPU[cpu] (BSP-written value); user shadow = 0. */
	movq	0x0E60(%r15), %rax
	movl	$0xC0000101, %ecx	/* IA32_GS_BASE */
	movq	%rax, %rdx
	shrq	$32, %rdx		/* edx = base[63:32]; eax = base[31:0] */
	wrmsr
	xorl	%eax, %eax
	xorl	%edx, %edx
	movl	$0xC0000102, %ecx	/* IA32_KERNEL_GS_BASE */
	wrmsr

	/* Jump into the kernel's higher half (ap_entry64 in smp.rs). */
	jmp	*0x0E68(%r15)
1:	hlt
	jmp	1b

	/* ---- GDT at org 0x0E00 (linear 0x8E00) ---- */
	.space 0x0E00 - (. - __ap_trampoline_start)
	.align 8
tramp_gdt:
	.quad	0				/* 0x00 null */
	.quad	0x00CF9A008000FFFF		/* 0x08 code32, base 0x8000 */
	.quad	0x00CF92008000FFFF		/* 0x10 data32, base 0x8000 */
	.quad	0x00AF9A000000FFFF		/* 0x18 code64 */
	.quad	0x00CF92000000FFFF		/* 0x20 data64 */
tramp_gdt_end:

	/* gdtr at org 0x0E40: limit fixed by the static GDT above; the base
	 * (linear 0x8E00) is written by the BSP — a symbol address cannot be
	 * used here (symbols carry the kernel link VMA). */
	.space 0x0E40 - (. - __ap_trampoline_start)
	.word tramp_gdt_end - tramp_gdt - 1
	.long 0

	/* ---- BSP-written data block ---- */
	.space 0x0E50 - (. - __ap_trampoline_start)
	/* u32 trampoline PML4 physical address */
	.long 0
	/* u32 cpu number (diagnostics; GS carries the runtime truth) */
	.long 0
	/* u64 AP boot stack top (kernel VA) */
	.quad 0
	/* u64 GS base (per-CPU area kernel VA) */
	.quad 0
	/* u64 ap_entry64 kernel VA */
	.quad 0
	/* u64 kernel CR3 (diagnostics) */
	.quad 0

	.space 0x0E80 - (. - __ap_trampoline_start)
	/* 16:16 far pointer -> pm32 (offset, selector) */
	.word 0, 0x08
	.space 0x0E88 - (. - __ap_trampoline_start)
	/* 32:16 far pointer -> long64 (linear offset, selector 0x18) */
	.long 0
	.word 0x18
	.word 0

	.space 0x0E90 - (. - __ap_trampoline_start)
	/* null idtr: limit 0, base 0 */
	.word 0
	.long 0

	.space 0x0F00 - (. - __ap_trampoline_start)
	.global __ap_trampoline_end
__ap_trampoline_end:

	.code64
