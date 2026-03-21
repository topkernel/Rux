//! MIT License
//!
//! Copyright (c) 2026 Fei Wang

//! RISC-V 64-bit kernel boot code - Linux style
//!
//! Boot flow:
//! 1. Kernel loaded at 0x80200000 (physical)
//! 2. Setup early page tables with identity + virtual mapping
//! 3. Enable MMU and relocate to virtual address
//! 4. Jump to rust_main (at virtual address)

.section .text.entry
.global _start
.global relocate_enable_mmu
.global dtb_pointer

// Constants
.set PAGE_SIZE, 4096
.set PAGE_SHIFT, 12
.set SATP_MODE_SV39, 8            // Mode = 8 for Sv39 (bits [63:60])
.set PAGE_OFFSET, 0xffffffd600000000  // Linux PAGE_OFFSET_L3

// Page table entry flags
.set PTE_V, 1 << 0     // Valid
.set PTE_R, 1 << 1     // Read
.set PTE_W, 1 << 2     // Write
.set PTE_X, 1 << 3     // Execute
.set PTE_A, 1 << 6     // Accessed
.set PTE_D, 1 << 7     // Dirty

// Kernel flags: VRWXAD
.set PTE_KERNEL, PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D

_start:
    // a0 = hart ID, a1 = DTB pointer
    mv tp, a0                    // Save hart ID
    mv s0, a1                    // Save DTB pointer

    // Disable interrupts
    csrw sie, zero
    csrw sip, zero

    // Setup early stack (physical address)
    la sp, _stack_bottom
    li t0, 65536
    mul t1, tp, t0               // t1 = hart_id * 64KB
    add sp, sp, t1
    add sp, sp, t0               // sp = stack_bottom + hart_id * 64KB + 64KB

    // Clear BSS (only first hart)
    la t0, bss_done
    li t1, 1
    amoadd.w t2, t1, (t0)
    bnez t2, 2f

    la t0, __bss_start
    la t1, __bss_end
1:
    bgeu t0, t1, 2f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
2:
    fence rw, rw

    // Save DTB pointer
    beqz s0, 3f
    la t0, dtb_pointer
    sd s0, 0(t0)
3:

    // Setup page tables (only first hart)
    la t0, pt_done
    li t1, 1
    amoadd.w t2, t1, (t0)
    bnez t2, 4f

    // ========================================
    // Setup early page tables (Linux-style)
    // ========================================
    // Sv39 with 4KB pages:
    // - Level 2 (PGD): VPN2 = va[38:30], each entry covers 512GB
    // - Level 1 (PMD): VPN1 = va[29:21], each entry covers 2MB (as leaf)
    // - Level 0 (PTE): VPN0 = va[20:12], each entry covers 4KB
    //
    // We use 2MB huge pages at level 1 for kernel mapping
    // Kernel: phys=0x80200000, virt=0xffffffd602000000
    //
    // For PAGE_OFFSET = 0xffffffd600000000:
    //   VPN2 = (0xffffffd600000000 >> 30) & 0x1FF = 344 (0x158)

    // Get physical addresses of page tables
    la s1, trampoline_pg_dir     // s1 = PA of trampoline_pg_dir
    la s4, trampoline_pmd        // s4 = PA of trampoline_pmd
    la s5, early_pg_dir          // s5 = PA of early_pg_dir
    la s2, early_pmd             // s2 = PA of early_pmd
    la s3, early_pmd_dev         // s3 = PA of early_pmd_dev

    // ========================================
    // Zero all page tables
    // ========================================
    // Zero trampoline_pg_dir
    mv t0, s1
    li t1, 512
1:
    sd zero, 0(t0)
    addi t0, t0, 8
    addi t1, t1, -1
    bnez t1, 1b

    // Zero trampoline_pmd
    mv t0, s4
    li t1, 512
1:
    sd zero, 0(t0)
    addi t0, t0, 8
    addi t1, t1, -1
    bnez t1, 1b

    // Zero early_pg_dir
    mv t0, s5
    li t1, 512
1:
    sd zero, 0(t0)
    addi t0, t0, 8
    addi t1, t1, -1
    bnez t1, 1b

    // Zero early_pmd
    mv t0, s2
    li t1, 512
1:
    sd zero, 0(t0)
    addi t0, t0, 8
    addi t1, t1, -1
    bnez t1, 1b

    // Zero early_pmd_dev
    mv t0, s3
    li t1, 512
