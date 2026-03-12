# Rux Design Principles

## Highest Principle (Absolutely Must Not Be Violated)

### **0. Complete POSIX/ABI Compatibility, No Innovation**

This is the **highest guiding principle** for Rux kernel development. All design and implementation decisions must adhere to this principle.

- **Core Requirements**:
  - **100% POSIX Compatible**: Fully comply with POSIX standards (IEEE Std 1003.1)
  - **Complete Linux ABI Compatibility**: Binary compatible with Linux kernel ABI
  - **System Call Compatibility**: System call numbers, parameters, and return values must be identical to Linux
  - **File System Compatibility**: Support Linux file system formats (ext4)
  - **ELF Format Compatibility**: Executable file format identical to Linux
  - **No Innovation Principle**: **Never** deviate from Linux standards for the sake of "better"

- **Implementation Approach**:
  - Directly reference Linux kernel implementation
  - Use the same system call numbers (`arch/riscv/kernel/syscalls`)
  - Use the same structure layouts and memory layouts
  - Use the same file system formats
  - Identical device interfaces and network protocol stacks

- **Strictly Prohibited**:
  - **Never** "optimize" Linux designs
  - **Never** create new system calls
  - **Never** change the behavior of existing interfaces
  - **Never** "reinvent the wheel"
  - **Never** deviate from standards for the sake of "elegance"

