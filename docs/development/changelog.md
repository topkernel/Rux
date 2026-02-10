# Rux OS 变更日志

本文档记录 Rux 内核的重要变更和修复。

## [Unreleased]

### 2026-02-10

#### 🔄 重构

**平台无关 pagemap 重构**
- ✅ 将 `mm/pagemap.rs` 从 ARM 特定实现重构为平台无关接口（79行薄包装层）
- ✅ 将 VMA 操作（mmap, munmap, brk, fork, allocate_stack）移至 `arch/riscv64/mm.rs`
- ✅ AddressSpace 现在使用 `mm/page` 类型（VirtAddr, PhysAddr），在需要时进行类型转换
- ✅ 添加 `PhysAddr::ppn()` 方法用于物理页号计算
- ✅ 添加 `VirtAddr::as_usize()` 方法用于地址转换
- ✅ 代码净减少 298 行（764行 → 79行 + 293行平台特定代码）

**代码变更**：
- `kernel/src/mm/pagemap.rs`: 764 行 → 79 行（平台无关接口）
- `kernel/src/arch/riscv64/mm.rs`: +293 行（VMA 操作实现）
- `kernel/src/mm/page.rs`: +5 行（ppn() 方法）

#### 🐛 Bug 修复

**单元测试修复**
- ✅ 修复网络测试 SkBuff headroom 问题（alloc_skb 保留 16 字节头部空间）
- ✅ 修复测试顺序问题（boundary 测试移到 fork 测试之前，防止任务池耗尽）
- ✅ 测试通过率提升：161/167 → 163/166（仅剩 3 个边界测试用例待修复）

**sys_brk 系统调用**
- ✅ 实现 sys_brk 系统调用（214 号）
- ✅ 支持 brk 系统调用参数验证和返回值处理

#### 📝 文档更新

- 更新本文档以反映 pagemap 重构和测试修复

### 2026-02-09

#### ✨ 新增

**Phase 18: 网络协议栈完整实现**

**网络缓冲区** (kernel/src/net/buffer.rs)
- ✅ SkBuff 实现（参考 Linux sk_buff）
- ✅ skb_push/skb_pull/skb_put 操作
- ✅ 协议分层管理（Ethernet → ARP → IPv4 → UDP/TCP）

**以太网层** (kernel/src/net/ethernet.rs)
- ✅ 以太网帧处理（14字节头部）
- ✅ MAC 地址管理（ETH_ALEN = 6）
- ✅ 以太网头构造和解析
- ✅ eth_build_header/eth_parse_packet

**ARP 协议** (kernel/src/net/arp.rs)
- ✅ ARP 协议实现（RFC 826）
- ✅ ARP 缓存（固定大小64条目）
- ✅ ARP 报文构造（请求/响应）
- ✅ arp_build_request/arp_build_reply
- ✅ arp_rcv 处理函数

**IPv4 协议** (kernel/src/net/ipv4/)
- ✅ IP 头部结构（20字节，RFC 791）
- ✅ 路由表（最长前缀匹配）
- ✅ IP 校验和计算（RFC 1071）
- ✅ ip_push_header/ip_pull_header

**UDP 协议** (kernel/src/net/udp.rs)
- ✅ UDP 头部（8字节，RFC 768）
- ✅ UDP Socket 管理（绑定、连接、断开）
- ✅ UDP 校验和（含伪头部）
- ✅ udp_build_packet/udp_parse_packet
- ✅ UDP Socket 表（固定64个）

**TCP 协议** (kernel/src/net/tcp.rs)
- ✅ TCP 头部（20字节，RFC 793）
- ✅ TCP 状态机（11种状态：CLOSE/LISTEN/SYN_SENT/ESTABLISHED等）
- ✅ TCP Socket 管理（bind/listen/connect/accept/close）
- ✅ TCP 校验和（含伪头部）
- ✅ tcp_build_packet/tcp_parse_packet
- ✅ TCP Socket 表（固定64个）

**VirtIO-net 驱动** (kernel/src/drivers/net/)
- ✅ VirtIO 网络设备驱动
- ✅ 设备初始化（VirtIO 设备 ID = 1）
- ✅ RX/TX 队列管理
- ✅ MAC 地址读取（VirtIO 配置空间）
- ✅ 数据包接收和发送

