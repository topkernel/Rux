# Rux 内核项目 - AI 助手指南

本文档为 Claude Code 等 AI 助手提供项目上下文和开发指南。

## ⚠️ 最高原则（绝对不可违反）

### **POSIX/ABI 完全兼容，绝不创新**

这是 Rux 内核开发的**最高指导原则**，所有设计和实现决策都必须服从于此原则。

**核心要求**：
- **100% POSIX 兼容**：完全遵守 POSIX 标准
- **Linux ABI 完全兼容**：与 Linux 内核 ABI 二进制兼容
- **系统调用兼容**：使用 Linux 的系统调用号
- **文件系统兼容**：支持 Linux 文件系统（ext4、btrfs）
- **ELF 格式兼容**：可执行文件格式与 Linux 一致

**严格禁止**：
- ❌ 绝不"优化" Linux 的设计
- ❌ 绝不创造新的系统调用
- ❌ 绝不改变现有接口的行为
- ❌ 绝不"重新发明轮子"
- ❌ 绝不为了"更优雅"而偏离标准

**实现方式**：
1. 直接参考 Linux 内核源码
2. 使用相同的系统调用号（`arch/x86/entry/syscalls`）
3. 使用相同的结构体布局
4. 使用相同的文件系统格式
5. 遵循 POSIX 标准

> **关键**：我们的目标是用 Rust 重写 Linux 内核，而不是创造新系统。任何偏离 Linux 标准的"创新"都是错误的。

**参考资源**：
- Linux 内核源码：https://elixir.bootlin.com/linux/latest/source/
- Linux man pages（`man 2 syscall`）
- Linux 内核文档：Documentation/
- POSIX 标准：https://pubs.opengroup.org/onlinepubs/9699919799/

---

## 项目概述

**Rux** 是一个完全用 Rust 编写的类 Linux 操作系统内核，支持多平台（aarch64、x86_64、riscv64）。

### 核心特征
- **语言**: Rust（no_std，除必要的平台汇编）
- **架构**: riscv64（默认）、aarch64、x86_64
- **目标**: Linux 兼容的操作系统内核
- **阶段**: Phase 10 完成（RISC-V 架构支持）

### 技术栈
- **构建**: Cargo + 自定义 build.rs
- **配置**: TOML + 交互式 menuconfig
- **测试**: QEMU 模拟 + GDB 调试
- **文档**: Markdown + 代码注释

## 项目结构

