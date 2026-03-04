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
  qemu-system-riscv64 --version  # 至少 5.0 版本
  ```

- **RISC-V 交叉编译工具链**（用于用户程序）
  ```bash
  riscv64-linux-gnu-gcc --version
  ```

### 可选工具

- **GDB 调试器**（用于调试）
  ```bash
  riscv64-unknown-elf-gdb --version
  ```

## 快速构建

### 1. 克隆仓库

```bash
git clone https://github.com/topkernel/rux.git
cd rux
```

### 2. 添加 Rust 目标

```bash
rustup target add riscv64gc-unknown-none-elf
rustup target add riscv64gc-unknown-linux-musl
```

### 3. 构建项目

```bash
# 构建内核
make build

# 构建用户态程序 (shell, apps, mini-ltp, toybox)
make user

# 构建 Rootfs 镜像
make rootfs
```

### 4. 运行内核

```bash
# 运行内核 (默认 shell)
make run

# 运行 GUI 桌面
make gui

# 运行单元测试
make test
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
        | |
        |_|

Platform Name             : riscv-virtio,qemu
Platform HART Count       : 4
...


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
mm:               user frame allocator 64MB          [ok]
mm:               16384 page descriptors             [ok]
intc:             PLIC @ 0x0C000000                  [ok]
intc:             external IRQ routing               [ok]
ipi:              SSIP software IRQ                  [ok]
bio:              buffer cache layer                 [ok]
fs:               ext4 driver loaded                 [ok]
fs:               ramfs mounted /                    [ok]
fs:               procfs mounted /proc               [ok]
fs:               devfs mounted /dev                 [ok]
driver:           virtio-blk PCI x1                  [ok]
driver:           virtio-net x1                      [ok]
driver:           virtio-gpu x1                      [ok]
driver:           virtio-input x1                    [ok]
sched:            CFS scheduler v1                   [ok]
trap:             sie.SEIE enabled                   [ok]
init:             loading /bin/shell                 [ok]
init:             ELF loaded to user space           [ok]
init:             init task (PID 1) enqueued         [ok]

/bin/shell#
```

## 常用命令

### 构建

```bash
# 构建内核（debug 模式）
make build

# 构建内核（release 模式，优化）
make build RELEASE=1

# 构建用户态程序
make user

# 构建 Rootfs 镜像
make rootfs

# 构建并运行单元测试
make test
```

### 运行

```bash
# 运行内核 (默认 shell)
make run

# 运行 GUI 桌面
make gui

# GDB 调试
make debug
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
# 清理内核构建产物
make clean

# 清理用户程序构建产物
make clean-user

# 完全清理
make distclean
```

## 多平台支持

### RISC-V 64位（唯一支持）

```bash
make build
make run
```

**注意**: ARM64 (aarch64) 架构已移除，暂不维护。

## Shell 使用

Rux 启动后会进入默认的 shell。内置命令：

```bash
/bin/shell# echo "Hello Rux!"
Hello Rux!

/bin/shell# pid
PID: 1, PPID: 0

/bin/shell# time
Uptime: 12345 ms

/bin/shell# help
Built-in commands: echo, help, exit, time, pid

/bin/shell# ls /
bin  app  test  dev  proc  tmp  var  etc  lib

/bin/shell# ls /app
desktop  calculator  clock  vshell

