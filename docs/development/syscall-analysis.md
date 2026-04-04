# Rux Syscall Compatibility Analysis Report

> Date: 2026-04-05
> Reference: `docs/development/syscall.tbl` (Linux RISC-V 64 syscall number table)
> Kernel file: `kernel/src/syscall/dispatch.rs`

## Overview

Rux currently implements approximately 120 syscall number mappings. This document analyzes:
1. Syscall number mismatches (incompatible with Linux ABI)
2. Unimplemented syscalls
3. Prioritized implementation plan

---

## 1. Syscall Number Mismatches (Must Fix)

The following syscalls use incorrect syscall numbers, making them incompatible with the Linux RISC-V 64 ABI.

| NR | Correct (Linux) | Rux Current | Severity | Notes |
|----|----------------|-------------|----------|-------|
| 39 | umount2 | sys_umount | Low | umount2 and umount have the same signature; functional impact minimal |
| 88 | utimensat | sys_futimesat | Low | NR is correct; function name mismatch, implementation may be correct |
| 276 | renameat2 | sys_renameat | Low | NR is correct but Rux ignores the flags parameter |

**Note:** All core syscalls (openat, read, write, mmap, clone, execve, etc.) have correct numbers.
The previous NR 117-120 mismatch (setresuid/getresuid/setresgid/getresgid at wrong NR) has been fixed.

---

## 2. Implemented Syscalls (By Category)

### File Operations

| NR | Syscall | Status |
|----|---------|--------|
| 17 | getcwd | OK |
| 23 | dup | OK |
| 24 | dup3 | OK |
| 25 | fcntl | OK |
| 29 | ioctl | OK |
| 32 | flock | OK |
| 33 | mknodat | OK (regular files/dirs only) |
| 34 | mkdirat | OK |
| 35 | unlinkat | OK |
| 36 | symlinkat | OK |
| 37 | linkat | OK |
| 38 | renameat | OK |
| 39 | umount2 | OK |
| 40 | mount | OK |
| 43 | statfs | OK |
| 44 | fstatfs | OK |
| 45 | truncate | OK |
| 46 | ftruncate | OK |
| 47 | fallocate | STUB (returns -ENOSYS) |
| 48 | faccessat | OK |
| 49 | chdir | OK |
| 50 | fchdir | OK |
| 52 | fchmod | OK |
| 53 | fchmodat | OK |
| 54 | fchownat | OK |
| 56 | openat | OK |
| 57 | close | OK |
| 59 | pipe2 | OK |
| 61 | getdents64 | OK |
| 62 | lseek | OK |
| 63 | read | OK |
| 64 | write | OK |
| 65 | readv | OK |
| 66 | writev | OK |
| 67 | pread64 | OK |
| 68 | pwrite64 | OK |
| 69 | preadv | OK |
| 70 | pwritev | OK |
| 71 | sendfile | OK |
| 78 | readlinkat | OK |
| 79 | fstatat | OK |
| 80 | fstat | OK |
| 81 | sync | OK |
| 82 | fsync | OK (success stub) |
| 83 | fdatasync | OK (success stub) |
| 88 | utimensat | OK |
| 166 | umask | OK |
| 276 | renameat2 | OK |
| 291 | statx | OK |
| 437 | openat2 | OK |

### Process Management

