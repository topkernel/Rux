# Rux Syscall Compatibility Analysis Report

> Date: 2026-04-05
> Reference: `docs/development/syscall.tbl` (Linux RISC-V 64 syscall number table)
> Kernel file: `kernel/src/syscall/dispatch.rs`

## Overview

Rux currently implements approximately 240 syscall number mappings. This document analyzes:
1. Syscall number mismatches (incompatible with Linux ABI)
2. Unimplemented syscalls
3. Prioritized implementation plan

---

## 1. Syscall Number Mismatches

**All syscalls now use correct Linux RISC-V 64 ABI numbers.**

Previously fixed:
- NR 117-120 setresuid/getresuid/setresgid/getresgid — moved to correct NR 147-150
- NR 143 setregid — moved to correct NR 143
- NR 134 was mapped twice (rt_sigaction + rt_sigsuspend) — fixed
- NR 136 (rt_sigpending) was missing — added
- NR 274/275 sched_setattr/sched_getattr — moved from wrong 351/352 to correct 274/275

Minor notes (no functional impact):
- NR 39: umount2 and umount have the same signature
- NR 88: function name `sys_futimesat` but implements utimensat
- NR 276: renameat2 ignores flags parameter

---

## 2. Implemented Syscalls (By Category)

### Async I/O

| NR | Syscall | Status |
|----|---------|--------|
| 0 | io_setup | STUB (returns -ENOSYS) |
| 1 | io_destroy | STUB (returns -ENOSYS) |
| 2 | io_submit | STUB (returns -ENOSYS) |
| 3 | io_cancel | STUB (returns -ENOSYS) |
| 4 | io_getevents | STUB (returns -ENOSYS) |

### Extended Attributes

| NR | Syscall | Status |
|----|---------|--------|
| 5-16 | \*xattr (12 syscalls) | STUBS (return -ENOSYS) |

### File Operations

| NR | Syscall | Status |
|----|---------|--------|
| 17 | getcwd | OK |
| 18 | lookup_dcookie | STUB (returns -ENOSYS) |
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
| 41 | pivot_root | STUB (returns -ENOSYS) |
| 42 | nfsservctl | STUB (returns -ENOSYS) |
| 43 | statfs | OK |
| 44 | fstatfs | OK |
| 45 | truncate | OK |
| 46 | ftruncate | OK |
| 47 | fallocate | STUB (returns -ENOSYS) |
| 48 | faccessat | OK |
| 49 | chdir | OK |
| 50 | fchdir | OK |
| 51 | chroot | OK (stub, no actual root switch) |
| 52 | fchmod | OK |
| 53 | fchmodat | OK |
| 54 | fchownat | OK |
| 55 | fchown | OK |
| 56 | openat | OK |
| 57 | close | OK |
| 58 | vhangup | OK (stub) |
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
| 75 | vmsplice | STUB (returns -ENOSYS) |
| 76 | splice | OK |
| 77 | tee | STUB (returns -ENOSYS) |
| 78 | readlinkat | OK |
| 79 | fstatat | OK |
| 80 | fstat | OK |
| 81 | sync | OK |
| 82 | fsync | OK (success stub) |
| 83 | fdatasync | OK (success stub) |
| 84 | sync_file_range | OK (delegates to sync_buffers) |
| 88 | utimensat | OK |
| 89 | acct | STUB (returns -ENOSYS) |
| 166 | umask | OK |
| 213 | readahead | OK (no-op) |
| 224 | swapon | STUB (returns -ENOSYS) |
| 225 | swapoff | STUB (returns -ENOSYS) |
| 234 | remap_file_pages | OK (deprecated, no-op) |
| 264 | name_to_handle_at | STUB (returns -ENOSYS) |
| 265 | open_by_handle_at | STUB (returns -ENOSYS) |
| 276 | renameat2 | OK |
| 285 | copy_file_range | OK |
| 286 | preadv2 | OK (delegates to preadv) |
| 287 | pwritev2 | OK (delegates to pwritev) |
| 291 | statx | OK |
| 437 | openat2 | OK |

### Process Management

