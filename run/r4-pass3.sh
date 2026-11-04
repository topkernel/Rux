#!/bin/bash
# r4-pass3.sh — post-F-4-fix verification boots on the final round-4
# kernel: stock memfd class verdict, cyclictest FIFO isolation rerun,
# the two unbuffered wedge decodes, plus hackbench/cyclictest sanity
# rows (fork/ns paths were touched — quick stability confirmation).
set -uo pipefail
cd "$(dirname "$0")/.."
export OUTDIR=run/qa-runs-r4

rt() {
    BUDGET="$1" test/qrtests/run-suite-x86.sh "$2" 2>&1 \
        | grep -aE "RES\||boot:" | cut -c1-170
}

ks() {
    INJBINS="$PWD/run/kselftest-bin" BUDGET="$1" \
        test/qrtests/run-suite-x86.sh "$2" 2>&1 \
        | grep -aE "RES\||boot:" | cut -c1-200
}

echo "=== stock memfd class verdict ==="
ks 200 "60 memfd"
echo "=== cyclictest FIFO p98 isolation rerun ==="
rt 200 "60 cyclictest -D 30 -i 1000 -m -p 98"
echo "=== posix_timers stall-site decode (unbuffered) ==="
ks 240 "150 posix_timers-unbuf"
echo "=== threadtest wedge decode (unbuffered) ==="
ks 240 "60 threadtest-unbuf -t 20"
echo "=== hackbench socketpair sanity (fork path touched) ==="
rt 200 "hackbench 20 process"
echo "=== cyclictest OTHER sanity ==="
rt 200 "60 cyclictest -D 30 -i 1000 -m"
echo "=== pass3 done ==="