```
Rux/
├── kernel/                 # 内核核心代码
│   ├── src/
│   │   ├── main.rs       # 内核入口点
│   │   ├── console.rs    # UART 控制台驱动
│   │   ├── config.rs     # 自动生成的配置
│   │   ├── print.rs      # 打印宏
│   │   ├── signal.rs     # 信号处理
│   │   ├── arch/         # 架构相关代码
│   │   │   ├── mod.rs
│   │   │   └── aarch64/  # ARM64 实现
│   │   │       ├── mod.rs
│   │   │       ├── boot.S        # 启动汇编
│   │   │       ├── boot.rs       # 初始化
│   │   │       ├── cpu.rs        # CPU 特性检测
│   │   │       ├── context.rs    # 上下文切换
│   │   │       ├── trap.S        # 异常向量表
│   │   │       ├── trap.rs       # 异常处理
│   │   │       ├── syscall.rs    # 系统调用处理
│   │   │       ├── user_program.S # 测试用户程序
│   │   │       └── mm/           # 架构相关内存管理
│   │   ├── drivers/      # 设备驱动
│   │   │   ├── mod.rs
│   │   │   ├── intc/         # 中断控制器
│   │   │   │   ├── mod.rs
│   │   │   │   └── gicv3.rs    # GICv3 驱动
│   │   │   └── timer/        # 定时器驱动
│   │   │       ├── mod.rs
│   │   │       └── armv8.rs   # ARMv8 架构定时器
│   │   ├── mm/           # 内存管理
│   │   │   ├── mod.rs
│   │   │   ├── allocator.rs  # 堆分配器
│   │   │   ├── page.rs       # 页帧管理
│   │   │   ├── pagemap.rs    # 页表管理
│   │   │   └── vma.rs        # 虚拟内存区域
│   │   ├── fs/            # 文件系统
│   │   │   ├── mod.rs
│   │   │   ├── vfs.rs        # 虚拟文件系统
│   │   │   ├── file.rs       # 文件描述符
│   │   │   ├── inode.rs      # inode 管理
│   │   │   ├── dentry.rs     # 目录项缓存
│   │   │   ├── buffer.rs     # 块缓存
│   │   │   ├── pipe.rs       # 管道实现
│   │   │   ├── elf.rs        # ELF 加载器
│   │   │   └── char_dev.rs   # 字符设备
│   │   └── process/       # 进程管理
│   │       ├── mod.rs
│   │       ├── task.rs       # 任务控制块
│   │       ├── sched.rs      # 调度器
│   │       ├── pid.rs        # PID 分配器
│   │       ├── usermod.rs    # 用户模式管理
│   │       └── test.rs       # 测试代码
│   ├── build.rs          # 生成 config.rs
│   └── Cargo.toml
│
├── build/                 # 构建工具
│   └── Makefile          # 构建脚本
│
├── test/                  # 测试脚本
│   ├── test_qemu.sh      # QEMU 测试
│   ├── run.sh            # 快速运行
│   └── debug.sh          # GDB 调试
│
├── docs/                  # 项目文档
│   ├── CONFIG.md         # 配置系统文档
│   ├── DESIGN.md         # 设计原则
│   ├── TODO.md           # 任务列表
│   ├── CODE_REVIEW.md    # 代码审查记录
│   ├── STRUCTURE.md      # 目录结构
│   └── QUICKREF.md       # 快速参考
│
├── Kernel.toml           # 内核配置文件
├── Cargo.toml            # 工作空间配置
├── Makefile              # 根 Makefile（快捷方式）
├── CLAUDE.md             # AI 助手开发指南
└── README.md             # 项目说明
```

## 关键文件说明

### 配置文件
- **Kernel.toml** - 内核主配置（编译时读取）
- **Cargo.toml** - Rust 工作空间配置
- **.cargo/config.toml** - Cargo 工具链配置

### 自动生成
- **kernel/src/config.rs** - 由 build.rs 根据 Kernel.toml 自动生成，不要手动编辑

### 重要脚本
- **build/Makefile** - 详细构建命令
- **test/run.sh** - 快速运行内核
- **test/test_qemu.sh** - GDB 调试脚本
- **test/debug.sh** - 详细调试脚本

### 汇编文件
- **kernel/src/arch/riscv64/trap.S** - 异常向量表（RISC-V，global_asm）
- **kernel/src/arch/aarch64/boot.S** - 启动代码、EL检测
- **kernel/src/arch/aarch64/trap.S** - 异常向量表（16个入口，2KB对齐）
- **kernel/src/arch/aarch64/user_program.S** - 测试用户程序代码

## 当前实现状态

### ✅ 已完成（Phase 1-17）

#### Phase 17 (2025-02-09) - RISC-V 系统调用和用户程序支持 ✅ **NEW**
1. **Trap 处理完整实现**（trap.S + trap.rs）
   - 汇编语言 trap 入口/出口代码
   - 272 字节 TrapFrame 上下文保存
   - sscratch 寄存器管理（支持连续系统调用）
2. **用户模式切换**（usermode_asm.S）
   - Linux 风格单一页表方法
   - sret 指令切换到 U-mode
   - sstatus.SPP=0 确保返回用户模式
3. **系统调用实现**（syscall.rs）
   - ✅ sys_exit (93) - 进程退出
   - ✅ sys_getpid (172) - 获取进程 ID
   - ✅ sys_getppid (110) - 获取父进程 ID
