# Rux Development Roadmap and Feature List

## Project Overview

**Current Status**: Phase 26 - Documentation Update and Design Philosophy Refinement

**Last Updated**: 2026-03-13

**Supported Architecture**: RISC-V 64-bit (RV64GC) - Only supported architecture

**Code Statistics**:
- **Rust Source Files**: 189
- **Total Lines of Code**: ~59,100
- **Kernel Unit Tests**: 51 test files
- **mini-lTP Tests**: 24 kernel compatibility tests

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

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **1.1 OpenSBI Integration** | M-mode firmware loading | ✅ | ✅ | P0 |
| | Memory layout (0x80200000) | ✅ | ✅ | P0 |
| | S-mode entry | ✅ | ✅ | P0 |
| **1.2 Boot Code** | Assembly boot entry | ✅ | ✅ | P0 |
| | Stack setup (16KB) | ✅ | ✅ | P0 |
| | BSS segment zeroing | ✅ | ✅ | P0 |
| | Rust code jump | ✅ | ✅ | P0 |
| | Data segment initialization | ✅ | ✅ | P0 |
| **1.3 UART Driver** | ns16550a driver | ✅ | ✅ | P0 |
| | Character output (putc) | ✅ | ✅ | P0 |
| | Character input (getc) | ✅ | ✅ | P0 |
| | println! macro | ✅ | ✅ | P0 |
| | Baud rate configuration | ⚠️ | ⚠️ | P1 |
| **1.4 CSR Management** | sstatus, sepc, stval, stvec, scause, satp, sie/sip | ✅ | ✅ | P0 |
| **1.5 Early Print** | Boot print, Error output | ✅ | ✅ | P0 |
| | Debug output | ⚠️ | ⚠️ | P2 |

### 2. Exception Handling

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **2.1 Exception Vector Table** | Direct mode | ✅ | ✅ | P0 |
| | Vectored mode | ❌ | ❌ | P2 |
| **2.2 Trap Handling** | TrapFrame save/restore | ✅ | ✅ | P0 |
| | User/kernel stack switch | ✅ | ✅ | P0 |
| | CSR register save | ✅ | ✅ | P0 |
| | sret return | ✅ | ✅ | P0 |
| **2.3 Exception Types** | System call (ecall) | ✅ | ✅ | P0 |
| | Breakpoint | ✅ | ✅ | P0 |
| | Page fault (load/store) | ✅ | ⚠️ | P0 |
| | Illegal instruction | ✅ | ✅ | P0 |
| | Alignment error | ⚠️ | ⚠️ | P1 |
| | Floating-point exception | ❌ | ❌ | P2 |

