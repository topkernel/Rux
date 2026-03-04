# Rux

<div align="center">

**A Linux-like OS kernel entirely written in Rust**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64-informational.svg)](https://github.com/rust-osdev/rust-embedded)
[![Tests](https://img.shields.io/badge/tests-75%20cases-brightgreen.svg)](docs/tests/unit-test-report.md)
[![Code](https://img.shields.io/badge/code-56%2C600%20lines-blue.svg)](docs/architecture/structure.md)

**默认平台：RISC-V 64位 (RV64GC)**

</div>

---

## 🤖 AI 生成声明

**本项目代码由 AI（Claude Code + GLM5）辅助生成和开发。**

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
- ✅ **文件系统兼容** - 支持 ext4 等 Linux 文件系统
- ✅ **ELF 格式兼容** - 可执行文件格式与 Linux 完全一致

**严格禁止**：
- ❌ 绝不"优化"或"改进" Linux 的设计
- ❌ 绝不创造新的系统调用或接口
- ❌ 绝不为了"更优雅"而偏离标准

---

## 📊 项目状态

| 指标 | 数值 | 详情 |
|------|------|------|
| **代码行数** | ~56,600 行 | [代码结构](docs/architecture/structure.md) |
| **源文件** | 178 个 Rust 文件 | [项目结构](docs/architecture/structure.md) |
| **内核测试** | 51 个测试文件 | [单元测试](docs/tests/unit-test-report.md) |
| **mini-ltp** | 24 个兼容性测试 | [开发路线](docs/progress/roadmap.md) |
| **平台支持** | RISC-V 64位 | [开发路线](docs/progress/roadmap.md) |

**模块分布**：
- 文件系统 (fs/): 11,200+ 行 (21.5%)
- 架构相关 (arch/): 8,500+ 行 (16.3%)
- 单元测试 (tests/): 7,000+ 行 (13.5%)
- 设备驱动 (drivers/): 5,700+ 行 (11.0%)
- 内存管理 (mm/): 4,300+ 行 (8.3%)
- 网络协议栈 (net/): 3,600+ 行 (6.9%)
- 进程调度 (sched/): 2,500+ 行 (4.8%)
- 进程管理 (process/): 1,800+ 行 (3.5%)
- 同步原语 (sync/): 700+ 行 (1.3%)

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

# 构建用户态程序 (shell, apps, mini-ltp, toybox)
make user

# 构建 Rootfs 镜像
make rootfs

# 运行内核 (默认 shell)
make run

# 运行 GUI 桌面
make gui

# 运行单元测试
make test
```

详细说明：[快速开始指南](docs/guides/getting-started.md)

---

## 🏆 启动shell日志

```
██████  ██    ██ ██   ██
██   ██ ██    ██  ██ ██
██████  ██    ██   ███
██   ██ ██    ██  ██ ██
██   ██  ██████  ██   ██
  [ RISC-V 64-bit | POSIX Compatible | v0.1.0 ]

Kernel starting...

Module            Description                        Status
----------------  --------------------------------   --------
console:          UART ns16550a driver               [ok]
smp:              4 CPU(s) online                    [ok]
trap:             stvec handler installed            [ok]
trap:             ecall syscall handler              [ok]
mm:               Sv39 3-level page table            [ok]
mm:               satp CSR configured                [ok]
mm:               buddy allocator order 0-12         [ok]
mm:               heap region 32MB @ 0x80A00000      [ok]
mm:               slab allocator 4MB                 [ok]
boot:             FDT/DTB parsed                     [ok]
boot:             cmd: root=/dev/vda rw init=...     [ok]
mm:               user frame allocator 64MB          [ok]
mm:               32768 page descriptors             [ok]
intc:             PLIC @ 0x0C000000                  [ok]
intc:             external IRQ routing               [ok]
ipi:              SSIP software IRQ                  [ok]
bio:              buffer cache layer                 [ok]
fs:               ext4 driver loaded                 [ok]
fs:               ramfs mounted /                    [ok]
fs:               procfs initialized                 [ok]
fs:               procfs mounted /proc               [ok]
driver:           virtio-blk PCI x1                  [ok]
driver:           GenDisk registered                 [ok]
fs:               ext4 mounted /                     [ok]
fs:               procfs remounted /proc             [ok]
driver:           virtio-net x1                      [ok]
sched:            CFS scheduler v1                   [ok]
sched:            runqueue per-CPU                   [ok]
sched:            PID allocator init                 [ok]
sched:            idle task (PID 0)                  [ok]
mm:               PCP cpu2 hotpage                   [ok]
trap:             sie.SEIE enabled                   [ok]
driver:           virtio-gpu probed                  [ok]
gpu:              1280x800 32bpp framebuffer         [ok]
fs:               devfs mounted /dev                 [ok]
driver:           evdev /dev/input/event0            [ok]
driver:           evdev /dev/input/event1            [ok]
driver:           PS/2 keyboard (stub)               [ok]
driver:           PS/2 mouse (stub)                  [ok]

init:             loading /bin/shell                 [ok]
init:             ELF loaded to user space           [ok]
init:             init task (PID 1) enqueued         [ok]


========================================
  Rux OS Shell v0.4 (musl libc)
========================================
Type 'help' for available commands

rux> 
```

## 启动gui
![desktop](image.png)
---

## 📁 项目结构

```
Rux/
├── kernel/                 # 内核源码 (~56,600 行)
│   ├── src/
│   │   ├── fs/           # 文件系统 (11,200+ 行)
│   │   │   ├── ext4/     # ext4 文件系统
│   │   │   ├── devfs/    # devfs 设备文件系统
│   │   │   └── procfs.rs # procfs 进程文件系统
│   │   ├── arch/         # RISC-V 架构 (8,500+ 行)
│   │   ├── drivers/      # 设备驱动 (5,700+ 行)
│   │   │   ├── gpu/      # GPU/帧缓冲驱动
│   │   │   ├── input/    # 输入设备驱动
│   │   │   ├── virtio/   # VirtIO 设备
│   │   │   └── net/      # 网络设备
│   │   ├── tests/        # 单元测试 (51 个文件)
│   │   ├── net/          # 网络协议栈 (3,600+ 行)
│   │   ├── mm/           # 内存管理 (4,300+ 行)
│   │   ├── sched/        # 进程调度 (2,500+ 行)
│   │   ├── process/      # 进程管理 (1,800+ 行)
│   │   ├── syscall/      # 系统调用 (2,800+ 行)
│   │   └── sync/         # 同步原语 (700+ 行)
│   └── build.rs          # 构建脚本
├── userspace/            # 用户态程序
│   ├── shell/            # 默认 Shell (no_std Rust)
│   ├── apps/             # GUI 应用 (desktop, calculator, clock, vshell)
│   ├── libs/gui/         # GUI 库 (rux_gui)
│   ├── tests/mini-ltp/   # 内核兼容性测试 (24 个)
│   └── toybox/           # Toybox (BusyBox 替代)
├── toolchain/            # 工具链 (musl libc)
├── docs/                 # 📚 文档中心
├── test/                 # 测试脚本
└── Cargo.toml            # 工作空间配置
```

详细结构：[项目结构文档](docs/architecture/structure.md)

---

## ✨ 主要特性

### 已实现功能

- **进程管理**: fork/execve/wait4/信号处理/CFS调度器
- **内存管理**: Sv39页表/Buddy分配器/Slab分配器/COW
- **文件系统**: ext4/procfs/devfs/ramfs
- **设备驱动**: VirtIO-blk/net/gpu/input, framebuffer, evdev
- **网络协议栈**: TCP/UDP/IPv4/ARP/Socket API
- **SMP多核**: 4核支持/负载均衡/IPI
- **GUI**: 桌面环境/计算器/时钟/可视化Shell

### 系统调用

支持 80+ 个 Linux 系统调用，包括：
- 文件: openat/close/read/write/lseek/fstat/mkdir/unlink
- 进程: fork/execve/wait4/exit/getpid/getppid/kill
- 内存: brk/mmap/munmap/mprotect
- 信号: kill/sigaction/sigprocmask
- 网络: socket/bind/listen/accept/connect/sendto/recvfrom
- IPC: pipe/pipe2/select/poll/eventfd

---

## 📚 文档

### 核心文档

- **[快速开始](docs/guides/getting-started.md)** - 5 分钟上手
- **[开发路线](docs/progress/roadmap.md)** - Phase 规划和当前状态 (Phase 24)
- **[项目结构](docs/architecture/structure.md)** - 源码组织
- **[设计原则](docs/architecture/design.md)** - POSIX 兼容和 Linux ABI 对齐

### 架构文档

- **[RISC-V 架构](docs/architecture/riscv64.md)** - RV64GC 支持详情
- **[启动流程](docs/architecture/boot.md)** - 从 OpenSBI 到内核启动
- **[变更日志](docs/development/changelog.md)** - 版本历史和更新记录

### 开发指南

- **[开发流程](docs/guides/development.md)** - 贡献代码和开发规范
- **[用户程序](docs/development/user-programs.md)** - ELF 加载和 execve

---

## 🧪 测试状态

### 内核单元测试
- **测试文件**: 51 个
- **覆盖范围**: 内存、进程、文件系统、网络、驱动等

### mini-ltp 内核兼容性测试
- **测试数量**: 24 个
- **覆盖范围**: fork, fileio, pipe, mmap, signal, execve 等核心系统调用

---

## 🤝 贡献

欢迎贡献！请查看 [开发路线图](docs/progress/roadmap.md) 了解当前需要帮助的任务。

### 开发规范

- 遵循 [Conventional Commits](https://www.conventionalcommits.org/) 规范
- 参考 [开发流程](docs/guides/development.md) 了解开发规范

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
