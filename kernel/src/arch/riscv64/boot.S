// MIT License
//
// Copyright (c) 2026 Fei Wang
//
// RISC-V 64-bit kernel boot code
//!
// Boot: Link at virtual address, load at physical address
//! Use trampoline page table to enable MMU and jump to virtual address
//!
//! Boot flow:
//! 1. Kernel loaded at 0x80200000 (physical), linked at 0xffffffff80000000 (virtual)
//! 2. Setup stack, clear BSS (using PC-relative addressing, works at physical address)
//! 3. Create trampoline page table (maps first 2MB of kernel VA -> PA)
//! 4. Create early page table (maps full kernel VA -> PA)
//! 5. Enable MMU, trap to virtual address
//! 6. Switch to full early page table
//! 7. Jump to rust_main at virtual address

.section .text.entry, "ax", @progbits
.global _start

.global KERNEL_MAP

.set KERNEL_PHYS,    0x80200000
.set KERNEL_VIRT,    0xffffffff80000000
.set PAGE_SIZE,      4096
.set PGD_SHIFT,      30
.set PMD_SHIFT,      21
.set SATP_MODE_SV39,  0x8000000000000000

// ============================================================================
// _start - Entry point (runs at physical address, MMU off)
// ============================================================================
// a0 = hart ID, a1 = DTB pointer
_start:
    mv tp, a0                    // Save hart ID to tp
    mv s0, a1                    // Save DTB pointer to s0

    // Disable interrupts
    csrw sie, zero
    csrw sip, zero

    // ========================================================================
    // Setup early stack (physical address - before MMU enable)
    // ========================================================================
    // Compute VA_OFFSET = KERNEL_VIRT - KERNEL_PHYS
    // KERNEL_VIRT = 0xFFFFFFFF80000000, KERNEL_PHYS = 0x80200000
    // Use shift/add trick to avoid sign-extension issues with li
    li t1, KERNEL_VIRT           // t1 = 0xFFFFFFFF80000000 (lui sign-extends correctly)
    li t3, 1025                  // 1025 = 0x401
    slli t3, t3, 21              // t3 = 0x401 << 21 = 0x80200000
    sub t1, t1, t3               // t1 = VA_OFFSET = KERNEL_VIRT - KERNEL_PHYS

    // _stack_bottom VMA - VA_OFFSET = _stack_bottom physical address
    la t0, _stack_bottom
    sub sp, t0, t1               // sp = _stack_bottom (physical address)
    li t0, 0x40000               // 256KB stack
    add sp, sp, t0               // sp = stack top (physical)

    // ========================================================================
    // Clear BSS (only boot hart, using physical addresses)
    // ========================================================================
    la t0, __bss_start
    sub t0, t0, t1               // t0 = __bss_start (physical)
    la t2, __bss_end
    sub t2, t2, t1               // t2 = __bss_end (physical)
4:
    bgeu t0, t2, 5f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 4b
