# Rux Kernel Quick Reference

**Last Updated**: 2026-04-09

## Project Structure

```
Rux/
├── kernel/     - Kernel source code (~102,400 lines)
├── userspace/  - User programs (mrsh, apps, tests, toybox)
├── toolchain/  - musl libc toolchain
├── build/      - Build tools (Makefile, menuconfig)
├── test/       - Test scripts
├── docs/       - Documentation
├── Kernel.toml - Kernel configuration
├── Makefile    - Shortcut commands
├── CLAUDE.md   - AI assistant guide
└── LICENSE     - MIT License
```

## Common Commands

### Build Related
```bash
make build           # Build kernel
make build RELEASE=1 # Build kernel (release mode, optimized)
make sdk             # Build musl libc SDK (required for user programs)
make user            # Build userspace programs (shell, apps, toybox)
make rootfs          # Build Rootfs image
make clean           # Clean build artifacts
make distclean       # Complete cleanup
```

### Run Related
```bash
make run             # Run kernel (QEMU, default mrsh shell)
make gui             # Run GUI desktop
make test            # Run kernel unit tests
make debug           # GDB debugging
```

### Verification Related
```bash
make verify          # Run proptest (1,088 property-based cases)
make kani            # Run Kani symbolic verification (157 proofs)
make spin            # Run SPIN concurrency models (4 models)
make miri            # Run Miri UB detection
```

### Configuration Related
```bash
make config          # View current configuration
make menuconfig      # Interactive configuration menu
vim Kernel.toml      # Manually edit configuration
```

### Information Related
```bash
make info            # Display project info
make help            # Show help
```

## Project Status

| Metric | Value |
|--------|-------|
| **Code Lines** | ~102,400 |
| **Source Files** | 278 (274 Rust + 3 ASM + 1 LD) |
| **Syscalls** | 348 dispatched |
| **Unit Tests** | 825 cases, 58 files |
| **proptest** | 1,088 cases, 98 modules |
| **Kani Proofs** | 157 harnesses, 22 modules |
| **SPIN Models** | 4 models, 8 LTL properties |
| **Linux LTP** | 1,838 tests |
| **Smoke Tests** | 15/15 passing |
| **Platform** | RISC-V 64-bit (RV64GC) |
| **Phase** | 51 — Memory Compaction |

## Quick Start

```bash
# First build
make build

# Build userspace
make sdk && make user && make rootfs

# Run kernel
make run

# Run tests
make test

# Run verification
make verify
```

## Architecture Support

### riscv64 (Default and Only Supported)
```bash
make build                          # Build
make run                            # Run
```

**Note**: ARM64 (aarch64) has been removed. x86_64 is not planned.

## Troubleshooting

### Build Failure
```bash
make clean
make build
```

### QEMU Won't Run
```bash
qemu-system-riscv64 --version      # Check QEMU version (>= 5.0)
ls target/riscv64gc-unknown-none-elf/debug/rux  # Check kernel binary
```

### Rootfs Issues
```bash
make sdk && make user && make rootfs  # Rebuild rootfs
```

### Configuration Not Taking Effect
```bash
cat kernel/src/config.rs            # Check generated configuration
make clean && make build            # Clean and rebuild
```

## More Information

- **README**: [README.md](../../README.md) - Project overview and features
- **Getting Started**: [getting-started.md](../guides/getting-started.md) - Quick start guide
- **Architecture**: [design.md](../architecture/design.md) - Design principles
- **Code Structure**: [structure.md](../architecture/structure.md) - Directory structure
- **Roadmap**: [roadmap.md](roadmap.md) - Development roadmap
- **Testing**: [testing.md](../test/testing.md) - Testing guide
- **AI Assistant Guide**: [CLAUDE.md](../../CLAUDE.md) - Project overview for AI tools
