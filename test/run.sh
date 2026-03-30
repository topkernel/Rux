#!/bin/bash
# Rux OS run script
#
# Features:
# 1. Check if kernel exists, build if not
# 2. Start QEMU
#    - test mode: use unit-test feature, force recompile
#    - console mode: console mode (can specify init program)
#    - gui mode: graphical mode (enable VirtIO-GPU display)
#
# Usage:
#   ./run.sh [mode] [init]
#   mode: console | gui | test
#   init: /bin/shell | /bin/sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

# Default init program
DEFAULT_INIT="/bin/sh"

# File to record last build features
FEATURES_FILE="target/.build_features"

# Check and build kernel
ensure_kernel() {
    local FEATURES="$1"
    local FORCE_REBUILD="${2:-false}"

    # Check if features changed
    local FEATURES_CHANGED=false
    if [ -f "$FEATURES_FILE" ]; then
        local LAST_FEATURES=$(cat "$FEATURES_FILE" 2>/dev/null || echo "")
        if [ "$LAST_FEATURES" != "$FEATURES" ]; then
            FEATURES_CHANGED=true
        fi
    fi

    if [ "$FORCE_REBUILD" = "true" ] || [ ! -f "target/riscv64gc-unknown-none-elf/debug/rux" ] || [ "$FEATURES_CHANGED" = "true" ]; then
        echo "Building kernel (features: $FEATURES)..."
        cargo build --target riscv64gc-unknown-none-elf --features "$FEATURES"
        echo "$FEATURES" > "$FEATURES_FILE"
    fi
}

# Run kernel (console mode, with rootfs)
run_kernel() {
    local INIT="${1:-$DEFAULT_INIT}"
    echo "Starting QEMU (4 cores, 2GB memory, console mode, init=$INIT)..."

    # Detect WSL for informational purposes
    if grep -qi microsoft /proc/version 2>/dev/null; then
        echo "Detected WSL environment"
    fi

    # Use -serial mon:stdio for all platforms:
    # - Sets host terminal to raw mode (Ctrl+C passes to guest, not kills QEMU)
    # - Enables Ctrl+A escape: Ctrl+A then X exits QEMU
    qemu-system-riscv64 \
        -M virt \
        -cpu rv64 \
        -m 2G \
        -smp 4 \
        -nographic \
        -serial mon:stdio \
        -drive file=test/rootfs.img,if=none,id=rootfs,format=raw \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -device virtio-gpu-pci \
        -kernel target/riscv64gc-unknown-none-elf/debug/rux \
        -append "root=/dev/vda rw init=$INIT console=ttyS0"
}

# Run kernel (GUI mode)
run_kernel_gui() {
    local INIT="${1:-$DEFAULT_INIT}"
    echo "Starting QEMU (4 cores, 2GB memory, GUI mode, init=$INIT)..."
    echo "Tip: Run /app/desktop in terminal shell to start desktop"
    qemu-system-riscv64 \
        -M virt \
        -cpu rv64 \
        -m 2G \
        -smp 4 \
        -serial mon:stdio \
        -drive file=test/rootfs.img,if=none,id=rootfs,format=raw \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -device virtio-gpu-pci \
        -device virtio-keyboard-pci \
        -device virtio-tablet-pci \
        -kernel target/riscv64gc-unknown-none-elf/debug/rux \
        -append "root=/dev/vda rw init=$INIT console=ttyS0"
}

# Main function
main() {
    local MODE="${1:-console}"
    local INIT="${2:-$DEFAULT_INIT}"

    if [ "$MODE" = "test" ]; then
        # Test mode: use unit-test feature, force recompile
        ensure_kernel "riscv64,unit-test" true
        echo "Starting QEMU (4 cores, unit tests)..."
        qemu-system-riscv64 \
            -M virt \
            -cpu rv64 \
            -m 2G \
            -smp 4 \
            -nographic \
            -drive file=test/rootfs.img,if=none,id=rootfs,format=raw \
            -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
            -kernel target/riscv64gc-unknown-none-elf/debug/rux
    elif [ "$MODE" = "gui" ]; then
        # GUI mode: enable VirtIO-GPU display
        ensure_kernel "riscv64" false
        run_kernel_gui "$INIT"
    else
        # Console mode
        ensure_kernel "riscv64" false
        run_kernel "$INIT"
    fi
}

# Run main function
main "$@"
