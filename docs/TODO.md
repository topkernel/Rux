# Rux 开发路线图与TODO

## 项目概览

**当前状态**：Phase 4 进行中 🔄 - VFS 框架持续完善

**最后更新**：2025-02-03

**最新成就**：
- ✅ 成功解决 alloc crate 符号可见性问题
- ✅ 实现自定义集合类型 (SimpleBox/SimpleVec/SimpleString/SimpleArc)
- ✅ VFS 框架就绪并成功初始化
- ✅ 文件操作接口定义完成 (file_open, file_close, file_read, file_write, file_fcntl, io_poll)
- ✅ 文件描述符表管理 (FdTable) 已在 file.rs 中实现
- ✅ 路径解析模块 (path.rs) 实现完成
- ✅ 超级块管理 (SuperBlock, FileSystemType) 已实现
- ✅ 文件系统注册机制 (register_filesystem, get_fs_type) 已实现
- ✅ 挂载/卸载操作框架 (do_mount, do_umount) 已实现
- ✅ SimpleString 添加路径操作方法 (starts_with, split_at, find, strip_prefix)
- ✅ 移除调试代码，优化内存分配器
- ✅ 清理链接器脚本（移除无用的 alloc 符号引用）

---

## Phase 1: 基础框架 ✅ **已完成**

### ✅ 已完成项目

- [x] **项目结构搭建**
  - [x] Workspace 配置
  - [x] 内核 crate 配置（no_std）
  - [x] 交叉编译配置（aarch64-unknown-none）
  - [x] 链接器脚本
  - [x] 构建和测试脚本

- [x] **平台启动代码 (aarch64)**
  - [x] 汇编启动代码 (`arch/aarch64/boot.S`)
  - [x] 异常级别检测和处理（EL1/EL2/EL3）
  - [x] 栈设置
  - [x] BSS 段清零
  - [x] 与 Rust 代码的链接

- [x] **UART 驱动**
  - [x] PL011 UART 驱动实现
  - [x] 字符输入/输出
  - [x] 波特率配置
  - [x] `println!` 宏实现

- [x] **基础内存管理**
  - [x] 页帧管理（PhysFrame、VirtPage）
  - [x] 页表项结构（PageTableEntry）
  - [x] MMU 基础代码（aarch64）
  - [x] 堆分配器（链表分配器）

**验证状态**：
```
$ ./run.sh
Hello from Rust!
Rux Kernel v0.1.0 starting...
```

---

## Phase 2: 中断与进程 **进行中**

### 🔄 当前任务

#### 2.0 进程管理基础 ✅ **已完成**
- [x] **EL0 切换机制** (`process/usermod.rs`, `arch/aarch64/context.rs`)
- [x] **EL0 切换机制** (`process/usermod.rs`, `arch/aarch64/context.rs`)
  - [x] 通过 `eret` 指令从 EL1 切换到 EL0
  - [x] SPSR 和 ELR_EL1 正确设置
  - [x] 用户栈（SP_EL0）正确设置
  - [x] 用户代码可以在 EL0 正常执行
  - [x] 验证测试：NOP、B . 等指令正常工作

- [x] **系统调用框架验证** (`arch/aarch64/syscall.rs`)
  - [x] 系统调用处理程序（`syscall_handler`）正常工作
  - [x] 系统调用分发机制正确
  - [x] `sys_read` 等系统调用实现正常
  - [x] 从内核直接调用系统调用验证成功

**完成时间**：2025-02-03

**验证状态**：
```
Testing direct syscall call from kernel...
[SVC:00]
sys_read: invalid fd
Syscall returned error (expected)
```

**已知问题**：
- HLT/SVC 指令从 EL0 触发 SError 而不是同步异常
  - 异常类型：0x0B (SError from EL0 32-bit)
  - ESR_EL1：EC=0x00 (Trapped WFI/WFE)
  - 这可能是 QEMU 特有行为或配置问题
  - 系统调用框架本身已验证可正常工作

