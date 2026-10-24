#!/bin/bash
# pa40-watch.sh — leg-31 standing watch (perf-audit, C-line). One line
# per 120s tick appended to run/pa40-watch.log:
#   - omain/lmain: origin/main and LOCAL main head (lanes merge into
#     local main without network; DNS to the remote is intermittently
#     down, fetch failures are logged and tolerated — leg-31 lesson:
#     kill watchers by exact cmdline, never pkill -f on the bare name;
#     the first restart attempt died to a pkill self-match).
#     kernel_new = kernel/ commits on local main since the leg-31
#     merge 59a4a3b8 (main 6f7e4ac3, dfx-align included; pa38 census
#     numbers stay the Tier-1 band reference). kernel_new>0 =
#     census-hold gate (rerun the quiet syscost boot before merging).
#   - THE TRIGGER: milestone markers (launcher.ready SET evidence
#     "key: bootevent.launcher.ready" / OnForeground / ArkUI / PBENCH)
#     in ANY oh-sc2xx serial modified since watcher start (pa37 grepped
#     only the newest file; a newer empty log could mask a live hit).
#   - samgr-callback (launcher lane) head — kernel-class hint for any
#     milestone hit (their boots are main+delta).
# Read-only: git fetch + greps. No QEMU, no writes outside this log.
cd /home/william/rux-agents/perf-audit || exit 1
LOG=run/pa40-watch.log
STAMP=run/pa40-watch.started
touch "$STAMP"
while true; do
  ts=$(date '+%F %T')
  git fetch -q origin 2>>"$LOG" || echo "$ts fetch-failed (dns/auth) — watching local main" >> "$LOG"
  om=$(git rev-parse --short origin/main 2>/dev/null)
  lm=$(git rev-parse --short main 2>/dev/null)
  kc=$(git log --oneline 6f7e4ac3..main -- kernel/ 2>/dev/null | wc -l)
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
      main HEAD 2>/dev/null && om_in_kn=yes
  echo "$ts omain=$om lmain=$lm kernel_new=$kc milestones=$ms sc_newest=$newest samgr_head=$kn samgr_has_main=$om_in_kn" >> "$LOG"
  sleep 120
done
