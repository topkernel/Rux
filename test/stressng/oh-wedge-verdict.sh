#!/bin/bash
# oh-wedge-verdict.sh — adjudicate one OH wedge-rate A/B roll serial log
# (test/stressng/oh-wedge-ab-x86.sh output) into the OH lane's outcome
# classes. Classes per the 27th close wedge table:
#   WEDGE      samgr/distributeddata-centered total-freeze: DEADLOCK
#              cluster or SLOW-TXN execs growing past 100s with the
#              service frozen (fresh parks, no ladder progress).
#   SPIRAL     early kill waves / ExecReboot before the ladder settles.
#   KVS-HANG   healthy cascade but distributeddata OnStart never
#              completes (no creation leg).
#   HEALTHY    creation/startUser leg reached, serial alive.
# Usage: oh-wedge-verdict.sh <serial.log> [more logs...]
set -uo pipefail
for LOG in "$@"; do
  TAG=$(basename "$LOG" -serial.log)
  DEAD=$(grep -ac 'DEADLOCK:' "$LOG" 2>/dev/null
  )
  MAXEX=$(grep -a -oE 'exec [0-9]+ ms' "$LOG" 2>/dev/null | grep -oE '[0-9]+' | sort -n | tail -1)
  MAXEX=${MAXEX:-0}
  BA_READY=$(grep -a -m1 -oE '^\[ *[0-9.]+\] ===MARKER bootevent.bootanimation.ready===' "$LOG" | grep -oE '[0-9]+\.[0-9]+')
  CREATION=$(grep -ac 'CreateNewUser' "$LOG" 2>/dev/null
  )
  STARTUSER=$(grep -ac 'startUser in service' "$LOG" 2>/dev/null
  )
  QUOTA=$(grep -ac 'SetBundleQuota' "$LOG" 2>/dev/null
  )
  REBOOT=$(grep -ac 'ExecReboot' "$LOG" 2>/dev/null
  )
  KILLW=$(grep -ac 'KOT-KILL\|SIGKILL' "$LOG" 2>/dev/null
  )
  BUNDLES=$(grep -ac 'bundles.ready' "$LOG" 2>/dev/null
  )
  LAST=$(tail -1 "$LOG" 2>/dev/null | grep -oE '^\[ *[0-9.]+' | grep -oE '[0-9.]+' )
  DEAD=${DEAD:-0}; CREATION=${CREATION:-0}; STARTUSER=${STARTUSER:-0}
  QUOTA=${QUOTA:-0}; REBOOT=${REBOOT:-0}; KILLW=${KILLW:-0}; BUNDLES=${BUNDLES:-0}
  LAST=${LAST:-0}
  # classification (first match wins; HEALTHY requires the creation leg)
  CLASS=HEALTHY
  if [ "$REBOOT" -gt 0 ] && [ "$CREATION" -eq 0 ]; then CLASS=SPIRAL
  elif [ "$DEAD" -ge 20 ] || [ "$MAXEX" -ge 100000 ]; then CLASS=WEDGE
  elif [ "$CREATION" -eq 0 ] && [ "$BUNDLES" -ge 1 ]; then CLASS=KVS-HANG
  elif [ "$CREATION" -eq 0 ] && [ "$LAST" -lt 300 ]; then CLASS=SPIRAL
  fi
  printf '%-10s %-9s dead=%-4s maxexec=%-7sms ba_ready=%-7s quota=%-3s bundles=%-3s creat=%-3s suser=%-3s rebt=%-3s last=%ss\n' \
    "$TAG" "$CLASS" "$DEAD" "$MAXEX" "${BA_READY:--}" "$QUOTA" "$BUNDLES" "$CREATION" "$STARTUSER" "$REBOOT" "$LAST"
done