### 3. System Calls

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **3.1 System Call Framework** | System call dispatch | ✅ | ✅ | P0 |
| | SyscallFrame | ✅ | ✅ | P0 |
| | Return value handling | ✅ | ✅ | P0 |
| | Parameter validation | ⚠️ | ⚠️ | P1 |
| **3.2 File System Syscalls** | sys_openat | ✅ | ✅ | P0 |
| | sys_close | ✅ | ✅ | P0 |
| | sys_read/write | ✅ | ✅ | P0 |
| | sys_lseek | ✅ | ✅ | P0 |
| | sys_fstat/fstatat | ✅ | ✅ | P1 |
| | sys_statx | ❌ | ❌ | P2 |
| | sys_ioctl | ✅ | ⚠️ | P2 |
| | sys_fcntl | ✅ | ⚠️ | P1 |
| | sys_fsync | ✅ | ✅ | P2 |
| | sys_readlinkat | ✅ | ✅ | P1 |
| **3.3 Process Management** | sys_fork/vfork | ✅ | ✅ | P0 |
| | sys_execve | ✅ | ✅ | P0 |
| | sys_wait4 | ✅ | ✅ | P0 |
| | sys_waitid | ❌ | ❌ | P1 |
| | sys_exit | ✅ | ✅ | P0 |
| | sys_getpid/getppid | ✅ | ✅ | P0 |
| | sys_gettid | ✅ | ✅ | P1 |
| | sys_set_tid_address | ✅ | ✅ | P2 |
| | sys_kill | ✅ | ✅ | P0 |
| | sys_getpriority/setpriority | ✅ | ⚠️ | P1 |
| **3.4 Signal Syscalls** | sys_rt_sigaction | ✅ | ✅ | P0 |
| | sys_rt_sigreturn | ⚠️ | ⚠️ | P1 |
| | sys_rt_sigprocmask | ✅ | ✅ | P1 |
| | sys_sigpending | ✅ | ⚠️ | P1 |
| | sys_sigaltstack | ⚠️ | ⚠️ | P2 |
| **3.5 Memory Management** | sys_brk | ✅ | ✅ | P1 |
| | sys_mmap/munmap | ✅ | ✅ | P1 |
| | sys_mprotect | ✅ | ✅ | P2 |
| | sys_mremap | ⚠️ | ⚠️ | P3 |
| | sys_madvise | ⚠️ | ⚠️ | P2 |
| | sys_mincore | ⚠️ | ⚠️ | P3 |
| | sys_msync | ⚠️ | ⚠️ | P2 |
| **3.6 IPC Syscalls** | sys_pipe/pipe2 | ✅ | ✅ | P0 |
| | sys_dup/dup2/dup3 | ✅ | ⚠️ | P1 |
| | sys_select/poll | ✅ | ⚠️ | P1 |
| | sys_epoll_create/ctl/wait | ✅ | ⚠️ | P1 |
| | sys_eventfd | ✅ | ⚠️ | P2 |
| **3.7 Socket Syscalls** | sys_socket | ✅ | ✅ | P1 |
| | sys_bind/listen | ✅ | ✅ | P1 |
| | sys_accept | ⚠️ | ⚠️ | P1 |
| | sys_connect | ✅ | ✅ | P1 |
| | sys_sendto/recvfrom | ⚠️ | ⚠️ | P1 |
| **3.8 Other Syscalls** | sys_uname | ✅ | ✅ | P1 |
| | sys_prlimit64 | ✅ | ✅ | P2 |
| | sys_getrandom | ✅ | ✅ | P2 |
| | sys_clone | ✅ | ⚠️ | P1 |
| | sys_futex | ✅ | ⚠️ | P1 |

### 4. Memory Management

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **4.1 Physical Memory** | PhysFrame/VirtPage | ✅ | ✅ | P0 |
| | FrameAllocator | ✅ | ✅ | P0 |
| | Physical memory detection | ✅ | ✅ | P0 |
| **4.2 Virtual Memory (Sv39)** | 3-level page table | ✅ | ✅ | P0 |
| | PageTableEntry | ✅ | ✅ | P0 |
| | Page table mapping | ✅ | ✅ | P0 |
| | Identity mapping | ✅ | ✅ | P0 |
| | MMU enable | ✅ | ✅ | P0 |
| | Platform-independent interface | ✅ | ✅ | P0 |
| | Huge page support | ❌ | ❌ | P3 |
| **4.3 Heap Memory** | BuddyAllocator | ✅ | ✅ | P0 |
| | Slab allocator | ✅ | ❌ | P2 |
| | Object cache (SlabCache) | ✅ | ❌ | P2 |
| **4.4 User Memory** | User address space | ✅ | ✅ | P1 |
| | VMA management | ✅ | ✅ | P1 |
| | mmap/munmap | ✅ | ✅ | P1 |
| | fork address space | ✅ | ✅ | P1 |
| | Guard page | ❌ | ❌ | P2 |
| **4.5 Copy-on-Write** | Write-on-copy | ✅ | ✅ | P1 |
| | fork COW | ✅ | ✅ | P1 |
| **4.6 Memory Reclamation** | Page reclamation | ❌ | ❌ | P2 |
| | kswapd | ❌ | ❌ | P2 |
| | OOM killer | ❌ | ❌ | P3 |