| NR | Syscall | Status |
|----|---------|--------|
| 90 | capget | OK (returns empty caps) |
| 91 | capset | STUB (returns -EPERM) |
| 92 | personality | OK (returns 0 = PER_LINUX) |
| 93 | exit | OK |
| 94 | exit_group | OK |
| 95 | waitid | OK |
| 96 | set_tid_address | OK |
| 97 | unshare | STUB (returns -ENOSYS) |
| 99 | set_robust_list | OK (stub) |
| 100 | get_robust_list | OK (returns NULL head) |
| 102 | getitimer | OK (returns zeros) |
| 103 | setitimer | STUB (returns -ENOSYS) |
| 104 | kexec_load | STUB (returns -ENOSYS) |
| 105 | init_module | STUB (returns -ENOSYS) |
| 106 | delete_module | STUB (returns -ENOSYS) |
| 112 | clock_settime | OK (requires root) |
| 117 | ptrace | STUB (returns -ENOSYS) |
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
| 151 | setfsuid | OK |
| 152 | setfsgid | OK |
| 153 | times | OK (returns jiffies) |
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
| 179 | sysinfo | OK (partial info) |
| 220 | clone | OK |
| 221 | execve | OK |
| 240 | perf_event_open | STUB (returns -ENOSYS) |
| 260 | wait4 | OK |
| 261 | prlimit64 | OK |
| 268 | setns | STUB (returns -ENOSYS) |
| 270 | process_vm_readv | STUB (returns -ENOSYS) |
| 271 | process_vm_writev | STUB (returns -ENOSYS) |
| 272 | kcmp | STUB (returns -ENOSYS) |
| 273 | finit_module | STUB (returns -ENOSYS) |
| 277 | seccomp | STUB (returns -ENOSYS) |
| 279 | memfd_create | STUB (returns -ENOSYS) |
| 280 | bpf | STUB (returns -ENOSYS) |
| 281 | execveat | OK |
| 282 | userfaultfd | STUB (returns -ENOSYS) |
| 283 | membarrier | OK (global barrier) |

### Signal

| NR | Syscall | Status |
|----|---------|--------|
| 74 | signalfd4 | STUB (returns -ENOSYS) |
| 98 | futex | OK |
| 128 | restart_syscall | STUB (returns -ENOSYS) |
| 132 | sigaltstack | OK |
| 133 | rt_sigsuspend | OK (full impl) |
| 134 | rt_sigaction | OK |
| 135 | rt_sigprocmask | OK |
| 136 | rt_sigpending | OK |
| 139 | rt_sigreturn | OK |

### Memory Management

| NR | Syscall | Status |
|----|---------|--------|
| 214 | brk | OK |
| 215 | munmap | OK |
| 216 | mremap | OK |
| 222 | mmap | OK |
| 223 | fadvise64 | OK (no-op) |
| 226 | mprotect | OK |
| 227 | msync | OK |
| 228 | mlock | OK (stub) |
| 229 | munlock | OK (stub) |
| 230 | mlockall | OK (stub) |
| 231 | munlockall | OK (stub) |
| 232 | mincore | OK |
| 233 | madvise | OK |
| 235 | mbind | STUB (returns -ENOSYS) |
| 236 | get_mempolicy | STUB (returns -ENOSYS) |
| 237 | set_mempolicy | STUB (returns -ENOSYS) |
| 238 | migrate_pages | STUB (returns -ENOSYS) |
| 239 | move_pages | STUB (returns -ENOSYS) |
| 284 | mlock2 | OK (stub) |
| 288 | pkey_mprotect | STUB (returns -ENOSYS) |
| 289 | pkey_alloc | STUB (returns -ENOSYS) |
| 290 | pkey_free | STUB (returns -ENOSYS) |
| 292 | io_pgetevents | STUB (returns -ENOSYS) |

### Time

| NR | Syscall | Status |
|----|---------|--------|
| 101 | nanosleep | OK |
| 107 | timer_create | STUB (returns -ENOSYS) |
| 108 | timer_gettime | STUB (returns -ENOSYS) |
| 109 | timer_getoverrun | STUB (returns -ENOSYS) |
| 110 | timer_settime | STUB (returns -ENOSYS) |
| 111 | timer_delete | STUB (returns -ENOSYS) |
| 113 | clock_gettime | OK |
| 114 | clock_getres | OK |
| 115 | clock_nanosleep | OK |
| 170 | settimeofday | OK (requires root, stub) |
| 171 | adjtimex | STUB (returns -ENOSYS) |
| 266 | clock_adjtime | STUB (returns -ENOSYS) |
| 169 | gettimeofday | OK |

### Scheduler

| NR | Syscall | Status |
|----|---------|--------|
| 118 | sched_setparam | OK |
| 119 | sched_setscheduler | OK |
| 120 | sched_getscheduler | OK |
| 121 | sched_getaffinity | OK (returns all CPUs) |
| 122 | sched_getparam | OK |
| 123 | sched_setaffinity | STUB (returns -ENOSYS) |
| 124 | sched_yield | OK |
| 125 | sched_get_priority_max | OK |
| 126 | sched_get_priority_min | OK |
| 127 | sched_rr_get_interval | OK |
| 274 | sched_setattr | OK |
| 275 | sched_getattr | OK |

### Network

| NR | Syscall | Status |
|----|---------|--------|
| 198 | socket | OK |
| 199 | socketpair | STUB (returns -ENOSYS) |
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
| 243 | recvmmsg | STUB (returns -ENOSYS) |
| 269 | sendmmsg | STUB (returns -ENOSYS) |

### IPC - POSIX Message Queues

| NR | Syscall | Status |
|----|---------|--------|
| 180 | mq_open | STUB (returns -ENOSYS) |
| 181 | mq_unlink | STUB (returns -ENOSYS) |
| 182 | mq_timedsend | STUB (returns -ENOSYS) |
| 183 | mq_timedreceive | STUB (returns -ENOSYS) |
| 184 | mq_notify | STUB (returns -ENOSYS) |
| 185 | mq_getsetattr | STUB (returns -ENOSYS) |

