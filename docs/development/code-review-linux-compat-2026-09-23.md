# Rux 内核 Linux 对比与全文件检视报告（2026-09-23）

**检视目标**：全仓库 278 个源文件（含 3 个 .S 汇编）逐文件、逐函数、逐数据结构、逐行检视；以 Linux 为基准对比语义差异；重点考察 POSIX/ABI 兼容性（目标：musl 静态编译的 Linux 软件可直接运行）。

**评审约定**：每项发现的"初判"标注三类——
- `设计不一致`：有意简化/取舍，语义自洽（用户评审是否接受）
- `疑似 bug`：与 Linux 差异且引发错误行为
- `待评审`：无法单方面判定

**类别**：LINUX-DIFF（语义差异）| ABI（POSIX/ABI）| BUG | OVERFLOW | RACE | TIMING | VISIBILITY | COMMENT | LICENSE | ARCH

**统计**：（检视完成后填写）

# 批次 3：kernel/src/process/ — Linux 对比全文件检视

范围：task.rs(2840)、fork.rs(502)、exec.rs(787)、exit.rs(809)、wait.rs(356)、pid.rs(160)、pid_hash.rs(262)、kthread.rs(231)、mod.rs(62)，共 6009 行。佐证交叉引用：syscall/process.rs、syscall/signal.rs、signal.rs、sync/futex.rs、fs/elf.rs、config.rs。

## 3.1 fork.rs（clone 语义）

函数清单：`CloneArgs` 定义；`do_fork`；`copy_thread`；`do_clone`。

**发现**
- [ABI][P1] fork.rs:238-257 + futex.rs:48-70 — 现状：exit 路径确有 `put_user(0)`+`futex_wake`（形式齐备），但 `FutexKey::matches` 对私有 futex 要求 `pid` 相等，key 的 pid 取 `current.pid()`（每线程不同）；Linux：私有 futex key 是 mm 指针（同 CLONE_VM 线程共享）。影响：join 等待者（另一线程）的 wait key 与退出线程的 wake key 恒不匹配 → musl `pthread_join` 永眠；同进程线程间一切私有 futex（mutex 竞争/cond）同理断裂。本批最重的 ABI 断点（sync 批次应复查 key 定义）。
- [ABI][P1] fork.rs:394-400 — CLONE_THREAD 仅 `set_tgid`，无 thread_group 链/group_leader/nr_threads，且 sys_kill 正 pid 只单目标；Linux kill(tgid) 遍历线程组。影响：多线程程序的进程定向信号（SIGINT/SIGTERM）只命中一个线程；配合 exit_group 缺失（3.4），多线程 musl 程序整体不可用。
- [语义][高] fork.rs:150-157 — CLONE_CHILD_SETTID/CLONE_PARENT_SETTID 写在父进程地址空间且在 mm 拷贝之后（Linux 在子上下文）；EFAULT 被忽略。
- [语义][中] fork.rs:188-201 — 非法 flag 组合返回 None → 统一 -ENOMEM（Linux -EINVAL）；PID 耗尽 ENOMEM 而非 EAGAIN。
- [语义][中] 子任务不继承 comm、nice、policy、rt_priority、cpus_allowed、oom_score_adj、sigaltstack。
- [语义][中] fork.rs:255-267 — CLONE_FILES 且父无 fdtable 时子新建表+std fds，违背共享语义（边缘）。
- [语义][低] sigmask 复制两次；CLONE_VFORK 用 UNINTERRUPTIBLE+手工 schedule 近似；CLONE_PARENT 未实现；CSIGNAL 低 8 位被忽略（恒 SIGCHLD 恰好等价）。
- [正确性][低] fork.rs:226 `add_child` 早于 copy_thread：TASK_NEW 防住了并发 wake（正面），do_wait 可观测半构建子进程（非僵尸，无害）。

**正面**：flag 前置校验与 Linux 一致；失败路径全量 unwind；vfork 双竞态已修；CLONE_SETTLS 时机正确。

## 3.2 exec.rs（execve vs load_elf_binary）

函数清单：`do_execve_elf`（含 UserAddrSpaceGuard、auxv/栈构建、PTE 收紧、解释器装载、VMA 登记）。

