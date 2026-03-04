# Rux 内核项目结构

本文档描述 Rux 内核项目的目录结构和文件组织。

---

## 📊 代码统计

**最后更新**: 2026-03-04

### 总体统计

| 指标 | 数值 |
|------|------|
| **Rust 源文件总数** | 178 个 |
| **总代码行数** | **~56,600 行** |
| **内核大小 (debug)** | ~3 MB |

### 模块代码行数分布

| 模块 | 代码行数 | 占比 | 说明 |
|------|----------|------|------|
| **fs/** | 11,200+ | 21.5% | 文件系统 |
| **arch/** | 8,500+ | 16.3% | 架构相关 (RISC-V) |
| **syscall/** | 2,800+ | 5.4% | 系统调用分发 |
| **tests/** | 7,000+ | 13.5% | 单元测试 |
| **drivers/** | 5,700+ | 11.0% | 设备驱动 |
| **mm/** | 4,300+ | 8.3% | 内存管理 |
| **net/** | 3,600+ | 6.9% | 网络协议栈 |
| **sched/** | 2,500+ | 4.8% | 进程调度 |
| **process/** | 1,800+ | 3.5% | 进程管理 |
| **sync/** | 700+ | 1.3% | 同步原语 |
| **其他** | ~4,000 | 7.7% | 主入口、配置等 |


### 测试统计

| 测试类型 | 数量 | 说明 |
|----------|------|------|
| **内核单元测试** | 51 个文件 | 内存、进程、文件系统、网络等 |
| **mini-ltp 测试** | 24 个测试 | 内核兼容性测试 |

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
│   ├── run.sh             # 快速运行脚本
│   ├── mkrootfs.sh        # 创建 rootfs 镜像
│   └── rootfs.img         # ext4 rootfs 镜像 (128MB)
│
├── userspace/              # 用户态程序
│   ├── shell/              # Shell (musl libc)
│   │   ├── src/main.rs     # Shell 主程序
│   │   ├── Makefile        # 构建脚本
│   │   └── user.ld         # 链接脚本
│   │
│   ├── apps/               # GUI 应用程序 (musl libc)
│   │   ├── desktop/        # 桌面环境
│   │   ├── calculator/     # 计算器
│   │   ├── clock/          # 时钟
│   │   └── vshell/         # 可视化 Shell
│   │
│   ├── libs/               # 共享库
│   │   └── gui/            # GUI 库 (rux_gui)
│   │
│   ├── tests/              # 用户态测试程序
│   │   ├── fork_test/      # fork 测试
│   │   └── mini-ltp/       # 内核兼容性测试套件
│   │       ├── src/        # 测试源码 (24 个 C 文件)
│   │       ├── output/     # 编译输出
│   │       │   ├── bin/    # 测试二进制文件
│   │       │   └── run_tests.sh  # 测试运行脚本
│   │       └── build.sh    # 构建脚本
│   │
│   ├── toybox/             # Toybox (BusyBox 替代品)
│   │   ├── toybox/         # Toybox 源码
│   │   └── build-toybox.sh # 构建脚本
│   │
│   ├── build               # 用户程序构建脚本
│   ├── Cargo.toml          # Cargo 配置
│   └── README.md           # 用户程序说明
│
├── toolchain/              # 工具链
│   ├── build-musl.sh       # musl libc 构建脚本
│   └── riscv64-rux-linux-musl/ # musl 工具链安装目录
│       ├── include/        # musl 头文件
│       └── lib/            # musl 静态库
│
├── docs/                   # 项目文档
│   ├── CLAUDE.md          # AI 助手开发指南
│   ├── architecture/      # 架构文档
│   │   ├── riscv64.md     # RISC-V 架构说明
│   │   └── structure.md   # 本文件 - 目录结构说明
│   ├── development/       # 开发文档
│   │   ├── changelog.md   # 变更日志
│   │   └── user-programs.md # 用户程序指南
│   ├── progress/          # 进度文档
│   │   └── roadmap.md     # 开发路线图
│   └── guides/            # 指南文档
│       └── getting-started.md # 快速开始
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
│   │   │       ├── mm.rs        # 内存管理
│   │   │       ├── smp.rs       # 多核支持
│   │   │       ├── ipi.rs       # 处理器间中断
│   │   │       ├── context.rs   # 上下文切换
│   │   │       └── cpu.rs       # CPU 操作
│   │   │
│   │   ├── syscall/      # 系统调用分发
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── dispatch.rs  # 系统调用分发器
│   │   │   ├── file.rs      # 文件系统调用
│   │   │   ├── process.rs   # 进程系统调用
│   │   │   ├── memory.rs    # 内存系统调用
│   │   │   ├── sched.rs     # 调度系统调用
│   │   │   ├── signal.rs    # 信号系统调用
│   │   │   ├── network.rs   # 网络系统调用
│   │   │   ├── io.rs        # I/O 系统调用
│   │   │   ├── time.rs      # 时间系统调用
│   │   │   └── misc.rs      # 其他系统调用
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
│   │   │   │   ├── probe.rs    # 设备探测
│   │   │   │   ├── offset.rs   # 寄存器偏移定义
│   │   │   │   └── virtio_pci.rs # PCI 传输层
│   │   │   ├── blkdev/      # 块设备
│   │   │   │   └── mod.rs      # VirtIO-blk 驱动
│   │   │   ├── input/       # 输入设备
│   │   │   │   ├── mod.rs
│   │   │   │   ├── evdev.rs    # evdev 驱动
│   │   │   │   ├── event.rs    # 输入事件定义
│   │   │   │   ├── ps2.rs      # PS/2 键盘/鼠标
│   │   │   │   └── virtio_input.rs # VirtIO 输入设备
│   │   │   ├── net/         # 网络设备
│   │   │   │   ├── mod.rs
│   │   │   │   ├── space.rs    # 网络设备基类
│   │   │   │   ├── loopback.rs # 回环设备
│   │   │   │   └── virtio_net.rs # VirtIO-net 驱动
│   │   │   ├── gpu/         # GPU/显示设备
│   │   │   │   ├── mod.rs
│   │   │   │   ├── framebuffer.rs # 帧缓冲核心
│   │   │   │   ├── fb_simple.rs   # 简单帧缓冲驱动
│   │   │   │   ├── fbdev.rs       # fbdev 设备接口
│   │   │   │   ├── virtio_gpu.rs  # VirtIO-GPU 驱动
│   │   │   │   └── virtio_cmd.rs  # GPU 命令处理
│   │   │   └── pci/         # PCI 总线
│   │   │       └── mod.rs      # PCI 枚举和驱动
│   │   │
│   │   ├── mm/           # 内存管理
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── page.rs      # 页管理 (PhysFrame/VirtPage)
│   │   │   ├── page_desc.rs # 页描述符
│   │   │   ├── allocator.rs # 堆分配器接口
│   │   │   ├── buddy_allocator.rs # Buddy 分配器
│   │   │   ├── slab.rs      # Slab 分配器
│   │   │   ├── pcp.rs       # Per-CPU 页缓存
│   │   │   ├── pagemap.rs   # 页表管理 (平台无关接口)
│   │   │   ├── mm_struct.rs # 进程内存描述符
│   │   │   ├── vma.rs       # 虚拟内存区域
│   │   │   └── meminfo.rs   # 内存信息接口
│   │   │
│   │   ├── fs/           # 文件系统
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── vfs.rs       # 虚拟文件系统
│   │   │   ├── file.rs      # 文件描述符
│   │   │   ├── inode.rs     # Inode 缓存
│   │   │   ├── dentry.rs    # 目录项缓存
│   │   │   ├── buffer.rs    # 块缓存
│   │   │   ├── bio.rs       # 块 I/O 层
│   │   │   ├── mount.rs     # 挂载管理
│   │   │   ├── superblock.rs # 超级块
│   │   │   ├── path.rs      # 路径解析
│   │   │   ├── stat.rs      # 文件状态结构
│   │   │   ├── rootfs.rs    # Root 文件系统
│   │   │   ├── pipe.rs      # 管道实现
│   │   │   ├── char_dev.rs  # 字符设备
│   │   │   ├── elf.rs       # ELF 加载器
│   │   │   ├── dev_t.rs     # 设备号定义
│   │   │   ├── devfs/       # devfs 文件系统
│   │   │   │   ├── mod.rs
│   │   │   │   └── registry.rs # 设备注册表
│   │   │   ├── procfs.rs    # procfs 文件系统
│   │   │   └── ext4/        # ext4 文件系统
│   │   │       ├── mod.rs      # ext4 主模块
│   │   │       ├── superblock.rs # 超级块解析
│   │   │       ├── inode.rs    # Inode 结构
│   │   │       ├── file.rs     # 文件操作
│   │   │       ├── dir.rs      # 目录操作
│   │   │       ├── allocator.rs # 块/Inode 分配器
│   │   │       ├── extent.rs   # Extent 树
│   │   │       └── indirect.rs # 间接块
│   │   │
│   │   ├── net/          # 网络协议栈
│   │   │   ├── mod.rs       # 网络模块
│   │   │   ├── buffer.rs    # SkBuff (网络缓冲区)
│   │   │   ├── socket.rs    # Socket 层
│   │   │   ├── ethernet.rs  # 以太网层
│   │   │   ├── arp.rs       # ARP 协议
│   │   │   ├── ipv4/        # IPv4 协议
│   │   │   │   ├── mod.rs
│   │   │   │   ├── route.rs   # 路由表
│   │   │   │   └── checksum.rs # IP 校验和
│   │   │   ├── tcp.rs       # TCP 协议
│   │   │   └── udp.rs       # UDP 协议
│   │   │
│   │   ├── process/      # 进程管理
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── task.rs      # 任务控制块
│   │   │   ├── fork.rs      # fork 实现
│   │   │   ├── pid.rs       # PID 管理
│   │   │   ├── usermod.rs   # 用户模式管理
│   │   │   └── wait.rs      # wait4 系统调用
│   │   │
│   │   ├── sched/        # 进程调度
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── sched.rs     # 调度器
│   │   │   └── cfs.rs       # CFS 调度器
│   │   │
│   │   ├── sync/         # 同步原语
│   │   │   ├── mod.rs       # 模块导出
│   │   │   ├── mutex.rs     # Mutex 锁
│   │   │   ├── semaphore.rs # 信号量
│   │   │   ├── condvar.rs   # 条件变量
│   │   │   └── futex.rs     # Fast Userspace Mutex
│   │   │
│   │   ├── tests/        # 单元测试 (51 个测试文件)
│   │   │   ├── mod.rs       # 测试框架入口
│   │   │   │
│   │   │   │  # 内存测试
│   │   │   ├── heap_allocator.rs    # 堆分配器测试
│   │   │   ├── page_allocator.rs    # 页分配器测试
│   │   │   ├── standard_alloc.rs    # 标准分配器测试
│   │   │   ├── mem_mmap.rs          # mmap 测试
│   │   │   ├── mem_cow.rs           # COW 测试
│   │   │   │
│   │   │   │  # 进程/调度测试
│   │   │   ├── fork.rs              # fork 测试
│   │   │   ├── getpid.rs            # getpid 测试
│   │   │   ├── wait4.rs             # wait4 测试
│   │   │   ├── process_tree.rs      # 进程树测试
│   │   │   ├── scheduler.rs         # 调度器测试
│   │   │   ├── preemptive_scheduler.rs # 抢占式调度测试
│   │   │   ├── sleep_wakeup.rs      # 睡眠/唤醒测试
│   │   │   ├── smp.rs               # SMP 测试
│   │   │   ├── smp_schedule.rs      # SMP 调度测试
│   │   │   │
│   │   │   │  # 文件系统测试
│   │   │   ├── file_open.rs         # 文件打开测试
│   │   │   ├── file_flags.rs        # 文件标志测试
│   │   │   ├── fdtable.rs           # fd 表测试
│   │   │   ├── path.rs              # 路径解析测试
│   │   │   ├── dcache.rs            # 目录缓存测试
│   │   │   ├── icache.rs            # Inode 缓存测试
│   │   │   ├── link.rs              # 链接测试
│   │   │   ├── fcntl.rs             # fcntl 测试
│   │   │   ├── fstat.rs             # fstat 测试
│   │   │   ├── mkdir_unlink.rs      # mkdir/unlink 测试
│   │   │   ├── ext4_allocator.rs    # ext4 分配器测试
│   │   │   ├── ext4_file_write.rs   # ext4 文件写入测试
│   │   │   ├── ext4_indirect_blocks.rs # ext4 间接块测试
│   │   │   │
│   │   │   │  # IPC 测试
│   │   │   ├── pipe2.rs             # 管道测试
│   │   │   ├── ipc_poll.rs          # poll 测试
│   │   │   ├── ipc_epoll.rs         # epoll 测试
│   │   │   ├── ipc_eventfd.rs       # eventfd 测试
│   │   │   │
│   │   │   │  # 信号测试
│   │   │   ├── signal.rs            # 信号测试
│   │   │   ├── signal_procmask.rs   # 信号掩码测试
│   │   │   │
│   │   │   │  # 网络测试
│   │   │   ├── network.rs           # 网络基础测试
│   │   │   ├── tcp_handshake.rs     # TCP 握手测试
│   │   │   │
│   │   │   │  # 驱动测试
│   │   │   ├── virtio_queue.rs      # VirtIO 队列测试
│   │   │   ├── virtio_net.rs        # VirtIO 网络测试
│   │   │   ├── framebuffer.rs       # 帧缓冲测试
│   │   │   │
│   │   │   │  # 系统调用测试
│   │   │   ├── syscall_file.rs      # 文件系统调用测试
│   │   │   ├── syscall_memory.rs    # 内存系统调用测试
│   │   │   ├── syscall_process.rs   # 进程系统调用测试
│   │   │   ├── syscall_sched.rs     # 调度系统调用测试
│   │   │   ├── syscall_signal.rs    # 信号系统调用测试
│   │   │   ├── syscall_network.rs   # 网络系统调用测试
│   │   │   ├── syscall_io.rs        # I/O 系统调用测试
│   │   │   ├── syscall_time.rs      # 时间系统调用测试
│   │   │   ├── syscall_misc.rs      # 杂项系统调用测试
│   │   │   ├── user_syscall.rs      # 用户态系统调用测试
│   │   │   ├── execve.rs            # execve 测试
│   │   │   │
│   │   │   │  # 其他测试
│   │   │   ├── listhead.rs          # 链表测试
│   │   │   ├── boundary.rs          # 边界测试
│   │   │   └── quick.rs             # 快速测试入口
│   │   │
│   │   ├── console.rs    # 控制台 (UART)
│   │   ├── config.rs     # 自动生成的配置 (不要手动编辑)
│   │   ├── main.rs       # 内核入口
│   │   ├── init.rs       # 内核初始化
│   │   ├── print.rs      # 打印宏
│   │   └── errno.rs      # 错误码定义
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

### userspace/ - 用户态程序

用户态程序目录，包含 Shell、GUI 应用、测试程序和工具：

```
userspace/
├── shell/              # Shell (no_std Rust + musl libc)
│   └── shell           # 编译后的二进制文件
│
├── apps/               # GUI 应用程序 (musl libc)
│   ├── desktop/        # 桌面环境
│   ├── calculator/     # 计算器
│   ├── clock/          # 时钟
│   └── vshell/         # 可视化 Shell
│
├── libs/               # 共享库
│   └── gui/            # GUI 库 (rux_gui)
│       ├── widget.rs   # 控件
│       ├── window.rs   # 窗口管理
│       └── input.rs    # 输入处理
│
├── tests/              # 用户态测试程序
│   ├── fork_test/      # fork 测试
│   └── mini-ltp/       # 内核兼容性测试套件
│       ├── src/        # 测试源码 (24 个测试)
│       │   ├── test_fork.c
│       │   ├── test_fileio.c
│       │   ├── test_pipe.c
│       │   └── ...
│       ├── output/
│       │   ├── bin/    # 测试二进制文件
│       │   └── run_tests.sh
│       └── build.sh
│
├── toybox/             # Toybox (BusyBox 替代品)
│   └── toybox/toybox   # 编译后的二进制文件
│
└── build               # 统一构建脚本
```

### rootfs 目录结构

rootfs 镜像 (`test/rootfs.img`) 内部结构：

```
/
├── bin/                # 基本命令
│   ├── shell           # Shell
│   ├── sh -> shell     # Shell 符号链接
│   ├── toybox          # Toybox
│   ├── ls -> toybox    # 常用命令符号链接
│   ├── cat -> toybox
│   ├── echo -> toybox
│   └── ...
│
├── app/                # GUI 应用
│   ├── desktop         # 桌面环境
│   ├── calculator      # 计算器
│   ├── clock           # 时钟
│   └── vshell          # 可视化 Shell
│
├── test/               # 测试程序
│   ├── fork_test       # fork 测试
│   └── mini-ltp/       # 内核兼容性测试
│       ├── bin/        # 24 个测试二进制文件
│       └── run_tests.sh
│
├── dev/                # 设备文件
│   ├── console
│   ├── null
│   ├── zero
│   ├── input/
│   │   └── event0      # 输入设备
│   └── fb0             # 帧缓冲
│
├── proc/               # procfs 挂载点
├── tmp/                # 临时文件
├── var/                # 变量数据
├── etc/                # 配置文件
└── lib/                # 库文件
```

### kernel/ - 内核源码

内核的核心源代码，按功能模块组织。

#### kernel/src/syscall/ - 系统调用分发

系统调用分发模块，将系统调用路由到具体实现：

| 文件 | 功能 | 系统调用 |
|------|------|----------|
| **dispatch.rs** | 系统调用分发器 | 所有系统调用入口 |
| **file.rs** | 文件系统调用 | open, close, read, write, lseek, fstat, mkdir, unlink, chdir, getcwd 等 |
| **process.rs** | 进程系统调用 | execve, wait4, exit, getpid, getppid 等 |
| **memory.rs** | 内存系统调用 | brk, mmap, munmap, mprotect 等 |
| **sched.rs** | 调度系统调用 | sched_yield, nice 等 |
| **signal.rs** | 信号系统调用 | kill, signal, sigprocmask 等 |
| **network.rs** | 网络系统调用 | socket, bind, listen, accept, connect, send, recv 等 |
| **io.rs** | I/O 系统调用 | poll, select, epoll 等 |
| **time.rs** | 时间系统调用 | time, gettimeofday, nanosleep 等 |
| **misc.rs** | 其他系统调用 | uname, sysinfo 等 |

#### kernel/src/arch/riscv64/ - RISC-V 架构

**重要**: 当前**仅支持 RISC-V 64位架构**。

| 文件 | 功能 | 代码量 |
|------|------|--------|
| **boot.S** | 启动代码 (汇编) | ~150 行 |
| **trap.S** | 异常向量表 (汇编) | ~200 行 |
| **boot.rs** | 初始化 | ~20 行 |
| **trap.rs** | 异常处理 | ~450 行 |
| **mm.rs** | 架构相关内存管理 | ~1,420 行 |
| **smp.rs** | 多核支持 | ~180 行 |
| **ipi.rs** | 处理器间中断 | ~130 行 |
| **context.rs** | 上下文切换 | ~270 行 |
| **cpu.rs** | CPU 操作 | ~140 行 |

**架构支持状态**:
- ✅ **RISC-V 64位 (RV64GC)** - 完全支持，当前默认平台
- ❌ **ARM64 (aarch64)** - 未实现
- ❌ **x86_64** - 未实现

---

## mini-ltp 测试套件

### 测试列表

| 测试名称 | 描述 |
|----------|------|
| test_fork | 进程创建 |
| test_getpid | 进程 ID 获取 |
| test_fileio | 文件 I/O (open/read/write/close) |
| test_pipe | 管道通信 |
| test_dup | 文件描述符复制 |
| test_mmap | 内存映射 |
| test_stat | 文件状态获取 |
| test_mkdir | 目录操作 |
| test_lseek | 文件定位 |
| test_time | 时间系统调用 |
| test_wait | 等待子进程 |
| test_exit | 进程退出 |
| test_brk | 堆内存管理 |
| test_chdir | 目录切换 |
| test_rename | 文件重命名 |
| test_unlink | 文件删除 |
| test_access | 访问权限检查 |
| test_writev | 向量 I/O |
| test_execve | 程序执行 |
| test_getuid | 用户/组 ID |
| test_nanosleep | 高精度睡眠 |
| test_ioctl | 终端 ioctl |
| test_fcntl | 文件控制 |
| test_fsync | 文件同步 |

### 运行测试

在 Rux 系统中：

```bash
cd /test/mini-ltp
./run_tests.sh
```

---

## 使用指南

### 编译

```bash
# 编译内核
make build

# 编译用户程序 (shell, apps, mini-ltp, toybox)
make user

# 创建 rootfs 镜像
make rootfs
```

### 运行

```bash
# 运行内核 (默认 shell)
make run

# 运行 GUI
make gui
```

### 测试

```bash
# 运行内核单元测试
make test

# 在 Rux 中运行 mini-ltp 测试
cd /test/mini-ltp
./run_tests.sh
```

---

## 注意事项

1. **config.rs 是自动生成的** - 不要手动编辑 `kernel/src/config.rs`，它由 `kernel/build.rs` 根据 `Kernel.toml` 自动生成。

2. **平台限制** - 当前仅支持 RISC-V 64位架构。

3. **系统调用兼容** - 使用 Linux 系统调用号，POSIX/ABI 完全兼容。

4. **模块导出** - 添加新模块时，确保在父模块的 `mod.rs` 中正确导出需要的接口。

5. **用户程序** - 使用 musl libc 静态链接，与内核 ABI 兼容。

---

**文档版本**: v5.0
**最后更新**: 2026-03-04
**维护者**: Rux 开发团队
