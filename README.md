# Rux

<div align="center">

**A Linux-like OS kernel entirely written in Rust**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64-informational.svg)](https://github.com/rust-osdev/rust-embedded)
[![Tests](https://img.shields.io/badge/tests-203%20cases-brightgreen.svg)](docs/tests/unit-test-report.md)
[![Code](https://img.shields.io/badge/code-45%2C204%20lines-blue.svg)](docs/architecture/structure.md)

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
- ✅ **文件系统兼容** - 支持 ext4、btrfs 等 Linux 文件系统
- ✅ **ELF 格式兼容** - 可执行文件格式与 Linux 完全一致

**严格禁止**：
- ❌ 绝不"优化"或"改进" Linux 的设计
- ❌ 绝不创造新的系统调用或接口
- ❌ 绝不为了"更优雅"而偏离标准

---

## 📊 项目状态

| 指标 | 数值 | 详情 |
|------|------|------|
| **代码行数** | 45,204 行 | [代码结构](docs/architecture/structure.md) |
| **测试用例** | 203 个 (99% 通过) | [测试报告](docs/tests/unit-test-report.md) |
| **测试模块** | 43 个 | [单元测试](docs/tests/unit-test-report.md) |
| **平台支持** | RISC-V 64位 | [开发路线](docs/progress/roadmap.md) |

**模块分布**：
- 文件系统 (fs/): 10,161 行 (22.5%)
- 架构相关 (arch/): 7,288 行 (16.1%)
- 设备驱动 (drivers/): 7,021 行 (15.5%)
- 单元测试 (tests/): 5,741 行 (12.7%)
- 网络协议栈 (net/): 3,626 行 (8.0%)
- 内存管理 (mm/): 3,412 行 (7.5%)
- 进程管理 (process/): 2,133 行 (4.7%)
- 进程调度 (sched/): 1,416 行 (3.1%)
- 同步原语 (sync/): 699 行 (1.5%)

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

# 构建用户态程序
make user

# 构建Rootfs
make rootfs

# 运行内核
make run  #启动默认的shell，rust + no_std
make run-cshell  #启动用C语言+musl实现的shell
make run-rust-shell  #启动rust语言+std实现的shell

# 运行单元测试
make test
```

详细说明：[快速开始指南](docs/guides/getting-started.md)

---

## 🏆 启动日志

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
mm:               heap region 16MB @ 0x80A00000      [ok]
mm:               slab allocator 1MB                 [ok]
boot:             FDT/DTB parsed                     [ok]
boot:             cmd: root=/dev/vda rw ini...       [ok]
mm:               user frame allocator 64MB          [ok]
mm:               16384 page descriptors             [ok]
intc:             PLIC @ 0x0C000000                  [ok]
intc:             external IRQ routing               [ok]
ipi:              SSIP software IRQ                  [ok]
bio:              buffer cache layer                 [ok]
fs:               ext4 driver loaded                 [ok]
fs:               ramfs mounted /                    [ok]
fs:               procfs mounted /proc               [ok]
driver:           virtio-blk PCI x1                  [ok]
driver:           virtio-net x1                      [ok]
sched:            CFS scheduler v1                   [ok]
trap:             sie.SEIE enabled                   [ok]
init:             loading /bin/shell                 [ok]
init:             ELF loaded to user space           [ok]
init:             init task (PID 1) enqueued         [ok]
```

---

## 📁 项目结构

```
Rux/
├── kernel/                 # 内核源码 (45,204 行)
│   ├── src/
│   │   ├── fs/           # 文件系统 (10,161 行)
│   │   ├── arch/         # RISC-V 架构 (7,288 行)
│   │   ├── drivers/      # 设备驱动 (7,021 行)
│   │   ├── tests/        # 单元测试 (5,741 行)
│   │   ├── net/          # 网络协议栈 (3,626 行)
│   │   ├── mm/           # 内存管理 (3,412 行)
│   │   ├── process/      # 进程管理 (2,133 行)
│   │   ├── sched/        # 进程调度 (1,416 行)
│   │   └── sync/         # 同步原语 (699 行)
│   └── build.rs          # 构建脚本
├── docs/                 # 📚 文档中心
├── test/                 # 测试脚本
├── userspace/            # 用户态程序
│   ├── shell/            # 默认 Shell (no_std)
│   ├── cshell/           # C Shell (musl libc)
│   └── rust-shell/       # Rust std Shell
├── toolchain/            # 工具链 (musl libc)
└── Cargo.toml            # 工作空间配置
```

详细结构：[项目结构文档](docs/architecture/structure.md)

---

## 📚 文档

### 核心文档

- **[快速开始](docs/guides/getting-started.md)** - 5 分钟上手
- **[开发路线](docs/progress/roadmap.md)** - Phase 规划和当前状态
- **[项目结构](docs/architecture/structure.md)** - 源码组织
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
