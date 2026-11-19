# Rux Memory Management Unit Improvement Plan

**Last Updated**: 2026-03-04
**Status**: Partially Completed

---

## 1. Current Implementation Comparison Analysis

### 1.1 Data Structure Comparison

| Feature | Rux Implementation | Linux Implementation | Gap Analysis |
|---------|-------------------|---------------------|--------------|
| **Physical Page Management** | `FrameAllocator` (Bump + Free List) | `struct page` + Buddy + Per-CPU Pages | Linux has complete page descriptors and reference counting |
| **VMA Storage** | Static Array `[Option<Vma>; 256]` | `maple_tree` (B-tree variant) | Rux cannot dynamically expand, lookup O(n) |
| **Address Space** | `AddressSpace` (contains VmaManager) | `mm_struct` (contains multiple locks, counters) | Linux has comprehensive locking mechanisms and reference counting |
| **Page Table Entry** | `PageTableEntry(u64)` | `pte_t`, `pmd_t`, `pud_t`, `pgd_t` | Linux supports multi-level page table type safety |
| **Memory Zones** | No Zone concept | ZONE_DMA/DMA32/NORMAL/MOVABLE | Linux supports different memory types |

### 1.2 Feature Comparison

| Feature | Rux | Linux | Priority |
|---------|-----|-------|----------|
| Basic Page Table Mapping | OK | OK | - |
| Identity Mapping | OK | OK | - |
| User Address Space | OK | OK | - |
| mmap/munmap | OK (complete) | OK (complete) | Completed |
| brk/sbrk | OK | OK | - |
| Copy-on-Write | OK | OK | Completed |
| Page Fault Handling | OK (COW) | OK (complete) | High |
| Buddy Allocator | OK | OK | - |
| Slab Allocator | OK | OK (kmalloc) | Completed |
| Per-CPU Pages | OK | OK | Completed |
| Reverse Mapping (rmap) | No | OK | Medium |
| LRU Page Reclamation | No | OK | Low |
| Memory Compaction | No | OK | Low |
| Huge Page Support | No | OK (HugeTLB) | Low |
| Memory Hot-plug | No | OK | Low |
| Multi-level Page Tables Sv48/Sv57 | No (Sv39 only) | OK | Low |

### 1.3 Architecture Differences

#### Rux Current Design
```
+-----------------------------------------+
|            AddressSpace                  |
|  +------------------------------------+ |
|  |         VmaManager                  | |
|  |  [Vma; 256] static array            | |
|  +------------------------------------+ |
|  root_ppn --> PageTable (3-level Sv39)  |
|  brk: heap pointer                       |
+-----------------------------------------+
```

#### Linux Design
```
+---------------------------------------------+
|              mm_struct                       |
|  +-----------------------------------------+|
|  | maple_tree mm_mt (VMA B-tree)           ||
|  | - O(log n) lookup/insert                ||
|  | - Dynamic expansion                      ||
|  +-----------------------------------------+|
|  pgd --> 4/5-level page table (Sv39/Sv48/Sv57) |
|  +-- mmap_lock (read-write semaphore)       |
|  +-- page_table_lock (spinlock)             |
|  +-- mm_users / mm_count (reference count)  |
|  +-- total_vm / locked_vm / rss (statistics)|
+---------------------------------------------+
```

---

## 2. Improvement Plan

### Phase 1: Infrastructure Enhancement (Priority: High)

#### 1.1 Implement struct page Page Descriptor
**Goal**: Establish metadata management for each physical page

**Files to Modify**:
- `kernel/src/mm/page.rs` - Add `struct Page`
- `kernel/src/mm/mod.rs` - Add page array management

**Implementation**:
```rust
pub struct Page {
    flags: AtomicU32,       // Page status flags
    refcount: AtomicI32,    // Reference count
    mapping: AtomicUsize,   // Associated address_space (for file mapping)
    private: AtomicUsize,   // Private data
    lru: ListHead,          // LRU list node
}

// Global page array
static PAGE_ARRAY: &[Page] = ...;  // One Page per physical page

fn pfn_to_page(pfn: usize) -> &'static Page;
fn page_to_pfn(page: &Page) -> usize;
fn virt_to_page(addr: VirtAddr) -> &'static Page;
```

**Reference**: Linux `include/linux/mm_types.h` struct page

#### 1.2 Improve Locking Mechanism
**Goal**: Add fine-grained locks for SMP concurrency support

**Files to Modify**:
- `kernel/src/mm/pagemap.rs` - Add mmap_lock
- `kernel/src/arch/riscv64/mm.rs` - Add page_table_lock

