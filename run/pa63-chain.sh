#!/bin/bash
# pa63-chain.sh — leg-39 quiet-window chain (2026-10-26 ~11:45; host
# load<1, 0 foreign QEMUs after the ~09:30 reboot — the quietest window
# in days; leg-38's pa60/pa61 deciders died unfired in that reboot).
# Re-arms ALL owed measurements on the 94c670da class in one serialized
# chain (<=1 lane QEMU at any moment):
#   1. census boot pa63main 780 s (pure main 94c670da x86 debug, built
#      in pa52-main, pa63-build-census.log RC=0) — Tier-1 + mm domain
#      rows + adjudication of the pa57 FAIL rows (tick 99.43 marginal,
#      volume 85.8M rerun-once) + the quiet code-1 slowtxn band question.
#   2. perf-gate parse + pa46-slowtxn parse (run/pa63-*-parse.txt).
#   3. pbench pair M,B,B,M (M = run/pa63-a-main-bzImage pure main
#      strong; B = run/pa63-b-branch-bzImage branch HEAD strong) —
#      fs domain rows on the class that now CARRIES F4 route-1 in main
#      (p6_mkdrmdir/p6_unlink medians + p3b_cow COW-fix watch + p5
#      net-batch re-measure) AND the D-7 branch-delta functional smoke.
#   4. LTP spot pa63ltp-r1 (17 metadata/create rows, branch riscv64
#      debug, run/pa59-ltp-r1.sh) — route-1 create-loop regression
#      class vs fleet baseline (route-1 now IN main: evidence banks on
#      the branch kernel = same route-1 code).
#   5. fsw pair: mech pa63fswm (branch x86 debug, fsw=1, WITH the FSW1
#      sd separator fix) vs ctl pa63fswc (pure main x86 debug = the
#      census kernel, fsw=1) — the honest sync-write split + route-1
#      publication-queue counters (pb) + buffer-cache hit rows.
#   6. pa55-protocol A/B re-run on the CURRENT class: A = M (pure main
#      strong), B = run/pa63-ab-fast-bzImage (main + the branch's exact
#      two-file wake/epoll delta: L3 sharding + 6fdada6b fast paths,
#      wait.rs+misc.rs verbatim from feature/perf-audit HEAD — 6fdada6b
#      alone does not apply to main, its epoll half presupposes the
#      sharding). Orders A,B,B,A,A,B. NOTE: 8.10.9 already banked this
#      lever's A/B on the b332d08a class; this re-run is the merge-shape
#      confirmation on 94c670da + single-regime data for the p5b family.
# Skips (with log lines, never aborts the chain) on missing kernels or
# pbench_boot's foreign-QEMU discipline; LTP waits for its build.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa63-chain.log
CENK=/home/william/rux-agents/pa52-main/target/x86_64-unknown-none/debug/bzImage
M=run/pa63-a-main-bzImage
B=run/pa63-b-branch-bzImage
AB=run/pa63-ab-fast-bzImage
LTPK=run/pa63-ltp-branch-rux
MECHK=run/pa63-fswm-bzImage

stamp() { date '+%F %T'; }
qcount() { pgrep -c qemu-system 2>/dev/null || echo 0; }
l1() { awk '{printf "%d", $1}' /proc/loadavg; }

echo "=== pa63-chain armed $(stamp) (class: main 94c670da / branch $(git rev-parse --short HEAD)) ===" >> "$LOG"

# Stage 0: two-stage quiet gate, pa60 discipline (9/8 for the first
# 2 h, then 13/10; 5 h hard deadline) — the foreign agent queue
# auto-resumed at ~11:35 (7 QEMUs, load 15), and the census boot MUST
# be quiet-class evidence (the pa57 FAIL adjudication depends on it).
GD=$(( $(date +%s) + 5*3600 ))
S1=$(( $(date +%s) + 2*3600 ))
while :; do
  [ "$(date +%s)" -ge "$GD" ] && break
  if [ "$(date +%s)" -lt "$S1" ]; then GL=9; GQ=8; else GL=13; GQ=10; fi
  [ "$(l1)" -le "$GL" ] && [ "$(qcount)" -le "$GQ" ] && break
  echo "$(stamp) stage0 gate wait: load1=$(l1) qemus=$(qcount) (need <=${GL}/<=${GQ})" >> "$LOG"
  sleep 120
done
echo "$(stamp) stage0 done: load1=$(l1) qemus=$(qcount)" >> "$LOG"

# Stage 1: census boot.
if [ -f "$CENK" ]; then
  echo "$(stamp) census boot pa63main start (kernel=$CENK load1=$(l1) qemus=$(qcount))" >> "$LOG"
  APPEND_EXTRA="syscost=1 sprobe=0" bash run/pa-boot.sh pa63main "$CENK" 780 >> "$LOG" 2>&1
  echo "$(stamp) census boot pa63main rc=$?" >> "$LOG"
else
  echo "$(stamp) census SKIPPED — kernel missing" >> "$LOG"
fi

# Stage 2: parses.
if [ -f run/pa-pa63main-serial.log ]; then
  bash run/perf-gate.sh --skip-boot --log run/pa-pa63main-serial.log --tag pa63main \
    > run/pa63-gate-parse.txt 2>&1
  echo "$(stamp) perf-gate parse rc=$? (run/pa63-gate-parse.txt)" >> "$LOG"
  bash run/pa46-slowtxn.sh run/pa-pa63main-serial.log > run/pa63-slowtxn-parse.txt 2>&1
  echo "$(stamp) slowtxn parse rc=$? (run/pa63-slowtxn-parse.txt)" >> "$LOG"
