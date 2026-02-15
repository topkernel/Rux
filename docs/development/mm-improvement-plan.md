# Rux 内存管理单元改进计划

## 一、当前实现对比分析

### 1. 数据结构对比

| 特性 | Rux 实现 | Linux 实现 | 差距分析 |
|------|----------|------------|----------|
| **物理页管理** | `FrameAllocator` (Bump + 空闲链表) | `struct page` + Buddy + Per-CPU Pages | Linux 有完整的页描述符和引用计数 |
| **VMA 存储** | 静态数组 `[Option<Vma>; 256]` | `maple_tree` (B树变体) | Rux 无法动态扩展，查找 O(n) |
| **地址空间** | `AddressSpace` (含 VmaManager) | `mm_struct` (含多个锁、计数器) | Linux 有完善的锁机制和引用计数 |
| **页表项** | `PageTableEntry(u64)` | `pte_t`, `pmd_t`, `pud_t`, `pgd_t` | Linux 支持多级页表类型安全 |
| **内存区域** | 无 Zone 概念 | ZONE_DMA/DMA32/NORMAL/MOVABLE | Linux 支持不同内存类型 |

### 2. 功能对比

| 功能 | Rux | Linux | 优先级 |
|------|-----|-------|--------|
| 基本页表映射 | ✅ | ✅ | - |
| 恒等映射 | ✅ | ✅ | - |
| 用户地址空间 | ✅ | ✅ | - |
| mmap/munmap | ✅ (基础) | ✅ (完整) | 高 |
| brk/sbrk | ✅ | ✅ | - |
| Copy-on-Write | ⚠️ (框架) | ✅ | 高 |
| 页面错误处理 | ⚠️ (基础) | ✅ (完整) | 高 |
| 伙伴分配器 | ✅ | ✅ | - |
| Slab 分配器 | ❌ | ✅ (kmalloc) | 中 |
| Per-CPU Pages | ❌ | ✅ | 中 |
| 反向映射 (rmap) | ❌ | ✅ | 中 |
| LRU 页面回收 | ❌ | ✅ | 低 |
| 内存碎片整理 | ❌ | ✅ | 低 |
| 大页支持 | ❌ | ✅ (HugeTLB) | 低 |
| 内存热插拔 | ❌ | ✅ | 低 |
| 多级页表 Sv48/Sv57 | ❌ (仅 Sv39) | ✅ | 低 |

### 3. 架构差异

#### Rux 当前设计
```
┌─────────────────────────────────────────┐
│            AddressSpace                  │
│  ┌────────────────────────────────────┐ │
│  │         VmaManager                  │ │
│  │  [Vma; 256] 静态数组                │ │
│  └────────────────────────────────────┘ │
│  root_ppn ──► PageTable (3级 Sv39)      │
│  brk: 堆指针                             │
└─────────────────────────────────────────┘
```

#### Linux 设计
```
┌─────────────────────────────────────────────┐
│              mm_struct                       │
│  ┌─────────────────────────────────────────┐│
│  │ maple_tree mm_mt (VMA B树)              ││
│  │ - O(log n) 查找/插入                    ││
│  │ - 动态扩展                               ││
│  └─────────────────────────────────────────┘│
│  pgd ──► 4/5级页表 (Sv39/Sv48/Sv57)        │
│  ├─ mmap_lock (读写信号量)                  │
│  ├─ page_table_lock (自旋锁)               │
│  ├─ mm_users / mm_count (引用计数)         │
│  └─ total_vm / locked_vm / rss (统计)      │
└─────────────────────────────────────────────┘
```

---

## 二、改进计划

### Phase 1: 基础设施完善 (优先级: 高)

#### 1.1 实现 struct page 页描述符
**目标**: 为每个物理页建立元数据管理

**改动文件**:
- `kernel/src/mm/page.rs` - 添加 `struct Page`
- `kernel/src/mm/mod.rs` - 添加页数组管理

**实现内容**:
```rust
pub struct Page {
    flags: AtomicU32,       // 页状态标志
    refcount: AtomicI32,    // 引用计数
    mapping: AtomicUsize,   // 关联的 address_space (用于文件映射)
    private: AtomicUsize,   // 私有数据
    lru: ListHead,          // LRU 链表节点
}

// 全局页数组
static PAGE_ARRAY: &[Page] = ...;  // 每个物理页一个 Page

fn pfn_to_page(pfn: usize) -> &'static Page;
fn page_to_pfn(page: &Page) -> usize;
fn virt_to_page(addr: VirtAddr) -> &'static Page;
```

**参考**: Linux `include/linux/mm_types.h` struct page