**发现**
- [ABI][高] exec.rs:170-172 + 629-644 + signal.rs:1096-1099 — 信号返回 ra 指向**用户栈**上的 2 指令 trampoline，sa_restorer 存而不用；栈 PTE 无 X、栈 VMA 仅 RW，page_fault 按 VMA 拒绝 EXEC。Linux：rt_sigaction 强制 SA_RESTORER，返回经 libc __restore_rt（文本段）。影响：musl 信号处理器返回跳栈上不可执行代码 → SIGSEGV；W^X 与"内核 trampoline"设计矛盾（建议尊重 sa_restorer）。
- [ABI][高] syscall/process.rs:110/141 — argv 上限 65 条/1024B、envp 257 条/4096B，超限静默截断，非 UTF-8 丢弃；Linux 131072 条/32 页单串/RLIMIT_STACK/4 总量，超限 E2BIG。影响：稍大 env 的 shell/python 行为损坏。
- [ABI][高] exec.rs 全文 — 多线程 exec 无 de_thread：兄弟线程继续跑旧映像但共享 fdtable 已被 close_cloexec、SignalStruct 已 flush。影响：线程进程 exec 后语义未定义。
- [语义][中] exec.rs:55-81 — cloexec/handler/pending 清理在镜像加载成功前，失败不回滚（Linux 仅在不可返回点后提交）。
- [语义][中] exec.rs:574-590 — auxv 15 对自洽但**缺 AT_PLATFORM(15)**，AT_HWCAP=0 无 ISA 探测；AT_CLKTCK=100 硬编码与 config HZ 双源。
- [正确性][中] exec.rs:474-475 — 无 PT_PHDR 时 AT_PHDR 回退栈上副本（musl ld.so 推 bias 得栈地址）；Linux 用 load_bias+e_phoff。
- [安全][低] 解释器基址固定 0x3FBF000000、PIE bias 0，无 ASLR。
- [正确性][低] 同页跨 RX/RW 段 PTE 收紧"后写者胜"；phdr 表拷贝不自防（pub(crate) 复用有险）。
- [设计][低] 全文件读入+全物理页预分配（无 demand paging）；itimer/posix_timers 未随 exec 重置。

**正面**：BSS 清零、段边界校验、RAII guard、tp=0 留给 musl TLS 重初始化、cred 快照回滚、vfork 唤醒在成功路径末尾。

## 3.3 exit.rs + wait 语义

函数清单：`release_task`；`reparent_children_to_init`；`do_exit`；`do_wait`；`do_wait_nonblock`；`write_siginfo`；`do_waitid`；W*/P_*/CLD_* 常量。

- [ABI][P1] dispatch.rs:142-143 — exit(93) 与 exit_group(94) 同映射 sys_exit→do_exit：只终止调用线程，无线程组击杀（Linux zap_other_threads）。多线程进程 exit_group 后其余线程存活。
- [语义][高] exit.rs:370-372/535-537 — wait4/waitpid 的 pid==0（同 pgid）与 pid<-1（pgid==-pid）一律当"任意子进程"；Linux 三段语义。shell 作业控制收错进程。
- [语义][高] exit.rs:752-754 — waitid+WNOHANG 无事件返回 -EAGAIN；Linux/POSIX 返回 0 且不动 infop。
- [语义][中] do_wait_nonblock 忽略 options（WUNTRACED 失效）；wait4 无 WCONTINUED；未知 options 不报 EINVAL。
- [语义][中] wait4 的 rusage 完全忽略；无 RUSAGE_CHILDREN 累计（bash time/make 统计为 0）。
- [语义][中] 过继只找 init(1)，不查 PR_SET_CHILD_SUBREAPER（prctl 存了不用）；pdeath_signal 存而不发。
- [并发][中] exit.rs:437 vs 743 — wait4 用 stop_reported、waitid 用 stop_signal()!=0 两套一次性报告机制不互斥（同一停止事件双报）。
- [正确性][低] 信号死 wstatus 无 WCOREDUMP 位；status 写失败忽略（与 nonblock 版不一致）。
- [清理][低] exit.rs:64-74 — 释放堆 PtRegs 是旧 fork 路径遗留死代码（两套 copy_thread 并存是隐患）。

**正面**：wstatus 位布局与 musl 宏一致；release_task 顺序正确；僵尸过继补发 SIGCHLD+直唤 init；do_waitid 过滤一致性重查。

