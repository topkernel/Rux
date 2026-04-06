# Rux Development Roadmap and Feature List

## Project Overview

**Current Status**: Phase 40 - OOM Killer & Memory Safety

**Last Updated**: 2026-04-06

**Supported Architecture**: RISC-V 64-bit (RV64GC) - Only supported architecture

**Code Statistics**:
- **Source Files**: 266 (262 Rust + 1 Assembly + 3 Linker Script)
- **Total Lines of Code**: ~95,800
- **Kernel Unit Tests**: ~68 test cases (60 test files)
- **mini-lTP Tests**: 25 kernel compatibility tests
- **Smoke Tests**: 15 tests (all passing)
- **Syscall Dispatch**: 345 syscall numbers wired

**Design Philosophy**:
- External interfaces must be 100% compatible with Linux ABI
- Internal implementation can use better designs when it doesn't affect compatibility

---

## Legend

| Symbol | Meaning |
|--------|---------|
| ✅ | Implemented and Tested |
| ⚠️ | Partial Implementation / Partial Test |
| ❌ | Not Implemented / Not Tested |
| P0 | Core feature, must implement |
| P1 | Important feature, should implement |
| P2 | Enhanced feature, can add |
| P3 | Advanced feature, optional |

---

## Feature Implementation Status Overview

### 1. Boot and Initialization

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **1.1 OpenSBI Integration** | M-mode firmware loading, memory layout, S-mode entry | ✅ | P0 |
| **1.2 Boot Code** | Assembly entry, MMU trampoline, VMA/LMA linker script, stack setup, BSS zeroing, Rust code jump, medany model | ✅ | P0 |
| **1.3 UART Driver** | ns16550a driver, putc/getc, blocking read, TTY ISIG, println! | ✅ | P0-P1 |
| **1.4 CSR Management** | sstatus/sepc/stval/stvec/scause/satp/sie/sip, sscratch/tp protocol, stimecmp (SSTC) | ✅ | P0 |
| **1.5 Early Print** | Boot print, error output, debug output | ✅ | ⚠️ | P0-P2 |

### 2. Exception and Trap Handling

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **2.1 Exception Vector** | Direct mode | ✅ | P0 |
| | Vectored mode | ❌ | P2 |
| **2.2 Trap Handling** | PtRegs save/restore (Linux-style), user/kernel stack switch, CSR save, sret | ✅ | P0 |
| **2.3 Exception Types** | ecall, breakpoint, page fault, illegal instruction, alignment error | ✅ | P0-P1 |
| | Floating-point save/restore | ✅ | P0 |
| | Floating-point exception | ❌ | P2 |
| **2.4 Trap Return** | ret_from_exception, ret_from_fork_user, ret_from_fork_kernel, signal frame delivery | ✅ | P0-P1 |

### 3. System Calls

> 345 syscall numbers dispatched. Status: **I**mplemented, **P**artial, **N**ot implemented.

**3.1 System Call Framework** — dispatch (345 NRs), PtRegs frame, return value handling | ✅ P0

**3.2 File System** — openat, close, read, write, readv, writev, pread64, pwrite64, sendfile, lseek, getdents64, fstat, fstatat, statx, ioctl, fcntl, fsync, readlinkat, flock, mkdirat, rmdir, unlinkat, faccessat, chdir, getcwd, umask, futimesat, fchmod, fchown, fallocate, renameat2, newfstatat | ✅/⚠️ P0-P2

**3.3 Process Management** — fork, vfork, clone (CLONE_VM/FILES/FS/SIGHAND/THREAD/SETTLS/CHILD_CLEARTID/PARENT_SETTID/DETACH), execve, wait4, waitid, exit, exit_group, getpid, getppid, gettid, set_tid_address, kill, tkill, tgkill, getpriority, setpriority, set_robust_list, sched_yield, sched_getaffinity, sched_setaffinity, clone3, setuid, setgid, setreuid, setregid, setresuid, setresgid, getuid, geteuid, getgid, getegid, prctl (PR_SET_NAME/PR_GET_NAME/PR_SET_DUMPABLE), uname, prlimit64 | ✅/⚠️ P0-P2

