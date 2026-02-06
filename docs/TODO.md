# Rux 开发路线图与TODO

## 项目概览

**当前状态**：Phase 10 完成 ✅ - RISC-V 64位架构支持

**最后更新**：2025-02-06

**默认平台**：RISC-V 64位（RV64GC）

## 平台测试状态

### 📊 测试覆盖范围

| 功能模块 | ARM64 (aarch64) | RISC-V64 | 备注 |
|---------|----------------|----------|------|
| **基础启动** | ✅ 已测试 | ✅ 已测试 | 两个平台都正常 |
| **异常处理** | ✅ 已测试 | ✅ 已测试 | trap handler 完整 |
| **UART 驱动** | ✅ 已测试 (PL011) | ✅ 已测试 (ns16550a) | 不同驱动 |
| **Timer Interrupt** | ✅ 已测试 (ARMv8 Timer) | ✅ 已测试 (SBI) | 不同实现 |
| **MMU/页表** | ✅ 已测试 (AArch64 4级页表) | ✅ 已测试 (Sv39 3级页表) | 不同架构 |
| **系统调用** | ✅ 已测试 | ⚠️ **未测试** | 框架已移植 |
| **进程调度** | ✅ 已测试 | ⚠️ **未测试** | 代码已共享 |
| **进程创建 (fork)** | ✅ 已测试 | ⚠️ **未测试** | 代码已共享 |
| **信号处理** | ✅ 已测试 | ⚠️ **未测试** | 代码已共享 |
| **文件系统 (VFS)** | ✅ 已测试 | ⚠️ **未测试** | 代码已共享 |
| **RootFS** | ✅ 已测试 | ⚠️ **未测试** | 代码已共享 |
| **Buddy System** | ✅ 已测试 | ⚠️ **未测试** | 代码已共享 |
| **ELF 加载器** | ✅ 已测试 | ⚠️ **未测试** | 代码已共享 |
| **SMP 多核** | ✅ 已测试 (PSCI+GIC) | ✅ 已测试 (SBI HSM) | 不同实现 |
| **IPI (核间中断)** | ✅ 已测试 (GIC SGI) | ❌ 未实现 | 需要 PLIC |
| **控制台同步** | ✅ 已测试 (spin::Mutex) | ✅ 已测试 (spin::Mutex) | 代码共享 |
| **Per-CPU 优化** | ✅ 已测试 | ⚠️ **未测试** | 代码已共享 |

### 🎯 RISC-V64 待测试功能

**Phase 2-9 核心功能**（代码已共享，需要在 RISC-V64 上验证）：

#### ⚠️ 高优先级（核心功能）
- ⏳ **系统调用验证** - 测试所有 28+ 系统调用
- ⏳ **进程调度测试** - 验证 Round Robin 调度
- ⏳ **fork 系统调用** - 创建新进程
- ⏳ **信号处理** - sigaction/kill/sigreturn

#### ⚠️ 中优先级（文件系统）
- ⏳ **VFS 框架** - 文件操作接口
- ⏳ **RootFS** - 内存文件系统
- ⏳ **文件描述符管理** - open/close/read/write

#### ⚠️ 低优先级（内存管理）
- ⏳ **Buddy System** - 内存分配/释放
- ⏳ **ELF 加载器** - 用户程序加载

### 📝 测试说明

**ARM64 测试完成的功能**：
- ✅ Phase 1-9 所有核心功能
- ✅ SMP 双核启动和调度
- ✅ GICv3 中断控制器
- ✅ IPI 核间中断
- ✅ 完整的系统调用接口
- ✅ 文件系统和 VFS

**RISC-V64 测试完成的功能**：
- ✅ Phase 10 基础架构
- ✅ 启动流程和 OpenSBI 集成
- ✅ 异常处理和 trap 机制
- ✅ Timer Interrupt
- ✅ Sv39 MMU 和页表管理

**注意**：大部分 Phase 2-9 的代码（系统调用、进程管理、文件系统等）是平台无关的，已经在 ARM64 上充分测试。RISC-V64 只需要验证这些功能在新的架构上能否正常工作。

**最新成就**：
- ✅ **RISC-V MMU 和页表支持** (2025-02-06)
  - RISC-V Sv39 虚拟内存管理实现
  - 3级页表结构（512 PTE/级）
  - 39位虚拟地址（512GB地址空间）
  - 4KB 页大小
  - 内核空间恒等映射（0x80200000+）
  - 设备内存映射（UART、CLINT）
  - satp CSR 管理（Sv39模式，MODE=8）
  - 页表映射：map_page()、map_region()
  - **MMU 已成功使能并运行**
- ✅ **RISC-V Timer Interrupt 支持** (2025-02-06)
  - SBI 0.2 TIMER extension (set_timer)
  - sie.STIE 中断使能
  - sstatus.SIE 全局中断使能
  - 周期性定时器中断（1 秒）
  - **关键修复**：stvec Direct 模式修复
    - 清除 stvec 最后两位确保 Direct 模式
    - 这是 Timer interrupt 不触发的根本原因
    - Vectored 模式会跳转到 stvec + 4 * cause
    - Direct 模式直接跳转到 stvec 地址
- ✅ **调试输出清理** (2025-02-06)
  - 移除 timer interrupt 详细输出
  - 移除 trap_handler 入口提示
  - 保留必要的初始化信息
  - 输出简洁清晰
- ✅ **测试脚本整理** (2025-02-06)
  - test_riscv.sh - 根目录快速测试
  - test/run_riscv.sh - RISC-V 运行脚本
  - test/debug_riscv.sh - GDB 调试脚本
  - test/all.sh - 全平台测试套件（riscv/aarch64/all）
