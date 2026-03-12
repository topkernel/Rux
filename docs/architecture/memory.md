# Rux Memory Management Design Document

This document details the design and implementation of the Rux kernel memory management subsystem.

**Last Updated**: 2026-03-04
**Code Location**: `kernel/src/mm/` (~4,300 lines of code)
**Architecture Support**: RISC-V Sv39

---

## Table of Contents

- [Overview](#overview)
- [Memory Layout](#memory-layout)
- [Physical Memory Management](#physical-memory-management)
- [Virtual Memory Management](#virtual-memory-management)
- [Kernel Heap Allocation](#kernel-heap-allocation)
- [Process Address Space](#process-address-space)
- [Copy-on-Write](#copy-on-write)
- [Memory Statistics](#memory-statistics)
- [API Reference](#api-reference)

---

## Overview

### Design Goals

1. **Linux Compatible**: ABI compatible with Linux kernel, supports standard system calls
2. **Efficient Allocation**: Multi-level allocators to reduce memory fragmentation
3. **SMP Optimized**: Per-CPU caches reduce lock contention
4. **Security Isolation**: Strict separation between kernel and user space

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
|   Page Tables (Sv39) / AddressSpace                        |
+-------------------------------------------------------------+
                              |
                              v
+-------------------------------------------------------------+
|                     Physical Memory Management              |
|   Frame Allocator / Page Descriptor                        |
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

| Module | File | Lines | Function |
|--------|------|-------|----------|
| **Physical Page Management** | page.rs | ~250 | Physical address/frame operations |
| **Page Descriptor** | page_desc.rs | ~350 | Per-page metadata |
| **Buddy Allocator** | buddy_allocator.rs | ~490 | Kernel heap allocation |
| **Slab Allocator** | slab.rs | ~610 | Small object allocation |
| **Per-CPU Pages** | pcp.rs | ~400 | CPU local cache |
| **VMA Management** | vma.rs | ~500 | Virtual memory area |
| **Address Space** | mm_struct.rs | ~550 | Process address space |
| **Page Table Mapping** | pagemap.rs | ~70 | Platform-independent interface |
| **RISC-V Page Tables** | arch/riscv64/mm/ | ~2,000 | Sv39 implementation |
| **Memory Statistics** | meminfo.rs | ~200 | /proc/meminfo |

---

## Memory Layout

### Physical Memory Layout

```
0x0000_0000 +-----------------------------+
            |     OpenSBI / Bootloader    |
0x0080_0000 +-----------------------------+
            |     Kernel code segment (.text) |
0x0080_2000 +-----------------------------+
            |     Kernel data segment (.data) |
0x0080_4000 +-----------------------------+
            |     Kernel BSS segment (.bss)   |
0x0080_8000 +-----------------------------+
            |     Kernel stack               |
0x0080_F000 +-----------------------------+
            |     Page descriptor array (mem_map) |
0x00A0_0000 +-----------------------------+
            |     Kernel heap (Buddy + Slab)    |
            |     Configurable size (default 16MB) |
0x08A0_0000 +-----------------------------+
            |     Available physical memory     |
            |     Managed by Frame Allocator    |
            |     ~2GB available                |
0x8000_0000 +-----------------------------+
```

### Virtual Memory Layout (Sv39)

```
Kernel Space (Upper 256GB)
--------------------------------------------------
0xFFFF_0000_0000_0000 +-------------------+
                       |  Kernel code/data |
                       |  Direct mapping area |
                       |  Device mapping area |
0xFFFF_FFFF_FFFF_FFFF +-------------------+

User Space (Lower 256GB)
--------------------------------------------------
0x0000_0000_0001_0000 +-------------------+
                       |  User code segment |
                       |  User data segment |
0x0000_0000_0100_0000 +-------------------+
                       |  User heap (brk) |
                       |  Grows upward    |
0x0000_0000_3000_0000 +-------------------+
                       |  mmap area       |
                       |  Starting at 0x5000_0000 |
0x0000_0000_6000_0000 +-------------------+
                       |  Shared libraries |
0x0000_0000_7FFF_F000 +-------------------+
                       |  User stack      |
                       |  Grows downward  |
0x0000_0000_7FFF_FFFF +-------------------+
```

### Address Space Constants

```rust
// Page size
pub const PAGE_SIZE: usize = 4096;

// Physical memory
pub const PHYS_MEMORY_SIZE: usize = 2 * 1024 * 1024 * 1024;  // 2GB

// Kernel virtual address base
pub const KERNEL_VIRT_BASE: usize = 0xFFFF_0000_0000_0000;

// User space range
pub const USER_VIRT_BASE: usize = 0x0000_0000_1000_0000;
pub const USER_VIRT_TOP: usize = 0x0000_0000_7FFF_FFFF;

// User address layout
pub const BRK_DEFAULT: usize = 0x3000_0000;      // 768MB
pub const MMAP_START: usize = 0x5000_0000;       // 1.25GB
pub const STACK_TOP: usize = 0x7FFF_F000;        // Stack top
pub const STACK_MAX_SIZE: usize = 8 * 1024 * 1024; // 8MB
```

---

## Physical Memory Management

### Frame Allocator

**File**: `kernel/src/mm/page.rs`

Manages allocation and deallocation of physical page frames.

```rust
pub struct FrameAllocator {
    next_free: AtomicUsize,    // Next free page
    free_list: AtomicUsize,    // Free list head
    total_frames: usize,       // Total pages
    use_page_desc: AtomicUsize, // Whether to use Page descriptor
}

// Core interface
pub fn alloc_frame() -> Option<PhysFrame>;
pub fn dealloc_frame(frame: PhysFrame);
```

**Features**:
- Linear allocation + free list recycling
- Atomic operations, SMP support
- Integration with Page descriptor

### Page Descriptor

**File**: `kernel/src/mm/page_desc.rs`

Maintains metadata for each physical page frame (similar to Linux `struct page`).

```rust
#[repr(C, align(64))]
pub struct Page {
    flags: PageFlags,          // Atomic flags
    _mapcount: AtomicI32,      // Mapping count
    _refcount: AtomicI32,      // Reference count
    private: AtomicUsize,      // Private data
    mapping: AtomicUsize,      // address_space pointer
    index: AtomicU64,          // Mapping offset
    _type: AtomicU32,          // Page type
    next_free: AtomicUsize,    // Free list pointer
}
```

**Page Flags**:

| Flag | Description |
|------|-------------|
| `Locked` | Page is locked |
| `Dirty` | Page is modified |
| `Referenced` | Page has been accessed |
| `UpToDate` | Page data is valid |
| `Lru` | In LRU list |
| `Reserved` | Reserved page |
| `Cow` | Copy-on-write page |
| `Anonymous` | Anonymous page |

**Global mem_map Array**:

```rust
// Page frame number to Page conversion
pub fn pfn_to_page(pfn: usize) -> &'static Page;
pub fn pfn_to_page_mut(pfn: usize) -> &'static mut Page;
pub fn page_to_pfn(page: &Page) -> usize;
```

---

## Virtual Memory Management

### RISC-V Sv39 Page Tables

**File**: `kernel/src/arch/riscv64/mm/`

Sv39 is the standard paging mode for RISC-V:

| Feature | Value |
|---------|-------|
| Virtual address bits | 39 bits |
| Address space size | 512 GB |
| Page table levels | 3 levels |
| Entries per level | 512 |
| Page size | 4 KB |
| PTE size | 8 bytes |

**Virtual Address Decomposition**:

```
+---------+---------+---------+------------+
|  VPN[2] |  VPN[1] |  VPN[0] |  Page offset |
|  9 bits |  9 bits |  9 bits |  12 bits   |
+---------+---------+---------+------------+
   L2 index    L1 index    L0 index
```

**Page Table Entry (PTE) Format**:

```
+----------------+--------------------------+
|     PPN        |        Flags             |
|    44 bits     |        10 bits           |
+----------------+--------------------------+

Flags:
- V: Valid
- R: Readable
- W: Writable
- X: Executable
- U: User accessible
- G: Global mapping
- A: Accessed
- D: Dirty
```

### AddressSpace

**File**: `kernel/src/arch/riscv64/mm/base.rs`

Manages the complete page table of a process.

```rust
pub struct AddressSpace {
    pgd: AtomicU64,              // Page table root address
    page_table_lock: SpinLock<()>, // Page table lock
    mm: Option<Arc<MmStruct>>,   // Associated mm_struct
}
```

**Core Operations**:

```rust
// Map page
pub fn map(&self, vaddr: VirtAddr, paddr: PhysAddr, perm: Perm) -> Result<(), MapError>;

// Unmap page
pub fn unmap(&self, vaddr: VirtAddr) -> Result<PhysAddr, MapError>;

// Modify permissions
pub fn protect(&self, vaddr: VirtAddr, perm: Perm) -> Result<(), MapError>;

// Query physical address
pub fn translate(&self, vaddr: VirtAddr) -> Option<PhysAddr>;
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
            | Per-CPU Pages |
            | (CPU local cache) |
            +---------------+
                    |
                    v
            +---------------+
            | Frame         |
            | Allocator     |
            +---------------+
```

### Buddy Allocator

**File**: `kernel/src/mm/buddy_allocator.rs`

Buddy system allocator managing kernel heap memory.

**Features**:
- Supports order 0 ~ 20 (4KB ~ 4GB)
- Metadata stored separately from user data
- O(log n) allocation/deallocation complexity
- Automatic coalescing of adjacent free blocks

```rust
pub struct BuddyAllocator {
    magic: AtomicUsize,           // Magic number detection
    heap_start: AtomicUsize,      // Heap start address
    heap_end: AtomicUsize,        // Heap end address
    free_lists: [AtomicUsize; MAX_ORDER + 1], // Free lists per order
    meta: MetaArray,              // Metadata array
}
```

**Allocation Process**:

```
1. Calculate order based on size
2. Find free block from free_lists[order]
3. If not available, split from higher order
4. Add split buddy blocks to corresponding order list
5. Return allocated address
```

**Deallocation Process**:

```
1. Calculate block order and page_idx
2. Find buddy block (buddy_idx = page_idx ^ (1 << order))
3. If buddy is free and same order, coalesce
4. Repeat until no more coalescing possible
5. Add to corresponding order free list
```

### Slab Allocator

**File**: `kernel/src/mm/slab.rs`

Small object allocator to reduce memory fragmentation.

**Supported Object Sizes**:
```
8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096 bytes
```

```rust
pub struct SlabCache {
    object_size: usize,       // Object size
    objects_per_slab: usize,  // Objects per slab
    free_list: u16,           // Free slab list
    partial_list: u16,        // Partial slab list
    full_list: u16,           // Full slab list
}
```

**Slab Structure**:

```
+---------------------------------------------+
|  SlabHeader (16 bytes)                      |
|  - cache_idx, object_size, total_objects    |
|  - free_objects, free_index, next, prev     |
+---------------------------------------------+
|  Object 0  |  Object 1  |  ...  | Object N  |
|  (fixed size) |  (fixed size) |     | (fixed size) |
+---------------------------------------------+
```

**Public Interface**:

```rust
// Allocate memory
pub fn kmalloc(size: usize) -> *mut u8;

// Free memory
pub fn kfree(ptr: *mut u8);

// Allocate and zero
pub fn kzalloc(size: usize) -> *mut u8;
```

### Per-CPU Pages (PCP)

**File**: `kernel/src/mm/pcp.rs`

Per-CPU page cache to reduce global allocator lock contention.

```rust
pub struct PerCpuPages {
    lists: [usize; MIGRATE_TYPES],   // Page lists per type
    counts: [usize; MIGRATE_TYPES],  // Page counts per type
    high: usize,                     // High water mark
    batch: usize,                    // Batch operation count
}
```

**Migration Types**:

| Type | Description |
|------|-------------|
| `Unmovable` | Cannot be moved (kernel use) |
| `Movable` | Can be moved (userspace pages) |
| `Reclaimable` | Can be reclaimed (swappable) |

**Allocation Flow**:

```
1. Allocate from local CPU cache (lock-free)
2. If local cache empty, batch acquire from global
3. If above high water mark, batch return to global
```

**Public Interface**:

```rust
// Allocate kernel page
pub fn alloc_kernel_page() -> Option<PhysFrame>;

// Allocate user page
pub fn alloc_user_page() -> Option<PhysFrame>;

// Free pages
pub fn free_kernel_page(frame: PhysFrame);
pub fn free_user_page(frame: PhysFrame);
```

---

## Process Address Space

### MmStruct

**File**: `kernel/src/mm/mm_struct.rs`

Process address space descriptor, corresponding to Linux `mm_struct`.

```rust
pub struct MmStruct {
    // Page table management
    pub pgd: u64,                      // Page table root
    vma_manager: RwLock<VmaManager>,   // VMA manager
    space_type: PageTableType,         // Address space type

    // Segment ranges
    start_code: AtomicUsize,
    end_code: AtomicUsize,
    start_data: AtomicUsize,
    end_data: AtomicUsize,

    // Heap management
    start_brk: AtomicUsize,
    brk: AtomicUsize,

    // Stack management
    start_stack: AtomicUsize,

    // Arguments and environment variables
    arg_start: AtomicUsize,
    arg_end: AtomicUsize,
    env_start: AtomicUsize,
    env_end: AtomicUsize,

    // Virtual memory statistics
    total_vm: AtomicU64,
    locked_vm: AtomicU64,
    // ...
}
```

### VMA (Virtual Memory Area)

**File**: `kernel/src/mm/vma.rs`

Describes a contiguous region in process address space.

```rust
pub struct Vma {
    start: VirtAddr,         // Start address
    end: VirtAddr,           // End address
    flags: VmaFlags,         // Permission flags
    vma_type: VmaType,       // VMA type
    offset: u64,             // File offset
    file: Option<Arc<File>>, // Associated file
}
```

**VMA Flags**:

| Flag | Description |
|------|-------------|
| `READ` | Readable |
| `WRITE` | Writable |
| `EXEC` | Executable |
| `SHARED` | Shared mapping |
| `PRIVATE` | Private mapping (COW) |
| `GROWSDOWN` | Grows downward (stack) |

**VMA Types**:

```rust
pub enum VmaType {
    Anonymous,    // Anonymous mapping
    File,         // File mapping
    Stack,        // Stack
    Heap,         // Heap
    Vdso,         // VDSO
}
```

### VmaManager

Uses BTreeMap to manage VMAs, supporting fast lookup.

```rust
pub struct VmaManager {
    vmas: BTreeMap<VirtAddr, Vma>,
    // ...
}

// Core operations
impl VmaManager {
    pub fn find_vma(&self, addr: VirtAddr) -> Option<&Vma>;
    pub fn insert(&mut self, vma: Vma) -> Result<(), VmaError>;
    pub fn remove(&mut self, start: VirtAddr) -> Option<Vma>;
    pub fn find_free_area(&self, len: usize, hint: VirtAddr) -> Option<VirtAddr>;
}
```

---

## Copy-on-Write

### COW Mechanism

When fork() creates a child process, physical pages are not immediately copied. Instead, the parent's pages are shared and marked read-only. When a process attempts to write, a page fault is triggered, and the kernel copies the page at that time.

**Implementation Flow**:

```
fork():
1. Copy parent's page table
2. Mark all writable pages as read-only
3. Set COW flag
4. Increment page reference count

Page fault handling:
1. Check if it's a COW page
2. Allocate new physical page
3. Copy content to new page
4. Update page table mapping
5. Set new page as writable
6. Decrement original page reference count
```

**Page Flags**:

```rust
// COW page flag
pub const Cow: u32 = 1 << 14;

// In Page descriptor
page.flags.set(PageFlag::Cow);
```

---

## Memory Statistics

### MemoryInfo

**File**: `kernel/src/mm/meminfo.rs`

Provides memory statistics similar to `/proc/meminfo`.

```rust
pub struct MemoryInfo {
    // Physical memory
    pub mem_total: usize,
    pub mem_free: usize,
    pub mem_available: usize,
    pub mem_used: usize,

    // Heap memory
    pub heap_total: usize,
    pub heap_used: usize,
    pub heap_free: usize,

    // Slab
    pub slab_pages: usize,
    pub slab_allocs: usize,
    pub slab_frees: usize,

    // Per-CPU Pages
    pub pcp_pages: [usize; 4],

    // Page status
    pub pages_free: usize,
    pub pages_used: usize,
    pub pages_reserved: usize,
    pub pages_mapped: usize,
    pub pages_dirty: usize,
    pub pages_cow: usize,
    pub pages_anon: usize,
}
```

**Access Methods**:

```rust
// Get memory statistics
let info = get_memory_info();
print_memory_info();

// Get summary (for procfs)
let summary = get_memory_summary();

// Check memory pressure
if is_memory_low() {
    // Trigger memory reclaim
}

if should_trigger_oom() {
    // Trigger OOM killer
}
```

---

## API Reference

### Kernel Heap Allocation

```rust
// Small object allocation (<= 4KB)
let ptr = kmalloc(128);
kfree(ptr);

// Allocate and zero
let ptr = kzalloc(256);

// Large object allocation (> 4KB)
let layout = Layout::from_size_align(8192, 4096).unwrap();
let ptr = HEAP_ALLOCATOR.alloc(layout);
HEAP_ALLOCATOR.dealloc(ptr, layout);
```

### Physical Page Allocation

```rust
// Allocate single page
let frame = alloc_frame().expect("out of memory");

// Allocate Per-CPU pages
let frame = alloc_kernel_page();
let frame = alloc_user_page();

// Free pages
dealloc_frame(frame);
free_kernel_page(frame);
free_user_page(frame);
```

### Virtual Memory Operations

```rust
// Create address space
let space = AddressSpace::new_user();

// Map page
space.map(vaddr, paddr, Perm::ReadWrite)?;

// Modify permissions
space.protect(vaddr, Perm::Read)?;

// Unmap page
space.unmap(vaddr)?;
```

### Process Address Space

```rust
// Get current process's mm
let mm = current_mm()?;

// brk system call
let new_brk = mm.do_brk(addr)?;

// mmap system call
let addr = mm.do_mmap(addr, len, prot, flags, fd, offset)?;

// munmap system call
mm.do_munmap(addr, len)?;
```

---

## Related Documentation

- [RISC-V Architecture](riscv64.md) - Sv39 page table details
- [Process Management](design.md) - fork/execve implementation
- [Test Report](../tests/unit-test-report.md) - Memory management tests

---

## Change Log

- **2026-03-04**: Created document
  - Detailed memory layout description
  - Recorded allocator designs
  - Added API reference