### IPC - System V

| NR | Syscall | Status |
|----|---------|--------|
| 186 | msgget | STUB (returns -ENOSYS) |
| 187 | msgctl | STUB (returns -ENOSYS) |
| 188 | msgrcv | STUB (returns -ENOSYS) |
| 189 | msgsnd | STUB (returns -ENOSYS) |
| 190 | semget | STUB (returns -ENOSYS) |
| 191 | semctl | STUB (returns -ENOSYS) |
| 192 | semtimedop | STUB (returns -ENOSYS) |
| 193 | semop | STUB (returns -ENOSYS) |
| 194 | shmget | STUB (returns -ENOSYS) |
| 195 | shmctl | STUB (returns -ENOSYS) |
| 196 | shmat | STUB (returns -ENOSYS) |
| 197 | shmdt | STUB (returns -ENOSYS) |

### Miscellaneous

| NR | Syscall | Status |
|----|---------|--------|
| 19 | eventfd2 | OK |
| 20 | epoll_create1 | OK |
| 21 | epoll_ctl | OK |
| 22 | epoll_pwait | OK |
| 26-28 | inotify_init1/add_watch/rm_watch | STUBS |
| 30 | ioprio_set | STUB (returns -ENOSYS) |
| 31 | ioprio_get | OK (returns 0) |
| 60 | quotactl | STUB (returns -ENOSYS) |
| 72 | pselect6 | OK |
| 73 | ppoll | OK |
| 85-87 | timerfd_create/settime/gettime | STUBS |
| 116 | syslog | OK |
| 160 | uname | OK |
| 262 | fanotify_init | STUB (returns -ENOSYS) |
| 263 | fanotify_mark | STUB (returns -ENOSYS) |
| 278 | getrandom | OK |
| 290 | eventfd | OK |
| 293 | rseq | STUB (returns -ENOSYS) |

### RISC-V Specific

| NR | Syscall | Status |
|----|---------|--------|
| 258 | riscv_hwprobe | OK (reads CSRs) |
| 259 | riscv_flush_icache | OK (fence.i) |

---

## 3. Unimplemented Syscalls

All Linux RISC-V 64 syscalls from NR 0-470 are now registered in dispatch.rs.
NR 244-248 are architecture-specific (arc/csky/nios2/or1k) — not applicable to RISC-V.
NR 295-402 are unassigned (no syscall exists at these numbers).

---

## 4. Implementation Plan

### Phase 1-4: All Previous Phases (DONE)

### Phase 5: NR 294, 403-470 — time64 variants & latest kernel features (DONE)

**_time64 variants (NR 403-423):**
On 64-bit RISC-V, all 21 time64 syscalls delegate to existing implementations.
- clock_gettime64/settime64/adjtime64, clock_getres_time64, clock_nanosleep_time64
- timer_gettime64/settime64, timerfd_gettime64/settime64
- utimensat_time64, pselect6_time64, ppoll_time64
- io_pgetevents_time64, recvmmsg_time64
- mq_timedsend_time64, mq_timedreceive_time64
- semtimedop_time64, rt_sigtimedwait_time64, futex_time64
- sched_rr_get_interval_time64

**Process management (NR 424-448):**
- pidfd_send_signal, pidfd_open, pidfd_getfd (stubs)
- io_uring_setup, io_uring_enter, io_uring_register (stubs)
- clone3 (stub)
- close_range (full implementation)
- faccessat2 (delegates to faccessat)
- process_madvise, memfd_secret, process_mrelease (stubs)

**Filesystem (NR 428-433, 441-470):**
- open_tree, move_mount, fsopen, fsconfig, fsmount, fspick (stubs)
- epoll_pwait2 (delegates to epoll_pwait)
- mount_setattr, quotactl_fd (stubs)
- landlock_create_ruleset, landlock_add_rule, landlock_restrict_self (stubs)
- futex_waitv (stub), set_mempolicy_home_node (stub), cachestat (stub)
- fchmodat2 (delegates to fchmodat), map_shadow_stack (stub)
- futex_wake/wait/requeue (delegate to futex)
- statmount, listmount (stubs)
- lsm_get_self_attr, lsm_set_self_attr, lsm_list_modules (stubs)
- mseal, setxattrat/getxattrat/listxattrat/removexattrat (stubs)
- open_tree_attr, file_getattr, file_setattr, listns (stubs)
- kexec_file_load (stub)

---

## 5. Statistics

| Category | Count |
|----------|-------|
| Total Linux RISC-V 64 syscalls | ~470 |
| Rux registered in dispatch.rs | ~340 |
| Rux implemented (full) | ~100 |
| Rux implemented (stub) | ~230 |
| Rux implemented (delegating to existing) | ~10 |
| Correct NR | ~340 |
| NR mismatched | 0 |
| Not applicable (arch-specific NR 244-248) | 5 |
| Implementation coverage | ~72% |