4. **用户程序工具链**
   - no_std 用户程序示例（hello_world）
   - 自定义链接器脚本（user.ld）
   - 嵌入式 ELF 加载器
5. **测试验证**：用户程序成功调用系统调用并正常终止

#### Phase 10 (2025-02-06) - RISC-V 64位架构 ✅
1. **RISC-V 启动框架**（boot.rs）
2. **异常向量表**（trap.S global_asm）
3. **异常处理框架**（trap.rs - S-mode CSR）
4. **UART 控制台驱动**（ns16550a）
5. **上下文切换**（context.rs）
6. **系统调用处理**（syscall.rs）
7. **CPU 操作**（cpu.rs）
8. **链接器脚本**（linker.ld）
9. **RISC-V 现在是默认平台**

#### Phase 1-9 (ARM aarch64)
1. 启动框架（boot.S）
2. 异常向量表（trap.S）
3. 异常处理框架（trap.rs）
4. UART 控制台驱动（PL011）
5. ARMv8 定时器驱动
6. GICv3 中断控制器驱动（⚠️ 初始化导致挂起，已暂时禁用）
7. 基础内存管理（页帧、堆分配器）
8. 配置系统（menuconfig）
9. 构建和测试脚本

### ✅ 已完成（Phase 2 - 核心框架）
1. **系统调用框架** - `syscall_handler` 完全正常
   - SVC 指令处理
   - 系统调用分发
   - 已实现：read, write, openat, pipe, getpid, getppid, exit, wait4, kill, sigaction, execve 等
   - **验证成功**：直接调用和从用户代码调用都正常工作

2. **进程调度器基础框架**
   - 调度器接口定义
   - 就绪队列管理
   - Round Robin 调度算法
   - 进程控制块（PCB）结构

3. **上下文切换** - `cpu_switch_to` 完全正常
   - 保存/恢复通用寄存器
   - 保存/恢复特殊寄存器（SP、ELR、SPSR）

4. **EL0 切换机制** - `switch_to_user` 验证成功
   - 通过 `eret` 指令从 EL1 切换到 EL0
   - 正确设置 SPSR、ELR_EL1、SP_EL0
   - 用户代码可以在 EL0 正常执行（NOP、B . 等指令）

5. **信号处理基础框架**
   - SignalStruct 和 SigPending 定义
   - 信号发送机制（send_signal）
   - 信号处理和检查

### ⚠️ 已知问题和限制

#### 1. MMU 使能问题（已决定暂时禁用）
- **状态**: MMU 启用后内核挂起
- **调查结果**: 页表描述符格式已修正，但仍发生递归异常
- **决定**: 暂时禁用 MMU，先实现其他不依赖 MMU 的功能
- **影响**: 无法使用虚拟内存映射，地址空间管理受限

#### 2. GIC/Timer 初始化问题（已暂时禁用）
- **状态**: 初始化导致内核在 "System ready" 后挂起
- **临时方案**: 禁用 GIC 和 Timer 初始化
- **影响**: 无法使用硬件中断和定时器
- **内核运行**: 无中断模式下正常工作

#### 3. HLT/SVC 指令从 EL0 触发 SError
- **现象**: 从 EL0 执行 HLT/SVC 触发异常类型 0x0B（SError from EL0 32-bit）
- **ESR_EL1**: EC=0x00（Trapped WFI/WFE 或 Uncategorized）
- **影响**: 无法直接从用户代码调用系统调用
- **替代方案**: 系统调用框架本身已验证可正常工作

#### 4. Task::new() 创建时挂起
- **状态**: 调用 `Task::new()` 创建新任务时内核挂起
- **可能原因**: 堆分配问题或 Task 结构体过大
- **临时方案**: 使用静态存储创建 idle task，普通进程创建待修复
- **影响**: 无法正常创建新进程（fork 功能受限）

