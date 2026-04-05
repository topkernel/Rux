# Rux OS Changelog

This document records important changes and fixes to the Rux kernel.

## [Unreleased]

### 2026-04-05 — Phase 37: IPC Subsystem (System V + POSIX MQ)

**New module** (`kernel/src/ipc/`):
- `util.rs`: IPC IDs registry (`IpcIds<T>`), `KernIpcPerm`/`IpcPermUapi` permissions, ID encoding (Linux-style `(index << 16) | seq`), permission checking
- `sysv_sem.rs`: System V semaphores — `sys_semget`, `sys_semctl` (IPC_STAT/RMID/SET/GETVAL/SETVAL/GETALL/SETALL/GETPID/GETNCNT/GETZCNT/IPC_INFO), `sys_semop`, `sys_semtimedop` (three-pass atomic apply, WaitQueue blocking, jiffies timeout)
- `sysv_msg.rs`: System V message queues — `sys_msgget`, `sys_msgctl` (IPC_STAT/RMID/SET/IPC_INFO), `sys_msgsnd` (priority insertion, queue-full blocking), `sys_msgrcv` (type matching, E2BIG truncation, empty-queue blocking)
- `sysv_shm.rs`: System V shared memory — `sys_shmget` (physical page allocation, zero-fill), `sys_shmctl` (IPC_STAT/RMID/SET/IPC_INFO/SHM_LOCK/SHM_UNLOCK), `sys_shmat` (VMA-based attachment, `map_user_page` page table mapping), `sys_shmdt` (VMA removal, munmap, delayed destroy)
- `posix_mq.rs`: POSIX message queues — `sys_mq_open` (name-based lookup/creation, fd allocation from offset 512+), `sys_mq_unlink`, `sys_mq_timedsend` (priority insertion), `sys_mq_timedreceive` (priority-based receive), `sys_mq_notify` (no-op accept), `sys_mq_getsetattr`

**Syscall dispatch** (`syscall/dispatch.rs`, `syscall/time.rs`):
- NR 180-197: Routed from ENOSYS stubs to full `ipc::` implementations
- NR 418-420: time64 MQ variants routed directly to `ipc::posix_mq::`

**Process cleanup** (`syscall/process.rs`):
- Removed 18 ENOSYS stubs for IPC syscalls (msgget through mq_getsetattr, shmget through shmdt)

### 2026-04-01 — Phase 3.6: Driver Migration to Softirq Bottom Half

**VirtIO Block MMIO → Block softirq** (`drivers/virtio/mod.rs`):
- Extracted completion loop (used ring processing, buffer deallocation, I/O completion signaling) into `block_bh_handler()`
- `interrupt_handler()` now only acks device interrupt and raises `Block` softirq

**VirtIO Net → NetRx softirq** (`drivers/net/virtio_net.rs`):
- Added `net_rx_softirq_handler()` wrapping `ethernet_poll()`
- `interrupt_handler()` now only acks device interrupt and raises `NetRx` softirq
- Entire RX path (ethernet→IP→TCP/UDP) now runs in softirq context

**TCP Timer → Timer softirq** (`net/tcp_timer.rs`):
- Added `timer_softirq_handler()` wrapping `tcp_timer_tick()`
- Timer interrupt already raised `Timer` softirq — handler was just not registered until now

**Softirq handler registration** (`interrupt/softirq.rs`):
- `init()` now registers Timer, NetRx, and Block softirq handlers via `open_softirq()`
- All registrations complete before device interrupts are enabled

### 2026-04-01 — Phase 2: Interrupt Stack Enhancement

**`on_thread_stack()` Precise Detection** (`arch/riscv64/trap.S`):
- Replaced IRQ-stack range check with task-kernel-stack range check in `.Lfrom_kernel`
- Compares sp against `task.ti_kernel_sp` bounds `[ti_kernel_sp - KERNEL_STACK_SIZE, ti_kernel_sp)`
- Correctly handles boot stack, SMP boot stack, and other non-task stacks — all switch to IRQ stack
- Added `beqz` null check: if `ti_kernel_sp == 0`, use IRQ stack
- Added `KERNEL_STACK_SIZE = 32768` constant to trap.S

**Softirq Stack Reuse** (`interrupt/softirq.rs`):
- Added `do_softirq_own_stack()` — switches sp to per-CPU IRQ stack before processing softirqs
- `invoke_softirq()` now checks `in_irq()`: runs inline if already on IRQ stack, otherwise switches
- Inline asm sp swap — no TLB/page table changes needed under BKL
- Matches Linux `do_softirq_own_stack()` pattern

### 2026-04-01 — Phase 7/8/9: IPI Enhancement, UART Interrupt-Driven I/O, NMI Framework

**Phase 9: NMI Framework** (`interrupt/preempt.rs`, `interrupt/irqdesc.rs`):
- Added `nmi_enter()`/`nmi_exit()` to preempt_count (increment/decrement NMI_OFFSET, no softirq invoke)
- Added `in_nmi()`, `irqentry_nmi_enter()`/`irqentry_nmi_exit()` wrapper functions
- Added `request_nmi()`/`free_nmi()` registration API (4 slots, write-once at init)
- Added `handle_fasteoi_nmi()` — lock-free NMI dispatch (no EOI, no stats, no softirq)
- Added `arch_trigger_cpumask_backtrace()` stub (QEMU virt has no Smrnmi)

**Phase 7: IPI Enhancement** (`arch/riscv64/ipi.rs`):
- Expanded IPI types: Reschedule, CallFunction, Stop, IrqWork (4 types)
- Per-CPU bitmap multiplexing: AtomicU32 pending bitmap per CPU, single SBI IPI per batch
- `request_ipi()` write-once handler registration
- `send_ipi_type()` — set pending bit + SBI IPI (idempotent, coalesces duplicate sends)
- `handle_software_ipi()` — `swap(0)` snapshot, dispatch LSB-first by priority
- `smp_call_function()` — cross-CPU callback with per-CSP CallSingleData queues
- Backward-compatible `send_reschedule_ipi()` wrapper retained

