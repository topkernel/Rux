//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RISC-V 64-bit kernel boot code
//!
//! Supports SMP multi-core boot
//!
//! When OpenSBI jumps to the kernel:
//! - a0 register contains hart ID
//! - a1 register contains device tree pointer (dtb)

.section .init
.global _start
.global dtb_pointer

_start:
    // a0 register contains hart ID passed by OpenSBI
    // Save hart ID to tp (x4) register (thread pointer)
    mv tp, a0

    // Save a1 (DTB pointer) to s0 register (callee-saved, preserved throughout function)
    mv s0, a1

    // Calculate hart_id * 64KB
    li t1, 65536              // STACK_SIZE = 64KB
    mul t1, tp, t1            // t1 = hart_id * 64KB

    // Calculate stack bottom address
    la sp, _stack_bottom
    add sp, sp, t1            // sp = _stack_bottom + hart_id * 64KB

    // Add 64KB to get stack top
    li t1, 65536
    add sp, sp, t1            // sp = stack top

    // ========== Clear BSS section (only first hart executes) ==========
    //
    // Use atomic flag to ensure only one hart clears BSS
    // Prevents secondary cores from overwriting initialized global variables
    //
    // bss_initialized is in .data section, initial value is 0
    // Use amoadd.w instruction to atomically check and set

    la t1, bss_initialized
    li t2, 1
    amoadd.w t2, t2, (t1)     // t2 = *t1; *t1 += 1 (atomic operation)
    bnez t2, _bss_clear_done  // If t2 != 0, another hart has already cleared

    // First hart executes BSS clearing
    la t1, __bss_start
    la t2, __bss_end
_bss_clear_loop:
    beq t1, t2, _bss_clear_done
    sd zero, 0(t1)           // Use 64-bit store for alignment
    addi t1, t1, 8
    j _bss_clear_loop
_bss_clear_done:

    // Memory barrier: ensure BSS clearing is visible to all harts
    fence rw, rw

    // ========== Save DTB pointer ==========
    // Use s0 (previously saved DTB pointer)
    // Only write if s0 is non-zero
    beqz s0, _skip_dtb
    la t1, dtb_pointer
    sd s0, 0(t1)
_skip_dtb:

    // Jump to Rust code
    call rust_main

    // Should not return, if returned then halt
_hang:
    wfi
    j _hang

    // ========== Data section ==========
    // bss_initialized and dtb_pointer are placed in .data section
    .section .data
    .align 3
    .global bss_initialized
bss_initialized:
    .word 0
    .align 3
    .global dtb_pointer
dtb_pointer:
    .dword 0
