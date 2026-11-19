#!/usr/bin/env bash
# coverage-battery.sh — userspace-driven kernel coverage (KCOV alignment).
#
# The reference kernel collects KERNEL coverage from USERSPACE test
# processes (kselftest drivers, kcov exposes the buffer). This script is
# Rux's shape of that:
#
#   1. build the NORMAL kernel (no unit-test) with cov-dump +
#      instrument-coverage — the battery runs as init on the real boot
#      path (init=/test/kselftest_battery from the rootfs)
#   2. the battery drives syscall/ABI clusters as real processes
#      (futex pair, fork/wait, pipe, memfd, mmap/mprotect, unix
#      socketpair, file I/O), then writes /proc/sys/kernel/cov_dump
#   3. the kernel emits the .profraw as console hex; the same
#      extraction/merge/llvm-cov pipeline as make coverage runs here
#
# Usage: scripts/coverage-battery.sh [report-only]   (reuse last boot log)

set -euo pipefail
cd "$(dirname "$0")/.."

TOOLCHAIN_BIN="$HOME/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin"
PROFDATA="$TOOLCHAIN_BIN/llvm-profdata"
COVTOOL="$TOOLCHAIN_BIN/llvm-cov"

COV_DIR="run/coverage-battery"
KERNEL_ELF="target/riscv64gc-unknown-none-elf/debug/rux"
ROOTFS_IMG="${ROOTFS_IMG:-test/rootfs.img}"

mkdir -p "$COV_DIR"

if [ "${1:-}" != "report-only" ]; then
    echo "=== [1/4] Build instrumented normal kernel (cov-dump, no unit-test) ==="
    CFLAGS="-march=rv64gc -mabi=lp64d" \
    RUSTFLAGS="-C instrument-coverage -Zno-profiler-runtime" \
    cargo build --target riscv64gc-unknown-none-elf --features "riscv64,cov-dump"

    echo "=== [2/4] Boot init=/test/kselftest_battery ==="
    # Same shape as the suite.sh probe boots: bounded run, snapshot-free
    # scratch image, verdict parsed from the log (the battery triggers
    # the profile dump itself; init exiting afterwards is expected).
    timeout 300 qemu-system-riscv64 \
        -M virt -accel tcg,thread=multi \
        -cpu rv64,zbb=true,zba=true,zbs=true \
        -m 1G -smp 2 -display none -serial stdio \
        -drive file="$ROOTFS_IMG",if=none,id=rootfs,format=raw,file.locking=off \
        -device virtio-blk-pci,disable-legacy=on,drive=rootfs \
        -device virtio-net-pci,netdev=net0,disable-legacy=on \
        -netdev user,id=net0 \
        -kernel "$KERNEL_ELF" \
        -append "root=/dev/vda rw init=/test/kselftest_battery console=ttyS0" \
        > "$COV_DIR/boot.log" 2>&1 || true
fi

echo "=== [3/4] Extract profile + battery verdict ==="
grep -c "COV: PASS" "$COV_DIR/boot.log" || true
grep "COV: FAIL" "$COV_DIR/boot.log" || echo "no battery failures"
python3 - "$COV_DIR/boot.log" "$COV_DIR/rux.profraw" <<'PYEOF'
import sys
lines = open(sys.argv[1], errors="replace").read().splitlines()
try:
    b = next(i for i, l in enumerate(lines) if l.strip().startswith("[cov-begin]"))
    e = next(i for i, l in enumerate(lines) if l.strip().startswith("[cov-end]"))
except StopIteration:
    sys.exit("coverage-battery: no [cov-begin]/[cov-end] markers in the boot log")
hexdata = "".join(l.strip() for l in lines[b + 1:e] if not l.strip().startswith("[cov]"))
data = bytes.fromhex(hexdata)
open(sys.argv[2], "wb").write(data)
print(f"profraw: {len(data)} bytes")
PYEOF
PROF_SIZE=$(stat -c%s "$COV_DIR/rux.profraw")
if [ "$PROF_SIZE" -lt 64 ]; then
    echo "coverage-battery: profraw too small" >&2
    exit 1
fi

echo "=== [4/4] Merge + per-subsystem report ==="
"$PROFDATA" merge -sparse "$COV_DIR/rux.profraw" -o "$COV_DIR/rux.profdata"
"$COVTOOL" export "$KERNEL_ELF" -instr-profile="$COV_DIR/rux.profdata" --summary-only \
    > "$COV_DIR/cov.json"
python3 scripts/coverage_summary.py "$COV_DIR/cov.json" \
    $(find kernel/src -name '*.rs' | sort) | tee "$COV_DIR/report.txt"
