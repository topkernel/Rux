# Rux Development Roadmap and Feature List

## Project Overview

**Current Status**: Phase 29 - ext4 File Write & User-Kernel Safety

**Last Updated**: 2026-03-28 (2)

**Supported Architecture**: RISC-V 64-bit (RV64GC) - Only supported architecture

**Code Statistics**:
- **Source Files**: 222 (218 Rust + 3 Assembly + 1 Linker Script)
- **Total Lines of Code**: ~74,800
- **Kernel Unit Tests**: 53 test files
- **mini-lTP Tests**: 25 kernel compatibility tests

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
| | Memory layout (VMA/LMA) | ✅ | ✅ | P0 |
| | S-mode entry | ✅ | ✅ | P0 |
| **1.2 Boot Code** | Assembly boot entry | ✅ | ✅ | P0 |
| | MMU trampoline (Linux-style) | ✅ | ✅ | P0 |
| | VMA/LMA linker script | ✅ | ✅ | P0 |
| | Stack setup (per-CPU) | ✅ | ✅ | P0 |
| | BSS segment zeroing | ✅ | ✅ | P0 |
| | Rust code jump | ✅ | ✅ | P0 |
| | Data segment initialization | ✅ | ✅ | P0 |
| | medany code model | ✅ | ✅ | P0 |
| **1.3 UART Driver** | ns16550a driver | ✅ | ✅ | P0 |
| | Character output (putc) | ✅ | ✅ | P0 |
| | Character input (getc) | ✅ | ✅ | P0 |
| | println! macro | ✅ | ✅ | P0 |
| | Baud rate configuration | ⚠️ | ⚠️ | P1 |
| **1.4 CSR Management** | sstatus, sepc, stval, stvec, scause, satp, sie/sip | ✅ | ✅ | P0 |
| | sscratch/tp protocol (user/kernel detect) | ✅ | ✅ | P0 |
| | stimecmp (SSTC extension) | ✅ | ✅ | P0 |
| **1.5 Early Print** | Boot print, Error output | ✅ | ✅ | P0 |
| | Debug output | ⚠️ | ⚠️ | P2 |

### 2. Exception Handling

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **2.1 Exception Vector Table** | Direct mode | ✅ | ✅ | P0 |
| | Vectored mode | ❌ | ❌ | P2 |
| **2.2 Trap Handling** | PtRegs save/restore (Linux-style) | ✅ | ✅ | P0 |
| | PtRegs at kernel stack top | ✅ | ✅ | P0 |
| | User/kernel stack switch | ✅ | ✅ | P0 |
| | CSR register save | ✅ | ✅ | P0 |
| | sret return | ✅ | ✅ | P0 |
| **2.3 Exception Types** | System call (ecall) | ✅ | ✅ | P0 |
| | Breakpoint | ✅ | ✅ | P0 |
| | Page fault (load/store/insn) | ✅ | ✅ | P0 |
| | Illegal instruction | ✅ | ✅ | P0 |
| | Alignment error | ⚠️ | ⚠️ | P1 |
| | Floating-point save/restore | ✅ | ✅ | P0 |
| | Floating-point exception | ❌ | ❌ | P2 |
| **2.4 Trap Return** | ret_from_exception | ✅ | ✅ | P0 |
| | ret_from_fork_user | ✅ | ✅ | P0 |
| | ret_from_fork_kernel | ✅ | ✅ | P0 |
| | Signal frame delivery | ✅ | ⚠️ | P0 |