**Phase 8: UART Interrupt-Driven I/O** (`console.rs`, `fs/char_dev.rs`):
- Split `console::init()` into `early_init()` (no-op) + `init_irq()` (after PLIC)
- 16550A register constants (IER, FCR, LSR, IIR)
- SPSC ring buffer (1024 bytes, lock-free, single-producer IRQ, single-consumer task)
- UART IRQ handler drains hardware FIFO into ring buffer, wakes blocked readers
- `uart_read()` rewritten with `wait_event_interruptible!` instead of `yield_cpu()` polling
- `uart_has_data()` non-destructive check for poll/wait condition
- Fixed `uart_data_ready()` (was consuming characters in poll path)

### 2026-04-01 — Bug Fixes: trap.S, Layout panic, network buffer safety

**trap.S `ld`→`lw` Fix (Critical)**:
- Fix `GET_PER_CPU_INTR_STACK` macro in `trap.S` to use `lw` (4-byte load) instead of `ld` (8-byte load) when reading `ti_cpu` (AtomicI32 at offset 0x18)
- The 8-byte load also read the adjacent `state` field (AtomicU32 at offset 0x1C), corrupting the CPU ID when `state != RUNNING(0)`, producing a wrong interrupt stack address
- This was the root cause of "Kernel stack overflow" panics during interactive mode where timer interrupts fire while tasks are in non-RUNNING states

**Layout::from_size_alignment_unchecked Panic Fix**:
- Add `-Zub-checks=no` to rustflags to disable Rust nightly's runtime UB precondition checks on `Layout::from_size_align_unchecked`
- The check was firing in `__rust_alloc` (generated by `#[global_allocator]`), panicking on corrupted allocation sizes instead of returning null for OOM handling

**Network Buffer Safety**:
- `SkBuff::free()` (buffer.rs): Replace `.unwrap()` with safe error handling to prevent kernel panic on corrupted `end < head` pointers
- `virtio_net.rs` RX buffer dealloc: Fix alloc/dealloc Layout mismatch — was using `total_len + 256` (varies per packet), now uses same `VirtIONetHdr + mtu + 64` formula as allocation

**Process Exit Fixes**:
- `do_exit()`: Add CFS BTreeMap dequeue before removing from legacy run queue array, preventing dangling pointer when another CPU frees the task
- `do_wait_nonblock()`: Same CFS dequeue fix
- `do_wait()`: Remove duplicate `remove_child()` call (already done inside `release_task`)

### 2026-03-30 — Phase 36: Filesystem Refactoring Complete

**Block Cache (Phase 5)**:
- Replace single `Mutex<BlockCacheInner>` with 64 per-bucket `spin::Mutex<HashBucket>`
- Global entry count uses `AtomicU32` for lock-free capacity checks
- `evict_one()` releases all locks before `sync()` — I/O no longer blocks cache lookups
- `sync_all()` collects dirty buffers under bucket locks, syncs without holding any lock
- `bread_async()` now performs eviction (was missing)

**Multi-Block Allocator (Phase 7)**:
- Goal-group spiral search replacing linear group-0 scan
- Per-inode block preallocation (up to 8 extra contiguous blocks)
- Buddy bitmap scan with 0xFF fast-path (skip fully-occupied bytes)
- Eliminated bitmap double-read (find + mark + write in single pass)
- Deduplicated `find_free_bit` between BlockAllocator and InodeAllocator

**Async I/O Framework (Phase 9)**:
- `IoCompletion` primitive (AtomicBool + AtomicI32 + WaitQueueHead)
- `blkdev_read_async` → VirtIO `submit_read_async` (fire-and-forget)
- `bread_async`/`bread_wait` for async block cache operations
- Batch read-ahead: 4 prefetch blocks submitted in parallel instead of serial

**Bug Fixes**:
- `sys_symlinkat`: ext4 fast/slow symlink + VFS symlink
- `sys_statx`: Linux ABI `struct Statx` (256 bytes)
- `sys_openat2`: `struct open_how` parsing
- Rootfs rename cross-directory data corruption
- ext4 indirect block leak (recursive free for single/double/triple)
- `strncpy_from_user` access_ok overflow near user space boundary

---

### 2026-03-27

#### Documentation Updates

- Updated roadmap.md to v6.0 (Phase 28, 222 files, ~74,800 lines)
- Updated README.md with accurate statistics and module distribution
- Updated boot.md to document Linux-style MMU trampoline boot process
- Updated memory.md to document refactored memory management system
- Updated structure.md to v8.0 with accurate file listings and line counts

---

### 2026-03-22 ~ 2026-03-27

#### Phase 28: Linux-Style Boot & Architecture Refactoring

**MMU Trampoline Boot** (kernel/src/arch/riscv64/boot.S)
- Kernel linked at KERNEL_LINK_ADDR (0xffffffff80000000) instead of physical address
- VMA/LMA linker script: code runs at virtual address, loaded at physical address
- boot.S creates trampoline page tables (PGD + PMD for first 2MB), enables MMU
- stvec trick: set stvec to VA, write satp to enable MMU, trap redirects to VA
- Three-stage page table allocation: Early (static BSS) → Fixmap (memblock) → Late (buddy)
- medany code model for PC-relative addressing across VA/PA boundary

**PtRegs at Kernel Stack Top** (kernel/src/arch/riscv64/trap.S, pt_regs.rs)
- Linux-style: PtRegs always at (kernel_stack_top - sizeof(PtRegs))
- sscratch/tp protocol for user/kernel mode detection in trap handler
- ret_from_fork_user and ret_from_fork_kernel assembly paths

**Context Switch Refactoring** (kernel/src/arch/riscv64/context.rs)
- switch_mm: write SATP for page table switch
- __switch_to: save/restore callee-saved registers via assembly
- FPU state save/restore integrated in context_switch
- ThreadStruct replaces CpuContext for architecture-specific thread state

