#!/bin/bash
# bisect-step-pbench.sh — one git-bisect step for pbench-landmine hunts (see
# CHARTER ledger 2026-11-06: classifies the checked-out kernel as GOOD
# or BAD for the pid=1 pbench get_meta landmine via pbquick.py).
# Builds the checked-out commit (release-opt, x86_64) and classifies it
# with pbquick.py.  Exit codes follow git-bisect-run semantics:
#   0 good (pbench full battery DONE)
#   1 bad  (pid=1 pbench SIGSEGV landmine)
#   125 skip (build failure / kernel panic / boot timeout)
set -u
cd 
C=$(git rev-parse --short HEAD)
LOG="run/mine-hunt/step-$C.log"
echo "=== step $C $(git log -1 --format='%s' HEAD | cut -c1-80)"
if ! make -C build build-release-opt PLATFORM=x86_64 > "$LOG" 2>&1; then
    echo "BUILD-FAIL $C (log: $LOG)"
    tail -5 "$LOG"
    exit 125
fi
BZ="target/x86_64-unknown-none/release/bzImage"
cp "$BZ" "work/bzImage-mh-$C"
timeout 300 python3 scripts/test/pbquick.py --bzimage "work/bzImage-mh-$C" \
    --tag "mh-$C" --timeout 240 > "run/mine-hunt/boot-$C.log" 2>&1
rc=$?
case $rc in
    0) echo "GOOD $C"; exit 0 ;;
    1) echo "BAD  $C (landmine fired)"; exit 1 ;;
    *) echo "SKIP $C (pbquick rc=$rc)"; tail -3 "run/mine-hunt/boot-$C.log"; exit 125 ;;
esac