### 3. System Calls

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **3.1 System Call Framework** | System call dispatch (67+ syscalls) | ✅ | ✅ | P0 |
| | PtRegs as syscall frame | ✅ | ✅ | P0 |
| | Return value handling | ✅ | ✅ | P0 |
| | Parameter validation | ⚠️ | ⚠️ | P1 |
| **3.2 File System Syscalls** | sys_openat | ✅ | ✅ | P0 |
| | sys_close | ✅ | ✅ | P0 |
| | sys_read/write | ✅ | ✅ | P0 |
| | sys_writev | ✅ | ✅ | P0 |
| | sys_lseek | ✅ | ✅ | P0 |
| | sys_getdents64 | ✅ | ✅ | P0 |
| | sys_fstat/fstatat | ✅ | ✅ | P1 |
| | sys_statx | ❌ | ❌ | P2 |
| | sys_ioctl | ✅ | ⚠️ | P2 |
| | sys_fcntl | ✅ | ⚠️ | P1 |
| | sys_fsync | ✅ | ✅ | P2 |
| | sys_readlinkat | ✅ | ✅ | P1 |
| | sys_flock | ✅ | ⚠️ | P2 |
| | sys_mkdirat/rmdir | ✅ | ✅ | P1 |
| | sys_unlinkat | ✅ | ✅ | P1 |
| | sys_faccessat | ✅ | ✅ | P1 |
| | sys_chdir | ✅ | ✅ | P0 |
| | sys_getcwd | ✅ | ✅ | P0 |
| | sys_umask | ✅ | ✅ | P1 |
| | sys_futimesat | ✅ | ✅ | P2 |
| **3.3 Process Management** | sys_fork/vfork | ✅ | ✅ | P0 |
| | sys_clone (CLONE_*) | ✅ | ✅ | P0 |
| | sys_execve | ✅ | ✅ | P0 |
| | sys_wait4 | ✅ | ✅ | P0 |
| | sys_waitid | ❌ | ❌ | P1 |
| | sys_exit/exit_group | ✅ | ✅ | P0 |
| | sys_getpid/getppid | ✅ | ✅ | P0 |
| | sys_gettid | ✅ | ✅ | P1 |
| | sys_set_tid_address | ✅ | ✅ | P2 |
| | sys_kill/tkill | ✅ | ✅ | P0 |
| | sys_getpriority/setpriority | ✅ | ⚠️ | P1 |
| | sys_set_robust_list | ✅ | ✅ | P2 |
| | sys_sched_yield | ✅ | ✅ | P0 |
| | sys_uname | ✅ | ✅ | P1 |
| | sys_prlimit64 | ✅ | ✅ | P2 |
| | sys_getuid/geteuid/getgid/getegid | ✅ | ✅ | P1 |
| **3.4 Signal Syscalls** | sys_rt_sigaction | ✅ | ✅ | P0 |
| | sys_rt_sigreturn | ✅ | ⚠️ | P1 |
| | sys_rt_sigprocmask | ✅ | ✅ | P1 |
| | sys_sigpending | ✅ | ⚠️ | P1 |
| | sys_sigaltstack | ✅ | ⚠️ | P2 |
| **3.5 Memory Management** | sys_brk | ✅ | ✅ | P1 |
| | sys_mmap/munmap | ✅ | ✅ | P1 |
| | sys_mprotect | ✅ | ✅ | P2 |
| | sys_mremap | ⚠️ | ⚠️ | P3 |
| | sys_madvise | ⚠️ | ⚠️ | P2 |
| | sys_mincore | ⚠️ | ⚠️ | P3 |
| | sys_msync | ⚠️ | ⚠️ | P2 |
| | sys_mlock/munlock | ⚠️ | ⚠️ | P3 |
| **3.6 IPC Syscalls** | sys_pipe/pipe2 | ✅ | ✅ | P0 |
| | sys_dup/dup2/dup3 | ✅ | ⚠️ | P1 |
| | sys_select/poll | ⚠️ | ⚠️ | P1 |
| | sys_epoll_create/ctl/wait | ⚠️ | ⚠️ | P1 |
| | sys_eventfd2 | ✅ | ⚠️ | P2 |
| **3.7 Socket Syscalls** | sys_socket | ✅ | ✅ | P1 |
| | sys_bind/listen | ✅ | ✅ | P1 |
| | sys_accept | ⚠️ | ⚠️ | P1 |
| | sys_connect | ✅ | ✅ | P1 |
| | sys_sendto/recvfrom | ⚠️ | ⚠️ | P1 |
| **3.8 Other Syscalls** | sys_getrandom | ✅ | ✅ | P2 |
| | sys_futex | ✅ | ⚠️ | P1 |
| | sys_nanosleep | ✅ | ✅ | P1 |
| | sys_clock_gettime/getres | ✅ | ✅ | P1 |
| | sys_gettimeofday | ✅ | ✅ | P1 |

