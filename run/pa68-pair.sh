#!/bin/bash
# pa68-pair.sh — leg-43: p4_fork_exit arbitration pair + main-tip re-anchor.
#   M(tag arm A) = run/pa68-main488-bzImage — main 488528dc strong.
#       The FIRST main tip carrying the branch levers (wake/epoll empty
#       fast paths 6fdada6b + socket p5 fast path b3757d92 + procfile;
#       route-4 + scoped-publication draining + F4-R2 already main-side).
#   B8(tag arm B) = run/pa66-b-branch-bzImage — branch 8be8a315 strong
#       (the kernel that read p4_fork_exit 662.9/649.9 in pa66; same
#       kernel file, re-run as a regime guard).
# Questions this pair answers:
#   1. p4_fork_exit: does main-with-levers read the 420-460 class (then
#      the pa66 650-class is a branch-stack interaction or a two-boot
#      coincidence — NOT the levers, they are all in M) or the 650 class
#      (then a lever now regresses MAIN and must be bisected further)?
#   2. p5b regime: M carries the socket lever — if M still spans the
#      5/41 regimes while B8 stays 5-class, the "lever forces fast
#      regime" claim survives; if M reads 5-class tight, adjudication
#      needs the regime-matched rerun on a lever-less control.
#   3. Full-board re-anchor of main 488528dc strong (pa67 A was 0cd29a75).
# Discipline: <=2 lane QEMUs; interleaved M,B8,B8,M cancels drift;
# hard load1<=45 ceiling per boot.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa68-pair.log
M=run/pa68-main488-bzImage
B8=run/pa66-b-branch-bzImage

stamp() { date '+%F %T'; }
l1() { awk '{printf "%d", $1}' /proc/loadavg; }
lane_q() {
  local n=0
  for pid in $(pgrep qemu-system 2>/dev/null); do
    if tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q 'rux-agents/perf-audit'; then
      n=$((n+1))
    fi
  done
  echo "$n"
}

echo "=== pa68 pair armed $(stamp) (M=$M B8=$B8) ===" >> "$LOG"
i=0
for arm in M B8 B8 M; do
  i=$((i+1)); tag="pa68pb${i}${arm}"
  k="$M"; [ "$arm" = "B8" ] && k="$B8"
  while [ "$(lane_q)" -ge 2 ] || [ "$(l1)" -gt 45 ]; do
    echo "$(stamp) boot $i arm=$arm wait: lane_q=$(lane_q) load1=$(l1)" >> "$LOG"
    sleep 60
  done
  echo "--- boot $i arm=$arm tag=$tag $(stamp) load1=$(l1) lane_q=$(lane_q) ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
      > "run/pa68-pb-${i}${arm}.log" 2>&1
  echo "boot $i arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done
echo "=== pa68 pair complete $(stamp) ===" >> "$LOG"
