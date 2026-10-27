#!/bin/bash
# pa66-pair.sh — leg-42: the post-D-7-revert, post-main-merge pair.
#   A = run/pa66-a-main57-bzImage — pure main 57e44a90 strong
#       (O_APPEND atomicity 57e44a90 + msync-wrap EINVAL ec87f42b +
#        ext4 dir-entry serialize 81fa78af + exit/sched/procfs fixes;
#        route-4 + D-6 already in via b7276078).
#   B = run/pa66-b-branch-bzImage — branch 8be8a315 strong
#       (A's content + branch-unique levers: socket p5 fast path
#        b3757d92, wake/epoll empty fast paths 6fdada6b, procfile,
#        F4-R1 null-journal, D-6 builder + fileops slice 2; D-7
#        REVERTED after the release-boot break).
# Reading the pair:
#   - B boot completion alone re-opens the branch-strong channel
#     (broken since d8d23064 07:23; pa63/pa64 B arms died at t=3s).
#   - A vs pa65m1 (b7276078 strong, same channel): main-side fs rows —
#     p6_append is the O_APPEND-atomicity re-verify row; p6_mkdrmdir
#     carries the dir-serialize cost; fsync/getdents guard.
#   - B vs A: branch-unique lever rows (p5b family, p2_ctxsw) + fs
#     guards (route-4 on both sides — delta must be ~0 on getdents).
# Discipline: <=2 lane QEMUs; interleaved A,B,B,A cancels drift; hard
# load1<=45 ceiling per boot only (foreign queue holds the afternoon).
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa66-pair.log
A=run/pa66-a-main57-bzImage
B=run/pa66-b-branch-bzImage

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

echo "=== pa66 pair armed $(stamp) (A=$A B=$B) ===" >> "$LOG"
i=0
for arm in A B B A; do
  i=$((i+1)); tag="pa66pb${i}${arm}"
  k="$A"; [ "$arm" = "B" ] && k="$B"
  while [ "$(lane_q)" -ge 2 ] || [ "$(l1)" -gt 45 ]; do
    echo "$(stamp) boot $i arm=$arm wait: lane_q=$(lane_q) load1=$(l1)" >> "$LOG"
    sleep 60
  done
  echo "--- boot $i arm=$arm tag=$tag $(stamp) load1=$(l1) lane_q=$(lane_q) ---" >> "$LOG"
  python3 test/pbench_boot.py --bzimage "$k" --tag "$tag" \
      --accel tcg,thread=multi --smp 2 --mem 2G --timeout 1200 \
      > "run/pa66-pb-${i}${arm}.log" 2>&1
  echo "boot $i arm=$arm rc=$? ($(stamp))" >> "$LOG"
  sleep 20
done
echo "=== pa66 pair complete $(stamp) ===" >> "$LOG"
