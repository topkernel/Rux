// MIT License
//
// Copyright (c) 2026 Fei Wang
//
// AArch64 kernel boot code — QEMU `virt` direct-boot (EL2 → EL1).
//
// Entry state (QEMU -kernel ELF load):
//   PC  = _start (physical, 0x40080000 = RAM base + text_offset)
//   x0  = device-tree pointer
//   EL  = 2 (highest non-secure EL; virt machine default)
//   MMU = off, caches = off
//
// Flow:
//   1. If at EL2: configure HCR/CNTHCTL, eret down to EL1h
//   2. Save x0 (DTB) into the dtb_pointer static (physical addressing)
//   3. Clear BSS (physical addressing), set TPIDR_EL1 = 0 (cpu index)
//   4. Build the early translation tables:
//        TTBR0: identity map of [0, 2GB) — early phys access (DTB, devices)
//        TTBR1: kernel image window at 0xffffffff80000000 (2MB blocks
//               covering phys [0x40000000, 0x42200000) — the image sits
//               at phys 0x40080000 with a 512KB offset inside the window;
//               block maps need both ends 2MB-aligned, hence the window is
//               mapped from phys 0x40000000)
//   5. MAIR/TCR/TTBR/SCTLR — MMU on (48-bit VA, 4K granule, 4 levels)
//   6. Populate KERNEL_MAP, switch to the high-half boot stack, branch to
//      rust_main at its linked VA.
//
// The image budget is 32MB (linker.ld ASSERTs); the early window maps 34MB.

.section .text.entry, "ax"
.global _start
.type _start, %function

// Early PL011 putc (physical addressing, pre- and post-MMU safe via the
// identity map): polls FR bit 5 (TX FIFO not full), writes DR.
.macro EARLY_PUTC, ch
    ldr     x20, =0x09000000