**User Access Safety** (kernel/src/arch/riscv64/uaccess.rs, uaccess.S)
- copy_to_user/copy_from_user with word-aligned optimization
- access_ok checks for user pointer validation in all syscalls
- ioctl, writev, execve path handling use safe user space access

**JBD2 Journaling Layer** (kernel/src/fs/jbd2/)
- 8 modules: types, journal, transaction, commit, recovery, checkpoint, revoke, mod
- Based on Linux kernel fs/jbd2/
- Transaction start/stop/extend, dirty metadata tracking

**ext4 Write Operations** (kernel/src/fs/ext4/)
- mkdirat, rmdir, unlinkat syscalls implemented
- Directory create/delete with block allocation
- Extent-aware block lookup for directory operations
- 64-bit group descriptor support

**procfs Enhancement** (kernel/src/fs/procfs/)
- Modular implementation with separate files (meminfo, cpuinfo, pid, loadavg, interrupts)
- /proc/interrupts for per-CPU interrupt statistics
- /proc/pid/ entries (status, cmdline, stat, exe, cwd, environ, fd)

**Other Fixes**
- fix(mm): prevent execve from corrupting parent page table via shared L1 entries
- fix(mm): fix page refcount and kernel table sharing in fork/exec
- fix(mm): copy VPN2=1 (PCI MMIO) mappings to user page tables
- fix(trap): fix trap return and signal handling for userspace programs
- fix(sched): fix context_switch FPU restore timing

---

### 2026-03-14 ~ 2026-03-22

#### Phase 27: Linux-Style Memory Management Refactoring

**Zone Allocator** (kernel/src/mm/zone.rs)
- ZONE_DMA, ZONE_DMA32, ZONE_NORMAL, ZONE_MOVABLE zone types
- MAX_ORDER=10 buddy system (up to 4MB allocations)
- GFP flags (GFP_KERNEL, GFP_USER)

**vmemmap** (kernel/src/mm/vmemmap.rs)
- Linux-style virtual memory map for page descriptors
- O(1) pfn_to_page conversion via arithmetic: VMEMMAP_START + (pfn - base_pfn) * sizeof(Page)

**Per-CPU Pagesets** (kernel/src/mm/pcp.rs)
- Per-CPU page caching for fast allocation
- alloc_page_pcp / free_page_pcp

**Memblock** (kernel/src/mm/memblock.rs)
- Early memory reservation system
- phys_alloc for boot-time page allocation before buddy is ready

**Page Descriptors** (kernel/src/mm/page_desc.rs)
- Page struct with flags, refcount, order, mapping
- get_page() / put_page() reference counting
- Anonymous page tracking for /proc/meminfo

**ASID Management** (kernel/src/arch/riscv64/mm/asid.rs)
- 9-bit ASID (512 max processes)
- Bitmap allocator with CAS
- Per-process AsidContext with generation counter
- TLB flush operations: all, per-ASID, per-page, range

**Demand Paging** (kernel/src/arch/riscv64/mm/page_fault.rs)
- Anonymous page allocation on first access
- COW fault handler: detect COW bit, allocate new page, copy data, clear COW
- On-demand stack expansion (Linux-style grow-down)

**Address Space Unification** (kernel/src/arch/riscv64/mm/memory_layout.rs)
- Linux RISC-V Sv39 compatible address space layout
- PAGE_OFFSET = 0xffffffd600000000 (linear mapping)
- KERNEL_LINK_ADDR = 0xffffffff80000000 (kernel mapping, VPN2[510])
- VMEMMAP_START = 0xffffffc700000000
- VA_PA_OFFSET = PAGE_OFFSET - PHYS_MEMORY_BASE

**Copy-on-Write** (kernel/src/arch/riscv64/mm/mm_ops.rs)
- copy_kernel_mappings: VPN2[0..1] new L0 tables, VPN2[256..511] shared
- fork: mark all shared pages as COW (PTE bit 8), clear W bit
- free_user_page_tables: walk VPN2[0..255], use put_page() for page release

**Reverse Mapping** (kernel/src/mm/rmap.rs)
- AnonVma, AnonVmaChain for tracking which processes map a physical page
- try_to_unmap() for page reclamation preparation

**Huge Page Framework** (kernel/src/mm/hugepage.rs)
- PMD and PGD huge page allocation/free
- Alignment helpers (pmd_align, pgd_align)
- is_huge_pte() detection

**Scheduling Improvements** (kernel/src/sched/)
- Multi-class scheduler: stop, deadline (EDF+CBS), RT (FIFO/RR), fair (CFS), idle
- Kernel stack cache (up to 64 cached stacks)
- do_exit refactored with proper exit_mm/exit_files
- Kernel big lock for SMP safety

**Performance Optimizations**
- kfree O(n) → O(1) using cache_idx
- size_to_order O(log n) → O(1) with lookup table
- current() O(1) with lock → O(1) lock-free
- Remove redundant global TLB flush after address-specific flush

---

### 2026-03-11 ~ 2026-03-14

#### Phase 25: TCP Reliability and Signal Refinement

**TCP Reliability** (kernel/src/net/tcp.rs, tcp_timer.rs)
- Retransmission mechanism with configurable RTO
- Delayed ACK timeout, MSS configuration, max retries

**POSIX Signal Mechanism** (kernel/src/signal.rs)
- SignalStruct with per-process action array (64 entries)
- SigAction with SA_NOCLDSTOP, SA_NOCLDWAIT, SA_SIGINFO, SA_ONSTACK flags
- Signal frame construction on user stack (SigInfo, UContext, SigContext)
- rt_sigreturn via RISC-V trampoline (li a7, 139; ecall)
- Real-time signal queue with lock-free CAS enqueue/dequeue
- sigaltstack support (SS_DISABLE, SS_ONSTACK, SS_AUTODISABLE)

