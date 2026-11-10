#!/bin/bash
# tier12-retry-rebase.sh — wait for a quiet host window, then run the
# merge-gate perf tiers 1,2 on the DEFAULT (feature-off) build of the
# REBASED tree (origin/main@ccac40ee + lane commits).
#
# Preflight parity with scripts/perf-gate.sh: it self-refuses at >= 2
# foreign qemu-system PIDs (GATE_MAX_FOREIGN=2), so this loop fires at
# <= 1. Polls every 180 s; gives up after 60 attempts (~3 h) with rc 4.
cd /home/william/rux-agents/debug-lockdep-p1
for i in $(seq 1 60); do
    n=$(pgrep -c qemu-system 2>/dev/null || echo 0)
    echo "[retry $i] foreign=$n load=$(cut -d' ' -f1 /proc/loadavg) at $(date '+%m-%d %H:%M:%S')"
    if [ "$n" -le 1 ]; then
        echo "[retry $i] quiet window — running perf-gate tiers 1,2 (default build)"
        bash scripts/perf-gate.sh --tiers 1,2 \
            --kernel target/x86_64-unknown-none/debug/bzImage \
            --tag lockdep-p1-rebase --secs 780
        rc=$?
        echo "gate rc=$rc"
        exit $rc
    fi
    sleep 180
done
echo "no quiet window found in 60 attempts (~3h)"; exit 4