1:  ldr     w21, [x20, #0x18]          // FR
    tbz     w21, #5, 2f                // THRE set → ready
    b       1b
2:  mov     w22, #\ch
    str     w22, [x20]                 // DR
.endm

_start:
    // Enable the PL011 (QEMU drops writes to a disabled UART):
    //   ICR = 0x7FF (clear masked ints), IBRD/FBRD ignored by QEMU,
    //   CR  = 0x301 (UARTEN | TXE | RXE)
    ldr     x20, =0x09000000
    ldr     x21, =0x7FF
    str     w21, [x20, #0x44]
    ldr     x21, =0x301
    str     w21, [x20, #0x30]
    EARLY_PUTC 'A' 
    // ------------------------------------------------------------------
    // 1. EL2 → EL1 (QEMU virt enters at EL2)
    // ------------------------------------------------------------------
    mrs     x1, CurrentEL
    cmp     x1, #0x8                    // CurrentEL = EL << 2; 0x8 = EL2
    b.eq    .Ldrop_to_el1
    cmp     x1, #0x4                    // 0x4 = EL1 — nothing to do
    b.eq    .Lat_el1

.Ldrop_to_el1:
    // HCR_EL2: RW (bit 31) — EL1 is AArch64; all interrupts route to EL1
    ldr     x0, =(1 << 31)
    msr     hcr_el2, x0
    // Let EL1 use the physical AND virtual timers (CNTHCTL bits 0/1)
    mov     x0, #0x3
    msr     cnthctl_el2, x0
    // SCTLR_EL1: everything OFF at EL1 entry (M=0! the old #0x5 set M=1
    // and let EL1 fetch through garbage TTBRs until the first isb —
    // the boot-time translation-fault loop)
    msr     sctlr_el1, xzr
    isb
    // Return to EL1h at .Lat_el1 with DAIF masked
    mov     x0, #0x3c5                  // D|A|I|F masked (9..6) | M=EL1h(0b0101)
    msr     spsr_el2, x0
    adr     x0, .Lat_el1
    msr     elr_el2, x0
    eret

.Lat_el1:
    EARLY_PUTC 'B'
    // Publish the embedded DTB's address (QEMU's ELF -kernel boot zeroes
    // x0, so there is no firmware pointer to save — see the blob embed).
    adrp    x19, aarch64_virt_dtb
    add     x19, x19, :lo12:aarch64_virt_dtb
    adrp    x1, dtb_pointer
    add     x1, x1, :lo12:dtb_pointer
    str     x19, [x1]
    // Enable FP/SIMD at EL1/EL0 (CPACR_EL1.FPEN = 0b11): core::mem::zeroed
    // and friends emit NEON stores (movi/str q0); without this every such
    // instruction traps with EC=7 and the boot dies in its first memset.
    mrs     x0, cpacr_el1
    orr     x0, x0, #(0b11 << 20)
    msr     cpacr_el1, x0
    isb
    // Park the vector base early so any early fault lands somewhere
    // visible instead of VA 0x200 (physical address; valid pre-MMU and
    // via the identity map afterwards)
    adrp    x0, aarch64_vector_table
    add     x0, x0, :lo12:aarch64_vector_table
    msr     vbar_el1, x0
    isb
    // (The DTB pointer was published above with physical addressing and
    // is re-published at its linked VA once the MMU is on; nothing may
    // read it in between. An older draft stored x0 here — the vector
    // table base by then — clobbering the DTB pointer until the
    // post-MMU re-publish repaired it; two review lanes removed the
    // store independently.)
    // Per-CPU identity: cpu index 0 (TPIDR_EL1 protocol, smp::cpu_id)
    msr     tpidr_el1, xzr

    // Mask DAIF during early boot
    msr     daifset, #0xf

    // ------------------------------------------------------------------
    // 3. Clear BSS (physical addressing via PC-relative arithmetic)
    // ------------------------------------------------------------------
    adrp    x1, __bss_start             // PC-relative → physical
    add     x1, x1, :lo12:__bss_start
    adrp    x2, __bss_end
    add     x2, x2, :lo12:__bss_end
.Lclear_bss:
    cmp     x1, x2
    b.hs    .Lbss_done
    stp     xzr, xzr, [x1], #16
    b       .Lclear_bss
.Lbss_done:

    // ------------------------------------------------------------------
    // 4. Early translation tables (in .bss; physical addressing)
    // ------------------------------------------------------------------
    // Zero the tables we use
    adrp    x1, idmap_l0
    add     x1, x1, :lo12:idmap_l0
    add     x2, x1, #0x1000
.Lz1:   stp     xzr, xzr, [x1], #16
        cmp     x1, x2
        b.lo    .Lz1
    adrp    x1, idmap_l1
    add     x1, x1, :lo12:idmap_l1
    add     x2, x1, #0x2000             // two L1 tables (2GB)
.Lz2:   stp     xzr, xzr, [x1], #16
        cmp     x1, x2
        b.lo    .Lz2
    adrp    x1, kern_l0
    add     x1, x1, :lo12:kern_l0
    add     x2, x1, #0x2000             // L0 + L1 + L2 (3 pages)
.Lz3:   stp     xzr, xzr, [x1], #16
        cmp     x1, x2
        b.lo    .Lz3

    // Descriptor templates:
    //   ATTR_NORMAL = 0x0400 (AttrIndx 0 | AF | SH_IS; block bit added per level)
    //   table ptr   = valid (0x3: valid + table)
    ldr     x10, =(1 << 10) | (0b11 << 8) | (0b00 << 2)   // AF | SH=IS | AttrIndx0 (normal)
    orr     x11, x10, #1                 // + valid, bit1 CLEAR = 0b01 BLOCK descriptor

    // ---- TTBR0 identity map: L0[0] → idmap_l1, blocks [0, 1GB) each ----
    adrp    x0, idmap_l0
    add     x0, x0, :lo12:idmap_l0
    adrp    x1, idmap_l1
    add     x1, x1, :lo12:idmap_l1
    orr     x1, x1, #3                   // page addr | 0b11 = table descriptor
    str     x1, [x0]
    // idmap_l1[0] → device block phys 0; [1] → normal block phys 1GB (RAM)
    // idmap_l1[0] must ALSO cover the device area with normal attrs?
    // Device attrs matter for correctness of MMIO ordering — map phys
    // [0, 0x40000000) as DEVICE (AttrIndx 1) and RAM above as NORMAL.
    adrp    x1, idmap_l1
    add     x1, x1, :lo12:idmap_l1
    // entry 0: phys 0, device (AttrIndx 1)
    mov     x2, #(0b001 << 2) | (1 << 10) | (0b11 << 8) | 0x1   // block: bits[1:0]=01
    str     x2, [x1]
    // entry 1: phys 1GB (0x40000000 = RAM base), normal block
    ldr     x2, =0x40000000
    orr     x2, x2, x11                  // normal attrs + valid + block
    str     x2, [x1, #8]

    // ---- TTBR1 early linear map: L0[0x180] -> lin_l1 ----
    // PAGE_OFFSET (0xffff_c000_0000_0000) sits at L0 index 0x180 — a
    // different subtree than the kernel window (L0[511]). One 1GB block at
    // lin_l1[1] covers phys [0x40000000, 0x80000000) = all RAM on -m 1G,
    // so PAGE_OFFSET-relative accesses (the heap window) are valid from
    // MMU-on; setup_linear_mapping refines this later.
    adrp    x0, kern_l0
    add     x0, x0, :lo12:kern_l0
    adrp    x1, lin_l1
    add     x1, x1, :lo12:lin_l1
    orr     x1, x1, #3
    str     x1, [x0, #0x180*8]

    adrp    x1, lin_l1
    add     x1, x1, :lo12:lin_l1
    ldr     x2, =0x40000000
    orr     x2, x2, x11                  // normal attrs + valid + 1GB block
    str     x2, [x1, #8]                 // L1 index 1 = phys 0x40000000

    // ---- TTBR1 kernel window: L0[511] → kern_l1 → kern_l2 (2MB blocks) ----
    // VA base 0xffffffff80000000 ↔ phys 0x40000000 (17 × 2MB = 34MB)
    adrp    x0, kern_l0
    add     x0, x0, :lo12:kern_l0
    adrp    x1, kern_l1
    add     x1, x1, :lo12:kern_l1
    orr     x1, x1, #3                   // page addr | 0b11 = table descriptor
    str     x1, [x0, #511*8]

    // kern_l1[0] → kern_l2 (the window is inside L1 entry 0's 1GB span)
    adrp    x0, kern_l1
    add     x0, x0, :lo12:kern_l1
    adrp    x1, kern_l2
    add     x1, x1, :lo12:kern_l2
    orr     x1, x1, #3                   // page addr | 0b11 = table descriptor
    // VA 0xffffffff80000000 sits at L1 index 510 (VA bits 38:30), NOT 0 —
    // the port's original [0] slot made every high-half fetch a level-1
    // translation fault (the boot-time rust_main-entry abort loop)
    str     x1, [x0, #510*8]

    // kern_l2[i] = block (phys 0x40000000 + i*2MB), normal RWX kernel
    mov     x0, #17                      // 17 blocks
    ldr     x1, =0x40000000
    adrp    x2, kern_l2
    add     x2, x2, :lo12:kern_l2
.Lfill_kern_l2:
    orr     x3, x1, x11                  // attrs + block
    str     x3, [x2], #8
    add     x1, x1, #0x200000
    subs    x0, x0, #1
    b.ne    .Lfill_kern_l2

    // ------------------------------------------------------------------
    // 5. MMU on: MAIR → TCR → TTBR0/1 → SCTLR.M
    // ------------------------------------------------------------------
    // MAIR_EL1: attr0 = normal WB-cacheable, attr1 = device nGnRE,
    //           attr2 = normal non-cacheable
    ldr     x0, =((0xFF << 0) | (0x04 << 8) | (0x44 << 16))
    msr     mair_el1, x0

    // TCR_EL1: IPS=0 (32-bit PA), TG1=4KB(2<<30), SH1=IS(3<<28),
    //          ORGN1/IRGN1=WB(1<<26 / 1<<24), T1SZ=16 (48-bit),
    //          TG0=4KB(0), SH0=IS(3<<12), ORGN0/IRGN0=WB, T0SZ=16
    // TCR = IPS(2b101=44-bit PA)<<32 | TG1(2b10)<<30 | SH1(3)<<28
    //       | ORGN1(1)<<26 | IRGN1(1)<<24 | T1SZ(16)<<16 | SH0(3)<<12
    //       | ORGN0(1)<<10 | IRGN0(1)<<8 | T0SZ(16)
    // IPS must cover QEMU virt's highmem windows: ECAM at phys
    // 0x40_1000_0000 and the 64-bit MMIO window at 0x80_0000_0000 — the
    // original 32-bit IPS faulted every access above 4GB with a level-2
    // address-size abort (cortex-a72 implements 44-bit PA).
    ldr     x0, =0x5B5103510
    msr     tcr_el1, x0

    // TTBR0 = idmap_l0 (phys), TTBR1 = kern_l0 (phys)
    adrp    x0, idmap_l0
    add     x0, x0, :lo12:idmap_l0
    msr     ttbr0_el1, x0
    adrp    x0, kern_l0
    add     x0, x0, :lo12:kern_l0
    msr     ttbr1_el1, x0
    isb

    // Canonical MMU-enable sequence: tables complete (dsb), no stale
    // TLB state (tlbi), then M|C|I with isb context sync
    dsb     sy
    tlbi    vmalle1
    dsb     sy
    isb
    mrs     x0, sctlr_el1
    orr     x0, x0, #1                  // M only (minimal-kernel parity; C/I come later)
    msr     sctlr_el1, x0
    isb

    EARLY_PUTC 'M'                     // MMU is on
    // Re-publish the DTB pointer at its LINKED VA (the pre-MMU adrp above
    // stored the physical address; Rust walks the .rodata blob at its
    // linked address).
    adrp    x19, aarch64_virt_dtb
    add     x19, x19, :lo12:aarch64_virt_dtb
    adrp    x1, dtb_pointer
    add     x1, x1, :lo12:dtb_pointer
    str     x19, [x1]
    // ------------------------------------------------------------------
    // 6. Populate KERNEL_MAP (physical addressing), switch to the
    //    high-half stack, branch to rust_main
    // ------------------------------------------------------------------
    // KernelMapping layout (repr(C), usize fields):
    //   +0 virt_addr, +8 virt_offset, +16 phys_addr, +24 size,
    //   +32 va_pa_offset, +40 va_kernel_pa_offset, +48 page_offset
    adrp    x0, KERNEL_MAP
    add     x0, x0, :lo12:KERNEL_MAP
    ldr     x1, =0xffffffff80080000   // image VA base (window + 512KB)
    str     x1, [x0]
    str     xzr, [x0, #8]                // no KASLR
    ldr     x1, =0x40080000             // image phys base
    str     x1, [x0, #16]
    ldr     x1, =0x2000000              // 32MB window budget
    str     x1, [x0, #24]
    ldr     x1, =0xffffc00000000000 - 0x40000000   // va_pa_offset
    str     x1, [x0, #32]
    ldr     x1, =0xffffffff40000000   // va_kernel_pa_offset
    str     x1, [x0, #40]
    ldr     x1, =0xffffc00000000000   // page_offset
    str     x1, [x0, #48]

    // High-half boot stack: _stack_top VA → same VA now that TTBR1 is live
    ldr     x0, =_stack_top
    mov     sp, x0

    EARLY_PUTC 'J'                     // about to enter Rust
    // Branch to rust_main at its LINKED (high) VA
    ldr     x30, =rust_main
    ret

// ============================================================================
// secondary_start — PSCI CPU_ON entry for secondary cores (stage 3)
//
// Entered at the PHYSICAL address by PSCI CPU_ON (start_secondaries() in
// smp.rs passes `secondary_start - va_kernel_pa_offset` as the entry point):
//   x0  = context_id (cpu index, packed by the boot core)
//   MMU off, D/I-caches off, DAIF masked
//   EL  = EL1h normally; EL2h on virtualization=on machines (dropped below)
//
// Sequence mirrors the boot core path: MPIDR cross-check of the cpu index,
// TPIDR_EL1 = index (smp::cpu_id protocol), per-cpu boot stack (physical,
// identity-mapped), restore the boot core's saved TTBR1/TCR/MAIR, MMU on
// against the static identity map (TTBR0 = idmap_l0 — the early identity
// tables are image .bss and stay live for exactly this purpose), convert
// the stack to its high VA, branch to secondary_cpu_entry (Rust).
// ============================================================================
.global secondary_start
.type secondary_start, %function
secondary_start:
    // --- optional EL2 → EL1 drop (same recipe as _start, minus the
    //     one-time DTB/BSS work secondaries must never redo) ---
    mrs     x9, CurrentEL
    cmp     x9, #0x8
    b.ne    .Lsec_at_el1
    ldr     x10, =(1 << 31)             // HCR_EL2.RW — EL1 is AArch64
    msr     hcr_el2, x10
    mov     x10, #0x3                   // CNTHCTL: physical+virtual timers
    msr     cnthctl_el2, x10
    msr     sctlr_el1, xzr              // MMU off at EL1 entry
    isb
    mov     x10, #0x3c5                 // D|A|I|F masked | M=EL1h
    msr     spsr_el2, x10
    adr     x10, .Lsec_at_el1
    msr     elr_el2, x10
    eret

.Lsec_at_el1:
    // Enable FP/SIMD at EL1/EL0 (CPACR_EL1.FPEN = 0b11) — the boot core
    // does this in _start; a PSCI-woken secondary starts from the reset
    // value and would trap its first NEON store (EC 7) into the illegal
    // instruction path, oopsing and killing the idle task. Invisible
    // until IRQs worked: secondaries never ran kernel code before that.
    mrs     x3, cpacr_el1
    orr     x3, x3, #(0b11 << 20)
    msr     cpacr_el1, x3
    isb

    // --- cpu index: trust the PSCI context_id, fall back to our own
    //     MPIDR affinity pack (Aff0 | Aff1<<8 | Aff2<<16) ---
    mrs     x20, mpidr_el1
    and     x21, x20, #0xFF             // Aff0
    ubfx    x22, x20, #8, #8            // Aff1
    ubfx    x23, x20, #16, #8           // Aff2
    orr     x21, x21, x22, lsl #8
    orr     x21, x21, x23, lsl #16      // x21 = packed affinity
    cbnz    x0, 1f
    mov     x0, x21                     // no context_id → derive locally
1:  cmp     x0, #4                      // MAX_CPUS (smp.rs const-asserts)
    b.hs    .Lsec_park                  // bad index has no stack
    msr     tpidr_el1, x0               // per-cpu identity protocol

    // --- per-cpu boot stack (physical; image .bss is identity-mapped) ---
    adrp    x1, SECONDARY_BOOT_STACKS
    add     x1, x1, :lo12:SECONDARY_BOOT_STACKS
    mov     x2, #0x10000                // SMP_BOOT_STACK_SIZE (smp.rs asserts)
    madd    x1, x0, x2, x1              // base + index * 64KB
    add     sp, x1, x2                  // top, physical

    // --- restore the boot core's MMU state (published by
    //     start_secondaries; TTBR1 = live kernel root) ---
    adrp    x3, BOOT_HART_TTBR
    add     x3, x3, :lo12:BOOT_HART_TTBR
    ldr     x3, [x3]
    msr     ttbr1_el1, x3
    adrp    x3, BOOT_HART_TCR
    add     x3, x3, :lo12:BOOT_HART_TCR
    ldr     x3, [x3]
    msr     tcr_el1, x3
    adrp    x3, BOOT_HART_MAIR
    add     x3, x3, :lo12:BOOT_HART_MAIR
    ldr     x3, [x3]
    msr     mair_el1, x3
    // TTBR0 = the static early identity map (image .bss, maps phys
    // [0, 2GB)) — the post-MMU fetch at the physical PC and the physical
    // stack both live inside it; per-task TTBR0 is loaded by context
    // switching later.
    adrp    x3, idmap_l0
    add     x3, x3, :lo12:idmap_l0
    msr     ttbr0_el1, x3
    // Park the vector base early so any fault lands visibly
    // (trap::init() reprograms VBAR_EL1 from Rust)
    adrp    x3, aarch64_vector_table
    add     x3, x3, :lo12:aarch64_vector_table
    msr     vbar_el1, x3
    dsb     sy
    tlbi    vmalle1                     // drop stale local TLB state
    dsb     sy
    isb
    mrs     x3, sctlr_el1
    orr     x3, x3, #1                  // M only (boot-core parity)
    msr     sctlr_el1, x3
    isb

    // --- stack PA → high VA (KERNEL_MAP.va_kernel_pa_offset at +40) ---
    adrp    x4, KERNEL_MAP
    add     x4, x4, :lo12:KERNEL_MAP
    ldr     x5, [x4, #40]
    add     sp, sp, x5

    // --- Rust secondary entry (x0 = cpu index preserved) ---
    ldr     x30, =secondary_cpu_entry
    ret

.Lsec_park:
    wfe
    b       .Lsec_park
.size secondary_start, . - secondary_start

// ==================== data ====================

// DTB pointer consumed by boot::get_dtb_pointer()
.data
.global dtb_pointer
dtb_pointer:
    .xword  0

// Kernel mapping block consumed by memory_layout (populated in step 6;
// 7 × u64: virt_addr, virt_offset, phys_addr, size, va_pa_offset,
// va_kernel_pa_offset, page_offset)
.align 8
.global KERNEL_MAP
KERNEL_MAP:
    .xword  0
    .xword  0
    .xword  0
    .xword  0
    .xword  0
    .xword  0
    .xword  0

// Early translation tables (.bss, 4KB aligned)
.bss
.align 12
.global idmap_l0
idmap_l0:    .space 4096
.global idmap_l1
idmap_l1:    .space 8192          // two L1 tables ([0,1GB) and [1GB,2GB))
.bss
.align 12
.global kern_l0
kern_l0:     .space 4096
.global kern_l1
kern_l1:     .space 4096
.global kern_l2
kern_l2:     .space 4096
.global lin_l1
lin_l1:      .space 4096          // early linear map (PAGE_OFFSET subtree L0[0x180])

// Per-CPU interrupt stack base (smp::init_per_cpu_intr_stacks publishes it)
.bss
.align 8
.global __per_cpu_intr_stacks_base
__per_cpu_intr_stacks_base:
    .xword  0
