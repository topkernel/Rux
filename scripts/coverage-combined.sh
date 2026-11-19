#!/usr/bin/env bash
# coverage-combined.sh — the campaign's headline metric: BOTH profiles merged.
#
# The reference kernel measures coverage across ALL test layers; our two
# layers are complementary (the in-kernel suite lights fs/net internals,
# the userspace kselftest program lights process/mm/sched/arch through
# the real boot path). This script merges both profraw files and reports
# the combined per-subsystem table.
#
# Prerequisites (run both first):
#   make coverage                    -> run/coverage/rux.profraw   (suite)
#   scripts/coverage-kselftest.sh    -> run/coverage-kselftest/rux.profraw
#
# Usage: scripts/coverage-combined.sh [--record]

set -euo pipefail
cd "$(dirname "$0")/.."

TOOLCHAIN_BIN="$HOME/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/bin"
PROFDATA="$TOOLCHAIN_BIN/llvm-profdata"
COVTOOL="$TOOLCHAIN_BIN/llvm-cov"

SUITE_RAW="run/coverage/rux.profraw"
USER_RAW="run/coverage-kselftest/rux.profraw"
SUITE_ELF="run/coverage/rux.elf"
USER_ELF="run/coverage-kselftest/rux.elf"
OUT_DIR="run/coverage-combined"

for f in "$SUITE_RAW" "$USER_RAW"; do
    if [ ! -s "$f" ]; then
        echo "coverage-combined: missing $f" >&2
        echo "  run 'make coverage' and 'scripts/coverage-kselftest.sh' first" >&2
        exit 1
    fi
done

mkdir -p "$OUT_DIR"
echo "=== Merging suite + userspace-test profiles ==="
"$PROFDATA" merge -sparse "$SUITE_RAW" "$USER_RAW" -o "$OUT_DIR/combined.profdata"

echo "=== Combined report ==="
# Each profile's counters only match the ELF that generated them (the
# suite boots a unit-test+cov-dump kernel, the userspace test a plain
# cov-dump kernel); export against BOTH preserved objects.
"$COVTOOL" export -object "$SUITE_ELF" -object "$USER_ELF" \
    -instr-profile="$OUT_DIR/combined.profdata" --summary-only \
    > "$OUT_DIR/cov.json"
python3 scripts/coverage_summary.py "$OUT_DIR/cov.json" \
    $(find kernel/src -name '*.rs' | sort) | tee "$OUT_DIR/report.txt"

if [ "${1:-}" = "--record" ]; then
    mkdir -p docs/development
    cp "$OUT_DIR/report.txt" docs/development/coverage-latest.txt
    echo "recorded: docs/development/coverage-latest.txt"
fi
