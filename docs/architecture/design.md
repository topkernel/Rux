# Rux 设计原则

## ⚠️ 最高原则（绝对不可违反）

### **0. POSIX/ABI 完全兼容，绝不创新**

这是 Rux 内核开发的**最高指导原则**，所有设计和实现决策都必须服从于此原则。

- **核心要求**：
  - **100% POSIX 兼容**：完全遵守 POSIX 标准（IEEE Std 1003.1）
  - **Linux ABI 完全兼容**：与 Linux 内核 ABI 二进制兼容
  - **系统调用兼容**：系统调用号、参数、返回值与 Linux 完全一致
  - **文件系统兼容**：支持 Linux 文件系统格式（ext4、btrfs 等）
  - **ELF 格式兼容**：可执行文件格式与 Linux 完全一致
  - **不创新原则**：**绝不**为了"更好"而偏离 Linux 标准

- **实现方式**：
  - 直接参考 Linux 内核实现
  - 使用相同的系统调用号（`arch/x86/entry/syscalls`）
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
  - Linux内核文档（Documentation/）

> **记住**：我们的目标是用 Rust 重写 Linux 内核，而不是创造一个新的操作系统。任何偏离 Linux 标准的"创新"都是错误的。

---

## 项目目标

Rux 是一个用 **Rust** 编写的 Linux 兼容操作系统内核，目标是实现与 Linux 内核 **完全兼容**的功能，包括：
- 完整的 POSIX API 支持
- Linux ABI 二进制兼容
- 可运行原生的 Linux 用户空间程序

除平台相关的必要汇编代码外，所有代码使用 Rust 编写。

## 核心设计原则

### 1. **Linux 兼容性（最高优先级）**

- **原则**：除平台相关的必要汇编代码外，所有内核代码使用 Rust 编写
- **理由**：
  - 内存安全：Rust 的所有权系统可在编译时防止内存错误
  - 并发安全：类型系统可防止数据竞争
  - 现代工具链：包管理、文档生成、测试框架
- **例外**：
  - 启动代码（boot.S）
  - 上下文切换（context_switch.S）
  - 中断入口（trap.S）
  - 特权级切换

### 2. **Rust 优先 (Rust-First)**

- **原则**：除平台相关的必要汇编代码外，所有内核代码使用 Rust 编写
- **理由**：
  - 内存安全：Rust 的所有权系统可在编译时防止内存错误
  - 并发安全：类型系统可防止数据竞争
  - 现代工具链：包管理、文档生成、测试框架
- **例外**：
  - 启动代码（boot.S）
  - 上下文切换（context_switch.S）
  - 中断入口（trap.S）
  - 特权级切换

**注意**：使用 Rust 是实现手段，不是目的。即使使用 Rust，也必须完全遵循 Linux 的设计和接口规范。

### 3. **平台抽象**

- **原则**：平台相关代码隔离在 `arch/` 目录
- **结构**：
  ```
  kernel/src/arch/
  ├── riscv64/        # RISC-V 支持（默认）
  ├── x86_64/         # x86_64 支持（暂未实现）
  └── aarch64/        # ARM64 支持（已移除，暂不维护）
  ```
- **平台抽象层**：
  - 统一的内存管理接口
  - 统一的中断处理框架
  - 统一的设备驱动接口

### 4. **模块化设计**

- **原则**：清晰的模块边界，便于开发和测试
- **模块划分**（参考 Linux 内核结构）：
  - `arch/`：平台相关代码（对应 Linux `arch/`）
  - `mm/`：内存管理（对应 Linux `mm/`）
  - `process/`：进程管理（对应 Linux `kernel/`）
  - `fs/`：文件系统（对应 Linux `fs/`）
  - `ipc/`：进程间通信（对应 Linux `ipc/`）
  - `net/`：网络协议栈（对应 Linux `net/`）
  - `drivers/`：设备驱动（对应 Linux `drivers/`）
  - `sync/`：同步原语（对应 Linux `kernel/`）

**重要**：模块划分和组织方式参考 Linux，但使用 Rust 实现。

### 5. **分层架构**

- **原则**：清晰的模块边界，便于开发和测试
- **模块划分**：
  - `arch/`：平台相关代码
  - `mm/`：内存管理
  - `process/`：进程管理
  - `fs/`：文件系统（VFS + 具体文件系统）
  - `ipc/`：进程间通信
  - `net/`：网络协议栈
  - `drivers/`：设备驱动
  - `sync/`：同步原语

