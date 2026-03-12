// MIT License
//
// Copyright (c) 2026 Fei Wang
//

// RISC-V user space access functions
//
// Optimized implementation based on Linux kernel (arch/riscv/lib/uaccess.S)
//
// Key design points:
// 1. Use SR_SUM bit to enable user memory access from kernel mode
// 2. Word-aligned copy for better performance (8 bytes at a time on RV64)
// 3. Unrolled loop (8 words per iteration) for bulk copy
// 4. Exception table for safe user space access
//
// Functions:
// - __copy_to_user: Copy from kernel to user space
// - __copy_from_user: Copy from user space to kernel
// - __clear_user: Zero user space memory

.section .text.uaccess

// Constants
.equ SR_SUM, 0x40000         // sstatus SUM bit (bit 18)
.equ SZREG, 8                // Register size (8 bytes on RV64)
.equ SZREG_MASK, -8          // ~(SZREG - 1) = ~7 = -8 (two's complement)
.equ SZREG_BITS, 64          // SZREG * 8 bits
.equ WORD_COPY_MIN, 71       // Minimum size for word copy (9*SZREG - 1)
.equ UNROLL_SIZE, 64         // 8 * SZREG

// ============================================================================
// Exception table macro
// ============================================================================

// EXTABLE(insn, fixup) - Add exception table entry
// When instruction at 'insn' causes an exception, jump to 'fixup'
.macro EXTABLE insn, fixup
    .pushsection .ex_table, "a"
    .balign 8
    .quad \insn
    .quad \fixup
    .popsection
.endm

// ============================================================================
// __copy_to_user - Copy data from kernel to user space
//
// Arguments:
//   a0: user space destination address
//   a1: kernel source address
//   a2: number of bytes to copy
//
// Returns:
//   a0: number of uncopied bytes (0 on success)
// ============================================================================
.global __copy_to_user
.type __copy_to_user, @function
__copy_to_user:
    // Enable user memory access (set SUM bit in sstatus)
    li      t6, SR_SUM
    csrs    sstatus, t6

    // Save return address
    mv      t5, ra

    // Call the internal copy function
    call    __copy_user_sum_enabled

    // Restore return address
    mv      ra, t5

    // Disable user memory access (clear SUM bit)
    li      t6, SR_SUM
    csrc    sstatus, t6

    ret
.size __copy_to_user, . - __copy_to_user

// ============================================================================
// __copy_from_user - Copy data from user space to kernel
//
// Arguments:
//   a0: kernel destination address
//   a1: user space source address
//   a2: number of bytes to copy
//
// Returns:
//   a0: number of uncopied bytes (0 on success)
// ============================================================================
.global __copy_from_user
.type __copy_from_user, @function
__copy_from_user:
    // Enable user memory access
    li      t6, SR_SUM
    csrs    sstatus, t6

    mv      t5, ra
    call    __copy_user_sum_enabled
    mv      ra, t5

    // Disable user memory access
    li      t6, SR_SUM
    csrc    sstatus, t6

    ret
.size __copy_from_user, . - __copy_from_user

// ============================================================================
// __copy_user_sum_enabled - Internal copy function (SUM already enabled)
//
// Arguments:
//   a0: destination address
//   a1: source address
//   a2: number of bytes to copy
//
// Returns:
//   a0: number of uncopied bytes (0 on success)
// ============================================================================
.global __copy_user_sum_enabled
.type __copy_user_sum_enabled, @function
__copy_user_sum_enabled:
    // Save terminal address for calculating uncopied bytes on exception
    add     t5, a0, a2

    // t0 = end of destination
    add     t0, a0, a2

    // If size is too small, use byte copy directly
    li      a3, WORD_COPY_MIN
    bltu    a2, a3, .Lbyte_copy_tail

    // ========================================================================
    // Phase 1: Align destination to word boundary (8 bytes)
    // ========================================================================
    addi    t1, a0, 7
    andi    t1, t1, SZREG_MASK      // t1 = aligned destination

    // Skip if already aligned
    beq     a0, t1, .Lskip_align_dst

    // Copy bytes until aligned
.Lalign_dst_loop:
    lb      a5, 0(a1)
    EXTABLE .Lalign_dst_load, .Lcopy_exception
.Lalign_dst_load:
    sb      a5, 0(a0)
    EXTABLE .Lalign_dst_store, .Lcopy_exception
.Lalign_dst_store:
    addi    a1, a1, 1
    addi    a0, a0, 1
    bltu    a0, t1, .Lalign_dst_loop

.Lskip_align_dst:
    // ========================================================================
    // Phase 2: Word copy (both src and dst aligned)
    // ========================================================================
    // Check if source is also aligned
    andi    a3, a1, 7
    bnez    a3, .Lshift_copy

    // Prepare for unrolled loop
    addi    t0, t0, -UNROLL_SIZE     // Don't overrun

.Lword_copy_loop:
    // Load 8 words
    ld      a4, 0(a1)
    EXTABLE .Lword_load_0, .Lcopy_exception
.Lword_load_0:
    ld      a5, 8(a1)
    EXTABLE .Lword_load_1, .Lcopy_exception
.Lword_load_1:
    ld      a6, 16(a1)
    EXTABLE .Lword_load_2, .Lcopy_exception
.Lword_load_2:
    ld      a7, 24(a1)
    EXTABLE .Lword_load_3, .Lcopy_exception
.Lword_load_3:
    ld      t1, 32(a1)
    EXTABLE .Lword_load_4, .Lcopy_exception
.Lword_load_4:
    ld      t2, 40(a1)
    EXTABLE .Lword_load_5, .Lcopy_exception
.Lword_load_5:
    ld      t3, 48(a1)
    EXTABLE .Lword_load_6, .Lcopy_exception