**3.4 Signal** — rt_sigaction, rt_sigreturn, rt_sigprocmask, sigpending, sigaltstack, rt_sigtimedwait, rt_sigqueueinfo, pidfd_open, pidfd_send_signal, rt_sigreturn | ✅/⚠️ P0-P2

**3.5 Memory Management** — brk, mmap, munmap (MAP_PRIVATE/SHARED/FIXED/ANONYMOUS/GROWSDOWN), mprotect, mremap, madvise, mincore, msync, mlock, munlock | ✅/⚠️ P1-P3

**3.6 IPC** — pipe, pipe2, dup, dup2, dup3, eventfd2, select, pselect6 (FdSet 1024 fd ABI, signal mask), poll, ppoll, epoll_create1, epoll_ctl (ADD/MOD/DEL/WAIT), epoll_wait, epoll_pwait, futex (WAIT/WAKE/REQUEUE/CMP_REQUEUE/CLOCK_REALTIME, PI futex) | ✅/⚠️ P0-P2

**3.7 System V IPC** — semget, semctl (IPC_STAT/RMID/SET/GETVAL/SETVAL/GETALL/SETALL/GETPID/GETNCNT/GETZCNT/IPC_INFO/SEM_INFO), semop, semtimedop, msgget, msgctl (IPC_STAT/RMID/SET/INFO/MSG_INFO), msgsnd, msgrcv (MSG_EXCEPT/MSG_COPY/non-destructive read), shmget, shmctl (IPC_STAT/RMID/SET/INFO/SHM_STAT/SHM_INFO/SHM_LOCK/SHM_UNLOCK), shmat, shmdt | ✅/⚠️ P1

**3.8 POSIX Message Queue** — mq_open, mq_unlink, mq_timedsend, mq_timedreceive, mq_notify, mq_getsetattr | ✅/⚠️ P1

**3.9 Socket** — socket, bind, listen, accept4, connect, sendto, recvfrom, sendmsg, recvmsg, shutdown, getsockname, getpeername, socketpair, setsockopt, getsockopt | ✅/⚠️ P1-P2

**3.10 Other** — getrandom, nanosleep, clock_gettime, clock_getres, clock_nanosleep, gettimeofday, statfs, fstatfs, uname, syslog, umask | ✅/⚠️ P1-P2

### 4. Memory Management

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **4.1 Physical Memory** | Page descriptor, FrameAllocator (zone-based), physical memory detection, Memblock (early allocator) | ✅ | P0 |
| **4.2 Virtual Memory (Sv39)** | 3-level page table, PageTableEntry (R/W/X/U/G/D/A/COW/S), linear mapping, kernel mapping, MMU enable, Fixmap, ASID management (9-bit), TLB flush, huge page support (PMD/PGD) | ✅ | P0-P3 |
| **4.3 Heap Memory** | Buddy allocator (MAX_ORDER=10), Zone allocator (DMA/DMA32/NORMAL/MOVABLE), Per-CPU pagesets (PCP), Slab allocator (10 size classes), Object cache (SlabCache) | ✅ | P0-P2 |
| **4.4 Page Descriptors** | vmemmap, O(1) pfn_to_page, page refcount, page flags | ✅ | P0 |
| **4.5 User Memory** | mm_struct, VMA management (BTreeMap), mmap/munmap, fork address space (COW), copy_kernel_mappings (VPN2 sharing), demand paging, on-demand stack expansion, guard page | ✅/❌ | P1-P2 |
| **4.6 Copy-on-Write** | COW bit, fork COW, COW fault handler, mmap MAP_PRIVATE COW, free_user_page_tables | ✅ | P1 |
| **4.7 Reverse Mapping** | AnonVma/AnonVmaChain, rmap wired into page fault/COW/fork/unmap paths | ✅ | P2 |
| **4.8 Memory Reclamation** | Zone watermarks (WMARK_MIN/LOW/HIGH), LRU list infrastructure (5 lists), kswapd kernel thread, vmscan reclaim engine, page cache zone-allocated pages, page_to_pfn() fix, try_to_unmap (task scan + PTE clear), OOM killer (oom_badness scoring, SIGKILL, kswapd escalation), /proc/pid/oom_score, /proc/pid/oom_score_adj | ✅/⚠️/❌ | P2-P3 |
| **4.9 Memory Info** | /proc/meminfo, page statistics | ✅ | P1 |
| **4.10 Swap** | Swap support | ❌ | P1 |
| **4.11 Page Cache** | Page cache with shrinker interface, read-ahead | ✅ | P1 |

