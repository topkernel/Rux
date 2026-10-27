#!/bin/bash
# pa57-census.sh — leg-37 debt 1: Tier-1 census gate on the CURRENT
# main tip (PURE main b332d08a-class, x86_64 debug, built in worktree
# pa52-main, log run/pa57-build-census.log) + the BINDER SLOW-TXN
# domain check on the same log — then, in the SAME window, the owed
# pa55 A/B re-run of the wake/epoll empty fast paths (6fdada6b) on
# the CURRENT MERGE SHAPE: A = pure main release-strong
# (run/pa57-a-ctl-bzImage), B = branch HEAD release-strong
# (run/pa57-b-fast-bzImage). The branch kernel diff vs main is exactly
# 6fdada6b's two files (wait.rs + misc.rs), so this A/B both settles
# the merge claim AND re-measures the p5b/p5c socket rows on the
# post-p5-merge main class (track-2 accounting).
#
# Why superseded: leg-36's pa52-census targeted a5af759c; main has
# since taken the OH package (sched/binder changes) + the lane's merge
# set (procfile, frrm/lazy-FP, socket p5, F4-R2). The census roll and
# the binder domain check are owed on the tree that will actually be
# merged into. A pure-main boot also adjudicates the open code-14
# SLOW-TXN question (pa56branch): if this boot shows code 14, the
# checker learns the class and the branch is cleared; if not, the
# branch gets a targeted look.
#
# Chain (all load-gated; <=1 lane QEMU at any moment):
#   0. wait for the census debug build (BUILD_RC in the build log)
#   1. two-stage quiet gate (same contract as pa52-census):
#      first 2 h load1<=9/qemus<=8, then 13/10; 6 h deadline.
#      On window: census boot pa57main 780 s (pa-boot.sh, syscost=1),
#      parse perf-gate.sh --skip-boot, then pa46-slowtxn.sh.
#   2. A/B pre-gate (load1<=13 AND qemus<=10, up to 90 min wait):
#      6 pbench boots, orders A,B,B,A,A,B (protocol = 8.10.4,
#      release-strong, TCG thread=multi smp2/2G). Skipped (logged)
#      if either kernel is missing.
# Census parse goes to run/pa57-gate-parse.txt + pa57-slowtxn-parse.txt;
# A/B rows to run/pa57-ab.log + run/pa57-pb-<n><arm>.log.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa57-census.log
BUILDLOG=run/pa57-build-census.log
KERNEL=/home/william/rux-agents/pa52-main/target/x86_64-unknown-none/debug/bzImage
A=run/pa57-a-ctl-bzImage
B=run/pa57-b-fast-bzImage
ABLOG=run/pa57-ab.log

echo "=== pa57-census armed $(date '+%F %T') (target: main b332d08a-class; two-stage gate 9/8 then 13/10, deadline 6h; A/B chained) ===" >> "$LOG"

# Stage 0: wait for the census debug build (max 40 min).
for i in $(seq 1 80); do
  if grep -q "BUILD_RC=" "$BUILDLOG" 2>/dev/null; then break; fi
  sleep 30
done
if ! grep -q "BUILD_RC=0" "$BUILDLOG" 2>/dev/null; then
  echo "$(date '+%F %T') census build FAILED or timed out — aborting (see $BUILDLOG)" >> "$LOG"
  exit 3
fi
echo "$(date '+%F %T') census kernel ready: $KERNEL" >> "$LOG"

DEADLINE=$(( $(date +%s) + 6*3600 ))
STAGE1_END=$(( $(date +%s) + 2*3600 ))
BOOTED=0
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  l1=$(awk '{printf "%d", $1}' /proc/loadavg)
  q=$(pgrep -c qemu-system 2>/dev/null || echo 99)
  if [ "$(date +%s)" -lt "$STAGE1_END" ]; then GATE_L=9; GATE_Q=8; else GATE_L=13; GATE_Q=10; fi
  echo "$(date '+%F %T') gate check: load1=$l1 qemus=$q (stage gate <=${GATE_L}/<=${GATE_Q})" >> "$LOG"
  if [ "$l1" -le "$GATE_L" ] && [ "$q" -le "$GATE_Q" ]; then
    echo "$(date '+%F %T') window open: census boot pa57main start (kernel=$KERNEL)" >> "$LOG"
    APPEND_EXTRA="syscost=1 sprobe=0" bash run/pa-boot.sh pa57main "$KERNEL" 780 >> "$LOG" 2>&1
    rc=$?
    echo "$(date '+%F %T') boot pa57main rc=$rc" >> "$LOG"
    BOOTED=1
    break
  fi
  sleep 120
done
if [ "$BOOTED" -ne 1 ]; then
  echo "=== pa57-census NO WINDOW within 6h deadline ($(date '+%F %T')); A/B NOT run — re-arm BOTH ===" >> "$LOG"
  exit 4
fi

# Parse: Tier-1 gate rows, then the binder SLOW-TXN domain check
# (also adjudicates the pa56branch code-14 open question).
bash run/perf-gate.sh --skip-boot --log run/pa-pa57main-serial.log --tag pa57main \
  > run/pa57-gate-parse.txt 2>&1
grc=$?
echo "$(date '+%F %T') perf-gate parse rc=$grc (run/pa57-gate-parse.txt)" >> "$LOG"
bash run/pa46-slowtxn.sh run/pa-pa57main-serial.log > run/pa57-slowtxn-parse.txt 2>&1
src=$?
echo "$(date '+%F %T') slowtxn parse rc=$src (run/pa57-slowtxn-parse.txt)" >> "$LOG"

# A/B pre-gate: up to 90 min for the quiet class to hold again.
ABDL=$(( $(date +%s) + 90*60 ))
until [ "$(date +%s)" -ge "$ABDL" ]; do
  l1=$(awk '{printf "%d", $1}' /proc/loadavg)
  q=$(pgrep -c qemu-system 2>/dev/null || echo 99)
  [ "$l1" -le 13 ] && [ "$q" -le 10 ] && break
  echo "$(date '+%F %T') A/B pre-gate waiting: load1=$l1 qemus=$q" >> "$LOG"
  sleep 120
done
if [ ! -f "$A" ] || [ ! -f "$B" ]; then
  echo "$(date '+%F %T') A/B SKIPPED — kernel(s) missing: A=$A B=$B (arm pa55-ab.sh with the banked kernels as fallback)" >> "$LOG"
  echo "=== pa57-census complete (census done, A/B skipped) $(date '+%F %T') ===" >> "$LOG"
  exit 0
fi
echo "=== pa57-ab start $(date '+%F %T') orders A,B,B,A,A,B (A=pure main strong, B=branch 6fdada6b strong) ===" >> "$ABLOG"
i=0
for arm in A B B A A B; do
  i=$((i+1))
  tag="pa57${i}${arm}"
  k="$A"; [ "$arm" = "B" ] && k="$B"
  echo "--- boot $i arm=$arm tag=$tag $(date '+%F %T') ---" >> "$ABLOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
      > "run/pa57-pb-${i}${arm}.log" 2>&1
  rc=$?
  echo "boot $i arm=$arm rc=$rc ($(date '+%F %T'))" >> "$ABLOG"
  echo "$(date '+%F %T') ab boot $i arm=$arm rc=$rc" >> "$LOG"
done
echo "=== pa57-ab complete $(date '+%F %T') ===" >> "$ABLOG"
echo "=== pa57-census+A/B complete $(date '+%F %T') ===" >> "$LOG"
exit 0