- **Reference Resources**:
  - Linux kernel source code (https://elixir.bootlin.com/linux/latest/source)
  - Linux man pages (POSIX standard functions)
  - Linux ABI documentation (`man 2 syscall`)
  - Linux kernel documentation (Documentation/)

> **Remember**: Our goal is to rewrite the Linux kernel in Rust, not to create a new operating system. Any "innovation" that deviates from Linux standards is wrong.

---

## Project Goals

Rux is a **Linux-compatible operating system kernel** written in Rust, aiming to achieve **complete compatibility** with the Linux kernel, including:
- Full POSIX API support
- Linux ABI binary compatibility
- Ability to run native Linux userspace programs

All code is written in Rust, except for necessary platform-specific assembly code.

---

## Core Design Principles

### 1. **Linux Compatibility (Highest Priority)**

All interfaces, system calls, and data structures must be identical to Linux.

**Checklist**:
- [ ] Are system call numbers consistent with Linux?
- [ ] Are data structure layouts consistent with Linux?
- [ ] Are POSIX standards followed?
- [ ] Has Linux kernel source code been referenced?
- [ ] Does it contain any "innovations"?

### 2. **Rust-First**

- **Principle**: All kernel code is written in Rust, except for necessary platform-specific assembly code
- **Rationale**:
  - Memory safety: Rust's ownership system prevents memory errors at compile time
  - Concurrency safety: Type system prevents data races
  - Modern toolchain: Package management, documentation generation, testing framework
- **Exceptions**:
  - Boot code (boot.S)
  - Context switching (naked functions in context.rs)
  - Interrupt entry (trap.S)
  - Privilege level switching

**Note**: Using Rust is a means of implementation, not an end. Even when using Rust, Linux design and interface specifications must be fully followed.

### 3. **Platform Abstraction**

- **Principle**: Platform-specific code is isolated in the `arch/` directory
- **Structure**:
  ```
  kernel/src/arch/
  └── riscv64/        # RISC-V 64-bit (only supported)
  ```
- **Platform Abstraction Layer**:
  - Unified memory management interface
  - Unified interrupt handling framework
  - Unified device driver interface

**Note**: ARM64 (aarch64) architecture has been removed and is no longer maintained.

### 4. **Modular Design**

- **Principle**: Clear module boundaries for easy development and testing
- **Module Division** (referencing Linux kernel structure):
  - `arch/`: Platform-specific code (corresponding to Linux `arch/`)
  - `mm/`: Memory management (corresponding to Linux `mm/`)
  - `process/`: Process management (corresponding to Linux `kernel/`)
  - `fs/`: File system (corresponding to Linux `fs/`)
  - `net/`: Network protocol stack (corresponding to Linux `net/`)
  - `drivers/`: Device drivers (corresponding to Linux `drivers/`)
  - `sync/`: Synchronization primitives (corresponding to Linux `kernel/`)
  - `syscall/`: System call dispatch

**Important**: Module division and organization follows Linux, but implemented in Rust.

### 5. **Layered Architecture**

```
+-------------------------------------+
|     User Space                      |
|     - Linux ELF binaries            |
|     - musl libc                     |
+-------------------------------------+
|     System Call Interface           |
|     - Fully compatible with Linux syscall |
+-------------------------------------+
|     VFS | IPC | Network (Net)       |
|     - Linux-compatible VFS          |
+-------------------------------------+
|     Process Mgmt | Memory Mgmt | Drivers |
|     - Linux process model           |
+-------------------------------------+
|     Platform Abstraction Layer      |
|     - riscv64 (only supported)      |
+-------------------------------------+
|     Hardware                        |
+-------------------------------------+
```

**Key Point**: All interfaces and layers align with Linux.

### 6. **Progressive Implementation**

- **Principle**: Start with a minimal runnable kernel, gradually add features
- **Priority**:
  1. Basic framework (boot, memory, interrupts)
  2. Process management (scheduling, context switching)
  3. System calls (user/kernel isolation)
  4. File system (VFS + ext4)
  5. Network protocol stack
  6. Advanced features (IPC, signals, real-time scheduling)
  7. GUI support

### 7. **Test-Driven**

- **Principle**: Every module should have corresponding tests
- **Test Types**:
  - Kernel unit tests (51 test files)
  - mini-ltp tests (24 kernel compatibility tests)
  - QEMU integration tests
- **Test Commands**:
  - `make test` - Run kernel unit tests
  - `cd /test/mini-ltp && ./run_tests.sh` - Run compatibility tests

### 8. **Comprehensive Documentation**

- **Principle**: Keep code and documentation synchronized
- **Documentation Types**:
  - API documentation (rustdoc)
  - Design documentation (this file)
  - Progress tracking (roadmap.md)
  - User documentation (getting-started.md)
  - Debugging reports (fork-exec-debug-report.md)

---

## POSIX/ABI Implementation Guidelines

### System Call Implementation

**Must** use Linux system call numbers (RISC-V):

```rust
// Directly use Linux RISC-V system call numbers
pub const __NR_read: usize = 63;
pub const __NR_write: usize = 64;
pub const __NR_openat: usize = 56;
pub const __NR_close: usize = 57;
pub const __NR_exit: usize = 93;
pub const __NR_getpid: usize = 172;
// ... completely according to Linux definitions
```

**Prohibited**:
- Creating new system calls
- Modifying system call numbers
- Changing system call parameters

### Structure Layouts

**Must** be completely identical to Linux structures:

```rust
// Must be completely identical to Linux struct stat
#[repr(C)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub __pad1: u64,
    pub st_size: i64,
    pub st_blksize: i32,
    pub __pad2: i32,
    pub st_blocks: i64,
    // ... field order, size, alignment must all be consistent
}
```

### File Systems

**Must** support Linux file system formats:
- ext4 (implemented)
- ramfs (implemented)
- procfs (implemented)
- devfs (implemented)

**Prohibited**:
- Creating new file system formats
- Modifying existing formats (unless Linux does too)

### Device Interfaces

**Must** use Linux device interfaces:
- Character devices (`/dev/xxx`)
- Block devices (`/dev/vda`)
- Input devices (`/dev/input/event0`)

**Reference**: Interface definitions under Linux `include/uapi/`

---

## Implementation Checklist

When implementing any feature, must verify:

- [ ] Consulted Linux kernel source implementation
- [ ] Confirmed use of identical system call numbers/structures
- [ ] Confirmed use of identical file formats
- [ ] Confirmed compliance with POSIX standards
- [ ] Read relevant Linux man pages
- [ ] Does not contain any "innovations" or "improvements"

**Remember**: When in doubt, directly reference Linux implementation.

---

## Technical Constraints

### Compiler
- Rust version: Stable
- Target platform: riscv64gc-unknown-none-elf (only supported)

### Runtime
- No standard library (no_std)
- No runtime (manually implement panic handling)

### Safety
- Isolate dangerous code in unsafe blocks whenever possible
- Explicitly mark all unsafe code
- Regularly audit the correctness of unsafe code

---

## Performance Targets

- **Boot time**: < 5 seconds (QEMU virt)
- **Context switch**: < 1us
- **Interrupt latency**: < 5us
- **System call**: < 100ns

---

## Contribution Guidelines

### Code Style
- Follow official Rust code style (rustfmt)
- Use meaningful variable and function names
- Appropriate comments and documentation

### Commit Standards
- Follow [Conventional Commits](https://www.conventionalcommits.org/)
- Clear commit messages
- Single commit does one thing
- Pass all tests before committing

### Review Process
- Code review must pass
- All tests must pass
- Documentation must be updated

---

## References

- [Linux Kernel Documentation](https://www.kernel.org/doc/html/latest/)
- [RISC-V Architecture Reference Manual](https://riscv.org/technical/specifications/)
- [POSIX Standard](https://pubs.opengroup.org/onlinepubs/9699919799/)
- [Linux man pages](https://man7.org/linux/man-pages/)

---

**Document Version**: v2.0.0
**Last Updated**: 2026-03-04
