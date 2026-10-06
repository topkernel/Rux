/* MIT License
 *
 * Copyright (c) 2026 Fei Wang
 *
 * x86_64 boot stub — multiboot1 protocol (QEMU `-kernel <ELF>`).
 *
 * Entered by QEMU in 32-bit protected mode, paging off:
 *   eax = 0x2BADB002 (multiboot magic), ebx = multiboot info ptr.
 *
 * Job: save the multiboot pointer, build early page tables
 * (identity low map + higher-half kernel map), enable long mode,
 * and jump to the first instruction of the kernel image at
 * VMA 0xffffffff80200000 (`__x86_64_start`, .text.boot64 in boot.rs).
 *
 * All early tables use 2MB pages: the TCG default CPU (qemu64) has
 * no 1GB-page support, and 2MB granularity is plenty for bring-up.
 * Only low dwords of entries are written — RAM is below 4GB and the
 * table area is zeroed first, so high dwords stay zero.
 */

.set MB_MAGIC, 0x1BADB002
/* flags: want mem_* (0), boot_device (1), cmdline (2), mmap (6) */
.set MB_FLAGS, 0x47

.set PDE_P, 1 << 0
.set PDE_RW, 1 << 1
.set PDE_PS, 1 << 7          /* 2MB page (in PD) */

.set KERNEL_IMAGE_VMA, 0xffffffff80200000  /* .text.boot64 first byte */

.section .multiboot_header, "a"
.align 4
.long MB_MAGIC
.long MB_FLAGS
.long -(MB_MAGIC + MB_FLAGS)

.section .boot.text, "ax"
.code32
.global _start
.type _start, @function
_start:
    cli
    cld
    /* Save multiboot handoff for Rust (boot.rs reads these) */
    movl %eax, (boot_mb_magic)
    movl %ebx, (boot_mb_info)

    /* Switch to the boot stack in this low region */
    movl $__boot_stack_top, %esp

    /* ---- Zero the early page-table area ---- */
    movl $__boot_page_tables_start, %edi
    movl $__boot_page_tables_end, %ecx
    subl %edi, %ecx
    shrl $2, %ecx
    xorl %eax, %eax
    rep stosl

    /* ---- Wire the table tree ---- */
    /* PML4[0]   -> boot_pdpt_lo   (identity map of low memory) */
    movl $boot_pml4, %eax
    movl $(boot_pdpt_lo + PDE_P | PDE_RW), (%eax)
    /* PML4[511] -> boot_pdpt_hi   (higher-half kernel map) */
    movl $(boot_pdpt_hi + PDE_P | PDE_RW), 2044(%eax)   /* 511*4 */

    /* PDPT_lo[0] -> boot_pd0 : phys 0   .. 1GB */
    movl $(boot_pd0 + PDE_P | PDE_RW), (boot_pdpt_lo)
    /* PDPT_lo[1] -> boot_pd1 : phys 1GB .. 2GB */
    movl $(boot_pd1 + PDE_P | PDE_RW), (boot_pdpt_lo + 4)

    /* PDPT_hi[510] -> boot_pd_kernel : VMA ffffffff80000000..+128MB
     * (entry VMA - 0xffffffff80000000 == entry phys) */
    movl $(boot_pd_kernel + PDE_P | PDE_RW), (boot_pdpt_hi + 2040)  /* 510*4 */

    /* Fill the three page directories (512 * 2MB each) */
    movl $boot_pd0, %edi
    movl $0x0, %esi
    call fill_pd
    movl $boot_pd1, %edi
    movl $0x40000000, %esi
    call fill_pd
    movl $boot_pd_kernel, %edi
    movl $0x0, %esi
    call fill_pd

    /* ---- Enable long mode ---- */
    /* CR4.PAE */
    movl %cr4, %eax
    orl $0x20, %eax
    movl %eax, %cr4

    /* CR3 = boot_pml4 */
    movl $boot_pml4, %eax
    movl %eax, %cr3

    /* EFER.LME (MSR 0xC0000080) */
    movl $0xC0000080, %ecx
    rdmsr
    orl $0x100, %eax
    wrmsr

    /* Load bootstrap GDT (code64 @0x08, data @0x10 — same selector
     * numbers as the runtime GDT loaded later by trap::init) */
    lgdt (boot_gdtdesc)

    /* CR0.PG (PE is already set by the loader) */
    movl %cr0, %eax
    orl $0x80000000, %eax
    movl %eax, %cr0

    /* Far jump into 64-bit code (still low/identity) */
    ljmp $0x08, $mode64_low

/* fill_pd: edi = table, esi = phys base. Clobbers eax, ecx, edi, esi. */
fill_pd:
    xorl %ecx, %ecx
.fill:
    movl %esi, %eax
    orl $(PDE_P | PDE_RW | PDE_PS), %eax
    movl %eax, (%edi,%ecx,4)
    addl $0x200000, %esi
    incl %ecx
    cmpl $512, %ecx
    jb .fill
    ret

.code64
mode64_low:
    /* Reload data selectors */
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    xorw %ax, %ax
    movw %ax, %fs
    movw %ax, %gs

    /* Jump to the first byte of the kernel image (.text.boot64) */
    movabsq $KERNEL_IMAGE_VMA, %rax
    jmp *%rax

.size _start, . - _start

/* ---- Early page tables (page-aligned region in .boot.pt) ---- */
.section .boot.pt, "aw"
.align 4096
boot_pml4:       .space 4096
boot_pdpt_lo:    .space 4096
boot_pdpt_hi:    .space 4096
boot_pd0:        .space 4096
boot_pd1:        .space 4096
boot_pd_kernel:  .space 4096

/* ---- Bootstrap GDT ---- */
.section .boot.data, "aw"
.align 8
boot_gdt:
    .quad 0                                  /* 0x00: null */
    .quad 0x00AF9A000000FFFF                 /* 0x08: 64-bit code (L=1) */
    .quad 0x00CF92000000FFFF                 /* 0x10: data */
boot_gdt_end:
boot_gdtdesc:
    .word boot_gdt_end - boot_gdt - 1
    .long boot_gdt

/* ---- Multiboot handoff globals (read by arch/x86_64/boot.rs) ---- */
.align 4
.global boot_mb_magic
boot_mb_magic:  .long 0
.global boot_mb_info
boot_mb_info:   .long 0