- ✅ **RISC-V 64位架构支持** (2025-02-06)
  - 完整的启动流程（boot.rs）
  - S-mode 异常处理（trap.rs、trap.S）
  - 上下文切换（context.rs）
  - 系统调用处理（syscall.rs）
  - CPU 操作（cpu.rs）
  - UART 驱动（ns16550a）
  - 链接器脚本（linker.ld）
- ✅ **RISC-V SMP 多核支持** (2025-02-06)
  - SMP 框架实现（smp.rs）
  - Per-CPU 栈管理（每 CPU 16KB，总共 64KB）
  - 动态启动核检测（原子 CAS 操作）
  - SBI HSM 集成（hart_start）
  - 最多支持 4 个 CPU 核心
  - **所有 CPU 都能成功启动并运行**
- ✅ **控制台输出同步** (2025-02-06)
  - spin::Mutex 保护 UART 访问
  - 行级别锁（每次 println! 只获取一次锁）
  - SMP 环境下输出完整，无字符交叉
  - **多核同时输出不再混乱**
  - 链接器脚本（linker.ld）
  - **RISC-V 现在是默认构建目标**
- ✅ **S-mode CSR 正确使用** (2025-02-06)
  - mstatus → sstatus
  - mepc → sepc
  - mtval → stval
  - mtvec → stvec
  - mcause → scause
  - mret → sret
- ✅ **OpenSBI 集成** (2025-02-06)
  - 正确的内存布局（内核 0x80200000）
  - M-mode（OpenSBI）和 S-mode（内核）权限分离
  - 自动加载 OpenSBI firmware

**前期成就**：
- ✅ **Per-CPU 运行队列** (2025-02-04)
  - 全局 RQ 改为 per-CPU 数组（MAX_CPUS=4）
  - this_cpu_rq() / cpu_rq() 访问函数
  - init_per_cpu_rq() 初始化函数
  - 次核调度器自动初始化
- ✅ **启动顺序优化** (2025-02-04)
  - 参考 Linux ARM64 内核启动顺序
  - GIC 初始化提前到 scheduler/VFS 之前
  - 次核完善初始化（runqueue、栈、IRQ）
  - 创建 BOOT_SEQUENCE.md 详细文档
- ✅ **SMP 双核启动成功** (2025-02-04)
  - PSCI CPU 唤醒机制
  - Per-CPU 栈管理
  - 次核启动入口点
  - CPU 数量检测
  - 次核 runqueue 初始化
- ✅ **GICv3 中断控制器** (2025-02-04)
  - 最小初始化完成
  - 系统寄存器访问（ICC_IAR1_EL1, ICC_EOIR1_EL1）
  - Spurious interrupt 处理 (IRQ 1023)
  - 中断屏蔽/恢复机制
  - 正确的初始化顺序（MMU → GIC → SMP）
- ✅ **IPI (核间中断)** (2025-02-04)
  - 基于 SGI 的 IPI 发送（ICC_SGI1R_EL1）
  - IPI 处理框架
  - Reschedule/Stop IPI 类型
- ✅ **MMU 多级页表** (2025-02-04)
  - 3 级页表结构（4KB 页面）
  - MMU 成功启用
  - 恒等映射和用户空间映射
- ✅ **中断风暴修复** (2025-02-04)
  - IRQ 时序控制优化
  - 在 SMP 初始化完成后启用 IRQ
- ✅ **全面代码审查** (2025-02-04)
  - 发现并记录 15 个问题
  - 清理 50+ 处调试输出
  - 条件编译优化（#[cfg(debug_assertions)]）
- ✅ **测试脚本完善** (2025-02-04)
  - test_suite.sh - 完整测试套件
  - test_smp.sh - SMP 功能测试
  - test_ipi.sh - IPI 功能测试
  - test_qemu.sh - QEMU 配置测试
- ✅ 成功解决 alloc crate 符号可见性问题
- ✅ **Buddy System 内存分配器** (2025-02-04)
  - 完整的伙伴系统实现
  - 支持内存释放和伙伴合并
  - O(log n) 分配/释放复杂度
  - 最大支持 4GB 内存块
  - 基于 4KB 页面的分配
  - 线程安全（原子操作）
- ✅ 实现自定义集合类型 (SimpleBox/SimpleVec/SimpleString/SimpleArc)
- ✅ VFS 框架就绪并成功初始化
- ✅ RootFS 内存文件系统完整实现

**已知待修复问题**（详见 [CODE_REVIEW.md](CODE_REVIEW.md)）：

**🔴 严重优先级**：
- ~~**内存分配器无法释放内存**~~ - ✅ 已解决：Buddy System 实现完成
- ~~**全局单队列调度器**~~ - ✅ **已完成 (2025-02-04)**
  - ✅ Per-CPU 运行队列实现
  - ✅ 负载均衡机制实现
  - ✅ 任务迁移算法
  - ✅ 多核调度完成

**高优先级**：
- ~~⏳ SimpleArc Clone 支持~~ ✅ 已修复（2025-02-04）
- ~~⏳ RootFS write_data offset bug~~ ✅ 已修复（2025-02-04）

**中优先级**：
- ~~⏳ VFS 函数指针安全性~~ ✅ 已修复（2025-02-04）
- ~~⏳ Dentry/Inode 缓存机制~~ ✅ 已修复（2025-02-04）
- ⏳ Task 结构体过大（660+ bytes）

**低优先级**：
- ⏳ 命名约定不一致
- ⏳ IPI 测试代码清理
- ⏳ CpuContext 分离（内核/用户寄存器）
- ~~⏳ 路径解析完善（符号链接、相对路径）~~ ✅ **已完成 (2025-02-04)**
  - ~~✅ 路径规范化 (path_normalize)~~ 已完成（2025-02-04）
  - ~~✅ `.` 和 `..` 支持~~ 已完成（2025-02-04）
  - ~~✅ 符号链接解析~~ 已完成（2025-02-04）
  - ⏳ 相对路径完整支持（需要当前工作目录）
