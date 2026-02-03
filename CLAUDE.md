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

**Rux** 是一个完全用 Rust 编写的类 Linux 操作系统内核，支持多平台（aarch64、x86_64、riscv64）。

### 核心特征
- **语言**: Rust（no_std，除必要的平台汇编）
- **架构**: aarch64（默认）、x86_64、riscv64
- **目标**: Linux 兼容的操作系统内核
- **阶段**: 早期开发阶段（Phase 1-2）

### 技术栈
- **构建**: Cargo + 自定义 build.rs
- **配置**: TOML + 交互式 menuconfig
- **测试**: QEMU 模拟 + GDB 调试
- **文档**: Markdown + 代码注释

## 项目结构

```
Rux/
├── kernel/                 # 内核核心代码
│   ├── src/
│   │   ├── main.rs       # 内核入口点
│   │   ├── arch/         # 架构相关代码
│   │   │   └── aarch64/  # ARM64 实现
│   │   │       ├── boot.S     # 启动汇编
│   │   │       ├── trap.S     # 异常向量表
│   │   │       ├── boot.rs    # 初始化
│   │   │       ├── trap.rs    # 异常处理
│   │   │       └── mm.rs      # 内存管理
│   │   ├── drivers/      # 设备驱动
│   │   │   ├── uart/         # UART 驱动
│   │   │   ├── timer/        # 定时器驱动
│   │   │   └── intc/         # 中断控制器
│   │   ├── mm/           # 内存管理
│   │   ├── console.rs    # 控制台（UART）
│   │   ├── config.rs     # 自动生成的配置
│   │   └── print.rs      # 打印宏
│   ├── build.rs          # 生成 config.rs
│   └── Cargo.toml
│
├── build/                 # 构建工具
│   ├── Makefile          # 构建脚本
│   ├── menuconfig.sh     # 交互式配置
│   └── config-demo.sh    # 配置演示
│
├── test/                  # 测试脚本
│   ├── test_suite.sh     # 完整测试套件
│   ├── test_qemu.sh      # QEMU 测试
│   ├── run.sh            # 快速运行
│   └── debug.sh          # GDB 调试
│
├── docs/                  # 项目文档
│   ├── CONFIG.md         # 配置系统文档
│   ├── DESIGN.md         # 设计原则
│   ├── TODO.md           # 任务列表
│   ├── STRUCTURE.md      # 目录结构
│   └── QUICKREF.md       # 快速参考
│
├── Kernel.toml           # 内核配置文件
├── Cargo.toml            # 工作空间配置
├── Makefile              # 根 Makefile（快捷方式）
└── README.md             # 项目说明
```

## 关键文件说明

### 配置文件
- **Kernel.toml** - 内核主配置（编译时读取）
- **Cargo.toml** - Rust 工作空间配置
- **.cargo/config.toml** - Cargo 工具链配置

### 自动生成
- **kernel/src/config.rs** - 由 build.rs 根据 Kernel.toml 自动生成，不要手动编辑

### 重要脚本
- **build/Makefile** - 详细构建命令
- **test/test_suite.sh** - 完整测试套件
- **test/run.sh** - 快速运行内核

### 汇编文件
- **kernel/src/arch/aarch64/boot.S** - 启动代码、EL检测
- **kernel/src/arch/aarch64/trap.S** - 异常向量表（16个入口，2KB对齐）

## 当前实现状态

### ✅ 已完成（Phase 1）
1. 启动框架（boot.S）
2. 异常向量表（trap.S）
3. 异常处理框架（trap.rs）
4. UART 控制台驱动
5. ARMv8 定时器驱动
6. GICv3 中断控制器驱动
7. 基础内存管理（页帧、堆分配器）
8. 配置系统（menuconfig）
9. 构建和测试脚本

### 🔄 进行中（Phase 2）
1. 进程调度器
2. 上下文切换
3. 地址空间管理（VMA）
4. 系统调用接口

### ⏳ 待实现（Phase 3-9）
1. 文件系统（VFS、ext4、btrfs）
2. 网络协议栈（TCP/IP）
3. IPC（管道、消息队列、共享内存）
4. x86_64 和 riscv64 平台支持
5. 用户空间工具

## 常见开发任务

### 编译和运行
```bash
make build          # 编译内核
make run            # 在 QEMU 中运行
make test           # 运行测试套件
make clean          # 清理构建产物
```

