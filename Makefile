# Rux Kernel Project Makefile
# Provides quick access from project root directory

.PHONY: all build clean run test debug help smp user rootfs gui verify miri kani
.PHONY: toybox mrsh sdk ltp ubuntu-image ubuntu-run

# ---- Ubuntu native desktop (udesk) ----
# Image and kernel artifacts
UBUNTU_IMG ?= work/ubuntu-gui.img
KERNEL_BIN ?= target/riscv64gc-unknown-none-elf/debug/rux
# The distro QEMU build has the SDL display backend; the custom 10.2.2 in
# /usr/local/bin is compiled without display backends (none/dbus only).
UBUNTU_QEMU ?= /usr/bin/qemu-system-riscv64
# CPU count (GUI verified 23/23 on -smp 4; override with SMP=1 make ubuntu-run)
SMP ?= 4

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

# ---- Ubuntu native desktop (udesk on Ubuntu 22.04 rootfs) ----
# Build (or rebuild) the Ubuntu GUI disk image: work/ubuntu-gui.img.
# Requires riscv64-linux-gnu-gcc and the shared Ubuntu rootfs (see
# test/ubuntu-gui/build-img.sh, override with SRC=/path CC=...).
ubuntu-image:
	@bash test/ubuntu-gui/build-img.sh

# Run the Ubuntu desktop in a native SDL window (via WSLg). Keyboard
# works BOTH in the SDL window itself (virtio-keyboard -> /dev/input/
# event0) and in this terminal (serial console). Login: root / rux.
# Ctrl-S SysInfo, Ctrl-A About, Tab cycle focus, Ctrl-W close window.
# NOTE: plain `-serial stdio` — NOT mon:stdio: the chardev mux would
# swallow Ctrl-A as its monitor-escape key, but Ctrl-A opens About.
ubuntu-run: build $(UBUNTU_IMG)
	@echo "Starting Ubuntu desktop (SDL window + keyboard in window or this terminal)..."
	$(UBUNTU_QEMU) -M virt -accel tcg,thread=single -cpu rv64 -m 2G -smp $(SMP) \
	  -snapshot -display sdl -serial stdio -monitor none \
	  -device virtio-keyboard-pci -device virtio-tablet-pci \
	  -drive file=$(UBUNTU_IMG),if=none,id=rootfs,format=raw \
	  -device virtio-blk-pci,drive=rootfs \
	  -device virtio-gpu-pci \
	  -kernel $(KERNEL_BIN) \
	  -append "root=/dev/vda rw init=/sbin/init console=ttyS0"

# Image rule: built on demand by ubuntu-run, refreshed by `make ubuntu-image`.
$(UBUNTU_IMG):
	@bash test/ubuntu-gui/build-img.sh

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

# Run SPIN concurrency model checking
spin:
	@echo "=== SPIN Concurrency Model Checking ==="
	@cd kernel/verify/spin && $(MAKE)
	@echo ""
	@echo "=== SPIN checks complete — see kernel/verify/spin/spin-report.txt ==="

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
	@echo "  make ubuntu-run      - Run Ubuntu desktop in an SDL window (root/rux)"
	@echo "  make ubuntu-image    - (Re)build the Ubuntu GUI disk image"
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