| NR | Syscall | Status |
|----|---------|--------|
| 93 | exit | OK |
| 94 | exit_group | OK |
| 95 | waitid | OK |
| 96 | set_tid_address | OK |
| 99 | set_robust_list | OK |
| 102 | getitimer | OK (returns zeros) |
| 103 | setitimer | STUB (returns -ENOSYS) |
| 112 | clock_settime | OK (requires root) |
| 129 | kill | OK |
| 130 | tkill | OK |
| 131 | tgkill | OK |
| 137 | rt_sigtimedwait | STUB (returns -ENOSYS) |
| 138 | rt_sigqueueinfo | STUB (returns -ENOSYS) |
| 142 | reboot | OK |
| 143 | setregid | OK |
| 144 | setgid | OK |
| 145 | setreuid | OK |
| 146 | setuid | OK |
| 147 | setresuid | OK |
| 148 | getresuid | OK |
| 149 | setresgid | OK |
| 150 | getresgid | OK |
| 154 | setpgid | OK |
| 155 | getpgid | OK |
| 156 | getsid | OK |
| 157 | setsid | OK |
| 158 | getgroups | OK |
| 159 | setgroups | OK |
| 161 | sethostname | OK (stub, no storage) |
| 162 | setdomainname | OK (stub, no storage) |
| 163 | getrlimit | OK |
| 164 | setrlimit | STUB (returns -ENOSYS) |
| 165 | getrusage | OK (returns zeros) |
| 167 | prctl | OK |
| 168 | getcpu | OK |
| 172 | getpid | OK |
| 173 | getppid | OK |
| 174 | getuid | OK |
| 175 | geteuid | OK |
| 176 | getgid | OK |
| 177 | getegid | OK |
| 178 | gettid | OK |
| 220 | clone | OK |
| 221 | execve | OK |
| 260 | wait4 | OK |
| 261 | prlimit64 | OK |
| 268 | setns | STUB (returns -ENOSYS) |
| 281 | execveat | OK |

### Signal

| NR | Syscall | Status |
|----|---------|--------|
| 74 | signalfd4 | STUB (returns -ENOSYS) |
| 98 | futex | OK |
| 128 | restart_syscall | STUB (returns -ENOSYS) |
| 133 | rt_sigpending | OK |
| 134 | rt_sigaction | OK |
| 135 | rt_sigprocmask | OK |
| 139 | rt_sigreturn | OK |

### Memory Management

| NR | Syscall | Status |
|----|---------|--------|
| 214 | brk | OK |
| 215 | munmap | OK |
| 216 | mremap | OK |
| 222 | mmap | OK |
| 226 | mprotect | OK |
| 227 | msync | OK |
| 228 | mlock | OK |
| 229 | munlock | OK |
| 232 | mincore | OK |
| 233 | madvise | OK |

### Time

| NR | Syscall | Status |
|----|---------|--------|
| 101 | nanosleep | OK |
| 113 | clock_gettime | OK |
| 114 | clock_getres | OK |
| 115 | clock_nanosleep | OK |
| 169 | gettimeofday | OK |

### Scheduler

| NR | Syscall | Status |
|----|---------|--------|
| 118 | sched_setparam | OK |
| 119 | sched_setscheduler | OK |
| 120 | sched_getscheduler | OK |
| 122 | sched_getparam | OK |
| 124 | sched_yield | OK |
| 127 | sched_rr_get_interval | OK |
| 140 | getpriority | OK |
| 141 | setpriority | OK |
| 351 | sched_getattr | OK |
| 352 | sched_setattr | OK |

### Network

| NR | Syscall | Status |
|----|---------|--------|
| 198 | socket | OK |
| 200 | bind | OK |
| 201 | listen | OK |
| 202 | accept | OK |
| 203 | connect | OK |
| 204 | getsockname | STUB (returns -ENOSYS) |
| 205 | getpeername | STUB (returns -ENOSYS) |
| 206 | sendto | OK |
| 207 | recvfrom | OK |
| 208 | setsockopt | STUB (returns -ENOSYS) |
| 209 | getsockopt | STUB (returns -ENOSYS) |
| 210 | shutdown | STUB (returns -ENOSYS) |
| 211 | sendmsg | OK |
| 212 | recvmsg | OK |
| 242 | accept4 | OK |

### Miscellaneous

| NR | Syscall | Status |
|----|---------|--------|
| 19 | eventfd2 | OK |
| 20 | epoll_create1 | OK |
| 21 | epoll_ctl | OK |
| 22 | epoll_pwait | OK |
| 72 | pselect6 | OK |
| 73 | ppoll | OK |
| 85 | timerfd_create | STUB (returns -ENOSYS) |
| 86 | timerfd_settime | STUB (returns -ENOSYS) |
| 87 | timerfd_gettime | STUB (returns -ENOSYS) |
| 116 | syslog | OK |
| 160 | uname | OK |
| 278 | getrandom | OK |
| 290 | eventfd | OK |

---

## 3. Unimplemented Syscalls

### P0 - Core (Required for basic programs)