### 5. Process Management

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **5.1 Process Control Block** | Task structure, ThreadStruct, process state enum, PID management, PID hash table (256 buckets, O(1) lookup), PID reuse, kernel stack cache (64 slots) | ✅ | P0-P1 |
| **5.2 Process Tree** | Parent-child/sibling, ListHead, init process (PID 1) | ✅ | P0 |
| **5.3 Process Scheduling** | Per-CPU run queue, CFS scheduler (v1, enabled by default), Deadline scheduler (EDF + CBS), Real-time FIFO/RR, load balancing, CPU idle loop (WFI) | ✅/⚠️ | P0-P2 |
| **5.4 Context Switch** | context_switch, general register save, FPU register save, tp register update, schedule_tail | ✅ | P0 |
| **5.5 User Mode Support** | U-mode switch, user stack setup, ELF loading, auxiliary vector (15 AT_* entries) | ✅ | P0 |
| **5.6 Clone Flags** | CLONE_VM/FILES/FS/SIGHAND/THREAD/SETTLS/CHILD_CLEARTID/PARENT_SETTID/DETACH, clear_child_tid, robust_list | ✅ | P1 |
| **5.7 Signal Handling** | SignalStruct (64 slots), SigAction, signal mask, SIGKILL/SIGSTOP, signal handler (user-mode frame), signal frame (SigContext + UContext), rt_sigreturn trampoline, real-time signal queue, sigaltstack | ✅/⚠️ | P0-P2 |
| **5.8 Process Exit** | do_exit, SIGCHLD, zombie reaping, do_wait/do_wait_nonblock | ✅ | P0 |
| **5.9 Per-Process State** | FsStruct, FdTable, brk, oom_score_adj | ✅ | P0-P3 |
| **5.10 Credentials** | Cred struct (uid/gid/euid/egid/suid/sgid/fsuid/fsgid), fork inheritance | ✅ | P1 |
| **5.11 Kernel Threads** | kthread_create, kthread_should_stop, kthread_run | ✅ | P1 |

### 6. Interrupts and Timers

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **6.1 PLIC** | Initialization, priority/enable, claim/complete | ✅ | P0 |
| **6.2 External Interrupts** | UART, VirtIO (MMIO + PCI), interrupt sharing | ✅/❌ | P0-P2 |
| **6.3 Timer Interrupt** | SBI TIMER/SSTC (stimecmp), periodic interrupt, high-precision timer | ✅/❌ | P0-P2 |
| **6.4 IPI** | SBI IPI send, reschedule IPI, IPI handling (SSIP + bitmap multi-cpu) | ✅ | P0 |

### 7. SMP Multicore

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **7.1 Multicore Boot** | SBI HSM, secondary core boot, per-CPU interrupt stacks, hot plug CPU | ✅/❌ | P0-P3 |
| **7.2 Per-CPU Data** | Stack, run queue, idle task, pagesets, variables | ✅/⚠️ | P0-P1 |
| **7.3 Synchronization** | spin::Mutex, RwLock, SeqLock, kernel big lock | ✅/❌ | P0-P2 |

### 8. Synchronization Primitives

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **8.1 Semaphore** | Semaphore, down/up | ✅ | P0 |
| **8.2 Condvar** | ConditionVariable, wait/signal/broadcast, wait_timeout | ✅/❌ | P0-P1 |
| **8.3 Mutex** | Mutex, MutexGuard, deadlock detection | ✅/❌ | P0-P3 |
| **8.4 Futex** | Futex wait/wake, PI futex (LOCK_PI/UNLOCK_PI), futex requeue (REQUEUE/CMP_REQUEUE) | ✅/⚠️ | P1-P2 |