### 配置内核
```bash
make menuconfig     # 交互式配置
vim Kernel.toml     # 手动编辑配置
make config         # 查看当前配置
```

### 调试
```bash
make debug          # GDB 调试
./test/debug.sh     # 详细调试脚本
```

### 添加新功能
1. 查阅 `docs/TODO.md` 找到相关任务
2. 在 `kernel/src/` 相应目录创建模块
3. 更新 `Kernel.toml` 配置（如需要）
4. 添加测试到 `test/`
5. 更新文档

## 架构特定信息

### aarch64（当前默认）
- **入口点**: 0x40000000
- **页大小**: 4096 字节
- **异常级别**: EL1（内核运行）
- **UART 基址**: 0x0900_0000（PL011）
- **定时器**: ARMv8 架构定时器
- **中断控制器**: GICv3

### 启动流程
1. **boot.S**: EL 检测，降级到 EL1
2. **设置栈**: 使用 16KB 栈空间
3. **清零 BSS**: 清零未初始化数据
4. **跳转到 _start**: Rust 代码入口
5. **初始化**: UART → 异常 → 定时器 → 中断

### 内存布局
```
0x4000_0000  内核代码和只读数据
0x4001_8000  内核数据段
0x4001_9000  BSS 段（清零）
0x4001_9000  栈空间（16KB）
0x4001_D000  页表（16KB）
```

## 代码约定

### Rust 代码
- **no_std**: 不使用标准库
- **panic 策略**: 终止（hang）
- **内联汇编**: 使用 `core::arch::asm!`
- **全局汇编**: 使用 `core::arch::global_asm!`

### 汇编代码
- **语法**: ARM64 (GNU as)
- **对齐**: 异常表 2KB 对齐（`.align 11`）
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

## 测试和调试

### QEMU 命令
```bash
# 基本运行
qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
  -kernel target/aarch64-unknown-none/debug/rux

# 调试模式（GDB）
qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
  -kernel target/aarch64-unknown-none/debug/rux -S -s

# 查看输出
qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
  -serial mon:stdio -kernel target/aarch64-unknown-none/debug/rux
```

### GDB 调试
```bash
# 连接到 QEMU
aarch64-none-elf-gdb target/aarch64-unknown-none/debug/rux
(gdb) target remote localhost:1234
(gdb) break *0x40000000
(gdb) continue
```

## 故障排查

### 编译问题
- 检查 Rust 版本：`rustc --version`
- 清理构建：`make clean`
- 检查目标工具链：`ls ~/.rustup/toolchains`

### 运行问题
- 检查 QEMU 版本：`qemu-system-aarch64 --version`
- 检查内核编译：`ls target/aarch64-unknown-none/debug/rux`
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

## 相关资源

- **设计文档**: [docs/DESIGN.md](docs/DESIGN.md)
- **任务列表**: [docs/TODO.md](docs/TODO.md)
- **快速参考**: [docs/QUICKREF.md](docs/QUICKREF.md)
- **结构说明**: [docs/STRUCTURE.md](docs/STRUCTURE.md)
- **配置指南**: [docs/CONFIG.md](docs/CONFIG.md)

## 项目上下文

### 开发阶段
- **当前**: Phase 1 完成，Phase 2 进行中
- **目标**: 完整的类 Linux 操作系统内核
- **进度**: 约 10% 完成（基础框架）

### 技术亮点
- 全 Rust 编写（除必要汇编）
- 多平台支持设计
- 模块化架构
- Linux 兼容目标

### 已知限制
- 仅支持 QEMU virt 机器
- GIC 地址需要调试
- 暂无用户空间
- 单核支持

## 协作提示

当帮助用户时：
1. 优先查看 `docs/TODO.md` 了解项目状态
2. 检查相关文档确认技术细节
3. 使用 `make` 命令而非直接调用 cargo
4. 注意 `config.rs` 是自动生成的
5. 遵循现有的代码风格和组织方式

### 关键约束
- **no_std**: 不能使用 Rust 标准库
- **平台汇编**: 必须用汇编编写启动代码
- **QEMU 依赖**: 开发和测试依赖 QEMU
- **单核设计**: 当前不支持 SMP

## 更新日志

### 2025-02-02
- 完成项目结构重组
- 添加配置系统（menuconfig）
- 实现异常处理框架
- 完善文档和测试脚本
