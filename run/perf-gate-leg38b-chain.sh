#!/bin/bash
# leg-38B: Tier-1+2 perf gate on the branch tip (e55df9d5 kernel,
# run/bzImage-leg38b-tip - origin/main 584c1105 merged + the family-B
# setattr fix 411709a7). Host was at 8 foreign QEMUs at arm time
# (03:36); sustained-quiet window (total qemu-system <= 1 for 60 s),
# 2 h deadline, then perf-gate --tiers 1,2.
set -u
ROOT="/home/william/rux-agents/samgr-callback"
LOG="$ROOT/run/perf-gate-leg38b.log"
KERNEL="$ROOT/run/bzImage-leg38b-tip"

say() { echo "$(date +%m%d-%H:%M:%S) $*" >> "$LOG"; }

: > "$LOG"
say "leg38b gate armed (kernel=$(md5sum "$KERNEL" | cut -c1-8), waiting for quiet)"

deadline=$(( $(date +%s) + 7200 ))
quiet=0
while [ "$(date +%s)" -lt "$deadline" ]; do
    n=$(pgrep -c qemu-system 2>/dev/null || echo 0)
    if [ "$n" -le 1 ]; then
        if [ "$quiet" = 0 ]; then quiet=$(date +%s); say "quiet since $(date +%H:%M:%S) (n=$n)"; fi
        if [ $(( $(date +%s) - quiet )) -ge 60 ]; then
            say "quiet window confirmed, starting tier 1+2"
            bash "$ROOT/run/perf-gate.sh" --tiers 1,2 --tag leg38bt2 \
                --kernel "$KERNEL" >> "$LOG" 2>&1
            say "gate rc=$?"
            exit 0
        fi
    else
        if [ "$quiet" != 0 ]; then say "quiet broken at n=$n"; fi
        quiet=0
    fi
    sleep 60
done
say "gave up: no quiet window in 2 h"
exit 1
