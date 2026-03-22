//! RISC-V 64-bit kernel boot code
//!
//! Simple boot: Run at physical address
//! Virtual address mapping will be set up later in Rust code
//!
//! Boot flow:
//! 1. Kernel loaded at 0x80200000 (physical)
//! 2. Setup stack, clear BSS
//! 3. Jump to rust_main

.section .text.entry
.global _start
.global dtb_pointer
.global KERNEL_MAP

.extern rust_main

// ========================================
// Constants
// ========================================
.set PAGE_SIZE, 4096
.set PAGE_SHIFT, 12

// ========================================
// _start - Entry point
// ========================================
// a0 = hart ID, a1 = DTB pointer
_start:
    mv tp, a0                    // Save hart ID to tp
    mv s0, a1                    // Save DTB pointer to s0

    // Disable interrupts
    csrw sie, zero
    csrw sip, zero

    // Setup early stack
    la sp, _stack_bottom
    li t0, 65536
    add sp, sp, t0               // sp = stack_bottom + 64KB

    // Clear BSS (only first hart)
    // Use atomic CAS to select boot hart
    la t0, bss_done
    li t1, 1
    amoadd.w t2, t1, (t0)
    bnez t2, .Lwait_bss

    // ========================================
    // Boot hart: Clear BSS
    // ========================================
    la t0, __bss_start
    la t1, __bss_end
.Lclear_bss_loop:
    bgeu t0, t1, .Lbss_done
    sd zero, 0(t0)
    addi t0, t0, 8
    j .Lclear_bss_loop

.Lbss_done:
    // BSS clear done - set flag for other harts to continue
    la t0, bss_clear_done
    li t1, 1
    sd t1, 0(t0)
    fence rw, rw

    // Save DTB pointer
    beqz s0, .Lskip_dtb
    la t0, dtb_pointer
    sd s0, 0(t0)
.Lskip_dtb:
    // Jump to rust_main
    call rust_main

    j .Lhang

// ========================================
// Secondary harts: Wait for BSS to be cleared
// ========================================
.Lwait_bss:
    // Wait for bss_clear_done flag
    la t0, bss_clear_done
.Lwait_loop:
    ld t1, 0(t0)
    beqz t1, .Lwait_loop
    fence rw, rw

    // Jump to rust_main
    call rust_main

.Lhang:
    wfi
    j .Lhang

// ========================================
// Data section
// ========================================
.section .data
.align 3
.global bss_done
bss_done:
    .word 0
.global bss_clear_done
bss_clear_done:
    .dword 0
.global dtb_pointer
dtb_pointer:
    .dword 0

// KERNEL_MAP structure (for compatibility)
.global KERNEL_MAP
KERNEL_MAP:
    .dword 0               // virt_addr
    .dword 0               // phys_addr
    .dword 0               // size
    .dword 0               // virt_offset
    .dword 0               // va_pa_offset
    .dword 0               // va_kernel_pa_offset
    .dword 0               // page_offset

// ========================================
// Early page tables (BSS section)
// ========================================
.section .bss
.align 12

// Trampoline page directory
.global trampoline_pg_dir
trampoline_pg_dir:
    .space 4096

// Trampoline PMD
.global trampoline_pmd
trampoline_pmd:
    .space 4096

// Early page directory
.global early_pg_dir
early_pg_dir:
    .space 4096

// Early PMD for kernel region
.global early_pmd
early_pmd:
    .space 4096

// Early PMD for device region
.global early_pmd_dev
early_pmd_dev:
    .space 4096