#### 1.2 完善锁机制
**目标**: 添加细粒度锁，支持 SMP 并发

**改动文件**:
- `kernel/src/mm/pagemap.rs` - 添加 mmap_lock
- `kernel/src/arch/riscv64/mm.rs` - 添加 page_table_lock

**实现内容**:
```rust
pub struct AddressSpace {
    root_ppn: u64,
    vma_manager: VmaManager,
    mmap_lock: RwLock<()>,        // VMA 操作的读写锁
    page_table_lock: SpinLock<()>, // 页表操作的自旋锁
    mm_users: AtomicI32,          // 用户计数 (线程共享)
    mm_count: AtomicI32,          // 引用计数 (mm_struct 生命期)
}
```

#### 1.3 改进 VMA 管理
**目标**: 使用更高效的数据结构，支持动态扩展

**方案 A (推荐)**: 使用 BTreeMap 替代静态数组
```rust
use alloc::collections::BTreeMap;

pub struct VmaManager {
    vmas: BTreeMap<VirtAddr, Vma>,  // 按起始地址排序
    lock: RwLock<()>,
}
```

**方案 B**: 实现 maple tree (Linux 6.1+ 方案)
- 更复杂，但性能更好
- 作为长期目标

---

### Phase 2: 核心功能完善 (优先级: 高)

#### 2.1 完善 Copy-on-Write
**目标**: 完整实现 fork 的 COW 机制

**改动文件**:
- `kernel/src/arch/riscv64/mm.rs` - 完善 `copy_page_table_cow()`
- `kernel/src/arch/riscv64/trap.rs` - 处理 COW 页面错误

**实现要点**:
1. 复制页表时标记可写页面为只读 + COW 标志
2. 使用 `refcount` 跟踪共享页面数
3. 写错误时检查 COW 标志，分配新页面
4. 当 `refcount == 1` 时直接恢复写权限

**参考**: Linux `mm/memory.c` do_wp_page()

```rust
// 页面错误处理伪代码
fn handle_page_fault(addr: VirtAddr, cause: FaultCause) {
    let vma = find_vma(addr)?;

    if cause == WriteFault && is_cow_page(addr) {
        if get_page_refcount(addr) > 1 {
            // 分配新页面，复制内容
            let new_page = alloc_page();
            copy_page_content(old_page, new_page);
            update_page_table(addr, new_page, WRITE);
            decrement_refcount(old_page);
        } else {
            // 只有一个引用，直接恢复写权限
            set_page_writable(addr);
        }
    }
}
```

#### 2.2 完善页面错误处理
**目标**: 支持按需分页 (demand paging)

**改动文件**:
- `kernel/src/arch/riscv64/trap.rs` - 扩展 page fault 处理
- `kernel/src/mm/pagemap.rs` - 添加 handle_mm_fault()

**实现要点**:
1. 解析 scause 判断错误类型 (读/写/执行)
2. 查找 VMA 验证权限
3. 匿名页面: 分配新页，清零
4. 文件页面: 从文件读取
5. 更新页表，设置正确的权限位

**参考**: Linux `mm/memory.c` handle_mm_fault()

```rust
fn handle_mm_fault(mm: &AddressSpace, addr: VirtAddr, flags: FaultFlags) -> Result<()> {
    let vma = mm.find_vma(addr)?;

    // 检查权限
    if flags.contains(FaultFlags::WRITE) && !vma.flags.contains(VmaFlags::WRITE) {
        return Err(FaultError::Permission);
    }

    // 分配或获取页面
    let page = if vma.vma_type == VmaType::Anonymous {
        alloc_zeroed_page()?
    } else {
        read_file_page(vma.file, vma.offset + (addr - vma.start))?
    };

    // 映射页面
    mm.map_page(addr, page, vma.flags.to_pte_flags())?;
    Ok(())
}
```

#### 2.3 实现 mmap 完整功能
**目标**: 支持 MAP_SHARED、MAP_FIXED、MAP_ANONYMOUS 等

**改动文件**:
- `kernel/src/mm/pagemap.rs` - 扩展 mmap 实现
- `kernel/src/arch/riscv64/syscall.rs` - 完善系统调用

**需要支持的标志**:
```rust
pub struct MmapFlags(u32);
impl MmapFlags {
    pub const SHARED: u32    = 0x01;   // 共享映射
    pub const PRIVATE: u32   = 0x02;   // 私有映射 (COW)
    pub const FIXED: u32     = 0x10;   // 强制地址
    pub const ANONYMOUS: u32 = 0x20;   // 匿名映射
    pub const STACK: u32     = 0x20000; // 栈映射
}
```

---

### Phase 3: 性能优化 (优先级: 中)

