# Rux 内核项目结构

本文档描述 Rux 内核项目的目录结构和文件组织。

---

## 📊 代码统计

**最后更新**: 2026-02-27

### 总体统计

| 指标 | 数值 |
|------|------|
| **Rust 源文件总数** | 140+ 个 |
| **总代码行数** | **49,490 行** |
| **内核大小 (debug)** | ~3 MB |

### 模块代码行数分布

| 模块 | 代码行数 | 占比 | 说明 |
|------|----------|------|------|
| **fs/** | 10,974 | 22.2% | 文件系统 |
| **arch/** | 9,413 | 19.0% | 架构相关 (RISC-V) |
| **tests/** | 7,039 | 14.2% | 单元测试 |
| **drivers/** | 5,736 | 11.6% | 设备驱动 |
| **mm/** | 4,295 | 8.7% | 内存管理 |
| **net/** | 3,608 | 7.3% | 网络协议栈 |
| **sched/** | 2,248 | 4.5% | 进程调度 |
| **process/** | 1,333 | 2.7% | 进程管理 |
| **sync/** | 684 | 1.4% | 同步原语 |
| **其他** | ~4,160 | 8.4% | 主入口、配置等 |

### 核心文件 Top 20

| 文件 | 行数 | 模块 | 说明 |
|------|------|------|------|
| [arch/riscv64/syscall.rs](../../kernel/src/arch/riscv64/syscall.rs) | 4,426 | arch | RISC-V 系统调用处理 |
| [fs/vfs.rs](../../kernel/src/fs/vfs.rs) | 1,493 | fs | 虚拟文件系统 |
| [fs/ext4/mod.rs](../../kernel/src/fs/ext4/mod.rs) | 1,651 | fs | ext4 文件系统 |
| [drivers/intc/gicv3.rs](../../kernel/src/drivers/intc/gicv3.rs) | 1,465 | drivers | GICv3 中断控制器 |
| [arch/riscv64/mm.rs](../../kernel/src/arch/riscv64/mm.rs) | 1,420 | arch | RISC-V 内存管理 |
| [net/tcp.rs](../../kernel/src/net/tcp.rs) | 1,067 | net | TCP 协议 |
| [fs/dentry.rs](../../kernel/src/fs/dentry.rs) | 1,012 | fs | 目录项缓存 |
| [fs/ext4/file.rs](../../kernel/src/fs/ext4/file.rs) | 930 | fs | ext4 文件操作 |
| [net/buffer.rs](../../kernel/src/net/buffer.rs) | 887 | net | 网络缓冲区 (SkBuff) |
| [fs/path.rs](../../kernel/src/fs/path.rs) | 874 | fs | 路径解析 |
| [fs/inode.rs](../../kernel/src/fs/inode.rs) | 826 | fs | Inode 缓存 |
| [net/ipv4/mod.rs](../../kernel/src/net/ipv4/mod.rs) | 802 | net | IPv4 协议 |
| [process/task.rs](../../kernel/src/process/task.rs) | 798 | process | 任务控制块 |
| [fs/procfs.rs](../../kernel/src/fs/procfs.rs) | 606 | fs | procfs 文件系统 |
| [drivers/net/virtio_net.rs](../../kernel/src/drivers/net/virtio_net.rs) | 514 | drivers | VirtIO 网卡驱动 |
| [fs/ext4/allocator.rs](../../kernel/src/fs/ext4/allocator.rs) | 507 | fs | ext4 分配器 |
| [sched/sched.rs](../../kernel/src/sched/sched.rs) | 506 | sched | 调度器 + fork |
| [arch/riscv64/trap.rs](../../kernel/src/arch/riscv64/trap.rs) | 446 | arch | 异常处理 |
| [fs/vfs_ops.rs](../../kernel/src/fs/vfs_ops.rs) | 438 | fs | VFS 操作 |
| [fs/rootfs.rs](../../kernel/src/fs/rootfs.rs) | 409 | fs | Root 文件系统 |

### 测试文件统计

| 测试模块 | 文件数 | 代码行数 | 状态 |
|----------|--------|----------|------|
| **基础测试** | 5 | ~1,200 | ✅ |
| **内存测试** | 4 | ~800 | ✅ |
| **进程测试** | 8 | ~1,400 | ✅ |
| **文件系统测试** | 9 | ~1,200 | ✅ |
| **IPC 测试** | 4 | ~500 | ✅ |
| **网络测试** | 4 | ~600 | ✅ |
| **驱动测试** | 3 | ~500 | ✅ |
| **其他测试** | 6 | ~1,439 | ✅ |
| **总计** | **43** | **7,039** | **99% 通过** |

---

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
│   ├── run_unit_tests.sh  # 单元测试脚本
│   └── debug.sh           # GDB 调试脚本
│
├── userspace/              # 用户态程序
│   ├── shell/              # 默认 Shell (no_std Rust)
│   │   ├── src/main.rs     # Shell 主程序
│   │   ├── user.ld         # 链接脚本
│   │   └── Cargo.toml      # Cargo 配置
│   ├── cshell/             # C Shell (musl libc)
│   │   ├── src/shell.c     # C Shell 源码
│   │   └── Makefile        # 构建脚本
│   ├── rust-shell/         # Rust std Shell
│   │   ├── src/main.rs     # Rust Shell 源码
│   │   └── Cargo.toml      # Cargo 配置
│   ├── musl.ld             # musl 程序链接脚本
│   ├── user.ld             # no_std 程序链接脚本
│   └── build.sh            # 构建脚本
│
├── toolchain/              # 工具链
│   ├── build-musl.sh       # musl libc 构建脚本
│   └── riscv64-rux-linux-musl/ # musl 工具链安装目录
│
├── docs/                   # 项目文档
│   ├── CONFIG.md          # 配置系统文档
│   ├── TODO.md            # 开发路线图
│   ├── QUICKREF.md        # 快速参考
│   ├── architecture/      # 架构文档
│   │   ├── riscv64.md     # RISC-V 架构说明
│   │   └── structure.md   # 本文件 - 目录结构说明
│   ├── development/       # 开发文档
│   │   ├── changelog.md   # 变更日志
│   │   └── user-programs.md # 用户程序指南
│   ├── progress/          # 进度文档
│   │   └── roadmap.md     # 开发路线图
│   ├── tests/             # 测试文档
│   │   └── unit-test-report.md # 单元测试报告
│   └── guides/            # 指南文档
│
├── kernel/                 # 内核源代码
│   ├── src/               # Rust 源代码
│   │   ├── arch/         # 架构相关代码
│   │   │   └── riscv64/  # RISC-V 架构实现
│   │   │       ├── mod.rs       # 模块导出
│   │   │       ├── boot.S       # 启动代码 (汇编)
│   │   │       ├── trap.S       # 异常向量表 (汇编)
│   │   │       ├── boot.rs      # 初始化
│   │   │       ├── trap.rs      # 异常处理
│   │   │       ├── syscall.rs   # 系统调用处理 (3,400行)
│   │   │       ├── mm.rs        # 内存管理 (1,420行)
│   │   │       ├── smp.rs       # 多核支持
│   │   │       ├── ipi.rs       # 处理器间中断
│   │   │       ├── context.rs   # 上下文切换
│   │   │       └── cpu.rs       # CPU 操作
│   │   │
│   │   ├── drivers/      # 设备驱动
│   │   │   ├── mod.rs       # 驱动模块导出
│   │   │   ├── intc/        # 中断控制器
│   │   │   │   ├── mod.rs
│   │   │   │   ├── plic.rs     # RISC-V PLIC 驱动
│   │   │   │   └── clint.rs    # RISC-V CLINT 驱动
│   │   │   ├── timer/       # 定时器驱动
│   │   │   │   ├── mod.rs
│   │   │   │   └── riscv64.rs  # RISC-V 定时器
│   │   │   ├── virtio/      # VirtIO 框架
│   │   │   │   ├── mod.rs      # VirtIO 模块
│   │   │   │   ├── queue.rs    # VirtQueue 实现
│   │   │   │   └── probe.rs    # 设备探测
│   │   │   ├── blkdev/      # 块设备
│   │   │   │   └── mod.rs      # VirtIO-blk 驱动
│   │   │   └── net/         # 网络设备
│   │   │       ├── mod.rs
│   │   │       ├── space.rs    # 网络设备基类
│   │   │       ├── loopback.rs # 回环设备
│   │   │       └── virtio_net.rs # VirtIO-net 驱动
│   │   │
│   │   ├── mm/           # 内存管理
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── page.rs      # 页管理 (PhysFrame/VirtPage)
│   │   │   ├── allocator.rs # 堆分配器 (BuddyAllocator)
│   │   │   ├── pagemap.rs   # 页表管理 (平台无关接口)
│   │   │   └── vma.rs       # 虚拟内存区域
│   │   │
│   │   ├── fs/           # 文件系统
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── vfs.rs       # 虚拟文件系统 (1,823行)
│   │   │   ├── vfs_ops.rs   # VFS 操作
│   │   │   ├── file.rs      # 文件描述符
│   │   │   ├── inode.rs     # Inode 缓存
│   │   │   ├── dentry.rs    # 目录项缓存
│   │   │   ├── buffer.rs    # 块缓存
│   │   │   ├── mount.rs     # 挂载管理
│   │   │   ├── superblock.rs # 超级块
│   │   │   ├── path.rs      # 路径解析
│   │   │   ├── rootfs.rs    # Root 文件系统
│   │   │   ├── pipe.rs      # 管道实现
│   │   │   ├── char_dev.rs  # 字符设备
│   │   │   ├── elf.rs       # ELF 加载器
│   │   │   └── ext4/        # ext4 文件系统
│   │   │       ├── mod.rs      # ext4 主模块
│   │   │       ├── superblock.rs # 超级块解析
│   │   │       ├── inode.rs    # Inode 结构
│   │   │       ├── allocator.rs # 块/Inode 分配器
│   │   │       └── file.rs     # 文件操作
│   │   │
│   │   ├── net/          # 网络协议栈
│   │   │   ├── mod.rs       # 网络模块
│   │   │   ├── buffer.rs    # SkBuff (网络缓冲区)
│   │   │   ├── socket.rs    # Socket 层
│   │   │   ├── ethernet.rs  # 以太网层
│   │   │   ├── arp.rs       # ARP 协议
│   │   │   ├── ipv4/        # IPv4 协议
│   │   │   │   ├── mod.rs
│   │   │   │   └── route.rs   # 路由表
│   │   │   ├── tcp.rs       # TCP 协议
│   │   │   └── udp.rs       # UDP 协议
│   │   │
│   │   ├── process/      # 进程管理
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── task.rs      # 任务控制块
│   │   │   ├── pid.rs       # PID 分配器
│   │   │   ├── usermod.rs   # 用户模式管理
│   │   │   ├── wait.rs      # wait4 系统调用
│   │   │   └── signal.rs    # 信号处理
│   │   │
│   │   ├── sched/        # 进程调度
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── sched.rs     # 调度器 + fork (506行)
│   │   │   └── pid.rs       # PID 管理
│   │   │
│   │   ├── sync/         # 同步原语
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── mutex.rs     # Mutex 锁
│   │   │   ├── semaphore.rs # 信号量 (411行)
│   │   │   └── condvar.rs   # 条件变量 (260行)
│   │   │
│   │   ├── tests/        # 单元测试
│   │   │   ├── mod.rs       # 测试框架入口
│   │   │   ├── boundary.rs  # 边界条件测试
│   │   │   ├── listhead.rs  # 双向链表测试
│   │   │   ├── path.rs      # 路径解析测试
│   │   │   ├── file_flags.rs # 文件标志测试
│   │   │   ├── fdtable.rs   # 文件描述符表测试
│   │   │   ├── heap_allocator.rs # 堆分配器测试
│   │   │   ├── page_allocator.rs # 页分配器测试
│   │   │   ├── scheduler.rs # 调度器测试
│   │   │   ├── signal.rs    # 信号处理测试
│   │   │   ├── smp.rs       # 多核测试
│   │   │   ├── process_tree.rs # 进程树测试
│   │   │   ├── fork.rs      # fork 测试
│   │   │   ├── execve.rs    # execve 测试
│   │   │   ├── wait4.rs     # wait4 测试
│   │   │   ├── getpid.rs    # getpid 测试
│   │   │   ├── user_syscall.rs # 用户系统调用测试
│   │   │   ├── preemptive_scheduler.rs # 抢占式调度测试
│   │   │   ├── sleep_wakeup.rs # 睡眠唤醒测试
│   │   │   ├── virtio_queue.rs # VirtIO 队列测试
│   │   │   ├── dcache.rs    # 目录项缓存测试
│   │   │   ├── fstat.rs     # fstat 测试
│   │   │   ├── fcntl.rs     # fcntl 测试
│   │   │   ├── link.rs      # link 测试
│   │   │   ├── mkdir_unlink.rs # mkdir/unlink 测试
│   │   │   ├── ext4_allocator.rs # ext4 分配器测试
│   │   │   ├── ext4_file_write.rs # ext4 写入测试
│   │   │   ├── ext4_indirect_blocks.rs # ext4 间接块测试
│   │   │   ├── standard_alloc.rs # 标准分配器测试
│   │   │   ├── network.rs   # 网络测试
│   │   │   ├── tcp_handshake.rs # TCP 握手测试
│   │   │   ├── virtio_net.rs # VirtIO-net 测试
│   │   │   ├── smp_schedule.rs # SMP 调度测试
│   │   │   ├── pipe2.rs     # pipe2 测试
│   │   │   ├── signal_procmask.rs # 信号掩码测试
│   │   │   ├── ipc_poll.rs  # poll 测试
│   │   │   ├── ipc_epoll.rs # epoll 测试
│   │   │   ├── ipc_eventfd.rs # eventfd 测试
│   │   │   ├── mem_mmap.rs  # mmap 测试
│   │   │   └── mem_cow.rs   # COW 测试
│   │   │
│   │   ├── console.rs    # 控制台 (UART)
│   │   ├── config.rs     # 自动生成的配置 (不要手动编辑)
│   │   ├── main.rs       # 内核入口
│   │   └── print.rs      # 打印宏
│   │
│   ├── build.rs          # 构建脚本 (生成 config.rs)
│   └── Cargo.toml        # 内核 crate 配置
│
├── .cargo/                 # Cargo 配置
│   └── config.toml       # Cargo 工具配置
│
├── target/                 # 编译输出 (git忽略)
│   └── riscv64gc-unknown-none-elf/
│       ├── debug/        # Debug 构建
│       └── release/      # Release 构建
│
├── Kernel.toml            # 内核配置文件
├── Cargo.toml             # 工作空间配置
├── Cargo.lock             # 依赖锁定
├── Makefile               # 项目根 Makefile
├── README.md              # 项目说明
├── CLAUDE.md              # AI 助手开发指南
├── LICENSE                # 许可证 (MIT)
└── .gitignore             # Git 忽略规则
```

---

## 目录说明

### build/ - 构建工具目录

包含所有与构建、配置相关的脚本和工具：

- **Makefile** - 主构建脚本，提供编译、运行、测试等命令
- **menuconfig.sh** - 交互式配置菜单（类似 Linux kernel menuconfig）
- **config-demo.sh** - 配置系统演示脚本

### test/ - 测试目录

包含所有测试和调试脚本：

- **run.sh** - 快速运行内核
- **run_unit_tests.sh** - 运行全量单元测试
- **test_qemu.sh** - QEMU 基本功能测试
- **test_suite.sh** - 完整的测试套件
- **debug.sh** - GDB 调试脚本

### docs/ - 文档目录

项目文档，按类型组织：

#### architecture/ - 架构文档
- **riscv64.md** - RISC-V 架构详细说明
- **structure.md** - 本文件，目录结构说明

#### development/ - 开发文档
- **changelog.md** - 变更日志和版本历史
- **user-programs.md** - 用户程序开发指南

#### progress/ - 进度文档
- **roadmap.md** - 开发路线图和功能清单

#### tests/ - 测试文档
- **unit-test-report.md** - 全量单元测试详细报告

### kernel/ - 内核源码

内核的核心源代码，按功能模块组织。

#### kernel/src/arch/ - 架构相关代码

**重要**: 当前**仅支持 RISC-V 64位架构**。

```
kernel/src/arch/
└── riscv64/         # RISC-V 架构实现（默认且唯一支持的平台）
    ├── boot.rs      # 初始化 (17行)
    ├── trap.rs      # 异常处理 (446行)
    ├── syscall.rs   # 系统调用 (3,400行 - 最大的单文件)
    ├── mm.rs        # 架构相关内存管理 (1,420行)
    ├── smp.rs       # 多核支持 (178行)
    ├── ipi.rs       # 处理器间中断 (127行)
    ├── context.rs   # 上下文切换 (269行)
    ├── cpu.rs       # CPU 操作 (136行)
    └── mod.rs       # 模块导出 (92行)
```

**架构支持状态**:
- ✅ **RISC-V 64位 (RV64GC)** - 完全支持，当前默认平台
- ❌ **ARM64 (aarch64)** - 未实现
- ❌ **x86_64** - 未实现

#### kernel/src/drivers/ - 设备驱动程序

设备驱动按类型组织：

| 子目录 | 功能 | 主要文件 | 代码量 |
|--------|------|----------|--------|
| **intc/** | 中断控制器 | plic.rs, clint.rs | ~500 行 |
| **timer/** | 定时器 | riscv64.rs | ~150 行 |
| **virtio/** | VirtIO 框架 | mod.rs, queue.rs, probe.rs | ~900 行 |
| **blkdev/** | 块设备 | mod.rs (VirtIO-blk) | ~250 行 |
| **net/** | 网络设备 | space.rs, loopback.rs, virtio_net.rs | ~1,000 行 |

#### kernel/src/mm/ - 内存管理代码

| 文件 | 功能 | 代码量 |
|------|------|--------|
| **page.rs** | 物理页/虚拟页管理 | ~400 行 |
| **allocator.rs** | Buddy 堆分配器 | ~300 行 |
| **pagemap.rs** | 页表管理（平台无关接口） | ~80 行 |
| **vma.rs** | 虚拟内存区域 | ~300 行 |

#### kernel/src/fs/ - 文件系统

**最大的模块**（9,020 行，占 24.1%）：

| 文件 | 功能 | 代码量 |
|------|------|--------|
| **vfs.rs** | 虚拟文件系统 | 1,823 行 |
| **ext4/mod.rs** | ext4 文件系统 | 1,651 行 |
| **dentry.rs** | 目录项缓存 | 1,012 行 |
| **ext4/file.rs** | ext4 文件操作 | 930 行 |
| **path.rs** | 路径解析 | 874 行 |
| **inode.rs** | Inode 缓存 | 826 行 |
| **vfs_ops.rs** | VFS 操作 | 438 行 |
| **rootfs.rs** | Root 文件系统 | 409 行 |
| 其他 (buffer, mount, pipe, elf 等) | - | ~2,000 行 |

#### kernel/src/net/ - 网络协议栈

| 文件 | 功能 | 代码量 |
|------|------|--------|
| **tcp.rs** | TCP 协议 | 1,067 行 |
| **buffer.rs** | SkBuff | 887 行 |
| **ipv4/mod.rs** | IPv4 协议 | 802 行 |
| **ethernet.rs** | 以太网层 | 396 行 |
| 其他 (socket, arp, udp 等) | - | ~500 行 |

#### kernel/src/process/ - 进程管理

| 文件 | 功能 | 代码量 |
|------|------|--------|
| **task.rs** | 任务控制块 | 798 行 |
| **signal.rs** | 信号处理 | ~400 行 |
| **usermod.rs** | 用户模式管理 | ~300 行 |
| **wait.rs** | wait4 系统调用 | ~200 行 |
| **pid.rs** | PID 分配器 | ~100 行 |

#### kernel/src/sched/ - 进程调度

| 文件 | 功能 | 代码量 |
|------|------|--------|
| **sched.rs** | 调度器 + fork + COW | 506 行 |
| **pid.rs** | PID 管理 | ~100 行 |

#### kernel/src/sync/ - 同步原语

| 文件 | 功能 | 代码量 |
|------|------|--------|
| **semaphore.rs** | 信号量 | 411 行 |
| **condvar.rs** | 条件变量 | 260 行 |
| **mutex.rs** | Mutex 锁 | ~100 行 |

#### kernel/src/tests/ - 单元测试

**43 个测试文件，5,885 行代码**，详见 [单元测试报告](../tests/unit-test-report.md)。

---

## 使用指南

### 编译内核

从项目根目录：

```bash
make build
# 或直接使用 Cargo（默认 RISC-V）
cargo build --package rux --features riscv64
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
# 运行全量单元测试
./test/run_unit_tests.sh

# 或运行完整测试套件
./test/test_suite.sh
```

### 调试内核

```bash
make debug
# 或
./test/debug.sh
```

---

## 添加新文件

### 新驱动

在 `kernel/src/drivers/` 下创建新模块：

1. 创建目录，如 `drivers/block/`
2. 创建 `kernel/src/drivers/block/mod.rs`
3. 在 `kernel/src/drivers/mod.rs` 中添加 `pub mod block;`
4. 导出需要的接口：`pub use block::*;`

### 新测试

在 `kernel/src/tests/` 下添加新文件：

1. 创建测试文件，如 `tests/new_feature.rs`
2. 在 `kernel/src/tests/mod.rs` 中添加 `pub mod new_feature;`
3. 运行测试验证功能

### 新架构支持

**注意**: 当前仅支持 RISC-V 64位。添加新架构（如 x86_64）需要：

1. 在 `kernel/src/arch/` 下创建新目录，如 `arch/x86_64/`
2. 实现必要的接口（boot, trap, syscall, mm 等）
3. 在 `kernel/Cargo.toml` 中添加对应的 feature
4. 添加对应的链接脚本

---

## 注意事项

1. **config.rs 是自动生成的** - 不要手动编辑 `kernel/src/config.rs`，它由 `kernel/build.rs` 根据 `Kernel.toml` 自动生成。

2. **平台限制** - 当前仅支持 RISC-V 64位架构。ARM64 支持已移除，x86_64 未实现。

3. **测试覆盖** - 43 个测试模块，203 个测试项，99% 通过率。

4. **代码规范** - 遵循 Linux 内核设计原则，POSIX/ABI 完全兼容。

5. **模块导出** - 添加新模块时，确保在父模块的 `mod.rs` 中正确导出需要的接口。

---

**文档版本**: v4.1
**最后更新**: 2026-02-15
**维护者**: Rux 开发团队
