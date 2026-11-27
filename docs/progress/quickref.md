# Rux Kernel Quick Reference

**Last Updated**: 2026-11-27

## Project Structure

```
Rux/
├── kernel/     - Kernel source code (375,622 lines)
├── userspace/  - User programs (mrsh, apps, tests, toybox)
├── toolchain/  - musl libc toolchain
├── build/      - Build tools (Makefile, menuconfig)
├── test/       - Test scripts
├── docs/       - Documentation
├── Kernel.toml - Kernel configuration
├── Makefile    - Shortcut commands
├── AGENTS.md   - AI assistant guide
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
| **Code Lines** | 375,622 |
| **Source Files** | 506 (492 Rust + 13 ASM + 1 LD) |
| **Syscalls** | 394 dispatched |
| **Unit Tests** | 1,598 cases |
| **proptest** | 1,104 cases |
| **Kani Proofs** | 157 harnesses |
| **SPIN Models** | 4 models |
| **Unsafe surface** | 1,022 lines = 0.27% — zero unsafe outside the 19 audited `unsafe.rs` files (401 files with `forbid(unsafe_code)`, machine gate rc=0) |
| **Linux LTP** | 1,869-test list |
| **Platform** | riscv64 + x86_64 (q35) + aarch64 — triple arch |
| **Phase** | Unsafe-consolidation campaign landed (2026-11-27) |

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

### riscv64 (default)
```bash
make build                          # Build
make run                            # Run
```

### x86_64 (q35) — since 2026-10-07
```bash
make build PLATFORM=x86_64          # boots Ubuntu 22.04 amd64
```

### aarch64 (QEMU virt) — since 2026-10-30
```bash
make build PLATFORM=aarch64
make test PLATFORM=aarch64          # in-kernel suite: 1,119 cases
# Boot: -M virt,gic-version=3 -cpu cortex-a72 (gic-version=3 is REQUIRED —
# default virt is GICv2 and the GIC driver/DTB are v3); the DTB is embedded
# in the image (auto-dumped from the installed QEMU when absent); -append
# and -dtb are no-ops for ELF -kernel boots on this architecture.
```
See [docs/architecture/aarch64.md](../architecture/aarch64.md) for the
memory map, boot protocol, and per-task state disciplines.

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
- **Testing**: [Test Reports](../test/README.md) - unit / integration / LTP / lmbench / geekbench / stress / formal
- **AI Assistant Guide**: [CLAUDE.md](../../CLAUDE.md) - Project overview for AI tools