### 4. Memory Management

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **4.1 Physical Memory** | Page descriptor (Page struct) | ✅ | ✅ | P0 |
| | FrameAllocator (zone-based) | ✅ | ✅ | P0 |
| | Physical memory detection | ✅ | ✅ | P0 |
| | Memblock (early allocator) | ✅ | ✅ | P0 |
| | Physical address validation | ✅ | ✅ | P0 |
| **4.2 Virtual Memory (Sv39)** | 3-level page table (PGD/PMD/PTE) | ✅ | ✅ | P0 |
| | PageTableEntry (R/W/X/U/G/D/A/COW/S) | ✅ | ✅ | P0 |
| | Linear mapping (PAGE_OFFSET) | ✅ | ✅ | P0 |
| | Kernel mapping (KERNEL_LINK_ADDR) | ✅ | ✅ | P0 |
| | MMU enable (boot trampoline) | ✅ | ✅ | P0 |
| | Platform-independent interface | ✅ | ✅ | P0 |
| | Fixmap (UART, DTB) | ✅ | ✅ | P0 |
| | ASID management (9-bit, 512 max) | ✅ | ✅ | P0 |
| | TLB flush (all/per-ASID/per-page) | ✅ | ✅ | P0 |
| | Huge page support (PMD/PGD) | ✅ | ⚠️ | P3 |
| | Three-stage page table allocation | ✅ | ✅ | P0 |
| **4.3 Heap Memory** | Buddy allocator (MAX_ORDER=10) | ✅ | ✅ | P0 |
| | Zone allocator (DMA/DMA32/NORMAL/MOVABLE) | ✅ | ✅ | P0 |
| | Per-CPU pagesets (PCP) | ✅ | ✅ | P1 |
| | Slab allocator (10 size classes) | ✅ | ❌ | P2 |
| | Object cache (SlabCache) | ✅ | ❌ | P2 |
| **4.4 Page Descriptors** | vmemmap (virtual memory map) | ✅ | ✅ | P0 |
| | O(1) pfn_to_page | ✅ | ✅ | P0 |
| | Page refcount (get_page/put_page) | ✅ | ✅ | P0 |
| | Page flags | ✅ | ✅ | P0 |
| **4.5 User Memory** | User address space (mm_struct) | ✅ | ✅ | P1 |
| | VMA management (BTreeMap) | ✅ | ✅ | P1 |
| | mmap/munmap | ✅ | ✅ | P1 |
| | fork address space (COW) | ✅ | ✅ | P1 |
| | copy_kernel_mappings (VPN2 sharing) | ✅ | ✅ | P0 |
| | Demand paging (anonymous pages) | ✅ | ✅ | P1 |
| | On-demand stack expansion | ✅ | ✅ | P1 |
| | Guard page | ❌ | ❌ | P2 |
| **4.6 Copy-on-Write** | COW bit (PTE bit 8) | ✅ | ✅ | P1 |
| | fork COW (share + mark) | ✅ | ✅ | P1 |
| | COW fault handler (page copy) | ✅ | ✅ | P1 |
| | free_user_page_tables (put_page) | ✅ | ✅ | P1 |
| **4.7 Reverse Mapping** | AnonVma / AnonVmaChain | ✅ | ⚠️ | P2 |
| **4.8 Memory Reclamation** | Page reclamation | ❌ | ❌ | P2 |
| | kswapd | ❌ | ❌ | P2 |
| | OOM killer | ❌ | ❌ | P3 |
| **4.9 Memory Info** | /proc/meminfo | ✅ | ✅ | P1 |
| | Page statistics | ✅ | ✅ | P1 |

