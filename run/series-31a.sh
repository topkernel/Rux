#!/bin/bash
# series-31a.sh — leg-31A calm-host staggered OH draw series.
# Waits for a calm host (qemu-system count <= 3 for two consecutive
# 60s polls, and >= 12G available memory) before EACH 45-min draw,
# then runs the next draw of the series sequentially (max 1 of my own
# QEMUs at a time). Writes per-draw tallies into run/series-31a-tally.txt
# as each draw completes. Detached-friendly (setsid).
set -uo pipefail
RUN="/home/william/rux-agents/oh-kernel/run"
cd "$RUN"

calm() {
    local n
    n=$(pgrep -c qemu-system 2>/dev/null || echo 99)
    local avail
    avail=$(free -m | awk '/^内存|^Mem:/{print $7}')
    [ "$n" -le 3 ] && [ "${avail:-0}" -ge 12288 ]
}

wait_calm() {
    while ! (calm && sleep 60 && calm); do
        sleep 60
    done
}

DRAWS="${DRAWS:-331A1 331A2 331A3}"
PORTS="${PORTS:-12231:12232:12233:12234}"

: > series-31a-tally.txt
i=0
for TAG in $DRAWS; do
    i=$((i+1))
    PORT=$(echo "$PORTS" | cut -d: -f$i)
    echo "[series] $(date +%H:%M:%S) waiting for calm before draw $TAG" >> series-31a-tally.txt
    wait_calm
    echo "[series] $(date +%H:%M:%S) calm; starting draw $TAG (qemu=$(pgrep -c qemu-system))" >> series-31a-tally.txt
    HDCPORT="$PORT" SECS=2700 ./oh-boot-31a.sh "$TAG" > "oh31a-$TAG-driver.log" 2>&1
    echo "[series] $(date +%H:%M:%S) draw $TAG finished; tally:" >> series-31a-tally.txt
    ./tally-31a.sh "oh31a-$TAG-serial.log" >> series-31a-tally.txt 2>&1
done
echo "[series] $(date +%H:%M:%S) SERIES COMPLETE" >> series-31a-tally.txt