- ~~⏳ 文件系统操作完善~~ ✅ **已完成 (2025-02-04)**
  - ~~✅ mkdir() - 创建目录~~ 已完成
  - ~~✅ unlink() - 删除文件~~ 已完成
  - ~~✅ rmdir() - 删除目录~~ 已完成
  - ⏳ rename() - 重命名（部分实现，返回 ENOSYS）

---

## Phase 1: 基础框架 ✅ **已完成 (ARM64 测试)**

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

## Phase 3: 系统调用与隔离 🔄 **进行中 (2025-02-03, ARM64 测试)**

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

### 3.2 信号处理 ✅ **部分完成 (2025-02-04)**

- [x] **信号框架** (`kernel/src/signal.rs`)
  - [x] 信号定义（Linux 兼容，Signal 枚举）
  - [x] 信号掩码（SignalStruct.mask）
  - [x] 信号处理函数（SigAction）
  - [x] 待处理信号队列（SigPending）
  - [x] **SigInfo 结构** - 带附加信息的信号
  - [x] **SigQueue** - 信号队列（head/tail 指针）
  - [x] **sigqueue()** - 发送带 siginfo 的信号
  - [x] **sigprocmask()** - 信号掩码操作
  - [x] **rt_sigaction()** - 信号处理函数设置

- [x] **信号发送**
  - [x] `kill` 系统调用 (62)
  - [x] `sigaction` 系统调用 (48) - 使用 rt_sigaction
  - [x] `rt_sigprocmask` 系统调用 (14) - 使用 sigprocmask
  - [x] 信号队列（SigPending）
  - [x] 信号传递机制（SigPending::add()）
  - [x] **带 siginfo 的信号发送**（sigqueue）

- [x] **信号处理和交付**
  - [x] `rt_sigreturn` 系统调用 (15)
  - [x] **信号交付（do_signal）** ✅ 已完成 (2025-02-04)
  - [x] **信号处理函数调用** ✅ 已完成 (2025-02-04)
  - [x] **完整的 sigreturn 实现（上下文恢复）** ✅ 已完成 (2025-02-04)

**完成时间**：2025-02-04（信号交付机制完成）

**已实现**（2025-02-04）：
- ✅ **sigqueue()** - 发送带 siginfo 的信号
- ✅ **sigprocmask()** - 信号掩码操作
- ✅ **rt_sigaction()** - 信号处理函数设置
- ✅ **系统调用集成**
  - sys_sigaction 使用 rt_sigaction
  - sys_rt_sigprocmask 使用 sigprocmask
- ✅ **信号帧构建（setup_frame）** ✅ 已完成 (2025-02-04)
  - 保存上下文到 UContext
  - 设置信号处理函数参数
  - 设置信号返回桩代码（svc #0x80）
  - 保存原始 PC 用于返回
- ✅ **信号上下文恢复（restore_sigcontext）** ✅ 已完成 (2025-02-04)
  - 恢复所有保存的寄存器
  - 恢复 PC 到信号中断前的位置
  - 恢复信号掩码
  - 清除信号帧

**待实现**：
- ⏳ 信号栈（sigaltstack）- 已有框架，待完善
- ⏳ 信号帧写入用户空间 - 当前保存在内核空间

**预计完成时间**：基本完成，待用户空间内存管理完善

---

## Phase 4: 文件系统 🔄 **进行中 (2025-02-03, ARM64 测试)**

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
- [x] **挂载点管理** (`fs/mount.rs`)
  - [x] VfsMount 挂载点结构
  - [x] MntNamespace 挂载命名空间
  - [x] MntFlags/MsFlags 挂载标志
  - [x] add_mount/remove_mount 挂载点操作
  - [x] find_mount/list_mounts 挂载点查询
  - [x] MountTreeIter 挂载点遍历
  - [x] get_init_namespace 初始命名空间
- [x] **RootFS 内存文件系统** (`fs/rootfs.rs`) 🆕
  - [x] RootFSNode 文件节点（目录/常规文件）
  - [x] RootFSSuperBlock RootFS 超级块
  - [x] create_file 创建文件
  - [x] lookup 查找文件
  - [x] list_dir 列出目录
  - [x] read_data/write_data 文件读写
  - [x] ROOTFS_FS_TYPE 文件系统类型
  - [x] init_rootfs RootFS 初始化
  - [x] 根文件系统挂载到命名空间
  - [x] VfsMount 添加超级块指针 (mnt_sb)

**预计完成时间**：5-7 天

**当前状态**：VFS 核心框架已完整实现，包括文件描述符管理、路径解析、文件系统注册、超级块管理、挂载点管理和 RootFS 内存文件系统。下一步是完成根文件系统挂载和集成。

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

## Phase 5: SMP 支持 ✅ **已完成 (2025-02-04, ARM64 测试)**

### 5.1 双核启动 ✅ **已完成**
- [x] **次核启动入口点** (`arch/aarch64/boot.S`)
  - [x] 次核入口点 (secondary_entry)
  - [x] MPIDR 读取和 CPU ID 判断
  - [x] Per-CPU 栈选择
  - [x] 跳转到 Rust 次核入口
- [x] **SMP 数据结构** (`arch/aarch64/smp.rs`)
  - [x] SmpData 全局数据
  - [x] CpuBootInfo per-CPU 信息
  - [x] CPU 状态管理 (Unknown/Booting/Running)
  - [x] 活动 CPU 计数 (get_active_cpu_count)
- [x] **Per-CPU 栈管理**
  - [x] 链接器脚本分配栈空间
  - [x] 每个 CPU 16KB 栈
  - [x] 支持 4 个 CPU
- [x] **PSCI CPU 唤醒** (`arch/aarch64/smp.rs`)
  - [x] PSCI_CPU_ON SMC/HVC 调用
  - [x] 次核启动地址传递
  - [x] CPU 1 成功启动

