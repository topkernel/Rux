#!/bin/bash
# pa52-census.sh — leg-36 standing debt 1: Tier-1 census gate on the
# CURRENT main-class kernel (PURE main a5af759c, x86_64 debug, built in
# worktree pa52-main) + the BINDER SLOW-TXN domain check on the same
# log. a5af759c = ddb3ed01 + one commit (RVV lazy enable) that touches
# kernel/src/process/fork.rs + cmdline.rs + main.rs — fork-path shared
# code, so the census roll is a real check, not a formality.
#
# Two-stage load gate (6 h deadline):
#   stage 1 (first 2 h): load1 <= 9 AND qemus <= 8 — quiet window;
#     with the volume row now banked at 107.5M quiet (pa50main) this
#     gives a chance of a fully green parse AND a second quiet-window
#     datapoint cross-checking the re-bank on the a5af759c class.
#   stage 2 (last 4 h): load1 <= 13 AND qemus <= 10 — the pa46-decider
#     census-gate class; census rows (TERM 8 / REFUSED 32) have held
#     exact at every load, the tick row's flake class is documented,
#     and a loaded volume FAIL carries the pace-coupling caveat.
# Boot is pa-boot.sh directly (perf-gate.sh's own preflight counts ALL
# host QEMUs and would refuse while any fleet QEMU lives); parse with
#   perf-gate.sh --skip-boot --log run/pa-pa52main-serial.log --tag pa52main
# then the binder check with
#   run/pa46-slowtxn.sh run/pa-pa52main-serial.log
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa52-census.log
KERNEL=/home/william/rux-agents/pa52-main/target/x86_64-unknown-none/debug/bzImage
DEADLINE=$(( $(date +%s) + 6*3600 ))
STAGE1_END=$(( $(date +%s) + 2*3600 ))

echo "=== pa52-census armed $(date '+%F %T') two-stage gate 9/8 then 13/10 deadline=6h ===" >> "$LOG"
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  l1=$(awk '{printf "%d", $1}' /proc/loadavg)
  q=$(pgrep -c qemu-system 2>/dev/null || echo 99)
  if [ "$(date +%s)" -lt "$STAGE1_END" ]; then
    GATE_L=9; GATE_Q=8
  else
    GATE_L=13; GATE_Q=10
  fi
  echo "$(date '+%F %T') gate check: load1=$l1 qemus=$q (stage gate <=${GATE_L}/<=${GATE_Q})" >> "$LOG"
  if [ "$l1" -le "$GATE_L" ] && [ "$q" -le "$GATE_Q" ]; then
    echo "$(date '+%F %T') window open: census boot pa52main start (kernel=$KERNEL)" >> "$LOG"
    APPEND_EXTRA="syscost=1 sprobe=0" bash run/pa-boot.sh pa52main "$KERNEL" 780 >> "$LOG" 2>&1
    rc=$?
    echo "$(date '+%F %T') boot pa52main rc=$rc" >> "$LOG"
    echo "=== pa52-census boot complete $(date '+%F %T') rc=$rc ===" >> "$LOG"
    exit $rc
  fi
  sleep 120
done
echo "=== pa52-census NO WINDOW within 6h deadline ($(date '+%F %T')) ===" >> "$LOG"
exit 4
