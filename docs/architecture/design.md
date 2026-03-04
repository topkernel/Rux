# Rux 设计原则

## ⚠️ 最高原则（绝对不可违反）

### **0. POSIX/ABI 完全兼容，绝不创新**

这是 Rux 内核开发的**最高指导原则**，所有设计和实现决策都必须服从于此原则。

- **核心要求**：
  - **100% POSIX 兼容**：完全遵守 POSIX 标准（IEEE Std 1003.1）
  - **Linux ABI 完全兼容**：与 Linux 内核 ABI 二进制兼容
  - **系统调用兼容**：系统调用号、参数、返回值与 Linux 完全一致
  - **文件系统兼容**：支持 Linux 文件系统格式（ext4）
  - **ELF 格式兼容**：可执行文件格式与 Linux 完全一致
  - **不创新原则**：**绝不**为了"更好"而偏离 Linux 标准

- **实现方式**：
  - 直接参考 Linux 内核实现
  - 使用相同的系统调用号（`arch/riscv/kernel/syscalls`）
  - 使用相同的结构体布局和内存布局
  - 使用相同的文件系统格式
  - 相同的设备接口、网络协议栈

- **严格禁止**：
  - ❌ **绝不**"优化" Linux 的设计
  - ❌ **绝不**创造新的系统调用
  - ❌ **绝不**改变现有接口的行为
  - ❌ **绝不**"重新发明轮子"
  - ❌ **绝不**为了"更优雅"而偏离标准

- **参考资源**：
  - Linux 内核源码（https://elixir.bootlin.com/linux/latest/source）
  - Linux man pages（POSIX 标准函数）
  - Linux ABI 文档（`man 2 syscall`）
  - Linux 内核文档（Documentation/）

> **记住**：我们的目标是用 Rust 重写 Linux 内核，而不是创造一个新的操作系统。任何偏离 Linux 标准的"创新"都是错误的。

---

## 项目目标

Rux 是一个用 **Rust** 编写的 Linux 兼容操作系统内核，目标是实现与 Linux 内核 **完全兼容**的功能，包括：
- 完整的 POSIX API 支持
- Linux ABI 二进制兼容
- 可运行原生的 Linux 用户空间程序

除平台相关的必要汇编代码外，所有代码使用 Rust 编写。

---

## 核心设计原则

### 1. **Linux 兼容性（最高优先级）**

所有接口、系统调用、数据结构必须与 Linux 完全一致。

**检查清单**：
- [ ] 系统调用号是否与 Linux 一致？
- [ ] 数据结构布局是否与 Linux 一致？
- [ ] 是否遵循了 POSIX 标准？
- [ ] 是否参考了 Linux 内核源码？
- [ ] 是否包含了任何"创新"？

### 2. **Rust 优先 (Rust-First)**

- **原则**：除平台相关的必要汇编代码外，所有内核代码使用 Rust 编写
- **理由**：
  - 内存安全：Rust 的所有权系统可在编译时防止内存错误
  - 并发安全：类型系统可防止数据竞争
  - 现代工具链：包管理、文档生成、测试框架
- **例外**：
  - 启动代码（boot.S）
  - 上下文切换（context.rs 中的 naked 函数）
  - 中断入口（trap.S）
  - 特权级切换

**注意**：使用 Rust 是实现手段，不是目的。即使使用 Rust，也必须完全遵循 Linux 的设计和接口规范。

### 3. **平台抽象**

- **原则**：平台相关代码隔离在 `arch/` 目录
- **结构**：
  ```
  kernel/src/arch/
  └── riscv64/        # RISC-V 64位（唯一支持）
  ```
- **平台抽象层**：
  - 统一的内存管理接口
  - 统一的中断处理框架
  - 统一的设备驱动接口

**注意**：ARM64 (aarch64) 架构已移除，暂不维护。

### 4. **模块化设计**

- **原则**：清晰的模块边界，便于开发和测试
- **模块划分**（参考 Linux 内核结构）：
  - `arch/`：平台相关代码（对应 Linux `arch/`）
  - `mm/`：内存管理（对应 Linux `mm/`）
  - `process/`：进程管理（对应 Linux `kernel/`）
  - `fs/`：文件系统（对应 Linux `fs/`）
  - `net/`：网络协议栈（对应 Linux `net/`）
  - `drivers/`：设备驱动（对应 Linux `drivers/`）
  - `sync/`：同步原语（对应 Linux `kernel/`）
  - `syscall/`：系统调用分发

**重要**：模块划分和组织方式参考 Linux，但使用 Rust 实现。

### 5. **分层架构**

```
┌─────────────────────────────────────┐
│     用户空间（User Space）           │
│     - Linux ELF 二进制               │
│     - musl libc                     │
├─────────────────────────────────────┤
│     系统调用接口 (System Call)       │
│     - 完全兼容 Linux syscall         │
├─────────────────────────────────────┤
│     VFS │ IPC │ 网络 (Net)          │
│     - Linux 兼容的 VFS               │
├─────────────────────────────────────┤
│     进程管理 │ 内存管理 │ 驱动      │
│     - Linux 进程模型                │
├─────────────────────────────────────┤
│     平台抽象层 (Arch Abstraction)    │
│     - riscv64 (唯一支持)             │
├─────────────────────────────────────┤
│     硬件 (Hardware)                 │
└─────────────────────────────────────┘
```

