# Rux Development Roadmap

## Project Overview

| | |
|---|---|
| **Architecture** | RISC-V 64-bit (RV64GC) |
| **Source Files** | 278 (274 Rust + 3 Assembly + 1 Linker Scripts) |
| **Code Lines** | ~102,400 |
| **Syscall Numbers** | 348 dispatched |
| **Unit Tests** | 825 cases across 58 test files |
| **Formal Verification** | 1099 cases across 98 modules |
| **Linux LTP** | 1,838 official tests |
| **Smoke Tests** | 15/15 passing |
| **Current Phase** | Phase 51 — Memory Compaction |

**Design Philosophy**: External interfaces 100% Linux ABI compatible. Internal implementation free to innovate.

---

## Module Completion

| Status | Modules |
|--------|---------|
| ✅ Complete (9) | Boot & Init · System Calls · Scheduler · Process Mgmt · Security · Diagnostics · Testing · Build & Tooling · Memory Mgmt |
| ⚠️ In Progress (9) | File System 92% · ELF Loader 92% · Interrupts 90% · Synchronization 90% · Block Device 85% · Network 85% · SMP 80% · Exception & Trap 71% · Graphics 70% |

---

## Feature Implementation Status

> ✅ Implemented · ⚠️ Partial · ❌ Not Implemented