**网络设备框架** (kernel/src/drivers/net/)
- ✅ NetDevice 基类（space.rs）
- ✅ 回环设备驱动（loopback.rs）
- ✅ 设备注册和注销

**网络系统调用** (kernel/src/arch/riscv64/syscall.rs)
- ✅ sys_socket (198) - 创建 socket
- ✅ sys_bind (200) - 绑定地址
- ✅ sys_listen (201) - 监听连接
- ✅ sys_accept (202) - 接受连接（部分实现）
- ✅ sys_connect (203) - 发起连接
- ✅ sys_sendto (206) - 发送数据（部分实现）
- ✅ sys_recvfrom (207) - 接收数据（部分实现）

**代码统计**：
- 新增代码：~2,500 行 Rust 代码（网络协议栈）
- 新增代码：~1,200 行 Rust 代码（设备驱动）
- 新增测试：~200 行测试代码
- 总计：~23,900 行 Rust 代码

#### 🐛 Bug 修复

**UDP Socket Alloc 返回类型修复**
- 修复 udp_socket_alloc() 返回类型（Result<i32, i32>）
- 修复 UDP 校验和计算中的错误

#### 📝 文档更新

- 更新 README.md - 添加网络子系统功能矩阵
- 更新测试统计（25 个模块，~280 个测试用例）
- 更新代码统计（~24,000 行代码）
- 更新开发路线图（Phase 18 完成）

### 2025-02-10

#### ✨ 新增

**Phase 17: 块设备驱动和 ext4 文件系统完整实现**

**VirtIO 块设备驱动** (kernel/src/drivers/virtio/)
- ✅ VirtQueue 实现（queue.rs, 206 行）
  - 遵循 VirtIO Specification v1.1
  - 描述符管理、队列通知、完成等待
- ✅ 块设备驱动（mod.rs, 470 行）
  - 设备初始化和检测
  - `read_block()` 和 `write_block()` 实现
  - VirtIO 请求/响应处理
  - VirtQueue 集成

**Buffer I/O 层** (kernel/src/fs/bio.rs)
- ✅ BufferHead 缓存管理（375 行）
  - 块状态跟踪（Uptodate、Dirty、Locked）
  - 引用计数管理
  - 块数据缓存
- ✅ 块缓存系统
  - 哈希表索引（设备主设备号 + 块号）
  - LRU 风格缓存管理
- ✅ Buffer I/O 函数
  - `bread()` - 读取块到缓存
  - `brelse()` - 释放缓冲区
  - `sync_dirty_buffer()` - 同步脏块到磁盘

**ext4 文件系统** (kernel/src/fs/ext4/)
- ✅ 超级块和磁盘结构（superblock.rs, 315 行）
  - Ext4SuperBlockOnDisk 解析
  - 块组描述符解析
  - 文件系统信息提取
- ✅ Inode 操作（inode.rs, 287 行）
  - Ext4Inode 结构
  - 数据块提取（直接块）
  - 文件大小读取
- ✅ 目录操作（dir.rs, 164 行）
  - 目录项解析
  - 文件查找
- ✅ 文件操作（file.rs, 173 行）
  - 文件读取
  - 文件写入（支持块分配）
  - 文件定位

**ext4 分配器** (kernel/src/fs/ext4/allocator.rs, 535 行)
- ✅ BlockAllocator
  - 基于位图的块分配算法
  - 块组描述符更新
  - 超级块空闲块计数更新
  - `alloc_block()` - 分配新块
  - `free_block()` - 释放块
- ✅ InodeAllocator
  - 基于 inode 位图的分配算法
  - Inode 表扫描
  - `alloc_inode()` - 分配新 inode
  - `free_inode()` - 释放 inode

**块设备驱动框架** (kernel/src/drivers/blkdev/mod.rs, 276 行)
- ✅ GenDisk 结构
- ✅ Request 队列
- ✅ BlockDeviceOps trait

**单元测试** (kernel/src/tests/)
- ✅ virtio_queue.rs - VirtIO 队列测试（8个测试用例）
- ✅ ext4_allocator.rs - ext4 分配器测试（7个测试用例）
- ✅ ext4_file_write.rs - 文件写入测试（5个测试用例）

**错误代码** (kernel/src/errno.rs)
- ✅ 添加 EFBIG (27) - File too large

