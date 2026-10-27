#!/bin/bash
# leg-39B: Tier-1+2 perf gate on the branch tip (kernel
# run/bzImage-leg38b-tip, md5 d79b024a = post-merge tip e55df9d5 +
# docs commit 5540365e content). The leg-38B watcher (2 h) gave up
# with no quiet window; this re-arm adds a memory-floor condition:
# qemu-system total <= 1 for 60 s AND MemFree >= 13000 MB (10 G hard
# host floor + the 4096 MB pa-boot KVM guest), deadline 6 h.
set -u
ROOT="/home/william/rux-agents/samgr-callback"
LOG="$ROOT/run/perf-gate-leg39b.log"
KERNEL="$ROOT/run/bzImage-leg38b-tip"

say() { echo "$(date +%m%d-%H:%M:%S) $*" >> "$LOG"; }

: > "$LOG"
say "leg39b gate armed (kernel=$(md5sum "$KERNEL" | cut -c1-8), waiting for quiet+mem)"

deadline=$(( $(date +%s) + 21600 ))
quiet=0
last_report=0
while [ "$(date +%s)" -lt "$deadline" ]; do
    n=$(pgrep -c qemu-system 2>/dev/null || echo 0)
    mem=$(awk '/^MemFree:/{print int($2/1024)}' /proc/meminfo)
    now=$(date +%s)
    # status line every 10 min regardless of state
    if [ $((now - last_report)) -ge 600 ]; then
        say "waiting (n=$n memfree=${mem}MB load=$(cut -d' ' -f1 /proc/loadavg))"
        last_report=$now
    fi
    if [ "$n" -le 1 ] && [ "${mem:-0}" -ge 13000 ]; then
        if [ "$quiet" = 0 ]; then quiet=$now; say "quiet since $(date +%H:%M:%S) (n=$n mem=${mem}MB)"; fi
        if [ $((now - quiet)) -ge 60 ]; then
            say "quiet window confirmed, starting tier 1+2"
            bash "$ROOT/run/perf-gate.sh" --tiers 1,2 --tag leg39bt2 \
                --kernel "$KERNEL" >> "$LOG" 2>&1
            say "gate rc=$?"
            exit 0
        fi
    else
        if [ "$quiet" != 0 ]; then say "quiet broken at n=$n mem=${mem}MB"; fi
        quiet=0
    fi
    sleep 60
done
say "gave up: no quiet window in 6 h"
exit 1
