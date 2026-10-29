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
	@if [ -f $(BUILD_DIR)/.config ]; then \
		echo "Active config : $(BUILD_DIR)/.config (make menuconfig)"; \
	else \
		echo "Active config : Kernel.toml (defaults; run make menuconfig to customize)"; \
	fi
	@echo "Platform      : $$( $(BUILD_DIR)/current-platform.sh )"
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

# ---- Platform selection (Linux-style: config-driven) ----
# The active platform comes from build/.config (set by `make menuconfig`),
# falling back to Kernel.toml's default_platform. Override per-invocation
# with `make PLATFORM=x86_64` (same spirit as Linux's ARCH=).
PLATFORM := $(shell $(BUILD_DIR)/current-platform.sh)
ifeq ($(PLATFORM),x86_64)
CARGO_TARGET := x86_64-unknown-none
CARGO_FEATURE_FLAGS := --no-default-features --features x86_64
KERNEL_BIN := $(PROJECT_ROOT)/target/$(CARGO_TARGET)/debug/rux
else ifeq ($(PLATFORM),aarch64)
CARGO_TARGET := aarch64-unknown-none
CARGO_FEATURE_FLAGS := --no-default-features --features aarch64
KERNEL_BIN := $(PROJECT_ROOT)/target/$(CARGO_TARGET)/debug/rux
else ifeq ($(PLATFORM),riscv64)
CARGO_TARGET := riscv64gc-unknown-none-elf
CARGO_FEATURE_FLAGS :=
KERNEL_BIN := $(PROJECT_ROOT)/target/$(CARGO_TARGET)/debug/rux
else
$(error Unsupported platform '$(PLATFORM)' — run `make menuconfig`)
endif

# Build kernel (per active configuration)
build:
	@echo "Building Rux kernel (platform=$(PLATFORM))..."
	@cd $(PROJECT_ROOT) && cargo build --target $(CARGO_TARGET) $(CARGO_FEATURE_FLAGS)
	@cd $(PROJECT_ROOT) && python3 build/gen-ksyms.py $(KERNEL_BIN) $(CARGO_TARGET) kernel/src/dfx/ksyms_generated.rs
	@cd $(PROJECT_ROOT) && cargo build --target $(CARGO_TARGET) $(CARGO_FEATURE_FLAGS)
ifeq ($(PLATFORM),x86_64)
	@cd $(PROJECT_ROOT) && python3 build/mkbzimage.py $(KERNEL_BIN) $(PROJECT_ROOT)/target/$(CARGO_TARGET)/debug/bzImage
endif

# Release build (without debug logs, per active configuration)
build-release:
	@echo "Building Rux kernel (Release, platform=$(PLATFORM))..."
	@cd $(PROJECT_ROOT) && cargo build --target $(CARGO_TARGET) $(CARGO_FEATURE_FLAGS) --release
ifeq ($(PLATFORM),x86_64)
	@cd $(PROJECT_ROOT) && python3 build/mkbzimage.py $(PROJECT_ROOT)/target/$(CARGO_TARGET)/release/rux $(PROJECT_ROOT)/target/$(CARGO_TARGET)/release/bzImage
endif

# Fully-optimized release build: same as build-release but forces the
# strong profile (lto=true, codegen-units=1, strip) via cargo env
# overrides, independent of what [profile.release] in the root
# Cargo.toml currently says. This is the build-option gate for perf-lane
# L0 measurements (perf-report.md 8.10): the default release profile is
# untouched; run this target to produce the strong-profile kernel and
# copy the bzImage out before/after any other release build (they share
# target/$(CARGO_TARGET)/release/bzImage).
build-release-opt: export CARGO_PROFILE_RELEASE_LTO := true
build-release-opt: export CARGO_PROFILE_RELEASE_CODEGEN_UNITS := 1
build-release-opt: export CARGO_PROFILE_RELEASE_STRIP := symbols
build-release-opt:
	@echo "Building Rux kernel (Release-opt LTO/CGU1/strip, platform=$(PLATFORM))..."
	@cd $(PROJECT_ROOT) && cargo build --target $(CARGO_TARGET) $(CARGO_FEATURE_FLAGS) --release
ifeq ($(PLATFORM),x86_64)
	@cd $(PROJECT_ROOT) && python3 build/mkbzimage.py $(PROJECT_ROOT)/target/$(CARGO_TARGET)/release/rux $(PROJECT_ROOT)/target/$(CARGO_TARGET)/release/bzImage
endif

# x86_64 convenience alias — equivalent to `PLATFORM=x86_64 make build`
build-x86:
	@$(MAKE) -C $(PROJECT_ROOT)/build PLATFORM=x86_64 build

# aarch64 convenience alias — equivalent to `PLATFORM=aarch64 make build`
build-aarch64:
	@$(MAKE) -C $(PROJECT_ROOT)/build PLATFORM=aarch64 build

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