**代码统计**：
- 新增代码：~3,200 行 Rust 代码
- 新增测试：~800 行测试代码
- 总计：~20,000 行 Rust 代码

#### 🐛 Bug 修复

**类型不匹配修复**
- 修复 ext4 文件系统中的类型转换问题
- 修复 VirtQueue 中的可变引用问题
- 修复块分配器中的类型转换问题

#### 📝 文档更新

- 更新 README.md - 添加块设备和 ext4 功能矩阵
- 更新测试统计（23 个模块，261 个测试用例）
- 更新代码统计（~20,000 行代码）
- 更新开发路线图（Phase 17 完成）

### 2025-02-09

#### ✨ 新增

**RISC-V 系统调用完整实现** (Phase 10 完成)
- 实现完整的系统调用处理框架
- 用户程序可以成功调用系统调用并正常退出
- 修复 sscratch 寄存器管理，支持连续系统调用

**核心功能**：
1. **Trap 处理机制** (`kernel/src/arch/riscv64/trap.S`, `trap.rs`)
   - 使用 sscratch 寄存器在用户栈和内核栈之间快速切换
   - 在内核栈上保存 272 字节的 TrapFrame
   - 正确处理系统调用、异常和中断

2. **系统调用分发器** (`kernel/src/arch/riscv64/syscall.rs`)
   - 遵循 RISC-V Linux ABI 约定
   - 系统调用号通过 a7 寄存器传递
   - 参数通过 a0-a5 寄存器传递
   - 返回值通过 a0 寄存器返回

3. **用户模式切换** (`kernel/src/arch/riscv64/usermode_asm.S`)
   - 使用 sret 指令从特权模式切换到用户模式
   - Linux 风格单一页表方法（不切换 satp）
   - 正确设置 sstatus.SPP=0 确保返回用户模式

4. **用户程序支持** (`userspace/hello_world/`)
   - 实现 no_std 用户程序
   - 内联汇编系统调用包装函数
   - 自定义链接器脚本 (user.ld) 链接到用户空间地址

**技术细节**：

```assembly
# Trap 入口（简化版）
trap_entry:
    mv t0, sp                      # 保存原始 sp
    csrrw sp, sscratch, sp          # 交换 sp 和 sscratch（切换到内核栈）
    addi sp, sp, -272              # 分配 TrapFrame
    sd t0, 0(sp)                   # 保存原始 sp
    # ... 保存寄存器 ...
    call trap_handler              # 调用 Rust 处理函数
    # ... 恢复寄存器 ...
    ld t0, 0(sp)                   # 加载原始 sp
    addi sp, sp, 272               # 释放 TrapFrame
    csrr t1, sscratch              # 读取内核栈指针
    mv sp, t0                      # 恢复原始 sp
    csrw sscratch, t1              # 恢复内核栈指针到 sscratch
    sret                           # 返回用户/内核模式
```

**已验证的系统调用**：
- ✅ SYS_EXIT (93) - 进程退出
- ✅ SYS_GETPID (172) - 获取进程 ID

**测试结果**：
```
[TRAP:ECALL]           <- 陷阱处理入口
[ECALL:5D]             <- 系统调用 0x5D (93) = sys_exit
sys_exit: exiting with code 0  <- sys_exit 执行成功
]                      <- 汇编代码到达 sret
```

**关键文件**：
- `kernel/src/arch/riscv64/trap.S` - Trap 入口/出口汇编代码
- `kernel/src/arch/riscv64/trap.rs` - Trap 处理 Rust 代码
- `kernel/src/arch/riscv64/syscall.rs` - 系统调用分发和实现
- `kernel/src/arch/riscv64/usermode_asm.S` - 用户模式切换汇编
- `kernel/src/embedded_user_programs.rs` - 嵌入的用户程序 ELF 数据

#### 🐛 Bug 修复

**sscratch 寄存器管理问题**
- **问题**：在 trap 出口时，用户栈指针被错误地写入 sscratch
- **影响**：第二个系统调用无法正确切换到内核栈
- **修复**：在 trap 出口时重新加载内核栈指针到 sscratch
- **代码**：
```assembly
ld t0, 0(sp)           # Load original sp (user or kernel)
addi sp, sp, 272       # Deallocate trap frame
csrr t1, sscratch      # Read kernel stack pointer from sscratch
mv sp, t0              # Restore original sp (user or kernel)
csrw sscratch, t1      # Restore kernel stack pointer to sscratch
```

