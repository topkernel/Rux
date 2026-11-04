#!/bin/bash
# r3-pass2.sh — second-pass classes on the TAI kernel (255f86f5-class):
# the three TAI-affected timer rows, the memfd crash-isolation probe,
# and a quiet (no-dfx) threadtest.
set -uo pipefail
cd "$(dirname "$0")/.."
export OUTDIR=run/qa-runs-r3

ks() {
    INJBINS="$PWD/run/kselftest-bin" BUDGET="$1" \
        test/qrtests/run-suite-x86.sh "$2" 2>&1 \
        | grep -aE "RES\||boot:" | cut -c1-220
}

echo "=== memfd crash isolation probe ==="
ks 200 "60 memfdstep"
echo "=== TAI rows ==="
ks 200 "60 set-timer-lat"
ks 200 "60 nsleep-lat"
ks 200 "60 nanosleep"
echo "=== quiet threadtest (no dfx prints) ==="
INJBINS="$PWD/run/kselftest-bin" OUTDIR=run/qa-runs-r3 BUDGET=300 DFX=" " \
    test/qrtests/run-suite-x86.sh "60 threadtest -t 20" 2>&1 \
    | grep -aE "RES\||boot:" | cut -c1-220
echo "=== pass2 done ==="
