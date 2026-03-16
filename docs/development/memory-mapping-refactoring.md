# Linux vs Rux Memory Implementation Analysis

> **Latest Update (2026-03-17)**: Added memblock module for Linux-style early memory management.

## Overview

This document details Rux kernel's memory implementation, including differences from Linux and the refactoring done to align with Linux's approach.

---

## Table of Contents

1. [Memory Layout](#1-memory-layout)
2. [Memblock - Early Memory Manager](#2-memblock---early-memory-manager)
3. [Page Table Allocation](#3-page-table-allocation)
4. [User Physical Memory Allocator](#4-user-physical-memory-allocator)
5. [Differences from Linux](#5-differences-from-linux)
6. [Future Work](#6-future-work)

---

## 1. Memory Layout

### 1.1 Address Space Layout (Sv39)

Rux uses RISC-V Sv39 paging with the3-level page tables, matching Linux:

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
| **Gap** | 0x82e00000 | 0x84000000 | 18 MB | Unused |
| User phys allocator | 0x84000000 | 0x88000000 | 64 MB | User process memory |
| Frame allocator | 0x88000000 | end of RAM | varies | Dynamic page tables |

---

## 2. Memblock - Early Memory Manager

### 2.1 Overview

Rux implements a Linux-style `memblock` module for early boot memory management. This replaces the previous hardcoded memory region calculations.

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
mm::memblock_reserve(heap_start, heap_size).ok();       // Heap
mm::memblock_reserve(slab_start, slab_size).ok();       // Slab
mm::memblock_reserve(0x84000000, 0x4000000).ok();       // User phys allocator (64MB)

// 4. Get frame allocator start from memblock
let frame_alloc_start = mm::memblock_get_available_region()
    .map(|r| r.base)
    .unwrap_or(0x88000000);
```

---

## 3. Page Table Allocation

### 3.1 Hybrid Allocation Strategy

Rux uses a hybrid approach for page table allocation:

1. **Early boot (static)**: Pre-allocated page tables from `.pagetables` section
2. **After frame allocator ready (dynamic)**: Dynamically allocated from frame allocator

```rust
// kernel/src/arch/riscv64/mm/base.rs

const MAX_KERNEL_PAGE_TABLES: usize = 256;

#[link_section = ".pagetables"]
static mut KERNEL_PAGE_TABLES: [PageTable; MAX_KERNEL_PAGE_TABLES] = [...];

static FRAME_ALLOCATOR_READY: AtomicBool = AtomicBool::new(false);

unsafe fn alloc_page_table() -> Option<&'static mut PageTable> {
    if is_frame_allocator_ready() {
        // Dynamic allocation from frame allocator
        let frame = alloc_kernel_page()?;
        let phys_addr = frame.start_address().as_usize() as u64;
        core::ptr::write_bytes(phys_addr as *mut u8, 0, PAGE_SIZE as usize);
        Some(&mut *(phys_addr as *mut PageTable))
    } else {
        // Static allocation for early boot
        let idx = KERNEL_PT_NEXT.fetch_add(1, Ordering::AcqRel);
        Some(&mut KERNEL_PAGE_TABLES[idx])
    }
}
```

### 3.2 Page Table Freeing

When a process exits, its page tables are freed:

```rust
// kernel/src/arch/riscv64/mm/base.rs
pub unsafe fn free_user_page_tables(root_ppn: u64) {
    // Walk and free all 3 levels of page tables
    // Only user space mappings (VPN2 0-255)
    for vpn2 in 0..256 {
        // ... free L1 and L0 tables
    }
    free_page_table(root_phys);
}

// Called from MmStruct::drop
impl Drop for MmStruct {
    fn drop(&mut self) {
        if self.space_type == PageTableType::User {
            unsafe {
                crate::arch::mm::free_user_page_tables(self.pgd);
            }
        }
    }
}
```

---

## 4. User Physical Memory Allocator

### 4.1 Purpose

The user physical memory allocator manages memory for user processes:

```rust
// kernel/src/arch/riscv64/mm/base.rs
static mut USER_PHYS_ALLOCATOR: PhysAllocator = PhysAllocator::new();

pub fn init_user_phys_allocator(start: u64, size: u64) {
    unsafe {
        let alloc_start = start + size;  // Allocate top-down
        let alloc_limit = start + 0x4000000;  // Reserve 64MB for kernel
        USER_PHYS_ALLOCATOR.init(alloc_start, alloc_limit);
    }
}
```

### 4.2 Current Hardcoding (Needs Fix)

Currently the user physical allocator region is hardcoded:

```rust
// main.rs - HARDCODED 64MB
arch::mm::init_user_phys_allocator(0x84000000, 0x4000000);
mm::memblock_reserve(0x84000000, 0x4000000).ok();
```

**Should be**: Dynamically calculated from memblock based on device tree.

---

## 5. Differences from Linux

### 5.1 Identity Mapping vs PAGE_OFFSET

| Aspect | Linux | Rux |
|--------|-------|-----|
| Kernel virtual base | PAGE_OFFSET (0xffffffe0xxxxxxxx) | Identity mapped (phys = virt) |
| Physical access | via PAGE_OFFSET + physical address | Direct physical address |
| Reason | Supports more than 4GB RAM | Simplicity for embedded systems |

**Rux approach works because:**
- QEMU virt machine has limited RAM (< 4GB)
- Physical addresses (0x80000000+) are directly accessible
- No need for complex address translation

### 5.2 Memblock Implementation

| Feature | Linux | Rux | Status |
|---------|-------|-----|--------|
| Memory discovery | Device tree | Device tree | ✅ Done |
| memblock_add() | Yes | Yes | ✅ Done |
| memblock_reserve() | Yes | Yes | ✅ Done |
| memblock_remove() | Yes | Yes | ✅ Done |
| memblock_mark_nomap() | Yes | Yes | ✅ Done |
| NUMA support | Yes | No (single node) | ⚠️ Simplified |
| Hotplug support | Yes | No | ⚠️ Not needed |

### 5.3 Memory Regions Still Hardcoded

The following regions are still hardcoded and should be dynamic:

| Region | Current | Should Be |
|--------|---------|-----------|
| User phys allocator | 0x84000000, 64MB | Calculated from memblock |
| Frame allocator start | After 0x88000000 | First available region from memblock |
| Kernel heap size | KERNEL_HEAP_SIZE config | Could be dynamic |

### 5.4 Page Table Allocation

| Feature | Linux | Rux |
|---------|-------|-----|
| Allocation | Slab allocator (kmem_cache) | Frame allocator (after init) |
| Early boot | memblock_alloc() | Static .pagetables section |
| Per-process tables | Freed on exit | Freed on exit (via MmStruct::drop) |

---

## 6. Future Work

### 6.1 Fully Dynamic User Physical Allocator

**Current issue:** User physical allocator region (0x84000000, 64MB) is hardcoded.

**Solution:**
1. Parse device tree for available memory
2. Calculate user physical allocator region from memblock
3. Size should be proportional to total RAM

```rust
// Proposed implementation
let total_mem = mm::memblock_total_memory();
let user_phys_size = (total_mem / 4).min(64 * 1024 * 1024); // 25% of RAM, max 64MB
let user_phys_start = /* find gap in memblock */;
```

### 6.2 Buddy Allocator Integration

Currently memblock is only used for early boot. Should integrate with buddy allocator:

1. memblock provides early boot allocation
2. After buddy allocator init, free unused memblock memory to buddy
3. memblock becomes read-only after boot

### 6.3 NUMA Support (Optional)

For multi-socket systems:
- Add NUMA node ID to memblock regions
- Per-node memory allocators
- NUMA-aware page allocation

### 6.4 Memory Hotplug (Optional)

For server systems:
- Add/remove memory regions at runtime
- Integration with device tree overlays

---

## 7. File Reference

| File | Purpose |
|------|---------|
| `kernel/src/mm/memblock.rs` | Memblock early memory manager |
| `kernel/src/cmdline.rs` | FDT parsing (bootargs, memory nodes) |
| `kernel/src/arch/riscv64/mm/base.rs` | Page table allocation, user phys allocator |
| `kernel/src/mm/mm_struct.rs` | MmStruct with Drop for page table freeing |
| `kernel/src/main.rs` | Memory initialization with memblock |

---

## 8. Boot Log Example

```
mm:               memblock: 1920 MB available
mm:               user frame allocator 64MB
mm:               32768 page descriptors
memblock:         total 2048MB, available 1938MB
```

This shows:
- 1920 MB available for frame allocator (after all reserved regions)
- 64 MB reserved for user physical allocator
- Total memory 2048 MB (2GB from device tree)
- Available for use: 1938 MB