#### 2.0.1 进程创建 (fork) ✅ **已完成 (2025-02-03)**
- [x] **fork 系统调用** (`arch/aarch64/syscall.rs`, `process/sched.rs`)
  - [x] `sys_fork` (系统调用号 57) - 创建子进程
  - [x] `sys_vfork` (系统调用号 58) - 创建共享地址空间的子进程
  - [x] 静态任务池实现（避免栈分配问题）
  - [x] `Task::new_task_at()` 函数用于在指定位置构造 Task
  - [x] **验证成功**：成功创建子进程，PID = 2

**验证状态**：
```
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
```

**关键改进**：
- 使用静态任务池（`TASK_POOL`）代替堆分配
- 任务池大小：16 个槽位
- 每个槽位：512 字节（足够存储 Task 结构体）

#### 2.0.2 文件系统系统调用 ✅ **部分完成 (2025-02-03)**
- [x] **文件描述符管理** (`fs/file.rs`, `arch/aarch64/syscall.rs`)
  - [x] `sys_close` (系统调用号 3) - 关闭文件描述符
  - [x] `sys_lseek` (系统调用号 8) - 重定位文件读写位置
  - [x] `sys_dup` (系统调用号 32) - 复制文件描述符
  - [x] `sys_dup2` (系统调用号 33) - 复制文件描述符到指定位置
  - [x] `close_file_fd()` 函数 - 关闭文件描述符的底层实现

**已完成系统调用列表**：
- ✅ sys_read (0) - 从文件描述符读取
- ✅ sys_write (1) - 写入到文件描述符
- ✅ sys_openat (2/245) - 打开文件
- ✅ sys_close (3) - 关闭文件描述符
- ✅ sys_lseek (8) - 重定位文件读写位置
- ✅ sys_pipe (22) - 创建管道
- ✅ sys_dup (32) - 复制文件描述符
- ✅ sys_dup2 (33) - 复制文件描述符到指定位置
- ✅ sys_sigaction (48) - 设置信号处理函数
- ✅ sys_fork (57) - 创建子进程
- ✅ sys_vfork (58) - 创建共享地址空间的子进程
- ✅ sys_execve (59) - 执行新程序
- ✅ sys_exit (60) - 退出进程
- ✅ sys_wait4 (61) - 等待子进程
- ✅ sys_kill (62) - 发送信号
- ✅ sys_getpid (110) - 获取进程 ID
- ✅ sys_getppid (110) - 获取父进程 ID
- ✅ sys_getuid (102) - 获取用户 ID
- ✅ sys_getgid (104) - 获取组 ID
- ✅ sys_geteuid (107) - 获取有效用户 ID
- ✅ sys_getegid (108) - 获取有效组 ID

**待实现**：
- ⏳ sys_readv/sys_writev - 向量读写
- ⏳ sys_pread64/sys_pwrite64 - 带偏移量的读写
- ⏳ sys_select/poll/epoll - I/O 多路复用
- ⏳ sys_ioctl - 设备控制
- ⏳ sys_fcntl - 文件控制操作

#### 2.1 中断和异常处理框架 ✅ **大部分完成**
- [x] **异常向量表** (`arch/aarch64/trap.S`)
  - [x] 同步异常处理
  - [x] IRQ 处理
  - [x] FIQ 处理
  - [x] SError 处理
  - [x] 栈帧布局修复（elr/esr/spsr 位置）
  - [x] 寄存器恢复修复
- [x] **中断控制器** (`drivers/intc/gicv3.rs`)
  - [x] GICv3 驱动初始化
  - [x] 中断使能/禁用
  - [x] 中断分发
- [x] **异常处理框架** (`arch/aarch64/trap.rs`)
  - [x] 上下文保存/恢复
  - [x] 异常分发
  - [x] 系统调用入口
  - [x] SVC 系统调用处理
  - [x] 异常类型识别和调试输出
- [x] **定时器驱动** (`drivers/timer/armv8.rs`)
  - [x] ARMv8 架构定时器
  - [x] 周期性中断
  - [x] 时间戳计数器

**完成时间**：2025-02-03

**依赖关系**：进程调度依赖于中断框架

---

#### 2.2 进程调度器 ✅ **基础框架完成**
- [x] **调度器框架** (`process/sched.rs`)
  - [x] 调度器接口定义
  - [x] 就绪队列管理
  - [x] Round Robin 调度算法
