# Rux 内核项目 - AI 助手指南

本文档为 Claude Code 等 AI 助手提供项目上下文和开发指南。

## ⚠️ 最高原则（绝对不可违反）

### **POSIX/ABI 完全兼容，绝不创新**

这是 Rux 内核开发的**最高指导原则**，所有设计和实现决策都必须服从于此原则。

**核心要求**：
- **100% POSIX 兼容**：完全遵守 POSIX 标准
- **Linux ABI 完全兼容**：与 Linux 内核 ABI 二进制兼容
- **系统调用兼容**：使用 Linux 的系统调用号
- **文件系统兼容**：支持 Linux 文件系统（ext4、btrfs）
- **ELF 格式兼容**：可执行文件格式与 Linux 一致

**严格禁止**：
- ❌ 绝不"优化" Linux 的设计
- ❌ 绝不创造新的系统调用
- ❌ 绝不改变现有接口的行为
- ❌ 绝不"重新发明轮子"
- ❌ 绝不为了"更优雅"而偏离标准

**实现方式**：
1. 直接参考 Linux 内核源码
2. 使用相同的系统调用号（`arch/x86/entry/syscalls`）
3. 使用相同的结构体布局
4. 使用相同的文件系统格式
5. 遵循 POSIX 标准

> **关键**：我们的目标是用 Rust 重写 Linux 内核，而不是创造新系统。任何偏离 Linux 标准的"创新"都是错误的。

**参考资源**：
- Linux 内核源码：https://elixir.bootlin.com/linux/latest/source/
- Linux man pages（`man 2 syscall`）
- Linux 内核文档：Documentation/
- POSIX 标准：https://pubs.opengroup.org/onlinepubs/9699919799/

---

## 项目概述

**Rux** 是一个完全用 Rust 编写的类 Linux 操作系统内核。

### 核心特征
- **语言**: Rust（no_std，除必要的平台汇编）
- **架构**: **仅支持 RISC-V (riscv64)**
  - ✅ RISC-V 64位 - 完全支持，当前默认架构
  - ❌ ARM64 (aarch64) - 已移除，暂不维护
  - ❌ x86_64 - 未实现
- **目标**: Linux 兼容的操作系统内核
- **阶段**: Phase 17 完成（RISC-V 架构完全实现）

### 技术栈
- **构建**: Cargo + 自定义 build.rs
- **配置**: TOML + 交互式 menuconfig
- **测试**: QEMU 模拟 + GDB 调试
- **文档**: Markdown + 代码注释


## 关键文件说明

### 配置文件
- **Kernel.toml** - 内核主配置（编译时读取）
- **Cargo.toml** - Rust 工作空间配置
- **.cargo/config.toml** - Cargo 工具链配置

### 自动生成
- **kernel/src/config.rs** - 由 build.rs 根据 Kernel.toml 自动生成，不要手动编辑

### 重要脚本
- **build/Makefile** - 详细构建命令
- **test/run.sh** - 快速运行内核

## 常见开发任务

### 编译和运行
```bash
# 构建内核
make build

# 构建用户态程序
make user

# 构建Rootfs
make rootfs

# 运行内核
make run  #启动默认的shell，rust + no_std

# 运行单元测试
make test
```

### 添加新功能
1. 查阅 `docs/progress/roadmap.md` 找到相关任务
2. 在 `kernel/src/` 相应目录创建模块
3. 更新 `Kernel.toml` 配置（如需要）
4. 添加测试到 `test/`
5. 更新文档

## 架构特定信息

### riscv64（当前默认且唯一支持）
- **入口点**: 0x80200000
- **页大小**: 4096 字节
- **异常级别**: S-mode（内核运行）
- **UART 基址**: 0x10000000（ns16550a）
- **定时器**: RISC-V 架构定时器
- **中断控制器**: PLIC (Platform-Level Interrupt Controller)

### 启动流程
1. **boot.S**: OpenSBI 初始化，跳入内核
2. **设置栈**: 使用足够大的栈空间
3. **清零 BSS**: 清零未初始化数据
4. **跳转到 _start**: Rust 代码入口
5. **初始化**: UART → 异常 → 定时器 → 中断

### 内存布局
```
0x80200000  内核代码段
0x80210000  内核数据段
...          BSS 段、栈空间等
```

## 代码约定