**Implementation**:
```rust
pub struct AddressSpace {
    root_ppn: u64,
    vma_manager: VmaManager,
    mmap_lock: RwLock<()>,        // Read-write lock for VMA operations
    page_table_lock: SpinLock<()>, // Spinlock for page table operations
    mm_users: AtomicI32,          // User count (shared by threads)
    mm_count: AtomicI32,          // Reference count (mm_struct lifetime)
}
```

#### 1.3 Improve VMA Management
**Goal**: Use more efficient data structures, support dynamic expansion

**Option A (Recommended)**: Use BTreeMap instead of static array
```rust
use alloc::collections::BTreeMap;

pub struct VmaManager {
    vmas: BTreeMap<VirtAddr, Vma>,  // Sorted by start address
    lock: RwLock<()>,
}
```

**Option B**: Implement maple tree (Linux 6.1+ approach)
- More complex, but better performance
- As a long-term goal

---

### Phase 2: Core Feature Enhancement (Priority: High)

#### 2.1 Complete Copy-on-Write
**Goal**: Fully implement fork's COW mechanism

**Files to Modify**:
- `kernel/src/arch/riscv64/mm.rs` - Improve `copy_page_table_cow()`
- `kernel/src/arch/riscv64/trap.rs` - Handle COW page faults

**Implementation Points**:
1. When copying page tables, mark writable pages as read-only + COW flag
2. Use `refcount` to track shared page count
3. On write fault, check COW flag, allocate new page
4. When `refcount == 1`, directly restore write permission

**Reference**: Linux `mm/memory.c` do_wp_page()

```rust
// Page fault handling pseudocode
fn handle_page_fault(addr: VirtAddr, cause: FaultCause) {
    let vma = find_vma(addr)?;

    if cause == WriteFault && is_cow_page(addr) {
        if get_page_refcount(addr) > 1 {
            // Allocate new page, copy content
            let new_page = alloc_page();
            copy_page_content(old_page, new_page);
            update_page_table(addr, new_page, WRITE);
            decrement_refcount(old_page);
        } else {
            // Only one reference, directly restore write permission
            set_page_writable(addr);
        }
    }
}
```

#### 2.2 Complete Page Fault Handling
**Goal**: Support demand paging

**Files to Modify**:
- `kernel/src/arch/riscv64/trap.rs` - Extend page fault handling
- `kernel/src/mm/pagemap.rs` - Add handle_mm_fault()

**Implementation Points**:
1. Parse scause to determine fault type (read/write/execute)
2. Lookup VMA to verify permissions
3. Anonymous pages: allocate new page, zero it
4. File pages: read from file
5. Update page table, set correct permission bits

**Reference**: Linux `mm/memory.c` handle_mm_fault()

```rust
fn handle_mm_fault(mm: &AddressSpace, addr: VirtAddr, flags: FaultFlags) -> Result<()> {
    let vma = mm.find_vma(addr)?;

    // Check permissions
    if flags.contains(FaultFlags::WRITE) && !vma.flags.contains(VmaFlags::WRITE) {
        return Err(FaultError::Permission);
    }

    // Allocate or get page
    let page = if vma.vma_type == VmaType::Anonymous {
        alloc_zeroed_page()?
    } else {
        read_file_page(vma.file, vma.offset + (addr - vma.start))?
    };

    // Map page
    mm.map_page(addr, page, vma.flags.to_pte_flags())?;
    Ok(())
}
```

#### 2.3 Implement Complete mmap Functionality
**Goal**: Support MAP_SHARED, MAP_FIXED, MAP_ANONYMOUS, etc.

**Files to Modify**:
- `kernel/src/mm/pagemap.rs` - Extend mmap implementation
- `kernel/src/arch/riscv64/syscall.rs` - Complete system calls

**Flags to Support**:
```rust
pub struct MmapFlags(u32);
impl MmapFlags {
    pub const SHARED: u32    = 0x01;   // Shared mapping
    pub const PRIVATE: u32   = 0x02;   // Private mapping (COW)
    pub const FIXED: u32     = 0x10;   // Force address
    pub const ANONYMOUS: u32 = 0x20;   // Anonymous mapping
    pub const STACK: u32     = 0x20000; // Stack mapping
}
```

---

### Phase 3: Performance Optimization (Priority: Medium)

#### 3.1 Implement Slab Allocator
**Goal**: Efficient small object allocation, replace direct buddy allocator usage

**Files to Modify**:
- `kernel/src/mm/slab.rs` (new file)

