# Rux Kernel — Missing Linux Features Analysis (2026-09-24)

> **Status note (2026-09-29)**: several P0 items below have since been
> implemented — AF_UNIX (with socket files and SO_PEERCRED), pty/devpts,
> tmpfs, and the 64MB user-memory cap are all in main now. This document is
> kept as the original gap analysis for reference.

**Scope**: a systematic inventory of whole features missing relative to Linux,
ranked by how much they block real-world software. Every conclusion was
verified by grep / code reading.

**Baseline**: the 446 deviations from
`code-review-linux-compat-2026-09-23.md` were all fixed (8 waves, 17 commits).
This report lists only **whole missing features**, not already-fixed
implementation deviations.

## Summary

| Level | Count | Definition |
|---|---|---|
| **P0** | 7 | Blocks entire classes of real software (desktop / containers / network config / databases) |
| **P1** | 19 | Core paths of important software degraded |
| **P2** | 35 | Usable but degraded / specialized scenarios blocked |
| **P3** | 20 | Edge cases, can wait long-term |

---

## P0: blocking whole classes of software (7 items)

| # | Feature | Affected real software | Rux status at the time |
|---|---|---|---|
| 1 | **AF_UNIX entirely missing** | X11/Wayland/docker daemon/D-Bus/syslog/tmux/sshd local forwarding | socket supported AF_INET only (socket.rs:1095); socketpair EOPNOTSUPP |
| 2 | **netlink/rtnetlink + interface-management ioctls + DHCP** | ip/ifconfig/NetworkManager/udev — **userspace could not configure networking at all** | zero netlink in the tree; ioctls were tty-class only |
| 3 | **pty/devpts** | sshd/tmux/screen/script/X terminals — every interactive subprocess | no pty layer (no tty_driver/n_tty, console passthrough only) |
| 4 | **inotify** | vim/cargo/webpack HMR/desktop file managers/systemd path units | init1 returned EMFILE, add/rm ENOSYS |
| 5 | **fcntl record locks + fake-success flock** | sqlite (**silent data corruption**)/apt/rpm package managers | flock always returned 0; F_SETLK fell into default-deny |
| 6 | **tmpfs** | shm_open/container /tmp and /dev/shm | do_mount only supported ext4/proc/devfs |
| 7 | **64MB global hard cap on user physical memory** | anything resident >64MB: JVM/CPython/rustc on large projects/chromium — immediate OOM | layout.rs:112 min(25%, 64MB) |

## P1: degraded core paths (19 items)

**Process/system**: ptrace (strace/gdb); core dumps + the whole /proc/sys
tree; all seven namespaces missing; cgroups v1/v2; rlimits stored but never
enforced (fdtable hard-capped at 1024); signalfd (systemd main loop); exec
without demand paging (whole-file pre-allocation)

**Files/storage**: xattr fully stubbed (capability markers);
sendfile/splice not zero-copy + tee/vmsplice ENOSYS; io_uring with 6 opcodes
only; procfs read-only without /proc/sys; mknod disabled (**mkfifo
unusable**); sysfs/uevent hotplug; swap: 426 lines written but never wired up
(swapon ENOSYS, no anonymous-page reclaim); chroot without isolation +
pivot_root ENOSYS

**Network**: IPv6 entirely (ethertype dropped outright); /proc/net/*; epoll
waiting was busy-yield (**spinning CPU under SMP4**) with no wait queues;
SCM_RIGHTS; memfd_create; no RTC for wall clock (boot at epoch 0)

## P2: degraded / specialized blocked (35 items, representative)

- **Fake-success class** (most dangerous — more deceptive than ENOSYS):
  SO_KEEPALIVE/SO_LINGER/SO_BROADCAST; fake TCP_NODELAY; fake IP_TTL/MULTICAST;
  three empty mlock stubs; THP (MAP_HUGETLB validated then given normal
  pages); 10 no-op madvise hints; prctl SECCOMP returning EINVAL instead of
  ENOSYS
- **Performance**: no vDSO (a syscall per time read); no O_DIRECT
  distinction; no block-layer merging/scheduling; TCP without SACK/wscale/
  timestamps negotiation; single congestion control
- **Security**: ASLR entirely absent; file capabilities (xattr-dependent);
  seccomp/NO_NEW_PRIVS
- **Facilities**: POSIX AIO all ENOSYS; AF_PACKET; UDP multicast/IGMP;
  getrusage all zeros; ITIMER_VIRTUAL/PROF; SIGEV_THREAD_ID; partition
  tables; USB/mmc; GPU DRM; initramfs; kmod; missing shutdown cascade; mount
  API v2

## Most counter-intuitive findings

1. **swap.rs: a complete 426-line implementation with zero callers of
   `swap_init()` in the tree** — and swapon was ENOSYS. "Written but never
   wired up."
2. **Fake-success flock + missing fcntl locks = silent data corruption**:
   sqlite had no lock protection under concurrent writers — more dangerous
   than returning ENOSYS.
3. **epoll waiting was busy-yield**: every event-loop program (nginx/redis/
   node) spun CPU under SMP (functionally compatible, power/latency
   degraded).

## Correctly implemented highlights (do not "fix" these)

eventfd with EFD_SEMAPHORE, POSIX mq priorities, the three SysV IPC
facilities, TCP half-close, IPv4 fragment reassembly, ICMP echo/unreach,
shared interrupt action chains, real FLUSH propagation, /proc/self/exe
reopening the real file, getrandom ChaCha20, renameat2 NOREPLACE, the
five-set capabilities model, sigaltstack, actually-enforced CPU affinity,
wall clock offset model

## Suggested implementation priority (by software unlocked)

1. **P0-7 (64MB memory cap)** — smallest change unlocking the most software:
   one line in layout.rs + zones already had 2GB
2. **P0-5 (fcntl record locks + real flock)** — data safety for sqlite and
   package managers
3. **P0-4 (inotify)** — editors/build tools/desktop
4. **P0-6 (tmpfs)** — /dev/shm + container foundation
5. **P0-3 (pty)** — tmux/sshd interactivity
6. **P0-1 (AF_UNIX)** — the desktop IPC foundation
7. **P0-2 (netlink+ioctl+DHCP)** — network configuration
