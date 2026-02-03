# Rux 内核项目结构

本文档描述 Rux 内核项目的目录结构和文件组织。

## 目录结构

```
Rux/
├── build/                  # 构建和配置工具
│   ├── Makefile           # 构建脚本
│   ├── menuconfig.sh      # 交互式配置工具
│   └── config-demo.sh     # 配置演示脚本
│
├── test/                   # 测试和调试脚本
│   ├── test_suite.sh      # 完整测试套件
│   ├── test_qemu.sh       # QEMU 测试脚本
│   ├── run.sh             # 快速运行脚本
│   └── debug.sh           # GDB 调试脚本
│
├── docs/                   # 项目文档
│   ├── CONFIG.md          # 配置系统文档
│   ├── DESIGN.md          # 设计文档
│   ├── TODO.md            # 开发路线图
│   └── STRUCTURE.md       # 本文件 - 目录结构说明
│
├── kernel/                 # 内核源代码
│   ├── src/               # Rust 源代码
│   │   ├── arch/         # 架构相关代码
│   │   │   └── aarch64/  # ARM64 架构实现
│   │   │       ├── boot.S    # 启动代码
│   │   │       ├── trap.S    # 异常向量表
│   │   │       ├── boot.rs   # 初始化
│   │   │       ├── trap.rs   # 异常处理
│   │   │       ├── syscall.rs # 系统调用
│   │   │       └── mod.rs    # 模块导出
│   │   ├── drivers/      # 设备驱动
│   │   │   ├── intc/     # 中断控制器
│   │   │   │   ├── mod.rs
│   │   │   │   └── gicv3.rs # GICv3 驱动
│   │   │   ├── timer/    # 定时器驱动
│   │   │   │   ├── mod.rs
│   │   │   │   └── armv8.rs # ARMv8 定时器
│   │   │   └── mod.rs    # 驱动模块导出
│   │   ├── mm/           # 内存管理
│   │   │   ├── allocator.rs # 堆分配器
│   │   │   ├── page.rs      # 页管理
│   │   │   └── mod.rs       # 模块导出
│   │   ├── process/      # 进程管理
│   │   │   ├── task.rs      # 任务控制块
│   │   │   ├── sched.rs     # 调度器
│   │   │   ├── pid.rs       # PID 分配
│   │   │   └── mod.rs       # 模块导出
│   │   ├── console.rs    # 控制台 (UART)
│   │   ├── config.rs     # 自动生成的配置
│   │   ├── main.rs       # 内核入口
│   │   └── print.rs      # 打印宏
│   ├── build.rs          # 构建脚本 (生成 config.rs)
│   ├── Cargo.toml        # 内核 crate 配置
│   └── linker-aarch64.ld # ARM64 链接脚本
│
├── .cargo/                 # Cargo 配置
│   └── config.toml       # Cargo 工具配置
│
├── target/                 # 编译输出 (git忽略)
│   └── aarch64-unknown-none/
│       ├── debug/        # Debug 构建
│       └── release/      # Release 构建
│
├── Kernel.toml            # 内核配置文件
├── Cargo.toml             # 工作空间配置
├── Cargo.lock             # 依赖锁定
├── Makefile               # 项目根 Makefile
├── README.md              # 项目说明
├── LICENSE                # 许可证
└── .gitignore             # Git 忽略规则

```

## 目录说明

### build/ - 构建工具目录

包含所有与构建、配置相关的脚本和工具：

- **Makefile** - 主构建脚本，提供编译、运行、测试等命令
- **menuconfig.sh** - 交互式配置菜单（类似 Linux kernel menuconfig）
- **config-demo.sh** - 配置系统演示脚本

### test/ - 测试目录

包含所有测试和调试脚本：

- **test_suite.sh** - 完整的测试套件，运行所有测试
- **test_qemu.sh** - QEMU 基本功能测试
- **run.sh** - 快速运行内核
- **debug.sh** - GDB 调试脚本

### docs/ - 文档目录

项目文档，包括：

- **CONFIG.md** - 配置系统详细使用说明
- **DESIGN.md** - 内核设计原则和架构
- **TODO.md** - 开发任务和路线图
- **STRUCTURE.md** - 本文件，目录结构说明

### kernel/ - 内核源码

内核的核心源代码：

#### kernel/src/arch/ - 架构相关代码

按 CPU 架构分目录，每个架构包含启动、异常处理、内存管理等。

#### kernel/src/drivers/ - 设备驱动程序

设备驱动按类型组织：
- **intc/** - 中断控制器驱动（GICv3）
- **timer/** - 定时器驱动（ARMv8 Timer）

#### kernel/src/mm/ - 内存管理代码

包含页帧管理和堆分配器。

#### kernel/src/process/ - 进程管理

包含任务控制块、调度器和PID分配器。

#### kernel/src/config.rs - 自动生成的配置常量（不要手动编辑）

### target/ - 编译输出

Cargo 编译生成的文件，已在 .gitignore 中忽略。

## 文件分类

### 配置文件
- `Kernel.toml` - 内核主配置
- `Cargo.toml` - Rust 工作空间配置
- `.cargo/config.toml` - Cargo 工具配置

### 构建脚本
- `Makefile` - 项目根 Makefile
- `build/Makefile` - 详细构建 Makefile
- `kernel/build.rs` - Rust 构建脚本

### 脚本工具
- `build/menuconfig.sh` - 配置菜单
- `build/config-demo.sh` - 配置演示
- `test/run.sh` - 运行内核
- `test/debug.sh` - 调试内核
- `test/test_suite.sh` - 测试套件

### 临时文件（git忽略）
- `*.log` - 日志文件
- `*.bin` - 二进制输出
- `*.dtb` - 设备树文件
- `target/` - 编译输出

## 使用指南

### 编译内核

从项目根目录：

```bash
make build
# 或直接使用 Cargo
cargo build --package rux --features aarch64 --release
```

### 配置内核

```bash
make menuconfig
# 或直接编辑 Kernel.toml
vim Kernel.toml
```

### 运行内核

```bash
make run
# 或
./test/run.sh
```

### 运行测试

```bash
make test
# 或运行完整测试套件
./test/test_suite.sh
```

### 调试内核

```bash
make debug
# 或
./test/debug.sh
```

## 添加新文件

### 新驱动

在 `kernel/src/drivers/` 下创建新模块，如 `drivers/block/`：

1. 创建 `kernel/src/drivers/block/mod.rs`
2. 在 `kernel/src/drivers/mod.rs` 中添加 `pub mod block;`
3. 导出需要的接口：`pub use block::*;`

### 新架构支持

在 `kernel/src/arch/` 下创建新目录，如 `arch/x86_64/`：

1. 创建架构特定目录和文件
2. 在 `kernel/Cargo.toml` 中添加对应的 feature 和配置
3. 添加对应的链接脚本 `kernel/src/linker-x86_64.ld`

### 新测试

在 `test/` 下添加新脚本，确保脚本开头有正确的路径检测：

```bash
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"
```

## 注意事项

1. **config.rs 是自动生成的** - 不要手动编辑 `kernel/src/config.rs`，它由 `kernel/build.rs` 根据 `Kernel.toml` 自动生成。

2. **脚本路径** - 所有脚本都应正确设置 `PROJECT_ROOT`，无论从哪里调用都能正确工作。

3. **临时文件** - 所有临时文件、日志、二进制输出都应在 .gitignore 中。

4. **多平台支持** - 当前主要支持 aarch64，x86_64 和 riscv64 支持正在开发中。

5. **模块导出** - 添加新模块时，确保在父模块的 `mod.rs` 中正确导出需要的接口。
