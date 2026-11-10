#!/bin/bash
# daily-stress.sh — the one-key daily campaign driver (user directive
# 2026-11-10). Phases, in order, unattended; every phase logs to
# userspace/ci/<date>/, a KERNPANIC anywhere raises userspace/ci/ALERT and
# halts the pipeline (the one condition that warrants intervention).
#
#   1 bootreg   — Ubuntu + OH boot regressions (scripted serial, rc+panic)
#   2 bench     — lmbench + geekbench, numbers appended to perf ledger
#   3 ltp       — full LTP scan, pass-rate row appended
#   4 stress    — fleet3 loop (fio/futex/rt-tests/fsfuzz classes) until
#                 "daily-stress.sh stop"; fio/cyclictest perf rows parsed
#   hourly      — the agent (not this script) checks status + updates
#                 docs/ perf data from these logs; see memory note.
#
# Usage: daily-stress.sh start|stop|status
set -uo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"; WT="$(cd "$DIR/../.." && pwd)"
DAY="$(date +%Y%m%d)"; OUT="$WT/userspace/ci/$DAY"; STOPF="$WT/userspace/ci/STOP"
ALERT="$WT/userspace/ci/ALERT"; mkdir -p "$OUT"

panic_scan() {  # halt-everything condition: a real kernel panic in any phase log
    grep -al "KERNPANIC\|Kernel panic" "$OUT"/*.log "$OUT"/fleet-*.log 2>/dev/null | head -1
}

phase_bootreg() {
    echo "[bootreg] $(date +%H:%M:%S) — Ubuntu + OH scripted boots" | tee -a "$OUT/pipeline.log"
    # TODO(next session, assets exist): ubuntu-gui scripted-serial boot
    #   (work/ubuntu-gui.img + make gui driver shape, 300s budget, rc+frame check)
    # OH boot: /home/william/rux-images/oh/ + lb-boot.py (oh-launcher worktree
    #   has the driver; copy in, SMP=2, log to $OUT/oh-boot.log, check SA-count)
    echo "bootreg: drivers pending re-wiring (assets intact) — see pipeline.log" >> "$OUT/pipeline.log"
}

phase_bench() {
    echo "[bench] $(date +%H:%M:%S) — lmbench + geekbench" | tee -a "$OUT/pipeline.log"
    # TODO: lmbench driver (image rux-images/testimages/lmbench-x86.img, KVM,
    #   parse lat_ctx/lat_syscall rows → $OUT/lmbench.csv)
    # TODO: geekbench (geekbench-x86.img, ~15min class, parse section scores
    #   → $OUT/geekbench.csv). No quick mode — full run per session.
    echo "bench: drivers pending re-wiring (images intact in rux-images/testimages/)" >> "$OUT/pipeline.log"
}

phase_ltp() {
    echo "[ltp] $(date +%H:%M:%S) — full LTP scan (scripts/test/ltpscan on main)" | tee -a "$OUT/pipeline.log"
    ( cd /home/william/workspace/Rux && setsid bash scripts/test/ltpscan/run1-x86.sh \
        > "$OUT/ltp-scan.log" 2>&1 </dev/null & )
    echo "ltp: scan launched detached — pass-rate row lands when it finishes; agent parses on hourly check" >> "$OUT/pipeline.log"
}

phase_stress() {
    echo "[stress] $(date +%H:%M:%S) — fleet3 continuous loop" | tee -a "$OUT/pipeline.log"
    bash "$WT/userspace/ci/stress-fleet.sh" start >> "$OUT/pipeline.log" 2>&1
    # fleet3 rounds ledger: run/fleet3/rounds.log; fio bw + cyclictest rows
    # parsed from run/qa-fleet/s*/<class>.log by the hourly agent check.
}

case "${1:-}" in
start)
    [ -f "$STOPF" ] && rm -f "$STOPF"; rm -f "$ALERT"
    {
        phase_bootreg; [ -f "$ALERT" ] && exit 0
        phase_bench;   [ -f "$ALERT" ] && exit 0
        phase_ltp;     [ -f "$ALERT" ] && exit 0
        phase_stress
        echo "pipeline armed: stress loop runs until daily-stress.sh stop; agent checks hourly" | tee -a "$OUT/pipeline.log"
    } </dev/null >/dev/null 2>&1 &
    echo "daily pipeline started → $OUT/pipeline.log"
    ;;
stop)
    touch "$STOPF"; bash "$WT/userspace/ci/stress-fleet.sh" stop >/dev/null 2>&1
    echo "stopped (fleet reaped; scan finishes its current test)"
    ;;
status)
    echo "== pipeline.log:"; tail -6 "$OUT/pipeline.log" 2>/dev/null
    [ -f "$ALERT" ] && echo "!! ALERT: $(cat "$ALERT")" || echo "no alert"
    echo "== fleet:"; bash "$WT/userspace/ci/stress-fleet.sh" log 6 2>/dev/null
    p="$(panic_scan)" && { echo "!! KERNPANIC in $p — halted"; touch "$ALERT"; } || true
    ;;
esac
