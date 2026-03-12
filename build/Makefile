# Rux Kernel Makefile
# Provides convenient build and configuration commands

# Project root directory
PROJECT_ROOT := $(shell pwd)/..
BUILD_DIR := $(shell pwd)

.PHONY: all config menuconfig build clean run help test

# Default target
all: build

# Display configuration info
config:
	@echo "Current kernel configuration:"
	@echo "=================="
	@echo "Kernel name: $$(grep '^name' $(PROJECT_ROOT)/Kernel.toml | head -1 | cut -d'"' -f2)"
	@echo "Version: $$(grep '^version' $(PROJECT_ROOT)/Kernel.toml | head -1 | cut -d'"' -f2)"
	@echo "Target platform: $$(grep '^default_platform' $(PROJECT_ROOT)/Kernel.toml | head -1 | cut -d'"' -f2)"
	@echo ""
	@echo "Memory configuration:"
	@echo "  Heap size: $$(grep '^kernel_heap_size' $(PROJECT_ROOT)/Kernel.toml | cut -d= -f2) MB"
	@echo "  Physical memory: $$(grep '^physical_memory' $(PROJECT_ROOT)/Kernel.toml | cut -d= -f2) MB"
	@echo ""
	@echo "Generated constants:"
	@echo "  KERNEL_NAME = $$(grep '^pub const KERNEL_NAME' $(PROJECT_ROOT)/kernel/src/config.rs | cut -d'"' -f2)"
	@echo "  KERNEL_VERSION = $$(grep '^pub const KERNEL_VERSION' $(PROJECT_ROOT)/kernel/src/config.rs | cut -d'"' -f2)"

# Interactive configuration menu
menuconfig:
	@echo "Starting configuration menu..."
	@$(BUILD_DIR)/menuconfig.sh

# Build kernel
build:
	@echo "Building Rux kernel..."
	@cd $(PROJECT_ROOT) && cargo build --target riscv64gc-unknown-none-elf --features debug_log

# Release build (without debug logs)
build-release:
	@echo "Building Rux kernel (Release)..."
	@cd $(PROJECT_ROOT) && cargo build --target riscv64gc-unknown-none-elf --release

# Quick build (compile only, no output)
build-quiet:
	@cd $(PROJECT_ROOT) && cargo build --target riscv64gc-unknown-none-elf --quiet

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	@cd $(PROJECT_ROOT) && cargo clean
	@rm -f $(PROJECT_ROOT)/kernel.bin
	@rm -f $(PROJECT_ROOT)/serial.log
	@rm -f $(PROJECT_ROOT)/virt.dtb
	@echo "Clean complete"


# Debug kernel
debug: build
	@echo "Starting QEMU (debug mode)..."
	@cd $(PROJECT_ROOT) && qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
		-kernel target/riscv64gc-unknown-none-elf/debug/rux \
		-S -s & \
		echo "QEMU PID: $$!" ; \
		echo "Connect with GDB: riscv64-unknown-elf-gdb -ex 'target remote localhost:1234'"

# Generate kernel binary
bin: build
	@echo "Generating kernel binary file..."
	@cd $(PROJECT_ROOT) && rust-objcopy -O binary target/riscv64gc-unknown-none-elf/debug/rux kernel.bin
	@echo "Binary file: $(PROJECT_ROOT)/kernel.bin"

# Display project info
info:
	@echo "Rux Kernel Project Info"
	@echo "=================="
	@echo "Project root: $(PROJECT_ROOT)"
	@echo "Build directory:   $(BUILD_DIR)"
	@echo ""
	@echo "Directory structure:"
	@echo "  kernel/  - Kernel source code"
	@echo "  build/   - Build and configuration tools"
	@echo "  test/    - Test scripts"
	@echo "  docs/    - Documentation"
	@echo ""
	@echo "Configuration files:"
	@ls -1 $(PROJECT_ROOT)/*.toml 2>/dev/null || echo "  (none)"

# Install dependencies
deps:
	@echo "Checking build dependencies..."
	@which cargo >/dev/null || (echo "Error: Rust not installed" && exit 1)
	@which qemu-system-riscv64 >/dev/null || (echo "Warning: QEMU not installed" && exit 0)
	@which rust-objcopy >/dev/null || (echo "Warning: rust-objcopy not installed" && exit 0)
	@echo "Dependency check complete"

# Display help
help:
	@echo "Rux Kernel Build System"
	@echo ""
	@echo "Usage (run from project root):"
	@echo "  cd .. && make -C build <target>"
	@echo ""
	@echo "Or use Makefile inside build directory:"
	@echo "  make <target>"
	@echo ""
	@echo "Available targets:"
	@echo "  make config       - Display current configuration"
	@echo "  make menuconfig   - Interactive configuration menu"
	@echo "  make build        - Build kernel"
	@echo "  make build-quiet  - Quiet build"
	@echo "  make clean        - Clean build artifacts"
	@echo "  make run          - Run kernel (QEMU)"
	@echo "  make test         - Run test suite"
	@echo "  make debug        - Debug kernel (QEMU + GDB)"
	@echo "  make bin          - Generate binary file"
	@echo "  make info         - Display project info"
	@echo "  make deps         - Check build dependencies"
	@echo "  make help         - Display this help"
	@echo ""
	@echo "Quick start:"
	@echo "  1. make menuconfig  # Configure kernel"
	@echo "  2. make build       # Build kernel"
	@echo "  3. make run         # Run kernel"
	@echo ""
	@echo "Configuration files:"
	@echo "  Kernel.toml - Main configuration file"
	@echo ""
	@echo "Documentation:"
	@echo "  docs/CONFIG.md - Configuration system docs"
	@echo "  docs/DESIGN.md  - Design docs"
	@echo "  TODO.md         - Task list"