## 3.4 wait.rs（等待队列基础设施）

- [语义][低] wake_up 忽略 mode 过滤（由 wake_up_process 兜底）；nr_exclusive 近似。
- [设计][低] Vec 头插/retain O(n)（Linux 双链表 O(1)）；add 按值拷入的 API 陷阱。
- 正面：R12-1 锁内唤醒；prepare_to_wait 同锁设状态+入队；两宏 re-check+dequeue 纪律。

## 3.5 pid.rs

- [语义][低] PID 耗尽 ENOMEM（Linux EAGAIN）；PID_MAX_LIMIT(4M) 仅导出未用；循环扫描+游标与 Linux 一致（正面）。

## 3.6 pid_hash.rs

- [并发][高] pid_hash_lookup 在 rcu_read_unlock 之后返回裸指针——56 个未 pin 调用点在解锁后解引用期间可被并发 reap 释放 → UAF 窗口（pinned 版已具备未全面换用）。
- [并发][中] pid_hash_remove 的 *prev = next 为普通写（Linux rcu_assign_pointer）。
- [语义][低] LIFO 序（procfs 列表不稳定）；collect_all 固定 64 条截断。

## 3.7 kthread.rs

- [资源][中] 内核线程自行退出无人清 KTHREAD_MAP → 条目泄漏 + PID 复用错配。
- [语义][低] kthread_stop 忙等轮询（Linux completion）；注释称存 name 但未存；kthread_bind 仅 cpu<32。

## 3.8 task.rs（PCB）

函数清单（分组）：StackCache 系列；TaskState 系列；SchedPolicy/TaskFlags/Cred；Task::new/new_idle_at/new_task_at；sleep/wake_up；时间片系列；调度访问器系列；进程树系列（add_child/remove_child/for_each_child 等）；地址空间系列；vfork 系列；thread_info 组（ti_flags/preempt_count/ti_kernel_sp/ti_cpu/on_cpu 等内核）；内核栈系列；杂项（oom/fdtable/signal/exit_code/stop_signal/comm/pgid/pending/clear_child_tid/robust_list/brk/cwd/umask/exe_path 等）；HZ 常量、task_offsets、get_current_fdtable。

- [清理][低] stack_cache_alloc 的 return 后第二个 unsafe 块为不可达死代码。
- [注释][低] "32KB" 实为 64KB；"O(log N)" 实为哈希 O(1)。
- [一致性][低] new_task_at 写 time_slice=HZ(100)，Task::new 用 TIME_SLICE_TICKS=10：双源不一致。
- [并发][中] add_child 双链 tripwire 只打印不阻止，旧父链表悬空（无防御效果）。
- [一致性][低] new_idle_at 以 *mut u64 直写 AtomicU64；Task::new 与 new_task_at 双轨。
- [安全][低] 新任务以 Cred::new_init()（FULL caps）出生靠随后覆盖：漏拷即提权，模式脆弱。
- [结构][信息] 缺 exit_signal、thread_group/group_leader、真实 rlimit、namespace、io_context——多线程/容器语义地基缺口。

## 3.9 mod.rs

- [语义][低] find_task_by_pid 返回 &'static mut（别名 UB 风险，全内核既定风格）；注释 O(log N) 错。

## 3.10 佐证快照（syscall 对接）

sys_clone 参数序正确、错误统一 ENOMEM；sys_set_tid_address 语义正确；sys_rt_sigaction 按 RISC-V ABI（restorer@16）正确往返 sa_restorer 但投递路径不用（3.2）；sys_wait4 WNOHANG 丢 options、-11 硬编码；sys_kill 权限近似到位但正 pid 无线程组扩散（3.1）。

## 批次 3 统计

| 级别 | 数量 | 条目 |
|---|---|---|
| P1 | 3 | CLEARTID futex key 断裂（join 永眠）；无线程组信号扩散；exit_group 不杀线程组 |
| 高 | 5 | sa_restorer 忽略+栈 trampoline 与 W^X 冲突；argv/envp 截断；多线程 exec 无 de_thread；waitpid pid==0/<-1 语义错；waitid WNOHANG 返回 -EAGAIN |
| 中 | 11 | SETTID 写父空间；EINVAL/ENOMEM 混淆；调度属性不继承；CLONE_FILES 边缘；exec 前置破坏；auxv 缺 AT_PLATFORM；AT_PHDR 回退；WUNTRACED×WNOHANG；rusage 忽略；subreaper/PDEATHSIG；双停止报告；pid_hash 非原子 unlink；kthread map 泄漏；add_child 无防御 |
| 低 | 14 | 死代码、注释漂移、time_slice 双源、WCOREDUMP、EFAULT 不一致、PID EAGAIN、双轨构造、cred 全权、procfs 64 截断、LIFO 序、O(n) 队列、API 陷阱、无 ASLR、后写者胜 |

