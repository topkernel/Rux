# Rux Memory Management Design Document

This document details the design and implementation of the Rux kernel memory management subsystem.

**Last Updated**: 2026-04-09
**Code Location**: `kernel/src/mm/` + `kernel/src/arch/riscv64/mm/`
**Architecture Support**: RISC-V 64-bit (RV64GC, Sv39)

---

## Table of Contents

- [Overview](#overview)
- [Virtual Memory Layout](#virtual-memory-layout)
- [Physical Memory Layout](#physical-memory-layout)
- [Boot-Time Memory Initialization](#boot-time-memory-initialization)
- [Page Descriptors (vmemmap)](#page-descriptors-vmemmap)
- [Physical Page Allocation](#physical-page-allocation)
- [Zone Allocator](#zone-allocator)
- [Per-CPU Pages (PCP)](#per-cpu-pages-pcp)
- [Kernel Heap Allocation](#kernel-heap-allocation)
- [Sv39 Page Tables](#sv39-page-tables)
- [Process Address Space](#process-address-space)
- [Copy-on-Write](#copy-on-write)
- [Page Fault Handling](#page-fault-handling)
- [ASID Management](#asid-management)
- [Memory Statistics](#memory-statistics)
- [API Reference](#api-reference)

---

## Overview

### Design Goals

1. **Linux Compatible**: ABI compatible with Linux kernel, uses Linux Sv39 virtual memory layout
2. **Linux-style Three-Stage Boot**: memblock -> fixmap -> buddy allocator
3. **SMP Safe**: Atomic operations, per-CPU caches, proper locking
4. **Efficient COW**: fork() shares pages, copies on write
5. **Demand Paging**: Anonymous pages allocated on first access

### Architecture Layers

```
+-------------------------------------------------------------+
|                     User Space System Calls                 |
|   brk() / mmap() / munmap() / mprotect() / ...             |
+-------------------------------------------------------------+
                              |
                              v
+-------------------------------------------------------------+
|                   Process Address Space Management          |
|   MmStruct / VmaManager / VMA                              |
+-------------------------------------------------------------+
                              |
                              v
+-------------------------------------------------------------+
|                     Virtual Memory Management               |
|   Page Tables (Sv39) / COW / Page Fault                    |
+-------------------------------------------------------------+
                              |
                              v
+-------------------------------------------------------------+
|                     Physical Memory Management              |
|   memblock -> Zone/Buddy -> Page Descriptors (vmemmap)     |
+-------------------------------------------------------------+
                              |
                              v
+-------------------------------------------------------------+
|                     Kernel Heap Allocator                   |
|   Buddy Allocator -> Slab Allocator -> kmalloc/kfree       |
|   Per-CPU Pages (PCP)                                      |
+-------------------------------------------------------------+
```

### Module Composition

| Module | File | Function |
|--------|------|----------|
| **Top-level** | mm/mod.rs | Re-exports, constants |
| **Address Types** | mm/page.rs | VirtAddr, PhysAddr (independent) |
| **Page Descriptors** | mm/page_desc.rs | Per-page metadata (struct Page) |
| **Page Allocation** | mm/page_alloc.rs | Buddy allocator, alloc_pages/free_pages |
| **Zone Allocator** | mm/zone.rs | Zone management, embedded buddy |
| **NUMA pglist** | mm/pglist.rs | NUMA node management |
| **Memblock** | mm/memblock.rs | Early boot memory allocator |
| **Memory Layout** | mm/layout.rs | Physical memory region layout |
| **vmemmap** | mm/vmemmap.rs | Virtual page descriptor mapping |
| **VMA** | mm/vma.rs | Virtual Memory Area management |
| **MmStruct** | mm/mm_struct.rs | Process address space descriptor |
| **PageMap** | mm/pagemap.rs | Perm, MapError, PageTableType |
| **Slab** | mm/slab.rs | Small object allocator |
| **Buddy (standalone)** | mm/buddy_allocator.rs | Standalone buddy for kernel heap |
| **PCP** | mm/pcp.rs | Per-CPU page cache |
| **MemInfo** | mm/meminfo.rs | /proc/meminfo |
| **RMAP** | mm/rmap.rs | Reverse mapping |
| **HugePage** | mm/hugepage.rs | Huge page support |
| **Arch MMU** | arch/riscv64/mm/mod.rs | Arch-specific module hub |
| **Memory Layout** | arch/riscv64/mm/memory_layout.rs | Sv39 constants, address types |
| **Page Tables** | arch/riscv64/mm/pagetable.rs | PTE, PageTable, Satp |
| **MMU Init** | arch/riscv64/mm/mmu_init.rs | Page table setup, mapping primitives |
| **MM Ops** | arch/riscv64/mm/mm_ops.rs | COW, mmap, fork, user address space |
| **Page Fault** | arch/riscv64/mm/page_fault.rs | Demand paging, stack expansion |
| **Exception** | arch/riscv64/mm/exception.rs | do_page_fault, exception table |
| **Fixmap** | arch/riscv64/mm/fixmap.rs | Early device mappings |
| **ASID** | arch/riscv64/mm/asid.rs | Address Space ID management |

---

## Virtual Memory Layout

### Sv39 Address Space (Linux-Compatible)

RISC-V Sv39 uses 39-bit virtual addresses with sign extension:

```
User Space (256GB):
0x0000_0000_0000_0000 ───────────────────── 0x0000_3FFF_FFFF_FFFF
  VPN2[0..255]

Kernel Space (256GB):
0xFFFF_C000_0000_0000 ───────────────────── 0xFFFF_FFFF_FFFF_FFFF
  VPN2[256..511]
```

### Kernel Virtual Memory Layout

```
High Address
0xFFFFFFFF_FFFFFFFF +-----------------------+
                    |                       |
0xFFFFFFFF_80000000 | Kernel Image Mapping  |  VPN2[510]
                    | (text/data/bss)       |  KERNEL_LINK_ADDR
                    | Linked at VA, loaded  |
                    | at PA 0x80200000      |
0xFFFFFFFF_7FFFFFFF +-----------------------+
                    | (unmapped)            |
                    +-----------------------+
                    | VMALLOC (64GB)        |  VMALLOC_START
                    |                       |  = PAGE_OFFSET - 64GB
                    +-----------------------+
                    | vmemmap (4GB)         |  VMEMMAP_START
                    | (page descriptors)    |  = VMALLOC_START - 4GB
                    | Each 4KB page holds   |
                    | 64 Page descriptors   |
0xFFFFFFD6_00000000 +-----------------------+
                    | Linear Mapping        |  PAGE_OFFSET
                    | phys + VA_PA_OFFSET   |  Dynamically sized
                    | Maps ALL physical    |  Based on actual RAM
                    | memory                |
                    +-----------------------+
                    | (user/kernel boundary)|
0xFFFF_C000_0000_0000 +-----------------------+
Low Address
```

### Key Constants

```rust
// Page size
pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_SHIFT: u64 = 12;

// Virtual address bits
pub const VA_BITS: u64 = 39;
pub const TASK_SIZE: usize = 256 * 1024 * 1024 * 1024;  // 256GB

// Kernel mapping
pub const KERNEL_LINK_ADDR: usize = 0xFFFFFFFF80000000;  // VPN2[510]
pub const KERNEL_ENTRY: u64 = 0x80200000;                 // Physical load address

// Linear mapping (Linux Sv39)
pub const PAGE_OFFSET: usize = 0xFFFFFFD600000000;
pub const VA_PA_OFFSET: usize = PAGE_OFFSET - PHYS_MEMORY_BASE;
// phys_to_virt(phys) = phys + VA_PA_OFFSET

// VMALLOC
pub const VMALLOC_SIZE: usize = 64 * 1024 * 1024 * 1024;  // 64GB
pub const VMALLOC_START: usize = PAGE_OFFSET - VMALLOC_SIZE;
pub const VMALLOC_END: usize = PAGE_OFFSET;

// vmemmap
pub const VMEMMAP_SIZE: usize = 4 * 1024 * 1024 * 1024;   // 4GB
pub const VMEMMAP_START: usize = VMALLOC_START - VMEMMAP_SIZE;
pub const VMEMMAP_END: usize = VMALLOC_START;

// PGD layout
pub const USER_PTRS_PER_PGD: usize = 256;   // VPN2[0..255] = user space
pub const KERNEL_PGD_START: usize = 256;     // VPN2[256..511] = kernel space
```

### User Space Layout

```
0x0000_0000           +-------------------+
                      | Null page guard   |  (unmapped, catches NULL deref)
0x0000_1000           +-------------------+
                      | ELF segments      |  .text, .rodata, .data, .bss
                      | (code, data, BSS) |
0x2000_0000           +-------------------+
                      | Heap (brk)        |  BRK_DEFAULT = 512MB
                      | Grows upward      |  max = TASK_SIZE/3
TASK_SIZE/3           +-------------------+
                      | (guard region)    |
MMAP_START           +-------------------+
                      | mmap area         |  Top-down allocation
                      |                   |  TASK_SIZE - 64GB
TASK_SIZE - 8MB      +-------------------+
                      | User stack        |  Grows downward
                      | Max 8MB           |
TASK_SIZE - PAGE_SIZE +-------------------+
```

### Address Conversion

```rust
// Linear mapping (for physical memory access)
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    VirtAddr::new(phys.0 + KERNEL_MAP.va_pa_offset)
}
pub fn virt_to_phys(virt: VirtAddr) -> PhysAddr {
    PhysAddr::new(virt.0 - KERNEL_MAP.va_pa_offset)
}

// Kernel mapping (for kernel text/data at KERNEL_LINK_ADDR)
// phys = virt - va_kernel_pa_offset
// va_kernel_pa_offset = KERNEL_LINK_ADDR - KERNEL_PHYS
```

---

## Physical Memory Layout

```
0x8000_0000 +-----------------------------+
            |     OpenSBI (128KB)         |
0x8002_0000 +-----------------------------+
            |     Kernel (text/data/bss)  |
            |     Linked at KERNEL_LINK_ADDR |
            |     ~8MB                     |
0x80A0_0000 +-----------------------------+
            |     Kernel Heap             |
            |     Configurable (default 32MB) |
0x82A0_0000 +-----------------------------+
            |     Slab Allocator (4MB)    |
0x82E0_0000 +-----------------------------+
            |     Available physical memory |
            |     Managed by zone/buddy allocator |
            |     ~2GB (depends on QEMU -m)  |
0xA000_0000 +-----------------------------+
```

### KernelMemoryLayout

Runtime-computed layout (not hardcoded):

```rust
pub struct KernelMemoryLayout {
    pub phys_memory_base: usize,   // 0x80000000
    pub phys_memory_size: usize,   // Total physical memory
    pub kernel_start: usize,       // 0x80200000
    pub kernel_end: usize,         // kernel_start + kernel_size
    pub heap_start: usize,         // 0x80A00000
    pub heap_end: usize,           // heap_start + heap_size
    pub slab_start: usize,         // heap_end
    pub slab_end: usize,           // slab_start + slab_size
    pub frame_alloc_start: usize,  // slab_end
    pub frame_alloc_size: usize,   // rest of physical memory
}
```

---

## Boot-Time Memory Initialization

### Three-Stage Page Table Allocation

Linux-style three-stage strategy for allocating page table pages:

| Stage | Allocator | VA Access | When |
|-------|-----------|-----------|------|
| **Early** | Static BSS arrays | `virt = phys + va_kernel_pa_offset` | Before linear mapping |
| **Fixmap** | memblock | `virt = phys + va_pa_offset` | After linear mapping, before buddy |
| **Late** | Zone buddy | `virt = phys + va_pa_offset` | After zone allocator init |

### Early Stage

Static arrays in `.bss` at `KERNEL_LINK_ADDR`:

```rust
#[link_section = ".bss"]
static mut EARLY_PMD: [PageTable; 8] = ...;    // 8 L1 tables
#[link_section = ".bss"]
static mut EARLY_PTE: [PageTable; 128] = ...;  // 128 L0 tables
```

Physical address: `phys = virt - va_kernel_pa_offset`

### Fixmap Stage

After `setup_linear_mapping()`, uses memblock for allocation. All physical memory is accessible via linear mapping.

### Late Stage

After `init_zone_system()`, uses the zone/buddy allocator.

### Stage Transitions

```rust
// In rust_main():
arch::riscv64::mm::setup_linear_mapping(&memory_regions);
arch::riscv64::mm::pt_ops_set_fixmap();   // Early -> Fixmap

// After zone allocator:
arch::riscv64::mm::pt_ops_set_late();     // Fixmap -> Late
```

### Complete Initialization Sequence

```
1. boot.S:     MMU trampoline -> early_pg_dir (identity + kernel VA)
2. rust_main:  arch::mm::init()
                - Create ROOT_PAGE_TABLE
                - Map kernel at KERNEL_LINK_ADDR (2MB huge pages)
                - Map UART identity mapping
                - Map DTB at linear mapping address
                - Switch to ROOT_PAGE_TABLE
3. rust_main:  Set KERNEL_MAP.va_pa_offset
4. rust_main:  memblock_init()
5. rust_main:  Parse DTB memory regions
6. rust_main:  memblock_reserve() for kernel, heap, slab
7. rust_main:  setup_linear_mapping()  -- phys + VA_PA_OFFSET at PAGE_OFFSET
8. rust_main:  pt_ops_set_fixmap()
9. rust_main:  init_heap() + init_slab()
10. rust_main: init_vmemmap()          -- map page descriptors
11. rust_main: init_page_descriptors()  -- zero all Page structs
12. rust_main: init_zone_system()      -- ZoneNormal from memblock free ranges
13. rust_main:  pt_ops_set_late()
14. rust_main:  setup_device_mappings() -- VirtIO, PLIC, CLINT, PCIe
```

---

## Page Descriptors (vmemmap)

**File**: `kernel/src/mm/page_desc.rs`, `kernel/src/mm/vmemmap.rs`

### Design

Linux-style vmemmap: page descriptors are mapped into a dedicated virtual address region instead of using a large static array.

```
VMEMMAP_START + (pfn - base_pfn) * sizeof(Page)  =  Page descriptor virtual address
```

Each 4KB physical page backing vmemmap holds `4096 / 64 = 64` page descriptors.

### Page Structure (64 bytes, cache-line aligned)

```rust
#[repr(C, align(64))]
pub struct Page {
    flags: PageFlags,       //  4 bytes - atomic flags
    _mapcount: AtomicI32,   //  4 bytes - PTE map count (-1 = unmapped)
    _refcount: AtomicI32,   //  4 bytes - reference count (0 = free)
    private: AtomicUsize,   //  8 bytes - buddy order / slab data
    mapping: AtomicUsize,   //  8 bytes - address_space pointer
    index: AtomicUsize,     //  8 bytes - offset in mapping
    _type: AtomicU32,       //  4 bytes - page type
    _reserved: AtomicU32,   //  4 bytes
    next_free: AtomicUsize, //  8 bytes - buddy free list pointer
    // padding to 64 bytes
}
```

### Page Flags

| Flag | Description |
|------|-------------|
| `Locked` | Page is locked |
| `Writeback` | Page writeback in progress |
| `Referenced` | Page has been accessed |
| `UpToDate` | Page data is valid |
| `Dirty` | Page is modified |
| `Lru` | In LRU list |
| `Head` | Compound page head |
| `Waiters` | Tasks waiting on this page |
| `Active` | On active LRU list |
| `Reserved` | Reserved page |
| `Private` | Private data |
| `Reclaim` | Reclaimable |
| `SwapBacked` | Backed by swap |
| `Unevictable` | Cannot be evicted |
| **`Cow`** | Copy-on-write page |
| **`Anonymous`** | Anonymous page |

### Page Types

```rust
pub enum PageType {
    Normal,      // Normal page
    Buddy,       // Buddy allocator free page
    Slab,        // Slab allocator page
    PageCache,   // Page cache
    Anonymous,   // Anonymous page
}
```

### Reference Counting

```rust
impl Page {
    pub fn get_page(&self) -> i32 { ... }   // Increment refcount, return new value
    pub fn put_page(&self) -> i32 { ... }   // Decrement refcount, return new value
    pub fn refcount(&self) -> i32 { ... }   // Read refcount
    pub fn is_free(&self) -> bool { ... }   // refcount == 0
}
```

**Critical for COW**: On fork, `get_page()` is called for ALL shared user pages. On process exit, `put_page()` is called, and the page is only freed when refcount reaches 0.

### PFN Conversion (O(1) via vmemmap)

```rust
pub fn pfn_to_page(pfn: usize) -> *const Page;
pub fn pfn_to_page_mut(pfn: usize) -> *mut Page;
pub fn page_to_pfn(page: &Page) -> usize;
pub fn pfn_valid(pfn: usize) -> bool;
pub fn phys_valid(phys: usize) -> bool;
```

---

## Physical Page Allocation

**File**: `kernel/src/mm/page_alloc.rs`

### Allocation Hierarchy

```
alloc_pages(gfp_flags, order)
    |
    +-- Zone system available? --> Zone::alloc_pages(order)
    |                               |
    |                               +-- Buddy split if needed
    |
    +-- Zone not ready? -----------> memblock_phys_alloc() (early boot)
```

### Core API

```rust
// Allocate 2^order contiguous pages (returns physical address, 0 on failure)
pub fn alloc_pages(gfp_flags: u32, order: usize) -> usize;

// Single page convenience
pub fn alloc_page(gfp_flags: u32) -> usize;

// Allocate + zero
pub fn get_zeroed_page(gfp_flags: u32) -> usize;

// Free pages
pub fn free_pages(addr: usize, order: usize);
pub fn free_page(addr: usize);

// Linux-compatible aliases
pub fn __get_free_pages(gfp_flags: u32, order: usize) -> usize;
pub fn __get_free_page(gfp_flags: u32) -> usize;
```

### GFP Flags

| Flag | Value | Description |
|------|-------|-------------|
| `GFP_KERNEL` | 0x01 | Normal kernel allocation, can sleep |
| `GFP_USER` | 0x02 | User page allocation |
| `GFP_ATOMIC` | 0x04 | Atomic allocation, cannot sleep |
| `GFP_DMA` | 0x08 | DMA-capable memory |
| `__GFP_ZERO` | 0x100 | Return zeroed page |

---

## Zone Allocator

**File**: `kernel/src/mm/zone.rs`

### Linux Zone Model

Physical memory is partitioned into zones. On RISC-V QEMU, only `ZoneNormal` is used:

```rust
pub enum ZoneType {
    ZoneDma,      // DMA zone (low 16MB on x86)
    ZoneDma32,    // DMA32 zone (low 4GB on x86-64)
    ZoneNormal,   // Normal zone (all remaining memory)
    ZoneMovable,  // Movable zone
}
```

### Zone Structure

```rust
pub struct Zone {
    zone_type: ZoneType,
    id: usize,
    node_id: usize,
    start_pfn: usize,
    end_pfn: usize,
    free_area: [FreeArea; MAX_ORDER + 1],  // Per-order free lists
    initialized: AtomicBool,
    lock: Mutex<()>,
}

pub struct FreeArea {
    free_list: AtomicUsize,   // Head of free list (PFN or FREE_LIST_NULL)
    count: AtomicUsize,       // Number of free blocks at this order
}
```

### Buddy Algorithm

**Constants**: `MAX_ORDER = 10` (max allocation: 2^10 = 1024 pages = 4MB)

**Free list**: Implemented as a singly-linked list using `Page.next_free` in page descriptors.

**Allocation**:
1. Search from requested order up to MAX_ORDER
2. If found, remove from free list
3. If not found at requested order, split from higher order:
   - Split block into two buddies
   - Add one buddy to free list of order-1
   - Repeat until reaching requested order

**Deallocation**:
1. Find buddy: `buddy_pfn = pfn ^ (1 << order)`
2. If buddy is free and same order, coalesce:
   - Remove buddy from free list
   - Merge into block of order+1
   - Repeat from step 1
3. If no coalescing possible, add to free list

### Initialization

```rust
// Called from rust_main after vmemmap and page descriptors are ready
pub fn init_zone_system(phys_start: usize, phys_size: usize, kernel_end: usize);
```

Creates ZoneNormal from memblock free ranges, adds all free pages to buddy allocator.

---

## Per-CPU Pages (PCP)

**File**: `kernel/src/mm/pcp.rs`

Per-CPU page cache to reduce global allocator lock contention.

### Structure

```rust
pub struct PerCpuPages {
    lists: [usize; MIGRATE_TYPES],   // Page lists per migration type
    counts: [usize; MIGRATE_TYPES],  // Page counts per type
    high: usize,                     // High water mark
    batch: usize,                    // Batch size
}
```

### Migration Types

| Type | Description |
|------|-------------|
| `Unmovable` | Cannot be moved (kernel use) |
| `Movable` | Can be moved (userspace pages) |
| `Reclaimable` | Can be reclaimed (swappable) |

### Allocation Flow

```
1. Try per-CPU cache (lock-free, fast)
2. If empty, batch-acquire from global zone allocator
3. If above high water mark, batch-return to global
```

---

## Kernel Heap Allocation

### Allocator Hierarchy

```
+---------------------------------------------+
|            kmalloc / kzalloc                |
|            (Public allocation interface)    |
+---------------------------------------------+
                    |
        +-----------+-----------+
        v                       v
+---------------+       +---------------+
| Slab Allocator|       | Buddy Allocator|
| (<= 4KB objects)|     | (> 4KB allocation) |
+---------------+       +---------------+
        |                       |
        +-----------+-----------+
                    v
            +---------------+
            | Zone / Buddy  |
            | Allocator     |
            +---------------+
```

### Slab Allocator

**File**: `kernel/src/mm/slab.rs`

Small object allocator for kernel data structures.

**Supported Object Sizes**: 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096 bytes

```rust
pub struct SlabCache {
    object_size: usize,
    objects_per_slab: usize,
    free_list: u16,
    partial_list: u16,
    full_list: u16,
}
```

**Public Interface**:

```rust
pub fn kmalloc(size: usize) -> *mut u8;
pub fn kfree(ptr: *mut u8);
pub fn kzalloc(size: usize) -> *mut u8;
```

### Buddy Allocator (Standalone)

**File**: `kernel/src/mm/buddy_allocator.rs`

Used for kernel heap before zone allocator is available. Manages a contiguous region starting at `HEAP_START`.

**Supported Orders**: 0-20 (4KB to 4GB)

---

## Sv39 Page Tables

**File**: `kernel/src/arch/riscv64/mm/pagetable.rs`

### Virtual Address Decomposition

```
+---------+---------+---------+------------+
|  VPN[2] |  VPN[1] |  VPN[0] |  Page offset |
|  9 bits |  9 bits |  9 bits |  12 bits   |
+---------+---------+---------+------------+
   L2 (PGD)  L1 (PMD)  L0 (PTE)
```

### Page Table Entry Format

```
+------+---+---+---+---+---+---+---+-----+------------------+
| PPN  |RSW| D | A | G | U | X | W | R | V |
|44 bits|2  | 1 | 1 | 1 | 1 | 1 | 1 | 1 | 1 |
+------+---+---+---+---+---+---+---+-----+------------------+
  [53:10]  [9:8]  7   6   5   4   3   2   1   0
```

**Bit definitions**:

| Bit | Name | Description |
|-----|------|-------------|
| 0 | V | Valid |
| 1 | R | Readable |
| 2 | W | Writable |
| 3 | X | Executable |
| 4 | U | User accessible |
| 5 | G | Global mapping |
| 6 | A | Accessed |
| 7 | D | Dirty |
| 8 | **COW** | Rux: Copy-on-write (software-defined) |
| 9:8 | RSW | Reserved for software |
| 53:10 | PPN | Physical Page Number |
| 62:61 | SVPBMT | Memory type (00=Normal, 01=NC, 10=IO) |

### Satp Register

```
+------+------+------------------+
| MODE | ASID |       PPN       |
| 4bits|16bits|     44 bits     |
+------+------+------------------+
  [63:60] [59:44]  [43:0]
```

- `MODE = 8`: Sv39 mode
- `ASID`: Address Space ID (for TLB tagging)
- `PPN`: Root page table physical page number

### PageTableEntry API

```rust
impl PageTableEntry {
    pub fn new() -> Self;                          // Zero entry
    pub fn from_bits(bits: u64) -> Self;
    pub fn bits(&self) -> u64;

    // Query
    pub fn is_valid(&self) -> bool;
    pub fn is_leaf(&self) -> bool;                 // R|W|X != 0
    pub fn is_readable(&self) -> bool;
    pub fn is_writable(&self) -> bool;
    pub fn is_executable(&self) -> bool;
    pub fn is_user(&self) -> bool;
    pub fn ppn(&self) -> u64;

    // Create
    pub fn new_table(ppn: u64) -> Self;           // Non-leaf entry
    pub fn new_page_kernel(ppn: u64) -> Self;     // Kernel page
    pub fn new_page_user(ppn: u64) -> Self;       // User page
    pub fn new_page_ro(ppn: u64) -> Self;         // Read-only page
}
```

---

## Process Address Space

### MmStruct

**File**: `kernel/src/mm/mm_struct.rs`

Process address space descriptor, corresponding to Linux `mm_struct`.

```rust
pub struct MmStruct {
    // Page table
    pgd: AtomicU64,                    // Root page table PPN
    pgd_lock: RwLock<()>,              // Page table lock
    space_type: PageTableType,         // Kernel / User

    // VMA management
    vma_manager: RwLock<VmaManager>,

    // Segment ranges
    start_code: AtomicUsize,
    end_code: AtomicUsize,
    start_data: AtomicUsize,
    end_data: AtomicUsize,

    // Heap
    start_brk: AtomicUsize,
    brk: AtomicUsize,

    // Stack
    start_stack: AtomicUsize,

    // Statistics
    total_vm: AtomicU64,
    locked_vm: AtomicU64,

    // Address space ID
    asid: AtomicU16,
}
```

### Key Operations

```rust
impl MmStruct {
    pub fn new_kernel(root_ppn: u64) -> Self;   // Kernel address space
    pub fn new_user(root_ppn: u64) -> Self;     // User address space

    pub fn enable(&self);                        // Switch to this address space
    pub fn flush_tlb(&self);
    pub fn flush_tlb_addr_page(vaddr: usize);

    // VMA management
    pub fn map_vma(&self, vma: Vma, perm: Perm);
    pub fn unmap_vma(&self, start: usize);
    pub fn mmap(&self, ...) -> usize;
    pub fn munmap(&self, addr: usize, size: usize);

    // Heap (brk syscall)
    pub fn do_brk(&self, new_brk: usize) -> usize;

    // Fork
    pub fn fork(&self) -> Result<MmStruct, MapError>;
}
```

### VMA (Virtual Memory Area)

**File**: `kernel/src/mm/vma.rs`

```rust
pub struct Vma {
    start: usize,           // Start address (page-aligned)
    end: usize,             // End address
    flags: VmaFlags,        // Permission flags
    offset: u64,            // File offset (for file-backed)
    vma_type: VmaType,      // Type
}
```

### VmaFlags

| Flag | Value | Description |
|------|-------|-------------|
| `READ` | 0x01 | Readable |
| `WRITE` | 0x02 | Writable |
| `EXEC` | 0x04 | Executable |
| `SHARED` | 0x08 | Shared mapping |
| `PRIVATE` | 0x10 | Private mapping (COW) |
| `GROWSDOWN` | 0x100 | Grows downward (stack) |
| `GROWSUP` | 0x200 | Grows upward |
| `VM_IO` | 0x400 | Memory-mapped I/O |

### VmaManager

Uses `BTreeMap<start_addr, Vma>` for O(log n) operations:

```rust
impl VmaManager {
    pub fn add(&self, vma: Vma) -> Result<(), VmaError>;
    pub fn find(&self, addr: usize) -> Option<Vma>;
    pub fn find_mut(&self, addr: usize) -> Option<&mut Vma>;
    pub fn remove(&self, start: usize);
    pub fn expand_downwards(&self, vma_start: usize, new_start: usize);
    pub fn find_stack_vma(&self, addr: usize) -> Option<&Vma>;
}
```

---

## Copy-on-Write

### Overview

When `fork()` creates a child process, physical pages are **not** copied immediately. Instead, parent and child share the same physical pages with read-only permissions. On first write, a page fault triggers a copy.

### fork() COW Flow

**File**: `kernel/src/arch/riscv64/mm/mm_ops.rs` - `copy_page_table_cow()`

```
1. Walk parent's page table (VPN2[0..255] only - user space)
2. For each valid entry:
   a. Kernel entry (U=0): Share directly (no COW)
   b. Non-leaf entry: Create new child page table, recurse
   c. User leaf entry (read-only):
      - Increment refcount by 1 (shared page)
      - Copy PTE to child (shared)
   d. User leaf entry (writable):
      - Increment refcount by 2 (once for sharing + once for COW)
      - Set Cow flag on Page descriptor
      - Clear W bit, set COW software bit in BOTH parent and child PTEs
```

### COW Write Fault Flow

**File**: `kernel/src/arch/riscv64/mm/mm_ops.rs` - `handle_cow_fault()`

```
1. Page fault occurs on COW page (W=0, COW bit set)
2. Walk page table to find PTE
3. Get Page descriptor, check refcount
4. If refcount == 1: Sole owner
   - Clear COW bit, restore W bit
   - No page copy needed
5. If refcount > 1: Shared page
   - Allocate new physical page
   - memcpy old page -> new page
   - Update PTE to point to new page with W bit set
   - Decrement old page refcount
```

### COW Flag Convention

- PTE bit 8 = COW software flag (`cow_flags::COW = 1 << 8`)
- Page descriptor flag: `PageFlag::Cow`
- On fork: `W` bit cleared, `COW` bit set in PTE
- On write fault: check `COW` bit, allocate new page, restore `W` bit

### Page Reference Counting Rules

**Critical**: The refcount determines when a page can be freed:

```rust
// On fork (copy_page_table_cow):
(*page).get_page();     // Read-only shared pages: refcount +1
(*page).get_page();     // Writable pages: refcount +1 (sharing)
(*page).get_page();     // Writable pages: refcount +1 (COW)

// On process exit (free_user_page_tables):
let new_ref = (*page).put_page();  // refcount -1
if new_ref == 0 {
    free_pages(phys_addr, 0);      // Only free when refcount == 0
}
```

---

## Page Fault Handling

**File**: `kernel/src/arch/riscv64/mm/page_fault.rs`, `kernel/src/arch/riscv64/mm/exception.rs`

### Fault Flags

```rust
pub struct FaultFlags;
impl FaultFlags {
    pub const READ: u32 = 0x01;
    pub const WRITE: u32 = 0x02;
    pub const EXEC: u32 = 0x04;
    pub const USER: u32 = 0x08;
    pub const KERNEL: u32 = 0x10;
}
```

### Fault Results

```rust
pub enum MmFaultResult {
    Handled,          // Page mapped, retry instruction
    Segfault,         // Address not in any VMA
    PermissionDenied, // Insufficient permissions
    OutOfMemory,      // Allocation failed
    AlreadyMapped,    // Mapped but wrong permissions
    CowPending,       // COW page, needs copy
}
```

### Page Fault Flow

```
Page Fault (from trap_handler)
    |
    v
exception::do_page_fault(regs, access_type)
    |
    +-- Kernel mode?
    |   +-- Stack overflow? -> panic
    |   +-- Exception table? -> fixup (EPC = fixup address)
    |   +-- Otherwise -> KernelPanic
    |
    +-- User mode?
        |
        v
    page_fault::handle_mm_fault(addr_space, fault_addr, flags)
        |
        +-- Page already mapped?
        |   +-- Write to COW page? -> return CowPending
        |   +-- Permissions OK? -> flush TLB, return Handled
        |   +-- Wrong permissions? -> return PermissionDenied
        |
        +-- Page not mapped?
            +-- Find VMA containing fault_addr
            +-- No VMA found? -> return Segfault
            +-- Check VMA permissions
            +-- Stack expansion needed? -> expand_downwards, allocate page
            +-- Normal anonymous page -> allocate zeroed page, map it
            +-- flush TLB, return Handled

    Back in exception::do_page_fault:
    +-- CowPending? -> handle_cow_fault() -> Handled
    +-- Segfault? -> send SIGSEGV to process
    +-- PermissionDenied? -> send SIGSEGV to process
    +-- OutOfMemory? -> send SIGKILL to process
    +-- KernelPanic? -> halt (debug build) or loop
```

### Stack Expansion

When a page fault occurs near the stack VMA (within expansion range):

1. Find stack VMA (has `GROWSDOWN` flag)
2. Verify fault address is above minimum stack limit
3. Expand VMA downward: `vma_manager.expand_downwards()`
4. Allocate zeroed page, map into page table

### Demand Paging

Anonymous pages are allocated on first access (not at mmap/brk time). The page fault handler allocates a zeroed page and maps it when the address is first touched.

---

## ASID Management

**File**: `kernel/src/arch/riscv64/mm/asid.rs`

Address Space IDs allow TLB entries to be tagged per-process, avoiding full TLB flush on context switch.

```rust
pub fn allocate_asid() -> u16;       // Allocate unique ASID
pub fn free_asid(asid: u16);         // Release ASID
pub fn make_satp(ppn: u64, asid: u16) -> u64;  // Create satp value with ASID
```

---

## User Address Space Creation

**File**: `kernel/src/arch/riscv64/mm/mm_ops.rs`

### `create_user_address_space()`

Creates a new user page table with kernel mappings copied:

```rust
pub fn create_user_address_space() -> Option<u64> {
    // 1. Allocate root page table (PGD)
    let user_root_ppn = alloc_page_table()?;

    // 2. Copy kernel PGD entries (VPN2[256..511])
    copy_kernel_mappings(user_root_ppn, kernel_root_ppn);

    // 3. Copy fixmap entries (for UART access during syscalls)
    copy_fixmap_mappings(user_root_ppn);

    Some(user_root_ppn)
}
```

### `copy_kernel_mappings()`

Copies kernel-space PGD entries to user page table:

```
VPN2[0..1] (MMIO/low memory):
  - Create NEW L1 tables (not shared!) for each entry
  - Copy only kernel (U=0) leaf entries into new L1 tables
  - This prevents use-after-free when processes exit and free their page tables

VPN2[256..511] (kernel space):
  - Directly share PGD entries (shared kernel L1/L0 tables)
  - Safe because kernel tables are never freed by user processes
```

### Process Exit: `free_user_page_tables()`

Walks VPN2[0..255] (user space only), frees all user data pages and page table pages:

```rust
pub unsafe fn free_user_page_tables(root_ppn: u64) {
    // Walk L2 -> L1 -> L0
    // For each user leaf entry (U=1):
    //   put_page() -> if refcount == 0, free_pages()
    // For each non-leaf table:
    //   free_page_table() (only for late-stage allocations)
}
```

---

## Memory Statistics

**File**: `kernel/src/mm/meminfo.rs`

Provides `/proc/meminfo`-compatible memory information.

```rust
pub struct MemoryInfo {
    pub mem_total: usize,
    pub mem_free: usize,
    pub mem_available: usize,
    pub mem_used: usize,
    pub heap_total: usize,
    pub heap_used: usize,
    pub heap_free: usize,
    pub slab_pages: usize,
    pub pages_free: usize,
    pub pages_used: usize,
    pub pages_dirty: usize,
    pub pages_cow: usize,
    pub pages_anon: usize,
    // ...
}
```

---

## Memblock Allocator

**File**: `kernel/src/mm/memblock.rs`

Early boot memory allocator used before the zone/buddy system is ready.

```rust
// Region types
pub struct MemBlockType {
    regions: [MemBlockRegion; 128],
    count: usize,
}

pub struct MemBlockRegion {
    pub base: usize,
    pub size: usize,
    pub flags: MemBlockFlags,
}

// Core API
pub fn memblock_init();
pub fn memblock_add(base: usize, size: usize) -> Result<(), MemBlockError>;
pub fn memblock_reserve(base: usize, size: usize) -> Result<(), MemBlockError>;
pub fn memblock_phys_alloc() -> Option<usize>;  // Allocate one page
pub fn memblock_total_memory() -> usize;
pub fn memblock_available_memory() -> usize;
```

Memblock tracks two region lists (memory + reserved). Available memory = memory - reserved. Used during boot before the buddy allocator is initialized.

---

## API Reference

### Kernel Heap Allocation

```rust
// Small object allocation (via slab)
let ptr = kmalloc(128);
kfree(ptr);

// Zeroed allocation
let ptr = kzalloc(256);

// Large allocation (via buddy)
let addr = alloc_pages(GFP_KERNEL, 2);  // 2^2 = 4 pages
free_pages(addr, 2);
```

### Physical Page Operations

```rust
// Allocate single page
let page = alloc_page(GFP_KERNEL);

// Convert addresses
let virt = phys_to_virt(PhysAddr::new(phys));
let phys = virt_to_phys(VirtAddr::new(virt));

// Page descriptor access
let page = pfn_to_page_mut(pfn);
page.get_page();   // Increment refcount
page.put_page();   // Decrement refcount
```

### Process Address Space

```rust
// Create user address space
let root_ppn = create_user_address_space()?;

// brk syscall
let new_brk = mm.do_brk(addr);

// mmap syscall
let addr = mm.mmap(vaddr, size, flags, vma_type, perm, map_flags);

// munmap syscall
mm.munmap(addr, size);

// Fork address space
let child_mm = parent_mm.fork()?;
```

### Page Table Operations

```rust
// Map a page
unsafe { map_page(root_ppn, virt, phys, flags); }

// Map 2MB huge page
unsafe { map_pmd_huge_page(virt, phys, flags); }

// Map kernel region (identity)
unsafe { map_kernel_region(virt, phys, size, flags); }

// Allocate page table (three-stage)
let phys = alloc_page_table()?;

// Walk page table
let (ppn, pte_bits) = PageTableWalker::walk(root_ppn, virt)?;
```

---

## Related Documentation

- [Boot Process](boot.md) - MMU trampoline, boot sequence
- [RISC-V Architecture](riscv64.md) - Sv39 page table details
- [Process Management](design.md) - fork/execve implementation

---

## Change Log

- **2026-04-09**: Memory feature updates
  - Added memory compaction (two-pointer scan, page migration)
  - Added swap subsystem (swap entry encoding, swap device, swap-out/in)
  - Added LRU page cache (LRU_INACTIVE_FILE, eviction, referenced flag rotation)
  - Added kswapd daemon and OOM killer
  - Added reverse mapping (rmap) for page reclamation
  - Updated date

- **2026-03-27**: Major rewrite
  - Updated to reflect Linux-style boot with KERNEL_LINK_ADDR
  - Added three-stage page table allocation documentation
  - Added vmemmap page descriptor system
  - Added zone allocator documentation
  - Added detailed COW and page fault handling flows
  - Added ASID management section
  - Added memblock allocator section
  - Updated virtual memory layout to Linux Sv39 constants
  - Added copy_kernel_mappings and free_user_page_tables documentation

- **2026-03-04**: Created document
  - Initial memory layout description
  - Allocator designs
  - API reference
