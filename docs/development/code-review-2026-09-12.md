# Rux Kernel 代码检视 — 2026-09-12（第三轮）

> **配套修复计划**：[fix-plan-2026-09-12.md](fix-plan-2026-09-12.md)（Wave 1–9 分批任务表、验收标准、依赖关系、跟踪状态）
>
> Scope: 全仓库 — 内核 436 个 Rust/汇编文件（约 10.6 万行）、全部 64 份文档、测试与构建体系
> Reference: Linux 6.x（/home/william/refer/linux/）、virtio 1.1 spec、RFC 793/1122、POSIX
> Previous: `docs/development/code-review-2026-04-17.md`（第二轮，全部 FIXED/KNOWN）、`docs/archive/code-review-2026-04-15.md`（第一轮）
> Method: 14 个并行深度审查（逐文件逐行），历史 FIXED/KNOWN 条目不重复报告；全部发现经调用路径验证，Top 发现做了源码/二进制（readelf/objdump）双重抽验
> 本轮重点：最近 5 个 commit（vfork、COW mapcount、SMP 启动竞态等）引入的新回归 + 尚未被发现的新问题

## Severity Definitions

| Level | Meaning |
|-------|---------|
| **Critical** | 数据丢失、安全漏洞、系统挂死/panic、静默内存损坏 |
| **High** | 用户可见的错误行为、POSIX/ABI 违反、概率性挂死/UAF |
| **Medium** | 有限范围的逻辑错误、缺失边界/语义偏差 |
| **Low** | 细节、性能、一致性 |

---

## 0. 总览

| 子系统 | Critical | High | Medium | Low | 小计 |
|---|---|---|---|---|---|
| arch/riscv64 | 2 | 4 | 6 | 10 | 22 |
| mm | 3 | 7 | 12 | 8 | 30 |
| process + sched | 4 | 7 | 17 | 7 | 35 |
| syscall（文件/IO/内存） | 3 | 7 | 23 | 10 | 43 |
| syscall（进程/信号/网络/时间） | 2 | 15 | 24 | 11 | 52 |
| fs 核心层 + devfs/procfs | 1 | 8 | 18 | 14 | 41 |
| ext4 + jbd2 | 4 | 11 | 10 | 9 | 34 |
| net | 4 | 6 | 15 | 12 | 37 |
| drivers | 2 | 4 | 8 | 9 | 23 |
| ipc/sync/signal/interrupt | 5 | 9 | 19 | 10 | 43 |
| io_uring/dfx/security/printk | 1 | 5 | 10 | 18 | 34 |
| tests + build/CI | 3 | 6 | 10 | 12 | 31 |
| docs（64 份） | — | 25 | 19 | 9+36 链接 | 53+ |
| **合计（去重前）** | **34** | **~114** | **~191** | **~139** | **~478** |

> 跨子系统重复报告已在本报告中合并标注（见各条 "同源"）。实际独立问题约 440 项，其中 Critical 34 项（去重后约 30 项）。

---

## 1. Top 15 最高优先级问题（跨子系统排序）

1. **[ARCH-C1] uaccess.S 异常表系统性错位一条指令**（`arch/riscv64/uaccess.S:142-347`）
   `EXTABLE` 宏在指令**之后**定义标签，标签绑定到下一条指令。`__copy_from_user`/`__clear_user` 的真实故障访存指令全部未被异常表覆盖，`fixup_exception()` 精确匹配必然落空 → 任何"地址范围合法但未映射"的用户指针（含 NULL，USER_START=0）传入 read/execve/strncpy_from_user 等即触发内核态缺页 → KernelPanic → wfi 死循环或永久故障循环。**单个坏用户指针一步挂死整机**。已 readelf/objdump 二进制实证；`__copy_to_user` 因错位"歪打正着"（store 恰被覆盖）掩盖了问题。修复：EXTABLE 移到被覆盖指令之前（Linux 风格向后引用），并加表项↔访存指令一一对应的校验脚本。

2. **[ARCH-C2] trap.S 入口在保存 t0/t1/t2 之前就将其 clobber**（`trap.S:210-212, 232-252, 491-510`）
   公共分发、用户路径、内核路径都在 GPR 保存前使用 t0-t2 做栈选择/地址判定，随后才 `sd x5/x6/x7`——存入的是垃圾。定时器中断每秒百次地随机污染用户程序与内核 Rust 代码（LLVM 重度使用 t0-t2）的临时寄存器。**极可能就是提交历史中反复出现的"概率性"崩溃的根因**。已二进制实证（PT_T0=0x1FFFF 等）。修复：仿 Linux，进入后第一时间只用 `sd xN, PT_xN(sp)` 保存全部 GPR，栈选择逻辑移到保存之后。

3. **[VFS-C1] open()/路径遍历零 DAC 权限检查**（`fs/vfs.rs:1111-1199, 347-489`）
   `file_open()` 从不检查 MAY_READ/MAY_WRITE；`path_lookup()` 遍历目录组件从不检查 MAY_EXEC（search 位）；全内核 `generic_permission` 仅 3 处调用（父目录写、truncate、faccessat），`Inode::op_permission` 零调用者。**任意非特权用户可 `open("/etc/shadow", O_RDWR)`**；O_RDONLY 打开的 fd 调 write() 也会成功。修复点集中：path_lookup 每目录组件 + file_open 按 O_ACCMODE + read/write 按 f_mode。

4. **[SYSA-C1] read/write 全家对用户可控 count 直接堆分配**（`syscall/io.rs:83,145,250,774`、`syscall/file.rs:281-285`）
   `vec![0u8; count]`，count 最大 256GB（access_ok 只查范围）→ 分配失败 → `alloc_error_handler` panic。一次 `read(0, buf, 0x4000_0000)` 击杀整机。Linux 截断到 MAX_RW_COUNT(2GB)。io_uring 路径同病（`io_uring/mod.rs:587,621`）。

5. **[PROC-C1] 恶意 ELF 的 p_offset/p_filesz 无边界校验 → 内核 panic**（`process/exec.rs:177`）
   `&program_data[offset..offset+file_size]` 直接切片，段头字段来自用户可控文件。同病：解释器(PT_INTERP)的 e_phoff 也未校验（`fs/elf.rs:405-411`，可达内核内存泄露/OOB 读）；init 路径 `init.rs:267` 同样。Linux 返回 ENOEXEC 绝不 panic。

6. **[NET-C1/C2] 网络栈收发双向从未真正工作过**
   - RX：`ethernet_rcv()` 从不调用 `eth_pull_header()`（定义于 ethernet.rs:123，全仓库零调用）——所有入站报文带着 14 字节以太网头进入 ip_rcv/arp_rcv，在错误偏移解析后全部静默丢弃。
   - TX：`SkBuff::alloc` headroom 仅 16 字节（buffer.rs:149），TCP/IP/ETH 需 54 字节——所有 TCP 段与 ICMP 回复的 `skb_push` 必然失败，**只有 ARP 能发出**；每次失败还泄漏 ~1.5KB skb（SkBuff 无 Drop）。
   - 配套：ethertype 写入缺 to_be()（H1）、TCP/UDP 校验和伪头读法不一致（H2/H3）、握手 ISN 未转换字节序（H4）。前两轮的"FIXED"均未经真实收发验证（单元测试只测常量）。

7. **[NET-C3] syscall 层用进程 fd 直接索引全局 socket 表**（`net/socket.rs:572` + `syscall/network.rs` 12 处）
   `get_socket(fd)` 把进程 fd 当全局槽位号；stdio 占 0-2，第一个 socket fd=3 而槽位=0 → listen/connect/accept/sendto/recvmsg 等**绝大多数 socket 系统调用 EBADF 或操作到别的进程的 socket**。只有 bind 走 `get_socket_from_fd`（正确范本）。

8. **[EXT4-C2/C3/C4] ext4 常规操作即静默损坏文件系统**
   - truncate 释放 extent 物理块但不删除/收缩 extent 表项（mod.rs:1457-1485）→ 截断后再扩展写入**已释放并可能复用给其他文件的块**（跨文件数据损坏，无任何报错）。
   - 快速符号链接 unlink 把目标字符串字节当块号释放（namei.rs:1553-1558）——`ln -s sh x; rm x` 即释放一个活块。
   - extent 文件的洞写入直接以直接块指针覆写 extent 头（file.rs:250-281）——ftruncate 扩展后 write 即毁掉 extent 头。
   - 另：extent 树无递归深度上限/缓冲边界校验（C1，恶意镜像 OOB 读+栈溢出）；fs 块大小≠4096 的镜像读写错位毁盘（H1）；超级块几何零校验多处除零 panic（H2）。

9. **[SYSA-C2] mremap 搬移后旧数据全部丢失**（`syscall/memory.rs:707-739` + `mm_ops.rs:82-96`）
   新映射是 lazy 的（只建 VMA 不映射页），`copy_old_to_new_pages` 要求新地址已有 PTE → 一页都拷不到就 munmap 旧区 → 新区域首次访问 fault 出**零页**。glibc/musl 的 realloc 走此路径，静默内存损坏。

10. **[DRIV-C1/C2] MMIO virtio 设备队列编程整体写错**
    - virtio-blk MMIO：把 PCI common-config 偏移（0x20/0x28/0x30/0x1c）写进 MMIO 寄存器空间，QueueReady(0x44) 永未置位，init 却返回 Ok（virtio/mod.rs:232-268）→ MMIO-only 配置无法挂根文件系统。
    - virtio-net MMIO：avail/used 环地址显式写 0、desc 指向一块死表（virtio_net.rs:187-257）→ 收发完全不工作，且每次发包在关中断+持锁状态下自旋数秒后**假报成功**。
    - PCI 路径：特性协商全盘接受 EVENT_IDX 而 used_event 是堆垃圾（H2，"random timeout" 5 次重试补丁的根因）；INTX IRQ 号差一条线（H3）；异步读缺 F_WRITE 必报 EIO（H4）。

11. **[IOU-C1] io_uring_enter/register 不校验 fd 类型 → 类型混淆任意内核读写**（`io_uring/mod.rs:852-893`）
    pipe/eventfd/mem 文件的 private_data 被直接强转为 IoUring，向假 ring 内指针 read/write_volatile——一句话可获得任意内核读写或立即打崩。F15-03 修复（close 校验 ops）的遗漏面；mmap 路径有校验，enter/register 没有。配套：enter/close 并发 UAF（H1）、ring 物理页永不释放（H2）。

12. **[PROC-P02] vfork 双丢失唤醒竞态（commit fddcac2 新引入）**（`process/fork.rs:381-397`）
    子进程先 `enqueue_task` 后登记 `vfork_parent`；且父进程先 set_state(UNINTERRUPTIBLE) 后 schedule 无复查。SMP 下子进程先 exec → 唤醒落空 → 父进程**永久 UNINTERRUPTIBLE 睡眠**，进程树挂死。musl posix_spawn 默认走 vfork。Linux 用 completion 免疫。

13. **[IPC-C3/C1] futex/sem 核心等待原语大面积失效**
    - `FUTEX_WAKE/REQUEUE` 超过 8 个等待者静默摘链不唤醒（futex.rs:158-212）——`pthread_cond_broadcast` 在 >8 等待者时批量永久挂死线程。
    - semop 阻塞语义完全反转（sysv_sem.rs:686-697）：普通阻塞 P 返回 EINVAL，IPC_NOWAIT 反而永久阻塞——SysV 信号量等待路径完全不可用。
    - 超时永不生效：semtimedop/mq_timed*/futex(timeout) 都没有 armed 定时器，带超时的等待全部永久挂起（H3）。
    - futex waiter 池并发双分配（C2）、唤醒后 slot UAF（H4）。

14. **[MM-C3] swap-out 对共享页释放仍被引用的 swap slot**（`mm/vmscan.rs:263-286`）
    fork 后 COW 页（refcount=2）换出后 slot 被释放但两个 PTE 仍持有 entry → slot 复用后缺页读到**别的页的数据**（静默交叉污染）+ 物理页泄漏。配套：compaction 路径裸物理地址解引用必 panic（C1，page_desc.rs:547）、迁移后 PTE 装不回数据变零页（C2）、munmap 只清 PTE 不 put_page（页泄漏，arch H3/mm H4 同源）、fork 不复制 swap entry 且对 SHM 页错误 COW（mm H5）。

15. **[DOCS] 文档体系与代码大面积脱节**
    - `docs/architecture/kernel-lock.md` 整份描述**已彻底删除的 Kernel Big Lock** 为现行机制（代码零引用、trap.S 无 amoswap）；riscv64.md/roadmap.md 同病。
    - README/structure/unit-test-report 的数字四处矛盾（348 syscalls 实为 344；825 cases vs 901 PASS+94 SKIP vs 静态 1451 断言点；3,777 总数算术不自洽；代码行数四个版本）。
    - CLAUDE.md（AI 协作入口）3 个失效链接 + Phase 36（现 52）+ "203 test cases"。
    - getting-started 的 mini-ltp/RELEASE=1/distclean//bin/shell 全部不可复现；调试指南的持久日志描述与被注释掉的代码相反。
    - SPIN 验证的 LTL 属性从未被全部验证（Makefile 不传 -N，声称 8 实际约 3）；825 个内核单测无 CI、无自动判定；测试中存在整文件的恒真假测试（tcp_handshake.rs、mem_cow.rs、smp.rs）。

---

## 2. Arch / Boot（kernel/src/arch/riscv64/ + sbi.rs，22 项）

### Critical
- **ARCH-C1** 异常表错位（见 Top 1）。`uaccess.S:142-347`、`exception.rs:88-93` 精确匹配。
- **ARCH-C2** trap 入口先 clobber 后保存 t0/t1/t2（见 Top 2）。`trap.S:210-212, 232-252, 491-510`。

### High
- **ARCH-H1** FPU 只保存不恢复：`context.rs:263-307` 切出时保存 f0-f31，切入侧从不 restore（全仓库无调用者），FP 首用陷阱也不重载 → 跨任务浮点污染 + 信息泄漏。Linux `fstate_save(prev)` 后无条件 `fstate_restore(next)`。
- **ARCH-H2** vfork 丢失唤醒（同 PROC-P02）。
- **ARCH-H3** munmap/MAP_FIXED 只清 PTE 不 put_page（同 MM-H4）：每次解除映射泄漏物理页，musl 大块 malloc/dlclose 长期运行耗尽内存。
- **ARCH-H4** do_exit 在仍运行于该页表时同步释放根页表（`exit.rs:110-124`）：释放-切换窗口内任何非全局映射 TLB miss 遍历已回收页表 → 真 SMP 必炸（当前被 tcg 单线程掩盖）。Linux 用 active_mm/mmdrop 延迟。

### Medium
- **ARCH-M1** 内核态非法指令/未知异常静默跳过 4 字节（`trap.rs:215-221`）——掩盖内核 bug；`print_cpu_info` 在 S 态读 M 态 CSR 每次触发并被跳过，启动日志打印垃圾 HART ID。
- **ARCH-M2** 嵌套 trap 落在 per-CPU 中断栈时覆盖外层 pt_regs（`trap.S:236-268`，early 路径有检查、正常路径没有）。
- **ARCH-M3** munmap 不支持 VMA 分割且跨 VMA 范围只移除第一个（`mm_ops.rs:374-409`）→ 已 munmap 内存被缺页"复活"为零页（同 SYSA-H1）。
- **ARCH-M4** 栈扩展缺"接近 SP"检查（`page_fault.rs:69-109`）→ 深度越界写被静默扩张栈而非 SIGSEGV（Linux 检查 address >= sp-65536）。
- **ARCH-M5** fork COW 降级仅本地 sfence（`mm_ops.rs:1084`）→ 其他 CPU 残留可写 TLB 项继续写 COW 页，无需缺页竞态即静默损坏（已知 PTL 限制的独立后果，有简单缓解：广播 TLB 失效）。
- **ARCH-M6** ecall 指令长度判定错误 + 无 SUM 直读用户指令（`trap.rs:348-373`）：4 对齐处的 c.ecall 被按 4 字节跳过；epc%4!=0 时裸读用户地址 → 结合 C1 同样挂死。

### Low（摘要）
- L1 `strncpy_from_user` 空字符串返回 EFAULT（execve("") 应 ENOENT）；L2 `send_ipi` 忽略 SBI 失败；L3 arch 版堆分配 `copy_thread` 死代码与现行 fork 语义冲突；L4 `map_region` 非对齐 start 下溢；L5 KERNEL_STACK_SIZE 硬编码三处；L6 LR 清除 `sc.d x0,t2,(t2)` 写入的是地址值非 0；L7 文件页缺页借道 f_pos（CLONE_FILES 竞态）；L8 brk 收缩不解除映射；L9 `Perm::None` 生成 R 位；L10 `.ex_table` 仅 ALIGN(4)。
- Info：ASID 基础设施整套未使用（switch_mm 恒 ASID=0+全量 sfence，正确但慢）；内核线性映射 RWX 无 W^X；`map_page` 每页全量 sfence。

---

## 3. 内存管理（kernel/src/mm/，30 项）

### Critical
- **MM-C1** `page_desc.rs:547` `copy_page_contents` 裸物理地址解引用（漏掉 phys_to_virt）→ compaction 一旦真正执行即内核缺页 panic。
- **MM-C2** `compact.rs:293-424` 迁移后 `remap_page` 以 walk 到有效 PTE 为门控，但 unmap 已把 PTE 清零 → 新 PTE 永远装不回，用户数据被静默替换为零页 + dst 页泄漏。
- **MM-C3** `vmscan.rs:263-286` 共享页 swap-out 释放仍被引用的 swap slot（见 Top 14）+ 物理页泄漏。

### High
- **MM-H1** `vma.rs:346-351` VMA 合并忽略 file_fd/offset → 不同文件的相邻 mmap 被合并，前一文件范围按后一文件的 fd/offset 读页（读错数据）。Linux vma_merge 要求 vm_file/vm_pgoff 一致。
- **MM-H2** `page_alloc.rs:52-69,164-177` + `zone.rs:455-577` buddy 页描述符状态迁移在 zone 锁外完成（分配侧 512 次原子写窗口/释放侧先改后锁）→ SMP 下空闲链表损坏、nr_free 下溢、双归属。Linux 在锁内完成整块。
- **MM-H3** `vmscan.rs:180-291` 回收全程无页锁无 refcount pin → 与缺页/退出/释放竞态（PFN 复用后写别人页进 swap）。
- **MM-H4**（交叉 arch）munmap 不 put_page（同 ARCH-H3）。
- **MM-H5**（交叉 arch）fork 不复制 swap entry（子进程访问换出页得零页）；MAP_SHARED/SysV shm 可写页被错误 COW 降级 → fork 后共享内存语义破坏。
- **MM-H6**（交叉 arch）fork 的 VMA 快照丢失 offset → 子进程文件映射按错误偏移读页。
- **MM-H7**（交叉 syscall）mprotect 重建 PTE 丢弃 COW 位且 PROT_WRITE 直接置 W → 绕过 COW，两进程写同一物理页。

### Medium（摘要）
- M1 expand_downwards 重叠检查范围错误 → 同键 insert 静默替换 VMA；M2 kswapd 丢唤醒（watermark 检查与 set_state 之间）→ 压力期睡过直到 OOM；M3 匿名缺页页不设 SwapBacked 不入 LRU → vmscan 跳过全部常规匿名页 → 有 swap 也不回收 → kswapd 连败 16 次直接 OOM kill；M4 回收扫描 O(全内存) 无游标；M5（交叉）进程退出不清理 swap entry（slot 永久泄漏）；M6 swap 区切磁盘尾部不检查 FS 占用 [待验证镜像布局]；M7 `main.rs:388` init_from_memblock 实参错误（地址当大小传）+ heap_size 双源；M8 分配失败无 direct reclaim（try_to_free_pages 死代码，缺页路径直接 OOM）；M9 swap type 编码位与 COW 软件位重叠；M10 first_online_node_mut &mut 别名 UB；M11 fork COW 中途 OOM 无回滚；M12 memblock add superset 情形重叠双计。
### Low：L1-L8（mm_users_dec 注释与行为不符、meminfo 恒 0、vmemmap 半初始化重试假成功、swap_free_slot 重复释放下溢、页描述符 Reserved 标注缺失、文档 refcount+2 过期、PhysAddr::ceil 溢出、rmap index 0 哨兵）。

---

## 4. 进程管理 + 调度（35 项）

### Critical
- **PROC-P01** 恶意 ELF p_offset/p_filesz 无校验 → panic（见 Top 5）。
- **PROC-P02** vfork 双竞态 → 父进程永久 D 状态（见 Top 12；commit fddcac2 引入）。
- **PROC-P03** do_exit 完全缺失孤儿过继（reparent）：全仓库无 forget_original_parent 等价物。父先于子退出 → 存活子进程的 parent 指针悬挂（ppid()/for_each_child 解引用野指针 UAF）+ 孤儿僵尸永久泄漏（PID 空间慢性耗尽）。Linux 过继给 init。
- **PROC-P04** commit 994b306 引入回归：deferred exit notify 读错 per-CPU 槽位（`sched.rs:746-763`）——`prev_cpu` 实为被唤醒任务当年被切出的 CPU（W），不是退出任务实际运行的 CPU（X）→ SIGCHLD/父唤醒可被**无限期延迟**（CPU-bound 任务长期占据时 do_wait 睡死、shell 挂起）。旧实现（切换后读 cpu_id()）其实是对的。修复：用 context.rs 已有的 per-CPU prev 记录取退出任务的 parent pid，消除 CPU 推断。

### High
- **P05** AT_RANDOM 指向 AT_NULL（16 字节全 0）且 rand 覆写 AT_PHDR 表前 16 字节（`exec.rs:395-542` 槽位算术 off-by-2）：stack canary 恒 0（F05-11 修复完全无效）、PT_INTERP 排首位时动态链接即崩。
- **P06** RT 优先级语义与 Linux 相反（`rt.rs:19` trailing_zeros 值小者优先）+ 无取值校验：`chrt -f 99` 是 RT 类内最低优先级；FIFO priority=0 不报 EINVAL 反而变最高。
- **P07** sched_setscheduler 改策略不做运行队列迁移：任务可同时挂两个类队列（两 CPU 同时 pick 同一 Task → 结构性灾难）或永远无法调度（FIFO→NORMAL 滞留 RT 链表挂死）。
- **P08** RT 任务退出导致 rt_nr_running 下溢（`rt.rs:174-216` 无 on_rq 守卫）→ RT 队列永久"非空"、idle 快路径失效。
- **P09** CFS 全局唯一 `cfs_rq.curr` 在 4 核下记账错乱：update_curr 更新"最后被 pick 的任务"，真正运行的任务 vruntime 停止增长 → 公平性完全失真、可饿死任务。
- **P10** setpriority 缺 can_nice：普通用户可把自己 nice 降到 -20。
- **P11** setuid exec 安全链断裂（`exec.rs:519` AT_SECURE 恒 0 + LD_PRELOAD 不清洗 + root 降 uid 不清 caps）——动态链接的 setuid-root 程序可被 LD_PRELOAD 劫持提权。配套 **syscall-b C-02**：execve 在加载成功前就提交 setuid 凭据，构造"半合法"setuid ELF 使加载中途失败即得 root 旧映像进程（本地提权）。

