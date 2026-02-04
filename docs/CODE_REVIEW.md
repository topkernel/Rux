# 代码审查记录与修复进度

本文档记录对 Rux 内核代码的全面审查结果，包括发现的设计和实现问题、与 Linux 内核的对比，以及修复进度。

**审查日期**：2025-02-03 至 2025-02-04
**审查范围**：VFS 层、文件系统、内存管理、进程管理、SMP、调试输出、代码质量

---

## 问题列表

### 🔴 严重问题

#### 1. 智能指针不一致 ✅ **已修复**
**文件**：多个文件
**问题描述**：
- 代码中混用 `alloc::sync::Arc` 和自定义的 `SimpleArc`
- 导致符号可见性问题 (`__rust_no_alloc_shim_is_unstable_v2`)

**修复方案**：
- 统一使用 `SimpleArc` 替代所有 `Arc<T>`
- 为 `SimpleArc` 添加 `Deref` trait 实现
- 修改的文件：
  - `collection.rs` - 添加 Deref trait
  - `dentry.rs` - Arc → SimpleArc
  - `inode.rs` - Arc → SimpleArc
  - `file.rs` - Arc → SimpleArc
  - `mount.rs` - Arc<VfsMount> → SimpleArc<VfsMount>
  - `rootfs.rs` - Arc → SimpleArc
  - `syscall.rs` - File creation with SimpleArc
  - `sched.rs` - File creation with SimpleArc

**状态**：✅ 已完成（2025-02-03）
**Commit**：`统一使用 SimpleArc`

---

#### 2. 全局可变状态无同步保护 ✅ **已修复**
**文件**：`kernel/src/fs/rootfs.rs`
**问题描述**：
```rust
// 之前：不安全，无同步保护
static mut GLOBAL_ROOTFS_SB: Option<*mut RootFSSuperBlock> = None;
static mut GLOBAL_ROOT_MOUNT: Option<*mut VfsMount> = None;
```

**对比 Linux**：
- Linux 使用 `spin_lock_t` 或 RCU 保护全局状态
- 使用 `atomic_long_t` 或 `atomic_ptr_t` 进行原子访问

**修复方案**：
- 使用 `AtomicPtr` 替代 `static mut`
- 添加 acquire/release 内存排序
```rust
// 之后：使用 AtomicPtr 保护
static GLOBAL_ROOTFS_SB: AtomicPtr<RootFSSuperBlock> = AtomicPtr::new(core::ptr::null_mut());
static GLOBAL_ROOT_MOUNT: AtomicPtr<VfsMount> = AtomicPtr::new(core::ptr::null_mut());

pub fn get_rootfs_sb() -> Option<*mut RootFSSuperBlock> {
    let ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    if ptr.is_null() { None } else { Some(ptr) }
}
```

**状态**：✅ 已完成（2025-02-03）
**Commit**：`fs/rootfs: Add synchronization protection for global state`

---

#### 3. MaybeUninit 未定义行为 ✅ **已修复**
**文件**：`kernel/src/fs/file.rs`
**问题描述**：
```rust
// 之前：未定义行为
let fds: [Option<SimpleArc<File>>; 1024] = unsafe {
    MaybeUninit::uninit().assume_init()
};
```

**修复方案**：
```rust
// 之后：安全的初始化
let fds: [Option<SimpleArc<File>>; 1024] = core::array::from_fn(|_| None);
```

**状态**：✅ 已完成（2025-02-03）

---

### 🟡 中等问题

#### 4. VFS 函数指针安全性问题 ✅ **已修复 (2025-02-04)**
**文件**：`kernel/src/fs/file.rs`
**问题描述**：
```rust
// 之前：使用裸指针 + unsafe fn
pub struct FileOps {
    pub read: Option<unsafe fn(*mut File, *mut u8, usize) -> isize>,
    pub write: Option<unsafe fn(*mut File, *const u8, usize) -> isize>,
}
```

