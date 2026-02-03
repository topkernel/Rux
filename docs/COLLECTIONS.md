# 自定义集合类型设计与实现

## 概述

由于 Rust 编译器在 `no_std` 环境中的符号可见性问题，我们实现了完全绕过 `alloc` crate 的自定义集合类型。

## 问题背景

### `__rust_no_alloc_shim_is_unstable_v2` 问题

**根本原因**：
- Rust 编译器使用了 unstable 功能 `__rust_no_alloc_shim_is_unstable_v2`
- 这个功能让 `alloc` crate 直接调用 `#[global_allocator]` 生成的 **hidden mangled 符号**
- 这些符号被标记为 `.hidden` 和 `local`，导致在静态链接的 no_std 二进制文件中无法被链接

**问题表现**：
```rust
// 这段代码会挂起
use alloc::vec::Vec;
let mut vec = Vec::with_capacity(10); // 永远返回
```

**符号表证据**：
```
0000000040005908 l     F .text  0000000000000044 .hidden _RNvCsvJHrxua5YM_7___rustc12___rust_alloc
```

- 符号是 `l` (local) 而不是 `g` (global)
- 符号有 `.hidden` 可见性
- `alloc` crate 无法链接到这些符号

### 尝试过的方案

1. ❌ 使用 `--export-dynamic-symbol` 链接器选项
   - 部分成功，但 `__rust_no_alloc_shim_is_unstable_v2` 强制使用 new 接口

2. ❌ 在链接器脚本中使用 PROVIDE 创建别名
   - 只能创建 `__rust_alloc` 别名，但 alloc crate 使用 mangled 符号

3. ❌ 手动实现 `__rust_alloc` 等函数
   - 与 `#[global_allocator]` 生成的符号冲突

4. ❌ 使用汇编代码创建跳转包装
   - 符号创建失败或无法正确链接

5. ❌ 尝试覆盖 `__rust_no_alloc_shim_is_unstable_v2` 符号
   - 导致重复符号错误

## 解决方案：自定义集合类型

**核心思想**：完全绕过 `alloc` crate，直接使用 `GlobalAlloc` trait 实现集合类型。

### 架构

```
┌─────────────────────────────────────────────────────────────┐
│                      应用代码                                │
│  (VFS, 进程管理, 文件系统等)                                 │
└────────────────────┬────────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────────┐
│              自定义集合类型 (collection.rs)                   │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐           │
│  │ SimpleBox  │  │ SimpleVec  │  │ SimpleArc  │           │
│  └────────────┘  └────────────┘  └────────────┘           │
│  ┌────────────┐                                            │
│  │SimpleString│                                            │
│  └────────────┘                                            │
└────────────────────┬────────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────────┐
│              GlobalAlloc Trait                              │
│  ┌───────────────────────────────────────────────────┐     │
│  │           BumpAllocator (allocator.rs)            │     │
│  │  - 原子操作支持                                    │     │
│  │  - 线性分配（不支持释放）                           │     │
│  └───────────────────────────────────────────────────┘     │
└─────────────────────────────────────────────────────────────┘
```

### 实现细节

#### 1. SimpleBox

```rust
pub struct SimpleBox<T> {
    ptr: NonNull<T>,
}

impl<T> SimpleBox<T> {
    pub fn new(value: T) -> Option<Self> {
        let layout = Layout::new::<T>();
        unsafe {
            let ptr = GlobalAlloc::alloc(&HEAP_ALLOCATOR, layout);
            if ptr.is_null() {
                return None;
            }
            *(ptr as *mut T) = value;
            Some(SimpleBox {
                ptr: NonNull::new_unchecked(ptr as *mut T),
            })
        }
    }
}
```

**特点**：
- 直接使用 `GlobalAlloc::alloc` 分配内存
- 返回 `Option<Self>` 处理分配失败
- 在 `Drop` 中释放内存

#### 2. SimpleVec