#### 5. println! 宏兼容性问题
- **状态**: 使用 `println!` 宏可能导致编译错误或运行时问题
- **原因**: `core::fmt::Write` 依赖可能在某些情况下不可用
- **替代方案**: 使用 `crate::console::putchar` 进行底层输出
- **建议**: 调试时优先使用 `putchar` 或 `debug_println!`（仅字符串）

### 📋 代码审查发现的问题（2025-02-03）

详见 [docs/CODE_REVIEW.md](docs/CODE_REVIEW.md)

#### ✅ 已修复
1. **智能指针不一致** - 统一使用 SimpleArc
2. **全局可变状态无同步保护** - 使用 AtomicPtr
3. **FdTable MaybeUninit UB** - 使用 from_fn 安全初始化

#### ⏳ 待修复（优先级排序）
1. **SimpleArc Clone 支持** - 影响多个文件系统操作
   - RootFSNode::find_child 返回 None
   - RootFSNode::list_children 返回空 Vec
   - RootFSSuperBlock::get_root 返回 None
2. **RootFS::write_data offset bug** - 忽略 offset 参数，替换整个文件
3. **VFS 函数指针安全性** - 使用裸指针可能导致内存安全问题
4. **Dentry/Inode 缓存机制** - 缺少哈希表加速查找
5. **路径解析不完整** - 缺少符号链接解析、相对路径处理
6. **CpuContext 混合内核/用户寄存器** - 代码组织问题

### 🔄 进行中（Phase 17 - 功能扩展）

**已完成（2025-02-09）**：
1. ✅ **RISC-V 系统调用和用户程序支持完全实现**
   - Trap 处理框架完整
   - 用户模式切换成功
   - 系统调用分发正常
   - 用户程序可以正常退出

**待完成**：
1. 完善文件系统相关系统调用（read、write、openat 等）
2. 实现进程管理相关功能
3. 添加更多用户程序示例

### ⏳ 待实现（Phase 18+）
1. 文件系统（VFS、ext4、btrfs）
2. 网络协议栈（TCP/IP）
3. IPC（管道、消息队列、共享内存）
4. x86_64 和 riscv64 平台支持
5. 用户空间工具

## 常见开发任务

### 编译和运行
```bash
make build          # 编译内核
make run            # 在 QEMU 中运行
make test           # 运行测试套件
make clean          # 清理构建产物
```

### 配置内核
```bash
make menuconfig     # 交互式配置
vim Kernel.toml     # 手动编辑配置
make config         # 查看当前配置
```

### 调试
```bash
make debug          # GDB 调试
./test/debug.sh     # 详细调试脚本
```

### 添加新功能
1. 查阅 `docs/TODO.md` 找到相关任务
2. 在 `kernel/src/` 相应目录创建模块
3. 更新 `Kernel.toml` 配置（如需要）
4. 添加测试到 `test/`
5. 更新文档

## 架构特定信息

### aarch64（当前默认）
- **入口点**: 0x40000000
- **页大小**: 4096 字节
- **异常级别**: EL1（内核运行）
- **UART 基址**: 0x0900_0000（PL011）
- **定时器**: ARMv8 架构定时器
- **中断控制器**: GICv3

### 启动流程
1. **boot.S**: EL 检测，降级到 EL1
2. **设置栈**: 使用 16KB 栈空间
3. **清零 BSS**: 清零未初始化数据
4. **跳转到 _start**: Rust 代码入口
5. **初始化**: UART → 异常 → 定时器 → 中断

### 内存布局
```
0x4000_0000  内核代码和只读数据
0x4001_8000  内核数据段
0x4001_9000  BSS 段（清零）
0x4001_9000  栈空间（16KB）
0x4001_D000  页表（16KB）
```

## 代码约定

### Rust 代码
- **no_std**: 不使用标准库
- **panic 策略**: 终止（hang）
- **内联汇编**: 使用 `core::arch::asm!`
- **全局汇编**: 使用 `core::arch::global_asm!`

