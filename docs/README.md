# Rux OS 文档中心

欢迎来到 Rux 操作系统内核的文档中心！

## 📚 快速导航

### 🚀 新手入门
- **[快速开始指南](guides/getting-started.md)** - 5 分钟上手 Rux OS
- **[配置系统](guides/configuration.md)** - menuconfig 和编译选项
- **[测试指南](guides/testing.md)** - 运行和编写测试

### 🏗️ 架构设计
- **[设计原则](architecture/design.md)** - POSIX 兼容和 Linux ABI 对齐
- **[代码结构](architecture/structure.md)** - 源码组织和模块划分
- **[RISC-V 架构](architecture/riscv64.md)** - RV64GC 支持详情
- **[启动流程](architecture/boot.md)** - 从 OpenSBI 到内核启动

### 💻 开发指南
- **[开发流程](guides/development.md)** - 贡献代码和开发规范
- **[集合类型](development/collections.md)** - SimpleArc、SimpleVec 等
- **[用户程序](development/user-programs.md)** - ELF 加载和 execve

### 📝 实现文档
- **[用户程序执行](../USER_EXEC_DEBUG.md)** - Linux 风格实现 🆕
  - 单页表设计
  - 用户模式切换
  - 系统调用处理

### 📊 项目进度
- **[开发路线图](progress/roadmap.md)** - Phase 规划和当前状态
- **[代码审查](progress/code-review.md)** - 已知问题和修复记录
- **[快速参考](progress/quickref.md)** - 常用命令和 API 速查
- **[变更日志](development/changelog.md)** - 版本历史和更新记录

### 📦 历史文档
- **[调试档案](archive/README.md)** - 历史调试记录（归档）

## 🎯 项目概述

**Rux** 是一个完全用 Rust 编写的类 Linux 操作系统内核，目标是实现 **100% POSIX 兼容** 和 **Linux ABI 兼容**。

### 核心特性

- ✅ **纯 Rust 实现**（除必要的平台汇编）
- ✅ **多架构支持**（RISC-V64、ARM64）
- ✅ **完整的进程管理**（fork、execve、wait4）
- ✅ **同步原语**（信号量、条件变量）
- ✅ **虚拟内存**（Sv39/4级页表）
- ✅ **SMP 多核**（4 核并发）
- ✅ **VFS 文件系统**（兼容 Linux）

### 开发状态

**当前版本**：v0.1.0 (Phase 15 完成)

**最新更新**：2025-02-09
- ✅ Linux 风格用户程序执行完整实现
- ✅ 单页表设计（U-bit 权限控制）
- ✅ 用户程序成功执行并输出
- ✅ 所有 19 个测试模块通过

详见 [变更日志](development/changelog.md)

## 🤖 AI 辅助开发

本项目使用 **Claude Sonnet 4.5** AI 辅助开发，探索 AI 在操作系统内核开发中的应用。

- 开发工具：[Claude Code CLI](https://claude.ai/code)
- 所有代码遵循 Linux 内核设计原则
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
2. 查看 [代码审查记录](progress/code-review.md) 避免已知问题
3. 阅读 [开发流程](guides/development.md) 了解贡献规范
4. 查看 [测试指南](guides/testing.md) 学习测试方法

### 如果你想深入理解架构
1. 阅读 [RISC-V 架构文档](architecture/riscv64.md)
2. 研究 [启动流程](architecture/boot.md)
3. 查阅 [快速参考](progress/quickref.md)
4. 查看 [归档文档](archive/README.md) 了解历史调试过程

## 🔍 搜索提示

- 按平台查找：架构文档中有 `aarch64`（ARM64）和 `riscv64`（RISC-V）标记
- 按 Phase 查找：路线图中使用 Phase 编号组织开发任务
- 按模块查找：代码结构文档按子系统组织

## 📞 获取帮助

- **问题反馈**：[GitHub Issues](https://github.com/your-username/rux/issues)
- **代码审查**：查看 [CODE_REVIEW.md](progress/code-review.md)
- **开发讨论**：参考 [DEVELOPMENT_WORKFLOW.md](guides/development.md)

---

**注意**：本项目主要用于学习和研究目的，不适合生产环境使用。

最后更新：2025-02-09