**测试验证**：
```
[SMP: Calling PSCI for CPU 1]
[SMP: PSCI result = 0000000000000000]
[CPU1 up]
SMP: 2 CPUs online
```

### 5.2 GICv3 中断控制器 ✅ **已完成 (2025-02-04)**
- [x] **GICv3 完全初始化** (`drivers/intc/gicv3.rs`)
  - [x] **Bug 修复**: GICD 内存访问问题
    - **问题**: read_volatile() 访问 GICD 寄存器导致内核挂起
    - **原因**: Rust volatile 操作与 MMU 映射的设备内存交互问题
    - **修复**: GicD/GicR read_reg/write_reg 改用内联汇编 ldr/str
  - [x] GICD 完全初始化（检测到 32 IRQs）
  - [x] GICR 初始化
  - [x] ICC_IAR1_EL1 - 中断确认
  - [x] ICC_EOIR1_EL1 - 中断结束
  - [x] ICC_SGI1R_EL1 - SGI 发送
- [x] **中断处理** (`arch/aarch64/trap.rs`)
  - [x] IRQ 处理框架
  - [x] SGI 处理（IPI）
  - [x] Spurious interrupt 处理 (ID 1023)
- [x] **中断屏蔽/恢复**
  - [x] mask_irq() - 保存并屏蔽 IRQ
  - [x] restore_irq() - 恢复 IRQ 状态
  - [x] DAIF 寄存器操作

**测试验证**：
```
gicd: Step 1 - Reading GICD_CTLR (inline asm)...
gicd: CTLR = 0x00000000
gicd: Step 2 - Reading TYPER...
gicd: 32 IRQs detected
gicd: Step 3 - Enabling GICD...
gicd: GICD initialization complete!
gic: GICD initialized successfully
GIC initialized - IRQ still disabled
IRQ enabled
```

### 5.3 IPI (核间中断) ✅ **已完成 (2025-02-04)**
- [x] **IPI 框架** (`arch/aarch64/ipi.rs`)
  - [x] IpiType 枚举 (Reschedule/Stop)
  - [x] send_ipi() - 发送 IPI
  - [x] handle_ipi() - 处理 IPI
  - [x] smp_send_reschedule() - 发送调度 IPI
- [x] **IPI 测试**
  - [x] CPU 0 → CPU 1 IPI 发送测试
  - [x] IPI 接收验证

**测试验证**：
```
[IPI: Sending IPI 1 to CPU 1]
[IPI: CPU 1 received IPI 1]
[IPI: Reschedule]
```

### 5.4 MMU 多级页表 ✅ **已完成 (2025-02-04)**
- [x] **3 级页表结构** (`arch/aarch64/mm.rs`)
  - [x] 页表创建和初始化
  - [x] 页表项格式 (4KB 页面)
  - [x] 恒等映射 (0x4000_0000 - 0x7FFF_FFFF)
  - [x] 用户空间映射 (0x0000_0000 - 0x3FFF_FFFF)
- [x] **MMU 启用**
  - [x] TCR_EL1 配置
  - [x] MAIR_EL1 配置
  - [x] TTBR0_EL1 设置
  - [x] TLB 刷新
  - [x] MMU 成功启用

**技术要点**：
- T0SZ = 16 (48 位 VA)
- 3 级页表 (4KB 页粒度)
- Normal memory (AttrIdx 0)
- Device memory (AttrIdx 1)

### 5.5 中断风暴修复 ✅ **已完成 (2025-02-04)**
- [x] **IRQ 时序控制**
  - [x] 启动时禁用 IRQ (boot.rs)
  - [x] GIC 初始化（IRQ 仍然禁用）
  - [x] SMP 启动（IRQ 仍然禁用）
  - [x] SMP 完成后启用 IRQ
- [x] **中断处理优化**
  - [x] handle_irq() 中使用中断屏蔽
  - [x] 防止递归中断调用

---

## Phase 6: 代码审查 ✅ **已完成 (2025-02-04, ARM64 测试)**

### 6.1 全面代码审查 ✅ **已完成**
- [x] **代码审查** - 整个项目审查
  - [x] 发现并记录 15 个问题
  - [x] 按严重程度分类（严重/中等/低）
  - [x] 与 Linux 内核对比
  - [x] 修复方案设计
- [x] **文档更新**
  - [x] CODE_REVIEW.md 创建
  - [x] 问题详细记录
  - [x] 修复优先级排序
  - [x] BOOT_SEQUENCE.md 创建 - 启动顺序分析与优化文档
  - [x] Linux ARM64 启动顺序参考
  - [x] 关键原则文档（MMU → GIC → SMP）
  - [x] 次核初始化详细步骤

### 6.2 调试输出清理 ✅ **已完成**
- [x] **清理 50+ 处调试输出**
  - [x] boot.rs - 2 处
  - [x] gicv3.rs - 17 处
  - [x] ipi.rs - 8 处
  - [x] allocator.rs - 1 处
  - [x] main.rs - 20+ 处
- [x] **条件编译优化**
  - [x] 使用 #[cfg(debug_assertions)]
  - [x] 区分调试和生产输出
  - [x] 使用 println!/debug_println! 宏

### 6.3 测试脚本完善 ✅ **已完成**
- [x] **test_suite.sh** - 完整测试套件
  - [x] 配置文件测试
  - [x] 编译测试
  - [x] SMP 测试
  - [x] MMU 测试
  - [x] 内存配置测试
- [x] **test_smp.sh** - SMP 功能测试
  - [x] 双核启动验证
  - [x] MMU/GIC 初始化验证
  - [x] 系统稳定性验证
- [x] **test_ipi.sh** - IPI 功能测试
  - [x] 双核启动测试
  - [x] GIC 初始化测试
  - [x] IRQ 控制测试
  - [x] 系统稳定性测试
