#!/bin/bash
# pa36-watch.sh — leg-28 standing watch (perf-audit, C-line).
# One line per 120s tick appended to run/pa36-watch.log:
#   - origin/main head + A-line footprint-fix merge check (trigger 2)
#   - new kernel/ commits on main since census baseline 8480751c,
#     excluding own test-only 015e5488 (census-hold gate)
#   - newest sc2xx serial log + milestone markers (OnForeground/ArkUI/
#     PBENCH) + the samgr-callback worktree HEAD (kernel-class hint)
# Read-only: git fetch + greps only. No QEMU, no writes outside this log.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa36-watch.log
while true; do
  ts=$(date '+%F %T')
  git fetch -q origin 2>>"$LOG"
  om=$(git rev-parse --short origin/main 2>/dev/null)
  fp=no
  for c in d08dca37 07a1e7be; do
    if git merge-base --is-ancestor "$c" origin/main 2>/dev/null; then fp="YES($c)"; fi
  done
  kc=$(git log --oneline 8480751c..origin/main -- kernel/ 2>/dev/null | grep -v '^015e5488' | wc -l)
  newest=$(ls -t /home/william/rux-agents/samgr-callback/run/oh-sc2*-serial.log \
                   /home/william/rux-agents/sc166k/run/oh-sc2*-serial.log 2>/dev/null | head -1)
  ms=none; msn=0; kn=-
  if [ -n "$newest" ]; then
    msn=$(grep -c -E 'OnForeground|ArkUI|PBENCH|launcher\.ready=true' "$newest" 2>/dev/null)
    [ "${msn:-0}" -gt 0 ] && ms="HIT($msn)"
    kn=$(git -C /home/william/rux-agents/samgr-callback rev-parse --short HEAD 2>/dev/null)
  fi
  echo "$ts main=$om footprint=$fp kernel_new=$kc serial=$(basename "$newest" 2>/dev/null) milestones=$ms samgr_head=$kn" >> "$LOG"
  sleep 120
done