### 5. Process Management

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **5.1 Process Control Block** | Task structure | ✅ | ✅ | P0 |
| | CpuContext | ✅ | ✅ | P0 |
| | Process state enum | ✅ | ✅ | P0 |
| | PID management | ✅ | ✅ | P0 |
| | PID namespace | ❌ | ❌ | P2 |
| **5.2 Process Tree** | Parent-child relationship | ✅ | ✅ | P0 |
| | Sibling relationship | ✅ | ✅ | P0 |
| | ListHead doubly linked list | ✅ | ✅ | P0 |
| | init process | ✅ | ✅ | P0 |
| **5.3 Process Scheduling** | Per-CPU run queue | ✅ | ✅ | P0 |
| | Round Robin algorithm | ✅ | ✅ | P0 |
| | Load balancing | ✅ | ⚠️ | P1 |
| | CFS scheduler | ✅ | ⚠️ | P1 |
| | Real-time scheduling | ❌ | ❌ | P3 |
| **5.4 Context Switch** | context_switch | ✅ | ✅ | P0 |
| | General register save | ✅ | ✅ | P0 |
| | Floating-point register save | ❌ | ❌ | P2 |
| **5.5 User Mode Support** | U-mode switch | ✅ | ✅ | P0 |
| | User stack setup | ✅ | ✅ | P0 |
| | User program loading | ✅ | ✅ | P0 |
| **5.6 Signal Handling** | SignalStruct | ✅ | ✅ | P0 |
| | SigAction | ✅ | ✅ | P0 |
| | Signal mask | ✅ | ✅ | P0 |
| | SIGKILL/SIGSTOP | ✅ | ✅ | P0 |
| | Signal handler | ✅ | ⚠️ | P1 |
| | Signal queue | ✅ | ⚠️ | P1 |
| | Real-time signal | ⚠️ | ❌ | P2 |

### 6. Interrupts and Timers

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **6.1 PLIC** | PLIC initialization | ✅ | ✅ | P0 |
| | Interrupt priority/enable | ✅ | ✅ | P0 |
| | Claim/Complete | ✅ | ✅ | P0 |
| **6.2 External Interrupts** | UART interrupt | ✅ | ✅ | P0 |
| | VirtIO interrupts | ✅ | ✅ | P0 |
| | Interrupt sharing | ❌ | ❌ | P2 |
| **6.3 Timer Interrupt** | SBI TIMER | ✅ | ✅ | P0 |
| | Periodic interrupt | ✅ | ✅ | P0 |
| | High-precision timer | ❌ | ❌ | P2 |
| **6.4 IPI** | SGI send | ✅ | ✅ | P0 |
| | Reschedule IPI | ✅ | ✅ | P0 |
| | IPI handling | ✅ | ✅ | P0 |

### 7. SMP Multicore

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **7.1 Multicore Boot** | SBI HSM | ✅ | ✅ | P0 |
| | Secondary core boot | ✅ | ✅ | P0 |
| | Hot plug CPU | ❌ | ❌ | P3 |
| **7.2 Per-CPU Data** | Per-CPU stack | ✅ | ✅ | P0 |
| | Per-CPU run queue | ✅ | ✅ | P0 |
| | Per-CPU variables | ❌ | ❌ | P1 |
| **7.3 Synchronization** | spin::Mutex | ✅ | ✅ | P0 |
| | RwLock | ✅ | ✅ | P1 |
| | SeqLock | ❌ | ❌ | P2 |

### 8. Synchronization Primitives

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **8.1 Semaphore** | Semaphore, down/up | ✅ | ✅ | P0 |
| **8.2 Condvar** | ConditionVariable | ✅ | ✅ | P0 |
| | wait/signal/broadcast | ✅ | ✅ | P0 |
| | wait_timeout | ❌ | ❌ | P1 |
| **8.3 Mutex** | Mutex, MutexGuard | ✅ | ✅ | P0 |
| | Deadlock detection | ❌ | ❌ | P3 |
| **8.4 Futex** | Futex wait/wake | ✅ | ⚠️ | P1 |