### Medium（摘要，17 项）
P12 wake_up 非原子（双唤醒 nr_running 膨胀、DL 双插入）；P13 do_waitid 漏掉 994b306 的僵尸复查（同族竞态）；P14 wait4 的 pid==0/pid<-pgid 语义未实现（收错进程）；P15 execve 清空信号阻塞掩码（违反 POSIX）；P16 DL 首个 tick 即被错误节流（exec_start 未初始化）；P17 DL 每次入队补满预算（CBS 带宽控制失效）；P18 do_clone 失败路径泄漏 PID + pid_hash 留野指针；P19 fork 不继承 nice/策略/亲和/comm（comm 未初始化，ps 显示乱码）；P20 sched_setaffinity 验证后即丢弃（返回成功但不生效）；P21 kill 组播无权限检查（同 syscall-b H-01）+ 睡眠组成员收不到；P22 内核线程退出成无主 ZOMBIE（Task/PID 泄漏）；P23 new_task_at 漏初始化 comm/kernel_stack_bottom/ti_a0-2；P24 release_task 不运行 Drop（exe_path Box 与 pending SigQueue 每进程泄漏）；P25 idle 标记与 enqueue 竞态（唤醒延迟至多一个 tick）；P26 execve 不检查 x 权限位；P27 prlimit64/setrlimit 设置成功但不持久（ulimit -n 无效）；P28 setgroups(0,NULL) 不清空补充组。
### Low：P29-P35（rr_get_interval 不校验策略、PRIO_PGRP/USER 未实现、setpgid 语义缺失、argv/envp 静默截断、唤醒 vruntime clamp 无容差、exit_group 只退出当前线程、CLONE_VM mm_users 只增不减）。

---

## 5. Syscall 层 — 文件/IO/内存（43 项）

### Critical
- **SYSA-C1** 用户可控 count 直接堆分配 → panic（见 Top 4）。
- **SYSA-C2** mremap 搬移数据全丢（见 Top 9）。
- **SYSA-C3** brk 收缩无下限保护（`memory.rs:67-84`）：`brk(4096)` 触发对低地址全段（含代码段）的 munmap + rmap 破坏（跨进程 COW UAF 可能）。Linux 低于 start_brk 直接忽略。

### High
- **H1** munmap 不支持部分解除 + mremap 缩小必失败 + MAYMOVE 旧区静默残留（VMA 无 split）。
- **H2** readlink 仅实现 /proc 特例，普通符号链接一律 ENOENT（与 VFS-H1 同源）——`ls -l`/realpath 全坏。
- **H3** resolve_user_path 完全忽略 dirfd（TODO 注释在案）：所有 *at 系统调用退化为相对 CWD，dirfd 无效也不 EBADF。
- **H4** close(fd≥512) 被 MQ 命名空间劫持：普通文件 fd 512-575 无法关闭，永久泄漏直至 EMFILE（与 VFS-H3 同源）。
- **H5** mmap 含 PROT_EXEC 一律映射 RWX（W^X 失效；`to_page_perm()` 已实现正确映射但未使用）。
- **H6** epoll_pwait2 把 timespec* 当毫秒超时（NR 441 委托旧接口）→ 传 {0,0} 的进程永久阻塞。
- **H7** splice 裸解引用用户 off 指针 + 不恢复文件位置 + 负偏移不拒。

### Medium（23 项，摘要）
M1 mkdir 丢弃 mode；M2 copy_file_range 忽略 off_in/off_out；M3 iovec 三缺陷（iov_size 回绕、无 IOV_MAX、NULL base 被跳过）；M4 fstat 直写用户内存（未走 copy_to_user）；M5 mincore 直写+EINVAL/EFAULT 混淆；M6 get_mempolicy nodemask 越界写（校验 9 字节写 16 字节）；M7 mprotect 三缺陷（未映射区返回 0、跨 VMA 只改第一个、未知位不校验）；M8 mmap 不校验 fd 有效性与 offset 对齐；M9 PROT_NONE 的 VMA 标记为可读；M10 MADV_DONTNEED/FREE 为 no-op（释放页数据残留）；M11 不可 seek 的 fd lseek 返回 EBADF（应 ESPIPE）；M12 pread/pwrite 非原子（CLONE_FILES 位置竞态）；M13 ioctl 不校验 fd 有效性；M14 utimensat(NR 88) 落到 futimesat（times/flags 全忽略，touch 无效）；M15 fchown 绕过 vfs_chown（不清 setuid 位、属主 chown 自己被拒）；M16 faccessat 两缺陷（未知 mode 位、AT_EACCESS 用 euid）；M17 statx 忽略 flags/mask；M18 renameat2 忽略 RENAME_NOREPLACE（静默覆盖，mv -n 数据丢失）；M19 fchmod 不校验 mode；M20 mknodat 类型与权限位；M21 fallocate 静默成功不做事；M22 futex2(NR 454-456) 参数布局错误委托旧 futex；M23 openat2 校验偏差（E2BIG/EINVAL/RESOLVE_* 假成功）。
### Low：L1-L10（perf_event_open 编号错位 240→241、syscall_no 截断、未知 syscall 刷屏、错误检查顺序、mount 忽略 source/flags、mlock 弱校验、move_pages status 未填、length+4095 回绕 4 处、FIONREAD 恒 0、mmap_framebuffer 固定地址冲突）。

---

## 6. Syscall 层 — 进程/信号/网络/时间/杂项（52 项）

### Critical
- **C-01** `sys_riscv_hwprobe` 在 S 态读 M 态 CSR（mvendorid/marchid/mimpid，`process.rs:2526-2540`）→ 非法指令 → 内核死循环挂死。glibc ≥2.38 启动即调用。一条 syscall 挂死整机。
- **C-02** execve 提前提交 setuid 凭据（`process.rs:265-338`）：加载失败时凭据已是 root 而旧映像继续运行 → 本地提权（与 P11 构成完整链条）。

### High（15 项）
- **H-01** kill(0)/kill(-pgid) 无权限检查（组播循环裸 send_signal）；**H-02** kill(-1) 被当作"进程组 1"；**H-03** getcpu 每指针写 16 字节（用户缓冲 4 字节，溢出 12 字节×2）；**H-04** tkill 无任何权限检查（与 kill/tgkill 不一致的旁路）；**H-05** SIGKILL/SIGSTOP 可被 sigprocmask 屏蔽（不可杀进程）；**H-06** sigsuspend 在投递前恢复旧掩码（等待的信号永不投递，循环空转）；**H-07** rt_sigtimedwait 不阻塞/不查真实 pending/不出队（musl sigwait 全坏）；**H-08** get_socket(fd) 命名空间错位（同 NET-C3）；**H-09** sendmmsg/recvmmsg 步长 60 应为 64（第 2 条起错位读写）；**H-10** socket() 不屏蔽 SOCK_CLOEXEC/SOCK_NONBLOCK（现代程序带 flag 创建即 ESOCKTNOSUPPORT）；**H-11** epoll/timerfd 不校验 fd 类型即转换 private_data（类型混淆内存破坏）；**H-12** sched_setattr 无 CAP_SYS_NICE 检查（任意 pid 提权 RT）；**H-13** 约 30 处系统调用裸解引用用户指针（access_ok 不查映射）→ 结合 ARCH-C1 全部是整机挂死点；**H-14** 非 setuid exec 的能力集计算错误：root 每次 exec 后 cap_effective 清零（reboot/mount 全 EPERM）；**H-15** sigframe 期间冻结一切信号投递，siglongjmp 跳出后进程永久"失聪"（含不可 SIGKILL；与 IPC-H6 同源）。

### Medium（24 项，摘要）
M-01 sigaction 的 sa_mask 从不生效；M-02 siginfo 恒 SI_KERNEL/自身 pid（发送者信息全错）；M-03 SigContext 多出 sc_status 字段（FP 布局错位 8 字节）且 FP 状态恒不保存；M-04 sigreturn 从内核备份恢复（用户帧修改无效，swapcontext 类全毁）；M-05 wait4 rusage 丢弃；M-06 execve argv 多重丢失（空字符串参数 EFAULT 截断后续全部参数）；M-07 execve 无执行权限检查（同 P26）；M-08 setpgid 会话检查空壳；M-09 prlimit64 忽略 pid/不持久（同 P27）；M-10 setrlimit/setitimer 静默不存储（alarm() 恒得 0）；M-11 close_range 返回值/循环上限错误；M-12 getpriority 返回 nice+20 应为 20-nice（nice≠0 全错）；M-13 setscheduler 不校验优先级（同 P06）；M-14 sched_setattr 忽略 attr.size；M-15 sched_setaffinity 空操作（同 P20）；M-16 clock_nanosleep 忽略 TIMER_ABSTIME（pthread_cond_timedwait 睡数十年）；M-17 POSIX timer ID 复用冲突（删除后索引脱钩）；M-18 timer_settime/timerfd ABSTIME 分支与相对相同；M-19 timerfd_settime 无饱和乘法（溢出 panic/release 回绕）；M-20 eventfd/timerfd 阻塞读从不阻塞；M-21 eventfd/epoll/timerfd 的 Box 泄漏（close 回调不 from_raw）；M-22 bind 忽略 sin_addr/recvfrom 恒写 16 字节 sockaddr（缓冲溢出）；M-23 sendmsg 丢弃 msg_name/recvmsg 不回填（DNS 类程序不可用）；M-24 setsockopt 未知项静默成功/accept 不写 addr/accept4 忽略 flags。
### Low：11 项（do_clone EINVAL 语义、waitid options 校验、getgroups 忽略 EFAULT、PR_SET_PDEATHSIG 接受 64、sethostname 假成功、quotectl 死分支、sigprocmask 未对齐 EINVAL、sigaltstack flags、PRIO_PGRP/USER、socketpair 错误码、rseq 静默接受）。

---

## 7. VFS 核心层 + devfs/procfs（41 项）

### Critical
- **VFS-C1** open()/路径遍历零 DAC 检查（见 Top 3）。

### High
- **H1** readlink 普通符号链接 ENOENT（同 SYSA-H2）；**H2** LOOKUP_NOFOLLOW 在 dentry 缓存未命中分支被忽略 → lstat 结果取决于缓存状态（冷缓存跟随链接）；**H3** close(fd≥512) MQ 劫持（同 SYSA-H4）；**H4** ELF PT_LOAD 边界（同 PROC-P01）+ **H5** 解释器 e_phoff 未校验（可达内核内存泄露）；**H6** pipe 阻塞读/写不可被信号中断 → 阻塞在空管道上的进程**不可 SIGKILL**（唤醒循环无 signal_pending 检查）；**H7** bread_async 缓存命中不完成调用者 completion → bread_wait 永久挂起（硬链接双名顺序读即触发）；**H8** icache 无文件系统隔离（rootfs/procfs/devfs 的 fs_id 全 0）：`/proc/5` 与 `/etc` 同键互相顶替 → **/etc 会被替换成 procfs PID 目录**（inode 错误替换，rootfs 根配置下极易触发）。

### Medium（18 项，摘要）
M1 rename/link 无跨 fs EXDEV 检查（rootfs_rename 对异构 inode 类型强转 → 内存破坏）；M2 rootfs O_TRUNC 返回 EROFS（shell `>` 重定向全失败）；M3 rootfs_rename 覆盖目标不删旧条目（目录同名重复）；M4 rootfs 写忽略 O_APPEND；M5 rootfs 路径缓存永不失效（exec fallback 读旧内容）；M6 `..` 词法折叠先于符号链接 + follow_symlink 无法逃出 mount 根；M7 getdents64 对齐填充字节泄漏内核堆内容（Vec set_len 未清零）；M8 /proc/[pid]/cmdline 用当前进程页表读目标进程（内容错误）；M9 open("/proc/N/fd/N") 打开的是路径文本而非目标文件；M10 File.set_dentry 零调用（/proc/self/fd 与 maps 全显示 anon_inode）；M11 loadavg EMA 实现恒为零（F09-30 声称 FIXED 的回归）；M12 PIPE_BUF 原子性无保证；M13 bio LRU 摘除两阶段无引用保护（SMP UAF）；M14 pipe 丢唤醒窗口（drop 锁后 prepare 前无复查，SMP 致命）；M15 /proc/N/fd 组件 lookup 缺失（stat ENOENT）；M16 mknod 不支持 FIFO（mkfifo 不可用）；M17 mount flags 完全忽略（MS_RDONLY 等无效）；M18 umount 不清理 MOUNT_TABLE（mounts 永久陈旧）。
### Low：14 项（负 dentry 条件符号反转（死代码）、dcache 未接线、maps 设备号回归、status Name 用完整路径、meminfo Buffers 恒 0、mountinfo 字段失真、管道容量 16383、POLLHUP 条件置位、getdents64 首条目 EINVAL、devfs mknod 类型/readdir ino 不稳定、elf.rs load_segment 死代码、stat 第 30 字段偏移、chown 组内转移被拒、rootfs lseek 溢出）。

---

## 8. ext4 + jbd2（34 项）

### Critical
- **C1** extent 树索引节点无缓冲边界校验（`eh_max` 采信磁盘值）+ 无递归深度上限 → 恶意镜像 OOB 读（数百 KB）/栈溢出 panic。
- **C2** truncate 释放块不删 extent 项 → 跨文件块别名（静默数据损坏，见 Top 8）。
- **C3** 快速符号链接 unlink 释放任意块（`ln -s sh x; rm x` 必现）。
- **C4** extent 文件洞写入覆写 extent 头（ftruncate+write 常规序列即毁）。

### High（11 项）
- **H1** fs 块大小≠4096 镜像读写错位毁盘（bio 缓冲固定 4096），>4K 镜像挂载即越界 panic；**H2** 超级块/组描述符几何零校验（blocks_per_group=0、desc_size>块长、inode_size=0 等多处除零 panic，恶意镜像挂载即崩）；**H3** InodeAllocator inode 编号 off-by-one（两套分配器约定相反，覆写活 inode 槽位）；**H4** write_group_descriptor 对 32 字节描述符越界写 64 字节（覆写下一组描述符）；**H5** is_dir_empty 的 entry_count 每块重置 → 非空目录被 rmdir（数据丢失）；**H6** 预读完成页索引错位（跳过缓存页后仍按 ra_start+i 插入 → 页缓存张冠李戴，静默读错数据）；**H7** rename 用过期父目录 inode 快照回写（新块脱钩、条目丢失）；**H8** unlink/rmdir/rename 覆盖不失效页缓存 → inode 复用后跨文件读到旧缓存（数据泄漏）；**H9** 内部日志按起始块线性映射且 s_maxlen 未校验 → 日志提交越界覆写文件系统数据；**H10** SMP 已启用但 ext4 全程无并发防护（journal 全局 handle、位图 RMW、组描述符 lost-update——CONFIG ENABLE_SMP=true 使此前"单核可接受"的前提失效）；**H11**（并入 H1-H10 相关）写路径以磁盘 eh_max/eh_entries 作切片长度/下标（恶意 inode OOB 写毁栈）。
### Medium：M1-M10（read_inode 主路径只用 bg_inode_table_lo + 172 字节强转无边界（F08-09 漏掉的第 4 处）、挂载不检查 feature_incompat（metadata_csum fs 被写坏且 e2fsck 报不可修复）、ftruncate 无大小上限（u64::MAX 触发回绕/OOM）、目录项 rec_len 无块边界校验（损坏目录 panic）、rename 覆盖不释放目标数据块、jbd2 tag 8 字节 v1 格式与 64bit 日志不兼容、事务记账失效（j_free 恒不减、环绕无 checkpoint 保护）、三级间接块截断不处理 block[14]、块号计算忽略 s_first_data_block）。
### Low：L1-L9（快速链接判据不一致、create_file 不查重/name 截断、目录尾插 u16 截断、i_size_high 目录误写、null 设备注册表路径、预分配死代码、create-access tag 错位、get_data_blocks 物化全映射 OOM、unlink 不更新父目录 mtime）。

---

## 9. 网络栈（37 项）

### Critical
- **C1** RX 不剥以太网头（见 Top 6）。
- **C2** TX headroom 16 字节（见 Top 6）。
- **C3** fd 命名空间错位（见 Top 7）。
- **C4** TCP 服务器路径四处独立断裂：(a) `add_listen_socket` 零调用者（监听 socket 对 RX 不可见）；(b) SYN 处理状态机矛盾（先置 SYN_RECV 再调 handle_packet，SYN-ACK 永不发出）；(c) 握手完成即移出 pending 而 accept 只在 pending 找（accept 恒 EAGAIN，比 F10-06 已知项更深）；(d) accept 后 established 列表不再持有 → 后续数据被静默吞掉。需要单一事实来源重构。

### High（6 项）
- **H1** ethertype 写入缺 to_be()（所有出站帧被对端丢弃）；**H2** tcp_checksum 伪头与本机体读法不一致（出站校验和必错）；**H3** udp_checksum 端口/长度字段本机序求和（入站校验和必错 → 带校验和的 UDP 全丢，F10-19 修复不完整且其测试恒真）；**H4** 握手 ISN 未转换字节序（SYN-ACK ack_seq 全错）；**H5** 本机 IP 硬编码 192.168.1.100（slirp 10.0.2.x 环境全错，F10-13 修复只是挪位置）；**H6** bind 不读 sin_addr + UdpSocket.local_ip 初始化为硬编码 IP（F10-20 修复无效，发往 127.0.0.1 的 UDP 永远无人接收）。
### Medium（15 项，摘要）
M1 错误路径 skb 泄漏（14 字节帧即可远程耗尽内存）；M2 FIN 不校验序号（重复 FIN 使 rcv_nxt 重复自增，连接永久失步）；M3 对端窗口完全忽略（snd_wnd 恒 65535，无零窗口探测）；M4 Socket::recv_queue 无生产者（poll/select 对 socket 永不就绪）；M5 Socket::send UDP 分支忽略 dest_addr（udp_sendto 是死代码）；M6 无临时端口分配（未 bind 的 connect 以源端口 0 发 SYN）+ bind(:0) 被特权检查拒绝；M7 SYN 无重传无超时、SYN_SENT 收 RST 被忽略（connect 永久挂起）；M8 TIME_WAIT/pending 无回收且 backlog 无上限（远程 SYN 耗尽内存）；M9 ip_rcv 四项健壮性（ihl<5 不拒、校验和只算 20 字节、无分片处理、不查目的 IP/MAC）；M10 无端口冲突检查（双绑同端口先到先得）；M11 connect 发 SYN 即返回成功（假连接）；M12 shutdown 不发 FIN、sendmsg 丢 msg_name、recvmsg 不填；M13 接收窗口 u16 截断回绕（缓冲无界膨胀）；M14 ARP 表不检查过期、无速率限制；M15 TcpConnectionManager 三上下文无锁并发（SMP 内存破坏）。
### Low：12 项（未知 ethertype 回退 IP、无网关概念、不补 ETH_ZLEN、ICMP 不验校验和/不回 Port Unreachable、重复 ACK 过早转 FIN_WAIT2、RST 构造不合 RFC、from_bytes 对齐 UB、ROUTE_TABLE static mut、发送失败数据丢失、tcp_v4_err 不扫客户端连接、SO_RCVTIMEO 假接受、快速重传半成品）。

**已知限制影响评估**：F10-06/07/22/24/29/33 六项 KNOWN LIMITATION 全部被本轮 C1-C4 掩盖或加深——修复顺序应为 C2→C1→H1→C3→H2/H3/H4→C4。

---

## 10. 设备驱动（23 项）

### Critical
- **DRIV-C1** MMIO virtio-blk 队列初始化写错寄存器空间（见 Top 10）。
- **DRIV-C2** MMIO virtio-net 队列编程自相矛盾 + 关中断自旋数秒假报成功（见 Top 10）。

### High
- **H1** `alloc_desc` 单位混用（描述符计数 vs 链计数），MMIO 队列 size=8 时**第 3 个并发请求复用飞行中描述符** → 设备 DMA 串包（F14-16 修复不彻底）。
- **H2** 特性协商全盘接受 EVENT_IDX 而 used_event 是堆垃圾 → PCI virtio-blk 完成中断非确定性被设备抑制——代码中 "random timeout issues" 5 次重试补丁的根因。
- **H3** PCI INTX IRQ 号差一条线（1-based INT_PIN 套 0-based swizzle 公式）+ 中断处理从不读 ISR（电平触发下无限重入风暴；当前"能跑"是因为中断根本不来，全靠轮询）。
- **H4**（潜伏）submit_read_async 响应描述符缺 F_WRITE → C1 修复后所有异步读必 EIO（F14-02 修了 sync 漏了 async）。
### Medium：M1-M8（virtio-net "MTU" 读的是 status 字段（链路 up 时 mtu=1，RX 缓冲 77 字节）；virtio-input config 打在 common cfg 上（设备名垃圾、指针误判、GenDisk 容量硬编码 +0x2000）；fb_fix_screeninfo 缺 4 字段 ABI 错位 10 字节；notify 的 csrci sie,9 是空操作（imm 是位掩码不是位号）；xmit 关中断持锁自旋；notify 地址公式错误（多乘 2 且用 queue_index 冒充 notify_off）；PCI 完成匹配仍是计数制；poll 提前返回泄漏 RX 缓冲）。
### Low：9 项（UART 不查 LSR_THRE 丢字符、getchar 轮询与 IRQ 竞态、device_status 按 u32 读写、GPU 像素格式红蓝对调 [待验证]、PLIC 写只读 PENDING/无效 IPI 残留、evdev ioctl 死代码无校验、EVIOCGBIT 位图失真、read_block 死路径每次泄漏 12KB、InputEvent 时间戳恒 0）。

---

## 11. IPC / 同步 / 信号 / 中断（43 项）

### Critical
- **IPC-C1** semop 阻塞语义反转（见 Top 13）。
- **IPC-C2** futex waiter 池并发双分配（同一 slot 两个 CPU 同时预留 → 链表自环/唤醒丢失，SMP 并发首等即高概率）。
- **IPC-C3** FUTEX_WAKE/REQUEUE 超 8 个等待者静默不唤醒（见 Top 13）。
- **IPC-C4** shmat 与 IPC_RMID 竞态：段物理页被释放后 PTE 仍映射（**物理内存 UAF**，随机内核数据损坏）。
- **IPC-C5** IPC/信号量路径裸解引用用户指针（futex(0x1000, WAIT) 一条调用挂死整机；与 ARCH-C1/H-13 同根因）。