.Lword_load_6:
    ld      t4, 56(a1)
    EXTABLE .Lword_load_7, .Lcopy_exception
.Lword_load_7:

    // Store 8 words
    sd      a4, 0(a0)
    EXTABLE .Lword_store_0, .Lcopy_exception
.Lword_store_0:
    sd      a5, 8(a0)
    EXTABLE .Lword_store_1, .Lcopy_exception
.Lword_store_1:
    sd      a6, 16(a0)
    EXTABLE .Lword_store_2, .Lcopy_exception
.Lword_store_2:
    sd      a7, 24(a0)
    EXTABLE .Lword_store_3, .Lcopy_exception
.Lword_store_3:
    sd      t1, 32(a0)
    EXTABLE .Lword_store_4, .Lcopy_exception
.Lword_store_4:
    sd      t2, 40(a0)
    EXTABLE .Lword_store_5, .Lcopy_exception
.Lword_store_5:
    sd      t3, 48(a0)
    EXTABLE .Lword_store_6, .Lcopy_exception
.Lword_store_6:
    sd      t4, 56(a0)
    EXTABLE .Lword_store_7, .Lcopy_exception
.Lword_store_7:

    addi    a0, a0, UNROLL_SIZE
    addi    a1, a1, UNROLL_SIZE
    bleu    a0, t0, .Lword_copy_loop

    // Restore t0 to original end
    addi    t0, t0, UNROLL_SIZE
    j       .Lbyte_copy_tail

    // ========================================================================
    // Phase 2b: Shift copy (dst aligned, src misaligned)
    // ========================================================================
.Lshift_copy:
    // Calculate aligned dst end
    andi    t1, t0, SZREG_MASK

    // Align source address
    andi    a1, a1, SZREG_MASK

    // Calculate shift amounts
    slli    t3, a3, 3               // t3 = misalignment * 8 (bits)
    li      a5, SZREG_BITS          // 64 bits
    sub     t4, a5, t3              // t4 = 64 - t3

    // Load first word
    ld      a5, 0(a1)
    EXTABLE .Lshift_load_first, .Lcopy_exception
.Lshift_load_first:

.Lshift_copy_loop:
    srl     a4, a5, t3              // Shift right by prev amount
    ld      a5, 8(a1)               // Load next word
    EXTABLE .Lshift_load, .Lcopy_exception
.Lshift_load:
    addi    a1, a1, 8
    sll     a2, a5, t4              // Shift left by complementary amount
    or      a2, a2, a4              // Combine
    sd      a2, 0(a0)
    EXTABLE .Lshift_store, .Lcopy_exception
.Lshift_store:
    addi    a0, a0, 8
    bltu    a0, t1, .Lshift_copy_loop

    // Restore src to original unaligned value
    add     a1, a1, a3

    // ========================================================================
    // Phase 3: Byte copy tail (remaining bytes)
    // ========================================================================
.Lbyte_copy_tail:
    bgeu    a0, t0, .Lcopy_done

.Lbyte_copy_loop:
    lb      a5, 0(a1)
    EXTABLE .Lbyte_load, .Lcopy_exception
.Lbyte_load:
    sb      a5, 0(a0)
    EXTABLE .Lbyte_store, .Lcopy_exception
.Lbyte_store:
    addi    a1, a1, 1
    addi    a0, a0, 1
    bltu    a0, t0, .Lbyte_copy_loop

.Lcopy_done:
    li      a0, 0
    ret

.Lcopy_exception:
    sub     a0, t5, a0
    ret
.size __copy_user_sum_enabled, . - __copy_user_sum_enabled

// ============================================================================
// __clear_user - Zero user space memory
//
// Arguments:
//   a0: user space address to clear
//   a1: number of bytes to clear
//
// Returns:
//   a0: number of uncleared bytes (0 on success)
// ============================================================================
.global __clear_user
.type __clear_user, @function
__clear_user:
    // Enable user memory access
    li      t6, SR_SUM
    csrs    sstatus, t6

    // a3 = terminal address
    add     a3, a0, a1

    // Calculate aligned boundaries
    addi    t0, a0, 7
    andi    t1, a3, SZREG_MASK      // t1 = aligned end
    andi    t0, t0, SZREG_MASK      // t0 = aligned start

    // Skip word clear if region too small
    bgeu    t0, t1, .Lclear_byte_only

    // Handle initial unaligned bytes
    bltu    a0, t0, .Lclear_align_head

.Lclear_word_loop:
    sd      zero, 0(a0)
    EXTABLE .Lclear_store, .Lclear_exception
.Lclear_store:
    addi    a0, a0, 8
    bltu    a0, t1, .Lclear_word_loop

    // Handle remaining bytes
    bltu    a0, a3, .Lclear_tail
    j       .Lclear_done

.Lclear_align_head:
    sb      zero, 0(a0)
    EXTABLE .Lclear_head_store, .Lclear_exception
.Lclear_head_store:
    addi    a0, a0, 1
    bltu    a0, t0, .Lclear_align_head
    j       .Lclear_word_loop

.Lclear_byte_only:
    bltu    a0, a3, .Lclear_tail
    j       .Lclear_done

.Lclear_tail:
    sb      zero, 0(a0)
    EXTABLE .Lclear_tail_store, .Lclear_exception
.Lclear_tail_store:
    addi    a0, a0, 1
    bltu    a0, a3, .Lclear_tail

.Lclear_done:
    // Disable user memory access
    li      t6, SR_SUM
    csrc    sstatus, t6
    li      a0, 0
    ret

.Lclear_exception:
    // Disable user memory access
    li      t6, SR_SUM
    csrc    sstatus, t6
    sub     a0, a3, a0
    ret
.size __clear_user, . - __clear_user