else
  echo "$(stamp) parses SKIPPED — no census serial" >> "$LOG"
fi

# Stage 3: pbench pair M,B,B,M (light re-gate 13/10, max 60 min —
# medians ride interleaved orders, but prefer a sane window).
if [ -f "$M" ] && [ -f "$B" ]; then
  GD=$(( $(date +%s) + 60*60 ))
  while [ "$(date +%s)" -lt "$GD" ]; do
    [ "$(l1)" -le 13 ] && [ "$(qcount)" -le 10 ] && break
    echo "$(stamp) pair pre-gate wait: load1=$(l1) qemus=$(qcount)" >> "$LOG"
    sleep 120
  done
  echo "=== pa63 pbench pair start $(stamp) orders M,B,B,M (M=pure main 94c670da strong, B=branch HEAD strong) load1=$(l1) qemus=$(qcount) ===" >> "$LOG"
  i=0
  for arm in M B B M; do
    i=$((i+1)); tag="pa63pb${i}${arm}"
    k="$M"; [ "$arm" = "B" ] && k="$B"
    echo "--- boot $i arm=$arm tag=$tag $(stamp) load1=$(l1) qemus=$(qcount) ---" >> "$LOG"
    python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
        --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
        > "run/pa63-pb-${i}${arm}.log" 2>&1
    echo "boot $i arm=$arm rc=$? ($(stamp))" >> "$LOG"
    sleep 20
  done
else
  echo "$(stamp) pbench pair SKIPPED — kernels missing M=$M B=$B" >> "$LOG"
fi

# Stage 4: LTP spot (waits for the riscv64 debug build, max 45 min).
if [ -f run/pa59-ltp-r1-rootfs.img ]; then
  GD=$(( $(date +%s) + 45*60 ))
  while [ ! -f "$LTPK" ] && [ "$(date +%s)" -lt "$GD" ]; do
    echo "$(stamp) LTP kernel wait ($LTPK)" >> "$LOG"; sleep 60
  done
  if [ -f "$LTPK" ]; then
    echo "$(stamp) LTP spot boot pa63ltp-r1 start (kernel=$LTPK)" >> "$LOG"
    bash run/pa59-ltp-r1.sh "$LTPK" pa63ltp-r1 >> "$LOG" 2>&1
    for n in $(seq 1 40); do
      sleep 30
      grep -q "SWEEP DONE\|power-down\|reboot: " run/pa-pa63ltp-r1-serial.log 2>/dev/null && break
    done
    sleep 15
    echo "$(stamp) LTP spot finished (run/pa-pa63ltp-r1-serial.log)" >> "$LOG"
  else
    echo "$(stamp) LTP spot SKIPPED — kernel never appeared" >> "$LOG"
  fi
else
  echo "$(stamp) LTP spot SKIPPED — rootfs missing" >> "$LOG"
fi

# Stage 5: fsw mechanism + control pair.
if [ -f "$MECHK" ] && [ -f "$CENK" ]; then
  for spec in "pa63fswm:$MECHK:mech" "pa63fswc:$CENK:ctl"; do
    tag="${spec%%:*}"; rest="${spec#*:}"; k="${rest%%:*}"; which="${rest##*:}"
    echo "$(stamp) fsw boot $which tag=$tag start (kernel=$k)" >> "$LOG"
    APPEND_EXTRA="fsw=1" bash run/pa-boot.sh "$tag" "$k" 480 >> "$LOG" 2>&1
    echo "$(stamp) fsw boot $which tag=$tag rc=$?" >> "$LOG"
  done
else
  echo "$(stamp) fsw pair SKIPPED — mech=$MECHK ctl=$CENK" >> "$LOG"
fi

# Stage 6: wake/epoll A/B on the current class (A=M reuse, B=ab-fast;
# light re-gate 13/10, max 60 min).
if [ -f "$AB" ]; then
  GD=$(( $(date +%s) + 60*60 ))
  while [ "$(date +%s)" -lt "$GD" ]; do
    [ "$(l1)" -le 13 ] && [ "$(qcount)" -le 10 ] && break
    echo "$(stamp) A/B pre-gate wait: load1=$(l1) qemus=$(qcount)" >> "$LOG"
    sleep 120
  done
  echo "=== pa63 A/B start $(stamp) orders A,B,B,A,A,B (A=pure main strong, B=main+wait/epoll two-file delta) load1=$(l1) qemus=$(qcount) ===" >> "$LOG"
  i=0
  for arm in A B B A A B; do
    i=$((i+1)); tag="pa63ab${i}${arm}"
    k="$M"; [ "$arm" = "B" ] && k="$AB"
    echo "--- boot $i arm=$arm tag=$tag $(stamp) load1=$(l1) qemus=$(qcount) ---" >> "$LOG"
    python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
        --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
        > "run/pa63-ab-${i}${arm}.log" 2>&1
    echo "boot $i arm=$arm rc=$? ($(stamp))" >> "$LOG"
    sleep 20
  done
else
  echo "$(stamp) A/B SKIPPED — ab kernel missing" >> "$LOG"
fi
echo "=== pa63-chain complete $(stamp) ===" >> "$LOG"