### 汇编代码
- **语法**: ARM64 (GNU as)
- **对齐**: 异常表 2KB 对齐（`.align 11`）
- **注释**: 使用 C 风格（`//` 或 `/* */`）

### 文件组织
- 每个架构在 `kernel/src/arch/*/` 有自己的目录
- 驱动在 `kernel/src/drivers/` 下按类型组织
- 使用模块化的 `mod.rs` 导出公共接口

## 配置系统

### Kernel.toml 解析
1. `kernel/build.rs` 在编译时读取 `Kernel.toml`
2. 生成 `kernel/src/config.rs`（包含常量定义）
3. 内核代码通过 `crate::config::*` 使用配置

### 添加新配置项
1. 在 `Kernel.toml` 添加配置项
2. 在 `kernel/build.rs` 的 `generate_config_code()` 中添加解析
3. 在 `kernel/src/config.rs` 添加对应的常量

## 测试和调试

### QEMU 命令
```bash
# 基本运行
qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
  -kernel target/aarch64-unknown-none/debug/rux

# 调试模式（GDB）
qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
  -kernel target/aarch64-unknown-none/debug/rux -S -s

# 查看输出
qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
  -serial mon:stdio -kernel target/aarch64-unknown-none/debug/rux
```

### GDB 调试
```bash
# 连接到 QEMU
aarch64-none-elf-gdb target/aarch64-unknown-none/debug/rux
(gdb) target remote localhost:1234
(gdb) break *0x40000000
(gdb) continue
```

## 故障排查

### 编译问题
- 检查 Rust 版本：`rustc --version`
- 清理构建：`make clean`
- 检查目标工具链：`ls ~/.rustup/toolchains`

### 运行问题
- 检查 QEMU 版本：`qemu-system-aarch64 --version`
- 检查内核编译：`ls target/aarch64-unknown-none/debug/rux`
- 查看输出：使用 `-serial mon:stdio` 选项

### 配置未生效
- 检查 `kernel/src/config.rs` 是否更新
- 清理并重新编译：`make clean && make build`

## 遵循"不创新"原则的开发指南

### 添加新功能时

**必须先做**：
1. 查阅 Linux 内核源码中对应功能的实现
2. 阅读相关的 Linux man pages
3. 查阅 POSIX 标准文档
4. 确认使用相同的接口和数据结构

**禁止行为**：
- ❌ 觉自己的"理解"修改接口
- ❌ 为了"更简洁"改变设计
- ❌ 认为"Linux 的设计太老"而更新
- ❌ 创造新的抽象或接口

### 具体实现指导

#### 系统调用
```rust
// ✅ 正确：使用 Linux 的系统调用号
pub const __NR_read: usize = 63;   // 与 Linux 完全一致

// ❌ 错误：自己定义系统调用号
pub const SYS_RUX_READ: usize = 1000;  // 绝对禁止！
```

#### 数据结构
```rust
// ✅ 正确：参考 Linux 的结构体
#[repr(C)]
pub struct Stat {
    st_dev: u64,
    st_ino: u64,
    st_mode: u32,
    st_nlink: u32,
    st_uid: u32,
    st_gid: u32,
    st_rdev: u64,
    st_size: i64,
    st_blksize: u64,
    st_blocks: u64,
    st_atime: u64,
    st_mtime: u64,
    st_ctime: u64,
    // 字段顺序、大小、对齐与 Linux 完全一致
}

// ❌ 错误：自己定义结构体
#[repr(C)]
pub struct RuxStat {
    // ... 不要自己发明结构！
}
```

#### 文件系统
```rust
// ✅ 正确：实现 ext4 文件系统
// 参考 Linux fs/ext4/ 目录的实现

// ❌ 错误：发明新的文件系统
// 不要创建 RuxFS 或其他"改进型"文件系统
```

### 代码审查检查点

在审查代码时，必须检查：