### 5. Process Management

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **5.1 Process Control Block** | Task structure (Linux-style) | ✅ | ✅ | P0 |
| | ThreadStruct (arch-specific) | ✅ | ✅ | P0 |
| | Process state enum (bitmap) | ✅ | ✅ | P0 |
| | PID management | ✅ | ✅ | P0 |
| | PID namespace | ❌ | ❌ | P2 |
| | Kernel stack cache (64 slots) | ✅ | ✅ | P1 |
| **5.2 Process Tree** | Parent-child relationship | ✅ | ✅ | P0 |
| | Sibling relationship | ✅ | ✅ | P0 |
| | ListHead doubly linked list | ✅ | ✅ | P0 |
| | init process (PID 1) | ✅ | ✅ | P0 |
| **5.3 Process Scheduling** | Per-CPU run queue | ✅ | ✅ | P0 |
| | Round Robin algorithm | ✅ | ✅ | P0 |
| | CFS scheduler (disabled by default) | ✅ | ⚠️ | P1 |
| | Deadline scheduler (EDF + CBS) | ✅ | ⚠️ | P2 |
| | Real-time FIFO/RR scheduler | ✅ | ⚠️ | P2 |
| | Load balancing | ✅ | ⚠️ | P1 |
| | CPU idle loop (WFI) | ✅ | ✅ | P0 |
| | Real-time scheduling (full) | ❌ | ❌ | P3 |
| **5.4 Context Switch** | context_switch (switch_mm + __switch_to) | ✅ | ✅ | P0 |
| | General register save (callee-saved) | ✅ | ✅ | P0 |
| | Floating-point register save | ✅ | ✅ | P0 |
| | tp register update | ✅ | ✅ | P0 |
| | schedule_tail (ret_from_fork) | ✅ | ✅ | P0 |
| **5.5 User Mode Support** | U-mode switch | ✅ | ✅ | P0 |
| | User stack setup | ✅ | ✅ | P0 |
| | User program loading (ELF) | ✅ | ✅ | P0 |
| | Auxiliary vector (15 AT_* entries) | ✅ | ✅ | P0 |
| **5.6 Clone Flags** | CLONE_VM | ✅ | ✅ | P1 |
| | CLONE_FILES | ✅ | ✅ | P1 |
| | CLONE_FS | ✅ | ✅ | P1 |
| | CLONE_SIGHAND | ✅ | ✅ | P1 |
| | CLONE_THREAD | ✅ | ✅ | P1 |
| | CLONE_SETTLS | ✅ | ✅ | P1 |
| | CLONE_CHILD_CLEARTID | ✅ | ✅ | P1 |
| | clear_child_tid (musl pthread) | ✅ | ✅ | P1 |
| | robust_list (robust mutex) | ✅ | ✅ | P1 |
| **5.7 Signal Handling** | SignalStruct (per-process) | ✅ | ✅ | P0 |
| | SigAction (64 signal slots) | ✅ | ✅ | P0 |
| | Signal mask (SigSet) | ✅ | ✅ | P0 |
| | SIGKILL/SIGSTOP | ✅ | ✅ | P0 |
| | Signal handler (user-mode frame) | ✅ | ⚠️ | P1 |
| | Signal frame (SigContext + UContext) | ✅ | ✅ | P1 |
| | rt_sigreturn (trampoline via ecall) | ✅ | ⚠️ | P1 |
| | Real-time signal queue (lock-free) | ✅ | ⚠️ | P2 |
| | sigaltstack (SS_ONSTACK/SS_DISABLE) | ✅ | ⚠️ | P2 |
| **5.8 Process Exit** | do_exit (exit_mm/files) | ✅ | ✅ | P0 |
| | SIGCHLD to parent | ✅ | ✅ | P0 |
| | Zombie reaping (release_task) | ✅ | ✅ | P0 |
| | do_wait / do_wait_nonblock | ✅ | ✅ | P0 |
| **5.9 Per-Process State** | FsStruct (root/cwd) | ✅ | ✅ | P0 |
| | FdTable (Arc-shared) | ✅ | ✅ | P0 |
| | brk (program break) | ✅ | ✅ | P1 |

### 6. Interrupts and Timers

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **6.1 PLIC** | PLIC initialization | ✅ | ✅ | P0 |
| | Interrupt priority/enable | ✅ | ✅ | P0 |
| | Claim/Complete | ✅ | ✅ | P0 |
| **6.2 External Interrupts** | UART interrupt | ✅ | ✅ | P0 |
| | VirtIO interrupts (MMIO + PCI) | ✅ | ✅ | P0 |
| | Interrupt sharing | ❌ | ❌ | P2 |
| **6.3 Timer Interrupt** | SBI TIMER / SSTC (stimecmp) | ✅ | ✅ | P0 |
| | Periodic interrupt | ✅ | ✅ | P0 |
| | High-precision timer | ❌ | ❌ | P2 |
| **6.4 IPI** | SBI IPI send | ✅ | ✅ | P0 |
| | Reschedule IPI | ✅ | ✅ | P0 |
| | IPI handling | ✅ | ✅ | P0 |