| Module | Feature | Status | Feature | Status | Feature | Status |
|--------|---------|--------|---------|--------|---------|--------|
| **1. Boot & Init** ✅ | OpenSBI Integration | ✅ | Assembly Entry | ✅ | MMU Trampoline | ✅ |
| | VMA/LMA Linker Script | ✅ | medany Code Model | ✅ | Stack Setup | ✅ |
| | BSS Zeroing | ✅ | UART (ns16550a) | ✅ | UART Blocking Read | ✅ |
| | TTY ISIG | ✅ | CSR Management | ✅ | sscratch/tp Protocol | ✅ |
| | stimecmp (SSTC) | ✅ | Early Print | ✅ | Boot Page Table (8MB) | ✅ |
| **2. Exception & Trap** ⚠️ | Direct Mode | ✅ | Vectored Mode | ❌ | PtRegs (Linux-style) | ✅ |
| | User/Kernel Stack Switch | ✅ | CSR Save/Restore | ✅ | ecall | ✅ |
| | Page Fault | ✅ | Breakpoint | ✅ | Illegal Instruction | ✅ |
| | FPU Save/Restore | ✅ | FP Exception | ❌ | ret_from_exception | ✅ |
| | ret_from_fork_user | ✅ | ret_from_fork_kernel | ✅ | Signal Frame Delivery | ✅ |
| **3. System Calls** ✅ (348) | File System (93) | ✅ | Process (90) | ✅ | Time & Timer (39) | ✅ |
| | Memory (30) | ✅ | IPC (21) | ✅ | Network (18) | ✅ |
| | I/O (18) | ✅ | Scheduler (15) | ✅ | Misc (14) | ✅ |
| | Signal (9) | ✅ | Diagnostics (1) | ✅ | | |
| **4. Scheduler** ✅ | Scheduling Class | ✅ | CFS v1 (vruntime) | ✅ | Deadline (EDF+CBS) | ✅ |
| | RT FIFO / RR | ✅ | Idle Class | ✅ | Stop Task | ✅ |
| | Global Run Queue | ✅ | CPU Affinity | ✅ | Load Balancing | ✅ |
| | Scheduler Tick | ✅ | Cross-CPU IPI | ✅ | CPU Idle (WFI) | ✅ |
| | POSIX Real-time | ❌ | | | | |
| **7. Memory Mgmt** ⚠️ | Page Descriptor | ✅ | Frame Allocator | ✅ | Memblock | ✅ |
| | Sv39 Page Table | ✅ | PTE Flags | ✅ | Linear Mapping | ✅ |
| | Kernel Mapping | ✅ | MMU Enable | ✅ | Fixmap | ✅ |
| | ASID (9-bit) | ✅ | TLB Flush | ✅ | Huge Page (PMD) | ✅ |
| | Huge Page (PGD) | ✅ | Buddy (MAX_ORDER=10) | ✅ | Zone DMA | ✅ |
| | Zone DMA32 | ✅ | Zone NORMAL | ✅ | Zone MOVABLE | ✅ |
| | Per-CPU Pagesets | ✅ | Slab (10 classes) | ✅ | SlabCache | ✅ |
| | vmemmap | ✅ | pfn_to_page (O(1)) | ✅ | Page Refcount | ✅ |
| | Page Flags | ✅ | mm_struct | ✅ | VMA (BTreeMap) | ✅ |
| | mmap / munmap | ✅ | Fork Address Space | ✅ | copy_kernel_mappings | ✅ |
| | Demand Paging | ✅ | Stack Expansion | ✅ | Guard Page | ❌ |
| | COW Bit | ✅ | Fork COW | ✅ | COW Fault Handler | ✅ |
| | MAP_PRIVATE COW | ✅ | free_user_page_tables | ✅ | AnonVma | ✅ |
| | AnonVmaChain | ✅ | Rmap: Page Fault | ✅ | Rmap: COW | ✅ |
| | Rmap: Fork | ✅ | Rmap: Unmap | ✅ | Rmap: Exec | ✅ |
| | Zone Watermarks | ✅ | LRU (5 lists) | ✅ | kswapd | ✅ |
| | vmscan | ✅ | Page Cache Shrinker | ✅ | try_to_unmap | ✅ |
| | OOM Killer | ✅ | kswapd OOM Escalation | ✅ | /proc/oom_score | ✅ |
| | /proc/oom_score_adj | ✅ | Swap | ✅ | LRU Page Cache | ✅ |
| | /proc/meminfo | ✅ | Page Statistics | ✅ | Page Cache | ✅ |
| ✅ Complete (9) | Boot & Init · System Calls · Scheduler · Process Mgmt · Security · Diagnostics · Testing · Build & Tooling · Memory Mgmt |
| | PID Reuse | ✅ | Kernel Stack Cache | ✅ | Parent-Child-Sibling | ✅ |
| | ListHead | ✅ | Init Process (PID 1) | ✅ | Register Save | ✅ |
| | FPU Save | ✅ | tp Update | ✅ | U-mode Switch | ✅ |
| | User Stack | ✅ | ELF Loading | ✅ | Auxiliary Vector | ✅ |
| | CLONE_VM/FILES/FS | ✅ | CLONE_SIGHAND/THREAD | ✅ | CLONE_SETTLS | ✅ |
| | CLONE_CLEARTID/SETTID | ✅ | CLONE_DETACH | ✅ | robust_list | ✅ |
| | SignalStruct (64) | ✅ | SigAction | ✅ | Signal Mask | ✅ |
| | SIGKILL/SIGSTOP | ✅ | User-mode Handler | ✅ | Signal Frame | ✅ |
| | rt_sigreturn | ✅ | Realtime Queue | ✅ | sigaltstack | ✅ |
| | Signal Edge Cases | ⚠️ | do_exit | ✅ | SIGCHLD | ✅ |
| | Zombie Reaping | ✅ | do_wait | ✅ | FsStruct | ✅ |
| | FdTable | ✅ | brk | ✅ | oom_score_adj | ✅ |
| | Cred (8 IDs) | ✅ | Fork Inheritance | ✅ | kthread_create | ✅ |
| | kthread_should_stop | ✅ | kthread_run | ✅ | | |
| **6. File System** ⚠️ | open/close | ✅ | Path Resolution | ✅ | Symbolic Link | ✅ |
| | Dentry Cache | ✅ | Inode Cache | ✅ | LRU Eviction | ✅ |
| | Superblock | ✅ | Mount/Unmount | ✅ | FdTable (Arc) | ✅ |
| | alloc/install fd | ✅ | fd Reuse | ✅ | O_CLOEXEC | ✅ |
| | Memory FS | ✅ | File/Dir Ops | ✅ | /proc/meminfo | ✅ |
| | /proc/cpuinfo | ✅ | /proc/version | ✅ | /proc/uptime | ✅ |
| | /proc/loadavg | ✅ | /proc/cmdline | ✅ | /proc/mounts | ✅ |
| | /proc/interrupts | ✅ | /proc/self | ✅ | /proc/pid/status | ✅ |
| | /proc/pid/stat | ✅ | /proc/pid/cmdline | ✅ | /proc/pid/exe | ✅ |
| | /proc/pid/cwd | ✅ | /proc/pid/environ | ✅ | /proc/pid/fd | ✅ |
| | /proc/pid/maps | ✅ | /proc/pid/oom_score | ✅ | /proc/pid/oom_score_adj | ✅ |
| | Device Registry | ✅ | /dev/input | ✅ | Circular Buffer | ✅ |
| | Blocking I/O | ✅ | Transaction | ✅ | Commit/Recovery | ✅ |
| | Checkpoint | ✅ | Revoke Records | ✅ | Crash Recovery | ✅ |
| | Superblock | ✅ | Block Group | ✅ | Inode | ✅ |
| | BlockAllocator | ✅ | InodeAllocator | ✅ | mballoc (locality) | ✅ |
| | mballoc (spiral) | ✅ | mballoc (prealloc) | ✅ | Dir/File Ops | ✅ |
| | Extent Tree | ✅ | JBD2 Integration | ✅ | Hard Link | ✅ |
| | Symlink | ✅ | Truncate | ✅ | Rename (renameat2) | ✅ |
| | O_EXCL | ✅ | Shrinker Interface | ✅ | Read-ahead | ✅ |
| | uid/gid Enforcement | ❌ | | | | |
| **9. Interrupts** ⚠️ | PLIC Init | ✅ | Priority/Enable | ✅ | Claim/Complete | ✅ |
| | UART Interrupt | ✅ | VirtIO MMIO | ✅ | VirtIO PCI | ✅ |
| | Interrupt Sharing | ❌ | SBI TIMER | ✅ | SSTC (stimecmp) | ✅ |
| | Periodic Interrupt | ✅ | High-precision Timer | ❌ | SBI IPI | ✅ |
| | Reschedule IPI | ✅ | SSIP + Bitmap | ✅ | | |
| **10. SMP** ⚠️ | SBI HSM | ✅ | Secondary Core Boot | ✅ | Per-CPU Stacks | ✅ |
| | CPU Hot Plug | ❌ | Stack/RunQueue/Idle | ✅ | Pagesets | ✅ |
| | Per-CPU Vars | ❌ | spin::Mutex | ✅ | RwLock | ✅ |
| | SeqLock | ✅ | Kernel Big Lock | ✅ | | |
| **11. Sync Primitives** ⚠️ | Semaphore (down/up) | ✅ | Condvar wait/signal | ✅ | Condvar broadcast | ✅ |
| | wait_timeout | ❌ | Mutex lock/unlock | ✅ | MutexGuard | ✅ |
| | Deadlock Detection | ✅ | Futex wait/wake | ✅ | PI Futex | ✅ |
| | REQUEUE | ✅ | CMP_REQUEUE | ✅ | CLOCK_REALTIME | ✅ |
| | Futex Edge Cases | ⚠️ | Tiny RCU | ✅ | SeqLock | ✅ | | |
| **12. ELF Loader** ⚠️ | ELF Header | ✅ | Program/Section Header | ✅ | Dynamic Linking | ✅ |
| | PT_INTERP | ✅ | Auxiliary Vector | ✅ | Page Table Creation | ✅ |
| | PT_LOAD Mapping | ✅ | VM_EXECUTABLE | ✅ | User Stack/BSS | ✅ |
| | Entry Point/execve | ✅ | Block Sources | ✅ | ASLR/KASLR | ❌ |
| | ld-musl | ✅ | Shebang (#!) | ✅ | | |
| **13. Block Device** ⚠️ | Device Detection | ✅ | VirtQueue | ✅ | Modern PCI | ✅ |
| | VirtIO MMIO | ✅ | Read (MMIO+PCI) | ✅ | Write (MMIO+PCI) | ✅ |
| | Multi-queue | ❌ | BufferHead | ✅ | Block Cache | ✅ |
| | bread/brelse | ✅ | GenDisk | ✅ | Request Queue | ✅ |
| | Request Scheduling | ❌ | | | | |
| **14. Network** ⚠️ | socket/bind/listen | ✅ | accept4/connect | ✅ | send/recv | ✅ |
| | sendmsg/recvmsg | ✅ | shutdown | ✅ | get{sock,peer}name | ✅ |
| | socketpair | ✅ | set/getsockopt | ✅ | Three-way Handshake | ✅ |
| | TCP State Machine | ✅ | Retransmission (RTO) | ✅ | Sliding Window | ✅ |
| | Congestion Control | ✅ | Fast Retransmit | ✅ | TCP Checksum | ✅ |
| | Four-way Close | ✅ | UDP Datagram | ✅ | UDP Checksum | ✅ |
| | IPv4 | ✅ | Routing Table | ✅ | ARP | ✅ |
| | ICMP | ✅ | IP Fragmentation | ❌ | VirtIO-net | ✅ |
| | Packet TX/RX | ✅ | Loopback | ✅ | SkBuff | ✅ |
| | Protocol Layering | ✅ | | | | |
| **15. Graphics** ⚠️ | Framebuffer | ✅ | fbdev | ✅ | VirtIO-GPU | ✅ |
| | GPU Acceleration | ❌ | evdev | ✅ | PS/2 Keyboard/Mouse | ✅ |
| | VirtIO Input | ✅ | Multi-touch/Gamepad | ❌ | rux_gui Library | ✅ |
| | Desktop/Calculator | ✅ | Clock/vshell | ✅ | Full Desktop | ❌ |
| **16. Diagnostics** ✅ | Panic Handler | ✅ | Stack Trace | ✅ | Hung Task Detector | ✅ |
| | printk | ✅ | Ring Buffer | ✅ | pr_* Macros | ✅ |
| | errno (50+) | ✅ | Result/Option | ✅ | | |
| **17. Testing** ✅ | Framework (60 files) | ✅ | ListHead/Path/FileFlags | ✅ | Heap/PageAlloc/COW | ✅ |
| | Scheduler/Signal | ✅ | fork/execve/wait4 | ✅ | file_open/FdTable | ✅ |
| | Dcache/Icache/ext4 | ✅ | virtio_queue | ✅ | Boot/Multicore | ✅ |
| | mini-lTP (25) | ✅ | Smoke Tests (15/15) | ✅ | | |
| **18. Build & Tooling** ✅ | Cargo Workspace | ✅ | Makefile | ✅ | QEMU Scripts | ✅ |
| | Kernel.toml | ✅ | menuconfig | ✅ | test/run.sh | ✅ |
| | README | ✅ | Architecture Docs | ✅ | Design/Dev Guides | ✅ |
| **19. Security** ✅ | Cap Type (u64) | ✅ | 41 CAP_* Constants | ✅ | capable() API | ✅ |
| | capget / capset | ✅ | Signal Permission | ✅ | File Permission | ✅ |
| | IPC Permission | ✅ | setuid/setgid Exec | ✅ | LSM Hook Framework | ✅ |
| | Capability LSM | ✅ | euid→cap Migration | ✅ | | |
---

## Development History

| Era | Phase | Theme | Key Deliverables |
|-----|-------|-------|-----------------|
| Foundation | 1–5 | Boot & Basics | OpenSBI, MMU trampoline, exceptions, buddy allocator, fork/execve, scheduler |
| Core Infra | 6–10 | Interrupts, SMP, Sync | PLIC/timer/IPI, 4-core HSM boot, spinlock/RwLock/mutex/condvar, VFS, ELF |
| User Mode | 11–15 | Signals, COW, Testing | U-mode switch, signal frame, clone flags, COW, pipe, 60 test files, mini-lTP |
| Storage | 16–17 | Block Device & Filesystem | Preemptive sched, VirtIO-blk, ext4 (inode, dir, extent tree), bio cache |
| Network | 18 | TCP/IP Stack | SkBuff, ARP, IPv4, UDP, TCP (handshake, state machine, retransmission), VirtIO-net |
| Platform | 18.5–22 | Modernization & Shell | VirtIO PCI 1.0+, musl toolchain, toybox, multi-shell, procfs, boot beautification |
| Scheduler | 23–25 | CFS & Reliability | CFS v1 (enabled by default), COW (fork+mmap), TCP retransmission, sigaltstack |
| Hardening | 26–28 | Linux-Style Arch | Zone allocator, vmemmap, PCP, memblock, ASID, rmap, MMU trampoline, FPU, JBD2 |
| Audit | 29–32 | Correctness & ABI | ext4 write correctness, 345 syscall audit (6 fixes), VFS path cleanup, concurrent I/O |
| Refactoring | 33–36 | VFS & FS Cleanup | inode.ops unification (-44% VFS code), JBD2 recovery, mballoc, async I/O |
| IPC | 37–38 | Inter-Process Communication | System V IPC (sem/msg/shm), POSIX MQ, 18 syscalls, 6 rounds correctness fixes |
| Memory Safety | 39–40 | Rmap & OOM | try_to_unmap (task scan), OOM killer (oom_badness, SIGKILL, kswapd escalation) |
| Security | 41 | Capabilities & LSM | POSIX.1e caps (41 CAP_*), capget/capset, LSM framework, signal/file/IPC permission, setuid/setgid exec |
| Networking | 42 | TCP Close & ICMP | TCP four-way close (FIN/RST/process_ack), ICMP echo reply, dest unreach, tcp_v4_err |
| Memory | 43 | Swap | Swap entry encoding (PTE bit 62), swap device (bitmap slot allocator, VirtIO-blk), swap-out (vmscan→swap_write→unmap_with_swap), swap-in (page fault→swap_read→map), LRU/rmap field conflict resolved (dedicated lru_next) |
| Async I/O | 44 | IO_uring | io_uring_setup/enter/register (NR 425-427), SQ/CQ ring buffers (mmap shared), opcodes: NOP/READ/WRITE/FSYNC/CLOSE/FADVISE, eventfd notification, Linux ABI compatible wire format |
| Memory | 45 | LRU Page Cache | Page cache pages on LRU_INACTIVE_FILE, LRU-based eviction (access-recency), Referenced flag for active/inactive rotation, /proc/meminfo real Cached/Active(file)/Inactive(file)/Swap stats |
| Timers | 46 | POSIX Timers | Timer wheel (BTreeMap + Hrtimer softirq), setitimer/getitimer (ITIMER_REAL with SIGALRM), timer_create/settime/gettime/delete/getoverrun, timerfd_create/settime/gettime (read returns expiration count), periodic timer re-arm |
| FS | 47 | JBD2 Crash Recovery | Two-pass recovery (PASS_SCAN finds last valid commit block, PASS_REPLAY replays only committed transactions), prevents replaying incomplete transaction data after crash |
| Sync | 48 | Tiny RCU | Non-preemptible RCU (rcu_read_lock = preempt_disable), per-CPU callback lists, softirq-driven callback processing, generation-counter grace period detection, QS hooks in __schedule and cpu_idle_loop, boot.S early page table expanded 4MB→8MB |
| Sync | 49 | RCU PID Hash Table | PID hash table rewritten from BTreeMap to RCU-protected chained hash table, lock-free lookup via rcu_read_lock/unlock, per-bucket spinlock for insert/remove, synchronize_rcu in release_task for safe deferred reclamation |
| Sync | 50 | SeqLock | Sequence lock for read-mostly data (RawSeqLock + SeqLock<T: Copy> + SeqLockWriteGuard), lock-free readers with retry-on-write, writer serialization via odd/even sequence counter, loopback/hugepage stats converted from Spinlock |
| Memory | 51 | Memory Compaction | Two-pointer scan compaction (migrate UP + free DOWN), page migration (unmap + copy + remap), compaction fallback in alloc_pages for high-order allocations, free block consolidation via buddy merge |

---

## Planned Features

| Priority | Feature | Description |
|----------|---------|-------------|
| P1 | PID namespace | Process isolation |
| P1 | cgroup v1 | Basic resource control (memory, CPU) |
| P1 | IP fragmentation | Jumbo frame support |
| P2 | Transparent huge pages | PMD fault handler integration |
| P2 | Device tree (DTB) | Hardware description parsing |
| P2 | Vectored trap mode | Faster interrupt dispatch |
| P3 | Virtualization | KVM, containers |
| P3 | Power management | Frequency scaling, hibernate |
| P3 | Multimedia | Audio, video |
| P3 | CPU hot plug | Runtime CPU add/remove |
| P3 | ASLR / KASLR | Address space layout randomization |
| P3 | POSIX real-time | Full real-time scheduling support |
| P3 | File capabilities | security.capability xattr support in ext4 |
| P3 | MAC module (Smack/SELinux) | Mandatory access control via LSM framework |

---

**Document Version**: v27.0
**Last Updated**: 2026-04-07