- [x] **test_qemu.sh** - QEMU 配置测试
  - [x] 单核测试
  - [x] 双核测试
  - [x] 四核测试
  - [x] 内存配置测试

### 6.4 Makefile 增强 ✅ **已完成**
- [x] **快捷命令**
  - [x] `make smp` - 运行 SMP 测试
  - [x] `make ipi` - 运行 IPI 测试
  - [x] `make test` - 运行测试套件
  - [x] `make run` - 快速运行内核

---

## Phase 7: 内存管理优化 ✅ **已完成 (2025-02-04, ARM64 测试)**

### 7.1 Buddy System 内存分配器 ✅ **已完成**
- [x] **Buddy System 实现** ([kernel/src/mm/buddy_allocator.rs](kernel/src/mm/buddy_allocator.rs))
  - [x] BlockHeader 数据结构（order, free, prev, next）
  - [x] BuddyAllocator 结构体（21 个空闲链表）
  - [x] alloc_blocks() - O(log n) 分配，支持块分割
  - [x] free_blocks() - 释放算法，支持伙伴合并
  - [x] get_buddy() - 伙伴地址计算
  - [x] GlobalAlloc trait 实现
- [x] **内存布局调整**
  - [x] 堆地址：0x6000_0000（从 0x8800_0000 迁移）
  - [x] MMU 页表映射（L2 entries 3-10，16MB）
- [x] **线程安全**
  - [x] 原子操作（AtomicUsize, CAS）
  - [x] 无锁并发访问

**技术特性**：
- 最小分配：4KB (order 0)
- 最大分配：4GB (order 20)
- 堆大小：16MB
- 算法复杂度：O(log n)

**测试验证**：
```
✓ Direct alloc works!
✓ SimpleVec::push works!
✓ SimpleBox works!
✓ SimpleString works!
✓ SimpleArc works!
✓ Fork success: child PID = 2
```

---

## Phase 8: Per-CPU 优化 ✅ **已完成 (2025-02-04, ARM64 测试)**

### 8.1 Per-CPU 运行队列 ✅ **已完成**
- [x] **调度器重构** ([kernel/src/process/sched.rs](kernel/src/process/sched.rs))
  - [x] 全局 RQ 改为 per-CPU 数组（PER_CPU_RQ[4]）
  - [x] this_cpu_rq() 函数 - 获取当前 CPU 的运行队列
  - [x] cpu_rq(cpu_id) 函数 - 获取指定 CPU 的运行队列
  - [x] init_per_cpu_rq(cpu_id) 函数 - 初始化 per-CPU 队列
  - [x] Per-CPU 任务队列
  - [x] 次核调度器初始化（在 secondary_cpu_start 中调用）
  - [x] schedule() 使用 this_cpu_rq()
  - [x] **负载均衡** ✅ 已完成 (2025-02-04)
  - [x] 负载检测机制 - rq_load() 函数
  - [x] 任务迁移（steal task）- steal_task() 函数
  - [x] 负载均衡算法 - load_balance() 函数
  - [x] 集成到 schedule() 调度器

**实施步骤**：
1. ✅ 修改 RunQueue 数据结构 - 完成
2. ✅ 实现 this_cpu_rq() 访问函数 - 完成
3. ✅ 在 SMP 初始化时调用 init_per_cpu_rq() - 完成
4. ✅ 更新 schedule() 函数使用 this_cpu_rq() - 完成
5. ✅ 实现基础负载均衡 - 完成

**完成时间**：2025-02-04
**难度**：⭐⭐⭐⭐ (高)
**优先级**：🔴 严重（SMP 性能瓶颈）

**验证状态**：
```
sched: Initializing CPU 0 runqueue
sched: CPU 0 runqueue [OK]
[CPU1 up]
[CPU1] init: runqueue
sched: Initializing CPU 1 runqueue
sched: CPU 1 runqueue [OK]
[CPU1] init: IRQ enabled
[CPU1] idle: waiting for work
SMP: 2 CPUs online
```

### 8.2 快速胜利 ✅ **已完成 (2025-02-04)**

#### Quick Win 1: SimpleArc Clone 支持 ✅ **已完成**
**问题**：导致多个文件系统操作返回 `None`
**位置**：[kernel/src/collection.rs:390](kernel/src/collection.rs:390)
**实施**：Clone trait 已在 collection.rs 实现
**修复**：
- RootFSNode::find_child() - 移除 TODO，使用 child.clone()
- RootFSNode::list_children() - 实现正确的子节点克隆
**验证**：✅ 编译通过

#### Quick Win 2: RootFS write_data offset bug ✅ **已完成**
**问题**：文件写入忽略 offset 参数
**位置**：[kernel/src/fs/rootfs.rs:185](kernel/src/fs/rootfs.rs:185)
**修复**：
```rust
pub fn write_data(&mut self, offset: usize, data: &[u8]) -> usize {
    if let Some(ref mut existing_data) = self.data {
        let required_size = offset + data.len();
        if existing_data.len() < required_size {
            existing_data.resize(required_size, 0);
        }
        existing_data[offset..offset + data.len()].copy_from_slice(data);
        data.len()
    } else {
        0
    }
}
```
**验证**：✅ 编译通过

#### Quick Win 3: Dentry/Inode 缓存机制 ✅ **已完成 (2025-02-04)**
**问题**：文件系统缺少缓存导致性能低下
**位置**：
- [kernel/src/fs/dentry.rs](kernel/src/fs/dentry.rs) - Dentry 缓存
- [kernel/src/fs/inode.rs](kernel/src/fs/inode.rs) - Inode 缓存
- [kernel/src/fs/rootfs.rs](kernel/src/fs/rootfs.rs) - RootFS 路径缓存

**实施**：
- Dentry 缓存 (dcache): 256-bucket 哈希表，FNV-1a 哈希
- Inode 缓存 (icache): 256-bucket 哈希表，FNV-1a 哈希
- RootFS 路径缓存: RootFS 专用路径缓存
- 缓存统计功能（命中/未命中计数）

