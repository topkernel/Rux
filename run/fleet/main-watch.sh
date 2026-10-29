#!/bin/bash
# s17 main-watch: poll origin/main for the two awaited merges (OH-A
# wake-placement fix; DFX perf fixes) and log every HEAD change.
# Detached runner: nohup/setsid from the session; writes
# run/fleet/logs/main-watch.log. Exits when the stop file appears.
cd /home/william/rux-agents/wwh-main-kernel || exit 1
STOP=run/fleet/main-watch.stop
LOG=run/fleet/logs/main-watch.log
LAST=$(git rev-parse --short origin/main 2>/dev/null)
echo "[main-watch $(date +%H:%M:%S)] start, origin/main=$LAST" >> "$LOG"
while [ ! -f "$STOP" ]; do
    sleep 300
    git fetch -q origin 2>/dev/null
    CUR=$(git rev-parse --short origin/main 2>/dev/null)
    if [ -n "$CUR" ] && [ "$CUR" != "$LAST" ]; then
        echo "[main-watch $(date +%H:%M:%S)] origin/main moved: $LAST -> $CUR" >> "$LOG"
        git log --oneline "$LAST..$CUR" >> "$LOG" 2>/dev/null
        LAST=$CUR
    fi
done
echo "[main-watch $(date +%H:%M:%S)] stop file, exiting" >> "$LOG"
rm -f "$STOP"