#### 3.1 实现 Slab 分配器
**目标**: 小对象高效分配，替代直接使用 buddy allocator

**改动文件**:
- `kernel/src/mm/slab.rs` (新建)

**实现方案**:
1. 使用 `linked_list` 管理空闲对象
2. 每个 slab 包含多个相同大小的对象
3. 支持 kmalloc-8, kmalloc-16, ..., kmalloc-4096

**参考**: Linux `mm/slub.c`

```rust
pub struct SlabCache {
    name: &'static str,
    object_size: usize,
    slabs_partial: ListHead,  // 部分使用的 slab
    slabs_full: ListHead,     // 完全使用的 slab
    slabs_free: ListHead,     // 空闲 slab
}

pub fn kmalloc(size: usize, flags: GFPFlags) -> *mut u8;
pub fn kfree(ptr: *mut u8);
```

#### 3.2 实现 Per-CPU Pages
**目标**: 减少 buddy allocator 的锁竞争

**改动文件**:
- `kernel/src/mm/page.rs` - 添加 Per-CPU 缓存

**实现要点**:
```rust
pub struct PerCpuPages {
    lists: [Vec<Page>; MIGRATE_TYPES],  // 每种迁移类型的页链表
    count: usize,                        // 缓存页数
    high: usize,                         // 高水位 (溢出时归还)
    batch: usize,                        // 批量操作数量
}

// 每个 CPU 一个
static PER_CPU_PAGES: PerCpu<PerCpuPages>;
```

#### 3.3 添加 Zone 支持
**目标**: 区分不同类型的内存

**改动文件**:
- `kernel/src/mm/zone.rs` (新建)

```rust
pub enum ZoneType {
    Normal,     // 普通内存
    Movable,    // 可迁移内存
    Device,     // 设备内存
}

pub struct Zone {
    zone_type: ZoneType,
    spanned_pages: usize,
    present_pages: usize,
    free_area: [FreeArea; MAX_ORDER],
}
```

---

### Phase 4: 高级功能 (优先级: 低)

#### 4.1 反向映射 (Reverse Mapping)
**目标**: 从物理页找到所有映射它的虚拟地址

**用途**:
- 页面迁移
- 页面回收
- COW 共享检测

**实现**:
```rust
pub struct AnonVma {
    root: AtomicPtr<AnonVma>,
    degree: AtomicU32,  // 引用度
    parent: *mut AnonVma,
}

impl Page {
    mapping: *mut AddressSpace,  // 对于文件页
    index: u64,                  // 页内偏移
}
```

#### 4.2 LRU 页面回收
**目标**: 内存不足时回收页面

**实现**:
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

#### 4.3 大页支持
**目标**: 支持 2MB/1GB 大页

**改动**:
- 页表项支持 PS (Page Size) 位
- 大页分配器
- hugetlbfs 文件系统

---

## 三、实施优先级和依赖关系

```
Phase 1.1 (struct page) ──┬──► Phase 2.1 (COW)
                          │
Phase 1.2 (锁机制) ───────┼──► Phase 2.2 (页面错误)
                          │
Phase 1.3 (VMA 改进) ─────┴──► Phase 2.3 (mmap)

Phase 1.1 (struct page) ──────► Phase 3.1 (Slab)
                              │
                              ├──► Phase 3.2 (Per-CPU)
                              │
                              └──► Phase 3.3 (Zone)

Phase 2.1 (COW) + Phase 3.1 (Slab) ──► Phase 4.1 (rmap)
                                          │
                                          └──► Phase 4.2 (LRU)

Phase 1.1 + Phase 1.2 ──► Phase 4.3 (大页)
```

---

## 四、预计工作量

| 阶段 | 工作量 | 说明 |
|------|--------|------|
| Phase 1 | 2-3 周 | 基础设施，需要仔细设计 |
| Phase 2 | 2-3 周 | 核心功能，需要大量测试 |
| Phase 3 | 3-4 周 | 性能优化，可选实施 |
| Phase 4 | 4+ 周 | 高级功能，长期目标 |

---

## 五、参考资源

1. **Linux 源码**: `refer/linux/mm/` 目录
2. **RISC-V 规范**: `refer/linux/arch/riscv/mm/`
3. **文档**:
   - Linux `Documentation/mm/`
   - `include/linux/mm.h`
   - `include/linux/mmzone.h`

## 六、注意事项

1. **保持 POSIX 兼容**: 所有改动必须符合 POSIX 标准
2. **不创新原则**: 参考而非"改进" Linux 设计
3. **渐进式开发**: 每个功能独立可测试
4. **测试覆盖**: 每个阶段都需要对应的测试用例
