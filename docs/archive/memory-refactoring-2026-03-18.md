# Rux Memory Subsystem Refactoring Experience Summary

**Date**: 2026-03-18
**Authors**: Claude + William
**Related Commits**: `8b10fbc`, `3234d82`, `f2b0bcd`, `4c3543d`, `a2d0fed`, `0c2a206`, `Page Table Allocation Refactoring`

---

## 1. Refactoring Goals

Migrate Rux kernel's memory management from a "custom design" to "fully following Linux implementation", ensuring:
- 100% Linux ABI compatibility
- Correct Sv39 virtual memory layout
- Dynamic memory mapping (based on actual physical memory size)
- Linux-style three-stage page table allocation

---

## 2. Core Problems Encountered

### Problem 0: Page Table Allocation Not Following Linux Design

**Symptom**:
Original implementation used a huge static array for page table allocation:
```rust
// Original implementation - not following Linux design
const MAX_KERNEL_PAGE_TABLES: usize = 4096;  // 16MB static memory!
static mut KERNEL_PAGE_TABLES: [PageTable; 4096] = [...];
```

**Problem Analysis**:
1. 16MB static memory waste, regardless of actual need
2. No distinction between early boot and normal operation phases
3. Not utilizing memblock for dynamic allocation

**Linux's Approach**:
Linux uses a three-stage page table allocation strategy:
1. **Early Stage**: Uses small static page tables (`early_pmd`, `early_pte`)
2. **Fixmap Stage**: Uses memblock for dynamic allocation
3. **Late Stage**: Uses buddy allocator

**Solution**:

```rust
// 1. Small static page tables (only for early boot)
const NUM_EARLY_PMD: usize = 4;   // L1 page tables
const NUM_EARLY_PTE: usize = 48;  // L0 page tables (covers 96MB)

static mut EARLY_PMD: [PageTable; NUM_EARLY_PMD] = [...];
static mut EARLY_PTE: [PageTable; NUM_EARLY_PTE] = [...];

// 2. Three-stage allocation logic
enum AllocStage {
    Early,   // Static arrays + identity mapping
    Fixmap,  // memblock allocation + linear mapping
    Late,    // buddy allocator + linear mapping
}

unsafe fn alloc_page_table() -> Option<u64> {
    match get_alloc_stage() {
        AllocStage::Early => {
            // Allocate from static arrays
            // Access via identity mapping
        }
        AllocStage::Fixmap => {
            // Allocate from memblock
            let phys_addr = memblock_phys_alloc()?;
            // Use identity mapping for kernel region, linear mapping for others
        }
        AllocStage::Late => {
            // Allocate from buddy allocator
            // Access via linear mapping
        }
    }
}
```

**Key Points**:
1. Early stage only maps essential regions (kernel + heap + slab), approximately 62MB
2. Fixmap stage uses memblock, can handle large memory linear mapping
3. Late stage uses buddy allocator, fully dynamic

---

### Problem 0.1: Insufficient memblock Reserved Regions Causing Allocation Failure

**Symptom**:
```
memblock_phys_alloc: reserve failed for 0x82e1d000
PANIC! map_page: failed to allocate L0 page table
```

**Root Cause Analysis**:
1. Original `MAX_MEMBLOCK_REGIONS = 32` was too small
2. Each 4KB page allocation created a separate reserved region
3. Reserved regions didn't merge adjacent entries

**Solution**:

1. Increase `MAX_MEMBLOCK_REGIONS`:
```rust
const MAX_MEMBLOCK_REGIONS: usize = 128;  // Was 32
```

2. Add merge logic in `add_reserved()`:
```rust
pub fn add_reserved(&mut self, base: usize, size: usize, flags: MemBlockFlags) -> Result<(), ()> {
    let new_end = base + size;

    // Check for adjacent or overlapping regions, merge them
    for i in 0..self.cnt {
        let region = &mut self.regions[i];
        let region_end = region.base + region.size;

        if base <= region_end && new_end >= region.base {
            // Merge: extend existing region
            let merged_base = base.min(region.base);
            let merged_end = new_end.max(region_end);
            region.base = merged_base;
            region.size = merged_end - merged_base;
            return Ok(());
        }
    }
    // No adjacent region, add new entry
    ...
}
```

