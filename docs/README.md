# Rux OS 文档中心

欢迎来到 Rux 操作系统内核的文档中心！

## 📚 快速导航

### 🚀 新手入门
- **[快速开始指南](guides/getting-started.md)** - 5 分钟上手 Rux OS
- **[配置系统](guides/configuration.md)** - menuconfig 和编译选项
- **[开发流程](guides/development.md)** - 贡献代码和开发规范

### 🏗️ 架构设计
- **[设计原则](architecture/design.md)** - POSIX 兼容和 Linux ABI 对齐
- **[代码结构](architecture/structure.md)** - 源码组织和模块划分
- **[RISC-V 架构](architecture/riscv64.md)** - RV64GC 支持详情
- **[启动流程](architecture/boot.md)** - 从 OpenSBI 到内核启动
- **[内存管理](architecture/memory.md)** - 物理内存、虚拟内存、分配器设计 🆕

### 💻 开发指南
- **[测试指南](guides/testing.md)** - 51 个内核测试 + 24 个 mini-ltp 兼容性测试

### 📊 项目进度
- **[开发路线图](progress/roadmap.md)** - Phase 规划和当前状态 (Phase 24)
- **[快速参考](progress/quickref.md)** - 常用命令和 API 速查
- **[变更日志](progress/changelog.md)** - 版本历史和更新记录

### 📦 历史文档
- **[调试档案](archive/README.md)** - 历史调试记录（归档）
- **[代码审查记录](archive/code-review.md)** - 已知问题和修复记录

## 🎯 项目概述

**Rux** 是一个完全用 Rust 编写的类 Linux 操作系统内核，目标是实现 **100% POSIX 兼容** 和 **Linux ABI 兼容**。

### 核心特性

- ✅ **纯 Rust 实现**（除必要的平台汇编）
- ✅ **RISC-V 64位架构**（唯一支持的架构）
- ✅ **完整的进程管理**（fork、execve、wait4、信号处理、COW）
- ✅ **CFS 调度器**（类似 Linux 的公平调度器）
- ✅ **虚拟内存**（Sv39 3级页表、Buddy 分配器、Slab 分配器）
- ✅ **SMP 多核**（4 核并发、IPI、负载均衡）
- ✅ **VFS 文件系统**（ext4、ramfs、procfs、devfs）
- ✅ **网络协议栈**（TCP/UDP/IPv4/ARP）
- ✅ **设备驱动**（VirtIO-blk/net/gpu/input）
- ✅ **GUI 桌面**（桌面环境、计算器、时钟、可视化 Shell）

### 开发状态

**当前版本**：v0.1.0 (Phase 24 完成)

**最新更新**：2026-03-04
- ✅ **devfs 文件系统** - 设备文件系统，替换自定义系统调用
- ✅ **mini-ltp 测试** - 24 个内核兼容性测试
- ✅ **COW 完善** - Copy-on-Write 页表处理修复
- ✅ **CFS 调度器** - 完全公平调度器实现
- ✅ **GUI 桌面** - 桌面环境、计算器、时钟应用
- ✅ **51 个内核测试** + **24 个 mini-ltp 测试**

**代码统计**：~56,600 行 Rust 代码，178 个源文件

详见 [变更日志](progress/changelog.md)

## 🤖 AI 辅助开发

本项目使用 **Claude Code + GLM5** AI 辅助开发，探索 AI 在操作系统内核开发中的应用。

- 开发工具：[Claude Code CLI](https://claude.ai/code)
- 所有代码遵循 Linux 内核设计原则和 POSIX 标准
- 开发者负责审查和测试所有 AI 生成的代码

详见 [CLAUDE.md](../CLAUDE.md)

## 📖 文档阅读路径

### 如果你是新开发者
1. 阅读 [快速开始指南](guides/getting-started.md)
2. 了解 [设计原则](architecture/design.md)
3. 查看 [代码结构](architecture/structure.md)
4. 跟随 [开发流程](guides/development.md)

### 如果你想贡献代码
1. 阅读 [开发路线图](progress/roadmap.md) 了解待完成任务
2. 查看 [代码审查记录](archive/code-review.md) 避免已知问题
3. 阅读 [开发流程](guides/development.md) 了解贡献规范
4. 查看 [测试指南](guides/testing.md) 学习测试方法

### 如果你想深入理解架构
1. 阅读 [RISC-V 架构文档](architecture/riscv64.md)
2. 研究 [启动流程](architecture/boot.md)
3. 阅读 [内存管理设计](architecture/memory.md)
4. 查阅 [快速参考](progress/quickref.md)
5. 查看 [归档文档](archive/README.md) 了解历史调试过程

## 📁 文档目录结构

```
docs/
├── README.md              # 本文件
├── architecture/          # 架构设计文档
│   ├── design.md          # 设计原则
│   ├── structure.md       # 代码结构
│   ├── riscv64.md         # RISC-V 架构
│   ├── boot.md            # 启动流程
│   └── memory.md          # 内存管理 🆕
├── guides/                # 开发指南
│   ├── getting-started.md # 快速开始
│   ├── configuration.md   # 配置系统
│   ├── development.md     # 开发流程
│   └── testing.md         # 测试指南
├── progress/              # 项目进度
│   ├── roadmap.md         # 开发路线图
│   ├── quickref.md        # 快速参考
│   └── changelog.md       # 变更日志
├── development/           # 开发记录
│   └── fork-exec-debug-report.md  # Fork+Exec 调试报告
└── archive/               # 历史文档归档
    ├── README.md          # 归档索引
    ├── code-review.md     # 代码审查记录
    └── ...                # 其他历史文档
```

## 🔍 搜索提示

- 按 Phase 查找：路线图中使用 Phase 编号组织开发任务
- 按模块查找：代码结构文档按子系统组织
- 按功能查找：测试指南按功能模块分类

## 📞 获取帮助

- **问题反馈**：[GitHub Issues](https://github.com/topkernel/rux/issues)
- **代码审查**：查看 [代码审查记录](archive/code-review.md)
- **开发讨论**：参考 [开发流程](guides/development.md)

---

**注意**：本项目主要用于学习和研究目的，不适合生产环境使用。

最后更新：2026-03-04