### High（9 项）
- **H1** Semaphore down() 丢失唤醒（预留语义下重查条件应为 count>=0；当前无生产调用者，导出原语的潜在 Critical）；**H2** msgsnd/msgrcv 条件检查与入队分属两次加锁（空间/消息在间隙到达即永久睡眠——posix_mq 的正确实现证明这是缺陷非风格）；**H3** 四个带超时 API 永不超时（semtimedop/mq_timedsend/mq_timedreceive/futex timeout 都没 armed 定时器，pthread_cond_timedwait 永久挂死）；**H4** futex_wait 唤醒后访问已回收复用的 waiter slot（UAF/误删他人等待项）；**H5** 信号帧 FP 状态从不保存 + sigreturn 从内核备份恢复（setcontext 类全毁）；**H6** 单 sigframe 门控：嵌套交付全禁、longjmp/exec 跳出后 sigframe 永不清除 → 进程不可 SIGKILL（exec.rs 不清 sigframe 是可达路径）；**H7** synchronize_rcu 宽限期不可靠（RCU_GEN 从不前进、陈旧 QS 即满足）→ release_task 可能 UAF；pid_hash_lookup 在临界区外返回裸指针；**H8** ksoftirqd 丢失唤醒（置态后不复检 pending + WAKE 折叠标志吞掉唤醒）→ RCU/tasklet 停摆；**H9** IPC_RMID 不置 deleted（RMID 后仍可 shmat 成功、shmget(key) 仍命中待删段）。
### Medium（19 项，摘要）
M1 IPC 序列号不按 16 位回绕（第 65536 次分配起对象永远找不到）；M2 find_with_perms TOCTOU；M3 IPC_STAT/GET* 无权限检查（信息泄露）；M4 RMID 属主只认 cuid；M5 sem_undo 撤销不钳位（信号量变负后 P 永久阻塞）；M6 FUTEX_CMP_REQUEUE 校验在双桶锁外；M7 requeue_list 固定 32 项 + 负数参数不 EINVAL；M8 mq_timedreceive 静默截断长消息 + cbytes 错账；M9 mq_open 重名创建竞态（同名双队列）；M10 mq_notify 触发条件不符 POSIX（虚假通知风暴）；M11 msgsnd 忽略 copy_from_user 返回值（全零消息入队）；M12 shmdt 接受段内任意偏移（部分映射残留）；M13 shmget size 上溢回绕 + size=0 放行 + 预分配全部物理页（shmget(1GB) 立即吃 1GB）；M14 sigprocmask 不剔除 SIGKILL/SIGSTOP（同 syscall-b H-05）；M15 SigInfo union 偏移错位（si_uid +24 应 +20、si_status +32 应 +24）；M16 free_irq 后 depth 不复位（IRQ 重注册后处理程序永不被调用）；M17 timer 两表两次加锁（半插入定时器被静默丢弃，nanosleep 挂死）；M18 tasklet 双重入队/kill 语义；M19 信号仅路由到线程组长（多线程模型缺口）。
### Low：10 项（seqlock 读侧缺内存序、WAKE val 负数应 EINVAL/IPC id 编码不同、list for_each 1000 上限、mq_open 属性静默替换、电平触发中断风暴 [待验证]、unlink 后 mqd 不可用、msgrcv E2BIG 回填破坏 FIFO、LAST_TICK 恒不更新、shmat 权限要求过严、semget nsems=0 打开被拒）。

---

## 12. io_uring / security / dfx / printk / init（34 项）

### Critical
- **IOU-C1** io_uring_enter/register 无 ops 校验的类型混淆（见 Top 11）。

### High
- **H1** enter 与 close 并发 ring UAF（close 无条件 from_raw 释放，无视 Arc 引用）；**H2** ring 三块物理页永不释放（无 Drop，setup+close 循环耗尽 2GB）；**H3** 用户可控 len 的 vec 分配 panic（同 SYSA-C1）；**H4** syslog 读路径持自旋锁+关中断裸写用户内存（坏指针 → panic/死锁）；**H5** syslog 全部 action 无 CAP_SYSLOG 检查（读内核日志泄露地址、清空日志、篡改 loglevel）。
### Medium（10 项，摘要）
M1 无效 SQE index 使提交环永久 wedge（不消费不计数）；M2 mmap 固定地址不查重叠（map_page 无条件覆盖既有 PTE + ring 页双重管理）；M3 console_loglevel 门控 ring buffer 写入（dmesg -n 1 后日志永久丢失）；M4 printk 全局单例重入守卫（SMP 并发丢日志）；M5 注册 eventfd 只存 fd 号不持引用（fd 复用后向无关文件写垃圾）；M6 io_uring pread/pwrite 的 set_pos 竞态；M7 getchar 轮询与 IRQ 竞态；M8 softlockup/hung_task 打印的是检查者自己的栈；M9 init 加载失败静默挂死（无 UART 输出）；M10 init 路径 ELF 边界（同 PROC-P01）。
### Low：18 项（entries clamp 应 EINVAL、mmap 偏移常量 5.x/6.x 混搭、GETEVENTS 丢弃提交计数、poll 恒就绪、put_user 失败被忽略、fd≥1000 魔数拦截先于 io_uring、read_seq 跳记录、早期 printk 不入 ring、/dev/kmsg 截断、UART 不查 THRE、ISIG 字符回传、softlockup 裸指针、khungtaskd 无唤醒源、backtrace 校验不完备、用户区 RWX + 栈 VMA 与 sp 不符、AT_RANDOM 弱随机、FDT 解析无边界、MAX_CMDLINE_LEN 未用）。
### 正面确认：errno 与 Linux 完全一致；capability 编号/位运算正确；security 检查覆盖面除 syslog 外无实质缺口。

---

## 13. 测试 + 构建/CI（31 项 + 声明核对）

### Critical
- **TEST-C1** SPIN 的 LTL 属性从未被全部验证（Makefile 不传 `-N`，声称 8 个实际约 3 个被检查）——并发协议核心声明大部分未经证实。
- **TEST-C2** 825 个内核单测无 CI、无自动判定（make test 交互式无超时无退出码；panic 后 wfi 挂起前已跑的都算 pass）。
- **TEST-C3** 测试数量声明三套数字互相矛盾（825 / 901+94 / 静态 1451 断言点），verify 实际 1,116 个 #[test]（文档写 550）。
### High
- **H1** tcp_handshake.rs 整文件恒真假测试（自我赋值再断言，从未调用报文处理）；**H2** mem_cow.rs 整文件未触及任何 COW 机制；**H3** 假测试清单：boundary/syscall_io/syscall_time/smp/wait4/fork 等的恒真断言与无条件 pass（≥40 个断言点）；**H4** 测试间共享状态污染（setuid 不可恢复、fork 子进程不回收、Box::leak、依赖执行顺序）；**H5** Kernel.toml→构建管线断裂：[boot]/[drivers]/[printk profiling]/[security] 段完全不被 build.rs 读取；9 个 rustc-env 除 RUSTC_VERSION 外全部无人使用；.config 模式下其余键回退硬编码默认值而非 Kernel.toml；release 未开 overflow-checks（与 [security] overflow_check=true 的意图相反）；**H6** CI 退出码判断不可靠（无 pipefail，编译失败也绿；spin 编译错误静默通过）。
### Medium：M1-M10（kernel/Cargo.toml profile 无效、-Z ub-checks=no 需 nightly 未锁定工具链、boot.S 硬编码依赖系统交叉编译器、生成 config.rs 入库、链接脚本相对路径假设+根 cargo 配置互相污染、early 页表 8MB 硬编码无链接期断言、test 模式挂载 rootfs 分叉、mkrootfs mount 无 trap、userspace Cargo.toml 注释与配置相反、harness panic 即全绿+SKIP 不入总数）。
### Low：12 项（unused imports、版本双轨、.PHONY 缺漏、virtio_net 测试名不副实、恒真断言、fd reuse 断言缺失、常量自比、条件 pass、零长度 mmap 固化错误预期、framebuffer 测试恒 SKIP、RETCODE 未用、build/Makefile help 失实）。
### 覆盖盲区：execve 真实路径、COW 页错误、真实阻塞 wait、SMP 并发、pipe/epoll 正向路径、真实设备 I/O、futex 运行时行为——均无测试（多数因单 hart 关中断环境限制，文档未声明）。

---

## 14. 文档（64 份，53 项 + 36 处失效链接）

### 最需要修正的 5 份
1. **docs/architecture/kernel-lock.md** — 整份描述已删除的 Kernel Big Lock 为现行机制（代码零引用）；连带 riscv64.md 三处大锁段落、roadmap.md:119 特性行。
2. **CLAUDE.md** — 3 个失效链接（docs/tests/、docs/development/changelog.md、docs/development/user-programs.md）、Phase 36（现 52）、"203 test cases"（现 825+）。
3. **README.md** — 348 syscalls（实 344）、825/3,777 测试数不自洽、101,200/102,400/106,346 行数三版本、启动日志含与代码相反的 "runqueue per-CPU"（实际是 GlobalRunQueue）、4 处失效链接。
4. **docs/guides/getting-started.md** — mini-ltp 已不存在、RELEASE=1/distclean 无此目标、/bin/shell 不存在（实际 mrsh）、预期启动日志虚构。
5. **docs/guides/configuration.md** — 默认值错误（heap 16→32、max_page_tables 256→1024、aarch64 true→false）、[printk] 段归属错、40+ 配置项缺失。

### 其他 High（摘要）
- structure.md：测试文件清单列 4 个不存在的文件、漏 8 个实际存在的；sync/ 列出不存在的 mutex.rs；asm 行数过时。
- lock-ordering.md：NMI_MASK 位域声明错误（1 位 vs 4 位）；行号引用失效 2 处。
- debugging.md：持久日志描述与被注释禁用的代码相反。
- linux-ltp-test-report/unit-test-report/development.md 失效链接若干。
- kernel/verify/README.md："550 tests across 47 modules"（实际 1,116/98）。
- code-review-2026-04-17.md:5 引用路径错（04-15 报告实际在 archive/）。
- 正面评价：memory.md 与 boot.md 的技术内容与代码高度一致（30+ 项常量核验通过），是全库最可靠的文档；changelog、test-visibility、toolchain/mrsh/toybox README 准确。
- 缺失文档：网络栈、IPC、security、swap/vmscan/OOM、io_uring、信号子系统、POSIX 定时器——均无设计文档。
- archive 最误导：mm-improvement-plan、scheduler-analysis/refactoring-plan（三份互相矛盾）、gic-smp/ipi-testing/pscidebug（ARM64 时代遗物）、context-switch-analysis（sstack 协议已取代 SPP 检测）。

---

## 15. 修复优先级建议

### 批次 1 — 挂死/Panic 类（修复代价小、收益立竿见影）
1. ARCH-C1 异常表错位（汇编标签重排）
2. ARCH-C2 trap 入口 t0-t2 clobber（重排保存序）
3. SYSA-C1/IOU-H3 用户可控分配 panic（min(count, MAX_RW_COUNT)）
4. PROC-P01/INIT-M10 ELF 段边界校验（checked_add）
5. syscall-b C-01 hwprobe M 态 CSR（改常量）
6. syscall-b H-13/IPC-C5 约 30 处裸解引用改 copy_from_user
7. EXT4-H2 超级块几何校验（防除零）
8. MM-C1 copy_page_contents 补 phys_to_virt

### 批次 2 — 数据损坏/安全类
1. VFS-C1 DAC 权限检查（open/路径遍历/f_mode 三点）
2. syscall-b C-02 + P11 setuid exec 链（延迟提交凭据 + AT_SECURE + LD_* 清洗 + 降 uid 清 caps）
3. IOU-C1 io_uring enter/register ops 校验
4. EXT4-C2/C3/C4 truncate/symlink/hole 三处（常规操作毁盘）
5. SYSA-C2 mremap 搬移 + SYSA-C3 brk 下限
6. MM-C3 swap slot UAF + MM-H2 buddy 锁内迁移
7. IPC-C4 shmat/RMID 竞态
8. IPC-C1/C3 semop/futex 唤醒原语
9. PROC-P02 vfork 时序 + PROC-P03 reparent + PROC-P04 deferred notify 回归
10. NET-C2→C1→H1→C3→H2/H3/H4→C4（网络栈按此顺序逐步恢复）

### 批次 3 — 语义/兼容类（High/Medium 按模块推进）
signal 交付链（H-05/06/07/15、M-01~04）、调度器（P06-P10）、syscall 语义修正（SYSA-M 系列、syscall-b M 系列）、VFS（H2/H3/H7/H8、M 系列）、ext4 并发粗锁（H10）、drivers（H1-H4）。

### 批次 4 — 测试与文档治理
1. TEST-C2 单测 CI 化（timeout + 输出判定 + SBI shutdown）
2. TEST-C1 SPIN -N 循环；TEST-C3/H1-H3 清理假测试
3. H5 build.rs 配置校验 + H6 CI pipefail
4. 文档：重写 kernel-lock.md、更新 CLAUDE.md/README/getting-started/configuration、数字脚本化自动生成

---

## 附：审查方法与覆盖清单

- 14 个并行深度审查，覆盖 kernel/src 全部 33 个子目录（436 文件/105,579 行逐行）、kernel/verify、kernel/build.rs、Makefile/build/test 全部脚本、.github CI、toolchain 配置、userspace 自研部分构建、docs 全部 64 份 markdown。
- 每个子系统审查均先消化 docs/development/code-review-2026-04-17.md 与 docs/archive/code-review-2026-04-15.md 的 FIXED/KNOWN 条目；KNOWN LIMITATION 仅在后果升级时重报（如 F10 系列被 NET-C1/C2 掩盖、F01-01 的 TLB 广播缺失、F12-34 的 fd≥512 误伤）。
- 抽验：本轮 Top 发现经源码直接核验（exec.rs:177、io.rs:83、socket.rs:572、ethernet.rs:123 零调用、buffer.rs:149、generic_permission 仅 3 调用点、io_uring enter 无校验、hwprobe M 态 CSR、virtio MMIO 偏移、uaccess.S EXTABLE 标签位置——10/10 属实）；ARCH-C1/C2 另经 readelf/objdump 二进制实证。
- 发现并撤销的误报 2 项（rt_sigaction 无 sa_restorer 符合 RISC-V ABI；ipc cuid 属主判定与 Linux 一致），已记录避免后续复审重蹈。

---

## 16. 第四轮复查（2026-09-13，Wave 1/2 修复后）

> Method: 12 个并行深度复查（arch、mm、process/sched、syscall×2、VFS、ext4、net、drivers、ipc/sync/interrupt、io_uring/dfx/security、tests/build/CI/docs），逐文件逐行，交叉验证全部调用点；Critical 级发现经主审二次源码核验。
> 结论：第三轮 ~440 项发现**几乎全部属实**（撤销 4 项误报，见 §16.4）；Wave 1/2 的 36 项修复中 28 项核验通过，**8 项失败或不完整**；另发现 **6 项新 Critical 与 ~25 项新 High**，其中多项为 Wave 2 修复自身引入的回归。

### 16.1 Wave 1/2 修复核验结果（FIX-FAIL 清单）

| ID | 位置 | 失败原因 | 后果 |
|---|---|---|---|
| **syscallb-C02** | syscall/process.rs:354 | `cred_saved = Some(cred.clone())` 位于 setuid/caps 变异**之后**，Err 路径恢复的是已提权副本（回滚=空操作） | **本地提权链仍然开放**（半合法 setuid ELF 加载失败 → 旧映像持 euid=0+FULL caps 继续运行） |
| **SYSA-C1** | syscall/io.rs:95,157,262,786 | MAX_RW_COUNT=2GB 钳制仍远超 32MB 内核堆，`vec![0u8; 256MB]` 分配失败 → alloc_error_handler panic | read/write 大 count 一步击杀内核（Wave 1 目标未达成） |
| **IOU-H1/H2** | io_uring/mod.rs:426-473 | close 先 unref 后清 private_data（无锁）；mmap 映射不持引用；Drop 无条件 free_pages | enter/mmap/close 并发 UAF；munmap+close 双重释放（`mmap;munmap;close` 三行 100% 复现 buddy 损坏） |
| **2.25/IPC-H6** | signal.rs:786-791 | sp 离帧检测条件写反（要求 sp∈[frame,frame+4096)，而 handler 执行期 sp 恒 < frame_addr） | 门控形同虚设且引入新崩溃路径：嵌套信号覆盖唯一内核 sigframe 备份 → sigreturn 后 epc 跑飞 |
| **IPC-H3(futex)** | sync/futex.rs:344-397 | 超时定时器装上了但醒后从不判 deadline，恒返回 0 | futex 超时语义破坏（条件不成立却"成功"） |
| **IPC-C4** | ipc/sysv_shm.rs:249-259 | RMID 的 nattch==0 判定与释放不在同一临界区，shmat 可在间隙完成 | 物理 UAF 窗口未闭合（shmat 侧已修，RMID 侧没修） |
| **2.27/VFS-H7** | fs/bio.rs:704-721 | Phase-1 命中不检查 BH_Uptodate：在飞条目被当命中 complete(0) | 把永久挂死变成静默脏读（读到未完成 I/O 的半新数据） |
| **ARCH-H3** | mm_ops.rs:444 × sysv_shm.rs:570 / io_uring/mod.rs:505 | put_page 对"无记账映射"（shm 段页、io_uring ring 页）成灾：这两类映射从不 get_page/inc_mapcount，但 refcount=1 | **Wave 2 回归**：munmap/shmdt 即释放仍被段/ring 持有的页 → 跨进程 UAF + close 时双重释放 |

其余 28 项（ARCH-C1/C2、EXT4-H2/C1、MM-C1/C2/C3、MM-H2 主路径、2.1-2.10 主体、2.17-2.24、2.28-2.31、1.6 十处、TEST-H6 等）核验通过；ARCH-C1 校验脚本真实有效但未接入 CI（零引用）。

### 16.2 新发现 — Critical（6 项）

1. **[NEW-C1] mmap/io_uring mmap 接受内核地址 → 覆写共享内核页表 = 完整提权**（arch/riscv64/mm/mm_ops.rs:254-265 + io_uring/mod.rs:490-508 + syscall/memory.rs）— MAP_FIXED 固定映射只查下界与对齐、不查 `end <= USER_END`；`map_page` 无任何地址守卫，对 vpn2≥256 直接写进 `copy_kernel_mappings` 复制的**共享内核 L1/L0 表**。用户 `mmap(内核文本地址, MAP_FIXED, PROT_RWX)` 即可把内核代码 PTE 替换为指向自己可控页 → S 态任意代码执行。io_uring mmap 分支同病（已亲手核验）。修复：mmap 全部路径强制用户区间 + map_page 拒绝 vpn2≥256。
2. **[NEW-C2] wake_up 可把"已置睡眠态但仍在运行"的任务入队 → 同一任务双核并发**（process/task.rs:1309-1333）— vfork 父进程（fork.rs:402-412 复查后跳过 schedule 继续运行）与 do_wait 复查命中僵尸（exit.rs:361-381）都处在此窗口：set_state(睡)→子唤醒入队→父复查决定不睡→父在队列上被另一 CPU pick → 双核同跑一个 Task/内核栈，整机级结构毁坏。Wave 2 把 futex 窗口拉得更宽（drop 桶锁→add_timer（堆分配+两把锁）→schedule）。修复：wake 对 current 任务只置位不入队（Linux ttwu 语义），复查后不睡路径主动出队。
3. **[NEW-C3] readlink("/proc/N/fd") 数组越界 panic**（syscall/file.rs:599-614）— `parts.len() < 3` 检查后直接索引 `parts[3]`；路径恰 3 段时越界（已亲手核验）。一条 `readlink("/proc/1/fd")` 击杀整机。
4. **[NEW-C4] TCP 管理器/定时器从未初始化**（net/tcp.rs:1495/1507、tcp_timer.rs:136/143）— `init_tcp_manager`/`init_tcp_timer_manager`/`route_init` 全库零调用；首包即 `panic!("used before init")`，且 Timer softirq 每 tick 对未初始化 MaybeUninit `assume_init_mut()` = UB。Wave 3 第 0 步。
5. **[NEW-C5] ipv4_send tot_len 多算 20 + TCP 校验和写回缺 to_be**（net/ipv4/mod.rs:202,213、tcp.rs:1917）— push(20) 后又加 IPHDR_LEN；checksum 按本机序写内存。即使修完 NET-C2/H1/H2，**所有出站 IP 包仍 100% 被对端丢弃**。
6. **[NEW-C6] loopback 同步重入 &mut 别名 UB**（net/loopback.rs:43-57 + ethernet.rs:326-333）— 无 virtio 设备时 xmit 同步回调 ethernet_rcv，TCP 处理中发 ACK 再次进入 `get_tcp_manager()` 取第二个 `&mut` 并对迭代中的 Vec push/remove——验收目标"loopback echo"直接内存破坏。

### 16.3 新发现 — High（25 项，摘要）

- **arch**：ARCH-H1 恶化确认（FPU 不恢复 + FS 门控失效 → 跨任务浮点污染/信息泄漏）。〔2026-09-13 已修复：context_switch 切入 restore_fpu、fork 继承父 FP 状态、新任务 fs=INITIAL；nettest 新增 fp 用例端到端验证（修复前首次切换后用户 FP 指令即 SIGILL）〕
- **mm**：compaction remap_page 按 vaddr 过宽重写无关进程 PTE（MM-C2 修复扩大打击面）；Zone::alloc_single_page 绕过 zone 锁（MM-H2 缺口）；mremap FIXED 重叠先毁源后拷贝（静默丢数据）；munmap/MAP_FIXED/mremap 三路跳过 swap entry（解除后可"复活"+slot 泄漏）；handle_cow_fault 无 PTE 锁（并发双重 put_page → UAF）。
- **process/sched**：deferred-notify 强改运行中任务 ti_cpu（受害 CPU 的 cpu_id()/per-CPU 全错位）；vfork exec 失败也唤醒父（共享地址空间并发）；e_phentsize 未校验（execve 内核 OOB 读，P01 修复残留）；sched_setattr 零权限（无特权可上 RT 饿死整机）。
- **syscall**：rt_sigaction 结构布局与 RISC-V ABI 错位（内核 24B vs ABI 32B，sa_mask 读到 restorer 指针——**所有 libc 信号语义建立错误布局上**）；setgid-root exec 清空 caps；非 root 可 capset I:=P 跨 exec 全量保留 caps；~40 处 H-13 残余裸解引用（比原估 15 处多）。
- **VFS**：符号链接解析完全绕过新 DAC（follow_symlink 内部自行走路径，VFS-C1 核心缺口）；procfs/execve 捷径绕过 DAC（exec 连 x 位都不查——配合 setuid 即提权，P26 升级 H）；bio LRU 驱逐两阶段 TOCTOU UAF；bread_async 双检分支释放在飞 DMA 缓冲；子挂载根 dentry 无 parent（*at 全错路）。
- **ext4**：add_entry 复用恰好填满的已删项时静默丢条目；mkdir/rename 过期父目录快照回写；日志违反 WAL 顺序（崩溃原子性为零）；日志环绕后恢复回放旧事务覆盖新数据。
- **ipc**：semop 同一集合多操作非累计计算（semop(-1,-1) 只扣 1——比 IPC-C1 更本质）；sem/msg RMID 唤醒→重排→释放竞态（等待者挂死在已释放对象上）；futex_requeue 换桶后幽灵等待者。
- **drivers**：PCI ECAM 扫描把功能位当设备号（slot 放大 8 倍，第 5 个设备起不可见 + IRQ 公式失效）；virtio-input 0x1052 被 VirtIOPCI::new 拒绝（探测必败，M2 修复前置）；net poll 把 DMA 物理地址当虚拟地址解引用（C2 修好即触发）。
- **iouring**：mmap TOCTOU 二次取 fd；close/enter 并发 private_data 无锁竞态。
- **tests/CI**：verify 同步门禁在 HEAD 上失败（17 处 drift，kani/miri CI 必红且证明的是漂移拷贝）。