---

### Problem 0.2: Incorrect Memory Reservation Timing

**Symptom**:
```
mm: fixmap allocated outside identity region: 0x80000000
trap: Kernel panic - page fault at 0xffffffd800000000
```

**Root Cause Analysis**:
Memory reservation happened AFTER fixmap stage started:
```rust
// Wrong order
pt_ops_set_fixmap();           // Switch to fixmap
setup_device_mappings();       // Allocate page tables (memblock not reserved)
memblock_reserve(...);         // Reserved too late!
```

**Solution**:
Complete all reservations BEFORE switching to fixmap stage:
```rust
// Correct order
memblock_reserve(0x80000000, 0xA00000);     // Reserve OpenSBI + kernel
memblock_reserve(heap_start, heap_size);     // Reserve heap
memblock_reserve(slab_start, slab_size);     // Reserve slab
pt_ops_set_fixmap();                         // Then switch
setup_device_mappings();                     // Now safe to allocate
```

---

### Problem 1: Invalid VMEMMAP_START Address

**Symptom**:
```
trap: Kernel panic - page fault at 0xffffffb800000000
```

**Root Cause Analysis**:
Initial implementation set `VMEMMAP_SIZE` to 64GB, resulting in:
```
VMEMMAP_START = VMALLOC_START - 64GB
            = 0xffffffc800000000 - 0x1000000000
            = 0xffffffb800000000
```

Check bit 38 of this address:
```
0xffffffb800000000
bit 38 = 0  ← This is a user space address!
```

**Sv39 Specification**:
- Valid kernel addresses must have bit 38 = 1
- Addresses with bit 38 = 0 are user space addresses

**Solution**:
Calculate `VMEMMAP_SIZE` using Linux formula:
```c
// Linux: arch/riscv/include/asm/pgtable.h
#define VMEMMAP_SHIFT \
    (VA_BITS - PAGE_SHIFT - 1 + STRUCT_PAGE_MAX_SHIFT)
#define VMEMMAP_SIZE BIT(VMEMMAP_SHIFT)

// For Sv39:
// VMEMMAP_SHIFT = 39 - 12 - 1 + 6 = 32
// VMEMMAP_SIZE = BIT(32) = 4GB
```

After correction:
```
VMEMMAP_START = 0xffffffc800000000 - 4GB
            = 0xffffffc700000000
bit 38 = 1  ← Valid kernel address
```

---

### Problem 2: Page Table Access Using Wrong Virtual Address

**Symptom**:
```
trap: Unknown exception: LoadAccessFault, badaddr=0xffffffd800350008
```

**Root Cause Analysis**:
In `alloc_page_table()` function, after dynamically allocating page table, physical address was used directly:
```rust
// Wrong code
let phys_addr = frame.start_address().as_usize() as u64;
core::ptr::write_bytes(phys_addr as *mut u8, 0, PAGE_SIZE);
```

But MMU was already enabled, physical addresses cannot be accessed directly.

**Solution**:
Distinguish between two cases:
1. **Early boot** (frame allocator not ready): Use static page tables, access via identity mapping
2. **Normal operation** (frame allocator ready): Use dynamic allocation, access via `phys_to_virt()`

```rust
unsafe fn alloc_page_table() -> Option<u64> {
    if is_frame_allocator_ready() {
        // Dynamic allocation
        let phys_addr = alloc_kernel_page()?;
        let virt_addr = phys_to_virt(PhysAddr::new(phys_addr));
        core::ptr::write_bytes(virt_addr.bits() as *mut u8, 0, PAGE_SIZE);
        Some(phys_addr)
    } else {
        // Static allocation, identity mapping
        let idx = KERNEL_PT_NEXT.fetch_add(1, ...);
        Some(&KERNEL_PAGE_TABLES[idx] as *const PageTable as u64)
    }
}

unsafe fn get_page_table_virt(phys_addr: u64) -> *mut PageTable {
    if is_frame_allocator_ready() {
        phys_to_virt(PhysAddr::new(phys_addr)).bits() as *mut PageTable
    } else {
        phys_addr as *mut PageTable  // Identity mapping
    }
}
```

---

### Problem 3: Insufficient Static Page Table Count