**结论**：单线程静态 musl 程序的 fork/exec/wait 主链路可用且多处对齐 Linux；但**线程 ABI 三处断裂**（futex 私有 key 用 tid、无线程组信号扩散、exit_group 空转）与**信号返回的 sa_restorer/W^X 矛盾**是运行真实 musl 动态程序前的必障。


# 批次 1：syscall 层 Linux 对比检视报告

基准：Linux riscv64（asm-generic syscall 表 / musl ABI），逐文件逐函数全量检视。共 11 文件、约 372 个函数/方法。

**总体架构性发现（影响多个文件）**
- [ARCH][疑似bug] trap.rs:135 经 enable_external_interrupt() 全局置 SUM=1 —— Linux 仅在 copy 窗口置 SUM。后果：syscall 层大量"裸访问用户指针"代码在 SUM=1 下能工作但绕过异常表：坏指针触发内核态 fault → panic 而非 EFAULT。
- [LINUX-DIFF][设计不一致] 返回约定正确，但 resolve_user_path/do_execve 用 u64 传负 errno 等符号 hack 脆弱。

## 1. dispatch.rs
- [ABI][疑似bug] dispatch.rs:296 — 240 号映射 sys_perf_event_open；Linux asm-generic **240=rt_tgsigqueueinfo，241=perf_event_open**。错号 + rt_tgsigqueueinfo 缺失。
- [LINUX-DIFF][设计不一致] musl 启动必需号全部在表且正确（除 240）。
- [COMMENT][待评审] 模块归属错置（rseq/fanotify 归 time.rs 等）。
- [COMMENT][待评审] exit(93)/exit_group(94) 共用 sys_exit。

## 2. mod.rs
- [COMMENT][疑似bug] mod.rs:238-240 — Select=280/Pselect6=281/Eventfd=290 全错（280=bpf、281=execveat、290=pkey_free）；249 应为 Dup3；156-159 号值错。枚举为污染源。
- [LINUX-DIFF][待评审] errno 值核对无误；缺 ENOLCK(37)/EPROTO(71)/EOVERFLOW(75)/ECANCELED(125)/EOWNERDEAD(130)/ENOTRECOVERABLE(131) 等常用。
- [LINUX-DIFF][待评审] FdSet 1024 位 ✓ 但 FD_SETSIZE 取自 config（若 ≠1024 静默不一致）。

## 3. file.rs（100 函数，清单略见 git 历史）
- [ABI][疑似bug] file.rs:492 — /proc readlink bufsiz 不足返回 ENAMETOOLONG；Linux 截断返回长度（从不 ENAMETOOLONG）。
- [ABI][疑似bug] file.rs:1501-1509 — mknodat ftype==0 返回 EINVAL；Linux 视为普通文件。
- [ABI][疑似bug] file.rs:1998-2031 — futex2 FUTEX2_PRIVATE 用 bit0；Linux = FUTEX_PRIVATE_FLAG = 0x80。标准 flags 判定颠倒。
- [BUG][疑似bug] file.rs:2012-2030 — futex_wait(455) 相对 timeout 透传给绝对语义的 WAIT_BITSET；mask 忽略。
- [LINUX-DIFF][设计不一致] utimensat 空壳（不读 times 不更新）；renameat2 忽略 flags（NOREPLACE 覆盖）；statx 忽略 flags/attributes。
- [BUG][疑似bug] file.rs:1183-1214 — statfs 不查路径存在性（任意路径返回假数据）。
- [BUG][疑似bug] file.rs:1807-1846 — copy_file_range 忽略 off 指针；首败返 0 假成功。
- [LINUX-DIFF][待评审] linkat/unlinkat/fchmodat 等 flags 不校验；openat2 size>24 应 E2BIG；faccessat 用 euid（Linux 无 AT_EACCESS 用 real uid）；fallocate 假成功；mount 忽略参数；fsync 为 stub。
- [RACE][待评审] dirfd 相对路径基于 path 字符串快照（rename 后语义漂移）。

