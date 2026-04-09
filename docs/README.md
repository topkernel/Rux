# Rux OS Documentation Center

Welcome to the Rux operating system kernel documentation center!

## 📚 Quick Navigation

### 🚀 Getting Started
- **[Getting Started Guide](guides/getting-started.md)** - Up and running with Rux OS in 5 minutes
- **[Configuration System](guides/configuration.md)** - menuconfig and build options
- **[Development Workflow](guides/development.md)** - Contributing code and development standards

### 🏗️ Architecture Design
- **[Design Principles](architecture/design.md)** - POSIX compatibility and Linux ABI alignment
- **[Code Structure](architecture/structure.md)** - Source code organization and module division
- **[RISC-V Architecture](architecture/riscv64.md)** - RV64GC support details
- **[Boot Process](architecture/boot.md)** - From OpenSBI to kernel boot
- **[Memory Management](architecture/memory.md)** - Physical memory, virtual memory, allocator design

### 💻 Development Guides
- **[Testing Guide](guides/testing.md)** - Unit tests + proptest + Kani proofs + SPIN models
- **[Formal Verification](development/formal-verification.md)** - 4-layer verification strategy (proptest + Kani + SPIN + Miri)
- **[Lock Hierarchy](architecture/lock-ordering.md)** - Kernel lock ordering and nesting rules

### 📊 Project Progress
- **[Roadmap](progress/roadmap.md)** - Phase planning and current status (Phase 51)
- **[Quick Reference](progress/quickref.md)** - Common commands and API cheat sheet
- **[Changelog](progress/changelog.md)** - Version history and update records

### 📦 Historical Documents
- **[Debug Archives](archive/README.md)** - Historical debug records (archived)
- **[Code Review Records](archive/code-review.md)** - Known issues and fix records

## 🎯 Project Overview

**Rux** is a Linux-like operating system kernel entirely written in Rust, aiming for **100% POSIX compatible** and **Linux ABI compatible**.

### Core Features

- ✅ **Pure Rust Implementation** (except for necessary platform assembly)
- ✅ **RISC-V 64-bit Architecture** (only supported architecture)
- ✅ **Complete Process Management** (fork, execve, wait4, signal handling, COW)
- ✅ **CFS Scheduler** (Linux-like fair scheduler)
- ✅ **Virtual Memory** (Sv39 3-level page table, Buddy allocator, Slab allocator)
- ✅ **SMP Multi-core** (4-core concurrency, IPI, load balancing)
- ✅ **VFS Filesystem** (ext4, ramfs, procfs, devfs)
- ✅ **Network Stack** (TCP/UDP/IPv4/ARP)
- ✅ **Device Drivers** (VirtIO-blk/net/gpu/input)
- ✅ **JBD2 Journaling** (ext4 journaling with crash recovery)
- ✅ **Security** (POSIX capabilities, LSM framework)
- ✅ **IO_uring** (async I/O interface)
- ✅ **POSIX Timers** (timer_create, timerfd, setitimer)

### Development Status

**Current Version**: v0.1.0 (Phase 51 completed)

**Latest Updates**: 2026-04-09
- ✅ **Memory Compaction** - High-order page allocation via migration and compaction
- ✅ **SeqLock** - Sequence lock for read-mostly concurrent access
- ✅ **RCU PID Hash** - RCU-protected PID hash table for scalable lookup
- ✅ **Tiny RCU** - Non-preemptible RCU, per-CPU callback lists, softirq-driven grace periods
- ✅ **Formal Verification** - 157 Kani proofs + 4 SPIN models + 1088 proptest cases + Miri CI
- ✅ **348 System Calls** - 88% Linux syscall coverage
- ✅ **60 Kernel Unit Tests** + **25 mini-ltp Tests** + **15 Smoke Tests**

**Code Statistics**: ~102,400 lines of code, 278 source files

See [Changelog](progress/changelog.md) for details

## 🤖 AI-Assisted Development

This project uses **Claude Code + Opus4.6/GLM5.1/Minimax2.7** AI-assisted development to explore AI applications in OS kernel development.

- Development Tool: [Claude Code CLI](https://claude.ai/code)
- External interfaces follow POSIX standards and maintain 100% Linux ABI compatibility
- Developers are responsible for reviewing and testing all AI-generated code

See [CLAUDE.md](../CLAUDE.md) for details

## 📖 Documentation Reading Paths

### If You Are a New Developer
1. Read [Getting Started Guide](guides/getting-started.md)
2. Understand [Design Principles](architecture/design.md)
3. Check [Code Structure](architecture/structure.md)
4. Follow [Development Workflow](guides/development.md)

### If You Want to Contribute Code
1. Read [Roadmap](progress/roadmap.md) to understand pending tasks
2. Check [Code Review Records](archive/code-review.md) to avoid known issues
3. Read [Development Workflow](guides/development.md) for contribution guidelines
4. Check [Testing Guide](guides/testing.md) to learn testing methods

### If You Want to Deeply Understand Architecture
1. Read [RISC-V Architecture Documentation](architecture/riscv64.md)
2. Study [Boot Process](architecture/boot.md)
3. Read [Memory Management Design](architecture/memory.md)
4. Check [Quick Reference](progress/quickref.md)
5. View [Archived Documents](archive/README.md) for historical debugging processes

## 📁 Documentation Directory Structure

```
docs/
├── README.md              # This file
├── architecture/          # Architecture design documents
│   ├── design.md          # Design principles
│   ├── structure.md       # Code structure
│   ├── riscv64.md         # RISC-V architecture
│   ├── boot.md            # Boot process
│   ├── memory.md          # Memory management
│   ├── kernel-lock.md     # Kernel locking design
│   └── lock-ordering.md   # Lock hierarchy documentation
├── guides/                # Development guides
│   ├── getting-started.md # Getting started
│   ├── configuration.md   # Configuration system
│   ├── development.md     # Development workflow
│   └── testing.md         # Testing guide
├── progress/              # Project progress
│   ├── roadmap.md         # Development roadmap
│   ├── quickref.md        # Quick reference
│   └── changelog.md       # Changelog
├── development/           # Development records
│   ├── formal-verification.md  # Formal verification design
│   ├── compaction-design.md    # Memory compaction design
│   └── fork-exec-debug-report.md  # Fork+Exec debug report
└── archive/               # Historical document archives
    ├── README.md          # Archive index
    ├── code-review.md     # Code review records
    └── ...                # Other historical documents
```

## 🔍 Search Tips

- Search by Phase: Roadmap uses Phase numbers to organize development tasks
- Search by Module: Code structure document organized by subsystem
- Search by Feature: Testing guide categorized by feature module

## 📞 Getting Help

- **Issue Feedback**: [GitHub Issues](https://github.com/topkernel/rux/issues)
- **Code Review**: Check [Code Review Records](archive/code-review.md)
- **Development Discussion**: Refer to [Development Workflow](guides/development.md)

---

**Note**: This project is primarily for learning and research purposes and is not suitable for production environments.

Last updated: 2026-04-09
