#!/bin/bash
# dfx-b8-perfgate-watch2.sh — fleet-gap watcher for the b8 perf-gate
# tiers 1,2 on the merged tree (tip 6d36d1f2). Waits for a quiet window
# (<2 foreign QEMUs, >=10G available), up to 8h, then runs the gate once.
# Detached; writes everything to run/dfx-b8-perfgate-r8-out.txt.
cd /home/william/rux-agents/dfx-align || exit 1
OUT=run/dfx-b8-perfgate-r8-out.txt
echo "watcher started $(date +%H:%M:%S) pid=$$" >> "$OUT"
for i in $(seq 1 480); do
    foreign=$(pgrep -c qemu-system 2>/dev/null || echo 9)
    avail=$(LC_ALL=C free -g | awk '/^Mem:/{print $7}')
    if [ "${foreign:-9}" -lt 2 ] && [ "${avail:-0}" -ge 10 ]; then
        echo "quiet window at $(date +%H:%M:%S) (foreign=$foreign avail=${avail}G) — launching perf-gate tiers 1,2 (merged tree 6d36d1f2)" > "$OUT"
        bash run/perf-gate.sh --tiers 1,2 --tag b8gate-m >> "$OUT" 2>&1
        echo "PERFGATE-RC=$?" >> "$OUT"
        exit 0
    fi
    # heartbeat once an hour so a stale watcher is visible as alive
    [ $((i % 60)) -eq 0 ] && echo "$(date +%H:%M:%S) still waiting (foreign=$foreign avail=${avail}G, iter $i)" >> "$OUT"
    sleep 60
done
echo "no quiet window in 8h" >> "$OUT"
exit 1
