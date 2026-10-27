#!/bin/bash
# pa72-reanchor.sh — leg 46 mission 2: D-7 re-anchor. ONE quiet-window
# banked pbench board on merged main (load1<=9 AND qemus<=8, the pa50
# gate). If no window opens before the deadline, log NO WINDOW and
# exit 4 — the board is NOT banked under load (ledger 8.10.14 standing
# rule: D-7/p5b regime questions need a quiet window; load 10-20
# windows are not comparable).
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa72-reanchor.log
K=run/pa72-main-strong-bzImage
GATE_LOAD="${GATE_LOAD:-9}"
GATE_QEMU="${GATE_QEMU:-8}"
DEADLINE=$(( $(date +%s) + 6*3600 ))

stamp() { date '+%F %T'; }
l1() { awk '{printf "%d", $1}' /proc/loadavg; }
lane_q() {
  local n=0
  for pid in $(pgrep qemu-system-x86_64 2>/dev/null); do
    if tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q 'rux-agents/perf-audit'; then
      n=$((n+1))
    fi
  done
  echo "$n"
}

echo "=== pa72-reanchor armed $(stamp) load-gate=${GATE_LOAD} qemu-gate=${GATE_QEMU} deadline=6h k=$K ===" >> "$LOG"
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  l=$(l1); q=$(pgrep -c qemu-system 2>/dev/null || echo 99); lq=$(lane_q)
  echo "$(stamp) gate: load1=$l qemus=$q lane_q=$lq (need <=${GATE_LOAD}/<=${GATE_QEMU})" >> "$LOG"
  if [ "$l" -le "$GATE_LOAD" ] && [ "$q" -le "$GATE_QEMU" ] && [ "$lq" -le 2 ]; then
    echo "$(stamp) window open: board boot pa72q start" >> "$LOG"
    python3 test/pbench_boot.py --bzimage "$K" --tag pa72q \
        --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
        > run/pa72q-boot.log 2>&1
    rc=$?
    echo "$(stamp) boot pa72q rc=$rc load1_at_end=$(l1)" >> "$LOG"
    echo "=== pa72-reanchor complete $(stamp) rc=$rc ===" >> "$LOG"
    exit $rc
  fi
  sleep 120
done
echo "=== pa72-reanchor NO WINDOW within 6h deadline ($(stamp)) ===" >> "$LOG"
exit 4