## 4. io.rs
- [OVERFLOW][疑似bug] io.rs:386/460/951/1013 — iovcnt 无 IOV_MAX(1024) 检查且 size_of×iovcnt 可溢出。
- [BUG][疑似bug] io.rs:1179-1212/1289/1326 — splice/sendfile 裸解引用用户 off 指针；splice 改 fd 位置不恢复。
- [RACE][疑似bug] io.rs:184-227/885-927 — pread/pwrite 用 get_pos/set_pos 模拟：多线程共享 fd 位置被破坏（Linux 原子）。
- [LINUX-DIFF][待评审] readv/writev 逐 iov 非原子；ioctl 0x5400 族恒 0（FIONBIO 假成功、TCSETS 丢失）；flock 恒成功；write fd1/2 绕过文件层直写 MMIO。

## 5. memory.rs
- [BUG][疑似bug] memory.rs:1330/1489/1501 — mincore/get_mempolicy 裸写用户指针。
- [ARCH][疑似bug] memory.rs:29-131 — sys_brk 对 new_brk 无上界检查（USER_END）：内核区映射 U 页即提权面。
- [LINUX-DIFF][疑似bug] memory.rs:1160-1167 — MADV_DONTNEED/FREE 不释放页（glibc/jemalloc 堆收缩失效）。
- [LINUX-DIFF][待评审] munmap 未映射区间 EINVAL（Linux 0）；mmap PROT_EXEC 简化为 RWX（W^X 失效）；fd>=1000 按 framebuffer 处理（合法 fd 误 ENXIO）；mremap 限制；mprotect 未映射返 0；mincore 溢出。

## 6. misc.rs
- [ABI][疑似bug] misc.rs:348-356 — pselect6 timeout 按 timeval 解析；**Linux 是 timespec**——量纲放大 1000 倍。
- [LINUX-DIFF][疑似bug] misc.rs:132-145 — poll(NULL,0,ms) 合法 sleep 惯用法被拒；nfds>1024 EINVAL（Linux 无限）。
- [ARCH][疑似bug] misc.rs:1504-1538 — getrandom 用 CLINT+LCG：非 CSPRNG（ssh/tls 依赖）。
- [LINUX-DIFF][待评审] epoll ERR/HUP 被掩码滤掉；close 后 epoll 条目不删；sigmask 全忽略；eventfd/timerfd 阻塞语义缺失（EAGAIN busy-loop）。

## 7. process.rs
- [ABI][疑似bug] process.rs:30-32 — **clone 参数 args[3]=tls, args[4]=child_tid；Linux riscv64 为 a3=child_tidptr, a4=tls**——pthread 创建即错位。
- [ABI][疑似bug] process.rs:1637-1641 — rt_sigqueueinfo 按 4 参解析；**Linux 3 参 (tgid,sig,uinfo)**。
- [BUG][疑似bug] process.rs:2453 — getrusage 写 136B；Linux rusage=144B。
- [BUG][疑似bug] process.rs:2815-2838/2747 — close_range/riscv_hwprobe 返回 count；Linux 返回 0。
- [BUG][疑似bug] signal.rs:546-549 — sys_tkill Err 双重取负返回正值。
- [LINUX-DIFF][设计不一致] prlimit64 只认 NOFILE；setrlimit 静默接受；execve pathname 限 256B（PATH_MAX 4096）。
- [LINUX-DIFF][待评审] gettid 返回 pid；tgkill 忽略 tgid；getgroups EFAULT 丢失；setpgid 空块；sysinfo 裸写+假数据；setuid cap 清空。

## 8. sched.rs
- [LINUX-DIFF][待评审] getpriority PRIO_PGRP/USER EINVAL；**renice 提优先级无 CAP_SYS_NICE**；sched_setaffinity 不存储；sched_setattr 不按 size 截读。