**验证**：✅ 编译通过，内核运行正常

**完成时间**：2025-02-04
**难度**：⭐⭐⭐ (中)
**优先级**：🟡 高（影响性能）

#### Quick Win 4: 路径规范化 ✅ **已完成 (2025-02-04)**
**问题**：路径解析缺少规范化，不支持 `.` 和 `..` 特殊目录
**位置**：
- [kernel/src/fs/path.rs](kernel/src/fs/path.rs) - 路径解析模块
- [kernel/src/fs/rootfs.rs](kernel/src/fs/rootfs.rs) - RootFS 查找集成

**实施**：
- path_normalize() - 路径规范化函数
  - 移除多余的 `/`
  - 处理 `.` (当前目录)
  - 处理 `..` (父目录)
  - 支持绝对路径和相对路径
- RootFS::lookup() 集成路径规范化
- 完整的单元测试覆盖

**验证**：✅ 编译通过，内核运行正常

**完成时间**：2025-02-04
**难度**：⭐⭐ (低)
**优先级**：🟡 高（影响正确性）

---

## Phase 9: 网络与高级功能 ⏳

---

## Phase 8: 网络与高级功能

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
| Phase 2 | 中断与进程 | ✅ 90% | 10 天 |
| Phase 3 | 系统调用与隔离 | ✅ 80% | 8 天 |
| Phase 4 | 文件系统 | ✅ 75% | 12 天 |
| Phase 5 | SMP 支持 | ✅ 100% | 7 天 |
| Phase 6 | 代码审查 | ✅ 100% | 3 天 |
| Phase 7 | 内存管理优化 | ✅ 100% | 1 天 |
| Phase 8 | Per-CPU 优化 | ✅ 80% | 1 天 |
| Phase 9 | 快速胜利 | ⏳ 0% | 1.5-3 天 |
| Phase 10 | 网络与IPC | ⏳ 0% | 21-31 天 |
| Phase 11 | 多平台支持 | ⏳ 0% | 17-24 天 |
| Phase 12 | 设备驱动 | ⏳ 0% | 12-17 天 |
| Phase 13 | 用户空间 | ⏳ 0% | 21-31 天 |
| Phase 14 | 优化与完善 | ⏳ 0% | 持续 |

**总体进度**：约 40%（Phase 1-8 基本完成，Phase 9 快速胜利待实现）

---

### 下一步行动

**当前重点**（Week 4-5，按优先级排序）：

#### ✅ P0 - 严重优先级（已完成核心功能）

**~~1. Per-CPU 运行队列~~** ✅ **已完成 (2025-02-04)**
- ✅ 创建 feature 分支：`git checkout -b feat/per-cpu-runqueue`
- ✅ 修改数据结构：全局 RQ → per-CPU 数组
- ✅ 实现 `this_cpu_rq()` 函数
- ✅ 实现 `cpu_rq(cpu_id)` 函数
- ✅ 实现 `init_per_cpu_rq(cpu_id)` 函数
- ✅ 次核调度器初始化（在 `secondary_cpu_start` 中调用）
- ✅ 更新 `schedule()` 使用 `this_cpu_rq()`
- ✅ 负载均衡（任务窃取） - 已完成 (2025-02-04)
  - rq_load() - 负载检测
  - find_busiest_cpu() - 查找繁忙 CPU
  - steal_task() - 任务迁移
  - load_balance() - 主函数
- ✅ 验证：双核独立调度，无死锁，负载均衡工作

**提交记录**：
- commit b687710: "优化启动顺序：GIC 提前，次核初始化完善"
- commit 257dd99: "feat: 实现负载均衡机制 (Load Balancing)"

**待完成优化**（Phase 10）：
- Dentry/Inode 缓存
- 网络协议栈
- IPC 机制

---

#### 🔴 P0 - 高优先级（影响正确性）

**~~2. SimpleArc Clone 支持~~** ✅ **已完成 (2025-02-04)**
- [x] Clone trait 已在 `collection.rs` 实现
- [x] 修复 RootFS 文件系统操作
- [x] 验证：编译通过

**~~3. RootFS write_data offset bug~~** ✅ **已完成 (2025-02-04)**
- [x] 修复 `write_data()` 函数
- [x] 支持从 offset 开始写入
- [x] 验证：编译通过

---

#### 🟢 P2 - 中优先级（优化和安全）

~~**4. VFS 函数指针安全性**（2-3 天）~~ ✅ **已完成 (2025-02-04)**
- [x] 使用引用和切片替代裸指针
- [x] FileOps 和 INodeOps 改进
- [x] 更新所有实现（reg、pipe、uart）

**5. Dentry/Inode 缓存**（2-3 天）
- [ ] 实现哈希表缓存
- [ ] LRU 淘汰策略

**6. Task 结构体优化**（1-2 天）
- [ ] CpuContext 分离
- [ ] 使用 Box 包装大型字段
- [ ] 优化字段布局

---

#### ⚡ 快速胜利（1 天内完成）

如果时间有限，建议按此顺序：
1. **RootFS write_data bug**（30分钟）✅ 已完成
2. **SimpleArc Clone**（1-2小时）✅ 已完成
3. **ELF 加载器完善**（2-3小时）✅ 已完成 (2025-02-04)
   - ✅ PT_LOAD 段加载实现
   - ✅ BSS 段清零（p_memsz > p_filesz）
   - ✅ 动态链接器路径提取（PT_INTERP）
   - ✅ execve 系统调用与文件系统集成
   - ✅ ElfLoadInfo 结构（加载信息）

---

**后续目标**（Phase 8-9 完成后）：
1. 进程调度算法优化（CFS）
2. 完善信号处理机制
3. 完整的 IPC 实现