- [x] **进程控制块** (`process/mod.rs`)
  - [x] PCB 结构定义
  - [x] 进程状态管理
  - [x] 进程创建/销毁
- [x] **上下文切换** (`arch/aarch64/context.rs`)
  - [x] 保存通用寄存器
  - [x] 保存特殊寄存器（SP、ELR、SPSR）
  - [x] 切换到下一个进程
  - [x] switch_to_user 函数
- [ ] **调度策略** (`process/sched_rr.rs`, `process/sched_cfs.rs`)
  - [x] Round Robin 调度
  - [ ] 完全公平调度（CFS）
  - [ ] 实时调度策略

**完成时间**：2025-02-03

**依赖关系**：系统调用依赖于进程管理

---

#### 2.3 进程地址空间 🔄 **进行中**
- [x] **地址空间管理** (`mm/vma.rs`)
  - [x] VMA（虚拟内存区域）结构
  - [x] 地址空间布局
  - [ ] mmap/munmap 支持
- [x] **页表管理** (`arch/aarch64/mm.rs`)
  - [x] 页表创建/销毁
  - [x] 页表条目结构
  - [x] 页表映射设置
  - [ ] 页表取消映射
  - [ ] 写时复制
- [ ] **内存映射** (`mm/mmap.rs`)
  - [ ] 匿名映射
  - [ ] 文件映射
  - [ ] 共享映射
- [ ] **缺页异常处理** (`mm/fault.rs`)
  - [ ] 缺页异常处理
  - [ ] 延迟分配
  - [ ] 写时复制

**⚠️ MMU 使能问题 - 已决定暂时禁用**

**问题描述**：
- 内核在 `msr sctlr_el1, x0` 指令后挂起
- 页表描述符格式已修复（AP、SH、AttrIndx 字段）
- MAIR 配置已修复（Normal memory at AttrIdx 0, Device memory at AttrIdx 1）
- 恒等映射已设置（0x40000000-0x7FFFFFFF）
- TLB 刷新已添加
- 但 MMU 启用后立即挂起，无异常输出

**调查结果**：
1. 页表条目格式已按 ARMv8 规范修正
2. T0SZ 值已修正（尝试过 T0SZ=16 和 T0SZ=0）
3. 通过 GDB 调试发现递归异常问题（异常处理程序本身触发异常）
4. 对于 64 位 VA（T0SZ=0），Entry[0] 映射到 0x0000_0000，不包含内核代码

**决定**：
- **暂时禁用 MMU**，先实现其他不依赖 MMU 的功能
- 内核当前在 MMU 禁用状态下运行正常
- 系统调用、进程调度等功能可以继续开发
- 等待更多功能实现后再重新审视 MMU 问题

**当前状态**：
- MMU 已在 `mm.rs::init()` 中明确禁用
- 内核可正常启动、处理中断、执行系统调用
- 所有非内存映射相关的功能可以正常工作

**预计完成时间**：延后至 Phase 4 或 Phase 5

---

#### 2.2 进程调度器
- [ ] **调度器框架** (`process/scheduler.rs`)
  - [ ] 调度器接口定义
  - [ ] 就绪队列管理
  - [ ] 调度算法（Round Robin、CFS）
- [ ] **进程控制块** (`process/pcb.rs`)
  - [ ] PCB 结构定义
  - [ ] 进程状态管理
  - [ ] 进程创建/销毁
- [ ] **上下文切换** (`arch/aarch64/context.S`)
  - [ ] 保存通用寄存器
  - [ ] 保存特殊寄存器（SP、ELR、SPSR）
  - [ ] 切换到下一个进程
- [ ] **调度策略** (`process/sched_rr.rs`, `process/sched_cfs.rs`)
  - [ ] Round Robin 调度
  - [ ] 完全公平调度（CFS）
  - [ ] 实时调度策略

**预计完成时间**：3-5 天

**依赖关系**：系统调用依赖于进程管理

---