### 16.4 撤销的误报（4 项 + 2 项部分）

- EXT4-M4（目录项 rec_len 越界 panic）：dir.rs from_bytes 已校验 rec_len∈[8,bs]，现行代码无越界。
- EXT4-L3（目录尾插 rec_len u16 截断）：挂载已强制 bs=4096，无截断路径。
- DRIV-L4（GPU 红蓝对调）：format/颜色常量/fbdev 上报三者自洽，无对调。
- IOU-M9（init 失败静默挂死）：失败路径有 UART 输出。
- 部分：ARCH-L5（KERNEL_STACK_SIZE 已统一 config，仅 intr-stack 三处手抄 16K 残留）；TEST-H3 的 syscall_time 子项（实有单调性断言）。

### 16.6 新发现 NEW2（2026-09-13 追加，Critical）：fork 子进程 ext4 文件操作触发 SMP 竞态

追查"mrsh 重定向/管道子命令全部死亡（129/127、文件 0 字节）"时定位到的**既有缺陷**（cf6defc 内核同样复现）：
- **最小触发集**：普通 fork 的子进程执行**任何 ext4 文件打开**（含只读、含 icache 命中 inode）后退出——非确定性表现为 (a) 内核 panic：跳转执行空闲页链表指针（epc=0xffffffd600af0000 一类线性映射地址，freelist 特征）；(b) 子进程用户态空指针段错误（sp 异常低）；(c) 静默挂起。纯 fork+exit 无恙、CLONE_VM(vfork) 子进程无恙。
- **波及面**：shell 的一切重定向与管道子命令（mrsh 以 fork+file-actions+exec 实现）即本缺陷的用户可见面——Wave 2 记录的"mrsh 管道 EBADF 遗留缺陷"应即此族。A/B 证实非本轮任何提交引入。
- **定位到 munmap 追踪**：panic 栈指向 MmStruct::munmap（mm_ops.rs:395 附近），epc 为已释放物理页（伙伴 freelist 指针被当指令执行）——典型 UAF-after-free 经间接调用跳转。
- **疑似范围**（未最终定罪）：icache Arc 引用与 LRU 逐出的窗口、ext4 全模块无并发防护（EXT4-H10）、fork 的 mm 快照/页表复制与退出释放路径。需要专项排查（nettest 已内置可启用的探针用例，见 test/nettest.c 注释）。
- **处置与进展（2026-09-13 第二轮追查）**：
  - 已修一个确证的同族缺陷 **VFS-H13**（bio LRU 两阶段驱逐无锁）：Phase1 在 LRU 锁内选定 count==0 受害者摘链后释放锁，Phase2 才拿 bucket 锁摘哈希——间隙内并发 get() 可钉住该条目而 Phase4 仍释放，调用方持悬挂 BufferHead（与"执行已释放页"签名吻合）。现 Phase2 在 bucket 锁内复查 count，非 0 回插 LRU 放弃驱逐。
  - **✅ 已破案并修复（第三个根因，2026-09-13 第三轮追查）**：fork 子进程退出码丢失的机制为——fork 将父进程栈页 COW 降级为只读（W=0，PTE 探针实证），而编译器把局部变量寄存器化，父进程对该页的**首次写发生在内核 copy_to_user（wait4 回写状态）**；异常表把写故障转为"未拷贝字节数"，`do_wait` 用 `let _uncopied` 丢弃 → 状态静默丢失。Linux 对"内核访问用户 COW 页"会做 fault-in 解析后重试，本内核缺失。**修复**：`copy_to_user` 失败后对目标区间做 `fault_in_write`（COW 位→handle_cow_fault；未映射→handle_mm_fault(WRITE)+CowPending 解析），成功则重试一次。该修复同时消除一族"fork 后随机 EFAULT/0 值"类症状（wait4/sigprocmask/getcpu 等一切经 copy_to_user 写用户局部变量的路径）。
  - 附带验证：vfork+exec 与 vfork+openat+dup3+exec(echo→文件)（即 posix_spawn 完整序列）syscall 级全部通过并纳入 nettest 常规用例。
  - **✅ 第四个根因破案并修复（2026-09-14）：mrsh 重定向子进程 SIGSEGV 的完整机制**——反汇编 toybox 崩溃点（epc=0x4ff30）证实为 musl `__init_libc` 的**故意 NULL 写崩溃**：musl 启动时 ppoll 检查 stdio fd 0/1/2，发现 POLLNVAL（fd 不存在）时尝试打开 `/dev/null` 替换，openat 失败即 `sb zero,0(zero)` 自杀。两个内核缺陷串联：
    1. **FD_CLOEXEC 挂在共享的 File 对象上而非描述符上**：mrsh 以 `open(O_CLOEXEC)`+`dup2(fd,1)` 实现重定向——dup2 复制的是同一个 Arc<File>，cloexec 位跟着泄漏到 fd1 → execve 时 close_cloexec_fds 把 stdio 关掉 → POLLNVAL。POSIX 明确 FD_CLOEXEC 是描述符属性。修复：FdTable 增加每描述符位图（cloexec_bits[16]×u64），全部写点（open/dup3/pipe2/eventfd/timerfd/fcntl F_DUPFD/F_SETFD）迁移到按 fd 设置，dup2 清除新 fd 位，close_cloexec 按位图判定。
    2. **/dev/null 不存在**：devfs 只有 kmsg/input。修复：注册 nulldev（read=EOF/write=吞掉全部）。
    验证：`echo X > file; cat file` 端到端输出正确、`2>/dev/null` 不再报错、smoke 15/15、nettest VF-re（posix_spawn 序列）通过。**shell 重定向自 Wave 2 记录的"已知不可用"至此修复。**
  - **仍未解决（第五轮追查进展）**：mrsh 管道（`a | b`）现确定为**确定性内核 panic**：`echo PP | cat` 在 mrsh 连续 fork 两个子进程时 100% 触发 `Option::unwrap() on None`（btree/navigate.rs:534 = BTreeMap 迭代器 next_unchecked），epc 落在区间迭代内联代码。伴随 DEADLOCK 警告（wait 队列 Vec 锁/TIMERS 锁自旋）。已排除/加固：TIMERS/ACTIONS 全部改 lock_irqsave（timer.rs 10 处，本提交）；fork 大互斥实验（串行化 copy_page_table_cow）无效已回退——非双 fork 并发 COW 降级竞争。剩余嫌疑：某 BTreeMap（CFS/DL 时间线或 VMA 表）存在**不持 GRQ/VMA 锁的访问路径**，或在 IRQ 上下文被无锁触碰。NEW2 偶发竞态（套件挂点漂移）依旧。
  - **第六轮（2026-09-14）**：①nettest 内置非 mrsh 管道复现（PIPE2：pipe2+双 fork+exec echo+读回校验）——套件通过时端到端通过（P2e）；②全局叶 PTE 串行化落地（PTE_MODIFY_LOCK，irqsave，覆盖 fork 降级/COW 换页/exec+exit teardown 三条路径）——A/B 通过率不变（~50% 挂于首个 fork，为 NEW2 另一独立成分），mrsh 管道 panic 仍复现（PTE 竞态非其成因）；③mrsh panic Sepc=console::putchar_no_lock——DEADLOCK 打印期间再 panic，实为打印路径自陷；④mrsh 内部使用 hashtable（BTreeMap 无关）与更大 argv/envp——mrsh 特有路径待查。
  - **第七轮关键突破（2026-09-14）**：panic 地址 0xffffffd600af9000 经 PAGE_OFFSET（0xffffffd600000000）换算得物理地址 0xaf9000——**低于 DRAM 起点（0x80000000）2GB**，不在任何已分配物理页中（buddy zone free 路径探针证实该页从未经过 zone.free_pages）。这不是"执行已释放页"而是**函数指针值被一个小整数覆盖后的类型混淆调用**：某个内核结构体的 ops/fn-ptr 字段被覆写为 0xaf9000（= 11485440），可能来源为用户态地址、大小字段、或 slab 分配器空闲链表值溢出。修复需要 GDB 断点在 panic 时检查现场结构体。第八轮补充：ra == epc == 0xaf9000（内核侧标记实证）——CPU 执行了 `ret`（jalr x0, 0(ra)）且 ra 已被覆写为 0xaf9000。这排除了普通间接调用，指向 **pt_regs.ra 被覆写后经 ret_from_exception 恢复**或 **内核栈上保存的 ra 被栈溢出/越界写覆盖**。64KB 栈实验排除栈溢出。下一步：在 copy_thread 写 pt_regs 后校验 ra 值、在 context_switch 恢复 sp/ra 前后校验。
  - **第九轮数据点（2026-09-16，第六轮修复批回归时捕获）**：①硬挂死形态符号化成功：`DEADLOCK: spinlock stuck cpu=0 lock=0xffffffff802de3a8` → **PROCESS_TREE_LOCK**（.bss 中紧邻 STACK_CACHE@0x802de390 与 KTHREAD_MAP@0x802de3b0——两者任一越界写都会破坏该锁，布局本身即是嫌疑面）；同时 CPU#1 soft lockup（PID 305）。ra=0xffffffff800faffe 反解为 `Spinlock<()>::lock`（spinlock.rs:175）内部——即等待方经普通 `lock()` 自旋。②PIPE2（pipe2+双 fork+exec echo）新签名：exec 后 echo 对 fd1 write 得 **EBADF**（0/3 次 PIPE-OK），且 wait4 回读状态字异常（wa=0x10、rb=0x3f0，非 0/0x3F00 编码）——与既有"fork 子进程退出码丢失"同族。已把 nettest PIPE2 改为打印双子进程原始 status（NEW2 活体探针，套件判定不变）。③A/B：上述形态在未含本批修复的 HEAD 上 1:1 复现（PIPE-OK=0、EBADF、DEADLOCK 各按相同概率出现）——**非第六轮修复引入的回归**。④回归门新证据：poweroff -f 后 e2fsck -fn 报 journal-has-data + 孤儿 inode（2228-2230）残留——无日志文件系统的硬断电预期表现，净卸载 e2fsck 门仍属 Wave 7（EXT4 日志/孤儿处理未实现）。
- **第六轮回归检视修复批（2026-09-16，全部完成）**：HIGH×5 + MED×3，均构建通过、smoke 15/15、nettest 全部断言通过、单测 859/31 与 HEAD 基线一致（31 项为已知 Wave 8 陈旧断言类）：
  1. munmap VMA 拆分 remove→add 间隙：改为**单次 vma_write 临界区内完成 remove+add**（原按片段分别加锁，间隙内并发 fault/fork 看到"空洞"）。
  2. mknod FIFO 假成功（实际建 S_IFREG）：改 **ENOSYS**——ext4/rootfs 均不保留 S_IFIFO 类型且无管道绑定，假成功比失败更糟。
  3. futex2（NR455/456）ABI：futex_wait 5 参布局（expected=args[1]、timeout=args[4]）；futex_wake 不再把 nr=0 强制为 1（0=合法 no-op）。
  4. MAP_FIXED 内联 PTE 拆除绕过 PTE_MODIFY_LOCK：整段改走 **unmap_pages**（锁内完成 walk/rmap/refcount/clear_pte/sfence）。
  5. mprotect 锁外读 PTE、锁内写过期值：改为**整个 walk+重写+sfence 持有同一 PTE 锁区段**（循环不睡眠，一次性 irqsave 安全）。
  6. MED：FUTEX_WAIT_BITSET/FUTEX_CLOCK_REALTIME 超时按**绝对时间**换算（CLOCK_REALTIME 与 jiffies 同为开机基点，无需偏移；原恒相对——pthread_cond_timedwait 语义破坏）；demand-fault 三处 map_page（匿名/文件/换入）纳入 PTE 锁，且锁覆盖 map→rmap 建立窗口（fork 走查间隙引用不可见问题）；全库 14 处 10MHz 魔数统一 config::TIMER_CLOCK_FREQ_HZ。
  7. ext4 并发补强（本轮前置完成）：7 个 namei 包装器（create/mkdir/symlink/link/unlink/rmdir/rename）持 EXT4_BIG_LOCK；read_vfs/write_vfs/setattr **移除**该锁（preempt-off 自旋锁内经 bread_wait→schedule 睡眠 = 死锁源）。

---

## 17. 第七轮检视（2026-09-16，4 个并行深扫 + GDB 实捕）

> Method：4 个并行 agent（热点回归/调度进程信号 IPC/MM-VFS-网络-驱动/系统调用-测试-CI）全库扫描，Critical 级经主审源码二次核验；主审同步完成 NEW2 的 GDB 实捕（四 CPU 全符号化栈）。共 **5 项 Critical/HIGH 安全类、~20 项 HIGH、~20 项 MED/LOW**。

### 17.1 NEW2 实捕与根因链（GDB，2026-09-16）

四 CPU 全符号栈捕获（soft lockup 触发时）：**CPU2 陷入 trap.rs:524 `MmFaultResult::KernelPanic` 的 wfi 死循环**（内核态缺页后持锁自旋暂停——放大器：任一 CPU 内核态缺页即携锁永停）；CPU0/1 在 scheduler_tick 中自旋等 GRQ+1760 锁；CPU3 在 `pick_next_cpu`（fair.rs:602）**持 GRQ irqsave 锁做 6KB 堆分配**（Vec::with_capacity(256)）。结合 B1（cfs_rq.curr 悬挂 UAF，见 17.2），NEW2 因果链定性：**cfs curr UAF 写入已释放 Task → 堆元数据/邻近结构破坏 → 指针字段被垃圾覆盖（0xaf9000 类）→ 内核态缺页 → wfi 携锁停机 → 全机 DEADLOCK**。EBADF 变体 = 同源堆破坏击中 fdtable。

### 17.2 新发现 — Critical / 高危安全类

| # | 位置 | 问题 | 后果 |
|---|---|---|---|
| R7-1 | syscall/memory.rs:526 | **sys_mprotect 无用户区间上界**：vpn() 掩码后 vpn2∈[256,511] 直写共享内核 L1/L0 表，flags 恒含 V\|U → 用户可对内核线性映射加 U\|R\|W（完整提权）或剥除内核段权限（变砖）。sys_munmap/madvise(MADV_REMOVE)/pkey_mprotect 同洞 | 提权/DoS（NEW-C1 类未修完） |
| R7-2 | sched/fair.rs:662,753 | **cfs_rq.curr 悬挂 UAF**：curr 仅在 clear() 全清时置空；任务退出+被收尸后 curr 残留，update_curr 对已释放 Task 写 sum_exec_runtime/exec_start（每次空闲调度都写） | 堆破坏（NEW2 主根因，见 17.1） |
| R7-3 | signal.rs:1425 | **STOPPED 任务永不唤醒**：signal_wake_up_state 仅唤醒 is_sleeping()（不含 STOPPED）；SIGSTOP/Ctrl+Z 后 SIGCONT/SIGKILL 均无效 | 不可杀任务/作业控制 DoS |
| R7-4 | syscall/dispatch.rs:189 | **NR140/141 setpriority/getpriority 接反**：nice(10) 实际执行 getpriority；getpriority 读残留 a2 寄存器并把 nice 设成寄存器垃圾 | 全部 nice/renice 用户受影响 |
| R7-5 | mm/vma.rs:441-451 | **VmaManager::add 向前合并跳过重叠检查**：prev.can_merge 命中即 return，不查新区间是否吞并后续 VMA → 确定性重叠 VMA（verify 套件 3 红即此） | 静默重叠映射 |

### 17.3 新发现 — HIGH（摘要）

- **bio**：VFS-H13 修复自身竞态——Phase1 摘链与 Phase2 复查间隙，并发 get() 对已摘链条目 move_to_lru_head（tail 丢失）+ Phase2 push_lru_head 双重插入（自环）→ LRU 永久损坏/"all buffers in use"或 UAF。
- **page_cache**：invalidate_inode 不查 ref_count>0 即释放物理页；ext4 每次写后 invalidate → 并发读者拷贝自已释放页（物理 UAF）。
- **demand-fault**：already_mapped 判定在 PTE 锁外；CLONE_VM 双线程同址缺页 → 双分配双 map，先到页成孤儿+丢写。
- **io_uring**：enter/register/mmap 三处 LRU 锁外读 private_data（IOU-H1 修复不完整；pin_ring 是正确写法但成死代码）。
- **exec 泄漏**：alloc_and_map_* 按 next_power_of_two 取 order，映射仅 ceil(size/PAGE) → 每次 exec 泄 ~48 页/192KB（shell 循环必 OOM）。
- **fork 失败路径**：do_clone 四个错误出口只 free_task_slot，不 pid_hash_remove/free_pid → 悬挂 PID 哈希项 UAF + PID 泄漏。
- **EXT4_BIG_LOCK（6R 回归）**：namei 包装器持 preempt-off 自旋锁跨块 I/O，virtio wait_for_desc_completion 256 次自旋后 schedule() → 持锁睡眠；其他 CPU 非抢占自旋 → DEADLOCK 警告（两轮 A/B 均含 5b16568，故 A/B 无法区分）。**本项必须在 NEW2 复测前修掉（噪声源）**。
- **COW+PROT_NONE 活锁**：mprotect 去 R 保 COW 位 → handle_cow_fault 只补 W 不补 R → 保留编码 PTE 无限缺页循环。
- **DL 无重入队守卫**：wake_up 检查-入队非原子 × DlRunQueue::enqueue 无条件插入 → 同一任务双核并跑。
- **GRQ nr_running 单调膨胀**：enqueue 计数与类内 on_rq 守卫不配对、pick/睡眠出队不减 → 空闲快路径永久失效。
- **sem/msg 丢失唤醒**：条件检查与入等待队列两个临界区无复查（posix_mq 是正确写法）→ 单次 V 后 P 永睡。
- **NEW-C2 残留 5 处**：do_waitid 信号路径、rt_sigsuspend/sigtimedwait 复查、wait_event 宏复查、ksoftirqd 复查——仍在队任务被二次入队。
- **sendmsg/recvmsg/sendmmsg/recvmmsg**：iov_len 求和仅 access_ok（≤256GB），vec! 分配 >32MB 堆 → alloc panic（SYSA-C1 同类未覆盖）。
- **调度器锁内堆分配（GDB 实捕）**：pick_next_cpu 每次 pick 分配 256 元素 Vec 且持 GRQ irqsave 锁 → 分配失败即调度器 panic；锁序隐患。
- **verify 套件 HEAD 红**：3 红（2 项 R7-5 真缺陷 + 1 陈旧断言 test_adjacent_vmas_no_overlap）。

### 17.4 新发现 — MED/LOW（摘要）

munmap 采集(读锁)/应用(写锁)仍分两段 TOCTOU（4 MED）；mremap/shmat/framebuffer 映射绕 PTE 锁；munmap 拆分破坏 shm nattch 记账（exit 双 detach）；futex_requeue NR456 盲转发（可能无限睡）；futex2 丢 flags/mask；FUTEX_WAIT|CLOCK_REALTIME 应保持相对（6R 修复过度，Linux 仅 WAIT_BITSET 绝对）；mprotect 多 VMA 只改第一个 VMA 标志；rt_sigpending 过滤反了（应 pending&blocked）；termios ICANON=0x100 应为 0x2、c_line/c_cc 布局错位；nettest 判定掩盖（仅 UDP/TCP 门控 PASS）；framebuffer mmap +2 页逃逸 USER_END 检查且无页引用；vmscan/compaction PTE 写绕锁；set_brk 绕 PTE 锁；scheduler_tick RR 分支 IRQ 内强置 RUNNING；pid_hash RCU 读侧提前退出；单 cfs_rq.curr 多 CPU 记账丢失；handle_cow_fault 独占分支不清 Cow 标志；VmaManager count 原子漂移；epoll_create1 丢 CLOEXEC；pipe2 EMFILE 泄 fd；CLOCK_BOOTTIME/MONOTONIC_RAW 未别名；pselect6 tv_sec 溢出；SyscallNo 死枚举与分发表矛盾；FUTEX_WAKE_OP 桩；mkrootfs 静默省略测试件。

### 17.5 修复记录（2026-09-16，批次 7A/7B-1/7B-2/7C，提交 5afdec1/fa9ea04/abe6b2c/+7C）

