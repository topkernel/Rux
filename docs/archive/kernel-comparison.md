# Operating System Kernel Architecture Comparison Analysis

This document provides a comparative analysis of the core design philosophies of ten operating systems:
- **chcore-lab-v2**: SJTU IPADS Lab, C microkernel
- **rCore**: Tsinghua University, Rust monolithic kernel, Linux compatible
- **zCore**: Tsinghua University, Rust microkernel, Zircon compatible
- **ArceOS**: Tsinghua University, Rust Unikernel, modular design
- **FTL-OS**: HIT (Shenzhen), Rust monolithic kernel, async coroutines
- **Theseus**: Rice University, Rust Cytokernel, single address space safe language OS
- **Asterinas**: DNXLabs, Rust Framekernel, Linux ABI compatible
- **Kerla**: Personal project, Rust monolithic kernel, Linux ABI compatible
- **RedLeaf**: UC Irvine, Rust microkernel, language safety isolation
- **Rux**: This project, Rust monolithic kernel, 100% Linux ABI compatible

---

## 1. Core Design Philosophy

| Kernel | Design Goal | Architecture | Language | Core Principle |
|--------|-------------|--------------|----------|----------------|
| **chcore-lab-v2** | Educational microkernel | Microkernel | C | Minimal kernel + Capability security |
| **rCore** | Educational + Linux compatible | Monolithic | Rust | Simplicity + async scheduling |
| **zCore** | Zircon research | Microkernel | Rust+async | Object-oriented + async concurrency |
| **ArceOS** | Modular Unikernel | Unikernel | Rust | Modular + compile-time config |
| **FTL-OS** | High-performance competition kernel | Monolithic+async | Rust | Performance first + stackless coroutine |
| **Theseus** | Safe language OS research | Cytokernel | Rust | Single address space + live evolution |
| **Asterinas** | Production Linux alternative | Framekernel | Rust | Memory safety + Linux ABI |
| **Kerla** | Linux compatible kernel | Monolithic | Rust | Linux ABI + simple implementation |
| **RedLeaf** | Language safety research | Microkernel | Rust | RRef + domain isolation |
| **Rux** | Linux ABI compatible | Monolithic | Rust | **100% POSIX/Linux ABI compatible** |

### Key Differences

**Rux's Unique Position**: Unlike the educational-focused rCore/zCore, Rux aims to **build a kernel that is fully Linux ABI compatible**. External interfaces must be identical to Linux, but internal implementation can leverage Rust's advantages for optimization.

### Emerging Architecture Analysis

**Theseus - Cytokernel**
- **Single Address Space (SAS)**: All code runs in one virtual address space
- **Single Privilege Level (SPL)**: All code runs in Ring 0 (kernel mode)
- **Language-defined Isolation**: Isolation through Rust type safety, not hardware
- **Live Evolution**: Any component can be replaced at runtime (Ship of Theseus paradox)
- **P.I.E. Principle**: Performance In Hardware, Isolation In Software

**Asterinas - Framekernel**
- **Minimal TCB**: Only OS framework (OSTD) can use unsafe Rust
- **Safe Rust Kernel**: All OS services must be written in safe Rust
- **Single Address Space**: Similar to monolithic kernel, all services in same address space
- **Zero-cost Abstraction**: API designed for zero overhead

**RedLeaf - Language-safe Microkernel**
- **RRef Mechanism**: Safe cross-domain references without serialization
- **Domain Isolation**: Isolation through Rust safety, not hardware
- **Proxy Domain**: Central communication hub, routes RPC calls

---

## 2. Memory Management Design Philosophy

### 2.1 Design Concept Comparison

| Kernel | Physical Memory Abstraction | Virtual Address Abstraction | Core Idea |
|--------|----------------------------|----------------------------|-----------|
| **chcore** | Buddy + Slab | vmspace + PMO | Layered isolation, PMO as memory object |
| **rCore** | Frame Allocator | MemorySet + MemoryArea | Strategy pattern, multiple mapping backends |
| **zCore** | VMO internal management | VMAR tree structure | VMO is first-class citizen, VMAR nestable |
| **ArceOS** | Two-level allocator | Linear mapping | Byte allocator + page allocator, compile-time selection |
| **FTL-OS** | Buddy + Per-CPU Cache | UserAreaHandler | Async memory management + RCU delayed free |
| **Theseus** | Frame/Page separation | MappedPages | Bijective mapping + compile-time safety |
| **Asterinas** | Frame + VmSpace | PageTable Cursor | Buddy allocation + concurrent page table traversal |
| **Kerla** | Page allocator | Vm + VmArea | Simple Linux style |
| **RedLeaf** | Buddy allocator | VSpace | RRef shared heap + domain ownership |
| **Rux** | Buddy + Slab | MemorySet | **Linux-style Buddy, zone division** |

