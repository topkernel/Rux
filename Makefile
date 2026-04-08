# Rux Kernel Project Makefile
# Provides quick access from project root directory

.PHONY: all build clean run test debug help smp user rootfs gui verify miri kani
.PHONY: toybox mrsh sdk ltp

# Default target: forward to build/Makefile
all:
	@$(MAKE) -C build all

# Build kernel
build:
	@$(MAKE) -C build build

# Clean
clean:
	@$(MAKE) -C build clean

# Configuration
config:
	@$(MAKE) -C build config

menuconfig:
	@$(MAKE) -C build menuconfig

# Build toybox (200+ Linux command line tools) - requires sdk
toybox: sdk
	@echo "Building toybox with musl libc..."
	@cd userspace/toybox && ./build-toybox.sh

# Build mrsh (minimal POSIX shell) - requires sdk
mrsh: sdk
	@echo "Building mrsh with musl libc..."
	@cd userspace/mrsh && ./build-mrsh.sh

# Build musl libc SDK (toolchain for cross-compilation)
sdk:
	@echo "Building musl libc SDK..."
	@cd toolchain && ./build-musl.sh

# Build LTP test suite (requires sdk)
ltp: sdk
	@echo "Building LTP test suite..."
	@cd userspace/linux-ltp && ./build.sh

# Build user programs (Rust std + musl) - requires sdk first
user: sdk
	@echo "Building user programs (debug)..."
	@./userspace/build debug
	@echo "Building user programs (release)..."
	@./userspace/build release

# Create rootfs image (containing mrsh and toybox)
rootfs: user toybox mrsh
	@echo "Building rootfs image with mrsh and toybox..."
	@./test/mkrootfs.sh

# Run kernel (QEMU) - default to mrsh
run:
	@echo "Starting QEMU (mrsh)..."
	@./test/run.sh console /bin/sh

# Run GUI mode (desktop environment)
gui:
	@echo "Starting QEMU (GUI - desktop)..."
	@./test/run.sh gui /app/desktop

# Run kernel test script
test:
	@./test/run.sh test

# Run formal verification (sync check + proptest)
verify:
	@echo "=== Step 1/2: Sync check ==="
	@python3 scripts/verify_sync_check.py
	@echo ""
	@echo "=== Step 2/2: Run verification tests ==="
	@cd kernel/verify && cargo test --target x86_64-unknown-linux-gnu
	@echo ""
	@echo "=== All verify steps passed ==="

# Run Miri UB detection on verify crate
miri:
	@echo "=== Step 1/2: Sync check ==="
	@python3 scripts/verify_sync_check.py
	@echo ""
	@echo "=== Step 2/2: Run Miri UB detection ==="
	@cd kernel/verify && MIRIFLAGS="-Zmiri-disable-isolation" cargo +nightly miri test
	@echo ""
	@echo "=== All Miri checks passed (no UB found) ==="

# Run Kani symbolic verification on verify crate
kani:
	@echo "=== Step 1/2: Sync check ==="
	@python3 scripts/verify_sync_check.py
	@echo ""
	@echo "=== Step 2/2: Run Kani verification ==="
	@cd kernel/verify && cargo kani
	@echo ""
	@echo "=== All Kani proofs passed ==="

# SMP test
smp: build
	@echo "SMP test removed, please use test.sh for unit tests"

# Debug
debug: build
	@$(MAKE) -C build debug

# Generate binary
bin:
	@$(MAKE) -C build bin

# Project info
info:
	@$(MAKE) -C build info

# Dependency check
deps:
	@$(MAKE) -C build deps

# Help
help:
	@echo "Rux Kernel Project"
	@echo ""
	@echo "Quick commands (from project root):"
	@echo "  make build           - Build kernel"
	@echo "  make clean           - Clean build"
	@echo "  make run             - Run kernel (mrsh)"
	@echo "  make gui             - Run GUI mode (desktop)"
	@echo "  make test            - Run tests"
	@echo "  make verify          - Run formal verification (sync check + proptest)"
	@echo "  make miri            - Run Miri UB detection on verify crate"
	@echo "  make kani            - Run Kani symbolic verification"
	@echo "  make rootfs          - Create rootfs image"
	@echo "  make debug           - Debug kernel"
	@echo "  make menuconfig      - Configure kernel"
	@echo ""
	@echo "Build user programs:"
	@echo "  make user            - Build all user programs (desktop, etc.)"
	@echo "  make toybox          - Build toybox (200+ command line tools)"
	@echo "  make mrsh            - Build mrsh (POSIX shell)"
	@echo ""
	@echo "Build toolchain & tests:"
	@echo "  make sdk             - Build musl libc SDK (cross-compile toolchain)"
	@echo "  make ltp             - Build LTP test suite (1826 test binaries)"
	@echo ""
	@echo "Directory structure:"
	@echo "  kernel/    - Kernel source code"
	@echo "  userspace/ - User programs"
	@echo "  build/     - Build and configuration tools"
	@echo "  test/      - Test scripts"
	@echo "  docs/      - Documentation"
