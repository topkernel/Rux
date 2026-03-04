# Rux 内存管理设计文档

本文档详细描述 Rux 内核内存管理子系统的设计和实现。

**最后更新**：2026-03-04
**代码位置**：`kernel/src/mm/` (~4,300 行代码)
**架构支持**：RISC-V Sv39

---

## 目录

- [概述](#概述)
- [内存布局](#内存布局)
- [物理内存管理](#物理内存管理)
- [虚拟内存管理](#虚拟内存管理)
- [内核堆分配](#内核堆分配)
- [进程地址空间](#进程地址空间)
- [Copy-on-Write](#copy-on-write)
- [内存统计](#内存统计)
- [API 参考](#api-参考)

---

## 概述

### 设计目标

1. **Linux 兼容**：与 Linux 内核 ABI 兼容，支持标准系统调用
2. **高效分配**：多层次分配器，减少内存碎片
3. **SMP 优化**：Per-CPU 缓存减少锁竞争
4. **安全隔离**：内核空间与用户空间严格分离

### 架构层次

```
┌─────────────────────────────────────────────────────────────┐
│                     用户态系统调用                            │
│   brk() / mmap() / munmap() / mprotect() / ...             │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                   进程地址空间管理                            │
│   MmStruct / VmaManager / VMA                              │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                     虚拟内存管理                             │
│   Page Tables (Sv39) / AddressSpace                        │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                     物理内存管理                             │
│   Frame Allocator / Page Descriptor                        │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                     内核堆分配器                             │
│   Buddy Allocator → Slab Allocator → kmalloc/kfree         │
│   Per-CPU Pages (PCP)                                      │
└─────────────────────────────────────────────────────────────┘
```

### 模块组成

| 模块 | 文件 | 行数 | 功能 |
|------|------|------|------|
| **物理页管理** | page.rs | ~250 | 物理地址/帧操作 |
| **页描述符** | page_desc.rs | ~350 | 每页元数据 |
| **Buddy 分配器** | buddy_allocator.rs | ~490 | 内核堆分配 |
| **Slab 分配器** | slab.rs | ~610 | 小对象分配 |
| **Per-CPU 页** | pcp.rs | ~400 | CPU 本地缓存 |
| **VMA 管理** | vma.rs | ~500 | 虚拟内存区域 |
| **地址空间** | mm_struct.rs | ~550 | 进程地址空间 |
| **页表映射** | pagemap.rs | ~70 | 平台无关接口 |
| **RISC-V 页表** | arch/riscv64/mm/ | ~2,000 | Sv39 实现 |
| **内存统计** | meminfo.rs | ~200 | /proc/meminfo |

---

## 内存布局

### 物理内存布局

```
0x0000_0000 ┌─────────────────────────────┐
            │     OpenSBI / Bootloader    │
0x0080_0000 ├─────────────────────────────┤
            │     内核代码段 (.text)       │
0x0080_2000 ├─────────────────────────────┤
            │     内核数据段 (.data)       │
0x0080_4000 ├─────────────────────────────┤
            │     内核 BSS 段 (.bss)       │
0x0080_8000 ├─────────────────────────────┤
            │     内核栈                   │
0x0080_F000 ├─────────────────────────────┤
            │     页描述符数组 (mem_map)   │
0x00A0_0000 ├─────────────────────────────┤
            │     内核堆 (Buddy + Slab)    │
            │     可配置大小 (默认 16MB)   │
0x08A0_0000 ├─────────────────────────────┤
            │     可用物理内存             │
            │     Frame Allocator 管理     │
            │     ~2GB 可用                │
0x8000_0000 └─────────────────────────────┘
```

### 虚拟内存布局 (Sv39)

```
内核空间 (高 256GB)
─────────────────────────────────────────
0xFFFF_0000_0000_0000 ┌───────────────────┐
                       │  内核代码/数据    │
                       │  直接映射区       │
                       │  设备映射区       │
0xFFFF_FFFF_FFFF_FFFF └───────────────────┘

用户空间 (低 256GB)
─────────────────────────────────────────
0x0000_0000_0001_0000 ┌───────────────────┐
                       │  用户代码段       │
                       │  用户数据段       │
0x0000_0000_0100_0000 ├───────────────────┤
                       │  用户堆 (brk)     │
                       │  向上增长         │
0x0000_0000_3000_0000 ├───────────────────┤
                       │  mmap 区域        │
                       │  0x5000_0000 起   │
0x0000_0000_6000_0000 ├───────────────────┤
                       │  共享库           │
0x0000_0000_7FFF_F000 ├───────────────────┤
                       │  用户栈           │
                       │  向下增长         │
0x0000_0000_7FFF_FFFF └───────────────────┘
```

### 地址空间常量

```rust
// 页大小
pub const PAGE_SIZE: usize = 4096;

// 物理内存
pub const PHYS_MEMORY_SIZE: usize = 2 * 1024 * 1024 * 1024;  // 2GB

// 内核虚拟地址基址
pub const KERNEL_VIRT_BASE: usize = 0xFFFF_0000_0000_0000;

// 用户空间范围
pub const USER_VIRT_BASE: usize = 0x0000_0000_1000_0000;
pub const USER_VIRT_TOP: usize = 0x0000_0000_7FFF_FFFF;

// 用户地址布局
pub const BRK_DEFAULT: usize = 0x3000_0000;      // 768MB
pub const MMAP_START: usize = 0x5000_0000;       // 1.25GB
pub const STACK_TOP: usize = 0x7FFF_F000;        // 栈顶
pub const STACK_MAX_SIZE: usize = 8 * 1024 * 1024; // 8MB
```

---

## 物理内存管理

### Frame Allocator

**文件**: `kernel/src/mm/page.rs`

管理物理页帧的分配和释放。

```rust
pub struct FrameAllocator {
    next_free: AtomicUsize,    // 下一个空闲页
    free_list: AtomicUsize,    // 空闲链表头
    total_frames: usize,       // 总页数
    use_page_desc: AtomicUsize, // 是否使用 Page 描述符
}

// 核心接口
pub fn alloc_frame() -> Option<PhysFrame>;
pub fn dealloc_frame(frame: PhysFrame);
```

**特性**：
- 线性分配 + 空闲链表回收
- 原子操作，支持 SMP
- 与 Page 描述符集成

### Page Descriptor

**文件**: `kernel/src/mm/page_desc.rs`

为每个物理页帧维护元数据（类似 Linux `struct page`）。

```rust
#[repr(C, align(64))]
pub struct Page {
    flags: PageFlags,          // 原子标志位
    _mapcount: AtomicI32,      // 映射计数
    _refcount: AtomicI32,      // 引用计数
    private: AtomicUsize,      // 私有数据
    mapping: AtomicUsize,      // address_space 指针
    index: AtomicU64,          // 映射偏移
    _type: AtomicU32,          // 页类型
    next_free: AtomicUsize,    // 空闲链表指针
}
```

**页标志位**：

| 标志 | 说明 |
|------|------|
| `Locked` | 页已锁定 |
| `Dirty` | 页已修改 |
| `Referenced` | 页已被访问 |
| `UpToDate` | 页数据有效 |
| `Lru` | 在 LRU 链表中 |
| `Reserved` | 保留页 |
| `Cow` | 写时复制页 |
| `Anonymous` | 匿名页 |

**全局 mem_map 数组**：

```rust
// 页帧号到 Page 的转换
pub fn pfn_to_page(pfn: usize) -> &'static Page;
pub fn pfn_to_page_mut(pfn: usize) -> &'static mut Page;
pub fn page_to_pfn(page: &Page) -> usize;
```

---

## 虚拟内存管理

### RISC-V Sv39 页表

**文件**: `kernel/src/arch/riscv64/mm/`

Sv39 是 RISC-V 的标准分页模式：

| 特性 | 值 |
|------|-----|
| 虚拟地址位数 | 39 位 |
| 地址空间大小 | 512 GB |
| 页表级数 | 3 级 |
| 每级表项数 | 512 |
| 页大小 | 4 KB |
| PTE 大小 | 8 字节 |

**虚拟地址分解**：

```
┌─────────┬─────────┬─────────┬────────────┐
│  VPN[2] │  VPN[1] │  VPN[0] │  页内偏移   │
│  9 bits │  9 bits │  9 bits │  12 bits   │
└─────────┴─────────┴─────────┴────────────┘
   L2 索引    L1 索引    L0 索引
```

**页表项 (PTE) 格式**：

```
┌────────────────┬──────────────────────────┐
│     PPN        │        Flags             │
│    44 bits     │        10 bits           │
└────────────────┴──────────────────────────┘

Flags:
- V: 有效位
- R: 可读
- W: 可写
- X: 可执行
- U: 用户态可访问
- G: 全局映射
- A: 已访问
- D: 已修改
```

### AddressSpace

**文件**: `kernel/src/arch/riscv64/mm/base.rs`

管理进程的完整页表。

```rust
pub struct AddressSpace {
    pgd: AtomicU64,              // 页表根地址
    page_table_lock: SpinLock<()>, // 页表锁
    mm: Option<Arc<MmStruct>>,   // 关联的 mm_struct
}
```

**核心操作**：

```rust
// 映射页面
pub fn map(&self, vaddr: VirtAddr, paddr: PhysAddr, perm: Perm) -> Result<(), MapError>;

// 解除映射
pub fn unmap(&self, vaddr: VirtAddr) -> Result<PhysAddr, MapError>;

// 修改权限
pub fn protect(&self, vaddr: VirtAddr, perm: Perm) -> Result<(), MapError>;

// 查询物理地址
pub fn translate(&self, vaddr: VirtAddr) -> Option<PhysAddr>;
```

---

## 内核堆分配

### 分配器层次

```
┌─────────────────────────────────────────────┐
│            kmalloc / kzalloc                │
│            (公共分配接口)                    │
└─────────────────────────────────────────────┘
                    │
        ┌───────────┴───────────┐
        ▼                       ▼
┌───────────────┐       ┌───────────────┐
│ Slab 分配器   │       │ Buddy 分配器  │
│ (≤ 4KB 对象)  │       │ (> 4KB 分配)  │
└───────────────┘       └───────────────┘
        │                       │
        └───────────┬───────────┘
                    ▼
            ┌───────────────┐
            │ Per-CPU Pages │
            │ (CPU 本地缓存) │
            └───────────────┘
                    │
                    ▼
            ┌───────────────┐
            │ Frame         │
            │ Allocator     │
            └───────────────┘
```

### Buddy Allocator

**文件**: `kernel/src/mm/buddy_allocator.rs`

伙伴系统分配器，管理内核堆内存。

**特性**：
- 支持 order 0 ~ 20（4KB ~ 4GB）
- 元数据与用户数据分开存储
- O(log n) 分配/释放复杂度
- 自动合并相邻空闲块

```rust
pub struct BuddyAllocator {
    magic: AtomicUsize,           // 魔数检测
    heap_start: AtomicUsize,      // 堆起始地址
    heap_end: AtomicUsize,        // 堆结束地址
    free_lists: [AtomicUsize; MAX_ORDER + 1], // 各 order 空闲链表
    meta: MetaArray,              // 元数据数组
}
```

**分配过程**：

```
1. 根据 size 计算 order
2. 从 free_lists[order] 查找空闲块
3. 如果没有，从更高 order 分割
4. 分割的伙伴块加入对应 order 链表
5. 返回分配的地址
```

**释放过程**：

```
1. 计算块的 order 和 page_idx
2. 查找伙伴块 (buddy_idx = page_idx ^ (1 << order))
3. 如果伙伴空闲且 order 相同，合并
4. 重复直到无法合并
5. 加入对应 order 的空闲链表
```

### Slab Allocator

**文件**: `kernel/src/mm/slab.rs`

小对象分配器，减少内存碎片。

**支持的对象大小**：
```
8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096 字节
```

```rust
pub struct SlabCache {
    object_size: usize,       // 对象大小
    objects_per_slab: usize,  // 每 slab 对象数
    free_list: u16,           // 空闲 slab 链表
    partial_list: u16,        // 部分 slab 链表
    full_list: u16,           // 满 slab 链表
}
```

**Slab 结构**：

```
┌─────────────────────────────────────────────┐
│  SlabHeader (16 bytes)                      │
│  - cache_idx, object_size, total_objects    │
│  - free_objects, free_index, next, prev     │
├─────────────────────────────────────────────┤
│  Object 0  │  Object 1  │  ...  │ Object N  │
│  (固定大小) │  (固定大小) │       │ (固定大小) │
└─────────────────────────────────────────────┘
```

**公共接口**：

```rust
// 分配内存
pub fn kmalloc(size: usize) -> *mut u8;

// 释放内存
pub fn kfree(ptr: *mut u8);

// 分配并清零
pub fn kzalloc(size: usize) -> *mut u8;
```

### Per-CPU Pages (PCP)

**文件**: `kernel/src/mm/pcp.rs`

每 CPU 页缓存，减少全局分配器锁竞争。

```rust
pub struct PerCpuPages {
    lists: [usize; MIGRATE_TYPES],   // 各类型页链表
    counts: [usize; MIGRATE_TYPES],  // 各类型页数
    high: usize,                     // 高水位
    batch: usize,                    // 批量操作数
}
```

**迁移类型**：

| 类型 | 说明 |
|------|------|
| `Unmovable` | 不可移动（内核使用） |
| `Movable` | 可移动（用户空间页） |
| `Reclaimable` | 可回收（可换出） |

**分配流程**：

```
1. 从本地 CPU 缓存分配（无锁）
2. 如果本地缓存为空，批量从全局获取
3. 如果超过高水位，批量归还全局
```

**公共接口**：

```rust
// 分配内核页
pub fn alloc_kernel_page() -> Option<PhysFrame>;

// 分配用户页
pub fn alloc_user_page() -> Option<PhysFrame>;

// 释放页
pub fn free_kernel_page(frame: PhysFrame);
pub fn free_user_page(frame: PhysFrame);
```

---

## 进程地址空间

### MmStruct

**文件**: `kernel/src/mm/mm_struct.rs`

进程地址空间描述符，与 Linux `mm_struct` 对应。

```rust
pub struct MmStruct {
    // 页表管理
    pub pgd: u64,                      // 页表根
    vma_manager: RwLock<VmaManager>,   // VMA 管理器
    space_type: PageTableType,         // 地址空间类型

    // 段范围
    start_code: AtomicUsize,
    end_code: AtomicUsize,
    start_data: AtomicUsize,
    end_data: AtomicUsize,

    // 堆管理
    start_brk: AtomicUsize,
    brk: AtomicUsize,

    // 栈管理
    start_stack: AtomicUsize,

    // 参数和环境变量
    arg_start: AtomicUsize,
    arg_end: AtomicUsize,
    env_start: AtomicUsize,
    env_end: AtomicUsize,

    // 虚拟内存统计
    total_vm: AtomicU64,
    locked_vm: AtomicU64,
    // ...
}
```

### VMA (Virtual Memory Area)

**文件**: `kernel/src/mm/vma.rs`

描述进程地址空间中一个连续区域。

```rust
pub struct Vma {
    start: VirtAddr,         // 起始地址
    end: VirtAddr,           // 结束地址
    flags: VmaFlags,         // 权限标志
    vma_type: VmaType,       // VMA 类型
    offset: u64,             // 文件偏移
    file: Option<Arc<File>>, // 关联文件
}
```

**VMA 标志**：

| 标志 | 说明 |
|------|------|
| `READ` | 可读 |
| `WRITE` | 可写 |
| `EXEC` | 可执行 |
| `SHARED` | 共享映射 |
| `PRIVATE` | 私有映射（COW） |
| `GROWSDOWN` | 向下增长（栈） |

**VMA 类型**：

```rust
pub enum VmaType {
    Anonymous,    // 匿名映射
    File,         // 文件映射
    Stack,        // 栈
    Heap,         // 堆
    Vdso,         // VDSO
}
```

### VmaManager

使用 BTreeMap 管理 VMA，支持快速查找。

```rust
pub struct VmaManager {
    vmas: BTreeMap<VirtAddr, Vma>,
    // ...
}

// 核心操作
impl VmaManager {
    pub fn find_vma(&self, addr: VirtAddr) -> Option<&Vma>;
    pub fn insert(&mut self, vma: Vma) -> Result<(), VmaError>;
    pub fn remove(&mut self, start: VirtAddr) -> Option<Vma>;
    pub fn find_free_area(&self, len: usize, hint: VirtAddr) -> Option<VirtAddr>;
}
```

---

## Copy-on-Write

### COW 机制

当 fork() 创建子进程时，不立即复制物理页，而是共享父进程的页面并标记为只读。当进程尝试写入时触发页故障，内核此时才复制页面。

**实现流程**：

```
fork():
1. 复制父进程的页表
2. 将所有可写页标记为只读
3. 设置 COW 标志位
4. 增加页的引用计数

页故障处理:
1. 检查是否为 COW 页
2. 分配新物理页
3. 复制内容到新页
4. 更新页表映射
5. 设置新页为可写
6. 减少原页引用计数
```

**页标志**：

```rust
// COW 页标志
pub const Cow: u32 = 1 << 14;

// 在 Page 描述符中
page.flags.set(PageFlag::Cow);
```

---

## 内存统计

### MemoryInfo

**文件**: `kernel/src/mm/meminfo.rs`

提供类似 `/proc/meminfo` 的内存统计。

```rust
pub struct MemoryInfo {
    // 物理内存
    pub mem_total: usize,
    pub mem_free: usize,
    pub mem_available: usize,
    pub mem_used: usize,

    // 堆内存
    pub heap_total: usize,
    pub heap_used: usize,
    pub heap_free: usize,

    // Slab
    pub slab_pages: usize,
    pub slab_allocs: usize,
    pub slab_frees: usize,

    // Per-CPU Pages
    pub pcp_pages: [usize; 4],

    // 页状态
    pub pages_free: usize,
    pub pages_used: usize,
    pub pages_reserved: usize,
    pub pages_mapped: usize,
    pub pages_dirty: usize,
    pub pages_cow: usize,
    pub pages_anon: usize,
}
```

**获取方式**：

```rust
// 获取内存统计
let info = get_memory_info();
print_memory_info();

// 获取摘要（用于 procfs）
let summary = get_memory_summary();

// 检查内存压力
if is_memory_low() {
    // 触发内存回收
}

if should_trigger_oom() {
    // 触发 OOM killer
}
```

---

## API 参考

### 内核堆分配

```rust
// 小对象分配（≤ 4KB）
let ptr = kmalloc(128);
kfree(ptr);

// 分配并清零
let ptr = kzalloc(256);

// 大对象分配（> 4KB）
let layout = Layout::from_size_align(8192, 4096).unwrap();
let ptr = HEAP_ALLOCATOR.alloc(layout);
HEAP_ALLOCATOR.dealloc(ptr, layout);
```

### 物理页分配

```rust
// 分配单页
let frame = alloc_frame().expect("out of memory");

// 分配 Per-CPU 页
let frame = alloc_kernel_page();
let frame = alloc_user_page();

// 释放
dealloc_frame(frame);
free_kernel_page(frame);
free_user_page(frame);
```

### 虚拟内存操作

```rust
// 创建地址空间
let space = AddressSpace::new_user();

// 映射页面
space.map(vaddr, paddr, Perm::ReadWrite)?;

// 修改权限
space.protect(vaddr, Perm::Read)?;

// 解除映射
space.unmap(vaddr)?;
```

### 进程地址空间

```rust
// 获取当前进程的 mm
let mm = current_mm()?;

// brk 系统调用
let new_brk = mm.do_brk(addr)?;

// mmap 系统调用
let addr = mm.do_mmap(addr, len, prot, flags, fd, offset)?;

// munmap 系统调用
mm.do_munmap(addr, len)?;
```

---

## 相关文档

- [RISC-V 架构](riscv64.md) - Sv39 页表详细说明
- [进程管理](design.md) - fork/execve 实现
- [测试报告](../tests/unit-test-report.md) - 内存管理测试

---

## 更新日志

- **2026-03-04**: 创建文档
  - 详细描述内存布局
  - 记录各分配器设计
  - 添加 API 参考