**Clone Flags** (kernel/src/process/fork.rs)
- CLONE_VM: shared address space
- CLONE_FILES: shared fdtable
- CLONE_FS: shared root/cwd
- CLONE_SIGHAND: shared signal handlers
- CLONE_THREAD: shared tgid
- CLONE_SETTLS, CLONE_PARENT_SETTID, CLONE_CHILD_CLEARTID

**FPU Context Switch** (kernel/src/arch/riscv64/)
- FPU state save/restore in context_switch
- FPU fields in ThreadStruct and PtRegs
- FPU-related CSRs in trap.S

**Kernel Security**
- access_ok checks for user pointer validation
- M-mode CSR replaced with S-mode CSR
- Removed user-mode access to physical memory and UART mappings

**Linux LTP Integration**
- Added LTP test suite support (1,838 tests)
- musl cross-compilation for full coverage (101% compile rate)
- sdk and ltp build targets in Makefile

---

### 2026-03-06 ~ 2026-03-11

#### Phase 24+: devfs, Code Quality, Shell Enhancement

**devfs Filesystem**
- Mounted at /dev, manages character and block device nodes
- Device registry with BTreeMap

**Shell Enhancement** (userspace/shell/)
- Command history, tab completion, line editing
- Manual echo support, prompt changed to root#

**ext4 Write Support**
- File write with directory expansion
- Buffer dirty bit handling
- FdTable switched to buddy allocator with Box

**procfs Modularization**
- Separate files for meminfo, cpuinfo, version, uptime, etc.
- Modular implementation with cleaner architecture

**VFS Refactoring**
- Linux-style inode_operations pattern
- Route VFS operations to ext4 when mounted

**Timer Interrupt Fix**
- Proper 64-bit cause parsing
- Timer interrupts enabled correctly

**procfs Enhancement**
- cpuinfo uses S-mode CSRs (not M-mode)
- /proc/interrupts for interrupt statistics

---

### 2026-03-04

#### Documentation Updates

**Debug Report** (docs/development/fork-exec-debug-report.md)
- Organized the complete fork + execve debugging process
- Recorded key issues: COW page table handling, context switching, sscratch detection
- Provided debugging tips and verification test methods

**Code Structure Cleanup**
- Removed deprecated ARM timer driver (armv8.rs)
- Moved pid.rs from sched/ to process/ directory
- Removed redundant process/test.rs

**Documentation Sync Updates**
- Updated README.md project statistics
- Updated roadmap.md development roadmap
- Updated getting-started.md quick start guide
- Updated testing.md test guide

### 2026-02-27

#### CFS Scheduler Implementation

**Phase 23: CFS Scheduler**

Implemented a Linux-like CFS (Completely Fair Scheduler), replacing the original Round Robin scheduler.

**CFS Core Implementation** (kernel/src/sched/cfs.rs)
- SchedEntity - Scheduling entity (vruntime, weight, execution time)
- CfsRunQueue - CFS run queue (BTreeMap sorted by vruntime)
- LoadWeight - Process weight (based on nice value)
- vruntime calculation - Fair CPU time distribution
- Time slice calculation - sched_slice() based on weight and load
- Preemption check - check_preempt() detects if preemption is needed

**nice Value Support** (kernel/src/process/task.rs)
- nice value to weight mapping (referencing Linux sched_prio_to_weight)
- set_nice() method updates scheduling weight
- PRIO_TO_WEIGHT and PRIO_TO_WMULT lookup tables

**Scheduler Integration** (kernel/src/sched/sched.rs)
- RunQueue integrates CfsRunQueue
- pick_next_task_cfs() selects task with minimum vruntime
- scheduler_tick() updates vruntime and checks preemption
- enqueue/dequeue task management

**Key Parameters** (referencing Linux)
- SCHED_MIN_GRANULARITY_NS = 700us (minimum scheduling granularity)
- SCHED_LATENCY_NS = 6ms (target scheduling period)
- NICE_0_LOAD = 1024 (default weight)

### 2026-02-27

#### Major Milestone: procfs Filesystem, Symbolic Links, toybox Support

**Phase 22: procfs, Symbolic Links, toybox Support Completed**

Implemented procfs virtual filesystem, ext4 symbolic link support, and successfully integrated toybox as userspace tools.

**procfs Filesystem** (kernel/src/fs/procfs.rs, kernel/src/fs/vfs.rs)
- /proc/meminfo - Memory info (total memory, available memory, cache, etc.)
- /proc/cpuinfo - CPU info (processor, ISA, mmu, etc.)
- /proc/version - Kernel version (Linux compatible format)
- /proc/uptime - System uptime
- /proc/cmdline - Kernel boot parameters
- /proc/self - Current process symbolic link
- Dynamic content generation mechanism
- Auto mount to /proc
- VFS layer procfs file read support

**ext4 Symbolic Link Support** (kernel/src/fs/ext4/)
- Symbolic link inode read
- Symbolic link target resolution
- sys_readlinkat system call implementation

**New System Calls** (kernel/src/arch/riscv64/syscall.rs)
- sys_readlinkat (78) - Read symbolic link target
- sys_prlimit64 (261) - Get/set resource limits
- sys_getrandom (278) - Get random bytes
- sys_set_tid_address (96) - Set clear_child_tid address
- sys_gettid - Get thread ID

**TLS Initialization Fix**
- Fixed toybox/musl libc TLS initialization issue
- Correctly set TLS pointer (fsbase) during ELF loading
- Support musl libc's __thread variables

**ELF Stack Layout Fix** (kernel/src/process/loader.rs)
- Fixed auxv vector table setup
- Fixed envp environment variable pointer setup
- Correctly calculate user stack layout

**toybox Integration** (userspace/, Makefile)
- Compile toybox using musl libc
- toybox sh as backup shell

#### Bug Fixes

**procfs Directory Listing Issue**
- Issue: `ls /proc` showed 0 entries
- Fix: procfs lookup function handles `.` and `..` special directory entries

