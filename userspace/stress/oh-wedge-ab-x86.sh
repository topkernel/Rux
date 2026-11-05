#!/bin/bash
# oh-wedge-ab-x86.sh — OH-lane wedge-rate A/B through the stress-port rig
# (joint run per the OH 27th close entry point 2): 5x cg26b vs 5x
# pollwake rolls, identical recipe, interleaved to cancel host-load
# drift. Wedge adjudication is done POST-HOC from the serial logs
# (see TASK.md round-9 Q4 for the class definitions); this driver
# only boots and preserves logs.
#
# Usage: test/stressng/oh-wedge-ab-x86.sh [SECS_PER_ROLL]
# Logs land in the oh-launcher worktree run/ as oh-ab9cg-N-serial.log /
# oh-ab9pw-N-serial.log (their rig's convention, visible to that lane).
set -uo pipefail
cd "$(dirname "$0")/../.."

SECS="${1:-1500}"
OHRIG=/home/william/rux-agents/oh-launcher
SYSIMG=run/system-lb5.img
RECIPE="SMP=2 LOGLEVEL=6 USERDATA=run/userdata-fresh2.img"

declare -A KERN=( [cg]="$OHRIG/run/bzImage-ladder-cg26b" [pw]="$OHRIG/run/bzImage-ladder-pollwake" )

for i in 1 2 3 4 5; do
  for side in cg pw; do
    TAG="ab9${side}-${i}"
    LOG="$OHRIG/run/oh-${TAG}-serial.log"
    echo "=== $(date +%H:%M:%S) roll $TAG (kernel ${KERN[$side]}) ==="
    ( cd "$OHRIG" && env $RECIPE python3 run/lb-boot.py "$TAG" "$SECS" "$SYSIMG" \
        "${KERN[$side]}" " debug=ams46,txnhunt,fuxstuck,slowtask negmemo=1" \
        > /dev/null 2>&1 )
    echo "=== $(date +%H:%M:%S) roll $TAG done: $(wc -l < "$LOG" 2>/dev/null || echo NOLOG) serial lines ==="
    # small cool-down so the next roll's QEMU never overlaps this one's teardown
    sleep 20
  done
done
echo "A/B complete: $(date)"