| NR | Syscall | Purpose | Notes |
|----|---------|---------|-------|
| 5-16 | \*xattr | Extended attributes | Filesystem metadata; requires VFS extension |
| 33 | ~~mknodat~~ | ~~Create special files~~ | **IMPLEMENTED** (regular files/dirs only) |
| 52 | ~~fchmod~~ | ~~Change file permissions~~ | **IMPLEMENTED** |
| 74 | ~~signalfd4~~ | ~~Signal notification fd~~ | **IMPLEMENTED** (stub) |
| 81 | ~~sync~~ | ~~Sync filesystem cache~~ | **IMPLEMENTED** |
| 82 | ~~fsync~~ | ~~Sync single file~~ | **IMPLEMENTED** (stub) |
| 83 | ~~fdatasync~~ | ~~Sync file data~~ | **IMPLEMENTED** (stub) |
| 102 | ~~getitimer~~ | ~~Get interval timer~~ | **IMPLEMENTED** (returns zeros) |
| 103 | ~~setitimer~~ | ~~Set interval timer~~ | **IMPLEMENTED** (stub) |
| 112 | ~~clock_settime~~ | ~~Set clock~~ | **IMPLEMENTED** |
| 128 | ~~restart_syscall~~ | ~~Restart interrupted syscall~~ | **IMPLEMENTED** (stub) |
| 131 | ~~tgkill~~ | ~~Send signal to thread group~~ | **IMPLEMENTED** |
| 137 | ~~rt_sigtimedwait~~ | ~~Wait for specific signal~~ | **IMPLEMENTED** (stub) |
| 138 | ~~rt_sigqueueinfo~~ | ~~Send signal with data~~ | **IMPLEMENTED** (stub) |
| 168 | ~~getcpu~~ | ~~Get CPU info~~ | **IMPLEMENTED** |
| 281 | ~~execveat~~ | ~~Execute at directory path~~ | **IMPLEMENTED** |

### P1 - Common (Improves POSIX compatibility)

| NR | Syscall | Purpose | Notes |
|----|---------|---------|-------|
| 26-28 | inotify_init1/add_watch/rm_watch | Filesystem events | Requires inotify subsystem |
| 47 | ~~fallocate~~ | ~~Preallocate file space~~ | **IMPLEMENTED** (stub) |
| 60 | quotactl | Disk quota management | Requires filesystem quota support |
| 75-77 | vmsplice/splice/tee | Zero-copy I/O | Requires pipe buffer management |
| 85-87 | ~~timerfd_create/settime/gettime~~ | ~~Timer file descriptors~~ | **IMPLEMENTED** (stubs) |
| 134 | rt_sigsuspend | Wait for signal | Complex signal handling |
| 142 | ~~reboot~~ | ~~Reboot system~~ | **IMPLEMENTED** |
| 161 | ~~sethostname~~ | ~~Set hostname~~ | **IMPLEMENTED** (stub) |
| 162 | ~~setdomainname~~ | ~~Set domain name~~ | **IMPLEMENTED** (stub) |
| 163 | ~~getrlimit~~ | ~~Get resource limit~~ | **IMPLEMENTED** |
| 164 | ~~setrlimit~~ | ~~Set resource limit~~ | **IMPLEMENTED** (stub) |
| 165 | ~~getrusage~~ | ~~Get resource usage~~ | **IMPLEMENTED** (returns zeros) |
| 194-197 | shmget/shmctl/shmat/shmdt | System V shared memory | Requires IPC namespace |
| 204-205 | ~~getsockname/getpeername~~ | ~~Socket address queries~~ | **IMPLEMENTED** (stubs) |
| 208-210 | ~~setsockopt/getsockopt/shutdown~~ | ~~Socket options~~ | **IMPLEMENTED** (stubs) |
| 211-212 | ~~sendmsg/recvmsg~~ | ~~Message-based I/O~~ | **IMPLEMENTED** |
| 242 | ~~accept4~~ | ~~Accept with flags~~ | **IMPLEMENTED** |
| 268 | ~~setns~~ | ~~Join namespace~~ | **IMPLEMENTED** (stub) |

### P2 - Advanced (Long-term goals)