### 6. **分层架构**

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
│     - riscv64 (默认)                 │
├─────────────────────────────────────┤
│     硬件 (Hardware)                 │
└─────────────────────────────────────┘
```

**关键点**：所有接口和层与 Linux 对齐。

### 7. **渐进式实现**

- **原则**：从最小可运行内核开始，逐步添加功能
- **优先级**：
  1. 基础框架（启动、内存、中断）
  2. 进程管理（调度、上下文切换）
  3. 系统调用（用户/内核隔离）
  4. 文件系统（VFS + ext4）
  5. 网络协议栈
  6. 高级功能（IPC、信号、实时调度）

### 7. **测试驱动**

- **原则**：每个模块都应有对应的测试
- **测试类型**：
  - 单元测试（模块级）
  - 集成测试（QEMU 模拟）
  - **Linux 测试兼容**：使用 LTP（Linux Test Project）验证兼容性
- **CI/CD**：
  - 自动化构建和测试
  - 多平台测试（aarch64、x86_64、riscv64）

### 8. **文档完善**

- **原则**：代码与文档同步更新
- **文档类型**：
  - API 文档（rustdoc）
  - 设计文档（DESIGN.md）
  - 进度追踪（TODO.md、ROADMAP.md）
  - 用户文档（使用指南）
- **参考文档**：
  - Linux 内核文档（必须参考）
  - POSIX 标准文档
  - ARM/Intel/RISC-V 架构手册

## POSIX/ABI 实现指南

### 系统调用实现

**必须**使用 Linux 的系统调用号：

```rust
// 直接使用 Linux 的系统调用号
pub const __NR_read: usize = 63;
pub const __NR_write: usize = 64;
pub const __NR_open: usize = 1024;
// ... 完全按照 Linux 的定义
```

**禁止**：
- ❌ 创造新的系统调用
- ❌ 修改系统调用号
- ❌ 改变系统调用参数

### 结构体布局

**必须**与 Linux 结构体完全一致：

```rust
// 必须与 Linux 的 struct pt_regs 完全一致
#[repr(C)]
pub struct PtRegs {
    pub regs: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
    // ... 字段顺序、大小、对齐都必须一致
}
```

### 文件系统

**必须**支持 Linux 的文件系统格式：
- ext4（必须）
- btrfs（必须）
- procfs（必须）
- sysfs（必须）

**禁止**：
- ❌ 创建新的文件系统格式
- ❌ 修改现有格式（除非 Linux 也改）

### 设备接口

**必须**使用 Linux 的设备接口：
- 字符设备（`/dev/xxx`）
- 块设备（`/dev/sda`）
- 网络设备（`eth0`）

**参考**：Linux `include/uapi/` 下的接口定义

## 实现检查清单

在实现任何功能时，必须验证：

- [ ] 查阅 Linux 内核源码实现
- [ ] 确认使用相同的系统调用号/结构体
- [ ] 确认使用相同的文件格式
- [ ] 确认符合 POSIX 标准
- [ ] 阅读相关 Linux man pages
- [ ] 不包含任何"创新"或"改进"

**记住**：如果有疑问，直接参考 Linux 的实现。

- **原则**：代码与文档同步更新
- **文档类型**：
  - API 文档（rustdoc）
  - 设计文档（DESIGN.md）
  - 进度追踪（TODO.md、ROADMAP.md）
  - 用户文档（使用指南）

## 技术约束

### 编译器
- Rust 版本：稳定版（stable）
- 目标平台：riscv64gc-unknown-none-elf（默认）
- 暂不支持：aarch64-unknown-none（已移除）、x86_64-unknown-none（未实现）

### 运行时
- 无标准库（no_std）
- **Buddy System 内存分配器**（已实现）✅
  - 支持 O(log n) 分配/释放
  - 伙伴合并机制减少内存碎片
  - 基于 4KB 页面的块分配
  - 线程安全（原子操作）
  - 最大支持 4GB 内存块 (order 20)
- 无运行时（手动实现 panic 处理）

### 安全性
- 尽可能使用 unsafe 块隔离危险代码
- 显式标记所有 unsafe 代码
- 定期审计 unsafe 代码的正确性

## 性能目标

- **启动时间**：< 100ms（单核，1GHz CPU）
- **上下文切换**：< 1μs
- **中断延迟**：< 5μs
- **系统调用**：< 100ns
- **吞吐量**：目标达到 Linux 的 80% 以上

## 里程碑

### Phase 1: 基础框架 ✅ (已完成)
- [x] 项目结构搭建
- [x] riscv64 平台启动
- [x] UART 驱动
- [x] 基础内存管理
- [x] 堆分配器

### Phase 2: 中断与进程 ✅ (大部分完成)
- [x] 中断和异常处理框架
- [x] 进程调度器
- [x] 上下文切换
- [x] 进程地址空间（基础）

### Phase 3: 系统调用与隔离 ✅ (大部分完成)
- [x] 系统调用接口
- [x] 用户/内核空间隔离
- [x] 信号处理（基础）

### Phase 4: 文件系统 ✅ (基础框架完成)
- [x] VFS 虚拟文件系统
- [x] RootFS 内存文件系统
- [ ] ext4 支持
- [ ] btrfs 支持

### Phase 5: SMP 支持 ✅ (已完成)
- [x] 多核启动
- [x] Per-CPU 数据
- [x] SBI 接口
- [x] PLIC 中断控制器
- [x] IPI 机制
- [x] MMU 多级页表 (Sv39)

### Phase 6: 代码审查 ✅ (已完成)
- [x] 全面代码审查
- [x] 调试输出清理
- [x] 测试脚本完善

### Phase 7: Per-CPU 优化 ✅ (基础完成)
- [x] Per-CPU 运行队列（PER_CPU_RQ[4]、this_cpu_rq/cpu_rq）
- [x] 启动顺序优化（参考 Linux ARM64）
- [ ] 负载均衡（Phase 9）
- [x] Buddy System 内存分配器（Phase 7 完成）

### Phase 8: 快速胜利 ✅ (已完成)
- [x] SimpleArc Clone 支持
- [x] RootFS write_data offset bug
- [x] RootFS 文件系统操作修复（find_child、list_children）

### Phase 9: 网络与高级功能 ⏳

## 贡献指南

### 代码风格
- 遵循 Rust 官方代码风格（rustfmt）
- 使用有意义的变量和函数名
- 适当的注释和文档

### 提交规范
- 清晰的提交信息
- 单个提交只做一件事
- 提交前通过所有测试

### 审查流程
- Code Review 必须通过
- 所有测试必须通过
- 文档必须更新

## 参考资料

- [The Rust Book](https://doc.rust-lang.org/book/)
- [Writing an OS in Rust](https://os.phil-opp.com/)
- [Linux Kernel Documentation](https://www.kernel.org/doc/html/latest/)
- [RISC-V Architecture Reference Manual](https://riscv.org/technical/specifications/)
- [OSDev.org Wiki](https://wiki.osdev.org/)

---

**文档版本**：v0.3.0
**最后更新**：2025-02-04