**修复方案**：
```rust
// 之后：使用引用 + 切片
pub struct FileOps {
    pub read: Option<fn(&File, &mut [u8]) -> isize>,
    pub write: Option<fn(&File, &[u8]) -> isize>,
    pub lseek: Option<fn(&File, isize, i32) -> isize>,
    pub close: Option<fn(&File) -> i32>,
}
```

**优点**：
- ✅ 使用引用替代裸指针 → 编译器保证非空
- ✅ 使用切片替代 (ptr, len) → 防止缓冲区溢出
- ✅ 移除 unsafe fn → 更安全
- ✅ 零成本抽象 → 无性能损失
- ✅ 保持 Linux 兼容 → 函数指针表模式

**修改的文件**：
- `kernel/src/fs/file.rs` - FileOps 定义和 reg_file_* 函数
- `kernel/src/fs/inode.rs` - INodeOps 定义
- `kernel/src/arch/aarch64/syscall.rs` - pipe_file_* 函数
- `kernel/src/process/sched.rs` - uart_file_* 函数
    // ...
}
```

**状态**：⏳ 待修复
**优先级**：中等（当前可工作，但不够安全）

---

#### 5. RootFS::write_data 不尊重 offset ⏳ **待修复**
**文件**：`kernel/src/fs/rootfs.rs:173`
**问题描述**：
```rust
pub fn write_data(&mut self, offset: usize, data: &[u8]) -> usize {
    // ...
    *existing_data = data.to_vec();  // 忽略了 offset！
    data.len()
}
```

**正确行为**（Linux fs/read_write.c）：
```rust
// 应该在 offset 位置写入，而不是替换整个文件
if offset > existing_data.len() {
    // 需要扩展文件
    existing_data.resize(offset, 0);
}
existing_data.splice(offset..offset, data);
```

**状态**：⏳ 待修复
**影响**：文件写入功能不正确

---

#### 6. 缺少 dentry/inode 缓存机制 ✅ **已修复 (2025-02-04)**
**文件**：`kernel/src/fs/dentry.rs`, `kernel/src/fs/inode.rs`, `kernel/src/fs/rootfs.rs`

**对比 Linux**：
- Linux 使用哈希表加速 dentry 查找 (`dentry_hashtable`)
- Linux 使用 inode 哈希表和 LRU 列表 (`inode_hashtable`, `inode_lru`)
- 显著提升路径解析性能

**修复方案**：

1. **Dentry 缓存 (dcache)** - `fs/dentry.rs`
   - 实现了 256-bucket 哈希表
   - 使用 FNV-1a 哈希算法
   - 支持 `dcache_lookup()`, `dcache_add()`, `dcache_remove()`
   - 线程安全（使用 Mutex 保护）

2. **Inode 缓存 (icache)** - `fs/inode.rs`
   - 实现了 256-bucket 哈希表
   - 使用 FNV-1a 哈希算法
   - 支持 `icache_lookup()`, `icache_add()`, `icache_remove()`
   - 缓存统计功能

3. **RootFS 路径缓存** - `fs/rootfs.rs`
   - RootFS 专用的路径缓存（不使用 Dentry/Inode）
   - 256-bucket 哈希表
   - 命中/未命中统计
   - 集成到 `RootFSSuperBlock::lookup()`

**状态**：✅ 已完成（2025-02-04）
**Commit**：`feat: 为 RootFS 实现路径缓存机制`
**优先级**：中等（功能正确，但性能不佳）

---

#### 7. SimpleArc 缺少 Clone 导致功能不完整 ⏳ **待修复**
**文件**：多个文件中的 TODO 注释

**影响的方法**：
```rust
// rootfs.rs:108 - find_child 无法返回克隆的引用
pub fn find_child(&self, name: &[u8]) -> Option<SimpleArc<RootFSNode>> {
    // TODO: SimpleArc 需要实现 clone
    None
}

// rootfs.rs:119 - list_children 无法返回克隆的列表
pub fn list_children(&self) -> Vec<SimpleArc<RootFSNode>> {
    // TODO: SimpleArc 需要实现 Vec clone
    Vec::new()
}

