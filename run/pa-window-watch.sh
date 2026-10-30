#!/bin/bash
# pa-window-watch.sh <tag> — provenance monitor: sample load1/qemu count
# every 60 s while a pa-boot Tier-1 boot (tag) runs, into
# run/<tag>-window.log. The volume row is enforced only in a load-gated
# window (load1<=9, foreign qemus<=8), so each enforcement boot carries
# its own window record.
cd "$(dirname "$0")/.." || exit 1
TAG="${1:?usage: pa-window-watch.sh <tag>}"
LOG="run/$TAG-window.log"
while [ ! -f "run/pa-$TAG-serial.log.done" ]; do
    echo "$(date '+%F %T') load1=$(awk '{printf "%.2f", $1}' /proc/loadavg) qemus=$(pgrep -c qemu-system 2>/dev/null || echo 0)" >> "$LOG"
    sleep 60
done
echo "$(date '+%F %T') window close: load1=$(awk '{printf "%.2f", $1}' /proc/loadavg) qemus=$(pgrep -c qemu-system 2>/dev/null || echo 0)" >> "$LOG"
