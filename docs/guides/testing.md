# Rux 内核测试指南

本文档说明 Rux 内核的测试体系，包括测试框架、测试状态和最佳实践。

**最后更新**：2026-03-04
**测试规模**：51 个内核测试 + 24 个 mini-ltp 兼容性测试

---

## 目录

- [测试体系总览](#测试体系总览)
- [测试环境配置](#测试环境配置)
- [内核单元测试](#内核单元测试)
- [用户态兼容性测试](#用户态兼容性测试)
- [添加新测试](#添加新测试)
- [测试最佳实践](#测试最佳实践)
- [已知限制](#已知限制)

---

## 测试体系总览

```
Rux 测试体系
├── 内核单元测试 (kernel/src/tests/)
│   │
│   ├── 基础数据结构 (4 个模块)
│   │   ├── listhead.rs    - 双向链表
│   │   ├── path.rs        - 路径解析
│   │   ├── file_flags.rs  - 文件标志
│   │   └── boundary.rs    - 边界条件
│   │
│   ├── 内存管理 (5 个模块)
│   │   ├── heap_allocator.rs   - 堆分配器
│   │   ├── page_allocator.rs   - 页分配器
│   │   ├── standard_alloc.rs   - 标准分配器
│   │   ├── mem_mmap.rs         - mmap 系统调用
│   │   └── mem_cow.rs          - Copy-on-Write
│   │
│   ├── 进程管理 (8 个模块)
│   │   ├── scheduler.rs            - 调度器
│   │   ├── process_tree.rs         - 进程树
│   │   ├── fork.rs                 - fork 系统调用
│   │   ├── execve.rs               - execve 系统调用
│   │   ├── getpid.rs               - 进程 ID
│   │   ├── wait4.rs                - wait4 系统调用
│   │   ├── preemptive_scheduler.rs - 抢占式调度
│   │   └── sleep_wakeup.rs         - 睡眠唤醒
│   │
│   ├── 信号处理 (2 个模块)
│   │   ├── signal.rs          - 信号处理
│   │   └── signal_procmask.rs - 信号掩码
│   │
│   ├── 文件系统 (8 个模块)
│   │   ├── file_open.rs   - 文件打开
│   │   ├── fdtable.rs     - 文件描述符表
│   │   ├── dcache.rs      - 目录项缓存
│   │   ├── icache.rs      - Inode 缓存
│   │   ├── fstat.rs       - 文件状态
│   │   ├── fcntl.rs       - 文件控制
│   │   ├── link.rs        - 硬链接
│   │   └── mkdir_unlink.rs - 目录操作
│   │
│   ├── ext4 文件系统 (3 个模块)
│   │   ├── ext4_allocator.rs      - ext4 分配器
│   │   ├── ext4_file_write.rs     - ext4 文件写入
│   │   └── ext4_indirect_blocks.rs - ext4 间接块
│   │
│   ├── IPC (4 个模块)
│   │   ├── pipe2.rs       - pipe2 系统调用
│   │   ├── ipc_poll.rs    - poll 系统调用
│   │   ├── ipc_epoll.rs   - epoll 系统调用
│   │   └── ipc_eventfd.rs - eventfd 系统调用
│   │
│   ├── 网络 (3 个模块)
│   │   ├── network.rs        - 网络框架
│   │   ├── tcp_handshake.rs  - TCP 握手
│   │   └── virtio_net.rs     - VirtIO 网卡
│   │
│   ├── 设备驱动 (2 个模块)
│   │   ├── virtio_queue.rs  - VirtIO 队列
│   │   └── framebuffer.rs   - 帧缓冲
│   │
│   ├── SMP 多核 (2 个模块)
│   │   ├── smp.rs          - SMP 多核启动
│   │   └── smp_schedule.rs - SMP 调度
│   │
│   ├── 用户模式 (1 个模块)
│   │   └── user_syscall.rs - 用户系统调用
│   │
│   └── 系统调用 (9 个模块)
│       ├── syscall_file.rs    - 文件系统调用
│       ├── syscall_memory.rs  - 内存系统调用
│       ├── syscall_process.rs - 进程系统调用
│       ├── syscall_sched.rs   - 调度系统调用
│       ├── syscall_signal.rs  - 信号系统调用
│       ├── syscall_network.rs - 网络系统调用
│       ├── syscall_io.rs      - I/O 系统调用
│       ├── syscall_time.rs    - 时间系统调用
│       └── syscall_misc.rs    - 杂项系统调用
│
├── 用户态兼容性测试 (userspace/tests/mini-ltp/)
│   │
│   ├── 文件操作 (8 个)
│   │   ├── test_fileio.c  - 文件读写
│   │   ├── test_stat.c    - 文件状态
│   │   ├── test_lseek.c   - 文件定位
│   │   ├── test_mkdir.c   - 目录操作
│   │   ├── test_rename.c  - 文件重命名
│   │   ├── test_unlink.c  - 文件删除
│   │   ├── test_access.c  - 访问权限
│   │   └── test_writev.c  - 向量 I/O
│   │
│   ├── 进程管理 (5 个)
│   │   ├── test_fork.c   - 进程创建
│   │   ├── test_execve.c - 程序执行
│   │   ├── test_wait.c   - 等待子进程
│   │   ├── test_exit.c   - 进程退出
│   │   └── test_getpid.c - 进程 ID
│   │
│   ├── 内存管理 (2 个)
│   │   ├── test_mmap.c - 内存映射
│   │   └── test_brk.c  - 堆内存
│   │
│   ├── 时间 (2 个)
│   │   ├── test_time.c      - 时间系统调用
│   │   └── test_nanosleep.c - 高精度睡眠
│   │
│   └── 其他 (7 个)
│       ├── test_pipe.c    - 管道通信
│       ├── test_dup.c     - 文件描述符复制
│       ├── test_chdir.c   - 目录切换
│       ├── test_getuid.c  - 用户/组 ID
│       ├── test_ioctl.c   - 终端 ioctl
│       ├── test_fcntl.c   - 文件控制
│       └── test_fsync.c   - 文件同步
│
└── 整机测试 (test/)
    ├── quick_test.sh    - 快速启动测试
    ├── run_riscv64.sh   - 完整运行测试
    └── debug_riscv.sh   - GDB 调试
```

---

## 测试环境配置

### 启用内核单元测试

Rux 使用 `unit-test` 特性控制测试编译：

```bash
# 编译时启用单元测试
cargo build --package rux --features riscv64,unit-test

# 运行测试
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux
```

### 正常编译（不含测试）

```bash
# 正常编译，不包含测试代码
make build

# 或直接使用 cargo
cargo build --package rux --features riscv64
```

### 测试环境

| 项目 | 配置 |
|------|------|
| **QEMU** | 6.2.0+ (RISC-V 64-bit) |
| **目标平台** | riscv64gc-unknown-none-elf |
| **CPU** | 4 核 (QEMU virt 机器) |
| **内存** | 2 GB |
| **MMU** | Sv39 (3级页表) |

---

## 内核单元测试

### 测试框架

Rux 是 `no_std` 内核，不能使用标准库的 `#[test]` 和 `cargo test`。使用自定义测试框架：

**框架位置**: `kernel/src/tests/mod.rs`

**核心组件**:
- `test_pass(name)` - 记录测试通过
- `test_fail(name, reason)` - 记录测试失败
- `test_skip(name, reason)` - 记录测试跳过
- `test_group_start(name)` - 开始测试组
- `test_assert!()` - 断言宏（失败不 panic）
- `test_assert_eq!()` - 相等断言宏

**测试入口**: `run_all_tests()` 函数

### 测试模块状态

#### 基础数据结构

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| listhead.rs | ✅ 完全通过 | 初始化、添加、删除、遍历 |
| path.rs | ✅ 完全通过 | 绝对路径、父目录、文件名提取 |
| file_flags.rs | ✅ 完全通过 | 访问模式、标志组合 |
| boundary.rs | ⚠️ 部分通过 | 进程池耗尽（预期行为） |

#### 内存管理

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| heap_allocator.rs | ✅ 完全通过 | Box、Vec、String 分配 |
| page_allocator.rs | ✅ 完全通过 | PhysAddr/VirtAddr、FrameAllocator |
| standard_alloc.rs | ✅ 完全通过 | 标准库分配器接口 |
| mem_mmap.rs | ✅ 完全通过 | mmap/munmap/mprotect/msync |
| mem_cow.rs | ✅ 完全通过 | COW 常量、页错误处理、fork 集成 |

#### 进程管理

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| scheduler.rs | ✅ 完全通过 | get_current_pid/ppid、find_task_by_pid |
| process_tree.rs | ✅ 完全通过 | 父子关系、兄弟关系、链表完整性 |
| fork.rs | ⚠️ 部分通过 | 基本 fork（资源池限制） |
| execve.rs | ✅ 完全通过 | 空路径、不存在文件、ELF 加载 |
| getpid.rs | ✅ 完全通过 | getpid/getppid 一致性 |
| wait4.rs | ✅ 完全通过 | ECHILD、WNOHANG |
| preemptive_scheduler.rs | ✅ 完全通过 | jiffies、need_resched、时间片 |
| sleep_wakeup.rs | ✅ 完全通过 | TaskState、wake_up |

#### 信号处理

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| signal.rs | ✅ 完全通过 | Signal 枚举、SigFlags、SigAction |
| signal_procmask.rs | ✅ 完全通过 | rt_sigprocmask、SIG_BLOCK/UNBLOCK |

#### 文件系统

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| file_open.rs | ✅ 完全通过 | RootFS 查找、创建、O_CREAT/O_EXCL |
| fdtable.rs | ✅ 完全通过 | alloc_fd、install_fd、close_fd、fd 重用 |
| dcache.rs | ✅ 完全通过 | dcache_add/lookup/remove、LRU |
| icache.rs | ✅ 完全通过 | icache_add/lookup/remove、LRU |
| fstat.rs | ✅ 完全通过 | fstat 系统调用 |
| fcntl.rs | ✅ 完全通过 | fcntl 系统调用 |
| link.rs | ✅ 完全通过 | link 系统调用 |
| mkdir_unlink.rs | ✅ 完全通过 | mkdir/unlink 系统调用 |

#### ext4 文件系统

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| ext4_allocator.rs | ✅ 完全通过 | BlockAllocator、InodeAllocator |
| ext4_file_write.rs | ✅ 完全通过 | 文件写入操作 |
| ext4_indirect_blocks.rs | ✅ 完全通过 | 单级间接块、索引计算 |

#### IPC

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| pipe2.rs | ✅ 完全通过 | pipe2、O_CLOEXEC、O_NONBLOCK |
| ipc_poll.rs | ✅ 完全通过 | poll、PollFd、POLLIN/POLLOUT |
| ipc_epoll.rs | ✅ 完全通过 | epoll_create/ctl/wait |
| ipc_eventfd.rs | ✅ 完全通过 | eventfd、事件通知 |

#### 网络

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| network.rs | ✅ 完全通过 | 网络子系统初始化 |
| tcp_handshake.rs | ✅ 完全通过 | TCP 连接建立、三次握手 |
| virtio_net.rs | ✅ 完全通过 | VirtIO-net 设备、数据包收发 |

#### 设备驱动

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| virtio_queue.rs | ✅ 完全通过 | VirtIO 数据结构、描述符 |
| framebuffer.rs | ✅ 完全通过 | framebuffer 初始化、像素操作 |

#### SMP 多核

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| smp.rs | ✅ 完全通过 | is_boot_hart、hart ID、MAX_CPUS |
| smp_schedule.rs | ⚠️ 部分通过 | Per-CPU 运行队列、load_balance |

#### 系统调用

| 模块 | 状态 | 测试内容 |
|------|------|----------|
| syscall_file.rs | ✅ 完全通过 | open/close/read/write/lseek/fstat |
| syscall_memory.rs | ✅ 完全通过 | brk/mmap/munmap/mprotect |
| syscall_process.rs | ✅ 完全通过 | fork/execve/wait4/exit/getpid |
| syscall_sched.rs | ✅ 完全通过 | sched_yield/nice |
| syscall_signal.rs | ✅ 完全通过 | kill/sigaction/sigprocmask |
| syscall_network.rs | ✅ 完全通过 | socket/bind/listen/accept/connect |
| syscall_io.rs | ✅ 完全通过 | poll/select/epoll |
| syscall_time.rs | ✅ 完全通过 | time/gettimeofday/nanosleep |
| syscall_misc.rs | ✅ 完全通过 | uname/sysinfo/prlimit64/getrandom |

---

## 用户态兼容性测试

### mini-ltp 测试套件

**位置**: `userspace/tests/mini-ltp/`

**测试列表** (24 个):

| 测试程序 | 测试内容 | 状态 |
|----------|----------|------|
| test_fork | 进程创建 | ✅ |
| test_getpid | 进程 ID 获取 | ✅ |
| test_fileio | 文件 I/O | ✅ |
| test_pipe | 管道通信 | ✅ |
| test_dup | 文件描述符复制 | ✅ |
| test_mmap | 内存映射 | ✅ |
| test_stat | 文件状态获取 | ✅ |
| test_mkdir | 目录操作 | ✅ |
| test_lseek | 文件定位 | ✅ |
| test_time | 时间系统调用 | ✅ |
| test_wait | 等待子进程 | ✅ |
| test_exit | 进程退出 | ✅ |
| test_brk | 堆内存管理 | ✅ |
| test_chdir | 目录切换 | ✅ |
| test_rename | 文件重命名 | ✅ |
| test_unlink | 文件删除 | ✅ |
| test_access | 访问权限检查 | ✅ |
| test_writev | 向量 I/O | ✅ |
| test_execve | 程序执行 | ✅ |
| test_getuid | 用户/组 ID | ✅ |
| test_nanosleep | 高精度睡眠 | ✅ |
| test_ioctl | 终端 ioctl | ✅ |
| test_fcntl | 文件控制 | ✅ |
| test_fsync | 文件同步 | ✅ |

### 构建 mini-ltp

```bash
cd userspace/tests/mini-ltp
./build.sh
```

### 运行 mini-ltp

```bash
# 在 Rux shell 中
/test/mini-ltp/run_tests.sh
```

---

## 添加新测试

### 添加内核单元测试

1. **创建测试文件** `kernel/src/tests/my_feature.rs`:

```rust
use crate::tests::{test_pass, test_fail, test_group_start};

pub fn test_my_feature() {
    test_group_start("my_feature");

    // 测试用例 1
    if some_condition {
        test_pass("test_case_1");
    } else {
        test_fail("test_case_1", "reason");
    }

    // 测试用例 2
    test_assert!(another_condition, "test_case_2");
}
```

2. **注册测试** 在 `kernel/src/tests/mod.rs` 中:

```rust
#[cfg(feature = "unit-test")]
pub mod my_feature;

// 在 run_all_tests() 中添加
my_feature::test_my_feature();
```

3. **编译运行**:

```bash
cargo build --package rux --features riscv64,unit-test
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux
```

### 添加 mini-ltp 测试

1. **创建 C 源文件** `userspace/tests/mini-ltp/src/test_xxx.c`:

```c
#include <stdio.h>
#include <unistd.h>

int main(void) {
    // 测试代码
    if (syscall_succeeds) {
        return 0;  // PASS
    } else {
        return 1;  // FAIL
    }
}
```

2. **构建测试**:

```bash
cd userspace/tests/mini-ltp
./build.sh
```

3. **更新 rootfs**:

```bash
make rootfs
```

---

## 测试最佳实践

### 测试命名规范

| 类型 | 命名格式 |
|------|----------|
| 测试文件 | `test_<module>.rs` 或 `<feature>.rs` |
| 测试函数 | `test_<feature>()` |
| 测试组 | `test_group_start("<module_name>")` |
| 测试用例 | 描述性名称，如 "basic_fork_success" |

### 测试结构

```rust
pub fn test_feature() {
    test_group_start("feature");

    // 1. 基本功能
    test_assert!(basic_check(), "basic_functionality");

    // 2. 边界条件
    test_assert_eq!(edge_case(), expected, "edge_case");

    // 3. 错误处理
    test_assert!(error_handled(), "error_handling");
}
```

### 避免的问题

| 问题 | 说明 |
|------|------|
| ❌ 全局状态依赖 | 每个测试应独立初始化 |
| ❌ 大对象栈分配 | 使用 Box 堆分配 |
| ❌ 复杂 drop 操作 | 可能触发 PANIC |

### 安全操作

| 操作 | 说明 |
|------|------|
| ✅ Box 分配 | 单个对象堆分配 |
| ✅ 简单栈分配 | 基本类型、小数组 |
| ✅ 整数运算 | 无内存操作 |

---

## 已知限制

### 1. Vec Drop PANIC

**问题**: `Vec` 离开作用域时释放内存可能触发 PANIC

**临时方案**: 跳过 Vec drop 相关测试，只测试基本操作

### 2. 无法使用 cargo test

**原因**: Rux 是 `no_std` 内核

**解决方案**: 使用自定义测试框架，在 QEMU 中运行

### 3. 资源池限制

**问题**: 部分测试（如多次 fork）受静态资源池限制

**解决方案**: 测试边界条件后跳过，或实现动态资源分配

---

## 测试覆盖统计

### 按模块分类

| 模块 | 测试文件数 | 状态 |
|------|-----------|------|
| 基础数据结构 | 4 | ✅ 优秀 |
| 内存管理 | 5 | ✅ 优秀 |
| 进程管理 | 8 | ✅ 良好 |
| 信号处理 | 2 | ✅ 优秀 |
| 文件系统 | 8 | ✅ 优秀 |
| ext4 | 3 | ✅ 优秀 |
| IPC | 4 | ✅ 优秀 |
| 网络 | 3 | ✅ 优秀 |
| 设备驱动 | 2 | ✅ 优秀 |
| SMP 多核 | 2 | ✅ 良好 |
| 用户模式 | 1 | ✅ 优秀 |
| 系统调用 | 9 | ✅ 优秀 |
| **总计** | **51** | **~98% 通过** |

### 历史趋势

| 日期 | 版本 | 测试文件 | 备注 |
|------|------|----------|------|
| 2026-02-09 | Phase 18.5 | ~40 | pagemap 重构 |
| 2026-02-11 | Phase 19 | 43 | COW + IPC |
| 2026-02-27 | Phase 22 | 43 | procfs + toybox |
| 2026-03-04 | Phase 24 | **51** | 系统调用测试 + framebuffer |

---

## 改进方向

### 短期
1. 增加并发测试
2. 添加性能基准测试
3. 完善边界条件测试

### 中期
1. 实现动态页表分配器
2. 完善 TCP/UDP 数据收发测试
3. 添加文件系统压力测试

### 长期
1. 建立 CI/CD 自动化测试
2. 添加模糊测试
3. 实现代码覆盖率统计

---

## 相关文档

- [开发流程规范](development.md)
- [设计文档](../architecture/design.md)
- [路线图](../progress/roadmap.md)

---

## 更新日志

- **2026-03-04**: 合并 unit-test-report.md 和 testing.md
  - 统一测试体系文档
  - 更新测试数量（51 内核 + 24 mini-ltp）
  - 添加测试体系总览图
- **2026-02-08**: 添加 fork/execve/wait4 测试
- **2026-02-08**: 初始版本，记录现有测试状态