#### 2.3 进程地址空间
- [ ] **地址空间管理** (`mm/vma.rs`)
  - [ ] VMA（虚拟内存区域）结构
  - [ ] 地址空间布局
  - [ ] mmap/munmap 支持
- [ ] **页表管理** (`mm/page_table.rs`)
  - [ ] 页表创建/销毁
  - [ ] 页表映射/取消映射
  - [ ] 页表共享（写时复制）
- [ ] **内存映射** (`mm/mmap.rs`)
  - [ ] 匿名映射
  - [ ] 文件映射
  - [ ] 共享映射
- [ ] **缺页异常处理** (`mm/fault.rs`)
  - [ ] 缺页异常处理
  - [ ] 延迟分配
  - [ ] 写时复制

**预计完成时间**：3-4 天

---

## Phase 3: 系统调用与隔离 🔄 **进行中 (2025-02-03)**

### 3.1 系统调用接口 ✅ **部分完成**

- [x] **系统调用框架** (`arch/aarch64/syscall.rs`)
  - [x] 系统调用表
  - [x] 参数解析
  - [x] 返回值处理

- [x] **系统调用实现** - 25+ 系统调用已实现
  - [x] 进程相关：`fork` (57)、`vfork` (58)、`execve` (59)、`exit` (60)、`wait4` (61)、`getpid` (39)、`getppid` (110)
  - [x] 文件相关：`read` (0)、`write` (1)、`openat` (2/245)、`close` (3)、`lseek` (8)、`pipe` (22)、`dup` (32)、`dup2` (33)
  - [x] 内存相关：`brk` (12)、`mmap` (9)、`munmap` (11)
  - [x] 其他：`ioctl` (16)、`uname` (63)、`getuid` (102)、`getgid` (104)、`geteuid` (107)、`getegid` (108)

- [x] **用户/内核隔离**
  - [x] 地址验证：`verify_user_ptr()`、`verify_user_ptr_array()`
  - [x] 参数复制：`copy_user_string()`、`copy_from_user()`
  - [x] 结果复制：`copy_to_user()`
  - [x] 用户空间地址范围定义（USER_SPACE_END）

**完成时间**：2025-02-03

**待实现**：
- ⏳ `gettimeofday` - 获取时间
- ⏳ `mprotect` - 修改内存保护
- ⏳ `mincore` - 查询页面状态
- ⏳ `madvise` - 内存建议
- ⏳ `readv`/`writev` - 向量 I/O
- ⏳ `select`/`poll` - I/O 多路复用

---

### 3.2 信号处理 ✅ **部分完成 (2025-02-03)**

- [x] **信号框架** (`signal/signal.rs`)
  - [x] 信号定义（Linux 兼容，Signal 枚举）
  - [x] 信号掩码（SignalStruct.mask）
  - [x] 信号处理函数（SigAction）
  - [x] 待处理信号队列（SigPending）

- [x] **信号发送**
  - [x] `kill` 系统调用 (62)
  - [x] `sigaction` 系统调用 (48)
  - [x] 信号队列（SigPending）
  - [x] 信号传递机制（SigPending::add()）

- [x] **信号处理**
  - [x] `rt_sigreturn` 系统调用 (15)
  - [ ] 信号交付（do_signal）
  - [ ] 信号处理函数调用
  - [ ] 完整的 sigreturn 实现（上下文恢复）

**完成时间**：2025-02-03（部分）

**待实现**：
- ⏳ `sigprocmask` - 信号掩码操作 (14)
- ⏳ `rt_sigprocmask` - 实时信号掩码 (14)
- ⏳ 信号交付机制（在异常处理中检查并调用）
- ⏳ 信号上下文保存/恢复
- ⏳ 信号栈（sigaltstack）

**预计完成时间**：1-2 天

---

## Phase 4: 文件系统 🔄 **进行中 (2025-02-03)**

### 4.1 VFS 虚拟文件系统
- [x] **VFS 框架** (`fs/vfs.rs`)
  - [x] VFS 初始化 (使用 SimpleArc)
  - [x] 文件系统接口框架
  - [x] 基础文件操作 (file_open, file_close, file_read, file_write)
  - [x] 文件控制接口 (file_fcntl)
  - [x] I/O 多路复用接口 (io_poll)
