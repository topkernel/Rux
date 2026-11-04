#!/bin/bash
# r4-battery.sh — round-4 battery on the merged kernel
# (origin/main 8d47783a + qa-suites: memfd seals, vm.memfd_noexec/pidns
# tgkill, TSC ratchet CAS; CLOCK_BOOTTIME/TAI already on main as
# ad0d5cb4). rt-tests verdict rows (hackbench pipe/socket, cyclictest
# clean numbers on the timer-fix kernel, pi_stress confirm) + the two
# open kselftest rows (memfd crash-site decode via memfd-unbuf,
# threadtest wedge, posix_timers TIMEOUT confirm).
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
        | grep -aE "RES\||boot:" | cut -c1-170
}

echo "=== rt-tests battery (merged kernel) ==="
rt 200 "hackbench 20 process"
rt 200 "hackbench -p 20"
rt 200 "60 cyclictest -D 30 -i 1000 -m"
rt 200 "60 cyclictest -D 30 -i 1000 -m -p 98"
rt 200 "45 pi_stress -D 15 -g 2"

echo "=== kselftest open rows ==="
ks 200 "60 memfd-unbuf"
ks 300 "60 threadtest -t 20"
ks 200 "150 posix_timers"
echo "=== r4 battery done ==="