### 7. SMP Multicore

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **7.1 Multicore Boot** | SBI HSM | ✅ | ✅ | P0 |
| | Secondary core boot | ✅ | ✅ | P0 |
| | Per-CPU interrupt stacks | ✅ | ✅ | P0 |
| | Hot plug CPU | ❌ | ❌ | P3 |
| **7.2 Per-CPU Data** | Per-CPU stack | ✅ | ✅ | P0 |
| | Per-CPU run queue | ✅ | ✅ | P0 |
| | Per-CPU idle task | ✅ | ✅ | P0 |
| | Per-CPU pagesets (PCP) | ✅ | ✅ | P1 |
| | Per-CPU variables | ⚠️ | ⚠️ | P1 |
| **7.3 Synchronization** | spin::Mutex | ✅ | ✅ | P0 |
| | RwLock | ✅ | ✅ | P1 |
| | Kernel big lock | ✅ | ✅ | P0 |
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
| | PI futex (LOCK_PI/UNLOCK_PI) | ✅ | ⚠️ | P2 |
| | Futex requeue (REQUEUE/CMP_REQUEUE) | ✅ | ⚠️ | P2 |

### 9. File System

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **9.1 VFS Framework** | file_open/close | ✅ | ✅ | P0 |
| | Path resolution | ✅ | ✅ | P0 |
| | Symbolic link resolution | ✅ | ✅ | P0 |
| | `.` and `..` handling | ✅ | ✅ | P0 |
| **9.2 File Descriptor** | FdTable (Arc-shared) | ✅ | ✅ | P0 |
| | alloc_fd/install_fd | ✅ | ✅ | P0 |
| | fd reuse | ✅ | ✅ | P0 |
| **9.3 RootFS** | Memory file system | ✅ | ✅ | P0 |
| | File/directory operations | ✅ | ✅ | P0 |
| | Permission management | ❌ | ❌ | P1 |
| **9.4 ProcFS** | meminfo/cpuinfo/version | ✅ | ✅ | P1 |
| | uptime/cmdline/loadavg | ✅ | ✅ | P1 |
| | /proc/self | ✅ | ✅ | P1 |
| | /proc/pid/ (status,stat,cmdline,exe,cwd,environ,fd) | ✅ | ✅ | P1 |
| | /proc/mounts | ✅ | ✅ | P1 |
| | /proc/interrupts | ✅ | ✅ | P1 |
| **9.5 DevFS** | devfs module | ✅ | ✅ | P1 |
| | Device registry | ✅ | ✅ | P1 |
| | /dev/input nodes | ✅ | ✅ | P1 |
| **9.6 Dentry/Inode Cache** | Dentry structure | ✅ | ✅ | P0 |
| | icache/dcache | ✅ | ✅ | P0 |
| | LRU eviction | ✅ | ✅ | P0 |
| **9.7 Superblock** | SuperBlock | ✅ | ✅ | P0 |
| | VFS mount | ⚠️ | ⚠️ | P1 |
| | VFS unmount | ❌ | ❌ | P1 |
| **9.8 Pipe** | create_pipe | ✅ | ✅ | P0 |
| | Circular buffer | ✅ | ✅ | P0 |
| | Blocking read/write | ✅ | ✅ | P0 |
| **9.9 JBD2 Journaling** | Journal module | ✅ | ⚠️ | P2 |
| | Transaction management | ✅ | ⚠️ | P2 |
| | Commit/recovery/checkpoint | ✅ | ⚠️ | P2 |
| | Revoke records | ✅ | ⚠️ | P2 |

### 10. ELF Loader

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **10.1 ELF Parsing** | ELF header parsing | ✅ | ✅ | P0 |
| | Program header parsing | ✅ | ✅ | P0 |
| | Section header parsing | ✅ | ✅ | P0 |
| | Dynamic linking | ❌ | ❌ | P2 |
| **10.2 User Address Space** | Page table creation | ✅ | ✅ | P0 |
| | PT_LOAD mapping with VMA | ✅ | ✅ | P0 |
| | User stack allocation | ✅ | ✅ | P0 |
| | BSS zeroing | ✅ | ✅ | P0 |
| | Auxiliary vector (15 entries) | ✅ | ✅ | P0 |
| | ASLR (KASLR offset field) | ❌ | ❌ | P2 |
| **10.3 Program Execution** | Entry point validation | ✅ | ✅ | P0 |
| | ELF loading (execve) | ✅ | ✅ | P0 |
| | Multiple source (PCI blk, MMIO blk, RootFS) | ✅ | ✅ | P0 |