- [x] **文件描述符管理** (`fs/file.rs`)
  - [x] FdTable 实现
  - [x] fd 分配/释放 (alloc_fd, close_fd)
  - [x] fd 复制 (dup_fd)
  - [x] 文件对象 (File) 和文件操作 (FileOps)
- [x] **路径解析** (`fs/path.rs`)
  - [x] 路径名解析 (filename_parentname, path_lookup)
  - [x] 绝对路径/相对路径判断
  - [x] 路径组件迭代器 (PathComponents)
  - [x] 父目录和文件名获取 (parent, file_name)
  - [ ] 符号链接解析 (TODO: follow_link)
- [x] **VFS 核心对象**
  - [x] File 结构和 FileOps (fs/file.rs)
  - [x] Inode 结构和 INodeOps (fs/inode.rs) - 已实现，使用 alloc::sync::Arc
  - [x] Dentry 结构 (fs/dentry.rs) - 已实现，使用 alloc::sync::Arc
  - [ ] 需要将 Inode/Dentry 更新为使用 SimpleArc
- [x] **文件系统注册机制** (`fs/superblock.rs`)
  - [x] FileSystemType 文件系统类型
  - [x] FsRegistry 文件系统注册表
  - [x] SuperBlock 超级块结构
  - [x] FsContext 挂载上下文
  - [x] register_filesystem/unregister_filesystem
  - [x] get_fs_type 查找文件系统类型
  - [x] do_mount/do_umount 挂载卸载操作
- [ ] **挂载点管理**
  - [ ] VfsMount 结构
  - [ ] 挂载命名空间
  - [ ] 根文件系统挂载
  - [ ] 挂载点遍历

**预计完成时间**：5-7 天

**当前状态**：VFS 核心框架已完整实现，包括文件描述符管理、路径解析、文件系统注册和超级块管理。下一步是实现挂载点管理和根文件系统挂载。

---

### 4.2 ext4 文件系统
- [ ] **ext4 实现** (`fs/ext4/`)
  - [ ] 超级块读取
  - [ ] inode 和块位图
  - [ ] 目录解析
  - [ ] 文件读取/写入
  - [ ] 日志（journaling）
- [ ] **缓存管理** (`fs/cache.rs`)
  - [ ] inode 缓存
  - [ ] 块缓存
  - [ ] 目录项缓存

**预计完成时间**：7-10 天

---

### 4.3 btrfs 文件系统
- [ ] **btrfs 实现** (`fs/btrfs/`)
  - [ ] B-tree 结构
  - [ ] 快照
  - [ ] 写时复制
  - [ ] 压缩

**预计完成时间**：10-14 天（可选）

---

## Phase 5: 网络与高级功能

### 5.1 网络协议栈
- [ ] **网络框架** (`net/net.rs`)
  - [ ] socket 接口
  - [ ] 协议族管理
  - [ ] 网络设备抽象
- [ ] **以太网驱动** (`drivers/net/virtio-net.rs`)
  - [ ] virtio-net 驱动
  - [ ] 数据包发送/接收
  - [ ] 中断处理
- [ ] **IP 协议** (`net/ip.rs`)
  - [ ] IPv4
  - [ ] 路由
  - [ ] 分片重组
- [ ] **TCP/UDP** (`net/tcp.rs`, `net/udp.rs`)
  - [ ] 连接管理
  - [ ] 滑动窗口
  - [ ] 拥塞控制

**预计完成时间**：14-21 天

---

### 5.2 IPC 机制
- [ ] **管道** (`ipc/pipe.rs`)
  - [ ] 匿名管道
  - [ ] 命名管道（FIFO）
- [ ] **消息队列** (`ipc/msg.rs`)
  - [ ] System V 消息队列
  - [ ] POSIX 消息队列
- [ ] **共享内存** (`ipc/shm.rs`)
  - [ ] System V 共享内存
  - [ ] POSIX 共享内存
- [ ] **信号量** (`ipc/sem.rs`)
  - [ ] System V 信号量
  - [ ] POSIX 信号量

**预计完成时间**：7-10 天

---