// rootfs.rs:192 - get_root 无法克隆根节点
pub fn get_root(&self) -> Option<SimpleArc<RootFSNode>> {
    // TODO: SimpleArc 需要实现 clone
    None
}
```

**SimpleArc 已有 Clone 实现**：
```rust
// collection.rs:390
impl<T> Clone for SimpleArc<T> {
    fn clone(&self) -> Self {
        self.inc_ref();
        SimpleArc { ptr: self.ptr }
    }
}
```

**问题根源**：
- Clone trait 已实现，但某些地方可能无法正确调用
- 可能是借用检查器问题

**状态**：⏳ 待修复
**优先级**：高（影响多个文件系统操作）

---

### 🔴 严重问题 (新增)

#### 10. 过多的调试输出严重影响性能 ⏳ **待修复**
**文件**：多个文件 (50+ 处)
**问题描述**：
- 大量使用 `putchar()` 进行逐字符输出
- 每次字符输出都需要 UART 访问，极其缓慢
- 调试信息混杂在正常代码中

**影响示例**：
```rust
// boot.rs - 使用循环逐字符输出
const MSG_MMU: &[u8] = b"MM: Enabling MMU...\n";
for &b in MSG_MMU {
    unsafe { putchar(b); }
}

// 多个文件都有类似的低效输出
// 至少 50+ 处这样的代码
```

**对比 Linux**：
- Linux 使用 `printk()` 带日志级别
- 生产构建中可以完全禁用调试输出
- 使用缓冲 I/O 而非逐字符输出

**修复方案**：
```rust
// 1. 使用已有的 println!/debug_println! 宏
// 2. 添加条件编译
#[cfg(debug_assertions)]
debug_println!("MM: Enabling MMU...");

// 3. 使用批量输出
println!("MM: Enabling MMU...");

// 4. 移除不必要的调试输出
```

**受影响文件**（部分）：
- `kernel/src/arch/aarch64/boot.rs` (10+ 处)
- `kernel/src/drivers/intc/gicv3.rs` (15+ 处)
- `kernel/src/arch/aarch64/ipi.rs` (8+ 处)
- `kernel/src/mm/heap.rs` (6+ 处)
- 其他多处

**状态**：⏳ 待修复
**优先级**：**高**（严重影响性能和代码可读性）

---

#### 11. ~~内存分配器无法释放内存~~ ✅ **已修复 (2025-02-04)**
**文件**：`kernel/src/mm/buddy_allocator.rs`（已实现）
**修复方案**：
实现了完整的 Buddy System（伙伴系统）内存分配器：

```rust
// BlockHeader - 块元数据
struct BlockHeader {
    order: u32,      // 块大小等级 (2^order * PAGE_SIZE)
    free: u32,       // 是否空闲
    prev: usize,     // 前驱指针
    next: usize,     // 后继指针
}

// 核心算法
impl BuddyAllocator {
    // 分配：从空闲链表查找，必要时分割大块
    fn alloc_blocks(&self, order: usize) -> *mut u8;

    // 释放：将块归还到空闲链表，与伙伴合并
    fn free_blocks(&self, block_ptr: *mut u8, order: usize);