5:

    // ========================================================================
    // Setup early page tables
    // ========================================================================
    // --- Clear page tables ---
    la t2, trampoline_pg_dir
    sub t2, t2, t1               // physical address
    li t3, 0
    sd t3, 0(t2)
    sd t3, 8(t2)
    sd t3, 16(t2)

    la t2, trampoline_pmd
    sub t2, t2, t1               // physical address
    sd t3, 0(t2)
    sd t3, 8(t2)
    sd t3, 16(t2)

    la t2, early_pg_dir
    sub t2, t2, t1               // physical address
    sd t3, 0(t2)
    sd t3, 8(t2)
    sd t3, 16(t2)

    la t2, early_pmd
    sub t2, t2, t1               // physical address
    sd t3, 0(t2)
    sd t3, 8(t2)
    sd t3, 16(t2)

    la t2, early_pmd_io
    sub t2, t2, t1               // physical address
    sd t3, 0(t2)
    sd t3, 8(t2)
    sd t3, 16(t2)

    // --- Create trampoline page table ---
    // trampoline_pg_dir: one PGD entry mapping KERNEL_VIRT -> trampoline_pmd
    // VPN2 for KERNEL_VIRT = (0xffffffff80000000 >> 30) & 0x1FF = 510
    la t2, trampoline_pg_dir
    sub t2, t2, t1               // t2 = trampoline_pg_dir (physical)

    la t3, trampoline_pmd
    sub t3, t3, t1               // t3 = trampoline_pmd (physical)
    srli t3, t3, 12              // t3 = PPN = phys >> 12
    slli t3, t3, 10              // t3 = PPN << 10 (PTE format)
    ori t3, t3, 0x01             // V (valid) only - non-leaf points to PMD

    // PGD index 510 (kernel space), offset = 510 * 8 = 4080
    li t4, 4080
    add t4, t2, t4
    sd t3, 0(t4)

    // trampoline_pmd: map first 4MB of kernel (need 2 PMD entries for stack)
    // PMD[0]: VA -> PA first 2MB
    la t3, trampoline_pmd
    sub t3, t3, t1               // t3 = trampoline_pmd (physical)
    li t4, KERNEL_PHYS
    srli t4, t4, 2               // PPN in PTE format
    li t5, 0xEF                  // PAGE_KERNEL_EXEC: V|R|W|X|G|A|D
    or t4, t4, t5
    sd t4, 0(t3)

    // PMD[1]: VA -> PA second 2MB
    li t4, KERNEL_PHYS
    li t6, 0x200000              // 2MB
    add t4, t4, t6
    srli t4, t4, 2               // PPN in PTE format
    or t4, t4, t5
    sd t4, 8(t3)

    // --- Create early page table (full kernel mapping) ---
    // early_pg_dir: same as trampoline but points to early_pmd
    la t2, early_pg_dir
    sub t2, t2, t1               // physical

    la t3, early_pmd
    sub t3, t3, t1               // physical
    srli t3, t3, 12
    slli t3, t3, 10
    ori t3, t3, 0x01             // V (valid) only - non-leaf points to PMD

    li t4, 4080
    add t4, t2, t4
    sd t3, 0(t4)

    // early_pmd: map first 8MB of kernel (4 PMD entries for text/rodata/data/bss/stack)
    // PMD[0]: VA 0xFFFFFFFF80000000 -> PA 0x80200000 (first 2MB: text)
    la t3, early_pmd
    sub t3, t3, t1               // physical
    li t4, KERNEL_PHYS
    srli t4, t4, 2               // PPN in PTE format
    li t5, 0xEF                  // PAGE_KERNEL_EXEC: V|R|W|X|G|A|D
    or t4, t4, t5
    sd t4, 0(t3)

    // PMD[1]: VA 0xFFFFFFFF80200000 -> PA 0x80400000 (second 2MB)
    li t4, KERNEL_PHYS
    li t6, 0x200000              // 2MB
    add t4, t4, t6
    srli t4, t4, 2
    or t4, t4, t5
    sd t4, 8(t3)

    // PMD[2]: VA 0xFFFFFFFF80400000 -> PA 0x80600000 (third 2MB)
    li t4, KERNEL_PHYS
    li t6, 0x400000              // 4MB
    add t4, t4, t6
    srli t4, t4, 2
    or t4, t4, t5
    sd t4, 16(t3)

    // PMD[3]: VA 0xFFFFFFFF80600000 -> PA 0x80800000 (fourth 2MB)
    li t4, KERNEL_PHYS
    li t6, 0x600000              // 6MB
    add t4, t4, t6
    srli t4, t4, 2
    or t4, t4, t5
    sd t4, 24(t3)

    // --- MMIO identity mapping (UART at 0x10000000) ---
    // early_pg_dir[0] -> early_pmd_io (non-leaf, identity maps first 1GB)
    // VPN2 for 0x10000000 = (0x10000000 >> 30) & 0x1FF = 0
    la t2, early_pg_dir
    sub t2, t2, t1               // physical

    la t3, early_pmd_io
    sub t3, t3, t1               // physical
    srli t3, t3, 12
    slli t3, t3, 10
    ori t3, t3, 0x01             // V (valid) only - non-leaf points to PMD
    sd t3, 0(t2)                 // PGD[0]

    // early_pmd_io[0]: VA 0x00000000 -> PA 0x00000000 (first 2MB, identity)
    la t3, early_pmd_io
    sub t3, t3, t1               // physical
    // PPN for 0x00000000 = 0, flags: V|R|W|A|D|G = 0xC7 (no X for device memory)
    li t4, 0xC7
    sd t4, 0(t3)

    // early_pmd_io[128]: VA 0x10000000 -> PA 0x10000000 (UART MMIO, 2MB)
    // PMD index = (0x10000000 >> 21) & 0x1FF = 128, offset = 128 * 8 = 1024
    li t5, 0x10000000
    srli t5, t5, 2               // PPN in PTE format
    li t6, 0xC7                  // V|R|W|A|D|G (device memory, no X)
    or t5, t5, t6
    li t4, 1024                  // offset = 128 * 8
    add t4, t3, t4
    sd t5, 0(t4)

    // early_pmd_io[256]: identity map 0x80000000-0x80200000 (for Fixmap stage alloc)
    // PMD index = (0x80000000 >> 21) & 0x1FF = 256, offset = 256 * 8 = 2048
    li t5, KERNEL_PHYS
    srli t5, t5, 2               // PPN in PTE format
    li t6, 0xEF                  // V|R|W|X|G|A|D (kernel memory)
    or t5, t5, t6
    li t4, 2048                  // offset = 256 * 8
    add t4, t3, t4
    sd t5, 0(t4)

    // ========================================================================
    // Save DTB pointer
    // ========================================================================
    beqz s0, 6f
    la t0, dtb_pointer
    sub t0, t0, t1               // physical address of dtb_pointer
    sd s0, 0(t0)
