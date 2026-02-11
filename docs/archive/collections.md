# 集合类型迁移记录

## 历史背景

**最后更新**：2025-02-09
**状态**：✅ 已迁移到标准 `alloc` crate

---

## 早期问题（已解决）

在 Rust 早期版本中，由于 `__rust_no_alloc_shim_is_unstable_v2` 的符号可见性问题，我们曾实现自定义集合类型（SimpleArc、SimpleVec、SimpleBox、SimpleString）来绕过 `alloc` crate。

**当时的问题**：
- Rust 编译器使用了 unstable 功能
- `alloc` crate 直接调用 hidden mangled 符号
- 在静态链接的 `no_std` 二进制文件中无法链接

**尝试过的方案**（均失败）：
1. ❌ `--export-dynamic-symbol` 链接器选项
2. ❌ 链接器脚本 PROVIDE 创建别名
3. ❌ 手动实现 `__rust_alloc` 函数
4. ❌ 汇编代码创建跳转包装
5. ❌ 覆盖 `__rust_no_alloc_shim_is_unstable_v2` 符号

---

## 最终解决方案：使用标准 alloc crate

### 验证结果

在 **Rust 1.95.0-nightly (2026-02-04)** 中，`__rust_no_alloc_shim_is_unstable_v2` 问题已被解决！

**测试结果**：
```
test: Testing standard alloc crate types...
test: 1. Testing alloc::vec::Vec...
test:    SUCCESS - Vec works correctly
test: 2. Testing alloc::boxed::Box...
test:    SUCCESS - Box works correctly
test: 3. Testing alloc::sync::Arc...
test:    SUCCESS - Arc works correctly
test: 4. Testing alloc::string::String...
test:    SUCCESS - String works correctly
test: All standard alloc crate types work correctly!
test: This means the __rust_no_alloc_shim_is_unstable_v2 issue is resolved.
```

---

## 迁移内容

### 删除的自定义类型

| 类型 | 替换为 | 文件 |
|------|--------|------|
| `SimpleArc<T>` | `alloc::sync::Arc<T>` | 已删除 collection.rs |
| `SimpleVec<T>` | `alloc::vec::Vec<T>` | 已删除 collection.rs |
| `SimpleBox<T>` | `alloc::boxed::Box<T>` | 已删除 collection.rs |
| `SimpleString` | `alloc::string::String` | 已删除 collection.rs |

### 修改的文件

**核心文件**：
- `kernel/src/collection.rs` - **已删除**
- `kernel/src/main.rs` - 移除 `mod collection;`
- `kernel/src/fs/vfs.rs` - SimpleArc → Arc
- `kernel/src/fs/file.rs` - SimpleArc → Arc
- `kernel/src/fs/rootfs.rs` - SimpleArc → Arc
- `kernel/src/fs/dentry.rs` - SimpleArc → Arc
- `kernel/src/fs/inode.rs` - SimpleArc → Arc
- `kernel/src/fs/pipe.rs` - SimpleArc → Arc
- `kernel/src/fs/mount.rs` - SimpleArc → Arc
- `kernel/src/fs/superblock.rs` - SimpleArc → Arc
- `kernel/src/sched/sched.rs` - SimpleArc → Arc

**测试文件**：
- `kernel/src/tests/arc_alloc.rs` - **已删除**
- `kernel/src/tests/dcache.rs` - SimpleArc → Arc
- `kernel/src/tests/fdtable.rs` - SimpleArc → Arc
- `kernel/src/tests/icache.rs` - SimpleArc → Arc
- `kernel/src/tests/standard_alloc.rs` - 新增标准 alloc 测试

---

## 代码变更统计

- **删除文件**：2 个（collection.rs, tests/arc_alloc.rs）
- **修改文件**：15 个
- **删除代码行**：~400 行自定义集合实现
- **添加代码行**：~50 行标准 alloc 测试

---

## 关键变更示例

### SimpleArc → Arc 迁移

**之前**（SimpleArc）：
```rust
use crate::collection::SimpleArc;

// 创建 Arc，返回 Option
let arc = match SimpleArc::new(value) {
    Some(a) => a,
    None => return Err(OutOfMemory),
};

// 访问数据
let data = arc.as_ref();
```

**现在**（标准 Arc）：
```rust
use alloc::sync::Arc;

// 创建 Arc，失败时 panic
let arc = Arc::new(value);

// 访问数据（自动 Deref）
let data = &*arc;
// 或
let data = &arc;
```

### Arc 方法变化

| SimpleArc 方法 | 标准 Arc 等价 | 说明 |
|----------------|---------------|------|
| `SimpleArc::new(v)` | `Arc::new(v)` | Arc 不返回 Option |
| `arc.as_ref()` | `&*arc` 或 `&arc` | Arc 自动 Deref |
| `arc.as_ptr()` | `Arc::as_ptr(&arc)` | 需要显式传递引用 |
| `SimpleArc::clone(v)` | `Arc::clone(&v)` | 标准接口 |

---

## 优势

1. **标准兼容** - 使用 Rust 标准库类型，完全兼容
2. **代码简化** - 无需维护自定义集合实现
3. **性能优化** - 标准库经过充分优化
4. **社区支持** - 标准库有更好的文档和社区支持
5. **未来兼容** - 随 Rust 版本更新自动受益

---

## 已知限制

### 内部可变性

标准的 `Arc<T>` 只提供不可变引用 `&T`。如果需要修改 `T` 的内容，必须使用内部可变性模式：

```rust
use core::sync::atomic::{AtomicUsize, Ordering};
use alloc::sync::Arc;

struct Data {
    value: AtomicUsize,
}

let data = Arc::new(Data { value: AtomicUsize::new(0) });

// 通过 AtomicUsize 修改值
data.value.store(42, Ordering::SeqCst);
```

**注意**：File 操作中的 `close()` 等方法需要 `&mut self`，这在 Arc 环境中需要特殊处理（使用 unsafe 转换）。

---

## 相关资源

- [Rust Tracking Issue #123015](https://github.com/rust-lang/rust/issues/123015) - __rust_no_alloc_shim_is_unstable
- [PR #86844](https://github.com/rust-lang/rust/pull/86844) - Support #[global_allocator] without allocator shim
- [Phil Opp's Allocator Design](https://os.phil-opp.com/allocator-designs/)

---

## 结论

Rux OS 现在完全使用标准的 Rust `alloc` crate，不再需要自定义集合类型。这得益于 Rust 编译器的改进，解决了早期的符号可见性问题。

**推荐实践**：
- ✅ 使用 `alloc::sync::Arc` 用于共享引用
- ✅ 使用 `alloc::boxed::Box` 用于堆分配
- ✅ 使用 `alloc::vec::Vec` 用于动态数组
- ✅ 使用 `alloc::string::String` 用于字符串
- ❌ 不再使用任何 `Simple*` 自定义类型
