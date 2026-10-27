#!/bin/bash
# pa60-census.sh — leg-38 debt 1: Tier-1 census gate on PURE main
# b4da2106 (the CURRENT main tip; the pa57 census ran b332d08a, one
# class behind) + the mm/socket domain checks owed on the new commits:
#   - 2968ad13 fix(mm): COW private-copy alloc out of the PTE section
#     (mm/COW is a GUARDED domain; census rows + tick/volume + the
#     p3b_cow pbench row on the main strong arm below watch it)
#   - 7a6e1bf4 fix(mm): small_alloc magazine two-CPU entry (the gate's
#     small_alloc REST-share domain row)
#   - 3e7186b6 + 84743e84 net batch MSG_MORE/TIME_WAIT (IPC/socket
#     domain: the pbench main arm's p5b/p5c rows re-measure the socket
#     family on this class)
# Also adjudicates the two pa57main FAIL rows (tick cpu0 99.43/s
# marginal; volume 85.8M at load 15-18 — rerun-once rule) and the
# pa57main code-1 OUT 11.77s slowtxn row (quiet-class evidence or not).
#
# Chain (load-gated; <=1 lane QEMU at any moment; waits for the pa57
# A/B chain to release its slot first):
#   1. two-stage quiet gate (first 2 h load1<=9/qemus<=8, then 13/10;
#      6 h deadline). On window: census boot pa60main 780 s
#      (pa-boot.sh, syscost=1), parse perf-gate.sh --skip-boot, then
#      pa46-slowtxn.sh.
#   2. quiet re-gate (load1<=13 AND qemus<=10, up to 90 min), then the
#      adjacent pbench pair M,B,B,M (M = run/pa60-a-main-bzImage pure
#      main b4da2106 strong; B = run/pa60-b-branch-bzImage branch HEAD
#      strong) — the route-1 p6_mkdrmdir/p6_unlink medians debt AND the
#      socket-family domain re-measure (M arm) in one pair. Skipped
#      (logged) if either kernel is missing.
# Parses: run/pa60-gate-parse.txt + run/pa60-slowtxn-parse.txt;
# pbench rows to run/pa60-pb-<i><arm>.log.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa60-census.log
KERNEL=/home/william/rux-agents/pa52-main/target/x86_64-unknown-none/debug/bzImage
M=run/pa60-a-main-bzImage
B=run/pa60-b-branch-bzImage

echo "=== pa60-census armed $(date '+%F %T') (target: pure main b4da2106; two-stage gate 9/8 then 13/10, deadline 6h; pbench pair M,B,B,M chained) ===" >> "$LOG"

# Stage 0: wait for the pa57 A/B chain to finish (it owns the QEMU slot).
while pgrep -f "pa57-census.sh" > /dev/null 2>&1; do
  echo "$(date '+%F %T') waiting: pa57-census chain still running" >> "$LOG"
  sleep 180
done
echo "$(date '+%F %T') pa57 chain released the slot" >> "$LOG"
if [ ! -f "$KERNEL" ]; then
  echo "$(date '+%F %T') census kernel MISSING ($KERNEL) — aborting" >> "$LOG"
  exit 3
fi

DEADLINE=$(( $(date +%s) + 6*3600 ))
STAGE1_END=$(( $(date +%s) + 2*3600 ))
BOOTED=0
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  l1=$(awk '{printf "%d", $1}' /proc/loadavg)
  q=$(pgrep -c qemu-system 2>/dev/null || echo 99)
  if [ "$(date +%s)" -lt "$STAGE1_END" ]; then GATE_L=9; GATE_Q=8; else GATE_L=13; GATE_Q=10; fi
  echo "$(date '+%F %T') gate check: load1=$l1 qemus=$q (stage gate <=${GATE_L}/<=${GATE_Q})" >> "$LOG"
  if [ "$l1" -le "$GATE_L" ] && [ "$q" -le "$GATE_Q" ]; then
    echo "$(date '+%F %T') window open: census boot pa60main start (kernel=$KERNEL)" >> "$LOG"
    APPEND_EXTRA="syscost=1 sprobe=0" bash run/pa-boot.sh pa60main "$KERNEL" 780 >> "$LOG" 2>&1
    rc=$?
    echo "$(date '+%F %T') boot pa60main rc=$rc" >> "$LOG"
    BOOTED=1
    break
  fi
  sleep 120
done
if [ "$BOOTED" -ne 1 ]; then
  echo "=== pa60-census NO WINDOW within 6h deadline ($(date '+%F %T')); pbench pair NOT run — re-arm BOTH ===" >> "$LOG"
  exit 4
fi

# Parse: Tier-1 gate rows, then the binder SLOW-TXN domain check.
bash run/perf-gate.sh --skip-boot --log run/pa-pa60main-serial.log --tag pa60main \
  > run/pa60-gate-parse.txt 2>&1
grc=$?
echo "$(date '+%F %T') perf-gate parse rc=$grc (run/pa60-gate-parse.txt)" >> "$LOG"
bash run/pa46-slowtxn.sh run/pa-pa60main-serial.log > run/pa60-slowtxn-parse.txt 2>&1
src=$?
echo "$(date '+%F %T') slowtxn parse rc=$src (run/pa60-slowtxn-parse.txt)" >> "$LOG"

# Adjacent pbench pair M,B,B,M in a fresh quiet window (<=90 min wait).
ABDL=$(( $(date +%s) + 90*60 ))
until [ "$(date +%s)" -ge "$ABDL" ]; do
  l1=$(awk '{printf "%d", $1}' /proc/loadavg)
  q=$(pgrep -c qemu-system 2>/dev/null || echo 99)
  [ "$l1" -le 13 ] && [ "$q" -le 10 ] && break
  echo "$(date '+%F %T') pbench pre-gate waiting: load1=$l1 qemus=$q" >> "$LOG"
  sleep 120
done
if [ ! -f "$M" ] || [ ! -f "$B" ]; then
  echo "$(date '+%F %T') pbench pair SKIPPED — kernel(s) missing: M=$M B=$B" >> "$LOG"
  echo "=== pa60-census complete (census done, pair skipped) $(date '+%F %T') ===" >> "$LOG"
  exit 0
fi
echo "=== pa60-pbench pair start $(date '+%F %T') orders M,B,B,M (M=pure main b4da2106 strong, B=branch HEAD strong) ===" >> "$LOG"
i=0
for arm in M B B M; do
  i=$((i+1))
  tag="pa60pb${i}${arm}"
  k="$M"; [ "$arm" = "B" ] && k="$B"
  echo "--- boot $i arm=$arm tag=$tag $(date '+%F %T') ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
      > "run/pa60-pb-${i}${arm}.log" 2>&1
  rc=$?
  echo "boot $i arm=$arm rc=$rc ($(date '+%F %T'))" >> "$LOG"
done
echo "=== pa60-census+pbench complete $(date '+%F %T') ===" >> "$LOG"
exit 0