### 2.2 Core Concept Analysis

**Theseus - MappedPages Pattern**
- `MappedPages`: Basic type for mapped memory with compile-time safety
- `AllocatedPages`/`AllocatedFrames`: Exclusively owned pages/frames
- Virtual to physical mapping must be bijective (one-to-one)
- Memory regions must be unmapped exactly once

**Asterinas - Frame + VmSpace**
- `Frame`: Physical page with reference counting
- `VmSpace`: User-space virtual address space management
- `PageTable Cursor`: Concurrent page table traversal and modification
- DMA and IOMMU support

**Kerla - Simple Linux Style**
- `Vm`: Page table + VMA list
- `VmAreaType`: Anonymous or File-backed
- Simple heap expansion and mmap implementation

**RedLeaf - RRef Shared Heap**
- RRef allocated on shared heap accessible to all domains
- Each allocation tracks domain ownership
- Borrow counting supports safe sharing

### 2.3 Insights for Rux

- ❌ Don't adopt zCore's VMAR tree structure (Linux uses linear)
- ❌ Don't adopt ArceOS/Theseus single address space model (Linux needs process isolation)
- ✅ External interfaces compatible with Linux's `vm_area_struct` and `mm_struct` layout
- ✅ Use same page flags as Linux
- ✅ FTL-OS's RCU mechanism can be referenced, external interfaces must be Linux compatible
- ✅ Asterinas's safe Rust practices can be learned from

---

## 3. Process and Scheduling Design Philosophy

### 3.1 Process Model Comparison

| Kernel | Process Abstraction | Thread Relationship | Resource Management |
|--------|---------------------|---------------------|---------------------|
| **chcore** | cap_group | Thread → cap_group | Capability table |
| **rCore** | Process | Thread → Process | fd_table, memory_set |
| **zCore** | Job → Process → Thread | Three-level tree | Handle table |
| **ArceOS** | No process concept | Task (kernel thread) | Single address space |
| **FTL-OS** | Process/AliveProcess split | Thread → Process | EventBus communication |
| **Theseus** | No process concept | Task | Single address space |
| **Asterinas** | Process | Thread → Process | POSIX compatible |
| **Kerla** | Process | Single-threaded | fd_table, signals |
| **RedLeaf** | Domain | Thread → Domain | RRef passing |
| **Rux** | Task (Process/Thread unified) | Thread → ThreadGroup | fd_table, fs_struct |

### 3.2 Scheduler Design

| Kernel | Scheduling Policy | Core Idea |
|--------|------------------|-----------|
| **chcore** | Pluggable (RR/PRIORITY) | Function pointer table, runtime switching |
| **rCore** | Cooperative + async | Thread is Future, scheduled by async runtime |
| **zCore** | Async scheduling | Deep integration with signal mechanism |
| **ArceOS** | FIFO/RR/CFS optional | Compile-time scheduling policy selection, Per-CPU runqueue |
| **FTL-OS** | async-task library | Stackless coroutine, context switch 4x faster than stacked |
| **Theseus** | Optional scheduler | Round-Robin, Priority, Epoch, etc. |
| **Asterinas** | CFS multi-level scheduling | Stop/RealTime/Fair/Idle four levels |
| **Kerla** | Round-Robin | Simple FIFO queue |
| **RedLeaf** | Priority scheduling | 16 priority levels, active/passive queues |
| **Rux** | CFS (Completely Fair Scheduler) | **Linux-style red-black tree + vruntime** |

### 3.3 Core Concept Analysis

**Theseus - Task Model**
- No traditional process concept, all code runs in single address space
- Task is scheduling unit, similar to kernel thread
- Minimalist design: state distributed across subsystems
- Supports live evolution: Task can restart at runtime

**Asterinas - POSIX Process Model**
- `Process`: Collection of threads sharing same user space
- `Thread`: Execution unit
- Complete POSIX support: process groups, sessions, job control
- Multi-level scheduling: Stop > RealTime > Fair > Idle

