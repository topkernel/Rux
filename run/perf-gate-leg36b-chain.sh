#!/bin/bash
# leg-36B v3: chained perf-gate rerun + one more instrumented draw.
# v1 raced (pidfile appears ~1 s after QEMU start; fixed in v2's
# wait_pidfile_gone). v3 adds draw sc335B: sc334B launched with
# dfx=...,taskdump before leg-36B discovered the EXT4-BIGLOCK-CENSUS
# rides the PERIODIC taskdump (dfx=periodic), not dfx=taskdump — the
# biglock owner/waiters split is the decisive instrument for the
# ftruncate-latency family, so sc335B re-draws with dfx=periodic.
# Then: sustained quiet window (total qemu-system <= 1 for 60 s, 5 h
# deadline), then perf-gate --tiers 1,2 on the branch tip build
# (a2f9f7ca; both new instruments are dfx-gated and off in gate boots,
# so the measured system is the merge candidate).
set -u
ROOT="/home/william/rux-agents/samgr-callback"
LOG="$ROOT/run/perf-gate-leg36b-t2retry.log"

say() { echo "$(date +%m%d-%H:%M:%S) $*" >> "$LOG"; }

wait_pidfile_gone() { # $1 = pidfile path
    local pf="$1"
    local spin=0
    while [ ! -f "$pf" ]; do
        sleep 5
        spin=$((spin + 1))
        if [ "$spin" -gt 120 ]; then return 1; fi
    done
    while :; do
        local pid=""
        [ -f "$pf" ] && pid="$(cat "$pf" 2>/dev/null)"
        if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
        sleep 30
    done
}

: > "$LOG"
say "chain v3 armed (waiting for sc334B, then sc335B with dfx=periodic, then quiet+gate)"

wait_pidfile_gone "$ROOT/run/oh-sc334B-qemu.pid"
say "sc334B draw finished"

cp --reflink=auto "$ROOT/run/userdata-scgold.img" "$ROOT/run/sc335B-userdata.img"
EXTRA_APPEND="dfx=txnhunt,wwcensus,leakhunt,slowtask,ams46,poolcensus,periodic" \
SMP=2 SECS=2400 \
    setsid bash "$ROOT/run/oh-boot-p.sh" sc335B \
    "$ROOT/run/sc335B-userdata.img" \
    "$ROOT/target/x86_64-unknown-none/debug/bzImage" \
    > "$ROOT/run/oh-sc335B-driver.log" 2>&1 &
say "sc335B draw launched (dfx=periodic for biglock census)"

wait_pidfile_gone "$ROOT/run/oh-sc335B-qemu.pid"
say "sc335B draw finished"

# sustained-quiet window, 5 h overnight deadline
deadline=$(( $(date +%s) + 18000 ))
quiet=0
while [ "$(date +%s)" -lt "$deadline" ]; do
    n=$(pgrep -c qemu-system 2>/dev/null || echo 0)
    if [ "$n" -le 1 ]; then
        if [ "$quiet" = 0 ]; then quiet=$(date +%s); say "quiet since $(date +%H:%M:%S) (n=$n)"; fi
        if [ $(( $(date +%s) - quiet )) -ge 60 ]; then
            say "quiet window confirmed, starting tier 1+2"
            bash "$ROOT/run/perf-gate.sh" --tiers 1,2 --tag leg36bt2 \
                --kernel "$ROOT/target/x86_64-unknown-none/debug/bzImage" \
                >> "$LOG" 2>&1
            say "gate rc=$?"
            exit 0
        fi
    else
        if [ "$quiet" != 0 ]; then say "quiet broken at n=$n"; fi
        quiet=0
    fi
    sleep 60
done
say "gave up: no quiet window in 5 h"
exit 1
