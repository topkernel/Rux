# Rux 内核快速参考

## 项目结构

```
Rux/
├── build/     - 构建工具 (make build/config/menuconfig)
├── test/       - 测试脚本 (test_suite.sh, run.sh, debug.sh)
├── docs/       - 文档 (CONFIG.md, DESIGN.md, STRUCTURE.md)
├── kernel/     - 内核源码
├── Kernel.toml - 内核配置
└── Makefile    - 快捷命令
```

## 常用命令

### 编译相关
```bash
make build           # 编译内核
make build-quiet     # 静默编译
make clean           # 清理构建产物
make bin             # 生成二进制文件
```

### 配置相关
```bash
make config          # 查看当前配置
make menuconfig      # 交互式配置菜单
vim Kernel.toml      # 手动编辑配置
```

### 运行相关
```bash
make run             # 运行内核 (QEMU)
make test            # 运行测试套件
make debug           # GDB 调试
```

### 信息相关
```bash
make info            # 显示项目信息
make help            # 显示帮助
make deps            # 检查依赖
```

## 目录功能

### build/ - 构建工具
- **Makefile** - 详细构建脚本，支持所有构建任务
- **menuconfig.sh** - 交互式配置菜单（类似 Linux kernel）
- **config-demo.sh** - 配置系统演示

### test/ - 测试脚本
- **test_suite.sh** - 完整测试套件
- **test_qemu.sh** - QEMU 功能测试
- **run.sh** - 快速运行内核
- **debug.sh** - GDB 调试脚本

### docs/ - 文档
- **CONFIG.md** - 配置系统详细文档
- **DESIGN.md** - 内核设计文档
- **STRUCTURE.md** - 目录结构说明
- **TODO.md** - 开发任务列表

## 配置文件

### Kernel.toml - 内核配置

```toml
[general]
name = "Rux"              # 内核名称
version = "0.1.0"         # 版本号

[platform]
default_platform = "aarch64"  # 目标平台

[memory]
kernel_heap_size = 16     # 堆大小 (MB)
physical_memory = 2048    # 物理内存 (MB)
page_size = 4096          # 页大小

[features]
enable_process = false    # 进程管理
enable_vfs = false        # 文件系统
enable_network = false    # 网络

[drivers]
enable_uart = true        # UART 驱动
enable_timer = true       # 定时器驱动
enable_gic = false        # GIC 中断控制器

[debug]
log_level = "info"        # 日志级别
debug_output = true       # 调试输出
```

修改配置后运行 `make build` 重新编译。

## 工作流程

### 开发流程
1. 编辑内核代码 (`kernel/src/`)
2. 编译 (`make build`)
3. 测试 (`make test`)
4. 调试 (`make debug`)

### 配置流程
1. 修改配置 (`make menuconfig` 或编辑 `Kernel.toml`)
2. 编译 (`make build`)
3. 运行 (`make run`)

## 快速开始

```bash
# 首次构建
make build

# 运行内核
make run

# 查看配置
make config

# 运行测试
make test

# 清理
make clean
```

## 架构支持

### aarch64 (默认)
```bash
make build                          # 编译
qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
  -kernel target/aarch64-unknown-none/debug/rux
```

### x86_64 (待实现)
```bash
# 需要先实现 x86_64 平台支持
make build CARGO_BUILD_FLAGS="--target x86_64-unknown-none"
```

### riscv64 (待实现)
```bash
# 需要先实现 riscv64 平台支持
make build CARGO_BUILD_FLAGS="--target riscv64-unknown-none"
```

## 故障排查

### 编译失败
```bash
make clean
make build
```

### QEMU 无法运行
```bash
# 检查 QEMU 是否安装
qemu-system-aarch64 --version

# 检查内核是否编译
ls target/aarch64-unknown-none/debug/rux
```

### 配置未生效
```bash
# 检查生成的配置
cat kernel/src/config.rs

# 清理并重新编译
make clean
make build
```

## 脚本路径说明

所有脚本都使用相对路径自动定位项目根目录：

```bash
# 从任何目录都可以调用
cd build && make build      # ✓ 正确
cd test && ./run.sh          # ✓ 正确
cd .. && make build          # ✓ 正确
```

## 更多信息

- **AI 助手指南**: [CLAUDE.md](CLAUDE.md) - 为 Claude Code 等准备的项目概览
- **项目说明**: [README.md](README.md) - 面向用户的介绍
- **配置系统**: [docs/CONFIG.md](docs/CONFIG.md)
- **设计文档**: [docs/DESIGN.md](docs/DESIGN.md)
- **目录结构**: [docs/STRUCTURE.md](docs/STRUCTURE.md)
- **任务列表**: [TODO.md](TODO.md)
