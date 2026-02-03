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

```
$ ./run.sh
Hello from Rust!
Rux Kernel v0.1.0 starting...
```

**已实现功能**：
- ✅ aarch64 平台启动代码
- ✅ UART 驱动 (PL011)
- ✅ 基础内存管理（页帧、堆分配器）
- ✅ 构建和测试脚本

### 🔄 Phase 3 进行中（2025-02-03）

**系统调用与隔离** - 核心功能已完成：
- ✅ 用户/内核地址空间隔离
- ✅ 用户空间数据复制（copy_from_user/copy_to_user）
- ✅ 28+ 系统调用实现
- ✅ 信号处理框架（sigaction/kill/rt_sigreturn/rt_sigprocmask）
- ✅ 信号处理函数调用机制（setup_frame 基础实现）

**当前内核输出**：
```
Rux Kernel v0.1.0 starting...
Target platform: aarch64
Initializing architecture...
arch::init() called
MM: MMU disabled (investigating translation fault issue)
Initializing trap handling...
System call support initialized
Initializing heap...
Initializing scheduler...
Scheduler: initialization complete
System ready
Getting PID...
Current PID: 0000000000000000
Testing fork syscall...
do_fork: start
do_fork: allocated pool slot
do_fork: creating task at pool slot
Task::new_task_at: start
Task::new_task_at: writing fields
Task::new_task_at: done
do_fork: task created at pool slot
do_fork: done
Fork success: child PID = 00000002
Entering main loop
```

**已实现系统调用 (39+)**：
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

**已发现并记录的问题**：
- ⚠️ MMU 使能问题（已决定暂时禁用，延后解决）
- ⚠️ GIC/Timer 初始化导致挂起（已暂时禁用）
- ⚠️ HLT/SVC 指令从 EL0 触发 SError（系统调用框架本身正常工作）
- ⚠️ println! 宏兼容性问题（优先使用 putchar）

---

## 📚 文档

- **[设计原则](docs/DESIGN.md)** - 项目的设计理念和技术约束
- **[开发路线图](docs/TODO.md)** - 详细的任务列表和进度追踪
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
cargo build --package rux --features aarch64 --release

# 在 QEMU 中运行
./run.sh
```

### 调试

```bash
# 使用 GDB 调试
./test_qemu.sh
```

---

## 📁 项目结构

```
Rux/
├── kernel/              # 内核代码
│   ├── src/
│   │   ├── arch/       # 平台相关代码
│   │   │   └── aarch64/    # ARM64 支持
│   │   ├── mm/         # 内存管理
│   │   ├── console.rs  # UART 驱动
│   │   ├── print.rs    # 打印宏
│   │   └── main.rs     # 内核入口
│   └── Cargo.toml
├── run.sh              # QEMU 运行脚本
├── test_qemu.sh        # GDB 调试脚本
├── docs/               # 文档目录
│   ├── DESIGN.md       # 设计原则
│   └── TODO.md         # 开发路线图
└── README.md           # 本文件
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

### Phase 2: 中断与进程 🔄
中断处理、进程调度、上下文切换、地址空间管理

### Phase 3: 系统调用与隔离 ⏳
系统调用接口、用户/内核隔离、信号处理

### Phase 4: 文件系统 ⏳
VFS、ext4、btrfs 支持

### Phase 5: 网络与 IPC ⏳
TCP/IP 协议栈、IPC 机制（管道、消息队列、共享内存）

### Phase 6: 多平台支持 ⏳
x86_64、riscv64 架构支持

### Phase 7: 设备驱动 ⏳
PCIe、存储控制器、网络设备

### Phase 8: 用户空间 ⏳
init 进程、shell、基础命令

### Phase 9: 优化与完善 ⏳
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