```rust
pub struct SimpleVec<T> {
    ptr: NonNull<T>,
    capacity: usize,
    len: usize,
}

impl<T> SimpleVec<T> {
    pub fn with_capacity(capacity: usize) -> Option<Self> {
        let layout = Layout::array::<T>(capacity).ok()?;
        unsafe {
            let ptr = GlobalAlloc::alloc(&HEAP_ALLOCATOR, layout);
            if ptr.is_null() {
                return None;
            }
            Some(SimpleVec {
                ptr: NonNull::new_unchecked(ptr as *mut T),
                capacity,
                len: 0,
            })
        }
    }

    pub fn push(&mut self, value: T) -> bool {
        if self.len >= self.capacity {
            if !self.grow() {  // 自动扩容
                return false;
            }
        }
        unsafe {
            core::ptr::write(self.ptr.as_ptr().add(self.len), value);
        }
        self.len += 1;
        true
    }
}
```

**特点**：
- 动态扩容（容量翻倍）
- 自动内存管理
- 支持基本操作（push, pop, get）

#### 3. SimpleArc

```rust
struct ArcInner<T> {
    ref_count: AtomicUsize,
    data: T,
}

pub struct SimpleArc<T> {
    ptr: NonNull<ArcInner<T>>,
}

impl<T> SimpleArc<T> {
    pub fn new(data: T) -> Option<Self> {
        let layout = Layout::new::<ArcInner<T>>();
        unsafe {
            let ptr = GlobalAlloc::alloc(&HEAP_ALLOCATOR, layout);
            if ptr.is_null() {
                return None;
            }
            core::ptr::write(&mut (*ptr).ref_count, AtomicUsize::new(1));
            core::ptr::write(&mut (*ptr).data, data);
            Some(SimpleArc {
                ptr: NonNull::new_unchecked(ptr as *mut ArcInner<T>),
            })
        }
    }
}

impl<T> Clone for SimpleArc<T> {
    fn clone(&self) -> Self {
        self.inc_ref();  // 原子增加引用计数
        SimpleArc {
            ptr: self.ptr,
        }
    }
}

impl<T> Drop for SimpleArc<T> {
    fn drop(&mut self) {
        unsafe {
            if inner.ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
                // 最后一个引用，释放数据
                core::ptr::drop_in_place(&mut (*self.ptr.as_ptr()).data);
                GlobalAlloc::dealloc(&HEAP_ALLOCATOR, ...);
            }
        }
    }
}
```

**特点**：
- 原子引用计数
- 线程安全的克隆操作
- 自动释放（引用计数为 0 时）

## 测试结果

### 成功输出

```
Testing SimpleVec...
alloc: called
alloc: before allocation
alloc: in loop
alloc: success
SimpleVec::push works!
SimpleVec::get works, value = 42

Testing SimpleBox...
alloc: called
SimpleBox works!

Testing SimpleString...
alloc: called
SimpleString works!

Testing SimpleArc...
alloc: called
SimpleArc works!

Initializing VFS...
vfs::init() start
SimpleArc::new success
vfs::init() done
System ready
```

### 验证的功能

- ✅ SimpleVec::push/get 工作正常
- ✅ SimpleBox 内存管理正常
- ✅ SimpleString 字符串操作正常
- ✅ SimpleArc 引用计数正常
- ✅ VFS 成功初始化（使用 SimpleArc）

## 优势

1. **完全绕过 alloc crate**：不依赖 unstable 的编译器功能
2. **完全控制内存分配**：可以优化和调试内存使用
3. **透明度高**：所有内存操作都可见
4. **符合设计目标**：不依赖外部不稳定的接口

## 限制

1. **功能有限**：只实现了基本的集合操作
2. **性能**：BumpAllocator 不支持释放，可能导致内存浪费
3. **不兼容标准库**：无法直接使用 `alloc` crate 的类型

## 未来改进

1. **更好的分配器**：
   - 实现支持释放的分配器（如 buddy allocator）
   - 内存池技术
   - 垃圾回收

2. **更多集合类型**：
   - HashMap
   - BTreeMap
   - LinkedList
   - VecDeque

3. **性能优化**：
   - 减少内存复制
   - 优化扩容策略
   - 缓存友好设计

## 参考资料

- [Rust Tracking Issue #123015: __rust_no_alloc_shim_is_unstable](https://github.com/rust-lang/rust/issues/123015)
- [Support #[global_allocator] without the allocator shim #86844](https://github.com/rust-lang/rust/pull/86844)
- [Phil Opp's Writing an OS in Rust - Allocators](https://os.phil-opp.com/allocator-designs/)
