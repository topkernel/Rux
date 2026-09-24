# Rux 内核 Linux 特性缺失分析（2026-09-24）

**检视目标**：系统性盘点相比 Linux 整块缺失的重要特性，按"运行真实世界软件的阻碍度"排序。全部结论经 grep/读码验证。

**基线**：code-review-linux-compat-2026-09-23.md 的 446 项偏差已全部修复（8 波，17 提交）。本报告只列**整块缺失**，不含已修复的实现偏差。

## 总览

| 级别 | 数量 | 定义 |
|---|---|---|
| **P0** | 7 | 挡住整类真实软件（桌面/容器/网络配置/数据库） |
| **P1** | 19 | 重要软件核心路径受损 |
| **P2** | 35 | 功能可用但退化/专用场景受阻 |
| **P3** | 20 | 边缘/可长期后排 |

---

## P0：挡住整类软件（7 项）

| # | 特性 | 受影响的真实软件 | Rux 现状 |
|---|---|---|---|
| 1 | **AF_UNIX 整块缺失** | X11/Wayland/docker daemon/D-Bus/syslog/tmux/sshd 本地转发 | socket 仅 AF_INET（socket.rs:1095）；socketpair EOPNOTSUPP |
| 2 | **netlink/rtnetlink + 接口管理 ioctl + DHCP** | ip/ifconfig/NetworkManager/udev——**用户态完全无法配置网络** | 全仓库零 netlink；ioctl 仅 tty 类 |
| 3 | **pty/devpts** | sshd/tmux/screen/script/X terminal——一切交互式子进程 | 无 pty 层（无 tty_driver/n_tty，仅 console 直通） |
| 4 | **inotify** | vim/cargo/webpack HMR/桌面文件管理器/systemd path 单元 | init1 返回 EMFILE、add/rm ENOSYS |
| 5 | **fcntl 记录锁 + flock 假成功** | sqlite（**静默数据损坏**）/apt/rpm 包管理器 | flock 恒返 0；F_SETLK 落入默认拒绝 |
| 6 | **tmpfs** | shm_open/容器 /tmp /dev/shm | do_mount 仅 ext4/proc/devfs |
| 7 | **用户物理内存 64MB 全局硬顶** | 任何驻留>64MB 程序：JVM/CPython/rustc 大项目/chromium——直接 OOM | layout.rs:112 min(25%, 64MB) |

## P1：核心路径受损（19 项）

**进程/系统**：ptrace（strace/gdb）；core dump+/proc/sys 整树；namespaces 七种全缺；cgroups v1/v2；rlimits 存储但零执行（fdtable 1024 硬顶）；signalfd（systemd 主循环）；exec 无 demand paging（全文件预分配）

**文件/存储**：xattr 全 stub（capability 标记）；sendfile/splice 非零拷贝+tee/vmsplice ENOSYS；io_uring 仅 6 opcode；procfs 只读无 /proc/sys；mknod 禁用（**mkfifo 不可用**）；sysfs/uevent 热插拔；swap 426 行写完未接线（swapon ENOSYS，匿名页无回收）；chroot 无隔离+pivot_root ENOSYS

**网络**：IPv6 整块（ethertype 直接丢包）；/proc/net/*；epoll 等待是 busy-yield（**SMP4 下空转烧 CPU**）无等待队列；SCM_RIGHTS；memfd_create；墙钟无 RTC（启动 epoch 0）

## P2：退化/专用受阻（35 项，代表性）

- **假成功类**（最危险——比 ENOSYS 更具欺骗性）：SO_KEEPALIVE/SO_LINGER/SO_BROADCAST；TCP_NODELAY 假；IP_TTL/MULTICAST 假；mlock 三空壳；THP（MAP_HUGETLB 校验后给普通页）；madvise 10 种 no-op；prctl SECCOMP 返回 EINVAL 而非 ENOSYS
- **性能类**：vDSO 缺失（每取时走 syscall）；O_DIRECT 无区分；块层无合并调度；TCP 无 SACK/wscale/timestamps 协商；拥塞控制单一
- **安全类**：ASLR 全缺；文件 capabilities（xattr 依赖）；seccomp/NO_NEW_PRIVS
- **设施类**：POSIX AIO 全 ENOSYS；AF_PACKET；UDP multicast/IGMP；getrusage 全 0；ITIMER_VIRTUAL/PROF；SIGEV_THREAD_ID；分区表；USB/mmc；GPU DRM；initramfs；kmod；shutdown 级联缺失；mount API v2

## 最反直觉的发现

1. **swap.rs 426 行完整实现但 `swap_init()` 全仓库零调用者**——swapon 又是 ENOSYS。处于"写完未接线"状态。
2. **flock 假成功 + fcntl 锁缺失 = 静默数据损坏**：sqlite 在并发写时无锁保护，比返回 ENOSYS 危害更大。
3. **epoll 等待是 busy-yield**：nginx/redis/node 等一切事件循环程序在 SMP 下空转烧 CPU（功能兼容但功耗/延迟劣化）。

## 已正确实现的亮点（避免误伤）

eventfd 含 EFD_SEMAPHORE、POSIX mq 优先级、SysV IPC 三件套、TCP 半关闭、IPv4 分片重组、ICMP echo/unreach、中断共享 action 链、FLUSH 真下发、/proc/self/exe 真实重开、getrandom ChaCha20、renameat2 NOREPLACE、五集合 capabilities 模型、sigaltstack、CPU affinity 真执行面、wall clock offset 模型

## 建议实施优先级（按解锁软件面）

1. **P0-7（64MB 内存顶）**——最小改动解锁最大软件面：layout.rs 一行改 + zone 已有 2GB
2. **P0-5（fcntl 记录锁 + flock 真实现）**——sqlite/包管理器数据安全
3. **P0-4（inotify）**——编辑器/构建工具/桌面
4. **P0-6（tmpfs）**——/dev/shm + 容器基础
5. **P0-3（pty）**——tmux/sshd 交互
6. **P0-1（AF_UNIX）**——桌面 IPC 底座
7. **P0-2（netlink+ioctl+DHCP）**——网络配置