**Kerla - Simple Process Model**
- `Process`: Process control block
- `ProcessState`: Runnable, BlockedSignalable, ExitedWith
- Simple Round-Robin scheduler
- Complete signal handling and process group support

**RedLeaf - Domain Model**
- Domain: Isolated component module
- Thread can migrate between Domains
- Continuation stack supports unwinding/resuming
- SMP support: thread rebalancing

---

## 4. File System Design Philosophy

### 4.1 Architecture Comparison

| Kernel | VFS Design | File System Location | Mount Model |
|--------|------------|---------------------|-------------|
| **chcore** | User-space implementation | User-space service | Via IPC |
| **rCore** | rcore_fs VFS | Kernel-space | MountFS mount point |
| **zCore** | No native VFS | None | None |
| **ArceOS** | VfsOps/VfsNodeOps traits | Kernel-space module | Multi-mount support |
| **FTL-OS** | Linux-style VFS | Kernel-space | Dentry cache + RCU |
| **Theseus** | FsNode trait | Kernel-space crate | MemFS + FAT |
| **Asterinas** | Linux VFS | Kernel-space | ext2/ramfs/procfs/sysfs |
| **Kerla** | trait-based VFS | Kernel-space | initramfs/tmpfs/devfs/procfs |
| **RedLeaf** | VFS interface | Kernel-space | xv6 filesystem port |
| **Rux** | Linux VFS | Kernel-space | **Linux mount command compatible** |

### 4.2 Core Concept Analysis

**Theseus - Trait-based VFS**
- `FsNode`: Basic filesystem node trait
- `File`: FsNode + ByteReader + ByteWriter
- `Directory`: FsNode + insert/remove/list
- `MemFile`: Memory file backed by MappedPages

**Asterinas - Linux VFS**
- `FsResolver`: Filesystem resolver
- Supported filesystems: ext2, ramfs, procfs, devpts, sysfs, cgroupfs, exfat, overlayfs
- Path lookup supports symlink following

**Kerla - Simple VFS**
- `INode` enum: FileLike, Directory, Symlink
- `RootFs`: Mount namespace
- Supported filesystems: initramfs (CPIO), tmpfs, devfs, procfs

---

## 5. Driver Model Design Philosophy

### 5.1 Architecture Comparison

| Kernel | Driver Location | Hardware Abstraction | Core Idea |
|--------|----------------|---------------------|-----------|
| **chcore** | User-space | Minimal kernel interface | Driver crash doesn't affect system |
| **rCore** | Kernel-space | Driver trait | Simple and direct, performance first |
| **zCore** | Configurable | HAL trait | Supports bare-metal/user-space modes |
| **ArceOS** | Kernel-space module | BaseDriverOps trait | Static/dynamic dispatch optional |
| **FTL-OS** | Kernel-space | Block device + SPI SD | Direct hardware access |
| **Theseus** | Kernel-space crate | No special abstraction | Regular kernel crate |
| **Asterinas** | Kernel-space component | **Safe Rust** | Component system + VirtIO |
| **Kerla** | Kernel-space extension | Static registration | virtio-net extension |
| **RedLeaf** | Kernel-space domain | RedSys framework | Declarative resource access |
| **Rux** | Kernel-space | Linux style | **Device model consistent with Linux** |

### 5.2 Core Concept Analysis

**Theseus - No Special Abstraction**
- Drivers are regular kernel crates
- Supports: e1000, ixgbe, mlx5 (network), ATA (storage), PS/2 (input)
- Unified initialization through `device_manager`

**Asterinas - Safe Rust Driver**
- Drivers must be written in **safe Rust** (`#![deny(unsafe_code)]`)
- Component system: block, console, framebuffer, input, keyboard, network, virtio
- VirtIO framework: Block, Input, Network, Console, Socket
- PCI support + IOMMU DMA

**Kerla - Static Registration**
- Static device registration: `pub static DEV_FS: Once<Arc<DevFs>>`
- Supports: Tty (serial), NullFile, Ptmx (PTY)
- virtio-net loaded as kernel extension

**RedLeaf - RedSys Framework**
- Drivers must declare accessed resources: RawMemoryRegion, IOPort, IRQ
- Driver domains: ixgbe, nvme, pci, virtio_block, virtio_net, tpm
- TPM integration supports secure boot

---

## 6. IPC Mechanism Design Philosophy

### 6.1 Mechanism Comparison