**Symptom**:
Panic triggered when mapping 8192 vmemmap pages.

**Root Cause Analysis**:
- `MAX_KERNEL_PAGE_TABLES = 256`
- Each 4KB page requires L1 and L0 two-level page tables
- 8192 pages may need > 256 page tables

**Solution**:
```rust
const MAX_KERNEL_PAGE_TABLES: usize = 4096;  // 16MB
```

---

### Problem 4: VirtAddr Sign Extension Error

**Symptom**:
```
PANIC! attempt to add with overflow
  Location: kernel/src/arch/riscv64/mm/base.rs:1745
```

**Root Cause Analysis**:
Original implementation used `VA_MASK` to truncate address:
```rust
// Wrong implementation
pub const fn new(addr: u64) -> Self {
    Self(addr & VA_MASK)  // VA_MASK = 0x7FFFFFFFFF
}
```

This corrupts high bits (bit 63-39) of kernel addresses.

**Solution**:
Correct Sv39 sign extension:
```rust
pub const fn new(addr: u64) -> Self {
    let bit38 = (addr >> 38) & 1;
    if bit38 == 1 {
        // Kernel address: extend bit 38 to high bits
        Self(addr | 0xFFFFFFC0_00000000)
    } else {
        // User address: clear high bits
        Self(addr & 0x0000007F_FFFFFFFF)
    }
}
```

---

### Problem 5: vmemmap Initialization Timing Issue

**Symptom**:
Accessing vmemmap region before TLB flush causes page fault.

**Root Cause Analysis**:
```rust
// Wrong order
let val = core::ptr::read_volatile(test_ptr);  // Access
core::arch::asm!("sfence.vma zero, zero");     // Flush

// TLB doesn't have new mapping yet, access fails!
```

**Solution**:
Ensure TLB flush before access:
```rust
// Correct order
core::arch::asm!("sfence.vma zero, zero");     // Flush first
let val = core::ptr::read_volatile(test_ptr);  // Then access
```

---

## 3. Correct Sv39 Memory Layout

### Virtual Address Space Division

```
Sv39 Address Space (39-bit virtual addresses):

User Space (bit 38 = 0):
0x00000000_00000000 - 0x0000003F_FFFFFFFF  (256GB)

Kernel Space (bit 38 = 1):
0xFFFFFFC0_00000000 - 0xFFFFFFFF_FFFFFFFF  (256GB)

Kernel Space Detail:
┌─────────────────────────────────────────┐ 0xFFFFFFFF_FFFFFFFF
│                                         │
│              (unused)                   │
│                                         │
├─────────────────────────────────────────┤ 0xFFFFFFD8_00000000
│         PAGE_OFFSET (linear mapping)    │
│         phys_to_virt(phys)              │
├─────────────────────────────────────────┤ 0xFFFFFFD0_00000000
│         VMALLOC_END                     │
├─────────────────────────────────────────┤
│         VMALLOC region (64GB)           │
├─────────────────────────────────────────┤ 0xFFFFFFC8_00000000
│         VMALLOC_START                   │
├─────────────────────────────────────────┤ 0xFFFFFFC8_00000000
│         VMEMMAP_END                     │
├─────────────────────────────────────────┤
│         VMEMMAP region (4GB)            │
│         pfn_to_page(pfn)                │
├─────────────────────────────────────────┤ 0xFFFFFFC7_00000000
│         VMEMMAP_START                   │
├─────────────────────────────────────────┤
│              (other regions)            │
└─────────────────────────────────────────┘ 0xFFFFFFC0_00000000
```

### Key Constant Definitions

```rust
// Following Linux definitions
pub const PAGE_OFFSET: usize = 0xffffffd800000000;
pub const KERN_VIRT_SIZE: usize = 128 * 1024 * 1024 * 1024;  // 128GB
pub const VMALLOC_SIZE: usize = 64 * 1024 * 1024 * 1024;     // 64GB
pub const VMEMMAP_SIZE: usize = 4 * 1024 * 1024 * 1024;      // 4GB

// Linux formula
pub const VMALLOC_END: usize = PAGE_OFFSET;
pub const VMALLOC_START: usize = PAGE_OFFSET - VMALLOC_SIZE;
pub const VMEMMAP_END: usize = VMALLOC_START;
pub const VMEMMAP_START: usize = VMALLOC_START - VMEMMAP_SIZE;
```