### 9. File System

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **9.1 VFS Framework** | file_open/close | ✅ | ✅ | P0 |
| | Path resolution | ✅ | ✅ | P0 |
| | Symbolic link resolution | ✅ | ✅ | P0 |
| | `.` and `..` handling | ✅ | ✅ | P0 |
| **9.2 File Descriptor** | FdTable | ✅ | ✅ | P0 |
| | alloc_fd/install_fd | ✅ | ✅ | P0 |
| | fd reuse | ✅ | ✅ | P0 |
| **9.3 RootFS** | Memory file system | ✅ | ✅ | P0 |
| | File/directory operations | ✅ | ✅ | P0 |
| | Permission management | ❌ | ❌ | P1 |
| **9.4 ProcFS** | meminfo/cpuinfo/version | ✅ | ✅ | P1 |
| | uptime/cmdline | ✅ | ✅ | P1 |
| | /proc/self | ✅ | ✅ | P1 |
| **9.5 DevFS** | devfs module | ✅ | ✅ | P1 |
| | Device registry | ✅ | ✅ | P1 |
| | /dev/input nodes | ✅ | ✅ | P1 |
| **9.6 Dentry/Inode Cache** | Dentry structure | ✅ | ✅ | P0 |
| | icache/dcache | ✅ | ✅ | P0 |
| | LRU eviction | ✅ | ✅ | P0 |
| **9.7 Superblock** | SuperBlock | ✅ | ✅ | P0 |
| | VFS mount | ⚠️ | ⚠️ | P1 |
| | VFS unmount | ❌ | ❌ | P1 |

### 10. ELF Loader

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **10.1 ELF Parsing** | ELF header parsing | ✅ | ✅ | P0 |
| | Program header parsing | ✅ | ✅ | P0 |
| | Section header parsing | ✅ | ✅ | P0 |
| | Dynamic linking | ❌ | ❌ | P2 |
| **10.2 User Address Space** | Page table creation | ✅ | ✅ | P0 |
| | PT_LOAD mapping | ✅ | ✅ | P0 |
| | User stack allocation | ✅ | ✅ | P0 |
| | ASLR | ❌ | ❌ | P2 |
| **10.3 Program Execution** | Entry point validation | ✅ | ✅ | P0 |
| | ELF loading | ✅ | ✅ | P0 |

### 11. Block Device Driver

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **11.1 VirtIO Framework** | VirtIO device detection | ✅ | ✅ | P0 |
| | VirtQueue | ✅ | ✅ | P0 |
| | Modern VirtIO PCI | ✅ | ✅ | P0 |
| **11.2 VirtIO-blk** | Read/write operations | ✅ | ✅ | P0 |
| | Multi-queue support | ❌ | ❌ | P2 |
| **11.3 Buffer I/O** | BufferHead, Block cache | ✅ | ✅ | P0 |
| | bread/brelse | ✅ | ✅ | P0 |
| **11.4 Block Device Framework** | GenDisk, Request queue | ✅ | ✅ | P0 |
| | Request scheduling | ❌ | ❌ | P2 |

### 12. ext4 File System

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **12.1 ext4 Basics** | Superblock parsing | ✅ | ✅ | P0 |
| | Block group descriptor | ✅ | ✅ | P0 |
| | Inode structure | ✅ | ✅ | P0 |
| | ext4 mount | ✅ | ✅ | P0 |
| **12.2 ext4 Allocator** | BlockAllocator | ✅ | ✅ | P0 |
| | InodeAllocator | ✅ | ✅ | P0 |
| | Bitmap management | ❌ | ❌ | P1 |
| **12.3 ext4 Operations** | Directory parsing | ✅ | ✅ | P0 |
| | File lookup/read | ✅ | ✅ | P0 |
| | File write | ✅ | ⚠️ | P1 |
| | File seek | ✅ | ✅ | P0 |
| | Directory create/delete | ❌ | ❌ | P1 |
| | Symbolic link | ✅ | ✅ | P2 |
| **12.4 Extent Tree** | Extent tree | ✅ | ✅ | P1 |
| | Indirect blocks | ✅ | ✅ | P0 |
| **12.5 Journaling** | Journaling | ❌ | ❌ | P2 |
| | fsync | ✅ | ⚠️ | P2 |