---

## 技术债务

### 需要重构的部分
- ~~[ ] 实现真正的内存分配器~~ ✅ 已完成 - Buddy System 实现完成
- ~~[ ] 实现Per-CPU 运行队列~~ ✅ 已完成 - Per-CPU 数据结构完成
  - ✅ per-CPU 运行队列数组（PER_CPU_RQ[4]）
  - ✅ this_cpu_rq() / cpu_rq() 访问函数
  - ✅ 次核自动初始化
  - ⏳ 负载均衡机制（Phase 9）
- [ ] 统一错误处理机制
- [ ] 完善日志系统

### 已知问题

**🔴 严重问题**（待修复）：
- ~~[ ] **内存分配器无法释放~~ ✅ 已解决 - Buddy System 完整实现
  - ~~当前使用 bump allocator~~ 已替换为 Buddy System
  - ~~dealloc() 是空实现~~ 已支持伙伴合并
  - ~~需要：Buddy System / Slab Allocator~~ ✅ Buddy System 已实现
- ~~[ ] **全局单队列调度器~~ ✅ **已完成 (2025-02-04)** - Per-CPU 运行队列 + 负载均衡
  - ✅ 所有 CPU 有独立的运行队列
  - ✅ this_cpu_rq() / cpu_rq() 访问函数
  - ✅ 次核自动初始化
  - ✅ 负载均衡机制实现
  - ✅ 任务迁移算法
  - **状态**：✅ 完全完成（Phase 9），SMP 调度功能完整

**🟡 中等问题**：
- [x] ~~SimpleArc Clone 支持~~ ✅ 已完成（2025-02-04）
- [x] ~~RootFS write_data offset bug~~ ✅ 已完成（2025-02-04）
- [x] ~~VFS 函数指针安全性~~ ✅ 已完成（2025-02-04）
  - 使用引用和切片替代裸指针
- [ ] Dentry/Inode 缓存 - 性能问题
  - **计划**：Phase 10 中优先级

**🟢 低优先级**：
- [ ] Task 结构体过大（660+ bytes）
- [ ] 命名约定不一致
- [ ] IPI 测试代码清理
- [ ] CpuContext 分离（内核/用户寄存器）

### 已解决问题 ✅
- [x] **MMU 使能问题** - 已解决 ✅
  - 使用 3 级页表结构
  - 正确配置 TCR_EL1、MAIR_EL1
  - MMU 已成功启用
- [x] **GIC 初始化挂起** - 已解决 ✅
  - GICD 内存访问问题修复（2025-02-04）
  - read_volatile() → 内联汇编 ldr/str
  - GICD/GICR 完全初始化（32 IRQs）
  - 使用内联汇编替代 Rust volatile 操作
- [x] **中断风暴** - 已解决 ✅
  - 优化 IRQ 启用时序
  - 在 SMP 初始化完成后启用
- [x] **Buddy System 内存分配器** - 已解决 ✅ (2025-02-04)
  - 完整实现伙伴系统算法
  - 支持 O(log n) 分配/释放
  - 伙伴合并机制减少碎片
  - 线程安全（原子操作）
  - 测试验证通过
- [x] **ELF 加载器基础** - 已解决 ✅ (2025-02-04)
  - PT_LOAD 段加载到内存
  - BSS 段清零处理
  - 动态链接器路径提取（PT_INTERP）
  - execve 与文件系统集成
  - 参考 Linux fs/binfmt_elf.c
  - **限制**：地址空间管理待完善（Phase 13）
- [x] **地址空间管理基础** - 已解决 ✅ (2025-02-04)
  - pagemap::AddressSpace 扩展 mmap/munmap/brk/allocate_stack
  - VMA 管理器集成（VmaManager）
  - mmap 系统调用实现
  - munmap 系统调用实现
  - brk 系统调用实现
  - 用户栈分配（allocate_stack）
  - 参考 Linux mm/mmap.c 和 mm/mm_types.h
  - **限制**：完整 PGD 初始化待实现（Phase 13）

---

## Phase 10: RISC-V 64位架构 ✅ **已完成** (2025-02-06)

### 目标
实现完整的 RISC-V 64位架构支持，并将其设置为默认构建目标。

### 完成状态 ✅

#### 核心功能实现 ✅
- [x] **启动流程** (boot.rs)
  - [x] 栈指针设置（0x801F_C000，16KB 栈）
  - [x] BSS 段清零
  - [x] trap 向量设置（stvec）
  - [x] main() 函数调用
  - [x] OpenSBI 集成

- [x] **异常处理** (trap.rs + trap.S)
  - [x] global_asm trap_entry（汇编入口）
  - [x] 寄存器保存/恢复（x1, x5-x31）
  - [x] S-mode CSR 保存（sstatus, sepc, stval）
  - [x] trap_handler Rust 函数
  - [x] 异常分发（scause 解析）
  - [x] sret 返回指令

- [x] **上下文切换** (context.rs)
  - [x] TaskContext 结构体
  - [x] cpu_switch_to 汇编实现
  - [x] 通用寄存器切换
  - [x] SP/RA 寄存器切换

- [x] **CPU 操作** (cpu.rs)
  - [x] get_core_id() - mhartid 读取
  - [x] enable_irq() - sie 中断使能
  - [x] disable_irq() - sie 中断禁用
  - [x] wfi() - Wait For Interrupt
  - [x] read_counter() - time CSR 读取
  - [x] get_counter_freq() - 10MHz (QEMU virt)

- [x] **系统调用** (syscall.rs)
  - [x] syscall_handler 函数
  - [x] 系统调用号解析（a7 寄存器）
  - [x] 系统调用分发
  - [x] 返回值设置（a0 寄存器）

- [x] **UART 驱动** (console.rs)
  - [x] RISC-V UART 基址（0x10000000 - ns16550a）
  - [x] UART 初始化
  - [x] putc/getc 函数
  - [x] 平台条件编译

