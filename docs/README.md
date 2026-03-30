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
- **[Memory Management](architecture/memory.md)** - Physical memory, virtual memory, allocator design 🆕

### 💻 Development Guides
- **[Testing Guide](guides/testing.md)** - 53 kernel unit tests + 25 mini-ltp compatibility tests

### 📊 Project Progress
- **[Roadmap](progress/roadmap.md)** - Phase planning and current status (Phase 36)
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
- ✅ **GUI Desktop** (desktop environment, calculator, clock, visual shell)

### Development Status

**Current Version**: v0.1.0 (Phase 36 completed)

**Latest Updates**: 2026-03-30
- ✅ **Filesystem Refactoring** - Multi-lock bio cache, mballoc, async I/O
- ✅ **JBD2 Journaling** - Journaling for ext4
- ✅ **VFS** - Dentry/inode cache, mount table, page cache, read-ahead
- ✅ **Interrupt-driven VirtIO** - Interrupt-driven block I/O
- ✅ **New Syscalls** - symlinkat, statx, openat2
- ✅ **devfs Filesystem** - Device filesystem, replacing custom system calls
- ✅ **mini-ltp Tests** - 25 kernel compatibility tests
- ✅ **COW Improvements** - Copy-on-Write page table handling fixes
- ✅ **CFS Scheduler** - Completely fair scheduler implementation
- ✅ **GUI Desktop** - Desktop environment, calculator, clock apps
- ✅ **53 Kernel Unit Tests** + **25 mini-ltp Tests**

**Code Statistics**: ~79,600 lines of code, 227 source files

See [Changelog](progress/changelog.md) for details

## 🤖 AI-Assisted Development

This project uses **Claude Code + GLM5** AI-assisted development to explore AI applications in OS kernel development.

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
│   └── memory.md          # Memory management 🆕
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

Last updated: 2026-03-30