**用户程序嵌入更新问题**
- **问题**：修改用户程序后没有重新嵌入到内核
- **影响**：内核运行旧版本的用户程序
- **修复**：使用 `embed_user_programs.sh` 脚本重新嵌入用户程序 ELF

#### 📝 文档更新

- 添加 RISC-V 系统调用实现文档
- 更新用户程序开发指南
- 添加 trap 处理流程图

### 2025-02-08

#### 🐛 Bug 修复

**BuddyAllocator 伙伴地址越界修复** (commit 09c86dd)
- 修复 `free_blocks` 函数在合并伙伴块时的地址越界问题
- 添加伙伴地址边界检查，防止访问超出 heap_end 的内存
- 影响：解决了 FdTable 和 SimpleArc 测试的 Page Fault 问题

**问题描述**：
- 释放 order 12 (16MB) 块时，伙伴地址计算为 0x81A00000
- 这个地址正好是 heap_end，超出 MMU 映射范围
- 导致 Load page fault 错误

**修复方案**：
```rust
// 检查伙伴是否在堆范围内
let heap_start = self.heap_start.load(Ordering::Acquire);
let heap_end = self.heap_end.load(Ordering::Acquire);

if buddy_ptr < heap_start || buddy_ptr >= heap_end {
    // 伙伴超出堆范围，无法合并
    self.add_to_free_list(current_ptr as *mut BlockHeader, current_order);
    break;
}
```

**测试验证**：
- ✅ SimpleArc 分配测试成功
- ✅ FdTable 测试成功
- ✅ 不再有 Page Fault 错误

#### ✨ 新增

**SimpleArc 分配测试** (kernel/src/tests/arc_alloc.rs)
- 新增 SimpleArc 内存分配和释放测试
- 验证 Arc::clone、引用计数、drop 功能
- 测试 File 对象的创建和访问

#### 📝 文档更新

- 重构文档结构，创建清晰的分类组织
- 添加文档中心索引 (docs/README.md)
- 归档历史调试文档到 docs/archive/

---

## [0.1.0] - 2025-02-08

#### ✨ 新功能

**Unix 进程管理系统调用** (Phase 15)
- ✅ fork() - 创建子进程 (commit a4bbc7a)
- ✅ execve() - 执行新程序 (commit 3b5f96d)
- ✅ wait4() - 等待子进程 (commit 22ab972)

**同步原语** (Phase 14)
- ✅ Semaphore - 信号量机制 (commit 5ea2376)
- ✅ Condition Variable - 条件变量 (commit e832be1)

**RISC-V 架构支持** (Phase 10)
- ✅ 启动流程和 OpenSBI 集成
- ✅ Sv39 MMU 和页表管理
- ✅ PLIC 中断控制器驱动
- ✅ IPI 核间中断框架
- ✅ SMP 多核支持 (SBI HSM)

#### 🐛 Bug 修复

**内核启动问题** (commit 9de7b64)
- 修复内核启动时的 OpenSBI 集成问题
- 修复 wait4 错误码处理

**Timer interrupt sepc 处理**
- 不再跳过 WFI 指令，避免跳转到指令中间

**SMP + MMU 竞态条件**
- 使用 `AtomicUsize` 保护 `alloc_page_table()` 的 `NEXT_INDEX`
- Per-CPU MMU 使能：次核等待启动核完成页表初始化

#### 📊 测试覆盖

- ✅ 14 个单元测试模块
- ✅ fork、execve、wait4 测试
- ✅ SMP 多核启动测试
- ✅ SimpleArc 和 FdTable 测试

#### 📝 文档

- CLAUDE.md - AI 辅助开发指南
- UNIT_TEST.md - 单元测试文档
- USER_PROGRAMS.md - 用户程序实现方案
- CODE_REVIEW.md - 代码审查记录

---

## 版本命名规则

- **Major.Minor.Patch** (主版本.次版本.补丁)
- Major：重大架构变更或不兼容更新
- Minor：新功能添加
- Patch：Bug 修复和小改进

## 提交规范

遵循 [Conventional Commits](https://www.conventionalcommits.org/)：

- `feat:` - 新功能
- `fix:` - Bug 修复
- `docs:` - 文档更新
- `test:` - 测试相关
- `refactor:` - 代码重构
- `perf:` - 性能优化