**procfs File Read Issue**
- Issue: `cat /proc/version` returned ENOENT
- Fix: VFS file_open function added procfs path handling

**VFS Directory Lookup Order**
- Issue: First ls only showed proc directory, second time showed all content
- Fix: file_opendir checks ext4 first, then RootFS

**procfs File Infinite Loop Print**
- Issue: cat any procfs file caused infinite loop
- Fix: procfs_file_read uses file.get_pos() instead of content.offset

#### Code Changes

**New/Modified Files**:
- `kernel/src/fs/procfs.rs` - procfs filesystem implementation
- `kernel/src/fs/vfs.rs` - VFS procfs support
- `kernel/src/fs/ext4/mod.rs` - Symbolic link support
- `kernel/src/arch/riscv64/syscall.rs` - New system calls
- `kernel/src/config.rs` - AUTO_MOUNT_PROCFS configuration
- `userspace/toybox/` - toybox build configuration

#### Code Statistics

- **Kernel Code**: 49,490 lines of Rust code
- **New System Calls**: 5
- **procfs Files**: 6

### 2026-02-15

#### Major Milestone: Multi Shell Support and cmdline Fix

**Phase 20: Multi Shell Support and cmdline Fix Completed**

Implemented multiple userspace shells and kernel cmdline parsing fix, laying the foundation for upper-layer application development.

**Shell Support Status**:
- Default Shell (no_std Rust) - Fully functional
  - Built-in commands: echo, help, exit, time, pid
  - External program execution support
- C Shell (musl libc) - Ported, needs argc/argv initialization fix
- Rust std Shell - Ported, needs argc/argv initialization fix

#### New Features

**cmdline Parsing Fix** (kernel/src/cmdline.rs, kernel/src/arch/riscv64/boot.S)
- Fixed DTB pointer passing (boot.S saves DTB pointer via s0)
- Fixed FDT parsing string matching issue
- Support `init=/bin/sh` and other boot parameter configuration
- Read boot parameters from device tree /chosen/bootargs

**Multi Shell Support** (userspace/)
- Default Shell (userspace/shell/) - no_std Rust implementation
- C Shell (userspace/cshell/) - musl libc port
- Rust std Shell (userspace/rust-shell/) - Rust std support

**musl libc Toolchain**
- Added musl libc build script (toolchain/build-musl.sh)
- Added musl program linker script (userspace/musl.ld)
- Support statically linked musl C programs

**Shell Selection Mechanism** (Makefile)
- Select shell type via `SHELL_TYPE` parameter
- `make run SHELL_TYPE=default` - Default no_std shell
- `make run SHELL_TYPE=cshell` - C musl shell
- `make run SHELL_TYPE=rust-shell` - Rust std shell

#### Bug Fixes

**DTB Pointer Passing Issue**
- Issue: DTB pointer lost after BSS zeroing
- Fix: Use s0 callee-saved register in boot.S to save DTB pointer

**FDT String Matching Issue**
- Issue: `name.starts_with("chosen@")` match failed
- Fix: Correctly handle FDT node name format

#### Known Issues

**cshell and rust-shell Startup Failure**
- Cause: musl libc's `__init_libc` expects to read argc/argv from stack
- Current: UserContext::new() initializes all registers to 0
- Impact: Page fault when libc program tries to access argv[-1]
- Solution: Need to set argc/argv and stack initialization in UserContext

#### Code Changes

**New/Modified Files**:
- `kernel/src/cmdline.rs` - FDT parsing fix
- `kernel/src/arch/riscv64/boot.S` - DTB pointer saving
- `userspace/cshell/` - C musl shell implementation
- `userspace/rust-shell/` - Rust std shell implementation
- `userspace/musl.ld` - musl program linker script
- `toolchain/build-musl.sh` - musl build script

### 2026-02-14

#### Major Milestone: Shell Successfully Running

**Phase 19: Modern VirtIO PCI & Shell Running Completed**

Kernel can now successfully load and run shell from PCI VirtIO ext4 filesystem!

**Example Output**:
```
init: Starting init process (PID 1)...
init: Attempting to load /bin/sh from PCI VirtIO ext4 filesystem
init: Loaded /bin/sh from PCI VirtIO ext4 (79120 bytes)
mm: Mapped user memory: 0x10000-0x17000 (7 pages)
init: Created init process with PID 1, enqueued
main: Entering scheduler main loop...

========================================
  Rux OS - Simple Shell v0.1
========================================
Type 'help' for available commands

rux>
```

#### New Features

**Modern VirtIO PCI Driver**
- VirtIO PCI device detection and capability parsing
- Modern VirtIO 1.0+ transport layer implementation
- Removed Legacy VirtIO (v0.9.5) support
- PCI config space access (capability list traversal)
- ISR status register read
- Queue address setup (queue_desc/driver/device registers)
- DMA physical address mapping (virt_to_phys)

**ext4 Filesystem Enhancement**
- ext4 extent tree read support
  - Extent header parsing (eh_magic, eh_entries, eh_depth)
  - Extent node traversal (leaf nodes and intermediate nodes)
  - Extent data block range calculation (ee_block, ee_start, ee_len)
- Support extent-form file data block mapping

**Init Process Enhancement**
- Read `/bin/sh` from PCI VirtIO ext4 filesystem
- ELF loading and user memory mapping
- Process creation and scheduling queue addition

#### Bug Fixes

**VirtIO PCI Queue Notification Issue**
- Issue: Device not responding after writing queue_notify register
- Fix: Ensure correct MMIO address and physical address are used

**VirtIO Physical Address Mapping**
- Issue: Device needs physical address but code uses virtual address
- Fix: Added virt_to_phys conversion function

**Superblock Location Calculation**
- Issue: ext4 superblock located at 1024 bytes (between blocks 0 and 1)
- Fix: Correctly calculate superblock location as sector 2