/bin/shell# /app/desktop
# 启动桌面环境 (需要 GUI 支持)
```

## 运行测试

### 内核单元测试

```bash
make test
```

测试模块分类（51 个测试文件）：

**内存测试**
- heap_allocator, page_allocator, standard_alloc
- mem_mmap, mem_cow

**进程/调度测试**
- fork, getpid, wait4, process_tree
- scheduler, preemptive_scheduler, sleep_wakeup
- smp, smp_schedule

**文件系统测试**
- file_open, file_flags, fdtable, path
- dcache, icache, link, fcntl, fstat, mkdir_unlink
- ext4_allocator, ext4_file_write, ext4_indirect_blocks

**IPC 测试**
- pipe2, ipc_poll, ipc_epoll, ipc_eventfd

**信号测试**
- signal, signal_procmask

**网络测试**
- network, tcp_handshake

**驱动测试**
- virtio_queue, virtio_net, framebuffer

**系统调用测试**
- syscall_file, syscall_memory, syscall_process
- syscall_sched, syscall_signal, syscall_network
- syscall_io, syscall_time, syscall_misc

### mini-ltp 内核兼容性测试

```bash
# 在 Rux shell 中运行
cd /test/mini-ltp
./run_tests.sh
```

24 个测试覆盖核心系统调用：
- test_fork, test_getpid, test_fileio, test_pipe
- test_dup, test_mmap, test_stat, test_mkdir
- test_lseek, test_time, test_wait, test_exit
- test_brk, test_chdir, test_rename, test_unlink
- test_access, test_writev, test_execve, test_getuid
- test_nanosleep, test_ioctl, test_fcntl, test_fsync

## 故障排查

### 编译错误

**问题**：找不到 Rust 目标
```bash
error: target not found
```

**解决**：
```bash
rustup target add riscv64gc-unknown-none-elf
rustup target add riscv64gc-unknown-linux-musl
```

**问题**：缺少交叉编译工具链
```bash
riscv64-linux-gnu-gcc: command not found
```

**解决**：
```bash
# Ubuntu/Debian
sudo apt-get install gcc-riscv64-linux-gnu

# Arch Linux
sudo pacman -S riscv64-linux-gnu-gcc
```

### 运行错误

**问题**：QEMU 版本过低
```bash
qemu-system-riscv64: unsupported machine
```

**解决**：升级 QEMU 到 5.0 或更高版本

**问题**：找不到 OpenSBI
```bash
qemu-system-riscv64: could not load bootloader
```

**解决**：
- QEMU >= 5.0 通常自带 OpenSBI
- 或手动指定 `-bios <path>`

**问题**：Rootfs 镜像不存在
```bash
fs: ext4 mount failed
```

**解决**：
```bash
make user
make rootfs
```

### 测试超时

**问题**：测试运行时间过长

**解决**：
1. 确认没有其他 QEMU 进程在运行：
   ```bash
   pkill qemu
   ```
2. 使用 release 模式构建：
   ```bash
   make build RELEASE=1
   ```

### MMU 相关问题

如果遇到 "Load access fault" 或 "Store access fault"：

1. 清理并重新构建：
   ```bash
   make clean && make build
   ```
2. 确认使用正确的内核版本

## rootfs 目录结构

```
/
├── bin/          # 基本命令
│   ├── shell     # Shell
│   ├── sh        # Shell 符号链接
│   ├── toybox    # Toybox
│   └── ls, cat...  # 常用命令符号链接
├── app/          # GUI 应用
│   ├── desktop   # 桌面环境
│   ├── calculator  # 计算器
│   ├── clock     # 时钟
│   └── vshell    # 可视化 Shell
├── test/         # 测试程序
│   ├── fork_test
│   └── mini-ltp/ # 内核兼容性测试
├── dev/          # 设备文件
├── proc/         # procfs 挂载点
├── tmp/          # 临时文件
└── etc/          # 配置文件
```

## 下一步

- 📖 阅读 [设计原则](../architecture/design.md)
- 🏗️ 了解 [代码结构](../architecture/structure.md)
- 🔧 查看 [开发流程](development.md)
- 📊 查看 [开发路线图](../progress/roadmap.md)
- 📝 查看 [用户程序指南](../development/user-programs.md)

## 获取帮助

- **文档中心**：返回 [文档首页](../../README.md)
- **问题反馈**：[GitHub Issues](https://github.com/topkernel/rux/issues)

---

最后更新：2026-03-04