6:

    // ========================================================================
    // Initialize KERNEL_MAP structure (physical address)
    // ========================================================================
    la t0, KERNEL_MAP
    sub t0, t0, t1               // physical address of KERNEL_MAP
    li t2, KERNEL_VIRT
    sd t2, 0(t0)                 // virt_addr (offset 0)
    li t2, KERNEL_PHYS
    sd t2, 16(t0)                // phys_addr (offset 16)
    // va_kernel_pa_offset = KERNEL_VIRT - KERNEL_PHYS (offset 40 = field 5)
    li t2, KERNEL_VIRT
    li t3, 1025
    slli t3, t3, 21              // t3 = 0x80200000
    sub t2, t2, t3               // t2 = va_kernel_pa_offset = KERNEL_VIRT - KERNEL_PHYS
    sd t2, 40(t0)                // va_kernel_pa_offset (offset 40)
    // size and offset will be filled by Rust code

    // ========================================================================
    // Enable MMU and jump to virtual address
    // ========================================================================
    // Step 1: Set stvec to VA of "2:" label
    // la already returns the correct VA (via GOT), no offset computation needed
    la t3, 2f
    csrw stvec, t3

    // Step 3: Compute satp for trampoline page table
    la t0, trampoline_pg_dir
    sub t0, t0, t1               // physical address
    srli t0, t0, 12              // PPN
    li t2, SATP_MODE_SV39
    or t0, t0, t2

    // Step 4: Enable MMU!
    sfence.vma
    csrw satp, t0

    // Step 5: The next instruction fetch will fault (VA != PA) and trap to stvec.
    // The fault redirects us to the "2:" label at virtual address.

    .align 2
2:
    // Now running at VIRTUAL address!
    // stvec was set to VA, so the fault redirected us here.

    // Step 5.5: Fix up stack pointer (was physical, now needs virtual)
    // sp was set to physical _stack_top before MMU enable
    // Convert to virtual: sp_va = sp_pa + VA_OFFSET
    la t0, _stack_top
    mv sp, t0                    // sp = virtual address of _stack_top

    // Step 6: Reload GP for virtual address space
    .option push
    .option norelax
    la gp, __global_pointer$
    .option pop

    // Step 7: Switch from trampoline (2MB only) to full early page table
    la t0, early_pg_dir
    // t0 is now a virtual address (PC-relative), convert to physical for satp
    li t2, KERNEL_VIRT
    li t3, 1025
    slli t3, t3, 21              // t3 = 0x80200000
    sub t2, t2, t3               // VA_OFFSET
    sub t0, t0, t2               // early_pg_dir physical address
    srli t0, t0, 12              // PPN
    li t2, SATP_MODE_SV39
    or t0, t0, t2
    csrw satp, t0
    sfence.vma

    // Step 8: Set stvec to VA of trap handler (will be properly set by Rust)
    la t0, trap_entry
    csrw stvec, t0

    // Step 9: Jump to rust_main (virtual address)
    la t0, rust_main
    jr t0

.Lhang:
    wfi
    j .Lhang