    // 伙伴查找：计算块的伙伴地址
    fn get_buddy(&self, block_ptr: usize, order: usize) -> usize;
}
```

**特性**：
- ✅ 支持 O(log n) 分配/释放
- ✅ 伙伴合并机制减少碎片
- ✅ 基于 4KB 页面的块分配
- ✅ 线程安全（原子操作）
- ✅ 最大支持 4GB 内存块 (order 20)

**对比 Linux**：
- 与 Linux mm/page_alloc.c 中的伙伴系统实现一致
- 使用相同的算法和数据结构

**状态**：✅ 已完成
**测试**：✅ 通过所有测试（SimpleVec、SimpleBox、SimpleString、SimpleArc、Fork）

---

#### 12. 全局单队列调度器限制多核扩展 ✅ **已修复 (2025-02-04)**
**文件**：`kernel/src/process/sched.rs`
**问题描述**：
```rust
// 全局运行队列 - 多核瓶颈
pub static mut RQ: RunQueue = RunQueue {
    tasks: [core::ptr::null_mut(); MAX_TASKS],
    current: core::ptr::null_mut(),
    nr_running: 0,
    idle: core::ptr::null_mut(),
};
```

**对比 Linux**：
- Linux 使用 per-CPU 运行队列（`struct rq`）
- 每个 CPU 有自己的任务队列
- 减少锁竞争，提高并行性

**性能问题**：
- 所有 CPU 必须访问同一个全局队列
- 需要全局锁，严重限制多核性能
- 无法实现真正的并行调度

**修复方案**（已实现）：
```rust
// Per-CPU 运行队列
static mut PER_CPU_RQ: [Option<Mutex<RunQueue>>; MAX_CPUS] =
    [None, None, None, None];

pub fn this_cpu_rq() -> Option<&'static Mutex<RunQueue>> {
    unsafe {
        let cpu_id = crate::arch::aarch64::boot::get_core_id() as usize;
        if cpu_id >= MAX_CPUS {
            return None;
        }
        PER_CPU_RQ[cpu_id].as_ref()
    }
}

pub fn cpu_rq(cpu_id: usize) -> Option<&'static Mutex<RunQueue>> {
    unsafe {
        if cpu_id >= MAX_CPUS {
            return None;
        }
        PER_CPU_RQ[cpu_id].as_ref()
    }
}

pub fn init_per_cpu_rq(cpu_id: usize) {
    // 初始化指定 CPU 的运行队列
}
```

**实施细节**：
- ✅ 全局 RQ 改为 per-CPU 数组（PER_CPU_RQ[4]）
- ✅ 实现 this_cpu_rq() - 获取当前 CPU 的运行队列
- ✅ 实现 cpu_rq(cpu_id) - 获取指定 CPU 的运行队列
- ✅ 实现 init_per_cpu_rq(cpu_id) - 初始化 per-CPU 队列
- ✅ 次核调度器自动初始化（在 secondary_cpu_start 中调用）
- ✅ schedule() 使用 this_cpu_rq()
- ⏳ 负载均衡机制（待 Phase 9 实现）

**状态**：✅ 已完成（2025-02-04）
**优先级**：**高**（SMP 扩展的关键障碍）
**Commit**：`优化启动顺序：GIC 提前，次核初始化完善`

**待完成优化**（Phase 9）：
- 负载均衡机制（任务迁移）
- 负载检测算法

---

#### 13. Task 结构体过大 ⏳ **待优化**
**文件**：`kernel/src/process/task.rs`
**问题描述**：
```rust
pub struct Task {
    pub pid: usize,           // 8 bytes
    pub state: TaskState,     // 1 byte + padding
    pub context: CpuContext,  // 312 bytes (27 * 8 + padding)
    pub page_table: *mut u8,  // 8 bytes
    pub heap: Option<Heap>,   // 可能 16+ bytes
    pub stack: Option<TaskStack>, // 16+ bytes
    // ... 总计 660+ bytes
}
```

**对比 Linux**：
- Linux `struct task_struct` 约 1.6KB（但包含更多功能）
- 使用 slab 分配器管理（task_struct slab）
- 分开存储冷热数据

**性能影响**：
- 每次创建任务都需要分配大量内存
- 缓存不友好
- 上下文切换时需要复制更多数据

**优化方案**：
```rust
// 1. 分离冷热数据
pub struct Task {
    // 热数据（频繁访问）
    pub pid: usize,
    pub state: TaskState,
    pub context: CpuContext,

    // 冷数据（不频繁访问）
    pub metadata: *mut TaskMetadata,
}

