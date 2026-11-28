#!/usr/bin/env bash
# Whole-kernel coverage report for the in-kernel test suite.
#
# Pipeline:
#   1. COV=1 make test           — boots the instrumented kernel; the suite
#                                  writes /coverage.profraw to the rootfs
#   2. debugfs cat               — extracts the profraw from the rootfs image
#   3. llvm-profdata merge       — sparse merge
#   4. llvm-cov export           — machine-readable per-file coverage
#   5. coverage_summary.py       — per-top-module table INCLUDING 0% files
#                                  (files absent from the mapping count 0%)
#
# Usage: scripts/coverage-report.sh [--skip-boot]   (reuse the last boot)
#
# Everything lands in run/coverage/ (untracked scratch); the table goes to
# stdout and docs/development/coverage-latest.json is refreshed when the
# caller passes --record.

set -euo pipefail

cd "$(dirname "$0")/.."

TOOLCHAIN_BIN="$HOME/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin"
PROFDATA="$TOOLCHAIN_BIN/llvm-profdata"
COV="$TOOLCHAIN_BIN/llvm-cov"

COV_DIR="run/coverage"
ROOTFS_IMG="${ROOTFS_IMG:-test/rootfs.img}"
KERNEL_ELF="target/riscv64gc-unknown-none-elf/debug/rux"
PLATFORM="${PLATFORM:-riscv64}"

SKIP_BOOT=0
RECORD=0
for arg in "$@"; do
    case "$arg" in
        --skip-boot) SKIP_BOOT=1 ;;
        --record) RECORD=1 ;;
    esac
done

mkdir -p "$COV_DIR"

if [ "$SKIP_BOOT" != "1" ]; then
    echo "=== [1/5] Boot instrumented kernel + run suite (COV=1 make test) ==="
    # Instrumented boots run slower (a counter increment per region) and
    # the suite keeps growing — the coverage flow gets a 20-minute budget
    # instead of run.sh's 10-minute default.
    COV=1 PLATFORM="$PLATFORM" MAKETEST_TIMEOUT=2400 make test 2>&1 | tee "$COV_DIR/boot.log"
else
    echo "=== [1/5] Skipping boot (--skip-boot) ==="
fi

echo "=== [2/5] Extract profraw hex from the boot log ==="
python3 - "$COV_DIR/boot.log" "$COV_DIR/rux.profraw" <<'PYEOF'
import sys
lines = open(sys.argv[1], errors="replace").read().splitlines()
try:
    b = next(i for i, l in enumerate(lines) if l.strip().startswith("[cov-begin]"))
    e = next(i for i, l in enumerate(lines) if l.strip().startswith("[cov-end]"))
except StopIteration:
    sys.exit("coverage-report: no [cov-begin]/[cov-end] markers in the boot log")
hexdata = "".join(l.strip() for l in lines[b + 1:e] if not l.strip().startswith("[cov]"))
open(sys.argv[2], "wb").write(bytes.fromhex(hexdata))
print(f"    profraw: {len(hexdata)//2} bytes from {e - b - 1} log lines")
PYEOF
PROF_SIZE=$(stat -c%s "$COV_DIR/rux.profraw")
echo "    profraw: $PROF_SIZE bytes"
if [ "$PROF_SIZE" -lt 64 ]; then
    echo "coverage-report: profraw too small — the suite probably failed before the dump" >&2
    exit 1
fi

# Preserve the ELF that generated these counters (see the kselftest
# script's note).
cp "$KERNEL_ELF" "$COV_DIR/rux.elf" 2>/dev/null || true

echo "=== [3/5] llvm-profdata merge ==="
"$PROFDATA" merge -sparse "$COV_DIR/rux.profraw" -o "$COV_DIR/rux.profdata"

echo "=== [4/5] llvm-cov export ==="
"$COV" export "$KERNEL_ELF" -instr-profile="$COV_DIR/rux.profdata" --summary-only \
    > "$COV_DIR/cov.json"

echo "=== [5/5] Summary (all files, 0% included) ==="
python3 scripts/coverage_summary.py "$COV_DIR/cov.json" \
    $(find kernel/src -name '*.rs' | sort) | tee "$COV_DIR/report.txt"

if [ "$RECORD" = "1" ]; then
    mkdir -p docs/development
    cp "$COV_DIR/report.txt" docs/development/coverage-latest.txt
    echo "recorded: docs/development/coverage-latest.txt"
fi