- **7A（5 项关键/高危）**：R7-1 mprotect/munmap/madvise(REMOVE) 用户区间界（NEW-C1 类收口）；R7-2 cfs_rq.curr 出队清空（NEW2 根因链主修复——空闲 tick 对已收尸 Task 的 sum_exec_runtime/exec_start 写入）；R7-3 STOPPED 可唤醒（signal_wake_up_state + wake_up 接受 STOPPED；trap.S 出口复查信号使 SIGKILL 语义成立）；R7-4 NR140/141 分发交换复原（getpriority 曾把寄存器残留值写进 nice）；R7-5 VmaManager::add 前向合并吞并检查（verify 1085/3→1088/0）。
- **7B-1（I/O 与内存安全）**：bio evict 重构（bucket 锁内 count 复查+evicting 标记+摘哈希，再 LRU 摘链——VFS-H13 两阶段修复的复活竞态闭口；get() 跳过 evicting 条目）；page_cache 引用语义重写（页出生 ref=0（原出生=1 且无配对 put=永久钉死、逐出全废）、insert 不再重复加引用、invalidate 钉住页标记 invalidated 由末次 put 释放、get() 对 invalidated 服务 miss）；demand-fault 三处 PTE 锁内 pte-none 复查（CLONE_VM 双缺页双映射）；io_uring enter/register/mmap 三处 private_data 移入 LIFECYCLE_LOCK 内读取；exec 分配幂次余量页即释放（~192KB/exec 泄漏）。
- **7B-2（调度/IPC 唤醒正确性）**：do_clone 四错误出口全量回退（pid_hash_remove+free_pid）；handle_cow_fault W 必带 R（PROT_NONE+写 → 保留编码活锁）；DL enqueue 双入队守卫；RT/DL enqueue 返回插入布尔 + grq.nr_running 增减配对（含 pick 侧）；NEW-C2 残留 5 处 no-sleep 复查出队；semop/msgsnd/msgrcv 注册后复查（丢失唤醒）；Semaphore::down() 先注册后减（fetch_sub→prepare_to_wait 窗口丢失唤醒）；sendmsg/recvmsg/mmsg iov 总量 4×RW_CHUNK 封顶；pick_next_cpu 6KB 堆分配改 32 槽栈数组（GDB 实捕的 GRQ 锁内分配）。
- **7C（MED/LOW 扫尾）**：munmap 采集并入同一 vma_write（TOCTOU）；mremap/shmat/framebuffer 映射纳入 PTE 锁；framebuffer +2 页重新校验 USER_END；futex_requeue NR456→ENOSYS（原盲转发可永久睡眠）；futex2 wait/wake 透传 FUTEX2_PRIVATE_FLAG；FUTEX_WAIT|CLOCK_REALTIME 保持相对（6R 过度修复回退，仅 WAIT_BITSET 绝对）；rt_sigpending 改 pending&blocked（原反）；termios ICANON=0x2、c_line 单字节@16、c_cc@17；nettest 判定门控全部组（原仅 UDP/TCP）；handle_cow_fault 独占分支清 Cow 描述符标志；mprotect 跨 VMA 全部更新标志。
- **已回退**：EXT4_BIG_LOCK→Mutex 转换（7B-2 实测 ~50% 启动挂死——信号量互斥路径需独立审计轮；Spinlock 恢复，互斥语义不变，其抢占失速噪声仍为记录在案的 R7-A2 质量问题）。
- **门禁结果**：smoke 15/15 ×4+（一度 ~50% 挂死后修复）；verify 1088/0；build/build-release 绿。NEW2 家族仍以已知形态出现（~1/3）：VF-re 空 b_data panic（ext4 write_inode 拿到 len=0 的 BufferHead——第八轮首要线索：BufferHead 释放后复用/双 miss 竞争）、M-s3 非法指令 epc=0xffffffd600af4004（线性映射低位物理地址族）、FE 静默挂。**nettest 判定修复后不再掩盖失败。**

---

## 18. 第八轮检视（2026-09-16，NEW2 专项 + 回归复查）

### 18.1 NEW2 根因定罪（专项 agent，全量代码证据）

**机制 1（根因，Critical）：RUNNING 任务在上下文保存前即可被全局 pick —— 双核运行同一任务 + 陈旧内核栈**。`__schedule`（sched.rs:711-767）把 prev 重新入全局树并在 **context_switch 之前解锁 GRQ**；而 prev 的 sp/ra 只在 `__switch_to`（context.rs:83-84 的 sd ra/sp）里保存。窗口内 CPU B 可从全局树 pick 到 prev 并以**上一次换出的陈旧 thread.sp** 恢复执行——同一任务、两个 CPU、一条内核栈：已结束的调用尾被重放（brelse/dealloc 二次执行=双重释放）、两路栈帧互踩（保存的 ra 被小整数覆盖=0xaf4000 非法指令族）、fdtable 被重放路径拆除（EBADF）、调度记账损坏（锁自旋 wedge）。**四个签名一次全解释**；且 len=0 的 b_data 实证为"已释放且被复用"的 BufferHead（slab 释放只涂 2 字节，未复用的 len 仍为 4096）。
- 放大器：`wait_buffer_io_done`（bio.rs:756）、`wait_for_desc_completion`/`wait_for_used_interruptible`（queue.rs:393/505）均为 **state=RUNNING 的裸 schedule() 轮询**——每次都开窗且 IRQ 唤醒对它们无效。
- 修复设计（Linux on_cpu 语义）：pick 时置 on_cpu；三类 pick 跳过 on_cpu 任务；`__switch_to` 在保存完 prev 寄存器后（fence rw,rw）清 on_cpu。**首轮实现已验证方向但存在未定位的新生任务路径缺陷（子进程被信号杀死+TIMERS 锁风暴），已回退**——待专项审计（trampoline/clear_fork_child/timer 交互）后重做。
- 机制 2（Critical）：virtio 同步读超时路径（mod.rs:496-504）在描述符仍在飞时 dealloc header/resp 并返回 Err，而 blkdev 层吞掉错误返回 Ok+零数据 —— 设备对已释放堆块 DMA 4KB（游走破坏）+ 零块被标 Uptodate 入缓存。修复方向：Request 带状态回传 + 超时不释放（登记 pending 由 BH 释放）。
- 机制 3（High）：全局 CURRENT_JOURNAL_HANDLE（namei.rs:37）跨 CPU 互踩 + 指向已返回栈帧。修复方向：per-task 字段。

### 18.2 回归复查发现（对 7A/7B/7C 自身的 7 项修正，全部已修）

| # | 修正 | 内容 |
|---|---|---|
| R8-1 | sched.rs | R7-2 的 curr 清空从未生效：pick 掉的任务 dequeue 返回 false，`dequeued &&` 条件永假——退出路径 curr 残留、update_curr 写已释放 Task 的损坏引擎一直运行。改为无条件清 |
| R8-2 | bio.rs | 7B-1 重构自身开窗：Phase 1 只看不摘，两 CPU 可选同一尾部 victim，双双通过 count 复查 → 双摘链（head/tail 清空）+ 双 Box::from_raw。Phase 2 增加 evicting 复查 |
| R8-3 | page_cache.rs | 7B-1 的 insert() invalidated 分支无条件释放旧帧——把 invalidate 承诺不碰的钉住页在 insert 里释放了。改为不缓存直接返回（旧条目由其末次 put 释放） |
| R8-4 | page_fault.rs | R7-C3 竞争失败方返回 AlreadyMapped，trap 处理器将其映射为 SIGSEGV——竞争失败被杀。改返回 Handled（指令重执行） |
| R8-5 | wait/signal/process/exit | 7B-2 的 5 处 NEW-C2 出队在噪声期二分中被误删（提交信息与树不符）——全部恢复 |
| R8-6 | semaphore.rs | down() 注册改为 EXCLUSIVE（尾插 FIFO）——非独占头插让瞬时 fast-path 注册者可偷走 up() 的唯一唤醒令牌 |
| R8-7 | mm_ops.rs | munmap "单锁" 实为两个临界区（guard 中途 drop）——真合并；首版手术残留重复 vma_write() 自死锁已修 |

### 18.2b on_cpu 首次实现回退记录与重试要点（给下一轮）

症状：子进程（含 shell exec 的 smoke_test 与 nettest FE/my_clone 裸 clone 子进程）以信号死亡；TIMERS BTreeMap 锁风暴（3 CPU 自旋）；shell prompt 前静默挂。已核对非因：new_task_at 两条 ptr::write 初始化路径均含 ti_on_cpu=false；idle 标记无害；`next==prev` 早退路径已推演安全。
重试前必查（按序）：
1. 新生任务首跑路径：copy_thread 设定的 trampoline（ra/sp）到 ret_from_exception 之间是否有**不经过 __switch_to 的换出点**（例如 trampoline 直接 schedule 或经 cpu_idle 路径绕过 context_switch），导致 on_cpu 置位后无人清除 → 永不可 pick → 看门狗/信号风暴。
2. `mark_picked_on_cpu` 对 `pcpu.idle` 返回值的处理：首版未标记 idle（保守），若标记则在 `next == prev(idle)` 早退路径 idle.on_cpu 残留 1 直到下次真实切换——检查该窗口内 pick 跳过逻辑是否因此误判"无可运行任务"触发 timer 风暴。
3. CFS pick 的 remove/reinsert 抖动放大：on_cpu 窗口内任务被反复 remove+stash+reinsert（每次 pick O(n)）——把 on_cpu 检查改为**只读跳过**（不 remove，改用游标推进或惰性跳过）可消除风暴。
4. 用 GDB 在 do_exit 断点捕获"信号死亡"子进程的 pt_regs（信号号+epc）直接定位。
配套（机制 2/3，独立可做）：virtio read_block 超时路径不释放在飞描述符 + Request 状态回传；CURRENT_JOURNAL_HANDLE per-task 化。

---

## 19. 第九轮检视（2026-09-17，3 agent：腐蚀专项 + A/B 全库扫描）

### 19.1 定罪：children-list 全零节点 = new_task_at 未初始化 wait_chldexit

Task 由 buddy（页粒度、释放不清零）整页分配；`new_task_at`（task.rs:1031-1284）**从不初始化 `wait_chldexit`（Spinlock<Vec<WaitQueueEntry>> @0x798）**（也不初始化 comm/pdeath_signal/dumpable/ti_a0-2）——每次 fork 拿到的是上一任占用者的字节。do_wait 首次 `prepare_to_wait → Vec::insert(0, entry)` 对垃圾 {ptr,len,cap} 操作 = **野指针 16 字节写 + len×16 字节拷贝**，可命中任意活 Task 的 sibling/children（= 全零/断链节点）；Spinlock 字为垃圾非零值 = 永久自旋（DEADLOCK 面）；deferred notify 按 PID 找到错误新任务再操作其垃圾队列 = 二次放大。**残留 ~1/6 NEW2 家族的本体。**
配套确认：buddy dealloc 无 double-free 校验（M3，页双主并发的使能器）；fork 三条 unwind 泄漏 16KB 内核栈；free_task_slot 无 Drop 泄漏内部 Vec/Box。

### 19.2 全库扫描发现（A/B agent）

- **[HIGH] 静默挂（FE/P2c）根因**：deferred exit notify 在 __schedule 尾部处理，但下个 pick 若是**新生任务**则 __switch_to 直接 ret 到 ret_from_fork，永不回到 __schedule 尾部——`schedule_tail` 是空占位；槽位被下一次退出覆盖 → 父进程 wait 永睡（B1）。
- **[HIGH] ZOMBIE→defer_exit_notify 之间被抢占**：timer IRQ 在窗口内 schedule → 退出尾部永不执行 → 通知丢失（A7）。
- **[HIGH] blkdev_write 仍吞设备错误**（R8-M2a 只改了 read）；PCI virtio handler 从不写 req.error（A1/A2）。
- **[HIGH] virtio alloc_desc 以链数而非描述符数限流**：queue_size=8、每请求 3 描述符，≥3 并发即描述符混叠——设备完成错误链/双等待者匹配同一 used 项（A3，SMP 下的活跃腐蚀源）。
- **[HIGH] rmap try_to_unmap 只做本地 sfence**——换出任务在其它 CPU 上继续用 stale TLB 访问已释放页（A9）。
- **[MED] ext4 file.rs journal_handle 指向 match 臂绑定后立即 move**——M3 修复自身在主写路径上仍有死帧解引用（A4）。
- **[MED] KERNPANIC 停机是 debug-only**——release 版继续 sret 回故障 epc 无限 fault 风暴（A5）。
- **[MED] bio get() Phase-3 重复检查把 Phase-1 tripwire 刚拒绝的死 bh 又递出去**（B2）；bread_async 查找无 tripwire/evicting 跳过（B7）。
- **[MED] do_waitid 缺 prepare_to_wait 后的僵尸复查**（B3）；IPC 唤醒后不校验 seq（RMID+复用可作用于新对象）（B8）；deferred notify 的 parent 指针跨 send_signal 使用（窄 UAF，B9）。
- **[MED] M2b 不完整**：超时+设备迟到完成时调用方 4KB 数据缓冲仍可能被 DMA（B5）；5000 次迭代预算非时间制 + 50M 自旋可持 EXT4_BIG_LOCK（B6）。
- **[LOW] termios 拷 60 字节进 52 字节结构（越界 8 字节）；wait_event_interruptible 信号路径缺 R8-5 出队；msg_iovlen 静默钳制；utimensat NR88 假成功；rootfs 组件空数组下溢；trap.S 死偏移常量；KERNPANIC 硬编码偏移。**
- **已核净**：on_cpu 协议全路径完整（含新生/仍胎/自快路径）；release_task 自旋无死锁（rcu 侧证明）；sem/msg 记账四路径平衡；mprotect/munmap 界在位；页缓存 insert 早退不致循环。

### 19.3 修复批（R9）

M1 补 new_task_at 缺失字段（wait_chldexit 等全量）；schedule_tail 处理 deferred notify；ZOMBIE→defer 区间 preempt_disable；blkdev_write+PCI handler 错误回传；alloc_desc 按描述符数限流；journal_handle 先绑定后设置；KERNPANIC 无条件停机；bio Phase-3/bread_async 死 bh 防护；do_waitid 复查；IPC seq 复验；deferred notify 父指针二次查找；fork 三 unwind 补 free_kernel_stack；buddy dealloc double-free tripwire；termios 52 字节；reap 自旋前开中断；wait_event_interruptible 信号路径出队；rmap 换出加全量 sfence（远程 shootdown 简化为全局冲刷）。

- **R9 门禁结果（8 轮）**：smoke 15/15 ×7（+1×14/15 已知陈旧项）；nettest 全 PASS 4/8；KERNPANIC 1/8（enqueue_task_locked 解引用野指针 0xffffffffdd33e8b8——wake 链上的坏 Task*，待查）；DEADLOCK 1/8；**wait4 状态损坏 0/8（原 ubiquitous）；buddy double-free 探针 0/8**；bio 死 bh 探针触发 3 次且全部被吸收（记录+按 miss 处理，未升级为 panic——防护生效）。
---

## 20. 第十轮检视与修复（2026-09-17）

### 20.1 PIPE2 EBADF 定罪（原始触发器）——pipe close 按"事件"而非"最后引用"释放

`FdTable::close_fd`/`Drop` 对每次 fd 关闭都调用 `(*file).close()`，但 File 是被所有 dup/fork 描述符共享的 Arc：`dup3(w,1); close(w)` 后 close **偷走共享 File 的 private_data** 并消耗 Pipe 引用 → exec 后 echo 对 fd1 write 得 EBADF（4/4 确定性复现）；socket/procfs/epoll close 同族。修复：仅当 `Arc::strong_count==1`（最后引用）才执行 close 语义。**修后 nettest 6/6 全 PASS、PIPE-OK 6/6、mrsh `echo PP | cat` 与重定向可用——第七轮以来首次。**

### 20.2 其余 R10 修复

wake 收集-后唤醒的 UAF（wait.rs/futex.rs 延迟 wake 野指针 → enqueue_task_locked 崩）尝试以 RCU 读侧包裹；**共享核心包裹实测引入新崩溃类（4/6）已回退**（专项重做：call_rcu 化 free_task_slot）；virtio 描述符限流修正 off-by-one（in_flight*3+3>queue_size）；alloc_desc 失败路径补 dealloc；msg 循环顶部 seq 复验（R9-11 作用域漏洞）；new_task_at 栈分配失败传播（不再带 sp=0 继续）；R9-16 restore_irq 真正落地；R9-18 全量 sfence 回退（sfence.vma 是 hart-local，全量无意义——远程 shootdown 为已记录开口项）。

### 20.3 门禁与遗留
### 20.7 第十四轮：全库补盲检视（net/drivers/interrupt + mm回收/ext4日志/dfx/security/boot）

> 覆盖审计承认 7-13 轮为签名驱动；本轮两个 agent 把**从未深扫**的子系统全部逐行扫完（每文件 clean/dirty 表见 agent 报告）。共 **16 HIGH / 17 MED / 29 LOW**。

**net/drivers/interrupt（agent 1）**：HIGH-1 irq_exit 内联 softirq 不查 in_softirq（lock_bh 是谎言——持锁者被 IRQ 打断后 handler 自旋自己的锁）；HIGH-2 LO_BACKLOG/ARP_CACHE 纯 lock() 而被 Timer 软中断 10ms 一次命中（同 CPU 永久楔 = DEADLOCK 面新源）；HIGH-3 TCP 服务端握手 SYN 序号从不 +1（accept 后发送窗计算环绕 → 服务器侧永久堵死）；HIGH-4 FIN 从不重传且 FIN_WAIT1/2 无超时（64 槽永久泄漏）；HIGH-5 close_fd 最后引用判定在并发 get_file 克隆下**永久跳过 close op**（管道 EOF 丢失 = 挂起/EBADF 面）且 R13-4 把 op 执行也移进了 entry 锁（socket close → virtio 自旋在锁内+IRQ off）；HIGH-6 UDP_SOCKET_TABLE static-mut 无锁；HIGH-7 TCP 槽被 timer 释放而进程 fd 仍在用（跨连接数据混淆）。MED：virtio-net RX 因 blk 的链限流只剩 2 缓冲、xmit 泄漏 hdr、poll 早退泄漏 RX 缓冲、xmit 超时后释放 DMA 中内存、virtio-input used 环 +8 应为 +4（越界读写 desc 表）、5 处 ECAM slot×8 未跟随、TCP 发送环不扣窗、RST 无序号验证、UDP 接收无界（远程 OOM）、socket 创建错误路径泄漏槽、PLIC 使能字非原子 RMW。LOW×14。
**mm/ext4/dfx/security（agent 2）**：F1 allocator.rs:478 `BISECT-NO-PLUS1` 标记仍在（EXT4-H3 "修复"只有注释没加 +1——inode 号错位一位）；F2 jbd2 并发 journal_stop 可双提交、start_this_handle 不查 t_state（提交中注册的缓冲**永不入日志**）；F9 Zone::free_pages 仍无 double-free 守卫（主分配器路径！R9-14 只装在没用的 BuddyAllocator 上）；F10 vmscan `put_page()<=0` 仍 free_page（**活的双重释放**——与 F9 组成 NEW2 型同页双主）；F14 swap_init 零调用（整个 swap 路径运行时死路）；F5 rename 覆盖文件目标不释放数据块；F6 目录迭代对删除项递归（50 连续空洞爆 16KB 栈）。MED：compact 迁移源不摘 LRU/目标不加 LRU、remap_page 仍覆盖 COW 子进程页、rmap/compact PTE 写仍绕 PTE_MODIFY_LOCK（§17.4 开口）、mount 重挂 ext4 换 GLOBAL_EXT4_FS。LOW：jbd2 commit 错误路径 bh 泄漏、capset inheritable ⊆ permitted 应为 ⊆ inheritable、ambient 合成而非真字段、3 处 10MHz 魔数漏网、kfree 无对齐校验、kswapd 丢唤醒（已知族）、swap 超容文案与行为相反。**security wave-1/2 修复 13 轮后完好**；boot 序列 NEW-C4 保持。

**修复优先级**：F10+F9（一行修双重释放）、HIGH-2/HIGH-1（新楔源）、F1（删标记加 +1）、HIGH-5（close op 移出锁+真最后释放）、HIGH-3（服务端 +1）、F5/F6。

### 20.40 第四十八轮：毒化验证全过 + 不变式缺口闭合 — 循环阶段收官

**R48**：R47 毒化的对抗验证全部通过（写/读格式精确匹配 offset_of 实测 0x50/0x54、写序无未防护窗口、new_task_at 复用即清毒、全树无残留硬编码 0x48）；R46 phantom 与毒化交互正确（毒化值永不进 phantom 分支、dead-guard 先拦）。**1 项确证发现已修**：alloc_task_slot 失败路径裸 dealloc 未毒化（new_task_at 已写真 state/pid 覆盖前世毒化后返还）——击破"Task 页返还必带毒"不变式，且 R46 heal 会把半初始化任务（垃圾 thread.sp）重新入队切换。修复：改走 free_task_slot。**全树 Task 页返还路径不变式闭合**（fork×4、task_put、alloc 失败×2）。
**最终门禁**：single 7/8（run3 B 型 1 次）+ multi 3/4（run9 B 型、run12 A 型各 1 次）——B 型从 ~50% 压至 ~10-15%，**未归零**。
**循环的诚实收官**：R39→R41→R42→R46→R47→R48 六层递进每层真实前进（幻影 RUNNING→防御→窗口→外部覆写→毒化偏移→不变式），残留 ~10% 的 B 型家族说明陈旧唤醒毒化之外仍有未定位产生器（或毒化未覆盖的释放路径）。零发现停止条件未达成——循环保留开放，方法论（活体快照/周期快照/同址追踪/对抗推演）与工具链（hunt/probe/DFX 三开关/owner/bt）全部入库待续。

### 20.39 第四十七轮：终极拼图 — 毒化偏移错 8 字节，全部死任务防护从未生效

**R47 双线收口**：
- 块 I/O 的 DMA/缓冲面全部算术验证无越界（blkdev_read 的 vec 恰等长、五条提交路径 desc len = buf.len、R17-C 合并块内 len 和 17≤64、bio 无跨段合并、页粒度堆无小块邻接 OOB）。
- **真凶**：`free_task_slot`（sched.rs:726）的毒化硬编码 `+0x48` 写 u64，而 state 在 **+0x50**、pid 在 **+0x54**（trap.rs R20 记录过字段插入的 +8 漂移；编译期 offset_of 确认）——**毒化 42 轮以来从未生效**，Task::wake_up / enqueue_task_locked / ensure_linked_locked / trap 的全部死任务防护读的是从未毒化过的 pid/state，全部空转。**R46 的"外部 4 字节零覆写"就是陈旧唤醒自己的 set_state(RUNNING)（状态字处写 0）落在已释放复用的 Task 页**——同地址双 Task 幻象（0xffffffd600e55000 的 306→319）、跨 pid 同型栈（位置性写入）、块 I/O 受害者语境（306 停在锁释放、319 死于 vec![0;n] 的 buddy 清零——都是受害者上下文而非写者上下文）全部解释。
- **修复**：毒化改用编译期常量 `TASK_STATE`/`TASK_PID`（offset_of!）两次 u32 写（未来字段插入不再静默漂移），更新两处陈旧契约注释。R15-6/R33/R46 的全部防护自此真正武装。
**门禁（修复版）**：single 8 轮 7 绿 + 1 次 A 型（模拟器 artifact）；**multi 4 轮 4/4 全绿——B 型（唯一真实内核缺陷）零复现**。
**循环状态**：B 型归零、A 型定性非缺陷、R40 零发现轮在案——**审查循环的停止条件达成**（R48 做最终零发现确认后收官）。

### 20.38 第四十六轮：B 型产生器定性为外部内存覆写 + 调度器侧不可维持性收口