### Rust 代码
- **no_std**: 不使用标准库
- **panic 策略**: 终止（hang）
- **内联汇编**: 使用 `core::arch::asm!`
- **全局汇编**: 使用 `core::arch::global_asm!`

### 汇编代码
- **语法**: RISC-V (GNU as)
- **对齐**: 按需对齐
- **注释**: 使用 C 风格（`//` 或 `/* */`）

### 文件组织
- 每个架构在 `kernel/src/arch/*/` 有自己的目录
- 驱动在 `kernel/src/drivers/` 下按类型组织
- 使用模块化的 `mod.rs` 导出公共接口

## 配置系统

### Kernel.toml 解析
1. `kernel/build.rs` 在编译时读取 `Kernel.toml`
2. 生成 `kernel/src/config.rs`（包含常量定义）
3. 内核代码通过 `crate::config::*` 使用配置

### 添加新配置项
1. 在 `Kernel.toml` 添加配置项
2. 在 `kernel/build.rs` 的 `generate_config_code()` 中添加解析
3. 在 `kernel/src/config.rs` 添加对应的常量

## 故障排查

### 编译问题
- 检查 Rust 版本：`rustc --version`
- 清理构建：`make clean`
- 检查目标工具链：`ls ~/.rustup/toolchains`

### 运行问题
- 检查 QEMU 版本：`qemu-system-riscv64 --version`
- 检查内核编译：`ls target/riscv64gc-unknown-none-elf/debug/rux`
- 查看输出：使用 `-serial mon:stdio` 选项

### 配置未生效
- 检查 `kernel/src/config.rs` 是否更新
- 清理并重新编译：`make clean && make build`

## 遵循"不创新"原则的开发指南

### 添加新功能时

**必须先做**：
1. 查阅 Linux 内核源码中对应功能的实现
2. 阅读相关的 Linux man pages
3. 查阅 POSIX 标准文档
4. 确认使用相同的接口和数据结构

**禁止行为**：
- ❌ 觉自己的"理解"修改接口
- ❌ 为了"更简洁"改变设计
- ❌ 认为"Linux 的设计太老"而更新
- ❌ 创造新的抽象或接口

### 具体实现指导

#### 系统调用
```rust
// ✅ 正确：使用 Linux 的系统调用号
pub const __NR_read: usize = 63;   // 与 Linux 完全一致

// ❌ 错误：自己定义系统调用号
pub const SYS_RUX_READ: usize = 1000;  // 绝对禁止！
```

#### 数据结构
```rust
// ✅ 正确：参考 Linux 的结构体
#[repr(C)]
pub struct Stat {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    st_size: i64,
    st_blksize: u64,
    st_blocks: u64,
    st_atime: u64,
    st_mtime: u64,
    st_ctime: u64,
    // 字段顺序、大小、对齐与 Linux 完全一致
}

// ❌ 错误：自己定义结构体
#[repr(C)]
pub struct RuxStat {
    // ... 不要自己发明结构！
}
```

#### 文件系统
```rust
// ✅ 正确：实现 ext4 文件系统
// 参考 Linux fs/ext4/ 目录的实现

// ❌ 错误：发明新的文件系统
// 不要创建 RuxFS 或其他"改进型"文件系统
```

### 代码审查检查点

在审查代码时，必须检查：

1. [ ] 系统调用号是否与 Linux 一致？
2. [ ] 数据结构布局是否与 Linux 一致？
3. [ ] 是否遵循了 POSIX 标准？
4. [ ] 是否参考了 Linux 内核源码？
5. [ ] 是否包含了任何"创新"？

如果发现违反原则的代码，必须拒绝并要求修改。

## 开发建议
1. 确定修改的模块（drivers/arch/mm 等）
2. 编辑 Rust 代码
3. 运行 `make build` 编译
4. 运行 `make test` 测试
5. 运行 `make run` 验证

### 添加新平台支持
1. 创建 `kernel/src/arch/<platform>/` 目录
2. 实现 boot.S、trap.rs、mm.rs 等
3. 添加链接器脚本
4. 配置 Cargo target
5. 更新文档

### 添加新驱动
1. 在 `kernel/src/drivers/` 下创建子目录
2. 实现驱动 trait/struct
3. 在 `Kernel.toml` 添加配置选项
4. 在初始化代码中注册驱动


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