**关键点**：所有接口和层与 Linux 对齐。

### 6. **渐进式实现**

- **原则**：从最小可运行内核开始，逐步添加功能
- **优先级**：
  1. 基础框架（启动、内存、中断）✅
  2. 进程管理（调度、上下文切换）✅
  3. 系统调用（用户/内核隔离）✅
  4. 文件系统（VFS + ext4）✅
  5. 网络协议栈 ✅
  6. 高级功能（IPC、信号、实时调度）✅
  7. GUI 支持 ✅

### 7. **测试驱动**

- **原则**：每个模块都应有对应的测试
- **测试类型**：
  - 内核单元测试（51 个测试文件）
  - mini-ltp 测试（24 个内核兼容性测试）
  - QEMU 集成测试
- **测试命令**：
  - `make test` - 运行内核单元测试
  - `cd /test/mini-ltp && ./run_tests.sh` - 运行兼容性测试

### 8. **文档完善**

- **原则**：代码与文档同步更新
- **文档类型**：
  - API 文档（rustdoc）
  - 设计文档（本文件）
  - 进度追踪（roadmap.md）
  - 用户文档（getting-started.md）
  - 调试报告（fork-exec-debug-report.md）

---

## POSIX/ABI 实现指南

### 系统调用实现

**必须**使用 Linux 的系统调用号（RISC-V）：

```rust
// 直接使用 Linux RISC-V 的系统调用号
pub const __NR_read: usize = 63;
pub const __NR_write: usize = 64;
pub const __NR_openat: usize = 56;
pub const __NR_close: usize = 57;
pub const __NR_exit: usize = 93;
pub const __NR_getpid: usize = 172;
// ... 完全按照 Linux 的定义
```

**禁止**：
- ❌ 创造新的系统调用
- ❌ 修改系统调用号
- ❌ 改变系统调用参数

### 结构体布局

**必须**与 Linux 结构体完全一致：

```rust
// 必须与 Linux 的 struct stat 完全一致
#[repr(C)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub __pad1: u64,
    pub st_size: i64,
    pub st_blksize: i32,
    pub __pad2: i32,
    pub st_blocks: i64,
    // ... 字段顺序、大小、对齐都必须一致
}
```

### 文件系统

**必须**支持 Linux 的文件系统格式：
- ext4（已实现）
- ramfs（已实现）
- procfs（已实现）
- devfs（已实现）

**禁止**：
- ❌ 创建新的文件系统格式
- ❌ 修改现有格式（除非 Linux 也改）

### 设备接口

**必须**使用 Linux 的设备接口：
- 字符设备（`/dev/xxx`）
- 块设备（`/dev/vda`）
- 输入设备（`/dev/input/event0`）

**参考**：Linux `include/uapi/` 下的接口定义

---

## 实现检查清单

在实现任何功能时，必须验证：

- [ ] 查阅 Linux 内核源码实现
- [ ] 确认使用相同的系统调用号/结构体
- [ ] 确认使用相同的文件格式
- [ ] 确认符合 POSIX 标准
- [ ] 阅读相关 Linux man pages
- [ ] 不包含任何"创新"或"改进"

**记住**：如果有疑问，直接参考 Linux 的实现。

---

## 技术约束

### 编译器
- Rust 版本：稳定版（stable）
- 目标平台：riscv64gc-unknown-none-elf（唯一支持）

### 运行时
- 无标准库（no_std）
- 无运行时（手动实现 panic 处理）

### 安全性
- 尽可能使用 unsafe 块隔离危险代码
- 显式标记所有 unsafe 代码
- 定期审计 unsafe 代码的正确性

---

## 性能目标

- **启动时间**：< 5 秒（QEMU virt）
- **上下文切换**：< 1μs
- **中断延迟**：< 5μs
- **系统调用**：< 100ns

---

## 贡献指南

### 代码风格
- 遵循 Rust 官方代码风格（rustfmt）
- 使用有意义的变量和函数名
- 适当的注释和文档

### 提交规范
- 遵循 [Conventional Commits](https://www.conventionalcommits.org/)
- 清晰的提交信息
- 单个提交只做一件事
- 提交前通过所有测试

### 审查流程
- 代码审查必须通过
- 所有测试必须通过
- 文档必须更新

---

## 参考资料

- [Linux Kernel Documentation](https://www.kernel.org/doc/html/latest/)
- [RISC-V Architecture Reference Manual](https://riscv.org/technical/specifications/)
- [POSIX Standard](https://pubs.opengroup.org/onlinepubs/9699919799/)
- [Linux man pages](https://man7.org/linux/man-pages/)

---

**文档版本**：v2.0.0
**最后更新**：2026-03-04