| NR | Syscall | Purpose | Notes |
|----|---------|---------|-------|
| 0-3 | io_setup/submit/cancel/getevents | Linux AIO | Async I/O; complex implementation |
| 30-31 | ioprio_set/get | I/O priority | I/O scheduling |
| 41 | pivot_root | Switch root filesystem | Containerization |
| 97 | unshare | Create new namespace | Containerization; requires namespace subsystem |
| 105-106 | init_module/delete_module | Kernel modules | Loadable modules; complex |
| 107-111 | timer_create/... | POSIX timers | High-resolution timers |
| 117 | ptrace | Process tracing | Debugger support; very complex |
| 122-123 | sched_setaffinity/getaffinity | CPU affinity | Multi-core scheduling |
| 125-126 | sched_get_priority_max/min | Priority range | Scheduling |
| 186-193 | msg\*/sem\* | System V IPC (msg/sem) | IPC; complex subsystem |
| 217-219 | add_key/request_key/keyctl | Key management | Security |
| 234-239 | mbind/get_mempolicy/... | NUMA memory policies | NUMA |
| 241 | perf_event_open | Performance monitoring | Profiling |
| 258 | riscv_hwprobe | RISC-V hardware probe | RISC-V specific |
| 259 | riscv_flush_icache | Flush I-Cache | RISC-V specific |
| 267 | syncfs | Sync filesystem | Data persistence |
| 270-271 | process_vm_readv/writev | Cross-process memory | Advanced IPC |
| 277 | seccomp | Syscall filtering | Security sandbox; very complex |
| 279 | memfd_create | Anonymous memory file | Nameless files |
| 424-470 | pidfd_*, io_uring*, landlock*, ... | Latest kernel features | Cutting-edge |

---

## 4. Implementation Plan

### Phase 1: Fix Syscall Number Mismatches (DONE)

1. **NR 117-120**: Moved setresuid/getresuid/setresgid/getresgid to correct NR 147-150
2. **NR 143**: Moved setregid to correct NR 143

### Phase 2: P0 Core Syscalls (DONE)

All P0 syscalls implemented:
1. sync/fsync/fdatasync (NR 81/82/83)
2. restart_syscall (NR 128) - stub
3. getitimer/setitimer (NR 102/103)
4. clock_settime (NR 112)
5. mknodat (NR 33)
6. fchmod (NR 52)
7. tgkill (NR 131)
8. getcpu (NR 168)
9. rt_sigtimedwait/rt_sigqueueinfo (NR 137/138) - stubs
10. execveat (NR 281)
11. signalfd4 (NR 74) - stub

### Phase 3: P1 Common Syscalls (PARTIALLY DONE)

Implemented:
1. Network completion: getsockopt/setsockopt/getsockname/getpeername/shutdown (stubs), sendmsg/recvmsg, accept4
2. Timer: timerfd_create/settime/gettime (stubs)
3. Resource limits: getrlimit/setrlimit
4. Other: reboot, sethostname, setdomainname, getrusage, setns (stub), fallocate (stub)

Remaining:
1. Filesystem: inotify (init1/add_watch/rm_watch)
2. Pipe/zero-copy: vmsplice/splice/tee
3. Shared memory: shmget/shmctl/shmat/shmdt (System V IPC)
4. Signal: rt_sigsuspend

### Phase 4: P2 Advanced Features (Long-term)

- Linux AIO, ptrace, NUMA policies, perf_event_open, io_uring
- RISC-V specific: riscv_hwprobe, riscv_flush_icache
- Containerization: unshare, setns, pivot_root
- Security: seccomp, key management
- Extended attributes: xattr operations (NR 5-16)

---

## 5. Statistics

| Category | Count |
|----------|-------|
| Total Linux RISC-V 64 syscalls | ~470 |
| Rux implemented (full) | ~95 |
| Rux implemented (stub) | ~25 |
| Rux implemented (total) | ~120 |
| Correct NR | ~120 |
| NR mismatched | ~3 (minor) |
| P0 unimplemented (core) | ~1 (xattr - complex) |
| P1 unimplemented (common) | ~8 |
| P2 unimplemented (advanced) | ~300+ |
| Implementation coverage | ~26% |