### 9. File System

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **9.1 VFS Framework** | file_open/close, path resolution, symbolic link, dentry/inode cache, LRU eviction, superblock, VFS mount/unmount | ✅ | P0-P1 |
| **9.2 File Descriptor** | FdTable (Arc-shared), alloc_fd/install_fd, fd reuse, O_CLOEXEC propagation | ✅ | P0 |
| **9.3 RootFS** | Memory filesystem, file/directory operations | ✅ | P0 |
| **9.4 ProcFS** | meminfo, cpuinfo, version, uptime, loadavg, cmdline, /proc/self, /proc/pid/ (status, stat, cmdline, exe, cwd, environ, fd, maps, oom_score, oom_score_adj), /proc/mounts, /proc/interrupts, /proc/self symlink | ✅ | P1 |
| **9.5 DevFS** | Device registry, /dev/input nodes | ✅ | P1 |
| **9.6 Pipe** | create_pipe, circular buffer, blocking read/write | ✅ | P0 |
| **9.7 JBD2 Journaling** | Journal module, transaction management, commit/recovery/checkpoint, revoke records | ✅/⚠️ | P2 |
| **9.8 ext4** | Superblock, block group descriptor, inode, BlockAllocator, InodeAllocator, mballoc (locality hint, goal-group spiral), directory operations, file read/write, seek, mkdir/rmdir, unlink, rename (renameat2), hard link (linkat), symbolic link, truncate (O_TRUNC), extent tree, JBD2 integration, O_EXCL | ✅/⚠️ | P0-P1 |
| **9.9 Page Cache** | Page cache with shrinker interface, read-ahead | ✅ | P1 |
| **9.10 Permission Management** | uid/gid enforcement | ❌ | P1 |

