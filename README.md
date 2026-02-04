# Rux

<div align="center">

**A Linux-like OS kernel entirely written in Rust**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-aarch64--x86__64--riscv64-informational.svg)](https://github.com/rust-osdev/rust-embedded)

Rux 是一个完全用 **Rust** 编写的类 Linux 操作系统内核（除必要的平台相关汇编代码外）。

</div>

---

## 🎯 项目目标

### ⚠️ 最高原则：POSIX/ABI 完全兼容，绝不创新

Rux 的核心目标是**用 Rust 重写 Linux 内核**，实现：

- **100% POSIX 兼容**：完全遵守 POSIX 标准
- **Linux ABI 兼容**：可运行原生 Linux 用户空间程序
- **系统调用兼容**：使用 Linux 的系统调用号和接口
- **文件系统兼容**：支持 ext4、btrfs 等 Linux 文件系统
- **ELF 格式兼容**：可执行文件格式与 Linux 完全一致

**严格禁止**：
- ❌ 绝不"优化"或"改进" Linux 的设计
- ❌ 绝不创造新的系统调用或接口
- ❌ 绝不为了"更优雅"而偏离标准

### 实现方式

除平台相关的必要汇编代码外，所有代码使用 Rust 编写，但**所有设计和接口必须完全遵循 Linux 标准**。

- **参考实现**：Linux 内核源码
- **接口标准**：POSIX 标准、Linux ABI
- **文档参考**：Linux man pages、内核文档

---

## 其他目标

- **多平台**：支持 aarch64、x86_64、riscv64 架构
- **模块化**：清晰的模块边界，便于开发和测试
- **可测试性**：完善的测试套件

---

## ✨ 当前状态

### ✅ Phase 1 完成（2025-02-02）

基础框架已就绪，内核可以在 QEMU (aarch64) 上成功启动：

**已实现功能**：
- ✅ aarch64 平台启动代码
- ✅ UART 驱动 (PL011)
- ✅ 基础内存管理（页帧、堆分配器）
- ✅ 构建和测试脚本

### ✅ Phase 2 完成（2025-02-03）

**进程管理**：
- ✅ 进程调度器框架
- ✅ 任务控制块 (TCB)
- ✅ 上下文切换
- ✅ Fork 系统调用

### ✅ Phase 3 完成（2025-02-03）

**系统调用与隔离** - 核心功能已完成：
- ✅ 用户/内核地址空间隔离
- ✅ 用户空间数据复制（copy_from_user/copy_to_user）
- ✅ 28+ 系统调用实现
- ✅ 信号处理框架（sigaction/kill/rt_sigreturn/rt_sigprocmask）
- ✅ 信号处理函数调用机制（setup_frame 基础实现）

**自定义集合类型** 🆕 - 绕过 alloc crate 的符号可见性问题：
- ✅ SimpleBox - 堆分配的值包装器
- ✅ SimpleVec - 动态数组
- ✅ SimpleString - 字符串包装器
- ✅ SimpleArc - 原子引用计数指针

**技术突破**：
成功解决了 Rust 编译器在 no_std 环境中的符号可见性问题（`__rust_no_alloc_shim_is_unstable_v2`），通过完全绕过 alloc crate，直接使用 GlobalAlloc trait 实现自定义集合类型。

**当前内核输出**：
```
Rux Kernel v0.1.0 starting...
Target platform: aarch64
Initializing architecture...
arch: IRQ disabled (will enable after GIC init)
Initializing trap handling...
System call support initialized
Initializing heap...
Initializing scheduler...
Scheduler: initialization complete
Initializing VFS...
GIC: Minimal init complete
Booting secondary CPUs...
[CPU1 up]
SMP: 2 CPUs online
SMP init complete, enabling IRQ...
IRQ enabled
System ready
Current PID: 0x0
Fork success: child PID = 0x2
Entering main loop
```

**已实现系统调用 (43+)**：
**进程管理**：
- ✅ fork/vfork (57/58) - 进程创建
- ✅ execve (59) - 执行程序
- ✅ exit (60) - 进程退出
- ✅ wait4 (61) - 等待子进程
- ✅ kill (62) - 发送信号
- ✅ getpid/getppid (39/110) - 获取进程 ID

**文件操作**：
- ✅ read/write (0/1) - 文件读写
- ✅ readv/writev (19/20) - 向量 I/O
- ✅ openat (2/245) - 打开文件
- ✅ close (3) - 关闭文件
- ✅ lseek (8) - 文件定位
- ✅ pipe (22) - 创建管道
- ✅ dup/dup2 (32/33) - 复制文件描述符
- ✅ fcntl (72) - 文件控制操作
- ✅ fsync/fdatasync (74/75) - 文件同步
- ✅ pselect6 (258) - I/O 多路复用（带信号掩码）
- ✅ ppoll (259) - I/O 多路复用（带信号掩码）

**目录操作**：
- ✅ mkdir (83) - 创建目录
- ✅ rmdir (84) - 删除目录
- ✅ unlink (82) - 删除文件链接
- ✅ getdents64 (61) - 读取目录项

**内存管理**：
- ✅ brk (12) - 改变数据段大小
- ✅ mmap (9) - 创建内存映射
- ✅ munmap (11) - 取消内存映射
- ✅ mprotect (10) - 改变内存保护属性
- ✅ mincore (27) - 查询页面驻留状态
- ✅ madvise (28) - 内存使用建议

**信号处理**：
- ✅ sigaction (48) - 设置信号处理
- ✅ rt_sigreturn (15) - 从信号处理返回
- ✅ rt_sigprocmask (14) - 信号掩码操作（完整实现）
- ✅ sigaltstack (131) - 信号栈支持
- ✅ kill (62) - 发送信号
- ✅ 信号帧结构体 (SignalFrame, UContext)
- ✅ 信号处理函数调用机制 (setup_frame, restore_sigcontext)

**系统信息**：
- ✅ uname (63) - 获取系统信息
- ✅ gettimeofday (96) - 获取系统时间
- ✅ clock_gettime (217) - 获取高精度时钟
- ✅ ioctl (16) - 设备控制
- ✅ getuid/getgid (102/104) - 获取用户/组 ID
- ✅ geteuid/getegid (107/108) - 获取有效用户/组 ID

**资源管理**：
- ✅ getrlimit/setrlimit (97/160) - 资源限制

### ✅ Phase 4 完成（2025-02-03）

**文件系统** - VFS 框架基础实现完成：
- ✅ VFS 初始化 (使用 SimpleArc)
- ✅ 文件操作接口 (file_open, file_close, file_read, file_write)
- ✅ 文件控制接口 (file_fcntl)
- ✅ I/O 多路复用接口 (io_poll)
- ✅ 文件描述符表管理 (FdTable, alloc_fd, close_fd, dup_fd)
- ✅ 路径解析模块 (Path, PathComponent, PathComponents)
- ✅ 超级块管理 (SuperBlock, SuperBlockFlags)
- ✅ 文件系统注册 (FileSystemType, FsRegistry)
- ✅ 挂载/卸载操作 (do_mount, do_umount, mount_fs, kill_super)
- ✅ 挂载点管理 (VfsMount, MntNamespace, MountTreeIter)
- ✅ **RootFS 内存文件系统** (RootFSNode, RootFSSuperBlock) - 完整实现
- ✅ **根文件系统挂载到命名空间** - 已完成
- ✅ SimpleString 路径操作方法扩展
- ✅ **全局状态同步保护** - 使用 AtomicPtr 保护全局变量
- ✅ **SimpleArc 统一** - VFS 层统一使用 SimpleArc
- ✅ **FdTable 安全初始化** - 修复 MaybeUninit UB 问题
- ✅ 优化：移除调试代码，清理链接器脚本

**RootFS 特性**：
- 基于 RAM 的文件存储
- 支持目录和常规文件
- 文件创建、查找、读取、写入
- 目录列表操作
- 自动 inode ID 分配
- 根文件系统挂载到命名空间
- **线程安全** - AtomicPtr 保护全局状态

### ✅ Phase 5 完成（2025-02-04）

**SMP (对称多处理) 支持** - 双核启动成功：
- ✅ 次核启动入口点 (boot.S secondary_entry)
- ✅ PSCI CPU 唤醒 (HVC 调用)
- ✅ Per-CPU 栈管理 (每个 CPU 16KB 栈)
- ✅ SMP 数据结构 (SmpData, CpuBootInfo)
- ✅ CPU 数量检测 (get_active_cpu_count)
- ✅ 测试脚本 (test_smp.sh)
- ✅ **GICv3 中断控制器** - 最小初始化完成
- ✅ **IPI (核间中断)** - 基于 SGI 的 IPI 机制
- ✅ **MMU 多级页表** - 已启用并正常工作
- ✅ **中断风暴修复** - IRQ 时序控制优化

**测试验证**：
```
[SMP: Calling PSCI for CPU 1]
[SMP: PSCI result = 0000000000000000]
[CPU1 up]
SMP: 2 CPUs online
```

**技术要点**：
- 使用 PSCI (Power State Coordination Interface) 唤醒次核
- HVC (Hypervisor Call) 而非 SMC (Secure Monitor Call) 用于 QEMU virt
- Per-CPU 栈空间通过链接器脚本分配
- 次核通过 PSCI_CPU_ON (0xC4000003) 启动
- GICv3 使用系统寄存器访问（避免 GICD 内存访问挂起）
- IPI 使用 ICC_SGI1R_EL1 发送 Software Generated Interrupts
- MMU 使用 3 级页表（4KB 页面）
- Spurious interrupt 处理（IRQ ID 1023）
- IRQ 在 SMP 初始化完成后启用（避免中断风暴）

### ✅ Phase 6 完成（2025-02-04）

**代码审查与优化** - 全面完成：
- ✅ **全面代码审查** - 发现并记录 15 个问题
- ✅ **调试输出清理** - 移除 50+ 处 putchar() 循环
- ✅ **条件编译优化** - 使用 #[cfg(debug_assertions)] 控制调试输出
- ✅ **测试脚本完善** - test_suite.sh, test_smp.sh, test_ipi.sh, test_qemu.sh
- ✅ **Makefile 增强** - 添加 `make smp` 和 `make ipi` 快捷命令
- ✅ **CODE_REVIEW.md** - 详细记录所有发现的问题和修复计划

**清理的文件**：
- [boot.rs](kernel/src/arch/aarch64/boot.rs) - 2 处
- [gicv3.rs](kernel/src/drivers/intc/gicv3.rs) - 17 处
- [ipi.rs](kernel/src/arch/aarch64/ipi.rs) - 8 处
- [allocator.rs](kernel/src/mm/allocator.rs) - 1 处
- [main.rs](kernel/src/main.rs) - 20+ 处

**发现的主要问题**（详见 [CODE_REVIEW.md](docs/CODE_REVIEW.md)）：
- 🔴 内存分配器无法释放内存（bump allocator 的 dealloc 是空实现）
- 🔴 全局单队列调度器限制多核扩展
- 🔴 过多的调试输出（已修复 ✅）
- 🟡 VFS 函数指针安全性问题
- 🟡 SimpleArc Clone 支持问题

### 🔄 Phase 7 进行中（2025-02-04）

**文件系统** - VFS 框架持续开发中：
- ✅ VFS 初始化 (使用 SimpleArc)
- ✅ 文件操作接口 (file_open, file_close, file_read, file_write)
- ✅ 文件控制接口 (file_fcntl)
- ✅ I/O 多路复用接口 (io_poll)
- ✅ 文件描述符表管理 (FdTable, alloc_fd, close_fd, dup_fd)
- ✅ 路径解析模块 (Path, PathComponent, PathComponents)
- ✅ 超级块管理 (SuperBlock, SuperBlockFlags)
- ✅ 文件系统注册 (FileSystemType, FsRegistry)
- ✅ 挂载/卸载操作 (do_mount, do_umount, mount_fs, kill_super)
- ✅ 挂载点管理 (VfsMount, MntNamespace, MountTreeIter)
- ✅ **RootFS 内存文件系统** (RootFSNode, RootFSSuperBlock) - 完整实现
- ✅ **根文件系统挂载到命名空间** - 已完成
- ✅ SimpleString 路径操作方法扩展
- ✅ **全局状态同步保护** - 使用 AtomicPtr 保护全局变量
- ✅ **SimpleArc 统一** - VFS 层统一使用 SimpleArc
- ✅ **FdTable 安全初始化** - 修复 MaybeUninit UB 问题
- ✅ 优化：移除调试代码，清理链接器脚本

**RootFS 特性**：
- 基于 RAM 的文件存储
- 支持目录和常规文件
- 文件创建、查找、读取、写入
- 目录列表操作
- 自动 inode ID 分配
- 根文件系统挂载到命名空间
- **线程安全** - AtomicPtr 保护全局状态

**待实现**：
- ⏳ 符号链接解析 (follow_link)
- ⏳ 完善 SimpleArc Clone 支持
- ⏳ ext4/btrfs 文件系统
- ⏳ 完善文件删除、重命名操作

**已发现并记录的问题**：
- ⚠️ MMU 使能问题（已决定暂时禁用，延后解决）
- ⚠️ GIC/Timer 初始化导致挂起（已暂时禁用）
- ⚠️ HLT/SVC 指令从 EL0 触发 SError（系统调用框架本身正常工作）
- ⚠️ println! 宏兼容性问题（优先使用 putchar）

---

## 📚 文档

- **[设计原则](docs/DESIGN.md)** - 项目的设计理念和技术约束
- **[开发路线图](docs/TODO.md)** - 详细的任务列表和进度追踪
- **[代码审查记录](docs/CODE_REVIEW.md)** - 代码审查发现的问题和修复进度
- **[自定义集合类型](docs/COLLECTIONS.md)** - SimpleBox/SimpleVec/SimpleArc 的设计与实现
- **[API 文档](https://docs.rs/)** - Rust 代码文档（待生成）

---

## 🚀 快速开始

### 环境要求

- Rust 工具链（stable）
- QEMU 系统模拟器
- aarch64 工具链（可选，用于调试）

### 构建和运行

```bash
# 克隆仓库
git clone https://github.com/your-username/rux.git
cd rux

# 构建内核
make build

# 在 QEMU 中运行（单核）
make run

# 或者直接运行测试脚本
./test/run.sh
```

### 调试

```bash
# 使用 GDB 调试
./test/debug.sh

# 测试 SMP 双核启动
./test/test_smp.sh

# 测试 IPI 功能
./test/test_ipi.sh

# 运行完整测试套件
./test/test_suite.sh

# 测试不同 QEMU 配置
./test/test_qemu.sh
```

---

## 📁 项目结构

```
Rux/
├── kernel/                 # 内核代码
│   ├── src/
│   │   ├── arch/           # 平台相关代码
│   │   │   └── aarch64/    # ARM64 支持
│   │   │       ├── boot.S     # 启动汇编 (含次核入口)
│   │   │       ├── smp.rs     # SMP 支持 (次核启动、Per-CPU 数据)
│   │   │       ├── ipi.rs     # IPI (核间中断) 支持
│   │   │       └── mm.rs      # 内存管理 (MMU、页表)
│   │   ├── mm/             # 内存管理
│   │   │   ├── allocator.rs # 堆分配器 (Bump Allocator)
│   │   │   ├── pagemap.rs   # 页表管理
│   │   │   └── vma.rs       # 虚拟内存区域
│   │   ├── drivers/        # 设备驱动
│   │   │   ├── intc/       # 中断控制器
│   │   │   │   └── gicv3.rs # GICv3 驱动
│   │   │   ├── timer/      # 定时器驱动
│   │   │   └── uart/       # UART 驱动
│   │   ├── collection.rs   # 自定义集合类型
│   │   ├── console.rs      # UART 驱动
│   │   ├── print.rs        # 打印宏
│   │   ├── process/        # 进程管理
│   │   │   ├── sched.rs    # 调度器
│   │   │   ├── task.rs     # 任务控制块
│   │   │   └── signal.rs   # 信号处理
│   │   ├── fs/             # 文件系统
│   │   │   ├── vfs.rs      # VFS 框架
│   │   │   ├── rootfs.rs   # RootFS 内存文件系统
│   │   │   ├── file.rs     # 文件抽象
│   │   │   └── inode.rs    # Inode 管理
│   │   └── main.rs         # 内核入口
│   └── Cargo.toml
├── test/                   # 测试脚本
│   ├── run.sh              # 快速运行内核
│   ├── test_smp.sh         # SMP 功能测试
│   ├── test_ipi.sh         # IPI 功能测试
│   ├── test_qemu.sh        # QEMU 配置测试
│   ├── test_suite.sh       # 完整测试套件
│   └── debug.sh            # GDB 调试脚本
├── docs/                   # 文档目录
│   ├── DESIGN.md           # 设计原则
│   ├── TODO.md             # 开发路线图
│   ├── CODE_REVIEW.md      # 代码审查记录
│   └── COLLECTIONS.md      # 自定义集合类型文档
├── build/                  # 构建工具
│   └── Makefile            # 构建脚本
├── Makefile                # 根 Makefile (快捷命令)
├── Kernel.toml             # 内核配置文件
├── Cargo.toml              # 工作空间配置
└── README.md               # 本文件
```

---

## 🛠️ 开发

### 构建系统

- **Cargo**：Rust 包管理和构建工具
- **链接器脚本**：`kernel/src/linker-aarch64.ld`
- **交叉编译**：通过 `.cargo/config.toml` 配置

### 添加新功能

1. 在 [`docs/TODO.md`](docs/TODO.md) 中找到对应的任务
2. 创建相应的模块文件
3. 实现功能并添加测试
4. 更新文档

### 代码风格

- 使用 `rustfmt` 格式化代码
- 使用 `clippy` 检查代码质量
- 遵循 [Rust API 指南](https://rust-lang.github.io/api-guidelines/)

---

## 🗺️ 路线图

### Phase 1: 基础框架 ✅
项目初始化、启动代码、UART 驱动、基础内存管理

### Phase 2: 中断与进程 ✅
中断处理、进程调度、上下文切换、地址空间管理

### Phase 3: 系统调用与隔离 ✅
系统调用接口、用户/内核隔离、信号处理、**自定义集合类型**

### Phase 4: 文件系统 ✅ 基础框架完成
VFS 框架、文件描述符、基本的文件操作

### Phase 5: SMP 支持 ✅ 基础框架完成
多核启动、Per-CPU 数据、PSCI 接口、GICv3 初始化、IPI 机制、MMU 启用

### Phase 6: 代码审查 ✅ 完成
全面代码审查、调试输出清理、测试脚本完善

### Phase 7: Per-CPU 优化 ⏳ 进行中
Per-CPU 运行队列、负载均衡、内存分配器改进

### Phase 8: 网络与 IPC ⏳
x86_64、riscv64 架构支持

### Phase 8: 设备驱动 ⏳
PCIe、存储控制器、网络设备

### Phase 9: 用户空间 ⏳
init 进程、shell、基础命令

### Phase 10: 优化与完善 ⏳
性能优化、稳定性提升、文档完善

详见 [`docs/TODO.md`](docs/TODO.md)

---

## 🤝 贡献

欢迎贡献！请查看 [`docs/TODO.md`](docs/TODO.md) 了解当前需要帮助的任务。

### 贡献流程

1. Fork 项目
2. 创建功能分支
3. 提交更改
4. 推送到分支
5. 创建 Pull Request

---

## 📄 许可证

MIT License - 详见 [LICENSE](LICENSE)

---

## 🙏 致谢

本项目受到以下项目的启发：

- [Phil Opp's Writing an OS in Rust](https://os.phil-opp.com/)
- [Redox OS](https://gitlab.redox-os.org/redox-os/redox)
- [Theseus OS](https://github.com/theseus-os/Theseus)
- [Linux Kernel](https://www.kernel.org/)

---

## 📮 联系方式

- 项目主页：[GitHub](https://github.com/your-username/rux)
- 问题反馈：[Issues](https://github.com/your-username/rux/issues)

---

**注意**：本项目主要用于学习和研究目的，不适合生产环境使用。
