# Linux vs Rux Memory Implementation Analysis

> **Latest Update (2026-03-18)**: Removed legacy USER_PHYS_ALLOCATOR, unified to zone system.

## Overview

This document details Rux kernel's memory implementation, including differences from Linux and the refactoring done to align with Linux's approach.

---

## Table of Contents

1. [Memory Layout](#1-memory-layout)
2. [Memblock - Early Memory Manager](#2-memblock---early-memory-manager)
3. [Zone System](#3-zone-system)
4. [Page Table Allocation](#4-page-table-allocation)
5. [Differences from Linux](#5-differences-from-linux)
6. [Future Work](#6-future-work)

---

## 1. Memory Layout

### 1.1 Address Space Layout (Sv39)

Rux uses RISC-V Sv39 paging with 3-level page tables, matching Linux:

| Region | Start | End | Size | Description |
|--------|-------|-----|------|-------------|
| User space | 0x0 | 0x0000007f_ffffffff | 256 GiB | User code, data, heap, stack |
| Kernel space | 0xffffffc0_00000000 | 0xffffffc0_ffffffff | 4 GiB | Kernel mappings (not yet used) |
| Physical identity | 0x80000000 | varies | varies | Identity mapped physical memory |

**TASK_SIZE** = 256 GiB (matches Linux RISC-V)

### 1.2 Physical Memory Layout (QEMU virt, 2GB RAM)

| Region | Start | End | Size | Description |
|--------|-------|-----|------|-------------|
| OpenSBI | 0x80000000 | 0x801fffff | ~2 MB | Firmware |
| Kernel code/data | 0x80200000 | 0x809fffff | ~8 MB | Kernel image |
| Kernel heap | 0x80a00000 | 0x82a00000 | 32 MB | Buddy allocator |
| Slab allocator | 0x82a00000 | 0x82e00000 | 4 MB | Slab objects |
| **Frame allocator** | 0x82e00000 | end of RAM | ~1970 MB | Dynamic allocation via zone system |

**Note:** The old separate user physical allocator (64MB at 0x84000000) has been removed.
All memory allocation now goes through the unified zone system.

---

## 2. Memblock - Early Memory Manager

### 2.1 Overview

Rux implements a Linux-style `memblock` module for early boot memory management.

**File:** `kernel/src/mm/memblock.rs`

### 2.2 Key Structures

```rust
/// Memory region descriptor
pub struct MemBlockRegion {
    pub base: usize,    // Physical start address
    pub size: usize,    // Region size in bytes
    pub flags: MemBlockFlags,
    pub nid: u32,       // NUMA node ID (0 for UMA)
}

/// Memblock manager
pub struct MemBlock {
    memory: MemBlockType,    // Available memory regions (from device tree)
    reserved: MemBlockType,  // Reserved regions (kernel, heap, etc.)
    initialized: AtomicBool,
    bottom: usize,           // Bottom of available memory (after reserved)
    top: usize,              // Top of available memory
}
```

### 2.3 API

```rust
// Initialize memblock
pub fn memblock_init();

// Add memory region (from device tree)
pub fn memblock_add(base: usize, size: usize) -> Result<(), ()>;

// Reserve memory region
pub fn memblock_reserve(base: usize, size: usize) -> Result<(), ()>;

// Get first available region for frame allocator
pub fn memblock_get_available_region() -> Option<MemBlockRegion>;

// Query functions
pub fn memblock_total_memory() -> usize;
pub fn memblock_available_memory() -> usize;
pub fn memblock_is_reserved(addr: usize) -> bool;
```

### 2.4 Initialization Flow

```rust
// In main.rs:
// 1. Initialize memblock
mm::memblock_init();

// 2. Parse memory regions from device tree
let memory_regions = unsafe { cmdline::parse_memory_regions(dtb_ptr) };
for region in &memory_regions {
    mm::memblock_add(region.base, region.size).ok();
}

// 3. Reserve used regions
mm::memblock_reserve(0x80000000, 0xA00000).ok();       // OpenSBI + kernel (10MB)
mm::memblock_reserve(heap_start, heap_size).ok();       // Heap (32MB)
mm::memblock_reserve(slab_start, slab_size).ok();       // Slab (4MB)

// 4. Get frame allocator start from memblock (dynamic!)
let frame_alloc_start = mm::memblock_get_available_region()
    .map(|r| r.base)
    .unwrap_or(0x82E00000);  // Fallback after kernel + heap + slab
```

---

## 3. Zone System

### 3.1 Overview

Rux implements a Linux-style zone system for physical page management. All memory allocation (kernel and user) goes through the unified zone system.

**Files:**
- `kernel/src/mm/zone.rs` - Zone definition and buddy allocator
- `kernel/src/mm/pglist.rs` - NUMA node representation
- `kernel/src/mm/page_alloc.rs` - Page allocation APIs

### 3.2 Zone Types

```rust
pub enum ZoneType {
    ZoneDma,      // DMA constrained devices (0-16MB typically)
    ZoneDma32,    // 32-bit DMA devices (0-4GB)
    ZoneNormal,   // Normal memory (all usable memory on RISC-V)
    ZoneMovable,  // Migratable pages (for memory compaction)
}
```

### 3.3 Allocation APIs

```rust
// Linux-compatible APIs
pub fn alloc_pages(gfp_flags: GfpFlags, order: usize) -> usize;
pub fn free_pages(addr: usize, order: usize);
pub fn get_zeroed_page(gfp_flags: GfpFlags) -> usize;

// GFP flags
pub struct GfpFlags {
    GFP_KERNEL,   // Kernel allocation (can sleep)
    GFP_USER,     // User allocation
    GFP_ATOMIC,   // Atomic allocation (cannot sleep)
    GFP_DMA,      // DMA allocation
    GFP_DMA32,    // DMA32 allocation
}
```

### 3.4 vmemmap

Page descriptors are mapped via vmemmap for O(1) PFN to page conversion:

```rust
// vmemmap_addr = VMEMMAP_START + pfn * sizeof(Page)
pub const fn pfn_to_vmemmap(pfn: usize) -> usize;

// pfn = (vmemmap_addr - VMEMMAP_START) / sizeof(Page)
pub const fn vmemmap_to_pfn(vaddr: usize) -> usize;
```

**File:** `kernel/src/mm/vmemmap.rs`

---

## 4. Page Table Allocation

### 4.1 Hybrid Allocation Strategy

Rux uses a hybrid approach for page table allocation:

1. **Early boot (static)**: Pre-allocated page tables from `.bss` section
2. **After frame allocator ready (dynamic)**: Dynamically allocated from frame allocator

```rust
// kernel/src/arch/riscv64/mm/base.rs

const MAX_KERNEL_PAGE_TABLES: usize = 256;

#[link_section = ".bss"]
static mut KERNEL_PAGE_TABLES: [PageTable; MAX_KERNEL_PAGE_TABLES] = [...];

static FRAME_ALLOCATOR_READY: AtomicBool = AtomicBool::new(false);

unsafe fn alloc_page_table() -> Option<&'static mut PageTable> {
    if is_frame_allocator_ready() {
        // Dynamic allocation from zone allocator
        let phys = alloc_pages(GfpFlags::GFP_KERNEL, 0);
        if phys != 0 {
            core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
            Some(&mut *(phys as *mut PageTable))
        } else {
            None
        }
    } else {
        // Static allocation for early boot
        let idx = KERNEL_PT_NEXT.fetch_add(1, Ordering::AcqRel);
        Some(&mut KERNEL_PAGE_TABLES[idx])
    }
}
```

### 4.2 Page Table Freeing

When a process exits, its page tables are freed via `MmStruct::drop`.

---

## 5. Differences from Linux

### 5.1 Identity Mapping vs PAGE_OFFSET

| Aspect | Linux | Rux |
|--------|-------|-----|
| Kernel virtual base | PAGE_OFFSET (0xffffffe0xxxxxxxx) | Identity mapped (phys = virt) |
| Physical access | via PAGE_OFFSET + physical address | Direct physical address |
| Reason | Supports more than 4GB RAM | Simplicity for embedded systems |

### 5.2 Memblock Implementation

| Feature | Linux | Rux | Status |
|---------|-------|-----|--------|
| Memory discovery | Device tree | Device tree | ✅ Done |
| memblock_add() | Yes | Yes | ✅ Done |
| memblock_reserve() | Yes | Yes | ✅ Done |
| memblock_remove() | Yes | Yes | ✅ Done |
| NUMA support | Yes | No (single node) | ⚠️ Simplified |

### 5.3 Zone System

| Feature | Linux | Rux | Status |
|---------|-------|-----|--------|
| Zone types | DMA/DMA32/Normal/Movable | ✅ Same | ✅ Done |
| Per-zone buddy | Yes | Yes | ✅ Done |
| Per-CPU pages | Yes | Yes | ✅ Done |
| GFP flags | Yes | Yes | ✅ Done |

### 5.4 Page Table Allocation

| Feature | Linux | Rux |
|---------|-------|-----|
| Allocation | Slab allocator (kmem_cache) | Zone allocator |
| Early boot | memblock_alloc() | Static .bss section |
| Per-process tables | Freed on exit | Freed on exit |

---

## 6. Future Work

### 6.1 ASID Support (Phase 3)

Implement Address Space ID for efficient TLB management:
- Replace global TLB flushes with ASID-targeted flushes
- Improve context switch performance

### 6.2 Page Table Locks (Phase 4)

Fine-grained locking for page table operations:
- Per-PMD locks for concurrent page faults
- SMP safety

### 6.3 Reverse Mapping (Phase 5)

Implement rmap for:
- Page migration
- Memory compaction
- Shared page tracking

### 6.4 Huge Pages (Phase 6)

Complete 2MB and 1GB huge page support.

---

## 7. File Reference

| File | Purpose |
|------|---------|
| `kernel/src/mm/memblock.rs` | Memblock early memory manager |
| `kernel/src/mm/zone.rs` | Zone definition and buddy allocator |
| `kernel/src/mm/pglist.rs` | NUMA node representation |
| `kernel/src/mm/page_alloc.rs` | Page allocation APIs |
| `kernel/src/mm/vmemmap.rs` | vmemmap page descriptor mapping |
| `kernel/src/mm/page_desc.rs` | Page descriptor structure |
| `kernel/src/mm/pcp.rs` | Per-CPU pages |
| `kernel/src/arch/riscv64/mm/base.rs` | Page table management |
| `kernel/src/main.rs` | Memory initialization |

---

## 8. Boot Log Example

```
mm:               vmemmap mapping initialized        [ok]
mm:               layout: kernel=0x80200000-0x80a0   [ok]
mm:               layout: heap=0x80a00000-0x82a000   [ok]
mm:               frame alloc @ 0x84e00000, 1970 MB  [ok]
mm:               524288 page descriptors            [ok]
mm:               zone allocator initialized         [ok]
memblock:         total 2048MB, available 1970MB     [ok]
```

This shows:
- vmemmap initialized for page descriptor mapping
- Dynamic frame allocator start address (0x84e00000)
- ~1970 MB available (no more separate user allocator)
- All memory managed by unified zone system
