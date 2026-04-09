# Rux Kernel Project Structure

This document describes the directory structure and file organization of the Rux kernel project.

---

## Code Statistics

**Last Updated**: 2026-04-08

### Overall Statistics

| Metric | Value |
|--------|-------|
| **Total Source Files** | 278 (274 Rust + 3 Assembly + 1 Linker Script) |
| **Total Lines of Code** | **~102,400 lines** |
| **Kernel Binary Size (debug)** | ~3 MB |

### Module Code Distribution

| Module | Files | Lines of Code | Percentage | Description |
|--------|-------|---------------|------------|-------------|
| **fs/** | 52 | 22,539 | 22.2% | File system (ext4, procfs, jbd2, VFS) |
| **syscall/** | 11 | 12,692 | 12.5% | System call dispatch |
| **mm/** | 25 | 9,843 | 9.7% | Memory management |
| **tests/** | 60 | 9,641 | 9.5% | Unit tests |
| **drivers/** | 28 | 9,047 | 8.9% | Device drivers |
| **arch/** | 21 | 7,697 | 7.6% | Architecture-specific (RISC-V) |
| **Top-level** | 12 | 6,257 | 6.2% | Main entry, console, config, etc. |
| **net/** | 12 | 5,854 | 5.8% | Network protocol stack |
| **process/** | 9 | 4,667 | 4.6% | Process management |
| **ipc/** | 6 | 3,308 | 3.3% | IPC (System V, POSIX MQ) |
| **sched/** | 8 | 3,482 | 3.4% | Process scheduling |
| **sync/** | 8 | 2,478 | 2.4% | Synchronization primitives |
| **interrupt/** | 8 | 1,676 | 1.7% | Interrupt subsystem |
| **dfx/** | 8 | 1,027 | 1.0% | Diagnostics and debugging |

### Test Statistics

| Test Type | Count | Description |
|-----------|-------|-------------|
| **Kernel Unit Tests** | 58 files, 825 cases | Memory, process, file system, network, etc. |
| **Formal Verification** | 1,088 proptest cases (98 modules); 157 Kani proofs (22 modules); 4 SPIN models (8 LTL) | 4-layer: proptest (L1), Kani (L2), SPIN (L3), Miri (L4) |
| **Smoke Tests** | 15 tests (all passing) | Core functionality validation |
| **Linux LTP Tests** | 1,838 tests | Official LTP test suite (syscall, mem, fs, etc.) |
| **Total** | **3,777 test cases + 161 formal verification proofs** | Comprehensive kernel verification coverage |

---

## Directory Structure

```
Rux/
+-- build/                  # Build and configuration tools
|   +-- Makefile           # Build script
|   +-- menuconfig.sh      # Interactive configuration tool
|   +-- config-demo.sh     # Configuration demo script
|
+-- test/                   # Test and debug scripts
|   +-- run.sh             # Quick run script
|   +-- mkrootfs.sh        # Create rootfs image
|   +-- rootfs.img         # ext4 rootfs image (128MB)
|
+-- scripts/                # Utility scripts
|   +-- verify_sync_check.py # Kernel/verify sync checker
|
+-- userspace/              # Userspace programs
|   +-- mrsh/               # mrsh (minimal POSIX shell, musl libc)
|   |   +-- build-mrsh.sh   # Build script
|   |   +-- mrsh/           # mrsh source
|   |
|   +-- apps/               # GUI applications (musl libc)
|   |   +-- desktop/        # Desktop environment
|   |   +-- calculator/     # Calculator
|   |   +-- clock/          # Clock
|   |   +-- vshell/         # Visual Shell
|   |
|   +-- libs/               # Shared libraries
|   |   +-- gui/            # GUI library (rux_gui)
|   |
|   +-- tests/              # Userspace test programs
|   |   +-- smoke_test/     # Smoke tests (15 tests)
|   |
|   +-- linux-ltp/          # Official LTP test suite (1,838 tests)
|   |   +-- build.sh        # Build script (musl cross-compile)
|   |   +-- README.md       # LTP documentation
|   |
|   +-- toybox/             # Toybox (200+ command line tools)
|   |   +-- build-toybox.sh # Build script
|   |
|   +-- build               # User program build script
|   +-- Cargo.toml          # Cargo configuration
|   +-- README.md           # User program documentation
|
+-- toolchain/              # Toolchain
|   +-- build-musl.sh       # musl libc build script
|   +-- riscv64-rux-linux-musl/ # musl toolchain installation directory
|       +-- include/        # musl headers
|       +-- lib/            # musl static libraries
|
+-- docs/                   # Project documentation
|   +-- architecture/      # Architecture documentation
|   |   +-- boot.md        # Boot process (MMU trampoline)
|   |   +-- design.md      # Design principles
|   |   +-- memory.md      # Memory management design
|   |   +-- riscv64.md     # RISC-V architecture documentation
|   |   +-- structure.md   # This file
|   |   +-- syscall.tbl    # System call number table
|   +-- development/       # Development documentation
|   |   +-- compaction-design.md  # Memory compaction design
|   |   +-- formal-verification.md # Formal verification guide
|   +-- progress/          # Progress documentation
|   |   +-- roadmap.md     # Development roadmap
|   |   +-- changelog.md   # Change log
|   |   +-- quickref.md    # Quick reference
|   +-- guides/            # Guide documentation
|   |   +-- getting-started.md # Quick start
|   |   +-- development.md # Development standards
|   |   +-- configuration.md # Kernel configuration
|   |   +-- debugging.md   # Debugging guide
|   +-- test/              # Test reports
|   |   +-- unit-test-report.md       # 825 kernel unit tests
|   |   +-- formal-verification-report.md # 1,088 proptest cases + Kani + SPIN + Miri
|   |   +-- testing.md      # Testing overview
|   +-- archive/           # Archived design docs
|
+-- kernel/                 # Kernel source code
|   +-- src/               # Rust source code
|   |   +-- arch/         # Architecture-specific code
|   |   |   +-- riscv64/  # RISC-V architecture implementation
|   |   |       +-- mod.rs       # Module export
|   |   |       +-- boot.S       # MMU trampoline + boot code
|   |   |       +-- trap.S       # Exception vector table
|   |   |       +-- uaccess.S    # User space access (fixup)
|   |   |       +-- linker.ld    # Linker script (VMA/LMA)
|   |   |       +-- pt_regs.rs   # PtRegs (trap frame) structure
|   |   |       +-- context.rs   # Context switching
|   |   |       +-- process.rs   # User mode management
|   |   |       +-- thread.rs    # Thread structure
|   |   |       +-- boot.rs      # Boot initialization
|   |   |       +-- trap.rs      # Exception handling
|   |   |       +-- smp.rs       # Multi-core support
|   |   |       +-- ipi.rs       # Inter-processor interrupt
|   |   |       +-- cpu.rs       # CPU operations
|   |   |       +-- uaccess.rs   # User space access helpers
|   |   |       +-- mm/          # Architecture MMU
|   |   |           +-- mod.rs
|   |   |           +-- memory_layout.rs  # Sv39 constants, KernelMapping
|   |   |           +-- pagetable.rs      # PTE, PageTable, Satp
|   |   |           +-- mmu_init.rs       # Page table alloc, mapping
|   |   |           +-- mm_ops.rs         # COW, mmap, fork, user AS
|   |   |           +-- page_fault.rs     # Demand paging, stack expand
|   |   |           +-- exception.rs      # do_page_fault, fixup table
|   |   |           +-- fixmap.rs         # Early device mappings
|   |   |           +-- asid.rs           # ASID management
|   |   |
|   |   +-- syscall/      # System call dispatch
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- dispatch.rs  # System call dispatcher
|   |   |   +-- file.rs      # File system calls
|   |   |   +-- process.rs   # Process system calls
|   |   |   +-- memory.rs    # Memory system calls
|   |   |   +-- sched.rs     # Scheduler system calls
|   |   |   +-- signal.rs    # Signal system calls
|   |   |   +-- network.rs   # Network system calls
|   |   |   +-- io.rs        # I/O system calls
|   |   |   +-- time.rs      # Time system calls
|   |   |   +-- misc.rs      # Other system calls
|   |   |
|   |   +-- drivers/      # Device drivers
|   |   |   +-- mod.rs       # Driver module export
|   |   |   +-- intc/        # Interrupt controller
|   |   |   |   +-- mod.rs
|   |   |   |   +-- plic.rs     # RISC-V PLIC driver
|   |   |   |   +-- clint.rs    # RISC-V CLINT driver
|   |   |   +-- timer/       # Timer driver
|   |   |   |   +-- mod.rs
|   |   |   |   +-- riscv64.rs  # RISC-V timer
|   |   |   +-- virtio/      # VirtIO framework
|   |   |   |   +-- mod.rs      # VirtIO module
|   |   |   |   +-- queue.rs    # VirtQueue implementation
|   |   |   |   +-- probe.rs    # Device probing
|   |   |   |   +-- offset.rs   # Register offset definitions
|   |   |   |   +-- virtio_pci.rs # PCI transport layer
|   |   |   +-- blkdev/      # Block devices
|   |   |   |   +-- mod.rs      # VirtIO-blk driver
|   |   |   +-- input/       # Input devices
|   |   |   |   +-- mod.rs
|   |   |   |   +-- evdev.rs    # evdev driver
|   |   |   |   +-- event.rs    # Input event definitions
|   |   |   |   +-- ps2.rs      # PS/2 keyboard/mouse
|   |   |   |   +-- virtio_input.rs # VirtIO input device
|   |   |   +-- net/         # Network devices
|   |   |   |   +-- mod.rs
|   |   |   |   +-- space.rs    # Network device base class
|   |   |   |   +-- loopback.rs # Loopback device
|   |   |   |   +-- virtio_net.rs # VirtIO-net driver
|   |   |   +-- gpu/         # GPU/display devices
|   |   |   |   +-- mod.rs
|   |   |   |   +-- framebuffer.rs # Framebuffer core
|   |   |   |   +-- fb_simple.rs   # Simple framebuffer driver
|   |   |   |   +-- fbdev.rs       # fbdev device interface
|   |   |   |   +-- virtio_gpu.rs  # VirtIO-GPU driver
|   |   |   |   +-- virtio_cmd.rs  # GPU command handling
|   |   |   +-- pci/         # PCI bus
|   |   |       +-- mod.rs      # PCI enumeration and drivers
|   |   |
|   |   +-- mm/           # Memory management
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- page.rs      # Physical/virtual address types
|   |   |   +-- page_desc.rs # Page descriptor (struct Page, 64B)
|   |   |   +-- page_alloc.rs # Page allocation API (buddy + zone)
|   |   |   +-- zone.rs      # Zone allocator (embedded buddy)
|   |   |   +-- pglist.rs    # NUMA pglist data
|   |   |   +-- memblock.rs  # Early boot memory allocator
|   |   |   +-- layout.rs    # Kernel physical memory layout
|   |   |   +-- vmemmap.rs   # Virtual page descriptor mapping
|   |   |   +-- mm_struct.rs # Process address space descriptor
|   |   |   +-- vma.rs       # Virtual Memory Area management
|   |   |   +-- pagemap.rs   # Page mapping types (Perm, MapError)
|   |   |   +-- buddy_allocator.rs # Standalone buddy for kernel heap
|   |   |   +-- slab.rs      # Slab allocator (kmalloc/kfree)
|   |   |   +-- pcp.rs       # Per-CPU page cache
|   |   |   +-- meminfo.rs   # Memory info (/proc/meminfo)
|   |   |   +-- rmap.rs      # Reverse mapping
|   |   |   +-- hugepage.rs  # Huge page support
|   |   |   +-- allocator.rs # Legacy heap allocator wrapper
|   |   |
|   |   +-- fs/           # File system
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- vfs.rs       # Virtual file system
|   |   |   +-- file.rs      # File descriptor
|   |   |   +-- inode.rs     # Inode cache
|   |   |   +-- dentry.rs    # Directory entry cache
|   |   |   +-- buffer.rs    # Block cache
|   |   |   +-- bio.rs       # Block I/O layer
|   |   |   +-- mount.rs     # Mount management
|   |   |   +-- superblock.rs # Superblock
|   |   |   +-- path.rs      # Path resolution
|   |   |   +-- stat.rs      # File status structure
|   |   |   +-- rootfs.rs    # Root file system
|   |   |   +-- pipe.rs      # Pipe implementation
|   |   |   +-- char_dev.rs  # Character device
|   |   |   +-- elf.rs       # ELF loader
|   |   |   +-- dev_t.rs     # Device number definitions
|   |   |   +-- fs_struct.rs # Filesystem info struct
|   |   |   +-- io_completion.rs # I/O completion primitive
|   |   |   +-- readahead.rs # Readahead logic
|   |   |   +-- page_cache.rs # Page cache
|   |   |   +-- permission.rs # Permission checks
|   |   |   +-- devfs/       # devfs file system
|   |   |   |   +-- mod.rs
|   |   |   |   +-- registry.rs # Device registry
|   |   |   +-- procfs/      # procfs file system
|   |   |   |   +-- mod.rs
|   |   |   |   +-- meminfo.rs  # /proc/meminfo
|   |   |   |   +-- cpuinfo.rs  # /proc/cpuinfo
|   |   |   |   +-- cmdline.rs  # /proc/cmdline
|   |   |   |   +-- interrupts.rs # /proc/interrupts
|   |   |   |   +-- loadavg.rs  # /proc/loadavg
|   |   |   |   +-- mounts.rs   # /proc/mounts
|   |   |   |   +-- pid.rs      # /proc/[pid]
|   |   |   |   +-- self_proc.rs # /proc/self
|   |   |   |   +-- uptime.rs   # /proc/uptime
|   |   |   |   +-- version.rs  # /proc/version
|   |   |   +-- ext4/        # ext4 file system
|   |   |   |   +-- mod.rs      # ext4 main module
|   |   |   |   +-- superblock.rs # Superblock parsing
|   |   |   |   +-- inode.rs    # Inode structure
|   |   |   |   +-- file.rs     # File operations
|   |   |   |   +-- dir.rs      # Directory operations
|   |   |   |   +-- allocator.rs # Block/Inode allocator
|   |   |   |   +-- extent.rs   # Extent tree
|   |   |   |   +-- indirect.rs # Indirect blocks
|   |   |   |   +-- namei.rs    # Path name lookup
|   |   |   +-- jbd2/        # JBD2 journaling
|   |   |       +-- mod.rs
|   |   |       +-- journal.rs
|   |   |       +-- transaction.rs
|   |   |       +-- commit.rs
|   |   |       +-- checkpoint.rs
|   |   |       +-- recovery.rs
|   |   |       +-- revoke.rs
|   |   |       +-- types.rs
|   |   |
|   |   +-- net/          # Network protocol stack
|   |   |   +-- mod.rs       # Network module
|   |   |   +-- buffer.rs    # SkBuff (network buffer)
|   |   |   +-- socket.rs    # Socket layer
|   |   |   +-- ethernet.rs  # Ethernet layer
|   |   |   +-- arp.rs       # ARP protocol
|   |   |   +-- ipv4/        # IPv4 protocol
|   |   |   |   +-- mod.rs
|   |   |   |   +-- route.rs   # Routing table
|   |   |   |   +-- checksum.rs # IP checksum
|   |   |   +-- tcp.rs       # TCP protocol
|   |   |   +-- tcp_timer.rs # TCP timer
|   |   |   +-- udp.rs       # UDP protocol
|   |   |
|   |   +-- process/      # Process management
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- task.rs      # Task control block
|   |   |   +-- fork.rs      # fork/clone implementation
|   |   |   +-- exec.rs      # execve implementation
|   |   |   +-- exit.rs      # Process exit and reaping
|   |   |   +-- kthread.rs   # Kernel threads
|   |   |   +-- pid.rs       # PID management
|   |   |   +-- pid_hash.rs  # PID hash table
|   |   |   +-- wait.rs      # wait4 system call
|   |   |
|   |   +-- sched/        # Process scheduling
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- sched.rs     # Scheduler core
|   |   |   +-- fair.rs      # CFS (Completely Fair Scheduler)
|   |   |   +-- rt.rs        # Real-time scheduler
|   |   |   +-- deadline.rs  # Deadline scheduler
|   |   |   +-- idle.rs      # Idle task
|   |   |   +-- stop_task.rs # Task stop (SIGSTOP)
|   |   |   +-- class.rs     # Scheduling class abstraction
|   |   |
|   |   +-- sync/         # Synchronization primitives
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- spinlock.rs  # Spinlock with preempt/IRQ/BH variants
|   |   |   +-- rwlock.rs    # RwSpinlock
|   |   |   +-- mutex.rs     # Mutex lock
|   |   |   +-- semaphore.rs # Semaphore
|   |   |   +-- condvar.rs   # Condition variable
|   |   |   +-- futex.rs     # Fast Userspace Mutex
|   |   |   +-- seqlock.rs   # Sequence lock (read-mostly data)
|   |   |   +-- rcu.rs       # Tiny RCU (non-preemptible)
|   |   |
|   |   +-- ipc/          # Inter-process communication
|   |   |   +-- mod.rs       # Module export, init()
|   |   |   +-- util.rs      # IPC IDs registry, permissions, constants
|   |   |   +-- sysv_sem.rs  # System V semaphores (semget/semctl/semop)
|   |   |   +-- sysv_msg.rs  # System V message queues (msgget/msgctl/msgsnd/msgrcv)
|   |   |   +-- sysv_shm.rs  # System V shared memory (shmget/shmctl/shmat/shmdt)
|   |   |   +-- posix_mq.rs  # POSIX message queues (mq_open/send/receive/notify)
|   |   |
|   |   +-- interrupt/    # Interrupt subsystem
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- irqchip.rs   # IRQ chip abstraction (IrqChip)
|   |   |   +-- irqdesc.rs   # IRQ descriptor and action management
|   |   |   +-- domain.rs    # IRQ domain (hwirq→virq mapping)
|   |   |   +-- softirq.rs   # Softirq bottom-half mechanism
|   |   |   +-- tasklet.rs   # Tasklet (deferred work)
|   |   |   +-- ksoftirqd.rs # Per-CPU softirq daemon
|   |   |   +-- preempt.rs   # Preempt count and context tracking
|   |   |
|   |   +-- dfx/          # Diagnostics and debugging
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- backtrace.rs # Stack backtrace
|   |   |   +-- bug.rs       # BUG/WARN macros
|   |   |   +-- hexdump.rs   # Hex dump utility
|   |   |   +-- hung_task.rs # Hung task detector
|   |   |   +-- sbi_debug.rs # SBI debug output
|   |   |   +-- softlockup.rs # Soft lockup detector
|   |   |   +-- taint.rs     # Kernel taint tracking
|   |   |
|   |   +-- tests/        # Unit tests (60 test files)
|   |   |   +-- mod.rs       # Test framework entry
|   |   |
|   |   |   |  # Memory tests
|   |   |   +-- heap_allocator.rs
|   |   |   +-- page_allocator.rs
|   |   |   +-- standard_alloc.rs
|   |   |   +-- mem_mmap.rs
|   |   |   +-- mem_cow.rs
|   |   |
|   |   |   |  # Process/scheduling tests
|   |   |   +-- fork.rs
|   |   |   +-- getpid.rs
|   |   |   +-- wait4.rs
|   |   |   +-- process_tree.rs
|   |   |   +-- scheduler.rs
|   |   |   +-- preemptive_scheduler.rs
|   |   |   +-- sleep_wakeup.rs
|   |   |   +-- smp.rs
|   |   |   +-- smp_schedule.rs
|   |   |   +-- execve.rs
|   |   |   +-- boundary.rs
|   |   |   +-- listhead.rs
|   |   |   +-- quick.rs
|   |   |
|   |   |   |  # File system tests
|   |   |   +-- file_open.rs
|   |   |   +-- file_flags.rs
|   |   |   +-- fdtable.rs
|   |   |   +-- path.rs
|   |   |   +-- dcache.rs
|   |   |   +-- icache.rs
|   |   |   +-- link.rs
|   |   |   +-- fcntl.rs
|   |   |   +-- fstat.rs
|   |   |   +-- mkdir_unlink.rs
|   |   |   +-- ext4_allocator.rs
|   |   |   +-- ext4_file_write.rs
|   |   |   +-- ext4_indirect_blocks.rs
|   |   |
|   |   |   |  # IPC tests
|   |   |   +-- pipe2.rs
|   |   |   +-- ipc_poll.rs
|   |   |   +-- ipc_epoll.rs
|   |   |   +-- ipc_eventfd.rs
|   |   |
|   |   |   |  # Signal tests
|   |   |   +-- signal.rs
|   |   |   +-- signal_procmask.rs
|   |   |
|   |   |   |  # Network tests
|   |   |   +-- network.rs
|   |   |   +-- tcp_handshake.rs
|   |   |
|   |   |   |  # Driver tests
|   |   |   +-- virtio_queue.rs
|   |   |   +-- virtio_net.rs
|   |   |   +-- framebuffer.rs
|   |   |
|   |   |   |  # System call tests
|   |   |   +-- syscall_file.rs
|   |   |   +-- syscall_memory.rs
|   |   |   +-- syscall_process.rs
|   |   |   +-- syscall_sched.rs
|   |   |   +-- syscall_signal.rs
|   |   |   +-- syscall_network.rs
|   |   |   +-- syscall_io.rs
|   |   |   +-- syscall_time.rs
|   |   |   +-- syscall_misc.rs
|   |   |   +-- user_syscall.rs
|   |   |
|   |   +-- console.rs    # Console (UART)
|   |   +-- config.rs     # Auto-generated config (do not edit manually)
|   |   +-- main.rs       # Kernel entry (rust_main)
|   |   +-- init.rs       # Init process creation
|   |   +-- printk.rs     # Printk with log levels and ring buffer
|   |   +-- print.rs      # Print macros
|   |   +-- errno.rs      # Error code definitions
|   |   +-- signal.rs     # Signal handling
|   |   +-- list.rs       # Linked list primitives
|   |   +-- sbi.rs        # SBI call interface
|   |   +-- cmdline.rs    # DTB command line parsing
|
|   +-- verify/             # Formal verification (proptest + Kani)
|   |   +-- Cargo.toml      # rux-verify crate (std, x86_64)
|   |   +-- README.md       # Verification test documentation
|   |   +-- src/            # Self-contained test copies
|   |       +-- mm/         # MM verification (buddy, VMA, zone, swap, etc.)
|   |       +-- sync/       # Sync verification (spinlock, seqlock, futex)
|   |       +-- arch/       # Architecture verification (pagetable, pt_regs)
|   |       +-- net/        # Network verification (TCP, route, checksum)
|   |       +-- fs/         # Filesystem verification (inode, ext4, ELF, etc.)
|   |       +-- security/   # Security verification (capabilities)
|   |       +-- signal/     # Signal verification
|   |       +-- process/    # Process verification (PID allocator)
|   |       +-- sched/      # Scheduler verification (CFS, RT, deadline)
|   |       +-- interrupt/  # Interrupt verification (IRQ descriptor)
|   |       +-- drivers/    # Driver verification (PCI, VirtIO, netdev)
|   |       +-- ipc/        # IPC verification
|   |       +-- errno/      # Error code verification
|   |
|   +-- verify/spin/         # SPIN/Promela concurrency models
|       +-- Makefile          # SPIN runner
|       +-- futex_wait_wake.pml  # Futex wait/wake model
|       +-- lock_ordering.pml    # Lock ordering deadlock model
|       +-- interrupt_preempt.pml # Preempt count balance model
|       +-- sched_enqueue_dequeue.pml # Scheduler consistency model
|   |
|   +-- build.rs          # Build script (generates config.rs)
|   +-- Cargo.toml        # Kernel crate configuration
|
+-- .cargo/                 # Cargo configuration
|   +-- config.toml       # Cargo tool configuration (code-model=medany)
|
+-- target/                 # Build output (git ignored)
|   +-- riscv64gc-unknown-none-elf/
|       +-- debug/        # Debug build
|       +-- release/      # Release build
|
+-- Kernel.toml            # Kernel configuration file
+-- Cargo.toml             # Workspace configuration
+-- Cargo.lock             # Dependency lock
+-- Makefile               # Project root Makefile
+-- README.md              # Project description
+-- CLAUDE.md              # AI assistant development guide
+-- LICENSE                # License (MIT)
+-- .gitignore             # Git ignore rules
```
---

## Directory Descriptions

### userspace/ - Userspace Programs

Userspace program directory, containing Shell, GUI applications, test programs, and utilities.

### rootfs Directory Structure

Internal structure of rootfs image (`test/rootfs.img`):

```
/
+-- bin/                # Basic commands
|   +-- sh              # mrsh (POSIX shell)
|   +-- toybox          # Toybox (200+ commands)
|   +-- ls -> toybox    # Common command symlinks
|   +-- cat -> toybox
|   +-- echo -> toybox
|   +-- ...
|
+-- app/                # GUI applications
|   +-- desktop         # Desktop environment
|   +-- calculator      # Calculator
|   +-- clock           # Clock
|   +-- vshell          # Visual Shell
|
+-- test/               # Test programs
|   +-- smoke_test      # Smoke tests (15 tests)
|   +-- linux-ltp/      # Official LTP tests (1,838 tests)
|       +-- testcases/bin/  # LTP test binaries
|       +-- run_ltp.sh
|       +-- run_quick.sh
|       +-- run_syscalls.sh
|
+-- dev/                # Device files
|   +-- console
|   +-- null
|   +-- zero
|   +-- input/
|   |   +-- event0      # Input device
|   +-- fb0             # Framebuffer
|
+-- proc/               # procfs mount point
+-- tmp/                # Temporary files
+-- var/                # Variable data
+-- etc/                # Configuration files
+-- lib/                # Library files
```

### kernel/ - Kernel Source Code

Core kernel source code, organized by functional modules.

#### kernel/src/syscall/ - System Call Dispatch

System call dispatch module, routing system calls to specific implementations:

| File | Function | System Calls |
|------|----------|--------------|
| **dispatch.rs** | System call dispatcher | All system call entry |
| **file.rs** | File system calls | open, close, read, write, lseek, fstat, mkdir, unlink, chdir, getcwd, etc. |
| **process.rs** | Process system calls | execve, wait4, exit, getpid, getppid, clone, etc. |
| **memory.rs** | Memory system calls | brk, mmap, munmap, mprotect, etc. |
| **sched.rs** | Scheduler system calls | sched_yield, nice, etc. |
| **signal.rs** | Signal system calls | kill, signal, sigprocmask, rt_sigreturn, etc. |
| **network.rs** | Network system calls | socket, bind, listen, accept, connect, send, recv, etc. |
| **io.rs** | I/O system calls | poll, select, epoll, eventfd, etc. |
| **time.rs** | Time system calls | time, gettimeofday, nanosleep, clock_gettime, etc. |
| **misc.rs** | Other system calls | uname, sysinfo, set_tid_address, etc. |

#### kernel/src/arch/riscv64/ - RISC-V Architecture

**Only RISC-V 64-bit (RV64GC) is supported.**

| File | Function | Code Lines |
|------|----------|------------|
| **boot.S** | MMU trampoline, boot code (assembly) | 441 |
| **trap.S** | Exception vector table, ret_from_fork (assembly) | 845 |
| **uaccess.S** | User space access fixup (assembly) | 360 |
| **linker.ld** | Linker script (VMA at KERNEL_LINK_ADDR) | 76 |
| **pt_regs.rs** | PtRegs (trap frame) structure | 440 |
| **context.rs** | Context switching (__switch_to) | 256 |
| **thread.rs** | Thread structure (callee-saved regs) | 379 |
| **process.rs** | User mode management (start_thread) | 300 |
| **trap.rs** | Exception handling, signal dispatch | 578 |
| **smp.rs** | Multi-core support, per-CPU stacks | 277 |
| **ipi.rs** | Inter-processor interrupts | 417 |
| **cpu.rs** | CPU operations | 136 |
| **uaccess.rs** | User space access (copy_to/from_user) | 379 |
| **boot.rs** | Boot initialization helpers | 31 |
| **mod.rs** | Architecture module export | 116 |

#### kernel/src/arch/riscv64/mm/ - RISC-V Memory Management

| File | Function | Code Lines |
|------|----------|------------|
| **mm_ops.rs** | COW, mmap, fork, user address space | 1131 |
| **mmu_init.rs** | Page table alloc, mapping, linear mapping | 857 |
| **memory_layout.rs** | Sv39 constants, KernelMapping, address types | 465 |
| **page_fault.rs** | Demand paging, stack expansion | 313 |
| **exception.rs** | do_page_fault, exception table, send_signal | 309 |
| **fixmap.rs** | Early device mappings (UART) | 238 |
| **asid.rs** | ASID allocation, TLB flush | 194 |
| **pagetable.rs** | PTE, PageTable, Satp structures | 212 |
| **mod.rs** | MM module re-exports | 119 |

#### kernel/src/mm/ - Memory Management

| File | Function | Code Lines |
|------|----------|------------|
| **page_alloc.rs** | Page allocation API (buddy + zone) | 586 |
| **page_desc.rs** | Page descriptor (struct Page, 64B) | 833 |
| **mm_struct.rs** | Process address space (MmStruct) | 773 |
| **vma.rs** | Virtual Memory Area (VMA, VmaManager) | 761 |
| **zone.rs** | Zone allocator (embedded buddy) | 632 |
| **slab.rs** | Slab allocator (kmalloc/kfree) | 670 |
| **memblock.rs** | Early boot memory allocator | 596 |
| **buddy_allocator.rs** | Standalone buddy (kernel heap) | 495 |
| **pcp.rs** | Per-CPU page cache | 371 |
| **pglist.rs** | NUMA pglist data | 325 |
| **rmap.rs** | Reverse mapping | 226 |
| **hugepage.rs** | Huge page support | 300 |
| **meminfo.rs** | /proc/meminfo | 258 |
| **layout.rs** | Kernel physical memory layout | 289 |
| **page.rs** | Physical/virtual address types | 149 |
| **vmemmap.rs** | Virtual page descriptor mapping | 185 |
| **pagemap.rs** | Page mapping types | 68 |
| **allocator.rs** | Legacy heap allocator wrapper | 14 |
| **mod.rs** | Module re-exports | 104 |

---

## Linux LTP Test Suite

The official LTP (Linux Test Project) test suite built with musl libc for comprehensive kernel compatibility testing.

### Compilation Statistics

| Category | Compiled | Expected | Rate |
|----------|----------|----------|------|
| **Total** | **1,838** | 1,826 | **101%** |
| Syscall tests | 1,378 | 1,367 | 101% |
| Memory tests | 108 | 108 | 100% |
| Containers | 46 | 46 | 100% |
| Controllers | 20 | 39 | 51% |
| Filesystem tests | 29 | 29 | 100% |
| Security tests | 24 | 24 | 100% |
| Scheduler tests | 23 | 23 | 100% |
| IO tests | 19 | 19 | 100% |

### Running Tests

In the Rux system:

```bash
/test/linux-ltp/run_quick.sh      # Quick test suite
/test/linux-ltp/run_syscalls.sh   # Syscall tests
/test/linux-ltp/run_ltp.sh        # Full LTP suite
```

### LTP Version

Current version: 20240524

---

## Usage Guide

### Compilation

```bash
make build    # Compile kernel
make user     # Compile user programs
make rootfs   # Create rootfs image
```

### Running

```bash
make run      # Run kernel (default shell)
make gui      # Run GUI
```

### Testing

```bash
make test    # Run kernel unit tests
make verify  # Run formal verification (sync check + proptest)
```

---

## Notes

1. **config.rs is auto-generated** - Do not manually edit `kernel/src/config.rs`, it is automatically generated by `kernel/build.rs` based on `Kernel.toml`.

2. **Platform Limitations** - Currently only supports RISC-V 64-bit architecture.

3. **System Call Compatibility** - Uses Linux system call numbers, fully POSIX/ABI compatible.

4. **Module Export** - When adding new modules, ensure proper export of required interfaces in the parent module's `mod.rs`.

5. **User Programs** - Use musl libc static linking, compatible with kernel ABI.

6. **Code Model** - `.cargo/config.toml` uses `code-model = "medany"` for PC-relative addressing (required for VMA/LMA linking).

---

**Document Version**: v12.0
**Last Updated**: 2026-04-09
