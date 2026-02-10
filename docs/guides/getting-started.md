# 快速开始指南

欢迎使用 Rux OS！本指南将帮助你在 5 分钟内构建和运行 Rux 内核。

## 环境要求

### 必需工具

- **Rust 工具链**（stable 或 nightly）
  ```bash
  rustc --version
  cargo --version
  ```

- **QEMU 系统模拟器**
  ```bash
  qemu-system-riscv64 --version  # 至少 4.0 版本
  ```

### 可选工具

- **GDB 调试器**（用于调试）
  ```bash
  riscv64-unknown-elf-gdb --version
  ```

## 快速构建

### 1. 克隆仓库

```bash
git clone https://github.com/your-username/rux.git
cd rux
```

### 2. 构建内核

```bash
# 使用默认配置构建（RISC-V 64位）
cargo build --package rux --features riscv64

# 或使用 Makefile
make build
```

### 3. 运行内核

```bash
# 快速测试（推荐）
./test/quick_test.sh

# 或直接使用 QEMU
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux
```

## 预期输出

如果一切正常，你应该看到：

```
OpenSBI v0.9
   ____                    _____ ____ _____
  / __ \                  / ____|  _ \_   _|
 | |  | |_ __   ___ _ __ | (___ | |_) || |
 | |  | | '_ \ / _ \ '_ \ \___ \|  _ < | |
 | |__| | |_) |  __/ | | |____) | |_) || |_
  \____/| .__/ \___|_| |_|_____/|____/_____|

Platform Name             : riscv-virtio,qemu
Platform HART Count       : 4
...
Rux OS v0.1.0 - RISC-V 64-bit
trap: Initializing RISC-V trap handling...
trap: RISC-V trap handling [OK]
mm: Initializing RISC-V MMU (Sv39)...
mm: MMU enabled successfully
smp: Initializing RISC-V SMP...
smp: RISC-V SMP initialized
[OK] Timer interrupt enabled, system ready.
```

## 常用命令

### 构建

```bash
# 构建内核（debug 模式）
cargo build --package rux --features riscv64

# 构建内核（release 模式，优化）
cargo build --package rux --features riscv64 --release

# 构建并运行单元测试
cargo build --package rux --features riscv64,unit-test
```

### 运行

```bash
# 快速测试（推荐日常使用）
./test/quick_test.sh

# 完整运行（支持 SMP 多核）
./test/run_riscv64.sh

# 多核测试（4核）
SMP=4 ./test/run_riscv64.sh

# GDB 调试
./test/debug_riscv.sh
```

### 配置

```bash
# 交互式配置（menuconfig）
make menuconfig

# 查看当前配置
make config

# 编辑配置文件
vim Kernel.toml
```

### 清理

```bash
# 清理构建产物
make clean

# 完全清理（包括依赖）
make distclean
```

## 多平台支持

### RISC-V 64位（默认）

```bash
cargo build --package rux --features riscv64
./test/quick_test.sh
```

### ARM64（已移除，暂不维护）

```bash
# ARM64 架构已移除，暂不维护
# 如需恢复，请参考 git 历史记录
# cargo build --package rux --features aarch64
# qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
#   -kernel target/aarch64-unknown-none/debug/rux
```

### 所有平台

```bash
# 测试所有平台
./test/all.sh

# 仅测试 RISC-V
./test/all.sh riscv

# 仅测试 ARM64
./test/all.sh aarch64
```

## 单元测试

### 运行所有测试

```bash
# 构建测试版本
cargo build --package rux --features riscv64,unit-test

# 运行（会自动运行所有 18 个测试模块）
./test/quick_test.sh
```

### 测试模块

当前测试模块（2025-02-08）：

1. file_open - 文件打开功能
2. listhead - 双向链表
3. path - 路径解析
4. file_flags - 文件标志
5. **fdtable** - 文件描述符管理 ✅ 已修复
6. heap_allocator - 堆分配器
7. page_allocator - 页分配器
8. scheduler - 调度器
9. signal - 信号处理
10. smp - 多核启动
11. process_tree - 进程树管理
12. fork - fork 系统调用
13. execve - execve 系统调用
14. wait4 - wait4 系统调用
15. boundary - 边界条件
16. smp_schedule - SMP 调度
17. getpid - getpid/getppid
18. **arc_alloc** - SimpleArc 分配 ✅ 新增

### 测试输出

测试成功完成示例：

```
test: ===== Starting Rux OS Unit Tests =====
test: Testing file_open...
test: file_open testing completed.
test: Testing FdTable management...
test: FdTable testing completed.
test: Testing SimpleArc allocation...
test: SimpleArc allocation test completed.
test: ===== All Unit Tests Completed =====
test: System halting.
```

## 故障排查

### 编译错误

**问题**：找不到 Rust 目标
```bash
error: target not found
```

**解决**：
```bash
rustup target add riscv64gc-unknown-none-elf
# aarch64 已移除，暂不需要添加
```

### 运行错误

**问题**：QEMU 版本过低
```bash
qemu-system-riscv64: unsupported machine
```

**解决**：升级 QEMU 到 4.0 或更高版本（RISC-V 支持）

**问题**：找不到 OpenSBI
```bash
qemu-system-riscv64: could not load bootloader
```

**解决**：
- QEMU >= 5.0 通常自带 OpenSBI
- 或手动指定 `-bios <path>`

### 测试超时

**问题**：测试运行时间过长

**解决**：
1. 使用 `timeout` 命令限制时间：
   ```bash
   timeout 5 ./test/quick_test.sh
   ```
2. 确认没有其他 QEMU 进程在运行：
   ```bash
   pkill qemu
   ```

### MMU 相关问题

如果遇到 "Load access fault" 或 "Store access fault"：

1. 清理并重新构建：
   ```bash
   make clean && make build
   ```
2. 确认使用正确的内核版本
3. 查看 [MMU 调试档案](../archive/mmu-debug.md)

## 下一步

- 📖 阅读 [设计原则](../architecture/design.md)
- 🏗️ 了解 [代码结构](../architecture/structure.md)
- 🔧 查看 [开发流程](development.md)
- 📊 查看 [开发路线图](../progress/roadmap.md)

## 获取帮助

- **文档中心**：返回 [文档首页](../README.md)
- **问题反馈**：[GitHub Issues](https://github.com/your-username/rux/issues)
- **代码审查**：查看 [代码审查记录](../progress/code-review.md)

---

最后更新：2025-02-08