### 5.3 同步原语
- [ ] **锁机制** (`sync/lock.rs`)
  - [ ] Mutex（自旋锁）
  - [ ] RwLock（读写锁）
  - [ ] SeqLock（序列锁）
  - [ ] RCU（读-拷贝-更新）
- [ ] **并发原语** (`sync/atomic.rs`)
  - [ ] Atomic 类型
  - [ ] 内存屏障
  - [ ] 原子操作

**预计完成时间**：3-5 天

---

## Phase 6: 多平台支持

### 6.1 x86_64 平台
- [ ] **x86_64 启动代码**
  - [ ] 汇编启动代码
  - [ ] 长模式设置
- [ ] **x86_64 内存管理**
  - [ ] 页表设置（4级页表）
  - [ ] MMU 配置
- [ ] **x86_64 中断处理**
  - [ ] IDT 设置
  - [ ] 中断处理
- [ ] **x86_64 驱动**
  - [ ] UART（8250/16550）
  - [ ] APIC
  - [ ] HPET 定时器

**预计完成时间**：10-14 天

---

### 6.2 riscv64 平台
- [ ] **riscv64 启动代码**
- [ ] **riscv64 内存管理**
- [ ] **riscv64 中断处理**
- [ ] **riscv64 驱动**

**预计完成时间**：7-10 天

---

## Phase 7: 设备驱动

### 7.1 PCIe 支持
- [ ] **PCIe 枚举** (`drivers/pci/`)
  - [ ] PCI 总线枚举
  - [ ] 设备配置
  - [ ] 资源分配
- [ ] **PCIe 驱动框架**
  - [ ] 驱动注册
  - [ ] 设备匹配
  - [ ] 资源映射

**预计完成时间**：7-10 天

---

### 7.2 存储控制器
- [ ] **AHCI 驱动** (`drivers/ahci.rs`)
  - [ ] SATA 控制器
  - [ ] 命令队列
  - [ ] DMA 支持
- [ ] **NVMe 驱动** (`drivers/nvme.rs`)
  - [ ] NVMe 控制器
  - [ ] 命令提交和完成
  - [ ] 多队列支持

**预计完成时间**：7-10 天

---

### 7.3 图形和输入
- [ ] **帧缓冲** (`drivers/framebuffer.rs`)
  - [ ] vesafb/efifb
  - [ ] 图形模式设置
- [ ] **键盘驱动** (`drivers/keyboard.rs`)
  - [ ] PS/2 键盘
  - [ ] USB 键盘
- [ ] **鼠标驱动** (`drivers/mouse.rs`)

**预计完成时间**：5-7 天（可选）

---

## Phase 8: 用户空间

### 8.1 用户空间工具
- [ ] **init 进程**
  - [ ] PID 1 init
  - [ ] 启动脚本解析
  - [ ] 进程管理
- [ ] **shell**
  - [ ] 基础命令支持
  - [ ] 管道和重定向
  - [ ] 作业控制
- [ ] **基础命令**
  - [ ] `ls`、`cd`、`pwd`
  - [ ] `cat`、`echo`、`cp`、`mv`
  - [ ] `mkdir`、`rm`
  - [ ] `ps`、`top`、`kill`

**预计完成时间**：7-10 天

---

### 8.2 用户空间库
- [ ] **libc 兼容层**
  - [ ] musl libc 移植
  - [ ] 标准库函数
- [ ] **动态链接器**
  - [ ] ELF 加载器
  - [ ] 动态链接
  - [ ] 符号解析

**预计完成时间**：14-21 天

---

## Phase 9: 优化与完善

### 9.1 性能优化
- [ ] **性能分析**
  - [ ] 火焰图生成
  - [ ] 热点分析
- [ ] **关键路径优化**
  - [ ] 调度器优化
  - [ ] 内存分配优化
  - [ ] 系统调用优化
- [ ] **并发优化**
  - [ ] 无锁算法
  - [ ] 批量处理
  - [ ] 中断合并

**预计完成时间**：持续进行**

---

### 9.2 稳定性提升
- [ ] **错误处理**
  - [ ] 错误恢复
  - [ ] 故障隔离
  - [ ] 内核转储