**R46 穷举证明**：__schedule 的 prev 不变量 + 全部 set_state/脱链写点 + 单 GRQ 锁覆盖——任何合法交错满足"离 CPU ⟹ (RUNNING∧链入) ∨ (¬RUNNING∧脱链)"；**捕获形态（RUNNING∧on_rq=0∧离CPU∧栈冻结于 trap 出口）不在合法交集中**。结合 R41 tripwire 历次零触发、RUNNING==0（零写入恰造此形态）、跨 pid/轮次栈形完全同型（竞态断点会漂移、位置覆写不会）——**产生器是外部 4 字节零覆写 Task+0x48（state 字段）**：SIGSTOP 停止的管道子进程（STOPPED）被零覆写为 RUNNING——历档 smash 家族（R18-3/R12-4/R15-6）的位置性覆写引擎。
**修复（调度器侧不可维持性，R41 家族镜像收口）**：wake_up_enqueue 拒绝分支内 GRQ 锁内权威 phantom 判定（RUNNING∧!on_cpu∧!is_linked → R46-RUNNING-PHANTOM tripwire + 治愈链入）；Task::wake_up 与 signal_wake_up_state 的 RUNNING∧!on_cpu hint 路由进权威复查——**唤醒路径每个拒绝分支现在要么转换+链入、要么验证本已可调度、要么修复发散**。
**multi 门禁 7/8**：run3 边界情形（幻象后无任何唤醒事件指向它——治愈点不可达）——治本需抓覆写源头。
**R47 计划**：taskdump 增加 Task 结构地址打印 → 复现时经 gdbstub（-s + icount 确定性）对 Task+0x48 设 hardware watchpoint → 覆写者一次现形（候选：alloc 清零路径越界、fork 子任务清零残余、zero_page 批量清零）。

### 20.37 第四十五轮：周期快照 DFX + B 型最终形态锁定（RUNNING + on_rq=0）

**dfx=periodic 运行时开关**（dfx/switches.rs + timer.rs 软中断尾部锁外，5s 周期，SBI 直写）：静默挂起不触发任何 watchdog 且挂起后 shell 停止消费 RX（magic 失效）——定时快照从 timer 软中断必然落地。周期快照会干扰基于提示符的外层检测（误报 timeout），但快照本身完整可靠。
**B 型最终形态（265 个周期快照的尾帧实锤）**：pid 335 **state=RUNNING、on_rq=0、4 CPU idle**——任务被置 RUNNING 但不在任何类队列。栈与历次捕获同型：`trap_exit → asm_need_resched → schedule` 链——**产生器在 __schedule 的 pick/prev 处理与 need_resched 的交互**（候选：pick 快路径重选后 stale need_resched 再次进 schedule 的路径；prev 重入队与 pick 出队的窗口）。**下一轮（R46）从该栈 + on_rq 语义直接推导产生器并修复——B 型归零即达成最终零发现收口。**

### 20.36 第四十四轮：A 型定性为模拟器 artifact + B 型残余为真并发 bug

**tcg thread=multi 对比实验（决定性）**：同一二进制，multi 下 8 轮门禁——**A 型死锁零出现**（single 下 ~12-25%）——**A 型（持锁消失、锁漂移、无输出）确证为 QEMU tcg 单线程 vcpu 互斥推进模型的 artifact**：单线程下某 vcpu 持锁窗口与其他 vcpu 的中断/设备模型交织产生的、真实硬件（真并行）与多线程模拟器上不存在的形态。历轮对 A 型的修复（锁内分配、复活链、防御）仍是正确的健壮性提升，但 A 型本身不再是内核缺陷。
**B 型（管道静默挂）在 multi 下 2/8 仍现**——**真实内核并发 bug**，R42 的 SLEEPING-but-linked 修复未覆盖全部产生器。multi 提供更真实的并发暴露（probe 的 pipe 10 STALL 捕获），但 magic 注入未达 RX 中断（multi 下 UART IRQ 路由/挂起形态差异，待下轮：multi+monitor 或 guest 侧周期 taskdump）。
**循环指令的状态**：A 型关闭（非缺陷）；B 型残余为唯一开放内核问题；R40 零发现 2/2 已达成过一轮——B 型残余归零后即可达成最终零发现收口。

### 20.35 第四十二/四十三轮：SLEEPING-but-linked 丢失唤醒（残余的最简产生器）+ 堆锁 ECALL 排除

**R42 终验发现 1 项 HIGH（已修）**：R39 的 set_state 重排序在"SLEEPING-but-linked 窗口"上破坏了 pre-R39 语义——睡眠者置 SLEEPING（waitqueue/futex 锁下，非 GRQ 锁）到自身 __schedule 摘链之间任务仍在类队列；窗口内 waker 的一次性 token 先耗（wait entry 标 woken/futex 摘链，结果被忽略）→ wake_up_enqueue 因"已在队"拒绝且不置 RUNNING（R41 返回 true 掩盖）→ 睡眠者照常摘链睡去 → **永久挂死**。R41 tripwire 不触发（任务真链入，非 stale flag）。修复：ensure_linked_locked 验证真链入时恢复 set_state(RUNNING)（比 pre-R39 更窄：仅 GRQ 锁下验证链入时翻转）。**这是 R39 以来历轮门禁静默残余的最简可构造产生器。**
**R43（A 型假说排除）**：堆锁内 R18-1/R9-14 探针的 SBI ECALL（OpenSBI M 态持 console 锁 + 轮询 UART THRE 的双 hart 死等假说）改为 taint 标志（零 ECALL under any lock）。门禁 6/8——A 型仍现，**ECALL 假说排除**。
**A 型假说矩阵（全部排除或部分解释）**：锁序环（排除，锁序图无环）、锁内分配 panic（R34 修复后仍现）、SBI ECALL/串口背压（R43 排除）、wake 复活（R33 修复后仍现）、prev 重入队幻影（R41 防御 + R42 窗口修复后仍现）。残余特征：持锁 CPU 无 panic 无输出消失、锁漂移（TIMERS/堆/TCP/ROUTE/GRQ）、~12-25% 布局敏感。候选方向（R44+）：QEMU tcg thread=single 的 vcpu 停喂模型、OpenSBI 更深行为、或持锁路径的其余外部交互（virtio MMIO 写在 QEMU 主循环忙时）。
**循环状态**：R40 零发现 2/2 + R42 发现 1 HIGH——零发现条件未最终达成；A 型为唯一开放问题。

### 20.34 第四十/四十一轮：零发现轮 2/2 达成 + B 型活体栈符号化 + prev 重入队防御

**R40 零发现轮（2 agent）**：sched/mm/core **零功能缺陷**（R39 三条 inserted=false 路径推演全过、STOPPED/传播语义、mm 回归、信号全流、DFX 自审；1 处契约注释纠偏）；net/fs 四项重点全过（TcpTxBatch 语义、锁序无新环、sendto 边界、file_id 无截断）+ 外围深挖 1 确证：**IPC 代际 16 位截断**（u32 seq 存储与 16 位编码比较不一致——65536 次分配后 SysV IPC 整体 EINVAL，已修为掩码存储）。
**R41 B 型活体栈（诊断内核 dfx-taskdump-bt 首战）**：pid 359/360 幻影 RUNNING 的栈顶链 `trap_exit → asm_need_resched → schedule`——任务在 trap 出口抢占点被切出后未被重 pick。修复：`ensure_linked_locked` 验证式防御（enqueue 被拒路径核实 is_linked；未链入则 R41-STALE-ONRQ tripwire + 清陈旧 flag + 重插；已链入则跳过防双链），覆盖 __schedule prev 重入队（requeue_prev_locked）与 wake_up_enqueue 双入口；三类队列新增 is_linked()（CFS/DL 指针扫、RT O(1) 自指检测）。
**门禁 6/8**（run1 A 型、run2 B 型残余；tripwire 零触发——残余幻影不经拒绝路径）。**第五次布局敏感**：on_rq 字段加入 taskdump 后 10/10 全过——待其复现时 on_rq 一锤定音（在队=pick 侧树/键损坏；不在队=入队侧）。

### 20.33 第三十九轮：B 型按构造闭合 — enqueue 幻影 RUNNING 根因 + 5 项修复

**R39 专项审计（交错推演全覆盖）**：
- **NEW-C2 补偿误摘——排除**（反证成立）：全部 24 处补偿点含任意指令边界抢占的完整推演——每处 dequeue 作用于执行中任务，RUNNING+离队对执行中任务正确，下一次 __schedule 必然重入队；双重摘除在 GRQ 锁全序下不可构造。
- **B 型唯一产生器锁定**：`enqueue_task_locked` 的**无条件先置 RUNNING 后类插入**——类插入经 on_rq 守卫静默拒绝时（陈旧 on_rq=true），任务幻影 RUNNING：不在任何队列、不在任何 CPU、无人再调度——**与 icount 活体快照（pid 344 RUNNING + 空 GRQ + 4 CPU wfi）精确形态匹配**。
**修复（5 项）**：
1. set_state(RUNNING) 移到插入决策之后（仅 inserted==true 才写；被拒时诚实保持 SLEEPING 可再唤醒）；4 个调用点全部核验行为等价。
2. wake_up_enqueue 传播插入结果（拒绝不再谎报成功）。
3. dequeue_task 的 policy() 读取移入 GRQ 锁内（与 change_task_policy 竞态会摘错类队列）。
4. RR tick 轮转配对 nr_running 计数（原每次轮转泄漏 1）。
5. sysv_msg/sem/posix_mq 共 5 处 add()+set_state 非原子 → 原子 prepare_to_wait（一次性唤醒令牌在 RUNNING 时被消费 → RMID/空 mq 收端永睡，form-A 确证）。
**门禁 7/8**：B 型 8 轮零复现（闭合确认）；run8 A 型死锁（锁漂移至 GRQ，~12% 残余）。taskdump 栈回溯 feature 化（dfx-taskdump-bt，默认关——其代码曾使堆锁死锁率升到 50%，违反 DFX 零开销原则）。
**A 型残余（唯一未解）**：持锁者消失无 panic 无消息，锁在 TIMERS/堆/TCP/ROUTE/GRQ 间漂移——R40 用 owner feature + 栈回溯 feature 的组合构建伏击。

### 20.32 第三十八轮：B 型确定性研究 — RUNNING-but-unscheduled 形态确认 + 栈回溯工具

**icount 确定性捕获链闭环**（test/probe-wedge.py）：icount 下 B 型在特定二进制 100% 复现（第 4 次 `echo PP | cat` 必挂）；探测器按 guest 输出节奏驱动（icount 下等待不改变指令时序），挂起后注入 UART magic `DUMP!` → RX 中断触发 taskdump——**首次拿到 B 型挂起时刻的活体任务快照**。
**B 型形态确认**：pid 344（mrsh）**state=RUNNING 却不在任何 CPU 上**（4 CPU 全 wfi idle、运行队列空）——**置了 RUNNING 但不在 GRQ**。头号嫌疑：R36 的 pipe/io_completion NEW-C2 撤销（dequeue_task 补偿）在特定时序下误摘仍需运行的任务，或 wake_up 置 RUNNING 与入队之间的窗口。次嫌疑：344 是 mrsh 的 wait4 轮询路径。
**布局敏感性第四次证实**：任何代码改动（本轮 taskdump 加回溯）都使 B 型在 icount 下消失——时序依赖竞争的标准形态；"改代码→复现消失"的迭代是死路，后续以**剩余嫌疑面代码审计**（NEW-C2 补偿族正确性、wake_up_enqueue 入队完整性）+ 栈回溯工具伏击（任何布局复现即出栈）推进。
**DFX 增强**：taskdump 每任务打印 thread.sp + 内核栈向下扫描的 6 个代码区返回地址（粗回溯，SBI 直写中断安全）。

### 20.31 第三十七轮：timer 槽位表重构搁置 + icount 确定性捕获工具（观测者悖论攻破）

**R37 重构（timer.rs 两表合一固定槽位、位图引导、锁内零分配、18 调用点零改动、语义逐 API 保持）技术完成但搁置**：R37 版门禁 5/8（3 死锁）vs R36 基线同期 4/4 + 7/8——差异在门禁自身 ~12-25% 时序波动范围内，收益未证明且回归风险存在。草案留 docs/development/r37-timer-slotted-table-draft.rs，待持有者身份明确后再评估合入。
**锁漂移确证**：残余死锁的锁形态随修复迁移（TIMERS→堆锁→ROUTE_TABLE）——持有者死于与具体锁无关的路径：不 panic（putchar_no_lock 直写 THR 无 THRE 轮询，panic 消息不可能被吞/背压）、不打印、不在任何 CPU 上（icount 现场：cpu2 等锁、cpu0/1 存活于中断处理、cpu3 内核取指 fault）——**持锁被调度走/持锁死循环**是剩余假设。
**icount 确定性捕获（R38 标准武器）**：`-icount shift=2` 使时序按指令数确定推进，monitor 不再影响复现——首轮即捕获完整 4-CPU 现场（此前 owner/monitor/FIFO 一切观测手段都掩盖复现）。工具入库 test/（hunt-wedge.sh 的确定性模式）。dump 栈提取的 $sp 展开问题待修。

### 20.30 第三十六轮：零发现验证轮 — 1/4 零发现 + 7 项确证（3 HIGH）

四个复审 agent（net / fs+ipc / sched+mm / core+drivers）对 R32-R35 改动面对抗验证：
- **sched/mm：零发现**（wake_up_enqueue 原子性/STOPPED 语义/timer budget 无饿死/nanosleep 中断约定/rmap-vma 锁序全部通过）。
- **core/drivers：3 项**——(H) send_signal_locked 残留 R32 前的顶层 pending.add 未删，三处 add 架空 prepare_signal 语义（SIG_IGN 残留瞬态位虚假 EINTR；RT 信号双重入队且 remove_one 只清 bitmap → 队列无界泄漏）；(H) setup_frame SA_RESTART 分支不还原 a0=orig_a0，重启的 ecall 带 -512 当第一参数（read(-512)→EBADF）；(M) taskdump 的 pid_hash 桶锁阻塞迭代在死锁现场会自身卡死（持桶锁调 wake 的嵌套是现实路径）→ 改 try-lock 跳过并标注 stuck 桶数。
- **net：1 项**——(H) R35 给 sys_sendto 的 kbuf try_reserve_exact(len) 无上限（len≤access_ok 256GB），30MB send 瞬时独占 94% 的 32MB 堆，期间他 CPU 未 try_reserve 化的分配 panic（R34 根因族回归）——按类型限幅（TCP 256KB 部分写 / UDP 65507+EMSGSIZE），暂存上界 256KB。其余 TcpTxBatch 9 记录点/emit 部分失败一致性/陈旧 ACK/ARP flush-GC 竞争全部验证通过。
- **fs/ipc：3 项**——(H) epoll file_id 用 Arc 堆地址，slab 尺寸分类分配使 close-reopen 工作流的地址+fd 号复用成为**预期行为**（事件错关联/合法 ADD 误 EEXIST）→ File 增加单调 generation file_id；(M) pipe 读写与 io_completion 共 5 处中止路径漏 R8-5/R9-17 NEW-C2 撤销（违反"运行任务不在 GRQ"不变量）；(L) mq_open find-then-create TOCTOU 同名双实例（第二个永生泄漏）→ mq_alloc 锁内复查+重试。
**门禁 6/8**（run2 B 型 + run6 TIMERS 锁死锁）——File 加字段致布局抖动，暴露 R34 留档的最后一颗雷：**add_timer_wakeup 的 BTreeMap insert 节点分配仍在 TIMERS 锁内**（当时标"冷路径不修"）。R37 以固定槽位表根治 TIMERS/ACTIONS。

### 20.29 第三十五轮：TCP 表锁出锁发射重构 + ARP 队列 + virtio-blk MMIO — 门禁再 8/8

**TcpTxBatch（链 2 根治）**：新增统一"锁内决策/锁外发射"机制（tcp.rs）——每个 TCP_TABLE_LOCK 持有者**取锁前** try_reserve 预留描述符数组+字节 arena（OOM → 干净 ENOMEM 降级，非锁内 panic）；锁内 5 个发送原语（send_syn/synack/ack/fin/tx_segment）只记录 wire-ready 描述符 + memcpy 进预留容量（**锁内零分配、零发包、零自旋**）；出锁后 emit_all 逐段 alloc_skb→build→xmit。7 处发射点全部出锁（tcp_rcv/connect/timer tick×2/Socket send/recv/close/shutdown）；sys_sendto/recvfrom 的用户拷贝移到锁外（try_reserve + copy_from_user）。重入安全与 Socket::close 同论证（loopback 只入队、virtio 直发，不回调本 CPU tcp_rcv）。残留的 recv/send_buffer VecDeque 增长与 retrans 段 owned Vec 均 try_reserve 化（失败优雅降级）——**锁内已无任何不可失败的 panic 分配**。
**ARP pending 队列**：固定槽位数组（32 总量/每 IP 4/3s 超时/1s 请求间隔），零锁内分配，GC 在 NetRx 软中断锁外释放——首包不再靠广播重试掩盖。
**virtio-blk MMIO**：feature negotiation（VERSION_1 word1 协商 + FEATURES_OK 验证）+ notify 4 字节写——MMIO 块路径首次符合 spec v2。
**R35-fix（总控抓的回归）**：sys_sendto 的 dest_addr 解析被误改为裸 from_raw_parts 解引用用户指针（syscall 上下文 SUM=0 → 确定性 KERNPANIC @SockAddrIn::addr）——改回 copy_from_user 异常表拷贝。
**门禁**：8/8 全绿（smoke 15/15×8、nettest 8/8、pp/x 8/8、kpanic 0、deadlock 0）。
**遗留**：RX 中断路径单独确认（驱动 agent 额度中断未完成）；TCP 正向数据流量真实 host 验证；RST 后 recv 报 ECONNREFUSED。

### 20.28 第三十四轮：死锁残余清零 + virtio-net 真实流量打通 — 门禁历史首次 8/8

**锁内堆分配审计（统一起因假说验证成立）**：全局锁序图实测无环（TIMERS→ACTIONS/BUDDY；TCP→LO_BACKLOG/TX_QUEUE/BUDDY/GRQ；waitq→GRQ；UART/GRQ/BUDDY 无出边）。permanent wedge 的真身是三链：
1. **链 1（TIMERS/TCP 两型统一起因，已消除）**：锁内堆分配失败 → `#[alloc_error_handler]` panic（panic=abort 无 unwind）→ panic 后 `loop{wfi}` **持锁永停**——幸存 CPU 各自的锁依赖随机决定卡 TIMERS 还是 TCP 锁（与两型现场签名吻合）；panic 短消息被测试输出淹没故从未见到。修复：timer 软中断临界区**零分配**（EXPIRY_BUDGET=64 锁外预留容量 + budget 计数保 push 永不增长 + 周期定时器原地改 expires 重挂，消灭 remove+insert 节点 churn）；tcp_timer tick 的 Vec::collect → 定长栈数组[64]。
2. **链 2（TCP 型放大器，列报告）**：TCP_TABLE_LOCK→virtio TX 锁内 10M+50M 次自旋（tcg 下秒级/包）——根治需 TX 移出表锁的重构。
3. **链 3（已消除）**：Socket::recv_queue plain lock 同 CPU 被 NetRx 软中断重入——recv/enqueue_packet/poll 全部改 lock_irqsave。
附带：GRQ/BUDDY 锁内探针打点改 SBI 直写（删 GRQ→UART、BUDDY→UART 边）；wait.rs 的 wake_up 持锁收集 Vec 改迭代内直 wake（消除事件源热路径的锁内堆分配——总控复核时发现）。

**virtio-net 真实流量首次打通（SLIRP PASS）**：此前全部测试流量走 loopback 短路，驱动从未见过真实设备。QEMU 正确参数（`/usr/bin/qemu-system-riscv64` + `-netdev user -device virtio-net-device -global virtio-mmio.force-legacy=false`）。修复 10 项：feature negotiation 从未写过（VERSION_1+MAC，每帧错位 2 字节的根因）、RX/TX 队列角色反转（spec 5.1.3）、notify MMIO 必须写 4 字节（u16 全部被丢弃）、ArpPacket packed（2 字节 padding 空洞）、设备真实 MAC、本地 IP 10.0.2.15、SYN_SENT 收 RST 不 abort、CLOSE 后 recv EOF、MTU 读偏移、探测失败静默。实测：ARP 双向、TX 链完成、RX DMA+phys_to_virt、UDP DNS 往返（94-97B 应答）、TCP SYN→RST 正确关闭；pcap 8 包干净会话；nettest 无回归。遗留：首包广播重试掩盖（无 ARP 队列）、RX 中断路径未单独确认、virtio-blk MMIO 路径同样缺 negotiation（PCI 不受影响）。

**门禁**：8/8 全绿（smoke 15/15×8、nettest 8/8、pp/x 8/8、kpanic 0、deadlock 0）——对比 R31（pp 4/8 x 1/8）、R32（7/8）、R33（6/8）。25% 复现的死锁在 TIMERS 零分配修复后消失。

### 20.27 第三十三轮：等待/唤醒协议专项 — 5 项修复 + 观测器悖论确立

**专项 agent 修复（5 项）**：
1. **A-1 (H)** `Task::wake_up` 陈旧过滤复活窗口——无锁 set_state(RUNNING)+enqueue 与 do_exit/reap 竞争：窗口内任务可被唤醒、跑完、退出、reap（栈已释放），陈旧唤醒再把死任务入队，第三 CPU pick 后 `__switch_to` 装载已释放的 thread.sp——与 A 型现场（trap 序言在堆地址 store fault 持 TCP 锁卡死）完全吻合。修：新 `sched::wake_up_enqueue`——GRQ 锁内原子完成重验（仍为 sleeping/STOPPED）+ 转 RUNNING + 入队；论证：锁下仍睡的任务不可能到达 do_exit（唤醒与 pick 都在本锁下）。
2. **A-2 (M)** `enqueue_task_locked` 增加 is_dead()（ZOMBIE|DEAD）二道闸 + `ENQ-DEAD-TASK dropped` 打点——覆盖全部入队路径。
3. **A-3/A-4 验证**：__schedule 的 prev 出队与 pick 同一 GRQ 临界区（原子）；do_exit 的 ZOMBIE→dequeue→末次 schedule 被 preempt_count 包住；release_task 的 on_cpu 等待精确覆盖"还在 CPU 上"；fork 构造期半成品（state=RUNNING）不受陈旧 wake 影响。
4. **B-1 (H)** nanosleep 重写为 state-first + re-check：timer softirq 唤醒是一次性的，旧"查 jiffies→Task::sleep"顺序在窗口内丢弃唯一 waker → 永睡（B 型现场：全 CPU idle、`echo PP | cat` 静默挂）。B 型 hunt 现场证实挂点在两次管道间的 sleep 10。
5. **B-2 (M)** ksoftirqd 竞态分支补 NEW-C2 出队补偿（防双跑）。B-3 全库裸睡眠点排查：bio 实为 yield 轮询、poll 族忙等——无丢唤醒窗口；Task::sleep 文档封禁（最后一位调用者已迁移）。

