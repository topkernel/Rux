# Rux Kernel MMU Debugging Complete Record

## Document Information

- **Creation Date**: 2025-02-04
- **Author**: Claude AI Assistant
- **Related Files**: `kernel/src/arch/aarch64/mm.rs`
- **Target**: Enable MMU on ARMv8-A (aarch64) architecture

---

## Table of Contents

1. [Problem Description](#1-problem-description)
2. [Attempted Solutions](#2-attempted-solutions)
3. [Key Findings](#3-key-findings)
4. [Final Solution](#4-final-solution)
5. [Technical Summary](#5-technical-summary)
6. [References](#6-references)

---

## 1. Problem Description

### 1.1 Initial State

Rux kernel had implemented a basic boot framework, but MMU was disabled. The goal was to enable MMU on QEMU virt machine (ARMv8-A) to implement virtual memory management.

### 1.2 Environment Information

```
Platform: QEMU virt machine
Architecture: ARMv8-A (aarch64)
CPU: cortex-a57
Memory: 2GB
Kernel Load Address: 0x4000_0000
UART Address: 0x0900_0000
```

### 1.3 Symptoms

After enabling MMU, the system immediately hung with no output.

---

## 2. Attempted Solutions

### 2.1 Solution 1: 48-bit VA + Level 1 (1GB blocks) - Failed

**Configuration**:
- T0SZ = 16 (48-bit virtual address)
- Starting Level: Level 1
- Page Table Granularity: 1GB blocks
- Mapping:
  - Entry 0: 0x0000_0000 (device region)
  - Entry 1: 0x4000_0000 (kernel region)

**Code**:
```rust
// Calculate descriptor
let l1_normal_desc = ((0x4000_0000u64 >> 30) & 0x3FFFF) << 30 |
                     (1 << 10) |  // AF
                     (3 << 8) |   // SH
                     (0 << 6) |   // AP
                     (0 << 2) |   // AttrIndx
                     0b01;        // Block

(*l1_table).entries[1].value = l1_normal_desc;
```

**Result**: System hung

**Analysis**: (Not discovered at the time)

---

### 2.2 Solution 2: 39-bit VA + Level 2 (2MB blocks) - Failed

**Configuration**:
- T0SZ = 25 (39-bit virtual address)
- Starting Level: Level 2
- Page Table Granularity: 2MB blocks
- Mapping:
  - Entry 0: 0x0000_0000
  - Entry 2: 0x4000_0000

**Key Error**: Used entry 2 instead of entry 1

**Result**: System hung

**Reason**:
```
For 0x4000_0000:
level 2 index = 0x4000_0000 >> 30 = 2  <- This is wrong!
```

---

### 2.3 Solution 3: Attempted to Enable Caching - Failed

**Modification**: Enable data cache and instruction cache in SCTLR

```rust
sctlr |= (1 << 0);   // M: MMU enable
sctlr |= (1 << 2);   // C: Data cache enable
sctlr |= (1 << 12);  // I: Instruction cache enable
```

**Result**: System still hung

---

## 3. Key Findings

### 3.1 Finding 1: Incorrect PC Address Level 1 Index Calculation

Added debug code to check PC and page table index:

```rust
let current_pc: u64;
asm!("adr {}, #0", out(reg) current_pc);

let pc_l1_index = (current_pc >> 39) & 0x1FF;
```

**Output**:
```
MM: Current PC = 0x000000004000678C
MM: PC L1 index = 0 (should be 1 for 0x4000_0000)
```

**Analysis**:
```
PC = 0x4000_678C
Binary: 0b0000_0000_0100_0000_0000_0000_0000_0110_0111_1000_1100
      = 0b0000_0000_0000_0000_0000_0000_0000_0000_0100_0000_0000_0000_0000_0000_0110_0111_1000_1100

Level 1 index uses VA[47:39] (bits 47-39)
Bits 39-47 of 0x4000_678C are all 0
So: 0x4000_678C >> 39 = 0
```

**Conclusion**:
- For 0x4000_678C, Level 1 index is **0**, not 1!
- Mapping kernel at entry 1 was incorrect
- Should map at entry 0

---

### 3.2 Finding 2: 1GB Block Too Large Causing Incorrect Address Mapping

Even after fixing to use entry 0, there was still a problem:

**Problem**:
```
Using 1GB block mapping for 0x0000_0000:
- Output address = (VA & ~0x3FFF_FFFF) | (descriptor PA << 30)

For VA = 0x4000_678C:
- Output PA = (0x4000_678C & ~0x3FFF_FFFF) | (0 << 30)
- Output PA = 0x0000_0000  <- Wrong! Should be 0x4000_678C
```

**Root Cause**:
- 1GB block granularity is too large
- All VAs in range 0x0000_0000-0x3FFF_FFFF would map to 0x0000_0000
- Cannot precisely map 0x4000_0000 region

---

### 3.3 Finding 3: UART Address Not Within 2MB Block

When attempting to use Level 2's 2MB blocks, there was another issue:

```
UART Address: 0x0900_0000
Level 2 Index: 0x0900_0000 >> 30 = 0

My entry 0 mapping: 0x0000_0000 - 0x001F_FFFF (2MB)
UART actual location: 0x0900_0000 (not in mapping range!)
```

**Conclusion**: Need to ensure all accessed addresses are within the mapping range.

---

## 4. Final Solution

### 4.1 Correct Configuration

**Virtual Address Space**: 39-bit (T0SZ=25)

**Page Table Hierarchy**:
- Starting Level: Level 2
- Block Size: 2MB
- Level 2 Index: VA[38:30]

**Key Calculation**:
```python
# For PC = 0x4000_678C
pc_l2_index = (0x4000_678C >> 30) & 0x1FF
            = 1  Correct!
```

**Page Table Mapping**:
```rust
// Entry 0: Device region
let l2_device_desc = ((0u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                     (1 << 10) |  // AF
                     (3 << 8) |   // SH
                     (0 << 6) |   // AP
                     (1 << 2) |   // Device memory
                     0b01;        // Block
(*l2_table).entries[0].value = l2_device_desc;

// Entry 1: Kernel region
let l2_normal_desc = ((0x4000_0000u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                     (1 << 10) |  // AF
                     (3 << 8) |   // SH
                     (0 << 6) |   // AP
                     (0 << 2) |   // Normal memory
                     0b01;        // Block
(*l2_table).entries[1].value = l2_normal_desc;
```

**TCR Configuration**:
```rust
let tcr: u64 = (25 << 0) |     // T0SZ: 39-bit VA (level 2-3)
               (0b01 << 8) |   // IRGN0: Normal WB-WA Inner
               (0b01 << 10) |  // ORGN0: Normal WB-WA Outer
               (0b11 << 12) |  // SH0: Inner shareable
               (0b00 << 14) |  // TG0: 4KB granule
               (1 << 23);      // EPD1: Disable TTBR1
```

**TTBR0 Configuration**:
```rust
// Point to Level 2 page table
let l2_table_addr = &raw mut LEVEL2_PAGE_TABLE.table as u64;
asm!("msr ttbr0_el1, {}", in(reg) l2_table_addr);
```

### 4.2 Successful Output

```
MM: Setting up L2 page tables (2MB blocks)...
MM: Clearing L2 table...
MM: L2 table cleared
MM: L2 entry 0 set (2MB device at 0x0000_0000)
MM: L2 entry 1 set (2MB normal at 0x4000_0000)
MM: Page tables setup complete (2 L2 entries)
MM: L2 page table addr=0x0000000040026000
MM: Setting MAIR...
MM: Setting TTBR0 to L2 table...
MM: Setting TCR (T0SZ=25, 39-bit VA, L2 start)...
MM: Computed TCR = 0x0000000000803519 (T0SZ=25, 39-bit VA, level 2 start)
MM: Flushing caches and TLBs...
MM: Current PC = 0x000000004000678C
MM: PC L2 index = 1 (should be 1 for 0x4000_0000)
MM: Enabling MMU only (caches disabled)...
MM: ISB after MMU enable...
MM: MMU setup complete!
MM: SCTLR after enable = 0x0000000000000001  <- MMU bit is set!
MM: Current PC = 0x0000000040006DB0           <- PC advanced!
MM: MMU enabled successfully!
```

**System Continues Running**:
```
Before trap init
Initializing trap handling...
After trap init
Initializing system calls...
System call support initialized
Initializing heap...
Testing direct allocator call...
```

**MMU Successfully Enabled!**

---

## 5. Technical Summary

### 5.1 ARMv8 Page Table Hierarchy

ARMv8 supports 4 levels of page tables (4KB granule):

| Level | Index Bits | Block Size | T0SZ Range |
|-------|------------|------------|------------|
| Level 0 | VA[47:39] | 1TB | 16-24 |
| Level 1 | VA[38:30] | 1GB | 25-33 |
| Level 2 | VA[29:21] | 2MB | 34-42 |
| Level 3 | VA[20:12] | 4KB | 43-51 |

**Starting Level Calculation**:
```
If T0SZ = 25:
  VA size = 64 - 25 = 39 bits
  Starting level = 48 - VA size = 48 - 39 = 9 (failed)

  Correct calculation:
  Starting level = (48 - T0SZ) / 9 rounded down
  = (48 - 25) / 9
  = 23 / 9
  = 2 (Level 2)
```

### 5.2 Level 2 Block Descriptor Format

**2MB Block Descriptor** (Block Descriptor at Level 2):

```
Bits [47:21]:  Output address[47:21] (physical address >> 21)
Bit [10]:     AF (Access Flag)
Bits [9:8]:   SH (Shareability)
Bits [7:6]:   AP (Access Permissions)
Bits [5:2]:   AttrIndx (Memory Attributes)
Bits [1:0]:   0b01 (Block Descriptor)
```

**Example Calculation** (mapping 0x4000_0000):
```python
pa = 0x4000_0000
pa_field = (pa >> 21) & 0x3FFFF_FFFF = 0x2000
descriptor = (pa_field << 21) | (1<<10) | (3<<8) | (0<<6) | (0<<2) | 0b01
           = 0x4000_0000 | 0x400 | 0x300 | 0x01
           = 0x4000_0701
```

### 5.3 TCR_EL1 Register

**Key Bits**:
- T0SZ[5:0]: Translation Table Size
  - T0SZ = 25 -> 39-bit VA
- TG0[1:0]: Translation Granule
  - 0b00 = 4KB
- IRGN0[1:0]: Inner Region Cacheability
  - 0b01 = Normal WB-WA Inner
- ORGN0[1:0]: Outer Region Cacheability
  - 0b01 = Normal WB-WA Outer
- SH0[1:0]: Shareability
  - 0b11 = Inner Shareable
- EPD1: Disable TTBR1

### 5.4 Address Translation Process Example

**Input**: VA = 0x4000_678C

**Steps**:
```
1. Check T0SZ=25 (39-bit VA), starting level=2
2. Extract Level 2 index: VA[38:30] = 1
3. Read page table entry 1
4. Descriptor type = Block (bits[1:0] = 0b01)
5. Extract output address: descriptor[47:21] = 0x4000_0000 >> 21 = 0x2000
6. Calculate output PA: (0x2000 << 21) | (VA & 0x1FFFFF)
7. Output PA = 0x4000_0000 | 0x678C = 0x4000_678C  (identity mapping)
```

---

## 6. References

### 6.1 ARM Official Documentation

- **ARM Architecture Reference Manual ARMv8-A**
  - Chapter D4: The AArch64 Virtual Memory System Architecture
  - Chapter G5: System Control Registers (in AArch64)

- **ARMv8-A Address Translation**
  - https://developer.arm.com/documentation/ddi0487/latest

### 6.2 Linux Kernel Source

- **arch/arm64/mm/mmu.c**
  - Page table initialization
  - MMU enable process

- **arch/arm64/kernel/traps.c**
  - Address exception handling

### 6.3 QEMU Documentation

- **QEMU virt machine**
  - Memory layout
  - Device address mapping

---

## 7. Debugging Tips Summary

### 7.1 Adding Debug Output

```rust
// Print key register values
asm!("mrs {}, sctlr_el1", out(reg) sctlr);

// Print current PC
asm!("adr {}, #0", out(reg) pc);

// Print page table index
let l2_index = (pc >> 30) & 0x1FF;
```

### 7.2 Systematic Debugging Method

1. **Verify Calculations**: Use Python or calculator to verify page table index calculations
2. **Step-by-step Verification**: First verify page table setup, then MMU enable
3. **Compare References**: Compare with Linux kernel implementation
4. **Use Documentation**: ARM ARM has detailed official documentation

### 7.3 Common Pitfalls

1. **Incorrect Page Table Index Calculation**: Confusing index bits for different levels
2. **Incorrect Block Descriptor Format**: Wrong bit widths, positions
3. **Wrong Address Range**: Actual accessed address not in mapping range
4. **T0SZ and Starting Level Mismatch**: Causing translation failure
5. **Forgetting to Set Attributes**: Missing AF, SH, AP attributes

---

## 8. Appendix: Complete Code

### 8.1 Page Table Setup Code

```rust
/// Setup page tables (using level 2, 2MB blocks)
///
/// Use T0SZ=25 (39-bit VA), starting from level 2, using 2MB blocks
/// - VA[38:30] indexes level 2 table (9 bits, 512 entries)
/// - Each level 2 entry: 2MB block
///
/// For 0x4000_678C:
/// - level 2 index = 0x4000_678C >> 30 = 1
///
/// Mapping strategy:
/// - Entry 0: 0x0000_0000 - 0x001F_FFFF (UART, etc.)
/// - Entry 1: 0x4000_0000 - 0x401F_FFFF (kernel)
unsafe fn setup_two_level_page_tables() {
    // Use level 2 table
    let l2_table = &raw mut LEVEL2_PAGE_TABLE.table;

    // Zero level 2 table
    for i in 0..512 {
        (*l2_table).entries[i].value = 0;
    }

    // Level 2 block descriptor format (2MB block):
    // [47:21] physical address >> 21
    // [10] AF = 1
    // [9:8] SH = 11 (Inner shareable)
    // [7:6] AP = 00 (EL1 RW)
    // [5:2] AttrIndx = 0000 (Normal) or 0001 (Device)
    // [1:0] = 01 (Block descriptor)

    // Entry 0: Map 0x0000_0000 - 0x001F_FFFF (2MB, device region)
    let l2_device_desc = ((0u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                         (1 << 10) |  // AF
                         (3 << 8) |   // SH
                         (0 << 6) |   // AP
                         (1 << 2) |   // Device memory
                         0b01;        // Block
    (*l2_table).entries[0].value = l2_device_desc;

    // Entry 1: Map 0x4000_0000 - 0x401F_FFFF (2MB, kernel region)
    let l2_normal_desc = ((0x4000_0000u64 >> 21) & 0x3FFFF_FFFF) << 21 |
                         (1 << 10) |  // AF
                         (3 << 8) |   // SH
                         (0 << 6) |   // AP
                         (0 << 2) |   // Normal memory
                         0b01;        // Block
    (*l2_table).entries[1].value = l2_normal_desc;

    // Data synchronization barrier
    asm!("dsb ish", options(nomem, nostack));
}
```

### 8.2 MMU Register Initialization

```rust
unsafe fn init_mmu_registers() {
    // Get level 2 page table physical address
    let l2_table_addr = &raw mut LEVEL2_PAGE_TABLE.table as u64;

    // Set MAIR_EL1
    let mair: u64 = (0x00 << 8) |  // Device nGnRnE
                    (0xFF << 0);   // Normal WB-RWA
    asm!("msr mair_el1, {}", in(reg) mair, options(nomem, nostack));

    // Set TTBR0_EL1
    asm!("msr ttbr0_el1, {}", in(reg) l2_table_addr, options(nomem, nostack));

    // Set TCR_EL1
    let tcr: u64 = (25 << 0) |     // T0SZ: 39-bit VA
                   (0b01 << 8) |   // IRGN0
                   (0b01 << 10) |  // ORGN0
                   (0b11 << 12) |  // SH0
                   (0b00 << 14) |  // TG0: 4KB
                   (1 << 23);      // EPD1
    asm!("msr tcr_el1, {}", in(reg) tcr, options(nomem, nostack));

    // Flush caches and TLB
    asm!("ic iallu", options(nomem, nostack);
    asm!("dsb ish", options(nomem, nostack));
    asm!("isb", options(nomem, nostack));
    asm!("tlbi vmalle1is", options(nomem, nostack));
    asm!("dsb ish", options(nomem, nostack));
    asm!("isb", options(nomem, nostack));

    // Enable MMU
    let sctlr: u64 = 1 << 0;  // M: MMU enable
    asm!("msr sctlr_el1, {}", in(reg) sctlr, options(nomem, nostack));
    asm!("isb", options(nomem, nostack));
}
```

---

**Document Version**: 1.0
**Last Updated**: 2025-02-04
**Status**: MMU Successfully Enabled
