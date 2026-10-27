#!/bin/bash
# pa39-watch.sh — leg-30 standing watch (perf-audit, C-line, one-mile
# waiting leg). One line per 120s tick appended to run/pa39-watch.log:
#   - origin/main head. kernel_new = kernel/ commits on origin/main
#     since the leg-29 merge 257698a5 (kernel content == fee7774f,
#     the pa38 census baseline). kernel_new>0 = census-hold gate.
#     (Leg-29 hash-tripwire lesson applied: watch main movement itself,
#     not fix hashes — fixes land as NEW hashes after rebase.)
#   - THE TRIGGER: milestone markers (launcher.ready SET evidence
#     "key: bootevent.launcher.ready" / OnForeground / ArkUI / PBENCH)
#     in ANY oh-sc2xx serial modified since watcher start (pa37 grepped
#     only the newest file; a newer empty log could mask a live hit).
#   - samgr-callback (launcher lane) head — the line attacking the
#     appspawn NULL-deref rip=0x7ff082996bac + launcher EBADF fd 32
#     spin; merged main 257698a5 at 52d9f946, so their boots are
#     main+delta (kernel-class hint for any milestone hit).
# Read-only: git fetch + greps. No QEMU, no writes outside this log.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa39-watch.log
STAMP=run/pa39-watch.started
touch "$STAMP"
while true; do
  ts=$(date '+%F %T')
  git fetch -q origin 2>>"$LOG"
  om=$(git rev-parse --short origin/main 2>/dev/null)
  kc=$(git log --oneline 257698a5..origin/main -- kernel/ 2>/dev/null | wc -l)
  ms=none; hits=0; newest=-
  for f in $(find /home/william/rux-agents/samgr-callback/run \
                     /home/william/rux-agents/sc166k/run \
                     /home/william/rux-agents/launcher-route/run \
                     /home/william/rux-agents/ui-launcher/run \
                     -maxdepth 1 -name 'oh-sc2*-serial.log' -newer "$STAMP" 2>/dev/null); do
    n=$(grep -c -E 'key: bootevent\.launcher\.ready|OnForeground|ArkUI|PBENCH ' "$f" 2>/dev/null)
    if [ "${n:-0}" -gt 0 ]; then
      hits=$((hits + n)); ms="HIT($(basename "$f") n=$n)"
    fi
    newest=$(basename "$f")
  done
  [ "$hits" -gt 0 ] || ms=none
  kn=$(git -C /home/william/rux-agents/samgr-callback rev-parse --short HEAD 2>/dev/null)
  om_in_kn=no
  git -C /home/william/rux-agents/samgr-callback merge-base --is-ancestor \
      "origin/main" HEAD 2>/dev/null && om_in_kn=yes
  echo "$ts main=$om kernel_new=$kc milestones=$ms sc_newest=$newest samgr_head=$kn samgr_has_main=$om_in_kn" >> "$LOG"
  sleep 120
done