## 9. signal.rs
- [BUG][疑似bug] signal.rs:332 — sigpending 裸写。
- [LINUX-DIFF][待评审] rt_sigprocmask 对齐检查（Linux 不要求）；sa_mask 剥 KILL/STOP；sigaltstack 裸读写；restart_syscall 恒 0；signalfd4 ENOSYS。
- 正面：SigActionUser 32B 布局与 asm-generic 一致；sigsuspend mask 交接正确。

## 10. time.rs
- [ABI][疑似bug] time.rs:553-601 — clock_nanosleep 返回负 errno；**Linux 特例返回正 errno**。
- [LINUX-DIFF][设计不一致] clock_gettime REALTIME==MONOTONIC（无 wall clock）；CPUTIME 恒 0。
- [BUG][疑似bug] time.rs:652/899 — POSIX timer 用 Vec::remove——删除中间 timer 后 id 错位。
- [LINUX-DIFF][待评审] getitimer/setitimer 恒 0；nanosleep 负值不 EINVAL。

## 11. network.rs
- [BUG][疑似bug] network.rs:879-886 — sendmsg 读 msg_name 但丢弃：**UDP sendmsg 永远发 NULL 目标**。
- [BUG][疑似bug] network.rs:948 — recvmsg 丢弃源地址。
- [BUG][疑似bug] network.rs:387-397 — getsockname 非 socket fd 假成功（getpeername 正确 ENOTSOCK）。
- [BUG][疑似bug] network.rs:1085-1163 — sendmmsg/recvmmsg 首败返 0（Linux ABI 0=无消息）。
- [LINUX-DIFF][设计不一致] setsockopt 全忽略（**SO_RCVTIMEO 无效 → 超时网络程序永久阻塞**）；getsockopt SO_ERROR 恒 0（非阻塞 connect 探测失效）。
- [LINUX-DIFF][待评审] bind/connect 忽略 addrlen；AF_UNIX EAFNOSUPPORT；accept 不写 addr；accept4 忽略 CLOEXEC；MSG_PEEK/DONTWAIT 缺失；SHUT_RD 无操作。

## 批次 1 统计
11 文件/约 372 函数/14197 行；**78 项发现**（LINUX-DIFF 44/ABI 8/BUG 13/OVERFLOW 3/RACE 3/ARCH 3/COMMENT 12）。License 11/11 齐全。
**Top 10 修复优先**：①clone a3/a4 对调 ②rt_sigqueueinfo 3 参 ③FUTEX2_PRIVATE=0x80 ④pselect6 timespec ⑤240 号错位 ⑥futex_wait 相对 timeout ⑦sendmsg/recvmsg msg_name ⑧getrandom LCG ⑨tkill 双取负 ⑩rusage 144B。

---

# 批次 2：arch/riscv64（含全部汇编）对比 Linux 检视报告

基准：Linux riscv64。范围：24 文件约 10.2k 行，三汇编逐指令。格式：[类别][初判] file:line — 现状；Linux；影响。

## 2.1 trap.S（vs entry.S）
已检符号 21 项（trap_entry 全部标签/宏/常量）。
- [OK] 保存集与 Linux handle_exception 一致（31 GPR+sstatus/sepc/stval/scause）；sscratch 协议等价。
- [DESIGN][中] trap.S:234-236 — 内核态 trap 不切 IRQ 栈（16KB×4 的 IRQ 栈实际闲置）；Linux 切 per-CPU hardirq 栈。
- [缺陷][低] .Learly_boot 未从 sscratch 恢复 tp（防御路径）；boot.S:389 .Lsecondary_hang 注释与行为不符。
- [RISK][低] .Lrestore_and_exit 先写 sstatus 再恢复 GPR（依赖 PT_STATUS.SIE=0 前提）；sc.d 清 LR 非标准。
- [CONS][低] TASK_TI_* 偏移硬编码双份维护（无 asm-offsets 生成）。

## 2.2 uaccess.S / uaccess.rs
已检符号 16 项。
- [OK] SUM 仪式与旧版 Linux __copy_user 一致；extable 16B 条目用法相同。
- [SEC][中] uaccess.rs:230-243 — copy_from_user 失败不清零尾缓冲（Linux memset 0）——内核信息泄露面。
- [缺陷][低] strncpy_from_user 空串误 EFAULT；shift-copy 尾字可越源端 7B 误报。
- [CONS][低] extable 线性扫描（Linux 二分）；无 MAX_RW_COUNT。

