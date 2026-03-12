# Rux Kernel Project Makefile
# Provides quick access from project root directory

.PHONY: all build clean run run-toybox test debug help smp user rootfs gui
.PHONY: shell toybox

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

# Build shell (musl libc)
shell:
	@echo "Building shell with musl libc..."
	@$(MAKE) -C userspace/shell

# Build toybox (200+ Linux command line tools)
toybox:
	@echo "Building toybox with musl libc..."
	@cd userspace/toybox && ./build-toybox.sh

# Build user programs (Rust std + musl) - compile both debug and release
user:
	@echo "Building user programs (debug)..."
	@./userspace/build debug
	@echo "Building user programs (release)..."
	@./userspace/build release

# Create rootfs image (containing shell and toybox)
rootfs: user toybox
	@echo "Building rootfs image with shell and toybox..."
	@./test/mkrootfs.sh

# Run kernel (QEMU) - default to shell
run:
	@echo "Starting QEMU (shell)..."
	@./test/run.sh console /bin/shell

# Run kernel (QEMU) - use toybox shell
run-toybox:
	@echo "Starting QEMU (toybox)..."
	@./test/run.sh console /bin/toybox

# Run GUI mode (desktop environment)
gui:
	@echo "Starting QEMU (GUI - desktop)..."
	@./test/run.sh gui /app/desktop

# Run kernel test script
test:
	@./test/run.sh test

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
	@echo "  make run             - Run kernel (shell)"
	@echo "  make run-toybox      - Run kernel (toybox shell)"
	@echo "  make gui             - Run GUI mode (desktop)"
	@echo "  make test            - Run tests"
	@echo "  make rootfs          - Create rootfs image"
	@echo "  make debug           - Debug kernel"
	@echo "  make menuconfig      - Configure kernel"
	@echo ""
	@echo "Build user programs:"
	@echo "  make user            - Build all user programs (shell, desktop, etc.)"
	@echo "  make shell           - Build shell (musl libc)"
	@echo "  make toybox          - Build toybox (200+ command line tools)"
	@echo ""
	@echo "Directory structure:"
	@echo "  kernel/    - Kernel source code"
	@echo "  userspace/ - User programs"
	@echo "  build/     - Build and configuration tools"
	@echo "  test/      - Test scripts"
	@echo "  docs/      - Documentation"
