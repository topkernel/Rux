#!/bin/bash
# perf-gate-leg38a-watch.sh — wait for the calm window, then run the
# merge-gate perf tiers 1,2 for the leg-38A tip (wake-placement
# reservation d6124bbc/ccfc708e/2beb2ded + main 2a960538 merged as
# 1b8be5a3; gate kernel snapshotted at run/bzImage-leg38A-tip so a
# later rebuild can never swap the image under the gate).
# Calm = foreign qemu < 2 AND load1 < 9 AND MemAvailable >= 12G (the
# host free floor 10G is hard; the gate boots one 4G QEMU at a time).
# Polls every 3 min; 8 h deadline; window log run/perf-gate-leg38a-watch.log;
# verdicts land in run/perf-gate-leg38a.log. A one-line heartbeat every
# 30 min keeps the log non-empty while waiting (the leg-36A lesson:
# an empty log was indistinguishable from a dead watcher).
# perf-gate rc=3 (preflight refused — another lane's QEMU took the
# window between the poll and the launch) -> keep polling, not exit.
cd /home/william/rux-agents/oh-kernel || exit 1
DEADLINE=$(( $(date +%s) + 8*3600 ))
TAG=leg38a
KERNEL="$PWD/run/bzImage-leg38A-tip"
LOGW=run/perf-gate-leg38a-watch.log
NEXT_HB=$(( $(date +%s) + 1800 ))
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    n=$(pgrep -c qemu-system 2>/dev/null || echo 0)
    l1=$(cut -d. -f1 /proc/loadavg)
    avail=$(awk '/MemAvailable/{print int($2/1048576)}' /proc/meminfo)
    if [ "$(date +%s)" -ge "$NEXT_HB" ]; then
        echo "$(date +%m%d-%H:%M:%S) still waiting (qemu=$n load1=$l1 avail=${avail}G)" >> "$LOGW"
        NEXT_HB=$(( $(date +%s) + 1800 ))
    fi
    if [ "${n:-9}" -lt 2 ] && [ "${l1:-99}" -lt 9 ] && [ "${avail:-0}" -ge 12 ]; then
        {
            echo "$(date +%m%d-%H:%M:%S) calm (qemu=$n load1=$l1 avail=${avail}G) — launching perf-gate tiers 1,2"
            md5sum "$KERNEL"
            uptime
        } >> "$LOGW"
        bash run/perf-gate.sh --tiers 1,2 --tag "$TAG" \
            --kernel "$KERNEL" \
            --pbench-baseline run/pb1-results.txt \
            > run/perf-gate-leg38a.log 2>&1
        rc=$?
        echo "$(date +%m%d-%H:%M:%S) perf-gate rc=$rc" >> "$LOGW"
        if [ "$rc" -eq 3 ]; then
            echo "$(date +%m%d-%H:%M:%S) window taken by another lane — keep polling" >> "$LOGW"
            sleep 180
            continue
        fi
        exit 0
    fi
    sleep 180
done
echo "$(date +%m%d-%H:%M:%S) no calm window in 8 h — giving up" >> "$LOGW"
exit 2