**Implementation Plan**:
1. Use `linked_list` to manage free objects
2. Each slab contains multiple objects of the same size
3. Support kmalloc-8, kmalloc-16, ..., kmalloc-4096

**Reference**: Linux `mm/slub.c`

```rust
pub struct SlabCache {
    name: &'static str,
    object_size: usize,
    slabs_partial: ListHead,  // Partially used slabs
    slabs_full: ListHead,     // Fully used slabs
    slabs_free: ListHead,     // Free slabs
}

pub fn kmalloc(size: usize, flags: GFPFlags) -> *mut u8;
pub fn kfree(ptr: *mut u8);
```

#### 3.2 Implement Per-CPU Pages
**Goal**: Reduce buddy allocator lock contention

**Files to Modify**:
- `kernel/src/mm/page.rs` - Add Per-CPU cache

**Implementation Points**:
```rust
pub struct PerCpuPages {
    lists: [Vec<Page>; MIGRATE_TYPES],  // Page lists for each migration type
    count: usize,                        // Cached page count
    high: usize,                         // High water mark (return on overflow)
    batch: usize,                        // Batch operation count
}

// One per CPU
static PER_CPU_PAGES: PerCpu<PerCpuPages>;
```

#### 3.3 Add Zone Support
**Goal**: Distinguish different types of memory

**Files to Modify**:
- `kernel/src/mm/zone.rs` (new file)

```rust
pub enum ZoneType {
    Normal,     // Normal memory
    Movable,    // Migratable memory
    Device,     // Device memory
}

pub struct Zone {
    zone_type: ZoneType,
    spanned_pages: usize,
    present_pages: usize,
    free_area: [FreeArea; MAX_ORDER],
}
```

---

### Phase 4: Advanced Features (Priority: Low)

#### 4.1 Reverse Mapping
**Goal**: Find all virtual addresses mapping to a physical page

**Use Cases**:
- Page migration
- Page reclamation
- COW sharing detection

**Implementation**:
```rust
pub struct AnonVma {
    root: AtomicPtr<AnonVma>,
    degree: AtomicU32,  // Reference degree
    parent: *mut AnonVma,
}

impl Page {
    mapping: *mut AddressSpace,  // For file pages
    index: u64,                  // Page offset
}
```

#### 4.2 LRU Page Reclamation
**Goal**: Reclaim pages when memory is low

**Implementation**:
```rust
pub struct LruLists {
    active_anon: ListHead,
    inactive_anon: ListHead,
    active_file: ListHead,
    inactive_file: ListHead,
}

fn shrink_inactive_list(nr_to_scan: usize) -> usize;
fn refill_inactive_list();
```

#### 4.3 Huge Page Support
**Goal**: Support 2MB/1GB huge pages

**Changes**:
- Page table entry support for PS (Page Size) bit
- Huge page allocator
- hugetlbfs file system

---

## 3. Implementation Priority and Dependencies

```
Phase 1.1 (struct page) --+--> Phase 2.1 (COW)
                          |
Phase 1.2 (locking) ------+--> Phase 2.2 (page fault)
                          |
Phase 1.3 (VMA improvement) --> Phase 2.3 (mmap)

Phase 1.1 (struct page) -----> Phase 3.1 (Slab)
                              |
                              +--> Phase 3.2 (Per-CPU)
                              |
                              +--> Phase 3.3 (Zone)

Phase 2.1 (COW) + Phase 3.1 (Slab) --> Phase 4.1 (rmap)
                                          |
                                          +--> Phase 4.2 (LRU)

Phase 1.1 + Phase 1.2 --> Phase 4.3 (huge pages)
```

---

## 4. Estimated Workload

| Phase | Workload | Notes |
|-------|----------|-------|
| Phase 1 | 2-3 weeks | Infrastructure, needs careful design |
| Phase 2 | 2-3 weeks | Core features, needs extensive testing |
| Phase 3 | 3-4 weeks | Performance optimization, optional |
| Phase 4 | 4+ weeks | Advanced features, long-term goal |

---

## 5. Reference Resources

1. **Linux Source**: `refer/linux/mm/` directory
2. **RISC-V Specification**: `refer/linux/arch/riscv/mm/`
3. **Documentation**:
   - Linux `Documentation/mm/`
   - `include/linux/mm.h`
   - `include/linux/mmzone.h`

## 6. Notes

1. **Maintain POSIX Compatibility**: All changes must comply with POSIX standards
2. **External Interface Compatibility**: User-visible interfaces must match Linux
3. **Incremental Development**: Each feature independently testable
4. **Test Coverage**: Each phase needs corresponding test cases