1:
    sd zero, 0(t0)
    addi t0, t0, 8
    addi t1, t1, -1
    bnez t1, 1b

    // ========================================
    // Setup trampoline page table (Linux-style)
    // Maps kernel_virt -> kernel_phys AND identity mapping
    // ========================================

    // Trampoline PGD[2] -> trampoline_pmd (for identity mapping 0x80000000)
    srli t0, s4, 12              // t0 = PPN of trampoline_pmd
    slli t0, t0, 10
    li t1, PTE_V
    or t0, t0, t1
    li t1, 2 * 8
    add t2, s1, t1
    sd t0, 0(t2)

    // Trampoline PGD[344] -> trampoline_pmd (for virtual mapping PAGE_OFFSET)
    li t1, 344 * 8
    add t2, s1, t1
    sd t0, 0(t2)

    // Trampoline PMD[1] -> 2MB page at kernel physical address 0x80200000
    li t0, 0x80200000
    srli t1, t0, 12
    slli t1, t1, 10
    li t2, PTE_KERNEL
    or t1, t1, t2
    li t2, 1 * 8
    add t2, s4, t2
    sd t1, 0(t2)

    // ========================================
    // Setup early page table (full mapping)
    // ========================================

    // Early PGD[2] -> early_pmd (for identity mapping region)
    srli t0, s2, 12
    slli t0, t0, 10
    li t1, PTE_V
    or t0, t0, t1
    li t1, 2 * 8
    add t2, s5, t1
    sd t0, 0(t2)

    // Early PGD[344] -> early_pmd (for virtual mapping PAGE_OFFSET region)
    li t1, 344 * 8
    add t2, s5, t1
    sd t0, 0(t2)

    // Early PGD[0] -> early_pmd_dev (for device region 0x00000000-0x3FFFFFFF)
    srli t0, s3, 12
    slli t0, t0, 10
    li t1, PTE_V
    or t0, t0, t1
    sd t0, 0(s5)

    // Early PGD[1] -> early_pmd_dev (for device region 0x40000000-0x7FFFFFFF)
    // This allows PCI MMIO at 0x40000000 to be accessed
    li t1, 1 * 8
    add t2, s5, t1
    sd t0, 0(t2)                 // PGD[1] also points to early_pmd_dev

    // ========================================
    // Setup early_pmd entries for 2MB huge pages
    // ========================================

    // Identity mapping: 0x80000000-0x88000000 (128MB)
    // This covers kernel + heap + slab + vmemmap + page tables
    li t0, 0x80000000
    li t3, 0x88000000
    li t4, 0
1:
    srli t5, t0, 12
    slli t5, t5, 10
    li t6, PTE_KERNEL
    or t5, t5, t6
    slli t6, t4, 3
    add t6, s2, t6
    sd t5, 0(t6)
    li t6, 0x200000
    add t0, t0, t6
    addi t4, t4, 1
    blt t0, t3, 1b

    // Virtual mapping: PAGE_OFFSET region (64MB)
    li t0, 0x80000000
    li t4, 0
1:
    srli t5, t0, 12
    slli t5, t5, 10
    li t6, PTE_KERNEL
    or t5, t5, t6
    slli t6, t4, 3
    add t6, s2, t6
    sd t5, 0(t6)
    li t6, 0x200000
    add t0, t0, t6
    addi t4, t4, 1
    li t6, 32
    blt t4, t6, 1b

    // ========================================
    // Setup early_pmd_dev for devices
    // ========================================

    // Map UART at 0x10000000 (identity mapped)
    li t0, 0x10000000
    srli t1, t0, 12
    slli t1, t1, 10
    li t2, PTE_V | PTE_R | PTE_W | PTE_A | PTE_D  // Device: no X
    or t1, t1, t2
    li t2, 128 * 8
    add t2, s3, t2
    sd t1, 0(t2)                 // early_pmd_dev[128] = UART

    // Also map UART in early_pmd for virtual address access
    li t2, 128 * 8
    add t2, s2, t2
    sd t1, 0(t2)                 // early_pmd[128] = UART

    // Map PLIC at 0x0C000000 (identity mapped)
    // VPN1 = (0x0C000000 >> 21) & 0x1FF = 96
    li t0, 0x0C000000
    srli t1, t0, 12
    slli t1, t1, 10
    li t2, PTE_V | PTE_R | PTE_W | PTE_A | PTE_D  // Device: no X
    or t1, t1, t2
    li t2, 96 * 8
    add t2, s3, t2
    sd t1, 0(t2)                 // early_pmd_dev[96] = PLIC

    // Map VIRTIO devices at 0x30000000 (identity mapped)
    // VPN1 = (0x30000000 >> 21) & 0x1FF = 384
    // This covers virtio-blk, virtio-net, etc.
    li t0, 0x30000000
    srli t1, t0, 12
    slli t1, t1, 10
    li t2, PTE_V | PTE_R | PTE_W | PTE_A | PTE_D  // Device: no X
    or t1, t1, t2
    li t2, 384 * 8
    add t2, s3, t2
    sd t1, 0(t2)                 // early_pmd_dev[384] = VIRTIO

    // Map CLINT at 0x02000000 (identity mapped)
    // VPN1 = (0x02000000 >> 21) & 0x1FF = 16
    li t0, 0x02000000
    srli t1, t0, 12
    slli t1, t1, 10
    li t2, PTE_V | PTE_R | PTE_W | PTE_A | PTE_D  // Device: no X
    or t1, t1, t2
    li t2, 16 * 8
    add t2, s3, t2
    sd t1, 0(t2)                 // early_pmd_dev[16] = CLINT

    // Map PCI MMIO at 0x40000000 (identity mapped)
    // VPN2 = (0x40000000 >> 30) & 0x1FF = 1, VPN1 = 0
    // Need to map this in PGD[1] -> early_pmd_dev
    li t0, 0x40000000
    srli t1, t0, 12
    slli t1, t1, 10
    li t2, PTE_V | PTE_R | PTE_W | PTE_A | PTE_D  // Device: no X
    or t1, t1, t2
    li t2, 0                     // VPN1 = 0 for 0x40000000
    add t2, s3, t2
    sd t1, 0(t2)                 // early_pmd_dev[0] = PCI MMIO start

    // ========================================
    // Map DTB at 0xbfe00000 (identity mapped)
    // VPN2 = 2, VPN1 = 511
    // Uses early_pmd which is already linked to PGD[2]
    // ========================================
    li t0, 0xbfe00000            // DTB address
    srli t1, t0, 12              // PPN
    slli t1, t1, 10
    li t2, PTE_V | PTE_R | PTE_W | PTE_A | PTE_D  // Device: no X
    or t1, t1, t2
    li t2, 511 * 8               // PMD[511] for DTB
    add t2, s2, t2
    sd t1, 0(t2)                 // early_pmd[511] = DTB

    fence rw, rw

