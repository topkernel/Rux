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
  - **仍未解决（第五轮追查进展）**：mrsh 管道（`a | b`）现确定为**确定性内核 panic**：`echo PP | cat` 在 mrsh 连续 fork 两个子进程时 100% 触发 `Option::unwrap() on None`（btree/navigate.rs:534 = BTreeMap 迭代器 next_unchecked），epc 落在区间迭代内联代码。伴随 DEADLOCK 警告（wait 队列 Vec 锁/TIMERS 锁自旋）。已排除/加固：TIMERS/ACTIONS 全部改 lock_irqsave（timer.rs 10 处，本提交）；fork 大互斥实验（串行化 copy_page_table_cow）无效已回退——非双 fork 并发 COW 降级竞争。剩余嫌疑：某 BTreeMap（CFS/DL 时间线或 VMA 表）存在**不持 GRQ/VMA 锁的访问路径**，或在 IRQ 上下文被无锁触碰。下一抓手：给三处 BTreeMap 各加"所属锁断言"或用 addr2line 解析 panic 时寄存器（t5/s2 曾指向用户地址）。NEW2 偶发竞态（套件挂点漂移）依旧。
  - nettest 的 fork+fs 探针与 E 用例默认禁用以保持套件确定性；修复前 shell 重定向/管道视为已知不可用。

### 16.5 对修复计划的影响

1. **新增 Wave 2R（回归热修）**：16.1 的 8 项 FIX-FAIL + 16.2 的 NEW-C1/C3（提权/panic 类）优先于 Wave 3。
2. **Wave 3 修复顺序修正**：0(init+route_init) → C2+Drop → C1 → H1 → tot_len/checksum-to_be（新增） → C3(24 处) → H2/H3/H4(含客户端 857) → loopback 队列化（新增） → C4 重构 → H5/H6 → M 系列。
3. **Wave 4 前置捆绑**：4.4 须连带 ECAM slot×8；4.7 须先过 0x1052 ID 关；4.1 须连带"探测顺序先于挂载"+MMIO 32 位 notify。
4. **Wave 5 并入**：NEW-C2（wake on_cpu 状态机）与 P07/P08/P12 合并为同一调度状态机改造。
5. Wave 7 ext4 增加 4 项新 H（add_entry 丢条目、mkdir 快照、WAL 顺序、环绕回放）。