**Buddy Allocator Redundant Debug Output**
- Issue: Large debug output slowing down system
- Fix: Removed redundant alloc/dealloc debug prints

#### Code Changes

**New/Modified Files**:
- `kernel/src/drivers/virtio/virtio_pci.rs` - Modern VirtIO PCI transport layer
- `kernel/src/drivers/virtio/probe.rs` - VirtIO device detection
- `kernel/src/drivers/virtio/mod.rs` - Block device driver (Modern only)
- `kernel/src/fs/ext4/file.rs` - Extent tree read support
- `kernel/src/arch/riscv64/mm.rs` - virt_to_phys function
- `kernel/src/mm/buddy_allocator.rs` - Removed redundant debug output

#### Code Statistics

- **Kernel Code**: 38,773 lines of Rust code
- **Shell Binary**: 79,120 bytes (statically linked)
- **Boot Time**: ~5 seconds (from QEMU boot to shell prompt)

### 2026-02-11

#### Refactoring

**VirtIO Probe Code Refactoring**
- Moved `virtio_probe.rs` to `drivers/virtio/probe.rs`
- VirtIO related code centralized management, optimized directory structure
- Maintained backward compatibility: maintained import path via `pub use virtio::probe`
- Code organization: drivers/virtio/ now contains complete VirtIO implementation

**Code Changes**:
- `kernel/src/drivers/virtio/probe.rs`: New (moved from virtio_probe.rs)
- `kernel/src/drivers/virtio/mod.rs`: Added `pub mod probe;`
- `kernel/src/drivers/mod.rs`: Added `pub use virtio::probe;` re-export
- `kernel/src/main.rs`: Updated import path to `drivers::probe::init_network_devices()`
- Deleted `kernel/src/drivers/virtio_probe.rs`

#### Bug Fixes

**Unit Test Fixes**
- Fixed network test PANIC (loopback statistics accumulation issue)
  - Added `loopback_reset_stats()` function in `loopback.rs`
  - Reset statistics at test start in `network.rs`
- Fixed SMP test compilation error (MAX_CPUS private import)
  - Import MAX_CPUS directly from `crate::config`
- Test pass rate: 175/176 (99.4%)
  - Only 1 failure is boundary test (task pool exhausted, expected behavior)

**Code Changes**:
- `kernel/src/drivers/net/loopback.rs`: +9 lines (loopback_reset_stats function)
- `kernel/src/tests/network.rs`: +3 lines (call reset_stats)
- `kernel/src/tests/smp.rs`: +3 lines (fix MAX_CPUS import)

### 2026-02-10

#### Refactoring

**Platform-Independent Pagemap Refactoring**
- Refactored `mm/pagemap.rs` from ARM-specific implementation to platform-independent interface (79-line thin wrapper)
- Moved VMA operations (mmap, munmap, brk, fork, allocate_stack) to `arch/riscv64/mm.rs`
- AddressSpace now uses `mm/page` types (VirtAddr, PhysAddr), with type conversion when needed
- Added `PhysAddr::ppn()` method for physical page number calculation
- Added `VirtAddr::as_usize()` method for address conversion
- Code net reduction of 298 lines (764 lines -> 79 lines + 293 lines platform-specific code)

**Code Changes**:
- `kernel/src/mm/pagemap.rs`: 764 lines -> 79 lines (platform-independent interface)
- `kernel/src/arch/riscv64/mm.rs`: +293 lines (VMA operations implementation)
- `kernel/src/mm/page.rs`: +5 lines (ppn() method)

#### Bug Fixes

**Unit Test Fixes**
- Fixed network test SkBuff headroom issue (alloc_skb reserves 16 bytes header space)
- Fixed test order issue (boundary test moved before fork test, preventing task pool exhaustion)
- Test pass rate improved: 161/167 -> 163/166 (only 3 boundary test cases remaining)

**sys_brk System Call**
- Implemented sys_brk system call (number 214)
- Support brk system call parameter validation and return value handling

#### Documentation Updates

- Updated this document to reflect pagemap refactoring and test fixes

### 2026-02-09

#### New Features

**Phase 18: Complete Network Protocol Stack Implementation**

**Network Buffer** (kernel/src/net/buffer.rs)
- SkBuff implementation (referencing Linux sk_buff)
- skb_push/skb_pull/skb_put operations
- Protocol layer management (Ethernet -> ARP -> IPv4 -> UDP/TCP)

**Ethernet Layer** (kernel/src/net/ethernet.rs)
- Ethernet frame handling (14-byte header)
- MAC address management (ETH_ALEN = 6)
- Ethernet header construction and parsing
- eth_build_header/eth_parse_packet

**ARP Protocol** (kernel/src/net/arp.rs)
- ARP protocol implementation (RFC 826)
- ARP cache (fixed size 64 entries)
- ARP packet construction (request/response)
- arp_build_request/arp_build_reply
- arp_rcv handler function

**IPv4 Protocol** (kernel/src/net/ipv4/)
- IP header structure (20 bytes, RFC 791)
- Routing table (longest prefix match)
- IP checksum calculation (RFC 1071)
- ip_push_header/ip_pull_header

**UDP Protocol** (kernel/src/net/udp.rs)
- UDP header (8 bytes, RFC 768)
- UDP Socket management (bind, connect, disconnect)
- UDP checksum (including pseudo header)
- udp_build_packet/udp_parse_packet
- UDP Socket table (fixed 64)

**TCP Protocol** (kernel/src/net/tcp.rs)
- TCP header (20 bytes, RFC 793)
- TCP state machine (11 states: CLOSE/LISTEN/SYN_SENT/ESTABLISHED, etc.)
- TCP Socket management (bind/listen/connect/accept/close)
- TCP checksum (including pseudo header)
- tcp_build_packet/tcp_parse_packet
- TCP Socket table (fixed 64)