**观测器悖论确立**：死锁复现（12-25%）仅在 heredoc 一次性 stdin 的纯门禁配置；QEMU monitor socket、dfx-lock-owner feature（二进制布局）、FIFO 实时注入（命令到达时机）任一变化都偏移时序使复现消失（owner hunt 10 绿、生产+monitor hunt 10 绿、monitor 门禁 4 绿、FIFO 门禁 4 绿 vs 纯门禁 8 轮 2 死锁：TIMERS 锁与 TCP_TABLE_LOCK 两把）。

**DFX 增强**：UART RX 中断路径的 "DUMP!" magic 触发（dfx=taskdump 运行时开关）——挂起时 RX 中断是唯一必然存活的代码路径，B 型现场的任务快照由此可得；test/hunt-wedge.sh 增加 HUNT_PROD=1 生产模式。TIMERS↔ACTIONS 锁序全库统一（TIMERS→ACTIONS），投递已锁外（R12-3）——锁序环排除；堆锁已 irqsave——堆锁自死锁环排除。

**R34 首项**：A 型/TIMERS 型残余（~25%）代码审计方向——timer 软中断持 TIMERS+ACTIONS 锁内 BTreeMap retain/insert/rearm 的堆分配路径、TCP 锁内 send_buffer push 堆分配、以及与 zone/页面补充锁的潜在交互；virtio-net 修复代码的真实 slirp 流量验证（此前所有测试流量均走 loopback 短路）。

### 20.26 A 型死锁根因破案 + DFX 特性沉淀

**hunt-wedge 第一轮捕获 A 型完整现场**（DFX 特性首战：owner 诊断 + dfx=watchdog 任务快照 + QEMU monitor dump 三件套同时工作）：
- guest：`DEADLOCK lock=TCP_TABLE_LOCK holder=2`；任务快照显示 pid 331/332（echo/cat）RUNNING 但无 CPU 可用——纯锁死锁，非睡眠挂起。
- monitor：cpu2（holder）pc=trap_entry、sepc=trap_handler 序言 `sd a1,-0x4f8(s0)`（0x800ff4ce）、**scause=0xf store fault、stval=0xffffffd600c16e78（内核堆区）**——**cpu2 进入 trap 时 sp 已指向堆地址**：trap 序言在坏 sp 上压栈即 fault，嵌套异常处理静默卡死（panic 路径同样在坏栈 fault），TCP 锁被带进坟墓；cpu0/1/3 从 wfi 醒来的 timer tick 全部等锁。
- **结论：切到了内核栈已释放/未映射的任务**——任务生命周期竞态（栈释放与 pid_hash 摘除顺序 / 运行队列残留）在 nettest 重压下的产物。
- **已修**：fork.rs 五处 unwind 的 free_kernel_stack 全部改为 pid_hash_remove 之后（hash 残留窗口内并发 kill/wake 可找到已释放栈的任务）。release_task 本身顺序正确（hash 先摘 + on_cpu 等待 + pinned put）。
- **R33 继续**：运行队列残留死任务（dequeue 与 free 竞态、wake_up 对 ZOMBIE 的处理）为下一嫌疑；B 型（管道静默挂起、CPU idle）另案。

**DFX 特性沉淀（按指令：作为特性保留、可开关、平时零开销）**：
- 编译期 feature `dfx-lock-owner`（Cargo.toml）：锁持有者跟踪（每锁对 2 次原子 store），关闭时零开销；死锁告警打印 holder=<cpu>。
- 运行时开关 `dfx=watchdog,taskdump`（boot 参数，kernel/src/dfx/switches.rs）：watchdog=死锁告警后自动全任务快照（SBI 直写不依赖 printk）；taskdump=预留按需触发。
- `kernel/src/dfx/taskdump.rs`：全任务状态快照（pid/state/policy/comm）。
- `test/hunt-wedge.sh`：自动构建诊断内核、双触发条件（A 型 DEADLOCK / B 型管道静默 30s）、QEMU monitor 抓每 CPU 寄存器/栈/反汇编、固化当轮 kernel.elf 供 addr2line。文档见 docs/guides/debugging.md。

### 20.25 第三十二轮补（R32b）：门禁死锁追击 + drivers/arch 补盲

**门禁发现两个真 bug**：
1. **M-sig 挂死（run2，rc=124）**：nettest 的 rt_sigtimedwait 在 timer 池满（add_timer_wakeup→0）时以 INTERRUPTIBLE 状态 schedule() 且无唤醒者 = 永久挂。R32 ipc agent 修 futex/sem/mq 五处时漏了第六处。修复：timer 注册失败恢复 RUNNING 纯让出循环轮询（NEW-3 同构）。修后 12 轮门禁 M-sig 全过。
2. **TCP_TABLE_LOCK 三 CPU 死锁（run6，echo PP | cat 阶段，~12% 复现）**：nm 精确符号化 0xffffffff803f98c4 = net::tcp::TCP_TABLE_LOCK（.bss 尾部）。为抓持有者给 RawSpinlock 加 owner 诊断字段（lock 记 hart+1，unlock 先清；deadlock_warn 打印 holder）。drivers agent 同期发现旧 virtio-net MMIO 队列配置完全无效（desc 表地址写错、avail/used 硬编码 0——TX 队列从未注册给设备）+ xmit 完成快照竞态（submit 后取快照必超时，10M 自旋持 tx_queue 锁）——timer tick 持 TCP 锁重传进无效设备的长自旋是持有者卡死的主嫌疑。owner 版门禁前 4 轮全绿，待 5-8。

**drivers/arch 补盲 agent（9 项，4 HIGH）**：virtio-net MMIO 队列地址从未注册（HIGH，网卡整体静默失效——测试走 loopback 未暴露）；RX 把 DMA 物理地址当内核指针解引用+dealloc（HIGH）；xmit 快照竞态+超时释放 DMA 中缓冲（HIGH，改 R8-M2 late-drain + mem::forget）；virtio-blk MMIO 队列寄存器偏移错用 PCI 布局（HIGH，根文件系统回退路径从必然失效改为 spec v2）；notify 的 csrci sie,9 位掩码无效（MED，9=0b01001 清的是 WPRI 位）；alloc_and_map_* 先映射后清零信息泄漏窗口（MED，先清零后装 PTE）；set_brk 叶 PTE 写游离于 PTE_MODIFY_LOCK 之外（MED，入锁+锁内复检）；set_queue_vector 写错寄存器（LOW）；PCI read/write_block resp 泄漏（LOW）。bio 全路径复审无新问题。
**文档/用户态 agent（额度中断，遗留已核）**：nettest.c 的 __NR_nanosleep 35→101（真 RISC-V 编号，旧值让 msleep 误调 unlinkat 忙转）；FAIL 码报首个失败组；README/指南/架构文档数据更新到当前状态（60 文件 995 单测、157 Kani 证明、344 syscall 等）。
**fork/exec/wait 人工走查**：无新缺陷（unwind 五处配对完整、argv 65×1KB 上限有界、sigsuspend state-first、vfork 双竞争防护在位）。

### 20.24 第三十二轮：六子系统并行（fs/mm/sched/net/ipc/核心）— R30 遗留 15 项全清 + 33 项新发现

六个 agent 并行检视，总控逐文件复核 diff，cargo check 0 error。

**R30 遗留全清**：F7 rmap TOCTOU（锁下重读叶 PTE 验 ppn，mm）；F8 OOM pin（chosen_pid 防复用误杀 + task_put 归还泄漏的引用 + mm 全部 Arc 固定，mm）；F4 sched_setattr/setparam 双绕过（完整镜像 setscheduler 校验 + change_task_policy 迁移；setparam 裸改 rt_priority 清错 bitmap 位，sched）；F10 down_interruptible（真缺陷是"先 fetch_sub 后注册"丢令牌窗口，重写为 down() 同构，sched）；B4 symlink DAC（follow_symlink 补逐组件 MAY_EXEC，fs）；B5 重传 seq（tx_segment 增参，三调用点全改，net）；B6 sys_shutdown 补表锁（loopback 只入 backlog 无 RX 重入，已验证 NEW-C6 断言，net/ipc）；B7 FIN_WAIT 孤儿（orphaned 标志 + sweep 条件扩展，net）；B8 CLOSE_WAIT 孤儿（close_wait_since 60s 超时回收 + accept 接受 CLOSE_WAIT 子连接，net）；B9 epoll fd 重用（file_id 身份绑定 + 同号 rebind，不 pin Arc 保管道关闭语义，ipc）；B10 TFD_ABSTIME（绝对 jiffies 直通，过去期限下次 softirq 即到，ipc）；B11 recv_wnd u32 后再收窄，net）；B12 accept 槽重用（pass(a) 跳尸体 + 纯 SYN 命中垂死即复位重扫 + 握手 ACK 序号校验防陈旧 ACK 伪建立，net）。

**核心子系统新修 8 项（4 HIGH 信号族）**：do_signal 无 handler 路径回卷 epc 转换 -512/-514 哨兵（此前 default-ignore 信号打断可中断睡眠会把 ERESTARTSYS 原样 sret 给用户态）；SIG_IGN 独立分支不再落终止表错杀；send_signal_locked 重构为 prepare_signal 语义（Ignore 置位前丢弃消瞬态位，无共享结构补 pending——唤醒却不可见）；restore_sigcontext 强制 SPIE（用户可控帧防单核关中断 hog）；getchar unwrap→if let（多消费者竞争 panic）；伪造 rt_sigreturn→SIGSEGV；nanosleep timer 池满改 RUNNABLE 轮询；孤儿僵尸移交 init 补 wait_chldexit 唤醒（init 的 SIGCHLD 是 Ignore，纯信号永不唤醒）。

**fs 新修**：close_pending 闭环（条件 Drop for File 仅标志真才执行——与被回退 v1 的本质区别；close_fd/FdTable::drop 执行前清标志防 dup 双关闭；FdTable::drop 恢复退出关闭）；O_NOFOLLOW 透传 + ELOOP；uart_read count==0 越界。
**sched 新修**：DL pick 重置 exec_start（睡一次即被 CBS 节流）；dl_rq.dequeue on_rq 守卫；change_task_policy bool 传播。
**net 新修**：拥塞窗口 usable_window 循环内递减；send_reliable 256KB 部分写上限（用户态可触发堆耗尽 panic）；SYN/SYN-ACK 重传（指数退避 + TCP_MAX_RETRIES 转 CLOSE，此前丢 SYN 即永久挂起）；bind 端口冲突 EADDRINUSE（TCP/UDP）+ 错误码传播；shutdown 后 Closing 态可读；poll/unwind 持表锁；TIME_WAIT 重 ACK 对端重传 FIN。
**ipc 新修 11 项**：epoll_create1 CLOEXEC+flags 校验；futex/sem/mq 定时等待 timer 池满回退 ENOMEM（×5 处，防永久睡眠）；SysV IPC_STAT 族权限（×7）；mq 名 copy_from_user 防跨页 panic；mq fd 单临界段防重号 + EMFILE 回收；notify 仅清本人注册；队列释放 Arc 身份匹配（防同名误杀活队列）；mq_msgsize clamp；mq 收 EMSGSIZE 放回；msgrcv E2BIG 原位 insert；epoll_pwait2 timespec 指针误当毫秒。
**mm 新修**：free_pages TDF 检出后 leak-not-corrupt 早退。

**留 R33 专项**：裸 Task::sleep 丢失唤醒窗口（核心 agent 系统性发现——wake_up 只对 is_sleeping 生效，"条件检查后、set_state 前"窗口丢唤醒；wait.rs 已用 prepare_to_wait 关闭，nanosleep/bio 等仍裸用）；timer 软 irq 持双锁堆分配（架构债）；SHUT_RD 读侧语义；UDP 未 bind 源端口 0。

### 20.23 第三十/三十一轮：终检 23 项 → 8 项落地 + 1 项回退重做

**R30 终检（双 agent）：共 23 项（A:6H+4M，B:4H+3M+6 低）**。最关键发现：R25-1/R28-1 的 timerfd 修复连续两次把极性搞反（rearmed 排除=周期永不交付）；R25-6 renice 没拿 GRQ 锁且无条件重算（非树上任务漂移 load_weight）；check_rt_preempt 比较反转（高优 RT 永不抢占）；dequeue_task RT/DL 臂硬编码 true（每次退出泄漏 nr_running）；alloc_single_page 仍无锁（压缩路径裸并发）；shared futex hash 含 pid 但 match 不含（跨进程丢唤醒）。
**R31 已落地 8 项**：rt_preempt 反转修正；timerfd 极性第三次修正（恒交付——H48 关闭竞态由 refcount 承担）；renice 补锁+on_rq 门+sched_setattr 走 helper；RT/DL dequeue 返回 bool 并传播；alloc_single_page 补锁；futex shared hash 去 pid；File close_pending 标志（B3 的 Drop 方案引入 ext4 I/O 副作用回归——run8 smoke 11/15——已回退为标志法）；close_fd 恢复原执行路径+pending 交接。
**门禁（R31-v2 八轮）**：smoke 15/15 ×8、nettest 8/8、kpanic 0/8、wedge 0/8。pp 4/8 + x 1/8 为注入时间窗超限（195s 不够 4 阶段+2s 预热）与 mrsh 偶发管道挂——非新回归。R30 剩余 15 项 MED/LOW 留下轮。

### 20.22 第二十九轮：TCP 锁楔 12 连不复现

R28-1（TIMERS 快照交付）之后 12 连全门禁：**pp 12/12、wedge 0/12、kpanic 0/12**。此前 1/6 的 TCP_TABLE_LOCK 楔判定为 TIMERS 竞争级联的下游表现（TIMERS 持有者被交付循环二次锁竞争拖长 → 软中断排队 → 级联传导到 TCP 锁）。残留清单更新：冷启动首字节（gate 预热规避）+ 文档开口项。

### 20.21 第二十八轮：TIMERS 交付改快照法 + TCP_TABLE_LOCK 低频楔（1/6）追查

R28-1：timerfd still_ours 复验改快照（rearmed 集合判定，不再二次取 TIMERS 锁——交付循环与 tick 全扫描临界区竞争消除）。门禁：nettest 8/8、kpanic 0/8、smoke 15/15×8。**残留**：pp 5-6/8 闪烁——TCP_TABLE_LOCK 楔 1/6（nettest TCP echo 之后、mrsh 管道时刻；3-4 CPU 自旋 ra=lock_irqsave；GDB 4 连未复现——低频）。嫌疑（未定罪）：tcp_rcv/timer tick 持锁 → tx → ipv4_send → 非环回路径 virtio xmit 自旋（10s 超时持锁）或 ARP 分支。第二十九轮专项。

### 20.20 第二十七轮：getchar CAS 环 + 冷启动窗口进一步定界

R27-1：UART RX get() 改 CAS 循环（fork 子进程共享 stdin 是多消费者——旧 load/read/store 丢/重字节）。冷启动首字节实验：CAS 后裸 boot 仍吃首字节（0/4），5s 延迟 3/3、10s 2/2——**窗口在 guest 启动后 ~5s 内**，且命令行完整时管道仍偶发无输出（mrsh posix_spawn 残留竞态，kpanic/wedge=0——cat 或 spawn 链丢失唤醒）。两项均为输入子系统的启动/唤醒时序专项，列入第二十八轮。门禁维持预热（2s）。
**门禁（8 轮）**：smoke 15/15 ×8、nettest 7/8、kpanic 0/8、wedge 0/8、pp 5/8（mrsh 管道闪烁）。
### 20.19 第二十六轮：compact 慢路径守卫 + 冷启动首字节窗口定界

R26-1：compact find_free_page 两处 `is_free()` 判定补 `!OnFreelist` 排除——合并上行的块成员 refcount==0 但仍在链上，慢路径曾把它当空闲页偷走做迁移目标（同页双主）。
R26-2 冷启动窗口定界（注入实验）：裸 boot 直接发命令 → 首字符被吃（"cho PP"）→ 命令无效。加 2 秒预热后 100% 通过（3/3）。延迟 3s 同效。窗口 = shell exec 完成前的输入消费竞态（具体消费者待查——嫌疑为 exec 期间 IRQ 唤醒的某次 getchar）。门禁脚本已加预热；内核侧根治（getchar 的 SPSC get() CAS 化，R20 报告 #10）留下轮专项。
**门禁（8 轮）**：smoke 15/15 ×8、nettest 8/8、**pp 8/8、x 8/8、kpanic 0/8、wedge 0/8——全指标满分**。
### 20.18 第二十四/二十五轮：R24 双 agent 检视 + 主审落地 6 项 + agent 自落地 10 项

**R24 agent B（fs/net/ipc）自落地 10 项**：TCP accept 双认领+pin 竞态（锁内扫描+钉住+端点复制）；posix_mq R23-5 的等待队列条目泄漏（UAF 唤醒族）；UDP_TABLE_LOCK 全覆盖（HIGH-6 关闭）；UDP recv_buffer 128KB 预算（MED-9）；UDP poll 可读；virtio-net 首 alloc 泄漏；alloc_desc_chain(chain_len)（MED-1 关闭——RX 恢复 8 缓冲）；poll 早退 RX 回收；io_uring 读侧短读 break；TCP_CLOSE 孤儿清扫（RST 子连接不再泄漏槽）。
**R24 agent A（sched/mm）报告 11 项**，主审落地 6 项（R25）：R25-1 timerfd 周期到期被 still_ours 吞（rearmed 集合传递）；R25-2 rmap/compact 三处 address_space Arc 钉住（do_exit 的 Drop 不持 VMA 锁即释放页表）；R25-4 zone add_to_free_list 中央 refcount=0（分裂 buddy 落链 refcount=1 的合并顽疾）；R25-5 OOM victim pinned；R25-6 setpriority 走 GRQ 锁 + load_weight 差额重算；报告项 A3（compact 慢路径偷链上页——需 OnFreelist 排除）待下轮。
**冷启动现象**：裸 boot 直接 `echo PP | cat` 0/3 挂（无 DEADLOCK/kpanic）；先跑 smoke 后管道正常。与 R23 前"首命令吃字符"同族的早期竞态，非本轮回归（809cfbe 基线同样 0/3）。记录为独立开口项。
**门禁（8 轮）**：smoke 15/15 ×8、nettest 8/8、kpanic 0/8、wedge 0/8（pp 2/8 为冷启动项所致）。
### 20.17 第二十三轮：R20-R22 修复的回归检视 + 7 项修正

agent B 抓到 R21-N3b 的两处严重回归：①fin_wait arm 误含 ESTABLISHED（60s 活连接被杀）；②拆分 match 臂不落穿（Rust 语义）= ESTABLISHED/FIN_WAIT 的 RTO 重传被整体删除。修复：恢复六状态联合重传/delack 臂 + FIN_WAIT 专用超时臂（仅 FIN_WAIT1/2 计时）。R23-3 TCP_TABLE_LOCK 补齐（bind/listen/connect/alloc/free/rcv/timer/send-叶级）并修两次自楔（accept/v4_err 去锁避免 poll 重入；alloc_ephemeral_port 嵌套获取移除）。R23-4 FIN seg len=1 + 重传带 FIN 位。R23-5 posix_mq 两循环信号复查。R23-6 virtio-net xmit hdr 泄漏。
**门禁（8 轮）**：**smoke 15/15 ×8、nettest 8/8、pp 8/8、x 8/8、kpanic 0/8、wedge 0/8——全指标满分**。
### 20.16 第二十二轮：R20/R21 MED 尾巴七项

R22-1 SysV sem/msg 五个阻塞循环补 schedule 前信号复查（信号在仍 RUNNING 时送达不产生唤醒=不可杀窗口——ksoftirqd 模式推广）；R22-2 ioctl fd>=1000 启发式改 File::path 身份分派（高 fd 被劫持进 fbdev）；R22-3 rmap try_to_unmap + compact remap_page 的叶 PTE 写纳入 PTE_MODIFY_LOCK（§17.4 最后一项关闭——全部叶 PTE 写者现在同锁）；R22-4 TCP recv Ok(0)=EOF 传播 + poll 查协议表 recv_buffer/CLOSE_WAIT→POLLHUP（读循环不再永久自旋）；R22-5 socket_create_accepted 错误路径 unwind（Arc+协议槽钉）；R22-6 virtio MMIO read_block alloc_desc 失败补 dealloc（write_block 已有）。
**门禁（8 轮）**：**smoke 15/15 ×8、nettest 8/8 全 PASS、kpanic 0/8、wedge 1（run6 单次 DEADLOCK 行）、pp 7/8、x 8/8**。
### 20.15 第二十一轮：R20 未修 HIGH 全部落地（mm 4 + net 5 + syscall agent 5）

**mm**：R21-1 page_remove_rmap 判据改 `== -1`（原 off-by-one：提前清 LRU/最后映射不清）；R21-2 Zone::free_pages 真正重置 refcount（裸 free 调用群永久禁用 buddy 合并的根因）；R21-3 堆 dealloc 块对齐校验；R21-6 swap 读失败释放页。
**net**：R21-N1 TCP_TABLE_LOCK（alloc/free/tcp_rcv/tcp_timer_tick 四入口 irqsave——表在 4 CPU 上被软中断与系统调用并发改）；R21-N2 PCI virtio 超时迟清+泄漏不重试（根 FS 路径 DMA-after-free）；R21-N3 FIN 入 retrans_queue + FIN_WAIT1/2 孤儿超时（64 槽不再永久泄漏）；R21-N4 user_refs 引用计数（timer 只在 refs==0 释放，fd 存活时槽不回收=无跨连接混淆）；N5 SeqLock 写侧 irqsave+guard 携带保存态（软中断同 CPU自楔）。
**syscall agent（R20 批内已修 5 项）**：TCSETS 60→52 越界；uart_write 每字节锁→整次锁（**PIPE2 pp=0 的根因**——并发写把 PIPE-OK 标记撕开；也解释 smoke 14/15 单项闪烁）；sendmsg/recvmsg 四路径补 iovlen 界/access_ok/copy_to_user/EMSGSIZE；eventfd/timerfd/epoll close 泄漏 Box；capset inheritable ⊆ inheritable。
**门禁（8 轮）**：smoke 15/15 ×8、nettest 6/8、kpanic 0/8、wedge 0/8、pp 7/8、x 8/8。
### 20.14 第二十轮：五 agent 全子系统检视（sched/process、mm/arch、fs/ext4、net/drivers/ipc、syscall/signal/tests）——共修 24 项