// ============================================================================
// secondary_start - Entry point for secondary CPUs via SBI HSM hart_start
// ============================================================================
// a0 = hartid, a1 = opaque (stack pointer, physical address)
// Runs at physical address, MMU off, satp=0, sstatus.SIE=0.
// Uses the boot hart's early_pg_dir page tables (already set up).
.global secondary_start
secondary_start:
    // Save hart_id (a0) before any function calls that clobber a0
    mv t4, a0

    // Disable interrupts
    csrw sie, zero
    csrw sip, zero

    // Set up stack from opaque parameter (physical address)
    mv sp, a1

    // Save hartid in tp (used by cpu_id() before scheduler sets tp)
    mv tp, t4

    // Set stvec to virtual address of the post-MMU label
    la t3, .Lsecondary_mmu_on
    csrw stvec, t3

    // Compute VA_OFFSET
    li t1, KERNEL_VIRT
    li t3, 1025
    slli t3, t3, 21              // t3 = 0x80200000
    sub t1, t1, t3               // t1 = VA_OFFSET

    // Get physical address of early_pg_dir (set up by boot hart)
    la t0, early_pg_dir
    sub t0, t0, t1               // t0 = PA of early_pg_dir
    srli t0, t0, 12              // PPN
    li t2, SATP_MODE_SV39
    or t0, t0, t2

    // Enable MMU — next instruction faults, traps to .Lsecondary_mmu_on at VA
    sfence.vma
    csrw satp, t0

.Lsecondary_mmu_on:
    // Now running at virtual address with early_pg_dir (maps 8MB kernel)

    // Fix up stack pointer: was physical, convert to virtual
    li t0, KERNEL_VIRT
    li t1, 1025
    slli t1, t1, 21
    sub t0, t0, t1               // t0 = VA_OFFSET
    add sp, sp, t0

    // Set up global pointer for virtual address space
    .option push
    .option norelax
    la gp, __global_pointer$
    .option pop

    // Switch to the permanent page table saved by the boot hart.
    // early_pg_dir only maps 8MB; the permanent table has linear mapping,
    // vmemmap, device mappings, etc.
    // Fence to ensure we see the boot hart's store to BOOT_HART_SATP
    fence
    la t0, BOOT_HART_SATP
    ld t0, 0(t0)                 // t0 = boot hart's satp (permanent page table)
    // If BOOT_HART_SATP is 0, spin (boot hart hasn't set it yet)
    beqz t0, .Lsecondary_hang

    sfence.vma
    csrw satp, t0
    sfence.vma

    // Set stvec to real trap handler
    la t0, trap_entry
    csrw stvec, t0

    // Set sscratch = 0 (kernel mode indicator)
    csrw sscratch, zero

    // Linux-style: load tp from pre-created idle task pointer array.
    // sched::init() pre-creates idle tasks for ALL CPUs and stores
    // pointers in __secondary_idle_tasks[].  Loading tp here (before
    // any C code or interrupt enable) makes current() valid from the
    // very first instruction of secondary_cpu_entry.
    la t0, __secondary_idle_tasks
    // tp currently holds hartid — use it as index (hartid * 8)
    slli t1, tp, 3
    add t0, t0, t1
    ld  t1, 0(t0)              // t1 = idle task pointer for this hart
    // If not yet populated (should not happen), fall back to hartid
    bnez t1, .Ltp_loaded
    mv  t1, tp
.Ltp_loaded:
    // Save hartid for secondary_cpu_entry argument
    mv  a0, tp
    // Set tp to idle task pointer
    mv  tp, t1

    // Jump to Rust secondary CPU entry (a0 = hartid, tp = idle task)
    la t0, secondary_cpu_entry
    jr t0

.Lsecondary_hang:
    wfi
    j .Lsecondary_hang

// ============================================================================
// Data section (page tables and global data)
// ============================================================================

.section .data
.align 12

// Trampoline page table: maps only first 2MB of kernel (for MMU transition)
.global trampoline_pg_dir
trampoline_pg_dir:
    .space PAGE_SIZE
.global trampoline_pmd
trampoline_pmd:
    .space PAGE_SIZE

// Early page table: maps full kernel (after trampoline switch)
.global early_pg_dir
early_pg_dir:
    .space PAGE_SIZE
.global early_pmd
early_pmd:
    .space PAGE_SIZE
.global early_pmd_io
early_pmd_io:
    .space PAGE_SIZE

.align 3
.global bss_clear_done
bss_clear_done:
    .dword 0
.global dtb_pointer
dtb_pointer:
    .dword 0

// KERNEL_MAP structure — field order MUST match Rust KernelMapping in
// arch/riscv64/mm/memory_layout.rs: virt_addr, virt_offset, phys_addr, size,
// va_pa_offset, va_kernel_pa_offset, page_offset.
.global KERNEL_MAP
KERNEL_MAP:
    .dword 0               // virt_addr
    .dword 0               // virt_offset
    .dword 0               // phys_addr
    .dword 0               // size
    .dword 0               // va_pa_offset
    .dword 0               // va_kernel_pa_offset
    .dword 0               // page_offset