## 2.3 boot.S / smp.rs
已检符号 18 项。
- [DESIGN][低] MMU 开启借取指 fault 跳转技巧（Linux fixmap trampoline）；早映射固定 8MB 窗口假设。
- [DOC][低] 0xC7 注释含 G 位错误（实无 G）。
- [CONS][低] 设备 PTE 未用 SVPBMT IO 位（真机风险）。
- [OK] 次核 SBI HSM 协议正确；tp 预载 idle；Acquire/Release 序对正确。

## 2.4 context.rs / thread.rs
已检符号 14 项。
- [PERF][高] context.rs:203-234 — switch_mm 恒 ASID=0 + 双份全量 sfence.vma；**asid.rs 全套分配器已实现但零消费者（死机制）**；内核线程切换还多刷两遍（Linux lazy TLB）。
- [SEC][中] thread.rs:163-184 — execve 后 FS=OFF 期间 FP 寄存器跨进程残留（fpu_init 无调用者）——信息泄露+数值污染。
- [OK] __switch_to 保存集/SUM 清序/on_cpu release 序正确。

## 2.5 trap.rs / pt_regs.rs / process.rs
已检符号 30+ 项。
- [OK] **pt_regs 布局与 Linux uapi 逐字节一致**（0x00-0x118）；syscall restart 语义一致。
- [SEC][中] trap.rs:124-142 — enable_external_interrupt 夹带 csrs SUM → 内核常态 SUM=1（纵深防御丢失）。
- [RISK][低] 每次内核 trap 读指令判 WFI（QEMU workaround）；handle_syscall 压缩判定裸读用户 epc 无 extable。
- [DESIGN][低] KERNPANIC 死 wfi 不停他 CPU（锁死锁）；SR-PROBE 诊断留生产路径。
- [缺陷][低] process.rs:87-158 arch copy_thread 死代码携隐患（活路径是 fork.rs 版）。

## 2.6 cpu.rs / mod.rs / ipi.rs / linker.ld
已检符号 30+ 项。
- [缺陷][中] ipi.rs:85-101 — send_ipi_type"已置不重发"丢 IPI 竞态 → smp_call_function 死锁（Linux 无条件发送）。
- [DESIGN][低] cpu_id 三处实现（mod.rs 硬编码 0x18 双维护）。
- [OK] save_and_disable_irq 用 csrrci 原子；linker 布局自洽。

## 2.7 mm/
已检符号 60+ 项。
- [PERF][高] mmu_init.rs:412-462 — map_page 每页无条件全量 sfence.vma（启动 2GB=1024 次；Linux mmu_gather 聚合）。
- [DESIGN][中] mm_ops.rs:921 — PTE_MODIFY_LOCK 全局单例串行所有 mm 的 PTE 写+长关中断窗（Linux per-mm lock+per-PTL）。
- [CONS][中] mm_ops.rs:669-695 — perm_to_flags(None)→V|R|A|D：**mmap(PROT_NONE) 匿名页实际可读**；mprotect 的 PROT_NONE 却正确——同内核语义割裂。
- [OK] Sv39 布局常量与 Linux 完全同构；COW-fork 加锁协议正确；page_fault 骨架对应 Linux。
- [RISK][低] virt_to_phys 线性区外原样返回；asid.rs/TRAP_STACKS/__switch_mm_linear 均死代码。

## 批次 2 统计
24/24 文件、214 符号；**36 项发现**（缺陷 6/安全 3/性能 2 高/设计 4/一致性 5/风险 7/文档 1/死代码 4）；确认正确 13 项（pt_regs 逐字节一致等）。
**Top 风险**：①无 ASID+每页全量 sfence（性能双高）②copy_from_user 不清零（安全）③FP 跨进程残留（安全）④SUM 常开 ⑤全局 PTE 锁 ⑥ipi 丢 IPI 死锁。



## 分批进度
- [x] 批次 1：syscall 层（ABI 基准）
- [x] 批次 2：arch/riscv64（含 3 个 .S）
- [x] 批次 3：process（fork/exec/wait/signal）
- [x] 批次 4：mm
- [x] 批次 5：fs（vfs/ext4/pipe）
- [x] 批次 6：ipc + sync
- [x] 批次 7：net + drivers
- [x] 批次 8：sched + timer + interrupt + 其余