1. [ ] 系统调用号是否与 Linux 一致？
2. [ ] 数据结构布局是否与 Linux 一致？
3. [ ] 是否遵循了 POSIX 标准？
4. [ ] 是否参考了 Linux 内核源码？
5. [ ] 是否包含了任何"创新"？

如果发现违反原则的代码，必须拒绝并要求修改。

## 开发建议
1. 确定修改的模块（drivers/arch/mm 等）
2. 编辑 Rust 代码
3. 运行 `make build` 编译
4. 运行 `make test` 测试
5. 运行 `make run` 验证

### 添加新平台支持
1. 创建 `kernel/src/arch/<platform>/` 目录
2. 实现 boot.S、trap.rs、mm.rs 等
3. 添加链接器脚本
4. 配置 Cargo target
5. 更新文档

### 添加新驱动
1. 在 `kernel/src/drivers/` 下创建子目录
2. 实现驱动 trait/struct
3. 在 `Kernel.toml` 添加配置选项
4. 在初始化代码中注册驱动

## 相关资源

- **设计文档**: [docs/DESIGN.md](docs/DESIGN.md)
- **任务列表**: [docs/TODO.md](docs/TODO.md)
- **代码审查记录**: [docs/CODE_REVIEW.md](docs/CODE_REVIEW.md)
- **快速参考**: [docs/QUICKREF.md](docs/QUICKREF.md)
- **结构说明**: [docs/STRUCTURE.md](docs/STRUCTURE.md)
- **配置指南**: [docs/CONFIG.md](docs/CONFIG.md)

## 项目上下文

### 开发阶段
- **当前**: Phase 1 完成，Phase 2 进行中
- **目标**: 完整的类 Linux 操作系统内核
- **进度**: 约 10% 完成（基础框架）

### 技术亮点
- 全 Rust 编写（除必要汇编）
- 多平台支持设计
- 模块化架构
- Linux 兼容目标

### 已知限制
- 仅支持 QEMU virt 机器
- GIC/Timer 初始化暂时禁用（导致挂起）
- MMU 暂时禁用（地址翻译问题待解决）
- println! 宏有兼容性问题（优先使用 putchar）
- Task::new() 创建时挂起（堆分配问题）
- 暂无用户空间
- 单核支持

---

## 💡 重要开发经验（2025-02-03）

### 调试技巧

#### 1. 内核挂起时的调试方法
```bash
# 使用 timeout 防止无限等待
timeout 2 qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
  -kernel target/aarch64-unknown-none/debug/rux

# 使用 GDB 调试
qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
  -kernel target/aarch64-unknown-none/debug/rux -S -s
```

#### 2. 打印调试（优先使用 putchar）
```rust
// ✅ 推荐：使用底层 putchar
unsafe {
    use crate::console::putchar;
    const MSG: &[u8] = b"Debug message\n";
    for &b in MSG {
        putchar(b);
    }
}

// ⚠️ 谨慎：println! 可能有兼容性问题
println!("Test message");  // 可能导致编译错误或运行时问题

// ✅ 安全：debug_println!（仅字符串）
debug_println!("Debug message");  // 仅支持字符串字面量
```

#### 3. 检查代码执行位置
```rust
// 在关键位置添加标记
unsafe {
    use crate::console::putchar;
    const MSG1: &[u8] = b"Checkpoint 1\n";
    for &b in MSG1 { putchar(b); }
    // ... 代码 ...
    const MSG2: &[u8] = b"Checkpoint 2\n";
    for &b in MSG2 { putchar(b); }
}
```

### 已知陷阱

#### 1. GIC/Timer 初始化导致挂起
```rust
// ❌ 不要启用（会导致挂起）
drivers::intc::init();
drivers::timer::init();
unsafe {
    asm!("msr daifclr, #2", options(nomem, nostack));
}

// ✅ 当前解决方案：暂时禁用
// 直接注释掉上述代码
```

