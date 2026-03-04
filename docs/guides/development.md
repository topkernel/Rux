# 开发流程规范 (Development Workflow)

本文档记录 Rux 内核开发的标准流程，确保每次代码修改都经过完整的验证和文档更新。

**最后更新**：2026-03-04

## 标准开发流程

### 1. 编写代码 (Write Code)

**原则**：
- 遵循 [DESIGN.md](../architecture/design.md) 中的设计原则
- 完全遵循 Linux ABI/POSIX 标准（见 [CLAUDE.md](../../CLAUDE.md)）
- 参考 Linux 内核源码实现

**步骤**：
1. 阅读 Linux 内核相关代码
2. 理解 POSIX 标准要求
3. 实现 Rust 代码
4. 添加必要的注释和文档

### 2. 内核单元测试 (Kernel Unit Tests)

**测试框架位置**: `kernel/src/tests/`

**测试数量**: 51 个测试模块

**测试内容分类**：

| 类别 | 测试模块 | 说明 |
|------|----------|------|
| **文件系统** | file_open, path, file_flags, fdtable, dcache, icache, fstat, fcntl, mkdir_unlink, link | VFS 和 ext4 测试 |
| **内存管理** | heap_allocator, page_allocator, mem_mmap, mem_cow, standard_alloc | 分配器和 COW 测试 |
| **进程管理** | fork, execve, wait4, process_tree, getpid, boundary | 进程生命周期测试 |
| **调度器** | scheduler, preemptive_scheduler, smp_schedule, sleep_wakeup | CFS 调度器测试 |
| **SMP 多核** | smp, smp_schedule | 多核启动和调度测试 |
| **信号处理** | signal, signal_procmask | 信号机制测试 |
| **IPC** | pipe2, ipc_poll, ipc_epoll, ipc_eventfd | 进程间通信测试 |
| **网络** | network, tcp_handshake, virtio_net | 网络协议栈测试 |
| **驱动** | virtio_queue, framebuffer | VirtIO 和帧缓冲测试 |
| **ext4** | ext4_allocator, ext4_file_write, ext4_indirect_blocks | ext4 文件系统测试 |
| **系统调用** | syscall_file, syscall_io, syscall_process, syscall_memory, syscall_time, syscall_network, syscall_sched, syscall_signal, syscall_misc | 系统调用分类测试 |
| **其他** | listhead, user_syscall, quick | 工具类测试 |

**运行测试**：
```bash
# 编译测试版本
cargo build --package rux --features riscv64,unit-test

# 运行测试
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux
```

**添加新测试**：

1. 在 `kernel/src/tests/` 创建新的测试文件：
```rust
// kernel/src/tests/my_feature.rs
use crate::tests::{test_pass, test_fail, test_group_start};

pub fn test_my_feature() {
    test_group_start("my_feature");

    // 测试代码
    if some_condition {
        test_pass("test_case_1");
    } else {
        test_fail("test_case_1", "reason");
    }
}
```

2. 在 `kernel/src/tests/mod.rs` 中注册：
```rust
#[cfg(feature = "unit-test")]
pub mod my_feature;

// 在 run_all_tests() 中添加
my_feature::test_my_feature();
```

### 3. 用户态兼容性测试 (mini-ltp)

**测试框架位置**: `userspace/tests/mini-ltp/`

**测试数量**: 24 个 C 语言测试程序

**测试列表**：

| 测试程序 | 测试内容 |
|----------|----------|
| test_fileio | 文件读写操作 |
| test_fork | fork 系统调用 |
| test_execve | execve 系统调用 |
| test_exit | exit 系统调用 |
| test_wait | wait/waitpid 系统调用 |
| test_getpid | getpid/getppid 系统调用 |
| test_pipe | pipe 管道 |
| test_dup | dup/dup2 系统调用 |
| test_mmap | mmap/munmap 内存映射 |
| test_brk | brk 堆内存调整 |
| test_lseek | lseek 文件定位 |
| test_mkdir | mkdir 创建目录 |
| test_unlink | unlink 删除文件 |
| test_rename | rename 重命名 |
| test_stat | stat/lstat 文件状态 |
| test_fcntl | fcntl 文件控制 |
| test_access | access 文件访问检查 |
| test_chdir | chdir 切换目录 |
| test_fsync | fsync 同步文件 |
| test_ioctl | ioctl 设备控制 |
| test_nanosleep | nanosleep 纳秒睡眠 |
| test_time | time/gettimeofday 时间 |
| test_getuid | getuid/geteuid 用户 ID |
| test_writev | writev 向量写 |

**构建测试**：
```bash
cd userspace/tests/mini-ltp
./build.sh
```

**运行测试**：
```bash
# 构建内核和 rootfs
make build && make user && make rootfs

# 启动内核
./test/run.sh

# 在 shell 中运行测试
/test/mini-ltp/run_tests.sh
```

**添加新测试**：

1. 创建 C 源文件 `userspace/tests/mini-ltp/src/test_xxx.c`：
```c
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>

int main(void) {
    // 测试代码
    if (syscall(SYS_xxx, ...) == 0) {
        printf("PASS\n");
        return 0;
    } else {
        printf("FAIL\n");
        return 1;
    }
}
```

2. 运行 `./build.sh` 编译

3. 更新 rootfs 添加测试程序

### 4. 整机测试 (Full System Testing)

**测试目标**：
- 验证内核正常启动
- 验证多核支持（SMP）
- 验证功能在真实环境中工作

