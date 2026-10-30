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

# ---- Platform selection (same spirit as `make PLATFORM=`) ----
# PLATFORM env var wins; else the config-driven default from
# build/current-platform.sh (build/.config, Kernel.toml, riscv64).
PLATFORM="${PLATFORM:-}"
if [ -z "$PLATFORM" ]; then
    PLATFORM="$(bash "$SCRIPT_DIR/../build/current-platform.sh")" || exit 1
fi

case "$PLATFORM" in
    riscv64)
        CARGO_TARGET="riscv64gc-unknown-none-elf"
        CARGO_BASE_FLAGS=""
        ;;
    aarch64)
        CARGO_TARGET="aarch64-unknown-none"
        # aarch64 is not a default feature — deselect riscv64 first.
        CARGO_BASE_FLAGS="--no-default-features"
        ;;
    x86_64)
        CARGO_TARGET="x86_64-unknown-none"
        CARGO_BASE_FLAGS="--no-default-features"
        ;;
    *)
        echo "run.sh: unsupported PLATFORM='$PLATFORM' (riscv64|aarch64|x86_64)" >&2
        exit 1
        ;;
esac
KERNEL_ELF="target/${CARGO_TARGET}/debug/rux"

# File to record last build features (per-platform: switching platforms
# without a features-file change must still rebuild the other target)
FEATURES_FILE="target/.build_features.${PLATFORM}"

# The aarch64 build embeds a QEMU 'virt' DTB blob (kernel/build.rs
# .incbin's kernel/src/arch/aarch64/virt.dtb; the file is gitignored
# because it is machine-generated). QEMU's ELF -kernel boot passes no
# DTB, so the kernel ships its own. Bootstrap it deterministically from
# the same qemu-system-aarch64 that boots the test kernel, with the
# same machine flags, whenever it is missing (an existing blob is never
# clobbered — a lane-provided one wins).
ensure_aarch64_dtb() {
    local DTB="kernel/src/arch/aarch64/virt.dtb"
    [ -f "$DTB" ] && return 0
    echo "Generating $DTB (QEMU virt dumpdtb, gic-version=3, -cpu cortex-a72 -m 1G -smp 2)..."
    # gic-version=3 is REQUIRED: the kernel's GIC driver and build.rs's
    # own regeneration are v3 (a plain -M virt dump describes GICv2 and
    # would mismatch the machine the test boot starts).
    qemu-system-aarch64 -M virt,gic-version=3,dumpdtb="$DTB" -cpu cortex-a72 -m 1G -smp 2 >/dev/null 2>&1
    if [ ! -s "$DTB" ]; then
        echo "run.sh: failed to dump the aarch64 virt DTB (is qemu-system-aarch64 installed?)" >&2
        rm -f "$DTB"
        return 1
    fi
}

# Check and build kernel
ensure_kernel() {
    local FEATURES="$1"
    local FORCE_REBUILD="${2:-false}"

    if [ "$PLATFORM" = "aarch64" ]; then
        ensure_aarch64_dtb || return 1
    fi

    # Check if features changed
    local FEATURES_CHANGED=false
    if [ -f "$FEATURES_FILE" ]; then
        local LAST_FEATURES=$(cat "$FEATURES_FILE" 2>/dev/null || echo "")
        if [ "$LAST_FEATURES" != "$FEATURES" ]; then
            FEATURES_CHANGED=true
        fi
    fi

    if [ "$FORCE_REBUILD" = "true" ] || [ ! -f "$KERNEL_ELF" ] || [ "$FEATURES_CHANGED" = "true" ]; then
        echo "Building kernel (platform=$PLATFORM, features: $FEATURES)..."
        cargo build --target "$CARGO_TARGET" $CARGO_BASE_FLAGS --features "$FEATURES"
        if [ "$PLATFORM" = "x86_64" ]; then
            # x86_64 boots the multiboot1 bzImage, not the raw ELF
            # (same extra step as build/Makefile's build target).
            python3 build/mkbzimage.py "$KERNEL_ELF" "target/${CARGO_TARGET}/debug/bzImage"
        fi
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
        -accel tcg,thread=single \
        -cpu rv64,zbb=true,zba=true,zbs=true \
        -m 2G \
        -smp 4 \
        -nographic \
        -serial mon:stdio \
        -drive file=${ROOTFS_IMG:-test/rootfs.img},if=none,id=rootfs,format=raw \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -device virtio-gpu-pci \
        -kernel target/riscv64gc-unknown-none-elf/debug/rux \
        -append "root=/dev/vda rw init=$INIT console=ttyS0"
}