| Mechanism | chcore | rCore | zCore | ArceOS | FTL-OS | Theseus | Asterinas | Kerla | RedLeaf | Rux |
|-----------|--------|-------|-------|--------|--------|---------|-----------|-------|---------|-----|
| Pipe | ❌ | ✅ | Socket | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Shared Memory | PMO | ✅ | VMO | Single AS | ✅ | Single AS | ✅ | ✅ | RRef | ✅ |
| Message Passing | Connection | ❌ | Channel | ❌ | EventBus | ❌ | ✅ | ❌ | Proxy | ❌ |
| Semaphore | Semaphore | ✅ | Futex | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Futex | ❌ | ✅ | ✅ | ❌ | ✅ | ❌ | ✅ | ✅ | ❌ | ✅ |

### 6.2 Core Concept Analysis

**Theseus - Single Address Space Communication**
- No traditional IPC, all code shares address space
- Synchronization through shared data structures and locks
- Direct function calls, no overhead

**Asterinas - POSIX IPC**
- pipe, FIFO, Unix socket
- Shared memory (shm/mmap)
- futex support
- Fully POSIX compliant

**Kerla - Simple IPC**
- Pipe
- Semaphore, condition variable, mutex
- Signal handling: rt_sigaction, rt_sigreturn, rt_sigprocmask

**RedLeaf - RRef + Proxy**
- **RRef**: Safe cross-domain reference without serialization
- **Proxy Domain**: Central communication hub, routes RPC
- Inter-domain communication through RRef ownership transfer

---

## 7. System Call Design Philosophy

### 7.1 Interface Comparison

| Kernel | System Call Style | Count | Compatibility |
|--------|------------------|-------|---------------|
| **chcore** | Capability-based | ~20 | Custom |
| **rCore** | Linux compatible | ~100 | Mostly compatible |
| **zCore** | Zircon + Linux | 200+ | Dual interface |
| **ArceOS** | No system calls | N/A | Library calls |
| **FTL-OS** | Linux compatible | ~100 | Mostly compatible |
| **Theseus** | No traditional syscalls | N/A | Library calls |
| **Asterinas** | Linux ABI | 200+ | **Fully compatible** |
| **Kerla** | Linux ABI | ~100 | Mostly compatible |
| **RedLeaf** | RPC-based | N/A | Domain calls |
| **Rux** | Linux ABI | **All** | **100% compatible** |

### 7.2 Core Concept Analysis

**Theseus - No System Calls**
- Single address space, applications linked with kernel as single binary
- Direct function calls, no context switch overhead
- Namespace isolation for different applications

**Asterinas - Fully Linux Compatible**
- 200+ system call implementations
- LTP (Linux Test Project) test verification
- Supports new syscalls like memfd_create, pidfd_open, epoll_pwait2

**Kerla - Linux ABI Compatible**
- Runs unmodified Linux ELF binaries
- Supports musl libc
- Complete signal handling and PTY/TTY support

**RedLeaf - RPC-based**
- Inter-domain communication via RPC
- RRef passing avoids serialization overhead
- More efficient than microkernel IPC

---

## 8. Synchronization Primitive Design Philosophy

### 8.1 Lock Implementation Comparison

| Kernel | Spinlock | Mutex | Design Idea |
|--------|----------|-------|-------------|
| **chcore** | Ticket Lock | Semaphore | Fairness first, manual implementation |
| **rCore** | spin crate | SleepLock | Use mature libraries, strategy pattern |
| **zCore** | Mutex | Mutex | Integrated with async |
| **ArceOS** | Spinlock | Mutex | Compile-time selection, SMP support |
| **FTL-OS** | SpinMutex/QSpinLock | SleepMutex | Complete lock suite, RCU support |
| **Theseus** | Spinlock | Mutex | Rust standard pattern |
| **Asterinas** | SpinLock | Mutex | Safe Rust implementation |
| **Kerla** | SpinLock | - | Simple implementation |
| **RedLeaf** | Spinlock | Mutex | In-domain synchronization |
| **Rux** | spin::Mutex | Mutex | **Linux-style atomic operations** |

### 8.2 FTL-OS Lock Suite

FTL-OS implements a complete lock suite worth referencing:
- **SpinMutex**: Short critical sections
- **RwSpinMutex**: Read-heavy scenarios
- **SleepMutex**: Long critical sections, async wait
- **SeqLock**: Read-intensive scenarios
- **QSpinLock**: Queue spinlock (Linux style)
- **RCU**: Read-Copy-Update, lock-free reads