### 11. Block Device Driver

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **11.1 VirtIO Framework** | VirtIO device detection | ✅ | ✅ | P0 |
| | VirtQueue | ✅ | ✅ | P0 |
| | Modern VirtIO PCI | ✅ | ✅ | P0 |
| | VirtIO MMIO | ✅ | ✅ | P0 |
| **11.2 VirtIO-blk** | Read/write operations (MMIO + PCI) | ✅ | ✅ | P0 |
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
| | File write (persist across reboot) | ✅ | ✅ | P1 |
| | File seek | ✅ | ✅ | P0 |
| | Directory create (mkdirat) | ✅ | ✅ | P1 |
| | Directory delete (rmdir) | ✅ | ✅ | P1 |
| | File delete (unlinkat) | ✅ | ✅ | P1 |
| | Symbolic link | ✅ | ✅ | P2 |
| | File truncate (O_TRUNC) | ✅ | ✅ | P1 |
| **12.4 Extent Tree** | Extent tree | ✅ | ✅ | P1 |
| | Indirect blocks | ✅ | ✅ | P0 |
| **12.5 Journaling** | JBD2 integration | ✅ | ⚠️ | P2 |
| | Transaction commit | ✅ | ⚠️ | P2 |
| | Recovery | ✅ | ⚠️ | P2 |
| | fsync | ✅ | ⚠️ | P2 |

### 13. Network Protocol Stack

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
| | Loopback device | ✅ | ✅ | P2 |
| **14.6 Protocol Stack** | SkBuff | ✅ | ✅ | P2 |
| | Protocol layering | ✅ | ✅ | P2 |

### 14. Graphics and Input

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **14.1 Graphics Driver** | framebuffer | ✅ | ✅ | P2 |
| | fbdev | ✅ | ✅ | P2 |
| | VirtIO-GPU | ✅ | ⚠️ | P2 |
| **14.2 Input Devices** | evdev | ✅ | ✅ | P2 |
| | PS/2 keyboard/mouse | ✅ | ⚠️ | P2 |
| | VirtIO input | ✅ | ⚠️ | P2 |
| **14.3 GUI Applications** | rux_gui library | ✅ | ⚠️ | P2 |
| | desktop | ✅ | ⚠️ | P2 |
| | calculator/clock | ✅ | ⚠️ | P2 |

### 15. Unit Testing

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **15.1 Test Framework** | unit-test feature | ✅ | ✅ | P0 |
| | Test cases (53 files) | ✅ | ✅ | P0 |
| **15.2 Data Structure Tests** | ListHead/Path/FileFlags | ✅ | ✅ | P0 |
| **15.3 Memory Tests** | heap/page allocator | ✅ | ✅ | P0 |
| | COW test | ✅ | ✅ | P0 |
| **15.4 Process Tests** | scheduler/signal | ✅ | ⚠️ | P0 |
| | fork/execve/wait4 | ✅ | ⚠️ | P0 |
| **15.5 File System Tests** | file_open/fdtable | ✅ | ✅ | P0 |
| | dcache/icache | ✅ | ✅ | P0 |
| **15.6 Device Tests** | virtio_queue | ✅ | ✅ | P0 |
| | ext4 tests | ✅ | ⚠️ | P0 |
| **15.7 Integration Tests** | System boot, Multicore | ✅ | ✅ | P0 |
| **15.8 mini-ltp Tests** | 25 kernel compatibility tests | ✅ | ✅ | P1 |

### 16. Build and Development Tools

| Feature | Sub-feature | Implementation | Test | Priority |
|---------|-------------|----------------|------|----------|
| **16.1 Build System** | Cargo workspace | ✅ | ✅ | P0 |
| | Makefile | ✅ | ✅ | P0 |
| **16.2 Configuration** | Kernel.toml | ✅ | ✅ | P0 |
| | menuconfig | ✅ | ✅ | P0 |
| **16.3 Test Scripts** | test/run.sh | ✅ | ✅ | P0 |
| **16.4 Documentation** | README, guides | ✅ | ✅ | P0 |
| | Architecture docs (boot/memory/structure) | ✅ | ✅ | P0 |

