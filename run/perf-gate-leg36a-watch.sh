#!/bin/bash
# perf-gate-leg36a-watch.sh — wait for the calm window, then run the
# merge-gate perf tiers 1,2 for the leg-36A tip (wake-placement
# reservation d6124bbc/ccfc708e/2beb2ded on top of the spawnfix queue).
# Calm = foreign qemu < 2 AND load1 < 9 (the Tier-1 volume row is only
# enforceable in a comparable window — pa50-rebank gate).
# Polls every 3 min; 4 h deadline; logs to run/perf-gate-leg36a-watch.log.
cd /home/william/rux-agents/oh-kernel || exit 1
DEADLINE=$(( $(date +%s) + 4*3600 ))
TAG=leg36a
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    n=$(pgrep -c qemu-system 2>/dev/null || echo 0)
    l1=$(cut -d. -f1 /proc/loadavg)
    if [ "${n:-9}" -lt 2 ] && [ "${l1:-99}" -lt 9 ]; then
        {
            echo "$(date +%m%d-%H:%M:%S) calm (qemu=$n load1=$l1) — launching perf-gate tiers 1,2"
            uptime
        } >> run/perf-gate-leg36a-watch.log
        bash run/perf-gate.sh --tiers 1,2 --tag "$TAG" \
            --pbench-baseline run/pb1-results.txt \
            > run/perf-gate-leg36a.log 2>&1
        rc=$?
        echo "$(date +%m%d-%H:%M:%S) perf-gate rc=$rc" >> run/perf-gate-leg36a-watch.log
        exit 0
    fi
    sleep 180
done
echo "$(date +%m%d-%H:%M:%S) no calm window in 4 h — giving up" \
    >> run/perf-gate-leg36a-watch.log
exit 2