---

## 9. Unique Features of Each Kernel

### 9.1 Theseus - Live Evolution

**Ship of Theseus Paradox**: If you replace every plank of a ship, is it still the same ship?
- Any component can be replaced at runtime
- Fine-grained dependency tracking (section level)
- Fault recovery: crate swap
- Multiple OS personalities: through namespaces

**Intralingual Design**:
- Transfer OS responsibilities to compiler
- Use language type system to provide OS guarantees
- Leverage Rust ownership model for resource management

### 9.2 Asterinas - Safe Rust Kernel

**Framekernel Architecture**:
- OS framework (OSTD): Encapsulates low-level unsafe code
- OS services (Kernel): **Must be written in safe Rust**
- Drivers also must use safe Rust

**Production-ready Goal**:
- x86-64 VM environment production ready in 2025
- SOSP'25 and USENIX ATC'25 papers
- Intel TDX support

### 9.3 Kerla - Simple Linux Compatible

**Features**:
- Runs unmodified Linux binaries
- Supports musl libc
- Complete PTY/TTY and job control
- smoltcp TCP/IP stack
- Firecracker microVM support
- Docker image support (experimental)

**Status**: Project archived, author moved to FTL project

### 9.4 RedLeaf - Language Safety Research

**RRef Mechanism**:
- Safe cross-domain reference
- Ownership transfer without serialization
- Borrow counting supports safe sharing

**Domain Isolation**:
- Isolation through Rust safety, not hardware
- More efficient than microkernel IPC
- Continuation stack supports fault recovery

**TPM Integration**:
- Domain measurement/fingerprinting
- Ed25519 signature verification
- Secure boot support

### 9.5 FTL-OS - High-performance Design

**Performance Optimization Techniques**:
1. **Stackless Coroutine**: Context switch 4x faster than stacked
2. **Per-CPU Cache**: Reduce lock contention
3. **RCU Mechanism**: Lock-free reads
4. **Non-blocking Filesystem Write**: Kernel sync thread async disk flush
5. **COW Optimization**: Complete fork COW support

**Competition Results**:
- Final Round Phase 1: 220 points (passed all tests)
- Final Round Phase 2: 117.7977 points, **First Place** (as of 2022.8.18)

### 9.6 ArceOS - Modular Design

**Compile-time Feature Selection**:
```bash
make A=apps/net/httpserver ARCH=riscv64 LOG=info SMP=4 run
```

**Module Structure**:
- `axhal`: Hardware abstraction layer
- `axconfig`: Platform configuration
- `axlog`: Logging
- `axalloc`: Memory allocation
- `axtask`: Task management
- `axsync`: Synchronization primitives
- `axdriver`: Device drivers
- `axfs`: File system
- `axnet`: Network stack
- `axdisplay`: Graphics display
- `axruntime`: Runtime library

---

## 10. Code Size Comparison

### 10.1 Total Code Lines

| Kernel | Kernel Code Lines | Language | Notes |
|--------|------------------|----------|-------|
| **Theseus** | ~78,700 lines | Rust | kernel/ directory |
| **Asterinas** | ~155,800 lines | Rust | kernel (117K) + ostd (38K) |
| **rCore** | ~25,000 lines | Rust | kernel/ directory |
| **zCore** | ~50,000 lines | Rust | zircon-object + linux-object + drivers |
| **ArceOS** | ~12,800 lines | Rust | modules/ directory |
| **FTL-OS** | ~37,000 lines | Rust | kernel (28K) + vfs (3K) + fat32 (6K) |
| **Kerla** | ~9,700 lines | Rust | kernel/ directory |
| **RedLeaf** | ~10,400 lines | Rust | kernel/ directory |
| **Rux** | ~54,700 lines | Rust | kernel/src/ directory |
| **chcore-lab-v2** | ~13,000 lines | C | kernel/ directory |

### 10.2 Module Code Size Detailed Comparison

#### Theseus (kernel: ~78,700 lines)
| Module | Lines | Function |
|--------|-------|----------|
| mod_mgmt | 4,495 | Module management |
| mlx_ethernet | 3,864 | Mellanox NIC driver |
| text_terminal | 3,155 | Terminal |
| memory | 2,596 | Memory management |
| ixgbe | 2,074 | Intel 10GbE driver |
| acpi | 1,998 | ACPI support |
| frame_allocator | 1,954 | Physical frame allocation |
| wasi_interpreter | 1,752 | WASI interpreter |
| task | 1,458 | Task management |
| crate_metadata | 1,430 | Crate metadata |