### 13. Process State Extension

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **13.1 TaskState** | Running/Interruptible/Uninterruptible | ✅ | ✅ | P0 |
| | Zombie/Stopped | ⚠️ | ⚠️ | P1 |
| **13.2 Sleep Wake** | set_state, wake_up | ✅ | ✅ | P0 |
| | sleep_on, wait_queue | ⚠️ | ⚠️ | P1 |
| **13.3 Time Management** | jiffies counter | ✅ | ✅ | P0 |
| | need_resched flag | ⚠️ | ⚠️ | P1 |

### 14. Network Protocol Stack

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **14.1 Socket Layer** | socket/bind/listen | ✅ | ✅ | P1 |
| | accept | ⚠️ | ⚠️ | P1 |
| | connect | ✅ | ✅ | P1 |
| | sendto/recvfrom | ⚠️ | ⚠️ | P1 |
| **14.2 TCP Protocol** | TCP connection | ✅ | ✅ | P1 |
| | Three-way handshake | ✅ | ✅ | P1 |
| | Four-way close | ⚠️ | ⚠️ | P1 |
| | TCP state machine | ✅ | ✅ | P1 |
| | Retransmission mechanism | ✅ | ⚠️ | P2 |
| | Sliding window | ⚠️ | ⚠️ | P2 |
| | Congestion control | ❌ | ❌ | P2 |
| **14.3 UDP Protocol** | UDP datagram | ✅ | ✅ | P1 |
| | Checksum | ✅ | ✅ | P1 |
| **14.4 IP Layer** | IPv4 | ✅ | ✅ | P1 |
| | Routing table | ✅ | ✅ | P1 |
| | ARP | ✅ | ✅ | P2 |
| | ICMP | ❌ | ❌ | P2 |
| **14.5 NIC Driver** | VirtIO-net | ✅ | ✅ | P1 |
| | Packet TX/RX | ✅ | ✅ | P1 |
| **14.6 Protocol Stack** | SkBuff | ✅ | ✅ | P2 |
| | Protocol layering | ✅ | ✅ | P2 |

### 15. Graphics and Input

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **15.1 Graphics Driver** | framebuffer | ✅ | ✅ | P2 |
| | fbdev | ✅ | ✅ | P2 |
| | VirtIO-GPU | ✅ | ⚠️ | P2 |
| **15.2 Input Devices** | evdev | ✅ | ✅ | P2 |
| | PS/2 keyboard/mouse | ✅ | ⚠️ | P2 |
| | VirtIO input | ✅ | ⚠️ | P2 |
| **15.3 GUI Applications** | rux_gui library | ✅ | ⚠️ | P2 |
| | desktop | ✅ | ⚠️ | P2 |
| | calculator/clock | ✅ | ⚠️ | P2 |

### 16. Unit Testing

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **16.1 Test Framework** | unit-test feature | ✅ | ✅ | P0 |
| | Test cases (51 files) | ✅ | ✅ | P0 |
| **16.2 Data Structure Tests** | ListHead/Path/FileFlags | ✅ | ✅ | P0 |
| **16.3 Memory Tests** | heap/page allocator | ✅ | ✅ | P0 |
| | COW test | ✅ | ✅ | P0 |
| **16.4 Process Tests** | scheduler/signal | ✅ | ⚠️ | P0 |
| | fork/execve/wait4 | ✅ | ⚠️ | P0 |
| **16.5 File System Tests** | file_open/fdtable | ✅ | ✅ | P0 |
| | dcache/icache | ✅ | ✅ | P0 |
| **16.6 Device Tests** | virtio_queue | ✅ | ✅ | P0 |
| | ext4 tests | ✅ | ⚠️ | P0 |
| **16.7 Integration Tests** | System boot, Multicore | ✅ | ✅ | P0 |
| **16.8 mini-ltp Tests** | All 24 tests | ✅ | ✅ | P1 |

