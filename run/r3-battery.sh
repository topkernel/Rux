#!/bin/bash
# r3-battery.sh — round-3 one-shot battery on the FINAL round-3 kernel
# (origin/main 66cd9918 + qa-suites fixes: memfd seals, vm.memfd_noexec,
# pidns tgkill/tkill, TSC ratchet CAS). rt-tests re-runs (one kernel for
# every row) + the kselftest subset at proper budgets.
set -uo pipefail
cd "$(dirname "$0")/.."
export OUTDIR=run/qa-runs-r3

rt() {  # rt-tests classes (no INJBINS)
    BUDGET="$1" test/qrtests/run-suite-x86.sh "$2" 2>&1 \
        | grep -aE "RES\||boot:" | cut -c1-170
}

ks() {  # kselftest classes (INJBINS injected)
    INJBINS="$PWD/run/kselftest-bin" BUDGET="$1" \
        test/qrtests/run-suite-x86.sh "$2" 2>&1 \
        | grep -aE "RES\||boot:" | cut -c1-170
}

echo "=== rt-tests battery (final kernel) ==="
rt 200 "hackbench 20 process"
rt 200 "hackbench -p 20"
rt 200 "hackbench -T 20"
rt 200 "hackbench -g 40 -l 500"
rt 200 "hackbench -i -g 2 -f 10"
rt 200 "60 cyclictest -D 30 -i 1000 -m"
rt 200 "60 cyclictest -D 30 -i 1000 -m -p 98"
rt 200 "45 pi_stress -D 15 -g 2"

echo "=== kselftest subset (final kernel) ==="
ks 200 "60 memfd"
ks 200 "150 posix_timers"
ks 200 "120 inconsistency-check -t 5"
ks 200 "180 raw_skew"
ks 200 "60 set-timer-lat"
ks 200 "60 nsleep-lat"
ks 200 "60 nanosleep"
ks 200 "60 mqueue-lat"
ks 200 "60 threadtest -t 20"
echo "=== battery done ==="
