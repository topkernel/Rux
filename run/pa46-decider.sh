#!/bin/bash
# pa46-decider.sh — leg-34 census queue, load-gated. TWO boots, each in
# its own quiet window, one QEMU at a time:
#   Boot A: pa46main  run/pa46-main-bzImage  (main 47d96961 debug) —
#           the owed Tier-1 census gate on the current main class
#           (census 8+-2/32+-4, ticks, per-op) + volume/nanosleep
#           re-bank rows + BINDER SLOW-TXN domain check (sweep_stale_
#           async reclaim changed the binder buffer lifecycle) + the
#           boot-wall reference for the F4-R2 comparison.
#   Boot B: pa46f4r2  run/pa46-f4r2-bzImage  (47d96961 + fd23662a,
#           the F4-R2 free-counter deferral) — F4-R2 Tier-1 census
#           (fs-touching change owes the census before any merge claim)
#           + the boot-wall comparison vs boot A.
# Gate before EACH boot: load1 <= GATE_LOAD (13) AND qemu count <=
#   GATE_QEMU (10). Overall deadline 5h from arm -> exit 4 (whatever
#   opened, ran; the rest is reported as no-window).
# Results to run/pa46-decider.log; stamps in run/pa-pa46main-* and
# run/pa-pa46f4r2-* (parse with perf-gate.sh --skip-boot --log).
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa46-decider.log
GATE_LOAD="${GATE_LOAD:-13}"
GATE_QEMU="${GATE_QEMU:-10}"
DEADLINE=$(( $(date +%s) + 5*3600 ))

gate_open() {
  local l1 q
  l1=$(awk '{printf "%d", $1}' /proc/loadavg)
  q=$(pgrep -c qemu-system 2>/dev/null || echo 99)
  echo "$(date '+%F %T') gate check: load1=$l1 qemus=$q (need <=${GATE_LOAD}/<=${GATE_QEMU})" >> "$LOG"
  [ "$l1" -le "$GATE_LOAD" ] && [ "$q" -le "$GATE_QEMU" ]
}

wait_gate() {
  while [ "$(date +%s)" -lt "$DEADLINE" ]; do
    gate_open && return 0
    sleep 120
  done
  return 1
}

run_boot() { # <tag> <bzimage>
  local tag="$1" bz="$2"
  if wait_gate; then
    echo "$(date '+%F %T') window open: census boot $tag ($bz) start" >> "$LOG"
    APPEND_EXTRA="syscost=1 sprobe=0" bash run/pa-boot.sh "$tag" "$bz" 780 >> "$LOG" 2>&1
    echo "$(date '+%F %T') boot $tag rc=$?" >> "$LOG"
    return 0
  fi
  echo "$(date '+%F %T') NO WINDOW within deadline (boot $tag)" >> "$LOG"
  return 4
}

echo "=== pa46-decider armed $(date '+%F %T') load-gate=${GATE_LOAD} qemu-gate=${GATE_QEMU} deadline=5h ===" >> "$LOG"
rcA=0; rcB=0
run_boot pa46main run/pa46-main-bzImage || rcA=$?
[ "$rcA" = "0" ] && run_boot pa46f4r2 run/pa46-f4r2-bzImage || rcB=$?
echo "=== pa46-decider complete $(date '+%F %T') rcA=$rcA rcB=$rcB ===" >> "$LOG"
exit $(( rcA != 0 ? rcA : rcB ))
