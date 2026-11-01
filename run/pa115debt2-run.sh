#!/bin/bash
# pa115debt2 — Tier-2 pbench battery (debug class) on main@afc41b8b.
# Anchor: the pa114pb quiet rerun board (same class, gate-open window),
# with the pa112pb/pa113pb family as context.
cd "$(dirname "$0")/.."
GATE_MAX_FOREIGN=8 run/perf-gate.sh --tiers 2 \
    --kernel run/pa115-main-afc41b8b-debug-bzImage --tag pa115debt2 \
    --pbench-baseline work/pbench-pa114pb-serial.log \
    > run/pa115debt2-gate.txt 2>&1
echo "gate rc=$?" >> run/pa115debt2-gate.txt
