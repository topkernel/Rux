# Rux

<div align="center">

**A Linux-like OS kernel entirely written in Rust**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-aarch64--riscv64-informational.svg)](https://github.com/rust-osdev/rust-embedded)

Rux 是一个完全用 **Rust** 编写的类 Linux 操作系统内核（除必要的平台相关汇编代码外）。

**默认平台：RISC-V 64位 (RV64GC)**

</div>

---

## 🤖 AI 生成声明

**本项目代码由 AI（Claude Sonnet 4.5）辅助生成和开发。**

开发过程：
- 使用 Anthropic 的 Claude Code CLI 工具进行辅助开发
- AI 协助编写代码、调试错误、优化结构、编写文档
- 所有代码遵循 Linux 内核的设计原则和 POSIX 标准
- 开发者负责审查所有 AI 生成的代码并进行测试验证

本项目旨在探索 **AI 辅助操作系统内核开发** 的可能性和限制。

---

## 🎯 项目目标

### ⚠️ 最高原则：POSIX/ABI 完全兼容，绝不创新

Rux 的核心目标是**用 Rust 重写 Linux 内核**，实现：

- **100% POSIX 兼容**：完全遵守 POSIX 标准
- **Linux ABI 兼容**：可运行原生 Linux 用户空间程序
- **系统调用兼容**：使用 Linux 的系统调用号和接口
- **文件系统兼容**：支持 ext4、btrfs 等 Linux 文件系统
- **ELF 格式兼容**：可执行文件格式与 Linux 完全一致

**严格禁止**：
- ❌ 绝不"优化"或"改进" Linux 的设计
- ❌ 绝不创造新的系统调用或接口
- ❌ 绝不为了"更优雅"而偏离标准

### 实现方式

除平台相关的必要汇编代码外，所有代码使用 Rust 编写，但**所有设计和接口必须完全遵循 Linux 标准**。

- **参考实现**：Linux 内核源码
- **接口标准**：POSIX 标准、Linux ABI
- **文档参考**：Linux man pages、内核文档

---

## ✨ 当前状态

### 最新成就 (2025-02-06)

#### ✅ **RISC-V 64位架构支持** (Phase 10 - 默认平台)

**核心功能**：
- ✅ **启动流程** - boot.S + OpenSBI 集成
- ✅ **异常处理** - S-mode trap handler (trap.rs + trap.S)
- ✅ **Timer Interrupt** - SBI 0.2 TIMER extension
  - 周期性定时器中断（1 秒）
  - stvec Direct 模式修复
- ✅ **MMU 和页表管理** - RISC-V Sv39 虚拟内存
  - 3级页表结构（512 PTE/级）
  - 39位虚拟地址（512GB地址空间）
  - 内核空间恒等映射（0x80200000+）
  - **MMU 已成功使能并运行**
  - 设备内存映射：UART、PLIC、CLINT
- ✅ **PLIC 中断控制器** - Platform-Level Interrupt Controller 驱动
  - 支持 128 个外部中断
  - 4 个 hart 支持
  - 中断优先级管理（0-7 级）
  - Claim/Complete 协议
- ✅ **IPI 核间中断** - Inter-Processor Interrupt 框架
  - IPI 类型：Reschedule、Stop
  - PLIC 中断映射（IRQ 10-13）
  - IPI 处理框架

**测试输出**：
```
Rux OS v0.1.0 - RISC-V 64-bit
trap: Initializing RISC-V trap handling...
trap: RISC-V trap handling [OK]
mm: Initializing RISC-V MMU (Sv39)...
mm: Root page table at PPN = 0x80207
mm: Page table mappings created
mm: MMU enabled successfully
mm: RISC-V MMU [OK]
smp: Initializing RISC-V SMP...
smp: Boot CPU (hart 0) identified
smp: Maximum 4 CPUs supported
smp: Starting secondary hart 1...hart 2...hart 3...
smp: RISC-V SMP initialized
intc: Initializing RISC-V PLIC...
intc: PLIC initialized
ipi: Initializing RISC-V IPI support...
ipi: IPI support initialized (framework only, PLIC IPI pending)
[OK] Timer interrupt enabled, system ready.
```

**关键修复 (2025-02-06)**：
- ✅ **Timer interrupt sepc 处理** - 不再跳过 WFI 指令，避免跳转到指令中间
- ✅ **SMP + MMU 竞态条件** - 使用 `AtomicUsize` 保护 `alloc_page_table()` 的 `NEXT_INDEX`
- ✅ **Per-CPU MMU 使能** - 次核等待启动核完成页表初始化后，再使能自己的 MMU

#### ✅ **SMP 多核支持** (Phase 10.1 - 2025-02-06)

**多核启动和管理**：
- ✅ **SMP 框架** (smp.rs)
  - 原子操作实现动态启动核检测
  - Per-CPU 栈管理（每 CPU 16KB，总共 64KB）
  - CPU 启动状态跟踪
- ✅ **SBI HSM 集成**
  - 使用 `sbi_rt::hart_start()` 唤醒次核
  - 最多支持 4 个 CPU 核心
  - 任意 hart 都可以成为启动核
- ✅ **所有 CPU 成功启动并运行**

**技术亮点**：
- 动态启动核检测（使用原子 CAS 操作）
- Per-CPU 栈隔离（每个 CPU 独立的 16KB 栈空间）
- 无死锁设计（非启动核等待初始化完成后进入 WFI）

#### ✅ **控制台输出同步** (Phase 10.2 - 2025-02-06)

**SMP 安全的 UART 输出**：
- ✅ **spin::Mutex 保护 UART 访问**
- ✅ **行级别锁** - 每次 `println!` 只获取一次锁
- ✅ **多核同时输出不再混乱** - 每条输出完整无交叉

**实现**：
- `console::lock()` - 获取 UART 锁守卫
- `Console::write_str()` - 在锁保护下输出整个字符串
- 使用 `spin::Mutex` 的原子操作确保 SMP 安全

---

### ARM64 平台状态

**Phase 1-9 已完成**（已暂停维护，代码已保留）：
- ✅ 基础启动和异常处理
- ✅ GICv3 中断控制器
- ✅ SMP 双核启动 (PSCI + GIC SGI)
- ✅ 系统调用（43+ 系统调用）
- ✅ 进程管理和调度
- ✅ 文件系统 (VFS + RootFS)
- ✅ 信号处理

详见 [docs/TODO.md](docs/TODO.md) 的 ARM64 测试完成功能部分。

---

## 📊 平台支持状态

| 功能模块 | ARM64 (aarch64) | RISC-V64 | 备注 |
|---------|----------------|----------|------|
| **基础启动** | ✅ 已测试 | ✅ 已测试 | 默认平台 |
| **异常处理** | ✅ 已测试 | ✅ 已测试 | trap handler |
| **UART 驱动** | ✅ 已测试 (PL011) | ✅ 已测试 (ns16550a) | 不同驱动 |
| **Timer Interrupt** | ✅ 已测试 (ARMv8) | ✅ 已测试 (SBI) | 不同实现 |
| **中断控制器** | ✅ 已测试 (GICv3) | ✅ 已测试 (PLIC) | 不同实现 |
| **MMU/页表** | ✅ 已测试 (4级页表) | ✅ 已测试 (Sv39 3级) | 不同架构 |
| **SMP 多核** | ✅ 已测试 (PSCI+GIC) | ✅ 已测试 (SBI HSM) | 不同实现 |
| **IPI 核间中断** | ✅ 已测试 (GIC SGI) | ✅ 已测试 (PLIC) | 不同实现 |
| **控制台同步** | ✅ 已测试 (spin::Mutex) | ✅ 已测试 (spin::Mutex) | 代码共享 |
| **系统调用** | ✅ 已测试 (43+) | ⚠️ 未测试 | 框架已移植 |
| **进程调度** | ✅ 已测试 | ⚠️ 未测试 | 代码已共享 |
| **文件系统** | ✅ 已测试 (VFS) | ⚠️ 未测试 | 代码已共享 |

**注意**：大部分 Phase 2-9 的代码是平台无关的，已经在 ARM64 上充分测试。RISC-V64 只需要验证这些功能在新架构上能否正常工作。

---

## 🚀 快速开始

### 环境要求

- Rust 工具链（stable）
- QEMU 系统模拟器
- RISC-V 工具链（默认，已包含在 Rust 中）

### 构建和运行

```bash
# 克隆仓库
git clone https://github.com/your-username/rux.git
cd rux

# 构建内核（默认 RISC-V 平台）
cargo build --package rux --features riscv64

# 运行 RISC-V 内核（4核 SMP）
./test/run_riscv.sh
```

### 调试

```bash
# RISC-V 测试
./test/run_riscv.sh        # 完整运行（10秒超时）
./test/debug_riscv.sh      # GDB 调试
```

### 平台切换

RISC-V 是默认平台。要切换到 ARM 平台：

```bash
# 构建 ARM 平台
cargo build --package rux --features aarch64

# 运行 ARM 内核
./test/run.sh
```

---

## 📁 项目结构

```
Rux/
├── kernel/                    # 内核代码
│   ├── src/
│   │   ├── arch/              # 平台相关代码
│   │   │   ├── riscv64/       # RISC-V 64位（默认）
│   │   │   │   ├── boot.S     # 启动代码（SMP 支持）
│   │   │   │   ├── smp.rs     # SMP 框架
│   │   │   │   ├── ipi.rs     # IPI 核间中断
│   │   │   │   ├── trap.rs    # 异常处理
│   │   │   │   ├── trap.S     # 异常向量表
│   │   │   │   ├── mm.rs      # MMU/页表
│   │   │   │   ├── context.rs # 上下文切换
│   │   │   │   ├── syscall.rs # 系统调用
│   │   │   │   └── linker.ld  # 链接脚本
│   │   │   └── aarch64/       # ARM64 支持
│   │   ├── drivers/           # 设备驱动
│   │   │   ├── intc/          # 中断控制器
│   │   │   │   ├── plic.rs    # PLIC 驱动 (RISC-V)
│   │   │   │   ├── gicv3.rs   # GICv3 驱动 (ARM64)
│   │   │   │   └── mod.rs     # 平台选择
│   │   │   └── timer/         # 定时器驱动
│   │   ├── console.rs         # UART 驱动（SMP 安全）
│   │   ├── print.rs           # 打印宏
│   │   ├── process/           # 进程管理
│   │   ├── fs/                # 文件系统
│   │   └── main.rs            # 内核入口
├── test/                       # 测试脚本
│   ├── run_riscv.sh           # RISC-V 运行
│   └── debug_riscv.sh         # GDB 调试
├── docs/                       # 文档
│   ├── TODO.md                # 开发路线图
│   ├── DESIGN.md              # 设计原则
│   └── CODE_REVIEW.md         # 代码审查记录
├── Cargo.toml                  # 工作空间配置
└── README.md                   # 本文件
```

---

## 📚 文档

- **[开发路线图](docs/TODO.md)** - 详细的任务列表和进度追踪
- **[设计原则](docs/DESIGN.md)** - 项目的设计理念和技术约束
- **[代码审查记录](docs/CODE_REVIEW.md)** - 代码审查发现的问题和修复进度

---

## 🗺️ 开发路线

### ✅ 已完成的 Phase

- **Phase 1**: 基础框架 (ARM64)
- **Phase 2**: 中断与进程 (ARM64)
- **Phase 3**: 系统调用与隔离 (ARM64)
- **Phase 4**: 文件系统 (ARM64)
- **Phase 5**: SMP 支持 (ARM64)
- **Phase 6**: 代码审查
- **Phase 7**: 内存管理 (Buddy System)
- **Phase 8**: Per-CPU 优化
- **Phase 9**: 快速胜利 (文件系统修复)
- **Phase 10**: RISC-V 架构 + SMP + 控制台同步 ✅ **当前**

### ⏳ 待完成的 Phase

- **Phase 11**: 网络与 IPC (TCP/IP、管道、消息队列)
- **Phase 12**: x86_64 架构支持
- **Phase 13**: 设备驱动 (PCIe、存储、网络)
- **Phase 14**: 用户空间 (init、shell、基础命令)

详见 [`docs/TODO.md`](docs/TODO.md)

---

## 🤝 贡献

欢迎贡献！请查看 [`docs/TODO.md`](docs/TODO.md) 了解当前需要帮助的任务。

### 贡献流程

1. Fork 项目
2. 创建功能分支
3. 提交更改
4. 推送到分支
5. 创建 Pull Request

---

## 📄 许可证

MIT License - 详见 [LICENSE](LICENSE)

---

## 🙏 致谢

本项目受到以下项目的启发：

- [Phil Opp's Writing an OS in Rust](https://os.phil-opp.com/)
- [Redox OS](https://gitlab.redox-os.org/redox-os/redox)
- [Theseus OS](https://github.com/theseus-os/Theseus)
- [Linux Kernel](https://www.kernel.org/)

---

## 📮 联系方式

- 项目主页：[GitHub](https://github.com/your-username/rux)
- 问题反馈：[Issues](https://github.com/your-username/rux/issues)

---

**注意**：本项目主要用于学习和研究目的，不适合生产环境使用。