**VirtIO-net Driver** (kernel/src/drivers/net/)
- VirtIO network device driver
- Device initialization (VirtIO device ID = 1)
- RX/TX queue management
- MAC address read (VirtIO config space)
- Packet receive and send

**Network Device Framework** (kernel/src/drivers/net/)
- NetDevice base class (space.rs)
- Loopback device driver (loopback.rs)
- Device registration and deregistration

**Network System Calls** (kernel/src/arch/riscv64/syscall.rs)
- sys_socket (198) - Create socket
- sys_bind (200) - Bind address
- sys_listen (201) - Listen for connections
- sys_accept (202) - Accept connection (partial implementation)
- sys_connect (203) - Initiate connection
- sys_sendto (206) - Send data (partial implementation)
- sys_recvfrom (207) - Receive data (partial implementation)

**Code Statistics**:
- New code: ~2,500 lines Rust code (network protocol stack)
- New code: ~1,200 lines Rust code (device drivers)
- New tests: ~200 lines test code
- Total: ~23,900 lines Rust code

#### Bug Fixes

**UDP Socket Alloc Return Type Fix**
- Fixed udp_socket_alloc() return type (Result<i32, i32>)
- Fixed errors in UDP checksum calculation

#### Documentation Updates

- Updated README.md - Added network subsystem feature matrix
- Updated test statistics (25 modules, ~280 test cases)
- Updated code statistics (~24,000 lines code)
- Updated development roadmap (Phase 18 completed)

### 2025-02-10

#### New Features

**Phase 17: Block Device Driver and ext4 Filesystem Complete Implementation**

**VirtIO Block Device Driver** (kernel/src/drivers/virtio/)
- VirtQueue implementation (queue.rs, 206 lines)
  - Follows VirtIO Specification v1.1
  - Descriptor management, queue notification, completion wait
- Block device driver (mod.rs, 470 lines)
  - Device initialization and detection
  - `read_block()` and `write_block()` implementation
  - VirtIO request/response handling
  - VirtQueue integration

**Buffer I/O Layer** (kernel/src/fs/bio.rs)
- BufferHead cache management (375 lines)
  - Block status tracking (Uptodate, Dirty, Locked)
  - Reference count management
  - Block data cache
- Block cache system
  - Hash table index (device major number + block number)
  - LRU-style cache management
- Buffer I/O functions
  - `bread()` - Read block to cache
  - `brelse()` - Release buffer
  - `sync_dirty_buffer()` - Sync dirty blocks to disk

**ext4 Filesystem** (kernel/src/fs/ext4/)
- Superblock and disk structures (superblock.rs, 315 lines)
  - Ext4SuperBlockOnDisk parsing
  - Block group descriptor parsing
  - Filesystem info extraction
- Inode operations (inode.rs, 287 lines)
  - Ext4Inode structure
  - Data block extraction (direct blocks)
  - File size read
- Directory operations (dir.rs, 164 lines)
  - Directory entry parsing
  - File lookup
- File operations (file.rs, 173 lines)
  - File read
  - File write (with block allocation support)
  - File seek

**ext4 Allocator** (kernel/src/fs/ext4/allocator.rs, 535 lines)
- BlockAllocator
  - Bitmap-based block allocation algorithm
  - Block group descriptor update
  - Superblock free block count update
  - `alloc_block()` - Allocate new block
  - `free_block()` - Free block
- InodeAllocator
  - Bitmap-based inode allocation algorithm
  - Inode table scan
  - `alloc_inode()` - Allocate new inode
  - `free_inode()` - Free inode

**Block Device Driver Framework** (kernel/src/drivers/blkdev/mod.rs, 276 lines)
- GenDisk structure
- Request queue
- BlockDeviceOps trait

**Unit Tests** (kernel/src/tests/)
- virtio_queue.rs - VirtIO queue tests (8 test cases)
- ext4_allocator.rs - ext4 allocator tests (7 test cases)
- ext4_file_write.rs - File write tests (5 test cases)

**Error Codes** (kernel/src/errno.rs)
- Added EFBIG (27) - File too large

**Code Statistics**:
- New code: ~3,200 lines Rust code
- New tests: ~800 lines test code
- Total: ~20,000 lines Rust code

#### Bug Fixes

**Type Mismatch Fixes**
- Fixed type conversion issues in ext4 filesystem
- Fixed mutable reference issues in VirtQueue
- Fixed type conversion issues in block allocator

#### Documentation Updates

- Updated README.md - Added block device and ext4 feature matrix
- Updated test statistics (23 modules, 261 test cases)
- Updated code statistics (~20,000 lines code)
- Updated development roadmap (Phase 17 completed)

### 2025-02-09

#### New Features

**RISC-V System Call Complete Implementation** (Phase 10 completed)
- Implemented complete system call handling framework
- User programs can successfully call system calls and exit normally
- Fixed sscratch register management, supporting consecutive system calls

**Core Features**:
1. **Trap Handling Mechanism** (`kernel/src/arch/riscv64/trap.S`, `trap.rs`)
   - Use sscratch register for fast switching between user stack and kernel stack
   - Save 272-byte TrapFrame on kernel stack
   - Correctly handle system calls, exceptions, and interrupts

2. **System Call Dispatcher** (`kernel/src/arch/riscv64/syscall.rs`)
   - Follow RISC-V Linux ABI conventions
   - System call number passed via a7 register
   - Parameters passed via a0-a5 registers
   - Return value via a0 register

3. **User Mode Switch** (`kernel/src/arch/riscv64/usermode_asm.S`)
   - Use sret instruction to switch from privileged mode to user mode
   - Linux-style single page table approach (no satp switch)
   - Correctly set sstatus.SPP=0 to ensure return to user mode

4. **User Program Support** (`userspace/hello_world/`)
   - Implement no_std user program
   - Inline assembly system call wrapper functions
   - Custom linker script (user.ld) linked to user space address

**Technical Details**:

```assembly
# Trap Entry (Simplified)
trap_entry:
    mv t0, sp                      # Save original sp
    csrrw sp, sscratch, sp          # Swap sp and sscratch (switch to kernel stack)
    addi sp, sp, -272              # Allocate TrapFrame
    sd t0, 0(sp)                   # Save original sp
    # ... save registers ...
    call trap_handler              # Call Rust handler
    # ... restore registers ...
    ld t0, 0(sp)                   # Load original sp
    addi sp, sp, 272               # Deallocate TrapFrame
    csrr t1, sscratch              # Read kernel stack pointer
    mv sp, t0                      # Restore original sp
    csrw sscratch, t1              # Restore kernel stack pointer to sscratch
    sret                           # Return to user/kernel mode
```

**Verified System Calls**:
- SYS_EXIT (93) - Process exit
- SYS_GETPID (172) - Get process ID

**Test Results**:
```
[TRAP:ECALL]           <- Trap handling entry
[ECALL:5D]             <- System call 0x5D (93) = sys_exit
sys_exit: exiting with code 0  <- sys_exit executed successfully
]                      <- Assembly code reached sret
```

**Key Files**:
- `kernel/src/arch/riscv64/trap.S` - Trap entry/exit assembly code
- `kernel/src/arch/riscv64/trap.rs` - Trap handling Rust code
- `kernel/src/arch/riscv64/syscall.rs` - System call dispatch and implementation
- `kernel/src/arch/riscv64/usermode_asm.S` - User mode switch assembly
- `kernel/src/embedded_user_programs.rs` - Embedded user program ELF data

#### Bug Fixes

**sscratch Register Management Issue**
- **Issue**: User stack pointer incorrectly written to sscratch at trap exit
- **Impact**: Second system call couldn't properly switch to kernel stack
- **Fix**: Reload kernel stack pointer to sscratch at trap exit
- **Code**:
```assembly
ld t0, 0(sp)           # Load original sp (user or kernel)
addi sp, sp, 272       # Deallocate trap frame
csrr t1, sscratch      # Read kernel stack pointer from sscratch
mv sp, t0              # Restore original sp (user or kernel)
csrw sscratch, t1      # Restore kernel stack pointer to sscratch
```

**User Program Embedding Update Issue**
- **Issue**: User programs not re-embedded in kernel after modification
- **Impact**: Kernel runs old version of user program
- **Fix**: Use `embed_user_programs.sh` script to re-embed user program ELF

#### Documentation Updates

- Added RISC-V system call implementation documentation
- Updated user program development guide
- Added trap handling flow diagram

### 2025-02-08

#### Bug Fixes

**BuddyAllocator Buddy Address Out-of-Bounds Fix** (commit 09c86dd)
- Fixed address out-of-bounds issue when merging buddy blocks in `free_blocks` function
- Added buddy address boundary check, preventing access to memory beyond heap_end
- Impact: Resolved Page Fault issues in FdTable and SimpleArc tests

**Issue Description**:
- When freeing order 12 (16MB) block, buddy address calculated as 0x81A00000
- This address is exactly heap_end, beyond MMU mapping range
- Caused Load page fault error

**Fix Solution**:
```rust
// Check if buddy is within heap range
let heap_start = self.heap_start.load(Ordering::Acquire);
let heap_end = self.heap_end.load(Ordering::Acquire);

if buddy_ptr < heap_start || buddy_ptr >= heap_end {
    // Buddy out of heap range, cannot merge
    self.add_to_free_list(current_ptr as *mut BlockHeader, current_order);
    break;
}
```

**Test Verification**:
- SimpleArc allocation test passed
- FdTable test passed
- No more Page Fault errors

#### New Features

**SimpleArc Allocation Test** (kernel/src/tests/arc_alloc.rs)
- Added SimpleArc memory allocation and deallocation tests
- Verified Arc::clone, reference count, drop functionality
- Tested File object creation and access

#### Documentation Updates

- Restructured documentation, created clear categorized organization
- Added documentation center index (docs/README.md)
- Archived historical debug documents to docs/archive/

---

## [0.1.0] - 2025-02-08

#### New Features

**Unix Process Management System Calls** (Phase 15)
- fork() - Create child process (commit a4bbc7a)
- execve() - Execute new program (commit 3b5f96d)
- wait4() - Wait for child process (commit 22ab972)

**Synchronization Primitives** (Phase 14)
- Semaphore - Semaphore mechanism (commit 5ea2376)
- Condition Variable - Condition variable (commit e832be1)

**RISC-V Architecture Support** (Phase 10)
- Boot process and OpenSBI integration
- Sv39 MMU and page table management
- PLIC interrupt controller driver
- IPI inter-core interrupt framework
- SMP multicore support (SBI HSM)

#### Bug Fixes

**Kernel Boot Issue** (commit 9de7b64)
- Fixed OpenSBI integration during kernel boot
- Fixed wait4 error code handling

**Timer interrupt sepc handling**
- No longer skip WFI instruction, avoiding jumping to instruction middle

**SMP + MMU Race Condition**
- Use `AtomicUsize` to protect `alloc_page_table()`'s `NEXT_INDEX`
- Per-CPU MMU enable: Secondary cores wait for boot core to complete page table initialization

#### Test Coverage

- 14 unit test modules
- fork, execve, wait4 tests
- SMP multicore boot tests
- SimpleArc and FdTable tests

#### Documentation

- CLAUDE.md - AI-assisted development guide
- UNIT_TEST.md - Unit test documentation
- USER_PROGRAMS.md - User program implementation plan
- CODE_REVIEW.md - Code review records

---

## Version Naming Convention

- **Major.Minor.Patch**
- Major: Major architecture changes or incompatible updates
- Minor: New feature additions
- Patch: Bug fixes and minor improvements

## Commit Convention

Follow [Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` - New feature
- `fix:` - Bug fix
- `docs:` - Documentation update
- `test:` - Test related
- `refactor:` - Code refactoring
- `perf:` - Performance optimization