### 17. Build and Development Tools

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **17.1 Build System** | Cargo workspace | ✅ | ✅ | P0 |
| | Makefile | ✅ | ✅ | P0 |
| **17.2 Configuration** | Kernel.toml | ✅ | ✅ | P0 |
| | menuconfig | ✅ | ✅ | P0 |
| **17.3 Test Scripts** | test/run.sh | ✅ | ✅ | P0 |
| **17.4 Documentation** | README, guides | ✅ | ✅ | P0 |

---

## Feature Statistics

### Implementation Status
- **Implemented (✅)**: ~320 features
- **Partial (⚠️)**: ~70 features
- **Not Implemented (❌)**: ~200 features

### Test Status
- **Tested (✅)**: ~280 features
- **Partial Test (⚠️)**: ~60 features
- **Not Tested (❌)**: ~250 features

### Priority Distribution
- **P0 (Core)**: ~180 items
- **P1 (Important)**: ~130 items
- **P2 (Enhanced)**: ~150 items
- **P3 (Advanced)**: ~130 items

---

## Development Phase Planning

### Phase 1-5: Basic Kernel ✅
Boot, exception handling, basic syscalls, memory management, process basics

### Phase 6-10: Core Features ✅
Interrupts, SMP, synchronization, filesystem basics, ELF loader

### Phase 11-15: Advanced Features ✅
User mode, process refinement, signals, pipes/IPC, unit testing

### Phase 16-17: High-level Features ✅
Preemptive scheduling, block devices, ext4 filesystem

### Phase 18: Network Protocol Stack ✅
SkBuff, Ethernet, ARP, IPv4, UDP, TCP, VirtIO-net, Socket syscalls

### Phase 18.5-18.6: Memory Refactoring ✅
Platform-independent pagemap, VirtIO probe refactoring

### Phase 19: Modern VirtIO PCI & Shell ✅
Modern VirtIO 1.0+ PCI driver, ext4 extent tree, shell running

### Phase 20: Multi Shell Support ✅
cmdline parsing fix, musl libc toolchain, multi shell support

### Phase 21: Boot Output Refactoring ✅
Boot log beautification, ASCII art logo, kernel version info

### Phase 22: procfs, Symbolic Links, toybox ✅
Full procfs, ext4 symbolic links, TLS fix, toybox integration

### Phase 23: CFS Scheduler, COW, Graphics ✅
CFS refinement, Copy-on-Write, graphics driver, input devices, GUI applications

### Phase 24: devfs, mini-ltp, Code Cleanup ✅
devfs filesystem, 24 mini-ltp tests, VFS path resolution, code cleanup

### Phase 25: TCP Reliability and Signal Refinement ✅
TCP retransmission mechanism, RTO calculation, signal mechanism refinement

---

## High Priority Features To Implement (P1)

### Memory Management
- [ ] Guard page support
- [ ] Page reclamation

### File System
- [ ] ext4 directory create/delete
- [ ] File truncate/extend
- [ ] Permission management (uid/gid)
- [ ] VFS unmount

### IPC
- [ ] Complete epoll implementation
- [ ] Message queue (sys_msgget/msgsnd/msgrcv)
- [ ] Shared memory (sys_shmget/shmat/shmdt)

### Network
- [ ] TCP congestion control
- [ ] ICMP support
- [ ] IP fragmentation

---

## Medium Priority Features (P2)

### System Calls
- [ ] sys_prctl
- [ ] POSIX timers
- [ ] High-precision timer

### Memory
- [ ] Slab allocator tests
- [ ] OOM killer

### Synchronization
- [ ] SeqLock
- [ ] wait_timeout for condvar

---

## Low Priority Features (P3)

- Virtualization (KVM, containers)
- Security (capability, selinux)
- Power management (frequency scaling, hibernate)
- Multimedia (audio, video)
- Real-time scheduling

---

**Document Version**: v5.0
**Last Updated**: 2026-03-12
**Maintainer**: Rux Development Team