// 2. 使用 Arc 共享只读数据
// 3. 优化 CpuContext 布局
```

**状态**：⏳ 待优化
**优先级**：中等

---

### 🟡 中等问题 (新增)

#### 14. 不一致的命名约定 ⏳ **待修复**
**文件**：多个文件
**问题描述**：
- 混用下划线和驼峰命名
- 函数名风格不统一

**示例**：
```rust
// kernel/src/drivers/intc/gicv3.rs
pub fn send_ipi_sgir()  // 下划线
pub fn initGIC()         // 驼峰（不一致！）

// kernel/src/arch/aarch64/smp.rs
pub fn boot_secondary_cpus()  // 下划线
pub fn getCoreID()             // 驼峰（不一致！）
```

**对比 Linux**：
- Linux 统一使用 `snake_case` 命名函数和变量
- 结构体使用 `snake_case`（C 风格）

**修复方案**：
- 统一使用 Rust 约定：函数/变量用 `snake_case`，类型用 `PascalCase`
- 运行 `rustfmt` 自动格式化

**状态**：⏳ 待修复
**优先级**：低（不影响功能，影响可读性）

---

#### 15. IPI 发送测试代码未清理 ⏳ **待清理**
**文件**：`kernel/src/main.rs:133-142`
**问题描述**：
```rust
// IPI 发送测试代码应该在测试后移除
unsafe {
    debug_println!("Sending IPI from CPU {} to CPU 1...", cpu_id);
    // 发送 SGI 到 CPU 1
    let sgir: u64 = (1 << 16) | 1;  // Target CPU 1, SGI #1
    core::arch::asm!(
        "msr sgi1r_el1, {}",
        in(reg) sgir,
        options(nomem, nostack)
    );
    debug_println!("IPI sent via ICC_SGI1R_EL1");
}
```

**建议**：
- 移到专门的测试模块
- 或通过配置选项控制
- 不应出现在生产代码中

**状态**：⏳ 待清理
**优先级**：低

---

### 🟢 低优先级问题

#### 8. CpuContext 混合内核和用户寄存器 ⏳ **待优化**
**文件**：`kernel/src/process/context.rs`

**问题描述**：
- 当前使用同一个结构体保存内核和用户寄存器
- 不符合 Linux 的分离设计

**对比 Linux**：
- Linux 使用 `struct pt_regs` 保存用户寄存器
- 内核寄存器直接使用栈或特殊寄存器
- 清晰分离不同特权级的上下文

**建议**：
```rust
// 分离内核和用户上下文
pub struct KernelContext {
    // 内核态寄存器
    x19_x30: [u64; 12],  // x19-x30 (callee-saved)
    sp_el1: u64,
}

