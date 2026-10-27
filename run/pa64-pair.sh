#!/bin/bash
# pa64-pair.sh — leg-41 track-2: the route-4 fs-row pbench pair.
# The pa63 chain's B arm was banked PRE-route-4 (11:43 build, route-4
# landed 12:26), so the owed "re-measure the fs rows" after route-4
# needs its own pair on post-route-4 kernels:
#   A = run/pa63-a-main-bzImage  — pure main 94c670da strong
#       (= main BEFORE route-4/D-6-follow-on landed there).
#   B = run/pa64-b-branch-bzImage — post-merge branch strong
#       (235bd6a7 = b7276078 + branch): carries route-4 (601a8de5) +
#       D-6 builder conversions + the branch's wake/epoll levers.
# The B-A delta on the fs rows is dominated by route-4 (wake/epoll
# touches no fs path; the builder conversions are dispatch-identical
# by construction — fileops-caps pins the matrices).
# Target rows: p6_getdents (route-4's row: pbench loops ONE fd, the
# cached-snapshot class), p6_mkdrmdir / p6_unlink (epoch-bump cost on
# the 9 mutation dispatch sites), p6_write_fsync / p6_append (guard
# rows: route-4 must not move them).
# Discipline: <=2 LANE QEMUs (perf-audit-owned) at any moment — this
# script waits while the pa63 chain holds one; no quiet-gate (the
# foreign queue has held load 20-33 all afternoon; interleaved
# A,B,B,A cancels drift, per the pa57/pa63 protocol). Only a hard
# load1<=45 ceiling per boot to avoid pathological thrash.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa64-pair.log
A=run/pa63-a-main-bzImage
B=run/pa64-b-branch-bzImage

stamp() { date '+%F %T'; }
l1() { awk '{printf "%d", $1}' /proc/loadavg; }
lane_q() {
  # QEMUs whose cmdline references this worktree (AGENTS.md PID-map rule)
  local n=0
  for pid in $(pgrep qemu-system 2>/dev/null); do
    if tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q 'rux-agents/perf-audit'; then
      n=$((n+1))
    fi
  done
  echo "$n"
}

echo "=== pa64 route-4 fs-row pair armed $(stamp) (A=$A B=$B) ===" >> "$LOG"
i=0
for arm in A B B A; do
  i=$((i+1)); tag="pa64pb${i}${arm}"
  k="$A"; [ "$arm" = "B" ] && k="$B"
  # wait for a lane slot (chain boot or suite boot holding one) + sane host
  while [ "$(lane_q)" -ge 2 ] || [ "$(l1)" -gt 45 ]; do
    echo "$(stamp) boot $i arm=$arm wait: lane_q=$(lane_q) load1=$(l1)" >> "$LOG"
    sleep 60
  done
  echo "--- boot $i arm=$arm tag=$tag $(stamp) load1=$(l1) lane_q=$(lane_q) ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
      > "run/pa64-pb-${i}${arm}.log" 2>&1
  echo "boot $i arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done
echo "=== pa64-pair complete $(stamp) ===" >> "$LOG"