# Run kernel (GUI mode)
run_kernel_gui() {
    local INIT="${1:-$DEFAULT_INIT}"
    echo "Starting QEMU (4 cores, 2GB memory, GUI mode, init=$INIT)..."
    qemu-system-riscv64 \
        -M virt \
        -accel tcg,thread=single \
        -cpu rv64,zbb=true,zba=true,zbs=true \
        -m 2G \
        -smp 4 \
        -serial mon:stdio \
        -drive file=${ROOTFS_IMG:-test/rootfs.img},if=none,id=rootfs,format=raw \
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
        ensure_kernel "${PLATFORM},unit-test" true
        # Bounded run + self-cleanup (test-infra fix, 2026-10-28): the
        # kernel prints its suite verdict and halts, but nothing wires
        # QEMU to exit on halt, so the process spins forever and every
        # session paid a manual kill. Wrap the boot in timeout(1)
        # (TERM, KILL 30 s later; MAKETEST_TIMEOUT overrides the 600 s
        # default) and derive the script's exit code from the SUITE
        # VERDICT captured in the log, not from how QEMU exited: a TERM
        # after a green verdict is success, a timeout with no verdict is
        # a real failure.
        local TIMEOUT_SECS="${MAKETEST_TIMEOUT:-600}"
        local LOG
        LOG="$(mktemp /tmp/rux-maketest.XXXXXX.log)"
        local QRC=0
        if [ "$PLATFORM" = "aarch64" ]; then
            # aarch64 test boot: QEMU virt, PL011 serial, PSCI SMP.
            # The ELF kernel carries its own DTB (embedded at build
            # time), so no -dtb/-append here. Resource discipline:
            # -smp 2 -m 1G and a snapshot+unlocked rootfs drive (the
            # ext4 tests read it; userspace binaries inside are data).
            echo "Starting QEMU (aarch64 virt gic-v3, 2 cores, unit tests, timeout=${TIMEOUT_SECS}s)..."
            timeout --signal=TERM --kill-after=30 "$TIMEOUT_SECS" \
                qemu-system-aarch64 \
                -M virt,gic-version=3 \
                -cpu cortex-a72 \
                -accel tcg,thread=multi \
                -m 1G \
                -smp 2 \
                -nographic \
                -no-reboot \
                -drive file=${ROOTFS_IMG:-test/rootfs.img},if=none,id=rootfs,format=raw,file.locking=off,snapshot=on \
                -device virtio-blk-pci,drive=rootfs \
                -kernel "$KERNEL_ELF" 2>&1 | tee "$LOG"
        elif [ "$PLATFORM" = "x86_64" ]; then
            # x86_64 test boot: q35, multiboot1 bzImage, serial console.
            echo "Starting QEMU (x86_64 q35, unit tests, timeout=${TIMEOUT_SECS}s)..."
            timeout --signal=TERM --kill-after=30 "$TIMEOUT_SECS" \
                qemu-system-x86_64 \
                -M q35 \
                -accel tcg,thread=single \
                -m 2G \
                -smp 4 \
                -nographic \
                -drive file=${ROOTFS_IMG:-test/rootfs.img},if=none,id=rootfs,format=raw \
                -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
                -kernel target/x86_64-unknown-none/debug/bzImage 2>&1 | tee "$LOG"
        else
            echo "Starting QEMU (4 cores, unit tests, timeout=${TIMEOUT_SECS}s)..."
            timeout --signal=TERM --kill-after=30 "$TIMEOUT_SECS" \
                qemu-system-riscv64 \
                -M virt \
                -accel tcg,thread=single \
                -cpu rv64,zbb=true,zba=true,zbs=true \
                -m 2G \
                -smp 4 \
                -nographic \
                -drive file=${ROOTFS_IMG:-test/rootfs.img},if=none,id=rootfs,format=raw \
                -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
                -kernel target/riscv64gc-unknown-none-elf/debug/rux 2>&1 | tee "$LOG"
        fi
        QRC="${PIPESTATUS[0]}"
        if grep -q 'ALL TESTS PASSED' "$LOG"; then
            if [ "$QRC" -ne 0 ]; then
                echo "test: suite verdict green; QEMU wrapper rc=$QRC (killed after kernel halt)"
            fi
            rm -f "$LOG"
            exit 0
        elif grep -q 'TESTS FAILED' "$LOG"; then
            echo "test: suite reported failures (wrapper rc=$QRC)" >&2
            rm -f "$LOG"
            exit 1
        else
            echo "test: no suite verdict reached (wrapper rc=$QRC); log kept: $LOG" >&2
            exit 1
        fi
    elif [ "$MODE" = "gui" ]; then
        # GUI mode: enable VirtIO-GPU display. riscv64-only for now:
        # the boot paths below pass -append init=..., which QEMU only
        # honors for the riscv64 kernel-image protocol (the aarch64
        # ELF boot carries its own DTB/cmdline).
        if [ "$PLATFORM" != "riscv64" ]; then
            echo "run.sh: gui mode is riscv64-only for now (PLATFORM=$PLATFORM)" >&2
            exit 1
        fi
        ensure_kernel "riscv64" false
        run_kernel_gui "$INIT"
    else
        # Console mode (riscv64-only — see the gui note)
        if [ "$PLATFORM" != "riscv64" ]; then
            echo "run.sh: console mode is riscv64-only for now (PLATFORM=$PLATFORM)" >&2
            exit 1
        fi
        ensure_kernel "riscv64" false
        run_kernel "$INIT"
    fi
}

# Run main function
main "$@"