- [ ] **测试覆盖**
  - [ ] 单元测试
  - [ ] 集成测试
  - [ ] 压力测试
- [ ] **调试工具**
  - [ ] 内核调试器
  - [ ] 运行时跟踪
  - [ ] 性能剖析

**预计完成时间**：持续进行**

---

### 9.3 文档完善
- [ ] **用户文档**
  - [ ] 安装指南
  - [ ] 使用手册
  - [ ] 故障排查
- [ ] **开发者文档**
  - [ ] 架构设计
  - [ ] API 文档
  - [ ] 贡献指南
- [ ] **示例代码**
  - [ ] 驱动开发示例
  - [ ] 应用程序示例

**预计完成时间**：持续进行

---

## 进度追踪

### 完成度统计

| Phase | 描述 | 完成度 | 预计工作量 |
|-------|------|--------|-----------|
| Phase 1 | 基础框架 | ✅ 100% | 5 天 |
| Phase 2 | 中断与进程 | 🔄 75% | 8-12 天（MMU 问题阻塞）|
| Phase 3 | 系统调用与隔离 | ⏳ 0% | 6-8 天 |
| Phase 4 | 文件系统 | ⏳ 0% | 12-17 天 |
| Phase 5 | 网络与IPC | ⏳ 0% | 21-31 天 |
| Phase 6 | 多平台支持 | ⏳ 0% | 17-24 天 |
| Phase 7 | 设备驱动 | ⏳ 0% | 12-17 天 |
| Phase 8 | 用户空间 | ⏳ 0% | 21-31 天 |
| Phase 9 | 优化与完善 | ⏳ 0% | 持续 |

**总体进度**：约 15%（Phase 1 完成，Phase 2 进行中）

---

### 下一步行动

**当前重点**（Week 3）：
1. **解决 MMU 使能挂起问题**（最高优先级）
   - 使用 QEMU GDB 调试，检查：
     - TTBR0、TCR、MAIR_EL1 寄存器值
     - 页表内容验证
     - MMU 启用后第一次地址翻译
   - 参考 ARMv8 ARM DDI 0487 规范验证页表格式
   - 尝试不同的 T0SZ 值（0、16、24等）
   - 考虑参考其他在 QEMU virt 上成功启用 MMU 的项目

2. **备选方案**（如 MMU 问题持续）：
   - 暂时禁用 MMU，在 MMU 关闭状态下继续开发
   - 实现用户模式执行的其他途径
   - 先完成其他不依赖 MMU 的功能

**后续目标**（MMU 问题解决后）：
1. 验证用户代码执行（EL0 → EL1 返回）
2. 实现完整的系统调用路径测试
3. 实现第一个多进程示例

---

## 技术债务

### 需要重构的部分
- [ ] 移除 `mm.rs` 中的调试代码（直接 UART 写入）
- [ ] 统一错误处理机制
- [ ] 完善日志系统（debug_println! 和 println! 的统一）

### 已知问题

**🔴 阻塞性问题**：
- [ ] **MMU 使能后内核挂起** - 影响：无法启用用户模式执行
  - 页表地址：0x40020000
  - 页表条目值：0x40000701
  - 挂起位置：`msr sctlr_el1, x0` 指令后
  - 已尝试：修复页表格式、MAIR 配置、TLB 刷新、不同 TCR 值
  - 状态：待调试

**🟡 中等问题**：
- [ ] 用户代码执行在 MMU 关闭时会导致 SError
- [ ] 系统调用测试代码被禁用（main.rs:83-87）

**🟢 低优先级**：
- [ ] 栈空间大小需要根据实际需求调整
- [ ] 链接器脚本中的符号需要验证

---

## 参考资料

- [Linux 系统调用表](https://man7.org/linux/man-pages/man2/syscalls.2.html)
- [ARMv8 架构参考手册](https://developer.arm.com/documentation/ddi0487/latest)
- [GICv3 规范](https://developer.arm.com/documentation/ihi0069/latest)
- [OSDev Wiki](https://wiki.osdev.org/)

---

**文档版本**：v0.2.0
**最后更新**：2025-02-03