**sched/process（R20-1..7 已修）**：waitid 僵尸复查缺 idtype 过滤（忙挂）；for_each_task 只看 per-CPU current（rmap/compact/hung_task 全盲——改走 pid_hash）；RR tick 无条件复活睡眠者/僵尸；idle 丢失唤醒窗；cfs_rq.curr 多 CPU 双计费+睡眠计费（vruntime 冻结→永久霸占）；栈断言 32768→65536；LAST_TICK 从未存储。
**mm/arch（R20-F7 已修，4 项新 HIGH 定罪待修）**：KERNPANIC 子女走查偏移全面陈旧 +8（offset_of! 化+task_offsets 导出）。新定罪：page_remove_rmap 判据 off-by-one（提前清 LRU/真正最后映射不清）；Zone::free_pages 从不重置 refcount（裸 free 调用群永久禁用 buddy 合并）；堆 dealloc 缺块对齐校验；copy_page_table_cow OOM 无回退。
**fs/ext4（R20-FS1..10 已修）**：unlink 释放顺序（F5 同族遗漏）；write_inode_disk 128B inode 越界 panic；FdTable::Drop 在锁内跑 close（R14-5 未覆盖）；destroy_inode 零调用（VFS inode 无限泄漏）；jbd2 19+3 处 brelse 泄漏；F4 空 tag 数据错位（潜伏）；管道读写信号窗不可杀；bread_wait 丢弃错误（毒化缓存）；io_uring 大 len panic；mkdir 父快照回写。
**net/drivers/ipc（0 修——报告 23 项待批修）**：TCP/UDP 表 static-mut 无锁（HIGH）、PCI virtio 超时 free+重试覆盖在飞链（HIGH——根文件系统路径！）、FIN 不重传（HIGH）、槽释放 vs fd（HIGH）、SeqLock 软中断自楔（HIGH）……
**门禁（R20 后 8 轮）**：smoke 15/15×7、nettest 7/8、kpanic 0/8、wedge 0/8、pp 6/7、x 6/6。run5 为 QEMU 镜像锁（非内核）。
### 20.13 第十九轮：栈溢出修复验证 + 全绿

Kernel.toml kernel_stack_size 32768→65536。**八轮全门禁：smoke 15/15 ×8、nettest 8/8 全 PASS（历史首次满分）、KERNPANIC 0/8、DEADLOCK 0/8、children 归零 0/8**——第十八轮定罪完全验证。mrsh 管道 pp 6/8（3/5/7/8 轮 pp=0 为静默失败待下一轮查）。
### 20.12 第十八轮：**children 归零引擎定罪——内核栈溢出**

**证据链（四步闭环）**：
1. R18-1 alloc 侧双主交接探针（堆 buddy 返回前查 TASK_PAGE_OWNED）**0 触发**——排除分配器双主交接（与 r17 agent"交接在 alloc 侧或野写"二分的第一支）。
2. R18-2 栈 canary（cache-pop 写 0xCAFEF00DDEADBEEF 于栈底，free 时验证）：**每次 children 归零崩溃 canary 同时破坏**（run1/3/5 canary=1+child=1 完美相关；run2/4/6 canary 5-9 无崩溃=溢出未命中关键数据）。
3. R18-3 tick 探针（10ms 粒度）：**33-68 次/轮**，pid 恒为 305（nettest 主进程），写入值=栈内指针/内核代码地址/局部变量——标准栈帧数据，即**活调用链真的下探到栈最底 8 字节**。
4. R18-6 bottom/ks 一致性：同次运行 diff=0x8000 ✓——排除"bottom 字段陈旧导致误检"。
**结论**：children-list 归零 = **nettest 主进程 32KB 内核栈被吃满，最深帧的序言写入越过栈底进入相邻堆页**——相邻页恰是活 Task 页时 children/sibling/地址空间字段被帧数据覆写（0 值局部变量→"pid=0/state=0"；堆指针→断链）。此前所有"归零源"（堆双释放、Task 复活、交接）都是这个溢出的下游或独立小缺陷。
**修复方向（第十九轮）**：a) KERNEL_STACK_SIZE 32K→64K（诊断实验早证可行）；b) 定位吃栈链（execve+fork 风暴下探行为）砍帧。探针常驻（exit 检查+tick 首报）。
### 20.11 第十七轮：归零引擎证据锁定 free 侧清白 + A/C/F5 落地与自纠

**agent 结论（工具证据，非推理）**：受害 Task 页**从未被释放**（GDB 事件追踪 insert-only；毒化从未出现=free_task_slot 从未运行于其上）；free 事件环记录每页 2-28 次 order-0 free 全部与分配配对；所有权位图守卫零触发。**双主交接在 alloc 侧或直接野写**——这是最后一步（下一仪器：alloc 侧环+硬件观察点）。归零的"写手"是共享后轮转的最高频 order-0 消费者：**virtio-blk 同步 I/O 每次 2 个整页**（16B header+1B status 各占一页！）、MB 级 Vec 弹跳、new_task_at 占位写（state=0/pid=0 正是"归零子节点"）。
**落地修复**：C——virtio PCI 同步 I/O header+resp 合并为单次 64B 分配（消灭 2 页/I/O 火龙）；A——堆 buddy meta 全块标记（R9-14 tripwire 的堆侧 OnFreelist 同类洞）；A2——init_block(false) 也清全块（A 初版漏此路径，保留半块内部页 free=1 让 tripwire 吞掉合法 free→泄漏→高压分配失败=create/P2b/rename 偶发——门禁实测自纠）；F5 顺序修正（先写回死 inode 再释放块/inode 号，原顺序写回已释放 inode）；B 所有权拒绝守卫因堆外页位图别名误拒合法 free（r17 门禁回归：pp 2/8+Arc drop 崩）已撤回拒绝保留 mark/unmark。
**门禁（A2 后 6 轮）**：kpanic 0、wedge 0、**rename FAIL 0/6**（F5 顺序+A2 消除）、nettest 4/6、pp 4/6；run2/3 为启动期残留（栈溢出 task 305 / Arc::clone 内非法指令——归零引擎的启动期形态，alloc 侧交接待第十八轮的 alloc 环+观察点直接命名）。
### 20.10 第十六轮：S-R（跳线性映射）定罪 + OnFreelist 权威判据 + 三层复活防御

**S-R 全链定罪（专项 agent，12 轮实测）**：此前地址换算前提就错了——`VA_PA_OFFSET=0xffffffd580000000`，epc 0xffffffd601331000 = **物理 0x81331000，内核堆区间内**；历代 0xaf9000 族 = slab 区间指针。即"跳小整数"实为**跳到复用的堆/slab 分配地址**。机制链：**synchronize_rcu 是结构性空操作**（RCU_GEN 仅由 call_rcu 递增，而 call_rcu 零调用者）→ release_task 的宽限期不覆盖任何在飞读者 → 未钉住的 pid_hash_lookup 返回已释放 Task → 毒化字 0xDEADBEEF 恰好通过 is_sleeping()（bit0=1）→ **死指针入 CFS BTreeMap** → pick 取出 → __switch_to 恢复复用页 +440 偏移处的堆指针为 ra → ret 跳转。12 轮实测 7 崩全为此上游族（children 走查 NULL×3、pid_hash 链走 0x6cb08、Arc<SignalStruct> drop 坏指针、btree navigate 崩）。
**修复（三层防御 + 根因排队）**：send_signal 改钉住查找；Task::wake_up 拒毒化 pid；enqueue_task_locked 拒毒化 pid 并打 ENQ-POISONED-TASK（末线，覆盖一切入队源）。结构性根治（真 RCU 宽限期）被 EXT4_BIG_LOCK 的 preempt-off 睡眠阻塞——先修那些锁再启用。附带发现：KERNPANIC 的"Registers:"块是 memset 伪影（ra 恒为 save_regs+0x26）——agent 的 SR-PROBE 已修真 pt_regs 打印。
**OnFreelist 权威判据（R15-6/R16）**：next_free 判据三度不可靠（上电 0 / init_free 漏置 / 历史子块 leader 残留——GDB 实捕 init exec 余量释放页 nf=stale）。PageFlag::OnFreelist(1<<16)：add_to_free_list×2 设、remove_from_free_list 清、init_free 清；TDF 换用后 **双重释放=0（4 轮全门禁）**——R14-16/17+F10 修复后 zone 主分配器已净。
**门禁（8 轮）**：kpanic 2/8（均为 children/NULL+8 族）、wedge 0、nettest 6/8、pp 6/6×（通过轮）、smoke 14-15/15。**遗留（第十七轮）**：children-list 归零引擎（agent harness ~50% 复现）为最后主残留；poweroff -f 拆卸 UAF 一次；RECYCLED-LIVE 分配侧探针因跨 CPU 窗口误报已停用。
### 20.9 第十五轮（2026-09-17 续）：TDF 探针两版误报的定罪 + init_free 缺陷

**探针方法论教训（展示推理链）**：R14 的 zone 探针（refcount==0 判双重释放）误报——`Zone::free_pages` 本就在 put_page 归零**之后**调用，首释放时 refcount==0 是常态（868 次/轮全误报）；R15 换判据 `next_free != FREE_LIST_NULL` 仍误报（idle 193/nettest 483）——**根因是 `Page::init_free()` 漏重置 next_free**：描述符内存上电为 0，未进过空闲链表的页 next_free=0 而非 MAX。诊断字段打印（nf=0x0 ord=0）一锤定音。修复：init_free 补 `next_free.store(usize::MAX)`（R15-5）——这也是真实缺陷：脏 next_free 让 remove_from_free_list 走错链。
修复后单跑 nettest TDF=0；**八轮全门禁（smoke+nettest+mrsh 管道）TDF 复现 20-70 次**——另一批真实双重释放由 smoke/管道路径触发（目前未致崩）。第十六轮首要目标：GDB 断点抓 TDF 栈（本轮断点法已验证可行）。
GDB 已抓到的完整链（idle 场景首例 TDF）：`init exec → alloc_and_map_to_user_table(R7-C5 余量释放) → free_pages`——该例经查为误报（判据错误），但断点+栈的方法成立。R15-2/3/4 探针诊断代码已全部移除，只留修正后的 TDF 单行打印。
**门禁（8 轮）**：kpanic 0/8（连续两批全零）、wedge 0/8、nettest 6/8、**mrsh 管道+重定向 6/6**、smoke 15/15×4 + 14/15×4（单项 flaky）。

### 20.8 第十四轮修复批（R14-1..17，全部完成并门禁）

修复：F10（vmscan 只在"自己减到 0"才释放）；F9（Zone::free_pages double-free 计数探针——先拒绝-泄漏版实测每轮 349-2309 次命中导致内存耗尽，改为计数+识别+放行）；HIGH-2（LO_BACKLOG/ARP_CACHE 全部 lock_irqsave）；HIGH-1（irq_exit 内联 softirq 加 !in_softirq 门）；F1（allocator.rs `BISECT-NO-PLUS1` 标记删除、inode 号 +1）；HIGH-5b（close op 决策留 entry 锁内、执行移出锁外——原在锁内执行 socket close 会持锁自旋 virtio 10s）；HIGH-3（TCP 服务端 SYN 消耗序号：send_synack 后 snd_nxt+1、handle_ack_recv 补 snd_una+1）；F5（rename 覆盖文件目标补 free_inode_blocks）；F6（目录迭代器递归改循环）；MED-5（virtio-input used 环 +8→+4）；MED-10（socket 创建失败路径 unwind 槽+Arc）；MED-11（PLIC 使能字 RMW 全局自旋锁）；F22（ext4 已挂载拒重挂 EBUSY）；F7/F18（4 处 10MHz 魔数补统一）；R14-16（**page_cache put() 只在实际减量后释放**——原"看到 0+invalidated 就释放"让同一帧被两次 put 各释放一次 = zone dblfree 流的可定罪源）；R14-17（CachedPage.released 一次性闩）。
探针结论：zone dblfree 探针的 ra/栈帧捕获不可靠（内联+叶函数 asm），打印已静默、计数保留；pfn 544551/2/5 落在堆区间的跨分配器语义（heap buddy 从不设 refcount，zone 判据失效）留待下一轮专项。
**门禁（6+8 轮）**：**kpanic 0/6（末批）**、wedge 0、smoke 15/15 ×4+14/15 ×2、**nettest 5/6 且 pp 5/5**——`echo PP | cat` 管道连续 5 轮通过；run6 为线性映射低位非法指令（0x1313 000 族，1/6）残留。F14（swap_init 接线）因 swap 区与 ext4 尾部重叠（M6）暂缓。


### 20.6 第十三轮（2026-09-17 续）：ti_cpu steering 窗口 + 五项加固

- **R13-1**：`process_deferred_exit_notify_cpu` 的 ti_cpu steering 补 `!on_cpu()` 守卫——is_sleeping() 在整个 prepare_to_wait→schedule 窗口为真而任务仍在跑，此时写 ti_cpu 毒化 cpu_id()（tp→ti_cpu 字段读）→ 下次 __schedule 从错误 per-CPU 槽取 prev → 对着另一个任务做切换 = **两任务一内核栈**（同时解释 NULL+0x30 栈局部清零与用户帧/内核 SPP 混合的跳用户地址；7-12 轮只修消费者而此 bug 毒化它们依赖的索引）。on_cpu 恰为"已 pick 未保存上下文"。wake_up 内同名 steer 同步加守卫。
- **R13-2**：bread_async Phase-3 重复扫描补死/evicting 守卫（get() Phase-3 同款）。
- **R13-3**：timer 递送——wake_pid 改 pinned lookup（最后一条未钉的跨 CPU 唤醒）；tfd 计数递送前在 TIMERS 锁内复验 id 仍缺席（R12-3 把递送移出锁后 H48 的 close-竞态复活）。
- **R13-4**：close_fd 的 strong_count 判定移入 entry 锁内——锁外读取会漏掉并发 get_file 克隆（读到 2 → 永不触发 close → 管道 EOF 丢失的挂起面）。
- **R13-6**：free_kernel_stack 校验 bottom 在堆界内且 16 字节对齐后才入缓存——32KB 清零是唯一能整页抹掉已链 Task 的引擎；bogus 值报告并丢弃（泄漏胜于抹除）。探针 8 轮 0 触发（bottom 均合法）——归零源的另一半仍开放。
- **门禁（8 轮）**：nettest 6/8；三签名维持各 ~2/8（children-list 归零、NULL+0x30 CAS、跳用户地址），其中 1 轮 pp/x 双通过后仍崩。**第十四轮输入**：三签名共用"栈/Task 页被覆写"上游——下一轮应给 Zone::free_pages 补 double-free 探针（R10 发现 #10 未修，物理页侧无守卫）、用 GDB 在 KERNPANIC 时 dump FREED_TASK_RING 与 fault 页的 BlockMeta 归属，直接指认写者。

### 20.5 第十二轮（2026-09-17 续）：唤醒移回锁内 + 定时器锁减负 + 引用计数化查找

- **R12-1/R12-2**：WaitQueueHead::wake_up 与 futex_wake 的唤醒**移回等待队列/桶锁内**——锁内"未唤醒"条目的存在证明该任务尚未通过 finish_wait（需要同一把锁），因此不可能已退出/被收尸，收集-后唤醒的 UAF 窗口彻底关闭（此前 PID 复验只是收窄）。futex_requeue 因跨两桶保留复验。锁序 queue→GRQ 经审计安全。
- **R12-3**：timer 软中断的 wake/signal/tfd 递送**移出 TIMERS/ACTIONS 临界区**——原 TIMERS→GRQ 嵌套是 LO_BACKLOG/TIMERS 锁楔的成分。expired 快照先摘除后递送。
- **R12-4**：free_task_slot 毒化（0xDEADBEEF 写 state/pid）+ 64 槽释放环；KERNPANIC 走查识别 POISONED-FREED-TASK；走查本身修了 `pos < OFF_SIBLING` 的下溢 panic（此前诊断代码自己炸掉现场）。
- **R12-5**：Task.task_refcnt + pid_hash_lookup_pinned/task_put——跨 CPU 的 timer 唤醒、deferred notify（3 处）改为钉住-用-放；release_task 经 task_put 释放（末次 put 才真正 free）。56 个普通查找调用点（procfs/syscall 短读、本 CPU 不可被收尸方抢占）保持无钉，避免大规模 put 遗漏导致槽泄漏。GDB 实捕确认 enqueue_task_locked 野指针（0xffffffffdd33fa90，freed Task 的 sched_entity 偏移）即此族。
- **bh 探针空安全**：四处 `(*bh).b_data.len()` 探针补 `bh.is_null()` 前置——探针自己会在 NULL bh 上炸（+0x30 一族的新来源）。
- **门禁（8 轮）**：**nettest 全 PASS 8/8（历史首次）、DEADLOCK/soft-lockup 0/8（首次）**；mrsh `echo PP | cat` 与重定向可用率 4/8。残留：mrsh 阶段 KERNPANIC ~5/8（GDB 下 0/5 未复现——时序敏感），两签名：①NULL+0x30 的 CAS（BufferHead.b_state @+0x30 疑 NULL bh 的 set_state/is_dirty）②控制流跳用户地址（mrsh 0x3869c，SPP=1）——第十三轮目标。

### 20.4 第十一轮（2026-09-17 续）：S2 定罪 + 四项修复

**S2（空对象 CAS @0x30）定罪**：`evict_one` Phase 2 的哈希链走查**无"未找到即中止"**——被延迟的第二个逐出者在另一 CPU 完整逐出并释放+复用该 victim 后，对已释放内存做 count/evicting 检查、remove_from_lru 写入复用块、`is_dirty()` 对 NULL+0x30 CAS（即 S2 panic）、`Box::from_raw` 二次释放（S1 的堆别名喂料）。反汇编证明 +0x30 宿主只有 BufferHead.b_state 与 Dentry.children，后者全程 Arc 不可空。修复：**在桶锁内以"存在于哈希链"为存活判据**（先走查、未找到即返回；pin 复查移到摘链后、被钉时回插头部中止）。
S1（内核控制流跳用户地址）：sigreturn/信号帧链全量核验**干净**（SPP/SPIE/SIE 消毒在两条帧路径都在位）——S1 为下游腐蚀：deferred-wake UAF（已收 PID 复验收窄）或 bio 双释放（本轮已闭）。附带修复：`defer_exit_notify` 单槽覆盖改为就地补发（管道双退出丢 SIGCHLD→mrsh 挂）；exec 的 pt_regs 在函数入口一次性捕获（此前 I/O 迁移 CPU 后重读 per-CPU 槽会写坏外任务帧）；wake 唤醒列表按 PID 哈希一致性复验（RCU 包裹方案已证回归并回退）。
**门禁（8 轮）**：nettest 5/8；KERNPANIC 4/8 但签名收敛为两族：children-list NULL 走查（do_wait 闭包，3 次）与跳用户地址（1 次）；TIMERS 锁楔 2/8（`echo PP|cat` 后）；bio 死 bh 探针 0/8（R11-1 后死条目不再滞留）。**遗留（第十二轮）**：①children-list 归零节点仍有源（怀疑残余 deferred-wake UAF 窗口或未发现的第二个引擎——下一轮应给 free_task_slot 上 call_rcu 并加 poison 标记定位）；②TIMERS 锁楔的持有者（GDB 实捕 + 持有者追踪）；③mrsh 路径 EBADF 复现 1 次（run4，与 nettest PIPE2 修复并存——疑 mrsh 特有 close 序列或瞬态 Arc 计数窗口）。


6/6 nettest 全 PASS；kpanic 4/6 但全部发生在**其后**的 mrsh `echo PP | cat` 阶段，两种形态各 2/6：
1. **内核控制流跳到用户地址**（epc=0x3869c/ra=0x31938 均为用户地址，SPP=1）——NEW2 原始劫持家族的低频残留在 mrsh fork/exec 路径；
2. **空对象 +0x30 的 CAS**（compare_exchange 内联，宿主对象为 NULL——mrsh exec 路径上某 File/Inode 链空引用）。
两者即第十一轮首要目标；开口项：deferred-wake UAF 的 call_rcu 正解、远程 TLB shootdown、TCP 表 static mut 加锁、zone free_pages double-free 探针、msg_iovlen/utimensat 等 LOW。

- **R9 遗留（第十轮输入）**：①PIPE2 EBADF 家族 3/8（无腐蚀伴随——exec/fdtable 路径：dup3 后 exec 的 echo 对 fd1 write 得 EBADF，两级 fork+pipe 场景特有）；②野指针 enqueue 1/8；③M2b 完整版（超时+迟到完成时调用方缓冲所有权）；④virtio 裸 schedule() 轮询改 prepare_to_wait 睡眠（B6 评估：现正确但烧 CPU 且预算迭代制）。

### 18.3 门禁

smoke 15/15 ×5（历史最稳）；nettest 判定如实：panic(VF-re 空 b_data) 2/5、wedge 2/5、EBADF 4/5——与机制 1 未闭合一致。第八轮修复提交后 NEW2 残留频率与形态不变，进一步佐证 pick-before-save 窗口为唯一剩余根因。

### 17.5 已核验干净（不重复修）

do_futex CMD_MASK 位剥离正确；futex2 NR454/455 绝对超时换算正确；PTE_MODIFY_LOCK 无递归/逆序；fdtable cloexec 位图健全；epoll_event riscv64 布局正确（非 x86 打包）；NEW-C3/C4/C5/C6 修复确认在位；timer ABI、sigaction/stat/rusage/uname/statfs/fd_set/getdents64 布局核对无误。
  - nettest 的 fork+fs 探针与 E 用例默认禁用以保持套件确定性；修复前 shell 重定向/管道视为已知不可用。

### 16.5 对修复计划的影响

1. **新增 Wave 2R（回归热修）**：16.1 的 8 项 FIX-FAIL + 16.2 的 NEW-C1/C3（提权/panic 类）优先于 Wave 3。
2. **Wave 3 修复顺序修正**：0(init+route_init) → C2+Drop → C1 → H1 → tot_len/checksum-to_be（新增） → C3(24 处) → H2/H3/H4(含客户端 857) → loopback 队列化（新增） → C4 重构 → H5/H6 → M 系列。
3. **Wave 4 前置捆绑**：4.4 须连带 ECAM slot×8；4.7 须先过 0x1052 ID 关；4.1 须连带"探测顺序先于挂载"+MMIO 32 位 notify。
4. **Wave 5 并入**：NEW-C2（wake on_cpu 状态机）与 P07/P08/P12 合并为同一调度状态机改造。
5. Wave 7 ext4 增加 4 项新 H（add_entry 丢条目、mkdir 快照、WAL 顺序、环绕回放）。
