# Rux Kernel Quick Reference

**Last Updated**: 2026-03-04

## Project Structure

```
Rux/
├── build/     - Build tools (make build/config/menuconfig)
├── test/       - Test scripts (quick_test.sh, run_riscv64.sh, debug_riscv.sh, all.sh)
├── docs/       - Documentation (CONFIG.md, DESIGN.md, STRUCTURE.md)
├── kernel/     - Kernel source code
├── Kernel.toml - Kernel configuration
└── Makefile    - Shortcut commands
```

## Common Commands

### Build Related
```bash
make build           # Build kernel
make build-quiet     # Quiet build
make clean           # Clean build artifacts
make bin             # Generate binary
```

### Configuration Related
```bash
make config          # View current configuration
make menuconfig      # Interactive configuration menu
vim Kernel.toml      # Manually edit configuration
```

### Run Related
```bash
make run             # Run kernel (QEMU)
make test            # Run test suite
make debug           # GDB debugging
```

### Information Related
```bash
make info            # Display project info
make help            # Show help
make deps            # Check dependencies
```

## Directory Functions

### build/ - Build Tools
- **Makefile** - Detailed build script, supports all build tasks
- **menuconfig.sh** - Interactive configuration menu (similar to Linux kernel)
- **config-demo.sh** - Configuration system demo

### test/ - Test Scripts
- **quick_test.sh** - Quick test (recommended for daily use)
- **run_riscv64.sh** - Full run script (supports SMP)
- **debug_riscv.sh** - GDB debugging script
- **all.sh** - Multi-platform test suite (riscv64 + aarch64)

### docs/ - Documentation
- **CONFIG.md** - Configuration system detailed documentation
- **DESIGN.md** - Kernel design documentation
- **STRUCTURE.md** - Directory structure documentation
- **TODO.md** - Development task list

## Configuration Files

### Kernel.toml - Kernel Configuration

```toml
[general]
name = "Rux"              # Kernel name
version = "0.1.0"         # Version number

[platform]
default_platform = "aarch64"  # Target platform

[memory]
kernel_heap_size = 16     # Heap size (MB)
physical_memory = 2048    # Physical memory (MB)
page_size = 4096          # Page size

[features]
enable_process = false    # Process management
enable_vfs = false        # File system
enable_network = false    # Network

[drivers]
enable_uart = true        # UART driver
enable_timer = true       # Timer driver
enable_gic = false        # GIC interrupt controller

[debug]
log_level = "info"        # Log level
debug_output = true       # Debug output
```

Run `make build` to rebuild after modifying configuration.

## Workflow

### Development Workflow
1. Edit kernel code (`kernel/src/`)
2. Build (`make build`)
3. Test (`make test`)
4. Debug (`make debug`)

### Configuration Workflow
1. Modify configuration (`make menuconfig` or edit `Kernel.toml`)
2. Build (`make build`)
3. Run (`make run`)

## Quick Start

```bash
# First build
make build

# Run kernel
make run

# View configuration
make config

# Run tests
make test

# Clean
make clean
```

## Architecture Support

### riscv64 (Default)
```bash
make build                          # Build
./test/quick_test.sh                # Run
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -bios default -kernel target/riscv64gc-unknown-none-elf/debug/rux
```

### aarch64 (Removed, not maintained)
```bash
# ARM64 architecture has been removed
# To restore: restore kernel/src/arch/aarch64/ directory and related code
# cargo build --package rux --features aarch64
# qemu-system-aarch64 -M virt -cpu cortex-a57 -m 2G -nographic \
#   -kernel target/aarch64-unknown-none/debug/rux
```

### x86_64 (To be implemented)
```bash
# x86_64 platform support needs to be implemented first
# Expected to start in Phase 11
```

## Troubleshooting

### Build Failure
```bash
make clean
make build
```

### QEMU Won't Run
```bash
# RISC-V: Check if QEMU is installed
qemu-system-riscv64 --version

# RISC-V: Check if kernel is compiled
ls target/riscv64gc-unknown-none-elf/debug/rux
```

### Configuration Not Taking Effect
```bash
# Check generated configuration
cat kernel/src/config.rs

# Clean and rebuild
make clean
make build
```

## Script Path Notes

All scripts use relative paths to automatically locate project root:

```bash
# Can be called from any directory
cd build && make build      # OK
cd test && ./run.sh          # OK
cd .. && make build          # OK
```

## More Information

- **AI Assistant Guide**: [CLAUDE.md](CLAUDE.md) - Project overview for Claude Code etc.
- **Project Description**: [README.md](README.md) - User-facing introduction
- **Configuration System**: [docs/CONFIG.md](docs/CONFIG.md)
- **Design Documentation**: [docs/DESIGN.md](docs/DESIGN.md)
- **Directory Structure**: [docs/STRUCTURE.md](docs/STRUCTURE.md)
- **Task List**: [TODO.md](TODO.md)
