#!/bin/bash
# oh-shotmon.sh — continuous screendump + shotstat watcher for OH boots.
#
# Every SECS (default 30) takes a screendump of each listed QEMU monitor
# socket, runs shotstat on it, and appends one CSV line per boot to
# run/oh-shotmon.csv. The first dump with non-black pixels also writes
# run/oh-shotmon-MILESTONE (path + stats) — the acceptance shot.
#
# Usage: oh-shotmon.sh <tag>[:<tag2>...] [secs]
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAGS="${1:?usage: oh-shotmon.sh <tag>[:<tag2>...] [secs]}"
SECS="${2:-30}"
CSV="$ROOT/run/oh-shotmon.csv"
MS="$ROOT/run/oh-shotmon-MILESTONE"
[ -f "$CSV" ] || echo "time,tag,shot,colors,nonblack,nonblack_pct" > "$CSV"

while true; do
    now=$(date +%H:%M:%S)
    IFS=':' read -ra list <<< "$TAGS"
    for tag in "${list[@]}"; do
        sock="$ROOT/run/oh-$tag-mon.sock"
        [ -S "$sock" ] || continue
        n=$(ls "$ROOT/run/oh-$tag"-shot*.ppm "$ROOT/run/oh-$tag"-dev*.ppm 2>/dev/null | wc -l)
        shot="$ROOT/run/oh-$tag-auto$n.ppm"
        # name the virtio-gpu device: the default QEMU console is a
        # 720x400 dummy, NOT the scanout surface (probe gradient proved
        # pixels only appear on the device console)
        python3 "$ROOT/scripts/qemu-mon.py" "$sock" \
            "screendump $shot ohgpu" >/dev/null 2>&1 || continue
        [ -f "$shot" ] || continue
        stats=$(python3 "$ROOT/test/debug/shotstat.py" "$shot" 2>/dev/null)
        colors=$(echo "$stats" | sed -n 's/^distinct colors: \([0-9]*\)/\1/p')
        nb=$(echo "$stats" | sed -n 's/^non-black: \([0-9]*\) .*/\1/p')
        pct=$(echo "$stats" | sed -n 's/^non-black: [0-9]* (\([0-9.]*\)%).*/\1/p')
        echo "$now,$tag,$shot,${colors:-?},${nb:-?},${pct:-?}" >> "$CSV"
        # MILESTONE: more than one distinct color and real non-black share.
        if [ "${colors:-0}" -gt 1 ] && [ "${nb:-0}" -gt 1000 ]; then
            {
                echo "MILESTONE $now $shot"
                echo "$stats"
            } >> "$MS"
        fi
    done
    sleep "$SECS"
done