- [x] **链接器脚本** (linker.ld)
  - [x] 内存布局定义（0x80200000）
  - [x] 避开 OpenSBI 区域（0x80000000-0x8001ffff）
  - [x] .text 段（代码）
  - [x] .data 段（数据）
  - [x] .bss 段（未初始化数据）
  - [x] 栈空间分配（16KB）

- [x] **运行脚本** (test/run_riscv64.sh)
  - [x] QEMU 命令构建
  - [x] OpenSBI 自动加载
  - [x] 单核/多核模式支持
  - [x] 内核二进制检查

- [x] **Timer Interrupt** 🆕 (2025-02-06)
  - [x] SBI 0.2 TIMER extension (set_timer)
  - [x] sie.STIE 中断使能
  - [x] sstatus.SIE 全局中断使能（使用内联汇编）
  - [x] 周期性定时器中断（1 秒）
  - [x] **关键修复**：stvec Direct 模式修复
    - [x] 清除 stvec 最后两位确保 Direct 模式
    - [x] 修复 Timer interrupt 不触发的问题
    - [x] Vectored 模式跳转到 stvec + 4 * cause
    - [x] Direct 模式跳转到 stvec 地址

- [x] **调试输出清理** 🆕 (2025-02-06)
  - [x] 移除 timer interrupt 详细输出
  - [x] 移除 trap_handler 入口提示
  - [x] 保留必要的初始化信息
  - [x] 输出简洁清晰

- [x] **测试脚本整理** 🆕 (2025-02-06)
  - [x] test_riscv.sh - 根目录快速测试
  - [x] test/run_riscv.sh - RISC-V 运行脚本
  - [x] test/debug_riscv.sh - GDB 调试脚本
  - [x] test/all.sh - 全平台测试套件（riscv/aarch64/all）

#### 关键修复 ✅
- [x] **M-mode → S-mode CSR 转换**
  - [x] mstatus → sstatus
  - [x] mepc → sepc
  - [x] mtval → stval
  - [x] mtvec → stvec
  - [x] mcause → scause
  - [x] mret → sret

- [x] **内存布局优化**
  - [x] 内核加载地址：0x80200000
  - [x] 栈指针：0x801F_C000
  - [x] 避开 OpenSBI：0x80000000-0x8001ffff

- [x] **配置文件更新**
  - [x] kernel/Cargo.toml - default = ["riscv64"]
  - [x] .cargo/config.toml - target = "riscv64gc-unknown-none-elf"
  - [x] riscv64gc-unknown-none-elf target 定义

#### 测试验证 ✅ (RISC-V64)
```
OpenSBI v0.9
...
Domain0 Next Mode: S-mode
...
Rux OS v0.1.0 - RISC-V 64-bit
trap: Initializing RISC-V trap handling...
trap: Exception vector table installed at stvec = 0x8020002c
trap: RISC-V trap handling [OK]
mm: Initializing RISC-V MMU (Sv39)...
mm: Current satp = 0x0 (MODE=0)
mm: Root page table at PPN = 0x80208
mm: Page table mappings created
mm: Enabling MMU (Sv39)...
mm: satp = 0x8000000000080208 (MODE=8, PPN=0x80208)
mm: MMU enabled successfully
mm: RISC-V MMU [OK]
[OK] Timer interrupt enabled, system ready.
```

**注意**：Phase 10 所有功能均在 RISC-V64 平台上测试通过。

#### 文档更新 ✅
- [x] README.md - RISC-V 说明
- [x] TODO.md - Phase 10 完成
- [x] 项目结构更新
- [x] 快速开始指南更新

### 技术突破

1. **成功迁移到 RISC-V 架构**
   - 从 ARM aarch64 切换到 RISC-V 64位
   - 保持相同的内核接口和功能

2. **正确处理权限分离**
   - OpenSBI 运行在 M-mode
   - 内核运行在 S-mode
   - 正确使用 S-mode CSR

3. **内存布局优化**
   - 避开 OpenSBI 固件区域
   - 正确的内核加载地址
   - 有效的栈空间管理

4. **完整的异常处理**
   - S-mode 异常向量
   - CSR 寄存器正确访问
   - 异常分发和处理

### 已知限制

- **RISC-V 特定**：
  - ⏳ PLIC (Platform-Level Interrupt Controller) 待实现
  - ⏳ CLINT (Core-Local Interrupt Controller) 待实现（使用 SBI 替代）
  - ⏳ SMP 多核支持待实现
  - ✅ Timer interrupt 已完成 (2025-02-06)
  - ✅ MMU (Sv39) 已完成 (2025-02-06)
  - ⏳ **Phase 2-9 功能待在 RISC-V64 上验证**
    - 虽然代码已共享并在 ARM64 上充分测试
    - 但需要在 RISC-V64 平台上验证运行

- **通用**：
  - ⏳ 用户空间程序加载待完善
  - ⏳ 文件系统功能扩展
  - ⏳ 网络协议栈实现

### 参考资料

- [RISC-V 特权架构规范](https://riscv.org/technical/specifications/)
- [RISC-V 指令集手册](https://riscv.org/technical/specifications/)
- [OpenSBI 文档](https://github.com/riscv/opensbi/blob/master/docs/)
- [QEMU RISC-V virt 平台](https://www.qemu.org/docs/master/system/riscv/virt.html)

---

## 参考资料

- [Linux 系统调用表](https://man7.org/linux/man-pages/man2/syscalls.2.html)
- [ARMv8 架构参考手册](https://developer.arm.com/documentation/ddi0487/latest)
- [GICv3 规范](https://developer.arm.com/documentation/ihi0069/latest)
- [OSDev Wiki](https://wiki.osdev.org/)

---

**文档版本**：v0.4.0
**最后更新**：2025-02-06