4:
    // Wait for page table setup
    fence rw, rw

    // ========================================
    // Enable MMU and relocate
    // ========================================
    la a0, early_pg_dir          // a0 = PA of early_pg_dir
    call relocate_enable_mmu

    // Now running at virtual address
    // Reload stack (virtual address)
    la sp, _stack_bottom
    li t0, 65536
    mul t1, tp, t0
    add sp, sp, t1
    add sp, sp, t0

    // ========================================
    // Initialize KERNEL_MAP (before rust_main)
    // ========================================
    la t0, KERNEL_MAP

    // va_pa_offset = PAGE_OFFSET - PHYS_MEMORY_BASE
    li t1, PAGE_OFFSET
    li t2, 0x80000000
    sub t1, t1, t2
    sd t1, 32(t0)               // KERNEL_MAP.va_pa_offset

    // page_offset = PAGE_OFFSET
    li t1, PAGE_OFFSET
    sd t1, 48(t0)               // KERNEL_MAP.page_offset

    // Jump to rust_main
    call rust_main

_hang:
    wfi
    j _hang

// ========================================
// relocate_enable_mmu
// a0: physical address of early_pg_dir
// ========================================
relocate_enable_mmu:
    // Calculate VA-PA offset
    li t0, PAGE_OFFSET
    li t6, 0x80000000
    sub t0, t0, t6               // t0 = va_pa_offset

    // Relocate return address
    add ra, ra, t0

    // Set stvec to trap handler (physical address)
1:
    auipc t1, %pcrel_hi(trap_handler_mmu)
    addi t1, t1, %pcrel_lo(1b)
    csrw stvec, t1

    // Compute satp = (mode << 60) | PPN
    li t2, SATP_MODE_SV39
    slli t2, t2, 60
    srli t3, a0, 12
    or t3, t3, t2

    // Enable MMU
    sfence.vma
    csrw satp, t3

    // Fence to ensure all page table writes are visible
    fence rw, rw

    // Jump to virtual address
2:
    auipc t1, %pcrel_hi(1f)
    addi t1, t1, %pcrel_lo(2b)
    add t1, t1, t0               // t1 = VA of label 1f
    jr t1

1:
    // Now running at virtual address
    ret

.align 2
.global trap_handler_mmu
trap_handler_mmu:
    // Trap handler during MMU transition
    // Use identity-mapped UART
    li t0, 0x10000000
    li t1, 'T'
    sb t1, 0(t0)
    j _hang

// ========================================
// Data section
// ========================================
.section .data
.align 3
bss_done:
    .word 0
pt_done:
    .word 0
.align 3
.global dtb_pointer
dtb_pointer:
    .dword 0

// ========================================
// BSS section - page tables (Linux-style)
// ========================================
.section .bss

// Trampoline page directory
.align 12
.global trampoline_pg_dir
trampoline_pg_dir:
    .space 4096

// Trampoline PMD
.align 12
.global trampoline_pmd
trampoline_pmd:
    .space 4096

// Early page directory
.align 12
.global early_pg_dir
early_pg_dir:
    .space 4096

// Early PMD for kernel region
.align 12
.global early_pmd
early_pmd:
    .space 4096

// Early PMD for device region
.align 12
.global early_pmd_dev
early_pmd_dev:
    .space 4096
