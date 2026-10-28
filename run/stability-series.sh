#!/bin/bash
# stability-series.sh — consecutive boots to the GNOME desktop, no manual
# intervention (leg-5 mission item 3). Runs fire.py N times back-to-back
# with the validation config (r5 image, smp2/2G, session mode, watchdog)
# and collects each roll's verdict into series.log.
#
# Usage: setsid bash run/stability-series.sh [N] [PREFIX] [SECONDS] &
set -u
cd "$(dirname "$0")/.."

N="${1:-3}"
PREFIX="${2:-stab}"
SECS="${3:-1800}"

SERIES_LOG="runs/gnome-${PREFIX}-series.log"
: > "$SERIES_LOG"
echo "series start $(date -Is) n=$N prefix=$PREFIX secs=$SECS kernel=$(git rev-parse --short HEAD)" >> "$SERIES_LOG"

for i in $(seq 1 "$N"); do
  tag="${PREFIX}${i}"
  echo "== boot $i/$N tag=$tag start $(date -Is)" >> "$SERIES_LOG"
  python3 test/ubuntu-gnome/fire.py --tag "$tag" --seconds "$SECS" \
    --img work/gnome-r5.img --dfx "watchdog,taskdump" \
    >> "$SERIES_LOG" 2>&1
  rc=$?
  verdict=$(grep -E "^verdict:" "runs/gnome-$tag/summary.txt" 2>/dev/null | head -1)
  m4=$(grep -E "M4_screen_lit" "runs/gnome-$tag/summary.txt" 2>/dev/null | head -1)
  panic=$(grep -cE "KERNEL_PANIC" "runs/gnome-$tag/summary.txt" 2>/dev/null)
  echo "== boot $i/$N done rc=$rc $verdict | $m4 | panic_lines=$panic end $(date -Is)" >> "$SERIES_LOG"
done
echo "series end $(date -Is)" >> "$SERIES_LOG"