### 10. ELF Loader

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **10.1 ELF Parsing** | Header, program header, section header, dynamic linking (ld-musl, PT_INTERP, auxv) | ✅ | P0-P2 |
| **10.2 User Address Space** | Page table creation, PT_LOAD mapping, VM_EXECUTABLE, user stack, BSS zeroing, auxiliary vector | ✅ | P0 |
| **10.3 Program Execution** | Entry point validation, execve, multiple block sources, ASLR (KASLR offset field) | ✅/❌ | P0-P2 |
| **10.4 Interpreter** | PT_INTERP handling, ld-musl dynamic linker, shebang (#!) script support | ✅ | P2 |

### 11. Block Device Driver

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **11.1 VirtIO Framework** | Device detection, VirtQueue, Modern VirtIO PCI, VirtIO MMIO | ✅ | P0 |
| **11.2 VirtIO-blk** | Read/write (MMIO + PCI), multi-queue support | ✅/❌ | P0-P2 |
| **11.3 Buffer I/O** | BufferHead, block cache, bread/brelse | ✅ | P0 |
| **11.4 Block Device Framework** | GenDisk, request queue, request scheduling | ✅/❌ | P0-P2 |

### 12. Network Protocol Stack

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **12.1 Socket Layer** | socket, bind, listen, accept4, connect, sendto, recvfrom, sendmsg, recvmsg, shutdown, getsockname, getpeername, socketpair, setsockopt, getsockopt | ✅/⚠️ | P1-P2 |
| **12.2 TCP Protocol** | Three-way handshake, TCP state machine, four-way close, retransmission, sliding window, congestion control (slow start, congestion avoidance, fast retransmit), TCP checksum | ✅/⚠️ | P1-P2 |
| **12.3 UDP Protocol** | UDP datagram, checksum | ✅ | P1 |
| **12.4 IP Layer** | IPv4, routing table, ARP, ICMP, IP fragmentation | ✅/❌ | P1-P2 |
| **12.5 NIC Driver** | VirtIO-net, packet TX/RX, loopback device | ✅ | P1-P2 |
| **12.6 Protocol Stack** | SkBuff, protocol layering | ✅ | P2 |

### 13. Graphics and Input

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **13.1 Graphics** | framebuffer, fbdev, VirtIO-GPU | ✅/⚠️ | P2 |
| **13.2 Input Devices** | evdev, PS/2 keyboard/mouse, VirtIO input | ✅/⚠️ | P2 |
| **13.3 GUI Applications** | rux_gui library, desktop, calculator, clock, vshell | ✅/⚠️ | P2-P3 |

### 14. Diagnostics

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **14.1 DFX** | Diagnostic subsystem, kernel panic handler, stack trace, hung task detector | ✅ | P1 |
| **14.2 Logging** | printk with log levels, ring buffer, pr_debug/pr_info/pr_warn/pr_err macros | ✅ | P0 |
| **14.3 Error Handling** | errno module (50+ error constants), Result/Option propagation | ✅ | P0 |

### 15. Unit Testing

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **15.1 Test Framework** | unit-test feature, 60 test files | ✅ | P0 |
| **15.2 Data Structure Tests** | ListHead, Path, FileFlags | ✅ | P0 |
| **15.3 Memory Tests** | heap, page allocator, COW | ✅ | P0 |
| **15.4 Process Tests** | scheduler, signal, fork/execve/wait4 | ✅/⚠️ | P0 |
| **15.5 File System Tests** | file_open, fdtable, dcache/icache, ext4 | ✅/⚠️ | P0 |
| **15.6 Device Tests** | virtio_queue | ✅ | P0 |
| **15.7 Integration Tests** | System boot, multicore | ✅ | P0 |
| **15.8 mini-lTP Tests** | 25 kernel compatibility tests | ✅ | P1 |
| **15.9 Smoke Tests** | 15 core functionality tests | ✅ | P0 |

### 16. Build and Development Tools

| Feature | Sub-feature | Status | Priority |
|---------|-------------|--------|----------|
| **16.1 Build System** | Cargo workspace, Makefile, QEMU launch scripts | ✅ | P0 |
| **16.2 Configuration** | Kernel.toml, menuconfig | ✅ | P0 |
| **16.3 Test Scripts** | test/run.sh, smoke test runner | ✅ | P0 |
| **16.4 Documentation** | README, architecture docs (boot, memory, RISC-V, structure), design docs, development guides | ✅ | P0 |

---

## Development Phase Planning

### Phase 1-5: Foundation ✅
Boot (OpenSBI, MMU trampoline, medany), exception handling (ecall, page fault, breakpoint), basic syscalls (read/write/exit), memory management (buddy, page tables), process basics (fork, execve, scheduler)

### Phase 6-10: Core Infrastructure ✅
Interrupts (PLIC, timer, IPI), SMP (4-core HSM boot, per-CPU data), synchronization (spinlock, RwLock, mutex, condvar, semaphore), filesystem VFS framework, ELF loader

### Phase 11-15: User Mode & Process Refinement ✅
User-mode support (U-mode switch, user stack, signal frame), process refinement (clone flags, COW, robust_list), signal handling (64 signal slots, rt_sigaction, real-time queue, sigaltstack), pipe, comprehensive unit testing (60 test files, mini-lTP)

### Phase 16-17: Storage & Filesystem ✅
Preemptive scheduling, block device driver (VirtIO-blk), ext4 filesystem (superblock, inode, directory, file operations, extent tree), bio buffer cache with block cache

### Phase 18: Network Stack ✅
SkBuff, Ethernet, ARP, IPv4, UDP, TCP (three-way handshake, state machine, retransmission), VirtIO-net, socket syscalls (socket/bind/listen/connect/sendto/recvfrom)

### Phase 18.5-18.6: Memory Refactoring ✅
Platform-independent pagemap abstraction, VirtIO PCI probe refactoring, per-CPU pagesets

### Phase 19: Modern VirtIO PCI & Shell ✅
Modern VirtIO 1.0+ PCI driver, ext4 extent tree, shell (mrsh) running on rootfs

### Phase 20: Multi Shell & Toolchain ✅
cmdline parsing fix, musl libc toolchain, multi shell support, toybox integration

### Phase 21: Boot Output Refactoring ✅
Boot log beautification, ASCII art logo, kernel version info

### Phase 22: procfs, Symlinks, toybox ✅
Full procfs (/proc/meminfo, /proc/cpuinfo, /proc/version, /proc/uptime, /proc/pid/*, /proc/mounts, /proc/interrupts), ext4 symbolic links, TLS fix

### Phase 23: CFS Scheduler, COW, Graphics ✅
CFS scheduler v1 (enabled by default, vruntime-based fair scheduling), copy-on-write (fork COW, mmap MAP_PRIVATE COW), graphics driver (framebuffer), input devices (evdev), GUI applications (rux_gui)

### Phase 24: devfs, mini-ltp, Code Cleanup ✅
devfs filesystem, 25 mini-lTP kernel compatibility tests, VFS path resolution cleanup

### Phase 25: TCP Reliability & Signal Refinement ✅
TCP retransmission with RTO calculation, signal mechanism refinement (sigaltstack, real-time queue)

### Phase 26: Documentation Update ✅
Architecture documentation (boot, memory, RISC-V, structure), design philosophy refinement

### Phase 27: Linux-Style Memory Management ✅
Zone allocator (DMA/DMA32/NORMAL/MOVABLE), vmemmap, per-CPU pagesets, memblock early allocator, three-stage page table allocation, page descriptors with refcount, demand paging, on-demand stack expansion, COW fault handler, ASID management (9-bit, 512 max), copy_kernel_mappings (VPN2 sharing), reverse mapping (AnonVma), huge page framework

### Phase 28: Linux-Style Boot & Architecture Refactoring ✅
MMU trampoline in boot.S, VMA/LMA linker script, kernel linked at KERNEL_LINK_ADDR, medany code model, PtRegs at kernel stack top, FPU context save/restore in context switch, sscratch/tp protocol, ret_from_fork paths, uaccess.S assembly, JBD2 journaling layer, sys_mkdirat/rmdir/unlinkat, kernel big lock, enhanced procfs

### Phase 29: ext4 File Write & Safety ✅
copy_from_user/copy_to_user safety, ext4 file write correctness (i_blocks, timestamps, O_APPEND, O_TRUNC), read-modify-write, extent tree depth > 0, environment variables via execve, toybox symlinks, shell PATH search, printk with log levels, PCI VirtIO block write (persist across reboot), sys_renameat/renameat2, sys_linkat (hard links)

### Phase 30: Extended Syscalls & Smoke Test ✅
pread64/preadv/writev, dup3, pipe2, kill(0), gettid, statfs/fstatfs, poll blocking, O_CLOEXEC propagation, ext4 O_EXCL, sys_sendfile, brk shrinking, clock_nanosleep, UART blocking read, TTY ISIG, mmap MAP_PRIVATE COW, comprehensive smoke test suite (15 tests)

### Phase 31: Syscall Dispatch Audit & Linux ABI Compatibility ✅
345 syscall dispatch arms audited against syscall.tbl, 6 incorrect mappings fixed, sys_ppoll implementation, lazy FPU enable via handle_illegal_instruction, shebang script support, TCGETS/TCSETS ioctl corrections, poll timeout=-1 as infinite wait

### Phase 32: VFS Dentry Tree Cleanup & Concurrent I/O ✅
Unified path resolution through dentry tree, O_CREAT dentry caching fix, rec_len=0 protection, concurrent I/O stress tests (2-process parallel read, /proc read)

### Phase 33: Unified VFS — All Operations Through inode.ops ✅
Unified readdir/open callbacks in INodeOps, all filesystems implement get_file_ops/readdir/open, unified VfsDirEntry, DIR_FILE_OPS shared, vfs.rs reduced from 2627 to 1469 lines

### Phase 34: JBD2 Journaling for ext4 Metadata ✅
JBD2 journal commit and recovery, synchronous commit (freeze → write descriptor+data+commit → update superblock), all ext4 metadata ops wrapped in transactions, journal superblock corruption fix (j_head initialization), ext4_unlink_inner data block leak fix, basic recovery (scan + replay)

### Phase 35: VFS Path Resolution Cleanup & Dead Code Removal ✅
Centralized read_user_path()/read_user_str() helpers, 14 syscalls refactored, sys_fchdir, fixed dirfd-ignored bugs, removed dead code (ext4 standalone list_dir, path.rs stubs)

### Phase 36: Filesystem Refactoring Completion ✅
Multi-lock bio cache (per-bucket spinlock), mballoc (locality hint, goal-group spiral search, block preallocation), async I/O framework (IoCompletion, batch read-ahead), additional fixes (symlinkat, statx, openat2, rootfs cross-directory corruption, ext4 indirect block leak, uaccess strncpy_from_user overflow)

### Phase 37: IPC Subsystem (System V + POSIX MQ) ✅
Complete IPC module (5 submodules): IpcIds generic registry, System V semaphores (semget/semctl/semop/semtimedop with SEM_UNDO), System V message queues (msgget/msgctl/msgsnd/msgrcv with MSG_EXCEPT/MSG_COPY), System V shared memory (shmget/shmctl/shmat/shmdt), POSIX message queues (mq_open/mq_unlink/mq_timedsend/mq_timedreceive/mq_notify/mq_getsetattr), 18 IPC syscalls dispatched

### Phase 38: IPC Correctness & Feature Completion ✅
6 rounds of correctness fixes: lost wakeup races (prepare_to_wait/finish_wait pattern), fork credential inheritance, POSIX MQ close path (sys_close interception for fd>=512), refcount-based queue lifecycle, SEM_UNDO dedup, SIGEV_SIGNAL for mq_notify, MSG_COPY non-destructive read, SHM_STAT/SHM_INFO/MSG_INFO/SEM_INFO, correct blocking for all IPC paths. Smoke test 15/15 PASS.

### Phase 39: Reverse Mapping & try_to_unmap ✅
Wired reverse mapping into all page table operation paths: page fault handler sets Anonymous flag/index/mapcount, COW handler sets rmap on new pages, fork path increments mapcount for shared pages, unmap/exec paths call page_remove_rmap before put_page. Implemented try_to_unmap using task-scan approach: iterate all tasks via pid_hash_for_each_task, check VMAs for virtual address stored in Page.index, verify PPN match via page table walk, clear PTE + flush TLB. Fixed LRU field repurposing bug in page_remove_rmap (Page.mapping/index conflicted with LRU prev/next pointers). Anonymous page reclaim disabled (requires swap support).

### Phase 40: OOM Killer & Memory Safety ✅
OOM killer following mm/oom_kill.c: oom_badness() scores by total_vm + oom_score_adj * totalpages/1000, select_bad_process() picks highest scorer (skips kernel threads, PID 1, MMF_OOM_DISABLE, immune tasks), oom_kill_process() sends SIGKILL to victim + mm-sharing processes then sets TIF_MEMDIE. kswapd OOM escalation after 16 consecutive reclaim failures with boot guard. Added pid_hash_for_each_task() for complete task iteration, oom_score_adj field in Task struct, /proc/pid/oom_score and /proc/pid/oom_score_adj procfs entries.

---

## High Priority Features (P1)

### Memory Management
- [ ] Swap support — enables anonymous page reclaim, completes the OOM→reclaim→free cycle
- [ ] Guard page support — stack overflow detection, memory safety
- [ ] LRU integration — add page cache pages to LRU_INACTIVE_FILE (deferred — needs dedicated LRU fields in Page struct)
- [ ] Slab allocator tests

### File System
- [ ] Permission management (uid/gid enforcement in file operations)
- [ ] IO_uring support

### Process
- [ ] PID namespace isolation
- [ ] cgroup v1 (basic resource control)

### Network
- [ ] ICMP support — required by ping, path MTU discovery
- [ ] IP fragmentation/reassembly — required for jumbo frames
- [ ] Complete TCP four-way close (FIN/ACK exchange)

---

## Medium Priority Features (P2)

### Memory
- [ ] Memory compaction — reduce external fragmentation
- [ ] Huge page integration with page fault handler (transparent huge pages)

### Synchronization
- [ ] SeqLock — lock-free reads for frequently-read data
- [ ] wait_timeout for condvar
- [ ] RCU mechanism — read-copy-update for lock-free reads

### System Calls
- [ ] POSIX timers (timer_create/timer_settime/timer_delete/timer_getoverrun)
- [ ] High-precision timer

### Architecture
- [ ] Device tree (DTB) parser — hardware description
- [ ] Vectored mode trap — faster interrupt dispatch

---

## Low Priority Features (P3)

- Virtualization (KVM, containers)
- Security (capability, SELinux)
- Power management (frequency scaling, hibernate)
- Multimedia (audio, video)
- CPU hot plug
- Real-time scheduling (full POSIX real-time)
- ASLR / KASLR — address space layout randomization

---

**Document Version**: v11.0
**Last Updated**: 2026-04-06
**Maintainer**: Rux Development Team