#### Asterinas (kernel+ostd: ~155,800 lines)
| Module | Lines | Function |
|--------|-------|----------|
| fs | 26,946 | File system |
| syscall | 13,449 | System calls |
| process | 11,628 | Process management |
| net | 10,601 | Network stack |
| vm | 4,066 | Virtual memory |
| util | 2,527 | Utility functions |
| device | 1,946 | Device management |
| sched | 1,507 | Scheduler |
| time | 1,473 | Time management |
| thread | 1,295 | Thread management |

#### rCore (kernel: ~25,000 lines)
| Module | Lines | Function |
|--------|-------|----------|
| arch | 6,414 | Architecture-specific |
| syscall | 5,060 | System calls |
| drivers | 4,834 | Drivers |
| fs | 1,813 | File system |
| process | 1,284 | Process management |
| lkm | 1,238 | Kernel modules |
| net | 1,193 | Network |
| sync | 865 | Synchronization primitives |
| rvm | 650 | Virtualization |
| signal | 410 | Signals |

#### FTL-OS (total: ~37,000 lines)
| Module | Lines | Function |
|--------|-------|----------|
| memory | 7,771 | Memory management |
| syscall | 3,492 | System calls |
| tools | 3,117 | Tools |
| process | 1,925 | Process management |
| drivers | 1,876 | Drivers |
| fs | 1,492 | File system |
| signal | 1,138 | Signals |
| fat32 | 5,720 | FAT32 file system |
| vfs | 3,156 | Virtual file system |

#### ArceOS (modules: ~12,800 lines)
| Module | Lines | Function |
|--------|-------|----------|
| axhal | 3,519 | Hardware abstraction layer |
| axnet | 2,834 | Network stack |
| axfs | 2,483 | File system |
| axtask | 1,464 | Task management |
| axdriver | 1,053 | Device drivers |
| axalloc | 366 | Memory allocation |
| axruntime | 361 | Runtime |
| axsync | 289 | Synchronization primitives |
| axlog | 230 | Logging |
| axconfig | 185 | Configuration |

#### Kerla (kernel: ~9,700 lines)
| Module | Lines | Function |
|--------|-------|----------|
| fs | 2,348 | File system |
| syscalls | 2,232 | System calls |
| process | 1,490 | Process management |
| net | 930 | Network |
| tty | 513 | TTY |
| arch | 363 | Architecture |
| mm | 296 | Memory management |

#### RedLeaf (kernel: ~10,400 lines)
| Module | Lines | Function |
|--------|-------|----------|
| interrupt | 1,704 | Interrupt handling |
| arch | 902 | Architecture |
| memory | 809 | Memory management |
| domain | 544 | Domain management |
| multibootv2 | 374 | Boot |
| redsys | 309 | Driver framework |
| console | 259 | Console |
| dev | 258 | Device |
| drivers | 223 | Drivers |

#### chcore-lab-v2 (kernel: ~13,000 lines)
| Module | Lines | Function |
|--------|-------|----------|
| include | 3,429 | Header files |
| arch | 2,724 | Architecture-specific |
| object | 2,220 | Kernel objects |
| mm | 1,492 | Memory management |
| lib | 1,087 | Utility library |
| tests | 842 | Tests |
| ipc | 573 | Inter-process communication |
| sched | 534 | Scheduler |
| syscall | 246 | System calls |
| irq | 215 | Interrupt handling |
| semaphore | 113 | Semaphore |

#### Rux (kernel: ~54,700 lines)
| Module | Lines | Function |
|--------|-------|----------|
| fs | 14,056 | File system |
| drivers | 7,981 | Drivers |
| tests | 7,376 | Test code |
| net | 5,177 | Network stack |
| syscall | 5,097 | System calls |
| arch | 5,097 | Architecture-specific |
| mm | 4,242 | Memory management |
| process | 2,426 | Process management |
| sched | 2,257 | Scheduler |
| sync | 1,147 | Synchronization primitives |

---

## 11. Rux Design Principles Summary

### 11.1 Must Follow