---

## Feature Statistics

### Implementation Status
- **Implemented (✅)**: ~360 features
- **Partial (⚠️)**: ~75 features
- **Not Implemented (❌)**: ~140 features

### Test Status
- **Tested (✅)**: ~300 features
- **Partial Test (⚠️)**: ~65 features
- **Not Tested (❌)**: ~210 features

### Code Quality
- **TODO/FIXME markers**: 94 across 42 files

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
devfs filesystem, 25 mini-ltp tests, VFS path resolution, code cleanup

### Phase 25: TCP Reliability and Signal Refinement ✅
TCP retransmission mechanism, RTO calculation, signal mechanism refinement

### Phase 26: Documentation Update ✅
Architecture documentation (boot, memory, structure), design philosophy refinement

### Phase 27: Linux-Style Memory Management Refactoring ✅
Zone allocator (DMA/DMA32/NORMAL/MOVABLE), vmemmap, per-CPU pagesets,
memblock early allocator, three-stage page table allocation, page descriptors
with refcount, demand paging, on-demand stack expansion, COW fault handler,
ASID management (9-bit, 512 max), copy_kernel_mappings (VPN2 sharing),
reverse mapping (AnonVma), huge page framework, fs_struct per-process

### Phase 28: Linux-Style Boot & Architecture Refactoring ✅
MMU trampoline in boot.S, VMA/LMA linker script, kernel linked at
KERNEL_LINK_ADDR (0xffffffff80000000), medany code model, PtRegs at
kernel stack top, FPU context save/restore in context switch,
sscratch/tp protocol, ret_from_fork paths, uaccess.S assembly,
JBD2 journaling layer (8 modules), sys_mkdirat/rmdir/unlinkat,
kernel big lock, enhanced procfs (/proc/pid/*, /proc/interrupts)

### Phase 29: ext4 File Write & User-Kernel Safety ✅
Fixed sys_read/sys_write to use copy_from_user/copy_to_user (kernel page
fault when accessing user memory directly), ext4 file write correctness
(i_blocks update, timestamps, O_APPEND, O_TRUNC block freeing), write_inode
read-modify-write to preserve on-disk fields, extent tree depth > 0 read path,
environment variable support through execve, toybox symlinks, shell PATH
search, printk with log levels and ring buffer, PCI VirtIO block write
(pre-configured queue pattern with retry, writes persist across reboot)

---

## High Priority Features To Implement (P1)

### Memory Management
- [ ] Guard page support
- [ ] Page reclamation (kswapd)
- [ ] Enable CFS scheduler by default
- [ ] Slab allocator tests

### File System
- [ ] Permission management (uid/gid)
- [ ] VFS unmount
- [ ] Bitmap allocator for ext4

### IPC
- [ ] Complete epoll implementation
- [ ] Message queue (sys_msgget/msgsnd/msgrcv)
- [ ] Shared memory (sys_shmget/shmat/shmdt)
- [ ] Complete select/poll implementation

### Network
- [ ] TCP congestion control
- [ ] ICMP support
- [ ] IP fragmentation
- [ ] Complete TCP four-way close

### Process
- [ ] PID reuse / PID hash table
- [ ] waitid syscall
- [ ] Complete user-mode signal handler invocation

---

## Medium Priority Features (P2)

### System Calls
- [ ] sys_prctl
- [ ] sys_statx
- [ ] POSIX timers
- [ ] High-precision timer
- [ ] sys_rename/renameat

### Memory
- [ ] OOM killer
- [ ] Memory compaction
- [ ] Huge page integration with page fault handler

### Synchronization
- [ ] SeqLock
- [ ] wait_timeout for condvar
- [ ] RCU mechanism

### Architecture
- [ ] Device tree (DTB) parser
- [ ] Vectored mode trap

---

## Low Priority Features (P3)

- Virtualization (KVM, containers)
- Security (capability, selinux)
- Power management (frequency scaling, hibernate)
- Multimedia (audio, video)
- CPU hot plug
- Real-time scheduling (full)
- ASLR / KASLR
- Dynamic linking (ld.so)

---

**Document Version**: v6.2
**Last Updated**: 2026-03-28
**Maintainer**: Rux Development Team