#### 2. Task::new() 创建时挂起
```rust
// ❌ 在栈上创建大型结构体可能导致问题
let task = Task::new(pid, policy);
let task_box = Box::leak(Box::new(task));  // 可能挂起

// ✅ 当前解决方案：使用静态存储
static mut TASK_STORAGE: Option<Task> = None;
// 在静态存储上直接构造
unsafe {
    Task::new_idle_at(&mut TASK_STORAGE);
}
```

#### 3. println! 宏的格式参数问题
```rust
// ❌ 解引用操作符在宏中不支持
debug_println!("Value = {}", *value);

// ✅ 解决方案：先解引用
let val = *value;
debug_println!("Value = {}", val);  // 仍可能不工作

// ✅ 最佳方案：使用 putchar 或 println!（如果有条件）
```

### 测试验证

#### 系统调用测试（已验证可用）
```rust
// 直接从内核调用系统调用处理函数
let mut frame = crate::arch::aarch64::syscall::SyscallFrame {
    x0: 1,      // fd = stdout
    x1: 0,      // buf = null
    x2: 10,     // count = 10
    x8: 0,      // SYS_READ = 0
    ..Default::default()
};
crate::arch::aarch64::syscall::syscall_handler(&mut frame);
// 预期返回：sys_read: invalid fd
```

#### EL0 切换测试（已验证可用）
```rust
// 设置系统寄存器并执行 eret
unsafe {
    asm!(
        "msr sp_el0, {}",      // 用户栈指针
        "msr elr_el1, {}",      // 入口点
        "msr spsr_el1, {}",     // SPSR (EL0t)
        "isb",
        "eret",
        in(reg) user_stack,
        in(reg) code_addr,
        in(reg) spsr_value,
        options(nomem, nostack)
    );
}
// 用户代码应包含：NOP, B . 等简单指令
```

### 代码统计（2025-02-03）
- **总代码行数**: ~5100 行 Rust 代码
- **架构支持**: aarch64（主要），x86_64/riscv64（待实现）
- **内核大小**: ~2MB（debug 模式）
- **编译时间**: ~0.3s（增量编译）

### 下一步建议

1. **修复已知问题**：
   - 解决 Task::new() 的堆分配问题
   - 修复 println! 宏的兼容性问题
   - 调查 GIC/Timer 初始化的根本原因

2. **功能扩展**：
   - 实现简化的进程创建机制
   - 添加更多系统调用实现
   - 完善文件系统功能

3. **架构支持**：
   - 添加 x86_64 平台支持
   - 添加 riscv64 平台支持

4. **性能优化**：
   - 实现多核支持（SMP）
   - 优化调度算法
   - 减少内核大小

---

当帮助用户时：
1. 优先查看 `docs/TODO.md` 了解项目状态
2. 检查相关文档确认技术细节
3. 使用 `make` 命令而非直接调用 cargo
4. 注意 `config.rs` 是自动生成的
5. 遵循现有的代码风格和组织方式

### 关键约束
- **no_std**: 不能使用 Rust 标准库
- **平台汇编**: 必须用汇编编写启动代码
- **QEMU 依赖**: 开发和测试依赖 QEMU
- **单核设计**: 当前不支持 SMP

## 更新日志