---

## 4. Key Lessons Learned

### 1. Don't Invent Your Own Solution

❌ **Wrong Approach**: Design your own memory layout, pick "reasonable-looking" values
```rust
// Self-designed approach
pub const VMEMMAP_SIZE: usize = 64 * 1024 * 1024 * 1024;  // Why 64GB? Because "seems enough"
```

✅ **Correct Approach**: Strictly follow Linux formula calculations
```rust
// Linux formula
pub const VMEMMAP_SIZE: usize = 1 << (39 - 12 - 1 + 6);  // = 4GB
```

### 2. Understand Hardware Specifications

Sv39 is not just "39-bit address space", it has constraints:
- bit 38 determines kernel vs user space
- Must perform correct sign extension
- Non-compliant addresses cause page faults

### 3. Pay Attention to Boot Phase Address Translation

After MMU is enabled, all memory access must use virtual addresses:
- Early (frame allocator not ready): Identity mapping
- Late (frame allocator ready): Linear mapping

### 4. TLB Coherency

TLB must be flushed after modifying page tables:
```rust
// After adding new page table entry
core::arch::asm!("sfence.vma zero, zero");

// Must flush before accessing new mapping!
```

### 5. Debugging Tips

1. **Print VPN indices**: Quickly locate which page table an address belongs to
2. **Check bit 38**: Verify if it's a valid kernel address
3. **Print ROOT_PAGE_TABLE address**: Confirm page table base is correct

---

## 5. References

1. **Linux Source Code**:
   - `arch/riscv/include/asm/page.h` - PAGE_OFFSET definition
   - `arch/riscv/include/asm/pgtable.h` - Virtual memory layout
   - `arch/riscv/mm/init.c` - Memory initialization

2. **RISC-V Specification**:
   - RISC-V Privileged Architecture - Sv39 page table format
   - bit 38 determines address space

3. **Project Files**:
   - `kernel/src/arch/riscv64/mm/base.rs` - Core memory management
   - `kernel/src/mm/vmemmap.rs` - vmemmap implementation
   - `kernel/src/mm/page_desc.rs` - Page descriptors

---

## 6. Final Results

After refactoring, the kernel can:
- ✅ Correctly establish linear mapping (based on actual physical memory size)
- ✅ Correctly establish vmemmap mapping
- ✅ Linux-style three-stage page table allocation
- ✅ Dynamic memblock allocation (supports large memory)
- ✅ Successfully boot and load shell

```
mm:               Sv39 3-level page table            [ok]
mm:               device mappings                    [ok]
mm:               linear mapping 2048 MB             [ok]
mm:               vmemmap mapping initialized        [ok]
memblock:         total 2048MB, available 2038MB     [ok]
...
init:             loading /bin/shell                 [ok]
```

### Memory Usage Optimization

| Item | Before Refactoring | After Refactoring |
|------|-------------------|-------------------|
| Static page tables | 4096 × 4KB = 16MB | (4+48) × 4KB = 208KB |
| Page table allocation | All static | Three-stage dynamic |
| memblock regions | 32 | 128 (with merging) |

---

## 7. Additional Lessons Learned

### 1. Reservation Must Precede Allocation

Memory management initialization order is critical:
```rust
// ✅ Correct order
1. Initialize memblock
2. Add memory regions
3. Reserve used regions (kernel, heap, slab)
4. Switch to fixmap stage
5. Begin dynamic allocation

// ❌ Wrong order
1. Initialize memblock
2. Switch to fixmap stage
3. Allocate memory (may allocate to unreserved regions!)
4. Reserve regions (too late)
```

### 2. memblock Regions Need Merging

Creating a reservation entry for each 4KB page allocation quickly exhausts space. Adjacent region merging must be implemented.

### 3. Identity Mapping Range is Limited

Early boot only maps specific regions (0x80200000 - 0x84000000), memory allocated in fixmap stage beyond this range must use linear mapping.

### 4. Debugging memblock Issues

When memblock allocation fails, printing the following information helps diagnosis:
- memory regions count and ranges
- reserved regions count and ranges
- specific address that failed allocation