| Principle | Description |
|-----------|-------------|
| **Linux ABI Compatible** | System call numbers, structure layouts must be identical to Linux |
| **POSIX Standard** | Behavior must comply with POSIX specifications |
| **External Interface Consistent** | User-visible interface behavior must match Linux |

### 11.2 Designs Worth Learning From

| Source | Learnable Content |
|--------|-------------------|
| **rCore** | Rust kernel code organization, error handling patterns |
| **zCore** | Async architecture concepts |
| **ArceOS** | Modular design philosophy, compile-time configuration |
| **FTL-OS** | RCU mechanism, high-performance lock implementations, stackless coroutine concepts |
| **Asterinas** | Safe Rust practices, Framekernel architecture concepts |
| **Theseus** | Intralingual design philosophy, live evolution mechanism |
| **Kerla** | Simple kernel implementation |
| **RedLeaf** | RRef ownership transfer mechanism |
| **Linux** | External interface specifications (interface only, not internal implementation) |

> **Note**: Internal implementation can learn from any excellent design. Only external interfaces must be Linux compatible.

### 11.3 Design Freedom

| Aspect | Description |
|--------|-------------|
| **External Interface** | Must be fully compatible with Linux (system calls, data structures, file formats) |
| **Internal Implementation** | Complete freedom, can use any design method, algorithm, data structure |
| **Optimization Direction** | Can pursue better performance, cleaner code, better security |
| **Architecture Choice** | Can learn from any OS's excellent design, as long as it doesn't affect external compatibility |

> **Core Philosophy**: External interface is constraint, internal implementation is freedom. We only promise interface compatibility, not identical internal design.

---

## 12. Key Data Structure Reference Table

| Concept | Linux/Rux | rCore | zCore | ArceOS | Theseus | Asterinas | Kerla | RedLeaf |
|---------|-----------|-------|-------|--------|---------|-----------|-------|---------|
| Process | task_struct | Process | Process | None | None | Process | Process | Domain |
| Thread | task_struct | Thread | Thread | Task | Task | Thread | Single | Thread |
| Address Space | mm_struct | MemorySet | VMAR | Single | Single | VmSpace | Vm | VSpace |
| Memory Object | vm_area_struct | MemoryArea | VMO | MemRegion | MappedPages | Frame | VmArea | RRef |
| File Descriptor | fd_array | FdTable | Handle | None | None | FileTable | OpenedFileTable | fd |
| VFS inode | struct inode | INode trait | None | VfsNodeOps | FsNode | InodeHandle | INode | VFS trait |
| Sched Entity | sched_entity | Thread | Thread | TaskInner | Task | SchedAttr | PId | Thread |
| Mutex | mutex | Mutex | Mutex | Spinlock | Spinlock | Mutex | SpinLock | Mutex |

---

## 13. Architecture Support Comparison

| Kernel | riscv64 | aarch64 | x86_64 | Other |
|--------|---------|---------|--------|-------|
| **chcore** | ✅ | ✅ | ✅ | - |
| **rCore** | ✅ | ✅ | ✅ | - |
| **zCore** | ✅ | ✅ | ❌ | - |
| **ArceOS** | ✅ | ✅ | ✅ | - |
| **FTL-OS** | ✅ | ❌ | ❌ | HiFive Unmatched |
| **Theseus** | Experimental | ✅ | ✅ | - |
| **Asterinas** | ✅ | ❌ | ✅ | LoongArch64 |
| **Kerla** | ❌ | ❌ | ✅ | Firecracker |
| **RedLeaf** | ❌ | ❌ | ✅ | - |
| **Rux** | ✅ | ❌ | ❌ | - |

---

## 14. References

- **Linux Kernel Source**: https://elixir.bootlin.com/linux/latest/source/
- **POSIX Standard**: https://pubs.opengroup.org/onlinepubs/9699919799/
- **chcore-lab-v2**: https://github.com/SJTU-IPADS/chcore-lab-v2
- **rCore Tutorial**: https://rcore-os.cn/rCore-Tutorial-Book-v3/
- **zCore**: https://github.com/rcore-os/zCore
- **ArceOS**: https://github.com/rcore-os/arceos
- **FTL-OS**: 2022 National College Student Computer System Capability Competition Kernel Implementation Track Entry
- **Theseus**: https://github.com/theseus-os/Theseus
- **Asterinas**: https://github.com/asterinas/asterinas
- **Kerla**: https://github.com/nuta/kerla
- **RedLeaf**: https://github.com/marksantcroos/redleaf (OSDI '20)