**测试命令**：
```bash
# 编译
make build

# 单核启动测试
timeout 3 qemu-system-riscv64 -M virt -cpu rv64 -m 2G \
  -nographic -serial mon:stdio \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux

# 多核启动测试
timeout 3 qemu-system-riscv64 -M virt -cpu rv64 -m 2G \
  -nographic -serial mon:stdio -smp 4 \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux

# 使用测试脚本
./test/run.sh

# GUI 测试
./test/run.sh gui
```

**验证要点**：
- [ ] 内核成功启动
- [ ] 所有 hart 初始化（多核模式）
- [ ] 测试输出正确
- [ ] 无 panic 或挂起

### 5. 更新文档 (Update Documentation)

**需要更新的文档**：

1. **代码审查记录** ([code-review.md](../progress/code-review.md))
   - 标记已修复的问题为 ✅
   - 记录修复方案和提交信息
   - 更新待修复问题列表

2. **路线图** ([roadmap.md](../progress/roadmap.md))
   - 标记已完成的任务
   - 添加新发现的任务
   - 更新进度

3. **设计文档** (如适用)
   - [design.md](../architecture/design.md) - 架构设计变更
   - [structure.md](../architecture/structure.md) - 目录结构变更

4. **新增文档** (如适用)
   - 新功能的说明文档
   - 调试指南
   - 测试指南

### 6. 提交代码 (Commit Code)

**提交前检查**：
```bash
# 查看修改
git status
git diff

# 编译验证
make build

# 运行内核测试
cargo build --package rux --features riscv64,unit-test
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux

# 运行整机测试
./test/run.sh
```

**提交规范**：
```bash
git add <files>
git commit -m "<type>: <description>

## 详细说明

### 修改内容
- 具体修改点 1
- 具体修改点 2

### 技术细节
- 技术说明
- 设计决策

### 验证
- ✅ 测试 1 通过
- ✅ 测试 2 通过

### 相关文件
- file1.rs
- file2.rs

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

**提交类型**：
- `feat`: 新功能
- `fix`: 错误修复
- `test`: 测试相关
- `docs`: 文档更新
- `refactor`: 代码重构
- `perf`: 性能优化
- `chore`: 构建/工具链相关

## 测试体系总览

```
Rux 测试体系
├── 内核单元测试 (kernel/src/tests/)
│   ├── 文件系统测试 (12 个模块)
│   ├── 内存管理测试 (5 个模块)
│   ├── 进程管理测试 (6 个模块)
│   ├── 调度器测试 (4 个模块)
│   ├── SMP 多核测试 (2 个模块)
│   ├── 信号处理测试 (2 个模块)
│   ├── IPC 测试 (4 个模块)
│   ├── 网络测试 (3 个模块)
│   ├── 驱动测试 (2 个模块)
│   ├── ext4 测试 (3 个模块)
│   └── 系统调用测试 (9 个模块)
│
├── 用户态兼容性测试 (userspace/tests/mini-ltp/)
│   ├── 文件操作测试 (8 个)
│   ├── 进程管理测试 (5 个)
│   ├── 内存管理测试 (2 个)
│   ├── 时间测试 (2 个)
│   └── 其他测试 (7 个)
│
└── 整机测试 (test/)
    ├── quick_test.sh - 快速启动测试
    ├── run_riscv64.sh - 完整运行测试
    └── debug_riscv.sh - GDB 调试
```

## 快速检查清单

在提交任何代码前，确保：

- [ ] **代码编译通过** (`make build`)
- [ ] **内核单元测试通过** (`cargo build --features unit-test`)
- [ ] **整机启动测试通过** (`./test/run.sh`)
- [ ] **文档已更新** (roadmap.md 等)
- [ ] **提交信息清晰** (遵循提交规范)
- [ ] **遵循 Linux ABI** (不创新标准)
- [ ] **代码审查完成** (自我审查或同行审查)

## 常见错误

### ❌ 错误做法

1. **只编译不测试**
   - 编译通过 ≠ 功能正确
   - 必须运行测试验证

2. **跳过文档更新**
   - roadmap.md 中的问题未标记
   - 未来无法追踪问题状态

3. **提交信息不清晰**
   - "fix bug" - 太简略
   - "update" - 无具体内容
   - 应该说明修改了什么、为什么、如何验证

4. **违反"不创新"原则**
   - 自己设计接口
   - 修改 Linux 标准行为
   - 必须完全兼容 Linux ABI

### ✅ 正确做法

1. **完整测试流程**
   ```bash
   make build           # 编译
   make test            # 运行内核测试
   ./test/run.sh        # 整机测试
   ```

2. **及时更新文档**
   - 每次修复问题后更新 roadmap.md
   - 完成功能后更新进度
   - 重大变更更新 design.md

3. **清晰提交信息**
   ```
   type: 简短描述（50 字符内）

   ## 详细说明
   - 修改点 1
   - 修改点 2

   ## 验证
   - ✅ 测试通过

   Co-Authored-By: Claude Opus 4.6
   ```

4. **严格遵循标准**
   - 参考 Linux 内核源码
   - 使用 Linux 系统调用号
   - 遵循 POSIX 标准

## 相关文档

- [CLAUDE.md](../../CLAUDE.md) - AI 助手开发指南
- [design.md](../architecture/design.md) - 设计原则
- [roadmap.md](../progress/roadmap.md) - 开发路线图
- [testing.md](testing.md) - 测试指南
- [testing.md](testing.md) - 测试指南

## 版本历史

- **2026-03-04**: 大幅更新文档
  - 更新内核单元测试信息（51 个测试模块）
  - 添加用户态兼容性测试（mini-ltp）章节
  - 更新测试体系总览
  - 修正过时的示例和路径
- **2026-02-08**: 创建文档，记录标准开发流程
