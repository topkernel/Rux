# Rux Userspace Program Build System

## Overview

This directory contains userspace programs for the Rux kernel. All programs are compiled as standalone binaries that can be loaded and executed by the kernel.

## Directory Structure

```
userspace/
├── Cargo.toml              # Rust workspace configuration
├── .cargo/
│   └── config.toml         # Cargo configuration
├── build                   # Build script
├── README.md               # This file
│
├── apps/                   # Application programs
│   ├── desktop/            # Desktop environment
│   ├── calculator/         # Calculator
│   ├── clock/              # Clock
│   └── vshell/             # Visual shell
│
├── libs/                   # Libraries
│   └── gui/                # GUI library (Rust std)
│
├── shell/                  # Shell program (C + musl libc)
│
└── toybox/                 # Toybox (200+ Linux command-line tools)
```

## Development Environment

### Prerequisites

- Rust toolchain (stable)
- RISC-V GCC toolchain: `riscv64-linux-gnu-gcc` (for compiling shell)
- musl libc toolchain (at `toolchain/riscv64-rux-linux-musl/`)

### Local Development (x86_64)

desktop and rux_gui use the standard library (std) and can be developed and tested locally:

```bash
cd userspace

# Build all programs
./build

# Build release version
./build release

# Clean build artifacts
./build clean
```

### Cross-Compilation to RISC-V

To cross-compile for RISC-V target:

```bash
# Install RISC-V target
rustup target add riscv64gc-unknown-linux-gnu

# Cross-compile
cargo build --target riscv64gc-unknown-linux-gnu
```

## User Programs

### shell

Command-line shell, built with C and musl libc.

**Location**: `shell/`

**Features**:
- Interactive command line
- Command execution and pipes
- Uses custom linker script (shell.ld)

**Build**:
```bash
make -C shell
# Or from project root
make shell
```

### desktop

Desktop environment, built with Rust std.

**Location**: `desktop/`

**Dependencies**: `libs/gui`

**Features**:
- Uses standard library
- Can be developed and tested locally
- Conditional compilation supports RISC-V system calls

**Build**:
```bash
./build release
```

### rux_gui

GUI library, built with Rust std.

**Location**: `libs/gui/`

**Features**:
- Basic drawing primitives
- Font rendering
- Double buffering
- Window management
- UI controls

**Platform Support**:
- RISC-V: Uses inline assembly for system calls
- Other platforms: Returns stub values (for development testing)

### toybox

A collection of 200+ Linux command-line tools.

**Location**: `toybox/`

**Included Commands**:
- File operations: ls, cat, cp, mv, rm, mkdir, ln, touch
- Text processing: echo, head, tail, wc, sort, uniq, grep, sed, awk
- System information: uname, hostname, id, whoami, free, df, du
- Others: date, sleep, true, false, test, env, yes, tee

**Build**:
```bash
make toybox
```

## Build Commands

Execute from project root:

```bash
# Build all userspace programs (shell, desktop, etc.)
make user

# Build shell only
make shell

# Build toybox
make toybox

# Create rootfs image
make rootfs

# Run kernel
make run
```

## System Call Interface

On RISC-V targets, programs use Linux ABI system calls:

### Register Conventions

- **a7**: System call number
- **a0-a5**: Parameters (up to 6)
- **a0**: Return value

### Common System Calls

| System Call | Number | Function |
|-------------|--------|----------|
| read | 63 | Read file |
| write | 64 | Write file |
| exit | 93 | Exit program |
| getpid | 172 | Get process ID |

## Debugging

### Inspecting Binaries

```bash
# View file information
file shell/shell

# View program size
ls -lh shell/shell

# Use readelf to view RISC-V ELF
riscv64-linux-gnu-readelf -h shell/shell
```

### Local Execution

```bash
# desktop can run locally (but framebuffer will fail)
./target/debug/desktop
```
