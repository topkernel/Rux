#!/bin/bash
# run-ci.sh — `make ci`: one complete unattended pass, no LLM needed.
# Phases: build → suites → image → bootreg(pending) → bench(pending)
# → LTP scan → stress classes (finite, 3 slots) → summary ledger.
set -uo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$DIR/../.." && pwd)"
cd "$ROOT"
SUM="$ROOT/run/ci-summary.log"
echo "== ci pass $(date '+%m%d %H:%M:%S') kernel=$(git rev-parse --short HEAD)" | tee "$SUM"

echo "[1/6] build kernel (x86_64)" | tee -a "$SUM"
PLATFORM=x86_64 make build > "$ROOT/run/ci-build.log" 2>&1 \
    && echo "  build OK" | tee -a "$SUM" || { echo "  BUILD FAILED" | tee -a "$SUM"; exit 1; }

echo "[2/6] build suites (fio rttests futex fsfuzz)" | tee -a "$SUM"
for s in fio rttests futex-selftests; do
    userspace/tests/$s/build-x86.sh > "$ROOT/run/ci-$s.log" 2>&1 \
        && echo "  $s OK" | tee -a "$SUM" || echo "  $s FAILED" | tee -a "$SUM"
done
mkdir -p run/fsfuzz-bin && toolchain/x86_64-rux-linux-musl/bin/musl-gcc -static -O2 \
    -o run/fsfuzz-bin/fsfuzz test/fsfuzz.c 2>/dev/null \
    && echo "  fsfuzz OK" | tee -a "$SUM"

echo "[3/6] guest image" | tee -a "$SUM"
[ -f run/rootfs-qa.img ] || cp /home/william/rux-images/testimages/ltp/rootfs-x86_64-ltp.img run/rootfs-qa.img
echo "  image ready ($(du -h run/rootfs-qa.img | cut -f1))" | tee -a "$SUM"

echo "[4/6] boot/bench regressions — PENDING drivers (see daily-stress.sh TODOs)" | tee -a "$SUM"

echo "[5/6] LTP full scan (detached; row lands on completion)" | tee -a "$SUM"
if [ -x scripts/test/ltpscan/run1-x86.sh ]; then
    ( setsid bash scripts/test/ltpscan/run1-x86.sh > run/ci-ltp.log 2>&1 </dev/null & )
    echo "  scan launched (log run/ci-ltp.log)" | tee -a "$SUM"
else echo "  ltpscan not in this tree — skipped" | tee -a "$SUM"; fi

echo "[6/6] stress classes — finite pass, 3 slots" | tee -a "$SUM"
ONCE=1 bash userspace/ci/stress-fleet.sh start | tee -a "$SUM"
echo "  fleet running finite pass; verdicts append to run/fleet3/rounds.log" | tee -a "$SUM"
echo "== ci pass armed. Poll: bash userspace/ci/stress-fleet.sh log 20" | tee -a "$SUM"