### 2025-02-09
- ✅ **重大里程碑：RISC-V 系统调用和用户程序支持完全实现** 🎉
  - **Trap 处理框架完整实现**
    - `kernel/src/arch/riscv64/trap.S`: 汇编语言 trap 入口/出口（272 字节 TrapFrame）
    - `kernel/src/arch/riscv64/trap.rs`: Rust 语言 trap 处理和异常分发
    - sscratch 寄存器管理：在 trap 出口时恢复内核栈指针，确保连续系统调用正常工作
  - **用户模式切换实现**
    - `kernel/src/arch/riscv64/usermode_asm.S`: Linux 风格单一页表方法
    - 使用 sret 指令从 S-mode 切换到 U-mode
    - 正确设置 sstatus.SPP=0、sstatus.SPIE=1、sstatus.UXL=2
  - **系统调用实现**
    - ✅ sys_exit (93): 进程退出，调用 do_exit()
    - ✅ sys_getpid (172): 获取进程 ID，返回当前进程 PID
    - ✅ sys_getppid (110): 获取父进程 ID
  - **用户程序工具链**
    - `userspace/hello_world/`: no_std 用户程序示例
    - 自定义链接器脚本 `user.ld` 链接到用户空间地址 0x10000
    - 内联汇编系统调用包装函数（syscall1/syscall3）
  - **嵌入式 ELF 加载器**
    - `kernel/embed_user_programs.sh`: 将用户程序 ELF 嵌入到内核源码
    - `kernel/src/embedded_user_programs.rs`: 自动生成的字节数组
  - **测试验证成功**
    - 用户程序成功调用 sys_exit(0) 并正常终止
    - 系统调用框架完全功能化
    - Trap 入口/出口、栈切换、上下文保存/恢复全部正常

- 🐛 **Bug 修复**
  - **sscratch 寄存器管理问题**：在 trap 出口时恢复内核栈指针到 sscratch
    - 问题：用户栈指针被错误地写入 sscratch，导致第二个系统调用失败
    - 修复：使用 `csrr t1, sscratch; mv sp, t0; csrw sscratch, t1` 恢复内核栈指针
  - **用户程序嵌入更新问题**：修改用户程序后需要重新运行 `embed_user_programs.sh`

- 📝 **文档更新**
  - 更新 `docs/development/changelog.md` 添加系统调用实现记录
  - 更新 `docs/architecture/riscv64.md` 添加完整的系统调用章节
  - 重写 `docs/development/user-programs.md` 用户程序开发指南
  - 更新 `README.md` 反映最新实现状态

- 📊 **代码统计（2025-02-09）**
  - **总代码行数**: ~5500 行 Rust 代码（kernel/）
  - **架构支持**: RISC-V64（默认）、ARM64（完整）、x86_64（待实现）
  - **内核大小**: ~3MB（debug 模式）
  - **用户程序**: ~5KB（hello_world）

### 2025-02-03
- ✅ **代码审查与修复**
  - 完成全面代码审查，对比 Linux 内核实现
  - 统一使用 SimpleArc（解决 alloc crate 符号可见性问题）
  - 全局状态同步保护（使用 AtomicPtr 替代 static mut）
  - FdTable MaybeUninit UB 修复（使用 from_fn 安全初始化）
  - 新增 [CODE_REVIEW.md](docs/CODE_REVIEW.md) 文档记录问题和修复进度
- ✅ **VFS 层改进**
  - 为 SimpleArc 添加 Deref trait 实现
  - 修复全局可变状态的线程安全问题
  - RootFS 全局状态使用 AtomicPtr 保护
- ✅ **EL0 切换机制验证成功**
  - 通过 `eret` 指令成功从 EL1 切换到 EL0
  - 验证用户代码可以在 EL0 正常执行
- ✅ **系统调用框架验证成功**
  - `syscall_handler` 正常工作
  - 直接调用系统调用测试通过
- ✅ **进程调度器基础框架完成**
  - 调度器接口、运行队列管理
  - Round Robin 调度算法
  - PID 获取功能正常
- ⚠️ **MMU 使能问题**
  - 页表描述符格式已修正
  - 但仍发生递归异常，已决定暂时禁用
- ⚠️ **GIC/Timer 初始化问题**
  - 导致内核挂起，已暂时禁用
  - 内核在无中断模式下正常工作
- 📝 **文档更新**
  - 更新 TODO.md 反映最新进展
  - 更新 README.md 添加当前状态

### 2025-02-02
- 完成项目结构重组
- 添加配置系统（menuconfig）
- 实现异常处理框架
- 完善文档和测试脚本
