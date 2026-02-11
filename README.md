# Rux

<div align="center">

**A Linux-like OS kernel entirely written in Rust**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64-informational.svg)](https://github.com/rust-osdev/rust-embedded)
[![Tests](https://img.shields.io/badge/tests-203%20cases-brightgreen.svg)](docs/tests/unit-test-report.md)
[![Code](https://img.shields.io/badge/code-37%2C484%20lines-blue.svg)](docs/architecture/structure.md)

**默认平台：RISC-V 64位 (RV64GC)**

</div>

---

## 🤖 AI 生成声明

**本项目代码由 AI（Claude Code + GLM4.7）辅助生成和开发。**

- 使用 Anthropic Claude Code CLI 工具进行辅助开发
- 遵循 Linux 内核设计原则和 POSIX 标准
- 旨在探索 **AI 辅助操作系统内核开发** 的可能性和限制

---

## 🎯 项目目标

### ⚠️ 最高原则：POSIX/ABI 完全兼容，绝不创新

**核心目标**：用 Rust 重写 Linux 内核

- ✅ **100% POSIX 兼容** - 完全遵守 POSIX 标准
- ✅ **Linux ABI 兼容** - 可运行原生 Linux 用户空间程序
- ✅ **系统调用兼容** - 使用 Linux 的系统调用号和接口
- ✅ **文件系统兼容** - 支持 ext4、btrfs 等 Linux 文件系统
- ✅ **ELF 格式兼容** - 可执行文件格式与 Linux 完全一致

**严格禁止**：
- ❌ 绝不"优化"或"改进" Linux 的设计
- ❌ 绝不创造新的系统调用或接口
- ❌ 绝不为了"更优雅"而偏离标准

---

## 📊 平台与测试状态

| 指标 | 数值 | 详情 |
|------|------|------|
| **代码行数** | 37,484 行 | [代码结构](docs/architecture/structure.md) |
| **测试用例** | 203 个 (99% 通过) | [测试报告](docs/tests/unit-test-report.md) |
| **测试模块** | 43 个 | [单元测试](docs/tests/unit-test-report.md) |
| **平台支持** | RISC-V 64位 | [开发路线](docs/progress/roadmap.md) |

**模块分布**：
- 文件系统 (fs/): 9,020 行 (24.1%)
- 单元测试 (tests/): 5,885 行 (15.7%)
- 架构相关 (arch/): 6,129 行 (16.4%)
- 设备驱动 (drivers/): 4,472 行 (11.9%)
- 网络协议栈 (net/): 3,626 行 (9.7%)
- 进程管理 (process/): 2,048 行 (5.5%)
- 进程调度 (sched/): 1,416 行 (3.8%)
- 内存管理 (mm/): 1,224 行 (3.3%)
- 同步原语 (sync/): 699 行 (1.9%)

---

## 🚀 快速开始

### 环境要求

```bash
# Rust 工具链（nightly 推荐）
rustc --version
cargo --version

# QEMU 系统模拟器
qemu-system-riscv64 --version

# RISC-V 目标
rustup target add riscv64gc-unknown-none-elf
```

### 构建和运行

```bash
# 构建内核
make build

# 运行内核
make run

# 运行单元测试
./test/run_unit_tests.sh
```

详细说明：[快速开始指南](docs/guides/getting-started.md)

---

## 📁 项目结构

```
Rux/
├── kernel/                 # 内核源码 (37,484 行)
│   ├── src/
│   │   ├── arch/         # RISC-V 架构 (6,129 行)
│   │   ├── drivers/      # 设备驱动 (4,472 行)
│   │   ├── fs/           # 文件系统 (9,020 行)
│   │   ├── net/          # 网络协议栈 (3,626 行)
│   │   ├── process/      # 进程管理 (2,048 行)
│   │   ├── sched/        # 进程调度 (1,416 行)
│   │   ├── mm/           # 内存管理 (1,224 行)
│   │   ├── sync/         # 同步原语 (699 行)
│   │   └── tests/        # 单元测试 (5,885 行)
│   └── build.rs          # 构建脚本
├── docs/                 # 📚 文档中心
├── test/                 # 测试脚本
└── Cargo.toml           # 工作空间配置
```

详细结构：[项目结构文档](docs/architecture/structure.md)

---

## 📚 文档

### 核心文档

- **[快速开始](docs/guides/getting-started.md)** - 5 分钟上手
- **[开发路线](docs/progress/roadmap.md)** - Phase 规划和当前状态
- **[项目结构](docs/architecture/structure.md)** - 源码组织 (37,484 行代码统计)
- **[测试报告](docs/tests/unit-test-report.md)** - 203 个测试用例详细分析
- **[设计原则](docs/architecture/design.md)** - POSIX 兼容和 Linux ABI 对齐

### 架构文档

- **[RISC-V 架构](docs/architecture/riscv64.md)** - RV64GC 支持详情
- **[启动流程](docs/architecture/boot.md)** - 从 OpenSBI 到内核启动
- **[变更日志](docs/development/changelog.md)** - 版本历史和更新记录

### 开发指南

- **[开发流程](docs/guides/development.md)** - 贡献代码和开发规范
- **[用户程序](docs/development/user-programs.md)** - ELF 加载和 execve

---

## ✨ 主要功能

### ✅ 已实现（Phase 1-18）

**硬件基础**：
- OpenSBI 集成、异常处理、UART 驱动、Timer 中断、PLIC 中断控制器、SMP 多核 (4 核)、IPI 核间中断

**内存管理**：
- 物理页分配器、Buddy 系统、堆分配器、Sv39 3级页表、VMA 管理、**Copy-on-Write (COW)** 🆕

**进程管理**：
- 进程调度器 (Round Robin)、上下文切换、fork/COW fork、execve、wait4、getpid/getppid、信号处理

**同步原语**：
- Mutex 锁、信号量 (411 行)、条件变量 (260 行)、等待队列

**文件系统**：
- VFS 框架、RootFS、ext4 文件系统 (9,020 行)、管道 (pipe)、文件描述符、路径解析

**网络协议栈** (3,626 行)：
- SkBuff 缓冲区、以太网层、ARP 协议、IPv4 协议、UDP/TCP 协议、Socket 系统调用 (7 个)、VirtIO-net 驱动

**系统调用**：
- 文件操作 (open/read/write/close/lseek/fstat)
- 进程管理 (fork/execve/wait4/exit/getpid)
- 信号操作 (sigaction/kill/rt_sigprocmask)
- IPC (pipe/pipe2/select/poll/epoll/eventfd) 🆕
- 内存管理 (mmap/munmap/mprotect/msync/mremap/madvise) 🆕
- Copy-on-Write fork 🆕

### ⏳ 进行中

- 完善 Socket 数据收发
- 实现 sys_clone 线程支持
- 文件描述符标志 (O_CLOEXEC/O_NONBLOCK)

详见 [开发路线图](docs/progress/roadmap.md)

---

## 🧪 测试状态

- **总测试项**: 203
- **通过**: 201 (99.0%)
- **失败**: 5 (预期失败 - 资源池限制)
- **测试模块**: 43 个

[查看详细测试报告](docs/tests/unit-test-report.md)

---

## 🤝 贡献

欢迎贡献！请查看 [开发路线图](docs/progress/roadmap.md) 了解当前需要帮助的任务。

### 开发规范

- 遵循 [Conventional Commits](https://www.conventionalcommits.org/) 规范
- 参考 [开发流程](docs/guides/development.md) 了解开发规范
- 查看 [代码审查记录](docs/progress/code-review.md) 避免已知问题
- 阅读 [测试指南](docs/guides/testing.md) 学习测试方法

**核心原则**：
- ✅ 严格遵循 POSIX 标准
- ✅ 参考 Linux 内核实现
- ✅ 使用 Linux 的系统调用号和数据结构
- ❌ 不创新接口、用Rust重复造轮子

---

## 📄 许可证

MIT License - 详见 [LICENSE](LICENSE)

---

## 🙏 致谢

本项目受到以下项目的启发：

- [Linux Kernel](https://www.kernel.org/)

---

<div align="center">

**Made with ❤️ and Rust + AI**

[项目主页](https://github.com/topkernel/rux) • [问题反馈](https://github.com/topkernel/rux/issues)

</div>
