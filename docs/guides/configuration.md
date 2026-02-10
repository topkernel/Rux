# Rux 内核配置系统使用指南

## 概述

Rux 内核提供了灵活的配置系统，支持通过 `Kernel.toml` 文件或交互式菜单来配置内核选项。

## 配置方式

### 方式 1: 直接编辑 Kernel.toml

```toml
[general]
name = "Rux"          # 内核名称
version = "0.1.0"     # 版本号

[platform]
default_platform = "riscv64"  # 目标平台（默认且仅支持）

[memory]
kernel_heap_size = 16         # 内核堆大小 (MB)
physical_memory = 2048        # 物理内存 (MB)

[features]
enable_process = false        # 启用进程管理
enable_vfs = false            # 启用虚拟文件系统
enable_network = false        # 启用网络协议栈

[debug]
log_level = "info"            # 日志级别
debug_output = true           # 调试输出
```

修改后运行：
```bash
cargo build --package rux --features riscv64
```

### 方式 2: 使用交互式配置菜单

```bash
./menuconfig.sh
```

这将启动一个类似 Linux kernel menuconfig 的图形化配置界面：

```
┌─────────────────────────────────────────────┐
│     Rux Kernel Configuration                │
├─────────────────────────────────────────────┤
│                                             │
│  选择配置类别:                               │
│                                             │
│  1. General     - 基本信息                  │
│  2. Platform    - 平台设置                  │
│  3. Memory      - 内存配置                  │
│  4. Features    - 功能特性                  │
│  5. Drivers     - 驱动配置                  │
│  6. Debug       - 调试选项                  │
│  7. Performance - 性能调优                  │
│  8. Security    - 安全选项                  │
│                                             │
│  <Ok>          <Cancel>                     │
└─────────────────────────────────────────────┘
```

## 配置类别

### 1. General（基本信息）
- 内核名称
- 版本号
- 开发者信息

### 2. Platform（平台）
- riscv64 - RISC-V 64位（默认且仅支持）
- aarch64 - ARM 64位（已移除，暂不维护）
- x86_64 - x86 64位（未实现）

### 3. Memory（内存）
- 内核堆大小
- 物理内存大小
- 页大小

### 4. Features（功能特性）
- 进程管理
- 调度器
- 虚拟文件系统
- 网络协议栈
- IPC机制

### 5. Drivers（驱动）
- UART驱动
- 定时器驱动
- PLIC中断控制器（RISC-V）
- CLINT定时器（RISC-V）
- VirtIO设备
- PCI设备（待实现）

### 6. Debug（调试）
- 日志级别: error, warn, info, debug, trace
- 调试输出
- 性能分析
- 内存跟踪

### 7. Performance（性能）
- 优化级别 (0-3)
- 链接时优化 (LTO)
- 代码生成单元

### 8. Security（安全）
- 栈保护
- 边界检查
- 溢出检查

## 工作流程

```
┌─────────────┐
│ Kernel.toml │  ← 编辑配置文件或使用 menuconfig
└──────┬──────┘
       │
       ▼
┌─────────────┐
│  build.rs   │  ← 解析 TOML，生成 Rust 代码
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ config.rs   │  ← 自动生成的配置常量
└──────┬──────┘
       │
       ▼
┌─────────────┐
│   编译内核   │
└─────────────┘
```

## 快速开始

1. **查看当前配置**
   ```bash
   cat Kernel.toml
   ```

2. **修改配置**
   ```bash
   # 方式 A: 直接编辑
   vim Kernel.toml

   # 方式 B: 使用菜单
   ./menuconfig.sh
   ```

3. **编译内核**
   ```bash
   cargo build --package rux --features riscv64
   ```

4. **运行内核**
   ```bash
   qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
     -bios default -kernel target/riscv64gc-unknown-none-elf/debug/rux
   ```

## 配置示例

### 最小配置（嵌入式系统）
```toml
[memory]
kernel_heap_size = 4
physical_memory = 128

[features]
enable_process = false
enable_vfs = false

[debug]
log_level = "error"
```

### 完整配置（桌面系统）
```toml
[memory]
kernel_heap_size = 32
physical_memory = 4096

[features]
enable_process = true
enable_scheduler = true
enable_vfs = true
enable_network = true
enable_pipe = true

[drivers]
enable_uart = true
enable_timer = true
enable_gic = true
enable_pci = true

[debug]
log_level = "debug"
```

## 注意事项

1. **配置文件路径**: `Kernel.toml` 必须在项目根目录
2. **自动生成**: `kernel/src/config.rs` 是自动生成的，不要手动编辑
3. **编译触发**: 修改 `Kernel.toml` 后会自动触发重新编译
4. **平台切换**: 切换目标平台需要相应的交叉编译工具链

## 故障排查

### 配置未生效
```bash
# 清理并重新编译
cargo clean
cargo build --package rux --features riscv64
```

### 查看生成的配置
```bash
cat kernel/src/config.rs
```

### 验证配置值
```bash
grep "KERNEL_NAME\|KERNEL_VERSION" kernel/src/config.rs
```