pub struct UserContext {
    // 用户态寄存器
    x0_x18: [u64; 19],  // x0-x18
    sp_el0: u64,
    elr_el1: u64,
    spsr_el1: u64,
}
```

**状态**：⏳ 待优化
**优先级**：低（当前可工作）

---

#### 9. 路径解析不完整 ✅ **已完成 (2025-02-04)**
**文件**：`kernel/src/fs/path.rs`, `kernel/src/fs/rootfs.rs`

**已完成功能**：
- ✅ 路径规范化 (`path_normalize`)
  - 移除多余的 `/`
  - 处理 `.` (当前目录)
  - 处理 `..` (父目录)
  - 支持绝对路径和相对路径
- ✅ RootFS::lookup() 集成路径规范化
- ✅ 符号链接解析 (`follow_link`)
  - 创建符号链接
  - 读取符号链接目标
  - 自动跟随符号链接
  - 循环检测（MAX_SYMLINKS = 40）
- ✅ 完整的单元测试覆盖

**待完成功能**：
- ⏳ 相对路径完整支持（需要当前工作目录）

**对比 Linux**：
- Linux 使用 `__link_path_walk` 处理复杂路径
- 支持符号链接跟随、循环检测
- 完整的路径规范化

**状态**：✅ 已完成（主要功能已完成）
**优先级**：中等
**Commit**：`feat: 实现路径规范化功能`, `feat: 实现符号链接支持`

---

#### 10. 文件系统操作不完整 ✅ **已完成 (2025-02-04)**
**文件**：`kernel/src/fs/rootfs.rs`

**已完成功能**：
- ✅ mkdir() - 创建目录
  - 规范化路径
  - 检查父目录存在性
  - 分配新的 inode ID
- ✅ unlink() - 删除文件
  - 检查目标不是目录
  - 从父目录中移除
- ✅ rmdir() - 删除目录
  - 检查目标是目录
  - 验证目录为空
  - 从父目录中移除
- ✅ RootFSNode 方法完善
  - add_child() - 修复 TODO，正确实现
  - remove_child() - 删除子节点
  - rename_child() - 重命名子节点
- ✅ SimpleArc 增强
  - 添加 as_ptr() 方法

**待完成功能**：
- ⏳ rename() - 完整实现（需要重新创建节点）

**对比 Linux**：
- Linux `fs/namei.c` - vfs_mkdir(), vfs_unlink(), vfs_rmdir(), vfs_rename()
- Linux `include/linux/fs.h` - inode_operations

**状态**：✅ 基本完成（主要功能已实现）
**优先级**：中等
**Commit**：`feat: 实现 RootFS 文件系统操作功能`

---

## 修复优先级

### 🔥 严重优先级（影响系统稳定性）
1. ~~**内存分配器无法释放内存**~~ ✅ **已修复 (2025-02-04)** - Buddy System 实现
2. ~~**全局单队列调度器**~~ ✅ **已修复 (2025-02-04)** - Per-CPU 运行队列实现
3. ~~**过多的调试输出**~~ ✅ **已修复 (2025-02-04)** - 已清理 50+ 处

### 高优先级（影响正确性）
4. ~~**SimpleArc Clone 问题**~~ ✅ **已修复 (2025-02-04)** - collection.rs 已实现 Clone trait
5. ~~**RootFS::write_data offset bug**~~ ✅ **已修复 (2025-02-04)** - 支持从 offset 写入

### 中优先级（影响安全性）
6. ~~**VFS 函数指针安全性**~~ ✅ **已修复 (2025-02-04)** - 使用引用和切片替代裸指针
7. ⏳ **Dentry/Inode 缓存** - 性能问题

### 低优先级（代码质量）
8. ⏳ **Task 结构体过大** - 内存和性能优化
9. ⏳ **命名约定不一致** - 代码可读性
10. ⏳ **IPI 测试代码清理** - 移除临时测试代码
11. ⏳ **CpuContext 分离** - 代码组织问题
12. ⏳ **路径解析完善** - 功能完整性

---

## 已完成的修复总结

### 2025-02-03
- ✅ **统一使用 SimpleArc** - 解决符号可见性问题
- ✅ **全局状态同步保护** - 使用 AtomicPtr 替代 static mut
- ✅ **MaybeUninit UB 修复** - 使用 from_fn 安全初始化数组

### 2025-02-04
- ✅ **Buddy System 内存分配器** - 完整实现支持内存释放和伙伴合并
- ✅ **全面代码审查** - 发现并记录 15 个问题
- ✅ **SMP 基础支持完成** - 双核启动、GIC 初始化、IPI 机制
- ✅ **清理调试输出** - 清理 50+ 处调试输出
- ✅ **Per-CPU 运行队列** - 实现多核独立调度
  - per-CPU 数组（PER_CPU_RQ[4]）
  - this_cpu_rq() / cpu_rq() 访问函数
  - 次核自动初始化
- ✅ **启动顺序优化** - 参考 Linux 内核
  - GIC 初始化提前到 scheduler/VFS 之前
  - 次核完善初始化（runqueue、栈、IRQ）
  - 创建 BOOT_SEQUENCE.md 文档
- ✅ **Phase 8 快速胜利完成** - 文件系统关键修复
  - SimpleArc Clone 支持（collection.rs 已实现）
  - RootFS::find_child() 修复 - 使用 SimpleArc::clone()
  - RootFS::list_children() 修复 - 实现正确的子节点克隆
  - RootFS::write_data() offset bug 修复 - 支持从 offset 写入
- ✅ **VFS 函数指针安全性优化** - 使用引用和切片替代裸指针
  - FileOps 和 INodeOps 改进
  - 移除不必要的 unsafe fn
  - 更新所有实现（reg、pipe、uart）
  - 零成本抽象，保持 Linux 兼容
- ✅ **负载均衡机制** - 完善 SMP 多核调度
  - 实现 rq_load() - 负载检测函数
  - 实现 find_busiest_cpu() - 查找最繁忙 CPU
  - 实现 steal_task() - 任务迁移函数
  - 实现 load_balance() - 负载均衡主函数
  - 集成到 schedule() 调度器
  - 参考 Linux kernel/sched/fair.c
- ✅ **信号交付机制** - 完善信号处理闭环 ✅ 已完成 (2025-02-04)
  - 改进 setup_frame() - 保存上下文到信号帧
  - 改进 restore_sigcontext() - 正确恢复上下文
  - 添加 UContext.uc_pc - 保存原始返回地址
  - 添加 Task.sigframe_addr 和 sigframe - 信号帧管理
  - 参考 Linux arch/arm64/kernel/signal.c
- ✅ **信号处理机制改进** - 完善信号发送和处理
  - 添加 SigInfo 结构 - 带附加信息的信号
  - 添加 SigQueue - 信号队列（head/tail 指针）
  - 实现 sigqueue() - 发送带 siginfo 的信号
  - 实现 sigprocmask() - 信号掩码操作（SIG_BLOCK/SIG_UNBLOCK/SIG_SETMASK）
  - 实现 rt_sigaction() - 信号处理函数设置
  - 更新 sys_sigaction 使用 rt_sigaction
  - 更新 sys_rt_sigprocmask 使用 sigprocmask
  - 参考 Linux kernel/signal.c

---

## 下一步修复计划

### 🔴 P0 - 高优先级（影响正确性）

~~1. **SimpleArc Clone 支持** (1-2 天)~~ ✅ **已完成 (2025-02-04)**
   - collection.rs 已实现 Clone trait
   - 修复文件系统操作返回 None 的问题

~~2. **RootFS write_data offset bug** (0.5-1 天)~~ ✅ **已完成 (2025-02-04)**
   - 已修复 write_data() 函数
   - 支持从 offset 开始写入

### 🟡 P1 - 中优先级（优化和安全）

~~3. **VFS 函数指针安全性** (2-3 天)~~ ✅ **已完成 (2025-02-04)**
   - 使用引用和切片替代裸指针
   - FileOps 和 INodeOps 改进
   - 更新所有实现（reg、pipe、uart）

4. **Dentry/Inode 缓存** (2-3 天)
   - 实现哈希表缓存
   - LRU 淘汰策略

### 🟢 P2 - 低优先级（代码质量）

~~5. **负载均衡机制** (Phase 9)~~ ✅ **已完成 (2025-02-04)**
   - 任务迁移算法
   - 负载检测
   - 实现 load_balance() 函数
   - 集成到 schedule() 调度器

6. **Task 结构体优化**
7. **命名约定统一**
8. **IPI 测试代码清理**
   - 实现负载均衡机制
   - 消除多核性能瓶颈

2. **修复 SimpleArc Clone 支持**
   - 修改全局 RQ 为 per-CPU 数组
   - 实现负载均衡机制

---

## 参考资源

---

## 参考资源

- Linux 内核源码：https://elixir.bootlin.com/linux/latest/source/
  - `fs/dcache.c` - Dentry 缓存实现
  - `fs/inode.c` - Inode 管理
  - `fs/read_write.c` - 文件读写操作
  - `include/linux/fs.h` - VFS 数据结构
  - `include/linux/dcache.h` - Dentry 定义
- POSIX 标准：https://pubs.opengroup.org/onlinepubs/9699919799/

---

**文档版本**：v0.1.0
**最后更新**：2025-02-03
